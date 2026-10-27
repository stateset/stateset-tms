# Incremental shipment catalog maintenance

Migration 022 reduces repeated summary construction when one shipment changes. The trigger compares the new load records with the preceding records by identity and projects only raw records that differ. The existing tuple comparison still prevents a physical catalog update when a raw change affects only private fields. Reordering otherwise identical shipments requires no summary upserts. This change does not normalize business writes: the application still reads, locks, decodes and rewrites the tenant aggregate.

The entire new collection is still checked for malformed records, duplicate identities and invalid required fields before any partial catalog can succeed. Departed identities are removed. An invalid catalog clears its summaries, and repairing its source rebuilds every row, including source records that were unchanged across the repair. Inserts and the original two-argument explicit rebuild function use a full projection. Source changes, catalog changes and validity changes commit or roll back together under the workspace transaction.

The new migration appends to the ledger; migration 021 and its deployed checksum remain unchanged. Business format stays 17. Migration 022 changes function definitions without backfilling or rewriting source records, documents or existing catalog rows. The existing trigger function and two-argument projector retain their identities. Tests keep an old writer connection alive with cached function plans across two concurrent application upgraders. This qualifies that connection behavior; production rolling upgrades remain a separate deployment procedure, and an older executable's startup fence still rejects a future migration ledger.

## Measured work and cost

For each workload, the harness takes five observations per version, alternating order. The preceding observations temporarily restore the exact migration-021 function bodies in the current schema and roll back both the function changes and the source update. They are a SQL maintenance comparison, not a comparison between historical API executables. All samples are retained in the report. Separate instrumented nested-statement profiles attribute work but are excluded from the timing samples.

| Workload | Preceding catalog trigger median | Current catalog trigger median | Preceding complete UPDATE median | Current complete UPDATE median |
| --- | ---: | ---: | ---: | ---: |
| One shipment location | 153.496 ms | 112.766 ms | 577.197 ms | 553.138 ms |
| All shipment locations | 248.586 ms | 209.577 ms | 833.238 ms | 751.070 ms |
| Reverse shipment order | 150.352 ms | 156.999 ms | 648.192 ms | 781.289 ms |
| History-only change | 53.826 ms | 50.845 ms | 431.085 ms | 452.361 ms |

Reproduce with a newly named output:

```bash
cargo build --locked --offline --example service_capacity_fixture
python3 scripts/qualify-load-catalog.py --compare-maintenance \
  --output /tmp/freight-catalog-maintenance-new.json
```

The harness refuses to overwrite an existing report. Without `--compare-maintenance`, it still runs source retention, catalog upgrade, bounded query-plan, transactional update, literal search, rollback, damage and isolation checks, plus enabled/disabled write observations. Small fixtures may legitimately use a sequential plan; the primary-key-plan requirement applies at 1,000 shipments or more.

## Correctness and remaining qualification

The real PostgreSQL differential test compares every projected field and validity flag with the released migration-021 full projector. Cases cover single changes, search-only fields, private-field changes, reorder, insertion, removal, identity replacement, missing optional fields versus explicit nulls, malformed collections, scalar members, missing/oversized/non-string identities, invalid statuses/customers/carriers, repeated repair, empty collections, and explicit rebuild after catalog damage. Physical row checks detect unnecessary rewrites. The existing catalog test covers source/index commit visibility, rollback, original command replay, tenant boundaries and catalog reads under an exclusive workspace-table lock.