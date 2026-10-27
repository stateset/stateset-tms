# Multi-stop freight execution

Create or revise an ordered route with origin pickup, up to eighteen intermediate pickups/deliveries, and final delivery. Each intermediate stop has a stable `stop_` identifier, service kind, facility, street/postal address, planned date, contact, and instructions. The editor supports adding, removing, and reordering stops. Dates must follow route order. Tendering and quoting require complete addresses; drafts can retain incomplete addresses for later correction.

The route is part of the exact customer quote, carrier rate confirmation, release document, and customer billing snapshot. Revisions preserve the original documents and hashes. A two-endpoint lane contract cannot price intermediate stops: obtain an explicit carrier offer covering the entire route. A previously confirmed appointment prevents changing its stop details or route position until actual cancellation is recorded.

## Carrier agreement and EDI boundary

The current sibling EDI `Tender204` model represents origin and destination only. It ignores unknown intermediate-stop metadata and its builder emits no S5 route stops. Freight therefore retains these tenders as `native_only`, with zero transmission attempts. Operator retry and worker selection independently prevent a partial 204 handoff, including a job whose persisted queue status is incorrectly changed to pending. Ordinary two-stop 204s continue through the existing adapter.

Issue and explicitly publish the full carrier rate confirmation. The assigned carrier reviews the exact document/hash and accepts through the authenticated portal. An unsolicited 990 cannot establish initial acceptance of the missing full route. Publication establishes a real communication channel, so the tender cannot subsequently be withdrawn as proven unsent. Use the existing carrier release workflow if necessary. Retired offers retain their original route/document without exposing a replacement carrier's plan or execution.

## Actual intermediate service

After physical origin pickup, operations or the currently assigned authenticated carrier confirms each intermediate stop in order. Record the actual UTC service time, facility contact, evidence reference, and findings. Arrival alone does not establish unloading or additional pickup. Missing earlier service cannot be silently filled by confirming a later stop.

Each record binds the exact stop snapshot, current tender, authenticated actor, original recording time, and evidence digest. Carrier submissions additionally bind the carrier's publication. Replaying a successful request returns the original record. Stale versions, duplicates, wrong carriers, and out-of-order observations fail atomically. Operations can explicitly correct a record; corrections append a successor and retain every original. The entire correction chain must remain intact before service is treated as complete. Approved carrier invoices, prepared settlements, and nonvoid customer invoice drafts/issued invoices lock further service changes.

Every stop supports facility-confirmed appointment windows and actual gate observations. Window starts must follow route order in UTC. Known gate times follow the full route, and intermediate service must fall between known arrival and departure. Departure requires actual service confirmation. Corrections revalidate the known sequence and retain the original appointment baseline and facility snapshot. Unobserved arrivals/departures remain unknown. See [appointments and gates](stops.md).

The [shared execution timeline](execution-timeline.md) additionally enforces chronology across gates, intermediate service, and native origin/final movement. It supports explicit repair of older conflicts and holds finance/final Sync until all known times agree.

## Delivery, finance, and Sync

Native carrier reporting and the operations delivery command require every intermediate confirmation. A signed final EDI D1 remains physical evidence of final delivery even if intermediate confirmations are missing. Freight preserves that observation, displays a route attention item, and continues holding finance and final Sync delivery writeback. This supports late entry of actual intermediate evidence without erasing the original delivery fact.

All intermediate services and finance-reviewed consolidated POD are required for carrier invoice approval, settlement, and customer billing. Service confirmations do not create charges, approve invoices, or execute payments. Consolidated POD must be reviewed against the entire route; per-line item quantities, partial loss allocation, and a separate signed POD for every stop are not implemented.

For imported orders, the existing ordered Sync queue retains the original order UUID. Tracking includes minimal ordered route identity, stop kind/location/date, service completion, and `freight_route_complete`. Private contacts, findings, actors, and evidence references stay in Freight. An incomplete final-delivery payload becomes `local_only`; a later complete-route payload can proceed. Actual Sync canonical reads retain supported camelCase fields, while arbitrary route metadata remains in its raw tracking JSON.

## API and persistence

- Load/quote plans accept optional `intermediate_stops: [{id, kind: "pickup"|"delivery", stop: {...}}]`; `stop` uses the existing stop shape. Empty arrays are omitted from persisted commercial snapshots to preserve legacy document bases.
- `POST /v1/loads/{id}/stop-services` requires `operations.write`.
- `POST /v1/stop-services/{id}/correct` requires `operations.write` and targets the current original/successor.
- `POST /v1/portal/offers/{id}/stop-services` requires `portal.carrier.report`, current carrier ownership, and current publication.
- Each request requires an `Idempotency-Key` and `{expected_load_version, stop_id, observed_at, contact, reference, note}`. The server derives the assignment, actor, recording time, and digest. Contact/reference/findings require 1–200 characters.
- Workspace responses include immutable `stop_services`, per-load `itinerary`, and `route_attention`. Carrier projections expose only current route completion/times plus that carrier's own submitted history.

Multi-stop plans were introduced by storage format 14 and migration 016. Format 15 adds shared execution chronology and final-tracking holds; format 16 adds retained native movement corrections and effective-time validation/writeback. Current format 17 adds source-derived operations work with retained ownership/follow-up history. These migrations preserve existing business fields, files, timestamps, and checksummed migration history. PostgreSQL tenant locking commits service evidence, load version, audit, receipt, and Sync enqueue together. See [storage deployment](storage.md).