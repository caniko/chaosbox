//! Internal `chaosbox hint` hook. A harness `PostToolUse` payload arrives on
//! stdin; when the project is enabled and the tool searched for an
//! identifier, the hint names the graph definitions the agent probably
//! wanted instead of continuing to grep. A `Read` payload only measures
//! whether the file was already served as source by an MCP call.
//!
//! Fail open: any error prints nothing and exits 0, and the whole hook is
//! bounded so it can never block the tool that triggered it.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use super::AnyReader;

/// Hints emitted per session: enough to steer, few enough to stay cheap.
const HINTS_MAX: u32 = 5;
/// Definitions named in one hint.
const HITS_MAX: i64 = 5;
/// Whole-hook budget, including the database round trip.
const BUDGET: Duration = Duration::from_millis(300);
/// Shorter searches match too much to be worth a query.
const IDENTIFIER_MIN: usize = 3;
/// Shell search programs a hint looks inside.
const SEARCH_PROGRAMS: [&str; 5] = ["rg", "grep", "ag", "ack", "ack-grep"];

#[derive(Debug, Deserialize)]
struct Hook {
    session_id: Option<String>,
    cwd: Option<PathBuf>,
    tool_name: Option<String>,
    tool_input: Option<Value>,
}

/// Read one hook payload and print at most one `additionalContext` result.
pub(super) async fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    if let Ok(Some(text)) = tokio::time::timeout(BUDGET, handle(&input)).await {
        let output = json!({"hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": text,
        }});
        println!("{output}");
    }
    0
}

async fn handle(input: &str) -> Option<String> {
    let hook: Hook = serde_json::from_str(input.trim()).ok()?;
    let enabled =
        chaosbox::enablement::enabled(hook.cwd.as_deref().unwrap_or_else(|| Path::new(".")))?;
    let tool = hook.tool_name.as_deref()?.to_ascii_lowercase();
    if tool == "read" {
        measure_read(&enabled, &hook);
        return None;
    }
    let pattern = searched_identifier(&tool, hook.tool_input.as_ref())?;
    let mut state = State::load(hook.session_id.as_deref())?;
    if state.hints >= HINTS_MAX || state.patterns.iter().any(|seen| seen == &pattern) {
        return None;
    }
    let hits = tokio::time::timeout(BUDGET, hits(&enabled.repo, &pattern))
        .await
        .ok()??;
    // Remember the pattern even without hits: repeating it cannot help.
    state.patterns.push(pattern);
    if hits.is_empty() {
        state.save();
        return None;
    }
    state.hints += 1;
    state.save();
    Some(render(&hits))
}

struct Hit {
    name: String,
    kind: String,
    file: String,
    line: u32,
    callers: usize,
}

async fn hits(repo: &str, pattern: &str) -> Option<Vec<Hit>> {
    let reader = AnyReader::connect(repo).await.ok()?;
    let rows = reader.search(pattern, HITS_MAX).await.ok()?;
    let mut hits = Vec::with_capacity(rows.len());
    for row in rows {
        let callers = reader
            .neighbors(&row.entity_id, Some(vec!["calls".to_owned()]))
            .await
            .map_or(0, |(_out, incoming)| incoming.len());
        hits.push(Hit {
            line: row.span.as_ref().map_or(0, |span| span.start_line),
            file: row
                .span
                .as_ref()
                .map_or_else(|| row.file.clone(), |span| span.file.clone()),
            name: row.name,
            kind: row.kind,
            callers,
        });
    }
    Some(hits)
}

fn render(hits: &[Hit]) -> String {
    let mut text = String::from("Chaosbox already has these definitions:\n");
    for hit in hits {
        let location = if hit.line == 0 {
            hit.file.clone()
        } else {
            format!("{}:{}", hit.file, hit.line)
        };
        let _ = writeln!(
            text,
            "{} {} {} ({} callers)",
            hit.name, hit.kind, location, hit.callers
        );
    }
    text.push_str("Chaosbox can return these definitions with source: use context/lookup.");
    text
}

/// The search term of a Grep/Glob input or a simple rg/grep/ag/ack command,
/// when it looks like an identifier.
fn searched_identifier(tool: &str, input: Option<&Value>) -> Option<String> {
    let input = input?;
    let raw = match tool {
        "grep" | "glob" => input.get("pattern")?.as_str()?.to_owned(),
        "bash" | "shell" => search_command(input.get("command")?.as_str()?)?,
        _ => return None,
    };
    identifier(&raw).then_some(raw)
}

/// First argument of a simple search command. No shell evaluation: the first
/// word must be the program, and the first non-flag word is the pattern.
/// ponytail: flag values (`rg -t rust foo`) can be mistaken for the pattern;
/// the lookup is advisory, so an occasional miss costs nothing.
fn search_command(command: &str) -> Option<String> {
    let mut words = command.split_whitespace();
    if !SEARCH_PROGRAMS.contains(&words.next()?.rsplit('/').next()?) {
        return None;
    }
    let raw = words.find(|word| !word.starts_with('-'))?;
    Some(raw.trim_matches(|c| c == '\'' || c == '"').to_owned())
}

fn identifier(value: &str) -> bool {
    value.chars().count() >= IDENTIFIER_MIN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_:.".contains(c))
}

/// Record a Read of a file Chaosbox served recently (measurement only).
fn measure_read(enabled: &chaosbox::enablement::Enabled, hook: &Hook) {
    let Some(path) = hook
        .tool_input
        .as_ref()
        .and_then(|input| input.get("file_path").or_else(|| input.get("path")))
        .and_then(Value::as_str)
    else {
        return;
    };
    let Ok(relative) = Path::new(path).strip_prefix(&enabled.root) else {
        return;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let root = enabled.root.to_string_lossy();
    if chaosbox::query_log::served_recent(&root, &relative) {
        chaosbox::query_log::record_hook("read_after_served", &relative);
    }
}

/// Per-session hint budget and the patterns it already answered.
struct State {
    path: PathBuf,
    hints: u32,
    patterns: Vec<String>,
}

impl State {
    fn load(session: Option<&str>) -> Option<Self> {
        Some(Self::load_at(state_path(session?)?))
    }

    fn load_at(path: PathBuf) -> Self {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut lines = text.lines();
        Self {
            hints: lines
                .next()
                .and_then(|count| count.parse().ok())
                .unwrap_or(0),
            patterns: lines.map(str::to_owned).collect(),
            path,
        }
    }

    fn save(&self) {
        let mut text = format!("{}\n", self.hints);
        for pattern in &self.patterns {
            text.push_str(pattern);
            text.push('\n');
        }
        let _ = write_private(&self.path, text.as_bytes());
    }
}

fn state_home() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
}

fn state_path(session: &str) -> Option<PathBuf> {
    state_path_in(state_home().as_deref(), session)
}

fn state_path_in(state: Option<&Path>, session: &str) -> Option<PathBuf> {
    let state = state?;
    let safe: String = session
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!safe.is_empty()).then(|| state.join("chaosbox/hints").join(safe))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        identifier, render, search_command, searched_identifier, state_path_in, Hit, State,
        HINTS_MAX,
    };
    use serde_json::json;

    #[test]
    fn bash_search_patterns_are_parsed_without_a_shell() {
        assert_eq!(
            search_command("rg parse_config src").as_deref(),
            Some("parse_config")
        );
        assert_eq!(
            search_command("grep -rn 'build_graph' .").as_deref(),
            Some("build_graph")
        );
        assert_eq!(
            search_command("/usr/bin/grep --color=never ent:42 .").as_deref(),
            Some("ent:42")
        );
        assert_eq!(
            search_command("cargo test graph"),
            None,
            "not a search program"
        );
        assert_eq!(search_command("echo rg pattern"), None);
        assert_eq!(
            searched_identifier("bash", Some(&json!({"command": "rg -n 'foo_bar' src"})))
                .as_deref(),
            Some("foo_bar")
        );
        assert_eq!(
            searched_identifier("grep", Some(&json!({"pattern": "*.rs"}))),
            None
        );
        assert_eq!(
            searched_identifier("read", Some(&json!({"file_path": "/x"}))),
            None
        );
    }

    #[test]
    fn identifiers_are_closed_and_long_enough() {
        assert!(!identifier("ab"));
        assert!(identifier("abc"));
        assert!(identifier("ent:0123"));
        assert!(identifier("Type::method"));
        assert!(!identifier("**/*.rs"));
        assert!(!identifier("a b"));
        assert!(!identifier("callers("));
    }

    #[test]
    fn hints_render_one_short_line_per_hit() {
        let hits = vec![
            Hit {
                name: "parse_config".into(),
                kind: "function".into(),
                file: "src/config.rs".into(),
                line: 42,
                callers: 2,
            },
            Hit {
                name: "helper".into(),
                kind: "definition".into(),
                file: "greeter.py".into(),
                line: 0,
                callers: 0,
            },
        ];
        let text = render(&hits);
        assert!(text.contains("parse_config function src/config.rs:42 (2 callers)\n"));
        assert!(text.contains("helper definition greeter.py (0 callers)\n"));
        assert!(text.ends_with("use context/lookup."));
    }

    #[test]
    fn session_state_is_sanitized_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = state_path_in(Some(dir.path()), "ses/../../evil id!").unwrap();
        assert_eq!(
            path,
            dir.path().join("chaosbox/hints/sesevilid"),
            "only [A-Za-z0-9_-] survives"
        );
        assert_eq!(state_path_in(Some(dir.path()), "///"), None);
        let state = State::load_at(path.clone());
        assert_eq!((state.hints, state.patterns.len()), (0, 0));
        let state = State {
            path,
            hints: HINTS_MAX,
            patterns: vec!["parse_config".into()],
        };
        state.save();
        let reloaded = State::load_at(state.path);
        assert_eq!((reloaded.hints, reloaded.patterns.len()), (HINTS_MAX, 1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&reloaded.path)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
