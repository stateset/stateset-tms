#!/usr/bin/env bash
# Local release gate: full production-browser, database and actual-service
# checks. Passing this gate does not qualify deployment capacity or live partners.
set -euo pipefail
cd "$(dirname "$0")/.."
# Refuse to start on an overloaded shared host, and name overload when a later stage
# fails during it. Such a failure still exits nonzero (75); it is never a pass.
python3 scripts/host-load.py preflight
freight_host_load="$(mktemp "${TMPDIR:-/tmp}/freight-host-load-XXXXXX")"
python3 scripts/host-load.py monitor --output "$freight_host_load" &
freight_host_monitor=$!
freight_release_finish() {
  local status=$?
  kill "$freight_host_monitor" 2>/dev/null || true
  wait "$freight_host_monitor" 2>/dev/null || true
  python3 scripts/host-load.py classify --record "$freight_host_load" --status "$status" || status=$?
  rm -f "$freight_host_load"
  exit "$status"
}
trap freight_release_finish EXIT
# Validate actual cluster identities, all four databases and interface freshness
# before migrations, fixtures or destructive tests can touch any database.
python3 scripts/release_databases.py --mcp --fresh
if [[ "${FREIGHT_E2E_EXTERNAL:-false}" == "true" || -n "${FREIGHT_E2E_URL:-}" || -n "${FREIGHT_E2E_API_BINARY:-}" ]]; then
  echo "Release checks must run the current build and owned browser services; clear FREIGHT_E2E_EXTERNAL, FREIGHT_E2E_URL and FREIGHT_E2E_API_BINARY." >&2
  exit 2
fi
export FREIGHT_REQUIRE_DATABASE_TESTS=true
freight_release_security_dir="$(mktemp -d "${TMPDIR:-/tmp}/freight-release-security-XXXXXX")"
cargo fetch --locked
# Resolve both standalone qualification graphs before any interface fixtures
# start, so stale sibling locks fail early instead of after the browser matrix.
cargo metadata --locked --manifest-path integration-contracts/Cargo.toml --format-version 1 > /dev/null
cargo metadata --locked --manifest-path runtime-contracts/Cargo.toml --format-version 1 > /dev/null
python3 scripts/audit-production.py --package stateset-freight --output "$freight_release_security_dir/production-audit.json"
python3 scripts/audit-production.py --package stateset-freight-tui --output "$freight_release_security_dir/tui-production-audit.json"
python3 scripts/audit-production.py --manifest-path integration-contracts/Cargo.toml --package stateset-freight-contracts --output "$freight_release_security_dir/codec-audit.json"
python3 scripts/audit-production.py --manifest-path runtime-contracts/Cargo.toml --package stateset-freight-runtime-wire --output "$freight_release_security_dir/wire-audit.json"
npm --prefix web audit --omit=dev
npm --prefix mcp audit --omit=dev
npm --prefix mcp run check:catalog
cargo build --locked --release --bin stateset-freight
DATABASE_URL="$FREIGHT_MCP_TEST_DATABASE_URL" "$PWD/target/release/stateset-freight" --migrate
FREIGHT_MCP_TEST_API_BINARY="$PWD/target/release/stateset-freight" FREIGHT_MCP_TEST_MIGRATIONS=verify npm --prefix mcp test
export FREIGHT_E2E_API_BINARY="$PWD/target/release/stateset-freight"
bash scripts/check.sh
bash scripts/check-contracts.sh
bash scripts/check-runtime-contracts.sh
