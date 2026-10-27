//! Independent review of primary carrier invoices. Provenance stays in the
//! append-only activity records; old approvals and their receipts stay intact.
use crate::domain::{Event, Invoice, Load, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const INDEPENDENT_REVIEW: &str =
    "Invoice approval requires a different named finance user from its entry, corrections, and entry reviews";
pub const NAMED_REVIEWER: &str =
    "Invoice review requires a named staff subject; configure a scoped credential or sign in";

fn named(actor: &str) -> bool {
    !actor.trim().is_empty()
        && actor == actor.trim()
        && actor.len() <= 100
        && !actor.chars().any(char::is_control)
        && !matches!(actor, "system" | "stateset-edi" | "legacy")
        && !actor.starts_with("credential:")
}

// Build once for a workspace projection, including deferred-history reads.
pub(crate) struct ReviewIndex<'a> {
    events: BTreeMap<&'a str, Vec<&'a Event>>,
    approvals: BTreeMap<&'a str, Vec<&'a Event>>,
    loads: BTreeMap<&'a str, &'a Load>,
    documents: BTreeMap<&'a str, &'a crate::documents::Document>,
}
impl<'a> ReviewIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        let mut index = Self::from_events(&workspace.events);
        index.loads = workspace.loads.iter().map(|v| (v.id.as_str(), v)).collect();
        index.documents = workspace
            .documents
            .iter()
            .map(|v| (v.id.as_str(), v))
            .collect();
        index
    }
    fn from_events(source: &'a [Event]) -> Self {
        let mut events: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
        let mut approvals: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
        for event in source {
            if event.kind == "invoice.approve" {
                approvals.entry(&event.entity_id).or_default().push(event);
            }
            if matches!(
                event.kind.as_str(),
                "invoice.received"
                    | "invoice.corrected"
                    | "invoice.canonical_revision"
                    | "invoice.entry_reviewed"
            ) {
                events.entry(&event.entity_id).or_default().push(event);
            }
        }
        Self {
            events,
            approvals,
            loads: BTreeMap::new(),
            documents: BTreeMap::new(),
        }
    }
    fn events(&self, invoice: &Invoice) -> impl Iterator<Item = &&'a Event> {
        self.events
            .get(invoice.id.as_str())
            .into_iter()
            .flatten()
            .chain(
                self.events
                    .get(invoice.load_id.as_str())
                    .into_iter()
                    .flatten(),
            )
    }
    fn revision(&self, invoice: &Invoice) -> Result<String, String> {
        let load = self
            .loads
            .get(invoice.load_id.as_str())
            .ok_or("Load not found")?;
        let source: Vec<_> = self
            .events(invoice)
            .filter(|v| v.kind != "invoice.entry_reviewed")
            .map(|v| json!([v.id, v.kind, v.actor]))
            .collect();
        // Stored-file reviews advance the document version independently of
        // the load. Bind its complete current metadata/reviews too, so an old
        // accepted/rejected/accepted POD cannot silently reuse an entry review.
        let pod = load
            .pod_document_id
            .as_deref()
            .and_then(|id| self.documents.get(id));
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([
                    "freight-invoice-review-v1",
                    invoice,
                    load.version,
                    load.carrier_id,
                    load.tender_id,
                    pod,
                    source
                ]))
                .map_err(|_| "Internal storage error")?
            )
        ))
    }
    fn valid_review(event: &Event, invoice: &Invoice, revision: &str) -> bool {
        event.kind == "invoice.entry_reviewed"
            && named(&event.actor)
            && event.details.as_ref().is_some_and(|d| {
                d["invoice_id"] == invoice.id
                    && d["revision"] == revision
                    && d["evidence_reference"]
                        .as_str()
                        .is_some_and(|s| note(s, "Evidence").is_ok())
                    && d["note"]
                        .as_str()
                        .is_some_and(|s| note(s, "Review").is_ok())
            })
    }
    fn reviewer(&self, invoice: &Invoice, revision: &str) -> Option<&'a Event> {
        self.events(invoice)
            .filter(|v| Self::valid_review(v, invoice, revision))
            .last()
            .copied()
    }
    fn approval_evidence(&self, invoice: &Invoice) -> Option<ApprovalEvidence<'a>> {
        if invoice.status != "approved" {
            return None;
        }
        // Read the exact links committed with the decision. Recomputing the
        // current revision loses the reviewer as soon as approval changes the
        // invoice, and later POD/load updates must not reinterpret that record.
        let [approval] = self.approvals.get(invoice.id.as_str())?.as_slice() else {
            return None;
        };
        let decision = approval.details.as_ref()?;
        let revision = decision["revision"].as_str()?;
        let review_id = decision["entry_review_id"].as_str()?;
        let approval_note = decision["note"].as_str()?;
        if revision.len() != 64
            || !revision.bytes().all(|v| v.is_ascii_hexdigit())
            || !named(&approval.actor)
            || note(approval_note, "Approval").is_err()
        {
            return None;
        }
        let mut linked = self.events(invoice).copied().filter(|event| {
            event.entity_id == invoice.id
                && event.kind == "invoice.entry_reviewed"
                && event.id == review_id
        });
        let review = linked.next()?;
        if linked.next().is_some()
            || !Self::valid_review(review, invoice, revision)
            || review.actor == approval.actor
        {
            return None;
        }
        let details = review.details.as_ref()?;
        Some(ApprovalEvidence {
            revision,
            entry_review_id: &review.id,
            entry_reviewed_by: &review.actor,
            entry_reviewed_at: &review.occurred_at,
            evidence_reference: details["evidence_reference"].as_str()?,
            entry_review_note: details["note"].as_str()?,
            approval_event_id: &approval.id,
            approved_by: &approval.actor,
            approved_at: &approval.occurred_at,
            approval_note,
        })
    }
    fn participated(&self, invoice: &Invoice, actor: &str) -> bool {
        self.events(invoice).any(|v| v.actor == actor)
    }
    pub(crate) fn summary(
        &self,
        invoice: &Invoice,
        actor: &str,
        required: bool,
    ) -> Result<Value, String> {
        self.summary_with_details(invoice, actor, required, true)
    }
    pub(crate) fn summary_with_details(
        &self,
        invoice: &Invoice,
        actor: &str,
        required: bool,
        include_details: bool,
    ) -> Result<Value, String> {
        let revision = self.revision(invoice)?;
        let reviewer = self.reviewer(invoice, &revision);
        let approval_evidence = self.approval_evidence(invoice);
        let reviewed_by = if invoice.status == "approved" {
            approval_evidence.as_ref().map(|v| v.entry_reviewed_by)
        } else {
            reviewer.map(|v| v.actor.as_str())
        };
        let state = if invoice.status == "approved" {
            "approved"
        } else if !required {
            "synthetic_demo"
        } else if !named(actor) {
            "named_subject_required"
        } else if reviewer.is_none() {
            "entry_review_required"
        } else if self.participated(invoice, actor) {
            "independent_approver_required"
        } else {
            "ready_for_approval"
        };
        let evidence_view = approval_evidence.as_ref().map(|evidence| {
            if include_details {
                json!(evidence)
            } else {
                json!({"revision":evidence.revision,"entry_review_id":evidence.entry_review_id,
                    "entry_reviewed_by":evidence.entry_reviewed_by,"entry_reviewed_at":evidence.entry_reviewed_at,
                    "approval_event_id":evidence.approval_event_id,"approved_by":evidence.approved_by,"approved_at":evidence.approved_at})
            }
        });
        Ok(
            json!({"required":required,"revision":revision,"state":state,
            "entry_reviewed_by":reviewed_by,"approval_evidence":evidence_view,
            "can_review_entry":required && named(actor) && invoice.status=="matched",
            "can_approve":invoice.status=="matched" && (!required || state=="ready_for_approval")}),
        )
    }
}

#[derive(Serialize)]
struct ApprovalEvidence<'a> {
    revision: &'a str,
    entry_review_id: &'a str,
    entry_reviewed_by: &'a str,
    entry_reviewed_at: &'a chrono::DateTime<chrono::Utc>,
    evidence_reference: &'a str,
    entry_review_note: &'a str,
    approval_event_id: &'a str,
    approved_by: &'a str,
    approved_at: &'a chrono::DateTime<chrono::Utc>,
    approval_note: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approval {
    expected_revision: String,
    note: String,
}
pub(crate) fn validate_approval_body(body: &Value) -> Result<(), String> {
    serde_json::from_value::<Approval>(body.clone())
        .map(|_| ())
        .map_err(|_| "Invoice approval requires expected_revision and note".into())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryReview {
    expected_revision: String,
    evidence_reference: String,
    note: String,
}
fn note(value: &str, field: &str) -> Result<(), String> {
    if value.trim().len() < 10
        || value.len() > 2000
        || value.chars().any(|c| c.is_control() && c != '\n')
    {
        return Err(format!(
            "{field} must contain 10–2000 characters of actual review evidence"
        ));
    }
    Ok(())
}
// Only this read projection can be constructed from root-scoped originals. It
// is deliberately not a Workspace or a mutation baseline.
#[derive(Deserialize)]
pub(crate) struct ApprovalSource {
    invoices: Vec<Invoice>,
    events: Vec<Event>,
}
impl ApprovalSource {
    pub(crate) fn evidence(&self, invoice_id: &str) -> Result<Value, String> {
        approval_record(&self.invoices, &self.events, invoice_id)
    }
}
fn approval_record(
    invoices: &[Invoice],
    events: &[Event],
    invoice_id: &str,
) -> Result<Value, String> {
    let mut invoices = invoices.iter().filter(|v| v.id == invoice_id);
    let invoice = invoices.next().ok_or("Invoice not found")?;
    if invoices.next().is_some() {
        return Err("Internal storage error".into());
    }
    Ok(json!({"invoice_id":invoice.id,"number":invoice.number,
        "approval_evidence":ReviewIndex::from_events(events).approval_evidence(invoice)}))
}
impl Workspace {
    /// Full recorded findings for one staff-selected invoice. This is derived
    /// from the same exact event links as the compact workspace projection.
    pub fn invoice_approval_evidence(&self, invoice_id: &str) -> Result<Value, String> {
        approval_record(&self.invoices, &self.events, invoice_id)
    }
    /// Read-only control for native clients. Authorization and the final
    /// revision/independence check still occur on the actual Store mutation.
    pub fn invoice_approval_control(&self, invoice_id: &str, actor: &str) -> Result<Value, String> {
        let invoice = self
            .invoices
            .iter()
            .find(|v| v.id == invoice_id)
            .ok_or("Invoice not found")?;
        ReviewIndex::new(self).summary(invoice, actor, true)
    }
    pub(crate) fn apply_invoice_review(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value, String> {
        let invoice = self
            .invoices
            .iter()
            .find(|v| v.id == entity)
            .ok_or("Invoice not found")?;
        if !named(actor) {
            return Err(NAMED_REVIEWER.into());
        }
        let index = ReviewIndex::new(self);
        let revision = index.revision(invoice)?;
        if operation == "invoice.entry_review" {
            let input: EntryReview = serde_json::from_value(body).map_err(|_| {
                "Invoice entry review requires expected_revision, evidence_reference, and note"
            })?;
            if input.expected_revision != revision {
                return Err(
                    "Invoice changed; refresh and inspect its current entry before reviewing"
                        .into(),
                );
            }
            if invoice.status != "matched" {
                return Err("Only matched invoices can receive an entry review".into());
            }
            note(&input.note, "Entry review note")?;
            note(
                &input.evidence_reference,
                "Carrier invoice evidence reference",
            )?;
            self.record(entity, "invoice.entry_reviewed", "Carrier invoice entry checked against actual source evidence; independent approval remains required");
            let event = self.events.last_mut().ok_or("Internal storage error")?;
            event.actor = actor.into();
            event.details = Some(json!({"invoice_id":entity,"revision":revision,
                "evidence_reference":input.evidence_reference,"note":input.note}));
            Ok(
                json!({"invoice_id":entity,"revision":revision,"review_id":event.id,"reviewed_by":actor}),
            )
        } else {
            let input: Approval = serde_json::from_value(body)
                .map_err(|_| "Invoice approval requires expected_revision and note")?;
            if input.expected_revision != revision {
                return Err(
                    "Invoice changed; refresh and inspect its current entry before approval".into(),
                );
            }
            note(&input.note, "Approval note")?;
            let review = index
                .reviewer(invoice, &revision)
                .ok_or("Record an entry review of the current carrier invoice before approval")?;
            if index.participated(invoice, actor) {
                return Err(INDEPENDENT_REVIEW.into());
            }
            let review_id = review.id.clone();
            let result = self.apply("invoice.approve", entity, json!({}))?;
            let event = self
                .events
                .iter_mut()
                .rev()
                .find(|e| e.entity_id == entity && e.kind == "invoice.approve")
                .ok_or("Internal storage error")?;
            event.details =
                Some(json!({"revision":revision,"entry_review_id":review_id,"note":input.note}));
            Ok(result)
        }
    }
}
