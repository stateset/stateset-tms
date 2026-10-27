import test from "node:test";
import assert from "node:assert/strict";
import { loadBoardCsv } from "../lib/load-board-csv.mjs";

const row = () => [
  "LOAD-1",
  "Customer",
  "Portland, OR",
  "Seattle, WA",
  "planned",
  "Unassigned",
  150001,
  125000,
  -25001,
  150001,
  0,
  125000,
  -1,
  "USD",
];

test("load CSV exports losses and credits as signed numeric decimal values", () => {
  assert.ok(
    loadBoardCsv([row()]).endsWith(
      '"1500.01","1250.00","-250.01","1500.01","0.00","1250.00","-0.01","USD"\r\n',
    ),
  );
});

test("load CSV protects formula/control prefixes in text including whitespace and full-width operators", () => {
  for (const [input, expected] of [
    ["=1+1", '"\'=1+1"'],
    [" +1", '"\' +1"'],
    ["-1", '"\'-1"'],
    ["@SUM(1)", '"\'@SUM(1)"'],
    [" \t=1+1", '"\' \t=1+1"'],
    ["\r\n=1+1", '"\'\r\n=1+1"'],
    ["\u0000=1+1", '"\'\u0000=1+1"'],
    ["＝1+1", '"\'＝1+1"'],
    [" ＋1", '"\' ＋1"'],
    ["－1", '"\'－1"'],
    ["＠SUM(1)", '"\'＠SUM(1)"'],
    ["\tPlain text", '"\'\tPlain text"'],
  ]) {
    const values = row();
    values[1] = input;
    assert.ok(loadBoardCsv([values]).includes(expected), JSON.stringify(input));
  }
});

test("load CSV retains Unicode, quotes and embedded record delimiters without changing its inputs", () => {
  const values = row();
  values[1] = 'Résumé, "North"\r\nDock 2';
  Object.freeze(values);
  const source = structuredClone(values);
  const csv = loadBoardCsv([values]);
  assert.ok(csv.startsWith('\uFEFF"Reference","Customer",'));
  assert.ok(csv.includes('"Résumé, ""North""\r\nDock 2"'));
  assert.deepEqual(values, source);
  assert.equal(
    loadBoardCsv([]).split("\r\n").length,
    2,
    "empty selection retains one header record",
  );
});

test("load CSV encodes whole cents exactly at safe integer boundaries", () => {
  const values = row();
  values[6] = Number.MAX_SAFE_INTEGER;
  values[7] = -Number.MAX_SAFE_INTEGER;
  values[8] = -0;
  assert.ok(
    loadBoardCsv([values]).includes(
      '"90071992547409.91","-90071992547409.91","0.00"',
    ),
  );
});

test("load CSV refuses incomplete or imprecise amounts instead of creating a partial export", () => {
  for (const invalid of [
    undefined,
    null,
    "125000",
    NaN,
    Infinity,
    -Infinity,
    1.5,
    Number.MAX_SAFE_INTEGER + 1,
  ]) {
    const values = row();
    values[6] = invalid;
    assert.throws(
      () => loadBoardCsv([row(), values]),
      /exact whole-cent amounts/,
    );
  }
  assert.throws(() => loadBoardCsv([row().slice(0, -1)]), /incomplete/);
  const values = row();
  values[1] = null;
  assert.throws(() => loadBoardCsv([values]), /incomplete/);
});

test("load CSV scales each row by its own currency and names it", () => {
  const eur = row();
  eur[13] = "eur";
  const jpy = row();
  jpy[13] = "JPY";
  const legacy = row();
  legacy[13] = undefined;
  const [, header, euro, yen, usd] = loadBoardCsv([eur, jpy, legacy]).split(
    /\uFEFF|\r\n/,
  );
  assert.ok(header.endsWith('"Net sell additions and credits","Currency"'));
  assert.ok(!header.includes("USD"));
  assert.ok(euro.endsWith('"1250.00","-0.01","EUR"'));
  assert.ok(
    yen.startsWith('"LOAD-1"') && yen.includes('"150001","125000","-25001"'),
  );
  assert.ok(yen.endsWith('"JPY"'));
  assert.ok(usd.endsWith('"-0.01","USD"'));
  const bad = row();
  bad[13] = "EURO";
  assert.throws(() => loadBoardCsv([bad]), /three-letter/);
});
