//! Agent and git integration (Graphify's `install` and `hook install`).
//! Edits are marked, idempotent blocks or single JSON entries; foreign
//! content, symlinked (home-manager managed), read-only or unparseable
//! targets are never rewritten. The exact snippet is printed instead.

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
    /// Claude Code (`.mcp.json`, `CLAUDE.md`).
    Claude,
    /// `OpenCode` (`opencode.json`, `AGENTS.md`).
    Opencode,
    /// Codex (`.codex/config.toml`, `AGENTS.md`).
    Codex,
}

/// `chaosbox install` / `uninstall`.
#[derive(Debug, clap::Args)]
pub(super) struct InstallArgs {
    #[arg(long, value_enum)]
    platform: Platform,
    /// Configure the project in `--dir` instead of the user's global setup.
    #[arg(long)]
    project: bool,
    #[arg(long, default_value = ".")]
    dir: PathBuf,
    /// Graph repository name agents should pass as `repo`.
    #[arg(long)]
    repo: Option<String>,
    /// Print the planned edits without writing.
    #[arg(long)]
    dry_run: bool,
}

fn guidance(repo: Option<&str>) -> String {
    let repo = repo.map_or_else(
        || "the repository name the operator indexed".to_owned(),
        |r| format!("`{r}`"),
    );
    format!(
        "## Chaosbox code graph\n\n\
         This code is indexed by Chaosbox; pass {repo} as `repo`. Ask the graph before \
         grepping for callers, dependencies or architecture: `context` (a question \
         becomes a bounded subgraph), `neighbors` (`direction: incoming` lists callers), \
         `path`, `explain`, and `stats` or `report` (communities, god nodes, surprising \
         connections, suggested questions). Entity arguments take ids or exact names. \
         Every relationship cites evidence (`evidence`); INFERRED edges are unique \
         lexical matches, and a missing edge is not proof that nothing uses a symbol.\n"
    )
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn plan_install(args: &InstallArgs, remove: bool) -> (Vec<Change>, Vec<String>) {
    let base = if args.project {
        args.dir.clone()
    } else {
        home()
    };
    let text = guidance(args.repo.as_deref());
    let mut notes = Vec::new();
    let changes = match (args.platform, args.project) {
        (Platform::Claude, true) => vec![
            plan_json(
                &base.join(".mcp.json"),
                "mcpServers",
                "chaosbox",
                &json!({"command":"chaosbox","args":["mcp"]}),
                remove,
            ),
            plan_block(&base.join("CLAUDE.md"), &text, MARKDOWN, remove),
        ],
        (Platform::Claude, false) => {
            // Claude Code owns ~/.claude.json; let its CLI make the edit.
            notes.push(if remove {
                "run: claude mcp remove --scope user chaosbox".to_owned()
            } else {
                "run: claude mcp add --scope user chaosbox -- chaosbox mcp".to_owned()
            });
            vec![plan_block(
                &base.join(".claude/CLAUDE.md"),
                &text,
                MARKDOWN,
                remove,
            )]
        }
        (Platform::Opencode, project) => {
            let config = if project {
                base.join("opencode.json")
            } else {
                base.join(".config/opencode/opencode.json")
            };
            let agents = if project {
                base.join("AGENTS.md")
            } else {
                base.join(".config/opencode/AGENTS.md")
            };
            vec![
                plan_json(
                    &config,
                    "mcp",
                    "chaosbox",
                    &json!({"type":"local","command":["chaosbox","mcp"],"enabled":true}),
                    remove,
                ),
                plan_block(&agents, &text, MARKDOWN, remove),
            ]
        }
        (Platform::Codex, project) => {
            let root = base.join(".codex");
            let config = root.join("config.toml");
            let agents = if project {
                base.join("AGENTS.md")
            } else {
                root.join("AGENTS.md")
            };
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
            vec![toml, plan_block(&agents, &text, MARKDOWN, remove)]
        }
    };
    (changes, notes)
}

pub(super) fn install(args: &InstallArgs, remove: bool) -> i32 {
    let (changes, notes) = plan_install(args, remove);
    apply(&changes, args.dry_run, &notes, 0o644)
}

#[cfg(test)]
mod tests {
    use super::{hook_body, plan_hook, plan_json, remove_block, upsert_block, MARKDOWN, SHELL};
    use serde_json::json;

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
