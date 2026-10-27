#!/usr/bin/env bash
# Dedicated Freight database only. Run as the schema/role administrator after
# --migrate, with writers drained. Passwords are environment data, not SQL/argv.
set -euo pipefail

: "${PGPASSWORD:?Set the schema owner password}"
: "${FREIGHT_RUNTIME_DATABASE_PASSWORD:?Set the runtime password}"
if [[ "$PGPASSWORD" == "$FREIGHT_RUNTIME_DATABASE_PASSWORD" ]]; then
  echo "Schema owner and runtime passwords must differ." >&2
  exit 2
fi
case "$FREIGHT_RUNTIME_DATABASE_PASSWORD" in
  *$'\n'*|*$'\r'*)
    echo "Runtime password must not contain CR or LF." >&2
    exit 2
    ;;
esac

freight_role_sql="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/configure-runtime-role.sql"
# psql encrypts \password client-side. -f/-c leave stdin exclusively for the
# two prompts; --single-transaction rolls back role, grants and password as one.
printf '%s\n%s\n' "$FREIGHT_RUNTIME_DATABASE_PASSWORD" "$FREIGHT_RUNTIME_DATABASE_PASSWORD" |
  psql -X --no-password --quiet --set=ON_ERROR_STOP=1 --single-transaction \
    --file="$freight_role_sql" --command='\password freight_app'
echo "Freight runtime role and SCRAM password configured."
