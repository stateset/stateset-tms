#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -f ../stateset-edi/crates/stateset-edi-x12/Cargo.toml ]]; then
  echo 'Check out stateset-edi next to stateset-freight to run the real codec qualification.' >&2
  exit 1
fi
CARGO_TARGET_DIR="$PWD/target" cargo test --locked --manifest-path integration-contracts/Cargo.toml
