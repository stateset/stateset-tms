# Load-board CSV exports

The load-board **Export** action downloads the currently visible, searched/filtered shipments as `stateset-freight-loads.csv`. The first thirteen columns, inclusive commercial totals, original base agreements and net additions/credits retain their positions and meanings. Since 0.2.0 the amount headers no longer say "USD" (for example `Buy` rather than `Buy USD`), and a fourteenth `Currency` column names each row's shipment currency. Rows in different currencies are never converted or summed. Export does not change source records or send data to EDI, Sync or another service.

The serializer distinguishes the first six text columns and the final currency column from the seven amount columns. Amounts are validated as safe whole minor units and formatted with integer arithmetic at the row currency's scale (2 decimals for USD/EUR, 0 for JPY, 3 for BHD; see [multi-currency](multi-currency.md)), including negative margins and credits. Previously, a generic formula guard put an apostrophe before negative amounts, turning real losses into spreadsheet text. Invalid, incomplete or imprecise amounts now prevent the whole download and display an error. Refreshing the load board permits retry while preserving the search selection.

Text fields escape quotes, retain embedded record delimiters, and guard ASCII/full-width formula operators after leading whitespace/control characters. Leading control characters are also guarded. The CSV has a UTF-8 byte-order mark and CRLF record endings. A protective apostrophe becomes part of guarded CSV text; this file is a spreadsheet viewing export, not a byte-for-byte interchange representation of every text field. Original customer and shipment records remain unchanged.

[OWASP's CSV injection guidance](https://community.owasp.org/attacks/CSV_Injection) identifies formula prefixes and warns that spreadsheet applications and save/reopen behavior differ. These guards qualify the tested import path; they do not establish universal Excel or downstream-consumer safety. Spreadsheet numeric precision can also be lower than the serializer's exact integer-cent range.

## Qualification

The production-browser scenarios create synthetic customers and shipments through the real API, filter out a separate shipment, download actual files, and verify that original names, rates and versions survive unchanged. A deliberately corrupted browser response tests the precision guard and successful refresh/retry; it is not evidence that the backend accepts that invalid amount. Node tests additionally cover negative credits, empty selections, full-width/control prefixes, input immutability and exact safe-integer boundary encoding.

An independent Python CSV reader verifies the thirteen columns, exact selected references, multiline Unicode customer text and decimal values against original API-created cents. LibreOffice 6.4.7.2 imports the actual downloads into ODS, whose XML supplies independent cell types, values and formula attributes. All fourteen amount cells in the two-shipment fixture are numeric and agree with the source cents. A `SUM(I2:I3)` added only to separate qualification copies gives:

| Production export | Loss cell type | Spreadsheet margin sum |
| --- | --- | --- |
| Preceding | Text | 1,000.02 USD |
| Corrected | Number | 750.01 USD |
| Original shipment cents | Integer cents | 750.01 USD |

The guarded customer imports as text without a formula. Benign formula controls evaluate to `2`, showing that this configuration can execute formulas; quoted leading-whitespace text already stays text in this installed Calc version. The broader prefix guard is not claimed as a newly demonstrated Calc exploit. Import options follow the [LibreOffice CSV parameter documentation](https://help.libreoffice.org/latest/en-US/text/shared/guide/csv_params.html); installed behavior is established by the controls rather than assumed from the latest manual.

## Reproduce

Use Node.js 22 with dependencies and the Freight API binary available, then run:

```bash
npm --prefix web run test:unit
npm --prefix web run build
FREIGHT_E2E_PRODUCTION=true npm --prefix web run test:e2e
python3 scripts/qualify-load-export.py \
  --artifacts web/test-results \
  --output /tmp/load-export-calc.json
```

Playwright needs an installed Chromium browser or `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH`. The final command requires Python 3 and LibreOffice on `PATH`, creates isolated temporary profiles, and never modifies the original downloads. Optional `--before-artifacts` accepts retained preceding regression downloads for the comparative SUM check. Local temporary artifacts are not a durable production archive.
