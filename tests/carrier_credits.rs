#[path = "support/canonical.rs"]
mod canonical;
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    documents::Upload,
    domain::{self, Workspace},
    store::Store,
};

const POD: &[u8] = b"%PDF-1.4\nsigned synthetic receiver delivery evidence\n%%EOF\n";
async fn op(s: &Store, kind: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "finance:credit-test"),
        &domain::id("request"),
        kind,
        id,
        body,
    )
    .await
    .unwrap_or_else(|e| panic!("{kind}: {e}"))["data"]
        .clone()
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
fn cash(direction: &str, target: &Value, amount: i64, reference: &str) -> Value {
    json!({"direction":direction,"currency":"USD","account":"operating-usd","external_reference":reference,"amount_cents":amount,
        "cleared_on":Utc::now().date_naive(),"note":"Verified actual cleared bank statement",
        "allocations":[{"target_id":target["id"],"expected_version":target["version"],"amount_cents":amount}]})
}
async fn target(s: &Store, carrier: &str, reference: &str, amount: i64) -> String {
    let now = Utc::now();
    let date = now.date_naive();
    let mut load=op(s,"load.create","",json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van",
        "commodity":"Credit application qualification","weight_lbs":1000,"miles":173,"buy_cents":amount,"sell_cents":amount+10000,
        "origin":{"facility":"Origin","city":"Portland","state":"OR","date":date,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},
        "destination":{"facility":"Receiver","city":"Seattle","state":"WA","date":date,"address_lines":["25 Receiver Way"],"postal_code":"98101","country":"US"}})).await;
    let id = load["id"].as_str().expect("load").to_string();
    load = op(
        s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":load["version"],"carrier_id":carrier}),
    )
    .await;
    load = op(
        s,
        "load.command",
        &id,
        json!({"action":"accept","expected_version":load["version"]}),
    )
    .await;
    let w = read(s).await;
    let tender = w
        .tenders
        .iter()
        .find(|v| Some(v.id.as_str()) == load["tender_id"].as_str())
        .expect("tender");
    let party = w
        .carriers
        .iter()
        .find(|v| v.id == carrier)
        .expect("carrier");
    op(s,"edi.event","",json!({"event_id":domain::id("event"),"event_type":"edi.shipment_status.received","occurred_at":Utc::now(),
        "payload":{"shipment_id":tender.shipment_reference,"partner_id":party.partner_id,"carrier_scac":party.scac,"latest_status":"D1","events":[{"status_code":"AF"},{"status_code":"D1"}]}})).await;
    let doc = s
        .upload_document(
            ("demo", "finance:credit-test"),
            &domain::id("upload"),
            &id,
            Upload {
                kind: "pod".into(),
                filename: format!("{reference}.pdf"),
                content_type: "application/pdf".into(),
                bytes: POD.to_vec(),
            },
        )
        .await
        .expect("POD")["data"]
        .clone();
    let w = read(s).await;
    let current = w.loads.iter().find(|v| v.id == id).expect("load");
    op(
        s,
        "load.command",
        &id,
        json!({"action":"attach_pod","expected_version":current.version,"document_id":doc["id"]}),
    )
    .await;
    op(s,"document.review",doc["id"].as_str().expect("doc"),json!({"expected_version":1,"decision":"accepted","note":"Inspected signed receiver evidence"})).await;
    let invoice=op(s,"invoice.create","",json!({"load_id":id,"number":format!("INV-{reference}"),"amount_cents":amount,"currency":"USD"})).await;
    op(
        s,
        "invoice.approve",
        invoice["id"].as_str().expect("invoice"),
        json!({}),
    )
    .await;
    op(s, "settlement.create", &id, json!({})).await["id"]
        .as_str()
        .expect("settlement")
        .to_string()
}
struct Fixture {
    s: Store,
    source: String,
    a: String,
    b: String,
    foreign: String,
    credit: Value,
}
async fn fixture() -> Fixture {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let original = read(&s).await.invoices[0].clone();
    op(&s, "invoice.approve", &original.id, json!({})).await;
    let prepared = op(&s, "settlement.create", "load_6", json!({})).await;
    op(
        &s,
        "cash.record",
        "",
        cash("payout", &prepared, 118000, "source-wire"),
    )
    .await;
    let draft=op(&s,"payable.create","load_6",json!({"expected_load_version":read(&s).await.loads[5].version,"invoice_id":original.id,"kind":"credit_memo","number":"CREDIT-4501","document_amount_cents":4501,"issued_on":Utc::now().date_naive(),"description":"Verified carrier service credit","evidence_reference":"actual-credit-4501"})).await;
    let credit=op(&s,"payable.review",draft["id"].as_str().expect("id"),json!({"expected_load_version":read(&s).await.loads[5].version,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier accounting","confirmation_reference":"carrier-credit-confirmation","note":"Verified original carrier credit memo"})).await;
    let a = target(&s, "ca1", "CREDIT-TARGET-A", 20000).await;
    let b = target(&s, "ca1", "CREDIT-TARGET-B", 10000).await;
    let foreign = target(&s, "ca4", "OTHER-CARRIER", 15000).await;
    Fixture {
        s,
        source: prepared["id"].as_str().expect("id").into(),
        a,
        b,
        foreign,
        credit,
    }
}
fn apply_input(w: &Workspace, source: &str, targets: &[(&str, i64)], reference: &str) -> Value {
    json!({"source_settlement_id":source,"expected_source_version":w.settlements.iter().find(|v|v.id==source).expect("source").version,
        "amount_cents":targets.iter().map(|v|v.1).sum::<i64>(),"applied_on":Utc::now().date_naive(),"evidence_reference":reference,
        "counterparty_contact":"Carrier AR representative","confirmation_reference":"carrier-application-confirmation","note":"Inspected the carrier's actual allocation confirmation",
        "allocations":targets.iter().map(|(id,amount)|json!({"target_id":id,"expected_version":w.settlements.iter().find(|v|v.id==*id).expect("target").version,"amount_cents":amount})).collect::<Vec<_>>()})
}
fn reverse_input(w: &Workspace, entry: &Value) -> Value {
    json!({"expected_version":entry["version"],"expected_source_version":w.settlements.iter().find(|v|Some(v.id.as_str())==entry["source_settlement_id"].as_str()).expect("source").version,
        "targets":entry["allocations"].as_array().expect("allocations").iter().map(|a|json!({"target_id":a["target_id"],"expected_version":w.settlements.iter().find(|v|Some(v.id.as_str())==a["target_id"].as_str()).expect("target").version})).collect::<Vec<_>>(),
        "reversed_on":Utc::now().date_naive(),"counterparty_contact":"Carrier correction desk","confirmation_reference":"carrier-retraction-confirmation","note":"Carrier confirmed retraction of this allocation"})
}
fn net(w: &Workspace, account: &str) -> i64 {
    w.journal
        .iter()
        .flat_map(|v| &v.lines)
        .filter(|v| v.account == account)
        .map(|v| v.debit_cents - v.credit_cents)
        .sum()
}
fn balanced(w: &Workspace) {
    for entry in &w.journal {
        assert_eq!(
            entry.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            entry.lines.iter().map(|v| v.credit_cents).sum::<i64>()
        );
    }
    assert_eq!(
        w.settlements
            .iter()
            .map(|v| w.settlement_balance(&v.id).expect("balance"))
            .sum::<i64>(),
        -net(w, "carrier_payable")
    );
}
async fn fails(s: &Store, operation: &str, entity: &str, input: Value) {
    let before = json!(read(s).await);
    assert!(s
        .mutate("demo", &domain::id("failed"), operation, entity, input)
        .await
        .is_err());
    assert_eq!(
        json!(read(s).await),
        before,
        "failed request must leave no financial, audit or idempotency residue"
    );
}

#[tokio::test]
async fn credit_application_splits_without_cash_or_expense_and_preserves_reversals_corrections_and_source_retraction(
) {
    let f = fixture().await;
    let before = read(&f.s).await;
    let original_invoices = json!(before.invoices);
    let original_bank = json!(before.cash_entries);
    let entry = op(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(
            &before,
            &f.source,
            &[(&f.a, 1251), (&f.b, 3250)],
            "APPLICATION-15",
        ),
    )
    .await;
    let id = entry["id"].as_str().expect("id");
    let w = read(&f.s).await;
    assert_eq!(w.settlement_balance(&f.source).expect("balance"), 0);
    assert_eq!(w.settlement_balance(&f.a).expect("balance"), 18749);
    assert_eq!(w.settlement_balance(&f.b).expect("balance"), 6750);
    assert_eq!(net(&w, "freight_expense"), 158499);
    assert_eq!(net(&w, "cash"), -118000);
    assert_eq!(json!(w.invoices), original_invoices);
    assert_eq!(json!(w.cash_entries), original_bank);
    assert!(w
        .journal
        .last()
        .expect("application posting")
        .lines
        .iter()
        .all(|v| v.account == "carrier_payable"));
    for settlement in [&f.a, &f.b, &f.foreign] {
        let w = read(&f.s).await;
        let target = w
            .settlements
            .iter()
            .find(|v| &v.id == settlement)
            .expect("target");
        op(
            &f.s,
            "cash.record",
            "",
            cash(
                "payout",
                &json!(target),
                w.settlement_balance(settlement).expect("balance"),
                settlement,
            ),
        )
        .await;
    }
    let w = read(&f.s).await;
    balanced(&w);
    assert_eq!(net(&w, "carrier_payable"), 0);
    assert_eq!(net(&w, "cash"), -158499);
    let reversed = op(
        &f.s,
        "carrier_credit.reverse",
        id,
        reverse_input(&w, &entry),
    )
    .await;
    assert_eq!(reversed["confirmation"], entry["confirmation"]);
    assert_eq!(
        reversed["reversal_confirmation"]["counterparty_contact"],
        "Carrier correction desk"
    );
    let w = read(&f.s).await;
    assert_eq!(w.settlement_balance(&f.source).expect("source"), -4501);
    assert_eq!(w.settlement_balance(&f.a).expect("target"), 1251);
    assert_eq!(w.settlement_balance(&f.b).expect("target"), 3250);
    let mut correction = apply_input(
        &w,
        &f.source,
        &[(&f.a, 1251), (&f.b, 1750)],
        "application-15",
    );
    fails(&f.s, "carrier_credit.apply", "", correction.clone()).await;
    correction["corrects_id"] = json!(id);
    let corrected = op(&f.s, "carrier_credit.apply", "", correction).await;
    let w = read(&f.s).await;
    assert_eq!(
        w.carrier_credit_applications[0].corrected_by_id.as_deref(),
        corrected["id"].as_str()
    );
    assert_eq!(
        w.finance_summary().expect("totals")["prepared_payable_cents"],
        1500
    );
    assert_eq!(
        w.finance_summary().expect("totals")["carrier_credit_cents"],
        1500
    );
    let mut branching = apply_input(&w, &f.source, &[(&f.b, 1500)], "application-15");
    branching["corrects_id"] = json!(id);
    fails(&f.s, "carrier_credit.apply", "", branching).await;
    op(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(&w, &f.source, &[(&f.b, 1500)], "APPLICATION-16"),
    )
    .await;
    let w = read(&f.s).await;
    balanced(&w);
    assert_eq!(net(&w, "carrier_payable"), 0);
    assert_eq!(net(&w, "cash"), -158499);
    assert_eq!(net(&w, "freight_expense"), 158499);
    op(&f.s,"payable.reverse",f.credit["id"].as_str().expect("credit"),json!({"expected_load_version":w.loads[5].version,"expected_version":f.credit["version"],"reversed_on":Utc::now().date_naive(),"counterparty_contact":"Carrier accounting","confirmation_reference":"credit-retracted","note":"Actual carrier retraction of original credit memo"})).await;
    let w = read(&f.s).await;
    balanced(&w);
    assert_eq!(
        w.settlement_balance(&f.source).expect("source"),
        4501,
        "retracting the source memo reopens its payable, not the target invoices"
    );
    assert_eq!(net(&w, "freight_expense"), 163000);
    assert_eq!(net(&w, "cash"), -158499);
}

#[tokio::test]
async fn carrier_credit_guards_reject_mixed_carriers_duplicates_stale_bank_amounts_and_invalid_dates_atomically(
) {
    let f = fixture().await;
    let w = read(&f.s).await;
    let input = apply_input(&w, &f.source, &[(&f.a, 4501)], "GUARD-15");
    for (field, value) in [
        ("amount_cents", json!(0)),
        ("amount_cents", json!(4502)),
        ("amount_cents", json!(4500)),
        ("expected_source_version", json!(0)),
        ("counterparty_contact", json!(" ")),
        ("evidence_reference", json!("")),
        ("applied_on", json!("1900-01-01")),
        (
            "applied_on",
            json!((Utc::now() + chrono::Duration::days(1)).date_naive()),
        ),
        ("document_id", json!("missing")),
        ("carrier_id", json!("forged")),
        ("allocations", json!([])),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        fails(&f.s, "carrier_credit.apply", "", bad).await;
    }
    fails(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(&w, &f.source, &[(&f.foreign, 4501)], "FOREIGN"),
    )
    .await;
    fails(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(&w, &f.source, &[(&f.source, 4501)], "SELF"),
    )
    .await;
    fails(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(&w, &f.source, &[(&f.a, 2000), (&f.a, 2501)], "DUPLICATE"),
    )
    .await;
    let mut wrong_file = input.clone();
    wrong_file["document_id"] = json!(
        w.loads
            .iter()
            .find(|v| v.id
                == w.settlements
                    .iter()
                    .find(|v| v.id == f.a)
                    .expect("target")
                    .load_id)
            .expect("load")
            .pod_document_id
    );
    fails(&f.s, "carrier_credit.apply", "", wrong_file).await;
    let small_target = w.settlements.iter().find(|v| v.id == f.b).expect("target");
    op(
        &f.s,
        "cash.record",
        "",
        cash(
            "payout",
            &json!(small_target),
            9500,
            "target-partially-paid",
        ),
    )
    .await;
    let fresh = read(&f.s).await;
    fails(
        &f.s,
        "carrier_credit.apply",
        "",
        apply_input(&fresh, &f.source, &[(&f.b, 501)], "EXCESS-TARGET"),
    )
    .await;
    let target = w.settlements.iter().find(|v| v.id == f.a).expect("target");
    let stale_bank = cash("payout", &json!(target), 20000, "stale-payout");
    let entry = op(&f.s, "carrier_credit.apply", "", input.clone()).await;
    fails(&f.s, "cash.record", "", stale_bank).await;
    let mut duplicate = apply_input(&read(&f.s).await, &f.source, &[(&f.a, 1)], "GUARD-15");
    duplicate["expected_source_version"] = input["expected_source_version"].clone();
    fails(&f.s, "carrier_credit.apply", "", duplicate).await;
    let mut reverse = reverse_input(&read(&f.s).await, &entry);
    reverse["targets"] = json!([]);
    fails(
        &f.s,
        "carrier_credit.reverse",
        entry["id"].as_str().expect("id"),
        reverse,
    )
    .await;
    let mut reverse = reverse_input(&read(&f.s).await, &entry);
    reverse["targets"][0]["expected_version"] = json!(0);
    fails(
        &f.s,
        "carrier_credit.reverse",
        entry["id"].as_str().expect("id"),
        reverse,
    )
    .await;
}

#[tokio::test]
async fn legacy_credit_allocation_recognizes_opening_liability_once_and_keeps_original_cost_basis()
{
    let f = fixture().await;
    let mut w = read(&f.s).await;
    let target_load = w
        .settlements
        .iter()
        .find(|v| v.id == f.a)
        .expect("target")
        .load_id
        .clone();
    w.journal
        .retain(|v| !v.lines.iter().any(|line| line.load_id == target_load));
    let mut zero = w.clone();
    let yesterday = Utc::now().date_naive() - chrono::Duration::days(1);
    let load = zero
        .loads
        .iter_mut()
        .find(|v| v.id == target_load)
        .expect("load");
    load.origin.date = yesterday;
    load.buy_cents = 0;
    zero.invoices
        .iter_mut()
        .find(|v| v.load_id == target_load)
        .expect("invoice")
        .amount_cents = 0;
    zero.settlements
        .iter_mut()
        .find(|v| v.id == f.a)
        .expect("settlement")
        .amount_cents = 0;
    assert_eq!(
        zero.carrier_credit_earliest_date(&f.a)
            .expect("zero liability has no opening journal"),
        yesterday
    );
    let input = apply_input(&w, &f.source, &[(&f.a, 4501)], "LEGACY");
    let entry = w
        .apply_carrier_credit("carrier_credit.apply", "", input, "finance:legacy")
        .expect("application");
    balanced(&w);
    assert_eq!(
        w.journal
            .iter()
            .filter(|v| v.source_kind == "carrier_invoice"
                && v.lines.iter().any(|l| l.load_id == target_load))
            .count(),
        1
    );
    assert_eq!(
        w.settlement_total(w.settlements.iter().find(|v| v.id == f.a).expect("target"))
            .expect("cost"),
        20000
    );
    let mut invalid_date = reverse_input(&w, &entry);
    invalid_date["reversed_on"] = json!("1900-01-01");
    assert!(w
        .clone()
        .apply_carrier_credit(
            "carrier_credit.reverse",
            entry["id"].as_str().expect("id"),
            invalid_date,
            "finance"
        )
        .is_err());
    let mut old = json!(w);
    old.as_object_mut()
        .expect("workspace")
        .remove("carrier_credit_applications");
    assert!(serde_json::from_value::<Workspace>(old)
        .expect("old workspace schema")
        .carrier_credit_applications
        .is_empty());
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_credit_evidence_races_with_refunds_and_keeps_one_allocation_after_reconnect() {
    let f = fixture().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("one");
    let b = Store::new(Some(&url)).await.expect("two");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("carrier-credit-test");
    let w = read(&f.s).await;
    canonical::put_workspace(&pool, &tenant, &json!(w))
        .await
        .expect("fixture");
    for doc in &w.documents {
        let (_, bytes) =
            f.s.document_content("demo", &doc.id)
                .await
                .expect("actual fixture bytes");
        sqlx::query(
            "INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)",
        )
        .bind(&tenant)
        .bind(&doc.id)
        .bind(bytes)
        .execute(&pool)
        .await
        .expect("fixture bytes");
    }
    let proof = b"%PDF-1.4\ncarrier allocation confirmation\n%%EOF\n";
    let doc = a
        .upload_document(
            (&tenant, "finance:pg"),
            "proof",
            "load_6",
            Upload {
                kind: "other".into(),
                filename: "application.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: proof.to_vec(),
            },
        )
        .await
        .expect("proof")["data"]
        .clone();
    let doc_id = doc["id"].as_str().expect("doc");
    let target_load = &w
        .settlements
        .iter()
        .find(|v| v.id == f.a)
        .expect("target")
        .load_id;
    let pod_id = w
        .loads
        .iter()
        .find(|v| &v.id == target_load)
        .expect("load")
        .pod_document_id
        .as_ref()
        .expect("POD");
    let mut input = apply_input(&w, &f.source, &[(&f.a, 4501)], "PG-CREDIT-APPLICATION");
    input["document_id"] = json!(doc_id);
    let before = json!(a.read(&tenant).await.expect("before"));
    for (file_id, original) in [(doc_id, proof.as_slice()), (pod_id.as_str(), POD)] {
        sqlx::query(
            "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
        )
        .bind(b"corrupted".as_slice())
        .bind(&tenant)
        .bind(file_id)
        .execute(&pool)
        .await
        .expect("corrupt");
        assert!(a
            .mutate(
                &tenant,
                "apply-a",
                "carrier_credit.apply",
                "",
                input.clone()
            )
            .await
            .is_err());
        assert_eq!(
            json!(b.read(&tenant).await.expect("unchanged")),
            before,
            "both own agreement and target POD bytes must be verified atomically"
        );
        sqlx::query(
            "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
        )
        .bind(original)
        .bind(&tenant)
        .bind(file_id)
        .execute(&pool)
        .await
        .expect("restore");
    }
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:a"),
            "apply-a",
            "carrier_credit.apply",
            "",
            input.clone()
        ),
        b.mutate_authorized(
            (&tenant, "finance:b"),
            "apply-b",
            "carrier_credit.apply",
            "",
            input.clone()
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let (winner, key) = if let Ok(v) = x {
        (v["data"].clone(), "apply-a")
    } else {
        (y.expect("one winner")["data"].clone(), "apply-b")
    };
    assert_eq!(
        a.mutate(&tenant, key, "carrier_credit.apply", "", input)
            .await
            .expect("replay")["replayed"],
        true
    );
    let w = b.read(&tenant).await.expect("posted");
    balanced(&w);
    assert_eq!(w.cash_entries.len(), 1);
    assert_eq!(w.carrier_credit_applications.len(), 1);
    assert!(a.mutate(&tenant,"reject-application-proof","document.review",doc_id,json!({"expected_version":1,"decision":"rejected","note":"Active financial evidence cannot disappear"})).await.is_err());
    let id = winner["id"].as_str().expect("application");
    a.mutate(
        &tenant,
        "reverse",
        "carrier_credit.reverse",
        id,
        reverse_input(&w, &winner),
    )
    .await
    .expect("reverse");
    let w = a.read(&tenant).await.expect("reopened");
    let mut correction = apply_input(&w, &f.source, &[(&f.a, 4501)], "PG-CREDIT-APPLICATION");
    correction["corrects_id"] = json!(id);
    correction["document_id"] = json!(doc_id);
    let source = w
        .settlements
        .iter()
        .find(|v| v.id == f.source)
        .expect("source");
    let refund = cash(
        "carrier_refund",
        &json!(source),
        4501,
        "actual-refund-competing-with-allocation",
    );
    let (x, y) = tokio::join!(
        a.mutate(
            &tenant,
            "correct-allocation",
            "carrier_credit.apply",
            "",
            correction
        ),
        b.mutate(&tenant, "actual-refund", "cash.record", "", refund)
    );
    assert_ne!(
        x.is_ok(),
        y.is_ok(),
        "a refund and credit allocation cannot consume the same balance"
    );
    let applied = x.is_ok();
    let reconnected = Store::new(Some(&url)).await.expect("reconnect");
    let w = reconnected.read(&tenant).await.expect("read");
    balanced(&w);
    assert_eq!(w.settlement_balance(&f.source).expect("source"), 0);
    assert_eq!(
        w.settlement_balance(&f.a).expect("target"),
        if applied { 15499 } else { 20000 }
    );
    assert_eq!(w.cash_entries.len(), if applied { 1 } else { 2 });
    assert_eq!(net(&w, "freight_expense"), 158499);
    assert_eq!(net(&w, "cash"), if applied { -118000 } else { -113499 });
    assert!(w.carrier_credit_applications[0].reversal.is_some());
    assert!(reconnected
        .read("other-carrier-credit-tenant")
        .await
        .expect("isolation")
        .carrier_credit_applications
        .is_empty());
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean files");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("clean workspace");
}
