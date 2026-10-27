# Getting started: your first shipment

This walkthrough takes one truckload from plan to paid in about ten minutes. You create the shipment, tender it to a carrier, dispatch a driver, track it, prove delivery, invoice the customer and record the payment. Every step calls the real API against the synthetic demo workspace. Nothing leaves your machine.

You need Rust 1.90+, Node.js 22 and `curl`. The JSON helpers below use `node`, which the web app already requires.

## 1. Start the demo API

From the repository root:

```sh
cargo build --locked
FREIGHT_DEMO=true FREIGHT_BIND_ADDR=127.0.0.1:8090 \
  FREIGHT_API_KEYS_JSON='{}' FREIGHT_INTEGRATIONS_JSON='{}' \
  ./target/debug/stateset-freight
```

Demo mode seeds three customers, four carriers, three drivers and eight shipments in memory, and accepts the fixed credential `freight-local-demo`. Restarting resets everything. Leave the API running and open a second terminal for the rest of the guide.

To watch the same data in a browser, run `bash scripts/dev.sh` instead and open <http://127.0.0.1:3010>.

## 2. Set up your shell

```sh
API=${API:-http://127.0.0.1:8090}
AUTH='Authorization: Bearer freight-local-demo'
TODAY=$(date -u +%F)

# POST a JSON body. Every mutation needs an Idempotency-Key.
post() {
  curl -sS -X POST "$API$1" -H "$AUTH" -H 'Content-Type: application/json' \
    -H "Idempotency-Key: $2" -d "$3"
}
# Print one field from JSON on stdin, e.g. `field data.status`.
field() {
  node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>{const r=JSON.parse(s);if(r.error){console.error(r.error.message);process.exit(1)}console.log(process.argv[1].split(".").reduce((v,k)=>v[k],r))})' "$1"
}
# Current version of a shipment. Commands must name the version they saw.
version() {
  curl -sS -H "$AUTH" "$API/v1/workspace" |
    node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).loads.find(l=>l.id===process.argv[1]).version))' "$1"
}

curl -sS "$API/health"
```

`/health` needs no credential and prints `{"service":"stateset-freight","status":"ok"}`.

## 3. Plan the shipment

A plan names the customer, lane, equipment and the agreed buy (carrier) and sell (customer) rates. Money is always in minor units: `150000` is $1,500.00.

```sh
LOAD=$(post /v1/loads first-load "{
  \"reference\": \"GS-1001\", \"customer_id\": \"c1\", \"mode\": \"FTL\",
  \"equipment\": \"Flatbed\", \"commodity\": \"Steel coils\",
  \"weight_lbs\": 24000, \"miles\": 180,
  \"origin\": {\"facility\": \"Portland DC\", \"city\": \"Portland\", \"state\": \"OR\",
    \"date\": \"$TODAY\", \"address_lines\": [\"15 Dock Street\"], \"postal_code\": \"97201\", \"country\": \"US\"},
  \"destination\": {\"facility\": \"Seattle DC\", \"city\": \"Seattle\", \"state\": \"WA\",
    \"date\": \"$TODAY\", \"address_lines\": [\"25 Receiving Ave\"], \"postal_code\": \"98101\", \"country\": \"US\"},
  \"buy_cents\": 120000, \"sell_cents\": 150000
}" | field data.id)
echo "$LOAD"
```

The shipment starts as `planned` at version 1. Add `"currency": "EUR"` (or any ISO 4217 code) to plan in another currency; every invoice and payable on the shipment then uses it. See [multi-currency shipments](multi-currency.md).

Run the same request again with the same key and you get the original response back with `"replayed": true`. Change the body under that key and the API returns 409. Retries are always safe.

## 4. Tender, accept and dispatch

Each load command carries an `action` and the `expected_version` you last saw. If someone else changed the shipment first, you get a 409 instead of overwriting their work.

```sh
command() {
  post "/v1/loads/$LOAD/commands" "first-load-$1" \
    "{\"action\": \"$1\", \"expected_version\": $(version "$LOAD")${2:+, $2}}" |
    field data.status
}

command tender '"carrier_id": "ca3"'                         # Atlas Freight Lines → tendered
command accept                                              # carrier accepts → accepted
command dispatch '"driver_id": "d3", "equipment_id": "e3"'   # Sam Rivera, TRK-316 → dispatched
```

Tendering checks that the carrier runs the required equipment and that its insurance covers the delivery date. Dispatch checks the driver's license, the equipment type and that neither is already on an active load. Try `command dispatch '"driver_id": "d1", "equipment_id": "e3"'` on another shipment to see the conflict message.

## 5. Track and deliver

```sh
command pickup                                      # → in_transit, location Portland, OR
command location '"location": "Olympia, WA"'         # → in_transit
command deliver '"pod_reference": "POD-GS-1001"'     # → delivered
```

Delivery needs proof: a POD reference here, or an uploaded document (`POST /v1/loads/{id}/documents?kind=pod`). Multi-stop shipments must confirm every intermediate stop first; see [stops](stops.md).

## 6. Invoice the customer

Draft the invoice. Its amount comes from the agreed sell rate, never from the request.

```sh
BILL=$(post /v1/bills first-bill "{
  \"load_id\": \"$LOAD\", \"number\": \"INV-GS-1001\",
  \"due_date\": \"$(date -u -d '+30 days' +%F 2>/dev/null || date -u -v+30d +%F)\"
}" | field data.id)
```

Issuing is gated on finance accepting the delivery evidence. Without that review, issuing fails with *Reviewed delivery evidence required before issuing a customer invoice*. Review the POD, then issue:

```sh
post "/v1/loads/$LOAD/pod/review" first-pod "{
  \"expected_version\": $(version "$LOAD"),
  \"decision\": \"accepted\", \"note\": \"Signed POD matches the delivery\"
}" | field data.pod_review.decision                      # → accepted

post "/v1/bills/$BILL/issue" first-issue '{"expected_version": 1}' | field data.status   # → issued

curl -sS -H "$AUTH" "$API/v1/bills/$BILL/statement" -o INV-GS-1001.html
curl -sS -H "$AUTH" "$API/v1/finance/journal.csv" | grep customer_invoice | tail -2
```

Issuing freezes the invoice and posts a balanced journal entry: a $1,500.00 debit to `accounts_receivable` and a matching credit to `freight_revenue`. Open `INV-GS-1001.html` to see the printable statement. It excludes your buy cost and internal notes.

## 7. Record the payment

Cash entries mirror a real bank transaction. The allocations must exactly total the bank amount.

```sh
post /v1/cash first-cash "{
  \"direction\": \"receipt\", \"currency\": \"USD\", \"account\": \"operating-usd\",
  \"external_reference\": \"BANK-GS-1001\", \"amount_cents\": 150000,
  \"cleared_on\": \"$TODAY\", \"note\": \"Matched against the bank statement\",
  \"allocations\": [{\"target_id\": \"$BILL\", \"expected_version\": 2, \"amount_cents\": 150000}],
  \"corrects_id\": null
}" | field data.direction                                 # → receipt

curl -sS -H "$AUTH" "$API/v1/workspace" | field finance.receivable_cents   # → 0
```

The invoice is now `paid` and the receivable is back to zero. Mistakes are reversed rather than edited: `POST /v1/cash/{id}/reverse` appends opposite postings and keeps the original.

## Where to go next

| To… | Read |
| --- | --- |
| Quote a customer and source carriers before booking | [Quotes](quotes.md), [carrier contracts](carrier-contracts.md) |
| Add detention, lumper or fuel charges | [Charges](charges.md) |
| Audit carrier invoices and pay carriers | [Payables](payables.md), [finance](finance.md) |
| Exchange 204/990/214/210 with carriers over EDI | [Tenders](tenders.md), [integrations](integrations.md) |
| Work the same shipments from a terminal | [Terminal guide](terminal.md) |
| Drive the TMS from an AI agent | [MCP adapter](api.md#tms-mcp-adapter) |
| Every route and request shape | [API reference](api.md) |
| Run against PostgreSQL with real users | [Storage](storage.md), [access](access.md), [SSO](sso.md) |
