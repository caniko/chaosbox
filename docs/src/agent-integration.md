# Agent and git integration

Graphify's operational surface, without its network or model dependencies.
Commands edit marked, idempotent blocks or single JSON entries. They never
rewrite content they don't own.

## Enable a project

```sh
chaosbox enable --repo canix          # .chaosbox.json + agent guidance
chaosbox enable --repo canix --dir src --dry-run
chaosbox disable                      # removes the marker and both blocks
```

Projects opt in with `.chaosbox.json` (`{"repo": "canix"}`) at the checkout
root. Commit it to share the opt-in; no global switch enables a project.
`enable` records the marker and upserts the managed block into `AGENTS.md`
(and `CLAUDE.md` when that is the agent file the project already uses). The
block gives self-contained instructions: ask Chaosbox before Grep, Glob or
Read for definitions, symbol source, callers/callees, change impact
(`review` for a diff) or architecture. Returned source is file content. Read
directly only what Chaosbox omitted or marked `stale`, and don't re-verify
with grep. Use normal tools if Chaosbox is unavailable or not enabled.
Subagents never see MCP instructions, so only the block guides them.
`disable` removes the marker and both blocks. Chaosbox rejects a marker naming
a different repository and leaves it unchanged.

## Install for an agent (user scope)

```sh
chaosbox install --platform claude    # ~/.claude/settings.json hint hooks
chaosbox install --platform opencode  # ~/.config/opencode/opencode.json
chaosbox install --platform codex     # ~/.codex/config.toml
chaosbox uninstall --platform codex
```

`install` registers the read-only `chaosbox mcp` server and, for Claude Code,
the PostToolUse `chaosbox hint` hook. It writes no guidance: enable each
project explicitly. Claude Code's user scope is owned by its own CLI, so
`install` prints the `claude mcp add --scope user …` command instead of
editing `~/.claude.json`; its `~/.claude/settings.json` hooks are edited
without rewriting foreign hook entries. `uninstall` removes only entries
identical to the ones `install` writes, and also removes a stale global
guidance block left by older versions.

The command refuses symlinks (such as home-manager targets), read-only files,
files that aren't plain JSON, and hand-formatted JSON that a rewrite would
reorder. It prints the exact snippet to add and exits 1. For Codex, it refuses
an existing `[mcp_servers.chaosbox]` outside the managed block.
`--dry-run` prints each planned file and its new contents.

## Hints

The hook calls the hidden `chaosbox hint` command. It reads one harness
PostToolUse JSON object from stdin. In an enabled project, it turns a
`Grep`/`Glob` search or a simple `rg`/`grep`/`ag`/`ack` command into at most
five one-line graph hits (`name kind file:line (N callers)`) plus a pointer to
`context`/`lookup`. No shell parses the command. The hook skips repeat patterns
and caps hints at five per session (state under
`$XDG_STATE_HOME/chaosbox/hints/<session>`). It runs for at most about 300 ms
and fails silently with exit 0, never blocking the triggering tool. OpenCode's
compaction plugin waits at most one second and appends the same hint to the
tool result; the installed Claude Code hook declares a two-second timeout.
For `Read`, the hook emits nothing to the agent. If an MCP call served the file
as source in the last 30 minutes, the hook logs `read_after_served` instead.

## Git hooks

```nu
chaosbox hook install --repo canix --exclude 'docs/**'
chaosbox hook uninstall
```

The hooks are `post-commit`, `post-merge`, and `post-checkout` on branch
switches. The hook directory comes from `git rev-parse --git-path hooks`, so
`core.hooksPath` and linked worktrees work. Installation appends the block to
existing shell hooks. Uninstall restores the original bytes or deletes only a
file the installer created. The installer refuses non-shell hooks, restricts
arguments to a shell-safe character set, and single-quotes them.

Hooks start `chaosbox run . --no-decisions` in the background, so git never
waits. They do so only when `CHAOSBOX_DB_BACKEND=typedb` and `chaosbox` are
available in the hook environment. A concurrent loser is rejected by the
generation-guarded publication.

## Watch

```nu
chaosbox watch . --repo canix --no-decisions --interval-secs 5
```

`watch` takes every `run` argument. Each interval hashes the path, length and
mtime of exactly the files a capture would select (honoring `.gitignore`,
`.chaosboxignore` and `--exclude`). Only a changed fingerprint reads sources,
and only a changed content-addressed snapshot id runs the pipeline, so touches
never republish. Failed runs are reported and watching continues; Ctrl+C stops.

## Name resolution and exit codes

`query lookup`, `explain`, `neighbors` and `path` (and their MCP counterparts)
accept an entity id, exact name or qualified name:

| Exit | Meaning |
| --- | --- |
| 0 | Resolved (or an `ent:` id, whose absence still prints `null`) |
| 3 | Ambiguous: several exact matches, or a saturated search; candidates printed |
| 4 | No entity with that name; substring matches printed as suggestions |

`run` keeps its own exit 4 for the coverage gate. MCP reports ambiguity as
`-32602` with `data.code = "ambiguous"` and the candidates. Unknown names keep
each tool's historical result.

## Usage ledger

Every CLI `query` and MCP `tools/call` appends one JSON line to
`$CHAOSBOX_QUERY_LOG`, or by default `$XDG_STATE_HOME/chaosbox/queries.jsonl`
(directory 0700, file 0600). A line records time, surface, tool, repo, pinned
build, duration, exit/error code, response bytes, result count and the
arguments. It redacts cursors and logs only the size of arguments over 1 KiB.
For a response with the shared top-level `source` array, it also logs deduped,
repo-relative `served_files` (up to 50), the character count of the returned
sources (`served_chars`), and the checkout `root` if the caller ran inside an
enabled one. `chaosbox hint` logs
`surface: "hook"` lines, including `read_after_served` when a Read reopens a
recently served file. The log rotates to `.1` above 64 MiB. Set
`CHAOSBOX_QUERY_LOG=off` to disable it. Write failures never affect queries.
The run-bound read-view connector uses a separate audit, not this log.

```sh
LOG=${XDG_STATE_HOME:-$HOME/.local/state}/chaosbox/queries.jsonl
jq -r 'select(.surface != "hook") | .tool' "$LOG" | sort | uniq -c | sort -rn   # calls per tool
jq -s 'map(.bytes // 0) | add' "$LOG"                                          # bytes returned
jq -s '([.[] | select(.served_files != null)] | length) as $served
  | ([.[] | select(.tool == "read_after_served")] | length) as $after
  | {served: $served, read_after_served: $after,
     rate: (if $served == 0 then 0 else $after / $served end)}' "$LOG"         # read-after-served rate
```

## MCP tool surface (token economy)

`chaosbox mcp` advertises every tool in one `tools/list` page and, in an
enabled project, sends imperative `instructions` that route definitions,
symbol source, callers/callees, change impact and architecture questions to
Chaosbox before Grep, Glob or Read. An unmarked checkout advertises no graph
tools and answers every graph `tools/call` with "Chaosbox is not enabled for
this project. Do not call it; use normal file tools."; `--intelligence*` and
`--workspace` keep exactly their own tools either way.

In an enabled project, omit `repo` to use the `.chaosbox.json` repository.
An explicit `repo` overrides it. Every tool takes
`max_tokens` (default 8000, maximum 20000, four characters per token). Results
are trimmed item by item, highest rank first, and report `truncated` counts:
`{shown, total, next_cursor?}` for one trimmed collection, or
`{"<collection>": {shown, total}}` when several lost items. Totals are the
pre-trim counts and survive later budget passes; `neighbors` resumes with its
existing cursor when rows are dropped. When whole-item trimming cannot fit the
budget, the answer is a small
`{"error": "result exceeds max_tokens", "max_tokens": N, "needed_tokens": M,
"hint": ...}` tool result, never an over-sized body or a protocol error.

`context`, `lookup` and `explain` take `include: "source" | "signatures" |
"none"` (default `source`). Verified working-tree text is line-numbered like
`cat -n` and returned in the top-level `source` array
(`{file, start_line, end_line, text}`); `signatures` keeps only the span's
first line, and `none` returns pointers. Text is served only while the file
still matches the digest the build recorded (`file_versions`); otherwise the
entry is a pointer with `stale: true`. Paths outside the enabled root —
including `..`, absolute paths and escaping symlinks — are never read. When
served or stale files differ from `HEAD`, a top-level `freshness` object names
the commit and the files.

`status` defaults to `detail: "summary"`, which keeps the build id, snapshots,
commit, counts, coverage totals and freshness but drops the per-file coverage
rows; `detail: "full"` keeps them. `export` and `report` obey the same budget
(whole nodes/edges, whole `##` sections). `search` returns
`{"nodes": [...]}` so truncation can be reported.

`review` runs `git diff -U0 --no-ext-diff <base>` in the enabled root (`base`
defaults to `HEAD` and is validated with
`git rev-parse --verify --end-of-options`) and maps the old side of each hunk
to the indexed entities whose spans cover it, keeping every affected sibling
and dropping only entities that enclose another. Hunks that no entity covers
are listed under `unmapped`. A file whose bytes at `base` no longer match the
build's recorded digest is listed once with `"reason": "stale"` and asserts no
entities. For every changed entity the answer lists incoming
`calls`/`references` callers plus test-file callers, most-called first.
