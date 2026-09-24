#!/usr/bin/env bash
# test-typedb: disposable real-TypeDB integration gate for Chaosbox.
# - Uses a temp TypeDB server only; never touches developer databases.
# - Applies the packaged schema, runs the pipeline with explicit disposable
#   fixture decisions, and proves the db check/migrate contract matrix plus
#   consumer queries.
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

# Refuse a busy port. Readiness succeeding against an already-running
# server while this script's disposable instance failed to bind would
# silently migrate and test a database this script does not own.
if timeout 2 bash -c "exec 3<>/dev/tcp/127.0.0.1/$PORT" 2>/dev/null; then
  echo "refusing to run: 127.0.0.1:$PORT is already in use; set TYPEDB_PORT to a free port" >&2
  exit 1
fi

echo "== typedb-server: $(typedb-server --version 2>&1 | head -n 1) =="
echo "== typedb-console: $(typedb-console --version 2>&1 | head -n 1) =="

echo "== mock Jev HTTP service gate (no credentials, loopback only) =="
cargo test -p chaosbox-jev http_tests --offline

echo "== disposable server =="
# The Nix-packaged server ships no bundled config.yml (the systemd unit
# passes --config) and a partial config is rejected outright, so start
# from the example the package itself ships and rewrite it into this run's
# scratch space: the live developer instance's data and ports stay
# untouched, and HTTP/monitoring/reporting are turned off so nothing else
# can collide or phone home from a test run.
# Resolve through profile symlinks (/run/current-system/sw/bin) to reach
# the package's own share/ directory where the template lives.
BIN_DIR="$(dirname "$(readlink -f "$(command -v typedb-server)")")"
SOURCE_CONFIG="$BIN_DIR/../share/typedb/config.yml.example"
if [ ! -f "$SOURCE_CONFIG" ] && [ -f "$BIN_DIR/config.yml" ]; then
  SOURCE_CONFIG="$BIN_DIR/config.yml"
fi
if [ ! -f "$SOURCE_CONFIG" ]; then
  echo "no typedb config template beside $BIN_DIR (expected share/typedb/config.yml.example or config.yml)" >&2
  exit 1
fi
sed -E \
  -e "s|^(    listen-address:).*|\1 127.0.0.1:$PORT|" \
  -e "s|^(        listen-address:).*|\1 127.0.0.1:$((PORT + 1))|" \
  -e "s|^(        enabled:) true\$|\1 false|" \
  -e "s|^(    data-directory:).*|\1 \"$WORK/data\"|" \
  -e "s|^(    directory:).*|\1 \"$WORK/logs\"|" \
  -e "s|^(    metrics:) true\$|\1 false|" \
  -e "s|^(    errors:) true\$|\1 false|" \
  "$SOURCE_CONFIG" >"$WORK/config.yml"
# The rewrite must have landed: a template drift that left the default
# port would make this script bind (and migrate!) the live instance's
# server instead of its own, which is exactly the ownership bug this
# whole gate refuses to paper over.
if ! grep -q "^    listen-address: 127.0.0.1:$PORT\$" "$WORK/config.yml"; then
  echo "failed to point the disposable server at 127.0.0.1:$PORT; template drift in $SOURCE_CONFIG?" >&2
  exit 1
fi
printf 'password' >"$WORK/pw"
typedb-server --config "$WORK/config.yml" &
SERVER_PID=$!

echo "== readiness (bounded) =="
ready=""
for _ in $(seq 1 60); do
  # Prove the child is still alive: a disposable server that died on bind
  # or during startup must fail this gate, not let a probe succeed against
  # somebody else's instance on the same address.
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "typedb-server exited during startup; logs in $WORK/logs" >&2
    wait "$SERVER_PID" || true
    exit 1
  fi
  if typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
    --username admin --password password \
    --command "server version" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 2
done
if [ -z "$ready" ]; then
  echo "typedb-server on 127.0.0.1:$PORT never became ready within the bounded wait" >&2
  exit 1
fi
typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
  --username admin --password password \
  --command "server version" | head -n 3

export CHAOSBOX_DB_BACKEND=typedb
export CHAOSBOX_TYPEDB_ADDR="127.0.0.1:$PORT"
export CHAOSBOX_TYPEDB_USER=admin
export CHAOSBOX_TYPEDB_PASSWORD_FILE="$WORK/pw"
export CHAOSBOX_TYPEDB_DATABASE=test-typedb
# This script is the server-present gate: with a disposable server running,
# a live test that cannot connect, authenticates badly, fails to migrate or
# fails conformance must FAIL. It must never report those as a passing skip.
export CHAOSBOX_REQUIRE_TYPEDB=1

echo "== pending before migration =="
if chaosbox db check --json --repo test 2>/dev/null; then
  echo "check must be pending before migration" >&2
  exit 1
fi

echo "== migrations =="
chaosbox db migrate --json --repo test

echo "== pipeline with explicit disposable fixture decisions =="
chaosbox run fixtures/demo-repo --repo test --fixture-decisions >"$WORK/graph.json"
test -s "$WORK/graph.json"

echo "== readiness =="
chaosbox db check --json --repo test

echo "== live TypeDB conformance (required: no skip is allowed here) =="
cargo test -p chaosbox-typedb --test live

echo "== consumer queries =="
chaosbox query status --repo test
chaosbox query search main --repo test | head -c 400
echo

echo "TYPEDB INTEGRATION OK"
