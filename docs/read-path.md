# Bounded snapshot reads

Workspace and control-tower reads now keep snapshot decoding, projection, and JSON encoding off Tokio's asynchronous request threads. `FREIGHT_READ_CONCURRENCY` bounds admitted read work, default two and configurable from 1–64. It is separate from `FREIGHT_DATABASE_MAX_CONNECTIONS`; both budgets apply per API process and must be sized across replicas.

The earlier [resource qualification](service-capacity.md) prevented observed database OOM kills but still measured 3.13-second operations p95 and about 0.92-second small-tenant/health p95. This change addresses decoding and scheduler contention without removing retained history or relaxing source verification.

## Processing and cancellation

1. Take the tenant's admission turn, then acquire a read slot before fetching a snapshot. Only one waiter per tenant joins the shared queue at a time. Queued callers can cancel without starting work. See the subsequent [tenant-aware admission change](tenant-read-admission.md).
2. An admitted task owns the slot through SQL, decoding, and completion even if its caller disconnects. This avoids releasing a slot while an abandoned query or blocking job still consumes resources.
3. Hold the existing storage-format lock while receiving and decoding the snapshot. PostgreSQL renders JSONB as text; the reader receives that text without first materializing the entire root `serde_json::Value` tree.
4. Check the exact format marker before interpreting business fields. Stream all business fields through the existing strict `Workspace` deserializer. Only the root `_freight_format` key is filtered; unknown business roots still fail. Unsupported case/policy structures retain their original values in quarantine.
5. Release the database transaction and connection before projecting the staff workspace or control-tower response. The same read slot remains owned through projection, encoding, and snapshot disposal.

`Store::read` applies the budget through snapshot cloning/decoding. The staff workspace, operations list, and operations detail routes additionally hold it through projection and encoding. Other routes still have their existing projection code; writes still use the original transactional decoder and financial/document guards. This change does not establish a global CPU or memory bound for every endpoint.

The blocking workers own their permits. Cancellation cannot free a running job's permit merely by dropping its awaiter. Admitted reads complete bounded work rather than being interrupted halfway through decoding. This is a concurrency budget, not a per-request time limit. Immutable read snapshots remain separate from later writes, which recheck current versions and source digests under the tenant lock.

Quarantine parsing also avoids cloning complete `Value` trees merely to attempt typed case/policy deserialization. It borrows the original value for that attempt and preserves it unchanged on failure. Successful decoded records and their canonical hashes remain identical.

## Evidence

```bash
python3 scripts/profile-read-decode.py \
  --fixture /tmp/owned-run/fixture-1000-large.json \
  --output /tmp/read-decode.json
```

| Wave / response | Before p95 (ms) | After p95 (ms) |
| --- | --- | --- |
| operations-read / operations | 3,130.8 | 1,461.1 |
| mixed-tenants-and-health / health | 918.1 | 126.6 |
| mixed-tenants-and-health / probe | 921.0 | 1,071.0 |
| mixed-tenants-and-health / workspace | 1,472.5 | 1,469.2 |
| same-load-version-race / competing-version | 7,867.6 | 3,987.2 |

The run completed 109 timed requests with 102 HTTP 200 responses, seven expected HTTP 409 conflicts, no unexpected errors, zero OOM kills and no idle rewrites. Original physical records and committed versions/locations were retained. The separate-tenant probe regressed; the raw report retains that observation. Read scheduling improves selected paths and does not establish uniform latency or tenant fairness.

The fixture contains 1,000 shipments, 20,000 archived events, 2,000 gate records, 1,000 original native pickup records, 1,100 work cases and 9,900 retained work events. The exact original pickup, gate and tender arrays must survive reads and concurrent updates. The workload includes health/small-tenant probes, competing and distinct-load writes, explicit reconciliation, original-response replay, final versions/locations, periodic workers and cgroup OOM checks. See the [procedure and resource limits](service-capacity.md).

The database peak increased from 524.6 to 543.6 MiB within its 768 MiB limit. Sampled API peak RSS increased from 387.2 to 522.3 MiB; the API had no imposed memory quota. Concurrent CPU work and allocator/thread behavior need separate attribution before claiming a memory improvement or fitting a constrained API container.

## Remaining work

Whole-tenant JSONB is still transferred, decoded and verified. Expensive mutation code and other route projections retain their existing scheduling. The subsequent [tenant-aware admission](tenant-read-admission.md) addresses one tenant filling shared admission; it does not establish a global CPU share, an open-arrival-rate capacity limit, bounded queue age, or a certified service objective. Database-backed deployments, multiple replicas, larger financial histories, Clerk/TLS, browser rendering, actual carrier traffic and production restore remain separate qualification gates. The reports on this page retain their measured bounded-read-release fingerprints.

The subsequent [single-job pipeline](read-pipeline.md) keeps decode and projection on the same blocking worker and adds opt-in phase attribution. Its reports qualify that measured implementation; the earlier reports on this page preserve their original source fingerprints.

For this checkout, first reverse the [integration queue deadline change](integration-wakeups.md) in an isolated source copy before applying this report's historical reconstruction patch. The original patches and report hashes remain unchanged; the two-stage reconstruction must match the report's original source fingerprint.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
