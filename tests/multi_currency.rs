use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{
    domain::{self, Workspace},
    store::Store,
};

async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "finance:test"),
        &domain::id("request"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
async fn try_op(s: &Store, operation: &str, id: &str, body: Value) -> Result<Value, String> {
    s.mutate_authorized(
        ("demo", "finance:test"),
        &domain::id("request"),
        operation,
        id,
        body,
    )
    .await
    .map(|v| v["data"].clone())
}
fn stop(facility: &str, city: &str, state: &str, postal: &str, date: chrono::NaiveDate) -> Value {
    json!({"facility":facility,"city":city,"state":state,"date":date,"address_lines":["1 Dock Street"],"postal_code":postal,"country":"US"})
}
fn eur_plan(reference: &str) -> Value {
    let today = Utc::now().date_naive();
    json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":24000,"miles":180,
        "origin":stop("Portland DC","Portland","OR","97201",today),
        "destination":stop("Seattle DC","Seattle","WA","98101",today),
        "buy_cents":123456,"sell_cents":150000,"currency":"eur"})
}
async fn command(s: &Store, id: &str, action: &str, extra: Value) -> Value {
    let w = read(s).await;
    let version = w.loads.iter().find(|v| v.id == id).expect("load").version;
    let mut body = json!({"action":action,"expected_version":version});
    if let Some(fields) = extra.as_object() {
        for (k, v) in fields {
            body[k] = v.clone();
        }
    }
    op(s, "load.command", id, body).await
}
async fn quote_version(s: &Store, id: &str) -> u64 {
    read(s)
        .await
        .quotes
        .iter()
        .find(|v| v.id == id)
        .expect("quote")
        .version
}
async fn eur_load(s: &Store, reference: &str) -> String {
    let created = op(s, "load.create", "", eur_plan(reference)).await;
    assert_eq!(created["currency"], "EUR");
    created["id"].as_str().expect("load id").to_string()
}
async fn deliver(s: &Store, id: &str) {
    command(s, id, "tender", json!({"carrier_id":"ca4"})).await;
    command(s, id, "accept", json!({})).await;
    let suffix = domain::id("eur");
    let driver = op(
        s,
        "driver.create",
        "",
        json!({"name":format!("EUR Driver {suffix}"),"phone":"555-0199","license_expires":(Utc::now()+Duration::days(365)).date_naive()}),
    )
    .await;
    let unit = op(
        s,
        "equipment.create",
        "",
        json!({"unit":format!("EUR-{suffix}"),"kind":"Dry van","plate":format!("EUR-{suffix}")}),
    )
    .await;
    command(
        s,
        id,
        "dispatch",
        json!({"driver_id":driver["id"],"equipment_id":unit["id"]}),
    )
    .await;
    command(s, id, "pickup", json!({})).await;
    command(
        s,
        id,
        "deliver",
        json!({"pod_reference":"External signed EUR delivery evidence"}),
    )
    .await;
    let w = read(s).await;
    let version = w.loads.iter().find(|v| v.id == id).expect("load").version;
    op(
        s,
        "pod.review",
        id,
        json!({"expected_version":version,"decision":"accepted","note":"Verified external receiver evidence"}),
    )
    .await;
}

#[tokio::test]
async fn eur_load_and_invoice_currency_guards() {
    let s = setup().await;
    for bad in ["EU", "E1R", "EURO"] {
        let mut plan = eur_plan("EUR-BAD");
        plan["currency"] = json!(bad);
        assert!(
            try_op(&s, "load.create", "", plan).await.is_err(),
            "{bad} must be rejected"
        );
    }
    let id = eur_load(&s, "EUR-1").await;
    command(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    command(&s, &id, "accept", json!({})).await;
    let mismatch = try_op(
        &s,
        "invoice.create",
        "",
        json!({"load_id":id,"number":"INV-USD-ON-EUR","amount_cents":120000,"currency":"USD"}),
    )
    .await
    .expect_err("USD invoice on EUR load must fail");
    assert!(mismatch.contains("match"), "{mismatch}");
    let invoice = op(
        &s,
        "invoice.create",
        "",
        json!({"load_id":id,"number":"INV-EUR-1","amount_cents":120000,"currency":"EUR"}),
    )
    .await;
    assert_eq!(invoice["currency"], "EUR");
    // Legacy payloads without a currency keep creating USD shipments.
    let mut legacy = eur_plan("LEGACY-USD");
    legacy.as_object_mut().expect("plan").remove("currency");
    let usd = op(&s, "load.create", "", legacy).await;
    assert_eq!(usd["currency"], "USD");
}

#[tokio::test]
async fn imported_order_currency_defaults_but_never_overrides_the_plan() {
    let s = setup().await;
    let import = |currency: Option<&str>, plan_currency: Option<&str>, reference: &str| {
        let mut order = json!({"id":uuid::Uuid::new_v4().to_string(),"orderNumber":reference,"status":"processing"});
        if let Some(code) = currency {
            order["currency"] = json!(code);
        }
        let mut plan = eur_plan(reference);
        match plan_currency {
            Some(code) => plan["currency"] = json!(code),
            None => {
                plan.as_object_mut().expect("plan").remove("currency");
            }
        }
        json!({"order":order,"load":plan})
    };
    let eur = op(&s, "sync.import", "", import(Some("eur"), None, "IMP-EUR")).await;
    assert_eq!(
        eur["currency"], "EUR",
        "order currency fills an omitted plan"
    );
    let usd = op(
        &s,
        "sync.import",
        "",
        import(Some("EUR"), Some("USD"), "IMP-USD"),
    )
    .await;
    assert_eq!(usd["currency"], "USD", "an explicit freight currency wins");
    let legacy = op(&s, "sync.import", "", import(None, None, "IMP-LEGACY")).await;
    assert_eq!(legacy["currency"], "USD");
    // A malformed upstream code is ignored rather than failing the import.
    let odd = op(&s, "sync.import", "", import(Some("EURO"), None, "IMP-ODD")).await;
    assert_eq!(odd["currency"], "USD");
}

#[tokio::test]
async fn plan_revision_without_currency_keeps_load_currency() {
    let s = setup().await;
    let id = eur_load(&s, "EUR-REPLAN").await;
    let mut plan = eur_plan("EUR-REPLAN");
    plan.as_object_mut().expect("plan").remove("currency");
    plan["sell_cents"] = json!(150100);
    let revised = op(
        &s,
        "load.revise",
        &id,
        json!({"expected_version":1,"plan":plan}),
    )
    .await;
    assert_eq!(
        revised["currency"], "EUR",
        "omitted currency must not reset"
    );
    // An explicit currency on a planned, uninvoiced load is still honored.
    let mut plan = eur_plan("EUR-REPLAN");
    plan["currency"] = json!("GBP");
    let revised = op(
        &s,
        "load.revise",
        &id,
        json!({"expected_version":2,"plan":plan}),
    )
    .await;
    assert_eq!(revised["currency"], "GBP");
}

#[tokio::test]
async fn eur_bill_cash_and_per_currency_summary_never_mix_amounts() {
    let s = setup().await;
    let before = read(&s).await.finance_summary().expect("summary");
    let id = eur_load(&s, "EUR-FIN").await;
    deliver(&s, &id).await;
    let draft = op(
        &s,
        "bill.create",
        "",
        json!({"load_id":id,"number":"BILL-EUR-1","due_date":Utc::now().date_naive()}),
    )
    .await;
    let bill = op(
        &s,
        "bill.issue",
        draft["id"].as_str().expect("bill"),
        json!({"expected_version":1}),
    )
    .await;
    assert_eq!(bill["snapshot"]["currency"], "EUR");
    let bill_id = bill["id"].as_str().expect("bill").to_string();
    let version = bill["version"].as_u64().expect("version");
    let cash = |currency: &str, amount: i64, reference: &str| json!({"direction":"receipt","currency":currency,"account":"Operating","external_reference":reference,"amount_cents":amount,"cleared_on":Utc::now().date_naive(),"note":"Matched cleared external bank statement","allocations":[{"target_id":bill_id,"expected_version":version,"amount_cents":amount}]});
    let mismatch = try_op(&s, "cash.record", "", cash("USD", 50_000, "usd-on-eur"))
        .await
        .expect_err("USD cash on EUR bill must fail");
    assert!(mismatch.contains("currency"), "{mismatch}");
    op(&s, "cash.record", "", cash("EUR", 50_000, "eur-receipt")).await;

    let w = read(&s).await;
    let summary = w.finance_summary().expect("summary");
    assert_eq!(
        summary["receivable_cents"], before["receivable_cents"],
        "USD legacy totals must not absorb EUR balances"
    );
    let eur = summary["currencies"]
        .as_array()
        .expect("currency groups")
        .iter()
        .find(|g| g["currency"] == "EUR")
        .expect("EUR group")
        .clone();
    assert_eq!(eur["net_receivable_cents"], 100_000);
    assert_eq!(eur["receivable_cents"], 100_000);
    assert!(summary["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .any(|r| r["account"] == "accounts_receivable" && r["currency"] == "EUR"));
    assert!(w
        .journal
        .iter()
        .any(|e| e.source_kind == "customer_invoice" && e.currency == "EUR"));
    // Both summary implementations must agree byte for byte.
    let indexed = stateset_freight::finance::FinanceReadIndex::new(&w)
        .summary()
        .expect("indexed summary");
    assert_eq!(indexed, summary);
    // The issued EUR statement labels and scales amounts as EUR.
    let (_, html) = stateset_freight::finance::statement(&w, &bill_id).expect("statement");
    assert!(html.contains("EUR 1500.00"), "{html}");
    assert!(!html.contains("USD 1500.00"), "{html}");
}

#[tokio::test]
async fn eur_standalone_quote_books_shipment_with_stops_and_currency() {
    let s = setup().await;
    let day = (Utc::now() + Duration::days(2)).date_naive();
    let draft = json!({"reference":"QUOTE-EUR","plan":{"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":1000,"miles":173,"currency":"EUR",
        "origin":stop("Portland DC","Portland","OR","97201",day),
        "destination":stop("Seattle DC","Seattle","WA","98101",day),
        "intermediate_stops":[{"id":"stop_tacoma","kind":"delivery","stop":stop("Tacoma receiver","Tacoma","WA","98402",day)}]},
        "sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Inclusive EUR freight quote terms"});
    let q = op(&s, "quote.create", "", draft.clone()).await;
    let id = q["id"].as_str().expect("quote").to_string();
    assert_eq!(q["revisions"][0]["plan"]["currency"], "EUR");
    let q = op(&s,"quote.offer",&id,json!({"expected_version":quote_version(&s,&id).await,"carrier_id":"ca4","amount_cents":125001,"valid_until":Utc::now()+Duration::hours(24),"contact":"Actual carrier dispatch","reference":"EUR-CARRIER-OFFER","note":"Inclusive EUR price from original correspondence"})).await;
    let offer = q["revisions"][0]["offers"][0]["id"].clone();
    op(
        &s,
        "quote.issue",
        &id,
        json!({"expected_version":quote_version(&s,&id).await,"offer_id":offer}),
    )
    .await;
    let (_, html) =
        stateset_freight::quotes::statement(&read(&s).await, &id, None).expect("statement");
    assert!(html.contains("EUR 1500.01"), "{html}");
    let q = op(&s,"quote.decision",&id,json!({"expected_version":quote_version(&s,&id).await,"decision":"accept","contact":"Customer transport lead","reference":"EUR-ACCEPT","note":"Actual customer agreement recorded"})).await;
    op(
        &s,
        "quote.book",
        &id,
        json!({"expected_version":q["version"]}),
    )
    .await;
    let w = read(&s).await;
    let load = w
        .loads
        .iter()
        .find(|l| l.reference == "QUOTE-EUR")
        .expect("booked load");
    assert_eq!(load.currency, "EUR");
    assert_eq!(load.intermediate_stops.len(), 1);
    assert_eq!(load.intermediate_stops[0].id, "stop_tacoma");

    // An amendment re-prices the booked shipment, so it must keep its currency.
    let (load_id, load_version) = (load.id.clone(), load.version);
    let amendment = |currency: &str, reference: &str| {
        let mut plan = draft["plan"].clone();
        plan["currency"] = json!(currency);
        json!({"expected_load_version":load_version,"reference":reference,"plan":plan,
            "sell_cents":150002,"valid_until":Utc::now()+Duration::hours(12),
            "terms":"Amended inclusive freight quote terms","reason":"Customer changed delivery window"})
    };
    let mut usd = amendment("USD", "QUOTE-EUR-A1");
    usd["expected_version"] = json!(quote_version(&s, &id).await);
    let refused = try_op(&s, "quote.amend", &id, usd)
        .await
        .expect_err("USD amendment of a EUR shipment must fail");
    assert!(refused.contains("currency"), "{refused}");
    let mut eur = amendment("eur", "QUOTE-EUR-A2");
    eur["expected_version"] = json!(quote_version(&s, &id).await);
    let child = op(&s, "quote.amend", &id, eur).await;
    assert_eq!(child["revisions"][0]["plan"]["currency"], "EUR");
    assert_eq!(
        read(&s)
            .await
            .loads
            .iter()
            .find(|l| l.id == load_id)
            .expect("load")
            .currency,
        "EUR"
    );
}

#[tokio::test]
async fn eur_contract_is_not_a_candidate_for_usd_quote() {
    let s = setup().await;
    let agreement = json!({"number":"EUR-LANE","carrier_id":"ca4","rate":{"customer_id":"c1","origin":{"country":"US","state":"OR","postal_code":"97201"},"destination":{"country":"US","state":"WA","postal_code":"98101"},"equipment":"Dry van","max_weight_lbs":45000,"max_miles":200,"effective_from":Utc::now().date_naive(),"effective_through":(Utc::now()+Duration::days(30)).date_naive(),"amount_cents":120000,"currency":"EUR","terms":"Inclusive EUR fuel and linehaul; no additional charges"},"contact":"Carrier contracting lead","reference":"EUR-CONTRACT-ORIGINAL","note":"Actual external EUR agreement inspected","reason":"Original reviewed EUR lane agreement"});
    let contract = op(&s, "contract.create", "", agreement).await;
    assert_eq!(contract["revisions"][0]["rate"]["currency"], "EUR");
    let day = (Utc::now() + Duration::days(2)).date_naive();
    let draft = json!({"reference":"QUOTE-USD","plan":{"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":1000,"miles":173,
        "origin":stop("Portland DC","Portland","OR","97201",day),
        "destination":stop("Seattle DC","Seattle","WA","98101",day)},
        "sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Customer inclusive quote terms"});
    let q = op(&s, "quote.create", "", draft).await;
    let w = read(&s).await;
    let candidates = w
        .contract_candidates(q["id"].as_str().expect("quote"))
        .expect("candidates");
    let eur = candidates["rates"]
        .as_array()
        .expect("rates")
        .iter()
        .find(|r| r["number"] == "EUR-LANE")
        .expect("EUR contract candidate")
        .clone();
    assert_eq!(eur["eligible"], false);
    assert!(eur["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .any(|r| r == "Currency does not match"));
}
