//! Text embeddings from an OpenAI-compatible `/v1/embeddings` endpoint (the
//! local llama.cpp/llama-swap server). Vectors are derived data: every caller
//! degrades to lexical retrieval when the endpoint is unset, slow or wrong,
//! and publication never waits on it.

use std::time::{Duration, Instant};

use chaosbox_pg::{store::PgStore, EMBED_DIMS};

/// Qwen3-Embedding is instruction-tuned on the query side only.
const QUERY_INSTRUCTION: &str = "Instruct: Given a question about a code repository, retrieve the code definitions, documentation sections and recorded engineering decisions that answer it\nQuery: ";

/// Inputs per request: bounded by the server's batch, small enough to keep
/// one request well inside its deadline.
const BATCH: i64 = 32;

/// Wall-clock budget one command spends embedding its backlog.
pub const BACKFILL_BUDGET: Duration = Duration::from_secs(120);

/// Client for one embedding model.
#[derive(Clone, Debug)]
pub struct Embedder {
    url: reqwest::Url,
    model: String,
    client: reqwest::Client,
    query_timeout: Duration,
}

impl Embedder {
    /// From `CHAOSBOX_EMBED_URL` (the full `/v1/embeddings` URL),
    /// `CHAOSBOX_EMBED_MODEL` and optional `CHAOSBOX_EMBED_TIMEOUT_MS` (query
    /// deadline, default 2000). `None` when no URL is configured.
    pub fn from_env() -> Result<Option<Self>, String> {
        let Ok(url) = std::env::var("CHAOSBOX_EMBED_URL") else {
            return Ok(None);
        };
        let model = std::env::var("CHAOSBOX_EMBED_MODEL")
            .map_err(|_| "CHAOSBOX_EMBED_MODEL unset while CHAOSBOX_EMBED_URL is set")?;
        let timeout = std::env::var("CHAOSBOX_EMBED_TIMEOUT_MS")
            .ok()
            .map(|ms| {
                ms.parse::<u64>()
                    .map_err(|e| format!("CHAOSBOX_EMBED_TIMEOUT_MS: {e}"))
            })
            .transpose()?
            .unwrap_or(2000);
        Self::new(&url, &model, Duration::from_millis(timeout)).map(Some)
    }

    /// Private session text is sent to this endpoint: only loopback HTTP(S)
    /// is accepted.
    pub fn new(url: &str, model: &str, query_timeout: Duration) -> Result<Self, String> {
        let url = reqwest::Url::parse(url).map_err(|e| format!("CHAOSBOX_EMBED_URL: {e}"))?;
        let loopback = url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        });
        if !matches!(url.scheme(), "http" | "https") || !loopback || model.trim().is_empty() {
            return Err("CHAOSBOX_EMBED_URL must be a loopback http(s) URL with a model".into());
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            url,
            model: model.to_owned(),
            client,
            query_timeout,
        })
    }

    /// Stored model key: model plus stored width, so a width change re-embeds.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}@{EMBED_DIMS}", self.model)
    }

    /// Unit vectors for document texts, in input order.
    pub async fn documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        self.request(texts, None).await
    }

    /// Unit vector for one search query, within the query deadline.
    pub async fn query(&self, text: &str) -> Result<Vec<f32>, String> {
        let input = [format!("{QUERY_INSTRUCTION}{text}")];
        let mut out = self.request(&input, Some(self.query_timeout)).await?;
        Ok(out.pop().expect("one input, one vector"))
    }

    async fn request(
        &self,
        input: &[String],
        timeout: Option<Duration>,
    ) -> Result<Vec<Vec<f32>>, String> {
        #[derive(serde::Deserialize)]
        struct Item {
            index: usize,
            embedding: Vec<f32>,
        }
        #[derive(serde::Deserialize)]
        struct Response {
            data: Vec<Item>,
        }
        let mut request = self
            .client
            .post(self.url.clone())
            .json(&serde_json::json!({"model": self.model, "input": input}));
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| format!("embedding request: {e}"))?;
        let mut data = response
            .json::<Response>()
            .await
            .map_err(|e| format!("embedding response: {e}"))?
            .data;
        data.sort_by_key(|item| item.index);
        if data.len() != input.len() || data.iter().enumerate().any(|(i, item)| item.index != i) {
            return Err("embedding response does not match its inputs".into());
        }
        data.into_iter().map(|item| unit(item.embedding)).collect()
    }
}

/// Truncate to the stored width (Matryoshka-trained models keep meaning in
/// the leading components) and L2-normalize.
fn unit(mut vector: Vec<f32>) -> Result<Vec<f32>, String> {
    if vector.len() < EMBED_DIMS {
        return Err(format!(
            "embedding width {} below stored width {EMBED_DIMS}",
            vector.len()
        ));
    }
    vector.truncate(EMBED_DIMS);
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err("degenerate embedding".into());
    }
    for x in &mut vector {
        *x /= norm;
    }
    Ok(vector)
}

/// Embed stored texts that have no vector for this model, oldest key first,
/// until none remain or `budget` elapses. Returns how many were stored.
// ponytail: one sequential batch at a time under a wall-clock budget; the
// next run continues the backlog. Parallel requests only if a first full
// workspace pass is measured too slow.
pub async fn backfill(
    store: &mut PgStore,
    embedder: &Embedder,
    budget: Duration,
) -> Result<usize, String> {
    let deadline = Instant::now() + budget;
    let key = embedder.key();
    let mut stored = 0;
    while Instant::now() < deadline {
        let pending = store
            .pending_texts(&key, BATCH)
            .await
            .map_err(|e| e.to_string())?;
        if pending.is_empty() {
            break;
        }
        let bodies: Vec<String> = pending.iter().map(|(_, body)| body.clone()).collect();
        let vectors = embedder.documents(&bodies).await?;
        let rows: Vec<(String, Vec<f32>)> = pending
            .into_iter()
            .map(|(sha, _)| sha)
            .zip(vectors)
            .collect();
        store
            .put_embeddings(&key, &rows)
            .await
            .map_err(|e| e.to_string())?;
        stored += rows.len();
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One-shot loopback server answering every request with `body`.
    async fn serve(body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = vec![0; 64 * 1024];
                let _ = socket.read(&mut buf).await;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        format!("http://{addr}/v1/embeddings")
    }

    #[tokio::test]
    async fn vectors_are_truncated_normalized_and_matched_to_inputs() {
        let mut long = vec![0.0_f32; EMBED_DIMS + 7];
        long[0] = 3.0;
        long[1] = 4.0;
        long[EMBED_DIMS] = 100.0;
        let body = serde_json::json!({"data":[
            {"index":1,"embedding":long},
            {"index":0,"embedding":vec![1.0_f32; EMBED_DIMS]},
        ]})
        .to_string();
        let embedder = Embedder::new(&serve(body).await, "m", Duration::from_secs(5)).unwrap();
        let out = embedder.documents(&["a".into(), "b".into()]).await.unwrap();
        assert_eq!(out[1].len(), EMBED_DIMS);
        assert!((out[1][0] - 0.6).abs() < 1e-6 && (out[1][1] - 0.8).abs() < 1e-6);
        assert!((out[0].iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-4);
        assert_eq!(embedder.key(), format!("m@{EMBED_DIMS}"));
        // A response that drops an input is rejected, never misaligned.
        let short = serde_json::json!({"data":[{"index":0,"embedding":vec![1.0_f32; EMBED_DIMS]}]});
        let embedder =
            Embedder::new(&serve(short.to_string()).await, "m", Duration::from_secs(5)).unwrap();
        assert!(embedder.documents(&["a".into(), "b".into()]).await.is_err());
        let narrow = serde_json::json!({"data":[{"index":0,"embedding":[1.0]}]});
        let embedder = Embedder::new(
            &serve(narrow.to_string()).await,
            "m",
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(embedder.query("q").await.is_err());
    }

    #[test]
    fn only_loopback_endpoints_are_accepted() {
        for ok in [
            "http://127.0.0.1:8080/v1/embeddings",
            "http://localhost/v1/embeddings",
            "http://[::1]:9/v1/embeddings",
        ] {
            assert!(
                Embedder::new(ok, "m", Duration::from_secs(1)).is_ok(),
                "{ok}"
            );
        }
        for bad in [
            "http://10.0.0.2/v1/embeddings",
            "https://api.example.com/v1/embeddings",
            "file:///tmp/x",
            "not a url",
        ] {
            assert!(
                Embedder::new(bad, "m", Duration::from_secs(1)).is_err(),
                "{bad}"
            );
        }
        assert!(Embedder::new(
            "http://127.0.0.1/v1/embeddings",
            " ",
            Duration::from_secs(1)
        )
        .is_err());
    }
}
