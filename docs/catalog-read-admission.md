# Catalog admission and remaining row contention

This page retains the unpromoted `052d236b` stage. The current [derived-index admission release](derived-index-admission.md), source `6b520248`, includes the qualified successor fix and is integrated in the main workspace.

The isolated catalog candidate removes the reproduced admission starvation: **all 60 small catalog queries succeed in each of three financial-load runs**. Candidate `052d236b` was **not promoted**. Both complete browser attempts finish with 104 passes and one database lock-timeout failure. At this stage the qualified application remained source `f61e222b`, with its existing 600 application and 71 recovery checks.

## Financial workload

All attempts use the actual production Dockerfile image, the same original 1,000/100-shipment financial fixtures and 246 scheduled arrivals. API/PostgreSQL limits remain 512/768 MiB, two CPUs each, without swap. Work/read concurrency stays one, with two Tokio threads and two database connections. Allocator settings and error logging remain unchanged.

| Attempt                               | Arrivals sent | Complete large reads / busy | Catalog successes | Writes committed | Exact replays | API peak MiB |
| ------------------------------------- | ------------: | --------------------------: | ----------------: | ---------------: | ------------: | -----------: |
| First, overlapping qualification jobs |     243 / 246 |                     28 / 32 |           60 / 60 |                3 |   Not reached |       465.32 |
| Second, separate workload             |     245 / 246 |                     32 / 28 |           60 / 60 |                5 |   Not reached |       475.84 |
| Third, timing observations added      |     246 / 246 |                     38 / 22 |           60 / 60 |                6 |             6 |       504.64 |

The first two runs **fail** because the client withholds later writes while still verifying the preceding original. The third adds only monotonic observations around the unchanged verifier and passes the independently declared full profile. Health and readiness return 200 for all 60 calls each in every attempt. No OOM, hard memory-limit fault or transport failure is observed.

Every accepted response is complete; warmed responses match original `f61e222b` business output except `operations.generated_at`. Exact versions, events, receipts, case evidence seals and financial originals are checked. Physical replay no-rewrite checks run in the original harness; the independent audit rechecks logical originals and receipt bodies. Checked database backups precede owned-resource removal.

Original-state SQL extraction takes roughly four to five seconds per write in the timing run. The complete guarded write and verification takes 6.3–9.4 seconds, near the ten-second arrival interval. An offline shortcut prototype avoids redundant serialization but was **not integrated**. The earlier failures remain observations; the later pass does not prove their precise cause or repeatable capacity. The approximately 505 MiB API peak leaves little room under this finite limit. This is neither a RAM bound nor production sizing or latency evidence.

## Admission and the next correctness gate

Catalog admission has one active slot per Store and separate finite defaults: 64 pending, at most 16 per tenant, with a 5,000 ms queue deadline. Candidate overrides are `FREIGHT_CATALOG_MAX_PENDING`, `FREIGHT_CATALOG_TENANT_MAX_PENDING` and `FREIGHT_CATALOG_QUEUE_TIMEOUT_MS`. Its optional Compose overlay omits unset values, preserving dependent defaults; six synthetic configuration checks pass. It adds no database connections or reserved connection. Snapshot/history admission remains separate.

Controlled held-worker tests fail on both earlier implementations and pass on this candidate: a real snapshot CPU worker can remain held while a catalog page completes. Cancellation tests retain the catalog lease through actual SQL rollback. The invalid first shared-Cargo-cache qualification is retained and excluded; subsequent controls and candidate checks use separate build directories.

Both browser failures occur while locking a workspace row. Separate controlled tests now reproduce a same-store overlap: cold catalog and history builders can hold that row outside local mutation admission, causing a new write to refuse under its existing fast lock budget. This reproduces a plausible mechanism; the browser traces alone do not identify their original lock holders.