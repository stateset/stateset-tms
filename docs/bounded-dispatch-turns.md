# Bounded outbound dispatch turns

A tenant cycle previously considered every eligible outbound job until it could transmit one. When canonical Sync sources were unavailable, it could perform an unbounded number of sequential source checks before returning its integration-worker slot. The existing four-source periodic refresh limit did not bound this separate dispatch phase.

The background worker now considers at most **four eligible outbound jobs per turn** and sends at most one. It retains a process-local bookmark containing the last considered original outbox ID. The next turn resumes after that ID in the original queue order, wraps once, and skips ineligible jobs. If the bookmark is absent, selection starts at the head. Changed eligibility does not shift an index into a filtered list. Only the selected four candidate payloads are cloned; the complete tenant snapshot and eligibility scans remain.

A source outage or source-review hold consumes a candidate position without claiming or modifying the original outbound request. Later turns can reach other shipments. Once the source recovers, the original job, payload and idempotency key can complete through the existing claim, lease and response checks. Per-shipment/destination ordering, carrier-release holds, multi-stop restrictions, source grants and financial evidence remain authoritative. Rotation cannot bypass an earlier pending/retry/sending/held job for the same shipment and destination.

Shutdown is checked before each candidate and inside the guarded claim transaction. If drain arrives during source verification, its source observation may finish, but no fresh handoff is claimed afterward. A handoff already claimed before drain still finishes its remote request and guarded outcome within the existing process deadline. Forced exit retains the existing uncertain lease behavior.

The bookmark is not persisted or shared across replicas. Restart starts selection at the head, and a panicking cycle retains its original scheduling fallback. The normal two-second cooldown still applies. Finite candidate turns may require more polls to process a backlog; this is not a latency, throughput or deployment-capacity claim. Four candidates bound source/handoff consideration counts, not whole-tenant SQL, decoding, eligibility-scan CPU, remote request duration or total process memory. The earlier periodic source refresh can still check up to four additional shipments in the same tenant cycle.

The existing public Rust `Integrations::dispatch_one` one-shot helper retains its previous exhaustive search behavior for compatibility. The process worker uses the bounded, cursor-bearing path. There is no new HTTP endpoint, environment setting, persistent format, schema migration or sibling protocol change.

## Verification

The [regressions](../src/dispatch_turn_tests.rs) create six actual imported/tendered shipments and two local tenders. Real loopback partners report canonical-source outages, capture exact outbound request bodies/keys, and hold individual requests. Checks cover four-candidate returns, cursor rotation to later healthy shipments, original unsent evidence, recovery with one original handoff each, changed eligibility, missing bookmarks, per-destination ordering, shutdown during source verification, and completion of a handoff already claimed before drain. PostgreSQL repeats original request recovery and checks persisted completions after reconnect.

```bash
RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integrations::dispatch_turn_tests
TEST_DATABASE_URL=postgres://user@localhost/disposable \
  RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integrations::dispatch_turn_tests -- --ignored --test-threads=1
```

Use an owned disposable PostgreSQL database. The fixture creates and removes its unique schema. The initial lint run required a named alias for the test wire-capture type; no application logic or assertions were relaxed.

## Historical source reconstruction

## Retained qualification

For reconstruction from the later catalog checkout, first reverse the [catalog storage boundary](load-catalog.md) with its verified core reversal patch, then follow this release's existing predecessor instructions. The reports below retain their original source and measurements.