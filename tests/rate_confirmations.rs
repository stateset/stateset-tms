#[path = "support/canonical.rs"]
mod canonical;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{
    documents::Upload,
    domain::{self, Load},
    rate_confirmations,
    store::Store,
};
const PDF: &[u8] = b"%PDF-1.7\nOriginal synthetic carrier handoff evidence\n%%EOF";
async fn run(s: &Store, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(("demo", "ops:rates"), &domain::id("key"), op, id, body)
        .await
        .expect(op)["data"]
        .clone()
}
async fn load(s: &Store, id: &str) -> Load {
    s.read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load")
}
async fn setup() -> (Store, String) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let day = (Utc::now() + Duration::days(1)).date_naive();
    let l=run(&s,"load.create","",json!({"reference":domain::id("RATE-LOAD"),"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street","Suite 2"],"postal_code":"97201","country":"US","contact":"Pickup contact","phone":"555-0100","instructions":"Ring dock <script>never execute</script>"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":125001,"sell_cents":150001})).await;
    let id = l["id"].as_str().expect("id").to_owned();
    run(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":1,"carrier_id":"ca4"}),
    )
    .await;
    (s, id)
}
fn issue(l: &Load, prior: Option<&str>) -> Value {
    json!({"expected_load_version":l.version,"expected_previous_id":prior,"terms":"Actual agreed inclusive USD freight terms\nCall carrier dispatch before any change <script>escaped</script>","reason":"Issue actual carrier transport rate record"})
}
async fn issued(s: &Store, id: &str) -> Value {
    run(s, "rate.issue", id, issue(&load(s, id).await, None)).await
}
fn handoff(l: &Load, version: u64, doc: Option<&str>) -> Value {
    json!({"expected_load_version":l.version,"expected_version":version,"sent_at":Utc::now(),"carrier_contact":"Actual carrier dispatch","channel":"email","reference":"Original outbound carrier message","note":"Inspected actual sent message and exact attachment","document_id":doc})
}
#[tokio::test]
async fn carrier_document_freezes_exact_tender_price_addresses_windows_and_issuer_without_customer_commercial_data(
) {
    let (s, id) = setup().await;
    let l = load(&s, &id).await;
    run(&s,"stop.appointment.book",&id,json!({"expected_load_version":l.version,"stop":"pickup","start_local":format!("{}T09:00",l.origin.date),"end_local":format!("{}T10:00",l.origin.date),"timezone":"America/Los_Angeles","contact":"Actual facility scheduler","confirmation_reference":"PRIVATE-APPOINTMENT-REF","note":"PRIVATE-APPOINTMENT-NOTE"})).await;
    let doc = issued(&s, &id).await;
    let w = s.read("demo").await.expect("read");
    let d = &w.rate_confirmations[0];
    assert_eq!(d.snapshot.buy.total_amount_cents, 125001);
    assert_eq!(
        d.snapshot.shipment_reference,
        w.tenders.last().expect("tender").shipment_reference
    );
    assert!(
        w.can_withdraw_tender(&load(&s, &id).await),
        "issuance is not handoff"
    );
    let (_, html, hash) = rate_confirmations::statement(&w, d.id.as_str()).expect("statement");
    for text in [
        "1250.01",
        "Suite 2",
        "America/Los_Angeles",
        "Actual facility scheduler",
        "&lt;script&gt;",
        "Carrier shipment ID",
    ] {
        assert!(html.contains(text), "{text}");
    }
    for private in [
        "1500.01",
        "Gross margin",
        "payment_instructions",
        "PRIVATE-APPOINTMENT-REF",
        "PRIVATE-APPOINTMENT-NOTE",
        "Northstar Retail",
    ] {
        assert!(!html.contains(private), "{private}");
    }
    assert_eq!(hash, stateset_freight::documents::sha256(html.as_bytes()));
    let mut corrupt = w.clone();
    corrupt.rate_confirmations[0].html.push_str("changed");
    assert!(rate_confirmations::statement(&corrupt, &d.id).is_err());
    let mut changed = w.clone();
    changed
        .loads
        .iter_mut()
        .find(|l| l.id == id)
        .expect("load")
        .sell_cents = 999999;
    changed
        .billing_profile
        .as_mut()
        .expect("profile")
        .legal_name = "Later issuer".into();
    changed.carriers.clear();
    changed.loads.clear();
    assert_eq!(
        rate_confirmations::statement(&changed, &d.id)
            .expect("retained")
            .1,
        html
    );
    assert_eq!(doc["issued_by"], "ops:rates");
    assert!(w.journal.is_empty());
}
#[tokio::test]
async fn manual_rate_handoff_prevents_proven_unsent_withdrawal_without_accepting_carrier_or_starting_edi(
) {
    let (s, id) = setup().await;
    let doc = issued(&s, &id).await;
    let did = doc["id"].as_str().expect("id");
    let before = s.read("demo").await.expect("read");
    let request = handoff(&load(&s, &id).await, 1, None);
    let h = run(&s, "rate.handoff", did, request.clone()).await;
    let w = s.read("demo").await.expect("read");
    let l = load(&s, &id).await;
    assert!(!w.can_withdraw_tender(&l));
    assert_eq!(l.status, domain::Status::Tendered);
    assert_eq!(json!(w.outbox), json!(before.outbox));
    assert!(w.journal.is_empty());
    assert!(!w.unresolved_tender_incidents(&id));
    assert!(s
        .mutate(
            "demo",
            "unsafe-withdraw",
            "load.command",
            &id,
            json!({"action":"withdraw_tender","expected_version":l.version})
        )
        .await
        .is_err());
    let mut duplicate = handoff(&l, 2, None);
    duplicate["reference"] = json!(" original outbound carrier message ");
    assert!(s
        .mutate("demo", "duplicate-handoff", "rate.handoff", did, duplicate)
        .await
        .is_err());
    run(&s,"rate.handoff.retract",did,json!({"expected_version":2,"expected_load_version":l.version,"handoff_id":h["handoffs"][0]["id"],"note":"Original sent log identifies wrong carrier attachment; this handoff entry was erroneous"})).await;
    let w = s.read("demo").await.expect("read");
    assert!(w.can_withdraw_tender(&load(&s, &id).await));
    assert!(w.rate_confirmations[0].handoffs[0].retraction.is_some());
}
#[tokio::test]
async fn revisions_require_exact_predecessor_and_keep_old_bytes_with_old_handoff_coordination_holding_new_commitments(
) {
    let (s, id) = setup().await;
    let first = issued(&s, &id).await;
    let original = first["html"].clone();
    let prior = first["id"].as_str().expect("id");
    let mut next = issue(&load(&s, &id).await, Some(prior));
    next["terms"] = json!("Actual revised carrier instructions confirmed externally");
    let second = run(&s, "rate.issue", &id, next.clone()).await;
    let mut stale = issue(&load(&s, &id).await, Some(prior));
    stale["terms"] = json!("Stale revision");
    assert!(s
        .mutate("demo", "old-predecessor", "rate.issue", &id, stale)
        .await
        .is_err());
    let w = s.read("demo").await.expect("read");
    assert!(!w.rate_confirmation_current(&w.rate_confirmations[0]));
    assert_eq!(
        w.rate_confirmations[0].html,
        original.as_str().expect("html")
    );
    assert_eq!(second["revision"], 2);
    let h = run(
        &s,
        "rate.handoff",
        prior,
        handoff(&load(&s, &id).await, 1, None),
    )
    .await;
    assert_eq!(h["handoffs"][0]["coordination_required"], true);
    let w = s.read("demo").await.expect("read");
    assert!(w.unresolved_tender_incidents(&id));
    run(
        &s,
        "load.command",
        &id,
        json!({"action":"accept","expected_version":load(&s,&id).await.version}),
    )
    .await;
    assert!(s
        .mutate(
            "demo",
            "held-dispatch",
            "load.command",
            &id,
            json!({"action":"dispatch","expected_version":load(&s,&id).await.version})
        )
        .await
        .expect_err("coordination hold")
        .contains("custody"));
    let l = load(&s, &id).await;
    run(&s,"rate.handoff.resolve",prior,json!({"expected_load_version":l.version,"expected_version":2,"handoff_id":h["handoffs"][0]["id"],"carrier_contact":"Actual carrier dispatch","confirmation_reference":"Actual replacement attachment confirmation","note":"Carrier confirms old attachment discarded and correct current version received externally"})).await;
    assert!(!s
        .read("demo")
        .await
        .expect("read")
        .unresolved_tender_incidents(&id));
    assert_eq!(
        rate_confirmations::statement(&s.read("demo").await.expect("read"), prior)
            .expect("retained")
            .1,
        original
    );
}
#[tokio::test]
async fn rate_issue_enforces_source_custody_pending_charges_versions_and_server_owned_price_identity(
) {
    let (s, id) = setup().await;
    let l = load(&s, &id).await;
    let before = json!(s.read("demo").await.expect("read"));
    for (field, value) in [
        ("buy_cents", json!(1)),
        ("issued_by", json!("spoofed")),
        ("carrier_id", json!("ca1")),
        ("html", json!("forged")),
        ("expected_load_version", json!(0)),
    ] {
        let mut body = issue(&l, None);
        body[field] = value;
        assert!(s
            .mutate("demo", &domain::id("bad"), "rate.issue", &id, body)
            .await
            .is_err());
    }
    assert_eq!(json!(s.read("demo").await.expect("read")), before);
    let mut w = s.read("demo").await.expect("read");
    let source_load = w.loads.iter_mut().find(|v| v.id == id).expect("load");
    source_load.sync_order_id = Some("original-commerce-order".into());
    let l = source_load.clone();
    assert_eq!(
        w.commitment_source("rate.issue", &id, &issue(&l, None))
            .expect("source")
            .id,
        id
    );
    assert!(w
        .apply_rate_confirmation("rate.issue", &id, issue(&l, None), "ops")
        .expect_err("unavailable")
        .contains("unavailable"));
    let doc = issued(&s, &id).await;
    let did = doc["id"].as_str().expect("id");
    let mut body = handoff(&load(&s, &id).await, 1, None);
    body["sent_at"] = json!(Utc::now() + Duration::hours(1));
    assert!(s
        .mutate("demo", "future-handoff", "rate.handoff", did, body)
        .await
        .is_err());
    run(
        &s,
        "rate.handoff",
        did,
        handoff(&load(&s, &id).await, 1, None),
    )
    .await;
    run(&s,"tender.release.request",&id,json!({"expected_load_version":load(&s,&id).await.version,"disposition":"replan","reason":"Actual carrier release coordination"})).await;
    assert!(s
        .mutate(
            "demo",
            "pending-release",
            "rate.issue",
            &id,
            issue(&load(&s, &id).await, Some(did))
        )
        .await
        .is_err());
}
#[tokio::test]
async fn handoff_file_evidence_is_load_bound_locked_and_replay_does_not_repeat_history() {
    let (s, id) = setup().await;
    let doc = issued(&s, &id).await;
    let did = doc["id"].as_str().expect("id");
    let upload = s
        .upload_document(
            ("demo", "ops:rates"),
            "upload-rate",
            &id,
            Upload {
                kind: "other".into(),
                filename: "actual-handoff.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("upload");
    let document = upload["data"]["id"].as_str().expect("doc");
    let wrong = s
        .upload_document(
            ("demo", "ops:rates"),
            "wrong-rate-load",
            "load_6",
            Upload {
                kind: "other".into(),
                filename: "wrong-load.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("wrong load proof");
    let invalid = handoff(&load(&s, &id).await, 1, wrong["data"]["id"].as_str());
    assert!(s
        .mutate("demo", "wrong-load-handoff", "rate.handoff", did, invalid)
        .await
        .is_err());
    let body = handoff(&load(&s, &id).await, 1, Some(document));
    let first = s
        .mutate_authorized(
            ("demo", "ops:rates"),
            "one-handoff",
            "rate.handoff",
            did,
            body.clone(),
        )
        .await
        .expect("handoff");
    let replay = s
        .mutate_authorized(
            ("demo", "ops:rates"),
            "one-handoff",
            "rate.handoff",
            did,
            body,
        )
        .await
        .expect("replay");
    assert_eq!(replay["data"], first["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        first["data"]["handoffs"][0]["document_sha256"],
        stateset_freight::documents::sha256(PDF)
    );
    assert!(s.mutate("demo","reject-rate-proof","document.review",document,json!({"expected_version":1,"decision":"rejected","note":"Must correct handoff evidence explicitly"})).await.is_err());
}

async fn addition(s: &Store, id: &str, doc: Option<&str>) -> Value {
    run(
        s,
        "load.command",
        id,
        json!({"action":"accept","expected_version":load(s,id).await.version}),
    )
    .await;
    run(s,"charge.propose",id,json!({"expected_load_version":load(s,id).await.version,"side":"buy","kind":"other","description":"Actual additional carrier loading service","amount_cents":1234,"evidence_reference":"PRIVATE-CARRIER-ADDITION-AGREEMENT","document_id":doc})).await
}
async fn approve(s: &Store, id: &str, c: &Value) {
    run(s,"charge.review",c["id"].as_str().expect("charge"),json!({"expected_load_version":load(s,id).await.version,"expected_version":1,"decision":"approved","counterparty_contact":"Actual carrier accounting","confirmation_reference":"PRIVATE-CARRIER-ADDITION-CONFIRMATION","note":"Verified actual 1234 cent addition from carrier correspondence"})).await;
}
#[tokio::test]
async fn proposed_additions_hold_issuance_and_approved_rate_changes_require_new_frozen_revision() {
    let (s, id) = setup().await;
    let original = issued(&s, &id).await;
    let old = original["id"].as_str().expect("id");
    let c = addition(&s, &id, None).await;
    assert!(s
        .mutate(
            "demo",
            "pending-charge",
            "rate.issue",
            &id,
            issue(&load(&s, &id).await, Some(old))
        )
        .await
        .expect_err("pending")
        .contains("proposed carrier charges"));
    approve(&s, &id, &c).await;
    let w = s.read("demo").await.expect("read");
    assert!(w.rate_confirmation_basis_changed(&w.rate_confirmations[0]));
    let next = run(
        &s,
        "rate.issue",
        &id,
        issue(&load(&s, &id).await, Some(old)),
    )
    .await;
    assert_eq!(next["snapshot"]["buy"]["total_amount_cents"], 126235);
    assert!(next["html"].as_str().expect("html").contains("1262.35"));
    assert!(!next["html"]
        .as_str()
        .expect("html")
        .contains("PRIVATE-CARRIER-ADDITION"));
    assert_eq!(
        rate_confirmations::statement(&s.read("demo").await.expect("read"), old)
            .expect("old")
            .1,
        original["html"]
    );
    let s = Store::new(None).await.expect("legacy");
    s.seed_demo().await.expect("seed");
    let legacy = issued(&s, "load_4").await;
    let w = s.read("demo").await.expect("read");
    let t = w
        .tenders
        .iter()
        .find(|t| t.id == legacy["tender_id"])
        .expect("adopted");
    assert!(t.legacy);
    assert_eq!(t.created_by, "legacy_snapshot");
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_rate_issue_and_handoff_corruption_races_reconnect_and_exact_bytes_are_atomic() {
    let (memory, id) = setup().await;
    let uploaded = memory
        .upload_document(
            ("demo", "ops:rates"),
            "rate-proof",
            &id,
            Upload {
                kind: "other".into(),
                filename: "carrier-agreement.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("upload");
    let document = uploaded["data"]["id"].as_str().expect("id");
    let charge = addition(&memory, &id, Some(document)).await;
    approve(&memory, &id, &charge).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("rate-pg");
    let mut data = json!(memory.read("demo").await.expect("memory"));
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("tenant");
    sqlx::query(
        "INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)",
    )
    .bind(&tenant)
    .bind(document)
    .bind(b"corrupt".as_slice())
    .execute(&pool)
    .await
    .expect("bad bytes");
    let l = a
        .read(&tenant)
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let body = issue(&l, None);
    let before = json!(a.read(&tenant).await.expect("before"));
    assert!(a
        .mutate_authorized(
            (&tenant, "ops:a"),
            "issue-once",
            "rate.issue",
            &id,
            body.clone()
        )
        .await
        .expect_err("corrupt")
        .contains("Internal storage"));
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(document)
    .execute(&pool)
    .await
    .expect("restore");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "issue-once",
            "rate.issue",
            &id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "ops:b"),
            "competing-issue",
            "rate.issue",
            &id,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let w = a.read(&tenant).await.expect("read");
    let doc = &w.rate_confirmations[0];
    let did = doc.id.clone();
    let hash = doc.sha256.clone();
    let html = doc.html.clone();
    assert_eq!(doc.snapshot.buy.total_amount_cents, 126235);
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    let body = handoff(l, 1, Some(document));
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt-again".as_slice())
    .bind(&tenant)
    .bind(document)
    .execute(&pool)
    .await
    .expect("corrupt again");
    let before = json!(w);
    assert!(a
        .mutate_authorized(
            (&tenant, "ops:a"),
            "handoff-once",
            "rate.handoff",
            &did,
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(document)
    .execute(&pool)
    .await
    .expect("restore");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "handoff-once",
            "rate.handoff",
            &did,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "ops:b"),
            "competing-handoff",
            "rate.handoff",
            &did,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    drop(a);
    drop(b);
    let reopened = Store::new(Some(&url)).await.expect("reconnect");
    let w = reopened.read(&tenant).await.expect("retained");
    assert_eq!(w.rate_confirmations.len(), 1);
    assert_eq!(w.rate_confirmations[0].handoffs.len(), 1);
    assert!(!w.rate_confirmations[0].handoffs[0].coordination_required);
    assert_eq!(
        rate_confirmations::statement(&w, &did).expect("exact bytes"),
        (format!("rate-confirmation-{did}-r1"), html, hash)
    );
    assert_eq!(
        w.rate_confirmations[0].handoffs[0]
            .document_sha256
            .as_deref(),
        Some(stateset_freight::documents::sha256(PDF).as_str())
    );
    assert!(!reopened
        .read("other-rate-tenant")
        .await
        .expect("isolated")
        .rate_confirmations
        .iter()
        .any(|v| v.id == did));
    assert!(w.journal.is_empty());
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean files");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean tenant");
    pool.close().await;
}
