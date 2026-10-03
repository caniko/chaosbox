#!/usr/bin/env bash
# Run the authenticated native regression after the disposable server is ready.
set -euo pipefail

log="${1:?usage: $0 LOG_FILE}"
cargo test --locked --color never -p chaosbox --test federation_typedb -- --ignored --exact federation_typedb_is_read_only_scoped_and_snapshot_bound --color never 2>&1 | tee "$log"
if ! grep -Eq '^test result: ok\. 1 passed; 0 failed; 0 ignored;' "$log"; then
  echo "mandatory TypeDB federation gate must execute exactly one passing test with no failures or skips" >&2
  exit 1
fi
