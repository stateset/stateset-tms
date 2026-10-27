# Multi-currency shipments

Each shipment has one ISO-style currency, set when it is created (`currency` on `POST /v1/loads`, a quote plan or a carrier contract rate). It defaults to USD. Every financial record that hangs off the shipment uses that same currency and never converts it: carrier invoices and documents, charges, rate confirmations, cancellation fees, customer bills and supplements, cash entries, credits, carrier credit applications and journal postings.

## Rules

- **Codes and scale.** Any three-letter code is accepted and normalized to upper case. Amounts are integer minor units at the currency's scale from `vendor/stateset-primitives` (`currency_decimal_places`): 2 decimals by default, 0 for JPY/KRW/CLP and similar, 3 for BHD/KWD/OMR and similar, 8 for BTC/ETH. Every amount is limited to 0–100,000,000 minor units. Rounding is never applied.
- **Matching.** A carrier invoice, cash entry or credit whose currency differs from its shipment or bill is refused. Carrier credits only transfer between settlements in the same currency. A contract rate is only a candidate for a quote in the same currency.
- **Amendments.** A customer amendment re-prices the booked shipment in place, so it must keep the shipment's currency. A different currency is a new agreement.
- **Imports.** `POST /v1/sync/import` and `/v1/commerce/import` use the source order's `currency` when the load plan omits one. Freight can be priced in a different currency from the goods, so an explicit plan currency always wins. A missing or malformed order code falls back to USD. The web import form prefills the order's currency.
- **Plan revisions.** `POST /v1/loads/{id}/plan` keeps the load's existing currency when the revised plan omits `currency`. An explicit currency on a planned, uninvoiced load is still honored.
- **Summaries.** `finance.currencies` lists one group per currency. The legacy top-level totals (`receivable_cents`, `overdue_cents`, and so on) are the USD group only, and are zero when a tenant has no USD activity. Nothing is summed across currencies.
- **Compatibility.** Stored USD records serialize without a `currency` field (quote plans, contract rates, catalog settlement rows), so their bytes and digests are unchanged. A missing currency always means USD.

## Interfaces

- **Web.** All amounts are formatted through `web/lib/money.mjs`, which mirrors the server's scale table exactly (browser CLDR data differs for some codes). USD shows as `$1,500.00`. Other currencies keep their code visible, for example `EUR €1,500.00` or `CAD $10.00`, so a dollar sign never stands for a non-USD amount. Load, quote and contract forms take a currency. Quotes from a source shipment and amendments lock it. The cash form picks a bank currency and only lists targets in that currency. Dashboard and finance totals are shown per currency, and margin percentages only appear when a single currency is in view.
- **Terminal.** Billing and Payables rows show USD amounts unchanged and prefix other amounts with their code (`EUR 1500.00`). The summary strip keeps its "USD" labels for USD-only tenants. Otherwise it lists each non-zero currency separately (`1500.00 · EUR 100.00`).
- **CSV.** The load-board export adds a final `Currency` column; see [load exports](load-exports.md).

## Not included

There is no FX conversion, revaluation, multi-currency settlement of one invoice, or currency-specific tax. `stateset_freight_commerce::convert_money` exists for reporting with caller-supplied rates; it is not used in posting.
