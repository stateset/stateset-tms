#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "${FREIGHT_REQUIRE_DATABASE_TESTS:-false}" == "true" ]]; then
  python3 scripts/release_databases.py
fi
cargo fmt --all -- --check
python3 -m unittest discover -s scripts -p 'test_*.py' -v
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --workspace --locked
cargo build --locked -p stateset-freight-tui
python3 scripts/check-tui-terminal.py target/debug/freight-tui
python3 scripts/check-tui-graphics.py target/debug/freight-tui
python3 scripts/check-tui-catalog-live.py target/debug/freight-tui
if [[ -n "${TEST_DATABASE_URL:-}" ]]; then
  # The manual response profiler needs its own owned financial fixture.
  cargo test --workspace --locked -- --ignored --test-threads=1 --skip profile_complete_financial_response_without_changing_its_content
fi
npm --prefix web run test:unit
npm --prefix web run build
freight_gateway_evidence_dir="$(mktemp -d "${TMPDIR:-/tmp}/freight-gateway-check-XXXXXX")"
node web/scripts/qualify-gateway-redirects.mjs "$freight_gateway_evidence_dir/result.json"
FREIGHT_ACCESS_TEST_PRODUCTION=true node --test web/tests/access-response-budget.test.mjs
cargo build --locked
target/debug/stateset-freight --pdf-worker --probe-policy
python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight
python3 scripts/check-tui-graphics.py target/debug/freight-tui --api-binary target/debug/stateset-freight --pdf
freight_preview_image_args=()
if [[ -n "${FREIGHT_PREVIEW_IMAGE_OUTPUT:-}" ]]; then
  freight_preview_image_args+=(--output "$FREIGHT_PREVIEW_IMAGE_OUTPUT")
fi
python3 scripts/check-document-preview-image.py "${freight_preview_image_args[@]}"
FREIGHT_TUI_TEST_API_BINARY="$PWD/target/debug/stateset-freight" cargo test --locked -p stateset-freight-tui --features api-process-tests --test api_process
cargo build --locked --example invoice_review_fixture
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e
