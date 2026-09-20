# TypeDB local bootstrap for single-host pilots.
#
# Generates admin + application credentials once, converges both passwords
# over the local admin socket on every run, and ensures the application
# user + database while the well-known bootstrap default still works.
# Idempotent: safe to run on every boot. Secrets never touch argv; the
# application password travels only inside a root-only script file.
# Only the public bootstrap default ("password" unless overridden) ever
# appears in process arguments, and only while it is still active.
set -euo pipefail
umask 077

: "${TYPEDB_BOOTSTRAP_AUTH_DIR:?TYPEDB_BOOTSTRAP_AUTH_DIR must be set}"
: "${TYPEDB_BOOTSTRAP_SOCKET:?TYPEDB_BOOTSTRAP_SOCKET must be set}"
: "${TYPEDB_BOOTSTRAP_ADDR:?TYPEDB_BOOTSTRAP_ADDR must be set}"
: "${TYPEDB_BOOTSTRAP_ADMIN_BIN:?TYPEDB_BOOTSTRAP_ADMIN_BIN must be set}"
: "${TYPEDB_BOOTSTRAP_CONSOLE_BIN:?TYPEDB_BOOTSTRAP_CONSOLE_BIN must be set}"
: "${TYPEDB_BOOTSTRAP_APP_USER:?TYPEDB_BOOTSTRAP_APP_USER must be set}"
: "${TYPEDB_BOOTSTRAP_DATABASE:?TYPEDB_BOOTSTRAP_DATABASE must be set}"

auth_dir="$TYPEDB_BOOTSTRAP_AUTH_DIR"
admin_pw="$auth_dir/admin-password"
app_pw="$auth_dir/app-password"
app_owner="${TYPEDB_BOOTSTRAP_APP_OWNER:-}"
bootstrap_password="${TYPEDB_BOOTSTRAP_DEFAULT_PASSWORD:-password}"

generate_into() {
  tmp="$(mktemp -p "$auth_dir")"
  openssl rand -hex 32 > "$tmp"
  chmod 400 "$tmp"
  mv -f "$tmp" "$1"
}

mkdir -p "$auth_dir"
chmod 0711 "$auth_dir"
[ -s "$admin_pw" ] || generate_into "$admin_pw"
chmod 400 "$admin_pw"
[ -s "$app_pw" ] || generate_into "$app_pw"
chmod 400 "$app_pw"
if [ -n "$app_owner" ]; then
  chown "$app_owner" "$app_pw"
fi

[ -x "$TYPEDB_BOOTSTRAP_ADMIN_BIN" ] || {
  echo "typedb-bootstrap: missing admin tool" >&2
  exit 1
}
for _ in $(seq 1 60); do
  [ -S "$TYPEDB_BOOTSTRAP_SOCKET" ] && break
  sleep 2
done
[ -S "$TYPEDB_BOOTSTRAP_SOCKET" ] || {
  echo "typedb-bootstrap: no admin socket" >&2
  exit 1
}

console_default() {
  "$TYPEDB_BOOTSTRAP_CONSOLE_BIN" --address "$TYPEDB_BOOTSTRAP_ADDR" --tls-disabled \
    --username admin --password "$bootstrap_password" "$@"
}

if console_default --command "user list" >/dev/null 2>&1; then
  if ! console_default --command "user list" | grep -Fqw "$TYPEDB_BOOTSTRAP_APP_USER"; then
    tmp_script="$(mktemp -p "$auth_dir")"
    chmod 600 "$tmp_script"
    trap 'rm -f "$tmp_script"' EXIT
    printf 'user create %s %s' "$TYPEDB_BOOTSTRAP_APP_USER" "$(cat "$app_pw")" > "$tmp_script"
    console_default --script "$tmp_script"
    rm -f "$tmp_script"
    trap - EXIT
  fi
  if ! console_default --command "database list" | grep -Fqw "$TYPEDB_BOOTSTRAP_DATABASE"; then
    console_default --command "database create $TYPEDB_BOOTSTRAP_DATABASE"
  fi
fi

# Converge both passwords to the stored files. Fails closed when the
# application user is missing (deleted out of band): recreate it via an
# interactive console session with the stored admin password, or wipe the
# data directory for a full automatic reprovision.
"$TYPEDB_BOOTSTRAP_ADMIN_BIN" --socket-path "$TYPEDB_BOOTSTRAP_SOCKET" \
  --command "user reset-password admin" <"$admin_pw"
"$TYPEDB_BOOTSTRAP_ADMIN_BIN" --socket-path "$TYPEDB_BOOTSTRAP_SOCKET" \
  --command "user reset-password $TYPEDB_BOOTSTRAP_APP_USER" <"$app_pw"
