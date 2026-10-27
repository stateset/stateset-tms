use crate::{
    domain::{self, Workspace},
    finance::FinanceReadIndex,
};
use serde_json::{json, Value};

pub(crate) fn fixture() -> Workspace {
    let mut w = domain::demo();
    let at = "2010-01-01T00:00:00Z";
    for load in &mut w.loads {
        load.origin.date = "2010-01-01".parse().expect("date");
    }
    for invoice in &mut w.invoices {
        invoice.status = "approved".into();
    }
    for (id, load, amount) in [("a", "load_6", 118000), ("b", "load_7", 108000)] {
        let load = w.loads.iter().find(|v| v.id == load).expect("load");
        w.bills.push(serde_json::from_value(json!({"id":format!("bill-{id}"),"number":id,"load_id":load.id,"customer_id":load.customer_id,
            "snapshot":{"customer":w.customers[0],"load_reference":load.reference,"origin":load.origin,"destination":load.destination,"commodity":load.commodity,"amount_cents":150000,"currency":"USD","source_load_version":1},
            "due_date":"2010-01-02","status":"issued","version":1,"created_at":at,"created_by":"fixture:finance","issued_at":at})).expect("bill"));
        w.settlements.push(serde_json::from_value(json!({"id":format!("settlement-{id}"),"load_id":load.id,"carrier_id":load.carrier_id,"amount_cents":amount,"status":"prepared","version":1})).expect("settlement"));
    }
    let cash = |id: &str, direction: &str, target: &str, amount: i64, reversed: bool| {
        serde_json::from_value(json!({"id":id,"direction":direction,"party_id":"fixture","account":"USD","external_reference":id,"amount_cents":amount,"currency":"USD","cleared_on":"2010-02-01","note":"Synthetic bank activity","allocations":[{"target_id":target,"load_id":"load_6","amount_cents":amount}],"recorded_at":at,"recorded_by":"fixture:bank","version":1,
            "reversal":if reversed {json!({"note":"Reversed synthetic entry","reversed_on":"2010-03-01","actor":"fixture:bank","journal_id":"reverse"})} else {Value::Null}})).expect("cash")
    };
    w.cash_entries = vec![
        cash("received", "receipt", "bill-a", 20000, false),
        cash("refund", "customer_refund", "bill-a", 5000, false),
        cash("reversed", "receipt", "bill-a", 7000, true),
        cash("paid", "payout", "settlement-a", 50000, false),
        cash(
            "carrier-return",
            "carrier_refund",
            "settlement-a",
            10000,
            false,
        ),
    ];
    w.cash_entries[0].allocations.push(
        serde_json::from_value(
            json!({"target_id":"bill-b","load_id":"load_7","amount_cents":3000}),
        )
        .expect("allocation"),
    );
    w.credits.push(serde_json::from_value(json!({"id":"credit","number":"credit","bill_id":"bill-a","load_id":"load_6","amount_cents":10000,"reason":"Synthetic credit","created_at":at,"created_by":"fixture:finance","version":1,"issued_on":"2010-02-02"})).expect("credit"));
    let mut reversed_credit = w.credits[0].clone();
    reversed_credit.id = "reversed-credit".into();
    reversed_credit.reversal = w.cash_entries[2].reversal.clone();
    w.credits.push(reversed_credit);
    w.journal.push(serde_json::from_value(json!({"id":"recognized","source_kind":"carrier_invoice","source_id":w.invoices[0].id,"posted_on":"2010-02-03","recorded_at":at,"actor":"fixture:finance","currency":"USD","memo":"Synthetic recognition","lines":[{"load_id":"load_6","account":"carrier_payable","debit_cents":0,"credit_cents":118000}]})).expect("journal"));
    w.journal.push(serde_json::from_value(json!({"id":"void","source_kind":"customer_invoice_void","source_id":"bill-a","posted_on":"2010-04-01","recorded_at":at,"actor":"fixture:finance","currency":"USD","memo":"Synthetic void history","lines":[]})).expect("journal"));
    w.carrier_adjustments.push(serde_json::from_value(json!({"id":"adjustment","load_id":"load_6","invoice_id":w.invoices[0].id,"carrier_id":"ca1","kind":"debit","number":"ADJUST","document_amount_cents":2500,"amount_cents":2500,"basis_total_cents":118000,"issued_on":"2010-05-01","description":"Synthetic adjustment","evidence_reference":"fixture","status":"posted","version":1,"created_at":at,"created_by":"fixture:finance","posted_at":at})).expect("adjustment"));
    w.carrier_credit_applications.push(serde_json::from_value(json!({"id":"application","source_settlement_id":"settlement-a","source_load_id":"load_6","carrier_id":"ca1","currency":"USD","amount_cents":1500,"applied_on":"2010-06-01","allocations":[{"target_id":"settlement-b","load_id":"load_7","amount_cents":1000},{"target_id":"settlement-a","load_id":"load_6","amount_cents":500}],"evidence_reference":"fixture","confirmation":{"counterparty_contact":"Synthetic carrier","confirmation_reference":"fixture","note":"Synthetic agreement","recorded_at":at,"recorded_by":"fixture:finance"},"recorded_at":at,"recorded_by":"fixture:finance","version":1})).expect("application"));
    w
}

fn assert_same(w: &Workspace) {
    let before = serde_json::to_vec(w).expect("originals");
    let index = FinanceReadIndex::new(w);
    for bill in &w.bills {
        assert_eq!(
            index.bill_totals(bill).map(|v| json!(v)),
            w.bill_totals(bill).map(|v| json!(v))
        );
        assert_eq!(
            index.customer_activity_date(bill),
            w.customer_activity_date(bill)
        );
    }
    for settlement in &w.settlements {
        assert_eq!(
            index.settlement_total(settlement),
            w.settlement_total(settlement)
        );
        assert_eq!(
            index.settlement_balance(&settlement.id),
            w.settlement_balance(&settlement.id)
        );
        assert_eq!(
            index.carrier_credit_effect(&settlement.id),
            w.carrier_credit_effect(&settlement.id)
        );
        assert_eq!(
            index.carrier_credit_earliest_date(&settlement.id),
            w.carrier_credit_earliest_date(&settlement.id)
        );
    }
    assert_eq!(index.summary(), w.finance_summary());
    assert_eq!(serde_json::to_vec(w).expect("originals"), before);
}

#[test]
fn indexed_finance_matches_cash_credit_adjustment_dates_and_summary() {
    let w = fixture();
    assert_same(&w);
    let index = FinanceReadIndex::new(&w);
    let totals = index.bill_totals(&w.bills[0]).expect("totals");
    assert_eq!(
        (
            totals.received_cents,
            totals.refunded_cents,
            totals.credited_cents,
            totals.balance_cents
        ),
        (20000, 5000, 10000, 125000)
    );
    assert_eq!(index.settlement_balance("settlement-a"), Ok(81500));
    assert_eq!(index.carrier_credit_effect("settlement-b"), Ok(-1000));
    assert_eq!(
        index.customer_activity_date(&w.bills[0]).to_string(),
        "2010-04-01"
    );
    assert_eq!(
        index
            .carrier_credit_earliest_date("settlement-a")
            .expect("date")
            .to_string(),
        "2010-05-01"
    );
    assert_eq!(
        index
            .carrier_credit_earliest_date("settlement-b")
            .expect("date"),
        chrono::Utc::now().date_naive()
    );
}

#[test]
fn index_rebuilds_after_reversal_void_draft_and_recognition_changes() {
    let mut w = fixture();
    w.bills[0].status = "void".into();
    w.bills[1].issued_at = None;
    w.carrier_adjustments[0].status = "reversed".into();
    w.carrier_adjustments[0].reversal = w.cash_entries[2].reversal.clone();
    w.carrier_credit_applications[0].reversal = w.cash_entries[2].reversal.clone();
    w.cash_entries[0].reversal = w.cash_entries[2].reversal.clone();
    w.journal[0].source_id = w.invoices[1].id.clone();
    assert_same(&w);
    assert_eq!(
        FinanceReadIndex::new(&w).carrier_credit_effect("settlement-a"),
        Ok(0)
    );
}

#[test]
fn first_matches_and_cancellation_obligations_keep_original_order() {
    let mut w = fixture();
    let mut duplicate = w.settlements[0].clone();
    duplicate.amount_cents = 1;
    duplicate.load_id = "missing".into();
    w.settlements.push(duplicate);
    let mut duplicate = w
        .loads
        .iter()
        .find(|v| v.id == "load_6")
        .expect("load")
        .clone();
    duplicate.origin.date = "2090-01-01".parse().expect("date");
    w.loads.push(duplicate);
    let mut invoice = w.invoices[0].clone();
    invoice.id = "later-approved".into();
    w.invoices.push(invoice);
    let mut fee:crate::cancellations::CancellationFee=serde_json::from_value(json!({"id":"fee","load_id":"load_7","side":"buy","party_id":"ca1","party_snapshot":{},"amount_cents":10000,"currency":"USD","description":"Synthetic cancellation","evidence_reference":"fixture","status":"posted","version":1,"created_at":"2010-01-01T00:00:00Z","created_by":"fixture:finance","target_id":"settlement-b","posted_at":"2010-07-01T00:00:00Z"})).expect("fee");
    w.cancellation_fees.push(fee.clone());
    fee.amount_cents = 2;
    w.cancellation_fees.push(fee);
    assert_same(&w);
    assert_eq!(
        FinanceReadIndex::new(&w).settlement_total(&w.settlements[1]),
        Ok(10000)
    );
    w.cancellation_fees[0].status = "reversed".into();
    assert_same(&w);
    assert_eq!(
        FinanceReadIndex::new(&w).settlement_total(&w.settlements[1]),
        Ok(0)
    );
}

#[test]
fn overflow_order_is_retained_and_unrelated_targets_stay_readable() {
    let mut w = fixture();
    let mut cash = w.cash_entries[0].clone();
    cash.allocations.truncate(1);
    w.cash_entries = [i64::MAX, 1, -i64::MAX]
        .into_iter()
        .map(|amount| {
            let mut v = cash.clone();
            v.allocations[0].amount_cents = amount;
            v
        })
        .collect();
    let index = FinanceReadIndex::new(&w);
    assert_eq!(
        index.bill_totals(&w.bills[0]).expect_err("overflow"),
        "Financial total overflow"
    );
    assert!(index.bill_totals(&w.bills[1]).is_ok());
    assert_same(&w);
    w.cash_entries.clear();
    let application = &mut w.carrier_credit_applications[0];
    application.allocations[0].amount_cents = i64::MAX;
    let mut duplicate = application.allocations[0].clone();
    duplicate.amount_cents = 1;
    application.allocations.push(duplicate);
    let index = FinanceReadIndex::new(&w);
    assert_eq!(
        index.carrier_credit_effect("settlement-b"),
        Err("Carrier credit total overflow".into())
    );
    assert_eq!(index.carrier_credit_effect("settlement-a"), Ok(1000));
    assert_same(&w);
}

#[test]
fn missing_sources_match_refusals_and_empty_summary_matches() {
    let empty = Workspace::default();
    assert_same(&empty);
    let mut w = fixture();
    w.loads.retain(|v| v.id != "load_6");
    assert_same(&w);
    assert_eq!(
        FinanceReadIndex::new(&w).carrier_credit_earliest_date("settlement-a"),
        Err("Load not found".into())
    );
    assert_eq!(
        FinanceReadIndex::new(&w).settlement_balance("missing"),
        Err("Settlement not found".into())
    );
}

#[test]
fn extreme_signed_values_refuse_without_panicking_or_wrapping() {
    let mut w = fixture();
    w.cash_entries.truncate(1);
    w.cash_entries[0].allocations.truncate(1);
    w.cash_entries[0].allocations[0].amount_cents = i64::MAX;
    w.credits.truncate(1);
    w.credits[0].amount_cents = 1;
    w.bills[0].snapshot.amount_cents = 0;
    assert_eq!(
        w.bill_totals(&w.bills[0])
            .expect_err("unrepresentable credit"),
        "Financial total overflow"
    );
    assert_same(&w);
    w.cash_entries[0].direction = "carrier_refund".into();
    w.cash_entries[0].allocations[0].target_id = "settlement-a".into();
    w.cash_entries[0].allocations[0].amount_cents = i64::MIN;
    assert_eq!(
        w.settlement_balance("settlement-a"),
        Err("Financial total overflow".into())
    );
    assert_same(&w);
    w.cash_entries.clear();
    w.credits.clear();
    w.journal[0].lines[0].debit_cents = i64::MAX;
    w.journal[0].lines[0].credit_cents = -1;
    assert_eq!(w.finance_summary(), Err("Financial total overflow".into()));
    assert_same(&w);
}
