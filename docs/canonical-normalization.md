# Canonical JSON normalization

The reuse boundary keeps four invariants:

- Every prior frame completes the ordinary full validation and normalization before its private wrapper can be created. Callers receive no mutable access to that wrapper's blocks.
- New root/object maps still parse and reject duplicate keys. A collection value reuses an original only when its root, array ordinal or object name, and exact payload bytes match. Collection payloads retain their validated depth of two; scalars retain depth one.
- Changed or new values use the existing canonicalizer and its numeric, nesting, literal-key and Unicode rules. Partitioning and serialized frame bytes match fresh packing, including configuration and root-kind changes.
- The wrapper is consumed by one preparation. Matching payloads move into the new frame; this cache creates no stored fields or separate authority. Whole typed workspace reconstruction, reconciliation and serialization still occur for a new write.

Status: further normalization and write preparation changes have local functional qualification. The codec borrows validated JSON subtrees and expanded numbers, counts block sizes without serializing temporary payload buffers, and packs original members without building a second canonical workspace. Retained-JSON reconciliation builds its record index only when needed. Complete-frame validation, numeric expansion budgets, duplicate-key refusal, partitioning and version-1 frame bytes remain guarded. That source retains format 19 and 26 migrations. The current provenance fence advances compatibility to format 21/migration 028 while preserving all original 27 migrations, codec v1, block layout and application dependency lock.

The preceding borrowed-buffer candidate passed 478 distinct functional tests, exact codec compatibility and the 46-check actual Freight/EDI/Sync recovery drill. Its four native before/after/after/before runs preserve outputs and originals, but the predeclared conservative 15% CPU target was not met. Those source-bound results remain retained; they are not relabelled as a passed performance gate.

The earlier component qualification passed 25 tests and the exact codec comparison retains 24 archived native pairs, 44 configuration pairs, 34 frame controls and five invalid-source controls. In finite counterbalanced CLI observations at 1,000 shipments, median packing user CPU changes from 0.395 to 0.320 seconds and peak RSS from 89.56 to 73.80 MB (decimal). Shared-host wall timings are mixed. These component results do not establish application latency or satisfy the preceding conservative end-to-end CPU gate.

The earlier combined storage/history/retention source passed 287 active Rust/component and 72 PostgreSQL 16 tests, strict all-target Clippy and formatting. Indexed history now groups requested block reads, validates the root once and preserves tenant/root/key ranges under automatic, custom and generic prepared plans. A separate single-candidate PostgreSQL 16 diagnostic at 1,000 shipments observes full-workspace/deferred reads around 1,208/977 ms and writes around 1,249–1,401 ms; exact originals and unchanged physical replay pass. It is not a matched performance improvement. Subsequent invoice-review changes have separately identified regression scope in the current qualification record.

The initial production browser run against PostgreSQL 16 passes 100 of 101 scenarios and exposes a stop-confirmation refresh race. The fix keeps confirmed dialogs held until refreshed route data is available; confirmed refresh failures retry only reads. Actual React components pass all three Chromium regressions, with two demonstrated failures against the preceding implementation. Subsequent qualification also catches a delayed journal notification resurrecting an already-cleared save; a held-response negative control fails against the preceding production bundle and passes after the mutation guard.

The preceding native comparison runs baseline/candidate/candidate/baseline at both 100 and 1,000 shipments on PostgreSQL 16.15. Each endpoint has six timed samples per variant across two runs, with its first read retained separately. At 1,000 shipments:

| Operation                  | Baseline median | Candidate median |
| -------------------------- | --------------: | ---------------: |
| Complete workspace         |     1,148.65 ms |        985.73 ms |
| Deferred-history workspace |       954.46 ms |        813.88 ms |
| Catalog                    |         2.31 ms |          2.33 ms |
| History page               |        34.62 ms |          5.89 ms |
| New save                   |     1,169.94 ms |      1,123.07 ms |
| Original replay            |         8.01 ms |          9.72 ms |

| Operation                  | Baseline median | Current median |
| -------------------------- | --------------: | -------------: |
| Complete workspace         |     1,035.78 ms |    1,083.55 ms |
| Deferred-history workspace |       900.17 ms |      899.82 ms |
| Catalog                    |         2.78 ms |        3.25 ms |
| History page               |        29.17 ms |        6.19 ms |
| New save                   |     1,244.60 ms |    1,133.29 ms |
| Original replay            |         6.03 ms |        6.70 ms |

Median new-save API CPU falls from 0.975 to 0.850 seconds, a **12.8% reduction**. The conservative **15% target remains unmet**. Median observed process peaks rise from 330.10 to 332.57 MB (decimal). At 100 shipments, new-save CPU falls from 0.100 to 0.075 seconds and wall time from 145.93 to 129.03 ms; replay rises from 5.13 to 6.11 ms, and process peaks fall from 51.13 to 48.80 MB. All eight runs preserve compared business outputs, originals and physical source tuples on replay. Explicitly validated additive invoice-review metadata is separated from otherwise exact business-response comparisons. These finite shared-host observations include competing host work and no service resource quotas or sustained arrivals; they establish neither deployment capacity nor a bound on latency/memory. Earlier profiles, including the failed 4.9% CPU gate above and the unpromoted custom normalizer, retain their original source and results.