# Shared workspace processing admission

The current [derived-index admission release](derived-index-admission.md) passes 610 application scenarios and 71 separate TLS/SCRAM recovery checks. This page retains the preceding source-`f61e222b` 600-scenario zero-invoice qualification. Its shared processing budget remains available in the current release. Catalog pages now have their own one-slot finite queue; cold catalog/history builders also join the existing tenant write queue before database acquisition. Leaving `FREIGHT_WORKSPACE_CONCURRENCY` unset disables that optional shared processing budget; it does not disable the new catalog and index admission behavior.

The retained source-`f61e222b` [financial-history diagnostic](finance-capacity.md) **fails** the same 512 MiB/work-one profile during fixed-rate traffic: the API is OOM-killed, the small catalog returns busy responses, and later requests disconnect. Three declared observations commit once; a fourth remains unapplied, with financial originals retained. This failure remains preserved; the current release above has a separate financial profile, and the earlier zero-invoice burst pass is not production sizing evidence.

Set `FREIGHT_WORKSPACE_CONCURRENCY` to an integer from **1 to 64** to share a processing budget across whole-workspace reads and mutation preparation in one Store. Leaving it unset preserves the preceding whole-workspace processing profile only. The limit counts jobs; it does not bound bytes, process memory or fleet capacity.

Read-only admission was insufficient in the retained [mixed memory diagnostic](api-mixed-memory-envelope.md): overlapping large reads and a write killed the 512 MiB API. In the preceding source-`f61e222b` qualification, shared processing set to one lets its production Dockerfile image complete the same finite mixed workload without OOM, preserves the original committed write and keeps health/readiness available. Excess large requests receive an explicit busy response. This qualifies that workload's behavior, not a production memory recommendation.

## Configuration and behavior

For a measured profile, add [compose.workspace-admission.yaml](../compose.workspace-admission.yaml) after the normal Compose files. For example, to inspect the separate-credential configuration with one processing job:

```sh
FREIGHT_WORKSPACE_CONCURRENCY=1 docker compose \
  -f compose.restricted.yaml \
  -f compose.workspace-admission.yaml config --quiet
```

Supply the ordinary database and authentication settings as described in [storage](storage.md#compose-with-separate-credentials). Add the database TLS overlay when using that deployment profile. The workspace overlay requires an explicit value; it does not change the base profile or set memory/CPU limits. Choose the job budget together with resource limits, actual workspace sizes, traffic and operational latency targets. The tested memory profile also sets `FREIGHT_READ_CONCURRENCY=1`, two Tokio threads and two database connections; the overlay alone does not reproduce those settings.

Processing admission uses the existing tenant-turn queue implementation, with an independent queue and the existing read settings: `FREIGHT_READ_MAX_PENDING` (default 64), `FREIGHT_READ_TENANT_MAX_PENDING` (16), and `FREIGHT_READ_QUEUE_TIMEOUT_MS` (5000). These are finite waiting limits, not reservations for each tenant. Snapshot admission and write admission still apply; their queue stages can add to the total request wait. The queue deadline does not interrupt an admitted CPU job or bound an entire request.

The processing lease is acquired before a PostgreSQL connection or tenant row lock. It remains owned through snapshot materialization, staff projection and encoding, or mutation preparation and the actual transaction commit/rollback. Cancelling an HTTP awaiter cannot release capacity while its worker or transaction still owns the work. Memory-mode mutations retain the same ownership and discard an abandoned candidate.

Known original command receipts can replay without joining the heavy processing queue. A miss still obtains admission and rechecks transactional source/receipt state. Changed request bodies and damaged originals remain refused. Indexed catalog/history paths and selected receipt reads retain their existing narrower boundaries.

For configured heavy transactions, a local **1 ms PostgreSQL lock timeout** prevents another process's locked tenant from occupying processing admission for the usual database lock-wait budget. Contention returns a retryable database-busy response; it does not automatically retry a write. Recover uncertain outcomes with the original idempotency key and exact request bytes. Cross-process correctness still comes from PostgreSQL transactions and locks.

Processing overload returns HTTP **503**, code `workspace_capacity_exceeded`, `Retry-After: 1` and `Cache-Control: no-store`. Database and existing snapshot/write budget failures retain their own codes. Preserve unsaved work and retry the original request explicitly.

## Qualified local profile

The actual production-image memory run uses a 512 MiB/two-CPU API without swap or allocator tuning, a 768 MiB/two-CPU PostgreSQL container, and work/read concurrency one. Three synthetic tenants contain 1,000, 100 and 1,000 shipments; each large tenant has 20,000 retained events. These fixtures have **zero invoices**. Eight full reads, a separate large-tenant location write, a small catalog probe, health and readiness start together, with idle observations before and after.

| Observation                   | Actual production image                              |
| ----------------------------- | ---------------------------------------------------- |
| Kernel API memory peak        | 341.80 MiB                                           |
| Large reads                   | Four complete 200 responses, four explicit busy 503s |
| Accepted large-read times     | 1.08–5.32 seconds                                    |
| Small catalog probe           | 3.16 seconds                                         |
| Location write                | 2.14 seconds, committed once                         |
| Health / readiness            | 5 / 4 ms                                             |
| Exact original receipt replay | 18 ms, no physical source rewrite                    |

Nineteen memory-harness tests and eleven snapshot-capture tests are separate from the 600 application scenarios. The capture helper now accepts identical wire-source files already frozen by the main manifest, leaves their bytes and metadata unchanged, and refuses conflicts or symlinks; all eight earlier containment tests remain. Failed preparation attempts are retained, and the complete recovery uses a fresh bundle. Storage format **21**, codec **1**, migration **028**, dependency locks and frontend inputs remain unchanged.

## Remaining capacity and product requirements

`Store::read()` can return an owned workspace that a caller retains after materialization. Encoded HTTP bodies, slow clients, upload buffers, other Store instances, other processes and replicas are outside this job limit. A single larger workspace, richer financial history or sustained arrivals can exceed the tested resource budget. Additional byte accounting, narrower storage/projection boundaries and deployment measurements remain necessary.