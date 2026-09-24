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
# Shut the disposable server down and wait for it before removing its
# data: rm racing a still-flushing RocksDB fails the cleanup and would flip
# a green run to a non-zero exit after the verdict was already printed.
cleanup() {
  if [ -n "${SERVER_PID:-}" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 30); do
      kill -0 "$SERVER_PID" 2>/dev/null || break
      sleep 1
    done
    kill -9 "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
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
if timeout -k 1 2 bash -c "exec 3<>/dev/tcp/127.0.0.1/$PORT" 2>/dev/null; then
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
  -e "s|^(        metrics:) true\$|\1 false|" \
  -e "s|^(        errors:) true\$|\1 false|" \
  "$SOURCE_CONFIG" >"$WORK/config.yml"
# Every isolation-critical setting must have landed in the generated
# config. A template drift that left any default would hand this script a
# server bound to the wrong address, writing to the wrong directory, or
# phoning home — checking only the gRPC address once let the 3.13.0
# diagnostics reporting slip through enabled, so every setting is checked
# here and the first miss fails the gate. (The rewrite above would bind,
# and migrate, the live developer instance's server instead of our own,
# which is exactly the ownership bug this whole gate refuses to paper over.)
require_config() {
  if ! grep -Eq "$1" "$WORK/config.yml"; then
    echo "isolation failure: $2 (template drift in $SOURCE_CONFIG?)" >&2
    exit 1
  fi
}
require_config "^    listen-address: 127\\.0\\.0\\.1:$PORT\$" \
  "disposable server not pointed at 127.0.0.1:$PORT"
require_config "^        listen-address: 127\\.0\\.0\\.1:$((PORT + 1))\$" \
  "disposable HTTP listener not pointed at 127.0.0.1:$((PORT + 1))"
require_config "^    data-directory: \"$WORK/data\"\$" \
  "disposable data directory not pointed at $WORK/data"
require_config "^    directory: \"$WORK/logs\"\$" \
  "disposable log directory not pointed at $WORK/logs"
require_config "^[[:space:]]*metrics:[[:space:]]*false( |\$)" \
  "diagnostics metrics reporting not disabled"
require_config "^[[:space:]]*errors:[[:space:]]*false( |\$)" \
  "diagnostics error reporting not disabled"
if grep -Eq '^[[:space:]]*enabled:[[:space:]]*true( |$)' "$WORK/config.yml"; then
  echo "isolation failure: some subsystem left enabled (template drift in $SOURCE_CONFIG?)" >&2
  exit 1
fi
if grep -Eq '^[[:space:]]*(metrics|errors):[[:space:]]*true( |$)' "$WORK/config.yml"; then
  echo "isolation failure: diagnostics reporting left enabled (template drift in $SOURCE_CONFIG?)" >&2
  exit 1
fi
if grep -q '0\.0\.0\.0' "$WORK/config.yml"; then
  echo "isolation failure: wildcard bind address survived the rewrite (template drift in $SOURCE_CONFIG?)" >&2
  exit 1
fi
printf 'password' >"$WORK/pw"
typedb-server --config "$WORK/config.yml" &
SERVER_PID=$!

echo "== readiness (bounded) =="
ready=""
# Total wall-clock contract for readiness, including every probe and its
# KILL grace: no single hanging console may push acceptance past this.
READINESS_BUDGET=300
PROBE_TIMEOUT=10
KILL_AFTER=5
deadline=$((SECONDS + READINESS_BUDGET))
for _ in $(seq 1 60); do
  if ((SECONDS >= deadline)); then
    echo "typedb-server on 127.0.0.1:$PORT exceeded the ${READINESS_BUDGET}s readiness deadline" >&2
    exit 1
  fi
  remaining=$((deadline - SECONDS))
  probe_timeout=$PROBE_TIMEOUT
  kill_after=$KILL_AFTER
  # Cap this probe (TERM wait + KILL grace) to the time left, so a probe
  # started just before the deadline cannot be accepted after it. A bare
  # `timeout 10` sends TERM only: a console that catches or blocks TERM
  # would hang the gate past every attempt count, so the KILL grace is
  # part of the bound, not an addition to it.
  if ((probe_timeout + kill_after > remaining)); then
    if ((remaining < 2)); then
      echo "typedb-server on 127.0.0.1:$PORT exceeded the ${READINESS_BUDGET}s readiness deadline (no time left for a bounded probe)" >&2
      exit 1
    fi
    kill_after=1
    probe_timeout=$((remaining - kill_after))
  fi
  # Prove the child is still alive: a disposable server that died on bind
  # or during startup must fail this gate, not let a probe succeed against
  # somebody else's instance on the same address.
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "typedb-server exited during startup; logs in $WORK/logs" >&2
    wait "$SERVER_PID" || true
    exit 1
  fi
  if timeout -k "$kill_after" "$probe_timeout" typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
    --username admin --password password \
    --command "server version" >/dev/null 2>&1; then
    # A probe that finished after the deadline is late, not ready: accept
    # only inside the budget even if the console answered.
    if ((SECONDS > deadline)); then
      echo "typedb-server on 127.0.0.1:$PORT answered after the ${READINESS_BUDGET}s readiness deadline; refusing the late success" >&2
      exit 1
    fi
    # Recheck liveness after a successful probe: the child could have died
    # between the pre-probe check and the console's answer, in which case
    # the answer may have come from somewhere else.
    if kill -0 "$SERVER_PID" 2>/dev/null; then
      ready=1
      break
    fi
    echo "typedb-server died around a successful readiness probe; refusing to accept it" >&2
    wait "$SERVER_PID" || true
    exit 1
  fi
  sleep 2
done
if [ -z "$ready" ]; then
  echo "typedb-server on 127.0.0.1:$PORT never became ready within the bounded wait" >&2
  exit 1
fi
# Same bound as the loop probes: this print must not reintroduce an
# unbounded console wait after readiness was already proven.
timeout -k "$KILL_AFTER" "$PROBE_TIMEOUT" typedb-console --address "127.0.0.1:$PORT" --tls-disabled \
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

# Explicit shutdown before the verdict: the EXIT trap only covers abnormal
# exits from here on, and waiting for the server here (rather than in the
# trap after the verdict) keeps a slow shutdown from flipping a green run.
cleanup
SERVER_PID=""

echo "TYPEDB INTEGRATION OK"
