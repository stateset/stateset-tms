# Tenant-aware read admission

The API now gives each tenant one waiting position in shared read admission. A burst of workspace/control-tower requests from one tenant cannot fill the global queue ahead of another tenant's read. Existing admitted work still completes; this is not preemption or a latency guarantee.

## Admission rule

`Store::read` and `Store::read_project` share the same tenant-turn registry and the same configured `FREIGHT_READ_CONCURRENCY` semaphore across store clones. A tenant's FIFO turn lock admits only its head waiter to the global FIFO semaphore. Once that waiter receives a slot, it releases the turn so the next request can contend. A lone tenant can use every idle slot; capacity is not permanently partitioned.

Local waiters hold no global slots and start no SQL or CPU jobs. Cancelling a local or global waiter releases its position. After admission, the existing owned SQL/decode/projection task retains its slot through actual completion even if the HTTP caller disconnects. The turn-registry lock is synchronous, short, and never held across an await. Registry entries hold weak references; registering a new tenant removes expired entries, so historical sequential identities cannot accumulate without limit.

This is admission fairness between observed tenant IDs within one API process. One tenant can still have multiple admitted jobs. Existing jobs are not interrupted; expensive writes, the database pool, other route projections, multiple replicas and upstream identity checks have separate scheduling. FIFO turns do not equalize CPU time or shipment-history size, bound queue age, or reject overload.

## Verification

Three admission tests cover both-slot utilization by one tenant, a second tenant advancing ahead of an older same-tenant tail, cancellation of head/local waiters, continued progress, and reclaiming 1,000 sequential tenant identities. A store-level test holds a real blocking projection, queues two reads from its tenant followed by a different tenant, and requires the probe to complete before the older local tail can consume the single slot. Existing cancellation, PostgreSQL lock, immutable-snapshot, version-race and source-integrity checks remain required.

The profiling driver runs the actual Freight HTTP/PostgreSQL workload with additional samples from the owned API's Linux `status`, `smaps_rollup`, `maps`, and individual thread statistics:

```bash
python3 scripts/profile-service-reads.py --sizes 1000 \
  --baseline docs/evidence/tenant-read-before.json \
  --output /tmp/tenant-read-profile.json
```

It preserves original fixture bytes, worker settings, PostgreSQL limits, expected response contents, replay/version checks, physical-record arrays, and OOM diagnostics. It adds observation overhead; compare runs of this driver with each other. Resident, anonymous and high-water memory and thread CPU observations do not identify allocation call sites or prove an allocator cause.

| Wave / response | Before p95 (ms) | After p95 (ms) | Samples per run |
| --- | --- | --- | --- |
| operations-read, one client | 332.3 | 517.1 | 16 |
| operations-read, eight clients | 1,644.7 | 2,548.9 | 16 |
| mixed-tenants-and-health / probe | 896.1 | 1,158.1 | 16 |
| mixed-tenants-and-health / health | 265.0 | 265.0 | 16 |
| mixed-tenants-and-health / workspace | 1,667.0 | 2,103.5 | 4 |
| serialized-writes / distinct-load-write | 6,222.5 | 5,834.7 | 4 |

Both runs completed all 109 timed requests with 102 HTTP 200 responses and seven expected HTTP 409 version conflicts, zero database OOM kills, unchanged idle revisions and exact retained original pickup/gate/tender arrays. The queue-order property is verified by the controlled admission/store tests. The timed HTTP run regressed for most read paths and does **not** establish a latency improvement, fairness of CPU service, or stable deployment performance. Retain the unfavorable results; repeated runs and deeper profiling are required before qualifying this scheduling behavior for deployment. With sixteen samples, nearest-rank p95 is the maximum observed response, not a reliable population percentile.

Sampled API peak resident memory was 644.0 MiB before and 645.2 MiB after, with no API memory limit imposed by the harness. Thread counts ranged from four to six before and five to six after. In the first single-client wave the sampled CPU-time delta increased from about 2.34 to 4.00 seconds, and observed threads increased from four to five. The process samples retain per-thread CPU and anonymous memory observations for further investigation. These observations do not establish whether worker handoff, allocation/retention, host effects or another factor caused the timing changes.

## Open work

For this checkout, first reverse the [integration queue deadline change](integration-wakeups.md) in an isolated source copy before applying this report's historical reconstruction patch. The original patches and report hashes remain unchanged; the two-stage reconstruction must match the report's original source fingerprint.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
