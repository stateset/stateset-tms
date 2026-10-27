#[path = "support/canonical.rs"]
mod canonical;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    documents::Upload,
    domain::{self, Workspace},
    finance,
    store::Store,
};

const PDF: &[u8] = b"%PDF-1.7\ncancellation agreement\n%%EOF";
async fn read(s: &Store, t: &str) -> Workspace {
    s.read(t).await.expect("workspace")
}
async fn mutate(s: &Store, t: &str, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized((t, "operator:case"), &domain::id("key"), op, id, body)
        .await
        .expect(op)["data"]
        .clone()
}
async fn version(s: &Store, t: &str) -> u64 {
    read(s, t)
        .await
        .loads
        .iter()
        .find(|v| v.id == "load_4")
        .expect("load")
        .version
}
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let release=mutate(&s,"demo","tender.release.request","load_4",json!({"expected_load_version":version(&s,"demo").await,"disposition":"cancel","reason":"Customer cancelled before pickup"})).await;
    mutate(&s,"demo","tender.release.confirm",release["id"].as_str().expect("release"),json!({"expected_load_version":version(&s,"demo").await,"carrier_contact":"Carrier dispatch","confirmation_reference":"CANCEL-CONFIRMED","note":"No custody; release confirmed separately from fee eligibility"})).await;
    s
}
async fn propose(
    s: &Store,
    t: &str,
    side: &str,
    amount: i64,
    reference: &str,
    doc: Option<&str>,
    corrects: Option<&str>,
) -> Value {
    let w = read(s, t).await;
    let tender = w
        .tenders
        .iter()
        .find(|v| v.load_id == "load_4" && v.status == "released")
        .expect("released");
    mutate(s,t,"cancellation.propose","load_4",json!({"expected_load_version":version(s,t).await,"side":side,"tender_id":if side=="buy"{Some(&tender.id)}else{None},"amount_cents":amount,"description":"Agreed truck ordered not used <script>private-proof</script>","evidence_reference":reference,"document_id":doc,"corrects_id":corrects})).await
}
async fn review_body(s: &Store, t: &str, fee: &Value, number: &str) -> Value {
    json!({"expected_load_version":version(s,t).await,"expected_version":fee["version"],"decision":"approve","counterparty_contact":"Actual counterparty","confirmation_reference":"ACCEPTED-FEE","note":"Reviewed cancellation agreement and actual invoice","number":number,"due_date":if fee["side"]=="sell"{Some(Utc::now().date_naive())}else{None}})
}
async fn approve(s: &Store, t: &str, fee: &Value, number: &str) -> Value {
    mutate(
        s,
        t,
        "cancellation.review",
        fee["id"].as_str().expect("fee"),
        review_body(s, t, fee, number).await,
    )
    .await
}
async fn cash(
    s: &Store,
    t: &str,
    target: &str,
    direction: &str,
    amount: i64,
    reference: &str,
) -> Value {
    let w = read(s, t).await;
    let v = if direction == "receipt" {
        w.bills
            .iter()
            .find(|v| v.id == target)
            .expect("bill")
            .version
    } else {
        w.settlements
            .iter()
            .find(|v| v.id == target)
            .expect("settlement")
            .version
    };
    mutate(s,t,"cash.record","",json!({"direction":direction,"account":"External bank","external_reference":reference,"amount_cents":amount,"currency":"USD","cleared_on":Utc::now().date_naive(),"note":"Actual cleared bank evidence","allocations":[{"target_id":target,"expected_version":v,"amount_cents":amount}]})).await
}
async fn reverse_body(s: &Store, t: &str, fee: &Value) -> Value {
    let w = read(s, t).await;
    let target = fee["target_id"].as_str().expect("target");
    let v = if fee["side"] == "sell" {
        w.bills
            .iter()
            .find(|v| v.id == target)
            .expect("bill")
            .version
    } else {
        w.settlements
            .iter()
            .find(|v| v.id == target)
            .expect("settlement")
            .version
    };
    json!({"expected_load_version":version(s,t).await,"expected_version":fee["version"],"expected_target_version":v,"reversed_on":Utc::now().date_naive(),"counterparty_contact":"Actual counterparty","confirmation_reference":"FEE-RETRACTED","note":"Counterparty confirmed retraction; original history retained"})
}
async fn reverse(s: &Store, t: &str, fee: &Value) -> Value {
    mutate(
        s,
        t,
        "cancellation.reverse",
        fee["id"].as_str().expect("fee"),
        reverse_body(s, t, fee).await,
    )
    .await
}
fn net(w: &Workspace, account: &str) -> i64 {
    w.journal
        .iter()
        .flat_map(|v| &v.lines)
        .filter(|v| v.account == account)
        .map(|v| v.debit_cents - v.credit_cents)
        .sum()
}

#[tokio::test]
async fn cancellation_fees_post_only_actual_agreements_and_reconcile_without_delivery_or_original_rates(
) {
    let s = setup().await;
    let before = read(&s, "demo").await;
    let load = before
        .loads
        .iter()
        .find(|v| v.id == "load_4")
        .expect("load");
    assert_eq!(load.status, domain::Status::Cancelled);
    assert!(load.carrier_id.is_none());
    assert_eq!(
        before.commercial_summary(load).expect("summary")["margin_cents"],
        0
    );
    let buy = propose(&s, "demo", "buy", 25001, "CARRIER-TONU", None, None).await;
    let sell = propose(&s, "demo", "sell", 37501, "CUSTOMER-CANCEL", None, None).await;
    assert_eq!(read(&s, "demo").await.journal.len(), before.journal.len());
    let buy = approve(&s, "demo", &buy, "TONU-001").await;
    let sell = approve(&s, "demo", &sell, "CANCEL-001").await;
    let buy_target = buy["target_id"].as_str().expect("payable");
    let sell_target = sell["target_id"].as_str().expect("receivable");
    let w = read(&s, "demo").await;
    let l = w.loads.iter().find(|v| v.id == "load_4").expect("load");
    assert_eq!(l.buy_cents, 142000);
    assert_eq!(l.sell_cents, 178000);
    assert!(l.pod_reference.is_none());
    assert!(!w.delivery_evidence_ready(l));
    assert_eq!(w.settlement_balance(buy_target).expect("balance"), 25001);
    assert!(!w.invoices.iter().any(|v| v.load_id == l.id));
    assert_eq!(
        w.commercial_summary(l).expect("margin")["margin_cents"],
        12500
    );
    let statement = finance::statement(&w, sell_target).expect("statement").1;
    assert!(statement.contains("CANCELLATION INVOICE"));
    assert!(statement.contains("Shipment cancelled"));
    assert!(!statement.contains("Freight services"));
    assert!(statement.contains("&lt;script&gt;"));
    assert!(!statement.contains("CARRIER-TONU"));
    assert!(!statement.contains("Actual counterparty"));
    cash(&s, "demo", buy_target, "payout", 25001, "BANK-BUY").await;
    cash(&s, "demo", sell_target, "receipt", 37501, "BANK-SELL").await;
    let w = read(&s, "demo").await;
    assert_eq!(w.settlement_balance(buy_target).expect("closed"), 0);
    assert_eq!(
        w.bill_totals(w.bills.iter().find(|v| v.id == sell_target).expect("bill"))
            .expect("totals")
            .balance_cents,
        0
    );
    assert_eq!(net(&w, "cash") - net(&before, "cash"), 12500);
    assert_eq!(
        net(&w, "freight_expense") - net(&before, "freight_expense"),
        25001
    );
    assert_eq!(
        net(&w, "freight_revenue") - net(&before, "freight_revenue"),
        -37501
    );
    assert_eq!(
        w.outbox.len(),
        before.outbox.len(),
        "fee decisions do not send carrier or commerce price changes"
    );
    assert_eq!(
        w.journal
            .iter()
            .map(|v| v
                .lines
                .iter()
                .map(|l| l.debit_cents - l.credit_cents)
                .sum::<i64>())
            .sum::<i64>(),
        0
    );
    assert_eq!(w.events.last().expect("audit").actor, "operator:case");
}

#[tokio::test]
async fn paid_cancellation_reversal_preserves_original_payout_and_creates_refundable_carrier_credit(
) {
    let s = setup().await;
    let before = read(&s, "demo").await;
    let fee = propose(&s, "demo", "buy", 25001, "RETRACT-CARRIER", None, None).await;
    let fee = approve(&s, "demo", &fee, "TONU-PAID").await;
    let target = fee["target_id"].as_str().expect("target");
    let payout = cash(&s, "demo", target, "payout", 25001, "PAID-TONU").await;
    let reversed = reverse(&s, "demo", &fee).await;
    let w = read(&s, "demo").await;
    assert_eq!(w.settlement_balance(target).expect("credit"), -25001);
    assert!(w
        .cash_entries
        .iter()
        .find(|v| v.id == payout["id"])
        .expect("original payout")
        .reversal
        .is_none());
    let corrected = propose(
        &s,
        "demo",
        "buy",
        15001,
        "RETRACT-CARRIER",
        None,
        fee["id"].as_str(),
    )
    .await;
    let corrected = approve(&s, "demo", &corrected, "TONU-PAID").await;
    cash(&s, "demo", target, "carrier_refund", 25001, "REFUND-TONU").await;
    cash(
        &s,
        "demo",
        corrected["target_id"].as_str().expect("new target"),
        "payout",
        15001,
        "CORRECTED-PAYOUT",
    )
    .await;
    let w = read(&s, "demo").await;
    assert_eq!(
        net(&w, "freight_expense") - net(&before, "freight_expense"),
        15001
    );
    assert_eq!(net(&w, "cash") - net(&before, "cash"), -15001);
    assert_eq!(w.cancellation_fees.len(), 2);
    assert_eq!(reversed["status"], "reversed");
    assert_eq!(
        w.cancellation_fees[0].corrected_by_id.as_deref(),
        corrected["id"].as_str()
    );
    assert_eq!(
        w.settlements
            .iter()
            .find(|v| v.id == target)
            .expect("original principal")
            .amount_cents,
        25001
    );
}

#[tokio::test]
async fn cancellation_guards_require_release_proof_strict_fields_and_explicit_nonbranching_corrections(
) {
    let s = setup().await;
    let fee = propose(&s, "demo", "buy", 12345, "DUPLICATE", None, None).await;
    let before = json!(read(&s, "demo").await);
    let base = json!({"expected_load_version":version(&s,"demo").await,"side":"buy","tender_id":fee["tender_id"],"amount_cents":12345,"description":"Cancellation","evidence_reference":"DUPLICATE"});
    for (field, value) in [
        ("amount_cents", json!(0)),
        ("amount_cents", json!(-1)),
        ("amount_cents", json!(100000001)),
        ("tender_id", json!("other")),
        ("expected_load_version", json!(0)),
        ("party_id", json!("forged")),
        ("document_id", json!("other-tenant-file")),
    ] {
        let mut bad = base.clone();
        bad[field] = value;
        bad["evidence_reference"] = json!(format!("GUARD-{field}"));
        assert!(s
            .mutate(
                "demo",
                &domain::id("bad"),
                "cancellation.propose",
                "load_4",
                bad
            )
            .await
            .is_err());
        assert_eq!(json!(read(&s, "demo").await), before);
    }
    let mut body = review_body(&s, "demo", &fee, "REJECTED").await;
    body["decision"] = json!("reject");
    let rejected = mutate(
        &s,
        "demo",
        "cancellation.review",
        fee["id"].as_str().expect("fee"),
        body,
    )
    .await;
    let mut implicit = base.clone();
    implicit["expected_load_version"] = json!(version(&s, "demo").await);
    assert!(s
        .mutate(
            "demo",
            "implicit",
            "cancellation.propose",
            "load_4",
            implicit
        )
        .await
        .is_err());
    let corrected = propose(
        &s,
        "demo",
        "buy",
        10001,
        "DUPLICATE",
        None,
        rejected["id"].as_str(),
    )
    .await;
    let before = json!(read(&s, "demo").await);
    let mut branch = base;
    branch["expected_load_version"] = json!(version(&s, "demo").await);
    branch["corrects_id"] = rejected["id"].clone();
    assert!(s
        .mutate("demo", "branch", "cancellation.propose", "load_4", branch)
        .await
        .is_err());
    assert_eq!(json!(read(&s, "demo").await), before);
    let body = review_body(&s, "demo", &corrected, "ACTIVE").await;
    let id = corrected["id"].as_str().expect("fee");
    let posted = s
        .mutate(
            "demo",
            "approve-once",
            "cancellation.review",
            id,
            body.clone(),
        )
        .await
        .expect("approve");
    let replay = s
        .mutate("demo", "approve-once", "cancellation.review", id, body)
        .await
        .expect("replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(posted["data"], replay["data"]);
    let unrelated = propose(&s, "demo", "buy", 20001, "UNRELATED-FEE", None, None).await;
    let duplicate_body = review_body(&s, "demo", &unrelated, "active").await;
    let before = json!(read(&s, "demo").await);
    assert!(s
        .mutate(
            "demo",
            "duplicate-number",
            "cancellation.review",
            unrelated["id"].as_str().expect("fee"),
            duplicate_body
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s, "demo").await), before);
    let other_version = read(&s, "demo")
        .await
        .loads
        .iter()
        .find(|v| v.id == "load_8")
        .expect("other load")
        .version;
    mutate(
        &s,
        "demo",
        "load.command",
        "load_8",
        json!({"action":"tender","expected_version":other_version,"carrier_id":"ca4"}),
    )
    .await;
    let before = json!(read(&s, "demo").await);
    assert!(s
        .mutate(
            "demo",
            "duplicate-primary",
            "invoice.create",
            "",
            json!({"load_id":"load_8","number":"ACTIVE","amount_cents":138000,"currency":"USD"})
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s, "demo").await), before);
    assert!(s
        .mutate("other", "tenant", "cancellation.review", id, json!({}))
        .await
        .is_err());
}

#[tokio::test]
async fn customer_cancellation_reversal_requires_bank_and_credit_unwind_and_protects_fee_documents()
{
    let s = setup().await;
    let doc = s
        .upload_document(
            ("demo", "ops"),
            "file",
            "load_4",
            Upload {
                kind: "other".into(),
                filename: "cancellation.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("upload")["data"]["id"]
        .as_str()
        .expect("doc")
        .to_owned();
    let fee = propose(
        &s,
        "demo",
        "sell",
        20001,
        "CUSTOMER-EVIDENCE",
        Some(&doc),
        None,
    )
    .await;
    let fee = approve(&s, "demo", &fee, "CUSTOMER-EVIDENCE-INV").await;
    let target = fee["target_id"].as_str().expect("target");
    assert!(s
        .mutate(
            "demo",
            "reject-doc",
            "document.review",
            &doc,
            json!({"expected_version":1,"decision":"rejected","note":"Cannot erase posted proof"})
        )
        .await
        .is_err());
    assert!(s
        .mutate(
            "demo",
            "generic-void",
            "bill.void",
            target,
            json!({"expected_version":1,"note":"Must reverse agreement"})
        )
        .await
        .is_err());
    let receipt = cash(&s, "demo", target, "receipt", 10001, "CUSTOMER-PARTIAL").await;
    let bill_version = read(&s, "demo")
        .await
        .bills
        .iter()
        .find(|v| v.id == target)
        .expect("bill")
        .version;
    let credit=mutate(&s,"demo","credit.create",target,json!({"expected_version":bill_version,"number":"CANCEL-CREDIT","amount_cents":1000,"reason":"Customer concession"})).await;
    let before = json!(read(&s, "demo").await);
    assert!(s
        .mutate(
            "demo",
            "paid-reverse",
            "cancellation.reverse",
            fee["id"].as_str().expect("fee"),
            reverse_body(&s, "demo", &fee).await
        )
        .await
        .is_err());
    assert_eq!(json!(read(&s, "demo").await), before);
    mutate(&s,"demo","cash.reverse",receipt["id"].as_str().expect("receipt"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Actual reconciliation corrected"})).await;
    assert!(s
        .mutate(
            "demo",
            "credit-still-active",
            "cancellation.reverse",
            fee["id"].as_str().expect("fee"),
            reverse_body(&s, "demo", &fee).await
        )
        .await
        .is_err());
    mutate(&s,"demo","credit.reverse",credit["id"].as_str().expect("credit"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Actual concession withdrawn"})).await;
    reverse(&s, "demo", &fee).await;
    let w = read(&s, "demo").await;
    assert_eq!(
        w.bills
            .iter()
            .find(|v| v.id == target)
            .expect("bill")
            .status,
        "void"
    );
    assert_eq!(
        w.commercial_summary(w.loads.iter().find(|v| v.id == "load_4").expect("load"))
            .expect("margin")["total_sell_cents"],
        0
    );
    assert!(finance::statement(&w, target)
        .expect("retained statement")
        .1
        .contains("VOID CANCELLATION INVOICE"));
}

#[tokio::test]
async fn independent_cancelled_payables_can_receive_agreed_carrier_credit_without_duplicate_recognition(
) {
    let s = setup().await;
    let before = read(&s, "demo").await;
    let source = propose(&s, "demo", "buy", 25001, "CANCEL-SOURCE", None, None).await;
    let source = approve(&s, "demo", &source, "CANCEL-SOURCE-INV").await;
    let source_id = source["target_id"].as_str().expect("source");
    let target = propose(&s, "demo", "buy", 10000, "CANCEL-TARGET", None, None).await;
    let target = approve(&s, "demo", &target, "CANCEL-TARGET-INV").await;
    let target_id = target["target_id"].as_str().expect("target");
    cash(
        &s,
        "demo",
        source_id,
        "payout",
        25001,
        "CANCEL-ORIGINAL-PAYOUT",
    )
    .await;
    reverse(&s, "demo", &source).await;
    let w = read(&s, "demo").await;
    let application=mutate(&s,"demo","carrier_credit.apply","",json!({"source_settlement_id":source_id,"expected_source_version":w.settlements.iter().find(|v|v.id==source_id).expect("source").version,"amount_cents":10000,"applied_on":Utc::now().date_naive(),"allocations":[{"target_id":target_id,"expected_version":w.settlements.iter().find(|v|v.id==target_id).expect("target").version,"amount_cents":10000}],"evidence_reference":"CANCEL-CREDIT-AGREED","counterparty_contact":"Carrier accounts","confirmation_reference":"APPLY-TO-TARGET","note":"Carrier confirmed application to the other cancellation invoice"})).await;
    let w = read(&s, "demo").await;
    assert_eq!(w.settlement_balance(source_id).expect("source"), -15001);
    assert_eq!(w.settlement_balance(target_id).expect("target"), 0);
    assert_eq!(
        net(&w, "freight_expense") - net(&before, "freight_expense"),
        10000
    );
    assert!(
        w.journal
            .iter()
            .filter(|v| v.source_kind == "carrier_invoice")
            .all(|v| v.lines.iter().all(|l| l.load_id != "load_4")),
        "no fabricated delivery invoice or second expense"
    );
    cash(
        &s,
        "demo",
        source_id,
        "carrier_refund",
        15001,
        "CANCEL-REMAINING-REFUND",
    )
    .await;
    let w = read(&s, "demo").await;
    assert_eq!(net(&w, "cash") - net(&before, "cash"), -10000);
    assert_eq!(w.settlement_balance(source_id).expect("closed"), 0);
    assert_eq!(application["amount_cents"], 10000);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cancellation_decisions_and_bank_allocations_keep_original_proof_under_races_and_corruption(
) {
    let fixture = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    let tenant = domain::id("cancel-test");
    canonical::put_workspace(&pool, &tenant, &json!(read(&fixture, "demo").await))
        .await
        .expect("owned data");
    let doc = a
        .upload_document(
            (&tenant, "ops"),
            "proof",
            "load_4",
            Upload {
                kind: "other".into(),
                filename: "cancellation.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("proof")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let fee = propose(&a, &tenant, "buy", 25001, "PG-CANCEL", Some(&doc), None).await;
    let body = review_body(&a, &tenant, &fee, "PG-TONU-INV").await;
    let id = fee["id"].as_str().expect("fee");
    let before = json!(read(&a, &tenant).await);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"broken".as_slice())
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("corrupt owned bytes");
    assert!(a
        .mutate(
            &tenant,
            "approve-retry",
            "cancellation.review",
            id,
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(read(&a, &tenant).await), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("restore");
    let (first, second) = tokio::join!(
        a.mutate(
            &tenant,
            "approve-retry",
            "cancellation.review",
            id,
            body.clone()
        ),
        b.mutate(&tenant, "competing", "cancellation.review", id, body)
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let w = read(&a, &tenant).await;
    let posted = &w.cancellation_fees[0];
    let target = posted.target_id.as_deref().expect("target");
    let target_version = w
        .settlements
        .iter()
        .find(|v| v.id == target)
        .expect("settlement")
        .version;
    let payment = json!({"direction":"payout","account":"bank","external_reference":"PG-TONU-BANK","amount_cents":25001,"currency":"USD","cleared_on":Utc::now().date_naive(),"note":"Cleared evidence","allocations":[{"target_id":target,"expected_version":target_version,"amount_cents":25001}]});
    let before_payment = json!(read(&a, &tenant).await);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt after approval".as_slice())
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("corrupt owned payment evidence");
    assert!(a
        .mutate(&tenant, "bank-one", "cash.record", "", payment.clone())
        .await
        .is_err());
    assert_eq!(
        json!(read(&a, &tenant).await),
        before_payment,
        "cash, journal, target and receipt roll back together"
    );
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("restore payment proof");
    let (first, second) = tokio::join!(
        a.mutate(&tenant, "bank-one", "cash.record", "", payment.clone()),
        b.mutate(&tenant, "bank-two", "cash.record", "", payment)
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    drop((a, b));
    let restarted = Store::new(Some(&url)).await.expect("reconnect");
    let w = read(&restarted, &tenant).await;
    assert_eq!(w.settlement_balance(target).expect("balance"), 0);
    assert_eq!(
        w.cancellation_fees[0].document_sha256.as_deref(),
        Some(stateset_freight::documents::sha256(PDF).as_str())
    );
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == "load_4")
            .expect("load")
            .status,
        domain::Status::Cancelled
    );
    assert!(read(&restarted, "another-cancel-tenant")
        .await
        .cancellation_fees
        .is_empty());
}

#[tokio::test]
async fn paid_customer_cancellation_retraction_retains_genuine_receipt_and_reconciles_only_the_amount_actually_received(
) {
    let s = setup().await;
    let fee = propose(
        &s,
        "demo",
        "sell",
        20001,
        "CUSTOMER-REFUND-AGREEMENT",
        None,
        None,
    )
    .await;
    let fee = approve(&s, "demo", &fee, "PAID-CUSTOMER-CANCELLATION").await;
    let target = fee["target_id"].as_str().expect("target");
    let receipt = cash(
        &s,
        "demo",
        target,
        "receipt",
        10001,
        "GENUINE-CUSTOMER-RECEIPT",
    )
    .await;
    reverse(&s, "demo", &fee).await;
    let w = read(&s, "demo").await;
    let bill = w.bills.iter().find(|v| v.id == target).expect("void bill");
    assert_eq!(bill.status, "void");
    let totals = w.bill_totals(bill).expect("totals");
    assert_eq!(totals.refund_due_cents, 10001);
    assert_eq!(totals.received_cents, 10001);
    assert_eq!(bill.snapshot.amount_cents, 20001);
    assert!(w
        .cash_entries
        .iter()
        .find(|v| Some(v.id.as_str()) == receipt["id"].as_str())
        .expect("receipt retained")
        .reversal
        .is_none());
    let refund=mutate(&s,"demo","cash.record","",json!({"direction":"customer_refund","currency":"USD","account":"Operating-USD","external_reference":"ACTUAL-CANCELLATION-REFUND","amount_cents":10001,"cleared_on":Utc::now().date_naive(),"note":"Actual bank refund after agreed cancellation fee retraction","allocations":[{"target_id":target,"expected_version":bill.version,"amount_cents":10001}],"confirmation":{"counterparty_contact":"Actual customer AP lead","confirmation_reference":"RETURN-CLEARED-FUNDS","evidence_reference":"Original cancellation retraction and bank statement","note":"Customer confirms actual refund of the received portion"}})).await;
    let w = read(&s, "demo").await;
    assert_eq!(
        w.bill_totals(w.bills.iter().find(|v| v.id == target).expect("bill"))
            .expect("totals")
            .balance_cents,
        0
    );
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == "load_4")
            .expect("load")
            .status,
        domain::Status::Cancelled
    );
    assert_eq!(refund["amount_cents"], 10001);
    assert_eq!(w.cash_entries[0].amount_cents, 10001);
    assert!(w.cash_entries[0].reversal.is_none());
    let statement = finance::statement(&w, target).expect("statement").1;
    assert!(statement.contains("VOID CANCELLATION"));
    assert!(statement.contains("Reconciled customer refunds"));
    assert!(!statement.contains("RETURN-CLEARED-FUNDS"));
}
