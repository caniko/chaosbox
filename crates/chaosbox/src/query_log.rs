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
/// Distinct source files recorded per response.
const SERVED_FILES_MAX: usize = 50;
/// The read-after-served reader scans only this tail of the ledger.
const SERVED_TAIL_BYTES: u64 = 1024 * 1024;
/// A read counts as redundant when the same file was served this recently.
const READ_AFTER_MS: u64 = 30 * 60 * 1000;

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
/// build, a result count, the payload size and the source it served; cursors
/// are redacted.
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
    // The shared response contract puts served source at top level. Recording
    // it makes "did the agent read again what it already had?" measurable.
    let source = response
        .and_then(|r| r.get("source"))
        .and_then(Value::as_array);
    let served_files = source.map(|items| {
        let mut files: Vec<String> = Vec::new();
        for file in items.iter().filter_map(|item| item.get("file")?.as_str()) {
            if !files.iter().any(|seen| seen == file) {
                files.push(file.to_owned());
            }
            if files.len() == SERVED_FILES_MAX {
                break;
            }
        }
        files
    });
    let served_chars = source.map(|items| {
        items
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .map(|text| text.chars().count())
            .sum::<usize>()
    });
    let root =
        crate::enablement::current().map(|enabled| enabled.root.to_string_lossy().into_owned());
    json!({"ts_ms":now_ms(),"surface":surface,"tool":tool,"repo":args["repo"],
        "build_id":response.map_or(&Value::Null, |r| &r["build_id"]),
        "duration_ms":duration_ms,"ok":code == 0,"code":code,
        "bytes":response.map(|r| r.to_string().len()),"count":count,
        "served_files":served_files,"served_chars":served_chars,"root":root,"args":args})
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
    let Some(path) = configured_path() else {
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

/// Record a harness measurement event (never agent-visible).
pub fn record_hook(tool: &str, file: &str) {
    let Some(path) = configured_path() else {
        return;
    };
    let entry = json!({"ts_ms":now_ms(),"surface":"hook","tool":tool,"file":file});
    let _ = append(&path, &entry);
}

fn configured_path() -> Option<PathBuf> {
    log_path(
        std::env::var("CHAOSBOX_QUERY_LOG").ok(),
        std::env::var("XDG_STATE_HOME").ok(),
        std::env::var("HOME").ok(),
    )
}

/// Whether `file` was served as source for `root` within the read window.
#[must_use]
pub fn served_recent(root: &str, file: &str) -> bool {
    served_recent_in(
        configured_path().as_deref(),
        root,
        file,
        now_ms(),
        READ_AFTER_MS,
    )
}

fn served_recent_in(
    path: Option<&Path>,
    root: &str,
    file: &str,
    now_ms: u64,
    window_ms: u64,
) -> bool {
    let Some(text) = path.and_then(|path| tail(path, SERVED_TAIL_BYTES).ok()) else {
        return false;
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|entry| {
            entry["root"].as_str() == Some(root)
                && entry["ts_ms"]
                    .as_u64()
                    .is_some_and(|ts| now_ms.saturating_sub(ts) <= window_ms)
                && (entry["served_files"]
                    .as_array()
                    .is_some_and(|files| files.iter().any(|served| served.as_str() == Some(file)))
                    || entry["source"].as_array().is_some_and(|items| {
                        items.iter().any(|item| item["file"].as_str() == Some(file))
                    }))
        })
}

/// The last `bytes` of a file, with a leading partial line dropped.
fn tail(path: &Path, bytes: u64) -> std::io::Result<String> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(bytes);
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))?;
    }
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let mut text = String::from_utf8_lossy(&buffer).into_owned();
    if start > 0 {
        if let Some(end) = text.find('\n') {
            text.drain(..=end);
        }
    }
    Ok(text)
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
    use super::{append, line, log_path, served_recent_in, SERVED_FILES_MAX};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn ledger_lines_record_served_source_deduped_and_bounded() {
        let source: Vec<serde_json::Value> = (0..SERVED_FILES_MAX + 5)
            .map(|i| json!({"file": format!("src/f{}.rs", i % 3), "text": "abcd"}))
            .collect();
        let entry = line(
            "mcp",
            "context",
            &json!({"repo":"r"}),
            3,
            0,
            Some(&json!({"source": source})),
        );
        assert_eq!(
            entry["served_files"],
            json!(["src/f0.rs", "src/f1.rs", "src/f2.rs"]),
            "relative paths, deduped"
        );
        assert_eq!(entry["served_chars"], json!(4 * (SERVED_FILES_MAX + 5)));
        let plain = line("mcp", "status", &json!({"repo":"r"}), 1, 0, None);
        assert_eq!(
            (plain["served_files"].clone(), plain["served_chars"].clone()),
            (json!(null), json!(null))
        );
    }

    #[test]
    fn read_after_served_reads_only_the_recent_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queries.jsonl");
        let old = json!({"ts_ms": 1_000u64, "root": "/repo", "served_files": ["src/old.rs"]});
        // Push the old line out of the 1 MiB tail window.
        let mut noise = String::new();
        for _ in 0..(super::SERVED_TAIL_BYTES / 1024 + 64) {
            noise.push_str(&"x".repeat(1023));
            noise.push('\n');
        }
        std::fs::write(&path, format!("{old}\n{noise}")).unwrap();
        let now = 10_000u64;
        assert!(
            !served_recent_in(Some(&path), "/repo", "src/old.rs", now, 60 * 60 * 1000),
            "an entry outside the tail is not found"
        );

        append(
            &path,
            &json!({"ts_ms": 9_000u64, "root": "/repo", "served_files": ["src/new.rs"]}),
        )
        .unwrap();
        assert!(
            served_recent_in(Some(&path), "/repo", "src/new.rs", now, 60 * 60 * 1000),
            "a recent served file is found"
        );
        assert!(!served_recent_in(
            Some(&path),
            "/other",
            "src/new.rs",
            now,
            60 * 60 * 1000
        ));
        assert!(!served_recent_in(
            Some(&path),
            "/repo",
            "src/new.rs",
            now,
            500
        ));
        assert!(!served_recent_in(
            Some(&dir.path().join("absent")),
            "/repo",
            "src/new.rs",
            now,
            60_000
        ));
    }

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
