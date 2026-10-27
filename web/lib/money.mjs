// Exact minor-unit money helpers shared by every web surface. The scale table
// mirrors `currency_decimal_places` in vendor/stateset-primitives so amounts
// round-trip with the API exactly; Intl/CLDR digits differ for some codes.
const ZERO = new Set([
  "BIF",
  "CLP",
  "DJF",
  "GNF",
  "ISK",
  "JPY",
  "KMF",
  "KRW",
  "PYG",
  "RWF",
  "UGX",
  "UYI",
  "VND",
  "VUV",
  "XAF",
  "XOF",
  "XPF",
]);
const THREE = new Set(["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"]);
const EIGHT = new Set(["BTC", "ETH"]);

// The API stores at most 100,000,000 minor units for any single amount.
export const MAX_MINOR = 100000000;

// Records written before multi-currency omit the field and are USD.
export function currencyCode(currency) {
  if (currency === undefined || currency === null || currency === "")
    return "USD";
  const code = String(currency).trim().toUpperCase();
  if (!/^[A-Z]{3}$/.test(code))
    throw new Error("Currency must be a three-letter code.");
  return code;
}

export function decimalPlaces(currency) {
  const code = currencyCode(currency);
  if (ZERO.has(code)) return 0;
  if (THREE.has(code)) return 3;
  if (EIGHT.has(code)) return 8;
  return 2;
}

// Signed integer minor units to an exact decimal string, e.g. "-1500.01".
export function minorToDecimal(minor, currency) {
  if (!Number.isSafeInteger(minor))
    throw new Error("Amount must be exact whole minor units.");
  const places = decimalPlaces(currency);
  const amount = BigInt(minor);
  const magnitude = amount < 0n ? -amount : amount;
  const sign = amount < 0n ? "-" : "";
  if (places === 0) return `${sign}${magnitude}`;
  const factor = 10n ** BigInt(places);
  const fraction = String(magnitude % factor).padStart(places, "0");
  return `${sign}${magnitude / factor}.${fraction}`;
}

const formatters = new Map();
function formatter(code, places) {
  const key = `${code}:${places}`;
  if (!formatters.has(key)) {
    let format;
    try {
      format = new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: code,
        currencyDisplay: "narrowSymbol",
        minimumFractionDigits: places,
        maximumFractionDigits: places,
      });
    } catch {
      format = null;
    }
    formatters.set(key, format);
  }
  return formatters.get(key);
}

// Display a minor-unit amount in its own currency. Intl receives the exact
// decimal string, so no floating-point division touches the value. Non-USD
// amounts keep the ISO code visible so "$" never stands for CAD/AUD/etc.
export function formatMoney(minor, currency) {
  const code = currencyCode(currency);
  const places = decimalPlaces(code);
  const exact = minorToDecimal(minor, code);
  const format = formatter(code, places);
  if (!format) return `${code} ${exact}`;
  const text = format.format(exact).replace(/ /g, " ");
  return code === "USD" || text.includes(code) ? text : `${code} ${text}`;
}

// Parse a positive operator-entered amount into exact minor units.
export function parseMoney(text, currency, { min = 1, max = MAX_MINOR } = {}) {
  const code = currencyCode(currency);
  const places = decimalPlaces(code);
  const value = String(text ?? "").trim();
  const pattern =
    places === 0 ? /^\d+$/ : new RegExp(`^\\d+(\\.\\d{1,${places}})?$`);
  if (!pattern.test(value))
    throw new Error(
      places === 0
        ? `Use a whole ${code} amount without decimals.`
        : `Use a positive ${code} amount with at most ${places} decimal places.`,
    );
  const [whole, fraction = ""] = value.split(".");
  const minor =
    BigInt(whole) * 10n ** BigInt(places) +
    BigInt(fraction.padEnd(places, "0") || "0");
  if (minor < BigInt(min) || minor > BigInt(max))
    throw new Error(
      `Amount must be between ${code} ${minorToDecimal(min, code)} and ${minorToDecimal(max, code)}.`,
    );
  return Number(minor);
}

// The step attribute for an amount input in this currency.
export function amountStep(currency) {
  const places = decimalPlaces(currency);
  return places === 0 ? "1" : `0.${"0".repeat(places - 1)}1`;
}

// Convert stored minor units back into an editable input value.
export function moneyInput(minor, currency) {
  return minorToDecimal(minor, currency);
}

function joinTotals(sums) {
  if (!sums.size) return formatMoney(0, "USD");
  return [...sums]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([code, minor]) => formatMoney(minor, code))
    .join(" · ");
}

// Total records per currency. Amounts in different currencies are never
// added together; each currency is shown separately and unconverted.
export function formatTotals(records, amount, currency = (r) => r.currency) {
  const sums = new Map();
  for (const record of records) {
    const code = currencyCode(currency(record));
    sums.set(code, (sums.get(code) || 0) + amount(record));
  }
  return joinTotals(sums);
}

// Show a finance-summary figure from its per-currency groups. The legacy
// top-level field is the USD group only and is used when groups are absent.
export function summaryTotals(finance, field) {
  const groups = (finance?.currencies || []).filter((g) => g[field]);
  if (!groups.length) return formatMoney(finance?.[field] || 0, "USD");
  return joinTotals(
    new Map(groups.map((g) => [currencyCode(g.currency), g[field]])),
  );
}

// Distinct currencies across records, used to decide whether a ratio such as
// margin percentage is meaningful without conversion.
export function currencySet(records, currency = (r) => r.currency) {
  return new Set(records.map((record) => currencyCode(currency(record))));
}
