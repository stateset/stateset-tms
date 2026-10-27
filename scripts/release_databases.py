"""Read-only database preflight for destructive qualification suites.

Connection aliases are not database identities. Use PostgreSQL's cluster ID and
database name, so localhost/IPv4/IPv6/socket aliases cannot bypass isolation.
Never print connection strings or database-client diagnostics containing them.
"""
import argparse
import json
import os
import subprocess
from urllib.parse import parse_qsl, unquote, urlencode, urlsplit, urlunsplit


DATABASES = ("TEST_DATABASE_URL", "FREIGHT_E2E_DATABASE_URL",
             "FREIGHT_TUI_TEST_DATABASE_URL")
MCP_DATABASE = "FREIGHT_MCP_TEST_DATABASE_URL"
QUERY = """SELECT json_build_object(
    'version', current_setting('server_version_num')::int,
    'cluster', system_identifier::text,
    'database', current_database(),
    'populated', EXISTS (
        SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND left(c.relname, 8)='freight_'
    )) FROM pg_control_system()"""


def inspect(name, url):
    try:
        parsed = urlsplit(url)
        if parsed.scheme not in ("postgres", "postgresql") or any(c in url for c in "\r\n\0"):
            raise ValueError("PostgreSQL URL required")
        userinfo, separator, host = parsed.netloc.rpartition("@")
        password = os.environ.get("PGPASSWORD", "")
        if separator:
            user, has_password, encoded_password = userinfo.partition(":")
            if has_password:
                password = unquote(encoded_password)
            netloc = user + "@" + host
        else:
            netloc = parsed.netloc
        query = []
        for key, value in parse_qsl(parsed.query, keep_blank_values=True):
            if key == "password":
                password = value
            else:
                query.append((key, value))
        public_url = urlunsplit((parsed.scheme, netloc, parsed.path, urlencode(query), ""))
        result = subprocess.run(
            ["psql", "--dbname", public_url, "-X", "-qAt", "-w", "-v", "ON_ERROR_STOP=1", "-c", QUERY],
            env={**os.environ, "PGPASSWORD": password, "PGCONNECT_TIMEOUT": "5"},
            text=True, capture_output=True, timeout=15, check=True)
        value = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError):
        raise ValueError(f"Cannot verify {name}; check connectivity and read access to pg_control_system().") from None
    if not isinstance(value, dict) or set(value) != {"version", "cluster", "database", "populated"}:
        raise ValueError(f"Invalid database identity for {name}.")
    if type(value["version"]) is not int or not 160000 <= value["version"] < 170000:
        raise ValueError(f"{name} must use PostgreSQL 16.")
    if (not isinstance(value["cluster"], str) or not value["cluster"].isdigit()
            or not isinstance(value["database"], str) or not value["database"]
            or type(value["populated"]) is not bool):
        raise ValueError(f"Invalid database identity for {name}.")
    return value


def check(environ=None, *, mcp=False, fresh=False, inspector=inspect):
    environ = os.environ if environ is None else environ
    names = DATABASES + ((MCP_DATABASE,) if mcp else ())
    # Reject incomplete configuration before contacting any server.
    for name in names:
        if not environ.get(name, "").strip():
            raise ValueError(f"{name} must identify a separate disposable PostgreSQL 16 database.")
    seen = {}
    for name in names:
        value = inspector(name, environ[name])
        identity = (value["cluster"], value["database"])
        if identity in seen:
            raise ValueError(f"{name} and {seen[identity]} identify the same database; use separate disposable databases.")
        seen[identity] = name
        if fresh and name != "TEST_DATABASE_URL" and value["populated"]:
            raise ValueError(f"{name} contains Freight relations; retain it and supply a fresh database.")
    return len(names)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mcp", action="store_true", help="Also require an isolated MCP database")
    parser.add_argument("--fresh", action="store_true", help="Require fresh interface databases before their first start")
    args = parser.parse_args()
    try:
        count = check(mcp=args.mcp, fresh=args.fresh)
    except ValueError as error:
        parser.exit(2, str(error) + "\n")
    print(f"Verified {count} separate PostgreSQL 16 qualification databases (read-only preflight).")


if __name__ == "__main__":
    main()
