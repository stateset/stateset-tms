//! Negotiated additions to the original freight agreement. A proposal or gate
//! duration is never a payable, revenue posting, or automatic customer charge.
use crate::domain::{self, Load, Result, Status, Workspace};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Charge {
    pub id: String,
    pub load_id: String,
    pub side: String,
    pub kind: String,
    pub description: String,
    pub amount_cents: i64,
    pub currency: String,
    pub party_id: String,
    pub party_name: String,
    pub tender_id: Option<String>,
    pub evidence_reference: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub status: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
    pub decisions: Vec<ChargeDecision>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChargeDecision {
    pub decision: String,
    pub counterparty_contact: Option<String>,
    pub confirmation_reference: Option<String>,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChargeLine {
    pub charge_id: String,
    pub kind: String,
    pub description: String,
    pub amount_cents: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChargeBasis {
    pub base_amount_cents: i64,
    pub charges: Vec<ChargeLine>,
    pub total_amount_cents: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    expected_load_version: u64,
    side: String,
    kind: String,
    description: String,
    amount_cents: i64,
    evidence_reference: String,
    document_id: Option<String>,
    corrects_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    expected_load_version: u64,
    expected_version: u64,
    decision: String,
    counterparty_contact: Option<String>,
    confirmation_reference: Option<String>,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Void {
    expected_load_version: u64,
    expected_version: u64,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid charge request fields".into())
}
fn bump(load: &mut Load, expected: u64) -> Result<()> {
    if load.version != expected {
        return Err("Load changed; refresh before continuing".into());
    }
    load.version = load
        .version
        .checked_add(1)
        .ok_or("Load version exhausted")?;
    load.updated_at = Utc::now();
    Ok(())
}
impl Workspace {
    pub fn has_approved_charges(&self, load_id: &str) -> bool {
        self.charges
            .iter()
            .any(|v| v.load_id == load_id && v.status == "approved")
    }
    pub fn pending_charges(&self, load_id: &str, side: &str) -> bool {
        self.charges
            .iter()
            .any(|v| v.load_id == load_id && v.side == side && v.status == "proposed")
    }
    pub fn charge_side_locked(&self, load_id: &str, side: &str) -> bool {
        if side == "buy" {
            self.invoices
                .iter()
                .any(|v| v.load_id == load_id && v.status == "approved")
                || self.settlements.iter().any(|v| v.load_id == load_id)
        } else {
            self.bills
                .iter()
                .any(|v| v.load_id == load_id && v.issued_at.is_some() && v.status != "void")
        }
    }
    pub fn charge_basis(&self, load: &Load, side: &str) -> Result<ChargeBasis> {
        if !["buy", "sell"].contains(&side) {
            return Err("Charge side must be buy or sell".into());
        }
        let base = if side == "buy" {
            load.buy_cents
        } else {
            load.sell_cents
        };
        let mut total = base;
        let mut lines = vec![];
        for charge in self
            .charges
            .iter()
            .filter(|v| v.load_id == load.id && v.side == side && v.status == "approved")
        {
            let party = if side == "buy" {
                load.carrier_id.as_deref()
            } else {
                Some(load.customer_id.as_str())
            };
            if party != Some(&charge.party_id)
                || (side == "buy" && charge.tender_id != load.tender_id)
            {
                return Err("Approved charge belongs to a different carrier assignment or customer; reconcile before billing".into());
            }
            total = total
                .checked_add(charge.amount_cents)
                .ok_or("Charge total exceeds supported amount")?;
            domain::money(total)?;
            lines.push(ChargeLine {
                charge_id: charge.id.clone(),
                kind: charge.kind.clone(),
                description: charge.description.clone(),
                amount_cents: charge.amount_cents,
            });
        }
        Ok(ChargeBasis {
            base_amount_cents: base,
            charges: lines,
            total_amount_cents: total,
        })
    }
    pub fn commercial_summary(&self, load: &Load) -> Result<Value> {
        let buy = self.charge_basis(load, "buy")?;
        let sell = self.charge_basis(load, "sell")?;
        let adjustments = self.carrier_adjustment_total(&load.id)?;
        let supplements = self.customer_supplement_total(&load.id)?;
        let credits = self.customer_credit_total(&load.id)?;
        let mut total_sell = sell
            .total_amount_cents
            .checked_add(supplements)
            .and_then(|v| v.checked_sub(credits))
            .ok_or("Commercial total overflow")?;
        let cancelled = load.status == Status::Cancelled;
        let total_buy = if cancelled {
            self.cancellation_total(&load.id, "buy")?
        } else {
            buy.total_amount_cents + adjustments
        };
        if cancelled {
            total_sell = self
                .cancellation_total(&load.id, "sell")?
                .checked_sub(credits)
                .ok_or("Commercial total overflow")?;
        }
        Ok(
            json!({"buy":buy,"sell":sell,"cancelled":cancelled,"carrier_adjustment_cents":adjustments,"total_buy_cents":total_buy,"cancellation_buy_cents":self.cancellation_total(&load.id,"buy")?,"cancellation_sell_cents":self.cancellation_total(&load.id,"sell")?,"customer_supplement_cents":supplements,"customer_credit_cents":credits,"total_sell_cents":total_sell,"margin_cents":total_sell-total_buy,
            "pending_buy":self.charges.iter().filter(|v|v.load_id==load.id&&v.side=="buy"&&v.status=="proposed").count(),
            "pending_sell":self.charges.iter().filter(|v|v.load_id==load.id&&v.side=="sell"&&v.status=="proposed").count(),
            "buy_locked":self.charge_side_locked(&load.id,"buy"),"sell_locked":self.charge_side_locked(&load.id,"sell")}),
        )
    }
    pub fn apply_charge_operation(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let load_id = if operation == "charge.propose" {
            entity.into()
        } else {
            self.charges
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.clone())
                .ok_or("Charge not found")?
        };
        let index = self
            .loads
            .iter()
            .position(|v| v.id == load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        let result;
        if operation == "charge.propose" {
            let input: Proposal = decode(body)?;
            if !["buy", "sell"].contains(&input.side.as_str()) {
                return Err("Charge side must be buy or sell".into());
            }
            if ![
                "detention",
                "lumper",
                "fuel",
                "layover",
                "extra_stop",
                "other",
            ]
            .contains(&input.kind.as_str())
            {
                return Err("Unsupported charge kind".into());
            }
            domain::required(&input.description, "Charge description")?;
            domain::required(&input.evidence_reference, "Charge evidence reference")?;
            domain::money(input.amount_cents)?;
            if input.amount_cents == 0 {
                return Err("Charge amount must be positive".into());
            }
            if !matches!(
                load.status,
                Status::Accepted | Status::Dispatched | Status::InTransit | Status::Delivered
            ) {
                return Err(
                    "Charges require an accepted carrier assignment or delivered shipment".into(),
                );
            }
            if self.charge_side_locked(&load.id, &input.side) {
                return Err("This side of the freight agreement already supports posted billing; reconcile its financial documents before adding charges".into());
            }
            if load.carrier_id.is_some() {
                self.adopt_tender(&mut load)?;
            }
            let (party_id, party_name, tender_id) = if input.side == "buy" {
                let carrier = self
                    .carriers
                    .iter()
                    .find(|v| Some(&v.id) == load.carrier_id.as_ref())
                    .ok_or("Carrier required")?;
                (
                    carrier.id.clone(),
                    carrier.name.clone(),
                    load.tender_id.clone(),
                )
            } else {
                let customer = self
                    .customers
                    .iter()
                    .find(|v| v.id == load.customer_id)
                    .ok_or("Customer not found")?;
                (customer.id.clone(), customer.name.clone(), None)
            };
            let reference = input.evidence_reference.trim().to_string();
            let previous: Vec<_> = self
                .charges
                .iter()
                .filter(|v| {
                    v.load_id == load.id
                        && v.side == input.side
                        && v.evidence_reference.eq_ignore_ascii_case(&reference)
                })
                .collect();
            if previous
                .iter()
                .any(|v| ["proposed", "approved"].contains(&v.status.as_str()))
            {
                return Err("Charge evidence reference already has an active proposal or approval on this side".into());
            }
            if let Some(corrects) = &input.corrects_id {
                let original = self
                    .charges
                    .iter()
                    .find(|v| &v.id == corrects)
                    .ok_or("Original charge not found")?;
                if original.load_id != load.id
                    || original.side != input.side
                    || original.party_id != party_id
                    || original.tender_id != tender_id
                    || !original.evidence_reference.eq_ignore_ascii_case(&reference)
                    || !["rejected", "voided"].contains(&original.status.as_str())
                    || original.corrected_by_id.is_some()
                {
                    return Err("Correction must replace one uncorrected rejected or voided charge for the same assignment, side, and reference".into());
                }
            } else if !previous.is_empty() {
                return Err(
                    "Use an explicit correction when reusing a rejected or voided charge reference"
                        .into(),
                );
            }
            let digest = if let Some(id) = &input.document_id {
                let doc = self
                    .documents
                    .iter()
                    .find(|v| &v.id == id && v.load_id == load.id)
                    .ok_or("Document not found")?;
                if doc.status() == "rejected" {
                    return Err("Rejected evidence cannot support a charge".into());
                }
                Some(doc.sha256.clone())
            } else {
                None
            };
            bump(&mut load, input.expected_load_version)?;
            let charge = Charge {
                id: domain::id("charge"),
                load_id: load.id.clone(),
                side: input.side,
                kind: input.kind,
                description: input.description,
                amount_cents: input.amount_cents,
                currency: load.currency.clone(),
                party_id,
                party_name,
                tender_id,
                evidence_reference: reference,
                document_id: input.document_id,
                document_sha256: digest,
                status: "proposed".into(),
                version: 1,
                created_at: Utc::now(),
                created_by: actor.into(),
                corrects_id: input.corrects_id,
                corrected_by_id: None,
                decisions: vec![],
            };
            if let Some(id) = &charge.corrects_id {
                let original = self
                    .charges
                    .iter_mut()
                    .find(|v| &v.id == id)
                    .ok_or("Original charge not found")?;
                original.corrected_by_id = Some(charge.id.clone());
                original.version = original
                    .version
                    .checked_add(1)
                    .ok_or("Charge version exhausted")?;
            }
            result = json!(charge);
            self.charges.push(charge);
        } else {
            let c = self
                .charges
                .iter()
                .position(|v| v.id == entity)
                .ok_or("Charge not found")?;
            let charge = self.charges[c].clone();
            let (expected_load, expected_charge, decision, contact, reference, note) =
                match operation {
                    "charge.review" => {
                        let i: Review = decode(body)?;
                        (
                            i.expected_load_version,
                            i.expected_version,
                            i.decision,
                            i.counterparty_contact,
                            i.confirmation_reference,
                            i.note,
                        )
                    }
                    "charge.void" => {
                        let i: Void = decode(body)?;
                        (
                            i.expected_load_version,
                            i.expected_version,
                            "voided".into(),
                            Some(i.counterparty_contact),
                            Some(i.confirmation_reference),
                            i.note,
                        )
                    }
                    _ => return Err("Unsupported charge operation".into()),
                };
            if charge.version != expected_charge {
                return Err("Charge changed; refresh before continuing".into());
            }
            domain::required(&note, "Charge decision note")?;
            match decision.as_str() {
                "approved" | "rejected"
                    if operation == "charge.review" && charge.status == "proposed" => {}
                "voided" if operation == "charge.void" && charge.status == "approved" => {}
                _ => return Err("Charge decision is not valid for its current state".into()),
            }
            if decision != "rejected" {
                domain::required(contact.as_deref().unwrap_or(""), "Counterparty contact")?;
                domain::required(
                    reference.as_deref().unwrap_or(""),
                    "Counterparty confirmation reference",
                )?;
                if self.charge_side_locked(&load.id, &charge.side) {
                    return Err("Posted billing locks its approved charge basis; reconcile financial documents before changing it".into());
                }
            }
            if decision == "approved" {
                if matches!(
                    load.status,
                    Status::Planned | Status::Tendered | Status::Cancelled
                ) || self.release_pending(&load)
                    || self.unresolved_tender_incidents(&load.id)
                {
                    return Err(
                        "Reconcile carrier assignment and custody before approving charges".into(),
                    );
                }
                if (charge.side == "buy"
                    && (load.carrier_id.as_ref() != Some(&charge.party_id)
                        || load.tender_id != charge.tender_id))
                    || (charge.side == "sell" && load.customer_id != charge.party_id)
                {
                    return Err(
                        "Charge belongs to a previous carrier assignment or customer".into(),
                    );
                }
                if let Some(id) = &charge.document_id {
                    let doc = self
                        .documents
                        .iter()
                        .find(|v| &v.id == id && v.load_id == load.id)
                        .ok_or("Document not found")?;
                    if doc.status() == "rejected"
                        || Some(&doc.sha256) != charge.document_sha256.as_ref()
                    {
                        return Err("Charge evidence has been rejected or changed".into());
                    }
                }
            }
            bump(&mut load, expected_load)?;
            self.charges[c].status = decision.clone();
            self.charges[c].version = self.charges[c]
                .version
                .checked_add(1)
                .ok_or("Charge version exhausted")?;
            self.charges[c].decisions.push(ChargeDecision {
                decision,
                counterparty_contact: contact,
                confirmation_reference: reference,
                note,
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
            });
            let basis = self.charge_basis(&load, &charge.side)?;
            if charge.side == "buy" {
                for invoice in self
                    .invoices
                    .iter_mut()
                    .filter(|v| v.load_id == load.id && v.status != "approved")
                {
                    invoice.variance_cents = invoice.amount_cents - basis.total_amount_cents;
                    invoice.status = if invoice.variance_cents == 0 {
                        "matched"
                    } else {
                        "exception"
                    }
                    .into();
                    invoice.agreement = Some(basis.clone());
                }
            } else if ["approved", "voided"].contains(&self.charges[c].status.as_str()) {
                for bill in self.bills.iter_mut().filter(|v| {
                    v.load_id == load.id && v.status == "draft" && v.supplement.is_none()
                }) {
                    bill.snapshot.amount_cents = basis.total_amount_cents;
                    bill.snapshot.agreement = Some(basis.clone());
                    bill.snapshot.source_load_version = load.version;
                    bill.version = bill
                        .version
                        .checked_add(1)
                        .ok_or("Bill version exhausted")?;
                }
            }
            result = json!(self.charges[c]);
        }
        self.loads[index] = load.clone();
        self.record(
            &load.id,
            operation,
            format!(
                "{} {} charge {}: {} cents; original base freight retained",
                result["side"].as_str().unwrap_or(""),
                result["kind"].as_str().unwrap_or(""),
                result["status"].as_str().unwrap_or(""),
                result["amount_cents"]
            ),
        );
        if let Some(event) = self.events.last_mut() {
            event.details = Some(result.clone());
        }
        Ok(result)
    }
}
