//! Matched financial projection CPU work on one immutable synthetic fixture.
//! Includes index construction and JSON output; excludes decode/SQL/HTTP.
use serde_json::{json, Value};
use stateset_freight::{domain::Workspace, finance::FinanceReadIndex};
use std::{hint::black_box, path::PathBuf, time::Instant};

fn project(w: &Workspace, indexed: bool) -> Result<Value, String> {
    let index = indexed.then(|| FinanceReadIndex::new(w));
    let bills = w
        .bills
        .iter()
        .map(|bill| {
            let totals = match &index {
                Some(v) => v.bill_totals(bill),
                None => w.bill_totals(bill),
            }?;
            let activity = match &index {
                Some(v) => v.customer_activity_date(bill),
                None => w.customer_activity_date(bill),
            };
            Ok(json!({"id":bill.id,"totals":totals,"activity_date":activity}))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let settlements = w.settlements.iter().map(|s| {
        let (balance,total,effect,date) = match &index {
            Some(v) => (v.settlement_balance(&s.id)?,v.settlement_total(s)?,v.carrier_credit_effect(&s.id)?,v.carrier_credit_earliest_date(&s.id)?),
            None => (w.settlement_balance(&s.id)?,w.settlement_total(s)?,w.carrier_credit_effect(&s.id)?,w.carrier_credit_earliest_date(&s.id)?),
        };
        Ok(json!({"id":s.id,"balance_cents":balance,"total_amount_cents":total,"credit_balance_cents":(-balance).max(0),"credit_application_cents":effect,"credit_application_earliest_date":date}))
    }).collect::<Result<Vec<_>,String>>()?;
    let summary = match &index {
        Some(v) => v.summary(),
        None => w.finance_summary(),
    }?;
    Ok(json!({"bills":bills,"settlements":settlements,"finance":summary}))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = PathBuf::from(std::env::args().nth(1).ok_or("Fixture path required")?);
    let samples: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "9".into())
        .parse()?;
    if !(3..=100).contains(&samples) {
        return Err("Choose 3–100 samples".into());
    }
    let raw = std::fs::read(&path)?;
    let mut value: Value = serde_json::from_slice(&raw)?;
    value
        .as_object_mut()
        .ok_or("Workspace object required")?
        .remove("_freight_format");
    let w: Workspace = serde_json::from_value(value)?;
    let original = stateset_freight::documents::sha256(&serde_json::to_vec(&w)?);
    let expected = project(&w, false)?;
    assert_eq!(expected, project(&w, true)?);
    let mut legacy = Vec::new();
    let mut indexed = Vec::new();
    for n in 0..samples {
        for is_indexed in if n % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let result = project(black_box(&w), is_indexed)?;
            let encoded = serde_json::to_vec(&result)?;
            black_box(&encoded);
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(expected, result);
            if is_indexed {
                indexed.push(elapsed);
            } else {
                legacy.push(elapsed);
            }
        }
    }
    assert_eq!(
        original,
        stateset_freight::documents::sha256(&serde_json::to_vec(&w)?)
    );
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"recorded_utc":chrono::Utc::now(),"scope":"Finite interleaved release-binary financial projection on one synthetic immutable workspace. Includes index construction and JSON encoding. Excludes decode, SQL, HTTP, all other projection work and process memory. No production capacity, latency SLO or A+ claim.","package_version":env!("CARGO_PKG_VERSION"),"fixture_sha256":stateset_freight::documents::sha256(&raw),"loads":w.loads.len(),"bills":w.bills.len(),"settlements":w.settlements.len(),"cash_entries":w.cash_entries.len(),"journal_entries":w.journal.len(),"samples_per_path":samples,"legacy_ms":legacy,"indexed_ms":indexed,"legacy_median_ms":median(&legacy),"indexed_median_ms":median(&indexed),"speedup":median(&legacy)/median(&indexed),"complete_financial_outputs_equal":true,"original_workspace_unchanged":true,"output_sha256":stateset_freight::documents::sha256(&serde_json::to_vec(&expected)?)})
        )?
    );
    Ok(())
}
