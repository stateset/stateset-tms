# Process drain and restart

Freight owns its HTTP server, integration scheduler and operations worker for the lifetime of one process. Unix SIGTERM and SIGINT request the same cooperative drain. Signal handlers register before opening storage; a stop during asynchronous startup cancels startup without launching services. Listener binding completes before workers start. An unexpected top-level service exit or signal error also initiates drain and results in an unsuccessful process exit.

Draining stops HTTP acceptance and rejects requests reaching the admission gate with HTTP 503, `Retry-After: 1`, `Cache-Control: no-store` and code `service_draining`. This includes health and callback routes. Requests already admitted can finish with their original result. A stopped listener can instead produce a connection failure. Retrying callers must retain the original command/upload key; EDI must retain its original signed delivery identity and body. A failed response does not prove that a business transaction failed to commit.

The integration scheduler stops admitting tenant cycles. An admitted cycle completes its current inbound, source-verification or dispatch stage and its guarded database outcome, then observes shutdown before the next stage. Source verification checks shutdown between shipments. The tender-expiry and operations sweeps stop before the next tenant while allowing their current storage operation to finish. Already entered stages retain their existing internal source, ordering, claim and receipt checks; drain does not add a new distributed transaction or revoke admitted work in another replica.

`FREIGHT_SHUTDOWN_TIMEOUT_MS` is the single process drain budget, default **30000**, accepted range **1–300000** milliseconds. Empty, signed, padded, fractional, zero, overflowing and out-of-range settings fail before storage opens. On successful drain the process logs `Freight drain completed` and exits zero. Deadline expiry logs an explicit error, aborts owned asynchronous service tasks and exits nonzero. Runtime teardown does not wait indefinitely for previously admitted blocking reads. OS scheduling, synchronous code and upstream database rollback remain outside a hard real-time guarantee.

Forced shutdown never clears durable leases or manufactures a failed or successful partner outcome. Unfinished work remains uncertain under the existing sixty-second lease and retry rules. Restart waits for a live lease before reclaiming its original request, preserves callback bodies, signatures, delivery IDs and cumulative attempts, and fences obsolete completions. A process stop does not provide remote exactly-once execution: EDI idempotency and Sync's existing tracking behavior still apply. Demo memory storage cannot survive a process exit.

The Compose API service has a **310-second** stop grace period, above the maximum accepted process budget. Other deployments must allow time beyond their configured drain before forced termination. An external SIGKILL bypasses cooperative drain. Database format, migrations, partner payloads and the web application are unchanged.

## Verification fixtures

The [unit checks](../src/lifecycle_tests.rs) exercise all current/late shutdown waiters, strict budgets and request admission: an already admitted handler retains its response while health, callback and staff requests stop before handler execution. The [worker checks](../src/integration_worker_tests.rs) hold an actual EDI response, request drain, finish that original inbound once, and leave a queued tenant and an actual outbound tender untouched. A stop requested before worker startup produces no claims, expiry changes or operations reconciliation.

The [actual-process checks](../tests/lifecycle.rs) start the real Freight executable with owned empty dotenv files and synthetic credentials, send OS SIGTERM/SIGINT, hold a real Sync response through drain, and observe successful completion or nonzero timeout. They also interrupt a deliberately unfinished database connection during startup. Private process logs remain under `/tmp/freight-shutdown-*`.

The PostgreSQL scenario forces a process deadline during an authenticated EDI fetch, independently reads the exact original persisted lease and callback, restarts, and verifies no early reclaim. It waits for the actual sixty-second lease, without editing fixture time or the database expiry. A second SIGTERM waits for the recovered fetch and guarded commit; the final store retains one applied outcome and both original attempts.

```bash
RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --test lifecycle
RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --lib lifecycle::tests
TEST_DATABASE_URL=postgres://user@localhost/disposable \
  RUSTC_WRAPPER= CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo test --locked --offline --test lifecycle -- --ignored --test-threads=1
```

Use an owned disposable PostgreSQL database; the fixture creates and removes its unique schema. These finite checks do not commission rolling deployments, upstream maintenance, public callback retry policies or a production availability objective.

## Historical source reconstruction

The development record also retains a batch run denied loopback sockets, its browser gate that started no browser tests, and the direct run that exposed a missing required version in the new tender fixture. The fixture supplies its actual original version; no product runtime behavior or assertion was relaxed. The intermediate recovery run was explicitly cancelled before this test-byte correction, and the final drill binds the corrected source.

The first PostgreSQL process fixture shared one release semaphore between the old and retried HTTP requests. Its retained database snapshot shows the callback already applied after lease expiry, so the later assertion that the retry was still held could not establish its premise. Each document attempt now has an independent response gate. The final scenario explicitly releases the old response while verifying that the retry lease stays unchanged, then drains and releases only that retry. The failed fixture log, original applied inbox and exact owned-schema cleanup are retained in the development record.

## Retained qualification

For the subsequent bounded-dispatch release, first apply the [bounded-dispatch reversal](bounded-dispatch-turns.md) in an isolated copy, then follow this page’s existing reconstruction. Historical reports and original patch bytes remain unchanged.
