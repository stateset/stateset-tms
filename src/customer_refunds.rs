//! Customer credit liabilities and actual bank refund evidence. No transfer is
//! executed and no genuine receipt is reversed to manufacture a refund.
use crate::{
    domain::{self, Result, Workspace},
    finance::CustomerBill,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomerConfirmation {
    pub counterparty_contact: String,
    pub confirmation_reference: String,
    pub evidence_reference: String,
    pub note: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmationInput {
    pub counterparty_contact: String,
    pub confirmation_reference: String,
    pub evidence_reference: String,
    pub note: String,
    pub document_id: Option<String>,
}
impl Workspace {
    pub(crate) fn customer_confirmation(
        &self,
        input: ConfirmationInput,
        loads: &[String],
        actor: &str,
    ) -> Result<CustomerConfirmation> {
        for (value, label) in [
            (&input.counterparty_contact, "Actual customer contact"),
            (
                &input.confirmation_reference,
                "Customer confirmation reference",
            ),
            (
                &input.evidence_reference,
                "Original customer agreement/bank evidence",
            ),
            (&input.note, "Customer findings"),
        ] {
            domain::required(value, label)?;
        }
        let digest = if let Some(id) = &input.document_id {
            let doc = self
                .documents
                .iter()
                .find(|v| &v.id == id && loads.contains(&v.load_id) && v.status() != "rejected")
                .ok_or(
                    "Customer evidence must belong to an affected shipment and not be rejected",
                )?;
            Some(doc.sha256.clone())
        } else {
            None
        };
        Ok(CustomerConfirmation {
            counterparty_contact: input.counterparty_contact.trim().into(),
            confirmation_reference: input.confirmation_reference.trim().into(),
            evidence_reference: input.evidence_reference.trim().into(),
            note: input.note.trim().into(),
            document_id: input.document_id,
            document_sha256: digest,
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
        })
    }
    pub fn customer_activity_date(&self, bill: &CustomerBill) -> NaiveDate {
        let credit_dates = self
            .credits
            .iter()
            .filter(|v| v.bill_id == bill.id)
            .flat_map(|v| {
                std::iter::once(v.issued_on.unwrap_or(v.created_at.date_naive()))
                    .chain(v.reversal.as_ref().map(|r| r.reversed_on))
            });
        let bank_dates = self
            .cash_entries
            .iter()
            .filter(|v| v.allocations.iter().any(|a| a.target_id == bill.id))
            .flat_map(|v| {
                std::iter::once(v.cleared_on).chain(v.reversal.as_ref().map(|r| r.reversed_on))
            });
        let void_dates = self
            .journal
            .iter()
            .filter(|v| v.source_id == bill.id && v.source_kind == "customer_invoice_void")
            .map(|v| v.posted_on);
        credit_dates
            .chain(bank_dates)
            .chain(void_dates)
            .chain(
                self.cancellation_target(&bill.id)
                    .and_then(|v| v.reversal.as_ref())
                    .map(|v| v.reversed_on),
            )
            .max()
            .unwrap_or(bill.issued_at.unwrap_or(bill.created_at).date_naive())
            .max(bill.issued_at.unwrap_or(bill.created_at).date_naive())
    }
    pub(crate) fn has_customer_refund_history(&self, bill: &str) -> bool {
        self.cash_entries.iter().any(|v| {
            v.direction == "customer_refund" && v.allocations.iter().any(|a| a.target_id == bill)
        })
    }
    pub(crate) fn customer_confirmation_documents<'a>(
        &'a self,
        action: &str,
        response: &Value,
    ) -> Result<Vec<&'a crate::documents::Document>> {
        let mut confirmations = Vec::new();
        if action == "credit.create" {
            let credit = self
                .credits
                .iter()
                .find(|v| Some(v.id.as_str()) == response["data"]["id"].as_str())
                .ok_or("Internal storage error")?;
            if let Some(c) = &credit.confirmation {
                confirmations.push(c);
            }
        } else if action == "cash.record" && response["data"]["direction"] == "customer_refund" {
            let cash = self
                .cash_entries
                .iter()
                .find(|v| Some(v.id.as_str()) == response["data"]["id"].as_str())
                .ok_or("Internal storage error")?;
            if let Some(c) = &cash.customer_confirmation {
                confirmations.push(c);
            }
            for allocation in &cash.allocations {
                confirmations.extend(
                    self.credits
                        .iter()
                        .filter(|v| v.bill_id == allocation.target_id)
                        .filter_map(|v| v.confirmation.as_ref()),
                );
            }
        }
        let mut docs = Vec::new();
        for proof in confirmations {
            if let Some(id) = &proof.document_id {
                let doc = self
                    .documents
                    .iter()
                    .find(|v| &v.id == id)
                    .ok_or("Internal storage error")?;
                if proof.document_sha256.as_ref() != Some(&doc.sha256) {
                    return Err("Internal storage error".into());
                }
                docs.push(doc);
            }
        }
        Ok(docs)
    }
    pub(crate) fn customer_finance_document_locked(&self, id: &str) -> bool {
        self.credits.iter().any(|v| {
            v.confirmation
                .as_ref()
                .is_some_and(|c| c.document_id.as_deref() == Some(id))
                && (v.reversal.is_none() || self.has_customer_refund_history(&v.bill_id))
        }) || self.cash_entries.iter().any(|v| {
            v.direction == "customer_refund"
                && v.customer_confirmation
                    .as_ref()
                    .is_some_and(|c| c.document_id.as_deref() == Some(id))
        }) || self.bills.iter().any(|bill| {
            bill.supplement
                .as_ref()
                .is_some_and(|a| a.document_id.as_deref() == Some(id))
                && self.has_customer_refund_history(&bill.id)
        })
    }
}
