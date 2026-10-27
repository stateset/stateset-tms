#[path = "support/canonical.rs"]
mod canonical;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{
    domain::{self, Workspace},
    quotes,
    store::Store,
};

async fn source_quote(s: &Store, reference: &str) -> (Value, Value, Value) {
    let order = json!({"id":uuid::Uuid::new_v4().to_string(),"orderNumber":"COMMERCE-ORIGINAL","status":"pending","grandTotal":2345,"metadata":{"source":"immutable source evidence"}});
    let mut load = draft("IGNORED")["plan"].clone();
    load["reference"] = json!("SOURCE-LOAD");
    load["buy_cents"] = json!(90000);
    load["sell_cents"] = json!(140000);
    let source = command(s, "sync.import", "", json!({"order":order,"load":load})).await;
    let mut body = draft(reference);
    body["planned_load_id"] = source["id"].clone();
    body["expected_load_version"] = source["version"].clone();
    let q = command(s, "quote.create", "", body).await;
    let id = q["id"].as_str().expect("quote");
    let q = offer(s, id, "ca4", 125001, "SOURCE-CARRIER-OFFER").await;
    let offer = q["revisions"][0]["offers"][0]["id"]
        .as_str()
        .expect("offer");
    issue(s, id, offer).await;
    let q = decide(s, id, "accept").await;
    (q, source, order)
}
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
fn draft(reference: &str) -> Value {
    let day = (Utc::now() + Duration::days(2)).date_naive();
    json!({"reference":reference,"plan":{"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"}},"sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Inclusive USD freight quote <script>terms</script>; acceptance and booking before deadline"})
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("workspace")
}
async fn command(s: &Store, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(("demo", "user:quote-ops"), &domain::id("key"), op, id, body)
        .await
        .expect(op)["data"]
        .clone()
}
async fn create(s: &Store, reference: &str) -> Value {
    command(s, "quote.create", "", draft(reference)).await
}
async fn version(s: &Store, id: &str) -> u64 {
    read(s)
        .await
        .quotes
        .iter()
        .find(|v| v.id == id)
        .expect("quote")
        .version
}
async fn offer(s: &Store, id: &str, carrier: &str, amount: i64, reference: &str) -> Value {
    command(s,"quote.offer",id,json!({"expected_version":version(s,id).await,"carrier_id":carrier,"amount_cents":amount,"valid_until":Utc::now()+Duration::hours(24),"contact":"Actual carrier dispatch","reference":reference,"note":"Inclusive price from original email RATE-PRIVATE"})).await
}
async fn issue(s: &Store, id: &str, offer_id: &str) -> Value {
    command(
        s,
        "quote.issue",
        id,
        json!({"expected_version":version(s,id).await,"offer_id":offer_id}),
    )
    .await
}
async fn decide(s: &Store, id: &str, decision: &str) -> Value {
    command(s,"quote.decision",id,json!({"expected_version":version(s,id).await,"decision":decision,"contact":"Customer transport lead","reference":format!("CUSTOMER-{decision}"),"note":"Actual customer agreement recorded from original correspondence"})).await
}
async fn accepted(s: &Store, reference: &str) -> Value {
    let q = create(s, reference).await;
    let id = q["id"].as_str().expect("quote");
    let q = offer(s, id, "ca4", 125001, "CARRIER-ORIGINAL").await;
    let offer = q["revisions"][0]["offers"][0]["id"]
        .as_str()
        .expect("offer");
    issue(s, id, offer).await;
    decide(s, id, "accept").await
}

#[tokio::test]
async fn quote_acceptance_books_exact_selected_rates_once_and_tenders_only_the_quoted_carrier() {
    let s = setup().await;
    let baseline = read(&s).await;
    let q = create(&s, "QUOTE-EXACT").await;
    let id = q["id"].as_str().expect("quote");
    offer(&s, id, "ca1", 110001, "CHEAPER-NOT-SELECTED").await;
    let q = offer(&s, id, "ca4", 125001, "EXPLICITLY-SELECTED").await;
    let chosen = q["revisions"][0]["offers"][1]["id"]
        .as_str()
        .expect("offer");
    let mut without_issuer = read(&s).await;
    without_issuer.billing_profile = None;
    assert!(without_issuer.apply_quote("quote.issue",id,json!({"expected_version":without_issuer.quotes.iter().find(|v|v.id==id).expect("quote").version,"offer_id":chosen}),"ops").is_err());
    issue(&s, id, chosen).await;
    let before = read(&s).await;
    assert_eq!(before.loads.len(), baseline.loads.len());
    assert_eq!(before.outbox.len(), baseline.outbox.len());
    assert_eq!(before.journal.len(), baseline.journal.len());
    decide(&s, id, "accept").await;
    let input = json!({"expected_version":version(&s,id).await});
    let booked = s
        .mutate_authorized(
            ("demo", "user:quote-ops"),
            "book-once",
            "quote.book",
            id,
            input.clone(),
        )
        .await
        .expect("book");
    let replay = s
        .mutate("demo", "book-once", "quote.book", id, input)
        .await
        .expect("lost response replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(booked["data"], replay["data"]);
    let load_id = booked["data"]["booked_load_id"].as_str().expect("load");
    let w = read(&s).await;
    let load = w.loads.iter().find(|v| v.id == load_id).expect("booked");
    assert_eq!(load.status, domain::Status::Planned);
    assert!(load.carrier_id.is_none());
    assert_eq!((load.buy_cents, load.sell_cents), (125001, 150001));
    assert_eq!(w.loads.len(), baseline.loads.len() + 1);
    assert_eq!(w.outbox.len(), baseline.outbox.len());
    assert_eq!(w.journal.len(), baseline.journal.len());
    assert!(s
        .mutate(
            "demo",
            "wrong-carrier",
            "load.command",
            load_id,
            json!({"action":"tender","expected_version":1,"carrier_id":"ca1"})
        )
        .await
        .is_err());
    assert!(s
        .mutate(
            "demo",
            "silent-edit",
            "load.revise",
            load_id,
            json!({"expected_version":1,"plan":{}})
        )
        .await
        .is_err());
    let statement = quotes::statement(&w, id, None).expect("customer quote").1;
    assert!(statement.contains("1500.01"));
    assert!(statement.contains("&lt;script&gt;"));
    assert!(!statement.contains("1250.01"));
    assert!(!statement.contains("Blue Ridge"));
    assert!(!statement.contains("RATE-PRIVATE"));
    command(
        &s,
        "load.command",
        load_id,
        json!({"action":"tender","expected_version":1,"carrier_id":"ca4"}),
    )
    .await;
    let w = read(&s).await;
    let tender = w
        .tenders
        .iter()
        .find(|v| v.load_id == load_id)
        .expect("actual tender");
    assert_eq!(tender.plan.buy_cents, 125001);
    assert_eq!(tender.carrier.id, "ca4");
    assert!(w
        .events
        .iter()
        .filter(|e| e.entity_id == load_id)
        .all(|e| e.actor == "user:quote-ops"));
    assert_eq!(w.journal.len(), baseline.journal.len());
}

#[tokio::test]
async fn quote_revisions_and_customer_withdrawal_keep_prior_acceptance_offers_and_documents() {
    let s = setup().await;
    let q = accepted(&s, "QUOTE-HISTORY").await;
    let id = q["id"].as_str().expect("quote");
    let mut revision = draft("QUOTE-HISTORY");
    revision
        .as_object_mut()
        .expect("object")
        .remove("reference");
    revision["expected_version"] = json!(version(&s, id).await);
    revision["reason"] = json!("Changed shipment plan");
    revision["sell_cents"] = json!(160001);
    let before = json!(read(&s).await);
    assert!(s
        .mutate(
            "demo",
            "implicit-change",
            "quote.revise",
            id,
            revision.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
    decide(&s, id, "withdraw").await;
    revision["expected_version"] = json!(version(&s, id).await);
    let q = command(&s, "quote.revise", id, revision).await;
    assert_eq!(q["revisions"].as_array().expect("revisions").len(), 2);
    assert_eq!(q["revisions"][0]["decision_history"][0][0], "accept");
    assert_eq!(q["revisions"][0]["decision_history"][1][0], "withdraw");
    assert_eq!(q["revisions"][0]["sell_cents"], 150001);
    assert_eq!(q["revisions"][1]["sell_cents"], 160001);
    assert_eq!(q["revisions"][1]["offers"], json!([]));
    assert_eq!(q["status"], "draft");
    let w = read(&s).await;
    let old = quotes::statement(&w, id, Some(1)).expect("old quote").1;
    assert!(old.contains("HISTORICAL QUOTE REVISION"));
    assert!(old.contains("1500.01"));
    assert!(!old.contains("1600.01"));
    assert!(quotes::statement(&w, id, Some(99)).is_err());
}

#[tokio::test]
async fn quote_guards_reject_unconfirmed_stale_wrong_revision_and_unqualified_offers_atomically() {
    let s = setup().await;
    let q = create(&s, "QUOTE-GUARDS").await;
    let id = q["id"].as_str().expect("quote");
    let before = json!(read(&s).await);
    for (field, value) in [
        ("amount_cents", json!(0)),
        ("amount_cents", json!(-1)),
        ("amount_cents", json!(100000001)),
        ("carrier_id", json!("ca2")),
        ("contact", json!("")),
        ("valid_until", json!(Utc::now() - Duration::seconds(1))),
        ("expected_version", json!(0)),
        ("actor", json!("forged")),
    ] {
        let mut body = json!({"expected_version":1,"carrier_id":"ca4","amount_cents":125001,"valid_until":Utc::now()+Duration::hours(24),"contact":"Actual contact","reference":"ACTUAL","note":"Actual rate proof"});
        body[field] = value;
        assert!(s
            .mutate("demo", &domain::id("bad"), "quote.offer", id, body)
            .await
            .is_err());
        assert_eq!(json!(read(&s).await), before);
    }
    assert!(s
        .mutate(
            "demo",
            "unaccepted-book",
            "quote.book",
            id,
            json!({"expected_version":1})
        )
        .await
        .is_err());
    let q = offer(&s, id, "ca4", 125001, "GUARD-OFFER").await;
    let offer = q["revisions"][0]["offers"][0]["id"]
        .as_str()
        .expect("offer");
    command(&s,"quote.offer.withdraw",id,json!({"expected_version":version(&s,id).await,"offer_id":offer,"contact":"Carrier dispatch","reference":"WITHDRAWN","note":"Carrier retracted its quote"})).await;
    assert!(s
        .mutate(
            "demo",
            "withdrawn-offer",
            "quote.issue",
            id,
            json!({"expected_version":version(&s,id).await,"offer_id":offer})
        )
        .await
        .is_err());
    assert!(s
        .mutate(
            "other",
            "other-tenant",
            "quote.book",
            id,
            json!({"expected_version":version(&s,id).await})
        )
        .await
        .is_err());
}

#[tokio::test]
async fn expired_and_changed_carrier_quotes_hold_booking_and_same_rate_reconfirmation_restores_tender_eligibility(
) {
    let s = setup().await;
    let q = accepted(&s, "QUOTE-EXPIRY").await;
    let id = q["id"].as_str().expect("quote");
    let mut w = read(&s).await;
    let index = w.quotes.iter().position(|q| q.id == id).expect("index");
    let version = w.quotes[index].version;
    let original = w.quotes[index].revisions[0].valid_until;
    w.quotes[index].revisions[0].valid_until = Utc::now() - Duration::seconds(1);
    assert_eq!(w.quotes[index].effective_status(), "expired");
    assert!(w
        .apply_quote("quote.book", id, json!({"expected_version":version}), "ops")
        .is_err());
    w.quotes[index].revisions[0].valid_until = original;
    w.quotes[index].revisions[0].offers[0].carrier.scac = "DIFF".into();
    assert!(w
        .apply_quote("quote.book", id, json!({"expected_version":version}), "ops")
        .is_err());
    w.quotes[index].revisions[0].offers[0].carrier.scac = read(&s).await.quotes[index].revisions[0]
        .offers[0]
        .carrier
        .scac
        .clone();
    let q = w
        .apply_quote("quote.book", id, json!({"expected_version":version}), "ops")
        .expect("book");
    let load_id = q["booked_load_id"].as_str().expect("load");
    w.quotes[index].procurements[0].offer.valid_until = Utc::now() - Duration::seconds(1);
    let load = w
        .loads
        .iter()
        .find(|v| v.id == load_id)
        .expect("load")
        .clone();
    let carrier = w
        .carriers
        .iter()
        .find(|v| v.id == "ca4")
        .expect("carrier")
        .clone();
    assert!(w.verify_quoted_tender(&load, &carrier).is_err());
    let q=w.apply_quote("quote.reconfirm",id,json!({"expected_version":version+1,"expected_load_version":load.version,"valid_until":Utc::now()+Duration::hours(24),"contact":"Carrier dispatch","reference":"SAME-RATE-EXTENDED","note":"Actual same carrier/price validity extension"}),"ops").expect("reconfirmed");
    assert_eq!(q["reconfirmations"].as_array().expect("history").len(), 1);
    w.verify_quoted_tender(&load, &carrier)
        .expect("valid again");
    assert_eq!(w.quotes[index].revisions[0].offers[0].amount_cents, 125001);
}

#[tokio::test]
async fn replacement_procurement_requires_safe_release_and_preserves_customer_and_original_carrier_agreements(
) {
    let s = setup().await;
    let q = accepted(&s, "QUOTE-REPROCURE").await;
    let id = q["id"].as_str().expect("quote");
    let q = command(
        &s,
        "quote.book",
        id,
        json!({"expected_version":version(&s,id).await}),
    )
    .await;
    let load_id = q["booked_load_id"].as_str().expect("load");
    command(
        &s,
        "load.command",
        load_id,
        json!({"action":"tender","carrier_id":"ca4","expected_version":1}),
    )
    .await;
    command(
        &s,
        "load.command",
        load_id,
        json!({"action":"accept","expected_version":2}),
    )
    .await;
    let input = json!({"expected_version":version(&s,id).await,"expected_load_version":3,"carrier_id":"ca1","amount_cents":130001,"valid_until":Utc::now()+Duration::hours(24),"contact":"Replacement carrier dispatch","reference":"REPLACEMENT-OFFER","note":"Actual inclusive replacement carrier quote","reason":"Original carrier confirmed it cannot cover the shipment"});
    let before = json!(read(&s).await);
    assert!(s
        .mutate(
            "demo",
            "unsafe-replacement",
            "quote.reprocure",
            id,
            input.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
    let release = command(
        &s,
        "tender.release.request",
        load_id,
        json!({"expected_load_version":3,"disposition":"replan","reason":"Carrier cannot cover"}),
    )
    .await;
    command(&s,"tender.release.confirm",release["id"].as_str().expect("release"),json!({"expected_load_version":4,"carrier_contact":"Original carrier dispatch","confirmation_reference":"ORIGINAL-RELEASED","note":"Actual carrier confirmation; no freight in custody"})).await;
    let mut input = input;
    input["expected_load_version"] = json!(5);
    let q = command(&s, "quote.reprocure", id, input).await;
    assert_eq!(q["procurements"].as_array().expect("history").len(), 2);
    assert_eq!(q["revisions"][0]["offers"][0]["amount_cents"], 125001);
    assert_eq!(q["procurements"][0]["offer"]["carrier"]["id"], "ca4");
    assert_eq!(q["procurements"][1]["offer"]["carrier"]["id"], "ca1");
    let w = read(&s).await;
    let load = w.loads.iter().find(|v| v.id == load_id).expect("load");
    assert_eq!((load.buy_cents, load.sell_cents), (130001, 150001));
    assert_eq!(
        w.quotes
            .iter()
            .find(|v| v.id == id)
            .expect("quote")
            .current()
            .expect("revision")
            .decision_kind
            .as_deref(),
        Some("accept")
    );
    assert_eq!(
        w.tenders
            .iter()
            .find(|v| v.load_id == load_id)
            .expect("old tender")
            .plan
            .buy_cents,
        125001
    );
    command(
        &s,
        "load.command",
        load_id,
        json!({"action":"tender","carrier_id":"ca1","expected_version":6}),
    )
    .await;
    let w = read(&s).await;
    let attempts: Vec<_> = w.tenders.iter().filter(|v| v.load_id == load_id).collect();
    assert_eq!(attempts.len(), 2);
    assert_ne!(
        attempts[0].shipment_reference,
        attempts[1].shipment_reference
    );
    assert_eq!(attempts[0].status, "released");
    assert_eq!(attempts[1].plan.buy_cents, 130001);
    assert_eq!(attempts[1].plan.sell_cents, 150001);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_quote_booking_is_atomic_once_across_instances_and_preserves_acceptance_after_reconnect(
) {
    let memory = setup().await;
    let q = accepted(&memory, "QUOTE-PG").await;
    let id = q["id"].as_str().expect("quote");
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    let tenant = domain::id("quote-test");
    canonical::put_workspace(&pool, &tenant, &json!(read(&memory).await))
        .await
        .expect("owned fixture");
    let before = a.read(&tenant).await.expect("before");
    let body = json!({"expected_version":q["version"]});
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "staff:first"),
            "first-book",
            "quote.book",
            id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "staff:second"),
            "second-book",
            "quote.book",
            id,
            body.clone()
        )
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let (winner, key) = if let Ok(result) = first {
        (result, "first-book")
    } else {
        (second.expect("second won"), "second-book")
    };
    let replay = a
        .mutate(&tenant, key, "quote.book", id, body)
        .await
        .expect("lost response replay");
    assert_eq!(replay["data"], winner["data"]);
    assert_eq!(replay["replayed"], true);
    let restarted = Store::new(Some(&url)).await.expect("restart");
    let after = restarted.read(&tenant).await.expect("after");
    assert_eq!(after.loads.len(), before.loads.len() + 1);
    assert_eq!(after.outbox.len(), before.outbox.len());
    assert_eq!(after.journal.len(), before.journal.len());
    let quote = after.quotes.iter().find(|v| v.id == id).expect("quote");
    assert_eq!(quote.revisions[0].decision_history[0].0, "accept");
    let load = after
        .loads
        .iter()
        .find(|l| Some(&l.id) == quote.booked_load_id.as_ref())
        .expect("load");
    assert_eq!((load.buy_cents, load.sell_cents), (125001, 150001));
    assert_eq!(load.status, domain::Status::Planned);
    assert!(load.carrier_id.is_none());
    assert!(restarted
        .read("other-quote-tenant")
        .await
        .expect("other")
        .quotes
        .is_empty());
    let invalid = json!({"expected_version":quote.version,"carrier_id":"ca4","amount_cents":125001,"valid_until":Utc::now()+Duration::hours(24),"contact":"Late offer","reference":"AFTER-BOOKING","note":"Cannot silently replace the booked agreement"});
    let snapshot = json!(after);
    assert!(a
        .mutate(&tenant, "late-offer", "quote.offer", id, invalid)
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("unchanged")), snapshot);
    let current = a.read(&tenant).await.expect("current");
    let current_quote = current.quotes.iter().find(|v| v.id == id).expect("quote");
    let load_id = current_quote.booked_load_id.as_deref().expect("load");
    let load = current
        .loads
        .iter()
        .find(|v| v.id == load_id)
        .expect("load");
    let body = json!({"expected_version":current_quote.version,"expected_load_version":load.version,"carrier_id":"ca1","amount_cents":130001,"valid_until":Utc::now()+Duration::hours(24),"contact":"PG replacement carrier","reference":"PG-REPLACEMENT","note":"Actual replacement rate evidence","reason":"Replacement procured before first tender"});
    let (first, second) = tokio::join!(
        a.mutate(&tenant, "replace-one", "quote.reprocure", id, body.clone()),
        b.mutate(&tenant, "replace-two", "quote.reprocure", id, body)
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let recovered = restarted.read(&tenant).await.expect("recovered");
    let quote = recovered.quotes.iter().find(|v| v.id == id).expect("quote");
    assert_eq!(quote.procurements.len(), 2);
    assert_eq!(quote.procurements[0].offer.amount_cents, 125001);
    assert_eq!(quote.procurements[1].offer.amount_cents, 130001);
    assert_eq!(
        recovered
            .loads
            .iter()
            .find(|v| v.id == load_id)
            .expect("same load")
            .buy_cents,
        130001
    );
    assert_eq!(recovered.journal.len(), before.journal.len());
}

#[tokio::test]
async fn accepted_quote_books_existing_sync_shipment_without_duplicating_or_changing_commerce_identity(
) {
    let s = setup().await;
    let (q, source, order) = source_quote(&s, "QUOTE-SYNC-BOUND").await;
    let id = q["id"].as_str().expect("quote");
    let source_id = source["id"].as_str().expect("source");
    let before = read(&s).await;
    let body = json!({"expected_version":q["version"],"expected_load_version":source["version"]});
    let booked = s
        .mutate("demo", "bound-book", "quote.book", id, body.clone())
        .await
        .expect("book existing");
    let replay = s
        .mutate("demo", "bound-book", "quote.book", id, body)
        .await
        .expect("replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(booked["data"]["booked_load_id"], source_id);
    let w = read(&s).await;
    let load = w
        .loads
        .iter()
        .find(|v| v.id == source_id)
        .expect("source retained");
    assert_eq!(w.loads.len(), before.loads.len());
    assert_eq!(load.reference, "SOURCE-LOAD");
    assert_eq!(load.source_order.as_ref(), Some(&order));
    assert_eq!(load.sync_order_id.as_deref(), order["id"].as_str());
    assert_eq!(load.order_number.as_deref(), Some("COMMERCE-ORIGINAL"));
    assert_eq!(
        (load.buy_cents, load.sell_cents, load.version),
        (125001, 150001, 2)
    );
    assert_eq!(booked["data"]["booking_source"]["buy_cents"], 90000);
    assert_eq!(booked["data"]["booking_source"]["source_order"], order);
    assert_eq!(w.outbox.len(), before.outbox.len());
    assert_eq!(w.journal.len(), before.journal.len());
    assert!(quotes::statement(&w, id, None)
        .expect("customer statement")
        .1
        .contains("COMMERCE-ORIGINAL"));
    command(
        &s,
        "load.command",
        source_id,
        json!({"action":"tender","expected_version":2,"carrier_id":"ca4"}),
    )
    .await;
    command(
        &s,
        "load.command",
        source_id,
        json!({"action":"accept","expected_version":3}),
    )
    .await;
    let w = read(&s).await;
    let attempt = w
        .tenders
        .iter()
        .find(|v| v.load_id == source_id)
        .expect("tender");
    command(&s,"edi.event","",json!({"event_id":"BOUND-PICKUP","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":attempt.shipment_reference,"partner_id":attempt.carrier.partner_id,"carrier_scac":attempt.carrier.scac,"latest_status":"AF","events":[{"status_code":"AF","city":"Portland","state":"OR"}]}})).await;
    let w = read(&s).await;
    let tracking = w
        .outbox
        .iter()
        .find(|v| v.load_id == source_id && v.destination == "stateset-sync-server")
        .expect("original order writeback");
    assert_eq!(
        tracking.path,
        format!("/orders/{}/tracking", order["id"].as_str().expect("id"))
    );
    assert_eq!(
        tracking.payload["trackingData"]["trackingNumber"],
        "SOURCE-LOAD"
    );
    assert_eq!(
        tracking.payload["trackingData"]["freight_status"],
        "in_transit"
    );
    assert!(!tracking.payload.to_string().contains("QUOTE-SYNC-BOUND"));
    assert!(!tracking.payload.to_string().contains("125001"));
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == source_id)
            .expect("load")
            .source_order
            .as_ref(),
        Some(&order)
    );
}

#[tokio::test]
async fn existing_source_requires_current_version_exact_plan_and_single_quote_ownership() {
    let s = setup().await;
    let (q, source, _) = source_quote(&s, "QUOTE-SOURCE-GUARDS").await;
    let id = q["id"].as_str().expect("quote");
    let source_id = source["id"].as_str().expect("source");
    let before = json!(read(&s).await);
    assert!(s
        .mutate(
            "demo",
            "missing-version",
            "quote.book",
            id,
            json!({"expected_version":q["version"]})
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
    let mut bad = draft("BAD-SOURCE");
    bad["planned_load_id"] = json!(source_id);
    bad["expected_load_version"] = json!(1);
    bad["plan"]["miles"] = json!(174);
    assert!(s
        .mutate("demo", "wrong-lane", "quote.create", "", bad)
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
    let mut changed = draft("UNUSED")["plan"].clone();
    changed["reference"] = json!("SOURCE-LOAD");
    changed["miles"] = json!(174);
    changed["buy_cents"] = json!(91000);
    changed["sell_cents"] = json!(140000);
    command(
        &s,
        "load.revise",
        source_id,
        json!({"expected_version":1,"plan":changed}),
    )
    .await;
    let before = json!(read(&s).await);
    assert!(s
        .mutate(
            "demo",
            "stale-source",
            "quote.book",
            id,
            json!({"expected_version":q["version"],"expected_load_version":1})
        )
        .await
        .is_err());
    assert!(s
        .mutate(
            "demo",
            "changed-source",
            "quote.book",
            id,
            json!({"expected_version":q["version"],"expected_load_version":2})
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_existing_source_booking_competing_quotes_keep_one_original_shipment() {
    let memory = setup().await;
    let (q, source, order) = source_quote(&memory, "QUOTE-EXISTING-PG").await;
    let first_id = q["id"].as_str().expect("quote");
    let source_id = source["id"].as_str().expect("load");
    let mut second = draft("QUOTE-COMPETING-PG");
    second["planned_load_id"] = json!(source_id);
    second["expected_load_version"] = json!(1);
    let second = command(&memory, "quote.create", "", second).await;
    let second_id = second["id"].as_str().expect("second quote");
    let offered = offer(&memory, second_id, "ca1", 120001, "SECOND-CARRIER").await;
    issue(
        &memory,
        second_id,
        offered["revisions"][0]["offers"][0]["id"]
            .as_str()
            .expect("offer"),
    )
    .await;
    let second = decide(&memory, second_id, "accept").await;
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    let tenant = domain::id("source-quote-test");
    canonical::put_workspace(&pool, &tenant, &json!(read(&memory).await))
        .await
        .expect("owned source");
    let before = a.read(&tenant).await.expect("before");
    let (first, second) = tokio::join!(
        a.mutate(
            &tenant,
            "first-source-book",
            "quote.book",
            first_id,
            json!({"expected_version":q["version"],"expected_load_version":1})
        ),
        b.mutate(
            &tenant,
            "second-source-book",
            "quote.book",
            second_id,
            json!({"expected_version":second["version"],"expected_load_version":1})
        )
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let restarted = Store::new(Some(&url)).await.expect("restart");
    let w = restarted.read(&tenant).await.expect("recovered");
    assert_eq!(w.loads.len(), before.loads.len());
    assert_eq!(
        w.quotes
            .iter()
            .filter(|v| v.booked_load_id.as_deref() == Some(source_id))
            .count(),
        1
    );
    let load = w.loads.iter().find(|v| v.id == source_id).expect("source");
    assert_eq!(load.reference, "SOURCE-LOAD");
    assert_eq!(load.source_order.as_ref(), Some(&order));
    assert_eq!(load.version, 2);
    assert_eq!(load.sell_cents, 150001);
    assert!(matches!(load.buy_cents, 120001 | 125001));
    assert_eq!(w.outbox.len(), before.outbox.len());
    assert_eq!(w.journal.len(), before.journal.len());
}

async fn booked(s: &Store, reference: &str) -> (String, String) {
    let q = accepted(s, reference).await;
    let id = q["id"].as_str().expect("quote").to_owned();
    let q = command(
        s,
        "quote.book",
        &id,
        json!({"expected_version":q["version"]}),
    )
    .await;
    (id, q["booked_load_id"].as_str().expect("load").into())
}
async fn load_action(s: &Store, load: &str, action: &str, extra: Value) -> Value {
    let w = read(s).await;
    let mut body = json!({"action":action,"expected_version":w.loads.iter().find(|v|v.id==load).expect("load").version});
    for (k, v) in extra.as_object().expect("extra") {
        body[k] = v.clone();
    }
    command(s, "load.command", load, body).await
}
async fn amend_draft(s: &Store, parent: &str, reference: &str) -> Value {
    let w = read(s).await;
    let parent = w.quotes.iter().find(|q| q.id == parent).expect("parent");
    let load = w
        .loads
        .iter()
        .find(|v| Some(&v.id) == parent.booked_load_id.as_ref())
        .expect("load");
    let mut body = draft(reference);
    body["expected_version"] = json!(parent.version);
    body["expected_load_version"] = json!(load.version);
    body["reason"] = json!("Customer changed delivery and accepted a new inclusive base agreement");
    body["sell_cents"] = json!(170001);
    body["plan"]["destination"]["city"] = json!("Tacoma");
    body["plan"]["destination"]["address_lines"] = json!(["99 Amended Dock", "Receiving Gate B"]);
    body["plan"]["destination"]["postal_code"] = json!("98401");
    body["plan"]["destination"]["contact"] = json!("Verified receiving lead");
    command(s, "quote.amend", &parent.id, body).await
}
async fn amend_accepted(s: &Store, parent: &str, reference: &str) -> Value {
    let q = amend_draft(s, parent, reference).await;
    let id = q["id"].as_str().expect("quote");
    let q = offer(s, id, "ca1", 140001, "AMENDED-CARRIER-OFFER").await;
    issue(
        s,
        id,
        q["revisions"][0]["offers"][0]["id"]
            .as_str()
            .expect("offer"),
    )
    .await;
    let w = read(s).await;
    let source = w
        .quotes
        .iter()
        .find(|q| q.id == id)
        .expect("quote")
        .current()
        .expect("revision")
        .source_load
        .as_ref()
        .expect("baseline");
    if let Some(check) = w.source_check(&source.id) {
        command(s,"quote.decision",id,json!({"expected_version":version(s,id).await,"decision":"accept","contact":"Actual customer transport lead","reference":"CUSTOMER-AMENDED-SOURCE-ACCEPTANCE","note":"Customer confirms the amended plan and verified source business data","source_digest":check.digest,"authorize_transport":false})).await
    } else {
        decide(s, id, "accept").await
    }
}
async fn book_amended(s: &Store, q: &Value, key: &str) -> Result<Value, String> {
    let w = read(s).await;
    let source = q["revisions"]
        .as_array()
        .expect("revisions")
        .last()
        .expect("revision")["source_load"]["id"]
        .as_str()
        .expect("source");
    let version = w
        .loads
        .iter()
        .find(|v| v.id == source)
        .expect("load")
        .version;
    s.mutate_authorized(
        ("demo", "user:amendment-ops"),
        key,
        "quote.book",
        q["id"].as_str().expect("quote"),
        json!({"expected_version":q["version"],"expected_load_version":version}),
    )
    .await
}
#[tokio::test]
async fn customer_amendment_preserves_original_agreement_and_activates_one_replacement_on_the_same_load(
) {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-BASE").await;
    let original = read(&s).await;
    let old = original
        .quotes
        .iter()
        .find(|q| q.id == parent)
        .expect("parent")
        .clone();
    let proposal = amend_draft(&s, &parent, "AMEND-CHILD").await;
    assert_eq!(json!(read(&s).await.loads), json!(original.loads));
    assert_eq!(json!(read(&s).await.quotes[0]), json!(old));
    let id = proposal["id"].as_str().expect("child");
    let q = offer(&s, id, "ca1", 140001, "AMENDED-CARRIER").await;
    issue(
        &s,
        id,
        q["revisions"][0]["offers"][0]["id"]
            .as_str()
            .expect("offer"),
    )
    .await;
    let q = decide(&s, id, "accept").await;
    assert_eq!(json!(read(&s).await.loads), json!(original.loads));
    let booked = book_amended(&s, &q, "amend-book-once")
        .await
        .expect("activate");
    let replay = s
        .mutate(
            "demo",
            "amend-book-once",
            "quote.book",
            id,
            json!({"expected_version":q["version"],"expected_load_version":1}),
        )
        .await
        .expect("original request replay");
    assert_eq!(replay["data"], booked["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        book_amended(&s, &q, "amend-book-once")
            .await
            .expect_err("new expected load version is a different logical request"),
        "Idempotency key already used for another request"
    );
    let w = read(&s).await;
    assert_eq!(w.loads.len(), original.loads.len());
    let revised = w.loads.iter().find(|v| v.id == load).expect("same load");
    assert_eq!((revised.buy_cents, revised.sell_cents), (140001, 170001));
    assert_eq!(revised.destination.city, "Tacoma");
    assert_eq!(
        revised.destination.details.address_lines,
        vec!["99 Amended Dock", "Receiving Gate B"]
    );
    assert_eq!(revised.reference, "AMEND-BASE");
    assert_eq!(
        revised.created_at,
        original.loads.last().expect("load").created_at
    );
    let ancestor = w.quotes.iter().find(|q| q.id == parent).expect("parent");
    assert_eq!(ancestor.status, "superseded");
    assert_eq!(
        ancestor
            .superseded_by
            .as_ref()
            .expect("supersession")
            .quote_id,
        id
    );
    assert_eq!(json!(ancestor.revisions), json!(old.revisions));
    assert_eq!(json!(ancestor.procurements), json!(old.procurements));
    assert_eq!(
        w.quotes.iter().filter(|q| q.active_booking(&load)).count(),
        1
    );
    assert!(w.outbox.is_empty());
    assert!(w.journal.is_empty());
    let original_html = quotes::statement(&w, &parent, None)
        .expect("old statement")
        .1;
    let new_html = quotes::statement(&w, id, None).expect("new statement").1;
    assert!(original_html.contains("1500.01"));
    assert!(!original_html.contains("1700.01"));
    assert!(new_html.contains("1700.01"));
    assert!(new_html.contains("AMEND-BASE"));
    for private in ["1400.01", "1250.01", "AMENDED-CARRIER", "Summit Transport"] {
        assert!(!new_html.contains(private));
    }
    assert!(s.mutate("demo","old-reconfirm","quote.reconfirm",&parent,json!({"expected_version":ancestor.version,"expected_load_version":revised.version,"valid_until":Utc::now()+Duration::days(1),"contact":"Dispatch","reference":"MUST-NOT-REOPEN","note":"Historical agreement cannot be revived"})).await.is_err());
    assert!(s
        .mutate(
            "demo",
            "old-carrier",
            "load.command",
            &load,
            json!({"action":"tender","expected_version":revised.version,"carrier_id":"ca4"})
        )
        .await
        .is_err());
    load_action(&s, &load, "tender", json!({"carrier_id":"ca1"})).await;
    assert_eq!(
        read(&s)
            .await
            .tenders
            .last()
            .expect("new tender")
            .plan
            .destination
            .city,
        "Tacoma"
    );
}
#[tokio::test]
async fn amendment_customer_acceptance_cannot_replace_a_carrier_until_safe_release_is_confirmed() {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-RELEASE").await;
    load_action(&s, &load, "tender", json!({"carrier_id":"ca4"})).await;
    load_action(&s, &load, "accept", json!({})).await;
    let q = amend_accepted(&s, &parent, "AMEND-RELEASED").await;
    let before = json!(read(&s).await);
    assert!(book_amended(&s, &q, "before-release").await.is_err());
    assert_eq!(json!(read(&s).await), before);
    let w = read(&s).await;
    let l = w.loads.iter().find(|v| v.id == load).expect("load");
    let release=command(&s,"tender.release.request",&load,json!({"expected_load_version":l.version,"disposition":"replan","reason":"Customer amended delivery; carrier confirmed coordination is needed"})).await;
    assert!(book_amended(&s, &q, "pending-release").await.is_err());
    let w = read(&s).await;
    command(&s,"tender.release.confirm",release["id"].as_str().expect("release"),json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"carrier_contact":"Actual original carrier dispatch","confirmation_reference":"ACTUAL-RELEASE","note":"Carrier confirms no pickup and releases exact tender"})).await;
    book_amended(&s, &q, "after-release")
        .await
        .expect("apply after release");
    load_action(&s, &load, "tender", json!({"carrier_id":"ca1"})).await;
    let w = read(&s).await;
    let attempts: Vec<_> = w.tenders.iter().filter(|v| v.load_id == load).collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status, "released");
    assert_eq!(attempts[0].plan.destination.city, "Seattle");
    assert_eq!(attempts[0].plan.buy_cents, 125001);
    assert_eq!(attempts[1].plan.destination.city, "Tacoma");
    assert_ne!(
        attempts[0].shipment_reference,
        attempts[1].shipment_reference
    );
}
#[tokio::test]
async fn competing_amendments_cannot_branch_and_rebasing_requires_new_customer_acceptance() {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-COMPETE").await;
    let a = amend_accepted(&s, &parent, "AMEND-FIRST").await;
    let b = amend_accepted(&s, &parent, "AMEND-SECOND").await;
    book_amended(&s, &a, "first-amendment")
        .await
        .expect("first");
    let before = json!(read(&s).await);
    assert!(book_amended(&s, &b, "competing-amendment").await.is_err());
    assert_eq!(json!(read(&s).await), before);
    let w = read(&s).await;
    let mut input = draft("AMEND-SECOND");
    input["reason"] = json!("Try to branch old agreement");
    input["expected_version"] = b["version"].clone();
    input["expected_parent_version"] = json!(
        w.quotes
            .iter()
            .find(|q| q.id == parent)
            .expect("parent")
            .version
    );
    input["expected_load_version"] =
        json!(w.loads.iter().find(|v| v.id == load).expect("load").version);
    input.as_object_mut().expect("body").remove("reference");
    assert!(s
        .mutate(
            "demo",
            "implicit-rebase",
            "quote.revise",
            b["id"].as_str().expect("quote"),
            input
        )
        .await
        .is_err());
    let next = amend_accepted(&s, a["id"].as_str().expect("active quote"), "AMEND-CHAIN").await;
    book_amended(&s, &next, "second-generation")
        .await
        .expect("linear amendment chain");
    let w = read(&s).await;
    assert_eq!(
        w.quotes.iter().filter(|q| q.active_booking(&load)).count(),
        1
    );
    assert_eq!(
        w.quotes
            .iter()
            .filter(|q| q.superseded_by.is_some())
            .count(),
        2
    );
}
#[tokio::test]
async fn changed_original_procurement_requires_an_explicit_amendment_revision() {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-REBASE").await;
    let q = amend_accepted(&s, &parent, "AMEND-STALE-BASE").await;
    command(&s,"quote.reconfirm",&parent,json!({"expected_version":version(&s,&parent).await,"expected_load_version":1,"valid_until":Utc::now()+Duration::days(2),"contact":"Actual carrier dispatch","reference":"ORIGINAL-EXTENSION","note":"Actual rate extension changes the original agreement version"})).await;
    assert!(book_amended(&s, &q, "stale-parent").await.is_err());
    let id = q["id"].as_str().expect("quote");
    decide(&s, id, "withdraw").await;
    let w = read(&s).await;
    let mut body = draft("AMEND-STALE-BASE");
    body.as_object_mut().expect("body").remove("reference");
    body["expected_version"] = json!(version(&s, id).await);
    body["expected_parent_version"] = json!(version(&s, &parent).await);
    body["expected_load_version"] =
        json!(w.loads.iter().find(|v| v.id == load).expect("load").version);
    body["reason"] = json!("Rebase against confirmed original extension");
    let q = command(&s, "quote.revise", id, body).await;
    assert_eq!(q["revisions"].as_array().expect("history").len(), 2);
    assert_eq!(q["revisions"][0]["decision_history"][0][0], "accept");
    assert!(q["revisions"][1]["source_consent"].is_null());
    assert!(book_amended(&s, &q, "not-accepted-again").await.is_err());
}

#[tokio::test]
async fn amendment_activation_preserves_confirmed_stop_and_charge_agreements_until_explicit_reconciliation(
) {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-OBLIGATIONS").await;
    let q = amend_accepted(&s, &parent, "AMEND-OBLIGATIONS-CHILD").await;
    let w = read(&s).await;
    let l = w.loads.iter().find(|v| v.id == load).expect("load");
    let day = l.destination.date;
    let appt=command(&s,"stop.appointment.book",&load,json!({"expected_load_version":l.version,"stop":"delivery","start_local":format!("{day}T10:00"),"end_local":format!("{day}T11:00"),"timezone":"America/Los_Angeles","contact":"Actual receiving dock","confirmation_reference":"ORIGINAL-DOCK-BOOKING","note":"Confirmed slot at original Seattle dock"})).await;
    let before = json!(read(&s).await);
    assert!(book_amended(&s, &q, "active-appointment").await.is_err());
    assert_eq!(json!(read(&s).await), before);
    let w = read(&s).await;
    command(&s,"stop.appointment.cancel",appt["id"].as_str().expect("appointment"),json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"reason":"Facility confirms original slot cancelled for customer amendment"})).await;
    load_action(&s, &load, "tender", json!({"carrier_id":"ca4"})).await;
    load_action(&s, &load, "accept", json!({})).await;
    let w = read(&s).await;
    let charge=command(&s,"charge.propose",&load,json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"side":"sell","kind":"other","description":"Original delivery addition still under discussion","amount_cents":10001,"evidence_reference":"ORIGINAL-PRICE-DISCUSSION"})).await;
    let w = read(&s).await;
    let released=command(&s,"tender.release.request",&load,json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"disposition":"replan","reason":"Customer and original carrier coordinated amended delivery"})).await;
    let w = read(&s).await;
    command(&s,"tender.release.confirm",released["id"].as_str().expect("release"),json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"carrier_contact":"Actual original dispatch","confirmation_reference":"OLD-CARRIER-RELEASED","note":"Carrier confirms no pickup and releases exact original tender"})).await;
    assert!(book_amended(&s, &q, "pending-charge").await.is_err());
    let w = read(&s).await;
    command(&s,"charge.review",charge["id"].as_str().expect("charge"),json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"expected_version":charge["version"],"decision":"rejected","note":"Customer confirms inclusive amended price replaces this proposed addition"})).await;
    book_amended(&s, &q, "reconciled-amendment")
        .await
        .expect("reconciled activation");
    let w = read(&s).await;
    assert_eq!(
        w.appointments.last().expect("appointment history").status,
        "cancelled"
    );
    assert_eq!(w.charges.last().expect("charge history").status, "rejected");
    assert!(w.journal.is_empty());
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == load)
            .expect("load")
            .sell_cents,
        170001
    );
}
#[tokio::test]
async fn physical_pickup_stops_amendment_activation_without_erasing_carrier_custody() {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-CUSTODY").await;
    let q = amend_accepted(&s, &parent, "AMEND-CUSTODY-CHILD").await;
    load_action(&s, &load, "tender", json!({"carrier_id":"ca4"})).await;
    load_action(&s, &load, "accept", json!({})).await;
    let w = read(&s).await;
    let tender = w
        .tenders
        .iter()
        .find(|v| v.load_id == load)
        .expect("tender");
    command(&s,"edi.event","",json!({"event_id":"amendment-physical-pickup","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":tender.shipment_reference,"partner_id":"brex","carrier_scac":"BREX","latest_status":"AF","events":[{"status_code":"AF"}]}})).await;
    let before = json!(read(&s).await);
    assert!(book_amended(&s, &q, "cargo-already-picked-up")
        .await
        .is_err());
    assert_eq!(json!(read(&s).await), before);
    assert_eq!(
        read(&s)
            .await
            .loads
            .iter()
            .find(|v| v.id == load)
            .expect("load")
            .carrier_id
            .as_deref(),
        Some("ca4")
    );
    assert_eq!(
        read(&s)
            .await
            .loads
            .iter()
            .find(|v| v.id == load)
            .expect("load")
            .destination
            .city,
        "Seattle"
    );
}
#[tokio::test]
async fn amendment_proposals_reject_customer_transfer_stale_basis_and_invalid_plan_without_changing_agreements(
) {
    let s = setup().await;
    let (parent, load) = booked(&s, "AMEND-VALIDATION").await;
    let w = read(&s).await;
    let original = w.quotes.iter().find(|q| q.id == parent).expect("quote");
    let l = w.loads.iter().find(|v| v.id == load).expect("load");
    let mut input = draft("AMEND-INVALID");
    input["expected_version"] = json!(original.version);
    input["expected_load_version"] = json!(l.version);
    input["reason"] = json!("Actual proposed amendment");
    for (field, value) in [
        ("customer", json!("c2")),
        ("stale", json!(999)),
        ("price", json!(-1)),
        ("postal", json!("")),
    ] {
        let mut body = input.clone();
        match field {
            "customer" => body["plan"]["customer_id"] = value,
            "stale" => body["expected_version"] = value,
            "price" => body["sell_cents"] = value,
            _ => body["plan"]["destination"]["postal_code"] = value,
        };
        assert!(s
            .mutate("demo", &domain::id("bad"), "quote.amend", &parent, body)
            .await
            .is_err());
        assert_eq!(json!(read(&s).await), json!(w));
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_competing_customer_amendments_preserve_a_single_active_contract_and_original_history(
) {
    let memory = setup().await;
    let (q, source, order) = source_quote(&memory, "PG-AMEND-BASE").await;
    let parent = q["id"].as_str().expect("parent").to_owned();
    let load = source["id"].as_str().expect("load").to_owned();
    command(
        &memory,
        "quote.book",
        &parent,
        json!({"expected_version":q["version"],"expected_load_version":1}),
    )
    .await;
    let first = amend_accepted(&memory, &parent, "PG-AMEND-FIRST").await;
    let second = amend_accepted(&memory, &parent, "PG-AMEND-SECOND").await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first instance");
    let b = Store::new(Some(&url)).await.expect("second instance");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("amendment-test");
    let original = read(&memory).await;
    let mut data = json!(original);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned tenant");
    let (first_result, second_result) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "user:first"),
            "amend-first",
            "quote.book",
            first["id"].as_str().expect("first"),
            json!({"expected_version":first["version"],"expected_load_version":2})
        ),
        b.mutate_authorized(
            (&tenant, "user:second"),
            "amend-second",
            "quote.book",
            second["id"].as_str().expect("second"),
            json!({"expected_version":second["version"],"expected_load_version":2})
        )
    );
    assert_eq!(
        [first_result.is_ok(), second_result.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let restarted = Store::new(Some(&url)).await.expect("reconnect");
    let w = restarted.read(&tenant).await.expect("recovered");
    assert_eq!(
        w.quotes.iter().filter(|q| q.active_booking(&load)).count(),
        1
    );
    assert_eq!(w.loads.len(), original.loads.len());
    assert_eq!(
        json!(
            w.quotes
                .iter()
                .find(|q| q.id == parent)
                .expect("parent")
                .revisions
        ),
        json!(original.quotes[0].revisions)
    );
    assert!(w.outbox.is_empty());
    assert!(w.journal.is_empty());
    let active = w
        .quotes
        .iter()
        .find(|q| q.active_booking(&load))
        .expect("active");
    assert_eq!(
        active
            .current()
            .expect("revision")
            .source_consent
            .as_ref()
            .expect("accepted source")
            .snapshot,
        order
    );
    let decision = w
        .source_reviews
        .last()
        .expect("source authorization")
        .decision
        .as_ref()
        .expect("decision");
    assert_eq!(decision.quote_id.as_deref(), Some(active.id.as_str()));
    let check = w.source_check(&load).expect("durable source check");
    assert!(check.active_review_id.is_none());
    assert!(check.authorized_review_id.is_some());
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == load)
            .expect("same source shipment")
            .source_order
            .as_ref(),
        Some(&order)
    );
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == load)
            .expect("load")
            .sell_cents,
        170001
    );
    assert!(restarted
        .read("unrelated-amendment-tenant")
        .await
        .expect("isolated")
        .quotes
        .is_empty());
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
}
