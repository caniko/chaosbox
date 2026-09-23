
//! Wire-level tests for [`super::JevClient::evaluate`] against a scripted
//! mock `POST /v1/systemone` server on 127.0.0.1 (raw `tokio` TCP, no new
//! dependencies, no credentials, no network beyond loopback).
use super::*;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Process-global credential env is shared by threads: serialize the
/// wire tests so each sees its own test key.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// One scripted HTTP response.
struct Script {
    status: u16,
    retry_after: Option<u64>,
    body: String,
}

fn find_crlf2(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn content_len(head: &[u8]) -> usize {
    let s = String::from_utf8_lossy(head).to_lowercase();
    s.lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Error",
    }
}

/// Serve the scripts in order; records raw request heads; returns the
/// endpoint URL and the join handle resolving to requests served.
async fn serve(
    scripts: Vec<Script>,
    heads: Arc<Mutex<Vec<String>>>,
) -> (String, tokio::task::JoinHandle<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let h = tokio::spawn(async move {
        let mut served = 0usize;
        for s in scripts {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            while let Ok(n) = sock.read(&mut tmp).await {
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(h) = find_crlf2(&buf) {
                    if buf.len() >= h + content_len(&buf[..h]) {
                        heads
                            .lock()
                            .unwrap()
                            .push(String::from_utf8_lossy(&buf[..h]).into_owned());
                        break;
                    }
                }
                if buf.len() > 4_000_000 {
                    break;
                }
            }
            let mut resp = format!(
                    "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                    s.status,
                    reason(s.status),
                    s.body.len()
                );
            if let Some(ra) = s.retry_after {
                let _ = write!(resp, "retry-after: {ra}\r\n");
            }
            resp.push_str("\r\n");
            resp.push_str(&s.body);
            if sock.write_all(resp.as_bytes()).await.is_err() {
                break;
            }
            served += 1;
        }
        served
    });
    (format!("http://{addr}/v1/systemone"), h)
}

fn test_policy(endpoint: String) -> JevPolicy {
    JevPolicy {
        endpoint,
        deadline: Duration::from_secs(10),
        max_questions_per_request: 4,
        max_retries: 3,
        ..JevPolicy::default()
    }
}

fn choice_questions() -> BTreeMap<String, Question> {
    BTreeMap::from([(
        "q1".to_owned(),
        Question::Choice {
            instructions: "pick one".to_owned(),
            criteria: BTreeMap::from([
                ("accept".to_owned(), None),
                ("reject".to_owned(), None),
                ("none".to_owned(), None),
            ]),
        },
    )])
}

fn valid_options() -> BTreeMap<String, BTreeSet<String>> {
    BTreeMap::from([(
        "q1".to_owned(),
        BTreeSet::from(["accept".to_owned(), "reject".to_owned(), "none".to_owned()]),
    )])
}

fn accept_body() -> String {
    serde_json::json!({
        "model": JEV_MODEL_PINNED,
        "answers": {"q1": {
            "type": "choice", "choice": "accept",
            "probabilities": {"accept": 0.9, "reject": 0.05, "none": 0.05},
            "confidence": 0.85,
        }},
        "usage": {"input_tokens": 10, "output_tokens": 0},
    })
    .to_string()
}

fn use_test_key(name: &str) {
    std::env::remove_var("CHAOSBOX_JEV_API_KEY_FILE");
    std::env::set_var("TYPESAFE_API_KEY", format!("test-key-{name}"));
}

#[tokio::test]
async fn retry_after_honored_then_success_over_http() {
    let _guard = env_lock().lock().unwrap();
    use_test_key("retry");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (url, server) = serve(
        vec![
            Script {
                status: 429,
                retry_after: Some(0),
                body: "{}".to_owned(),
            },
            Script {
                status: 200,
                retry_after: None,
                body: accept_body(),
            },
        ],
        heads.clone(),
    )
    .await;
    let mut client = JevClient::new(test_policy(url)).unwrap();
    let resp = client
        .evaluate(
            serde_json::json!({"repo": "demo"}),
            choice_questions(),
            &valid_options(),
        )
        .await
        .unwrap();
    assert_eq!(resp.model, JEV_MODEL_PINNED);
    assert_eq!(client.attempts.len(), 2);
    assert_eq!(client.attempts[0].http_status, Some(429));
    assert_eq!(client.attempts[0].retry_after_secs, Some(0));
    assert_eq!(client.attempts[1].http_status, Some(200));
    // Bearer auth on the wire, never a query param or log line.
    let heads = heads.lock().unwrap();
    assert_eq!(heads.len(), 2);
    assert!(heads[0].contains("authorization: Bearer test-key-retry"));
    assert!(!heads[0].contains("test-key-retry\""));
    assert_eq!(server.await.unwrap(), 2);
}

#[tokio::test]
async fn auth_failure_never_retried() {
    let _guard = env_lock().lock().unwrap();
    use_test_key("auth");
    let heads = Arc::new(Mutex::new(Vec::new()));
    let (url, server) = serve(
        vec![Script {
            status: 401,
            retry_after: None,
            body: "{}".to_owned(),
        }],
        heads,
    )
    .await;
    let mut client = JevClient::new(test_policy(url)).unwrap();
    let err = client
        .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
        .await
        .unwrap_err();
    assert!(matches!(err, JevError::Auth(_)), "got {err:?}");
    assert_eq!(server.await.unwrap(), 1, "auth errors must not be retried");
}

#[tokio::test]
async fn out_of_scope_choice_rejected_over_http() {
    let _guard = env_lock().lock().unwrap();
    use_test_key("scope");
    let body = serde_json::json!({
        "model": JEV_MODEL_PINNED,
        "answers": {"q1": {
            "type": "choice", "choice": "invented",
            "probabilities": {"invented": 1.0},
            "confidence": 0.9,
        }},
        "usage": {"input_tokens": 5, "output_tokens": 0},
    })
    .to_string();
    let (url, server) = serve(
        vec![Script {
            status: 200,
            retry_after: None,
            body,
        }],
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let mut client = JevClient::new(test_policy(url)).unwrap();
    let err = client
        .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
        .await
        .unwrap_err();
    assert!(matches!(err, JevError::Schema(_)), "got {err:?}");
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn missing_answer_is_protocol_error() {
    let _guard = env_lock().lock().unwrap();
    use_test_key("missing");
    let body = serde_json::json!({
        "model": JEV_MODEL_PINNED,
        "answers": {},
        "usage": {"input_tokens": 5, "output_tokens": 0},
    })
    .to_string();
    let (url, server) = serve(
        vec![Script {
            status: 200,
            retry_after: None,
            body,
        }],
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let mut client = JevClient::new(test_policy(url)).unwrap();
    let err = client
        .evaluate(serde_json::json!({}), choice_questions(), &valid_options())
        .await
        .unwrap_err();
    assert!(matches!(err, JevError::Protocol(_)), "got {err:?}");
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn malformed_probability_rejected_over_http() {
    let _guard = env_lock().lock().unwrap();
    use_test_key("badprob");
    let body = serde_json::json!({
        "model": JEV_MODEL_PINNED,
        "answers": {"q1": {"type": "noul", "noul": 7.5}},
        "usage": {"input_tokens": 5, "output_tokens": 0},
    })
    .to_string();
    let questions = BTreeMap::from([(
        "q1".to_owned(),
        Question::Noul {
            instructions: "y?".to_owned(),
            criteria: None,
        },
    )]);
    let (url, server) = serve(
        vec![Script {
            status: 200,
            retry_after: None,
            body,
        }],
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let mut client = JevClient::new(test_policy(url)).unwrap();
    let err = client
        .evaluate(serde_json::json!({}), questions, &BTreeMap::new())
        .await
        .unwrap_err();
    assert!(matches!(err, JevError::Schema(_)), "got {err:?}");
    assert_eq!(server.await.unwrap(), 1);
}
