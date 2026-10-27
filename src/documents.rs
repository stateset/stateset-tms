use crate::{
    domain::{self, Load, Receipt, Result, Status, Workspace},
    store::Store,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const MAX_DOCUMENT_BYTES: usize = 10 * 1024 * 1024;
pub struct Upload {
    pub kind: String,
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub load_id: String,
    pub kind: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: usize,
    pub sha256: String,
    pub uploaded_at: DateTime<Utc>,
    pub uploaded_by: String,
    pub version: u64,
    pub reviews: Vec<Review>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub decision: String,
    pub note: String,
    pub reviewed_by: String,
    pub reviewed_at: DateTime<Utc>,
    pub reference: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub expected_version: u64,
    pub decision: String,
    pub note: String,
}
impl ReviewInput {
    fn validate(&self) -> Result<()> {
        if !["accepted", "rejected"].contains(&self.decision.as_str()) {
            return Err("Review decision must be accepted or rejected".into());
        }
        domain::required(&self.note, "Evidence review note")
    }
}
impl Document {
    pub fn status(&self) -> &str {
        self.reviews
            .last()
            .map_or("pending_review", |v| v.decision.as_str())
    }
}
pub fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_upload(kind: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("Document must contain 1 byte to 10 MiB".into());
    }
    if ![
        "pod",
        "bill_of_lading",
        "rate_confirmation",
        "carrier_invoice",
        "other",
    ]
    .contains(&kind)
    {
        return Err("Unsupported shipment document kind".into());
    }
    if filename.trim().is_empty()
        || filename.len() > 180
        || filename
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(
            "Document filename must be 1–180 characters without paths or control characters".into(),
        );
    }
    let name = filename.to_ascii_lowercase();
    let valid = match content_type {
        "application/pdf" => {
            name.ends_with(".pdf")
                && bytes.starts_with(b"%PDF-")
                && bytes[bytes.len().saturating_sub(1024)..]
                    .windows(5)
                    .any(|v| v == b"%%EOF")
        }
        "image/jpeg" => {
            (name.ends_with(".jpg") || name.ends_with(".jpeg"))
                && bytes.starts_with(&[0xff, 0xd8, 0xff])
                && bytes.ends_with(&[0xff, 0xd9])
        }
        "image/png" => {
            name.ends_with(".png")
                && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
                && bytes.ends_with(b"\x00\x00\x00\x00IEND\xaeB`\x82")
        }
        _ => false,
    };
    if !valid {
        return Err(
            "Upload a PDF, JPEG, or PNG whose filename and content match its media type".into(),
        );
    }
    Ok(())
}

impl Upload {
    pub(crate) fn document(&self, load_id: &str, actor: &str) -> Result<Document> {
        validate_upload(&self.kind, &self.filename, &self.content_type, &self.bytes)?;
        Ok(Document {
            id: domain::id("doc"),
            load_id: load_id.into(),
            kind: self.kind.clone(),
            filename: self.filename.clone(),
            content_type: self.content_type.clone(),
            size_bytes: self.bytes.len(),
            sha256: sha256(&self.bytes),
            uploaded_at: Utc::now(),
            uploaded_by: actor.into(),
            version: 1,
            reviews: vec![],
        })
    }
}

impl Store {
    pub async fn upload_document(
        &self,
        principal: (&str, &str),
        key: &str,
        load_id: &str,
        upload: Upload,
    ) -> Result<Value> {
        let document = upload.document(load_id, principal.1)?;
        let Upload {
            kind,
            filename,
            content_type,
            bytes,
        } = upload;
        let (tenant, actor) = principal;
        let fingerprint = crate::store::fingerprint(
            "document.upload",
            load_id,
            &json!({"kind":kind,"filename":filename,"content_type":content_type,"sha256":document.sha256,"size_bytes":document.size_bytes}),
        );
        let blob_id = document.id.clone();
        let owned = (key.to_owned(), load_id.to_owned(), actor.to_owned());
        self.update_receipted(tenant,Some((blob_id,bytes)),None,move |w| {
            let (key, load_id, actor) = (owned.0.as_str(), owned.1.as_str(), owned.2.as_str());
            if let Some(receipt)=w.receipts.get(key) {
                let response=receipt.checked_response(&fingerprint)?;
                return Ok(json!({"data":response,"replayed":true}));
            }
            if !w.loads.iter().any(|v|v.id==load_id) {return Err("Load not found".into());}
            w.record(load_id,"document.uploaded",format!("{} uploaded: {}",kind,filename));
            if let Some(event)=w.events.last_mut() {
                event.actor=actor.into();
                event.details=Some(json!({"document_id":document.id,"sha256":document.sha256,"size_bytes":document.size_bytes}));
            }
            let response=json!(document);
            w.documents.push(document);
            w.receipts.insert(key.into(),Receipt{fingerprint,response:response.clone(),stopped:None});
            Ok(json!({"data":response,"replayed":false}))
        }).await
    }
    pub async fn document_content(&self, tenant: &str, id: &str) -> Result<(Document, Vec<u8>)> {
        let workspace = self.read(tenant).await?;
        let document = workspace
            .documents
            .iter()
            .find(|v| v.id == id)
            .ok_or("Document not found")?
            .clone();
        let bytes = self.read_binary(tenant, id).await?;
        if bytes.len() != document.size_bytes || sha256(&bytes) != document.sha256 {
            tracing::error!(document_id = id, "Stored document integrity check failed");
            return Err("Internal storage error".into());
        }
        Ok((document, bytes))
    }
}

impl Workspace {
    pub fn set_delivery_evidence(
        &self,
        load: &mut Load,
        reference: Option<String>,
        document_id: Option<String>,
    ) -> Result<()> {
        if self.evidence_locked(&load.id) {
            return Err(
                "Delivery evidence already supports issued customer billing, an approved carrier invoice, or a settlement".into(),
            );
        }
        if let Some(id) = document_id {
            if reference.is_some() {
                return Err("Choose a stored POD document or an external reference".into());
            }
            let document = self
                .documents
                .iter()
                .find(|v| v.id == id && v.load_id == load.id)
                .ok_or("Document not found")?;
            if document.kind != "pod" || document.status() == "rejected" {
                return Err("Select a POD document that has not been rejected".into());
            }
            load.pod_reference = Some(format!("document:{id}"));
            load.pod_document_id = Some(id);
        } else {
            let reference = reference.ok_or("Proof of delivery reference required")?;
            domain::required(&reference, "Proof of delivery")?;
            load.pod_reference = Some(reference);
            load.pod_document_id = None;
        }
        load.pod_review = None;
        Ok(())
    }
    pub fn delivery_evidence_ready(&self, load: &Load) -> bool {
        self.delivery_evidence_ready_with_execution(load, || self.execution_consistent(load))
    }
    // Retain the preceding staff selector for full-response comparisons. The
    // serving path now shares one immutable movement context as well as gates.
    #[cfg(test)]
    pub(crate) fn delivery_evidence_ready_with_gates(
        &self,
        load: &Load,
        gates: &[&crate::stops::StopEvent],
    ) -> bool {
        self.delivery_evidence_ready_with_execution(load, || {
            self.execution_consistent_with_gates(load, gates)
        })
    }
    pub(crate) fn delivery_evidence_ready_with_execution(
        &self,
        load: &Load,
        consistent: impl FnOnce() -> bool,
    ) -> bool {
        self.delivery_evidence_ready_with_route(load, || self.route_ready(load), consistent)
    }
    pub(crate) fn delivery_evidence_ready_with_route(
        &self,
        load: &Load,
        route_ready: impl FnOnce() -> bool,
        consistent: impl FnOnce() -> bool,
    ) -> bool {
        if load.status != Status::Delivered
            || self.unresolved_tender_incidents(&load.id)
            || !route_ready()
            || !consistent()
        {
            return false;
        }
        if let Some(id) = &load.pod_document_id {
            self.documents.iter().any(|v| {
                v.id == *id && v.load_id == load.id && v.kind == "pod" && v.status() == "accepted"
            })
        } else {
            load.pod_review.as_ref().is_some_and(|v| {
                v.decision == "accepted" && Some(&v.reference) == load.pod_reference.as_ref()
            })
        }
    }
    pub fn evidence_locked(&self, load_id: &str) -> bool {
        self.invoices
            .iter()
            .any(|v| v.load_id == load_id && v.status == "approved")
            || self.settlements.iter().any(|v| v.load_id == load_id)
            || self
                .bills
                .iter()
                .any(|v| v.load_id == load_id && v.issued_at.is_some() && v.status != "void")
    }
    pub fn review_document(&mut self, id: &str, body: Value, actor: &str) -> Result<Value> {
        let input: ReviewInput =
            serde_json::from_value(body).map_err(|_| "Invalid evidence review request")?;
        input.validate()?;
        let index = self
            .documents
            .iter()
            .position(|v| v.id == id)
            .ok_or("Document not found")?;
        let document = &self.documents[index];
        if input.decision != "accepted"
            && self.rate_confirmations.iter().any(|v| {
                v.handoffs
                    .iter()
                    .any(|h| h.document_id.as_deref() == Some(id))
            })
        {
            return Err("Retained rate-confirmation handoff evidence cannot be rejected; correct its handoff record instead".into());
        }
        if input.decision != "accepted" && self.customer_finance_document_locked(id) {
            return Err("Document supports retained customer credit/refund history; record the financial correction instead of rejecting its evidence".into());
        }
        if input.decision != "accepted"
            && self
                .cancellation_fees
                .iter()
                .any(|v| v.posted_at.is_some() && v.document_id.as_deref() == Some(id))
        {
            return Err("Document supports retained cancellation billing or payment history and cannot be rejected".into());
        }
        if document.version != input.expected_version {
            return Err("Document changed; refresh before reviewing".into());
        }
        if input.decision != "accepted"
            && self
                .charges
                .iter()
                .any(|v| v.status == "approved" && v.document_id.as_deref() == Some(id))
        {
            return Err("Document supports an approved charge; reconcile that charge before rejecting its evidence".into());
        }
        if input.decision != "accepted"
            && self
                .carrier_adjustments
                .iter()
                .any(|v| v.status == "posted" && v.document_id.as_deref() == Some(id))
        {
            return Err("Document supports a posted carrier adjustment; reverse that adjustment before rejecting its evidence".into());
        }
        if input.decision != "accepted"
            && self
                .carrier_credit_applications
                .iter()
                .any(|v| v.reversal.is_none() && v.document_id.as_deref() == Some(id))
        {
            return Err("Document supports an active carrier credit application; reverse that application before rejecting its evidence".into());
        }
        if input.decision != "accepted"
            && self.bills.iter().any(|v| {
                v.issued_at.is_some()
                    && v.status != "void"
                    && v.supplement
                        .as_ref()
                        .is_some_and(|a| a.document_id.as_deref() == Some(id))
            })
        {
            return Err("Document supports an issued customer supplement; void that supplement before rejecting its evidence".into());
        }
        if input.decision != "accepted"
            && self
                .loads
                .iter()
                .any(|v| v.pod_document_id.as_deref() == Some(id) && self.evidence_locked(&v.id))
        {
            return Err(
                "Delivery evidence already supports issued customer billing, an approved carrier invoice, or a settlement".into(),
            );
        }
        let review = Review {
            decision: input.decision,
            note: input.note,
            reviewed_by: actor.into(),
            reviewed_at: Utc::now(),
            reference: document.sha256.clone(),
        };
        let document = &mut self.documents[index];
        document.version = document
            .version
            .checked_add(1)
            .ok_or("Document version exhausted")?;
        document.reviews.push(review.clone());
        let result = json!(document);
        let load_id = document.load_id.clone();
        self.record(
            &load_id,
            "document.reviewed",
            format!("Document {} {}: {}", id, review.decision, review.note),
        );
        if let Some(event) = self.events.last_mut() {
            event.details = Some(json!({"document_id":id,"review":review}));
        }
        Ok(result)
    }
    pub fn review_external_pod(&mut self, id: &str, body: Value, actor: &str) -> Result<Value> {
        let input: ReviewInput =
            serde_json::from_value(body).map_err(|_| "Invalid evidence review request")?;
        input.validate()?;
        let index = self
            .loads
            .iter()
            .position(|v| v.id == id)
            .ok_or("Load not found")?;
        let load = &self.loads[index];
        if load.version != input.expected_version {
            return Err("Load changed; refresh before reviewing".into());
        }
        if load.status != Status::Delivered || load.pod_reference.is_none() {
            return Err("Delivered shipment and POD reference required for review".into());
        }
        if load.pod_document_id.is_some() {
            return Err("Review the attached document instead of its external reference".into());
        }
        if input.decision != "accepted" && self.evidence_locked(id) {
            return Err(
                "Delivery evidence already supports issued customer billing, an approved carrier invoice, or a settlement".into(),
            );
        }
        let review = Review {
            decision: input.decision,
            note: input.note,
            reviewed_by: actor.into(),
            reviewed_at: Utc::now(),
            reference: load.pod_reference.clone().ok_or("POD reference required")?,
        };
        let load = &mut self.loads[index];
        load.version = load
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        load.updated_at = Utc::now();
        load.pod_review = Some(review.clone());
        let result = json!(load);
        self.record(
            id,
            "pod.reviewed",
            format!(
                "External delivery evidence {}: {}",
                review.decision, review.note
            ),
        );
        if let Some(event) = self.events.last_mut() {
            event.details = Some(json!({"review":review}));
        }
        Ok(result)
    }
}
