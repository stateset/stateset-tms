# Indexed original shipment history

The subsequent [tender deadline index](tender-deadline-index.md) addresses a separately observed overlap between periodic full-source reads. Its source-bound record includes continuous background memory observations; the migration 024 results below retain their historical scope.

Shipment events, appointments and gate history now use tenant/kind/shipment indexes of original array positions. Warm PostgreSQL pages no longer validate and filter every unrelated retained record. Original payloads remain exclusively in `freight_workspaces.data`; the API still returns the original typed records and their source positions. The [history API, staff access rules and version-1 cursor contract](deferred-history.md) are preserved.

Source writes advance a portable `history_revision` and invalidate the derived index in the same transaction. The first subsequent history read locks the tenant row, rechecks readiness, and rebuilds from the current authoritative source before selecting its page. Competing cold readers recheck after acquiring the lock. This avoids eager projection during ingestion, when PostgreSQL still holds temporary JSON parser allocations. Warm reads use the cached scope positions and extract only the requested original collection. JSONB decompression, scanning the selected scope's ordinal array, and arbitrary per-record byte sizes remain costs.

Each page uses one PostgreSQL `REPEATABLE READ` transaction for the readiness check, optional rebuild and original selection. A command committing between those statements cannot mix source and index generations. Warm readers can finish their coherent committed snapshot without locking out a tenant writer. Competing cold rebuilds retry serialization conflicts in at most three transactions, retaining the admitted read slot; exhaustion returns the existing retryable database-busy response. This per-page snapshot does not turn cursors into snapshots spanning multiple requests.

The index contains collection validity/counts, shipment identity/version metadata, and strictly increasing positive ordinal arrays. It never copies event, appointment or gate payloads. Selected originals are checked against their source identities. Scope deletion changes an independent cardinality counter, so damaged mappings cannot silently become a successful smaller history. An operator can rebuild derived state with `SELECT freight_rebuild_history('<tenant>')`; malformed authoritative collections remain errors and retain their original values.

## Compatibility and recovery

Business format remains **18**; migration **024** adds four derived tables and the source revision column. Existing-source backfill is synchronous at migration startup and preserves source JSON, physical row revision, timestamps, business versions and receipts. There is no background rebuild queue. Cold reads can wait behind tenant writes and hold the tenant lock while rebuilding; this cost must be included in deployment planning.

Back up the whole schema, including all `freight_history_*` tables, functions and triggers. The revision survives logical restore; PostgreSQL's physical `xmin` does not identify the restored source. The current recovery drill restores all 147 application tables across actual Freight, EDI and Sync, then resumes a history cursor captured before loss. Original queue work, financial records and exact replay still survive.

An already prepared preceding-format writer can continue through the migration and invalidates the new index correctly. A preceding executable's **startup** refuses the newer migration ledger. The source-reversal patch is for isolated reproduction, not a database downgrade or an operational rollback procedure.

## Verification

## Measured costs

Warm page medians are milliseconds. Every measured original-history response is byte-for-byte identical across versions; no response fields are normalized.

| Shipments / depth | Global events before → after | Shipment events | Shipment gates | Shipment appointments |
| --- | --- | --- | --- | --- |
| 100 / 4 | 13.7 → 5.6 | 15.4 → 3.7 | 13.7 → 3.9 | 11.1 → 3.4 |
| 1,000 / 4 | 90.4 → 17.9 | 111.9 → 13.8 | 99.4 → 17.2 | 77.4 → 12.6 |
| 1,000 / 16 | 107.9 → 44.5 | 131.7 → 41.8 | 217.4 → 77.6 | 161.6 → 57.2 |

Cold/dirty reads and native commands below are single observations per fixture, not percentiles. The cold initial gate page is the first history request after insertion; the dirty shipment-event page is the first history request after an actual location command. Source verification runs outside timed samples. The physical-source observer uses one complete JSON checksum instead of allocating a second text representation just to measure its length.

| Shipments / depth | Initial history before → after ms | First history after write ms | Native location command ms | Added derived-table MiB |
| --- | --- | --- | --- | --- |
| 100 / 4 | 18.1 → 40.3 | 10.8 → 20.6 | 183.2 → 157.5 | 0.23 |
| 1,000 / 4 | 141.3 → 270.8 | 63.1 → 219.5 | 1729.5 → 1568.5 | 1.09 |
| 1,000 / 16 | 244.0 → 586.2 | 78.7 → 342.3 | 3216.5 → 3281.4 | 1.40 |

Warm reads improve in every measured case, while first/dirty reads regress because they build the index. The dense native command is slightly slower in this single observation; no write-capacity improvement is established. Warm-read sampled API RSS in MiB is 100/4: 34.2 → 36.5, 1,000/4: 169.5 → 215.2, 1,000/16: 248.1 → 248.1. The 1,000/depth-four observation increases; no API memory-bound improvement follows. Observed PostgreSQL cgroup high-water rises from 747.6 to 768.0 MiB. Both final runs have zero OOM kills and complete every functional check, but the new run reaches the database limit with `memory.failcnt=813` (before: zero). This observed memory pressure remains an open resource gate. These results do not establish sustained capacity or a safe production memory budget.

The independent review verifies 192 warmup/measured HTTP response archives, 48 additional original-history pages and 1,776 returned originals across both variants. Selected first/last shipment histories are complete; global validation explicitly covers two pages per fixture. Complete legacy workspaces still match the preceding response hashes after removing only `operations.generated_at`. Native command receipts replay the exact original result with the documented `replayed` flag change and no physical source update. Later event appends preserve an open cursor window; changed shipment versions reject old stop cursors.

Run `scripts/profile-indexed-history.py --variant before|after --inventory <retained-inventory.json> --output <new-report.json>` to repeat the paired protocol. Retain the baseline executable/source copies and reference fixture archives. Failed runs are not passing measurement samples.

## Operating limits