#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f .env ]]; then set -a; source .env; set +a; fi
export FREIGHT_DEMO="${FREIGHT_DEMO:-true}"
if [[ "$(node -p 'Number(process.versions.node.split(".")[0])')" -lt 22 ]]; then
  echo 'Node.js 22 or newer is required. Activate it before running this script.' >&2
  exit 1
fi
cargo build --locked
if [[ ! -d web/node_modules ]]; then npm --prefix web ci; fi
./target/debug/stateset-freight &
freight_api_pid=$!
cleanup() { kill "$freight_api_pid" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
for attempt in {1..50}; do
  if curl --fail --silent "http://${FREIGHT_BIND_ADDR:-127.0.0.1:8090}/health" >/dev/null; then break; fi
  if ! kill -0 "$freight_api_pid" 2>/dev/null; then echo 'Freight API failed to start.' >&2; exit 1; fi
  sleep 0.2
done
npm --prefix web run dev
