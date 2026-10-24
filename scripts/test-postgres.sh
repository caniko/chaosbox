#!/usr/bin/env bash
# test-postgres: disposable real-PostgreSQL integration gate for Chaosbox.
# - Uses a temp cluster on a private unix socket only; never touches
#   developer databases. Needs initdb/pg_ctl/psql with pgvector (or
#   PG_BIN=<bin dir of such a build>).
# - Applies the packaged schema, runs the pipeline with explicit disposable
#   fixture decisions, proves the db check/migrate contract matrix plus
#   consumer queries, then runs every hermetic `live_pg_` test (each starts
#   its own cluster) and the compaction plugin against this binary.
set -euo pipefail

WORK="$(mktemp -d "${TMPDIR:-/tmp}/chaosbox-test-postgres.XXXXXX")"
PORT="${PGTEST_PORT:-55440}"
bin() { "${PG_BIN:+$PG_BIN/}$1" "${@:2}"; }
cleanup() {
  bin pg_ctl -D "$WORK/data" -m fast -w stop >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "== postgres: $(bin postgres --version) =="

echo "== mock Jev HTTP service gate (no credentials, loopback only) =="
cargo test -p chaosbox-jev http_tests --offline

echo "== disposable server =="
bin initdb -D "$WORK/data" -N --auth=trust --no-locale -E UTF8 -U chaosbox_test >/dev/null
bin pg_ctl -D "$WORK/data" -l "$WORK/server.log" -w start \
  -o "-k $WORK -p $PORT -c listen_addresses='' -c fsync=off" >/dev/null
bin psql -XAtq -h "$WORK" -p "$PORT" -U chaosbox_test -d postgres -c 'CREATE DATABASE test_postgres'

export CHAOSBOX_DB_BACKEND=postgres
export CHAOSBOX_DATABASE_URL="host=$WORK port=$PORT user=chaosbox_test dbname=test_postgres"

# Qualify the binary under test, never whichever chaosbox is on PATH: an
# installed release would answer for an older schema. The packaged runner
# sets this.
if [ -z "${CHAOSBOX_TEST_BIN:-}" ]; then
  cargo build --locked -p chaosbox --bin chaosbox
  CHAOSBOX_TEST_BIN="$(cargo metadata --locked --format-version 1 --no-deps |
    grep -o '"target_directory":"[^"]*"' | cut -d'"' -f4)/debug/chaosbox"
fi
test -x "$CHAOSBOX_TEST_BIN" || {
  echo "CHAOSBOX_TEST_BIN is not executable: $CHAOSBOX_TEST_BIN" >&2
  exit 1
}
export CHAOSBOX_TEST_BIN
echo "== binary under test: $CHAOSBOX_TEST_BIN =="
chaosbox() { "$CHAOSBOX_TEST_BIN" "$@"; }

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

echo "== hermetic live PostgreSQL tests (no skip is allowed here) =="
cargo test --workspace --offline -- --ignored live_pg_
node --test plugins/chaosbox-compaction/test/*.test.mjs

echo "== consumer queries =="
chaosbox query status --repo test
# Write first, then truncate: a closed pipe makes the CLI exit 101 on EPIPE.
chaosbox query search main --repo test >"$WORK/search.json"
head -c 400 "$WORK/search.json"
echo

echo "== zero-model structural publication and fresh-process refresh =="
mkdir "$WORK/syntax"
printf 'export function before() {}\nexport const value = 1;\n' >"$WORK/syntax/app.ts"
chaosbox run "$WORK/syntax" --repo syntax --no-decisions --max-candidates 0 >"$WORK/first.json" 2>"$WORK/first.log"
printf 'export function after() {}\nexport const value = 2;\n' >"$WORK/syntax/app.ts"
# The changed file now has an uncached co-occurrence candidate. Syntax-only
# publication must still refresh, even across a process restart.
chaosbox run "$WORK/syntax" --repo syntax --no-decisions --max-candidates 10 >"$WORK/second.json" 2>"$WORK/second.log"
# The run contract emits usage only for live inference. Both cold zero-model
# runs must report zero decisions instead.
grep -q 'decisions: accepted=0 rejected=0 abstained=0 negative=0 failed=0' "$WORK/first.log"
grep -q 'decisions: accepted=0 rejected=0 abstained=0 negative=0 failed=0' "$WORK/second.log"
chaosbox query status --repo syntax >"$WORK/status.json"
python3 - "$WORK" <<'PY'
import json
import pathlib
import sys
root = pathlib.Path(sys.argv[1])
first, second, status = [json.loads((root / p).read_text()) for p in ("first.json", "second.json", "status.json")]
assert len(first["links"]) == len(second["links"]) == 3
assert first["build_id"] != second["build_id"]
assert second["generation"] == first["generation"] + 1
assert {n["label"] for n in second["nodes"] if n["kind"] == "Definition"} == {"after", "value"}
assert status["coverage"]["structural_relations"] == 3
assert status["coverage"]["decision_relations"] == 0
assert status["coverage"]["files"][0]["status"] == "parsed"
PY
echo "== admitted reader pinning and revocation =="
python3 scripts/tests/read_view_smoke.py "$WORK"
# A decision-bearing active build still receives the exit-4 guard.
if chaosbox run "$WORK/syntax" --repo test --no-decisions --max-candidates 0 >"$WORK/deferred.json" 2>"$WORK/deferred.log"; then
  echo "decision-bearing active build was replaced without coverage" >&2
  exit 1
else
  test "$?" -eq 4
fi
grep -q '^coverage:' "$WORK/deferred.log"

echo "POSTGRES INTEGRATION OK"
