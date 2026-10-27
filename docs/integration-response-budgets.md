# Integration response budgets

Freight now reads every EDI/Sync response through the same 2,000,000-byte budget: order listings, canonical order snapshots, canonical EDI document details, generated outbound X12, and tracking acknowledgements. Previously only canonical order snapshots counted streamed bytes. Outbound handoffs checked `Content-Length` but then read an unbounded body; listings and document details had no byte budget.

An oversized declared length is rejected before reading the body. Every received chunk is also checked before appending, including responses with chunked transfer encoding or no length header. Exactly 2,000,000 bytes are allowed. Oversized responses fail without waiting for the partner to finish its body. Non-success HTTP statuses are classified before reading their bodies, so a stalled error body cannot obscure the actual status.

JSON is decoded from the bounded original bytes. Generated EDI acknowledgements must be valid UTF-8 and retain their exact text; malformed bytes are rejected instead of being replaced during text decoding. The existing X12 prefix and Sync `data.ok=true` checks remain. These checks do not certify a complete X12 interchange or carrier transport: the EDI gateway owns validation, envelope/control numbers, partner delivery and acknowledgement reconciliation.

Oversized or unreadable bodies use the existing durable failure path. An outbox attempt remains uncertain/attempted and retryable, retaining its original target, ID/idempotency key and payload; it must not become gateway-accepted or synced. An inbound fetch failure retains the original callback, delivery identities and attempt history without applying partial business changes. A later valid response can complete the retained work. Sync's tracking endpoint may retain another tracking event if it does not deduplicate the key; this change does not add remote exactly-once execution.

The existing complete-request timeouts still apply: five seconds for canonical order verification and twenty seconds for other connector calls, including body consumption. This byte budget bounds accepted response bytes, not total process memory, decoded JSON allocations, aggregate workspace size, transaction duration, tenant fairness or deployment capacity. No persistent format, migration, endpoint, credential scope or sibling implementation changes.

## Reproduce the transport checks

The [tests](../src/integration_response_tests.rs) use owned loopback TCP partners with independently constructed HTTP framing. They cover fixed-length, chunked and connection-close bodies; exact-limit acceptance; declared and streamed excess with the response deliberately unfinished; truncated bodies; invalid bytes; prompt status classification; exact original X12 text; connector credentials/paths; original-key/payload recovery for EDI and Sync; and retained inbound evidence with successful retry. The PostgreSQL scenario repeats both outbound recoveries and the inbound recovery in a disposable schema, then reconnects and reads the original retained work.

```bash
RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integrations::response_tests
TEST_DATABASE_URL=postgres://user@localhost/disposable \
  RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integrations::response_tests -- --ignored
```

Use only an owned disposable database for the ignored scenario. Each fixture creates and removes a uniquely named schema.

## Retained qualification

## Historical source reconstruction

For the subsequent tenant-worker release, first apply the [tenant-worker reversal](tenant-integration-workers.md) in an isolated copy, then follow this page’s existing reconstruction. Recorded report and original patch bytes remain unchanged.

For the subsequent process-drain release, first apply the [process-drain reversal](process-drain.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.

For the subsequent bounded-dispatch release, first apply the [bounded-dispatch reversal](bounded-dispatch-turns.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.
