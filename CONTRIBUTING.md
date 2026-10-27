# Contributing

StateSet Freight is a developer alpha. Explain the operator problem, resulting behavior and validation in focused issues and pull requests. Use synthetic reproductions; do not submit customer records, credentials, private documents or backups.

## Local setup and checks

Use Rust 1.90+, Node.js 22 and Python 3.8+. The [README](README.md) has demo quickstarts. The normal workspace needs no sibling repositories.

```sh
npm --prefix web ci
npm --prefix mcp ci
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
npm --prefix web run test:unit
npm --prefix mcp run check:catalog
python3 scripts/check-open-source.py --source-alpha
python3 -m unittest discover -s scripts -p test_publication_tools.py -v
python3 -m unittest discover -s scripts -p test_source_alpha.py -v
python3 -m unittest discover -s scripts -p test_source_alpha_stage.py -v
python3 -m unittest discover -s scripts -p "test_tui_*.py" -v
```

After API guide changes, regenerate the MCP catalog and embedded guides with `node mcp/scripts/catalog.mjs`, then review their diff. For terminal changes:

```sh
cargo build --locked -p stateset-freight -p stateset-freight-tui
python3 scripts/check-tui-terminal.py target/debug/freight-tui
python3 scripts/check-tui-catalog-live.py target/debug/freight-tui
FREIGHT_TUI_TEST_API_BINARY="$PWD/target/debug/stateset-freight" cargo test --locked -p stateset-freight-tui --features api-process-tests --test api_process
FREIGHT_MCP_TEST_API_BINARY="$PWD/target/debug/stateset-freight" npm --prefix mcp test
```

Default Rust checks skip database-dependent tests. With `TEST_DATABASE_URL` pointing to a **disposable, isolated PostgreSQL database**, run `cargo test --locked --workspace -- --ignored --test-threads=1`. Tests create, alter and drop tables. Never use a production or shared database. PostgreSQL 16 is the intended full qualification target; it is outside the hosted community workflow.

`scripts/check.sh` also runs a web production build, browser tests and actual API/TUI checks. Install Playwright Chromium first. Set `FREIGHT_REQUIRE_DATABASE_TESTS=true` and an isolated `TEST_DATABASE_URL` to require database coverage. No checked-in historic report substitutes for a fresh result. See [alpha scope](docs/alpha-status.md).

## Review expectations

Keep authorization and tenant identity on the server. Preserve signed integer monetary amounts, original requests, version checks and idempotency. Review screens must not write. Uncertain writes must retain their exact original request. Add migrations rather than editing applied migrations.

The included community workflow uses ephemeral GitHub-hosted runners, read-only permissions and no repository secrets. Before enabling public contributions, maintainers must remove persistent runner access or enforce organization-level restrictions to trusted workflows; a PR can change workflow files. Configure branch protection and fork approval policy for the actual repository.

By contributing, you agree your contributions may be distributed under **MIT OR Apache-2.0**. Preserve third-party attribution and identify dependency terms. Report security issues through [SECURITY.md](SECURITY.md).

For a browser executable with a separate temporary-directory namespace, set `FREIGHT_E2E_DOWNLOADS_DIR` to an absolute private directory visible to both Playwright and the browser. Keep failed run artifacts separately; use Playwright's `--output` option for distinct qualification runs.
