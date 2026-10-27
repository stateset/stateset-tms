# Shipment catalog

`/catalog` is a separate staff shipment browser. Sign-in verifies identity through `GET /v1/access`; opening or searching the catalog does not fetch `/v1/workspace`. Staff can search references, commerce order numbers, route cities/states and reported locations, filter status, move through bounded pages, and open the authoritative shipment in the operations workspace. The catalog shows physical shipment summaries. Operational holds, agreed prices, financial totals and source approvals remain in the full shipment workflow.

## Storage and consistency

Migration 021 adds `freight_load_catalog` and `freight_load_search`. The first records projection validity; the second stores one small shipment summary per tenant/load identity. Its primary key and status/customer/carrier indexes support tenant-scoped keyset browsing. The default page is 25 summaries, with a maximum of 100 and one extra row to determine whether another page exists. The [filtered-query change](catalog-filter-plans.md) uses at most eight bound statement shapes so supplied status/customer/carrier equalities remain available to generic prepared plans. Literal substring search is ASCII case-insensitive; `%` and `_` have no wildcard meaning. Substring search still scans candidate catalog rows and has no trigram/full-text index.

An `AFTER INSERT OR UPDATE OF data` trigger maintains the catalog in the same transaction as the authoritative workspace. A failed or rolled-back source write cannot leave a committed catalog update. Unchanged load arrays bypass maintenance; changed arrays compare records by identity, construct summaries for changed raw records, upsert changed summaries and remove departed identities. See the [incremental maintenance change](catalog-maintenance.md) for retained measurements and regressions. Other shipment rows retain their physical row versions. The source workspace, original files, audit records, finances, receipts and queue identities remain authoritative. Shipment commands continue to recheck their versions and source grants under the existing tenant lock.

The backfill reads original workspaces without rewriting their JSON, timestamps or physical records. It preserves malformed original collections for diagnosis, marks their catalog invalid and removes the partial projection. Duplicate or invalid shipment identities also invalidate the catalog. Returned summaries are strictly decoded before a page succeeds. This partial read model does not validate every unrelated business record; full workspace reads and commands retain their original validation and unknown-root rejection.

Business storage format remains 17; the migration ledger is now version 22. Migration 022 changes maintenance functions without rewriting existing source/catalog records; migration 021 remains unchanged. Startup still rejects edited, gapped or future migration history. Running compatible writers maintain the projection through the database trigger. Backfill and index construction need a qualified deployment maintenance window and database headroom. Include both new tables and projection functions/triggers in coordinated backups.

## API and access

`GET /v1/loads` accepts `limit`, `cursor`, `status`, `customer_id`, `carrier_id` and `q`. It returns `loads`, `next_cursor` and `limit`. Ordering uses load identity with PostgreSQL's `C` collation, matching Rust's string order. Cursors bind the tenant and normalized filters, carry a version and reject malformed or oversized input. They are navigation state, not credentials. Each page is a fresh committed view; changing a shipment so it enters or leaves a filter does not establish a frozen multi-page snapshot. Search again to start from the current first page.

The API derives tenant and staff capabilities from the existing authenticated principal. Carrier/customer principals cannot use this staff catalog or the access endpoint. The web gateway forwards allowed filters once, rejects repeated filter values and ignores forged tenant claims. Clerk organization changes remount the workspace. Local sign-out clears records immediately and fences delayed reads; failed cookie deletion requires explicit retry before another credential sign-in.

## Verification

In the 1,000-shipment fixture, 25 summaries total 10,298 bytes, compared with 13,522,659 bytes of whole-workspace JSON. The default query uses the tenant/load primary-key index and does not access `freight_workspaces`. These are different payload scopes, not an HTTP latency comparison. The report also retains paired source-update observations with catalog maintenance enabled/disabled, including added maintenance cost. They are finite standalone SQL samples, not deployment throughput or service objectives.

The preceding migration-021 release’s three paired update observations measured 475–508 ms with catalog maintenance and 319–439 ms with that trigger disabled. The catalog trigger itself took 131–136 ms. This added write cost is a material tradeoff in this fixture; it must be included when qualifying write-heavy workloads. The observations do not establish production capacity or a useful latency objective.

Reproduce with a fresh fixture binary:

```bash
cargo build --locked --offline --example service_capacity_fixture
python3 scripts/qualify-load-catalog.py \
  --postgres-bin /usr/lib/postgresql/12/bin \
  --output /tmp/freight-load-catalog-sql.json
```

## Remaining work