# Canscribe in Chaosbox

Imported from `https://github.com/caniko/canscribe`, commit
`7702db52381cab32c2161747c2dfcb1308c27b7d`. The original source, tests,
Python lock, platform extras and native-runtime packaging are preserved here.
Future transcription changes belong to this directory.

Install with `uv sync --extra cpu` (or one of `amd`, `nvidia`, `apple`). The
`canscribe` and `ct` entrypoints remain compatible. Chaosbox exposes the same
engine with `chaosbox transcribe ...`; `CHAOSBOX_CANSCRIBE_BIN` selects an
explicit engine executable. It does not invoke a shell or implicitly download
models while running unrelated graph commands.

The root flake's `chaosbox-transcription` package binds the CPU engine. The
ordinary `chaosbox` package remains usable without the ML dependency closure.
GPU environments keep their existing mutually exclusive extras in this
directory's flake and uv configuration.

This is source and CLI consolidation, not graph ingestion: transcripts retain
the existing output format and are not silently inserted into the graph.
Retire the standalone repository only after this import is committed,
published, and its production consumers have moved to the bundled path.
