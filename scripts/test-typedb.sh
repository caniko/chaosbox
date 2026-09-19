#!/usr/bin/env bash
# test-typedb: disposable real-TypeDB integration gate for Chaosbox.
# - Uses a temp TypeDB server only; never touches developer databases.
# - Applies the packaged schema, runs the mock-Jev pipeline, and proves the
#   db check/migrate contract matrix plus consumer queries.
# - Deterministic cleanup via trap. Exits 3 with PENDING when the TypeDB
#   binaries are unavailable (honest pending, not a passing placeholder).
set -euo pipefail

WORK="$(mktemp -d "${TMPDIR:-/tmp}/chaosbox-test-typedb.XXXXXX")"
cleanup() {
  if [ -n "${SERVER_PID:-}" ]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

if ! command -v typedb-server >/dev/null 2>&1 || ! command -v typedb-console >/dev/null 2>&1; then
  echo "PENDING: no 'typedb-server'/'typedb-console' available; enter the live shell (nix develop .#typedb) which provides the temporary packages" >&2
  exit 3
fi

PORT="${TYPEDB_PORT:-1729}"
echo "== typedb-server: $(typedb-server --version 2>&1 | head -n 1) =="
echo "== typedb-console: $(typedb-console --version 2>&1 | head -n 1) =="

echo "== mock Jev HTTP service gate (no credentials, loopback only) =="
cargo test -p chaosbox-jev http_tests --offline

echo "== disposable server =="
printf 'password' >"$WORK/pw"
typedb-server --storage.data-directory "$WORK/data" \
  --server.listen-address "127.0.0.1:$PORT" \
  --logging.directory "$WORK/logs" &
SERVER_PID=$!

echo "== readiness (bounded) =="
for _ in $(seq 1 60); do
  if typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
    --username admin --password password \
    --command "server version" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
  --username admin --password password \
  --command "server version" | head -n 3

export CHAOSBOX_DB_BACKEND=typedb
export CHAOSBOX_TYPEDB_ADDR="127.0.0.1:$PORT"
export CHAOSBOX_TYPEDB_USER=admin
export CHAOSBOX_TYPEDB_PASSWORD_FILE="$WORK/pw"
export CHAOSBOX_TYPEDB_DATABASE=test-typedb

echo "== pending before migration =="
if chaosbox db check --json --repo test 2>/dev/null; then
  echo "check must be pending before migration" >&2
  exit 1
fi

echo "== migrations =="
chaosbox db migrate --json --repo test

echo "== pipeline with mock Jev =="
chaosbox run fixtures/demo-repo --repo test >"$WORK/graph.json"
test -s "$WORK/graph.json"

echo "== readiness =="
chaosbox db check --json --repo test

echo "== consumer queries =="
chaosbox query status --repo test
chaosbox query search main --repo test | head -c 400
echo

echo "TYPEDB INTEGRATION OK"
