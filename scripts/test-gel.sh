#!/usr/bin/env bash
# test-gel: disposable real-Gel integration gate for Chaosbox.
# - Uses a temp Gel instance only; never touches developer databases implicitly.
# - Applies committed migrations, runs the pipeline against a local mock Jev
#   HTTP service (no TypeSafe credentials needed).
# - Deterministic cleanup via trap. Exits 3 with PENDING when no Gel server
#   is available (honest pending, not a passing placeholder).
set -euo pipefail

WORK="$(mktemp -d "${TMPDIR:-/tmp}/chaosbox-test-gel.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

if ! command -v gel >/dev/null 2>&1; then
  echo "PENDING: no 'gel' CLI/server available; real Gel integration not exercisable here" >&2
  echo "pending: gel server unavailable (see docs/HANDOFF.md for the harbor-db dependency)" >&2
  exit 3
fi

echo "== mock Jev HTTP service gate (no credentials, loopback only) =="
cargo test -p chaosbox-jev http_tests --offline

echo "== schema assets =="
test -f "${CHAOSBOX_SCHEMA_DIR:-dbschema}/default.esdl" || test -f dbschema/default.esdl
ls dbschema/migrations/*.edgeql >/dev/null

echo "== disposable instance =="
export GEL_INSTANCE_DIR="$WORK/instance"
gel server init --non-interactive --data-dir "$WORK/data" --port 0 2>&1 | tail -n 2

echo "== migrations =="
gel migration apply --non-interactive

echo "== pipeline with mock Jev =="
chaosbox run fixtures/demo-repo --repo test-gel >"$WORK/graph.json"
test -s "$WORK/graph.json"

echo "== readiness =="
export CHAOSBOX_GEL_CREDENTIALS_FILE="$WORK/instance/credentials.json"
chaosbox db check --json --repo test-gel

echo "GEL INTEGRATION OK"
