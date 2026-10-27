import { currencyCode, minorToDecimal } from "./money.mjs";

// Column types are part of the load-board export contract. Text is guarded
// against formula prefixes; signed monetary values remain numeric decimals in
// the row's own currency, named by the final Currency column.
const headers = [
  "Reference",
  "Customer",
  "Origin",
  "Destination",
  "Status",
  "Carrier",
  "Buy",
  "Sell",
  "Margin",
  "Base buy",
  "Net buy additions and adjustments",
  "Base sell",
  "Net sell additions and credits",
  "Currency",
];
const CURRENCY = headers.length - 1;

function textCell(value) {
  if (typeof value !== "string")
    throw new Error(
      "Shipment export is incomplete. Refresh the workspace and try again.",
    );
  const protect =
    /^[\s\p{Cc}]*[=+\-@＝＋－＠]/u.test(value) || /^[\p{Cc}]/u.test(value);
  return `"${protect ? "'" : ""}${value.replaceAll('"', '""')}"`;
}

function moneyCell(minor, currency) {
  if (!Number.isSafeInteger(minor))
    throw new Error(
      "Export needs exact whole-cent amounts. Refresh the workspace and try again.",
    );
  // Format integer minor units without a floating-point division/rounding.
  return `"${minorToDecimal(minor, currency)}"`;
}

export function loadBoardCsv(rows) {
  const records = rows.map((row) => {
    if (!Array.isArray(row) || row.length !== headers.length)
      throw new Error(
        "Shipment export is incomplete. Refresh the workspace and try again.",
      );
    const currency = currencyCode(row[CURRENCY]);
    return row
      .map((value, column) =>
        column < 6
          ? textCell(value)
          : column === CURRENCY
            ? textCell(currency)
            : moneyCell(value, currency),
      )
      .join(",");
  });
  // UTF-8 BOM preserves names on common spreadsheet CSV import paths. Quoted
  // multiline fields and CRLF records keep the first thirteen columns in place.
  return (
    "\uFEFF" +
    [headers.map(textCell).join(","), ...records].join("\r\n") +
    "\r\n"
  );
}
