//! Carrier-confirmed application of an existing credit to other prepared
//! payables. Reclassifies the same carrier's payable between loads; no cash or
//! expense/revenue is created by an application or its reversal.
use crate::{
    domain::{self, Result, Settlement, Workspace},
    finance::{CashAllocation, JournalLine, Reversal},
    payables::PayableDecision,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierCreditApplication {
    pub id: String,
    pub source_settlement_id: String,
    pub source_load_id: String,
    pub carrier_id: String,
    pub currency: String,
    pub amount_cents: i64,
    pub applied_on: NaiveDate,
    pub allocations: Vec<CashAllocation>,
    pub evidence_reference: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub confirmation: PayableDecision,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub version: u64,
    pub reversal: Option<Reversal>,
    pub reversal_confirmation: Option<PayableDecision>,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllocationInput {
    target_id: String,
    expected_version: u64,
    amount_cents: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    source_settlement_id: String,
    expected_source_version: u64,
    amount_cents: i64,
    applied_on: NaiveDate,
    allocations: Vec<AllocationInput>,
    evidence_reference: String,
    document_id: Option<String>,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
    corrects_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetVersion {
    target_id: String,
    expected_version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReverseInput {
    expected_version: u64,
    expected_source_version: u64,
    targets: Vec<TargetVersion>,
    reversed_on: NaiveDate,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid carrier credit application fields".into())
}
fn next(version: u64) -> Result<u64> {
    version
        .checked_add(1)
        .ok_or_else(|| "Carrier credit version exhausted".into())
}
fn version(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        Err("Carrier credit or settlement changed; refresh before continuing".into())
    } else {
        Ok(())
    }
}
fn confirmation(
    contact: String,
    reference: String,
    note: String,
    actor: &str,
) -> Result<PayableDecision> {
    for (value, label) in [
        (&contact, "Carrier contact"),
        (&reference, "Carrier confirmation reference"),
        (&note, "Carrier credit decision note"),
    ] {
        domain::required(value, label)?;
    }
    Ok(PayableDecision {
        counterparty_contact: contact.trim().into(),
        confirmation_reference: reference.trim().into(),
        note: note.trim().into(),
        recorded_at: Utc::now(),
        recorded_by: actor.into(),
    })
}
impl Workspace {
    pub fn carrier_credit_effect(&self, settlement_id: &str) -> Result<i64> {
        self.carrier_credit_applications
            .iter()
            .filter(|v| v.reversal.is_none())
            .try_fold(0i64, |total, v| {
                let incoming = v
                    .allocations
                    .iter()
                    .filter(|a| a.target_id == settlement_id)
                    .try_fold(0i64, |sum, a| {
                        sum.checked_add(a.amount_cents)
                            .ok_or("Carrier credit total overflow")
                    })?;
                total
                    .checked_add(if v.source_settlement_id == settlement_id {
                        v.amount_cents
                    } else {
                        0
                    })
                    .and_then(|sum| sum.checked_sub(incoming))
                    .ok_or_else(|| "Carrier credit total overflow".into())
            })
    }
    fn credit_settlement(&self, id: &str, expected: u64) -> Result<Settlement> {
        let settlement = self
            .settlements
            .iter()
            .find(|v| v.id == id)
            .ok_or("Settlement not found")?;
        version(settlement.version, expected)?;
        Ok(settlement.clone())
    }
    fn eligible_credit_settlement(&self, settlement: &Settlement) -> Result<()> {
        let load = self
            .loads
            .iter()
            .find(|v| v.id == settlement.load_id)
            .ok_or("Load not found")?;
        if let Some(fee) = self.cancellation_target(&settlement.id) {
            if fee.side == "buy"
                && fee.party_id == settlement.carrier_id
                && fee.posted_at.is_some()
                && matches!(fee.status.as_str(), "posted" | "reversed")
                && load.status == crate::domain::Status::Cancelled
                && !self.unresolved_tender_incidents(&load.id)
            {
                return Ok(());
            }
            return Err(
                "Cancellation credit target is not an eligible retained carrier obligation".into(),
            );
        }
        if load.carrier_id.as_deref() != Some(&settlement.carrier_id)
            || !self.delivery_evidence_ready(load)
            || self.release_pending(load)
            || !self
                .invoices
                .iter()
                .any(|v| v.load_id == load.id && v.status == "approved")
        {
            return Err("Credit application requires the current carrier, approved invoice, and reviewed delivery evidence without a release or custody hold".into());
        }
        Ok(())
    }
    // Applications use current balances, never reconstructed historical balances.
    // An earlier effective date cannot consume balances changed by later activity.
    pub fn carrier_credit_earliest_date(&self, settlement_id: &str) -> Result<NaiveDate> {
        let settlement = self
            .settlements
            .iter()
            .find(|v| v.id == settlement_id)
            .ok_or("Settlement not found")?;
        let load = self
            .loads
            .iter()
            .find(|v| v.id == settlement.load_id)
            .ok_or("Load not found")?;
        let mut earliest = self
            .cancellation_target(settlement_id)
            .and_then(|v| v.posted_at)
            .map_or(load.origin.date, |v| v.date_naive());
        if self.invoices.iter().any(|invoice| {
            invoice.load_id == load.id
                && invoice.status == "approved"
                && invoice.amount_cents > 0
                && !self.journal.iter().any(|entry| {
                    entry.source_kind == "carrier_invoice" && entry.source_id == invoice.id
                })
        }) {
            earliest = earliest.max(Utc::now().date_naive());
        }
        for entry in &self.journal {
            if entry
                .lines
                .iter()
                .any(|line| line.load_id == load.id && line.account == "carrier_payable")
            {
                earliest = earliest.max(entry.posted_on);
            }
        }
        for adjustment in self
            .carrier_adjustments
            .iter()
            .filter(|v| v.load_id == load.id && v.posted_at.is_some())
        {
            earliest = earliest.max(adjustment.issued_on);
            if let Some(reversal) = &adjustment.reversal {
                earliest = earliest.max(reversal.reversed_on);
            }
        }
        Ok(earliest)
    }
    fn credit_date(&self, settlement_id: &str, date: NaiveDate) -> Result<()> {
        if date < self.carrier_credit_earliest_date(settlement_id)?
            || date > Utc::now().date_naive()
        {
            return Err("Credit application/reversal date cannot precede existing payable activity or be in the future".into());
        }
        Ok(())
    }
    pub fn apply_carrier_credit(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        match operation {
            "carrier_credit.apply" => {
                let input: ApplyInput = decode(body)?;
                domain::money(input.amount_cents)?;
                if input.amount_cents == 0
                    || input.allocations.is_empty()
                    || input.allocations.len() > 100
                {
                    return Err(
                        "Positive carrier credit amount and 1–100 allocations required".into(),
                    );
                }
                domain::required(
                    &input.evidence_reference,
                    "Carrier credit application reference",
                )?;
                let reference = input.evidence_reference.trim().to_string();
                let proof = confirmation(
                    input.counterparty_contact,
                    input.confirmation_reference,
                    input.note,
                    actor,
                )?;
                let source = self.credit_settlement(
                    &input.source_settlement_id,
                    input.expected_source_version,
                )?;
                let currency = domain::currency_code(
                    &self
                        .loads
                        .iter()
                        .find(|v| v.id == source.load_id)
                        .ok_or("Carrier credit source load not found")?
                        .currency,
                )?;
                self.eligible_credit_settlement(&source)?;
                self.credit_date(&source.id, input.applied_on)?;
                if self
                    .settlement_balance(&source.id)?
                    .checked_neg()
                    .ok_or("Carrier credit total overflow")?
                    < input.amount_cents
                {
                    return Err("Application exceeds available carrier credit balance".into());
                }
                let duplicates: Vec<_> = self
                    .carrier_credit_applications
                    .iter()
                    .filter(|v| {
                        v.carrier_id == source.carrier_id
                            && v.evidence_reference.eq_ignore_ascii_case(&reference)
                    })
                    .collect();
                if duplicates.iter().any(|v| v.reversal.is_none()) {
                    return Err("Carrier credit reference already applied".into());
                }
                if let Some(id) = &input.corrects_id {
                    let prior = self
                        .carrier_credit_applications
                        .iter()
                        .find(|v| &v.id == id)
                        .ok_or("Corrected carrier credit application not found")?;
                    if prior.reversal.is_none()
                        || prior.corrected_by_id.is_some()
                        || prior.carrier_id != source.carrier_id
                        || !prior.evidence_reference.eq_ignore_ascii_case(&reference)
                    {
                        return Err("Correction requires the uncorrected reversed application for this exact carrier and reference".into());
                    }
                } else if !duplicates.is_empty() {
                    return Err(
                        "Reversed carrier credit reference requires an explicit correction link"
                            .into(),
                    );
                }
                let document_sha256 = if let Some(id) = &input.document_id {
                    let doc = self
                        .documents
                        .iter()
                        .find(|v| &v.id == id && v.load_id == source.load_id)
                        .ok_or("Carrier credit evidence document not found")?;
                    if doc.status() == "rejected" {
                        return Err("Carrier credit evidence document is rejected".into());
                    }
                    Some(doc.sha256.clone())
                } else {
                    None
                };
                let mut seen = BTreeSet::new();
                let mut allocated = 0i64;
                let mut allocations = vec![];
                let mut lines = vec![];
                for allocation in input.allocations {
                    domain::money(allocation.amount_cents)?;
                    if allocation.amount_cents == 0
                        || allocation.target_id == source.id
                        || !seen.insert(allocation.target_id.clone())
                    {
                        return Err("Credit allocations must be positive, unique, and exclude their source settlement".into());
                    }
                    let target =
                        self.credit_settlement(&allocation.target_id, allocation.expected_version)?;
                    if target.carrier_id != source.carrier_id {
                        return Err(
                            "Credit source and every target must belong to the same carrier".into(),
                        );
                    }
                    if self
                        .loads
                        .iter()
                        .find(|v| v.id == target.load_id)
                        .ok_or("Carrier credit target load not found")?
                        .currency
                        != currency
                    {
                        return Err("Credit source and every target must share one currency".into());
                    }
                    self.eligible_credit_settlement(&target)?;
                    self.credit_date(&target.id, input.applied_on)?;
                    if allocation.amount_cents > self.settlement_balance(&target.id)? {
                        return Err("Credit allocation exceeds outstanding target payable".into());
                    }
                    allocated = allocated
                        .checked_add(allocation.amount_cents)
                        .ok_or("Carrier credit total overflow")?;
                    lines.push(JournalLine {
                        load_id: target.load_id.clone(),
                        account: "carrier_payable".into(),
                        debit_cents: allocation.amount_cents,
                        credit_cents: 0,
                    });
                    allocations.push(CashAllocation {
                        target_id: target.id,
                        load_id: target.load_id,
                        amount_cents: allocation.amount_cents,
                    });
                }
                if allocated != input.amount_cents {
                    return Err(
                        "Credit allocations must exactly equal the application amount".into(),
                    );
                }
                lines.push(JournalLine {
                    load_id: source.load_id.clone(),
                    account: "carrier_payable".into(),
                    debit_cents: 0,
                    credit_cents: input.amount_cents,
                });
                let application = CarrierCreditApplication {
                    id: domain::id("carrier-credit"),
                    source_settlement_id: source.id.clone(),
                    source_load_id: source.load_id.clone(),
                    carrier_id: source.carrier_id,
                    currency: currency.clone(),
                    amount_cents: input.amount_cents,
                    applied_on: input.applied_on,
                    allocations,
                    evidence_reference: reference,
                    document_id: input.document_id,
                    document_sha256,
                    confirmation: proof,
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                    version: 1,
                    reversal: None,
                    reversal_confirmation: None,
                    corrects_id: input.corrects_id.clone(),
                    corrected_by_id: None,
                };
                for load_id in std::iter::once(&application.source_load_id)
                    .chain(application.allocations.iter().map(|v| &v.load_id))
                {
                    let invoice_id = self
                        .invoices
                        .iter()
                        .find(|v| v.load_id == *load_id && v.status == "approved")
                        .map(|v| v.id.clone());
                    if let Some(invoice_id) = invoice_id {
                        self.recognize_carrier_invoice(&invoice_id)?;
                    }
                }
                self.post(
                    ("carrier_credit_application", &application.id),
                    application.applied_on,
                    actor,
                    "Carrier-confirmed credit applied between payables",
                    &currency,
                    lines,
                    None,
                )?;
                if let Some(id) = input.corrects_id {
                    let prior = self
                        .carrier_credit_applications
                        .iter_mut()
                        .find(|v| v.id == id)
                        .ok_or("Corrected carrier credit application not found")?;
                    prior.corrected_by_id = Some(application.id.clone());
                    prior.version = next(prior.version)?;
                }
                self.carrier_credit_applications.push(application.clone());
                self.refresh_credit_application(&application, operation)?;
                Ok(json!(application))
            }
            "carrier_credit.reverse" => {
                let input: ReverseInput = decode(body)?;
                let index = self
                    .carrier_credit_applications
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Carrier credit application not found")?;
                let application = self.carrier_credit_applications[index].clone();
                version(application.version, input.expected_version)?;
                if application.reversal.is_some() {
                    return Err("Carrier credit application already reversed".into());
                }
                self.credit_settlement(
                    &application.source_settlement_id,
                    input.expected_source_version,
                )?;
                self.credit_date(&application.source_settlement_id, input.reversed_on)?;
                let expected: BTreeSet<_> = application
                    .allocations
                    .iter()
                    .map(|v| v.target_id.as_str())
                    .collect();
                let actual: BTreeSet<_> =
                    input.targets.iter().map(|v| v.target_id.as_str()).collect();
                if expected != actual || actual.len() != input.targets.len() {
                    return Err("Reversal requires the exact original allocation targets and current versions".into());
                }
                for target in input.targets {
                    self.credit_settlement(&target.target_id, target.expected_version)?;
                    self.credit_date(&target.target_id, input.reversed_on)?;
                }
                let proof = confirmation(
                    input.counterparty_contact,
                    input.confirmation_reference,
                    input.note,
                    actor,
                )?;
                let journal_id = self.reverse_post(
                    "carrier_credit_application",
                    entity,
                    "carrier_credit_application_reversal",
                    input.reversed_on,
                    actor,
                    &proof.note,
                )?;
                let current = &mut self.carrier_credit_applications[index];
                current.version = next(current.version)?;
                current.reversal = Some(Reversal {
                    note: proof.note.clone(),
                    reversed_on: input.reversed_on,
                    actor: actor.into(),
                    journal_id,
                });
                current.reversal_confirmation = Some(proof);
                let application = current.clone();
                self.refresh_credit_application(&application, operation)?;
                Ok(json!(application))
            }
            _ => Err("Unsupported carrier credit operation".into()),
        }
    }
    fn refresh_credit_application(
        &mut self,
        application: &CarrierCreditApplication,
        operation: &str,
    ) -> Result<()> {
        for (settlement_id, load_id) in std::iter::once((
            &application.source_settlement_id,
            &application.source_load_id,
        ))
        .chain(
            application
                .allocations
                .iter()
                .map(|v| (&v.target_id, &v.load_id)),
        ) {
            self.refresh_target("payout", settlement_id)?;
            self.record(
                load_id,
                operation,
                "Carrier credit allocation recorded; no bank movement or expense change",
            );
            if let Some(event) = self.events.last_mut() {
                event.details = Some(json!(application));
            }
        }
        Ok(())
    }
}
