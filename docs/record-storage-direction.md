# Separate original records from whole-workspace persistence

Status: retained pre-integration storage experiment, measured against format 18 and its 25 migrations. The current application uses format 21 and 31 migrations; [canonical block integration](canonical-blocks.md) subsequently implemented the separate original-record source. This document retains the experiment's original scope and does not qualify the current deployment.

## What the raw data says

The retained 1,000-shipment/history-depth-four fixture contains about 24 MB of source JSON. Stop events, audit events and appointments account for about 64%; operations cases account for another 22%; shipment records account for about 5%. These are complete retained native API source snapshots, including original values, rather than estimates from UI summaries.

Moving only shipments to a table would leave most retained data in the aggregate. A table per root collection would still rewrite a growing audit array and the complete operations-case collection for a small tracking change. The experiment therefore preserves each array element by its ordinal and each object member by its literal key. Duplicate business IDs are preserved rather than silently deduplicated.

`scripts/profile-storage-records.py` compares two isolated storage primitives using actual before/after outcomes from the qualified Freight API. Its SQL lives outside the application migrations. The probe reconstructs every root, checks original payloads and array order, observes `xmin`/`ctid` for unrelated records, and checks that selecting an original receipt performs no physical write. It also checks that a stale tenant revision refuses the entire transaction.

SQL plans include actual execution and WAL instrumentation. Both experimental layouts omit production catalog, history, deadline and format triggers. Native business application and delta preparation are outside these SQL observations. These measurements cannot establish an application latency improvement, concurrency contract, deployment capacity or production storage safety.

## Verified experiment and decision

| Shipments, history depth 4 | Initial original records | Changed record payloads per save | Changed record payload bytes | Initial aggregate MiB | Initial record layout MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100 | 4,067 | 3–4 | 2,743–6,225 | 0.211 | 3.586 |
| 1,000 | 40,607 | 3–4 | 2,750–6,236 | 1.484 | 35.312 |

Each save also updates two collection descriptors and the tenant revision. At least 4,108 or 40,648 unrelated existing record/descriptor tuples retain their actual `xmin` and `ctid`, respectively. The changed-payload figure excludes SQL and metadata bytes; it is not a complete request size or an HTTP latency measurement. The retained SQL plans and primitive timings exclude application logic and production triggers, and use psql text SQL rather than the native SQLx protocol.

The initial per-record layout consumes **about 17–24 times as much disk** as the compressed aggregate in these highly repetitive synthetic fixtures. Table, TOAST and index bytes are included. Post-mutation sizes are retained separately; dead versions, autovacuum and timing affect those observations. Global memory sampling includes both layouts in the same cluster and cannot establish a separate layout's memory peak.

This experiment supports separating the semantic identity of records and writing actual deltas. It does **not** select one uncompressed JSONB tuple per record for production. Qualify compression or bounded block packing against the same originals, row/ordinal integrity rules, write payloads, full reads and disk measurements before committing a physical representation. Preserve unfavorable measurements rather than optimizing only SQL execution time.

## Semantic boundary to preserve

Use one tenant header to serialize existing business transactions, root descriptors to retain collection shape and expected membership, and separate original record payloads. Store array order independently of business identity. Preserve literal receipt keys, nulls, empty collections, large numbers, nested histories and quarantined operations evidence.

The tenant header must retain the exact root manifest. Per-collection counts and contiguous ordinals must be checked before materialization. Otherwise a missing root or record could quietly become a default collection during typed decoding. Missing or inconsistent metadata must fail closed. A complete original must never be reconstructed from an unchecked partial result.

The first application integration should continue running the existing complete typed business mutation and global operations reconciliation. Write only the changed records after that mutation. Keep tenant-wide serialization until shipment, carrier-capacity, finance, source and ownership dependencies justify narrower transactions. This reduces write amplification without discarding cross-record checks. It leaves full-workspace reading and application CPU as separate costs.

The existing aggregate and the record tables must not become competing writable sources of truth. Legacy complete API responses should be reconstructed from canonical records. Any retained projection must be explicitly rebuildable and tied to the same portable committed revision.

## Required application and migration gates

The subsequent [native block codec comparison](storage-blocks.md) qualifies lossless 16/64-record packing against all ten originals and reduces the 1,000-shipment initial disk penalty to about 1.89× at span 64. The [borrowed-entry decoder](block-decoder.md) subsequently reduces measured native unpack CPU/allocation while preserving exact frame compatibility and complete integrity checks. Packing and unfavorable complete-read observations remain explicit costs. At the time of this experiment, the next implementation gate was a canonical record-store adapter with a qualified format transition, these transaction invariants, replacement derived maintenance and full end-to-end application/integration/recovery measurements. The subsequent [canonical integration](canonical-blocks.md) records that application transition and its exact historical qualification; current sustained capacity and full-stack recovery remain open. A shadow table that still requires every production save to rewrite the aggregate does not meet that gate.
