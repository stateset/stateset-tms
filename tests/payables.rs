#[path = "support/canonical.rs"]
mod canonical;
#[path = "support/invoice_review.rs"]
mod invoice_review;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{documents::Upload, domain, store::Store};
async fn seeded() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn v(s: &Store) -> u64 {
    s.read("demo").await.expect("read").loads[5].version
}
async fn op(s: &Store, kind: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "finance:payables"),
        &domain::id("request"),
        kind,
        id,
        body,
    )
    .await
    .unwrap_or_else(|e| panic!("{kind}: {e}"))["data"]
        .clone()
}
async fn approve_primary(s: &Store) -> Value {
    let invoice = s.read("demo").await.expect("read").invoices[0].clone();
    op(s, "invoice.approve", &invoice.id, json!({})).await
}
fn draft(invoice: &Value, version: u64, kind: &str, num: &str, amount: i64) -> Value {
    json!({"expected_load_version":version,"invoice_id":invoice["id"],"kind":kind,"number":num,"document_amount_cents":amount,"issued_on":Utc::now().date_naive(),"description":"Actual carrier financial document","evidence_reference":format!("EVIDENCE-{num}")})
}
async fn create(s: &Store, invoice: &Value, kind: &str, num: &str, amount: i64) -> Value {
    op(
        s,
        "payable.create",
        "load_6",
        draft(invoice, v(s).await, kind, num, amount),
    )
    .await
}
fn review_body(version: u64, record: &Value, approved: bool) -> Value {
    json!({"expected_load_version":version,"expected_version":record["version"],"decision":if approved{"approved"}else{"rejected"},"counterparty_contact":"Carrier accounts representative","confirmation_reference":"Actual carrier confirmation CASE-15","note":"Inspected actual carrier document and verified classification and amount"})
}
async fn review(s: &Store, a: &Value, approved: bool) -> Value {
    op(
        s,
        "payable.review",
        a["id"].as_str().expect("id"),
        review_body(v(s).await, a, approved),
    )
    .await
}
async fn reverse(s: &Store, a: &Value) -> Value {
    op(s,"payable.reverse",a["id"].as_str().expect("id"),json!({"expected_load_version":v(s).await,"expected_version":a["version"],"reversed_on":Utc::now().date_naive(),"counterparty_contact":"Carrier accounts correction desk","confirmation_reference":"Carrier retraction CASE-16","note":"Actual carrier confirmation retracts this prior document"})).await
}
async fn settlement(s: &Store) -> Value {
    let w = s.read("demo").await.expect("read");
    json!(w
        .settlements
        .iter()
        .find(|v| v.load_id == "load_6")
        .expect("settlement"))
}
fn cash(direction: &str, target: &Value, amount: i64, reference: &str) -> Value {
    json!({"direction":direction,"currency":"USD","account":"operating-usd","external_reference":reference,"amount_cents":amount,"cleared_on":Utc::now().date_naive(),"note":"Verified actual bank statement line","allocations":[{"target_id":target["id"],"expected_version":target["version"],"amount_cents":amount}]})
}
fn net(w: &domain::Workspace, name: &str) -> i64 {
    w.journal
        .iter()
        .flat_map(|v| &v.lines)
        .filter(|v| v.account == name)
        .map(|v| v.debit_cents - v.credit_cents)
        .sum()
}
async fn fails(s: &Store, kind: &str, id: &str, body: Value, contains: &str) {
    let before = json!(s.read("demo").await.expect("before"));
    let e = s
        .mutate("demo", &domain::id("fail"), kind, id, body)
        .await
        .expect_err("reject");
    assert!(e.contains(contains), "{e}");
    assert_eq!(json!(s.read("demo").await.expect("after")), before);
}

#[tokio::test]
async fn supplemental_invoice_credit_after_payment_refund_and_reversals_preserve_original_claim_and_cash(
) {
    let s = seeded().await;
    let original = approve_primary(&s).await;
    let prepared = op(&s, "settlement.create", "load_6", json!({})).await;
    let paid = op(
        &s,
        "cash.record",
        "",
        cash("payout", &prepared, 118000, "original-wire"),
    )
    .await;
    let additional = review(
        &s,
        &create(&s, &original, "supplemental_invoice", "SUP-15", 12001).await,
        true,
    )
    .await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.settlement_balance(prepared["id"].as_str().expect("id"))
            .expect("balance"),
        12001
    );
    assert_eq!(
        w.settlement_total(&w.settlements[0]).expect("total"),
        130001
    );
    assert_eq!(w.settlements[0].amount_cents, 118000);
    assert_eq!(json!(w.invoices[0]), original);
    assert_eq!(json!(w.cash_entries[0]), paid);
    op(
        &s,
        "cash.record",
        "",
        cash("payout", &settlement(&s).await, 12001, "supplemental-wire"),
    )
    .await;
    let credit = review(
        &s,
        &create(&s, &original, "credit_memo", "CREDIT-15", 4501).await,
        true,
    )
    .await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.settlement_balance(&w.settlements[0].id).expect("credit"),
        -4501
    );
    assert_eq!(w.settlements[0].status, "credit_due");
    assert_eq!(
        w.finance_summary().expect("summary")["carrier_credit_cents"],
        4501
    );
    fails(
        &s,
        "cash.record",
        "",
        cash("payout", &settlement(&s).await, 1, "overpay"),
        "outstanding balance",
    )
    .await;
    fails(
        &s,
        "cash.record",
        "",
        cash(
            "carrier_refund",
            &settlement(&s).await,
            4502,
            "excess-refund",
        ),
        "outstanding balance",
    )
    .await;
    let refund = op(
        &s,
        "cash.record",
        "",
        cash(
            "carrier_refund",
            &settlement(&s).await,
            4501,
            "actual-carrier-refund",
        ),
    )
    .await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.settlement_balance(&w.settlements[0].id).expect("closed"),
        0
    );
    assert_eq!(net(&w, "carrier_payable"), 0);
    assert_eq!(net(&w, "freight_expense"), 125500);
    assert_eq!(net(&w, "cash"), -125500);
    op(&s,"cash.reverse",refund["id"].as_str().expect("id"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Bank corrected the refund statement reference"})).await;
    let mut corrected = cash(
        "carrier_refund",
        &settlement(&s).await,
        4501,
        "actual-carrier-refund",
    );
    corrected["corrects_id"] = refund["id"].clone();
    op(&s, "cash.record", "", corrected).await;
    let reversed = reverse(&s, &credit).await;
    assert_eq!(reversed["decision"], credit["decision"]);
    assert_eq!(
        reversed["reversal_decision"]["confirmation_reference"],
        "Carrier retraction CASE-16"
    );
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.settlement_balance(&w.settlements[0].id).expect("payable"),
        4501
    );
    assert_eq!(net(&w, "carrier_payable"), -4501);
    assert_eq!(json!(w.invoices[0]), original);
    assert_eq!(additional["status"], "posted");
    for journal in &w.journal {
        assert_eq!(
            journal.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            journal.lines.iter().map(|v| v.credit_cents).sum::<i64>()
        );
    }
}
#[tokio::test]
async fn revised_totals_post_only_differences_and_stale_basis_requires_a_retained_correction() {
    let s = seeded().await;
    let original = approve_primary(&s).await;
    let revision = review(
        &s,
        &create(&s, &original, "invoice_revision", "INV-2084", 128001).await,
        true,
    )
    .await;
    assert_eq!(revision["amount_cents"], 10001);
    let pending = create(
        &s,
        &original,
        "invoice_revision",
        "REVISED-NEW-NUMBER",
        120000,
    )
    .await;
    assert_eq!(pending["amount_cents"], -8001);
    review(
        &s,
        &create(&s, &original, "supplemental_invoice", "EXTRA-1", 1).await,
        true,
    )
    .await;
    fails(
        &s,
        "payable.review",
        pending["id"].as_str().expect("id"),
        review_body(v(&s).await, &pending, true),
        "Recognized invoice total changed",
    )
    .await;
    let rejected = review(&s, &pending, false).await;
    let mut correction = draft(
        &original,
        v(&s).await,
        "invoice_revision",
        "REVISED-NEW-NUMBER",
        120000,
    );
    correction["corrects_id"] = rejected["id"].clone();
    let corrected = review(
        &s,
        &op(&s, "payable.create", "load_6", correction).await,
        true,
    )
    .await;
    assert_eq!(corrected["amount_cents"], -8002);
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.carrier_invoice_total(original["id"].as_str().expect("id"))
            .expect("total"),
        120000
    );
    assert_eq!(net(&w, "carrier_payable"), -120000);
    assert_eq!(w.invoices[0].amount_cents, 118000);
    let carrier = w
        .carriers
        .iter()
        .find(|c| Some(&c.id) == w.loads[5].carrier_id.as_ref())
        .expect("carrier");
    let receipt=op(&s,"edi.event","",json!({"event_id":"revised-back-to-original","event_type":"edi.freight_invoice.received","occurred_at":Utc::now(),"payload":{"shipment_id":w.loads[5].reference,"partner_id":carrier.partner_id,"carrier_scac":carrier.scac,"invoice_number":original["number"],"net_amount":"1180.00","currency":"USD"}})).await;
    assert_eq!(
        receipt["finance_review"], true,
        "returning to the original amount is not an ignorable duplicate after other adjustments"
    );
    let prepared = op(&s, "settlement.create", "load_6", json!({})).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.settlement_total(&w.settlements[0]).expect("total"),
        120000
    );
    assert_eq!(prepared["amount_cents"], 118000);
}
#[tokio::test]
async fn canonical_second_210_is_classified_before_posting_and_unapproved_revision_keeps_its_original_evidence(
) {
    let s = seeded().await;
    let event = |id: &str, num: &str, amount: &str| json!({"event_id":id,"event_type":"edi.freight_invoice.received","occurred_at":Utc::now(),"payload":{"shipment_id":"FR-1053","partner_id":"brex","carrier_scac":"BREX","invoice_number":num,"net_amount":amount,"currency":"USD"}});
    // load_6's actual seeded carrier determines the authenticated partner.
    let w = s.read("demo").await.expect("read");
    let carrier = w
        .carriers
        .iter()
        .find(|v| Some(&v.id) == w.loads[5].carrier_id.as_ref())
        .expect("carrier");
    let mut canonical = event("canonical-revision", "REVISED PRIMARY 15", "1180.01");
    canonical["payload"]["shipment_id"] = json!(w.loads[5].reference);
    canonical["payload"]["partner_id"] = json!(carrier.partner_id);
    canonical["payload"]["carrier_scac"] = json!(carrier.scac);
    let received = op(&s, "edi.event", "", canonical.clone()).await;
    assert_eq!(received["finance_review"], true);
    let doc = received["carrier_document_id"].as_str().expect("doc");
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.invoices[0].amount_cents, 118000);
    assert!(w.journal.is_empty());
    assert_eq!(json!(w.carrier_documents[0].event), canonical);
    op(&s,"payable.document.apply_revision",doc,json!({"expected_load_version":v(&s).await,"expected_version":1,"note":"Finance inspected the canonical revised invoice before approval"})).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.invoices[0].amount_cents, 118001);
    assert_eq!(w.invoices[0].number, "REVISED PRIMARY 15");
    assert_eq!(w.invoices[0].variance_cents, 1);
    assert!(w.journal.is_empty());
    assert_eq!(w.carrier_documents[0].status, "applied_revision");
    let proposal=op(&s,"charge.propose","load_6",json!({"expected_load_version":v(&s).await,"side":"buy","kind":"other","amount_cents":1,"description":"Carrier agreed one-cent addition","evidence_reference":"AGREED-PENNY"})).await;
    op(&s,"charge.review",proposal["id"].as_str().expect("charge"),json!({"expected_load_version":v(&s).await,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier","confirmation_reference":"Confirmed penny","note":"Verified revised amount agreement"})).await;
    let original = approve_primary(&s).await;
    let mut second = canonical.clone();
    second["event_id"] = json!("supplemental-canonical");
    second["payload"]["invoice_number"] = json!("SUP-210");
    second["payload"]["net_amount"] = json!("12.51");
    let received = op(&s, "edi.event", "", second).await;
    let mut body = draft(
        &original,
        v(&s).await,
        "supplemental_invoice",
        "SUP-210",
        1251,
    );
    body["carrier_document_id"] = received["carrier_document_id"].clone();
    let mut tampered = body.clone();
    tampered["document_amount_cents"] = json!(1252);
    fails(
        &s,
        "payable.create",
        "load_6",
        tampered,
        "canonical carrier document",
    )
    .await;
    let a = review(&s, &op(&s, "payable.create", "load_6", body).await, true).await;
    assert_eq!(a["amount_cents"], 1251);
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.carrier_documents[1].status, "posted");
    assert_eq!(net(&w, "carrier_payable"), -119252);
}
#[tokio::test]
async fn credits_duplicates_invalid_dates_and_wrong_source_files_fail_without_partial_postings() {
    let s = seeded().await;
    let invoice = approve_primary(&s).await;
    let mut future = draft(&invoice, v(&s).await, "supplemental_invoice", "FUTURE", 1);
    future["issued_on"] = json!(Utc::now().date_naive() + chrono::Duration::days(1));
    fails(
        &s,
        "payable.create",
        "load_6",
        future,
        "cannot be in the future",
    )
    .await;
    let wrong = s
        .upload_document(
            ("demo", "finance"),
            "other-file",
            "load_7",
            Upload {
                kind: "carrier_invoice".into(),
                filename: "other.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: b"%PDF-1.4\nwrong shipment\n%%EOF\n".to_vec(),
            },
        )
        .await
        .expect("file");
    let mut foreign = draft(
        &invoice,
        v(&s).await,
        "supplemental_invoice",
        "WRONG-FILE",
        1,
    );
    foreign["document_id"] = wrong["data"]["id"].clone();
    fails(
        &s,
        "payable.create",
        "load_6",
        foreign,
        "Supporting document not found",
    )
    .await;
    fails(
        &s,
        "payable.create",
        "load_6",
        draft(&invoice, v(&s).await, "credit_memo", "TOO-MUCH", 118001),
        "Amount must be between",
    )
    .await;
    let a = review(
        &s,
        &create(&s, &invoice, "supplemental_invoice", "SUP-UNIQUE", 10001).await,
        true,
    )
    .await;
    fails(
        &s,
        "payable.create",
        "load_6",
        draft(
            &invoice,
            v(&s).await,
            "supplemental_invoice",
            "sup-unique",
            10001,
        ),
        "active draft or posting",
    )
    .await;
    let credit = review(
        &s,
        &create(&s, &invoice, "credit_memo", "CREDIT-ALL", 128001).await,
        true,
    )
    .await;
    fails(&s,"payable.reverse",a["id"].as_str().expect("id"),json!({"expected_load_version":v(&s).await,"expected_version":a["version"],"reversed_on":Utc::now().date_naive(),"counterparty_contact":"Carrier","confirmation_reference":"Case","note":"Must reverse dependent credit first"}),"Amount must be between").await;
    let mut spoof = draft(&invoice, v(&s).await, "supplemental_invoice", "SPOOF", 1);
    spoof["created_by"] = json!("admin");
    fails(
        &s,
        "payable.create",
        "load_6",
        spoof,
        "Invalid carrier payable request fields",
    )
    .await;
    reverse(&s, &credit).await;
    let before = s.read("demo").await.expect("read");
    assert_eq!(net(&before, "carrier_payable"), -128001);
    let wrong = create(&s, &invoice, "supplemental_invoice", "WRONG-MODE", 129001).await;
    let rejected = review(&s, &wrong, false).await;
    let mut correction = draft(
        &invoice,
        v(&s).await,
        "invoice_revision",
        "WRONG-MODE",
        129001,
    );
    correction["corrects_id"] = rejected["id"].clone();
    let corrected = review(
        &s,
        &op(&s, "payable.create", "load_6", correction).await,
        true,
    )
    .await;
    assert_eq!(corrected["amount_cents"],1000,"corrected classification posts a revised-total difference, not the full invoice a second time");
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_posting_refund_races_and_supporting_file_corruption_preserve_atomic_history() {
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("one");
    let b = Store::new(Some(&url)).await.expect("two");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("payables-test");
    canonical::put_workspace(&pool, &tenant, &json!(domain::demo()))
        .await
        .expect("fixture");
    let original = a.read(&tenant).await.expect("read").invoices[0].clone();
    invoice_review::approve(&a, &tenant, "primary", &original.id)
        .await
        .expect("approve");
    let bytes = b"%PDF-1.4\ncarrier credit proof\n%%EOF\n";
    let doc = a
        .upload_document(
            (&tenant, "finance"),
            "file",
            "load_6",
            Upload {
                kind: "carrier_invoice".into(),
                filename: "credit.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: bytes.to_vec(),
            },
        )
        .await
        .expect("file")["data"]
        .clone();
    let mut input = draft(&json!(original), 1, "credit_memo", "PG-CREDIT", 2501);
    input["document_id"] = doc["id"].clone();
    let adjustment = a
        .mutate(&tenant, "draft", "payable.create", "load_6", input)
        .await
        .expect("draft")["data"]
        .clone();
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupted".as_slice())
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("doc"))
    .execute(&pool)
    .await
    .expect("corrupt");
    let before = json!(a.read(&tenant).await.expect("before"));
    let body = review_body(2, &adjustment, true);
    assert!(a
        .mutate_authorized(
            (&tenant, "finance:a"),
            "post",
            "payable.review",
            adjustment["id"].as_str().expect("id"),
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(b.read(&tenant).await.expect("unchanged")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(bytes.as_slice())
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("doc"))
    .execute(&pool)
    .await
    .expect("restore");
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:a"),
            "post",
            "payable.review",
            adjustment["id"].as_str().expect("id"),
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "finance:b"),
            "post-b",
            "payable.review",
            adjustment["id"].as_str().expect("id"),
            body
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let prepared = a
        .mutate(&tenant, "prepare", "settlement.create", "load_6", json!({}))
        .await
        .expect("prepare")["data"]
        .clone();
    assert_eq!(prepared["amount_cents"], 118000);
    let w = a.read(&tenant).await.expect("read");
    assert_eq!(
        w.settlement_total(&w.settlements[0]).expect("total"),
        115499
    );
    a.mutate(
        &tenant,
        "pay-original-net",
        "cash.record",
        "",
        cash("payout", &prepared, 115499, "net-payment"),
    )
    .await
    .expect("pay");
    let w = a.read(&tenant).await.expect("read");
    let credit = a
        .mutate(
            &tenant,
            "second-credit",
            "payable.create",
            "load_6",
            draft(
                &json!(original),
                w.loads[5].version,
                "credit_memo",
                "PG-REFUND-CREDIT",
                1001,
            ),
        )
        .await
        .expect("credit")["data"]
        .clone();
    let w = a.read(&tenant).await.expect("read");
    a.mutate(
        &tenant,
        "post-second-credit",
        "payable.review",
        credit["id"].as_str().expect("id"),
        review_body(w.loads[5].version, &credit, true),
    )
    .await
    .expect("post credit");
    let w = a.read(&tenant).await.expect("read");
    let target = json!(w.settlements[0]);
    let (x, y) = tokio::join!(
        a.mutate(
            &tenant,
            "refund-a",
            "cash.record",
            "",
            cash("carrier_refund", &target, 1001, "refund-a")
        ),
        b.mutate(
            &tenant,
            "refund-b",
            "cash.record",
            "",
            cash("carrier_refund", &target, 1001, "refund-b")
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    drop(a);
    drop(b);
    let reopened = Store::new(Some(&url)).await.expect("reopen");
    let w = reopened.read(&tenant).await.expect("read");
    assert_eq!(w.carrier_adjustments[0].status, "posted");
    assert_eq!(net(&w, "carrier_payable"), 0);
    assert_eq!(net(&w, "cash"), -114498);
    assert_eq!(w.carrier_adjustments.len(), 2);
    assert!(reopened
        .read("foreign-payable-tenant")
        .await
        .expect("foreign")
        .carrier_adjustments
        .is_empty());
}
