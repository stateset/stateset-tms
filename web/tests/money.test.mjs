import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  amountStep,
  currencyCode,
  currencySet,
  decimalPlaces,
  formatMoney,
  formatTotals,
  minorToDecimal,
  parseMoney,
  summaryTotals,
} from "../lib/money.mjs";

test("missing currency is the legacy USD default", () => {
  assert.equal(currencyCode(undefined), "USD");
  assert.equal(currencyCode(""), "USD");
  assert.equal(currencyCode(" eur "), "EUR");
  assert.throws(() => currencyCode("EURO"), /three-letter/);
});

test("scales match the API currency table, not CLDR", () => {
  assert.equal(decimalPlaces("USD"), 2);
  assert.equal(decimalPlaces("JPY"), 0);
  assert.equal(decimalPlaces("BHD"), 3);
  assert.equal(decimalPlaces("BTC"), 8);
  // CLDR uses 0 digits for HUF cash amounts; the API keeps 2.
  assert.equal(decimalPlaces("HUF"), 2);
});

// money.mjs hand-copies currency_decimal_places from the vendored iCommerce
// primitives. Parse both sources so a change to either fails here instead of
// silently mis-scaling amounts in the browser.
test("scale table matches vendored stateset-primitives exactly", () => {
  const rust = readFileSync(
    new URL("../../vendor/stateset-primitives/src/money.rs", import.meta.url),
    "utf8",
  );
  const body = rust.match(
    /pub const fn currency_decimal_places[\s\S]*?match code\.as_bytes\(\) \{([\s\S]*?)\n {4}\}/,
  );
  assert.ok(body, "currency_decimal_places match block");
  const upstream = new Map();
  for (const arm of body[1].matchAll(/((?:\|?\s*b"[A-Z]{3}"\s*)+)=>\s*(\d+)/g)) {
    for (const code of arm[1].matchAll(/b"([A-Z]{3})"/g))
      upstream.set(code[1], Number(arm[2]));
  }
  assert.ok(upstream.size >= 26, "parsed every explicit upstream code");
  assert.match(body[1], /_ => 2,/, "unknown codes default to two decimals");
  for (const [code, places] of upstream)
    assert.equal(decimalPlaces(code), places, code);

  const web = readFileSync(new URL("../lib/money.mjs", import.meta.url), "utf8");
  const listed = new Set(
    [...web.matchAll(/const (?:ZERO|THREE|EIGHT) = new Set\(\[([^\]]*)\]\)/g)]
      .flatMap((set) => [...set[1].matchAll(/"([A-Z]{3})"/g)].map((code) => code[1])),
  );
  assert.deepEqual([...listed].sort(), [...upstream.keys()].sort());
});

test("exact decimal conversion never uses floating point", () => {
  assert.equal(minorToDecimal(150001, "USD"), "1500.01");
  assert.equal(minorToDecimal(-1, "EUR"), "-0.01");
  assert.equal(minorToDecimal(150001, "JPY"), "150001");
  assert.equal(minorToDecimal(1500001, "BHD"), "1500.001");
  assert.throws(() => minorToDecimal(1.5, "USD"), /whole minor units/);
});

test("formatting labels every amount with its own currency", () => {
  assert.equal(formatMoney(150001, "USD"), "$1,500.01");
  assert.equal(formatMoney(150001, undefined), "$1,500.01");
  assert.equal(formatMoney(150000, "EUR"), "EUR €1,500.00");
  assert.equal(formatMoney(150000, "JPY"), "JPY ¥150,000");
  assert.equal(formatMoney(1500001, "BHD"), "BHD 1,500.001");
  // Dollar-symbol currencies must never be confused with USD.
  assert.match(formatMoney(100, "CAD"), /^CAD /);
  assert.equal(formatMoney(-2501, "USD"), "-$25.01");
});

test("parsing respects each currency's scale and the API range", () => {
  assert.equal(parseMoney("1500.01", "USD"), 150001);
  assert.equal(parseMoney("1500", "EUR"), 150000);
  assert.equal(parseMoney("1500", "JPY"), 1500);
  assert.equal(parseMoney("1.001", "BHD"), 1001);
  assert.throws(() => parseMoney("1.5", "JPY"), /whole JPY/);
  assert.throws(() => parseMoney("1.001", "EUR"), /at most 2 decimal/);
  assert.throws(() => parseMoney("0", "USD"), /between USD 0.01/);
  assert.throws(() => parseMoney("1000000.01", "USD"), /1000000.00/);
  assert.throws(() => parseMoney("-1", "USD"), /positive/);
  assert.equal(parseMoney("0", "USD", { min: 0 }), 0);
});

test("input step follows the currency scale", () => {
  assert.equal(amountStep("USD"), "0.01");
  assert.equal(amountStep("JPY"), "1");
  assert.equal(amountStep("BHD"), "0.001");
});

test("totals are grouped per currency and never added across them", () => {
  const loads = [
    { currency: "USD", amount: 100 },
    { amount: 50 },
    { currency: "EUR", amount: 200 },
  ];
  assert.equal(
    formatTotals(loads, (l) => l.amount),
    "EUR €2.00 · $1.50",
  );
  assert.equal(
    formatTotals([], (l) => l.amount),
    "$0.00",
  );
  assert.deepEqual([...currencySet(loads)].sort(), ["EUR", "USD"]);
});

test("finance summary totals use per-currency groups", () => {
  const finance = {
    receivable_cents: 0,
    currencies: [
      { currency: "EUR", receivable_cents: 100000 },
      { currency: "USD", receivable_cents: 0 },
    ],
  };
  assert.equal(summaryTotals(finance, "receivable_cents"), "EUR €1,000.00");
  assert.equal(summaryTotals({ overdue_cents: 5 }, "overdue_cents"), "$0.05");
  assert.equal(summaryTotals({}, "overdue_cents"), "$0.00");
});
