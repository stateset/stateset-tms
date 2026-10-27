# StateSet Freight

A transportation management system with a Rust API, React web app, Ghostty-compatible terminal console and MCP adapter. A shared shipment record connects planning, carrier tenders, dispatch, tracking, delivery, invoice audit and settlement preparation. Typed boundaries use vendored StateSet iCommerce primitives; Freight owns its workflows and storage.

**Source-only developer alpha.** Interfaces and migrations can change. This snapshot contains source and synthetic tests, without private development history, qualification logs, screenshots, compiled dependencies or release binaries. See [alpha scope and limits](docs/alpha-status.md). Historical measurements in technical guides are background, not verification of this snapshot or production capacity.

## Try the terminal

Install Rust 1.90 or newer, then run:

```sh
./scripts/freight-tui --demo
```

The synthetic demo needs no database or credentials. In Ghostty and compatible terminals, use `1`–`5` to switch workspaces, `/` to search, `Ctrl+P` for commands and `?` for help. Billing, Payables and Queues are read-only. Press `v` on a shipment to open its synthetic evidence preview; Page Up/Down changes PDF pages. Direct Kitty graphics negotiates support automatically, and `--graphics kitty-tmux` enables the explicit single-tmux path. Character/text fallback remains available. See the [terminal guide](docs/terminal.md) for live connections, reviewed actions and encrypted save recovery.

## Run the API and web app

Install Rust 1.90+, Node.js 22 and npm:

```sh
cp .env.example .env
npm --prefix web ci
bash scripts/dev.sh
```

Open <http://127.0.0.1:3010>. The example configuration uses synthetic, in-memory data and a loopback API. Restarting resets the demo. Linux PDF previews require standard Poppler/util-linux tools and enforced Landlock support for the native worker. The Compose profiles use a separate unprivileged renderer with no API/database credentials or network access. Missing isolation returns an unavailable preview while original downloads remain accessible; see [renderer operation](docs/terminal.md#isolated-pdf-renderer-service). Review [access configuration](docs/access.md) before using real identities or data.

New to the API? [Getting started](docs/getting-started.md) takes one shipment from plan to paid with `curl` in about ten minutes.

## Explore and contribute

| Component | Source / guide |
| --- | --- |
| Freight API | `src/`, [API guide](docs/api.md) |
| Web app | `web/` |
| Terminal | `crates/freight-tui/`, [terminal guide](docs/terminal.md) |
| MCP adapter | `mcp/`, [setup](docs/api.md#tms-mcp-adapter) |
| iCommerce primitives | `crates/commerce-bridge/`, `vendor/stateset-primitives/`, `vendor/stateset-pricing/` |
| Database migrations | `migrations/`, [storage guide](docs/storage.md) |

The default workspace builds without sibling repositories. `integration-contracts/` and `runtime-contracts/` are optional harnesses with sibling dependencies and are outside the alpha checks. Some historical profiling scripts also require separately supplied inputs; omitted private reports cannot be reproduced from this snapshot.

Start with [CONTRIBUTING.md](CONTRIBUTING.md). Report vulnerabilities privately through [SECURITY.md](SECURITY.md).

## License

First-party code is available under **MIT OR Apache-2.0**, at your option: [LICENSE](LICENSE), [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE). Dependencies retain their own terms; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). Bundled binary and container distribution requires a separate review.
