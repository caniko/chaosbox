//! Agent and git integration: per-project `enable`, user-scope `install` and
//! `hook install`. Edits are marked, idempotent blocks or single JSON entries;
//! foreign content, symlinked (home-manager managed), read-only or
//! unparseable targets are never rewritten. The exact snippet is printed
//! instead.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const BEGIN: &str = ">>> chaosbox >>>";
const END: &str = "<<< chaosbox <<<";

/// Comment syntax framing a marked block.
#[derive(Clone, Copy)]
struct Marks {
    open: &'static str,
    close: &'static str,
}

const SHELL: Marks = Marks {
    open: "# ",
    close: "",
};
const MARKDOWN: Marks = Marks {
    open: "<!-- ",
    close: " -->",
};

fn mark(marks: Marks, text: &str) -> String {
    format!("{}{text}{}", marks.open, marks.close)
}

/// Byte range of the marked block, including its trailing newline.
fn find_block(text: &str, marks: Marks) -> Option<(usize, usize)> {
    let begin = format!("{}\n", mark(marks, BEGIN));
    let start = text
        .match_indices(&begin)
        .map(|(i, _)| i)
        .find(|&i| i == 0 || text[..i].ends_with('\n'))?;
    let end_mark = mark(marks, END);
    let mut end = start + text[start..].find(&end_mark)? + end_mark.len();
    if text[end..].starts_with('\n') {
        end += 1;
    }
    Some((start, end))
}

/// Insert or replace the marked block; everything outside it is untouched.
fn upsert_block(text: &str, body: &str, marks: Marks) -> String {
    let block = format!(
        "{}\n{}{}{}\n",
        mark(marks, BEGIN),
        body,
        if body.ends_with('\n') { "" } else { "\n" },
        mark(marks, END)
    );
    match find_block(text, marks) {
        Some((start, end)) => format!("{}{block}{}", &text[..start], &text[end..]),
        None if text.is_empty() || text.ends_with('\n') => format!("{text}{block}"),
        None => format!("{text}\n{block}"),
    }
}

fn remove_block(text: &str, marks: Marks) -> Option<String> {
    find_block(text, marks).map(|(start, end)| format!("{}{}", &text[..start], &text[end..]))
}

/// Existing text of an editable target; `Err` explains why it is left alone.
fn editable(path: &Path) -> Result<Option<String>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("symlink (likely managed by home-manager or another tool)".into())
        }
        Ok(meta) if !meta.is_file() => Err("not a regular file".into()),
        Ok(meta) if meta.permissions().readonly() => Err("read-only".into()),
        Ok(_) => std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

/// Atomic replace keeping an existing file's mode (`mode` for new files).
fn write_file(path: &Path, text: &str, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    let mode = std::fs::metadata(path).map_or(mode, |m| m.permissions().mode());
    super::query_cmd::write_output(path, text.as_bytes())?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// One planned edit, printed as JSON; applied only when `contents` is set.
#[derive(Debug)]
struct Change {
    path: PathBuf,
    action: &'static str,
    contents: Option<String>,
    note: Option<String>,
}

impl Change {
    fn report(&self) -> Value {
        json!({"path":self.path,"action":self.action,"note":self.note})
    }
}

fn refuse(path: &Path, why: &str, snippet: &str) -> Change {
    Change {
        path: path.to_owned(),
        action: "refused",
        contents: None,
        note: Some(format!("{why}; add this yourself:\n{snippet}")),
    }
}

/// Plan the project marker: create, leave alone, delete, or refuse a marker
/// that records a different repository.
fn plan_marker(path: &Path, repo: Option<&str>, remove: bool) -> Change {
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(&json!({ "repo": repo.unwrap_or_default() }))
            .unwrap_or_default()
    );
    let existing = match editable(path) {
        Ok(existing) => existing,
        Err(why) => return refuse(path, &why, &contents),
    };
    let recorded = existing
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.get("repo")?.as_str().map(str::to_owned));
    if existing.is_some() && recorded.is_none() {
        return refuse(path, "not a Chaosbox marker", &contents);
    }
    let change = |action, contents| Change {
        path: path.to_owned(),
        action,
        contents,
        note: None,
    };
    if remove {
        return match recorded {
            Some(_) => change("delete", None),
            None => change("unchanged", None),
        };
    }
    let Some(repo) = repo else {
        return change("unchanged", None);
    };
    match recorded.as_deref() {
        Some(current) if current == repo => change("unchanged", None),
        Some(current) => refuse(
            path,
            &format!("a different repo is already recorded ({current})"),
            &contents,
        ),
        None => change("create", Some(contents)),
    }
}

/// Plan a marked-block edit (`remove` deletes the block instead).
fn plan_block(path: &Path, body: &str, marks: Marks, remove: bool) -> Change {
    let block = upsert_block("", body, marks);
    let existing = match editable(path) {
        Ok(existing) => existing,
        Err(why) => return refuse(path, &why, &block),
    };
    let current = existing.clone().unwrap_or_default();
    let next = if remove {
        remove_block(&current, marks)
    } else {
        Some(upsert_block(&current, body, marks))
    };
    let (action, contents) = match next {
        Some(next) if Some(&next) == existing.as_ref() => ("unchanged", None),
        Some(next) if remove => ("remove", Some(next)),
        Some(next) => (
            if existing.is_some() {
                "update"
            } else {
                "create"
            },
            Some(next),
        ),
        None => ("unchanged", None),
    };
    Change {
        path: path.to_owned(),
        action,
        contents,
        note: None,
    }
}

/// Plan a JSON entry at `parent.key`, refusing to replace a different one.
fn plan_json(path: &Path, parent: &str, key: &str, value: &Value, remove: bool) -> Change {
    let snippet =
        serde_json::to_string_pretty(&json!({ parent: { key: value } })).unwrap_or_default();
    let existing = match editable(path) {
        Ok(existing) => existing,
        Err(why) => return refuse(path, &why, &snippet),
    };
    let canonical = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
    let mut config: Value = match existing.as_deref() {
        None => json!({}),
        Some(text) => match serde_json::from_str(text) {
            // Rewriting would reorder or reformat a hand-written file; only
            // canonical files (including ones written here) are edited.
            Ok(config @ Value::Object(_)) if canonical(&config) == text => config,
            Ok(Value::Object(_)) => {
                return refuse(path, "editing would reformat this file", &snippet)
            }
            _ => return refuse(path, "not plain JSON (comments or another shape)", &snippet),
        },
    };
    let current = config.get(parent).and_then(|p| p.get(key)).cloned();
    match (current, remove) {
        (Some(current), _) if &current != value => {
            return refuse(
                path,
                &format!("a different `{parent}.{key}` exists"),
                &snippet,
            );
        }
        (Some(_), false) | (None, true) => {
            return Change {
                path: path.to_owned(),
                action: "unchanged",
                contents: None,
                note: None,
            };
        }
        (Some(_), true) => {
            if let Some(entries) = config[parent].as_object_mut() {
                entries.remove(key);
            }
        }
        (None, false) => {
            if !config[parent].is_object() {
                config[parent] = json!({});
            }
            config[parent][key] = value.clone();
        }
    }
    let text = canonical(&config);
    Change {
        path: path.to_owned(),
        action: if remove {
            "remove"
        } else if existing.is_some() {
            "update"
        } else {
            "create"
        },
        contents: Some(text),
        note: None,
    }
}

/// Apply (unless `dry_run`) and print every change; new files get `mode`.
/// Returns 1 if anything was refused or failed.
fn apply(changes: &[Change], dry_run: bool, notes: &[String], mode: u32) -> i32 {
    let mut code = 0;
    for change in changes {
        let mut report = change.report();
        let result = match (change.action, &change.contents) {
            ("refused", _) => Err(None),
            (_, Some(text)) if dry_run => {
                report["contents"] = json!(text);
                Ok(())
            }
            (_, _) if dry_run => Ok(()),
            ("delete", _) => std::fs::remove_file(&change.path).map_err(|e| Some(e.to_string())),
            (_, Some(text)) => write_file(&change.path, text, mode).map_err(Some),
            (_, None) => Ok(()),
        };
        if let Err(failure) = result {
            code = 1;
            if let Some(e) = failure {
                report["action"] = json!("failed");
                report["note"] = json!(e);
            }
        }
        println!("{report}");
    }
    for note in notes {
        println!("{}", json!({"note": note}));
    }
    code
}

/// `chaosbox hook`.
#[derive(Debug, clap::Subcommand)]
pub(super) enum HookCmd {
    /// Refresh the graph after commits, merges and branch switches. Runs in
    /// the background (`run --no-decisions`) so git never waits, and only
    /// when `CHAOSBOX_DB_BACKEND=typedb` and `chaosbox` are available.
    Install {
        /// Repository (any directory inside it).
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Graph repository name passed to `run --repo`.
        #[arg(long)]
        repo: String,
        #[arg(long = "source-paths")]
        source_paths: Vec<String>,
        #[arg(long = "exclude")]
        exclude: Vec<String>,
        /// Print the planned edits without writing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove the hook blocks `install` added, leaving other hook content.
    Uninstall {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}

const HOOKS: [&str; 3] = ["post-commit", "post-checkout", "post-merge"];

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Shell-safe hook arguments: a closed character set, then single-quoted.
fn hook_word(value: &str) -> Result<String, String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/-*?{},".contains(c))
    {
        return Err(format!("unsupported character in hook argument {value:?}"));
    }
    Ok(format!("'{value}'"))
}

fn hook_body(repo: &str, source_paths: &[String], exclude: &[String]) -> Result<String, String> {
    let mut args = format!("--repo {}", hook_word(repo)?);
    for path in source_paths {
        let _ = write!(args, " --source-paths {}", hook_word(path)?);
    }
    for glob in exclude {
        let _ = write!(args, " --exclude {}", hook_word(glob)?);
    }
    Ok(format!(
        "# Managed by `chaosbox hook install`: refresh the graph in the background.\n\
         if [ \"$(basename \"$0\")\" != post-checkout ] || [ \"$3\" = 1 ]; then\n\
         \x20 if [ \"${{CHAOSBOX_DB_BACKEND:-}}\" = typedb ] && command -v chaosbox >/dev/null 2>&1; then\n\
         \x20   (cd \"$(git rev-parse --show-toplevel)\" && nohup chaosbox run . {args} --no-decisions </dev/null >/dev/null 2>&1 &)\n\
         \x20 fi\n\
         fi\n"
    ))
}

fn plan_hook(path: &Path, body: Option<&str>) -> Change {
    let shebang = "#!/bin/sh\n";
    let existing = match editable(path) {
        Ok(existing) => existing,
        Err(why) => return refuse(path, &why, body.unwrap_or_default()),
    };
    if let Some(text) = &existing {
        let first = text.lines().next().unwrap_or_default();
        if !(first.starts_with("#!") && first.contains("sh")) {
            return refuse(path, "not a shell hook", body.unwrap_or_default());
        }
    }
    let mut change = plan_block(path, body.unwrap_or_default(), SHELL, body.is_none());
    if let Some(next) = &change.contents {
        if existing.is_none() {
            change.contents = Some(format!("{shebang}{next}"));
        } else if body.is_none() && next == shebang {
            // Only our block was ever there: remove the file entirely.
            change.action = "delete";
            change.contents = None;
        }
    }
    change
}

pub(super) fn hook(cmd: &HookCmd) -> i32 {
    let (dir, body, dry_run) = match cmd {
        HookCmd::Install {
            dir,
            repo,
            source_paths,
            exclude,
            dry_run,
        } => match hook_body(repo, source_paths, exclude) {
            Ok(body) => (dir, Some(body), *dry_run),
            Err(e) => {
                eprintln!("hook install: {e}");
                return 1;
            }
        },
        HookCmd::Uninstall { dir, dry_run } => (dir, None, *dry_run),
    };
    // Honors core.hooksPath and linked worktrees.
    let hooks = match git(dir, &["rev-parse", "--git-path", "hooks"]) {
        Ok(hooks) => dir.join(hooks),
        Err(e) => {
            eprintln!("hook: not a git repository: {e}");
            return 1;
        }
    };
    let changes: Vec<Change> = HOOKS
        .iter()
        .map(|name| plan_hook(&hooks.join(name), body.as_deref()))
        .collect();
    apply(&changes, dry_run, &[], 0o755)
}

/// Agent platforms `install` can configure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(super) enum Platform {
    /// Claude Code (`~/.claude/settings.json` hooks; MCP via its CLI).
    Claude,
    /// `OpenCode` (`~/.config/opencode/opencode.json`).
    Opencode,
    /// Codex (`~/.codex/config.toml`).
    Codex,
}

/// `chaosbox install` / `uninstall` (user scope only).
#[derive(Debug, clap::Args)]
pub(super) struct InstallArgs {
    #[arg(long, value_enum)]
    platform: Platform,
    /// Print the planned edits without writing.
    #[arg(long)]
    dry_run: bool,
}

/// `chaosbox enable`: record a project's marker and steering guidance.
#[derive(Debug, clap::Args)]
pub(super) struct EnableArgs {
    /// Graph repository name recorded in `.chaosbox.json`.
    #[arg(long)]
    repo: String,
    /// Any directory inside the checkout.
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Print the planned edits without writing.
    #[arg(long)]
    dry_run: bool,
}

/// `chaosbox disable`.
#[derive(Debug, clap::Args)]
pub(super) struct DisableArgs {
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    #[arg(long)]
    dry_run: bool,
}

/// Imperative, self-contained steering for the project's agents (including
/// subagents, which never see MCP instructions).
fn guidance(repo: &str) -> String {
    format!(
        "## Chaosbox is enabled for this project\n\n\
         This project is indexed by Chaosbox as repo `{repo}`. You MUST ask Chaosbox \
         before Grep, Glob or Read for: a definition, the source of a symbol, callers \
         or callees, the impact of a change (`review` for a diff), or architecture.\n\n\
         - Source returned by Chaosbox is the file content: do not Read it again.\n\
         - Read directly only lines Chaosbox did not return or marked `stale`.\n\
         - Do not re-verify a Chaosbox answer with grep.\n\
         - If Chaosbox is unavailable or says this project is not enabled, fall back \
         to the normal tools.\n\
         - Subagents do not see MCP instructions: pass this rule on when you delegate.\n"
    )
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Checkout root of `dir` (linked worktrees included).
fn git_root(dir: &Path) -> Result<PathBuf, String> {
    let root = git(dir, &["rev-parse", "--show-toplevel"])?;
    (!root.is_empty())
        .then(|| PathBuf::from(root))
        .ok_or_else(|| "empty git root".to_owned())
}

fn plan_enable(root: &Path, repo: &str) -> Result<Vec<Change>, Change> {
    let marker = plan_marker(&root.join(chaosbox::enablement::MARKER), Some(repo), false);
    // A refused marker leaves the project alone entirely: guidance must never
    // name a repository the marker does not record.
    if marker.action == "refused" {
        return Err(marker);
    }
    let text = guidance(repo);
    let agents = root.join("AGENTS.md");
    let claude = root.join("CLAUDE.md");
    let mut changes = vec![marker, plan_block(&agents, &text, MARKDOWN, false)];
    // Keep the agent file the project already uses; a project with neither
    // gets CLAUDE.md (Claude Code reads it without extra configuration).
    if claude.exists() || !agents.exists() {
        changes.push(plan_block(&claude, &text, MARKDOWN, false));
    }
    Ok(changes)
}

fn plan_disable(root: &Path) -> Vec<Change> {
    vec![
        plan_marker(&root.join(chaosbox::enablement::MARKER), None, true),
        plan_block(&root.join("AGENTS.md"), "", MARKDOWN, true),
        plan_block(&root.join("CLAUDE.md"), "", MARKDOWN, true),
    ]
}

pub(super) fn enable(args: &EnableArgs) -> i32 {
    if let Err(error) = hook_word(&args.repo) {
        eprintln!("enable: {error}");
        return 1;
    }
    match git_root(&args.dir) {
        Ok(root) => {
            let changes = match plan_enable(&root, &args.repo) {
                Ok(changes) => changes,
                Err(refusal) => vec![refusal],
            };
            apply(&changes, args.dry_run, &[], 0o644)
        }
        Err(error) => {
            eprintln!("enable: not a git repository: {error}");
            1
        }
    }
}

pub(super) fn disable(args: &DisableArgs) -> i32 {
    match git_root(&args.dir) {
        Ok(root) => apply(&plan_disable(&root), args.dry_run, &[], 0o644),
        Err(error) => {
            eprintln!("disable: not a git repository: {error}");
            1
        }
    }
}

/// The `PostToolUse` handler `install` adds to Claude Code.
const HINT_MATCHER: &str = "Grep|Glob|Bash|Read";
const HINT_COMMAND: &str = "chaosbox hint";

fn is_our_hook(entry: &Value) -> bool {
    entry.get("type").and_then(Value::as_str) == Some("command")
        && entry.get("command").and_then(Value::as_str) == Some(HINT_COMMAND)
}

/// Plan the managed `chaosbox hint` `PostToolUse` hooks in a Claude Code
/// settings file. Foreign hooks are kept; only entries whose command is
/// exactly ours are added or removed.
fn plan_hooks(path: &Path, remove: bool) -> Change {
    let entry = json!({
        "matcher": HINT_MATCHER,
        "hooks": [{"type": "command", "command": HINT_COMMAND}],
    });
    let snippet = serde_json::to_string_pretty(&json!({"hooks": {"PostToolUse": [entry]}}))
        .unwrap_or_default();
    let change = |action, contents| Change {
        path: path.to_owned(),
        action,
        contents,
        note: None,
    };
    let existing = match editable(path) {
        Ok(existing) => existing,
        Err(why) => return refuse(path, &why, &snippet),
    };
    let Some(text) = existing else {
        return if remove {
            change("unchanged", None)
        } else {
            change("create", Some(format!("{snippet}\n")))
        };
    };
    let mut config: Value = match serde_json::from_str(&text) {
        Ok(config @ Value::Object(_)) => config,
        _ => return refuse(path, "not a JSON object", &snippet),
    };
    if config.get("hooks").is_some_and(|hooks| !hooks.is_object()) {
        return if remove {
            change("unchanged", None)
        } else {
            refuse(path, "`hooks` is not a JSON object", &snippet)
        };
    }
    let groups = match config["hooks"].get("PostToolUse") {
        None => Vec::new(),
        Some(Value::Array(groups)) => groups.clone(),
        Some(_) if remove => return change("unchanged", None),
        Some(_) => return refuse(path, "`hooks.PostToolUse` is not an array", &snippet),
    };
    let mut changed = groups.clone();
    if remove {
        for group in &mut changed {
            if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                hooks.retain(|hook| !is_our_hook(hook));
            }
        }
        changed.retain(|group| {
            !group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        });
        if changed == groups {
            return change("unchanged", None);
        }
        if changed.is_empty() {
            if let Some(hooks) = config["hooks"].as_object_mut() {
                hooks.remove("PostToolUse");
            }
        } else {
            config["hooks"]["PostToolUse"] = Value::Array(changed);
        }
    } else {
        if groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| hooks.iter().any(is_our_hook))
        }) {
            return change("unchanged", None);
        }
        if !config["hooks"].is_object() {
            config["hooks"] = json!({});
        }
        changed.push(entry);
        config["hooks"]["PostToolUse"] = Value::Array(changed);
    }
    if config["hooks"]
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        if let Some(config) = config.as_object_mut() {
            config.remove("hooks");
        }
    }
    change(
        "update",
        Some(format!(
            "{}\n",
            serde_json::to_string_pretty(&config).unwrap_or_default()
        )),
    )
}

fn plan_install(args: &InstallArgs, remove: bool) -> (Vec<Change>, Vec<String>) {
    let base = home();
    let mut notes = Vec::new();
    let mut changes = match args.platform {
        Platform::Claude => {
            // Claude Code owns ~/.claude.json; let its CLI make the edit.
            notes.push(if remove {
                "run: claude mcp remove --scope user chaosbox".to_owned()
            } else {
                "run: claude mcp add --scope user chaosbox -- chaosbox mcp".to_owned()
            });
            vec![plan_hooks(&base.join(".claude/settings.json"), remove)]
        }
        Platform::Opencode => vec![plan_json(
            &base.join(".config/opencode/opencode.json"),
            "mcp",
            "chaosbox",
            &json!({"type":"local","command":["chaosbox","mcp"],"enabled":true}),
            remove,
        )],
        Platform::Codex => {
            let config = base.join(".codex/config.toml");
            let table = "[mcp_servers.chaosbox]\ncommand = \"chaosbox\"\nargs = [\"mcp\"]\n";
            let foreign = editable(&config).ok().flatten().is_some_and(|t| {
                let outside = remove_block(&t, SHELL).unwrap_or(t);
                outside.contains("[mcp_servers.chaosbox]")
            });
            let toml = if foreign && !remove {
                refuse(
                    &config,
                    "`[mcp_servers.chaosbox]` already exists outside the managed block",
                    table,
                )
            } else {
                plan_block(&config, table, SHELL, remove)
            };
            vec![toml]
        }
    };
    // An older `install` wrote a global guidance block; remove it on
    // uninstall, but never write one now (enable is per project).
    if remove {
        let (path, marks) = match args.platform {
            Platform::Claude => (base.join(".claude/CLAUDE.md"), MARKDOWN),
            Platform::Opencode => (base.join(".config/opencode/AGENTS.md"), MARKDOWN),
            Platform::Codex => (base.join(".codex/AGENTS.md"), MARKDOWN),
        };
        changes.push(plan_block(&path, "", marks, true));
    }
    (changes, notes)
}

pub(super) fn install(args: &InstallArgs, remove: bool) -> i32 {
    let (changes, notes) = plan_install(args, remove);
    apply(&changes, args.dry_run, &notes, 0o644)
}

#[cfg(test)]
mod tests {
    use super::{
        guidance, hook_body, plan_disable, plan_enable, plan_hook, plan_hooks, plan_json,
        remove_block, upsert_block, Change, MARKDOWN, SHELL,
    };
    use serde_json::json;

    fn apply_changes(changes: &[Change]) {
        for change in changes {
            match (change.action, &change.contents) {
                (_, Some(text)) => {
                    if let Some(parent) = change.path.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(&change.path, text).unwrap();
                }
                ("delete", None) => {
                    let _ = std::fs::remove_file(&change.path);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn guidance_is_imperative_bounded_and_self_contained() {
        let text = guidance("canix");
        assert!(
            text.chars().count() <= 1200,
            "{} chars",
            text.chars().count()
        );
        assert!(text.contains("`canix`"));
        assert!(text.contains("MUST ask Chaosbox"));
        assert!(text.contains("Do not re-verify"));
        assert!(text.contains("Subagents"));
    }

    #[test]
    fn enable_and_disable_round_trip_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        apply_changes(&plan_enable(root, "canix").unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join(".chaosbox.json")).unwrap(),
            "{\n  \"repo\": \"canix\"\n}\n"
        );
        let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(agents.contains(">>> chaosbox >>>"));
        assert!(root.join("CLAUDE.md").is_file());

        // Idempotent, and a different repo is refused.
        let again = plan_enable(root, "canix").unwrap();
        assert!(again.iter().all(|change| change.action == "unchanged"));
        assert_eq!(plan_enable(root, "other").unwrap_err().action, "refused");

        apply_changes(&plan_disable(root));
        assert!(!root.join(".chaosbox.json").exists());
        assert_eq!(std::fs::read_to_string(root.join("AGENTS.md")).unwrap(), "");
        assert_eq!(std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(), "");
    }

    #[test]
    fn enable_keeps_existing_agent_files_intact() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("AGENTS.md"), "# House rules\n").unwrap();
        apply_changes(&plan_enable(root, "canix").unwrap());
        let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
        assert!(agents.starts_with("# House rules\n"));
        assert!(
            !root.join("CLAUDE.md").exists(),
            "an existing AGENTS.md is the project's agent file"
        );
        apply_changes(&plan_disable(root));
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "# House rules\n"
        );
    }

    #[test]
    fn hint_hooks_are_added_idempotently_and_removed_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&json!({
                "model": "opus",
                "hooks": {"PostToolUse": [{
                    "matcher": "Write",
                    "hooks": [{"type": "command", "command": "my-hook"}],
                }]},
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
        let created = plan_hooks(&path, false);
        assert_eq!(created.action, "update");
        apply_changes(std::slice::from_ref(&created));
        assert_eq!(plan_hooks(&path, false).action, "unchanged");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let groups = config["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 2, "the foreign hook group survives");
        assert_eq!(groups[0]["matcher"], "Write");
        assert_eq!(groups[1]["hooks"][0]["command"], "chaosbox hint");

        let removed = plan_hooks(&path, true);
        apply_changes(std::slice::from_ref(&removed));
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(config["model"], "opus");
        assert_eq!(plan_hooks(&path, true).action, "unchanged");

        // A missing settings file is never created by uninstall.
        let missing = dir.path().join("absent.json");
        assert_eq!(plan_hooks(&missing, true).action, "unchanged");
        assert!(!missing.exists());
        assert_eq!(plan_hooks(&missing, false).action, "create");
    }

    #[test]
    fn marked_blocks_are_idempotent_and_reversible() {
        let original = "# Project\n\nOwn notes.\n";
        let once = upsert_block(original, "Use the graph.\n", MARKDOWN);
        assert_eq!(upsert_block(&once, "Use the graph.\n", MARKDOWN), once);
        let changed = upsert_block(&once, "Updated.\n", MARKDOWN);
        assert_eq!(changed.matches(">>> chaosbox >>>").count(), 1);
        assert_eq!(remove_block(&changed, MARKDOWN).as_deref(), Some(original));
        assert_eq!(remove_block(original, MARKDOWN), None);
        let shell = upsert_block("#!/bin/sh\necho mine\n", "echo ours\n", SHELL);
        assert!(shell.starts_with("#!/bin/sh\necho mine\n# >>> chaosbox >>>\n"));
    }

    #[test]
    fn json_and_hook_edits_never_clobber_foreign_content() {
        let dir = tempfile::tempdir().unwrap();
        let ours = json!({"command":"chaosbox","args":["mcp"]});
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "{\"mcpServers\":{\"other\":{}}, \"z\":1}\n").unwrap();
        assert_eq!(
            plan_json(&path, "mcpServers", "chaosbox", &ours, false).action,
            "refused",
            "hand-formatted files are never reformatted"
        );
        let created = plan_json(
            &dir.path().join("new.json"),
            "mcpServers",
            "chaosbox",
            &ours,
            false,
        );
        assert_eq!(created.action, "create");
        std::fs::write(&path, created.contents.unwrap()).unwrap();
        assert_eq!(
            plan_json(&path, "mcpServers", "chaosbox", &ours, false).action,
            "unchanged"
        );
        let removed = plan_json(&path, "mcpServers", "chaosbox", &ours, true);
        assert_eq!(
            removed.contents.as_deref(),
            Some("{\n  \"mcpServers\": {}\n}\n")
        );
        std::fs::write(&path, "{\"mcpServers\":{\"chaosbox\":{\"command\":\"x\"}}}").unwrap();
        assert_eq!(
            plan_json(&path, "mcpServers", "chaosbox", &ours, false).action,
            "refused"
        );
        std::fs::write(&path, "{ // comment\n}").unwrap();
        assert_eq!(
            plan_json(&path, "mcpServers", "chaosbox", &ours, false).action,
            "refused"
        );

        let body = hook_body("repo", &[], &["gen/**".into()]).unwrap();
        assert!(body.contains("--repo 'repo' --exclude 'gen/**' --no-decisions"));
        assert!(hook_body("re'po", &[], &[]).is_err());
        assert!(hook_body("repo;rm", &[], &[]).is_err());
        let hook = dir.path().join("post-commit");
        std::fs::write(&hook, "#!/usr/bin/env python3\n").unwrap();
        assert_eq!(plan_hook(&hook, Some(&body)).action, "refused");
        std::fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
        let installed = plan_hook(&hook, Some(&body)).contents.unwrap();
        std::fs::write(&hook, &installed).unwrap();
        assert_eq!(
            plan_hook(&hook, None).contents.as_deref(),
            Some("#!/bin/sh\necho mine\n"),
            "uninstall restores the original bytes"
        );
        let fresh = dir.path().join("post-merge");
        let created = plan_hook(&fresh, Some(&body)).contents.unwrap();
        std::fs::write(&fresh, &created).unwrap();
        let removal = plan_hook(&fresh, None);
        assert_eq!((removal.action, removal.contents), ("delete", None));
    }
}
