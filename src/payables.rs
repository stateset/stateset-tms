//! Append-only changes to an approved carrier payable. Original invoices,
//! settlement principal, and reconciled bank entries remain intact.
use crate::{
    domain::{self, EdiEvent, Load, Result, Settlement, Workspace},
    finance::{JournalLine, Reversal},
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierDocument {
    pub id: String,
    pub load_id: String,
    pub carrier_id: String,
    pub tender_id: Option<String>,
    pub number: String,
    pub amount_cents: i64,
    pub currency: String,
    pub event: EdiEvent,
    pub status: String,
    pub version: u64,
    pub received_at: DateTime<Utc>,
    pub adjustment_id: Option<String>,
    pub resolution: Option<PayableDecision>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierAdjustment {
    pub id: String,
    pub load_id: String,
    pub invoice_id: String,
    pub carrier_id: String,
    pub tender_id: Option<String>,
    pub kind: String,
    pub number: String,
    pub document_amount_cents: i64,
    pub amount_cents: i64,
    pub basis_total_cents: i64,
    pub issued_on: NaiveDate,
    pub description: String,
    pub evidence_reference: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub carrier_document_id: Option<String>,
    pub status: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub decision: Option<PayableDecision>,
    pub reversal: Option<Reversal>,
    pub posted_at: Option<DateTime<Utc>>,
    pub reversal_decision: Option<PayableDecision>,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayableDecision {
    pub note: String,
    pub counterparty_contact: String,
    pub confirmation_reference: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    expected_load_version: u64,
    invoice_id: String,
    kind: String,
    number: String,
    document_amount_cents: i64,
    issued_on: NaiveDate,
    description: String,
    evidence_reference: String,
    document_id: Option<String>,
    carrier_document_id: Option<String>,
    corrects_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    expected_load_version: u64,
    expected_version: u64,
    decision: String,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reverse {
    expected_load_version: u64,
    expected_version: u64,
    reversed_on: NaiveDate,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentDecision {
    expected_load_version: u64,
    expected_version: u64,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid carrier payable request fields".into())
}
fn bump(load: &mut Load, expected: u64) -> Result<()> {
    if load.version != expected {
        return Err("Load changed; refresh before continuing".into());
    }
    load.version = next(load.version)?;
    load.updated_at = Utc::now();
    Ok(())
}
fn next(version: u64) -> Result<u64> {
    version
        .checked_add(1)
        .ok_or_else(|| "Carrier payable version exhausted".into())
}
fn number(value: &str) -> Result<String> {
    domain::required(value, "Carrier document number")?;
    if value.chars().any(char::is_control) {
        return Err("Carrier document number cannot contain control characters".into());
    }
    Ok(value.trim().to_ascii_uppercase())
}
fn decision(
    contact: String,
    reference: String,
    note: String,
    actor: &str,
) -> Result<PayableDecision> {
    for (v, f) in [
        (&contact, "Counterparty contact"),
        (&reference, "Carrier confirmation reference"),
        (&note, "Payable review note"),
    ] {
        domain::required(v, f)?;
    }
    Ok(PayableDecision {
        note,
        counterparty_contact: contact,
        confirmation_reference: reference,
        recorded_at: Utc::now(),
        recorded_by: actor.into(),
    })
}
impl Workspace {
    pub fn carrier_invoice_total(&self, invoice_id: &str) -> Result<i64> {
        let invoice = self
            .invoices
            .iter()
            .find(|v| v.id == invoice_id)
            .ok_or("Invoice not found")?;
        let total = self
            .carrier_adjustments
            .iter()
            .filter(|v| v.invoice_id == invoice_id && v.status == "posted")
            .try_fold(invoice.amount_cents, |a, v| {
                a.checked_add(v.amount_cents)
                    .ok_or("Carrier payable total overflow")
            })?;
        domain::money(total)?;
        Ok(total)
    }
    pub fn settlement_total(&self, settlement: &Settlement) -> Result<i64> {
        if let Some(fee) = self.cancellation_target(&settlement.id) {
            return Ok(if fee.status == "posted" {
                fee.amount_cents
            } else {
                0
            });
        }
        let invoice = self
            .invoices
            .iter()
            .find(|v| v.load_id == settlement.load_id && v.status == "approved");
        match invoice {
            Some(v) => self
                .carrier_adjustments
                .iter()
                .filter(|a| a.invoice_id == v.id && a.status == "posted")
                .try_fold(settlement.amount_cents, |total, a| {
                    total
                        .checked_add(a.amount_cents)
                        .ok_or_else(|| "Carrier settlement total overflow".into())
                }),
            None => Ok(settlement.amount_cents),
        }
    }
    pub fn refresh_carrier_settlement(&mut self, id: &str) -> Result<()> {
        let load = self
            .settlements
            .iter()
            .find(|v| v.id == id)
            .ok_or("Settlement not found")?
            .load_id
            .clone();
        if self
            .carrier_adjustments
            .iter()
            .any(|v| v.load_id == load && v.status == "posted")
        {
            self.refresh_target("payout", id)?;
        }
        Ok(())
    }
    pub fn carrier_refund_start_date(&self, load_id: &str) -> Option<NaiveDate> {
        self.carrier_adjustments
            .iter()
            .filter(|v| v.load_id == load_id)
            .filter_map(|v| {
                if v.status == "posted" && v.amount_cents < 0 {
                    Some(v.issued_on)
                } else if v.status == "reversed" && v.amount_cents > 0 {
                    v.reversal.as_ref().map(|r| r.reversed_on)
                } else {
                    None
                }
            })
            .min()
    }
    pub fn carrier_adjustment_total(&self, load_id: &str) -> Result<i64> {
        self.carrier_adjustments
            .iter()
            .filter(|v| v.load_id == load_id && v.status == "posted")
            .try_fold(0i64, |a, v| {
                a.checked_add(v.amount_cents)
                    .ok_or_else(|| "Carrier adjustment total overflow".into())
            })
    }
    pub fn receive_additional_carrier_document(
        &mut self,
        load: &Load,
        event: &EdiEvent,
        invoice_number: &str,
        amount: i64,
        currency: &str,
    ) -> Result<Value> {
        domain::required(invoice_number, "Carrier invoice number")?;
        domain::money(amount.checked_abs().ok_or("Carrier amount overflow")?)?;
        if currency != load.currency {
            return Err("Invoice currency must match load currency".into());
        }
        if self.invoices.iter().any(|v| {
            v.load_id == load.id
                && v.number.eq_ignore_ascii_case(invoice_number)
                && v.amount_cents == amount
                && self
                    .carrier_invoice_total(&v.id)
                    .is_ok_and(|total| total == amount)
        }) {
            self.record(&load.id,"carrier_document.duplicate",format!("Carrier invoice {invoice_number} repeats the primary amount; no additional payable"));
            return Ok(
                json!({"ignored":true,"reason":"duplicate_carrier_invoice","load_id":load.id}),
            );
        }
        if let Some(prior) = self
            .carrier_documents
            .iter()
            .find(|v| v.event.event_id == event.event_id)
        {
            return Ok(
                json!({"finance_review":true,"carrier_document_id":prior.id,"load_id":load.id}),
            );
        }
        let document = CarrierDocument {
            id: domain::id("carrier-doc"),
            load_id: load.id.clone(),
            carrier_id: load.carrier_id.clone().ok_or("Carrier required")?,
            tender_id: load.tender_id.clone(),
            number: invoice_number.into(),
            amount_cents: amount,
            currency: currency.into(),
            event: event.clone(),
            status: "pending".into(),
            version: 1,
            received_at: Utc::now(),
            adjustment_id: None,
            resolution: None,
        };
        let result =
            json!({"finance_review":true,"carrier_document_id":document.id,"load_id":load.id});
        self.carrier_documents.push(document);
        self.record(&load.id,"carrier_document.received",format!("Additional 210 {invoice_number} retained for finance classification; amount {amount} cents was not posted"));
        Ok(result)
    }
    fn finish_carrier_document(
        &mut self,
        source: &str,
        status: &str,
        proof: PayableDecision,
    ) -> Result<()> {
        let doc = self
            .carrier_documents
            .iter_mut()
            .find(|v| v.id == source)
            .ok_or("Carrier document not found")?;
        doc.status = status.into();
        doc.version = next(doc.version)?;
        doc.resolution = Some(proof);
        let event_id = doc.event.event_id.clone();
        for item in &mut self.inbox {
            if item.status == "finance_review"
                && item
                    .normalized_event
                    .as_ref()
                    .is_some_and(|v| v["event_id"] == event_id)
            {
                item.status = "finance_reviewed".into();
                item.updated_at = Utc::now();
            }
        }
        Ok(())
    }
    pub fn apply_payable(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let load_id = if operation == "payable.create" {
            entity.into()
        } else if operation.starts_with("payable.document.") {
            self.carrier_documents
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.clone())
                .ok_or("Carrier document not found")?
        } else {
            self.carrier_adjustments
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.clone())
                .ok_or("Carrier adjustment not found")?
        };
        let index = self
            .loads
            .iter()
            .position(|v| v.id == load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        let result;
        if operation == "payable.create" {
            let input: Draft = decode(body)?;
            let invoice = self
                .invoices
                .iter()
                .find(|v| {
                    v.id == input.invoice_id && v.load_id == load.id && v.status == "approved"
                })
                .ok_or("Approved carrier invoice not found")?
                .clone();
            if load.carrier_id.is_some() {
                self.adopt_tender(&mut load)?;
            }
            let carrier = load.carrier_id.clone().ok_or("Carrier required")?;
            let num = number(&input.number)?;
            domain::money(input.document_amount_cents)?;
            if input.document_amount_cents == 0 && input.kind != "invoice_revision" {
                return Err("Carrier document amount must be positive".into());
            }
            if input.issued_on > Utc::now().date_naive() || input.issued_on.year() < 1900 {
                return Err(
                    "Carrier document date must be valid and cannot be in the future".into(),
                );
            }
            domain::required(&input.description, "Adjustment description")?;
            domain::required(&input.evidence_reference, "Adjustment evidence reference")?;
            let basis = self.carrier_invoice_total(&invoice.id)?;
            let amount=match input.kind.as_str(){"supplemental_invoice"=>input.document_amount_cents,"credit_memo"=>-input.document_amount_cents,"invoice_revision"=>input.document_amount_cents-basis,_=>return Err("Adjustment kind must be supplemental_invoice, credit_memo, or invoice_revision".into())};
            if amount == 0 {
                return Err("Carrier document produces no change to the recognized payable".into());
            }
            domain::money(basis + amount)?;
            if self.cancellation_invoice_number_exists(&carrier, &num, None) {
                return Err(
                    "Carrier document number already records a cancellation obligation".into(),
                );
            }
            if input.kind == "supplemental_invoice"
                && self.invoices.iter().any(|v| {
                    v.number.eq_ignore_ascii_case(&num)
                        && self
                            .loads
                            .iter()
                            .any(|l| l.id == v.load_id && l.carrier_id.as_ref() == Some(&carrier))
                })
            {
                return Err("Carrier document number already belongs to a primary invoice".into());
            }
            let reference = input.evidence_reference.trim().to_owned();
            if input.kind == "supplemental_invoice"
                && self.carrier_adjustments.iter().any(|v| {
                    v.carrier_id == carrier
                        && v.kind == "invoice_revision"
                        && v.number.eq_ignore_ascii_case(&num)
                        && ["draft", "posted"].contains(&v.status.as_str())
                })
            {
                return Err("Carrier number already classified as a revised total; it cannot also be added as supplemental cost".into());
            }
            let matching: Vec<_> = self
                .carrier_adjustments
                .iter()
                .filter(|v| {
                    v.carrier_id == carrier
                        && v.kind == input.kind
                        && (if input.kind == "invoice_revision" {
                            v.invoice_id == invoice.id
                                && v.evidence_reference.eq_ignore_ascii_case(&reference)
                        } else {
                            v.number.eq_ignore_ascii_case(&num)
                        })
                })
                .collect();
            if matching
                .iter()
                .any(|v| ["draft", "posted"].contains(&v.status.as_str()))
            {
                return Err(
                    "Carrier financial document already has an active draft or posting".into(),
                );
            }
            if let Some(corrects) = &input.corrects_id {
                let old = self
                    .carrier_adjustments
                    .iter()
                    .find(|v| &v.id == corrects)
                    .ok_or("Correction must identify the original carrier financial document")?;
                if old.invoice_id != invoice.id
                    || old.carrier_id != carrier
                    || old.tender_id != load.tender_id
                    || old.carrier_document_id != input.carrier_document_id
                    || old.number != num
                    || old.evidence_reference != reference
                    || old.corrected_by_id.is_some()
                    || !["rejected", "reversed"].contains(&old.status.as_str())
                {
                    return Err("Correction must replace one uncorrected rejected or reversed carrier adjustment".into());
                }
            } else if !matching.is_empty() {
                return Err("A previously rejected or reversed financial document requires an explicit correction".into());
            }
            let digest = if let Some(id) = &input.document_id {
                let doc = self
                    .documents
                    .iter()
                    .find(|v| &v.id == id && v.load_id == load.id && v.status() != "rejected")
                    .ok_or("Supporting document not found")?;
                Some(doc.sha256.clone())
            } else {
                None
            };
            if let Some(id) = &input.carrier_document_id {
                let source = self
                    .carrier_documents
                    .iter()
                    .find(|v| {
                        &v.id == id
                            && v.load_id == load.id
                            && v.carrier_id == carrier
                            && v.tender_id == load.tender_id
                    })
                    .ok_or("Carrier source document not found")?;
                if input.kind == "credit_memo" && source.amount_cents >= 0 {
                    return Err("A positive 210 header does not establish a credit memo; inspect an actual carrier credit document".into());
                }
                if source.amount_cents < 0 && input.kind != "credit_memo" {
                    return Err("A negative carrier total requires explicit verified credit-memo classification".into());
                }
                if !source.number.eq_ignore_ascii_case(&num)
                    || source.amount_cents.checked_abs() != Some(input.document_amount_cents)
                    || source.currency != load.currency
                {
                    return Err("Adjustment must preserve the canonical carrier document number, total, and currency".into());
                }
                if source.event.payload["invoice_date"]
                    .as_str()
                    .is_some_and(|v| v != input.issued_on.to_string())
                {
                    return Err("Use the canonical carrier document date".into());
                }
                if source.status != "pending"
                    && (input.corrects_id.is_none()
                        || source.adjustment_id.as_ref() != input.corrects_id.as_ref())
                {
                    return Err("Carrier document already allocated or resolved".into());
                }
            }
            bump(&mut load, input.expected_load_version)?;
            let adjustment = CarrierAdjustment {
                id: domain::id("payable"),
                load_id: load.id.clone(),
                invoice_id: invoice.id,
                carrier_id: carrier,
                tender_id: load.tender_id.clone(),
                kind: input.kind,
                number: num,
                document_amount_cents: input.document_amount_cents,
                amount_cents: amount,
                basis_total_cents: basis,
                issued_on: input.issued_on,
                description: input.description,
                evidence_reference: reference,
                document_id: input.document_id,
                document_sha256: digest,
                carrier_document_id: input.carrier_document_id,
                status: "draft".into(),
                version: 1,
                created_at: Utc::now(),
                created_by: actor.into(),
                decision: None,
                reversal: None,
                reversal_decision: None,
                posted_at: None,
                corrects_id: input.corrects_id,
                corrected_by_id: None,
            };
            if let Some(id) = &adjustment.corrects_id {
                let prior = self
                    .carrier_adjustments
                    .iter_mut()
                    .find(|v| &v.id == id)
                    .ok_or("Carrier adjustment not found")?;
                prior.corrected_by_id = Some(adjustment.id.clone());
                prior.version = next(prior.version)?;
            }
            if let Some(id) = &adjustment.carrier_document_id {
                let source = self
                    .carrier_documents
                    .iter_mut()
                    .find(|v| &v.id == id)
                    .ok_or("Carrier document not found")?;
                source.status = "allocated".into();
                source.adjustment_id = Some(adjustment.id.clone());
                source.version = next(source.version)?;
                let event_id = source.event.event_id.clone();
                for item in &mut self.inbox {
                    if item.status == "finance_reviewed"
                        && item
                            .normalized_event
                            .as_ref()
                            .is_some_and(|v| v["event_id"] == event_id)
                    {
                        item.status = "finance_review".into();
                        item.updated_at = Utc::now();
                    }
                }
            }
            result = json!(adjustment);
            self.carrier_adjustments.push(adjustment);
        } else if operation == "payable.review" || operation == "payable.reverse" {
            let a = self
                .carrier_adjustments
                .iter()
                .position(|v| v.id == entity)
                .ok_or("Carrier adjustment not found")?;
            let original = self.carrier_adjustments[a].clone();
            let (expected_load, expected, approved, proof, reversed_on) =
                if operation == "payable.review" {
                    let input: Decision = decode(body)?;
                    if !["approved", "rejected"].contains(&input.decision.as_str()) {
                        return Err("Payable decision must be approved or rejected".into());
                    }
                    (
                        input.expected_load_version,
                        input.expected_version,
                        input.decision == "approved",
                        decision(
                            input.counterparty_contact,
                            input.confirmation_reference,
                            input.note,
                            actor,
                        )?,
                        None,
                    )
                } else {
                    let input: Reverse = decode(body)?;
                    (
                        input.expected_load_version,
                        input.expected_version,
                        false,
                        decision(
                            input.counterparty_contact,
                            input.confirmation_reference,
                            input.note,
                            actor,
                        )?,
                        Some(input.reversed_on),
                    )
                };
            if original.version != expected {
                return Err("Carrier adjustment changed; refresh before continuing".into());
            }
            bump(&mut load, expected_load)?;
            if (reversed_on.is_none() && original.status != "draft")
                || (reversed_on.is_some() && original.status != "posted")
            {
                return Err("Carrier adjustment is not in the required state".into());
            }
            if approved {
                if load.carrier_id.as_ref() != Some(&original.carrier_id)
                    || load.tender_id != original.tender_id
                    || !self.delivery_evidence_ready(&load)
                    || self.release_pending(&load)
                {
                    return Err("Current carrier ownership and reviewed delivery evidence required for payable posting".into());
                }
                let basis = self.carrier_invoice_total(&original.invoice_id)?;
                if original.kind == "invoice_revision" && basis != original.basis_total_cents {
                    return Err("Recognized invoice total changed; reject and correct the revision draft before posting".into());
                }
                domain::money(basis + original.amount_cents)?;
                if let Some(id) = &original.document_id {
                    let doc = self
                        .documents
                        .iter()
                        .find(|v| &v.id == id && v.load_id == load.id && v.status() != "rejected")
                        .ok_or("Supporting document not found")?;
                    if original.document_sha256.as_ref() != Some(&doc.sha256) {
                        return Err("Supporting document digest changed".into());
                    }
                }
                self.recognize_carrier_invoice(&original.invoice_id)?;
                let amount = original.amount_cents.abs();
                let (debit, credit) = if original.amount_cents > 0 {
                    ("freight_expense", "carrier_payable")
                } else {
                    ("carrier_payable", "freight_expense")
                };
                self.post(
                    ("carrier_adjustment", entity),
                    Utc::now().date_naive(),
                    actor,
                    &format!(
                        "Carrier {} {}: {}",
                        original.kind, original.number, proof.note
                    ),
                    &load.currency,
                    vec![
                        JournalLine {
                            load_id: load.id.clone(),
                            account: debit.into(),
                            debit_cents: amount,
                            credit_cents: 0,
                        },
                        JournalLine {
                            load_id: load.id.clone(),
                            account: credit.into(),
                            debit_cents: 0,
                            credit_cents: amount,
                        },
                    ],
                    None,
                )?;
                self.carrier_adjustments[a].status = "posted".into();
                self.carrier_adjustments[a].posted_at = Some(Utc::now());
                if let Some(id) = &original.carrier_document_id {
                    self.finish_carrier_document(id, "posted", proof.clone())?;
                }
            } else if let Some(date) = reversed_on {
                if date
                    < original
                        .posted_at
                        .ok_or("Posting date required")?
                        .date_naive()
                    || date > Utc::now().date_naive()
                {
                    return Err("Reversal date cannot precede posting or be in the future".into());
                }
                domain::money(
                    self.carrier_invoice_total(&original.invoice_id)? - original.amount_cents,
                )?;
                let journal = self.reverse_post(
                    "carrier_adjustment",
                    entity,
                    "carrier_adjustment_reversal",
                    date,
                    actor,
                    &proof.note,
                )?;
                self.carrier_adjustments[a].status = "reversed".into();
                self.carrier_adjustments[a].reversal = Some(Reversal {
                    note: proof.note.clone(),
                    reversed_on: date,
                    actor: actor.into(),
                    journal_id: journal,
                });
            } else {
                self.carrier_adjustments[a].status = "rejected".into();
                if let Some(id) = &original.carrier_document_id {
                    let source = self
                        .carrier_documents
                        .iter_mut()
                        .find(|v| &v.id == id)
                        .ok_or("Carrier document not found")?;
                    source.status = "pending".into();
                    source.version = next(source.version)?;
                }
            }
            if reversed_on.is_some() {
                self.carrier_adjustments[a].reversal_decision = Some(proof);
            } else {
                self.carrier_adjustments[a].decision = Some(proof);
            }
            self.carrier_adjustments[a].version = next(original.version)?;
            if approved || reversed_on.is_some() {
                if let Some(id) = self
                    .settlements
                    .iter()
                    .find(|v| v.load_id == load.id)
                    .map(|v| v.id.clone())
                {
                    self.refresh_target("payout", &id)?;
                }
            }
            result = json!(self.carrier_adjustments[a]);
        } else if operation == "payable.document.dismiss"
            || operation == "payable.document.apply_revision"
        {
            let input: DocumentDecision = decode(body)?;
            domain::required(&input.note, "Carrier document decision note")?;
            let d = self
                .carrier_documents
                .iter()
                .position(|v| v.id == entity)
                .ok_or("Carrier document not found")?;
            let doc = self.carrier_documents[d].clone();
            if doc.version != input.expected_version {
                return Err("Carrier document changed; refresh before continuing".into());
            }
            if doc.status != "pending" {
                return Err("Only pending carrier documents can be classified".into());
            }
            bump(&mut load, input.expected_load_version)?;
            let proof = PayableDecision {
                note: input.note,
                counterparty_contact: "Authenticated EDI document".into(),
                confirmation_reference: doc.event.event_id.clone(),
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
            };
            if operation == "payable.document.apply_revision" {
                if doc.tender_id != load.tender_id
                    || Some(&doc.carrier_id) != load.carrier_id.as_ref()
                {
                    return Err("Carrier source document belongs to a previous assignment".into());
                }
                let invoice = self
                    .invoices
                    .iter()
                    .find(|v| v.load_id == load.id && v.status != "approved")
                    .ok_or("Unapproved primary invoice required")?;
                let invoice_id = invoice.id.clone();
                let prior_number = invoice.number.clone();
                let prior = invoice.amount_cents;
                if self.invoices.iter().any(|v| {
                    v.id != invoice_id
                        && v.number.eq_ignore_ascii_case(&doc.number)
                        && self.loads.iter().any(|l| {
                            l.id == v.load_id && l.carrier_id.as_ref() == Some(&doc.carrier_id)
                        })
                }) {
                    return Err("Carrier invoice number already exists".into());
                }
                let basis = self.charge_basis(&load, "buy")?;
                let invoice = self
                    .invoices
                    .iter_mut()
                    .find(|v| v.id == invoice_id)
                    .ok_or("Invoice not found")?;
                invoice.amount_cents = doc.amount_cents;
                invoice.number = doc.number.clone();
                invoice.variance_cents = doc.amount_cents - basis.total_amount_cents;
                invoice.agreement = Some(basis);
                invoice.status = if invoice.variance_cents == 0 {
                    "matched"
                } else {
                    "exception"
                }
                .into();
                self.record(&load.id,"invoice.canonical_revision",format!("Canonical carrier document {} revised unapproved invoice {prior_number} ({prior} cents) to {} ({} cents)",doc.event.event_id,doc.number,doc.amount_cents));
                self.finish_carrier_document(entity, "applied_revision", proof)?;
            } else {
                self.finish_carrier_document(entity, "dismissed", proof)?;
            }
            result = json!(self.carrier_documents[d]);
        } else {
            return Err("Unsupported carrier payable operation".into());
        }
        self.loads[index] = load.clone();
        self.record(&load.id,operation,"Carrier payable evidence retained; original approved invoice and bank history preserved");
        if let Some(event) = self.events.last_mut() {
            event.details = Some(result.clone());
        }
        Ok(result)
    }
}
