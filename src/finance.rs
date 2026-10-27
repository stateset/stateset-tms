//! Freight subledger: immutable billing snapshots, external cash reconciliation,
//! and balanced postings committed with tenant business state.
use crate::domain::{self, Customer, Load, Result, Status, Stop, Workspace};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

pub use crate::finance_read::FinanceReadIndex;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingSnapshot {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub customer: Customer,
    pub load_reference: String,
    pub origin: Stop,
    pub destination: Stop,
    pub commodity: String,
    pub amount_cents: i64,
    pub currency: String,
    pub source_load_version: u64,
    #[serde(default)]
    pub issuer: Option<BillingProfile>,
    #[serde(default)]
    pub agreement: Option<crate::charges::ChargeBasis>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BillingProfile {
    pub legal_name: String,
    pub address: String,
    pub contact: String,
    pub payment_instructions: String,
    pub version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    expected_version: u64,
    legal_name: String,
    address: String,
    contact: String,
    payment_instructions: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomerBill {
    pub id: String,
    pub number: String,
    pub load_id: String,
    pub customer_id: String,
    pub snapshot: BillingSnapshot,
    pub due_date: NaiveDate,
    pub status: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub issued_at: Option<DateTime<Utc>>,
    pub void_reason: Option<String>,
    #[serde(default)]
    pub supplement: Option<SupplementAgreement>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SupplementAgreement {
    pub parent_bill_id: String,
    pub parent_number: String,
    pub description: String,
    pub counterparty_contact: String,
    pub confirmation_reference: String,
    pub evidence_reference: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewSupplement {
    expected_version: u64,
    expected_load_version: u64,
    number: String,
    due_date: NaiveDate,
    amount_cents: i64,
    description: String,
    counterparty_contact: String,
    confirmation_reference: String,
    evidence_reference: String,
    document_id: Option<String>,
    corrects_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditNote {
    #[serde(default)]
    pub issued_on: Option<NaiveDate>,
    #[serde(default)]
    pub confirmation: Option<crate::customer_refunds::CustomerConfirmation>,
    pub id: String,
    pub number: String,
    pub bill_id: String,
    pub load_id: String,
    pub amount_cents: i64,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub version: u64,
    pub reversal: Option<Reversal>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reversal {
    pub note: String,
    pub reversed_on: NaiveDate,
    pub actor: String,
    pub journal_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CashAllocation {
    pub target_id: String,
    pub load_id: String,
    pub amount_cents: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CashEntry {
    #[serde(default)]
    pub customer_confirmation: Option<crate::customer_refunds::CustomerConfirmation>,
    pub id: String,
    pub direction: String,
    pub party_id: String,
    pub account: String,
    pub external_reference: String,
    pub amount_cents: i64,
    pub currency: String,
    pub cleared_on: NaiveDate,
    pub note: String,
    pub allocations: Vec<CashAllocation>,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub version: u64,
    pub reversal: Option<Reversal>,
    pub corrects_id: Option<String>,
    pub corrected_by_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalLine {
    pub load_id: String,
    pub account: String,
    pub debit_cents: i64,
    pub credit_cents: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub source_kind: String,
    pub source_id: String,
    pub posted_on: NaiveDate,
    pub recorded_at: DateTime<Utc>,
    pub actor: String,
    pub currency: String,
    pub memo: String,
    pub reversal_of: Option<String>,
    pub lines: Vec<JournalLine>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewBill {
    load_id: String,
    number: String,
    due_date: NaiveDate,
    bill_to_address: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviseBill {
    expected_version: u64,
    number: String,
    due_date: NaiveDate,
    bill_to_address: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    expected_version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VoidBill {
    expected_version: u64,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCredit {
    issued_on: Option<NaiveDate>,
    confirmation: Option<crate::customer_refunds::ConfirmationInput>,
    expected_version: u64,
    number: String,
    amount_cents: i64,
    reason: String,
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
struct NewCash {
    confirmation: Option<crate::customer_refunds::ConfirmationInput>,
    direction: String,
    account: String,
    external_reference: String,
    amount_cents: i64,
    currency: String,
    cleared_on: NaiveDate,
    note: String,
    allocations: Vec<AllocationInput>,
    corrects_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReverseInput {
    expected_bill_version: Option<u64>,
    expected_version: u64,
    reversed_on: NaiveDate,
    note: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct BillTotals {
    pub refunded_cents: i64,
    pub refund_due_cents: i64,
    pub remaining_creditable_cents: i64,
    pub received_cents: i64,
    pub credited_cents: i64,
    pub balance_cents: i64,
    pub overdue_days: i64,
}
impl BillTotals {
    pub(crate) fn from_activity(
        bill: &CustomerBill,
        received: i64,
        credits: i64,
        refunded: i64,
    ) -> Result<Self> {
        let balance = if bill.issued_at.is_some() {
            (if bill.status != "void" {
                bill.snapshot.amount_cents
            } else {
                0
            })
            .checked_sub(received)
            .and_then(|v| v.checked_add(refunded))
            .and_then(|v| v.checked_sub(credits))
            .ok_or("Financial total overflow")?
        } else {
            0
        };
        Ok(Self {
            refunded_cents: refunded,
            refund_due_cents: credit_balance(balance)?,
            remaining_creditable_cents: if bill.issued_at.is_some() && bill.status != "void" {
                bill.snapshot
                    .amount_cents
                    .checked_sub(credits)
                    .ok_or("Financial total overflow")?
            } else {
                0
            },
            received_cents: received,
            credited_cents: credits,
            balance_cents: balance,
            overdue_days: if balance > 0 {
                (Utc::now().date_naive() - bill.due_date).num_days().max(0)
            } else {
                0
            },
        })
    }
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid financial request fields".into())
}
pub(crate) fn number(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 60
        || !value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b"-_/.".contains(&v))
    {
        return Err("Financial document number must contain 1–60 letters, digits, hyphens, underscores, periods, or slashes".into());
    }
    Ok(value.to_ascii_uppercase())
}
fn positive(amount: i64) -> Result<()> {
    domain::money(amount)?;
    if amount == 0 {
        return Err("Amount must be positive".into());
    }
    Ok(())
}
fn version(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        Err("Financial record changed; refresh before continuing".into())
    } else {
        Ok(())
    }
}
fn next(value: u64) -> Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| "Financial record version exhausted".into())
}
fn sum(mut values: impl Iterator<Item = i64>) -> Result<i64> {
    values.try_fold(0i64, |a, b| {
        a.checked_add(b)
            .ok_or_else(|| "Financial total overflow".into())
    })
}
pub(crate) fn credit_balance(balance: i64) -> Result<i64> {
    0i64.checked_sub(balance.min(0))
        .ok_or_else(|| "Financial total overflow".into())
}
pub(crate) fn credit_sum(mut values: impl Iterator<Item = i64>) -> Result<i64> {
    values.try_fold(0i64, |a, b| {
        a.checked_add(credit_balance(b)?)
            .ok_or_else(|| "Financial total overflow".into())
    })
}
pub(crate) fn reconciled_cash<'a>(
    entries: impl Iterator<Item = (&'a CashEntry, &'a CashAllocation)>,
) -> Result<i64> {
    entries
        .filter(|(v, _)| {
            ["payout", "carrier_refund"].contains(&v.direction.as_str()) && v.reversal.is_none()
        })
        .try_fold(0i64, |total, (v, allocation)| {
            let amount = if v.direction == "carrier_refund" {
                allocation
                    .amount_cents
                    .checked_neg()
                    .ok_or("Financial total overflow")?
            } else {
                allocation.amount_cents
            };
            total
                .checked_add(amount)
                .ok_or_else(|| "Financial total overflow".into())
        })
}
pub(crate) fn pair(load: &str, debit: &str, credit: &str, amount: i64) -> Vec<JournalLine> {
    vec![
        JournalLine {
            load_id: load.into(),
            account: debit.into(),
            debit_cents: amount,
            credit_cents: 0,
        },
        JournalLine {
            load_id: load.into(),
            account: credit.into(),
            debit_cents: 0,
            credit_cents: amount,
        },
    ]
}

impl Workspace {
    pub fn customer_supplement_total(&self, load_id: &str) -> Result<i64> {
        sum(self
            .bills
            .iter()
            .filter(|v| {
                v.load_id == load_id
                    && v.supplement.is_some()
                    && v.issued_at.is_some()
                    && v.status != "void"
            })
            .map(|v| v.snapshot.amount_cents))
    }
    pub fn customer_credit_total(&self, load_id: &str) -> Result<i64> {
        sum(self
            .credits
            .iter()
            .filter(|v| v.load_id == load_id && v.reversal.is_none())
            .map(|v| v.amount_cents))
    }
    fn validate_supplement(&self, bill: &CustomerBill) -> Result<()> {
        let Some(agreement) = &bill.supplement else {
            return Ok(());
        };
        let parent = self
            .bills
            .iter()
            .find(|v| v.id == agreement.parent_bill_id)
            .ok_or("Original customer invoice not found")?;
        if parent.issued_at.is_none()
            || parent.status == "void"
            || parent.supplement.is_some()
            || parent.load_id != bill.load_id
            || parent.customer_id != bill.customer_id
        {
            return Err("Supplement requires its active issued original customer invoice".into());
        }
        let load = self
            .loads
            .iter()
            .find(|v| v.id == bill.load_id)
            .ok_or("Load not found")?;
        if load.customer_id != bill.customer_id || load.currency != bill.snapshot.currency {
            return Err("Supplement belongs to a different shipment customer or currency".into());
        }
        if let Some(id) = &agreement.document_id {
            let document = self
                .documents
                .iter()
                .find(|v| &v.id == id && v.load_id == bill.load_id)
                .ok_or("Agreement document not found")?;
            if document.status() == "rejected"
                || agreement.document_sha256.as_ref() != Some(&document.sha256)
            {
                return Err("Supplement agreement evidence is rejected or changed".into());
            }
        }
        Ok(())
    }
    fn billing_snapshot(&self, load: &Load) -> Result<BillingSnapshot> {
        let customer = self
            .customers
            .iter()
            .find(|v| v.id == load.customer_id)
            .ok_or("Customer not found")?;
        let agreement = self.charge_basis(load, "sell")?;
        positive(agreement.total_amount_cents)?;
        Ok(BillingSnapshot {
            customer: customer.clone(),
            load_reference: load.reference.clone(),
            origin: load.origin.clone(),
            intermediate_stops: load.intermediate_stops.clone(),
            destination: load.destination.clone(),
            commodity: load.commodity.clone(),
            amount_cents: agreement.total_amount_cents,
            currency: load.currency.clone(),
            source_load_version: load.version,
            issuer: self.billing_profile.clone(),
            agreement: Some(agreement),
        })
    }
    // Each argument is a distinct journal attribute; the currency joined the
    // existing signature when postings stopped being implicitly USD.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn post(
        &mut self,
        source: (&str, &str),
        date: NaiveDate,
        actor: &str,
        memo: &str,
        currency: &str,
        lines: Vec<JournalLine>,
        reversal_of: Option<String>,
    ) -> Result<String> {
        let currency = domain::currency_code(currency)?;
        if self
            .journal
            .iter()
            .any(|v| v.source_kind == source.0 && v.source_id == source.1)
        {
            return Err("Financial source already posted".into());
        }
        if lines.is_empty()
            || lines.iter().any(|v| {
                v.debit_cents < 0
                    || v.credit_cents < 0
                    || (v.debit_cents == 0) == (v.credit_cents == 0)
                    || ![
                        "cash",
                        "accounts_receivable",
                        "freight_revenue",
                        "carrier_payable",
                        "freight_expense",
                    ]
                    .contains(&v.account.as_str())
            })
        {
            return Err("Invalid financial journal lines".into());
        }
        let debits = sum(lines.iter().map(|v| v.debit_cents))?;
        if debits != sum(lines.iter().map(|v| v.credit_cents))? {
            return Err("Financial journal is not balanced".into());
        }
        let id = domain::id("journal");
        self.journal.push(JournalEntry {
            id: id.clone(),
            source_kind: source.0.into(),
            source_id: source.1.into(),
            posted_on: date,
            recorded_at: Utc::now(),
            actor: actor.into(),
            currency,
            memo: memo.into(),
            reversal_of,
            lines,
        });
        Ok(id)
    }
    pub fn recognize_carrier_invoice(&mut self, id: &str) -> Result<()> {
        if self
            .journal
            .iter()
            .any(|v| v.source_kind == "carrier_invoice" && v.source_id == id)
        {
            return Ok(());
        }
        let invoice = self
            .invoices
            .iter()
            .find(|v| v.id == id && v.status == "approved")
            .ok_or("Approved invoice not found")?
            .clone();
        if invoice.amount_cents > 0 {
            self.post(
                ("carrier_invoice", id),
                Utc::now().date_naive(),
                "system",
                &format!("Carrier invoice {} approved", invoice.number),
                &invoice.currency,
                pair(
                    &invoice.load_id,
                    "freight_expense",
                    "carrier_payable",
                    invoice.amount_cents,
                ),
                None,
            )?;
        }
        Ok(())
    }
    pub fn bill_totals(&self, bill: &CustomerBill) -> Result<BillTotals> {
        let received = sum(self
            .cash_entries
            .iter()
            .filter(|v| v.direction == "receipt" && v.reversal.is_none())
            .flat_map(|v| v.allocations.iter())
            .filter(|v| v.target_id == bill.id)
            .map(|v| v.amount_cents))?;
        let credits = sum(self
            .credits
            .iter()
            .filter(|v| v.bill_id == bill.id && v.reversal.is_none())
            .map(|v| v.amount_cents))?;
        let refunded = sum(self
            .cash_entries
            .iter()
            .filter(|v| v.direction == "customer_refund" && v.reversal.is_none())
            .flat_map(|v| v.allocations.iter())
            .filter(|v| v.target_id == bill.id)
            .map(|v| v.amount_cents))?;
        BillTotals::from_activity(bill, received, credits, refunded)
    }
    pub fn settlement_balance(&self, id: &str) -> Result<i64> {
        let settlement = self
            .settlements
            .iter()
            .find(|v| v.id == id)
            .ok_or("Settlement not found")?;
        let reconciled = reconciled_cash(self.cash_entries.iter().flat_map(|v| {
            v.allocations
                .iter()
                .filter(|a| a.target_id == id)
                .map(move |a| (v, a))
        }))?;
        let credit_effect = self.carrier_credit_effect(id)?;
        let balance = self
            .settlement_total(settlement)?
            .checked_sub(reconciled)
            .and_then(|v| v.checked_add(credit_effect))
            .ok_or("Financial total overflow")?;
        Ok(balance)
    }
    pub(crate) fn refresh_target(&mut self, direction: &str, id: &str) -> Result<()> {
        if matches!(direction, "receipt" | "customer_refund") {
            let index = self
                .bills
                .iter()
                .position(|v| v.id == id)
                .ok_or("Customer invoice not found")?;
            let totals = self.bill_totals(&self.bills[index])?;
            let bill = &mut self.bills[index];
            bill.version = next(bill.version)?;
            if bill.status == "void" {
                return Ok(());
            }
            bill.status = if totals.balance_cents < 0 {
                "credit_due"
            } else if totals.balance_cents == 0 {
                if totals.received_cents == totals.refunded_cents {
                    "credited"
                } else {
                    "paid"
                }
            } else if totals.received_cents > 0 || totals.credited_cents > 0 {
                "partially_paid"
            } else {
                "issued"
            }
            .into();
        } else {
            let balance = self.settlement_balance(id)?;
            let total = self.settlement_total(
                self.settlements
                    .iter()
                    .find(|v| v.id == id)
                    .ok_or("Settlement not found")?,
            )?;
            let settlement = self
                .settlements
                .iter_mut()
                .find(|v| v.id == id)
                .ok_or("Settlement not found")?;
            settlement.version = next(settlement.version)?;
            settlement.status = if balance < 0 {
                "credit_due"
            } else if balance == 0 {
                "reconciled"
            } else if balance < total {
                "partially_reconciled"
            } else {
                "prepared"
            }
            .into();
        }
        Ok(())
    }
    pub fn apply_finance(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        match operation {
            "bill.profile" => {
                let input: ProfileInput = decode(body)?;
                version(
                    self.billing_profile.as_ref().map_or(0, |v| v.version),
                    input.expected_version,
                )?;
                domain::required(&input.legal_name, "Billing legal name")?;
                domain::required(&input.contact, "Billing contact")?;
                if input.address.trim().is_empty()
                    || input.address.len() > 1000
                    || input.payment_instructions.trim().is_empty()
                    || input.payment_instructions.len() > 2000
                {
                    return Err("Billing address and payment instructions are required (up to 1,000/2,000 characters)".into());
                }
                let profile = BillingProfile {
                    legal_name: input.legal_name,
                    address: input.address,
                    contact: input.contact,
                    payment_instructions: input.payment_instructions,
                    version: next(input.expected_version)?,
                };
                self.billing_profile = Some(profile.clone());
                self.record(
                    "billing_profile",
                    operation,
                    "Billing issuer identity updated; issued invoice snapshots retained",
                );
                Ok(json!(profile))
            }
            "bill.create" => {
                let input: NewBill = decode(body)?;
                let number = number(&input.number)?;
                if self.bills.iter().any(|v| v.number == number) {
                    return Err("Customer invoice number already exists".into());
                }
                if self
                    .bills
                    .iter()
                    .any(|v| v.load_id == input.load_id && v.status != "void")
                {
                    return Err("A customer invoice already exists for this load".into());
                }
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == input.load_id)
                    .ok_or("Load not found")?;
                if load.status != Status::Delivered {
                    return Err("Customer billing requires a delivered load".into());
                }
                if input.due_date < Utc::now().date_naive() {
                    return Err("Invoice due date cannot precede its issue date".into());
                }
                let mut snapshot = self.billing_snapshot(load)?;
                if let Some(address) = input.bill_to_address {
                    validate_address(&address)?;
                    snapshot.customer.billing_address = address;
                }
                let bill = CustomerBill {
                    id: domain::id("bill"),
                    number,
                    load_id: load.id.clone(),
                    customer_id: load.customer_id.clone(),
                    snapshot,
                    due_date: input.due_date,
                    status: "draft".into(),
                    version: 1,
                    created_at: Utc::now(),
                    created_by: actor.into(),
                    issued_at: None,
                    void_reason: None,
                    supplement: None,
                };
                self.record(
                    &bill.load_id,
                    operation,
                    format!(
                        "Customer invoice {} drafted from contracted sell rate",
                        bill.number
                    ),
                );
                let response = json!(bill);
                self.bills.push(bill);
                Ok(response)
            }
            "bill.supplement" => {
                let input: NewSupplement = decode(body)?;
                positive(input.amount_cents)?;
                let number = number(&input.number)?;
                if self.bills.iter().any(|v| v.number == number) {
                    return Err("Customer invoice number already exists".into());
                }
                for (value, label) in [
                    (&input.description, "Customer charge description"),
                    (&input.counterparty_contact, "Customer agreement contact"),
                    (
                        &input.confirmation_reference,
                        "Customer confirmation reference",
                    ),
                    (&input.evidence_reference, "Customer agreement reference"),
                ] {
                    domain::required(value, label)?;
                }
                if input.due_date < Utc::now().date_naive() {
                    return Err("Invoice due date cannot precede its issue date".into());
                }
                let parent_index = self
                    .bills
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Original customer invoice not found")?;
                let parent = self.bills[parent_index].clone();
                version(parent.version, input.expected_version)?;
                if parent.supplement.is_some()
                    || parent.issued_at.is_none()
                    || parent.status == "void"
                {
                    return Err(
                        "Supplement requires an active issued original customer invoice".into(),
                    );
                }
                let load_index = self
                    .loads
                    .iter()
                    .position(|v| v.id == parent.load_id)
                    .ok_or("Load not found")?;
                version(self.loads[load_index].version, input.expected_load_version)?;
                if !self.delivery_evidence_ready(&self.loads[load_index]) {
                    return Err(
                        "Reviewed delivery evidence required before drafting a supplement".into(),
                    );
                }
                let reference = input.evidence_reference.trim().to_string();
                let duplicates: Vec<_> = self
                    .bills
                    .iter()
                    .filter(|v| v.load_id == parent.load_id)
                    .filter(|v| {
                        v.supplement
                            .as_ref()
                            .is_some_and(|a| a.evidence_reference.eq_ignore_ascii_case(&reference))
                    })
                    .collect();
                if duplicates.iter().any(|v| v.status != "void") {
                    return Err(
                        "Customer agreement reference already supports an active supplement".into(),
                    );
                }
                if let Some(id) = &input.corrects_id {
                    let old = self
                        .bills
                        .iter()
                        .find(|v| &v.id == id)
                        .ok_or("Correction invoice not found")?;
                    let agreement = old
                        .supplement
                        .as_ref()
                        .ok_or("Only a supplement can be corrected here")?;
                    if old.status != "void"
                        || agreement.parent_bill_id != parent.id
                        || agreement.corrected_by_id.is_some()
                        || !agreement
                            .evidence_reference
                            .eq_ignore_ascii_case(&reference)
                    {
                        return Err("Correction must link the uncorrected void supplement for this original invoice and agreement reference".into());
                    }
                } else if !duplicates.is_empty() {
                    return Err("Voided agreement requires an explicit correction link".into());
                }
                let document_sha256 = if let Some(id) = &input.document_id {
                    let document = self
                        .documents
                        .iter()
                        .find(|v| &v.id == id && v.load_id == parent.load_id)
                        .ok_or("Agreement document not found")?;
                    if document.status() == "rejected" {
                        return Err("Agreement document is rejected".into());
                    }
                    Some(document.sha256.clone())
                } else {
                    None
                };
                let mut snapshot = parent.snapshot.clone();
                snapshot.amount_cents = input.amount_cents;
                snapshot.agreement = None;
                snapshot.issuer = self.billing_profile.clone();
                snapshot.source_load_version = self.loads[load_index].version;
                let bill = CustomerBill {
                    id: domain::id("bill"),
                    number,
                    load_id: parent.load_id.clone(),
                    customer_id: parent.customer_id.clone(),
                    snapshot,
                    due_date: input.due_date,
                    status: "draft".into(),
                    version: 1,
                    created_at: Utc::now(),
                    created_by: actor.into(),
                    issued_at: None,
                    void_reason: None,
                    supplement: Some(SupplementAgreement {
                        parent_bill_id: parent.id.clone(),
                        parent_number: parent.number.clone(),
                        description: input.description.trim().into(),
                        counterparty_contact: input.counterparty_contact.trim().into(),
                        confirmation_reference: input.confirmation_reference.trim().into(),
                        evidence_reference: reference,
                        document_id: input.document_id,
                        document_sha256,
                        corrects_id: input.corrects_id.clone(),
                        corrected_by_id: None,
                    }),
                };
                self.validate_supplement(&bill)?;
                if let Some(id) = input.corrects_id {
                    let old = self
                        .bills
                        .iter_mut()
                        .find(|v| v.id == id)
                        .ok_or("Correction invoice not found")?;
                    old.supplement
                        .as_mut()
                        .ok_or("Correction agreement not found")?
                        .corrected_by_id = Some(bill.id.clone());
                    old.version = next(old.version)?;
                }
                self.loads[load_index].version = next(self.loads[load_index].version)?;
                self.loads[load_index].updated_at = Utc::now();
                // The original financial snapshot and its posting remain immutable.
                self.bills[parent_index].version = next(parent.version)?;
                let response = json!(bill);
                self.record(
                    &parent.load_id,
                    operation,
                    format!("Customer supplement drafted: {response}"),
                );
                self.bills.push(bill);
                Ok(response)
            }
            "bill.revise" => {
                let input: ReviseBill = decode(body)?;
                let number = number(&input.number)?;
                if self
                    .bills
                    .iter()
                    .any(|v| v.id != entity && v.number == number)
                {
                    return Err("Customer invoice number already exists".into());
                }
                if input.due_date < Utc::now().date_naive() {
                    return Err("Invoice due date cannot precede its issue date".into());
                }
                let bill = self
                    .bills
                    .iter_mut()
                    .find(|v| v.id == entity)
                    .ok_or("Customer invoice not found")?;
                version(bill.version, input.expected_version)?;
                if bill.status != "draft" {
                    return Err("Only draft customer invoices can be revised".into());
                }
                bill.version = next(bill.version)?;
                bill.number = number;
                bill.due_date = input.due_date;
                if let Some(address) = input.bill_to_address {
                    validate_address(&address)?;
                    bill.snapshot.customer.billing_address = address;
                }
                let response = json!(bill);
                let load = bill.load_id.clone();
                self.record(&load, operation, "Draft customer invoice revised");
                Ok(response)
            }
            "bill.issue" => {
                let input: Version = decode(body)?;
                let index = self
                    .bills
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Customer invoice not found")?;
                let bill = &self.bills[index];
                version(bill.version, input.expected_version)?;
                if bill.status != "draft" {
                    return Err("Only draft customer invoices can be issued".into());
                }
                if bill.due_date < Utc::now().date_naive() {
                    return Err(
                        "Invoice due date cannot precede its issue date; revise the draft".into(),
                    );
                }
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == bill.load_id)
                    .ok_or("Load not found")?;
                if !self.delivery_evidence_ready(load) {
                    return Err(
                        "Reviewed delivery evidence required before issuing a customer invoice"
                            .into(),
                    );
                }
                if self.pending_charges(&load.id, "sell") {
                    return Err("Review pending customer charges before issuing its invoice".into());
                }
                self.validate_supplement(bill)?;
                let mut snapshot = if bill.supplement.is_some() {
                    let mut snapshot = bill.snapshot.clone();
                    snapshot.issuer = self.billing_profile.clone();
                    snapshot
                } else {
                    self.billing_snapshot(load)?
                };
                snapshot.customer.billing_address = bill.snapshot.customer.billing_address.clone();
                validate_address(&snapshot.customer.billing_address)?;
                if snapshot.issuer.is_none() {
                    return Err(
                        "Configure the billing issuer identity before issuing an invoice".into(),
                    );
                }
                let load_id = load.id.clone();
                let amount = snapshot.amount_cents;
                let currency = snapshot.currency.clone();
                let bill = &mut self.bills[index];
                bill.snapshot = snapshot;
                bill.issued_at = Some(Utc::now());
                bill.status = "issued".into();
                bill.version = next(bill.version)?;
                let response = json!(bill);
                let number = bill.number.clone();
                self.post(
                    ("customer_invoice", entity),
                    Utc::now().date_naive(),
                    actor,
                    &format!("Customer invoice {number} issued"),
                    &currency,
                    pair(&load_id, "accounts_receivable", "freight_revenue", amount),
                    None,
                )?;
                self.record(
                    &load_id,
                    operation,
                    format!(
                        "Customer invoice {number} issued; delivery to the customer is not implied"
                    ),
                );
                Ok(response)
            }
            "bill.void" => {
                let input: VoidBill = decode(body)?;
                domain::required(&input.note, "Void reason")?;
                let index = self
                    .bills
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Customer invoice not found")?;
                let bill = self.bills[index].clone();
                version(bill.version, input.expected_version)?;
                if self.cancellation_target(&bill.id).is_some() {
                    return Err("Reverse the cancellation fee agreement to void its invoice".into());
                }
                if bill.status == "void" {
                    return Err("Customer invoice already void".into());
                }
                if self.bills.iter().any(|v| {
                    v.status != "void"
                        && v.supplement
                            .as_ref()
                            .is_some_and(|a| a.parent_bill_id == bill.id)
                }) {
                    return Err(
                        "Void active supplements before voiding the original invoice".into(),
                    );
                }
                let totals = self.bill_totals(&bill)?;
                if totals.received_cents != totals.refunded_cents || totals.credited_cents > 0 {
                    return Err(
                        "Resolve net customer funds with actual refunds or correct erroneous receipts, and reverse active credit notes before voiding the invoice"
                            .into(),
                    );
                }
                if bill.issued_at.is_some() {
                    self.reverse_post(
                        "customer_invoice",
                        entity,
                        "customer_invoice_void",
                        Utc::now().date_naive(),
                        actor,
                        &input.note,
                    )?;
                }
                let current = &mut self.bills[index];
                current.status = "void".into();
                current.version = next(current.version)?;
                current.void_reason = Some(input.note.clone());
                let response = json!(current);
                self.record(
                    &bill.load_id,
                    operation,
                    format!("Customer invoice {} voided: {}", bill.number, input.note),
                );
                Ok(response)
            }
            "credit.create" => {
                let input: NewCredit = decode(body)?;
                positive(input.amount_cents)?;
                domain::required(&input.reason, "Credit reason")?;
                let number = number(&input.number)?;
                if self.credits.iter().any(|v| v.number == number) {
                    return Err("Credit note number already exists".into());
                }
                let bill = self
                    .bills
                    .iter()
                    .find(|v| v.id == entity)
                    .ok_or("Customer invoice not found")?
                    .clone();
                version(bill.version, input.expected_version)?;
                if bill.issued_at.is_none() || bill.status == "void" {
                    return Err("Credit notes require an issued customer invoice".into());
                }
                let issued_on = input.issued_on.unwrap_or_else(|| Utc::now().date_naive());
                valid_date(
                    issued_on,
                    bill.issued_at.ok_or("Issue date required")?.date_naive(),
                )?;
                let totals = self.bill_totals(&bill)?;
                if input.amount_cents > totals.remaining_creditable_cents {
                    return Err("Credit exceeds the remaining invoice amount".into());
                }
                if input.amount_cents > totals.balance_cents && input.confirmation.is_none() {
                    return Err(
                        "Customer confirmation is required when a credit reduces received funds"
                            .into(),
                    );
                }
                let confirmation = input
                    .confirmation
                    .map(|v| {
                        self.customer_confirmation(v, std::slice::from_ref(&bill.load_id), actor)
                    })
                    .transpose()?;
                let credit = CreditNote {
                    issued_on: Some(issued_on),
                    confirmation,
                    id: domain::id("credit"),
                    number,
                    bill_id: bill.id.clone(),
                    load_id: bill.load_id.clone(),
                    amount_cents: input.amount_cents,
                    reason: input.reason,
                    created_at: Utc::now(),
                    created_by: actor.into(),
                    version: 1,
                    reversal: None,
                };
                self.post(
                    ("credit_note", &credit.id),
                    issued_on,
                    actor,
                    &credit.reason,
                    &bill.snapshot.currency,
                    pair(
                        &bill.load_id,
                        "freight_revenue",
                        "accounts_receivable",
                        credit.amount_cents,
                    ),
                    None,
                )?;
                let response = json!(credit);
                self.credits.push(credit);
                self.refresh_target("receipt", entity)?;
                self.record(&bill.load_id, operation, "Customer credit note posted");
                Ok(response)
            }
            "credit.reverse" => {
                let input: ReverseInput = decode(body)?;
                domain::required(&input.note, "Reversal note")?;
                let index = self
                    .credits
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Credit note not found")?;
                let credit = self.credits[index].clone();
                version(credit.version, input.expected_version)?;
                if credit.reversal.is_some() {
                    return Err("Credit note already reversed".into());
                }
                let bill = self
                    .bills
                    .iter()
                    .find(|v| v.id == credit.bill_id)
                    .ok_or("Customer invoice not found")?;
                if self.has_customer_refund_history(&bill.id)
                    && input.expected_bill_version.is_none()
                {
                    return Err("Current invoice version is required when reversing a credit with refund history".into());
                }
                if let Some(expected) = input.expected_bill_version {
                    version(bill.version, expected)?;
                }
                valid_date(input.reversed_on, self.customer_activity_date(bill))?;
                let journal = self.reverse_post(
                    "credit_note",
                    entity,
                    "credit_note_reversal",
                    input.reversed_on,
                    actor,
                    &input.note,
                )?;
                self.credits[index].reversal = Some(Reversal {
                    note: input.note.clone(),
                    reversed_on: input.reversed_on,
                    actor: actor.into(),
                    journal_id: journal,
                });
                self.credits[index].version = next(credit.version)?;
                self.refresh_target("receipt", &credit.bill_id)?;
                self.record(
                    &credit.load_id,
                    operation,
                    format!("Credit note reversed: {}", input.note),
                );
                Ok(json!(self.credits[index]))
            }
            "cash.record" => self.record_cash(decode(body)?, actor),
            "cash.reverse" => self.reverse_cash(entity, decode(body)?, actor),
            _ => Err("Unsupported financial operation".into()),
        }
    }
    pub(crate) fn reverse_post(
        &mut self,
        source_kind: &str,
        source_id: &str,
        reversal_kind: &str,
        date: NaiveDate,
        actor: &str,
        note: &str,
    ) -> Result<String> {
        let original = self
            .journal
            .iter()
            .find(|v| v.source_kind == source_kind && v.source_id == source_id)
            .ok_or("Original journal entry not found")?
            .clone();
        let lines = original
            .lines
            .into_iter()
            .map(|v| JournalLine {
                load_id: v.load_id,
                account: v.account,
                debit_cents: v.credit_cents,
                credit_cents: v.debit_cents,
            })
            .collect();
        self.post(
            (reversal_kind, source_id),
            date,
            actor,
            note,
            &original.currency,
            lines,
            Some(original.id),
        )
    }
    fn carrier_payout_start_date(&self, load: &Load) -> Result<NaiveDate> {
        // A plan is not an actual custody timestamp. Native reports retain
        // authenticated pickup evidence and source-bound reviewed corrections.
        // Use the first effective pickup for the current assignment, never an
        // EDI processing timestamp, gate arrival or unrelated carrier report.
        if !self.execution_consistent(load) {
            return Err("Resolve movement evidence before carrier payout reconciliation".into());
        }
        let mut earliest = None;
        for source in self.carrier_submissions.iter().filter(|source| {
            source.load_id == load.id
                && Some(&source.tender_id) == load.tender_id.as_ref()
                && Some(&source.carrier_id) == load.carrier_id.as_ref()
                && source.outcome == "applied"
                && source.kind == "pickup"
        }) {
            let observed = self
                .effective_movement_time(source)?
                .ok_or("Actual carrier pickup time required")?;
            if observed > Utc::now() {
                return Err("Actual carrier pickup time cannot be in the future".into());
            }
            earliest = Some(earliest.map_or(observed, |prior: DateTime<Utc>| prior.min(observed)));
        }
        Ok(earliest.map_or(load.origin.date, |time| time.date_naive()))
    }
    fn record_cash(&mut self, input: NewCash, actor: &str) -> Result<Value> {
        if !["receipt", "payout", "carrier_refund", "customer_refund"]
            .contains(&input.direction.as_str())
        {
            return Err(
                "Cash reconciliation requires receipt/payout/carrier_refund/customer_refund".into(),
            );
        }
        let currency = domain::currency_code(&input.currency)?;
        domain::required(&input.account, "Bank account label")?;
        domain::required(&input.external_reference, "Cleared bank reference")?;
        domain::required(&input.note, "Reconciliation note")?;
        if input.amount_cents <= 0 || input.allocations.is_empty() || input.allocations.len() > 100
        {
            return Err("Positive bank amount and 1–100 allocations required".into());
        }
        let account = input.account.trim().to_ascii_lowercase();
        let reference = input.external_reference.trim().to_owned();
        let matching: Vec<_> = self
            .cash_entries
            .iter()
            .filter(|v| v.account == account && v.external_reference == reference)
            .collect();
        if matching.iter().any(|v| v.reversal.is_none()) {
            return Err("Bank reference already reconciled".into());
        }
        let prior = if let Some(id) = &input.corrects_id {
            let prior = self
                .cash_entries
                .iter()
                .find(|v| v.id == *id)
                .ok_or("Corrected cash entry not found")?;
            if prior.account != account
                || prior.external_reference != reference
                || prior.direction != input.direction
                || prior.currency != currency
                || prior.reversal.is_none()
                || prior.corrected_by_id.is_some()
            {
                return Err("Correction must identify the uncorrected reversed entry for this exact bank reference".into());
            }
            Some(prior.clone())
        } else {
            if !matching.is_empty() {
                return Err("A reversed bank reference requires an explicit corrects_id".into());
            }
            None
        };
        let mut seen = BTreeSet::new();
        let mut party: Option<String> = None;
        let mut allocations = Vec::new();
        let mut lines = Vec::new();
        for allocation in input.allocations {
            positive(allocation.amount_cents)?;
            if !seen.insert(allocation.target_id.clone()) {
                return Err("Duplicate cash allocation target".into());
            }
            let (party_id, load_id, balance, earliest) = if matches!(
                input.direction.as_str(),
                "receipt" | "customer_refund"
            ) {
                let bill = self
                    .bills
                    .iter()
                    .find(|v| {
                        v.id == allocation.target_id
                            && v.issued_at.is_some()
                            && (input.direction == "customer_refund" || v.status != "void")
                    })
                    .ok_or("Issued customer invoice not found")?;
                if bill.snapshot.currency != currency {
                    return Err("Cash currency must match the customer invoice currency".into());
                }
                version(bill.version, allocation.expected_version)?;
                (
                    bill.customer_id.clone(),
                    bill.load_id.clone(),
                    if input.direction == "customer_refund" {
                        self.bill_totals(bill)?.refund_due_cents
                    } else {
                        self.bill_totals(bill)?.balance_cents
                    },
                    if input.direction == "customer_refund" {
                        self.customer_activity_date(bill)
                    } else {
                        bill.issued_at.ok_or("Issue date required")?.date_naive()
                    },
                )
            } else {
                let settlement = self
                    .settlements
                    .iter()
                    .find(|v| v.id == allocation.target_id)
                    .ok_or("Settlement not found")?;
                version(settlement.version, allocation.expected_version)?;
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == settlement.load_id)
                    .ok_or("Load not found")?;
                if load.currency != currency {
                    return Err("Cash currency must match the settlement currency".into());
                }
                (
                    settlement.carrier_id.clone(),
                    settlement.load_id.clone(),
                    if input.direction == "carrier_refund" {
                        -self.settlement_balance(&settlement.id)?
                    } else {
                        self.settlement_balance(&settlement.id)?
                    },
                    if let Some(fee) = self.cancellation_target(&settlement.id) {
                        if self.unresolved_tender_incidents(&fee.load_id) {
                            return Err("Resolve carrier custody incidents before cancellation payment reconciliation".into());
                        }
                        if input.direction == "carrier_refund" {
                            fee.reversal
                                .as_ref()
                                .ok_or("Reversed cancellation fee required before a refund")?
                                .reversed_on
                        } else {
                            fee.posted_at
                                .ok_or("Cancellation posting date required")?
                                .date_naive()
                        }
                    } else if input.direction == "carrier_refund" {
                        self.carrier_refund_start_date(&load.id)
                            .ok_or("Posted carrier credit required before a refund")?
                    } else {
                        self.carrier_payout_start_date(load)?
                    },
                )
            };
            valid_date(input.cleared_on, earliest)?;
            if party.as_ref().is_some_and(|v| v != &party_id) {
                return Err(
                    "All cash allocations must belong to the same customer or carrier".into(),
                );
            }
            party = Some(party_id);
            if allocation.amount_cents > balance {
                return Err("Cash allocation exceeds the outstanding balance".into());
            }
            lines.extend(if input.direction == "receipt" {
                pair(
                    &load_id,
                    "cash",
                    "accounts_receivable",
                    allocation.amount_cents,
                )
            } else if input.direction == "customer_refund" {
                pair(
                    &load_id,
                    "accounts_receivable",
                    "cash",
                    allocation.amount_cents,
                )
            } else if input.direction == "payout" {
                pair(&load_id, "carrier_payable", "cash", allocation.amount_cents)
            } else {
                pair(&load_id, "cash", "carrier_payable", allocation.amount_cents)
            });
            allocations.push(CashAllocation {
                target_id: allocation.target_id,
                load_id,
                amount_cents: allocation.amount_cents,
            });
        }
        if sum(allocations.iter().map(|v| v.amount_cents))? != input.amount_cents {
            return Err("Allocations must equal the cleared bank amount exactly".into());
        }
        let party_id = party.ok_or("Cash party required")?;
        let confirmation = if input.direction == "customer_refund" {
            Some(
                self.customer_confirmation(
                    input
                        .confirmation
                        .ok_or("Actual customer refund confirmation is required")?,
                    &allocations
                        .iter()
                        .map(|v| v.load_id.clone())
                        .collect::<Vec<_>>(),
                    actor,
                )?,
            )
        } else {
            if input.confirmation.is_some() {
                return Err(
                    "Customer confirmation fields apply only to customer refund reconciliation"
                        .into(),
                );
            }
            None
        };
        let cash = CashEntry {
            customer_confirmation: confirmation,
            id: domain::id("cash"),
            direction: input.direction,
            party_id,
            account,
            external_reference: reference,
            amount_cents: input.amount_cents,
            currency: currency.clone(),
            cleared_on: input.cleared_on,
            note: input.note,
            allocations,
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
            version: 1,
            reversal: None,
            corrects_id: input.corrects_id,
            corrected_by_id: None,
        };
        self.post(
            ("cash", &cash.id),
            cash.cleared_on,
            actor,
            &cash.note,
            &cash.currency,
            lines,
            None,
        )?;
        if let Some(prior) = prior {
            let old = self
                .cash_entries
                .iter_mut()
                .find(|v| v.id == prior.id)
                .ok_or("Cash entry not found")?;
            old.corrected_by_id = Some(cash.id.clone());
            old.version = next(old.version)?;
        }
        self.cash_entries.push(cash.clone());
        for allocation in &cash.allocations {
            self.refresh_target(&cash.direction, &allocation.target_id)?;
            self.record(
                &allocation.load_id,
                "cash.recorded",
                format!(
                    "External {} reconciled: {} cents · {}",
                    cash.direction, allocation.amount_cents, cash.external_reference
                ),
            );
        }
        Ok(json!(cash))
    }
    fn reverse_cash(&mut self, id: &str, input: ReverseInput, actor: &str) -> Result<Value> {
        domain::required(&input.note, "Reversal note")?;
        let index = self
            .cash_entries
            .iter()
            .position(|v| v.id == id)
            .ok_or("Cash entry not found")?;
        let cash = self.cash_entries[index].clone();
        version(cash.version, input.expected_version)?;
        if cash.reversal.is_some() {
            return Err("Cash entry already reversed".into());
        }
        if input.expected_bill_version.is_some() {
            return Err("Invoice version is used only for customer credit reversal".into());
        }
        let earliest = if matches!(cash.direction.as_str(), "receipt" | "customer_refund") {
            cash.allocations
                .iter()
                .filter_map(|a| self.bills.iter().find(|v| v.id == a.target_id))
                .map(|bill| self.customer_activity_date(bill))
                .max()
                .unwrap_or(cash.cleared_on)
                .max(cash.cleared_on)
        } else {
            cash.cleared_on
        };
        valid_date(input.reversed_on, earliest)?;
        let journal = self.reverse_post(
            "cash",
            id,
            "cash_reversal",
            input.reversed_on,
            actor,
            &input.note,
        )?;
        self.cash_entries[index].reversal = Some(Reversal {
            note: input.note.clone(),
            reversed_on: input.reversed_on,
            actor: actor.into(),
            journal_id: journal,
        });
        self.cash_entries[index].version = next(cash.version)?;
        for allocation in &cash.allocations {
            self.refresh_target(&cash.direction, &allocation.target_id)?;
            self.record(
                &allocation.load_id,
                "cash.reversed",
                format!("Cash reconciliation reversed: {}", input.note),
            );
        }
        Ok(json!(self.cash_entries[index]))
    }
    pub fn finance_summary(&self) -> Result<Value> {
        let totals = self
            .bills
            .iter()
            .map(|v| self.bill_totals(v))
            .collect::<Result<Vec<_>>>()?;
        let balances: Vec<_> = self
            .bills
            .iter()
            .zip(totals.iter())
            .map(|(bill, total)| (bill.snapshot.currency.as_str(), total))
            .collect();
        let payable = self
            .settlements
            .iter()
            .map(|v| {
                let currency = self
                    .loads
                    .iter()
                    .find(|load| load.id == v.load_id)
                    .ok_or("Settlement load not found")?
                    .currency
                    .as_str();
                Ok((currency, self.settlement_balance(&v.id)?))
            })
            .collect::<Result<Vec<_>>>()?;
        summary_value(&self.journal, balances, payable)
    }
}
/// Build the finance summary with one figures group per currency. Top-level
/// totals retain their historical meaning as the USD group (zero when the
/// tenant has no USD activity); `currencies` always carries every group, so
/// no consumer ever sums amounts across currencies.
pub(crate) fn summary_value(
    journal: &[JournalEntry],
    balances: Vec<(&str, &BillTotals)>,
    payable: Vec<(&str, i64)>,
) -> Result<Value> {
    let mut accounts: BTreeMap<(String, String), (i64, i64)> = BTreeMap::new();
    for entry in journal {
        for line in &entry.lines {
            let row = accounts
                .entry((line.account.clone(), entry.currency.clone()))
                .or_default();
            row.0 = row
                .0
                .checked_add(line.debit_cents)
                .ok_or("Financial total overflow")?;
            row.1 = row
                .1
                .checked_add(line.credit_cents)
                .ok_or("Financial total overflow")?;
        }
    }
    let rows: Vec<_> = accounts
        .into_iter()
        .map(|((account, currency), (debit, credit))| {
            Ok(json!({"account":account,"currency":currency,"debit_cents":debit,"credit_cents":credit,"net_debit_cents":debit.checked_sub(credit).ok_or("Financial total overflow")?}))
        })
        .collect::<Result<Vec<_>>>()?;
    #[derive(Default)]
    struct Figures {
        receivable: Vec<i64>,
        balances: Vec<i64>,
        overdue: Vec<i64>,
        payable: Vec<i64>,
    }
    let mut groups: BTreeMap<&str, Figures> = BTreeMap::new();
    groups.insert("USD", Figures::default());
    for (currency, total) in &balances {
        let group = groups.entry(currency).or_default();
        if total.balance_cents > 0 {
            group.receivable.push(total.balance_cents);
        }
        if total.overdue_days > 0 {
            group.overdue.push(total.balance_cents);
        }
        group.balances.push(total.balance_cents);
    }
    for (currency, balance) in &payable {
        groups.entry(currency).or_default().payable.push(*balance);
    }
    let mut currencies = Vec::new();
    let mut legacy = None;
    for (currency, group) in groups {
        let figures = json!({"currency":currency,"receivable_cents":sum(group.receivable.into_iter())?,"customer_credit_cents":credit_sum(group.balances.iter().copied())?,"net_receivable_cents":sum(group.balances.into_iter())?,"overdue_cents":sum(group.overdue.into_iter())?,"prepared_payable_cents":sum(group.payable.iter().copied().filter(|v|*v>0))?,"carrier_credit_cents":credit_sum(group.payable.into_iter())?});
        if currency == "USD" {
            legacy = Some(figures.clone());
        }
        currencies.push(figures);
    }
    let legacy = legacy.ok_or("USD summary group required")?;
    Ok(
        json!({"currency":"USD","accounts":rows,"currencies":currencies,"receivable_cents":legacy["receivable_cents"],"customer_credit_cents":legacy["customer_credit_cents"],"net_receivable_cents":legacy["net_receivable_cents"],"overdue_cents":legacy["overdue_cents"],"prepared_payable_cents":legacy["prepared_payable_cents"],"carrier_credit_cents":legacy["carrier_credit_cents"],"journal_entries":journal.len()}),
    )
}
fn valid_date(date: NaiveDate, earliest: NaiveDate) -> Result<()> {
    if date < earliest || date > Utc::now().date_naive() {
        return Err("Clearing/reversal date must be on or after its source date and cannot be in the future".into());
    }
    Ok(())
}
fn validate_address(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 1000 {
        Err("Customer bill-to address is required (up to 1,000 characters)".into())
    } else {
        Ok(())
    }
}

fn html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn dollars(cents: i64, currency: &str) -> String {
    stateset_freight_commerce::parse_currency(currency).map_or_else(
        |_| {
            let amount = cents.unsigned_abs();
            format!(
                "{}{}.{:02}",
                if cents < 0 { "-" } else { "" },
                amount / 100,
                amount % 100
            )
        },
        |code| stateset_freight_commerce::format_money(cents, code),
    )
}
pub fn statement(workspace: &Workspace, id: &str) -> Result<(String, String)> {
    let bill = workspace
        .bills
        .iter()
        .find(|v| v.id == id)
        .ok_or("Customer invoice not found")?;
    let totals = workspace.bill_totals(bill)?;
    let snapshot = &bill.snapshot;
    let filename: String = bill
        .number
        .chars()
        .map(|v| {
            if v.is_ascii_alphanumeric() || v == '-' {
                v
            } else {
                '_'
            }
        })
        .collect();
    let cancellation = workspace.cancellation_target(&bill.id);
    let label = if cancellation.is_some() {
        if bill.status == "void" {
            "VOID CANCELLATION INVOICE"
        } else {
            "CANCELLATION INVOICE"
        }
    } else if bill.status == "draft" {
        if bill.supplement.is_some() {
            "DRAFT SUPPLEMENTAL CUSTOMER INVOICE"
        } else {
            "DRAFT CUSTOMER INVOICE"
        }
    } else if bill.status == "void" {
        if bill.supplement.is_some() {
            "VOID SUPPLEMENTAL CUSTOMER INVOICE"
        } else {
            "VOID CUSTOMER INVOICE"
        }
    } else if bill.supplement.is_some() {
        "SUPPLEMENTAL CUSTOMER INVOICE"
    } else {
        "CUSTOMER INVOICE"
    };
    let issued = bill
        .issued_at
        .map(|v| v.date_naive().to_string())
        .unwrap_or_else(|| "Not issued".into());
    let credits = workspace
        .credits
        .iter()
        .filter(|v| v.bill_id == id && v.reversal.is_none())
        .map(|v| {
            format!(
                "<tr><td>Credit {}</td><td>{} {}</td></tr>",
                html(&v.number),
                html(&snapshot.currency),
                dollars(v.amount_cents, &snapshot.currency)
            )
        })
        .collect::<String>();
    let issuer = snapshot
        .issuer
        .as_ref()
        .map(|profile| {
            format!(
                "<h2>{}</h2><p>{}<br>{}</p>",
                html(&profile.legal_name),
                html(&profile.address),
                html(&profile.contact)
            )
        })
        .unwrap_or_else(|| "<p>Billing issuer identity is not configured.</p>".into());
    let instructions = snapshot
        .issuer
        .as_ref()
        .map(|profile| html(&profile.payment_instructions))
        .unwrap_or_default();
    let base_amount = snapshot
        .agreement
        .as_ref()
        .map_or(snapshot.amount_cents, |v| v.base_amount_cents);
    let charge_lines = snapshot
        .agreement
        .as_ref()
        .map(|basis| {
            basis
                .charges
                .iter()
                .map(|charge| {
                    format!(
                        "<tr><td>{}</td><td>{} {}</td></tr>",
                        html(&charge.description),
                        html(&snapshot.currency),
                        dollars(charge.amount_cents, &snapshot.currency)
                    )
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let output = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{label} {number}</title><style>body{{font:15px system-ui,sans-serif;color:#18332d;max-width:820px;margin:48px auto;padding:24px}}h1{{font-size:28px}}.meta{{color:#526860}}table{{width:100%;border-collapse:collapse;margin:24px 0}}td,th{{padding:12px;text-align:left;border-bottom:1px solid #dbe6e0}}td:last-child,th:last-child{{text-align:right}}.total{{font-size:20px;font-weight:700}}@media print{{body{{margin:0}}}}</style><body><p class="meta">STATESET FREIGHT · {label}</p><h1>{number}</h1><p>Status: {status} · Issued: {issued} · Due: {due}</p>{issuer}<h2>Bill to</h2><p>{customer}<br>{bill_to_address}<br>{contact}</p><h2>Shipment {reference}</h2><p>{origin} → {destination}<br>{shipment_dates}</p><table><thead><tr><th>Description</th><th>Amount</th></tr></thead><tbody>{base_line}{charge_lines}<tr><td>Invoice total</td><td>{currency} {amount}</td></tr>{void_line}{credits}<tr><td>Reconciled receipts</td><td>{currency} {received}</td></tr><tr><td>Reconciled customer refunds</td><td>{currency} {refunded}</td></tr><tr class="total"><td>{balance_label}</td><td>{currency} {balance}</td></tr></tbody></table><h2>Payment instructions</h2><p>{instructions}</p><p class="meta">{rate_note} Issuance records the invoice; electronic delivery is recorded separately by your billing process.</p></body></html>"#,
        number = html(&bill.number),
        status = html(&bill.status),
        issued = issued,
        due = bill.due_date,
        issuer = match &bill.supplement {
            Some(agreement) => format!(
                "<p>Supplement to invoice {} · Customer confirmation: {}</p>{issuer}",
                html(&agreement.parent_number),
                html(&agreement.confirmation_reference)
            ),
            None => issuer,
        },
        currency = html(&snapshot.currency),
        rate_note = if cancellation.is_some() {
            "This invoice records only the agreed cancellation fee. Transport was not completed."
                .to_string()
        } else {
            format!(
                "Amounts follow the agreed inclusive {} freight rate.",
                html(&snapshot.currency)
            )
        },
        instructions = instructions,
        bill_to_address = html(&snapshot.customer.billing_address),
        customer = html(&snapshot.customer.name),
        contact = html(&snapshot.customer.contact),
        reference = html(&snapshot.load_reference),
        origin = html(&format!(
            "{}, {} · {}",
            snapshot.origin.city, snapshot.origin.state, snapshot.origin.facility
        )),
        destination = html(&format!(
            "{}, {} · {}",
            snapshot.destination.city, snapshot.destination.state, snapshot.destination.facility
        )),
        shipment_dates = if cancellation.is_some() {
            format!(
                "Shipment cancelled · Planned pickup {} · Planned delivery {}",
                snapshot.origin.date, snapshot.destination.date
            )
        } else {
            format!(
                "Pickup {} · Delivery {}",
                snapshot.origin.date, snapshot.destination.date
            )
        },
        base_line = if cancellation.is_some() {
            String::new()
        } else {
            format!(
                "<tr><td>Freight services · {}</td><td>{} {}</td></tr>",
                html(
                    bill.supplement
                        .as_ref()
                        .map_or(snapshot.commodity.as_str(), |v| v.description.as_str())
                ),
                html(&snapshot.currency),
                dollars(base_amount, &snapshot.currency)
            )
        },
        amount = dollars(snapshot.amount_cents, &snapshot.currency),
        charge_lines = charge_lines,
        void_line = if bill.status == "void" && bill.issued_at.is_some() {
            format!(
                "<tr><td>Invoice void/retraction</td><td>{} -{}</td></tr>",
                html(&snapshot.currency),
                dollars(snapshot.amount_cents, &snapshot.currency)
            )
        } else {
            String::new()
        },
        received = dollars(totals.received_cents, &snapshot.currency),
        refunded = dollars(totals.refunded_cents, &snapshot.currency),
        balance_label = if totals.balance_cents < 0 {
            "Customer credit / refund due"
        } else {
            "Outstanding balance"
        },
        balance = dollars(totals.balance_cents.abs(), &snapshot.currency)
    );
    let output = if snapshot.intermediate_stops.is_empty() {
        output
    } else {
        output.replace(
            "</html>",
            &format!(
                "{}</html>",
                crate::itinerary::route_document(&snapshot.intermediate_stops)
            ),
        )
    };
    Ok((filename, output))
}
fn csv(value: &str) -> String {
    let protect = if value.trim_start().starts_with(['=', '+', '-', '@'])
        || value.starts_with(['\t', '\r', '\n'])
    {
        "'"
    } else {
        ""
    };
    format!("\"{protect}{}\"", value.replace('"', "\"\""))
}
pub fn journal_csv(workspace: &Workspace) -> String {
    let mut rows=vec!["journal_id,posted_on,source_kind,source_id,reversal_of,load_id,account,debit_cents,credit_cents,currency,memo,actor".into()];
    for entry in &workspace.journal {
        for line in &entry.lines {
            rows.push(
                [
                    entry.id.clone(),
                    entry.posted_on.to_string(),
                    entry.source_kind.clone(),
                    entry.source_id.clone(),
                    entry.reversal_of.clone().unwrap_or_default(),
                    line.load_id.clone(),
                    line.account.clone(),
                    line.debit_cents.to_string(),
                    line.credit_cents.to_string(),
                    entry.currency.clone(),
                    entry.memo.clone(),
                    entry.actor.clone(),
                ]
                .iter()
                .map(|v| csv(v))
                .collect::<Vec<_>>()
                .join(","),
            );
        }
    }
    rows.join("\r\n") + "\r\n"
}
