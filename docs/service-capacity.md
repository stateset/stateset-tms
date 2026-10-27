# HTTP/PostgreSQL workload qualification

The retained source-`f61e222b` [financial-history diagnostic](finance-capacity.md) **fails** the same 512 MiB/work-one profile during fixed-rate traffic: the API is OOM-killed, the small catalog returns busy responses, and later requests disconnect. Three declared observations commit once; a fourth remains unapplied, with financial originals retained. This failure is preserved; the current release above has its own source-bound profile, and the earlier zero-invoice burst pass is not production sizing evidence.

The preceding source's [API memory-envelope diagnostic](api-memory-envelope.md) adds an actual 512 MiB/two-CPU API limit. Its default two-read profile OOM-kills the API during a read burst; the same offered burst with one snapshot read completes without OOM, at the cost of seconds of queueing. Both outcomes are retained. That read-only observation does not qualify mixed writes or production capacity. The older workload, implementation changes and measurements below retain their original source scope.

This qualification runs the real optimized Freight API, its periodic workers, and isolated PostgreSQL 16. It measures reads and writes with retained shipment, native pickup, gate, event, tender, and operations history. It tests the whole API/database path rather than inferring service capacity from the [in-process projection profile](operations-performance.md).

## Reproduce

```bash
python3 scripts/qualify-service-capacity.py \
  --sizes 1000 --output /tmp/freight-service-before.json
python3 scripts/qualify-service-capacity.py \
  --baseline /tmp/freight-service-before.json \
  --sizes 1000 --output /tmp/freight-service-after.json
```

Requires Linux, Python 3, Rust 1.90+, cached locked Cargo dependencies, `psql`, a local Docker Unix socket, and the cached `postgres:16` image. The harness uses `--release --offline --locked`, two build jobs, a unique ownership label, loopback ports, isolated databases, synthetic scoped credentials, and an empty owned `.env`. It stops only its labelled container and child processes. Original fixtures, application/build/PostgreSQL logs, request samples, resource samples, and query plans remain in its temporary artifact directory. Reports are written to new output paths; credentials and response bodies are excluded from those reports.

The API uses two Tokio asynchronous worker threads. This is not an API CPU or memory quota. PostgreSQL has a two-CPU quota, 768 MiB memory limit, and a 512 MiB data tmpfs. The tmpfs data and WAL also consume the database memory budget; disk-backed deployments need separate measurement. These limits define this experiment; replicas and deployment hardware require their own qualification. Both normal periodic workers remain enabled. A 35-second idle observation crosses their normal sweep interval.

The 1,000-shipment input contains 20,000 archived events, 2,000 gate records, 1,000 original native pickup records, 1,000 legacy tender snapshots, 1,100 operations cases, and 9,900 retained case events. These typed synthetic fixtures have consistent execution times and verified case digests. They are not proof of actual carrier handoffs, finance correctness, or user outcomes. The comparison reuses the exact fixture clock and requires identical fixture bytes/counts.

## Observations and changes

The idle observation also found committed rewrites of unchanged tenant data. The previous expiry worker always opened a writing transaction and saved every workspace, even when no tender qualified.

The changes address those findings:

- `FREIGHT_DATABASE_MAX_CONNECTIONS` now configures each API process's pool, with a default of two and an accepted integer range of 1–64. Invalid values fail startup without echoing configuration contents. The reduced default queues excess connection demand instead of starting ten concurrent aggregate queries. All eight client requests still participate in the same workload.
- Tender expiry first checks the current typed workspace. With no eligible tender it avoids the writing transaction. A potential expiry is rechecked under the tenant lock. A losing sweep rolls back without saving the aggregate or creating an empty tenant.
- Actual expiry remains an operator exception: the current carrier/tender and `Tendered` status remain, the load version advances once, and one system audit event records the missed response deadline. Existing exceptions are preserved. A late 990 can still be applied. Version exhaustion rejects the whole batch without partially recording exceptions.

| Observation                   | Earlier ten connections  | Four-connection trial | Two-connection run |
| ----------------------------- | ------------------------ | --------------------- | ------------------ |
| Idle revisions unchanged      | No                       | Yes                   | Yes                |
| Eight-client read wave        | 8 HTTP 500 / 16 requests | 16 HTTP 200           | 16 HTTP 200        |
| Remaining write/replay checks | Not reached              | Database restarted    | Passed             |
| Database OOM kills            | 1                        | 1                     | 0                  |
| Database peak (MiB)           | 768.0                    | 768.0                 | 524.6              |

The two-connection run completed 109 timed requests: 102 HTTP 200 responses and seven expected HTTP 409 version conflicts. Replay, exact physical-record retention, all expected operations rows, final committed versions/locations, and resource monitoring passed. The cgroup recorded zero memory-limit failures and zero OOM kills. These outcomes describe this finite resource envelope.

Read latency remains material: the operations wave at eight clients had an observed 3,130.8 ms p95, and the small-tenant/health mixed-wave p95 values were about 921/918 ms. Racing versions reached 7,867.6 ms because even rejected commands wait on aggregate work. Queueing preserves correctness here; it does not establish a latency or throughput objective.

## Contracts and measurements

Each workload includes operations reads at one, four, and eight clients; a mixed wave of full workspace, operations, separate-tenant probe, and health reads; eight competing versioned updates to one load; distinct-load writes serialized with explicit reconciliation; original-response replay; and final version/location verification. Expected 409 conflicts are distinguished from failures. The operations reads must include all 1,100 expected cases and a full 25-row page. The root workspace must return the exact original pickup, gate, and tender records; database snapshots also verify those records after concurrent writes, and the probe must receive only its tenant's issues.

Every request records status, bytes, response time, client queue time, and total time. Observed p50/p95/p99 values use nearest ranks from these finite samples. Requests run in bounded-client waves, so these are not open-arrival-rate benchmarks or established tail-latency bounds. The resource observer records PostgreSQL waits/blockers plus API CPU time and RSS. Failed waves and monitor errors remain in the report. PostgreSQL logs and cgroup v1/v2 memory diagnostics are captured before cleanup. A failure is retained rather than converted into a successful capacity claim.

The query plan measures the tenant-row lookup. `EXPLAIN` does not include sending/decompressing the entire result, JSON decoding, source projection, response encoding, or network transit. The end-to-end HTTP samples include those service-path costs. No statement about fleet capacity follows from a fast row lookup alone.

## Operating and remaining limits

Configure the connection budget in the API environment, restart the API processes, and size the combined pools of all replicas against database memory and workload measurements. The default is a conservative starting point, not an automatic memory guarantee for arbitrarily large histories. Aggregate JSONB reads still transfer and decode full tenant data; writes still serialize the whole tenant. Public HTTPS, Clerk, browser rendering, actual carrier traffic, consolidated financial history, production restore, sustained offered load, and multi-replica capacity remain separate gates.

The recorded version predates the [bounded snapshot read path](read-path.md) and subsequent [tenant-aware admission](tenant-read-admission.md). Its measurements and source fingerprints remain historical; follow those reports for their measured versions and current scheduling qualification.

For this checkout, first reverse the [integration queue deadline change](integration-wakeups.md) in an isolated source copy before applying this report's historical reconstruction patch. The original patches and report hashes remain unchanged; the two-stage reconstruction must match the report's original source fingerprint.

For the subsequent snapshot-admission release, first reverse [read overload protection](read-overload.md) using its source patch in an isolated copy, then follow this report's existing reconstruction steps. Its original report, source and patch attestations remain historical.

Current source reconstruction now begins with the [bounded-dispatch reversal](bounded-dispatch-turns.md), then the [process-drain reversal](process-drain.md), then the [tenant-worker reversal](tenant-integration-workers.md), then the [integration-response reversal](integration-response-budgets.md), then the [runtime database-budget reversal](database-budgets.md), then follows the snapshot-overload and earlier reversal steps already described here. Historical performance reports and original patches retain their existing hashes.
