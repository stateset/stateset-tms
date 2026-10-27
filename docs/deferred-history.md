# Original shipment history on demand

The current PostgreSQL implementation uses [indexed original positions](indexed-history.md), added in migration 024. The measurements and release record below describe the preceding migration 023 implementation. The API and browser contracts remain current.

The staff workspace requests `GET /v1/workspace?history=deferred`. Current shipment execution, appointment selection, chronology, evidence holds, finance and operating queues are preserved. The three original collections (`events`, `appointments`, `stop_events`) are omitted and replaced by a `history` descriptor containing their counts. Omission does not mean an empty history. The default workspace response, and `?history=complete`, retain the complete legacy response.

Staff can open original history from the activity log or a shipment, and expand appointment/gate evidence when needed. Pages show retained records newest first, with an explicit request for older records. Original event JSON is expanded on demand. Appointment contacts, references, notes, cancellations and replacement links remain available. Current appointment and gate cards continue to use the same derived execution data.

## Read contract

`GET /v1/history?kind=appointments&load_id=<id>&limit=25` returns original typed records with their source array positions. `kind` is `events`, `appointments` or `gates`; stop histories require a shipment identity. Events can be scoped to a shipment or the whole staff workspace. The default page size is 25; accepted sizes are 1–100. Subsequent requests use the returned `next_cursor` with the same kind and shipment filter. There is no historical-record cutoff.

The cursor captures an append window, not an MVCC snapshot spanning requests. The upper source position, tail identity hash, selected count, tenant/filter scope and, for stop histories, the shipment version are checked. Later appended events stay outside an open view and are reported with `has_new_records`; explicit refresh opens a new view. A changed shipment version or invalidated source window returns HTTP 409 and requires refresh. Already loaded records remain visible. Refreshing stop history also refreshes the current shipment cards. This version fence is deliberately conservative: an unrelated shipment change can require a new stop-history view.

Source ordering is descending retained array position, rather than a claimed event-time sort. Corrected physical facts retain their original rows and correction links. Native event records are immutable after recording. Appointments can acquire cancellation/replacement state under the shipment version fence. Unrecorded out-of-band edits that preserve the checked metadata are outside the cursor guarantee.

The server derives the tenant from verified credentials. Existing staff read capability is required; carrier/customer credentials cannot use this endpoint. Cursor scope does not authorize access. Responses carry the verified tenant, subject and role and prohibit caching. The browser sends its reviewed access owner, the gateway checks that owner against the captured credential, and component/session generations prevent a delayed reply from entering another workspace. Credential denial closes the staff workspace.

The new frontend requires the matching history API. Deploy the API before the frontend, and reverse the source pair together when rolling back. Older clients can continue using the complete workspace endpoint. No storage downgrade is required.

## Storage and resource limits

Storage remains format 18/migration 023. No source rows or integration wire contracts are changed. PostgreSQL selects at most one page plus a lookahead row across the SQL/application boundary, but still decompresses and scans the aggregate JSONB collection. It validates collection/identity metadata globally and fully decodes only returned records and the lookahead. A successful page does not certify every unreturned record's complete model.

Database reads, decoding, projection and JSON encoding retain read admission through actual worker completion, including when the requesting HTTP task disappears. Encoding runs off the async executor. Memory-mode history uses borrowed source references and serializes only selected records; it still scans the source arrays.

Page sizes bound record counts, not arbitrary original record byte sizes. Loaded pages accumulate while a history view remains open. The main workspace still loads the aggregate and returns other current/retained collections; the separate [shipment catalog](load-catalog.md) remains the indexed list boundary. Normalized history storage, allocation attribution, sustained concurrent dense reads and large-history browser task measurements remain further work.

## Measurement and qualification

Release API measurements below are milliseconds, median of seven sequential reads, with observed minimum–maximum. Response bytes are uncompressed JSON; sampled RSS includes warmup. These are same-current-binary complete versus deferred modes, not a claim about all historical code performance. Complete mode runs first; PostgreSQL is shared between modes and can retain warmed caches. The order is not counterbalanced, so these timings are not an isolated causal estimate.

| Shipments / depth | Complete ms               | Deferred ms               | Complete bytes | Deferred bytes | Complete / deferred peak RSS MiB |
| ----------------- | ------------------------- | ------------------------- | -------------- | -------------- | -------------------------------- |
| 100 / 4           | 80.6 [75.9–86.8]          | 73.7 [69.6–82.7]          | 2,457,854      | 934,676        | 80.4 / 56.1                      |
| 1,000 / 4         | 869.1 [829.8–1,076.6]     | 720.2 [679.9–779.8]       | 24,672,407     | 9,353,342      | 674.7 / 458.9                    |
| 1,000 / 16        | 1,483.3 [1,435.8–1,641.4] | 1,091.5 [1,049.4–1,122.2] | 47,460,407     | 9,386,343      | 972.5 / 608.1                    |

The review independently verifies 24 original-history pages containing 888 returned records. Selected first/last shipment histories are complete; global events are explicitly limited to two validation pages per fixture. Browser checks exercise appointment pagination across 25+5 originals, changed-version refresh with retained rows and refreshed current cards, different browser/facility time zones, reviewed-owner denial and late success/denial isolation across sign-out.

The measurement driver is `scripts/profile-deferred-history.py`. It uses the same fixed-clock 100/4, 1,000/4 and 1,000/16 shipment/history fixtures as the [preceding full-workspace comparison](workspace-history-performance.md). Each mode uses a fresh release API process, one warmup and seven sequential timed requests against owned PostgreSQL 16. The default complete response must match the preceding normalized complete hash, removing only `operations.generated_at`. The compact response must match every remaining field after removing exactly the three history collections and checking the new descriptor. All response bytes and fixture bytes are archived locally with hashes; physical row revisions and retained JSON checksums must stay unchanged.

Original-history checks run outside timing samples. They retrieve all pages of events, appointments and gates for the first and last shipment, and the first two global event pages, comparing every returned record and original position to the fixture. These finite shared-host observations do not establish an SLA, production capacity, byte bounds or operator outcomes.