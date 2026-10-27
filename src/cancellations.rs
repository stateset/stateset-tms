//! Explicit obligations after operational cancellation. These agreements never
//! create delivery/POD, restore carrier custody, or charge the original rate.
use crate::{
    charges::{ChargeBasis, ChargeLine},
    domain::{self, Result, Settlement, Status, Workspace},
    finance::{BillingSnapshot, CustomerBill, Reversal},
    payables::PayableDecision,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CancellationFee {
    pub id: String,
    pub load_id: String,
    pub side: String,
    pub party_id: String,
    pub party_snapshot: Value,
    pub tender_id: Option<String>,
    pub amount_cents: i64,
    pub currency: String,
    pub description: String,
    pub evidence_reference: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub status: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub decision: Option<PayableDecision>,
    pub number: Option<String>,
    pub target_id: Option<String>,
    pub posted_at: Option<DateTime<Utc>>,
    pub reversal: Option<Reversal>,
    pub reversal_confirmation: Option<PayableDecision>,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    expected_load_version: u64,
    side: String,
    tender_id: Option<String>,
    amount_cents: i64,
    description: String,
    evidence_reference: String,
    document_id: Option<String>,
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
    number: Option<String>,
    due_date: Option<NaiveDate>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reverse {
    expected_load_version: u64,
    expected_version: u64,
    expected_target_version: u64,
    reversed_on: NaiveDate,
    counterparty_contact: String,
    confirmation_reference: String,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid cancellation fee request".into())
}
fn check(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        Err("Cancellation fee or shipment changed; refresh before continuing".into())
    } else {
        Ok(())
    }
}
fn proof(contact: String, reference: String, note: String, actor: &str) -> Result<PayableDecision> {
    for (value, label) in [
        (&contact, "Counterparty contact"),
        (&reference, "Confirmation reference"),
        (&note, "Decision note"),
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
    pub(crate) fn cancellation_invoice_number_exists(
        &self,
        carrier: &str,
        number: &str,
        correction_of: Option<&str>,
    ) -> bool {
        let mut ancestors = std::collections::BTreeSet::new();
        let mut prior = correction_of;
        while let Some(id) = prior {
            if !ancestors.insert(id) {
                break;
            }
            prior = self
                .cancellation_fees
                .iter()
                .find(|v| v.id == id)
                .and_then(|v| v.corrects_id.as_deref());
        }
        self.cancellation_fees.iter().any(|v| {
            v.side == "buy"
                && v.party_id == carrier
                && v.number
                    .as_ref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(number))
                && !(v.status == "reversed" && ancestors.contains(v.id.as_str()))
        })
    }
    pub fn cancellation_target(&self, target: &str) -> Option<&CancellationFee> {
        self.cancellation_fees
            .iter()
            .find(|v| v.target_id.as_deref() == Some(target))
    }
    pub fn cancellation_total(&self, load: &str, side: &str) -> Result<i64> {
        self.cancellation_fees
            .iter()
            .filter(|v| v.load_id == load && v.side == side && v.status == "posted")
            .try_fold(0i64, |a, v| {
                a.checked_add(v.amount_cents)
                    .ok_or_else(|| "Cancellation fee total overflow".into())
            })
    }
    fn fee_evidence(&self, fee: &CancellationFee) -> Result<()> {
        if let Some(id) = &fee.document_id {
            let document = self
                .documents
                .iter()
                .find(|v| &v.id == id && v.load_id == fee.load_id)
                .ok_or("Cancellation agreement document not found")?;
            if document.status() == "rejected"
                || fee.document_sha256.as_ref() != Some(&document.sha256)
            {
                return Err("Cancellation agreement evidence is rejected or changed".into());
            }
        }
        Ok(())
    }
    pub fn apply_cancellation(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let load_id = if operation == "cancellation.propose" {
            entity.to_owned()
        } else {
            self.cancellation_fees
                .iter()
                .find(|v| v.id == entity)
                .ok_or("Cancellation fee not found")?
                .load_id
                .clone()
        };
        let load_index = self
            .loads
            .iter()
            .position(|v| v.id == load_id)
            .ok_or("Load not found")?;
        let load = self.loads[load_index].clone();
        domain::currency_code(&load.currency)?;
        if load.status != Status::Cancelled {
            return Err("Cancellation fees require an operationally cancelled shipment".into());
        }
        if self.unresolved_tender_incidents(&load.id) {
            return Err(
                "Resolve carrier custody incidents before recording cancellation fees".into(),
            );
        }
        let response = match operation {
            "cancellation.propose" => {
                let input: Proposal = decode(body)?;
                check(load.version, input.expected_load_version)?;
                domain::money(input.amount_cents)?;
                if input.amount_cents == 0 {
                    return Err("Cancellation fee must be positive".into());
                }
                domain::required(&input.description, "Cancellation service description")?;
                domain::required(
                    &input.evidence_reference,
                    "Cancellation agreement reference",
                )?;
                let (party_id, party_snapshot) = match input.side.as_str() {
                    "buy" => {
                        let tender = self
                            .tenders
                            .iter()
                            .find(|v| {
                                Some(&v.id) == input.tender_id.as_ref()
                                    && v.load_id == load.id
                                    && v.status == "released"
                            })
                            .ok_or(
                                "Select the released carrier tender for this cancellation fee",
                            )?;
                        if !self.releases.iter().any(|v| {
                            v.tender_id == tender.id
                                && v.status == "confirmed"
                                && v.confirmation.is_some()
                        }) {
                            return Err(
                                "Carrier fee requires actual confirmed carrier release".into()
                            );
                        }
                        (tender.carrier_id.clone(), json!(tender.carrier))
                    }
                    "sell" => {
                        if input.tender_id.is_some() {
                            return Err("Customer fee cannot select a carrier tender".into());
                        }
                        let customer = self
                            .customers
                            .iter()
                            .find(|v| v.id == load.customer_id)
                            .ok_or("Customer not found")?;
                        (customer.id.clone(), json!(customer))
                    }
                    _ => return Err("Cancellation fee side must be buy or sell".into()),
                };
                let reference = input.evidence_reference.trim().to_owned();
                let matching: Vec<_> = self
                    .cancellation_fees
                    .iter()
                    .filter(|v| {
                        v.load_id == load.id
                            && v.side == input.side
                            && v.party_id == party_id
                            && v.evidence_reference.eq_ignore_ascii_case(&reference)
                    })
                    .collect();
                if matching
                    .iter()
                    .any(|v| ["proposed", "posted"].contains(&v.status.as_str()))
                {
                    return Err("Cancellation agreement reference already has an active fee".into());
                }
                if let Some(id) = &input.corrects_id {
                    let old = self
                        .cancellation_fees
                        .iter()
                        .find(|v| &v.id == id)
                        .ok_or("Corrected cancellation fee not found")?;
                    if !["rejected", "reversed"].contains(&old.status.as_str())
                        || old.corrected_by_id.is_some()
                        || old.load_id != load.id
                        || old.side != input.side
                        || old.party_id != party_id
                        || old.tender_id != input.tender_id
                        || !old.evidence_reference.eq_ignore_ascii_case(&reference)
                    {
                        return Err("Correction must link the uncorrected rejected/reversed fee for this exact shipment, party, tender, and agreement".into());
                    }
                } else if !matching.is_empty() {
                    return Err(
                        "A prior cancellation fee requires an explicit correction link".into(),
                    );
                }
                let document_sha256 = if let Some(id) = &input.document_id {
                    Some(
                        self.documents
                            .iter()
                            .find(|v| {
                                &v.id == id && v.load_id == load.id && v.status() != "rejected"
                            })
                            .ok_or("Cancellation agreement document not found")?
                            .sha256
                            .clone(),
                    )
                } else {
                    None
                };
                let fee = CancellationFee {
                    id: domain::id("cancel-fee"),
                    load_id: load.id.clone(),
                    side: input.side,
                    party_id,
                    party_snapshot,
                    tender_id: input.tender_id,
                    amount_cents: input.amount_cents,
                    currency: load.currency.clone(),
                    description: input.description.trim().into(),
                    evidence_reference: reference,
                    document_id: input.document_id,
                    document_sha256,
                    status: "proposed".into(),
                    version: 1,
                    created_at: Utc::now(),
                    created_by: actor.into(),
                    decision: None,
                    number: None,
                    target_id: None,
                    posted_at: None,
                    reversal: None,
                    reversal_confirmation: None,
                    corrects_id: input.corrects_id.clone(),
                    corrected_by_id: None,
                };
                if let Some(id) = input.corrects_id {
                    let old = self
                        .cancellation_fees
                        .iter_mut()
                        .find(|v| v.id == id)
                        .ok_or("Corrected cancellation fee not found")?;
                    old.corrected_by_id = Some(fee.id.clone());
                    old.version = old
                        .version
                        .checked_add(1)
                        .ok_or("Cancellation fee version exhausted")?;
                }
                let result = json!(fee);
                self.cancellation_fees.push(fee);
                result
            }
            "cancellation.review" => {
                let input: Decision = decode(body)?;
                check(load.version, input.expected_load_version)?;
                let index = self
                    .cancellation_fees
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Cancellation fee not found")?;
                let fee = self.cancellation_fees[index].clone();
                check(fee.version, input.expected_version)?;
                if fee.status != "proposed"
                    || !["approve", "reject"].contains(&input.decision.as_str())
                {
                    return Err(
                        "Only a proposed cancellation fee can be approved or rejected".into(),
                    );
                }
                let decision = proof(
                    input.counterparty_contact,
                    input.confirmation_reference,
                    input.note,
                    actor,
                )?;
                if input.decision == "approve" {
                    self.fee_evidence(&fee)?;
                    let number = crate::finance::number(
                        input
                            .number
                            .as_deref()
                            .ok_or("Actual invoice number required")?,
                    )?;
                    let target = if fee.side == "buy" {
                        if self.cancellation_invoice_number_exists(
                            &fee.party_id,
                            &number,
                            fee.corrects_id.as_deref(),
                        ) || self.invoices.iter().any(|v| {
                            v.number.eq_ignore_ascii_case(&number)
                                && self.loads.iter().any(|l| {
                                    l.id == v.load_id
                                        && l.carrier_id.as_deref() == Some(&fee.party_id)
                                })
                        }) || self.carrier_adjustments.iter().any(|v| {
                            v.carrier_id == fee.party_id && v.number.eq_ignore_ascii_case(&number)
                        }) {
                            return Err("Carrier invoice number already recorded".into());
                        }
                        if input.due_date.is_some() {
                            return Err(
                                "Carrier approval does not issue a customer due date".into()
                            );
                        }
                        let settlement = Settlement {
                            id: domain::id("settlement"),
                            load_id: load.id.clone(),
                            carrier_id: fee.party_id.clone(),
                            amount_cents: fee.amount_cents,
                            status: "prepared".into(),
                            version: 1,
                            created_at: Some(Utc::now()),
                        };
                        let id = settlement.id.clone();
                        self.settlements.push(settlement);
                        id
                    } else {
                        if self
                            .bills
                            .iter()
                            .any(|v| v.number.eq_ignore_ascii_case(&number))
                        {
                            return Err("Customer invoice number already exists".into());
                        }
                        let due_date =
                            input.due_date.ok_or("Customer invoice due date required")?;
                        if due_date < Utc::now().date_naive() {
                            return Err("Invoice due date cannot precede its issue date".into());
                        }
                        let customer: domain::Customer =
                            serde_json::from_value(fee.party_snapshot.clone())
                                .map_err(|_| "Invalid cancellation customer snapshot")?;
                        if customer.billing_address.trim().is_empty() {
                            return Err("Customer billing address is required before issuing a cancellation invoice".into());
                        }
                        let issuer = self.billing_profile.clone().ok_or(
                            "Configure the billing issuer identity before issuing an invoice",
                        )?;
                        let bill = CustomerBill {
                            id: domain::id("bill"),
                            number: number.clone(),
                            load_id: load.id.clone(),
                            customer_id: fee.party_id.clone(),
                            snapshot: BillingSnapshot {
                                intermediate_stops: load.intermediate_stops.clone(),
                                customer,
                                load_reference: load.reference.clone(),
                                origin: load.origin.clone(),
                                destination: load.destination.clone(),
                                commodity: fee.description.clone(),
                                amount_cents: fee.amount_cents,
                                currency: load.currency.clone(),
                                source_load_version: load.version,
                                issuer: Some(issuer),
                                agreement: Some(ChargeBasis {
                                    base_amount_cents: 0,
                                    charges: vec![ChargeLine {
                                        charge_id: fee.id.clone(),
                                        kind: "cancellation".into(),
                                        description: fee.description.clone(),
                                        amount_cents: fee.amount_cents,
                                    }],
                                    total_amount_cents: fee.amount_cents,
                                }),
                            },
                            due_date,
                            status: "issued".into(),
                            version: 1,
                            created_at: Utc::now(),
                            created_by: actor.into(),
                            issued_at: Some(Utc::now()),
                            void_reason: None,
                            supplement: None,
                        };
                        let id = bill.id.clone();
                        self.bills.push(bill);
                        id
                    };
                    self.post(
                        ("cancellation_fee", &fee.id),
                        Utc::now().date_naive(),
                        actor,
                        &format!("Cancellation fee {} · {number}", fee.description),
                        &load.currency,
                        crate::finance::pair(
                            &load.id,
                            if fee.side == "buy" {
                                "freight_expense"
                            } else {
                                "accounts_receivable"
                            },
                            if fee.side == "buy" {
                                "carrier_payable"
                            } else {
                                "freight_revenue"
                            },
                            fee.amount_cents,
                        ),
                        None,
                    )?;
                    let current = &mut self.cancellation_fees[index];
                    current.target_id = Some(target);
                    current.number = Some(number);
                    current.posted_at = Some(Utc::now());
                    current.status = "posted".into();
                } else {
                    self.cancellation_fees[index].status = "rejected".into();
                }
                let current = &mut self.cancellation_fees[index];
                current.decision = Some(decision);
                current.version = current
                    .version
                    .checked_add(1)
                    .ok_or("Cancellation fee version exhausted")?;
                json!(current)
            }
            "cancellation.reverse" => {
                let input: Reverse = decode(body)?;
                check(load.version, input.expected_load_version)?;
                let index = self
                    .cancellation_fees
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Cancellation fee not found")?;
                let fee = self.cancellation_fees[index].clone();
                check(fee.version, input.expected_version)?;
                if fee.status != "posted" {
                    return Err("Only a posted cancellation fee can be reversed".into());
                }
                let confirmation = proof(
                    input.counterparty_contact,
                    input.confirmation_reference,
                    input.note,
                    actor,
                )?;
                let target = fee
                    .target_id
                    .as_deref()
                    .ok_or("Cancellation posting target not found")?;
                let earliest = self
                    .cash_entries
                    .iter()
                    .filter(|v| v.allocations.iter().any(|a| a.target_id == target))
                    .flat_map(|v| {
                        std::iter::once(v.cleared_on)
                            .chain(v.reversal.as_ref().map(|r| r.reversed_on))
                    })
                    .chain(
                        self.carrier_credit_applications
                            .iter()
                            .filter(|v| {
                                v.source_settlement_id == target
                                    || v.allocations.iter().any(|a| a.target_id == target)
                            })
                            .flat_map(|v| {
                                std::iter::once(v.applied_on)
                                    .chain(v.reversal.as_ref().map(|r| r.reversed_on))
                            }),
                    )
                    .max()
                    .unwrap_or(fee.posted_at.ok_or("Posting date required")?.date_naive());
                if input.reversed_on < earliest
                    || input.reversed_on
                        < fee.posted_at.ok_or("Posting date required")?.date_naive()
                    || input.reversed_on > Utc::now().date_naive()
                {
                    return Err("Cancellation reversal cannot precede posting/bank activity or be in the future".into());
                }
                if fee.side == "sell" {
                    let bill = self
                        .bills
                        .iter()
                        .find(|v| v.id == target)
                        .ok_or("Cancellation invoice not found")?;
                    check(bill.version, input.expected_target_version)?;
                    if input.reversed_on < self.customer_activity_date(bill) {
                        return Err(
                            "Cancellation reversal cannot precede customer credit or bank activity"
                                .into(),
                        );
                    }
                    let totals = self.bill_totals(bill)?;
                    if totals.credited_cents != 0 {
                        return Err("Reverse active customer credit notes before reversing the cancellation fee; genuine bank receipts remain recorded".into());
                    }
                } else {
                    let settlement = self
                        .settlements
                        .iter()
                        .find(|v| v.id == target)
                        .ok_or("Settlement not found")?;
                    check(settlement.version, input.expected_target_version)?;
                }
                let journal = self.reverse_post(
                    "cancellation_fee",
                    &fee.id,
                    "cancellation_fee_reversal",
                    input.reversed_on,
                    actor,
                    &confirmation.note,
                )?;
                self.cancellation_fees[index].status = "reversed".into();
                self.cancellation_fees[index].version = self.cancellation_fees[index]
                    .version
                    .checked_add(1)
                    .ok_or("Cancellation fee version exhausted")?;
                self.cancellation_fees[index].reversal = Some(Reversal {
                    note: confirmation.note.clone(),
                    reversed_on: input.reversed_on,
                    actor: actor.into(),
                    journal_id: journal,
                });
                self.cancellation_fees[index].reversal_confirmation = Some(confirmation.clone());
                if fee.side == "sell" {
                    let bill = self
                        .bills
                        .iter_mut()
                        .find(|v| v.id == target)
                        .ok_or("Cancellation invoice not found")?;
                    bill.status = "void".into();
                    bill.version = bill
                        .version
                        .checked_add(1)
                        .ok_or("Customer invoice version exhausted")?;
                    bill.void_reason = Some(confirmation.note);
                } else {
                    self.refresh_target("payout", target)?;
                }
                json!(self.cancellation_fees[index])
            }
            _ => return Err("Unsupported cancellation fee operation".into()),
        };
        self.loads[load_index].version = self.loads[load_index]
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        self.loads[load_index].updated_at = Utc::now();
        self.record(
            &load.id,
            operation,
            format!(
                "Cancellation fee {} recorded; shipment remains cancelled",
                response["status"].as_str().unwrap_or("proposal")
            ),
        );
        Ok(response)
    }
}
