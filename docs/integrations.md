# StateSet integration runbook

## Source evidence

The adapters were traced against these local checkouts:

| Repository | Source revision | Contract inspected |
| --- | --- | --- |
| `stateset-edi` | `fc154836b2e8925fbd14272f52324ddc1b6da540` | `src/http/routes/outbound.rs`, `src/http/routes/document_ops.rs`, `src/callbacks.rs`, `migrations/0060_customer_callbacks.sql`, `src/state.rs`, `src/translate.rs`, X12 freight transaction codecs |
| `stateset-sync-server` | `1e406933c888b1c7efa4253174d72b08d3a2531a` | `src/http/router.rs`, `src/http/handlers/orders.rs`, `src/http/handlers/fulfillment.rs`, `src/domain/mod.rs`, `src/http/response.rs`, `src/http/pagination.rs`, `src/orchestrators/order_sync/mod.rs`, `src/database/orders.rs::update_order_tracking` |

The checked-out working files, including local changes, are the authoritative evidence for this implementation; these commit IDs identify the baseline, not a guarantee that every file equals HEAD. The optional contract harness compiles the actual checked-out EDI codec rather than a copied model.

`stateset-app` informed Next.js/React conventions. `next-temporal-rs` informed Rust/tenant/outbox conventions; no Temporal worker is deployed by freight. `stateset-icommerce` reinforced exact money and separating operational completion from payment settlement (`models/cart.rs`, `models/invoice.rs` in `stateset-core`).

## Configure per-tenant connections

`FREIGHT_INTEGRATION_CONCURRENCY` bounds simultaneous tenant cycles per API process, default two and range 1–16. A tenant runs one cycle at a time; ready tenants rotate while another tenant waits on its partner. Tender expiry runs independently of those network calls. See [scheduling, lease behavior and limits](tenant-integration-workers.md).

SIGTERM/SIGINT stop new background cycles and allow admitted work to finish within `FREIGHT_SHUTDOWN_TIMEOUT_MS` (default 30000). An unfinished handoff remains uncertain under its original lease; retain original request keys when retrying. See [process drain and restart](process-drain.md).

Each background dispatch turn considers up to four eligible outbound jobs and retains a process-local original-job bookmark for the next turn. Source outages cannot cause that phase to visit every candidate before returning its slot. Per-shipment/destination ordering and current source grants still govern every claim. See [bounded dispatch turns](bounded-dispatch-turns.md).

Example, with placeholder credentials:

```dotenv
FREIGHT_INTEGRATIONS_JSON='{"acme":{"edi_url":"https://edi.example.com","edi_token":"replace-with-tenant-scoped-edi-key","edi_tenant_id":"acme","callback_keys":{"1":"replace-with-a-random-secret-of-at-least-32-characters"},"sync_url":"https://sync.example.com","sync_api_key":"replace-with-sync-tenant-api-key","sync_tenant_id":"00000000-0000-4000-8000-000000000901"}}'
```

Use the Sync server's canonical tenant UUID for `sync_tenant_id`; the Freight workspace key (for example `acme`) is a separate namespace. Order IDs also come from the canonical Sync UUIDs. Use upstream tenant-scoped credentials. An EDI platform admin credential also works with `x-tenant-id`, but grants a larger upstream authority. The API refuses malformed URLs, non-local cleartext destinations, mismatched URL/credential pairs, missing upstream tenant IDs, or callback secrets shorter than 32 characters. Redirects are disabled. Configuration is operator-owned environment data; visitors cannot supply arbitrary upstream URLs.

Native iCommerce can replace Sync as a tenant’s order source, with typed order import and explicit versioned shipment steps. See [native iCommerce configuration and recovery](icommerce.md).

## Ownership

| Data/action | Authority |
| --- | --- |
| Commerce order and order identity | Configured Sync server or native iCommerce |
| Freight plan, buy/sell agreement, assignments, exception handling | freight |
| X12 validation, envelope/control numbers, partner transport, acknowledgments | EDI gateway |
| 990 verdict, 214 reported movement, 210 invoiced amount | Carrier evidence through EDI |
| Delivery POD reference, invoice approval, settlement preparation | Freight operator with supporting evidence |
| Freight customer billing/subledger/reconciliation | Freight tenant transaction; cash records describe externally cleared transactions |
| Bank execution/external GL posting | External finance/payment system; not executed here |

[Customer amendments](customer-amendments.md) use a separate customer/offer acceptance and safe carrier release before activating the new base plan/rates on the same shipment. They do not emit an electronic change to the old tender. A subsequent tender uses a fresh carrier shipment reference and the amended 204 plan while retaining the original BOL/order identity and all historical responses.

The [shared execution timeline](execution-timeline.md) also holds final Sync payloads with contradictory execution times. New payloads carry `freight_execution_consistent`; legacy payloads are checked against current facts. False snapshots stay local, and a later consistent correction can send a fresh snapshot. [Reviewed native movement corrections](movement-corrections.md) add `freight_actual_milestones`, containing only kind/effective actual UTC time/corrected flag. New native reports are included in their own transaction’s snapshot; correction snapshots preserve physical status/location/tracking watermark and original order identity. Invalid current correction evidence also holds final writeback. Staff proof remains private; custom milestone metadata lives in Sync’s raw tracking JSON outside its typed canonical projection. Actual EDI processing timestamps remain distinct from timezone-qualified stop observations.

## Outbound 204

This adapter transmits two-stop tenders only. [Multi-stop routes](multi-stop.md) retain a `native_only` outbox record and require the full carrier document/portal workflow. The actual sibling model ignores intermediate metadata and emits no S5 stops; retry and worker selection cannot bypass the route hold. A final 214 D1 remains physical delivery evidence but missing intermediate confirmations hold finance and final Sync writeback. A later complete-route payload can proceed through the original order's tracking endpoint.

Creating a tender commits the load transition and outbox request together. The dispatcher sends:

```http
POST /v1/edi/outbound/204/{partner_id}
Authorization: Bearer <edi credential>
x-tenant-id: <upstream tenant>
Idempotency-Key: <stable freight outbox ID>
Content-Type: application/json
```

The body follows the gateway's actual `Tender204`: shipment reference, SCAC, payment method, pickup date, BOL reference, weight, ship-from/to parties, and **at least one order**. Imported orders use the original order number; standalone loads use their freight reference as the freight order reference. Order quantity is one shipment, not a claim about SKU-level order units.

Both ship-from/to parties include the plan's street lines, postal code, and country through the existing N3/N4 model. New tenders reject missing street/postal data and invalid wire text. Appointment windows, operating contacts/instructions, and gate evidence stay in Freight; this gateway model does not transmit those fields. Coordinate appointments directly with the facility/carrier. The separate [carrier rate confirmation](carrier-rate-confirmations.md) freezes and exposes the price, full stop instructions, and timezone windows for your actual operating channel; the 204 remains unchanged. See [stop execution](stops.md).

Each new attempt uses an independent uppercase `SF-…` shipment identity in the 204. Partners must echo that identity in 990/214/210 shipment references. The original freight reference remains the BOL, and source commerce IDs/order numbers remain unchanged. Re-tendering to the same carrier creates a new identity. Historical partner/SCAC validation uses the immutable attempt snapshot. Legacy assignments retain their existing load-reference shipment identity and are explicitly marked as having uncertain transmission history.

The inspected codec's `Tender204` has no purpose/cancellation field and its builder emits `B2A*00` originals. Freight therefore supports recorded external carrier release rather than fabricating an electronic cancellation. Only a proven never-communicated offer can be withdrawn locally: EDI transmission history and separately recorded carrier rate-document handoffs constrain withdrawal, as do explicit portal publications. [Native carrier portal decisions](partner-portal.md) bind the exact published tender/rate hash; a later matching 990 confirms that state without duplicating the decision or posting finance. For a transmitted or accepted assignment, request release, contact the carrier outside Freight, and record its confirmation before re-planning/cancelling. Pending release pauses new handoffs/dispatch while preserving the current owner. [Release safety, late evidence, and operator steps](tenders.md).

The gateway requires PostgreSQL for outbound idempotency. A successful X12 response becomes `gateway_accepted`. Transport receipts/997 reconciliation remain owned by EDI; the dashboard deliberately does not call this `delivered`. A 990 yields `carrier_responded` and updates the business lifecycle.

Requests use a 20-second timeout, 60-second leases, exponential backoff, eight attempts before `held`, and the original key on retries. If a worker crashes after upstream acceptance, another worker reclaims the lease and reuses the same upstream key. Upstream EDI idempotency must be enabled. Credentials and raw error bodies are not copied into user-facing error messages.

Before an HTTP attempt, the outbox durably changes `transmission` to `attempted` and increments cumulative `total_attempts`. Operator retry never clears these markers. Missing legacy markers deserialize to `unknown`, which cannot prove an offer unsent. Release confirmation rejects a live handoff lease; an expired lease can be fenced, leaving its transmission outcome explicitly uncertain. A delayed response from that worker cannot change a newer tender.

## Signed callbacks

Register a callback subscription in EDI using its existing tenant-admin API:

```http
PUT /v1/callbacks/freight
Authorization: Bearer <edi tenant credential>
Content-Type: application/json

{
  "expected_version": 0,
  "url": "https://freight.example.com/webhooks/edi/acme",
  "secret": "same-secret-as-callback_keys-version-1",
  "partner_id": null,
  "enabled": true
}
```

Use the actual expected version when updating an existing subscription. EDI must have PostgreSQL and its connection encryption key configured. Route `/webhooks/edi/*` to the **Rust API**, and the dashboard routes to Next.js.

The callback is a `document.recorded` notification, **not** a normalized shipment event. Its headers are `x-stateset-event-id`, `x-stateset-timestamp`, `x-stateset-key-version`, and `x-stateset-signature`. Freight verifies HMAC-SHA256 over `timestamp.event-id.exact-body-bytes`, rejects timestamps outside five minutes, and selects the configured secret by key version. Keep old key versions configured until their queued callbacks settle.

For accepted inbound 990/214/210 documents, freight commits the original notification and its signature metadata to the tenant's inbound queue before returning success. The callback request does not contact EDI. A worker then fetches `GET /v1/documents/{document_id}`, validates metadata against the callback, reads `data.parsed`, and converts it to a freight event. Other document types/directions are acknowledged as ignored. A durable receipt keyed by document identity prevents replayed callbacks from repeating a transition or invoice. Conflicting contents for an existing document identity are rejected. Partner/SCAC checks prevent an unrelated carrier from changing a load.

The inbound queue retains the exact original body, original signing timestamp/delivery ID/key version/signature, all delivery identities, canonical event, processing result, and attempt journal. Incoming acceptance (`queued`) is separate from business completion (`applied`). Transient failures retry with backoff; after eight unsuccessful attempts the item is `held`. Operators inspect evidence and retry from Integrations. Retrying resets the backoff cycle but preserves total attempts and prior failures. Workers use a 60-second lease; completion checks its lease token so a recovered job cannot be overwritten by a worker whose lease expired. Business changes, normalized-event receipts, and the applied inbox state commit atomically.

An internal authenticated `POST /v1/events` also accepts normalized `edi.load_tender_response.received`, `edi.shipment_status.received`, and `edi.freight_invoice.received` events. Do not send a `document.recorded` notification there.

A retired attempt's 990 is retained/audited without affecting the current tender. Historical physical movement or a 210 instead creates a carrier incident and `reconciliation_required` inbound state. New tendering/dispatch, EDI handoffs, and finance evidence approval pause until operations records its investigation. Resolving the incident marks the inbox record `reconciled`, retaining its original body/event/history; it does not silently apply the old carrier's invoice or transfer custody. A current carrier pickup during a pending release advances the real movement and blocks the release for investigation.

214 support currently covers AF pickup, X6 in transit, X1 arrival, AG estimate, and D1 delivery. Every observation is processed in document order; a combined AF/D1 document can move an accepted shipment through pickup to delivery in one atomic commit. One invalid observation rejects the entire document without partial changes. Each observation's original date/time/location is preserved in structured activity evidence. An AG estimate does not move the physical last-reported location, and repeated earlier-status observations cannot regress a delivered shipment. A D1 does not fabricate a POD. Missing lifecycle prerequisites retain the document for investigation/retry. Older location-only events are acknowledged and audited without changing the load; delayed pickup/delivery evidence may advance the lifecycle while preserving the newest tracking watermark. The codec's local date/time fields do not provide a timezone-aware business-event clock; `occurred_at` is currently the document processing timestamp. Full carrier-specific clock reconciliation and additional status/reason mappings require carrier guide qualification.

The current 210 codec supplies a header net amount and references, not a certified L0/L1 accessorial breakdown. Freight matches that amount against the original buy plus separately documented/finance-approved carrier additions. It never infers a charge approval from the 210 total or a 214/gate duration. Approval of an additional agreement re-audits the actual received invoice without rewriting it. The optional real-codec harness covers a 132,551-cent 210 against base 125,000 plus approved 7,551. [Charge evidence, holds, and posting boundaries](charges.md).

An additional 210 after a primary already exists is retained for finance classification without posting another payable. Negative 210 totals are also retained for review, including before a primary exists; their original sign is preserved. Signed inbox items show `finance_review`, then `finance_reviewed` after classification/dismissal, retaining original signature/body/event/history. Finance can apply a canonical replacement to an unapproved primary, or bind it to a supplemental/revised-total adjustment after approval. Negative headers require explicit verified credit-memo classification; positive headers alone cannot establish credit designation. Exact source facts, carrier/tender identity, versioned decisions, and separate journals prevent silent duplication or rewriting of the approved invoice. The sixth real-codec test follows an additional 1,251-cent 210 and a negative N2 `-251` credit through this path. [Carrier documents, adjustments, and refunds](payables.md).

## Sync-server

The integration selects orders using:

```http
GET /v1/tenants/{tenant}/orders?page=1&page_size=100
x-stateset-api-key: <sync credential>
```

It reads the actual `data.items` and `data.pagination` response. Import fetches the selected order again by UUID, stores its canonical source snapshot, and links it to an operator-supplied freight plan. Duplicate order links are rejected. Source order addresses/rates are not guessed from inconsistent commerce payloads.

Pickup/location/delivery/POD changes, appointment/gate evidence changes, and assignment withdrawal/decline/release enqueue:

```http
POST /v1/tenants/{tenant}/orders/{order_uuid}/tracking
x-stateset-api-key: <sync credential>
Idempotency-Key: <stable freight outbox ID>
Content-Type: application/json

{"trackingData":{"trackingNumber":"FR-123","carrier":"Carrier name","estimatedDelivery":"2026-10-16","freight_status":"in_transit","location":"City, ST","pod_reference":null,"freight_version":4}}
```

`trackingNumber` and `estimatedDelivery` must use camelCase. Although the write endpoint accepts arbitrary JSON, canonical order reads deserialize it through `TrackingInfo` (`serde(rename_all="camelCase")`). Snake-case fields receive a successful write response but disappear from that canonical projection. The runtime harness reproduces this loss on a dedicated synthetic order and verifies that Freight's corrected fields survive a real write/read round trip. Sync's raw tracking JSON also retains Freight's additional fields below; its typed public projection does not expose those fields.

Newly generated Freight jobs use the corrected keys. Previously retained outbox payloads and upstream events remain original evidence; this change does not rewrite attempted jobs, receipt history, or real orders. If an older deployment sent snake-case tracking, inspect the canonical order and reconcile missing values explicitly rather than treating its HTTP success as proof of a correct read model.

The inspected `database/orders.rs::update_order_tracking` also unconditionally sets the source order to `SHIPPED`. Freight verifies canonical source state before physical tracking handoff and holds unsafe source statuses and cancellation payloads. Pre-pickup withdrawal/decline/gate facts remain `local_only` so the endpoint cannot prematurely mark an unshipped order shipped; later physical facts can still proceed. For the complete recovery policy, see [commerce source safeguards and reconciliation](source.md). It does not remotely cancel commerce orders or erase known completed/cancelled source state.

Success requires `data.ok=true`. The adapter does not use sync-server's EDI ingress: its current handler returns `action: ignored` for freight event types. Tracking updates are serialized per load/destination so a failed older write cannot later overwrite a newer confirmed state. The inspected sync implementation stores this tracking data and records a tracking event; it does not implement freight's detailed lifecycle or payment approval. Retrying may record an additional sync tracking event if that upstream endpoint does not deduplicate the provided key; business tracking data is an idempotent replacement.

Tracking additionally includes `freight_stops: {pickup: {appointment: {starts_at, ends_at, timezone} or null, arrival_at, departure_at}, delivery: {...}}`. Only current-attempt actual times are included. Contact details, evidence references, staff subjects, and internal notes remain local. The upstream `UpdateTrackingRequest.tracking_data` is a nonempty JSON value, so the original endpoint stores these additional fields without a new upstream route/schema. This does not trigger dock booking or detention charges.

For a stored POD, `pod_reference` contains the opaque `document:<freight_document_id>` reference. File bytes and finance reviews remain owned by Freight. The reference is not a public download URL and does not attest finance approval; downloads require a valid Freight credential/session for that tenant. External POD references are also passed as references without fetching their URLs.

Charge proposals/decisions and buy/sell agreement totals are local Freight financial data. They do not change commerce order totals, payment state, or sync tracking. The inspected 204 request model has no supported price-amendment field; record actual agreement with each counterparty through the operating channel rather than assuming an automatic EDI amendment.

## Recovery

All connector responses enforce a [2,000,000-byte streamed body budget](integration-response-budgets.md), including order listings, canonical documents and handoff acknowledgements. A declared oversized length fails immediately; missing/chunked lengths are checked while reading. Failed statuses are classified before reading error bodies. Oversized, truncated or malformed acknowledgements cannot confirm a handoff. Inspect the retained failure and retry the original work after correcting the upstream response; creating a replacement tender can duplicate an already attempted transmission.

- Inspect the integration handoff and error status. Correct credentials/upstream configuration before retrying a held request.
- For inbound documents, receipt means the work is retained locally. Inspect its failure and original evidence; reconcile missing carrier acceptance or other prerequisites, then use Retry. EDI outages after receipt do not require the sender to keep the work alive.
- Document processing timestamps are not a substitute for shipment chronology. A delayed AF/D1 may advance the lifecycle after its prerequisites exist, even behind the latest received tracking timestamp. The watermark never decreases; older location-only updates remain suppressed. Each applied observation retains its original date/time and reason fields for partner/timezone qualification.
- A failed sync write blocks later writes for that same load until resolved, preserving order. EDI and sync destinations do not block each other.
- A tender response overdue after two hours creates an exception. It never silently reallocates freight that a carrier may already have accepted.
- Withdraw/cancel locally only when Freight proves the offer was never attempted. Otherwise use a release request and record actual carrier confirmation before re-planning/cancellation. Pickup evidence blocks release; recording a request alone never removes ownership. Freight does not emit 204 cancellation messages.
- Decline, unsent withdrawal, and confirmed release supersede only that tender attempt's handoffs. A stale worker cannot overwrite a result belonging to a newer lease or tender.
- An inbound `reconciliation_required` record needs carrier investigation through the load's tender history, rather than a transport Retry. Resolve with the actual contact/reference and retain the original conflicting evidence.
- Back up PostgreSQL before upgrades; inbound documents, idempotency receipts, and handoff records are part of recovery state, not disposable cache.

Live credentials and partner certification evidence were not supplied. Contract tests and the actual codec qualification are reproducible; production partner transport acceptance remains an operational commissioning step.

## Operations recovery queue

The [control tower](control-tower.md) derives work from held inbound/outbound jobs, tender/release conflicts, source reviews, tracking freshness, and route/POD evidence. Claiming or deferring work never changes an integration job or releases its safety hold. Previously held jobs stay active during retry; source closure retains the complete case history. Staff notes/owners are private and excluded from Sync writeback, EDI payloads, and partner projections. The actual-service qualification verifies named recovery after signed D1 while complete-route Sync remains held until actual intermediate service.
