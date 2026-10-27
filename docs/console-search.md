# Console search execution

Nonempty terminal console searches now evaluate the exposed-value predicate once. A materialized result retains matching record keys, which supplies the exact total, complete membership digest, cursor anchor and selected page. Only the selected keys fetch summaries, through primary-key lookups bounded by the requested page plus its lookahead row. Empty normalized searches keep the preceding indexed keyset query.

This changes read execution only. The repeatable-read transaction, authorization and cursor scope remain in the existing application path. Search still matches literal exposed values, including formatted money and Unicode; private payloads and serialized field names remain excluded. Canonical originals, receipts, invalidation triggers, business format 21 and all 31 migrations are unchanged. Counts and membership hashing still require every matching key; this is not constant-time search.

The finite 5,000-bill fixture measures broad first and late pages, a sparse invoice-number search and a missing term in six alternating pairs per planning mode. Sparse and missing searches avoid the preceding second full predicate scan. Broad searches add materialization and key sorting, and can regress. Read the retained samples and medians before selecting a deployment workload; an exact-count search still scales with the tenant's candidate records. These shared-host instrumented SQL observations establish neither HTTP latency nor a sustained capacity or memory budget. The retained earlier release images and browser/service qualification have their own exact source scope.

Final median PostgreSQL execution times, preceding → new, in milliseconds:

| Search and page | Generic plan | Custom plan | Automatic planning |
| --- | ---: | ---: | ---: |
| Broad, first page | 21.61 → 23.07 | 31.34 → 36.18 | 16.32 → 18.95 |
| Broad, late page | 19.41 → 22.78 | 17.19 → 18.64 | 16.74 → 18.83 |
| Sparse invoice number | 51.58 → 27.94 | 47.72 → 33.31 | 43.33 → 20.49 |
| Missing term | 42.99 → 21.29 | 37.00 → 17.87 | 71.23 → 40.07 |

Sparse/missing medians improve by 30–53%; broad medians regress by 1.45–4.84ms. The fixture also retains 5,000 settlements in the other financial view. It does not represent 10,000 shipments. Both the initial profile and the final 144 observed executions are retained; differences between planning modes also include shared-host drift.

Reproduce with a disposable PostgreSQL 16 database:

```sh
TEST_DATABASE_URL="$OWNED_TEST_DATABASE_URL" \
  cargo test --locked --lib postgres_console_search_shares_one_match_scan_and_bounds_summary_fetches \
  -- --ignored --nocapture
```