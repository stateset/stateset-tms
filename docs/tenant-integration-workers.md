# Tenant integration workers

Previously one loop awaited each tenant's inbound EDI fetch, scheduled Sync source checks and outbound handoff before visiting the next tenant. A held partner response delayed unrelated customers. Tender-expiry checks ran at the end of that same loop, so external network waits also delayed overdue operating work.

The worker now schedules bounded tenant cycles. `FREIGHT_INTEGRATION_CONCURRENCY` defaults to **2**, with an integer range of **1–16**. Empty, signed, whitespace-padded, fractional, zero and out-of-range values reject startup before opening the store. Configure the variable in the process environment or Compose. It is a per-process budget for background cycles; operator-initiated API calls retain their own existing HTTP/read limits. Account for every replica and the existing database/read budgets.

Each configured tenant has one queue entry or one running cycle. Only running cycles become Tokio tasks. Completing a cycle returns the tenant to the back of the queue, eligible after two seconds. The scheduler skips entries still in their cooldown. A tenant waiting on a partner cannot start overlapping cycles, and completing fast tenants can rotate while another tenant remains held. If all slots are occupied, other tenants wait; this does not reserve a fast lane or establish deployment capacity or a latency objective.

A cycle retains the existing order: one inbound claim, scheduled source verification, then outbound dispatch. Source checks inspect at most four active imported shipments, retain the per-tenant rotating cursor, and become eligible thirty seconds after the previous check completes. They run in a subsequent cycle, without accumulating missed ticks. The source-selection workspace is dropped before remote verification. Active dispatches still use whole-tenant snapshots and guarded mutations; this does not normalize storage or bound application CPU.

Claims, leases, original request keys, callback identities, source grants, per-shipment/destination ordering, carrier coordination and financial checks retain their existing transaction rules. The scheduler is local to one process; competing replicas still rely on those database claims. Interrupted cycles do not clear leases or declare a transmission failed. A panicking child returns its tenant to the queue after the normal delay; persisted lease and receipt checks govern any subsequent work.

Tender expiry has an independent schedule, initially and then every thirty seconds, skipping missed ticks. It enumerates all stored tenants, including tenants without configured integrations. It records the existing overdue exception without releasing a carrier or creating another tender. Database contention, read admission and CPU can still delay that sweep; remote calls in the integration loop no longer own its scheduling.

Dropping the worker cancels both the scheduling and expiry futures and aborts its tenant tasks through their owned `JoinSet`. In-flight durable leases retain their ordinary crash-recovery behavior. Existing admitted store reads may finish their already-owned SQL/decode work after a caller is cancelled. There is no new persisted schema, format, migration or partner mapping.

## Verification

[Focused tests](../src/integration_worker_tests.rs) hold actual loopback HTTP responses while checking progress by two ready tenants, one active cycle per tenant, the global claim bound, rotation of waiting tenants, cancellation without detached completion, overdue work without a configured connector, panic recovery with backoff, and strict configuration. The PostgreSQL scenario repeats three-tenant progress and original completion checks across a reconnect. Tests retain the existing internal workflow races; independent database fixtures run serially to avoid contending on the shared migration-startup lock.

```bash
RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integration_worker::tests
TEST_DATABASE_URL=postgres://user@localhost/disposable \
  RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib integration_worker::tests -- --ignored --test-threads=1
```

Use an owned disposable database for the ignored test. It creates and removes a uniquely named schema.

## Retained qualification

## Historical source reconstruction

For the subsequent process-drain release, first apply the [process-drain reversal](process-drain.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.

For the subsequent bounded-dispatch release, first apply the [bounded-dispatch reversal](bounded-dispatch-turns.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.
