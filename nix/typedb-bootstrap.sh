# TypeDB local bootstrap for single-host pilots.
#
# Generates admin + application credentials once, ensures the application
# user + database over the loopback console, and converges both passwords
# to the stored values on every run. Idempotent: safe to run on every boot.
#
# Auth model: the well-known bootstrap default ("password" unless
# overridden) authenticates only until the first successful rotation;
# afterwards only the stored admin password does. When neither works the
# script fails closed: recover the admin password via console and re-run.
#
# Secrecy: no secret ever appears in argv. Console authentication reads
# the password from stdin under a pty (the pinned console has no
# password-file option and panics without a terminal), and secret-bearing
# commands run from a root-only (0600) script file. Console output is
# captured privately and discarded; failures report only the operation,
# never the transcript, so generated values never reach logs or the
# journal even when the server echoes commands back on error.
#
# Trust: loopback address only (TLS is disabled), constrained names, a
# caller-owned non-symlink state directory, and an exclusive lock, so two
# concurrent runs cannot generate and rotate different credentials. Both
# rotated credentials are verified by re-authenticating, so a silent
# non-application fails loudly instead of reporting success.
#
# Consumption: run as root after typedb.service starts, then point the
# deployment at the generated application credential, e.g.
#   services.chaosbox.passwordFile = "/var/lib/typedb-auth/app-password";
# ordering: typedb.service -> typedb-bootstrap -> db migrate -> application.
# Verified against TypeDB CE 3.13.0 console grammar
# (user create/list, user update-password, database create/list).
set -euo pipefail
umask 077

: "${TYPEDB_BOOTSTRAP_AUTH_DIR:?TYPEDB_BOOTSTRAP_AUTH_DIR must be set}"
: "${TYPEDB_BOOTSTRAP_ADDR:?TYPEDB_BOOTSTRAP_ADDR must be set}"
: "${TYPEDB_BOOTSTRAP_CONSOLE_BIN:?TYPEDB_BOOTSTRAP_CONSOLE_BIN must be set}"
: "${TYPEDB_BOOTSTRAP_APP_USER:?TYPEDB_BOOTSTRAP_APP_USER must be set}"
: "${TYPEDB_BOOTSTRAP_DATABASE:?TYPEDB_BOOTSTRAP_DATABASE must be set}"

fail() {
  # stdout, not stderr: the VM test driver captures stdout only, and
  # systemd journals both identically. Messages carry no secrets.
  echo "typedb-bootstrap: $*"
  exit 1
}

# Loopback only: the console session below disables TLS. Only IPv4
# loopback forms are accepted (no bracketed IPv6 parsing).
case "$TYPEDB_BOOTSTRAP_ADDR" in
  127.0.0.1:* | localhost:*) ;;
  *) fail "non-loopback address '$TYPEDB_BOOTSTRAP_ADDR' refused (TLS is disabled)" ;;
esac
host="${TYPEDB_BOOTSTRAP_ADDR%:*}"
port="${TYPEDB_BOOTSTRAP_ADDR##*:}"
case "$port" in
  '' | *[!0-9]*) fail "invalid port in '$TYPEDB_BOOTSTRAP_ADDR'" ;;
esac

# Bare console tokens: anything else could break command parsing.
case "$TYPEDB_BOOTSTRAP_APP_USER" in
  '' | *[!a-zA-Z0-9_-]*) fail "invalid application user '$TYPEDB_BOOTSTRAP_APP_USER'" ;;
esac
case "$TYPEDB_BOOTSTRAP_DATABASE" in
  '' | *[!a-zA-Z0-9_-]*) fail "invalid database '$TYPEDB_BOOTSTRAP_DATABASE'" ;;
esac
[ "$TYPEDB_BOOTSTRAP_APP_USER" != "admin" ] || fail "application user must not be 'admin'"

auth_dir="$TYPEDB_BOOTSTRAP_AUTH_DIR"
admin_pw="$auth_dir/admin-password"
app_pw="$auth_dir/app-password"
app_owner="${TYPEDB_BOOTSTRAP_APP_OWNER:-}"
bootstrap_password="${TYPEDB_BOOTSTRAP_DEFAULT_PASSWORD:-password}"

[ -x "$TYPEDB_BOOTSTRAP_CONSOLE_BIN" ] || fail "missing console tool"

mkdir -p "$auth_dir"
# Validate before mutating: a pre-existing directory must already be a
# caller-owned non-symlink before permissions are touched.
[ ! -L "$auth_dir" ] || fail "state directory must not be a symlink"
[ -d "$auth_dir" ] || fail "state path is not a directory"
[ "$(stat -c %u "$auth_dir")" -eq "$(id -u)" ] || fail "state directory not owned by caller"
chmod 0711 "$auth_dir"

my_uid="$(id -u)"

# Pre-existing state must be caller-owned regular files, never symlinks
# (dangling links included: the symlink check runs before existence).
# The application file may additionally be owned by its operator owner.
check_state_file() {
  local file="$1" owner_ok="$2" uid
  [ ! -L "$file" ] || fail "$file must not be a symlink"
  [ -e "$file" ] || return 0
  [ -f "$file" ] || fail "$file is not a regular file"
  uid="$(stat -c %u "$file")"
  case " $owner_ok " in
    *" $uid "*) ;;
    *) fail "$file has unexpected owner" ;;
  esac
}

# Serialize provisioning: concurrent fresh runs must not generate and
# rotate different credentials. Children never inherit the lock fd.
# A pre-existing lock is validated like any other state file first.
check_state_file "$auth_dir/.bootstrap.lock" "$my_uid"
exec 200>"$auth_dir/.bootstrap.lock"
[ -f "/dev/fd/200" ] || fail "cannot open state lock"
flock -w 120 200 || fail "cannot acquire state lock"

tmp_files=()
cleanup() {
  if [ "${#tmp_files[@]}" -gt 0 ]; then
    rm -f "${tmp_files[@]}"
  fi
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT TERM

# Appends to NEW_TMP (return via stdout would need command substitution,
# whose subshell would lose the tmp_files bookkeeping for the EXIT trap).
NEW_TMP=""
new_tmp() {
  NEW_TMP="$(mktemp -p "$auth_dir")"
  chmod 600 "$NEW_TMP"
  tmp_files+=("$NEW_TMP")
}

generate_into() {
  new_tmp
  local tmp="$NEW_TMP"
  if ! openssl rand -hex 32 >"$tmp"; then
    fail "credential generation failed"
  fi
  chmod 400 "$tmp"
  mv -f "$tmp" "$1"
}

check_state_file "$admin_pw" "$my_uid"
owner_ok="$my_uid"
if [ -n "$app_owner" ]; then
  owner_ok="$owner_ok $(id -u "$app_owner" 2>/dev/null || echo "?")"
fi
check_state_file "$app_pw" "$owner_ok"

[ -s "$admin_pw" ] || generate_into "$admin_pw"
chmod 400 "$admin_pw"
[ -s "$app_pw" ] || generate_into "$app_pw"
chmod 400 "$app_pw"
if [ -n "$app_owner" ]; then
  chown "$app_owner" "$app_pw"
fi

# Snapshot both credentials once and validate the generated format.
# Everything below uses these snapshots, never the files again, so an
# operator-owned credential file cannot change mid-run and newlines in
# file content can never reach console command construction.
admin_pw_content="$(cat "$admin_pw")"
app_pw_content="$(cat "$app_pw")"
case "$admin_pw_content" in
  *[!0-9a-f]* | "") fail "admin credential has invalid format" ;;
esac
[ "${#admin_pw_content}" -eq 64 ] || fail "admin credential has invalid format"
case "$app_pw_content" in
  *[!0-9a-f]* | "") fail "application credential has invalid format" ;;
esac
[ "${#app_pw_content}" -eq 64 ] || fail "application credential has invalid format"

# Wait for the server port (bounded) so a cold start never looks like bad
# credentials. Host/port are validated above; passed positionally, never
# interpolated into evaluated code.
for _ in $(seq 1 60); do
  # shellcheck disable=SC2016 # $1/$2 are intentional positional params
  if timeout 1 bash -c 'exec 3<>/dev/tcp/"$1"/"$2"' probe "$host" "$port" 2>/dev/null; then
    break
  fi
  sleep 2
done
# shellcheck disable=SC2016 # $1/$2 are intentional positional params
timeout 1 bash -c 'exec 3<>/dev/tcp/"$1"/"$2"' probe "$host" "$port" 2>/dev/null \
  || fail "server unreachable at $TYPEDB_BOOTSTRAP_ADDR"

# Run the console with the password on stdin under a pty: the pinned
# console offers no password-file option and its hidden-input prompt
# panics without a terminal, while --password would expose the secret in
# argv. Passwords arrive with exactly one trailing newline; anything else
# misbehaves at the console prompt. Output lands in $CONSOLE_OUT (a
# private temp file, cleaned at exit) and is discarded; failures report
# only the operation, never the transcript.
# Single-quote one token for sh: script(1) -c splits on spaces but honors
# quotes, not backslash escapes (verified against util-linux script).
sh_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

# console_with_pw takes the password VALUE (a validated snapshot held in
# a shell variable; printf below is a builtin, so values never appear in
# argv). Callers pass snapshots only, so credential files are read
# exactly once, with exactly one trailing newline as the console prompt
# requires.
CONSOLE_OUT=""
CONSOLE_USER="admin"
console_with_pw() {
  local pw_value="$1" label="$2"
  shift 2
  local cmd token
  cmd=""
  for token in "$TYPEDB_BOOTSTRAP_CONSOLE_BIN" \
    --address "$TYPEDB_BOOTSTRAP_ADDR" --tls-disabled --username "$CONSOLE_USER" "$@"; do
    cmd="$cmd $(sh_quote "$token")"
  done
  new_tmp
  CONSOLE_OUT="$NEW_TMP"
  # Pin SHELL: script(1) runs -c through $SHELL, and a non-POSIX login
  # shell (e.g. nu) parses the quoted command differently. 200>&- keeps
  # the state lock out of child processes so a stray child can never
  # block the next run. stdin arrives via pipe: with process substitution
  # script(1) always exits 0, swallowing console failures. Each console
  # call is bounded (45s each: a stuck console must never hang the boot);
  # transient failures are retried because every mutation below is
  # idempotent or guarded, and credential resolution treats persistent
  # failure as rejection only after retries. Four attempts fit the service
  # start timeout with margin, and the first failing operation aborts.
  for _ in $(seq 1 4); do
    if printf '%s\n' "$pw_value" | SHELL="${BASH:-/bin/sh}" timeout 45 script -qec "$cmd" /dev/null 200>&- >"$CONSOLE_OUT" 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "typedb-bootstrap: $label failed after 4 attempts" >&2
  return 1
}

console_as() {
  console_with_pw "$1" "$2" "${@:3}"
}

# Resolve the working admin credential: stored first, bootstrap default
# while the install is still fresh. Anything else fails closed. The
# default is a well-known value, not a generated secret, and needs no
# snapshot file.
admin_pw_current=""
if console_as "$admin_pw_content" "admin authentication" --command "server version" 2>/dev/null; then
  admin_pw_current="$admin_pw_content"
elif console_as "$bootstrap_password" "admin authentication" --command "server version" 2>/dev/null; then
  admin_pw_current="$bootstrap_password"
else
  fail "no working admin credential (stored and bootstrap default both rejected); recover the admin password via console and re-run"
fi

# Secret-bearing commands run from a caller-only script file, never argv.
run_secret_script() {
  local script="$1" label="$2"
  new_tmp
  local tmp_script="$NEW_TMP"
  printf '%s\n' "$script" >"$tmp_script"
  console_with_pw "$admin_pw_current" "$label" --script "$tmp_script"
}

# Exact user match: console echoes commands as "+ ..." lines, and prefix
# matching (grep -w) would confuse "app" with "app-old". Output comes
# through a pty, so carriage returns are stripped before matching.
user_exists() {
  console_with_pw "$admin_pw_current" "user listing" --command "user list" 2>/dev/null \
    || return 1
  tr -d '\r' <"$CONSOLE_OUT" | grep -v '^+ ' | grep -Fxq "$1"
}

if ! user_exists "$TYPEDB_BOOTSTRAP_APP_USER"; then
  # A retry that lost its response may have created the user already.
  run_secret_script "user create $TYPEDB_BOOTSTRAP_APP_USER $app_pw_content" "application user creation" \
    || user_exists "$TYPEDB_BOOTSTRAP_APP_USER"
fi

# database create is idempotent: re-creating an existing database succeeds.
console_with_pw "$admin_pw_current" "database ensure" \
  --command "database create $TYPEDB_BOOTSTRAP_DATABASE"

# Converge both passwords to the stored files, application first: rotating
# the admin identity ends the console session. Each rotation is verified
# by re-authenticating with the stored credential afterwards.
run_secret_script "user update-password $TYPEDB_BOOTSTRAP_APP_USER $app_pw_content" \
  "application password converge"
CONSOLE_USER="$TYPEDB_BOOTSTRAP_APP_USER"
console_with_pw "$app_pw_content" "application credential verify" \
  --command "server version" \
  || fail "application credential did not take effect; re-run to converge again"
CONSOLE_USER="admin"
run_secret_script "user update-password admin $admin_pw_content" \
  "admin password converge"
console_with_pw "$admin_pw_content" "admin credential verify" \
  --command "server version" \
  || fail "admin credential did not take effect; recover the admin password via console and re-run"
