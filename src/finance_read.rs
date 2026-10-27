//! Borrowed, response-local financial sources. Never persisted or reused after
//! a write: original record order, first-match selection and checked sums stay
//! authoritative. Construction does no amount arithmetic, so an unrelated
//! malformed target cannot change another target's read result.
use crate::{
    cancellations::CancellationFee,
    domain::{Invoice, Load, Result, Settlement, Workspace},
    finance::{BillTotals, CashAllocation, CashEntry, CreditNote, CustomerBill},
    payables::CarrierAdjustment,
};
use chrono::{NaiveDate, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

struct CreditEffect<'a> {
    outgoing: i64,
    incoming: Vec<&'a CashAllocation>,
}

/// A read-only index borrowing one immutable workspace snapshot. It contains
/// references and dates, not a second financial ledger or cached authority.
pub struct FinanceReadIndex<'a> {
    workspace: &'a Workspace,
    cash: BTreeMap<&'a str, Vec<(&'a CashEntry, &'a CashAllocation)>>,
    credits: BTreeMap<&'a str, Vec<&'a CreditNote>>,
    settlements: BTreeMap<&'a str, &'a Settlement>,
    loads: BTreeMap<&'a str, &'a Load>,
    approved: BTreeMap<&'a str, Vec<&'a Invoice>>,
    adjustments: BTreeMap<&'a str, Vec<&'a CarrierAdjustment>>,
    cancellations: BTreeMap<&'a str, &'a CancellationFee>,
    credit_effects: BTreeMap<&'a str, Vec<CreditEffect<'a>>>,
    recognized_invoices: BTreeSet<&'a str>,
    payable_dates: BTreeMap<&'a str, NaiveDate>,
    adjustment_dates: BTreeMap<&'a str, NaiveDate>,
    void_dates: BTreeMap<&'a str, NaiveDate>,
}

fn latest<'a>(dates: &mut BTreeMap<&'a str, NaiveDate>, id: &'a str, date: NaiveDate) {
    dates
        .entry(id)
        .and_modify(|v| *v = (*v).max(date))
        .or_insert(date);
}
fn sum(values: impl IntoIterator<Item = i64>, error: &'static str) -> Result<i64> {
    values
        .into_iter()
        .try_fold(0i64, |a, b| a.checked_add(b).ok_or_else(|| error.into()))
}

impl<'a> FinanceReadIndex<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        let mut index = Self {
            workspace,
            cash: BTreeMap::new(),
            credits: BTreeMap::new(),
            settlements: BTreeMap::new(),
            loads: BTreeMap::new(),
            approved: BTreeMap::new(),
            adjustments: BTreeMap::new(),
            cancellations: BTreeMap::new(),
            credit_effects: BTreeMap::new(),
            recognized_invoices: BTreeSet::new(),
            payable_dates: BTreeMap::new(),
            adjustment_dates: BTreeMap::new(),
            void_dates: BTreeMap::new(),
        };
        for entry in &workspace.cash_entries {
            for allocation in &entry.allocations {
                index
                    .cash
                    .entry(&allocation.target_id)
                    .or_default()
                    .push((entry, allocation));
            }
        }
        for credit in &workspace.credits {
            index
                .credits
                .entry(&credit.bill_id)
                .or_default()
                .push(credit);
        }
        for settlement in &workspace.settlements {
            index
                .settlements
                .entry(&settlement.id)
                .or_insert(settlement);
        }
        for load in &workspace.loads {
            index.loads.entry(&load.id).or_insert(load);
        }
        for invoice in workspace.invoices.iter().filter(|v| v.status == "approved") {
            index
                .approved
                .entry(&invoice.load_id)
                .or_default()
                .push(invoice);
        }
        for adjustment in &workspace.carrier_adjustments {
            if adjustment.status == "posted" {
                index
                    .adjustments
                    .entry(&adjustment.invoice_id)
                    .or_default()
                    .push(adjustment);
            }
            if adjustment.posted_at.is_some() {
                let date = adjustment
                    .reversal
                    .as_ref()
                    .map_or(adjustment.issued_on, |r| {
                        r.reversed_on.max(adjustment.issued_on)
                    });
                latest(&mut index.adjustment_dates, &adjustment.load_id, date);
            }
        }
        for fee in &workspace.cancellation_fees {
            if let Some(target) = &fee.target_id {
                index.cancellations.entry(target).or_insert(fee);
            }
        }
        for application in workspace
            .carrier_credit_applications
            .iter()
            .filter(|v| v.reversal.is_none())
        {
            // One entry per application/target, including a self-allocation.
            // Preserve the inner incoming sum, then outgoing addition, then
            // incoming subtraction; regrouping signed totals changes overflow.
            let mut targets: BTreeMap<&str, Vec<&CashAllocation>> = BTreeMap::new();
            targets
                .entry(&application.source_settlement_id)
                .or_default();
            for allocation in &application.allocations {
                targets
                    .entry(&allocation.target_id)
                    .or_default()
                    .push(allocation);
            }
            for (target, incoming) in targets {
                index
                    .credit_effects
                    .entry(target)
                    .or_default()
                    .push(CreditEffect {
                        outgoing: if target == application.source_settlement_id {
                            application.amount_cents
                        } else {
                            0
                        },
                        incoming,
                    });
            }
        }
        for entry in &workspace.journal {
            if entry.source_kind == "carrier_invoice" {
                index.recognized_invoices.insert(&entry.source_id);
            }
            if entry.source_kind == "customer_invoice_void" {
                latest(&mut index.void_dates, &entry.source_id, entry.posted_on);
            }
            for line in entry
                .lines
                .iter()
                .filter(|v| v.account == "carrier_payable")
            {
                latest(&mut index.payable_dates, &line.load_id, entry.posted_on);
            }
        }
        index
    }

    pub fn bill_totals(&self, bill: &CustomerBill) -> Result<BillTotals> {
        let cash = self.cash.get(bill.id.as_str()).into_iter().flatten();
        let received = sum(
            cash.clone()
                .filter(|(v, _)| v.direction == "receipt" && v.reversal.is_none())
                .map(|(_, a)| a.amount_cents),
            "Financial total overflow",
        )?;
        let credited = sum(
            self.credits
                .get(bill.id.as_str())
                .into_iter()
                .flatten()
                .filter(|v| v.reversal.is_none())
                .map(|v| v.amount_cents),
            "Financial total overflow",
        )?;
        let refunded = sum(
            cash.filter(|(v, _)| v.direction == "customer_refund" && v.reversal.is_none())
                .map(|(_, a)| a.amount_cents),
            "Financial total overflow",
        )?;
        BillTotals::from_activity(bill, received, credited, refunded)
    }

    pub fn settlement_total(&self, settlement: &Settlement) -> Result<i64> {
        if let Some(fee) = self.cancellations.get(settlement.id.as_str()) {
            return Ok(if fee.status == "posted" {
                fee.amount_cents
            } else {
                0
            });
        }
        let invoice = self
            .approved
            .get(settlement.load_id.as_str())
            .and_then(|v| v.first());
        let adjustments = invoice.and_then(|v| self.adjustments.get(v.id.as_str()));
        adjustments
            .into_iter()
            .flatten()
            .try_fold(settlement.amount_cents, |total, a| {
                total
                    .checked_add(a.amount_cents)
                    .ok_or_else(|| "Carrier settlement total overflow".into())
            })
    }

    pub fn carrier_credit_effect(&self, id: &str) -> Result<i64> {
        self.credit_effects
            .get(id)
            .into_iter()
            .flatten()
            .try_fold(0i64, |total, effect| {
                let incoming = sum(
                    effect.incoming.iter().map(|a| a.amount_cents),
                    "Carrier credit total overflow",
                )?;
                total
                    .checked_add(effect.outgoing)
                    .and_then(|v| v.checked_sub(incoming))
                    .ok_or_else(|| "Carrier credit total overflow".into())
            })
    }

    pub fn settlement_balance(&self, id: &str) -> Result<i64> {
        let settlement = self.settlements.get(id).ok_or("Settlement not found")?;
        let reconciled = crate::finance::reconciled_cash(
            self.cash
                .get(id)
                .into_iter()
                .flatten()
                .map(|(entry, allocation)| (*entry, *allocation)),
        )?;
        let effect = self.carrier_credit_effect(id)?;
        self.settlement_total(settlement)?
            .checked_sub(reconciled)
            .and_then(|v| v.checked_add(effect))
            .ok_or_else(|| "Financial total overflow".into())
    }

    pub fn carrier_credit_earliest_date(&self, id: &str) -> Result<NaiveDate> {
        let settlement = self.settlements.get(id).ok_or("Settlement not found")?;
        let load = self
            .loads
            .get(settlement.load_id.as_str())
            .ok_or("Load not found")?;
        let mut date = self
            .cancellations
            .get(id)
            .and_then(|v| v.posted_at)
            .map_or(load.origin.date, |v| v.date_naive());
        if self
            .approved
            .get(load.id.as_str())
            .into_iter()
            .flatten()
            .any(|v| v.amount_cents > 0 && !self.recognized_invoices.contains(v.id.as_str()))
        {
            date = date.max(Utc::now().date_naive());
        }
        for dates in [&self.payable_dates, &self.adjustment_dates] {
            if let Some(activity) = dates.get(load.id.as_str()) {
                date = date.max(*activity);
            }
        }
        Ok(date)
    }

    pub fn customer_activity_date(&self, bill: &CustomerBill) -> NaiveDate {
        let credits = self
            .credits
            .get(bill.id.as_str())
            .into_iter()
            .flatten()
            .flat_map(|v| {
                std::iter::once(v.issued_on.unwrap_or(v.created_at.date_naive()))
                    .chain(v.reversal.as_ref().map(|r| r.reversed_on))
            });
        let cash = self
            .cash
            .get(bill.id.as_str())
            .into_iter()
            .flatten()
            .flat_map(|(v, _)| {
                std::iter::once(v.cleared_on).chain(v.reversal.as_ref().map(|r| r.reversed_on))
            });
        credits
            .chain(cash)
            .chain(self.void_dates.get(bill.id.as_str()).copied())
            .chain(
                self.cancellations
                    .get(bill.id.as_str())
                    .and_then(|v| v.reversal.as_ref())
                    .map(|v| v.reversed_on),
            )
            .max()
            .unwrap_or(bill.issued_at.unwrap_or(bill.created_at).date_naive())
    }

    pub fn summary(&self) -> Result<Value> {
        let totals = self
            .workspace
            .bills
            .iter()
            .map(|v| self.bill_totals(v))
            .collect::<Result<Vec<_>>>()?;
        let balances: Vec<_> = self
            .workspace
            .bills
            .iter()
            .zip(totals.iter())
            .map(|(bill, total)| (bill.snapshot.currency.as_str(), total))
            .collect();
        let payable = self
            .workspace
            .settlements
            .iter()
            .map(|v| {
                let currency = self
                    .loads
                    .get(v.load_id.as_str())
                    .ok_or("Settlement load not found")?
                    .currency
                    .as_str();
                Ok((currency, self.settlement_balance(&v.id)?))
            })
            .collect::<Result<Vec<_>>>()?;
        crate::finance::summary_value(&self.workspace.journal, balances, payable)
    }
}
