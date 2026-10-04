# Source-selected continuation

`checkpoint prepare` captures a bounded window of complete normalized native
JSONL messages without inference. Every message needs a native `id` and a
`type` of `user`, `assistant` or `tool`. System, compaction and synthetic records
are excluded; the latest native user message is retained as a mandatory anchor,
even outside the window.

```sh
chaosbox checkpoint prepare --source-jsonl source.jsonl --source opencode \
  --session SESSION --scope private:can --repo canix --start-line 1 \
  --max-records 16 --output input.json
chaosbox checkpoint assemble --input input.json --source-jsonl source.jsonl \
  --work private-work --privacy-reviewed --max-requests 16 \
  --max-input-tokens 1000000 --max-chars 120000 --output checkpoint.json
```

Assembly rechecks the exact original source before reuse or inference. Pinned
`jev-1.13.0` classifies supplied records through independent Choice questions;
Rust renders the selected original bytes with fixed templates and source ids.
No model writes checkpoint prose, invents references or adds evidence.

User/tool records are retained. Low-probability or low-confidence decisions and
uncertainty retain the record as unresolved. Assistant assertions are explicitly
marked as assertions; they do not prove execution or implementation. High-
confidence irrelevant assistant text may be omitted with a source digest,
unless it contains tools. Complete tool-call/result boundaries must fit the
selected window. Oversized records or packets fail instead of truncating intent.

The private SQLite work journal binds source/scope/repository identity and
persists conservative input reservations before network dispatch. Budgets are
cumulative across windows and concurrent attempts. Successful responses replay,
including negative selections and abstentions. Failed or interrupted sends
retain their reservation; `--retry` explicitly spends another one. Probability
distributions, maximum choices and model identity are validated before a
response is recorded as successful.

A checkpoint is a historical navigation view. It cannot authorize execution,
corroborate itself or replace the original recoverable history. This bounded
command does not establish automatic native-session transfer, an end-to-end
compaction audit or measured improvements in real coding sessions.
