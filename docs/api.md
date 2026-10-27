# Freight API

The workspace root includes an `operations` summary with source-derived counts, response sample denominators, evidence status, and tracking policy. It omits issue histories. [Control tower API](control-tower.md#access-and-api) documents paginated filtered lists and separate complete history reads. Case actions bind the exact reviewed version, source digest, active/cleared state, and authenticated subject; source changes return 409. Ownership never modifies physical shipment or financial state.

Each staff load includes `execution_timeline: {consistent, evidence_valid, record_count, conflicts, stops}`. Facts expose kinds, effective actual UTC times, source, references, and actors; corrected native facts additionally retain `original_observed_at` and the private current correction. Staff `movement_sources` contains exact original reports/digests and full correction history; the workspace root includes `movement_corrections`. Current carrier offers and customer shipments receive original/effective times and a correction marker without staff proof/identity fields. Gate/service/native tracking and correction requests share one chronology validator. Conflicts or invalid correction evidence feed `route_attention` and hold delivery readiness/final Sync writeback. See [shared execution timeline](execution-timeline.md) and [movement correction request/evidence](movement-corrections.md#atomic-source-bound-correction).

All `/v1/*` routes require `Authorization: Bearer <service credential or Clerk session JWT>`. Static service credentials fix their tenant/subject/role. Staff JWTs must pass the configured Clerk signature/issuer/origin/time checks and live session/membership verification; server-owned organization and role mappings fix the canonical Freight tenant and capabilities. Mutations use an `Idempotency-Key` of 1–200 characters; the [original-save resolver](save-resolution.md) instead identifies the original request with `request_key` in its body. Replaying the same logical request returns its original result; changing the request under that key returns 409. Domain validation errors return 422, missing entities 404, and stale-version conflicts 409. Mutation responses use `{data, replayed}`; the workspace endpoint returns the workspace object directly.

[Carrier contract API](carrier-contracts.md#api) adds agreement creation/revision/withdrawal and exact original statements under `/v1/carrier-contracts`; `/v1/quotes/{id}/contract-rates` evaluates applicability, and `/contract-offers` or `/contract-reprocure` records fresh capacity at the server-derived inclusive price. Staff workspace `carrier_contracts` excludes HTML; carrier offers/awards retain optional exact `rate_source`. Partner projections exclude the private catalogue and agreement evidence.

A role without permission receives 403 before mutation, replay, or business-upstream access. Staff authentication consults Clerk before granting business access; provider failure returns a generic 503 and cannot fall back to a stale membership. Pending/invalid/revoked sessions receive 401; missing/unmapped memberships receive 403. The SSO gateway sends `X-Freight-Identity-Mode: clerk` to prohibit static credential fallback, and `X-Freight-Organization-Id` to guard the organization that rendered a request/download. A mismatched signed organization is rejected. Neither header can grant identity or a tenant. Workspace responses include `access: {role, subject, capabilities}` for the current principal. Staff audit subjects are `clerk:<user ID>`. See [SSO configuration and verification](sso.md), [access controls and the role matrix](access.md).

| Method | Route                                                    | Purpose                                                                                                                                                                                                               |
| ------ | -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GET    | `/health`                                                | Process/storage readiness, including supported database format; incompatible storage returns 503; no credentials required                                                                                             |
| GET    | `/v1/console` | Staff bounded Billing/Payables/Queues projection; required `view`, optional `limit`, `cursor`, `q`, financial `balance` or queue `attention`/`direction`; workspace totals |
| GET    | `/v1/operations`                                         | Staff queue; optional `view`, `search`, `limit`, `cursor`; source facts, summaries, metrics, policy                                                                                                                   |
| GET    | `/v1/operations/issues/{id}`                             | Current condition and complete retained case/history within the authenticated tenant                                                                                                                                  |
| POST   | `/v1/operations/issues/{id}/actions`                     | `operations.write`; reviewed `{expected_version, expected_source_active, source_sha256, action, note, follow_up_at?, defer_until?}`; administrator required for `force_release`                                       |
| POST   | `/v1/operations/reconcile`                               | `operations.write`; body exactly `{}`; reconcile authoritative sources without changing their business facts                                                                                                          |
| POST   | `/v1/operations/policy`                                  | Administrator `operations.configure`; `{expected_version, tracking_stale_minutes, note}`                                                                                                                              |
| POST   | `/v1/loads/{id}/source/refresh`                          | Operations fetches canonical Sync order; body exactly `{}`; [source check/review shapes](source.md#api)                                                                                                               |
| POST   | `/v1/source-reviews/{id}/resolve`                        | Operations records version/digest-fenced customer confirmation or safe cancellation closure                                                                                                                           |
| POST   | `/v1/quotes`                                             | Operations creates a customer quote for a new or explicitly bound planned/Sync-imported shipment; [quote request shapes](quotes.md#api)                                                                               |
| POST   | `/v1/quotes/{id}/amendments`                             | Propose a separately accepted replacement customer plan/base agreement; original remains effective until activation; [request/consent shapes](customer-amendments.md#api)                                             |
| POST   | `/v1/quotes/{id}/revise`                                 | Retain the prior revision and draft a new lane/price/terms revision                                                                                                                                                   |
| POST   | `/v1/quotes/{id}/offers`                                 | Record actual carrier price/contact/reference/validity against the draft revision                                                                                                                                     |
| POST   | `/v1/quotes/{id}/offers/withdraw`                        | Retain actual carrier offer retraction evidence on an unbooked quote                                                                                                                                                  |
| POST   | `/v1/quotes/{id}/issue`                                  | Select a qualified live offer and freeze the customer quote/issuer                                                                                                                                                    |
| POST   | `/v1/quotes/{id}/decision`                               | Record actual customer acceptance/decline/withdrawal; retains decision history                                                                                                                                        |
| POST   | `/v1/quotes/{id}/book`                                   | Atomically book exact accepted rates on one new or existing planned shipment; existing source requires its current version                                                                                            |
| POST   | `/v1/quotes/{id}/reconfirm`                              | Confirm the same current carrier/rate with new validity and retained evidence                                                                                                                                         |
| POST   | `/v1/quotes/{id}/reprocure`                              | Record replacement carrier procurement after safe withdrawal/release; retains customer sell agreement and original awards/tenders                                                                                     |
| GET    | `/v1/quotes/{id}/statement?revision=N`                   | Reader-accessible customer document; quote-price/lane/terms only; omits private buy/procurement evidence                                                                                                              |
| POST   | `/v1/loads/{id}/cancellation-fees`                       | Propose an explicitly agreed carrier/customer fee after cancellation; `charges.write`; [request shapes](cancellations.md#api)                                                                                         |
| POST   | `/v1/cancellation-fees/{id}/review`                      | Finance approves/rejects a fee with actual counterparty evidence; approval creates its separate payable/customer invoice                                                                                              |
| POST   | `/v1/cancellation-fees/{id}/reverse`                     | Finance retracts a fee with confirmation/current target version, retains bank history, and leaves actual refunds/credits for reconciliation                                                                           |
| POST   | `/v1/loads/{id}/rate-confirmations`                      | Issue exact-tender frozen carrier rate/stop document; operations only; [request/evidence shapes](carrier-rate-confirmations.md#api-evidence-and-access)                                                               |
| GET    | `/v1/rate-confirmations/{id}/statement`                  | Original HTML bytes and SHA-256; tenant readers                                                                                                                                                                       |
| POST   | `/v1/rate-confirmations/{id}/handoffs`                   | Record actual external carrier communication; current doc/load versions, contact/reference/channel/time, optional file                                                                                                |
| POST   | `/v1/rate-confirmations/{id}/handoffs/retract`           | Retain/correct erroneous communication evidence; does not recall a message                                                                                                                                            |
| POST   | `/v1/rate-confirmations/{id}/handoffs/resolve`           | Record actual carrier coordination against an old/changed handoff; retains current ownership                                                                                                                          |
| POST   | `/v1/documents/{id}/shares`                              | Explicit named customer/carrier-tender file access; checked versions, content review and original bytes; [sharing rules](documents.md#sharing-api-and-persistence)                                                    |
| POST   | `/v1/document-shares/{id}/revoke`                        | Stop future grant authorization, retain file/evidence/history; current grant version                                                                                                                                  |
| GET    | `/v1/portal/files/{id}/content`                          | Scoped original file; atomic current-policy/byte check and download-authorization audit                                                                                                                               |
| GET    | `/v1/portal`                                             | Server-scoped carrier offers or customer shipments/issued invoices; partner roles only; [scope/configuration](partner-portal.md)                                                                                      |
| POST   | `/v1/quotes/{id}/portal-publications`                    | Operations explicitly publishes an issued customer revision; frozen exact HTML/hash                                                                                                                                   |
| POST   | `/v1/portal/customer-quotes/{id}/response`               | Customer accepts, declines, or withdraws its exact publication/version; no booking/custody effect                                                                                                                     |
| POST   | `/v1/portal/customer-quotes/{id}/source/refresh`         | Customer verifies canonical source for its current issued publication; scoped status summary only                                                                                                                     |
| GET    | `/v1/portal/customer-quotes/{id}/statement`              | Customer's own immutable published quote bytes/hash, including historical revisions                                                                                                                                   |
| POST   | `/v1/loads/{id}/portal-offers`                           | Staff operations publishes the exact current carrier document; counts as communication                                                                                                                                |
| POST   | `/v1/portal/offers/{id}/response`                        | Authenticated carrier accepts/declines exact published tender/rate hash with current version                                                                                                                          |
| POST   | `/v1/portal/offers/{id}/tracking`                        | Current scoped carrier reports physical pickup/location/delivery with observed time and retained proof                                                                                                                |
| POST   | `/v1/carrier-submissions/{id}/time-corrections`          | Operations appends an exact-source reviewed native pickup/delivery time correction; preserves original report and physical state; current version/predecessor and financial locks apply                               |
| POST   | `/v1/portal/offers/{id}/pod`                             | Raw original carrier POD upload and atomic unreviewed attachment after delivery; strict query fields and 10 MiB bound                                                                                                 |
| GET    | `/v1/portal/submissions/{id}/content`                    | Exact own carrier-submitted POD download; no generic staff files or other counterparties                                                                                                                              |
| GET    | `/v1/portal/rate-confirmations/{id}/statement`           | Own carrier publication only; exact original HTML/hash                                                                                                                                                                |
| GET    | `/v1/portal/bills/{id}/statement`                        | Own customer issued invoice only; drafts/other counterparties return 404                                                                                                                                              |
| GET    | `/v1/workspace`                                          | Tenant loads, document metadata, master data, invoices, settlements, events, inbound queue, handoffs, connection state                                                                                                |
| POST   | `/v1/finance/profile`                                    | Billing issuer: `{expected_version, legal_name, address, contact, payment_instructions}`; first expected version is 0                                                                                                 |
| POST   | `/v1/bills`                                              | Customer invoice draft: `{load_id, number, due_date, bill_to_address?}`; amount derives from the sell contract                                                                                                        |
| POST   | `/v1/bills/{id}/plan`                                    | Draft revision: `{expected_version, number, due_date, bill_to_address?}`                                                                                                                                              |
| POST   | `/v1/bills/{id}/supplements`                             | Draft a separate documented customer supplement against an issued original invoice; see shape below                                                                                                                   |
| POST   | `/v1/bills/{id}/issue`                                   | Issue/freeze snapshot and post receivable: `{expected_version}`                                                                                                                                                       |
| POST   | `/v1/bills/{id}/void`                                    | Void/reverse issued billing: `{expected_version, note}`; requires zero net retained funds and no active credits                                                                                                       |
| GET    | `/v1/bills/{id}/statement`                               | Tenant-scoped printable HTML attachment; buy cost/internal finance notes excluded                                                                                                                                     |
| POST   | `/v1/bills/{id}/credits`                                 | `{expected_version, number, amount_cents, reason, issued_on?, confirmation?}`; caps total active credits at original amount; actual confirmation required when reducing received funds                                |
| POST   | `/v1/credits/{id}/reverse`                               | `{expected_version, expected_bill_version?, reversed_on, note}`; current bill version required after refund history; append reversing journal                                                                         |
| POST   | `/v1/cash`                                               | Record external receipt/payout/customer_refund/carrier_refund and exact allocations; see example below                                                                                                                |
| POST   | `/v1/cash/{id}/reverse`                                  | `{expected_version, reversed_on, note}`; reverses bookkeeping, does not reverse a transfer                                                                                                                            |
| POST   | `/v1/carrier-credit-applications`                        | Record confirmed same-carrier credit allocations across prepared invoices; strict shape below                                                                                                                         |
| POST   | `/v1/carrier-credit-applications/{id}/reverse`           | Reverse with fresh carrier confirmation and current source/target versions; retains original proof                                                                                                                    |
| GET    | `/v1/finance/journal.csv`                                | Balanced freight subledger export with source, reversal, and load IDs                                                                                                                                                 |
| GET | `/v1/loads/{id}/documents` | Staff evidence catalog: newest 100 documents for the exact tenant shipment, metadata only, with `total`; private/no-store. |
| GET | `/v1/documents/{id}/preview` | Staff PNG/JPEG preview: bounded RGB8 derivative with source/content hashes and dimensions; private/no-store; original and review state unchanged. |
| POST   | `/v1/loads/{id}/documents?kind=pod&filename=receipt.pdf` | Raw PDF/JPEG/PNG body, up to 10 MiB; content type must match; idempotent immutable upload                                                                                                                             |
| GET    | `/v1/documents/{id}/content`                             | Authenticated exact-byte attachment with digest and no-store headers                                                                                                                                                  |
| POST   | `/v1/documents/{id}/review`                              | Finance review: `{expected_version, decision: "accepted" or "rejected", note}`                                                                                                                                        |
| POST   | `/v1/loads/{id}/pod/review`                              | Finance review of the current external POD reference; same review shape; expected load version                                                                                                                        |
| POST   | `/v1/loads`                                              | Create a freight plan                                                                                                                                                                                                 |
| POST   | `/v1/loads/{id}/plan`                                    | Revise an untendered plan: `{expected_version, plan}`                                                                                                                                                                 |
| POST   | `/v1/loads/{id}/commands`                                | Execute a version-checked lifecycle action                                                                                                                                                                            |
| POST   | `/v1/loads/{id}/charges`                                 | Propose documented buy/sell addition or explicit correction; `charges.write`                                                                                                                                          |
| POST   | `/v1/loads/{id}/carrier-adjustments`                     | Finance draft: supplemental invoice, credit memo, or revised total against an approved invoice                                                                                                                        |
| POST   | `/v1/carrier-adjustments/{id}/review`                    | Finance approve/post or reject a draft                                                                                                                                                                                |
| POST   | `/v1/carrier-adjustments/{id}/reverse`                   | Append opposite posting; retain original approval                                                                                                                                                                     |
| POST   | `/v1/carrier-documents/{id}/dismiss`                     | `{expected_load_version, expected_version, note}`; retain an inapplicable canonical 210                                                                                                                               |
| POST   | `/v1/carrier-documents/{id}/apply-revision`              | Same decision shape; apply source number/amount to an unapproved primary and re-audit                                                                                                                                 |
| POST   | `/v1/charges/{id}/review`                                | Finance approve/reject a proposal; strict versioned decision                                                                                                                                                          |
| POST   | `/v1/charges/{id}/void`                                  | Finance retract approval before its side posts; original decisions retained                                                                                                                                           |
| POST   | `/v1/loads/{id}/appointments`                            | Record/replace a facility-confirmed stop window; shape below                                                                                                                                                          |
| POST   | `/v1/appointments/{id}/cancel`                           | `{expected_load_version, reason}`; retains original booking/proof                                                                                                                                                     |
| POST   | `/v1/loads/{id}/stop-events`                             | Record observed gate arrival/departure; shape below                                                                                                                                                                   |
| POST   | `/v1/stop-events/{id}/correct`                           | Append a gate correction; shape below                                                                                                                                                                                 |
| POST   | `/v1/loads/{id}/stop-services`                           | Operations confirms actual intermediate service; exact current version/stop/time/contact/reference/findings                                                                                                           |
| POST   | `/v1/stop-services/{id}/correct`                         | Operations appends a service correction retaining the original evidence                                                                                                                                               |
| POST   | `/v1/portal/offers/{id}/stop-services`                   | Current assigned carrier confirms actual intermediate service through its scoped publication                                                                                                                          |
| POST   | `/v1/loads/{id}/release-requests`                        | Request carrier release: `{expected_load_version, disposition: "replan" or "cancel", reason}`                                                                                                                         |
| POST   | `/v1/release-requests/{id}/confirm`                      | Record externally confirmed carrier release with the confirmation body below                                                                                                                                          |
| POST   | `/v1/release-requests/{id}/retain`                       | Record externally confirmed retention of the existing carrier with the confirmation body below                                                                                                                        |
| POST   | `/v1/tender-incidents/{id}/resolve`                      | Record investigation of a carrier conflict; current ownership is retained; same confirmation body                                                                                                                     |
| POST   | `/v1/customers`                                          | Create customer: `name`, `contact`, optional `billing_address`                                                                                                                                                        |
| POST   | `/v1/carriers`                                           | Create carrier: `name`, `scac`, `mc_number`, `insurance_expires`, `equipment`, `partner_id`                                                                                                                           |
| POST   | `/v1/drivers`                                            | Create driver: `name`, `phone`, `license_expires`                                                                                                                                                                     |
| POST   | `/v1/equipment`                                          | Create equipment: `unit`, `kind`, `plate`                                                                                                                                                                             |
| POST   | `/v1/{carriers,customers,drivers,equipment}/{id}`        | Edit a record with `{expected: <previous record>, data: <new fields>}`; rejects stale edits                                                                                                                           |
| POST   | `/v1/invoices`                                           | Receive/audit: `number`, `load_id`, `amount_cents`, `currency`                                                                                                                                                        |
| POST   | `/v1/invoices/{id}/resolve`                              | Correct from revised carrier invoice: `expected_amount_cents`, `amount_cents`, `note`                                                                                                                                 |
| POST   | `/v1/invoices/{id}/entry-review`                         | Finance records `{expected_revision, evidence_reference, note}` against the actual carrier invoice; posts no money                                                                                                    |
| POST   | `/v1/invoices/{id}/approve`                              | Independent named finance user submits `{expected_revision, note}` after entry review; matched/delivered invoice and reviewed POD required; stored bytes verified before commit. Explicit synthetic demo accepts `{}` |
| POST   | `/v1/loads/{id}/settlement`                              | Prepare settlement for approved invoice; body `{}`                                                                                                                                                                    |
| POST   | `/v1/events`                                             | Ingest supported normalized EDI event; event ID is the deduplication key                                                                                                                                              |
| GET    | `/v1/sync/orders?page=1`                                 | Read upstream sync order candidates                                                                                                                                                                                   |
| POST   | `/v1/sync/import`                                        | `{order_id, load: <freight plan>}`; re-fetch source and atomically link a new load                                                                                                                                    |
| GET | `/v1/commerce/orders?page=1` | Read validated native iCommerce order candidates; operations role required |
| GET | `/v1/commerce/orders/{id}` | Read a normalized typed native order snapshot |
| POST | `/v1/commerce/import` | `{order_id, load}`; fetch native source server-side and atomically import the explicit freight plan |
| GET | `/v1/loads/{id}/commerce-shipments?page=1` | Operations-only native shipment discovery for the retained load/order; 25 rows per live page, explicit eligibility reasons and active handoffs; accepts only page |
| GET | `/v1/loads/{id}/commerce-tracking?shipment_id=UUID` | Preview one legal native shipment status step, manifest and expected versions |
| POST | `/v1/loads/{id}/commerce-tracking` | `{expected_load_version, shipment_id, expected_shipment_version}`; queue the reviewed native shipment step with a durable original receipt |
| GET | `/v1/outbox/{id}/commerce-review` | Operations previews external native advancement for a held handoff; original identity, manifest, binding and newer version are required |
| POST | `/v1/outbox/{id}/commerce-review` | `{expected_load_version, expected_total_attempts, expected_shipment_version, contact, reference, note}`; record external review and retire only the local handoff with a durable original receipt; no native PATCH |
| POST   | `/v1/outbox/{id}/retry`                                  | Requeue held/retrying integration request; body `{}`                                                                                                                                                                  |
| POST   | `/v1/inbox/{id}/retry`                                   | Requeue retained failed inbound EDI document; body `{}`; lease ownership cannot be overridden                                                                                                                         |
| POST   | `/webhooks/edi/{local_tenant}`                           | Signed EDI document notification; uses callback headers instead of bearer auth                                                                                                                                        |

Freight plan example (dates must be actual planned dates):

```json
{
  "reference": "FR-1001",
  "customer_id": "customer_id_from_workspace",
  "mode": "FTL",
  "equipment": "Dry van",
  "commodity": "Packaged goods",
  "weight_lbs": 24000,
  "miles": 180,
  "origin": {
    "facility": "Portland DC",
    "city": "Portland",
    "state": "OR",
    "date": "2026-10-15",
    "address_lines": ["15 Dock Street", "Building 2"],
    "postal_code": "97201",
    "country": "US",
    "contact": "Shipping desk",
    "phone": "+1 503 555 0100",
    "instructions": "Use gate 2"
  },
  "destination": {
    "facility": "Seattle receiving",
    "city": "Seattle",
    "state": "WA",
    "date": "2026-10-16",
    "address_lines": ["25 Receiving Ave"],
    "postal_code": "98101",
    "country": "US"
  },
  "buy_cents": 120001,
  "sell_cents": 150025
}
```

Supported modes: `FTL`, `LTL`. Equipment: `Dry van`, `Reefer`, `Flatbed`. Each shipment has one currency (USD by default; see [multi-currency](multi-currency.md)); money is integer minor units in that currency, never floating-point arithmetic in the API/domain.

Plans optionally accept `intermediate_stops: [{id, kind, stop}]`, with up to eighteen stable `stop_` IDs between origin and destination. These stops use the existing stop shape, kind `pickup` or `delivery`, and ordered dates. Their actual service endpoints require `{expected_load_version, stop_id, observed_at, contact, reference, note}`. See [multi-stop execution](multi-stop.md).

Stop street/postal details may be absent from legacy records and API drafts; new tenders require both. Stop country defaults to `US` when missing. Wire limits are facility 60, city 30, each of up to two street lines 55, postal code 15, and uppercase two-letter state/country. Wire fields are ASCII and reject X12 delimiters/controls; contacts/phone/instructions have separate length limits. See [stop data and migration behavior](stops.md).

Appointment booking/rescheduling body (strict fields; operations permission):

```json
{
  "expected_load_version": 1,
  "stop": "pickup",
  "start_local": "2026-10-15T09:00",
  "end_local": "2026-10-15T10:00",
  "timezone": "America/Los_Angeles",
  "disambiguation": "reject",
  "contact": "Facility scheduler",
  "confirmation_reference": "External booking reference",
  "note": "Facility and carrier confirmed the window directly"
}
```

`stop` is `pickup`, `delivery`, or a planned intermediate `stop_` identifier. Local times use minute precision. `disambiguation` defaults to `reject`; `earlier`/`later` explicitly choose a repeated daylight-saving time. Nonexistent local times always fail. Starts must match the plan's local stop date, the end is within 48 hours at/after the start, and active appointment starts must follow the full route order in UTC. Active windows extend carrier/driver qualification checks through their final local day. Rescheduling creates a replacement linked to its predecessor. Gate arrival/completed stops protect the original appointment baseline; cancel a booking before revising its stop details/date.

Gate record body:

```json
{
  "expected_load_version": 3,
  "stop": "pickup",
  "kind": "arrival",
  "local_time": "2026-10-15T09:12",
  "timezone": "America/Los_Angeles",
  "disambiguation": "reject",
  "reference": "Gate ledger row 15",
  "note": "Confirmed actual arrival with the gate clerk"
}
```

`kind` is `arrival` or `departure`. Correction bodies use `expected_load_version`, `local_time`, `timezone`, optional `disambiguation`, `reference`, and `note`; stop/kind/appointment/tender identity derive from the original event. Original and correcting records remain. The known event sequence must stay chronological and actual times cannot exceed server time by more than five minutes. Arrival/departure require the relevant carrier/lifecycle prerequisites and never advance shipment state or POD approval themselves.

Workspace responses expose `appointments`, `stop_events`, and load `stop_execution: {has_overdue, pickup, delivery, ...intermediate_stop_ids}`. Each stop summary has its active appointment/current-attempt arrival/departure, `arrival_status` (`early`, `on_time`, `late`, `unscheduled`, or `unrecorded`), `late_minutes`, `overdue`, `dwell_minutes`, `dwell_open`, and `dwell_complete`. Completed stops missing a departure have unknown dwell; metrics do not create financial charges. Audit actors/proof/timezone resolution and all histories are retained. Imported-order writeback includes only operational window/actual timestamps through `trackingData.freight_stops`.

Workspace loads expose `pod_ready` for current finance review state. File content is excluded from the workspace response. Upload metadata includes an immutable digest, uploader, document version, and review history. See [document storage, evidence binding, and migration behavior](documents.md).

For paid-customer credits/refunds, `confirmation` is `{counterparty_contact, confirmation_reference, evidence_reference, note, document_id?}`. `customer_refund` bank entries require it; other bank directions reject it. Credit `issued_on` defaults to today and cannot precede invoice issuance or exceed today. Refund clearing and customer reversals cannot precede the latest invoice financial activity. See [workflow, exact request fields, totals, and integrity rules](customer-refunds.md).

Workspace bills include `activity_date` and totals `received_cents`, `credited_cents`, `refunded_cents`, signed `balance_cents`, positive `refund_due_cents`, and `remaining_creditable_cents`. Finance summary separates positive `receivable_cents` from `customer_credit_cents` and signed `net_receivable_cents`. Void cancellation invoices remain eligible refund targets when net funds are owed.

Staff workspace responses additionally expose `portal_offers`, `portal_decisions`, `carrier_submissions`, `customer_quote_publications` (excluding HTML), `customer_quote_decisions`, and `document_shares`. These are retained publication/decision/access histories; they are not the partner projection. Partner endpoints never return the staff workspace, raw source orders, bank/journal, private rate evidence, or other counterparties. `carrier`/`customer` credentials require server-owned `counterparty_id` and are denied every staff route, including generic file/document downloads. See [exact native-response shapes and source/qualification rules](partner-portal.md#api-and-persistence).

Workspace `rate_confirmations` include retained snapshots, original HTML digest, metadata version, handoffs/retractions/resolutions, `current`, and `basis_changed`; HTML itself is omitted and downloaded through the statement route. Handoff changes advance load versions and constrain proven-unsent withdrawal. Old/changed handoff review is reflected in load `tender_control.open_incidents` and existing custody-sensitive gates. See [carrier document workflow](carrier-rate-confirmations.md).

Workspace responses additionally expose `billing_profile`, `bills` (including totals and aging), `credits`, `cash_entries`, `journal`, and the `finance` summary. Settlements have their own `version` and `balance_cents`. Customer records optionally include `billing_address`; legacy updates that omit it preserve its current value.

Additional charges appear in `charges`. Loads expose `commercial: {buy, sell, margin_cents, pending_buy, pending_sell, buy_locked, sell_locked}`. Each side's basis is `{base_amount_cents, charges: [{charge_id, kind, description, amount_cents}], total_amount_cents}`. Raw load `buy_cents`/`sell_cents` remain the original base rates. Invoices and bill snapshots include an optional `agreement` basis; older snapshots without it retain their existing totals. Customer statement lines include only the sell basis. Settlements use the matched approved invoice amount.

Proposal/correction body:

```json
{
  "expected_load_version": 5,
  "side": "buy",
  "kind": "detention",
  "description": "Agreed additional loading service",
  "amount_cents": 13751,
  "evidence_reference": "RATE-123/DETENTION-1",
  "document_id": null,
  "corrects_id": null
}
```

`side` is `buy` or `sell`. Supported kinds are `detention`, `lumper`, `fuel`, `layover`, `extra_stop`, and `other`. Amounts are positive minor units in the shipment's currency, and base plus approved additions cannot exceed 100,000,000 minor units. The evidence reference identifies one active proposal/approval per load/side, case-insensitively. Optional stored files must belong to the same load/tenant. A correction requires a rejected/voided predecessor for the same assignment, counterparty, side, and reference, without a prior replacement. Original amounts and decisions are preserved.

Review body:

```json
{
  "expected_load_version": 6,
  "expected_version": 1,
  "decision": "approved",
  "counterparty_contact": "Carrier accounting representative",
  "confirmation_reference": "External agreement CASE-15",
  "note": "Reviewed the evidence and verified the exact additional amount"
}
```

Review decisions are `approved`/`rejected`. Contact/confirmation are required for approval; rejection requires a reason in `note`. A void body has the same version/contact/confirmation/note fields without `decision`. These shapes reject unknown fields, including supplied actors or statuses. Server identity is recorded. Approvals/voids increment the load/charge versions, re-audit carrier amounts without rewriting them, and refresh a customer draft's amount/basis/version when applicable. Pending proposals hold their side's posting. Supporting stored bytes are checked atomically at charge approval and financial posting; generic document rejection cannot invalidate an approved charge's source.

Accepted carrier ownership is required to propose a charge. Carrier additions retain their tender identity; retired proposals cannot affect replacements. Release/custody holds prevent approval, and approved additions prevent silent cancellation/re-planning/release. The original basis locks when carrier billing is approved/a settlement exists or a non-void customer invoice has issued. Use the separate carrier adjustment routes for actual later carrier documents. See [charge lifecycle](charges.md) and [carrier adjustments](payables.md).

Carrier credit application body:

```json
{
  "source_settlement_id": "credit_bearing_settlement_id",
  "expected_source_version": 3,
  "amount_cents": 4501,
  "applied_on": "2026-10-15",
  "allocations": [
    {
      "target_id": "other_prepared_settlement_id",
      "expected_version": 1,
      "amount_cents": 4501
    }
  ],
  "evidence_reference": "Carrier allocation confirmation 15",
  "counterparty_contact": "Carrier AR representative",
  "confirmation_reference": "External carrier confirmation CASE-15",
  "note": "Inspected the carrier's actual allocation confirmation",
  "document_id": null,
  "corrects_id": null
}
```

Only `finance.write` may mutate these records. The positive 1–100,000,000 minor-unit total, in the source shipment's currency, must exactly equal 1–100 unique positive target allocations, fit the source credit, and fit all target payables. The source is excluded from its targets, and every settlement must belong to the same carrier with current ownership, approved invoice, reviewed delivery evidence, and no release/custody hold. Caller-supplied carrier/currency/status/actor fields are rejected. Optional supporting evidence belongs to the source shipment and captures its digest; posting transactionally verifies source/target evidence. No bank transaction or expense is generated. A per-carrier application reference cannot be active twice; a reversed reference requires its explicit nonbranching correction link.

Reversal body:

```json
{
  "expected_version": 1,
  "expected_source_version": 4,
  "targets": [
    { "target_id": "other_prepared_settlement_id", "expected_version": 2 }
  ],
  "reversed_on": "2026-10-15",
  "counterparty_contact": "Carrier correction desk",
  "confirmation_reference": "External carrier retraction CASE-16",
  "note": "Carrier confirmed retraction of this allocation"
}
```

Reversal requires exactly the original target IDs and their current versions, appends opposite postings, and retains the original `confirmation` separately from `reversal_confirmation`. It can reopen target payables after payment. Both dates must follow existing payable activity on all affected settlements, with no future date; no historical balance is inferred. Workspace responses include `carrier_credit_applications` with sources, allocations, proof, reversal, and correction links (empty by default for old snapshots). Each settlement exposes signed `credit_application_cents`, effective `balance_cents`, and `credit_application_earliest_date`. All affected settlement versions advance atomically, including when only the source credit is partially consumed. See [workflow and preview](carrier-credit-applications.md).

Customer supplement draft body (`id` is the active issued original invoice):

```json
{
  "expected_version": 3,
  "expected_load_version": 8,
  "number": "CUSTOMER-SUP-15",
  "due_date": "2026-11-15",
  "amount_cents": 12501,
  "description": "Additional unloading agreed after original billing",
  "counterparty_contact": "Customer AP representative",
  "confirmation_reference": "External customer confirmation CASE-15",
  "evidence_reference": "Unique unloading agreement 15",
  "document_id": null,
  "corrects_id": null
}
```

This strict finance-only shape accepts a positive additional amount in the original invoice's currency supported by the recorded customer agreement. It rejects caller-supplied status/actor/customer/currency fields. The returned bill's optional `supplement` retains the original invoice ID/number, agreement/contact/confirmation, file digest, and explicit correction links. Old original records deserialize with `supplement: null`. Creation posts no money and increments parent/load versions. Issue via `/bills/{supplement_id}/issue` after reviewing its immutable agreement; use the existing plan/credit/receipt/void/statement routes for that independent invoice. Its snapshot keeps the original customer/shipment and its own additional amount. Duplicate agreement references, nested supplements, active-original voids, and implicit/branching corrections fail. Correcting a void requires a new number and that exact original invoice/agreement reference. Stored agreement bytes are checked atomically at issuance. Current commercial totals expose `customer_supplement_cents`, `customer_credit_cents`, and `total_sell_cents`; only issued non-void supplements increase totals, and active credits reduce them. See [complete workflow](customer-supplements.md).

Carrier adjustment draft body:

```json
{
  "expected_load_version": 8,
  "invoice_id": "original_approved_invoice_id",
  "kind": "supplemental_invoice",
  "number": "CARRIER-SUP-15",
  "document_amount_cents": 1251,
  "issued_on": "2026-10-15",
  "description": "Agreed additional unloading service",
  "evidence_reference": "Actual carrier document and agreement reference",
  "document_id": null,
  "carrier_document_id": null,
  "corrects_id": null
}
```

Kinds are `supplemental_invoice` (addition), `credit_memo` (reduction), and `invoice_revision` (replace the recognized total; derive only the difference). Amounts are integer minor units in the shipment's currency; a replacement total may be zero. Document numbers allow 1–200 characters without controls and normalize trimming/ASCII case. Drafts retain their recognized basis, carrier/tender, creator, optional file digest, and source. `carrier_document_id` binds the canonical number, amount magnitude, currency, and date. Negative 210 totals remain signed in the source and require explicit verified credit-memo classification; positive headers cannot establish credit designation. Active duplicate financial documents and branching/implicit corrections are rejected.

Review requires `{expected_load_version, expected_version, decision: "approved" or "rejected", counterparty_contact, confirmation_reference, note}`. Reversal requires the version/contact/reference/note fields plus `reversed_on`, without `decision`. Posting requires current ownership and reviewed delivery evidence; stored bytes are checked atomically. Stale revised-total bases require rejection/correction. Explicit corrections can repair classification while preserving the prior draft/posting and source identity. Reversal dates cannot precede posting or be in the future; credits/debit reversals cannot make the recognized invoice total negative.

Workspace responses additionally expose `carrier_documents` and `carrier_adjustments`. Additional 210 receipt returns `{finance_review: true, carrier_document_id, load_id}` without posting money. Its inbox status is `finance_review`; classification/dismissal becomes `finance_reviewed` while retaining original notification/event/history. Received documents stay `pending`/`allocated` until finance resolves them. Adjustment statuses are `draft`, `posted`, `rejected`, or `reversed`.

Settlement `amount_cents` remains original principal; `total_amount_cents` includes active posted differences. `balance_cents` is signed after net payouts/refunds; `credit_balance_cents` is its positive credit magnitude. Negative balances have status `credit_due`. The finance summary reports positive `prepared_payable_cents` and separate `carrier_credit_cents`. Load `commercial.total_buy_cents`, `carrier_adjustment_cents`, and margin include posted carrier differences while original agreement bases remain unchanged.

Bank direction `carrier_refund` uses credit-bearing settlement targets and positive allocations. Each amount must fit the current credit balance, share one carrier, and sum exactly to the actual cleared refund. Its date cannot precede the supporting credit document/reversal. Existing bank-reference/version/reversal/correction guards apply; the app records the refund without initiating one.

Tender history appears in `tenders`, `releases`, and `tender_incidents`. Each load exposes `tender_id` and `tender_control: {can_withdraw, release_pending, open_incidents}`. Tender attempts retain their carrier/plan snapshots and independent `shipment_reference`; use that reference as the carrier-facing `shipment_id`, including when tendering again to the same carrier. The freight load reference and source commerce identity remain stable. Outbox records expose `tender_id`, cumulative `total_attempts`, and persistent `transmission` (`not_started`, `attempted`, or legacy `unknown`). Resetting retry backoff does not erase transmission evidence.

Release confirmation, retention, and incident resolution require this strict body (unknown fields are rejected):

```json
{
  "expected_load_version": 4,
  "carrier_contact": "Carrier dispatcher name and contact",
  "confirmation_reference": "External conversation or case reference",
  "note": "What the carrier confirmed and how custody was checked"
}
```

The authenticated operations subject is recorded by the server. Requesting release pauses EDI handoffs and dispatch while retaining ownership. Confirmation cannot release freight after pickup, with unresolved carrier conflicts/financial documents, or while an HTTP handoff has a live lease. An expired handoff lease is fenced without claiming its previous request was unsent. See [carrier release and reconciliation](tenders.md).

Authenticated release negotiation uses these additional routes. All mutation bodies are strict; actor, carrier, tender, and disposition derive from the authenticated identity and stored request.

| Method | Route                                            | Permission and body                                                                                                                                                                                     |
| ------ | ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| POST   | `/v1/release-requests/{id}/portal-publications`  | `operations.write`; `{expected_load_version, expected_previous_id?, message, respond_by}`; UTC RFC3339 deadline in the future and within seven days                                                     |
| POST   | `/v1/portal/releases/{publication_id}/response`  | `portal.carrier.release`; `{expected_load_version, document_sha256, decision: "release"\|"retain", contact, reference, note, no_custody?}`; release requires `no_custody: true`, retention must omit it |
| POST   | `/v1/release-responses/{id}/apply`               | `operations.write`; `{expected_load_version}`; applies original retained acknowledgement after all current checks                                                                                       |
| GET    | `/v1/portal/releases/{publication_id}/statement` | `portal.read`; exact original HTML/hash for its owning carrier only                                                                                                                                     |
| GET    | `/v1/release-publications/{id}/statement`        | Staff read; exact original HTML/hash                                                                                                                                                                    |

Staff workspace responses include `release_publications` without HTML and retained `release_responses` with source evidence hashes and separate application evidence. Carrier `GET /v1/portal` includes scoped `release_requests`: frozen snapshot/document hash, status, action eligibility, own response, and a current load version only while that exact assignment/request remains current. It excludes private operations reasons and replacement-carrier execution. A native positive acknowledgement held by a live lease/finance/custody review returns `outcome: "awaiting_operations"` and no application; it retains the assignment. Later application leaves the original carrier evidence unchanged. Expiry prevents both release response and application, while native retention can close an expired request. Publication/response/application require idempotency keys and version fences. See [workflow and hold semantics](tenders.md#authenticated-carrier-release).

External cash reconciliation example:

```json
{
  "direction": "receipt",
  "currency": "USD",
  "account": "operating-usd",
  "external_reference": "bank-statement-transaction-id",
  "amount_cents": 100001,
  "cleared_on": "2026-10-15",
  "note": "Matched against the external bank statement",
  "allocations": [
    {
      "target_id": "bill_id_from_workspace",
      "expected_version": 2,
      "amount_cents": 100001
    }
  ],
  "corrects_id": null
}
```

Use `direction: "payout"` with settlement targets for carrier payments. All 1–100 allocations must belong to one counterparty and exactly total the bank amount. The API checks outstanding balances, target versions, valid dates, and unique account/reference identity. A reversed reference requires an explicit `corrects_id`; see [financial lifecycle and journal rules](finance.md). Financial request shapes reject unknown fields, including client-supplied actors or statuses. Original customer invoice totals derive from approved agreements; supplemental amounts require their separate recorded customer agreement.

Load commands contain `action`, `expected_version`, and the action's fields:

| Action               | Fields                           | Permitted state                                                                                                           |
| -------------------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `tender`             | `carrier_id`                     | planned                                                                                                                   |
| `accept` / `decline` | none                             | tendered; records manual carrier response                                                                                 |
| `dispatch`           | `driver_id`, `equipment_id`      | accepted                                                                                                                  |
| `pickup`             | none                             | dispatched                                                                                                                |
| `location`           | `location`                       | in_transit                                                                                                                |
| `deliver`            | `pod_reference` or `document_id` | in_transit                                                                                                                |
| `attach_pod`         | `pod_reference` or `document_id` | delivered; evidence cannot change after invoice approval/settlement                                                       |
| `exception`          | `note`                           | any open load                                                                                                             |
| `resolve`            | none                             | load with an exception                                                                                                    |
| `withdraw_tender`    | none                             | tendered; every associated handoff must be proven never attempted; returns to planned                                     |
| `cancel`             | none                             | planned, or tendered with proven never-attempted handoffs; other pre-pickup assignments require confirmed carrier release |

The normalized EDI envelope contains `event_id`, `event_type`, `occurred_at`, and `payload`. The payload must identify the tender attempt's `shipment_reference` as `shipment_id` and its snapshot `partner_id`; provided SCACs must match. An active legacy assignment retains its original load reference until released. An ambiguous load-reference fallback cannot select a new tender. See the integration runbook for the exact upstream callback and normalized-event distinction.

Signed callbacks return `{received: true, action: "queued", inbox_id, status, replayed}` **after** durable receipt, without contacting EDI in the callback request. The worker's separate `applied` outcome confirms the business transition committed. `pending`, `processing`, `retry`, `held`, `applied`, and `ignored` describe ordinary processing. `reconciliation_required` means authenticated historical carrier movement/invoice evidence was retained as a conflict without applying it to the replacement assignment. Operations incident resolution changes that item to `reconciled`; it does not post its invoice or replay physical movement. Retry is unavailable for this business reconciliation state. The original authenticated body, canonical event when available, processing result, and completed attempt history remain in the tenant workspace. Operator retry restarts the backoff cycle while retaining cumulative attempts and history.

For 214 tracking events, `events` is processed in document order. Its final `status_code` must match `latest_status`; absent `events` retains compatibility with a single normalized `latest_status`. A failure anywhere in a document rolls back the entire batch. Individual observations retain source date/time/location and document event identity in activity details; receipt time does not replace their original evidence. AG estimates do not move the physical last-reported location, and later repeated pickup/arrival observations do not regress a delivered shipment.

## Snapshot admission overload

Queued snapshot reads have per-process global/tenant count limits and a queue deadline. Exhaustion/expiry returns HTTP 503, `Retry-After: 1`, `Cache-Control: no-store`, and `error.code: "read_capacity_exceeded"`. No snapshot work starts for those requests. The gateway retains the response and retry hint; credential verification treats it as temporary unavailability and issues no session. Physical tracking commands avoid unnecessary source-read preflights, while tender/dispatch and other commitment workflows retain canonical source checks. See [settings, lifecycle, tests and limits](read-overload.md).

Known PostgreSQL lock/statement cancellation and pool acquisition expiry return HTTP 503 with the same retry/cache headers and `error.code: "database_temporarily_unavailable"`. Database I/O loss, a closed pool and explicit PostgreSQL connection/shutdown conditions return the same code with `message: "Database is temporarily unavailable. Check the original request before retrying."`. Protocol violations, decode failures and ordinary integrity errors retain storage-error behavior. These classifications do not retry operations or establish that a disconnected write failed to commit. Credential verification preserves this code without issuing a session or automatically retrying. Retain the original mutation key and exact body and use receipt-aware recovery; every HTTP 5xx response remains an uncertain browser save. See [runtime settings and recovery evidence](database-budgets.md).

`GET /v1/workspace` preserves complete approval evidence by default, including with `history=deferred`. Staff may opt into `X-Freight-Approval-Details: deferred` to omit only `entry_review_note`, `approval_note` and `evidence_reference` from each populated `approval_control.approval_evidence`. The seven decision/revision/actor/time fields remain. `complete` preserves the default; unsupported header values return 400. The header changes representation, never authorization. Workspace responses declare `Cache-Control: no-store` and `Vary: X-Freight-Approval-Details`.

The staff UI sends the optional compact header. A compatible preceding API ignores it and still supplies complete evidence, which remains directly usable without the new endpoint. Existing clients and older UIs receive the complete response by default. No write endpoint, financial policy, migration or stored format changes. This resource still materializes the complete tenant source; its response limit is not a database/decoding capacity qualification.

## TMS MCP adapter

The repository includes a local stdio MCP server at `mcp/src/index.mjs`. It uses the official MCP server SDK 2.3.1 and supports the 2026-07-28 protocol plus legacy 2025 clients. Its reviewed catalog covers all 126 protected API operations, including staff and partner reads, planning, procurement, execution, finance, integrations, save resolution, and original document/POD uploads. The unprotected health, metrics and signed webhook endpoints are outside this tool surface. The adapter delegates authorization, source/version checks and receipts to the same API as other clients.

Install with `npm --prefix mcp ci`, then configure a host using `mcp/client-config.example.json`. Point the command directly at Node 22 or later and `mcp/src/index.mjs`; stdout carries only MCP messages. Configure `FREIGHT_MCP_API_URL` as the backend API origin and exactly one of `FREIGHT_MCP_TOKEN_FILE` or `FREIGHT_MCP_TOKEN`. Prefer a private credential file; its contents are read afresh before each request. Use HTTPS outside loopback, or explicitly allow a trusted private HTTP network with `FREIGHT_MCP_ALLOW_HTTP=true`. TLS uses Node's normal verification and optional `NODE_EXTRA_CA_CERTS`. No credential, tenant, arbitrary URL or identity-header tool argument exists.

For Clerk, additionally set `FREIGHT_MCP_IDENTITY_MODE=clerk` and the fixed `FREIGHT_MCP_ORGANIZATION_ID=org_...`. Supply a current signed session JWT through the credential source. The API checks current membership/session status for every business request; the required Clerk header prevents fallback to a static credential. Token refresh may update the private file; changing organizations requires updating the configured binding and restarting the MCP process.

Four tools are exposed:

- `freight_catalog`: discover operation names, registered methods/paths, descriptions and 28 bundled request guides. Guides are available through `freight://guides/<name>` resources; they contain documentation rather than live records.
- `freight_read`: call a registered GET operation, with an entity `id` where applicable and literal string `query` values. Use paginated `load_catalog`, `operations_board` and `history_page` for bounded discovery. Original JSON is returned in `response.text`; binary content is base64. Preserve integer precision when interpreting JSON text.
- `freight_write`: supply a registered operation, original `idempotency_key`, and exact `body_json` object text. JSON is validated but never parsed and reserialized for forwarding, preserving original bytes and integer cents. Include the API's expected versions, source digests and genuine evidence. Supported original-save resolution uses operation `resolve_save` and the original `request_key`, canonical path/body and note described above.
- `freight_upload`: supply canonical `body_base64`, a plain `content_type`, original key, and the API's entity/query fields. The adapter retains the API's 10 MiB document limit; JSON writes retain its 1 MiB limit.

Results expose `ok`, `http_status` when received, original request metadata/digest and complete response bytes/digest. API 401/403/409/422/503 responses remain errors with their status and details. A missing complete response after dispatch is `unconfirmed` for writes: it may follow a real commit. The adapter never automatically retries, follows redirects, invents success or generates a replacement key. Retain the original tool arguments durably in the host before invoking a write, then explicitly replay that same original or use the resolver for a supported request. This adapter has no durable command journal, and the resolver supports its documented path set, not every API operation. Cancellation cannot undo an accepted request. A `not_sent` result identifies local rejection before dispatch. Document reads may append API download audit evidence; tool annotations are conservative hints, not authorization.

The default response budget is 16 MiB, configurable through `FREIGHT_MCP_MAX_RESPONSE_BYTES` (1 KiB–64 MiB). Inbound MCP frames allow 32 MiB, including base64 overhead for the full document limit. Configure compatible client frame limits for large results; SDK qualification uses 64 MiB. Timeouts default to 30 seconds and may be set with `FREIGHT_MCP_TIMEOUT_MS` (1–120,000). An interrupted or over-budget write response remains unconfirmed. Returned business text and guides are data; they do not authorize further actions.

A nonroot container can be built with `docker build -t stateset-freight-mcp -f mcp/Dockerfile mcp`. It serves stdio, so launch it with an open stdin (`-i`) through a host. Pass credentials by protected file/environment, use a read-only filesystem and supply network access to the configured API. This is a process-scoped MCP connection; it does not commission a public MCP HTTP/OAuth service or configure an external host/account.

Native iCommerce connections use operator-configured credentials and tenant routing. See [iCommerce ownership, lifecycle and uncertain PATCH recovery](icommerce.md).


## Terminal financial and queue pages

`GET /v1/console` requires staff `workspace.read`. `/v1/access` advertises `features.console_catalog: true` when it is available. The authenticated principal determines the tenant; tenant, owner and other unknown query overrides are rejected. The endpoint is read-only and declares `Cache-Control: no-store`.

Required `view` is `billing`, `payables` or `integrations`. `limit` defaults to 50 and accepts 1–100. `q` is a trimmed, Unicode case-insensitive literal substring of exposed values (including amounts formatted in each record's currency), at most 200 normalized UTF-8 bytes. Search runs before paging and never searches omitted payloads, authentication or serialized field names.

Billing and Payables accept `balance=all|open|credits|zero|unknown`; Billing also accepts `overdue`. Classification uses signed integer cents, with positive balances open, negative balances credits, and overdue requiring a positive balance and positive reported overdue days. Missing values never become zero; financial arithmetic overflow fails the read. Queues accept `attention=true|false` and `direction=all|outbound|inbound`. Attention includes exactly retry, held and failed. Filters from another view are rejected, even if set to their defaults.

Responses include `tenant_id`, `view`, `limit`, `total` (all matching rows), `next_cursor` and arrays `bills`, `settlements`, `outbox`, `inbox`. Irrelevant arrays are empty. At most `limit` records are returned across all arrays combined. Financial `finance` cards always describe the entire workspace, independently of search, filter or page; `totals_scope` is `workspace`. Queue pages do not calculate financial totals. Bills omit customer contact/payment instructions and full commercial snapshots; queues omit payloads, upstream responses, callback bodies/authentication, histories and lease tokens. Recorded last-error text remains available as operational evidence and must not be used for secrets.

Opaque cursors accept at most 2048 bytes and bind the authenticated tenant/subject, normalized filters, page size and ordered matching identities. Financial rows sort by ID. Queues sort outbound first, then inbound, with direction-qualified IDs so equal raw IDs cannot collide. Membership changes invalidate the cursor; field changes that retain membership return fresh values. Cursors are navigation state, never authorization. Start at page one after a rejected cursor. Each page is a fresh read rather than a fixed snapshot across pages.

With migration 030 installed, PostgreSQL reads use a rebuildable console index. Warm pages fetch at most `limit + 1` projected rows through a keyset index; the extra row determines the next cursor. Balance, direction, attention and overdue equalities use fixed prepared statement shapes so PostgreSQL can plan them as indexed predicates. Exact counts, matching identity digests and literal substring search still scan matching projected candidates; they are not constant-cost or full-text/trigram searches. Original workspace payloads are not decoded on warm reads.

The first read after a canonical write, cache loss or UTC date rollover rebuilds all three projections under the original tenant row lock. This cold rebuild uses normal snapshot and workspace admission before acquiring a database connection. Canonical header/block triggers invalidate the projection in the source transaction, including writes from an already-running preceding binary. Rebuilds never rewrite original business records, receipts, versions or source timestamps. Financial arithmetic errors quarantine financial views without representing them as empty or blocking queue reads. Duplicate identities fail closed. Cancellation retains page, snapshot and workspace admission through the actual rebuild transaction's commit or rollback.

The projection expires on UTC date changes so overdue days, filters, search values and workspace totals are rebuilt together. A date change during a read causes retry rather than returning expired aging. Memory/demo stores retain the native projection implementation. Cursor version 2 hashes matching direction-qualified identities identically in memory and SQL; version-1 tokens are rejected and callers should restart at page one. Tokens still preserve fresh-field behavior when matching membership is unchanged.

Install migration 030 with the migration role before starting this API, then refresh runtime grants using the existing runbook. Stored business format 21 is unchanged. Older binaries cannot restart against the new exact migration ledger; existing compatible canonical writers invalidate the new read cache. This local implementation and query-plan qualification do not establish production capacity or operator acceptance.

### Terminal evidence previews

`GET /v1/loads/{id}/documents` requires `workspace.read`. The tenant comes from the authenticated principal. It returns `{load_id, documents, total}` with the newest 100 documents, ordered by upload time descending and ID ascending. Each item contains ID, load ID, filename, kind, content type, size, original SHA256, version and latest review status. Review notes, binary content and other loads are excluded. A missing shipment returns 404. `total` makes truncation explicit; use the web workspace for older evidence.

`GET /v1/documents/{id}/preview` uses the same staff permission and tenant boundary as the original download. The store verifies original length and SHA256 before rendering. PNG and JPEG are decoded with strict 8192-pixel dimension limits, a 16-megapixel ceiling and a 64 MiB decoded-buffer ceiling, then oriented from image metadata, resized to fit 960×960 without enlarging, and composited onto white. Only these two decoders are enabled. PDF, malformed and oversized images return 422; PDF originals remain available through the existing download route. One process-wide preview permit admits work without queuing; contention returns 503. Blocking decode retains its permit if the requesting connection is cancelled.

A successful preview has `Content-Type: application/vnd.stateset.rgb8` and exactly width×height×3 row-major RGB bytes. Headers `X-Preview-Width`, `X-Preview-Height`, `X-Content-SHA256`, `X-Source-SHA256`, `X-Document-ID` and `X-Load-ID` bind the bytes to the catalog item and original. Both new routes return `Cache-Control: private, no-store`; previews also return `X-Content-Type-Options: nosniff`. Previewing never approves a document, updates a load, or changes the original bytes. Clients must treat the derivative as a visual aid, retaining the original for evidence decisions.
