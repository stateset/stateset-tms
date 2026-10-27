#[path = "support/canonical.rs"]
mod canonical;
#[path = "support/invoice_review.rs"]
mod invoice_review;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    documents::Upload,
    domain::{self, Workspace},
    finance,
    store::Store,
};

const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
async fn seeded() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn mutate(s: &Store, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        (
            "demo",
            if op == "charge.propose" {
                "user:operations"
            } else {
                "user:finance"
            },
        ),
        &domain::id("req"),
        op,
        id,
        body,
    )
    .await
    .unwrap_or_else(|e| panic!("{op}: {e}"))["data"]
        .clone()
}
async fn version(s: &Store, load: &str) -> u64 {
    s.read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == load)
        .expect("load")
        .version
}
fn proposal(side: &str, amount: i64, reference: &str) -> Value {
    json!({"side":side,"kind":"detention","description":"Agreed additional loading service","amount_cents":amount,"evidence_reference":reference})
}
async fn propose(s: &Store, load: &str, side: &str, amount: i64, reference: &str) -> Value {
    let mut body = proposal(side, amount, reference);
    body["expected_load_version"] = json!(version(s, load).await);
    mutate(s, "charge.propose", load, body).await
}
fn decision(load_version: u64, charge: &Value, approved: bool) -> Value {
    json!({"expected_load_version":load_version,"expected_version":charge["version"],"decision":if approved {"approved"} else {"rejected"},"counterparty_contact":"Verified counterparty representative","confirmation_reference":"External agreement CASE-15","note":"Reviewed original evidence and verified the exact additional amount"})
}
async fn review(s: &Store, charge: &Value, approved: bool) -> Value {
    let load = charge["load_id"].as_str().expect("load");
    mutate(
        s,
        "charge.review",
        charge["id"].as_str().expect("charge"),
        decision(version(s, load).await, charge, approved),
    )
    .await
}
async fn void(s: &Store, charge: &Value) -> Value {
    mutate(s,"charge.void",charge["id"].as_str().expect("id"),json!({"expected_load_version":version(s,charge["load_id"].as_str().expect("load")).await,"expected_version":charge["version"],"counterparty_contact":"Counterparty accounting","confirmation_reference":"Retraction CASE-16","note":"The named counterparty confirmed this charge approval was erroneous"})).await
}
async fn fails(s: &Store, key: &str, op: &str, id: &str, body: Value, contains: &str) {
    let before = json!(s.read("demo").await.expect("before"));
    let error = s
        .mutate("demo", key, op, id, body)
        .await
        .expect_err("reject");
    assert!(error.contains(contains), "{error}");
    assert_eq!(json!(s.read("demo").await.expect("after")), before);
}
fn net(w: &Workspace, account: &str) -> i64 {
    w.journal
        .iter()
        .flat_map(|v| &v.lines)
        .filter(|v| v.account == account)
        .map(|v| v.debit_cents - v.credit_cents)
        .sum()
}
fn cash(direction: &str, target: &Value, amount: i64, reference: &str) -> Value {
    json!({"direction":direction,"account":"Operating-USD","external_reference":reference,"amount_cents":amount,"currency":"USD","cleared_on":Utc::now().date_naive(),"note":"Matched actual cleared bank statement","allocations":[{"target_id":target["id"],"expected_version":target["version"],"amount_cents":amount}]})
}

#[tokio::test]
async fn documented_additions_reach_invoice_settlement_customer_lines_and_balanced_cash_without_changing_base_rates(
) {
    let s = seeded().await;
    let buy = propose(&s, "load_6", "buy", 13751, "CARRIER-DETENTION-15").await;
    let mut sell_input = proposal("sell", 19001, "CUSTOMER-DETENTION-15");
    sell_input["expected_load_version"] = json!(version(&s, "load_6").await);
    sell_input["description"] = json!("Additional loading <script>alert('unsafe')</script>");
    let sell = mutate(&s, "charge.propose", "load_6", sell_input).await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.commercial_summary(&w.loads[5]).expect("summary")["margin_cents"],
        31000
    );
    assert!(w.journal.is_empty());
    let buy = review(&s, &buy, true).await;
    let sell = review(&s, &sell, true).await;
    assert_eq!(buy["decisions"][0]["recorded_by"], "user:finance");
    assert_eq!(sell["created_by"], "user:operations");
    let w = s.read("demo").await.expect("read");
    let invoice = w
        .invoices
        .iter()
        .find(|v| v.load_id == "load_6")
        .expect("invoice");
    assert_eq!(
        invoice.amount_cents, 118000,
        "the received invoice has not been rewritten to make it match"
    );
    assert_eq!(invoice.variance_cents, -13751);
    let corrected=mutate(&s,"invoice.resolve",&invoice.id,json!({"expected_amount_cents":118000,"amount_cents":131751,"note":"Carrier supplied an actual revised invoice including the agreed additional service"})).await;
    assert_eq!(corrected["status"], "matched");
    let approved = mutate(&s, "invoice.approve", &invoice.id, json!({})).await;
    assert_eq!(approved["agreement"]["base_amount_cents"], 118000);
    assert_eq!(approved["agreement"]["total_amount_cents"], 131751);
    let settlement = mutate(&s, "settlement.create", "load_6", json!({})).await;
    assert_eq!(settlement["amount_cents"], 131751);
    let bill=mutate(&s,"bill.create","",json!({"load_id":"load_6","number":"CHARGE-CUSTOMER-15","due_date":Utc::now().date_naive()})).await;
    assert_eq!(bill["snapshot"]["amount_cents"], 168001);
    let issued = mutate(
        &s,
        "bill.issue",
        bill["id"].as_str().expect("bill"),
        json!({"expected_version":1}),
    )
    .await;
    assert_eq!(
        issued["snapshot"]["agreement"]["charges"][0]["amount_cents"],
        19001
    );
    mutate(
        &s,
        "cash.record",
        "",
        cash("receipt", &issued, 168001, "Receipt-CHARGES"),
    )
    .await;
    mutate(
        &s,
        "cash.record",
        "",
        cash("payout", &settlement, 131751, "Payout-CHARGES"),
    )
    .await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.loads[5].buy_cents, 118000);
    assert_eq!(w.loads[5].sell_cents, 149000);
    assert_eq!(
        w.commercial_summary(&w.loads[5]).expect("summary")["margin_cents"],
        36250
    );
    assert_eq!(net(&w, "accounts_receivable"), 0);
    assert_eq!(net(&w, "carrier_payable"), 0);
    assert_eq!(net(&w, "cash"), 36250);
    for entry in &w.journal {
        assert_eq!(
            entry.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            entry.lines.iter().map(|v| v.credit_cents).sum::<i64>()
        );
    }
    let (_, html) = finance::statement(&w, bill["id"].as_str().expect("id")).expect("statement");
    assert!(
        html.contains("USD 1490.00") && html.contains("USD 190.01") && html.contains("USD 1680.01")
    );
    assert!(
        html.contains("&lt;script&gt;alert(&#39;unsafe&#39;)&lt;/script&gt;")
            && !html.contains("<script>")
    );
    assert!(
        !html.contains("137.51")
            && !html.contains("CARRIER-DETENTION-15")
            && !html.contains("CASE-15")
            && !html.contains("user:finance")
    );
}
#[tokio::test]
async fn pending_reviews_hold_only_the_relevant_posting_and_approval_refreshes_a_draft_version() {
    let s = seeded().await;
    let bill = mutate(
        &s,
        "bill.create",
        "",
        json!({"load_id":"load_6","number":"DRAFT-CHARGE","due_date":Utc::now().date_naive()}),
    )
    .await;
    let buy = propose(&s, "load_6", "buy", 10001, "BUY-1").await;
    let sell = propose(&s, "load_6", "sell", 12001, "SELL-1").await;
    let w = s.read("demo").await.expect("read");
    let invoice = w
        .invoices
        .iter()
        .find(|v| v.load_id == "load_6")
        .expect("invoice");
    fails(
        &s,
        "pending-buy",
        "invoice.approve",
        &invoice.id,
        json!({}),
        "pending carrier charges",
    )
    .await;
    fails(
        &s,
        "pending-sell",
        "bill.issue",
        bill["id"].as_str().expect("bill"),
        json!({"expected_version":1}),
        "pending customer charges",
    )
    .await;
    review(&s, &buy, false).await;
    mutate(&s, "invoice.approve", &invoice.id, json!({})).await;
    let approved_sell = review(&s, &sell, true).await;
    let w = s.read("demo").await.expect("read");
    let draft = w
        .bills
        .iter()
        .find(|v| Some(v.id.as_str()) == bill["id"].as_str())
        .expect("draft");
    assert_eq!(draft.version, 2);
    assert_eq!(draft.snapshot.amount_cents, 161001);
    fails(
        &s,
        "stale-issue",
        "bill.issue",
        &draft.id,
        json!({"expected_version":1}),
        "changed;",
    )
    .await;
    let old = json!(draft);
    void(&s, &approved_sell).await;
    let w = s.read("demo").await.expect("read");
    let draft = w
        .bills
        .iter()
        .find(|v| Some(v.id.as_str()) == bill["id"].as_str())
        .expect("draft");
    assert_eq!(draft.version, 3);
    assert_eq!(draft.snapshot.amount_cents, 149000);
    assert_eq!(old["snapshot"]["amount_cents"], 161001);
    mutate(&s, "bill.issue", &draft.id, json!({"expected_version":3})).await;
}
#[tokio::test]
async fn posted_buy_and_sell_agreements_cannot_be_silently_changed_or_cross_applied() {
    let s = seeded().await;
    let buy = review(
        &s,
        &propose(&s, "load_6", "buy", 1, "BUY-PENNY").await,
        true,
    )
    .await;
    let sell = review(
        &s,
        &propose(&s, "load_6", "sell", 2, "SELL-PENNIES").await,
        true,
    )
    .await;
    let w = s.read("demo").await.expect("read");
    let i = w
        .invoices
        .iter()
        .find(|v| v.load_id == "load_6")
        .expect("invoice");
    mutate(&s,"invoice.resolve",&i.id,json!({"expected_amount_cents":118000,"amount_cents":118001,"note":"Actual revised invoice received"})).await;
    mutate(&s, "invoice.approve", &i.id, json!({})).await;
    let mut input = proposal("buy", 5, "LATE-BUY");
    input["expected_load_version"] = json!(version(&s, "load_6").await);
    fails(
        &s,
        "posted-buy",
        "charge.propose",
        "load_6",
        input,
        "posted billing",
    )
    .await;
    let void_input = |charge: &Value, load_version: u64| json!({"expected_load_version":load_version,"expected_version":charge["version"],"counterparty_contact":"Counterparty","confirmation_reference":"Void-case","note":"Cannot change a posted basis"});
    fails(
        &s,
        "void-posted-buy",
        "charge.void",
        buy["id"].as_str().expect("id"),
        void_input(&buy, version(&s, "load_6").await),
        "Posted billing",
    )
    .await;
    let bill = mutate(
        &s,
        "bill.create",
        "",
        json!({"load_id":"load_6","number":"FROZEN-CHARGES","due_date":Utc::now().date_naive()}),
    )
    .await;
    let issued = mutate(
        &s,
        "bill.issue",
        bill["id"].as_str().expect("bill"),
        json!({"expected_version":1}),
    )
    .await;
    fails(
        &s,
        "void-posted-sell",
        "charge.void",
        sell["id"].as_str().expect("id"),
        void_input(&sell, version(&s, "load_6").await),
        "Posted billing",
    )
    .await;
    let w = s.read("demo").await.expect("read");
    assert_eq!(json!(w.bills[0]), issued);
    assert_eq!(w.bills[0].snapshot.amount_cents, 149002);
}
#[tokio::test]
async fn rejected_and_voided_proposals_keep_a_single_explicit_correction_chain_and_strict_idempotency(
) {
    let s = seeded().await;
    let mut input = proposal("buy", 2501, "REFERENCE-15");
    input["expected_load_version"] = json!(1);
    let first = s
        .mutate_authorized(
            ("demo", "user:ops"),
            "same-charge",
            "charge.propose",
            "load_6",
            input.clone(),
        )
        .await
        .expect("proposal");
    let replay = s
        .mutate_authorized(
            ("demo", "user:other-ops"),
            "same-charge",
            "charge.propose",
            "load_6",
            input.clone(),
        )
        .await
        .expect("replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["data"], replay["data"]);
    input["amount_cents"] = json!(2502);
    fails(
        &s,
        "same-charge",
        "charge.propose",
        "load_6",
        input,
        "Idempotency key already",
    )
    .await;
    let old = review(&s, &first["data"], false).await;
    let mut replacement = proposal("buy", 2502, "reference-15");
    replacement["expected_load_version"] = json!(version(&s, "load_6").await);
    fails(
        &s,
        "implicit-correction",
        "charge.propose",
        "load_6",
        replacement.clone(),
        "explicit correction",
    )
    .await;
    replacement["corrects_id"] = old["id"].clone();
    let second = mutate(&s, "charge.propose", "load_6", replacement).await;
    assert_eq!(second["corrects_id"], old["id"]);
    let second = review(&s, &second, true).await;
    let second = void(&s, &second).await;
    let mut fork = proposal("buy", 9999, "REFERENCE-15");
    fork["expected_load_version"] = json!(version(&s, "load_6").await);
    fork["corrects_id"] = old["id"].clone();
    fails(
        &s,
        "fork-correction",
        "charge.propose",
        "load_6",
        fork,
        "one uncorrected",
    )
    .await;
    assert_eq!(second["decisions"].as_array().expect("decisions").len(), 2);
    assert_eq!(second["amount_cents"], 2502);
    let mut spoof = proposal("sell", 1, "SPOOF");
    spoof["expected_load_version"] = json!(version(&s, "load_6").await);
    spoof["created_by"] = json!("admin");
    fails(
        &s,
        "spoof",
        "charge.propose",
        "load_6",
        spoof,
        "Invalid charge request fields",
    )
    .await;
    let mut zero = proposal("sell", 0, "ZERO");
    zero["expected_load_version"] = json!(version(&s, "load_6").await);
    fails(
        &s,
        "zero",
        "charge.propose",
        "load_6",
        zero,
        "must be positive",
    )
    .await;
}
#[tokio::test]
async fn carrier_release_cannot_drop_approved_costs_and_retired_proposals_cannot_follow_a_new_tender(
) {
    let s = seeded().await;
    let pending = propose(&s, "load_4", "buy", 5000, "OLD-CARRIER-PROPOSAL").await;
    let approved = review(
        &s,
        &propose(&s, "load_4", "buy", 1000, "COMMITTED-COST").await,
        true,
    )
    .await;
    let request = |version: u64| json!({"expected_load_version":version,"disposition":"replan","reason":"Carrier coordinate release"});
    fails(
        &s,
        "release-cost",
        "tender.release.request",
        "load_4",
        request(version(&s, "load_4").await),
        "approved charges",
    )
    .await;
    void(&s, &approved).await;
    let release = mutate(
        &s,
        "tender.release.request",
        "load_4",
        request(version(&s, "load_4").await),
    )
    .await;
    mutate(&s,"tender.release.confirm",release["id"].as_str().expect("release"),json!({"expected_load_version":version(&s,"load_4").await,"carrier_contact":"Old carrier dispatcher","confirmation_reference":"Released-empty","note":"Confirmed no custody and assignment release"})).await;
    mutate(
        &s,
        "load.command",
        "load_4",
        json!({"action":"tender","carrier_id":"ca4","expected_version":version(&s,"load_4").await}),
    )
    .await;
    mutate(
        &s,
        "load.command",
        "load_4",
        json!({"action":"accept","expected_version":version(&s,"load_4").await}),
    )
    .await;
    fails(
        &s,
        "old-proposal-approve",
        "charge.review",
        pending["id"].as_str().expect("id"),
        decision(version(&s, "load_4").await, &pending, true),
        "previous carrier assignment",
    )
    .await;
    review(&s, &pending, false).await;
    let w = s.read("demo").await.expect("read");
    let load = &w.loads[3];
    assert_eq!(
        w.charge_basis(load, "buy")
            .expect("basis")
            .total_amount_cents,
        load.buy_cents
    );
}
#[tokio::test]
async fn attached_charge_evidence_is_tenant_and_load_bound_and_approval_locks_its_review() {
    let s = seeded().await;
    let doc = s
        .upload_document(
            ("demo", "user:ops"),
            "support",
            "load_6",
            Upload {
                kind: "rate_confirmation".into(),
                filename: "agreement.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("upload")["data"]
        .clone();
    let mut input = proposal("buy", 3001, "DOCUMENT-PROOF");
    input["expected_load_version"] = json!(1);
    input["document_id"] = doc["id"].clone();
    let charge = mutate(&s, "charge.propose", "load_6", input).await;
    assert_eq!(charge["document_sha256"], doc["sha256"]);
    let charge = review(&s, &charge, true).await;
    fails(&s,"reject-charge-file","document.review",doc["id"].as_str().expect("doc"),json!({"expected_version":1,"decision":"rejected","note":"Cannot discard approved cost evidence"}),"approved charge").await;
    let mut other = proposal("sell", 3001, "WRONG-LOAD-DOCUMENT");
    other["expected_load_version"] = json!(1);
    other["document_id"] = doc["id"].clone();
    fails(
        &s,
        "wrong-load-proof",
        "charge.propose",
        "load_7",
        other,
        "Document not found",
    )
    .await;
    void(&s, &charge).await;
    mutate(&s,"document.review",doc["id"].as_str().expect("doc"),json!({"expected_version":1,"decision":"rejected","note":"Finance established this agreement file was incorrect"})).await;
    let w = s.read("other-tenant").await.expect("isolated");
    assert!(w.charges.is_empty() && w.documents.is_empty());
}
#[tokio::test]
async fn an_unapproved_request_cannot_overflow_the_agreement_or_bypass_stale_load_and_charge_guards(
) {
    let s = seeded().await;
    let charge = propose(&s, "load_6", "sell", 100000000, "EXCESSIVE").await;
    fails(
        &s,
        "too-large",
        "charge.review",
        charge["id"].as_str().expect("id"),
        decision(version(&s, "load_6").await, &charge, true),
        "Amount must be between",
    )
    .await;
    let mut stale = decision(1, &charge, true);
    fails(
        &s,
        "stale-load",
        "charge.review",
        charge["id"].as_str().expect("id"),
        stale.clone(),
        "Load changed;",
    )
    .await;
    stale["expected_load_version"] = json!(version(&s, "load_6").await);
    stale["expected_version"] = json!(9);
    fails(
        &s,
        "stale-charge",
        "charge.review",
        charge["id"].as_str().expect("id"),
        stale,
        "Charge changed;",
    )
    .await;
    review(&s, &charge, false).await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_charge_decisions_and_supporting_bytes_commit_atomically_under_corruption_and_concurrency(
) {
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("one");
    let b = Store::new(Some(&url)).await.expect("two");
    let tenant = domain::id("charge-test");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    canonical::put_workspace(&pool, &tenant, &json!(domain::demo()))
        .await
        .expect("fixture");
    let doc = a
        .upload_document(
            (&tenant, "ops"),
            "proof",
            "load_6",
            Upload {
                kind: "rate_confirmation".into(),
                filename: "proof.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: PDF.to_vec(),
            },
        )
        .await
        .expect("doc")["data"]
        .clone();
    let mut input = proposal("buy", 1251, "PG-CHARGE");
    input["expected_load_version"] = json!(1);
    input["document_id"] = doc["id"].clone();
    let charge = a
        .mutate_authorized(
            (&tenant, "ops"),
            "proposal",
            "charge.propose",
            "load_6",
            input,
        )
        .await
        .expect("propose")["data"]
        .clone();
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupted".as_slice())
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("id"))
    .execute(&pool)
    .await
    .expect("corrupt");
    let before = json!(a.read(&tenant).await.expect("before"));
    let review_body = decision(2, &charge, true);
    assert_eq!(
        a.mutate_authorized(
            (&tenant, "finance:a"),
            "approval",
            "charge.review",
            charge["id"].as_str().expect("id"),
            review_body.clone()
        )
        .await
        .expect_err("corrupt evidence"),
        "Internal storage error"
    );
    assert_eq!(json!(b.read(&tenant).await.expect("unchanged")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("id"))
    .execute(&pool)
    .await
    .expect("restore");
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:a"),
            "approval",
            "charge.review",
            charge["id"].as_str().expect("id"),
            review_body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "finance:b"),
            "second-approval",
            "charge.review",
            charge["id"].as_str().expect("id"),
            review_body
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let w = a.read(&tenant).await.expect("read");
    assert_eq!(w.charges[0].status, "approved");
    assert_eq!(w.charges[0].decisions.len(), 1);
    assert_eq!(w.loads[5].version, 3);
    let invoice = w
        .invoices
        .iter()
        .find(|v| v.load_id == "load_6")
        .expect("invoice");
    a.mutate(&tenant,"actual-revised-invoice","invoice.resolve",&invoice.id,json!({"expected_amount_cents":118000,"amount_cents":119251,"note":"Carrier supplied an actual revised document"})).await.expect("correction");
    let invoice_approval = invoice_review::body(&a, &tenant, &invoice.id).await;
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupted-again".as_slice())
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("id"))
    .execute(&pool)
    .await
    .expect("corrupt");
    let before = json!(b.read(&tenant).await.expect("before approval"));
    assert!(b
        .mutate_authorized(
            (&tenant, invoice_review::CHECKER),
            "invoice-approval",
            "invoice.approve",
            &invoice.id,
            invoice_approval.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("id"))
    .execute(&pool)
    .await
    .expect("restore");
    b.mutate_authorized(
        (&tenant, invoice_review::CHECKER),
        "invoice-approval",
        "invoice.approve",
        &invoice.id,
        invoice_approval,
    )
    .await
    .expect("retry exact failed request");
    let mut sell = proposal("sell", 1251, "PG-SELL");
    sell["expected_load_version"] = json!(3);
    sell["document_id"] = doc["id"].clone();
    let sell = a
        .mutate_authorized(
            (&tenant, "ops"),
            "sell-proposal",
            "charge.propose",
            "load_6",
            sell,
        )
        .await
        .expect("sell proposal")["data"]
        .clone();
    a.mutate_authorized(
        (&tenant, "finance:sell"),
        "sell-approval",
        "charge.review",
        sell["id"].as_str().expect("id"),
        decision(4, &sell, true),
    )
    .await
    .expect("sell approval");
    let bill = a
        .mutate(
            &tenant,
            "customer-draft",
            "bill.create",
            "",
            json!({"load_id":"load_6","number":"PG-ADDITIONS","due_date":Utc::now().date_naive()}),
        )
        .await
        .expect("draft")["data"]
        .clone();
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt-at-final-posting".as_slice())
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("doc"))
    .execute(&pool)
    .await
    .expect("corrupt");
    let before = json!(a.read(&tenant).await.expect("before posting"));
    assert!(a
        .mutate(
            &tenant,
            "customer-issue",
            "bill.issue",
            bill["id"].as_str().expect("bill"),
            json!({"expected_version":1})
        )
        .await
        .is_err());
    assert!(b
        .mutate(
            &tenant,
            "settlement",
            "settlement.create",
            "load_6",
            json!({})
        )
        .await
        .is_err());
    assert_eq!(
        json!(b.read(&tenant).await.expect("unchanged postings")),
        before
    );
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(doc["id"].as_str().expect("doc"))
    .execute(&pool)
    .await
    .expect("restore");
    a.mutate(
        &tenant,
        "customer-issue",
        "bill.issue",
        bill["id"].as_str().expect("bill"),
        json!({"expected_version":1}),
    )
    .await
    .expect("restored proof permits exact failed issue request");
    b.mutate(
        &tenant,
        "settlement",
        "settlement.create",
        "load_6",
        json!({}),
    )
    .await
    .expect("restored proof permits exact failed settlement request");
    drop(a);
    drop(b);
    let reopened = Store::new(Some(&url)).await.expect("reopen");
    let w = reopened.read(&tenant).await.expect("read");
    assert_eq!(w.charges.len(), 2);
    assert_eq!(net(&w, "carrier_payable"), -119251);
    assert_eq!(net(&w, "accounts_receivable"), 150251);
    assert_eq!(w.settlements[0].amount_cents, 119251);
    assert_eq!(
        w.charges[0].document_sha256,
        doc["sha256"].as_str().map(str::to_owned)
    );
    assert!(reopened
        .read("foreign-charge-tenant")
        .await
        .expect("tenant isolated")
        .charges
        .is_empty());
}
