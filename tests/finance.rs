#[path = "support/canonical.rs"]
mod canonical;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    domain::{self, Workspace},
    finance::{self, CustomerBill},
    store::Store,
};

async fn seeded() -> Store {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    store
}
async fn mutate(store: &Store, operation: &str, id: &str, body: Value) -> Result<Value, String> {
    store
        .mutate_authorized(
            ("demo", "finance:test"),
            &domain::id("request"),
            operation,
            id,
            body,
        )
        .await
        .map(|v| v["data"].clone())
}
async fn bill(store: &Store, load_id: &str, number: &str) -> CustomerBill {
    let workspace = store.read("demo").await.expect("read");
    let load = workspace
        .loads
        .iter()
        .find(|v| v.id == load_id)
        .expect("load");
    if load.status == domain::Status::InTransit {
        mutate(store,"load.command",load_id,json!({"action":"deliver","pod_reference":"External signed evidence","expected_version":load.version})).await.expect("delivery");
        let current = store
            .read("demo")
            .await
            .expect("read")
            .loads
            .into_iter()
            .find(|v| v.id == load_id)
            .expect("load");
        mutate(store,"pod.review",load_id,json!({"expected_version":current.version,"decision":"accepted","note":"Verified external receiver evidence"})).await.expect("review");
    }
    let created = mutate(
        store,
        "bill.create",
        "",
        json!({"load_id":load_id,"number":number,"due_date":Utc::now().date_naive()}),
    )
    .await
    .expect("draft");
    serde_json::from_value(
        mutate(
            store,
            "bill.issue",
            created["id"].as_str().expect("id"),
            json!({"expected_version":1}),
        )
        .await
        .expect("issue"),
    )
    .expect("bill")
}
fn cash_input(direction: &str, target: &str, version: u64, amount: i64, reference: &str) -> Value {
    json!({"direction":direction,"currency":"USD","account":"Operating-USD","external_reference":reference,"amount_cents":amount,"cleared_on":Utc::now().date_naive(),"note":"Matched cleared external bank statement","allocations":[{"target_id":target,"expected_version":version,"amount_cents":amount}]})
}
async fn supplement_input(store: &Store, parent: &CustomerBill, number: &str) -> Value {
    let w = store.read("demo").await.expect("workspace");
    let current = w.bills.iter().find(|v| v.id == parent.id).expect("parent");
    let load = w
        .loads
        .iter()
        .find(|v| v.id == parent.load_id)
        .expect("load");
    json!({"expected_version":current.version,"expected_load_version":load.version,
        "number":number,"due_date":Utc::now().date_naive(),"amount_cents":12501,
        "description":"Additional unloading agreed after original billing",
        "counterparty_contact":"Customer AP representative","confirmation_reference":"customer-confirmed-15",
        "evidence_reference":"unloading-agreement-15"})
}

#[tokio::test]
async fn supplemental_billing_keeps_paid_original_and_reconciles_exact_credit_receipt_and_void_history(
) {
    let store = seeded().await;
    let parent = bill(&store, "load_6", "PRIMARY").await;
    mutate(
        &store,
        "cash.record",
        "",
        cash_input(
            "receipt",
            &parent.id,
            parent.version,
            149000,
            "primary-paid",
        ),
    )
    .await
    .expect("paid");
    let input = supplement_input(&store, &parent, "SUP-1").await;
    let draft = mutate(&store, "bill.supplement", &parent.id, input)
        .await
        .expect("supplement");
    let id = draft["id"].as_str().expect("id");
    assert_eq!(draft["snapshot"]["amount_cents"], 12501);
    assert_eq!(
        store.read("demo").await.expect("read").journal.len(),
        2,
        "draft posts no revenue"
    );
    let issued = mutate(&store, "bill.issue", id, json!({"expected_version":1}))
        .await
        .expect("issue");
    assert_eq!(
        issued["snapshot"]["amount_cents"], 12501,
        "do not re-bill primary's 149000 cents"
    );
    let credit = mutate(&store,"credit.create",id,json!({"expected_version":2,"number":"SUP-CREDIT","amount_cents":2501,"reason":"Customer agreed service correction"})).await.expect("credit");
    let receipt = mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", id, 3, 10000, "supplement-paid"),
    )
    .await
    .expect("receipt");
    let w = store.read("demo").await.expect("read");
    assert_eq!(account(&w, "accounts_receivable"), 0);
    assert_eq!(account(&w, "cash"), 159000);
    assert_eq!(account(&w, "freight_revenue"), -159000);
    assert_eq!(w.bills[0].status, "paid");
    assert_eq!(json!(w.bills[0].snapshot), json!(parent.snapshot));
    let commercial = w.commercial_summary(&w.loads[5]).expect("commercial");
    assert_eq!(commercial["total_sell_cents"], 159000);
    assert_eq!(commercial["customer_supplement_cents"], 12501);
    assert_eq!(commercial["customer_credit_cents"], 2501);
    let (_, output) = finance::statement(&w, id).expect("statement");
    assert!(output.contains("SUPPLEMENTAL CUSTOMER INVOICE"));
    assert!(output.contains("Supplement to invoice PRIMARY"));
    assert!(output.contains("Additional unloading"));
    assert!(output.contains("USD 125.01"));
    assert!(!output.contains("1490.00"));
    assert!(!output.contains("1180.00"));
    assert!(
        !output.contains("unloading-agreement-15"),
        "internal agreement reference stays off customer statement"
    );
    balanced(&w);
    mutate(&store,"cash.reverse",receipt["id"].as_str().expect("receipt"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Bank allocation correction"})).await.expect("reverse receipt");
    mutate(&store,"credit.reverse",credit["id"].as_str().expect("credit"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Retract mistaken credit"})).await.expect("reverse credit");
    let before = store.read("demo").await.expect("read");
    let current = before
        .bills
        .iter()
        .find(|v| v.id == id)
        .expect("supplement");
    mutate(
        &store,
        "bill.void",
        id,
        json!({"expected_version":current.version,"note":"Customer supplement withdrawn"}),
    )
    .await
    .expect("void");
    let w = store.read("demo").await.expect("read");
    assert_eq!(w.bills[0].status, "paid");
    assert_eq!(account(&w, "accounts_receivable"), 0);
    assert_eq!(account(&w, "freight_revenue"), -149000);
    assert_eq!(account(&w, "cash"), 149000);
    assert_eq!(
        w.customer_supplement_total("load_6").expect("supplements"),
        0
    );
    balanced(&w);
}

#[tokio::test]
async fn supplement_guards_reject_stale_duplicate_forged_and_orphaned_requests_atomically() {
    let store = seeded().await;
    let parent = bill(&store, "load_6", "GUARD-PRIMARY").await;
    let input = supplement_input(&store, &parent, "SUP-GUARD").await;
    for (field, value) in [
        ("amount_cents", json!(0)),
        ("amount_cents", json!(-1)),
        ("amount_cents", json!(100000001)),
        ("expected_version", json!(0)),
        ("expected_load_version", json!(0)),
        ("counterparty_contact", json!(" ")),
        ("evidence_reference", json!("")),
        ("due_date", json!("1900-01-01")),
        ("customer_id", json!("forged")),
        ("status", json!("issued")),
        ("document_id", json!("missing")),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        let before = json!(store.read("demo").await.expect("read"));
        assert!(
            mutate(&store, "bill.supplement", &parent.id, bad)
                .await
                .is_err(),
            "{field}"
        );
        assert_eq!(json!(store.read("demo").await.expect("read")), before);
    }
    let draft = mutate(&store, "bill.supplement", &parent.id, input.clone())
        .await
        .expect("draft");
    assert!(mutate(&store, "bill.supplement", &parent.id, input)
        .await
        .is_err());
    let mut duplicate = supplement_input(&store, &parent, "OTHER-NUMBER").await;
    duplicate["evidence_reference"] = json!(" UNLOADING-AGREEMENT-15 ");
    assert!(mutate(&store, "bill.supplement", &parent.id, duplicate)
        .await
        .is_err());
    let w = store.read("demo").await.expect("read");
    assert!(mutate(
        &store,
        "bill.void",
        &parent.id,
        json!({"expected_version":w.bills[0].version,"note":"Would orphan a supplement"})
    )
    .await
    .is_err());
    assert!(mutate(
        &store,
        "bill.supplement",
        draft["id"].as_str().expect("id"),
        supplement_input(&store, &parent, "NESTED").await
    )
    .await
    .is_err());
    mutate(
        &store,
        "bill.void",
        draft["id"].as_str().expect("id"),
        json!({"expected_version":1,"note":"Incorrect supplemental amount"}),
    )
    .await
    .expect("void");
    let mut correction = supplement_input(&store, &parent, "CORRECTED").await;
    assert!(
        mutate(&store, "bill.supplement", &parent.id, correction.clone())
            .await
            .is_err()
    );
    correction["corrects_id"] = draft["id"].clone();
    correction["amount_cents"] = json!(10001);
    let new = mutate(&store, "bill.supplement", &parent.id, correction)
        .await
        .expect("correction");
    let w = store.read("demo").await.expect("read");
    assert_eq!(
        w.bills[1]
            .supplement
            .as_ref()
            .expect("agreement")
            .corrected_by_id
            .as_deref(),
        new["id"].as_str()
    );
    let mut branching = supplement_input(&store, &parent, "BRANCH").await;
    branching["corrects_id"] = draft["id"].clone();
    assert!(mutate(&store, "bill.supplement", &parent.id, branching)
        .await
        .is_err());
    assert_eq!(store.read("demo").await.expect("read").journal.len(), 1);
}

#[tokio::test]
async fn supplemental_snapshot_keeps_original_customer_and_shipment_and_escapes_customer_description(
) {
    let store = seeded().await;
    let parent = bill(&store, "load_6", "SNAPSHOT-PRIMARY").await;
    let mut input = supplement_input(&store, &parent, "SNAPSHOT-SUP").await;
    input["description"] = json!("Unloading <script>alert('x')</script>");
    input["confirmation_reference"] = json!("approved <unsafe>");
    let draft = mutate(&store, "bill.supplement", &parent.id, input)
        .await
        .expect("draft");
    let id = draft["id"].as_str().expect("id");
    let w = store.read("demo").await.expect("read");
    let customer = w
        .customers
        .iter()
        .find(|v| v.id == parent.customer_id)
        .expect("customer");
    let mut changed = json!(customer);
    changed["name"] = json!("New master-data identity");
    mutate(
        &store,
        "customer.update",
        &parent.customer_id,
        json!({"data":changed,"expected":customer}),
    )
    .await
    .expect("master change");
    let issued = mutate(&store, "bill.issue", id, json!({"expected_version":1}))
        .await
        .expect("issue");
    assert_eq!(
        issued["snapshot"]["customer"],
        json!(parent.snapshot.customer)
    );
    assert_eq!(issued["snapshot"]["origin"], json!(parent.snapshot.origin));
    assert_eq!(issued["snapshot"]["amount_cents"], 12501);
    let w = store.read("demo").await.expect("read");
    let (_, output) = finance::statement(&w, id).expect("statement");
    assert!(output.contains("&lt;script&gt;"));
    assert!(output.contains("approved &lt;unsafe&gt;"));
    assert!(!output.contains("<script>"));
    assert!(!output.contains("New master-data identity"));
    let mut old = json!(w.bills[0]);
    old.as_object_mut().expect("bill").remove("supplement");
    assert!(serde_json::from_value::<CustomerBill>(old)
        .expect("legacy primary")
        .supplement
        .is_none());
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_supplement_integrity_concurrent_issuance_and_receipts_survive_reconnect() {
    use stateset_freight::documents::Upload;
    let fixture = seeded().await;
    let parent = bill(&fixture, "load_6", "PG-SUP-PRIMARY").await;
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("store a");
    let b = Store::new(Some(&url)).await.expect("store b");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("supplement-test");
    canonical::put_workspace(
        &pool,
        &tenant,
        &json!(fixture.read("demo").await.expect("fixture")),
    )
    .await
    .expect("seed");
    let bytes = b"%PDF-1.4\ncustomer agreement evidence\n%%EOF\n";
    let document = a
        .upload_document(
            (&tenant, "finance:test"),
            "evidence",
            "load_6",
            Upload {
                kind: "other".into(),
                filename: "customer-agreement.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: bytes.to_vec(),
            },
        )
        .await
        .expect("upload")["data"]
        .clone();
    let doc_id = document["id"].as_str().expect("document");
    let mut input = supplement_input(&fixture, &parent, "PG-SUP").await;
    input["document_id"] = json!(doc_id);
    let draft = a
        .mutate(&tenant, "draft", "bill.supplement", &parent.id, input)
        .await
        .expect("draft")["data"]
        .clone();
    let id = draft["id"].as_str().expect("id");
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt".as_slice())
    .bind(&tenant)
    .bind(doc_id)
    .execute(&pool)
    .await
    .expect("corrupt");
    let before = json!(a.read(&tenant).await.expect("before"));
    let issue = json!({"expected_version":1});
    assert!(a
        .mutate(&tenant, "issue-a", "bill.issue", id, issue.clone())
        .await
        .is_err());
    assert_eq!(
        json!(b.read(&tenant).await.expect("unchanged")),
        before,
        "no journal, audit, receipt or bill change on corrupted evidence"
    );
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(bytes.as_slice())
    .bind(&tenant)
    .bind(doc_id)
    .execute(&pool)
    .await
    .expect("restore");
    let (x, y) = tokio::join!(
        a.mutate(&tenant, "issue-a", "bill.issue", id, issue.clone()),
        b.mutate(&tenant, "issue-b", "bill.issue", id, issue.clone())
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let winning_key = if x.is_ok() { "issue-a" } else { "issue-b" };
    assert_eq!(
        a.mutate(&tenant, winning_key, "bill.issue", id, issue)
            .await
            .expect("replay")["replayed"],
        true
    );
    assert!(a.mutate(&tenant,"reject-proof","document.review",doc_id,json!({"expected_version":1,"decision":"rejected","note":"Must not invalidate posted customer agreement"})).await.is_err());
    let receipt = cash_input("receipt", id, 2, 12501, "pg-supplement-deposit");
    let mut competing = receipt.clone();
    competing["external_reference"] = json!("other-deposit");
    let (x, y) = tokio::join!(
        a.mutate(&tenant, "receipt-a", "cash.record", "", receipt),
        b.mutate(&tenant, "receipt-b", "cash.record", "", competing)
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let reconnected = Store::new(Some(&url)).await.expect("reconnect");
    let w = reconnected.read(&tenant).await.expect("read");
    assert_eq!(w.bills.len(), 2);
    assert_eq!(w.bills[1].status, "paid");
    assert_eq!(w.bills[0].snapshot.amount_cents, 149000);
    assert_eq!(w.journal.len(), 3);
    assert_eq!(account(&w, "accounts_receivable"), 149000);
    assert_eq!(account(&w, "cash"), 12501);
    assert_eq!(account(&w, "freight_revenue"), -161501);
    assert!(reconnected
        .read("other-supplement-tenant")
        .await
        .expect("isolation")
        .bills
        .is_empty());
    balanced(&w);
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup files");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
}
fn account(workspace: &Workspace, name: &str) -> i64 {
    workspace
        .journal
        .iter()
        .flat_map(|v| v.lines.iter())
        .filter(|v| v.account == name)
        .map(|v| v.debit_cents - v.credit_cents)
        .sum()
}
fn balanced(workspace: &Workspace) {
    for entry in &workspace.journal {
        assert_eq!(
            entry.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            entry.lines.iter().map(|v| v.credit_cents).sum::<i64>(),
            "{}",
            entry.id
        );
        assert!(entry
            .lines
            .iter()
            .all(|v| (v.debit_cents > 0) ^ (v.credit_cents > 0)));
    }
    assert_eq!(
        workspace
            .journal
            .iter()
            .flat_map(|v| &v.lines)
            .map(|v| v.debit_cents - v.credit_cents)
            .sum::<i64>(),
        0
    );
}

#[tokio::test]
async fn billing_snapshots_issue_once_and_freeze_customer_and_sell_rate() {
    let store = seeded().await;
    let draft = mutate(
        &store,
        "bill.create",
        "",
        json!({"number":"cust-001","load_id":"load_6","due_date":Utc::now().date_naive()}),
    )
    .await
    .expect("draft");
    let id = draft["id"].as_str().expect("id");
    assert_eq!(draft["number"], "CUST-001");
    assert_eq!(draft["snapshot"]["amount_cents"], 149000);
    assert!(store.read("demo").await.expect("read").journal.is_empty());
    let before = json!(store.read("demo").await.expect("read"));
    assert!(mutate(
        &store,
        "bill.create",
        "",
        json!({"number":"cust-001","load_id":"load_7","due_date":Utc::now().date_naive()})
    )
    .await
    .is_err());
    assert!(mutate(&store,"bill.create","",json!({"number":"other","load_id":"load_7","due_date":Utc::now().date_naive(),"amount_cents":1})).await.is_err());
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
    let input = json!({"expected_version":1});
    let first = store
        .mutate_authorized(
            ("demo", "finance:issuer"),
            "issue",
            "bill.issue",
            id,
            input.clone(),
        )
        .await
        .expect("issue");
    let replay = store
        .mutate_authorized(("demo", "finance:issuer"), "issue", "bill.issue", id, input)
        .await
        .expect("replay");
    assert_eq!(first["data"], replay["data"]);
    assert_eq!(replay["replayed"], true);
    let snapshot = store.read("demo").await.expect("read").bills[0]
        .snapshot
        .clone();
    mutate(&store,"bill.profile","",json!({"expected_version":1,"legal_name":"Updated Issuer LLC","address":"New real-world billing address","contact":"billing@example.test","payment_instructions":"New remittance instructions"})).await.expect("update issuer");
    let customer = store.read("demo").await.expect("read").customers[0].clone();
    mutate(&store,"customer.update","c1",json!({"expected":customer,"data":{"name":"Renamed customer","contact":"new@example.test"}})).await.expect("rename master record");
    let after = store.read("demo").await.expect("read");
    assert_eq!(json!(after.bills[0].snapshot), json!(snapshot));
    assert_eq!(
        after.bills[0]
            .snapshot
            .issuer
            .as_ref()
            .expect("issuer snapshot")
            .legal_name,
        "Synthetic Demo Freight LLC"
    );
    assert_eq!(
        after
            .billing_profile
            .as_ref()
            .expect("current issuer")
            .legal_name,
        "Updated Issuer LLC"
    );
    assert_eq!(after.journal.len(), 1);
    assert_eq!(after.journal[0].actor, "finance:issuer");
    assert_eq!(account(&after, "accounts_receivable"), 149000);
    assert_eq!(account(&after, "freight_revenue"), -149000);
    assert!(mutate(
        &store,
        "bill.revise",
        id,
        json!({"expected_version":2,"number":"edited","due_date":Utc::now().date_naive()})
    )
    .await
    .is_err());
    assert!(mutate(
        &store,
        "load.command",
        "load_6",
        json!({"action":"attach_pod","expected_version":1,"pod_reference":"replacement"})
    )
    .await
    .is_err());
    balanced(&after);
}

#[tokio::test]
async fn receipts_credits_and_carrier_payouts_reconcile_to_known_balances() {
    let store = seeded().await;
    let bill = bill(&store, "load_6", "B-100").await;
    mutate(&store,"credit.create",&bill.id,json!({"expected_version":2,"number":"CR-100","amount_cents":9000,"reason":"Agreed service adjustment"})).await.expect("credit");
    mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", &bill.id, 3, 40000, "deposit-1"),
    )
    .await
    .expect("partial receipt");
    let final_receipt = mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", &bill.id, 4, 100000, "deposit-2"),
    )
    .await
    .expect("final receipt");
    let invoice = store.read("demo").await.expect("read").invoices[0].clone();
    mutate(&store, "invoice.approve", &invoice.id, json!({}))
        .await
        .expect("carrier approval");
    let count = store.read("demo").await.expect("read").journal.len();
    let settlement = mutate(&store, "settlement.create", "load_6", json!({}))
        .await
        .expect("prepare");
    assert_eq!(
        store.read("demo").await.expect("read").journal.len(),
        count,
        "preparation does not book the same payable again"
    );
    let id = settlement["id"].as_str().expect("id");
    mutate(
        &store,
        "cash.record",
        "",
        cash_input("payout", id, 1, 50000, "wire-1"),
    )
    .await
    .expect("partial payout");
    let final_payout = mutate(
        &store,
        "cash.record",
        "",
        cash_input("payout", id, 2, 68000, "wire-2"),
    )
    .await
    .expect("final payout");
    let done = store.read("demo").await.expect("read");
    assert_eq!(done.bills[0].status, "paid");
    assert_eq!(done.settlements[0].status, "reconciled");
    assert_eq!(account(&done, "cash"), 22000);
    assert_eq!(account(&done, "accounts_receivable"), 0);
    assert_eq!(account(&done, "carrier_payable"), 0);
    assert_eq!(account(&done, "freight_revenue"), -140000);
    assert_eq!(account(&done, "freight_expense"), 118000);
    balanced(&done);
    let original = json!(done.journal);
    mutate(&store,"cash.reverse",final_receipt["id"].as_str().expect("id"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Deposit was returned by the bank"})).await.expect("reverse receipt");
    mutate(&store,"cash.reverse",final_payout["id"].as_str().expect("id"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Bank rejected final carrier wire"})).await.expect("reverse payout");
    let reopened = store.read("demo").await.expect("read");
    assert_eq!(
        reopened
            .bill_totals(&reopened.bills[0])
            .expect("totals")
            .balance_cents,
        100000
    );
    assert_eq!(reopened.settlement_balance(id).expect("balance"), 68000);
    assert_eq!(
        json!(&reopened.journal[..done.journal.len()]),
        original,
        "original postings are immutable"
    );
    balanced(&reopened);
}

#[tokio::test]
async fn bad_cash_allocations_do_not_change_business_journal_audit_or_receipts() {
    let store = seeded().await;
    let a = bill(&store, "load_6", "A").await;
    let b = bill(&store, "load_7", "B").await;
    let before = json!(store.read("demo").await.expect("read"));
    let mut bad = cash_input("receipt", &a.id, 2, 10000, "bank");
    bad["allocations"] = json!([{"target_id":a.id,"expected_version":2,"amount_cents":5000},{"target_id":b.id,"expected_version":2,"amount_cents":5000}]);
    let mut mismatch = cash_input("receipt", &a.id, 2, 10000, "bank");
    mismatch["amount_cents"] = json!(9999);
    let mut future = cash_input("receipt", &a.id, 2, 1, "bank");
    future["cleared_on"] = json!(Utc::now().date_naive() + chrono::Duration::days(1));
    let mut duplicate = cash_input("receipt", &a.id, 2, 2, "bank");
    duplicate["allocations"] = json!([{"target_id":a.id,"expected_version":2,"amount_cents":1},{"target_id":a.id,"expected_version":2,"amount_cents":1}]);
    for body in [
        bad,
        mismatch,
        future,
        duplicate,
        cash_input("receipt", &a.id, 1, 10000, "bank"),
        cash_input("receipt", &a.id, 2, 149001, "bank"),
    ] {
        assert!(mutate(&store, "cash.record", "", body).await.is_err());
        assert_eq!(json!(store.read("demo").await.expect("read")), before);
    }
    mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", &a.id, 2, 1, "bank"),
    )
    .await
    .expect("record");
    assert!(mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", &b.id, 2, 1, "bank")
    )
    .await
    .is_err());
}

#[tokio::test]
async fn batched_receipts_keep_one_party_and_concurrent_allocations_cannot_overpay() {
    let store = seeded().await;
    let a = bill(&store, "load_6", "A").await;
    let b = bill(&store, "load_1", "B").await;
    let mut batch = cash_input("receipt", &a.id, 2, 20000, "batch");
    batch["allocations"] = json!([{"target_id":a.id,"expected_version":2,"amount_cents":10000},{"target_id":b.id,"expected_version":2,"amount_cents":10000}]);
    mutate(&store, "cash.record", "", batch)
        .await
        .expect("batch");
    let (first, second) = tokio::join!(
        mutate(
            &store,
            "cash.record",
            "",
            cash_input("receipt", &a.id, 3, 139000, "race-a")
        ),
        mutate(
            &store,
            "cash.record",
            "",
            cash_input("receipt", &a.id, 3, 139000, "race-b")
        )
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let after = store.read("demo").await.expect("read");
    assert_eq!(after.cash_entries.len(), 2);
    assert_eq!(after.bills[0].status, "paid");
    assert_eq!(
        after
            .bill_totals(&after.bills[1])
            .expect("totals")
            .received_cents,
        10000
    );
    balanced(&after);
}

#[tokio::test]
async fn cash_corrections_preserve_reversals_and_cannot_reuse_an_active_bank_reference() {
    let store = seeded().await;
    let a = bill(&store, "load_6", "A").await;
    let b = bill(&store, "load_7", "B").await;
    let input = cash_input("receipt", &a.id, 2, 50000, "deposit");
    let first = store
        .mutate_authorized(
            ("demo", "finance:recorder"),
            "cash-key",
            "cash.record",
            "",
            input.clone(),
        )
        .await
        .expect("record");
    let replay = store
        .mutate_authorized(
            ("demo", "finance:recorder"),
            "cash-key",
            "cash.record",
            "",
            input,
        )
        .await
        .expect("replay");
    assert_eq!(first["data"], replay["data"]);
    let id = first["data"]["id"].as_str().expect("id");
    mutate(&store,"cash.reverse",id,json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Incorrect bank allocation amount"})).await.expect("reverse");
    assert!(mutate(
        &store,
        "cash.record",
        "",
        cash_input("receipt", &a.id, 4, 40000, "deposit")
    )
    .await
    .is_err());
    let mut correction = cash_input("receipt", &b.id, 2, 40000, "deposit");
    correction["corrects_id"] = json!(id);
    mutate(&store, "cash.record", "", correction.clone())
        .await
        .expect("explicit correction");
    assert!(mutate(&store, "cash.record", "", correction).await.is_err());
    let after = store.read("demo").await.expect("read");
    assert_eq!(after.cash_entries.len(), 2);
    assert!(after.cash_entries[0].reversal.is_some());
    assert!(after.cash_entries[0].corrected_by_id.is_some());
    assert_eq!(
        after
            .bill_totals(&after.bills[0])
            .expect("original customer balance")
            .received_cents,
        0
    );
    assert_eq!(
        after
            .bill_totals(&after.bills[1])
            .expect("totals")
            .received_cents,
        40000
    );
    balanced(&after);
}

#[tokio::test]
async fn credit_reversals_and_voids_append_balanced_reversals_without_reusing_numbers() {
    let store = seeded().await;
    let bill = bill(&store, "load_6", "BILL").await;
    let credit=mutate(&store,"credit.create",&bill.id,json!({"expected_version":2,"number":"CR","amount_cents":149000,"reason":"Full service credit"})).await.expect("credit");
    assert_eq!(
        store.read("demo").await.expect("read").bills[0].status,
        "credited"
    );
    assert!(mutate(
        &store,
        "bill.void",
        &bill.id,
        json!({"expected_version":3,"note":"Cannot void an active credit"})
    )
    .await
    .is_err());
    mutate(&store,"credit.reverse",credit["id"].as_str().expect("id"),json!({"expected_version":1,"reversed_on":Utc::now().date_naive(),"note":"Credit issued to wrong shipment"})).await.expect("reverse");
    mutate(
        &store,
        "bill.void",
        &bill.id,
        json!({"expected_version":4,"note":"Cancel original billing document"}),
    )
    .await
    .expect("void");
    let after = store.read("demo").await.expect("read");
    assert_eq!(after.bills[0].status, "void");
    assert_eq!(account(&after, "accounts_receivable"), 0);
    assert_eq!(account(&after, "freight_revenue"), 0);
    assert_eq!(after.journal.len(), 4);
    balanced(&after);
    assert!(mutate(
        &store,
        "bill.create",
        "",
        json!({"number":"BILL","load_id":"load_6","due_date":Utc::now().date_naive()})
    )
    .await
    .is_err());
    assert!(!after.evidence_locked("load_6"));
}

#[tokio::test]
async fn statement_escapes_untrusted_text_and_exports_only_customer_facing_data() {
    let store = seeded().await;
    let bill = bill(&store, "load_6", "BILL-PRINT").await;
    let mut workspace = store.read("demo").await.expect("read");
    workspace.bills[0].snapshot.customer.name = "<script>bad()</script> & customer".into();
    let (_, html) = finance::statement(&workspace, &bill.id).expect("statement");
    assert!(html.contains("&lt;script&gt;bad()&lt;/script&gt; &amp; customer"));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("1180.00"));
    assert!(html.contains("1490.00"));
    workspace.journal[0].memo = "  =DANGEROUS()\",value".into();
    let csv = finance::journal_csv(&workspace);
    assert!(csv.contains("\"'  =DANGEROUS()\"\",value\""));
    workspace.bills[0].due_date = Utc::now().date_naive() - chrono::Duration::days(31);
    assert_eq!(
        workspace
            .bill_totals(&workspace.bills[0])
            .expect("aging")
            .overdue_days,
        31
    );
    assert_eq!(
        workspace.finance_summary().expect("summary")["overdue_cents"],
        149000
    );
}

#[tokio::test]
async fn issue_requires_reviewed_evidence_and_failed_billing_is_atomic() {
    let store = seeded().await;
    mutate(&store,"load.command","load_6",json!({"action":"attach_pod","expected_version":1,"pod_reference":"new-unreviewed-reference"})).await.expect("replace");
    let draft = mutate(
        &store,
        "bill.create",
        "",
        json!({"number":"hold","load_id":"load_6","due_date":Utc::now().date_naive()}),
    )
    .await
    .expect("draft");
    let before = json!(store.read("demo").await.expect("read"));
    assert!(mutate(
        &store,
        "bill.issue",
        draft["id"].as_str().expect("id"),
        json!({"expected_version":1})
    )
    .await
    .is_err());
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_financial_posting_is_atomic_under_reconnect_and_concurrent_reconciliation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("url");
    let a = Store::new(Some(&url)).await.expect("connect");
    let b = Store::new(Some(&url)).await.expect("second");
    let tenant = domain::id("financial-test");
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    canonical::put_workspace(&pool, &tenant, &json!(domain::demo()))
        .await
        .expect("fixture");
    let created = a
        .mutate(
            &tenant,
            "draft",
            "bill.create",
            "",
            json!({"load_id":"load_6","number":"BILL","due_date":Utc::now().date_naive()}),
        )
        .await
        .expect("draft");
    let id = created["data"]["id"].as_str().expect("id");
    a.mutate(
        &tenant,
        "issue",
        "bill.issue",
        id,
        json!({"expected_version":1}),
    )
    .await
    .expect("issue");
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:a"),
            "bank-a",
            "cash.record",
            "",
            cash_input("receipt", id, 2, 149000, "bank-a")
        ),
        b.mutate_authorized(
            (&tenant, "finance:b"),
            "bank-b",
            "cash.record",
            "",
            cash_input("receipt", id, 2, 149000, "bank-b")
        )
    );
    assert_ne!(first.is_ok(), second.is_ok());
    drop(a);
    drop(b);
    let restarted = Store::new(Some(&url)).await.expect("reconnect");
    let after = restarted.read(&tenant).await.expect("read");
    assert_eq!(after.cash_entries.len(), 1);
    assert_eq!(after.journal.len(), 2);
    assert_eq!(after.bills[0].status, "paid");
    balanced(&after);
    assert_eq!(
        after.receipts.len(),
        3,
        "failed concurrent request leaves no receipt"
    );
    assert_eq!(
        after
            .events
            .iter()
            .filter(|v| v.kind == "cash.recorded")
            .count(),
        1
    );
    assert!(restarted
        .read("unrelated-finance-tenant")
        .await
        .expect("read")
        .journal
        .is_empty());
}

#[test]
fn issuer_and_bill_to_identity_are_required_before_issue_and_use_optimistic_configuration() {
    let mut workspace = domain::demo();
    workspace.billing_profile = None;
    let draft = workspace
        .apply_finance(
            "bill.create",
            "",
            json!({"number":"IDENTITY","load_id":"load_6","due_date":Utc::now().date_naive()}),
            "finance",
        )
        .expect("draft");
    let id = draft["id"].as_str().expect("id");
    let before = json!(workspace);
    assert!(workspace
        .apply_finance("bill.issue", id, json!({"expected_version":1}), "finance")
        .is_err());
    assert_eq!(json!(workspace), before);
    let profile = json!({"expected_version":0,"legal_name":"Example Freight LLC","address":"123 Actual Street, Portland, OR","contact":"billing@example.test","payment_instructions":"Use agreed payment instructions"});
    workspace
        .apply_finance("bill.profile", "", profile.clone(), "finance")
        .expect("profile");
    assert!(workspace
        .apply_finance("bill.profile", "", profile, "finance")
        .is_err());
    workspace.bills[0].snapshot.customer.billing_address.clear();
    assert!(workspace
        .apply_finance("bill.issue", id, json!({"expected_version":1}), "finance")
        .is_err());
    workspace.apply_finance("bill.revise",id,json!({"expected_version":1,"number":"IDENTITY","due_date":Utc::now().date_naive(),"bill_to_address":"Customer billing department, 456 Main St"}),"finance").expect("bill to");
    workspace
        .apply_finance("bill.issue", id, json!({"expected_version":2}), "finance")
        .expect("issue");
    let (_, html) = finance::statement(&workspace, id).expect("statement");
    assert!(html.contains("Example Freight LLC"));
    assert!(html.contains("Customer billing department, 456 Main St"));
}
