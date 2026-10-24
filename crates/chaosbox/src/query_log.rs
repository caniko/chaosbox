//! Local JSONL usage ledger (Graphify's `graphify-queries.log`, plus outcome,
//! size and latency): one line per CLI `query` or MCP `tools/call`, so "do
//! agents use this, and does it answer?" is a query instead of a forensic
//! exercise. Opt out with `CHAOSBOX_QUERY_LOG=off`. Never fails the caller.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Rotate to `<log>.1` beyond this size.
const ROTATE_BYTES: u64 = 64 * 1024 * 1024;
/// Arguments longer than this are summarized, not stored.
const ARGS_BYTES_MAX: usize = 1024;

/// `$CHAOSBOX_QUERY_LOG` (`off` disables), else
/// `$XDG_STATE_HOME/chaosbox/queries.jsonl`, else `~/.local/state/...`.
#[must_use]
pub fn log_path(
    setting: Option<String>,
    state_home: Option<String>,
    home: Option<String>,
) -> Option<PathBuf> {
    match setting.filter(|s| !s.is_empty()) {
        Some(off) if off == "off" => None,
        Some(path) => Some(PathBuf::from(path)),
        None => state_home
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.map(|h| Path::new(&h).join(".local/state")))
            .map(|state| state.join("chaosbox/queries.jsonl")),
    }
}

/// One ledger line. `response` (when the caller has it) supplies the pinned
/// build, a result count and the payload size; cursors are redacted.
#[must_use]
pub fn line(
    surface: &str,
    tool: &str,
    args: &Value,
    duration_ms: u128,
    code: i64,
    response: Option<&Value>,
) -> Value {
    let mut args = args.clone();
    if let Some(cursor) = args.get_mut("cursor") {
        *cursor = json!("<redacted>");
    }
    let encoded = args.to_string();
    let args = if encoded.len() > ARGS_BYTES_MAX {
        json!({"omitted_bytes": encoded.len()})
    } else {
        args
    };
    let count = response.and_then(|r| {
        r.as_array()
            .map(Vec::len)
            .or_else(|| r["nodes"].as_array().map(Vec::len))
            .or_else(|| r["evidence"].as_array().map(Vec::len))
            .or_else(|| {
                let (out, inc) = (r["outgoing"].as_array()?, r["incoming"].as_array()?);
                Some(out.len() + inc.len())
            })
    });
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    json!({"ts_ms":ts_ms,"surface":surface,"tool":tool,"repo":args["repo"],
        "build_id":response.map_or(&Value::Null, |r| &r["build_id"]),
        "duration_ms":duration_ms,"ok":code == 0,"code":code,
        "bytes":response.map(|r| r.to_string().len()),"count":count,"args":args})
}

/// Append one line to the configured ledger, ignoring every failure.
pub fn record(
    surface: &str,
    tool: &str,
    args: &Value,
    started: Instant,
    code: i64,
    response: Option<&Value>,
) {
    let Some(path) = log_path(
        std::env::var("CHAOSBOX_QUERY_LOG").ok(),
        std::env::var("XDG_STATE_HOME").ok(),
        std::env::var("HOME").ok(),
    ) else {
        return;
    };
    let entry = line(
        surface,
        tool,
        args,
        started.elapsed().as_millis(),
        code,
        response,
    );
    let _ = append(&path, &entry);
}

fn append(path: &Path, entry: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        crate::sync::settings::private_directory(parent).map_err(std::io::Error::other)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        std::fs::rename(path, rotated)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // One write per line keeps concurrent appenders from interleaving.
    file.write_all(format!("{entry}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{append, line, log_path};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn ledger_lines_redact_cursors_bound_args_and_honor_opt_out() {
        assert_eq!(log_path(Some("off".into()), None, Some("/h".into())), None);
        assert_eq!(
            log_path(None, None, Some("/h".into())),
            Some(PathBuf::from("/h/.local/state/chaosbox/queries.jsonl"))
        );
        assert_eq!(
            log_path(None, Some("/s".into()), Some("/h".into())),
            Some(PathBuf::from("/s/chaosbox/queries.jsonl"))
        );
        let response = json!({"build_id":"build:1","outgoing":[1,2],"incoming":[3]});
        let entry = line(
            "mcp",
            "neighbors",
            &json!({"repo":"r","id":"ent:a","cursor":"secret"}),
            7,
            0,
            Some(&response),
        );
        assert_eq!(entry["args"]["cursor"], "<redacted>");
        assert_eq!(
            (entry["count"].clone(), entry["build_id"].clone()),
            (json!(3), json!("build:1"))
        );
        assert_eq!(
            (entry["repo"].clone(), entry["ok"].clone()),
            (json!("r"), json!(true))
        );
        let long = line(
            "cli",
            "context",
            &json!({"query":"x".repeat(2000)}),
            1,
            4,
            None,
        );
        assert!(long["args"]["omitted_bytes"].as_u64().unwrap() > 2000);
        assert_eq!(long["ok"], false);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/chaosbox/queries.jsonl");
        append(&path, &entry).unwrap();
        append(&path, &long).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
