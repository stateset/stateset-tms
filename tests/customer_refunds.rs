#[path = "support/canonical.rs"]
mod canonical;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    documents::Upload,
    domain,
    finance::{self, CustomerBill},
    store::Store,
};
const PDF: &[u8] = b"%PDF-1.7\nActual synthetic customer credit and refund agreement\n%%EOF";
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn run(s: &Store, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "user:refund-finance"),
        &domain::id("request"),
        op,
        id,
        body,
    )
    .await
    .expect(op)["data"]
        .clone()
}
async fn current(s: &Store, id: &str) -> CustomerBill {
    s.read("demo")
        .await
        .expect("read")
        .bills
        .into_iter()
        .find(|v| v.id == id)
        .expect("bill")
}
async fn issue(s: &Store, load: &str, number: &str) -> CustomerBill {
    let w = s.read("demo").await.expect("read");
    let l = w.loads.iter().find(|v| v.id == load).expect("load");
    if l.status == domain::Status::InTransit {
        run(s,"load.command",load,json!({"action":"deliver","expected_version":l.version,"pod_reference":"Actual receiver evidence"})).await;
    }
    let w = s.read("demo").await.expect("read");
    let l = w.loads.iter().find(|v| v.id == load).expect("load");
    if !w.delivery_evidence_ready(l) {
        run(s,"pod.review",load,json!({"expected_version":l.version,"decision":"accepted","note":"Finance verified actual external receiver evidence"})).await;
    }
    let b = run(
        s,
        "bill.create",
        "",
        json!({"load_id":load,"number":number,"due_date":Utc::now().date_naive()}),
    )
    .await;
    serde_json::from_value(
        run(
            s,
            "bill.issue",
            b["id"].as_str().expect("id"),
            json!({"expected_version":1}),
        )
        .await,
    )
    .expect("bill")
}
fn confirmation(doc: Option<&str>) -> Value {
    json!({"counterparty_contact":"Actual customer accounts payable","confirmation_reference":"CUSTOMER-CONFIRM-ORIGINAL","evidence_reference":"Original customer credit/refund correspondence","note":"Customer confirms credit and actual cleared refund","document_id":doc})
}
fn cash(direction: &str, bill: &CustomerBill, amount: i64, reference: &str) -> Value {
    let mut input = json!({"direction":direction,"account":"Operating-USD","external_reference":reference,"amount_cents":amount,"currency":"USD","cleared_on":Utc::now().date_naive(),"note":"Actual cleared bank evidence reconciled by finance","allocations":[{"target_id":bill.id,"expected_version":bill.version,"amount_cents":amount}]});
    if direction == "customer_refund" {
        input["confirmation"] = confirmation(None);
    }
    input
}
async fn paid(s: &Store, number: &str) -> CustomerBill {
    let b = issue(s, "load_6", number).await;
    run(
        s,
        "cash.record",
        "",
        cash(
            "receipt",
            &b,
            b.snapshot.amount_cents,
            "ORIGINAL-REAL-RECEIPT",
        ),
    )
    .await;
    current(s, &b.id).await
}
async fn credit(s: &Store, bill: &CustomerBill, amount: i64, doc: Option<&str>) -> Value {
    run(s,"credit.create",&bill.id,json!({"expected_version":bill.version,"number":domain::id("CN"),"amount_cents":amount,"reason":"Actual agreed service credit <script>private findings</script>","confirmation":confirmation(doc)})).await
}
fn account(w: &domain::Workspace, name: &str) -> i64 {
    w.journal
        .iter()
        .flat_map(|j| &j.lines)
        .filter(|l| l.account == name)
        .map(|l| l.debit_cents - l.credit_cents)
        .sum()
}
#[tokio::test]
async fn paid_credit_and_partial_customer_refunds_preserve_actual_receipt_and_close_independently_calculated_balances(
) {
    let s = setup().await;
    let bill = paid(&s, "PAID-CUSTOMER").await;
    let before = s.read("demo").await.expect("read");
    let original_receipt = json!(before.cash_entries[0]);
    let snapshot = json!(bill.snapshot);
    credit(&s, &bill, 9001, None).await;
    let w = s.read("demo").await.expect("read");
    let b = current(&s, &bill.id).await;
    let t = w.bill_totals(&b).expect("totals");
    assert_eq!(t.received_cents, 149000);
    assert_eq!(t.credited_cents, 9001);
    assert_eq!(t.balance_cents, -9001);
    assert_eq!(t.refund_due_cents, 9001);
    assert_eq!(t.remaining_creditable_cents, 139999);
    assert_eq!(b.status, "credit_due");
    assert_eq!(w.finance_summary().expect("summary")["receivable_cents"], 0);
    assert_eq!(
        w.finance_summary().expect("summary")["customer_credit_cents"],
        9001
    );
    assert_eq!(account(&w, "accounts_receivable"), -9001);
    let first = run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &b, 3001, "ACTUAL-REFUND-ONE"),
    )
    .await;
    assert_eq!(first["direction"], "customer_refund");
    let b = current(&s, &bill.id).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.bill_totals(&b).expect("totals").refund_due_cents, 6000);
    run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &b, 6000, "ACTUAL-REFUND-TWO"),
    )
    .await;
    let w = s.read("demo").await.expect("read");
    let b = current(&s, &bill.id).await;
    let t = w.bill_totals(&b).expect("totals");
    assert_eq!(
        (
            t.received_cents,
            t.refunded_cents,
            t.credited_cents,
            t.balance_cents
        ),
        (149000, 9001, 9001, 0)
    );
    assert_eq!(json!(w.cash_entries[0]), original_receipt);
    assert_eq!(json!(b.snapshot), snapshot);
    assert_eq!(account(&w, "accounts_receivable"), 0);
    assert_eq!(account(&w, "cash"), 139999);
    assert_eq!(account(&w, "freight_revenue"), -139999);
    let html = finance::statement(&w, &bill.id).expect("statement").1;
    assert!(html.contains("1490.00"));
    assert!(html.contains("90.01"));
    assert!(html.contains("Reconciled customer refunds"));
    for private in [
        "CUSTOMER-CONFIRM-ORIGINAL",
        "private findings",
        "Operating-USD",
    ] {
        assert!(!html.contains(private));
    }
    for entry in &w.journal {
        assert_eq!(
            entry.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            entry.lines.iter().map(|v| v.credit_cents).sum::<i64>()
        );
    }
}
#[tokio::test]
async fn paid_credit_and_refund_require_actual_confirmation_current_versions_and_remaining_credit_limits(
) {
    let s = setup().await;
    let bill = paid(&s, "REFUND-GUARDS").await;
    let before = json!(s.read("demo").await.expect("read"));
    assert!(s.mutate("demo","unconfirmed-credit","credit.create",&bill.id,json!({"expected_version":bill.version,"number":"NO-CONSENT","amount_cents":9001,"reason":"Missing actual customer confirmation"})).await.is_err());
    assert_eq!(json!(s.read("demo").await.expect("read")), before);
    credit(&s, &bill, 9001, None).await;
    let bill = current(&s, &bill.id).await;
    for field in ["confirmation", "amount", "version", "reference"] {
        let mut body = cash("customer_refund", &bill, 9001, "GUARD-REFUND");
        match field {
            "confirmation" => {
                body.as_object_mut().expect("object").remove("confirmation");
            }
            "amount" => {
                body["allocations"][0]["amount_cents"] = json!(9002);
                body["amount_cents"] = json!(9002);
            }
            "version" => body["allocations"][0]["expected_version"] = json!(1),
            _ => body["external_reference"] = json!(""),
        };
        let before = json!(s.read("demo").await.expect("read"));
        assert!(s
            .mutate("demo", &domain::id("bad"), "cash.record", "", body)
            .await
            .is_err());
        assert_eq!(json!(s.read("demo").await.expect("read")), before);
    }
    assert!(s.mutate("demo","excess-credit","credit.create",&bill.id,json!({"expected_version":bill.version,"number":"OVER-PRINCIPAL","amount_cents":140000,"reason":"Would exceed original invoice amount","confirmation":confirmation(None)})).await.is_err());
    let body = cash("customer_refund", &bill, 9001, "REFUND-ONCE");
    let first = s
        .mutate("demo", "refund-key", "cash.record", "", body.clone())
        .await
        .expect("refund");
    let replay = s
        .mutate("demo", "refund-key", "cash.record", "", body.clone())
        .await
        .expect("replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["data"], replay["data"]);
    assert!(s
        .mutate("demo", "other-key", "cash.record", "", body)
        .await
        .is_err());
}
#[tokio::test]
async fn credit_retraction_after_actual_refund_reopens_receivable_without_reversing_bank_history() {
    let s = setup().await;
    let bill = paid(&s, "CREDIT-RETRACT").await;
    let cr = credit(&s, &bill, 9001, None).await;
    let bill = current(&s, &bill.id).await;
    let refund = run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &bill, 9001, "REFUND-RETAINED"),
    )
    .await;
    let w = s.read("demo").await.expect("read");
    let bill = current(&s, &bill.id).await;
    let mut input = json!({"expected_version":cr["version"],"reversed_on":Utc::now().date_naive(),"note":"Customer confirms actual credit was recorded to the wrong invoice"});
    assert!(s
        .mutate(
            "demo",
            "no-invoice-version",
            "credit.reverse",
            cr["id"].as_str().expect("credit"),
            input.clone()
        )
        .await
        .is_err());
    input["expected_bill_version"] = json!(bill.version - 1);
    assert!(s
        .mutate(
            "demo",
            "stale-invoice-version",
            "credit.reverse",
            cr["id"].as_str().expect("credit"),
            input.clone()
        )
        .await
        .is_err());
    input["expected_bill_version"] = json!(bill.version);
    run(
        &s,
        "credit.reverse",
        cr["id"].as_str().expect("credit"),
        input,
    )
    .await;
    let after = s.read("demo").await.expect("read");
    let bill = current(&s, &bill.id).await;
    assert_eq!(
        after.bill_totals(&bill).expect("totals").balance_cents,
        9001
    );
    assert_eq!(json!(after.cash_entries), json!(w.cash_entries));
    assert_eq!(after.cash_entries[1].id, refund["id"]);
    assert_eq!(account(&after, "cash"), 139999);
    assert_eq!(account(&after, "accounts_receivable"), 9001);
}
#[tokio::test]
async fn refund_reversal_and_explicit_bank_correction_preserve_original_allocations_and_prevent_branching(
) {
    let s = setup().await;
    let bill = paid(&s, "REFUND-CORRECTION").await;
    credit(&s, &bill, 9001, None).await;
    let bill = current(&s, &bill.id).await;
    let refund = run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &bill, 9001, "REFUND-CORRECTED"),
    )
    .await;
    run(&s,"cash.reverse",refund["id"].as_str().expect("refund"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Bank statement confirms only a partial refund cleared"})).await;
    let bill = current(&s, &bill.id).await;
    assert_eq!(
        s.read("demo")
            .await
            .expect("read")
            .bill_totals(&bill)
            .expect("totals")
            .refund_due_cents,
        9001
    );
    let mut body = cash("customer_refund", &bill, 3001, "REFUND-CORRECTED");
    assert!(s
        .mutate(
            "demo",
            "implicit-correction",
            "cash.record",
            "",
            body.clone()
        )
        .await
        .is_err());
    body["corrects_id"] = refund["id"].clone();
    let corrected = run(&s, "cash.record", "", body.clone()).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.cash_entries[1].corrected_by_id.as_deref(),
        corrected["id"].as_str()
    );
    assert_eq!(w.cash_entries[1].allocations[0].amount_cents, 9001);
    assert_eq!(
        w.bill_totals(&current(&s, &bill.id).await)
            .expect("totals")
            .refund_due_cents,
        6000
    );
    assert!(s
        .mutate("demo", "branch-correction", "cash.record", "", body)
        .await
        .is_err());
}
#[tokio::test]
async fn customer_refund_proof_is_bound_to_affected_shipments_and_preserved_after_posting() {
    let s = setup().await;
    let bill = paid(&s, "REFUND-PROOF").await;
    let upload = |kind: &str| Upload {
        kind: kind.into(),
        filename: "actual-credit-agreement.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: PDF.to_vec(),
    };
    let wrong = s
        .upload_document(("demo", "finance"), "wrong-file", "load_7", upload("other"))
        .await
        .expect("wrong load file")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert!(s.mutate("demo","wrong-credit-file","credit.create",&bill.id,json!({"expected_version":bill.version,"number":"WRONG-FILE-CREDIT","amount_cents":9001,"reason":"Must use same shipment evidence","confirmation":confirmation(Some(&wrong))})).await.is_err());
    let doc = s
        .upload_document(
            ("demo", "finance"),
            "good-file",
            &bill.load_id,
            upload("other"),
        )
        .await
        .expect("file")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let cr = credit(&s, &bill, 9001, Some(&doc)).await;
    assert!(cr["confirmation"]["document_sha256"].is_string());
    let bill = current(&s, &bill.id).await;
    let mut body = cash("customer_refund", &bill, 9001, "REFUND-PROOF-BANK");
    body["confirmation"] = confirmation(Some(&doc));
    run(&s, "cash.record", "", body).await;
    assert!(s.mutate("demo","reject-posted-proof","document.review",&doc,json!({"expected_version":1,"decision":"rejected","note":"Posted bank/source proof must retain its original evidence"})).await.is_err());
}

#[tokio::test]
async fn customer_refund_batches_require_one_customer_and_keep_positive_receivables_separate_from_credit_balances(
) {
    let s = setup().await;
    let a = paid(&s, "BATCH-PARENT").await;
    credit(&s, &a, 9001, None).await;
    let sup=run(&s,"bill.supplement",&a.id,json!({"expected_version":current(&s,&a.id).await.version,"expected_load_version":s.read("demo").await.expect("read").loads.iter().find(|v|v.id==a.load_id).expect("load").version,"number":"BATCH-SUPPLEMENT","due_date":Utc::now().date_naive(),"amount_cents":5001,"description":"Actual later customer addition","counterparty_contact":"Actual customer AP","confirmation_reference":"SUPPLEMENT-AGREED","evidence_reference":"Original agreed unloading addition"})).await;
    let sup = run(
        &s,
        "bill.issue",
        sup["id"].as_str().expect("supplement"),
        json!({"expected_version":1}),
    )
    .await;
    let b = current(&s, sup["id"].as_str().expect("id")).await;
    run(
        &s,
        "cash.record",
        "",
        cash("receipt", &b, 5001, "SUPPLEMENT-RECEIPT"),
    )
    .await;
    let b = current(&s, &b.id).await;
    credit(&s, &b, 2001, None).await;
    let other = issue(&s, "load_7", "OTHER-CUSTOMER").await;
    run(
        &s,
        "cash.record",
        "",
        cash(
            "receipt",
            &other,
            other.snapshot.amount_cents,
            "OTHER-RECEIPT",
        ),
    )
    .await;
    credit(&s, &current(&s, &other.id).await, 3001, None).await;
    let a = current(&s, &a.id).await;
    let b = current(&s, &b.id).await;
    let other = current(&s, &other.id).await;
    let mut mixed = cash("customer_refund", &a, 1, "MIXED-REFUND");
    mixed["amount_cents"] = json!(2);
    mixed["allocations"] = json!([{"target_id":a.id,"expected_version":a.version,"amount_cents":1},{"target_id":other.id,"expected_version":other.version,"amount_cents":1}]);
    let before = json!(s.read("demo").await.expect("read"));
    assert!(s
        .mutate("demo", "mixed-customers", "cash.record", "", mixed)
        .await
        .is_err());
    assert_eq!(json!(s.read("demo").await.expect("read")), before);
    let mut batch = cash("customer_refund", &a, 11002, "ACTUAL-BATCH-REFUND");
    batch["allocations"] = json!([{"target_id":a.id,"expected_version":a.version,"amount_cents":9001},{"target_id":b.id,"expected_version":b.version,"amount_cents":2001}]);
    run(&s, "cash.record", "", batch).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.finance_summary().expect("summary")["customer_credit_cents"],
        3001
    );
    assert_eq!(
        w.bill_totals(&current(&s, &a.id).await)
            .expect("a")
            .balance_cents,
        0
    );
    assert_eq!(
        w.bill_totals(&current(&s, &b.id).await)
            .expect("b")
            .balance_cents,
        0
    );
}
#[tokio::test]
async fn customer_credit_and_bank_reversals_cannot_be_backdated_before_refund_activity() {
    let s = setup().await;
    let bill = paid(&s, "CHRONOLOGY").await;
    let cr = credit(&s, &bill, 9001, None).await;
    let bill = current(&s, &bill.id).await;
    run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &bill, 9001, "TODAY-REFUND"),
    )
    .await;
    let mut historical = s.read("demo").await.expect("read");
    historical.bills[0].issued_at = Some(Utc::now() - chrono::Duration::days(4));
    historical.credits[0].issued_on = Some(Utc::now().date_naive() - chrono::Duration::days(2));
    historical.cash_entries[0].cleared_on = Utc::now().date_naive() - chrono::Duration::days(3);
    let before = json!(historical);
    let result=historical.apply_finance("credit.reverse",cr["id"].as_str().expect("credit"),json!({"expected_version":1,"expected_bill_version":historical.bills[0].version,"reversed_on":Utc::now().date_naive()-chrono::Duration::days(1),"note":"Cannot rewrite current balances before the real refund"}),"finance:chronology");
    assert!(result.is_err());
    assert_eq!(json!(historical), before);
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_competing_customer_refunds_and_corrupt_credit_proof_are_atomic_and_durable() {
    let memory = setup().await;
    let bill = paid(&memory, "PG-CUSTOMER-REFUND").await;
    let document = memory
        .upload_document(
            ("demo", "finance:test"),
            "refund-proof",
            &bill.load_id,
            Upload {
                kind: "other".into(),
                filename: "refund-agreement.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("file")["data"]["id"]
        .as_str()
        .expect("doc")
        .to_owned();
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("customer-refund-pg");
    let mut data = json!(memory.read("demo").await.expect("memory"));
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("tenant");
    sqlx::query(
        "INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)",
    )
    .bind(&tenant)
    .bind(&document)
    .bind(b"corrupted".as_slice())
    .execute(&pool)
    .await
    .expect("corrupt original bytes");
    let credit = json!({"expected_version":bill.version,"number":"PG-CREDIT","amount_cents":9001,"reason":"Actual customer credit agreement","confirmation":confirmation(Some(&document))});
    let before = json!(a.read(&tenant).await.expect("before"));
    assert!(a
        .mutate(
            &tenant,
            "credit-once",
            "credit.create",
            &bill.id,
            credit.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("unchanged")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&document)
    .execute(&pool)
    .await
    .expect("restore");
    a.mutate_authorized(
        (&tenant, "finance:original"),
        "credit-once",
        "credit.create",
        &bill.id,
        credit,
    )
    .await
    .expect("same failed key after restoration");
    let current = a
        .read(&tenant)
        .await
        .expect("current")
        .bills
        .into_iter()
        .find(|v| v.id == bill.id)
        .expect("bill");
    let mut input = cash("customer_refund", &current, 9001, "ACTUAL-PG-REFUND");
    input["confirmation"] = confirmation(Some(&document));
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupted-again".as_slice())
    .bind(&tenant)
    .bind(&document)
    .execute(&pool)
    .await
    .expect("corrupt before bank post");
    let before = json!(a.read(&tenant).await.expect("before"));
    assert!(a
        .mutate(&tenant, "refund-a", "cash.record", "", input.clone())
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("unchanged")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&document)
    .execute(&pool)
    .await
    .expect("restore");
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:first"),
            "refund-a",
            "cash.record",
            "",
            input.clone()
        ),
        b.mutate_authorized(
            (&tenant, "finance:second"),
            "refund-b",
            "cash.record",
            "",
            input
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
    assert_eq!(w.bill_totals(&w.bills[0]).expect("totals").balance_cents, 0);
    assert_eq!(w.cash_entries.len(), 2);
    assert!(w.cash_entries[0].reversal.is_none());
    assert!(w.cash_entries[1].customer_confirmation.is_some());
    assert_eq!(account(&w, "cash"), 139999);
    assert_eq!(account(&w, "accounts_receivable"), 0);
    assert!(restarted
        .read("unrelated-refund-tenant")
        .await
        .expect("isolated")
        .cash_entries
        .is_empty());
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("file cleanup");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("row cleanup");
    pool.close().await;
}

#[tokio::test]
async fn late_entered_credit_and_refund_keep_actual_document_and_bank_dates_instead_of_inventing_today_clearing(
) {
    let s = setup().await;
    let bill = paid(&s, "ACTUAL-DATES").await;
    let mut w = s.read("demo").await.expect("read");
    let today = Utc::now().date_naive();
    w.bills[0].issued_at = Some(Utc::now() - chrono::Duration::days(4));
    w.cash_entries[0].cleared_on = today - chrono::Duration::days(3);
    let cr=w.apply_finance("credit.create",&bill.id,json!({"expected_version":bill.version,"number":"ACTUAL-CREDIT-DATE","amount_cents":9001,"issued_on":today-chrono::Duration::days(2),"reason":"Actual credit document issued before bank refund","confirmation":confirmation(None)}),"finance:late-entry").expect("actual credit date");
    assert_eq!(
        cr["created_at"].as_str().expect("recorded")[..10],
        today.to_string()
    );
    assert_eq!(
        w.journal.last().expect("credit journal").posted_on,
        today - chrono::Duration::days(2)
    );
    let mut refund = cash("customer_refund", &w.bills[0], 9001, "ACTUAL-PAST-CLEARING");
    refund["cleared_on"] = json!(today - chrono::Duration::days(1));
    w.apply_finance("cash.record", "", refund, "finance:late-entry")
        .expect("actual past clearing");
    assert_eq!(
        w.cash_entries.last().expect("refund").cleared_on,
        today - chrono::Duration::days(1)
    );
    assert_eq!(w.bill_totals(&w.bills[0]).expect("totals").balance_cents, 0);
}

#[tokio::test]
async fn full_customer_return_can_void_billing_and_a_refund_correction_preserves_void_status_and_real_receipt(
) {
    let s = setup().await;
    let bill = paid(&s, "FULL-RETURN-VOID").await;
    let original_receipt = json!(s.read("demo").await.expect("read").cash_entries[0]);
    let cn = credit(&s, &bill, 149000, None).await;
    let b = current(&s, &bill.id).await;
    let refund = run(
        &s,
        "cash.record",
        "",
        cash("customer_refund", &b, 149000, "FULL-ACTUAL-RETURN"),
    )
    .await;
    let b = current(&s, &bill.id).await;
    run(&s, "credit.reverse", cn["id"].as_str().expect("credit id"), json!({"expected_version":1,"expected_bill_version":b.version,"reversed_on":Utc::now().date_naive(),"note":"Replace full service credit with invoice void after actual funds return"})).await;
    let b = current(&s, &bill.id).await;
    run(&s, "bill.void", &b.id, json!({"expected_version":b.version,"note":"Invoice removed after agreed full return; original receipt retained"})).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(current(&s, &bill.id).await.status, "void");
    for name in ["accounts_receivable", "freight_revenue", "cash"] {
        assert_eq!(
            account(&w, name),
            0,
            "Full actual return and void must clear {name}"
        );
    }
    run(&s, "cash.reverse", refund["id"].as_str().expect("refund id"), json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Bank evidence identifies erroneous refund reconciliation; retain correction history"})).await;
    let b = current(&s, &bill.id).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(b.status, "void");
    assert_eq!(w.bill_totals(&b).expect("totals").refund_due_cents, 149000);
    assert_eq!(
        w.finance_summary().expect("summary")["customer_credit_cents"],
        149000
    );
    let mut corrected = cash("customer_refund", &b, 149000, "FULL-ACTUAL-RETURN");
    corrected["corrects_id"] = refund["id"].clone();
    run(&s, "cash.record", "", corrected).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(json!(w.cash_entries[0]), original_receipt);
    assert_eq!(current(&s, &bill.id).await.status, "void");
    assert_eq!(
        w.bill_totals(&current(&s, &bill.id).await)
            .expect("totals")
            .balance_cents,
        0
    );
    for name in ["accounts_receivable", "freight_revenue", "cash"] {
        assert_eq!(
            account(&w, name),
            0,
            "Corrected full return must clear {name}"
        );
    }
    let statement = finance::statement(&w, &bill.id).expect("statement").1;
    assert!(statement.contains("Invoice void/retraction"));
    assert!(statement.contains("Reconciled customer refunds"));
}
