# Carrier contract rates and procurement

These lane agreements price two-endpoint shipments. A [multi-stop route](multi-stop.md) is explicitly ineligible even when its origin, destination, equipment, dates, and limits match. Record an actual carrier offer covering the full route instead; an extra-stop charge does not silently expand a lane agreement's scope.

In **Quotes**, **Carrier lane agreements** retains reusable, staff-reviewed external pricing. A lane agreement is separate from available capacity, a carrier tender, customer acceptance, and financial posting. The catalogue supports inclusive flat rates in one [currency](multi-currency.md) per agreement (USD by default) for full truckload shipments, exact directed US/Canadian postal lanes, equipment, optional customer scope, pickup/delivery period, and maximum declared weight/mileage.

## Record and maintain the agreement

1. Inspect the original carrier agreement. Choose **Record carrier agreement** and select the actual carrier, agreement number, customer scope, directed postal lane, equipment, limits, inclusive price, date range, and original terms. Record the actual carrier contact, external source reference, substantive private findings, and revision reason. Acknowledge the reviewed evidence and save. This retains a named operator's attestation; it does not independently verify a carrier signature or communicate acceptance.
2. **Original terms and revision history** preserves each frozen carrier/customer snapshot, normalized scope, amount, terms, source evidence, actor/time, original HTML, and SHA-256 hashes. **Download agreement revision** returns those exact original recorded bytes. This is Freight's record of the reviewed external agreement, not an uploaded original signed contract. The original source remains identified by its external reference.
3. **Revise carrier agreement** creates a new immutable revision under the same agreement number/carrier, with fresh external evidence and the reviewed agreement version. Changing the carrier or agreement number requires a separate agreement. Duplicate carrier/number pairs are rejected, including case variants.
4. **Withdraw for new sourcing** records actual carrier evidence and stops further sourcing. The original revisions, their documents, independently confirmed offers, bookings, tenders, and invoices remain. Withdrawal is final for that catalogue record; a new agreement needs its own number. No existing financial obligation is reversed.

The source digest binds the agreement ID, revision, carrier/customer snapshots, exact rate terms, original evidence, reason, actor, and time. Hash and reproducible HTML checks reject corruption. The agreement's current version and original revision digest are required when sourcing; background refresh cannot substitute a newly revised price into an open reviewed form.

## Evaluate and confirm capacity

Open a quote and choose **Find contract rates**. Every agreement is compared on the server against that quote's current plan; inapplicable candidates display reasons. Nothing automatically selects the cheapest carrier or books capacity.

- Country, state/province, and postal codes must match the exact origin-to-destination direction. Case/spacing are normalized, as are US ZIP9 separators. ZIP5 does **not** cover ZIP9; reverse lanes, wildcard/zone searches, and geographic/routing inference are not implemented.
- The shipment must be FTL with matching equipment, within the stated weight and mileage limits, and in the agreement's customer scope. Both pickup and delivery must fall within the inclusive agreement dates. Limits use the declared quote plan; this does not calculate a routed mileage or independently measure freight weight.
- The carrier's current SCAC/EDI partner identity must match the recorded revision, its equipment must qualify, and its insurance must cover delivery and the current date. Expired or withdrawn rates cannot be newly sourced.
- Draft quotes can receive offers. A booked shipment can use replacement procurement only when safely returned to planning and unassigned. The existing custody, financial, load-version, source-review, and fresh canonical Sync controls apply at commitment.

Inspect the exact agreement, then **Confirm capacity at this rate**. The form freezes the selected agreement/version/hash and quote/load versions. Record current carrier capacity contact, reference, findings, and expiry, then acknowledge that capacity and the exact inclusive rate were confirmed. The request cannot provide a caller-selected price, carrier, actor, or rate-source snapshot: those derive from the stored agreement and authenticated operations identity.

Sourcing records a normal carrier offer plus its exact agreement revision/digest. Customer quote issuance, actual customer acceptance, booking, tender, carrier acceptance, and delivery remain separate. Customer issuance cannot outlive the selected capacity offer. Revising or withdrawing the reusable agreement later does not rewrite a separately confirmed price/capacity offer; that offer retains its own expiry, withdrawal, reconfirmation, qualification, and custody controls. Every subsequent commitment verifies the original source terms/hash and exact offer price. Contract replacement procurement changes only the safely unassigned buy agreement and retains the accepted customer sell agreement and previous awards.

These rates are inclusive base prices. Agreement terms must accurately describe the external commitment. Fuel-index formulas, tariff classes, per-mile/per-weight arithmetic, spot-market feeds, automatic accessorials, customer sell-side contracts, and capacity reservation require additional workflows. Ordinary documented additions still use [charge proposals and finance review](charges.md).

## Access and persistence

Creation, revision, withdrawal, capacity attestation, and replacement procurement require `operations.write`. All tenant staff readers can inspect/download the private catalogue. Carrier/customer portal roles cannot reach these staff routes and never receive the catalogue, private contract terms, or source evidence through portal projections. Native customer acceptance does not disclose the carrier's buy contract. Published carrier/customer documents retain their established separate scopes.

Migration `015_carrier_contracts.sql` advances format 12 to **13**, preserving previous business records, file bytes, timestamps, and migration history. It adds `carrier_contracts`; carrier offers have optional retained `rate_source`. Old writers are fenced before they can discard the catalogue or procurement provenance. See [upgrade/rollback](storage.md).

## API

All writes require `Idempotency-Key` and strict bodies. UTC/RFC3339 applies to capacity expiry; agreement dates use `YYYY-MM-DD`.

| Method | Route | Body or result |
| --- | --- | --- |
| POST | `/v1/carrier-contracts` | `{number, carrier_id, rate, contact, reference, note, reason}` |
| POST | `/v1/carrier-contracts/{id}/revise` | Same fields plus `expected_version`; number/carrier fixed |
| POST | `/v1/carrier-contracts/{id}/withdraw` | `{expected_version, contact, reference, note}` |
| GET | `/v1/carrier-contracts/{id}/statement?revision=N` | Exact recorded HTML/hash, attachment/no-store protections; omit revision for current |
| GET | `/v1/quotes/{id}/contract-rates` | Quote/version, offer/replacement context, current booked load version, candidates with applicability/reasons and original digest |
| POST | `/v1/quotes/{id}/contract-offers` | `{expected_version, agreement_id, expected_rate_version, revision_sha256, valid_until, contact, reference, note}` |
| POST | `/v1/quotes/{id}/contract-reprocure` | Same sourcing fields plus `expected_load_version, reason`; fresh source verification before an imported commitment |

`rate` contains `{customer_id?, origin, destination, equipment, max_weight_lbs, max_miles, effective_from, effective_through, amount_cents, terms}`. Lane points are `{country, state, postal_code}`. Missing/null customer scope means all customers. Supported limits are 1–80,000 lbs and 1–10,000 miles; price is 1–100,000,000 integer minor units in the agreement's currency. The period must end today or later and within three years. Terms require 1–2,000 characters; other human evidence fields require 1–200. Staff workspace responses include revision metadata without HTML, original evidence, withdrawal history, and offer/award `rate_source` pointers.

## Verified evidence

Six Rust tests exercise exact 125,001-cent sourcing and independent capacity, applicability and current qualification, stale/forged requests, corruption before issuance/tender, retained old terms after a new 130,001-cent revision/withdrawal, staff-only original downloads, and safe replacement while preserving 150,001-cent customer pricing. PostgreSQL tests race competing uses and revision/withdrawal against sourcing, reject corrupt HTML/source evidence transactionally, reuse failed keys only after restoration, and retain original selections/actor/bytes after reconnect across four owned tenants. Migration qualification includes the previous format-12 database/history.

The actual sibling codec builds a real 204 from a contract-sourced quoted Sync import. Actual-service runtime qualification sources a contract on the canonical Sync shipment, then retains its identity through native customer acceptance, booking, real EDI tender/callbacks, native carrier execution/POD, reviewed finance, safe cancellation, and restart. A separate real order is booked and remotely cancelled before contract replacement; that new endpoint independently fetches canonical source state and holds without changing the quote version, original awards, or buy amount. Synthetic production browsers exercise agreement entry/revision, a concurrent revision while the capacity form is open, refreshed exact-rate sourcing, withdrawal without rewriting the confirmed offer, booking, original-byte retention, mobile layout, viewer restrictions, and partner denial. Historical contract and customer quote requests also verify that the gateway forwards the exact revision. These prove local application/protocol behavior, not production transport certification.