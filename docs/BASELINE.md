# Baseline (2026-09-18, host atlas)

## Revisions

| Repo | Ref | SHA |
|---|---|---|
| Graphify-Labs/graphify (upstream) | HEAD (= v8 tip) | `26b02b5e3430e4ab85dd7e72c7b98836d8e65c48` |
| Graphify-Labs/graphify | main | `91f4d120b630ee35c79bf3c75ccd186870a808f9` |
| Graphify-Labs/graphify | tag v1.0.0 | `0a31c0862b600d0755b0b8da41d6cdf99df135df` |
| caniko/harbor-rs | trunk | `7a3328e186258dca31f9801227bc4e6fd8db4f36` (local checkout same) |
| caniko/harbor-db | trunk | `a1ae83b150cf5907ce7c1a4a90c215d13f3801f5` (local checkout `5c605fd`, behind) |
| caniko/simit | trunk | `39aed87150b01131110f269f591bf38c50ad4bef` (local checkout `b16a5af` on `codex/release-whitespace`) |
| caniko/chaosbox | (new, empty at start) | initialized this session |
| gel-tokio (crates.io) | 0.11.0 | API verified: `create_client()`, `Client::query/query_json/query_single_json`, `QueryArgs`/`QueryResult` |
| TypeSafe docs | api + models (2026-09-18) | endpoint `POST /v1/systemone`, model `jev-1.13.0`, aliases `jev-latest`/`jev-preview` -> `jev-1.13.0`; 64k total / 32k state+longest; 250k tok/s, 1200 req/min; 429 honors `retry-after` |

## Graphify audit (reference only, never executed)

- `graphify/extract.py` + `graphify/extractors/` (40+ language extractors incl. `rust.py`, `markdown.py`, `engine.py`, `base.py`, `resolution.py`) — extraction
- `graphify/symbol_resolution.py`, `resolver_registry.py`, `markdown_resolution.py`, `ruby_resolution.py`, `cross_repo_calls.py`, `cross_repo_types.py` — identity resolution
- `graphify/cache.py` — cache
- `graphify/build.py`, `global_graph.py`, `cluster.py`, `dedup.py` — assembly
- `graphify/affected.py`, `watch.py` — incremental
- `graphify/cli.py`, `serve.py`, `mcp_ingest.py`, `querylog.py` — query/MCP
- `graphify/export.py`, `exporters/` — export; runtime output is NetworkX node-link JSON (`graphify-out/graph.json`: `nodes[{id,label,source_file,source_location}]`)
- `tests/`, fixtures under `tests/` — reference behavior

## Harbor interfaces consumed

- harbor-rs: `mkToolchain` (+`toolchainProfile`), `craneLib` dep/build split, `mkCross`, `mkDevShells`, flake `checks`/`formatter`/`packages`/`apps` layout (mirrored in `flake.nix`)
- harbor-db: `MigrationPlan`/`MigrationOperation`/`CommandSpec` plan format v1 (`PLAN_VERSION=1`), `run_plan`/`load_plan`, `Backend` (Postgres/ClickHouse only — no Gel backend yet, tracked as pending in HANDOFF)
- simit: workspace aggregate GitHub CI mode (`simit.toml`: `runtime="nix"`, `workspace=true`); local simit 0.17.13; named gates + ordered publication await the parallel simit session
