//! File disclosure is explicit, counterparty-bound, and independent of finance.
use crate::{
    auth::{Capability, Principal, Role},
    documents::Document,
    domain::{self, Result, Workspace},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
impl crate::store::Store {
    pub async fn shared_document_content(
        &self,
        principal: &Principal,
        id: &str,
    ) -> Result<(Document, Vec<u8>)> {
        self.authorized_document(&principal.tenant_id, |w| {
            w.authorize_shared_download(id, principal)
        })
        .await
    }
}
pub const FINANCE_REQUIRED: &str = "Sharing carrier financial files requires finance permission";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedFile {
    pub load_id: String,
    pub document_id: String,
    pub filename: String,
    pub kind: String,
    pub content_type: String,
    pub size_bytes: usize,
    pub sha256: String,
}
impl From<&Document> for SharedFile {
    fn from(d: &Document) -> Self {
        Self {
            load_id: d.load_id.clone(),
            document_id: d.id.clone(),
            filename: d.filename.clone(),
            kind: d.kind.clone(),
            content_type: d.content_type.clone(),
            size_bytes: d.size_bytes,
            sha256: d.sha256.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revocation {
    pub note: String,
    pub revoked_at: DateTime<Utc>,
    pub revoked_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadAuthorization {
    pub id: String,
    pub authorized_at: DateTime<Utc>,
    pub authorized_by: String,
    pub share_version: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentShare {
    pub id: String,
    pub load_id: String,
    pub load_reference: String,
    pub document_id: String,
    pub document_version: u64,
    pub file: SharedFile,
    pub audience: String,
    pub counterparty_id: String,
    pub counterparty_name: String,
    pub tender_id: Option<String>,
    pub shipment_reference: Option<String>,
    pub message: String,
    pub review_note: String,
    pub shared_at: DateTime<Utc>,
    pub shared_by: String,
    pub shared_role: Role,
    pub supersedes_id: Option<String>,
    pub version: u64,
    pub revocation: Option<Revocation>,
    pub download_authorizations: Vec<DownloadAuthorization>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateShare {
    pub expected_load_version: u64,
    pub expected_document_version: u64,
    pub audience: String,
    pub carrier_tender_id: Option<String>,
    pub expected_previous_id: Option<String>,
    pub message: String,
    pub review_note: String,
    pub acknowledge_content: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeShare {
    pub expected_version: u64,
    pub note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trusted<T> {
    role: Role,
    request: T,
}
impl Workspace {
    pub fn carrier_file_shared(&self, tender: &str) -> bool {
        // Publication communicates file access, even without a recorded GET.
        // Revocation cannot prove that the carrier never received instructions.
        self.document_shares
            .iter()
            .any(|s| s.audience == "carrier" && s.tender_id.as_deref() == Some(tender))
    }
    fn share_document(&self, s: &DocumentShare) -> Result<&Document> {
        let d = self
            .documents
            .iter()
            .find(|d| d.id == s.document_id)
            .ok_or("Internal storage error")?;
        if s.load_id != s.file.load_id
            || s.document_id != s.file.document_id
            || json!(SharedFile::from(d)) != json!(s.file)
        {
            return Err("Internal storage error".into());
        }
        Ok(d)
    }
    fn share_owned(&self, id: &str, p: &Principal) -> Result<&DocumentShare> {
        let audience = match p.role {
            Role::Customer => "customer",
            Role::Carrier => "carrier",
            _ => return Err("Shared file not found".into()),
        };
        if !p.valid_scope() {
            return Err("Shared file not found".into());
        }
        self.document_shares
            .iter()
            .find(|s| {
                s.id == id
                    && s.audience == audience
                    && Some(&s.counterparty_id) == p.counterparty_id.as_ref()
            })
            .ok_or("Shared file not found".into())
    }
    pub fn shared_file_views(&self, p: &Principal) -> Result<Vec<Value>> {
        let mut views = vec![];
        for s in &self.document_shares {
            if self.share_owned(&s.id, p).is_err() {
                continue;
            }
            let d = self.share_document(s)?;
            let available = s.revocation.is_none() && d.status() != "rejected";
            views.push(json!({"id":s.id,"load_reference":s.load_reference,"filename":s.file.filename,"kind":s.file.kind,"content_type":s.file.content_type,"size_bytes":s.file.size_bytes,"sha256":s.file.sha256,
                "message":s.message,"shared_at":s.shared_at,"shipment_reference":s.shipment_reference,"available":available,"status":if s.revocation.is_some(){"revoked"}else if !available {"held"}else {"available"},"review_status":d.status(),"revoked_at":s.revocation.as_ref().map(|r|r.revoked_at)}));
        }
        Ok(views)
    }
    pub fn authorize_shared_download(&mut self, id: &str, p: &Principal) -> Result<Document> {
        let s = self.share_owned(id, p)?;
        // Return 404 for denied content, without disclosing private revocation notes.
        if s.revocation.is_some() {
            return Err("Shared file not found".into());
        }
        let d = self.share_document(s)?.clone();
        if d.status() == "rejected" {
            return Err("Shared file not found".into());
        }
        let load_id = s.load_id.clone();
        let access = DownloadAuthorization {
            id: domain::id("file-access"),
            authorized_at: Utc::now(),
            authorized_by: p.subject.clone(),
            share_version: s.version,
            sha256: s.file.sha256.clone(),
        };
        self.document_shares
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or("Shared file not found")?
            .download_authorizations
            .push(access.clone());
        self.record(
            &load_id,
            "document.download_authorized",
            "Scoped partner file download authorized; network receipt is not established",
        );
        if let Some(e) = self.events.last_mut() {
            e.actor = p.subject.clone();
            e.details = Some(json!({"share_id":id,"document_id":d.id,"authorization":access}));
        }
        Ok(d)
    }
    pub fn apply_share(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "share.revoke" {
            let input: Trusted<RevokeShare> =
                serde_json::from_value(body).map_err(|_| "Invalid share revocation fields")?;
            if !input.role.allows(Capability::DocumentsWrite) {
                return Err("Document sharing requires staff document permission".into());
            }
            domain::required(&input.request.note, "Private revocation reason")?;
            let s = self
                .document_shares
                .iter_mut()
                .find(|s| s.id == entity)
                .ok_or("Document share not found")?;
            if s.version != input.request.expected_version {
                return Err("Document share changed; refresh before revoking".into());
            }
            if s.revocation.is_some() {
                return Err("Document share already revoked".into());
            }
            s.version = s
                .version
                .checked_add(1)
                .ok_or("Document share version exhausted")?;
            s.revocation = Some(Revocation {
                note: input.request.note.trim().into(),
                revoked_at: Utc::now(),
                revoked_by: actor.into(),
            });
            let load_id = s.load_id.clone();
            let result = json!(s);
            self.record(&load_id,operation,"Partner file access revoked; original file, financial evidence, and prior authorizations retained");
            return Ok(result);
        }
        if operation != "share.create" {
            return Err("Unsupported document sharing operation".into());
        }
        let input: Trusted<CreateShare> =
            serde_json::from_value(body).map_err(|_| "Invalid document share fields")?;
        if !input.role.allows(Capability::DocumentsWrite) {
            return Err("Document sharing requires staff document permission".into());
        }
        let r = input.request;
        for (v, label) in [
            (&r.message, "Message for recipient"),
            (&r.review_note, "Private content review note"),
        ] {
            domain::required(v, label)?;
        }
        if !r.acknowledge_content {
            return Err(
                "Inspect the original file and acknowledge its suitability for the exact recipient"
                    .into(),
            );
        }
        if !["customer", "carrier"].contains(&r.audience.as_str()) {
            return Err("Select customer or carrier file access".into());
        }
        let d = self
            .documents
            .iter()
            .find(|d| d.id == entity)
            .ok_or("Document not found")?
            .clone();
        if d.version != r.expected_document_version {
            return Err("Document changed; inspect its current review before sharing".into());
        }
        if d.status() == "rejected" {
            return Err("Rejected evidence cannot be shared".into());
        }
        if ["carrier_invoice", "rate_confirmation"].contains(&d.kind.as_str()) {
            if r.audience == "customer" {
                return Err("Carrier invoice and rate files cannot be shared with a customer; use the issued customer invoice or quote".into());
            }
            if !input.role.allows(Capability::FinanceWrite) {
                return Err(FINANCE_REQUIRED.into());
            }
        }
        let index = self
            .loads
            .iter()
            .position(|l| l.id == d.load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        if load.version != r.expected_load_version {
            return Err("Load changed; review the recipient before sharing".into());
        }
        let (counterparty_id, counterparty_name, tender_id, shipment_reference, reference) =
            if r.audience == "customer" {
                if r.carrier_tender_id.is_some() {
                    return Err("Customer file access cannot select a carrier tender".into());
                }
                let c = self
                    .customers
                    .iter()
                    .find(|c| c.id == load.customer_id)
                    .ok_or("Customer not found")?;
                (
                    c.id.clone(),
                    c.name.clone(),
                    None,
                    None,
                    load.reference.clone(),
                )
            } else {
                if r.carrier_tender_id.is_none() && load.tender_id.is_none() {
                    self.adopt_tender(&mut load)?;
                    load.version = load
                        .version
                        .checked_add(1)
                        .ok_or("Load version exhausted")?;
                    load.updated_at = Utc::now();
                    self.loads[index] = load.clone();
                }
                let selected = r
                    .carrier_tender_id
                    .as_ref()
                    .or(load.tender_id.as_ref())
                    .ok_or("Select an actual carrier tender")?;
                let t = self
                    .tenders
                    .iter()
                    .find(|t| &t.id == selected && t.load_id == load.id)
                    .ok_or("Carrier tender not found")?;
                (
                    t.carrier_id.clone(),
                    t.carrier.name.clone(),
                    Some(t.id.clone()),
                    Some(t.shipment_reference.clone()),
                    t.plan.reference.clone(),
                )
            };
        let previous = self.document_shares.iter().rev().find(|s| {
            s.document_id == entity
                && s.audience == r.audience
                && s.counterparty_id == counterparty_id
                && s.tender_id == tender_id
        });
        if previous.map(|s| s.id.as_str()) != r.expected_previous_id.as_deref() {
            return Err("Document share changed; refresh before publishing access".into());
        }
        if previous.is_some_and(|s| s.revocation.is_none()) {
            return Err("File already shared with this exact recipient and tender".into());
        }
        let s = DocumentShare {
            id: domain::id("file-share"),
            load_id: load.id.clone(),
            load_reference: reference,
            document_id: d.id.clone(),
            document_version: d.version,
            file: SharedFile::from(&d),
            audience: r.audience,
            counterparty_id,
            counterparty_name,
            tender_id,
            shipment_reference,
            message: r.message.trim().into(),
            review_note: r.review_note.trim().into(),
            shared_at: Utc::now(),
            shared_by: actor.into(),
            shared_role: input.role,
            supersedes_id: previous.map(|s| s.id.clone()),
            version: 1,
            revocation: None,
            download_authorizations: vec![],
        };
        self.document_shares.push(s.clone());
        self.record(&load.id,operation,format!("Exact file {} shared with {} {}; file disclosure does not accept freight or approve finance",d.filename,s.audience,s.counterparty_name));
        Ok(json!(s))
    }
}
