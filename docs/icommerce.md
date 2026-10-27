# Native iCommerce bridge

Freight uses the engine's `stateset-primitives` typed IDs, `CurrencyCode` and `Money` through `stateset-freight-commerce`, plus `stateset-pricing` for its currency scale lookup and reporting-only rounding. Freight does not use the pricing crate's tax, promotion, line-item or order-total calculations. Both crates are pinned at 1.39.0. The exact upstream source, commit, hashes and MIT/Apache notices are retained in `vendor/stateset-primitives/UPSTREAM.json` and `vendor/stateset-pricing/UPSTREAM.json`. Their standalone manifests resolve workspace metadata; library source is unchanged. `web/tests/money.test.mjs` fails if the browser's copied currency scale table in `web/lib/money.mjs` differs from the vendored `currency_decimal_places`. This build does not depend on a sibling checkout or an unverified registry publication. The broader embedded commerce engine and its database do not run inside Freight.

Freight owns transportation plans, carrier tenders, buy/sell rates, stops, custody, documents, billing and recovery. iCommerce owns merchandise orders, fulfillment, inventory, packing, labels and native shipment records. Order merchandise totals never become freight prices. Money uses exact decimal strings and integer minor units at each currency's scale, including signed 64-bit boundaries; excess precision and overflow fail rather than round, and no workflow converts currency. Native HTTP DTOs do not provide the complete physical freight plan: operators supply its customer, route, equipment, weight and rates explicitly.

## Configure a tenant

Set an entry in operator-owned `FREIGHT_INTEGRATIONS_JSON`:

```json
{"acme":{"commerce_url":"https://commerce.example.com","commerce_token":"replace-with-a-tenant-authorized-token-at-least-32-characters","commerce_tenant_id":"acme"}}
```

The upstream HTTP deployment must enable tenant routing and authorize that credential for the configured tenant. Requests send `Authorization: Bearer …` and `x-tenant-id`; the freight workspace and native tenant are independent namespaces. Native origins require HTTPS, with HTTP allowed only on loopback for local development. Origins cannot contain paths, queries or credentials; redirects are refused and responses are bounded to 2 MiB. Native and legacy Sync order sources are mutually exclusive per freight tenant. EDI may coexist. Do not change providers for existing imported loads: their retained provider identity prevents silent retargeting. Reconfiguring a connection does not retarget already queued native handoffs.

## Import

Operations staff can list `/v1/commerce/orders?page=1`, inspect `/v1/commerce/orders/{id}`, then POST `/v1/commerce/import` with an `Idempotency-Key`:

```json
{"order_id":"00000000-0000-4000-8000-000000000001","load":{"reference":"CUSTOMER-LOAD-1","customer_id":"existing-freight-customer","mode":"FTL","equipment":"Dry van","commodity":"Goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Origin","city":"Portland","state":"OR","date":"2026-10-23","address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Destination","city":"Seattle","state":"WA","date":"2026-10-24","address_lines":["25 Receiving Avenue"],"postal_code":"98101","country":"US"},"buy_cents":123456,"sell_cents":150000}}
```

Use real dates/addresses and reviewed rates. The API fetches source identity and quantities itself; callers cannot inject source snapshots or upstream credentials. Only pending/confirmed/processing, unfulfilled orders with no shipped units qualify as new forward loads. Cancelled, held, unknown and partially/completely fulfilled orders are retained for source review but refused as new imports. Existing `/v1/sync/orders` and `/v1/sync/import` also use the tenant's configured provider, preserving the dashboard import flow. Original imports replay from the Freight receipt even when the source has changed or become unavailable.

The existing source observation and explicit review rules protect imported loads. Later changes in merchandise quantities, fulfillment or status can hold new commitments; physical custody/history remain intact. No native import emits legacy Sync tracking writeback.

## Review one shipment step

Complete warehouse packing and carrier handoff in iCommerce. The native shipment must belong to the original order and its product/SKU manifest must exactly cover all imported quantities. Partial shipments require a separate allocation workflow and cannot be advanced as if they represented the entire load.

The dashboard exposes this review on imported native loads, using the existing encrypted original-save journal. The iCommerce integration card opens native order import. API/MCP clients can use the same protected operations.

Once Freight is physically `in_transit` or `delivered`:

1. GET `/v1/loads/{load_id}/commerce-tracking?shipment_id={native_shipment_uuid}`. Inspect the native manifest, current status/version, load version and proposed next step.
2. POST the same route with a fresh `Idempotency-Key` and `{expected_load_version, shipment_id, expected_shipment_version}` from that review. The server verifies the native record again, refreshes the source and atomically queues the frozen handoff with its audit and receipt. Another unresolved native handoff for that load blocks new intent. The first queued handoff also binds the load to that native shipment UUID permanently through retained history; completing it does not permit a different native shipment to reuse the same physical delivery.
3. Inspect the integration queue. `synced` records the exact version/status confirmation or an explicitly described observed convergence. For a delivered load, repeat a new preview/review after each confirmed step until native delivery is complete.

The legal forward steps are `shipped`/`failed` → `in_transit` → `out_for_delivery` → `delivered`. One review queues one step. Freight does not jump from shipped to delivered, reverse an advanced native state, or create/pack/allocate a native shipment. PATCH bodies contain only `expected_version` and `status`. Tracking numbers, carrier labels, addresses, prices and inventory are never fabricated or overwritten by this bridge.

## Find native shipments

Operations can browse `GET /v1/loads/{id}/commerce-shipments?page=1` from the load detail's **Find order shipments** picker. Freight derives the order UUID from the retained native import and fixes the upstream origin, credential and tenant from operator configuration. Only one positive bounded `page` is accepted; caller-supplied order, tenant, URL, offset and filter overrides are refused. Each native request fixes `order_id`, `limit=25` and the checked page offset. The upstream page must match those bounds and contain unique same-order shipment identities; an invalid or unrelated row refuses the entire response. The adapter keeps its 2 MiB response bound and never exposes upstream recipient/address/price fields through this list.

Rows include native shipment number, identity, status and version, plus `ready_for_review`, `blocked` or `up_to_date` and an explanation. Partial manifests, warehouse-only states and a different retained shipment UUID cannot start another step. Existing pending/sending/retry/held native jobs are shown and block a second handoff. The list is read-only: it creates no local receipt, job or source observation and sends no native PATCH. Source holds remain in force at confirmation.

Selection returns only a shipment identity and fetches the existing individual tracking preview afresh. It does not use the discovery page as a write body. Confirmation then re-fetches the native record, checks the reviewed versions and verifies canonical source readiness as before. Changes observed in the current load version, verified owner or native handoff invalidate the dashboard list and preview. Manual UUID entry remains available. Pages reflect live records rather than a stable snapshot; native total counts are informational, and `has_more` comes from the upstream bounded overfetch contract.

## Uncertain outcomes

Native shipment PATCH has optimistic version checks but no durable idempotency receipt. Freight freezes the original version/body, native identity, manifest, origin and tenant in its outbox. A failed/invalid/interrupted response or expired sending lease moves the job to `held`; automatic retry stops. The original Freight request can replay its local queue receipt without another upstream fetch or queue insertion. The save-resolution endpoint can recover or fence an uncertain local enqueue using the original route, body and key.

Inspect the held job and native shipment before explicitly POSTing `/v1/outbox/{id}/retry` with a new local retry key. The dispatcher first reads the native record. Exact original next-version/status/manifest convergence completes the job with a message that the original PATCH receipt is unavailable; it does not claim proof of which writer committed it. If the original version/status/manifest remain unchanged, the explicit retry sends that original PATCH. Any other version, manifest, order identity or connection binding keeps the job held. It never rebases onto a newer native version or automatically queues a different lifecycle step. Source holds also continue to apply. Retain and investigate unresolved jobs instead of deleting their history.

## Review externally advanced handoffs

When another native writer has moved a held shipment beyond its original expected version, Operations can preview `GET /v1/outbox/{id}/commerce-review`. The native record must retain the complete original manifest and order identity, the original fixed origin/tenant binding, and a strictly greater version. An unchanged version stays held because its original PATCH could still apply. Active leases, altered manifests and changed connections refuse review.

After investigating external activity, POST that path with `expected_load_version`, `expected_total_attempts`, `expected_shipment_version`, and genuine `contact`, `reference`, and `note` (each 1–200 bytes without control characters). The server fetches the native record again and checks current local versions under the tenant transaction. The observed greater native version fences a late original optimistic write; it does not establish which writer applied the original PATCH. The resulting `external_reviewed` status retires only the old local handoff. It retains the original payload, identity, transmission and attempt history, records the actor and observation, and keeps `original_patch_outcome: "unknown"`. It changes neither native shipment nor physical Freight status and sends no PATCH.

The staff Integration handoffs queue exposes this review and its recorded evidence. Its confirmation uses the encrypted original-save journal. Replay with the exact original key/body returns the original local receipt without native access; the original-save resolver can recover a completed review or fence an uncommitted one. Integration-only and viewer roles cannot perform this operations decision. A source hold does not prevent retiring old intent; fresh source checks still gate any new commitment. A next lifecycle step requires a separate fresh shipment preview and confirmation. The retained native shipment UUID binding survives retirement.

## Persistence and qualification

Native snapshots, plans and connection bindings use existing generic source/outbox/receipt fields; no secret is persisted in a handoff. Migration 029 extends the existing order-source wakeup projection to native jobs, recomputing changed scheduling deadlines without rewriting original queue bodies, workspace timestamps or history revisions. The canonical storage format is unchanged. Older workers do not recognize the native destination and their configuration parser rejects native connection fields; deploy the current API and workers together rather than mixing binaries during activation.