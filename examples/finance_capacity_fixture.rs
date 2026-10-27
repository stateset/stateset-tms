//! Synthetic retained finance for resource diagnostics, never live accounting evidence.
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{
    documents::Review,
    domain::{self, Event, Status, Workspace},
};
use std::{
    collections::BTreeMap,
    io::{self, Read},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let mut value: Value = serde_json::from_str(&input)?;
    if let Some(format) = value
        .as_object_mut()
        .ok_or("Workspace object required")?
        .remove("_freight_format")
    {
        if format != json!(21) {
            return Err("Only storage format21 fixture inputs are supported".into());
        }
    }
    let mut w: Workspace = serde_json::from_value(value)?;
    assert!(
        !w.loads.is_empty() && w.invoices.is_empty() && w.bills.is_empty() && w.journal.is_empty()
    );
    let now = Utc::now();
    w.billing_profile = domain::demo().billing_profile;
    let mut expected = BTreeMap::<String, i64>::new();
    let mut completed = 0;
    for i in 0..w.loads.len() {
        if i % 4 == 0 {
            continue;
        }
        let load = &mut w.loads[i];
        load.status = Status::Delivered;
        load.exception = None;
        let reference = format!("SYNTHETIC-POD-{}", load.id);
        load.pod_reference = Some(reference.clone());
        load.pod_review = Some(Review {decision:"accepted".into(), note:"Generated synthetic external POD review; no real shipment or authenticity assertion".into(), reviewed_by:"synthetic:pod-reviewer".into(), reviewed_at:now, reference});
        let has_delivery_gates = w
            .stop_events
            .iter()
            .any(|v| v.load_id == load.id && v.stop == "delivery");
        for (kind, minutes) in [("arrival", 30), ("departure", 20)] {
            if has_delivery_gates {
                continue;
            }
            let mut gate = w
                .stop_events
                .iter()
                .find(|v| v.load_id == load.id)
                .expect("original gate")
                .clone();
            gate.id = format!("finance-delivery-{}-{kind}", load.id);
            gate.stop = "delivery".into();
            gate.kind = kind.into();
            gate.supersedes_id = None;
            gate.occurred_at = now - Duration::minutes(minutes);
            gate.recorded_at = gate.occurred_at;
            gate.local_time = gate.occurred_at.format("%Y-%m-%dT%H:%M:%S").to_string();
            w.stop_events.push(gate);
        }
        let id = load.id.clone();
        let buy = load.buy_cents;
        let sell = load.sell_cents;
        let invoice=w.apply("invoice.create","",json!({"load_id":id,"number":format!("SYN-INV-{i:05}"),"amount_cents":buy,"currency":"USD"}))?;
        let invoice_id = invoice["id"].as_str().ok_or("invoice id")?.to_owned();
        // Construct explicit synthetic retained decisions using the public current-source revision.
        // This is a fixture, not evidence of actual independent human/HTTP approval.
        let control = w.invoice_approval_control(&invoice_id, "synthetic:checker")?;
        let review_id = domain::id("fixture-review");
        w.events.push(Event {id:review_id.clone(),entity_id:invoice_id.clone(),kind:"invoice.entry_reviewed".into(),message:"Generated synthetic retained review".into(),occurred_at:now,actor:"synthetic:entry-reviewer".into(),details:Some(json!({"invoice_id":invoice_id,"revision":control["revision"],"evidence_reference":"Synthetic invoice record only","note":"Generated synthetic source comparison; not a human decision"}))});
        w.apply("invoice.approve", &invoice_id, json!({}))?;
        let decision = w
            .events
            .iter_mut()
            .rev()
            .find(|e| e.kind == "invoice.approve" && e.entity_id == invoice_id)
            .ok_or("decision")?;
        decision.actor = "synthetic:checker".into();
        decision.details = Some(
            json!({"revision":control["revision"],"entry_review_id":review_id,"note":"Generated distinct synthetic actor; not an independent operator outcome"}),
        );
        assert!(!w.invoice_approval_evidence(&invoice_id)?["approval_evidence"].is_null());
        let settlement = w.apply("settlement.create", &id, json!({}))?;
        let bill=w.apply_finance("bill.create","",json!({"load_id":id,"number":format!("SYN-BILL-{i:05}"),"due_date":now.date_naive()+Duration::days(30)}),"synthetic:finance")?;
        let bill_id = bill["id"].as_str().ok_or("bill id")?;
        let bill = w.apply_finance(
            "bill.issue",
            bill_id,
            json!({"expected_version":1}),
            "synthetic:finance",
        )?;
        let paid_buy = if i % 2 == 0 { buy / 2 } else { buy };
        let paid_sell = if i % 2 == 0 { sell / 2 } else { sell };
        for (direction, target, version, amount) in [
            (
                "receipt",
                bill["id"].clone(),
                bill["version"].clone(),
                paid_sell,
            ),
            (
                "payout",
                settlement["id"].clone(),
                settlement["version"].clone(),
                paid_buy,
            ),
        ] {
            w.apply_finance("cash.record","",json!({"direction":direction,"account":"Synthetic-USD","external_reference":format!("synthetic-{direction}-{i:05}"),"amount_cents":amount,"currency":"USD","cleared_on":now.date_naive(),"note":"Generated synthetic bank reconciliation; no bank transaction executed or verified","allocations":[{"target_id":target,"expected_version":version,"amount_cents":amount}]}),"synthetic:reconciler")?;
        }
        for (account, delta) in [
            ("freight_expense", buy),
            ("carrier_payable", paid_buy - buy),
            ("freight_revenue", -sell),
            ("accounts_receivable", sell - paid_sell),
            ("cash", paid_sell - paid_buy),
        ] {
            *expected.entry(account.into()).or_default() += delta;
        }
        completed += 1;
    }
    let mut actual = BTreeMap::<String, i64>::new();
    for entry in &w.journal {
        assert_eq!(
            entry.lines.iter().map(|v| v.debit_cents).sum::<i64>(),
            entry.lines.iter().map(|v| v.credit_cents).sum::<i64>()
        );
        for line in &entry.lines {
            *actual.entry(line.account.clone()).or_default() +=
                line.debit_cents - line.credit_cents;
        }
    }
    assert_eq!(actual, expected);
    assert_eq!(w.journal.len(), completed * 4);
    assert_eq!(w.cash_entries.len(), completed * 2);
    assert!(w.loads.iter().all(|l| w.execution_consistent(l)));
    for invoice in &w.invoices {
        assert!(!w.invoice_approval_evidence(&invoice.id)?["approval_evidence"].is_null());
    }
    w.reconcile_operations(now);
    w.verify_operations_cases()?;
    let summary: Value = w.finance_summary()?;
    eprintln!(
        "{}",
        json!({"scope":"Generated synthetic finance. No real operator approval, carrier invoice authenticity or bank outcome implied.","generated_at":now,"loads":w.loads.len(),"invoices":w.invoices.len(),"bills":w.bills.len(),"settlements":w.settlements.len(),"cash_entries":w.cash_entries.len(),"journal_entries":w.journal.len(),"journal_lines":w.journal.iter().map(|v|v.lines.len()).sum::<usize>(),"account_balances_cents":actual,"finance_summary":summary})
    );
    println!("{}", serde_json::to_string(&w)?);
    Ok(())
}
