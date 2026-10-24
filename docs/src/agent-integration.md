# Agent and git integration

Graphify's operational surface, without its network or model dependencies.
Every command here edits marked, idempotent blocks or single JSON entries and
never rewrites content it does not own.

## Install for an agent

```nu
chaosbox install --platform claude --project --repo canix    # .mcp.json + CLAUDE.md
chaosbox install --platform opencode --project --repo canix  # opencode.json + AGENTS.md
chaosbox install --platform codex --project --repo canix     # .codex/config.toml + AGENTS.md
chaosbox install --platform claude --dry-run                 # user scope, plan only
chaosbox uninstall --platform codex --project
```

`install` registers the read-only `chaosbox mcp` server and a short guidance
block telling agents to ask the graph before grepping, which tools answer what,
and that INFERRED edges are lexical matches whose absence proves nothing.
Without `--project` it targets the user's global configuration. Claude Code's
user scope is owned by its own CLI, so `install` prints the
`claude mcp add --scope user …` command instead of editing `~/.claude.json`.

Targets that are symlinks (for example home-manager managed), read-only, not
plain JSON, or hand-formatted JSON that a rewrite would reorder are refused;
the exact snippet to add is printed instead and the command exits 1. Codex
refuses when `[mcp_servers.chaosbox]` already exists outside the managed
block. `uninstall` removes only an entry identical to the one `install` writes.
`--dry-run` prints each planned file and its new contents.

## Git hooks

```nu
chaosbox hook install --repo canix --exclude 'docs/**'
chaosbox hook uninstall
```

The hooks are `post-commit`, `post-merge`, and `post-checkout` on branch
switches. The hook directory comes from `git rev-parse --git-path hooks`, so
`core.hooksPath` and linked worktrees work. Existing shell hooks keep their
content: the block is appended, and uninstall restores the original bytes or
deletes a file only this command created. Non-shell hooks are refused.
Arguments are restricted to a shell-safe character set and single-quoted.

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

`query lookup`, `explain`, `neighbors` and `path` (and the same MCP tools)
accept an entity id or an exact name or qualified name:

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
arguments: cursors are redacted, and arguments over 1 KiB are recorded only by
size. The file rotates to `.1` above 64 MiB. Set `CHAOSBOX_QUERY_LOG=off` to
disable it; write failures never affect the query. The run-bound read-view
connector keeps its own audit and is not logged here.
