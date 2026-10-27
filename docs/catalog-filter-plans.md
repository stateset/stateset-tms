# Indexed filtered shipment pages

The shipment catalog now keeps status, customer and carrier filters usable by PostgreSQL after prepared statements become generic. The supplied predicates use direct equality against the existing composite indexes. All seven values, including tenant, cursor and page limit, remain bound parameters. User values never become SQL text. The API, result fields, literal search rules, cursor scope, authorization and transactional read snapshot stay the same.

The implementation keeps the original canonical SQL and builds at most eight fixed query shapes, one for each combination of supplied identity/status filters. They are initialized once and selected by filter presence. PostgreSQL can cache each shape independently; values and pagination do not create additional shapes. The unfiltered query remains byte-identical to the original. This increases the maximum catalog statement shapes per connection from one to eight. It needs no migration, index build, source rewrite or storage-format change: the business format remains 17 and the migration ledger remains 22.

## Observed failure and correction

The original nullable predicates, such as `($3 IS NULL OR status=$3)`, worked logically but prevented the generic planner from relying on the status equality. The real regression test reproduces this under `force_generic_plan` and under the default `auto` setting after six warm executions. It prepares the actual SELECT and explains its EXECUTE; preparing an EXPLAIN wrapper would not establish the same cache behavior.

The fixture has two separate tenants with 1,000 shipments each. Each tenant has 50 matching shipments after 950 nonmatching identities in key order. The current and preceding statements run against the same retained source and database statistics, using separate fresh prepared-statement caches. Every status/customer/carrier filter combination is exercised.

| Plan mode | Preceding unrelated rows filtered | Current unrelated rows filtered |
| --- | ---: | ---: |
| Generic | 950 | 0 |
| Automatic after warming | 950 | 0 |
| Custom | 0 | 0 |

The 96 retained plan observations include exact statement text, arguments, server prepared-statement metadata, buffers and execution timing. They are one observation per version/filter/mode per engine, not repeated controlled timing trials. This qualifies fewer unrelated row examinations in this selectivity fixture. It does not establish a general HTTP latency reduction, deployment throughput or service objective.

## Verification

On each engine, the new regression captures 48 before/after plans, compares 32 pages with the independent typed in-memory implementation, verifies cursor traversal and literal `%`/`_` matching, checks hostile-looking strings remain data, and retains distinct tenant references. Three in-process Axum requests additionally verify combined filters and URL-decoded search against the actual sqlx/PostgreSQL store, permitted viewer access, denied carrier access and denied unknown credentials. Exact source bytes, row revisions and timestamps remain unchanged by these reads. Existing PostgreSQL suites continue to check invalid catalogs, rollback, source repair, original replay and browsing while the workspace table is exclusively locked.

Reproduce the focused regression with a disposable PostgreSQL 12 or 16 database:

```bash
TEST_DATABASE_URL=postgres://user@127.0.0.1:5432/disposable_freight \
  cargo test --locked --offline --lib postgres_filtered_catalog_plans \
  -- --ignored --nocapture --test-threads=1
```

## Limits and historical reconstruction