# Postgres URL <-> credential helpers shared by every script here that shells
# out to psql/pg_dump. A full postgres:// URL embeds its password; passing
# the whole URL as an argv element exposes that password to any other local
# user (via `ps`/`/proc/<pid>/cmdline`) for the life of the child process.
# These split the URL from its password so callers can pass the password via
# PGPASSWORD (the child's own environment, not its argv) instead. Mirrors
# scripts/ingest.py's _conn_env().

# ponytail: extracted value is used verbatim as PGPASSWORD, not URL-decoded
# -- a password containing a percent-escaped character (e.g. %40 for '@')
# would need to be typed into PGPASSWORD already-decoded. None of this
# repo's configured passwords use one; revisit if that changes.
pg_password() {
  local url="$1"
  if [[ "$url" =~ ^postgres(ql)?://[^:/@[:space:]]+:([^@[:space:]]*)@ ]]; then
    printf '%s' "${BASH_REMATCH[2]}"
  fi
}

pg_url_strip_password() {
  local url="$1"
  if [[ "$url" =~ ^(postgres(ql)?://)([^:/@[:space:]]+):[^@[:space:]]*@(.*)$ ]]; then
    printf '%s%s@%s' "${BASH_REMATCH[1]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[4]}"
  else
    printf '%s' "$url"
  fi
}
