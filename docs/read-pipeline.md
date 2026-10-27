# Single-job projected reads

Projected workspace/control-tower reads now keep snapshot decoding, projection, encoding and response-value disposal in one blocking job. The job retains the read permit throughout. Tenant-turn admission, snapshot semantics, strict storage decoding, original retained evidence, and write guards remain in force. Storage format remains 17 with migrations 001–019.

## Resource lifecycle

An admitted asynchronous task acquires a database connection and the format lock, then receives the snapshot text. It passes that owned transaction and text to one blocking job. The job decodes the complete typed workspace while holding the format lock, commits the read transaction, and only then projects and encodes the response. It drops the temporary response value before releasing its permit. In memory mode, it clones under the owned read guard and drops the guard before projection, allowing later mutations to proceed against current versions.

The blocking worker uses the existing runtime handle to wait for the SQL commit. It does not run a new nested runtime or block an asynchronous request thread. The originating runtime must continue driving IO; a current-thread runtime must still be inside its outer `Runtime::block_on`. A stopped runtime can return IO errors or panic on timer use. This follows [Tokio's runtime-handle contract](https://docs.rs/tokio/latest/tokio/runtime/struct.Handle.html#method.block_on), also inspected in the locked 1.53.2 dependency source. The PostgreSQL regression runs on a current-thread runtime and exercises the actual commit/connection lifecycle.

Queued callers can cancel before starting work. Once admitted, the owned asynchronous task and blocking job retain the slot after caller cancellation until their SQL/CPU work finishes. Decode/commit failures skip projection and return their existing errors. This change does not impose a worker-thread cap, per-request deadline, queue-age bound or whole-process memory limit. Writes and other endpoint projections retain their existing scheduling.

## Phase profiling

Enable `RUST_LOG=stateset_freight::read_profile=debug` to emit opt-in `freight_read_profile` records for projected workspace/control-tower reads. Each record contains a random correlation ID, tenant ID, phase name, elapsed wall milliseconds and worker thread ID. It excludes workspace payloads, credentials, actor details, shipment documents and rates. The default log level does not emit these records. Logging itself adds allocation, formatting and file-write overhead.

The phase driver combines those records with the existing retained HTTP/PostgreSQL workload and process/thread samples:

```bash
python3 scripts/profile-read-pipeline.py --sizes 1000 \
  --baseline docs/evidence/read-pipeline-before.json \
  --output /tmp/read-pipeline.json
```

The before run captured 80 complete projected reads; 33 changed workers between decode and projection. For the large tenant, median admission wait was about 540 ms, snapshot fetch 184 ms, decoding 164 ms and projection 174 ms. Waiting for the second blocking job measured only about 0.02 ms median and 1.1 ms p95. These observations locate substantial costs in admission and whole-tenant work rather than proving the second queue wait was the main bottleneck.

Phase wall times include preemption and observation overhead. Snapshot fetch includes pool wait, format-lock queries and transfer. Projection includes disposal of the consumed typed snapshot. Completion includes admission wait and phase logging, while HTTP response times also include authentication and transport. Large-tenant summaries combine operations and workspace routes. Pipeline-owned blocking-job counts are inferred from explicit queue-stage events and checked against the implementation; they exclude SQLx/Tokio internal work. These are finite synthetic observations, not CPU-call-stack attribution or a capacity SLA.

## Measured results

| Default logging workload | Before | After | Samples per run |
| --- | --- | --- | --- |
| Operations p95, one client | 517.1 ms | 526.6 ms | 16 |
| Operations p95, eight clients | 2,548.9 ms | 2,177.6 ms | 16 |
| Mixed probe p95 | 1,158.1 ms | 1,309.4 ms | 16 |
| Mixed health p95 | 265.0 ms | 352.1 ms | 16 |
| Mixed workspace p95 | 2,103.5 ms | 2,270.5 ms | 4 |
| Competing-version write p95 | 5,221.2 ms | 7,178.0 ms | 8 |
| Distinct-load write p95 | 5,834.7 ms | 6,910.5 ms | 4 |
| Sampled API peak RSS | 645.2 MiB | 560.3 MiB | Per-wave procfs samples |
| Observed thread counts | 5–6 | 4–5 | Per-wave procfs samples |

The default logging run improved the eight-client operations measurement and sampled memory while other latencies regressed. Neither comparison demonstrates uniform responsiveness, allocator causality or a qualified deployment objective. The API had no imposed CPU or memory quota; host scheduling and repeated-run variability remain unqualified. The sixteen-sample nearest-rank p95 is the maximum observed response, not a reliable population percentile.

## Correctness checks and open work

The new PostgreSQL regression holds a real projection, verifies that all read connections have returned to the pool and the format row can be locked independently, cancels the original caller, verifies the slot remains occupied, and commits a later source update. It then exercises an atomic newer-format upgrade, unsupported business-root rejection, slot/connection recovery and a successful current-version read after restoration. Existing tests retain memory snapshot/write concurrency, tenant ordering, cancellation under a real SQL lock, format fencing, quarantine and byte/source/financial integrity.

For this checkout, first reverse the [integration queue deadline change](integration-wakeups.md) in an isolated source copy before applying this report's historical reconstruction patch. The original patches and report hashes remain unchanged; the two-stage reconstruction must match the report's original source fingerprint.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
