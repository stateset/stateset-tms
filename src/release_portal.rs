//! Authenticated carrier release evidence is separate from safe application.
use crate::{
    domain::{self, Load, Result, Status, Workspace},
    tenders::{CarrierRelease, TenderAttempt},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseSnapshot {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub load_reference: String,
    pub shipment_reference: String,
    pub carrier_name: String,
    pub disposition: String,
    pub origin: String,
    pub destination: String,
    pub equipment: String,
    pub message: String,
    pub respond_by: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleasePublication {
    pub id: String,
    pub release_id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub snapshot: ReleaseSnapshot,
    pub basis_sha256: String,
    pub html: String,
    pub sha256: String,
    pub supersedes_id: Option<String>,
    pub published_at: DateTime<Utc>,
    pub published_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseApplication {
    pub applied_at: DateTime<Utc>,
    pub applied_by: String,
    pub mode: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseResponse {
    pub id: String,
    pub publication_id: String,
    pub release_id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub document_sha256: String,
    pub decision: String,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub no_custody: bool,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub outcome: String,
    pub initial_hold: Option<String>,
    pub evidence_sha256: String,
    pub application: Option<ReleaseApplication>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishInput {
    pub expected_load_version: u64,
    pub expected_previous_id: Option<String>,
    pub message: String,
    pub respond_by: DateTime<Utc>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseInput {
    pub expected_load_version: u64,
    pub document_sha256: String,
    pub decision: String,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub no_custody: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedResponse {
    carrier_id: String,
    subject: String,
    request: ResponseInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyInput {
    expected_load_version: u64,
}
fn digest(v: &impl Serialize) -> Result<String> {
    Ok(crate::documents::sha256(
        &serde_json::to_vec(v).map_err(|_| "Internal storage error")?,
    ))
}
fn escape(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn render(s: &ReleaseSnapshot) -> String {
    let output=format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Carrier release request {}</title><style>body{{font:16px system-ui;max-width:800px;margin:32px auto;padding:24px;color:#18332d}}p{{line-height:1.7;white-space:pre-wrap}}</style><h1>Carrier release request</h1><p>Carrier: {}\nShipment: {}\nLoad: {}</p><p>Pickup: {}\nDelivery: {}\nEquipment: {}</p><h2>Request</h2><p>{}</p><p>After confirmed release: {}.</p><p>Release response and application deadline: {} (UTC).</p><p>Release requires confirmation that this carrier has not picked up and does not have custody of this exact assignment. A request alone does not release freight. Recorded movement, custody, financial records, and live handoff leases can hold application for operations review. Retaining an assignment does not accept a tender or establish pickup. No payment or cancellation fee is agreed through this response.</p></html>",escape(&s.shipment_reference),escape(&s.carrier_name),escape(&s.shipment_reference),escape(&s.load_reference),escape(&s.origin),escape(&s.destination),escape(&s.equipment),escape(&s.message),if s.disposition=="cancel" {"cancel this shipment"} else {"return this shipment to planning"},s.respond_by.to_rfc3339());
    if s.intermediate_stops.is_empty() {
        output
    } else {
        output.replace(
            "</html>",
            &format!(
                "{}</html>",
                crate::itinerary::route_document(&s.intermediate_stops)
            ),
        )
    }
}
impl ReleasePublication {
    fn basis(&self) -> Value {
        json!({"id":self.id,"release_id":self.release_id,"load_id":self.load_id,"tender_id":self.tender_id,"carrier_id":self.carrier_id,"snapshot":self.snapshot,"supersedes_id":self.supersedes_id,"published_at":self.published_at,"published_by":self.published_by})
    }
    pub fn verify(&self) -> Result<()> {
        if digest(&self.basis())? != self.basis_sha256
            || crate::documents::sha256(self.html.as_bytes()) != self.sha256
            || render(&self.snapshot) != self.html
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
}
impl ReleaseResponse {
    fn proof(&self) -> Value {
        json!({"id":self.id,"publication_id":self.publication_id,"release_id":self.release_id,"load_id":self.load_id,"tender_id":self.tender_id,"carrier_id":self.carrier_id,"document_sha256":self.document_sha256,"decision":self.decision,"contact":self.contact,"reference":self.reference,"note":self.note,"no_custody":self.no_custody,"recorded_at":self.recorded_at,"recorded_by":self.recorded_by,"outcome":self.outcome,"initial_hold":self.initial_hold})
    }
    fn verify(&self, p: &ReleasePublication) -> Result<()> {
        p.verify()?;
        if digest(&self.proof())? != self.evidence_sha256
            || self.publication_id != p.id
            || self.release_id != p.release_id
            || self.load_id != p.load_id
            || self.tender_id != p.tender_id
            || self.carrier_id != p.carrier_id
            || self.document_sha256 != p.sha256
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
}
impl Workspace {
    pub fn release_request_published(&self, tender: &str) -> bool {
        self.release_publications
            .iter()
            .any(|p| p.tender_id == tender)
    }
    pub fn release_publication_for(&self, id: &str, carrier: &str) -> Result<&ReleasePublication> {
        self.release_publications
            .iter()
            .find(|p| p.id == id && p.carrier_id == carrier)
            .ok_or("Carrier release publication not found".into())
    }
    fn release_entities(
        &self,
        p: &ReleasePublication,
    ) -> Result<(&CarrierRelease, &TenderAttempt, &Load)> {
        p.verify()?;
        let r = self
            .releases
            .iter()
            .find(|r| r.id == p.release_id)
            .ok_or("Internal storage error")?;
        let t = self
            .tenders
            .iter()
            .find(|t| t.id == r.tender_id)
            .ok_or("Internal storage error")?;
        let l = self
            .loads
            .iter()
            .find(|l| l.id == r.load_id)
            .ok_or("Internal storage error")?;
        if r.load_id != p.load_id
            || r.tender_id != p.tender_id
            || t.carrier_id != p.carrier_id
            || p.snapshot.disposition != r.disposition
            || p.snapshot.shipment_reference != t.shipment_reference
        {
            return Err("Internal storage error".into());
        }
        Ok((r, t, l))
    }
    fn release_current(&self, p: &ReleasePublication) -> Result<bool> {
        let (r, t, l) = self.release_entities(p)?;
        Ok(r.status == "pending"
            && l.tender_id.as_deref() == Some(&t.id)
            && l.carrier_id.as_deref() == Some(&p.carrier_id)
            && matches!(
                l.status,
                Status::Tendered | Status::Accepted | Status::Dispatched
            )
            && self
                .release_publications
                .iter()
                .rev()
                .find(|v| v.release_id == p.release_id)
                .is_some_and(|v| v.id == p.id))
    }
    fn release_hold(&self, p: &ReleasePublication) -> Result<Option<&'static str>> {
        let (_, _, l) = self.release_entities(p)?;
        if self.has_approved_charges(&l.id)
            || self.invoices.iter().any(|i| i.load_id == l.id)
            || self
                .bills
                .iter()
                .any(|b| b.load_id == l.id && b.status != "void")
        {
            return Ok(Some("financial_review"));
        }
        if self.unresolved_tender_incidents(&l.id) {
            return Ok(Some("custody_review"));
        }
        if self.outbox.iter().any(|j| {
            j.tender_id.as_deref() == Some(&p.tender_id)
                && j.status == "sending"
                && j.leased_until.is_none_or(|t| t > Utc::now())
        }) {
            return Ok(Some("handoff_in_progress"));
        }
        Ok(None)
    }
    fn apply_recorded_release(
        &mut self,
        response: &ReleaseResponse,
        version: u64,
        actor: &str,
        mode: &str,
    ) -> Result<()> {
        let p = self.release_publication_for(&response.publication_id, &response.carrier_id)?;
        response.verify(p)?;
        if !self.release_current(p)? {
            return Err("Release request changed; reconcile current assignment or movement before applying carrier evidence".into());
        }
        if p.snapshot.respond_by <= Utc::now() {
            return Err("Carrier release authorization expired; publish a new request and obtain current confirmation".into());
        }
        self.apply_tender_operation("tender.release.confirm",&response.release_id,json!({"expected_load_version":version,"carrier_contact":response.contact,"confirmation_reference":response.reference,"note":response.note}),&response.recorded_by)?;
        // Keep the original carrier's acknowledgement time distinct from later
        // operations application; never rewrite the source as staff attestation.
        let r = self
            .releases
            .iter_mut()
            .find(|r| r.id == response.release_id)
            .ok_or("Carrier release not found")?;
        r.confirmation
            .as_mut()
            .ok_or("Internal storage error")?
            .recorded_at = response.recorded_at;
        let application = ReleaseApplication {
            applied_at: Utc::now(),
            applied_by: actor.into(),
            mode: mode.into(),
        };
        if let Some(saved) = self
            .release_responses
            .iter_mut()
            .find(|r| r.id == response.id)
        {
            saved.application = Some(application);
        }
        Ok(())
    }
    pub fn release_portal_views(&self, carrier: &str) -> Result<Vec<Value>> {
        self.release_publications.iter().filter(|p|p.carrier_id==carrier).map(|p| {
            let(r,_,l)=self.release_entities(p)?;let current=self.release_current(p)?;
            let response=self.release_responses.iter().find(|d|d.publication_id==p.id);if let Some(d)=response{d.verify(p)?;}
            Ok(json!({"id":p.id,"snapshot":p.snapshot,"document_sha256":p.sha256,"published_at":p.published_at,"status":r.status,"is_current":current,"load_version":if current {Some(l.version)} else {None},
                "can_release":current&&response.is_none()&&p.snapshot.respond_by>Utc::now(),"can_retain":current&&response.is_none(),"application_pending":response.is_some_and(|d|d.outcome=="awaiting_operations"&&d.application.is_none())&&current,
                "response":response.map(|d|json!({"decision":d.decision,"contact":d.contact,"reference":d.reference,"note":d.note,"no_custody":d.no_custody,"recorded_at":d.recorded_at,"recorded_by":d.recorded_by,"outcome":d.outcome,"application":d.application.as_ref().map(|a|json!({"applied_at":a.applied_at,"mode":a.mode}))}))}))
        }).collect()
    }
    pub fn apply_release_portal(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "portal.release.publish" {
            let input: PublishInput = serde_json::from_value(body)
                .map_err(|_| "Invalid carrier release publication fields")?;
            domain::required(&input.message, "Message for carrier")?;
            if input.respond_by <= Utc::now() || input.respond_by > Utc::now() + Duration::days(7) {
                return Err("Release response/application deadline must be in the future and within seven days".into());
            }
            let r = self
                .releases
                .iter()
                .find(|r| r.id == entity)
                .ok_or("Carrier release not found")?;
            let l = self
                .loads
                .iter()
                .find(|l| l.id == r.load_id)
                .ok_or("Load not found")?;
            if l.version != input.expected_load_version {
                return Err("Load changed; refresh before publishing the release request".into());
            }
            if r.status != "pending"
                || l.tender_id.as_deref() != Some(&r.tender_id)
                || !matches!(
                    l.status,
                    Status::Tendered | Status::Accepted | Status::Dispatched
                )
            {
                return Err("Release request changed; only the pending current pre-pickup assignment may be published".into());
            }
            let t = self
                .tenders
                .iter()
                .find(|t| t.id == r.tender_id)
                .ok_or("Tender not found")?;
            if l.carrier_id.as_deref() != Some(&t.carrier_id) {
                return Err("Carrier assignment changed; reconcile before publication".into());
            }
            let prior = self
                .release_publications
                .iter()
                .rev()
                .find(|p| p.release_id == entity);
            if prior.map(|p| p.id.as_str()) != input.expected_previous_id.as_deref() {
                return Err("Release publication changed; refresh before republishing".into());
            }
            let s = ReleaseSnapshot {
                intermediate_stops: t.plan.intermediate_stops.clone(),
                load_reference: t.plan.reference.clone(),
                shipment_reference: t.shipment_reference.clone(),
                carrier_name: t.carrier.name.clone(),
                disposition: r.disposition.clone(),
                origin: format!(
                    "{} · {}, {} · {}",
                    t.plan.origin.facility,
                    t.plan.origin.city,
                    t.plan.origin.state,
                    t.plan.origin.date
                ),
                destination: format!(
                    "{} · {}, {} · {}",
                    t.plan.destination.facility,
                    t.plan.destination.city,
                    t.plan.destination.state,
                    t.plan.destination.date
                ),
                equipment: t.plan.equipment.clone(),
                message: input.message.trim().into(),
                respond_by: input.respond_by,
            };
            let html = render(&s);
            let mut p = ReleasePublication {
                id: domain::id("release-publication"),
                release_id: r.id.clone(),
                load_id: r.load_id.clone(),
                tender_id: r.tender_id.clone(),
                carrier_id: t.carrier_id.clone(),
                basis_sha256: String::new(),
                snapshot: s,
                sha256: crate::documents::sha256(html.as_bytes()),
                html,
                supersedes_id: prior.map(|p| p.id.clone()),
                published_at: Utc::now(),
                published_by: actor.into(),
            };
            p.basis_sha256 = digest(&p.basis())?;
            p.verify()?;
            let load_id = p.load_id.clone();
            self.release_publications.push(p.clone());
            let l = self
                .loads
                .iter_mut()
                .find(|l| l.id == load_id)
                .ok_or("Load not found")?;
            l.version = l.version.checked_add(1).ok_or("Load version exhausted")?;
            l.updated_at = Utc::now();
            self.record(&load_id,operation,"Carrier release request published with exact tender, disposition, message and response/application deadline; assignment retained");
            let mut result = json!(p);
            result.as_object_mut().map(|o| o.remove("html"));
            return Ok(result);
        }
        if operation == "portal.release.apply" {
            let input: ApplyInput = serde_json::from_value(body)
                .map_err(|_| "Invalid carrier release application fields")?;
            let response = self
                .release_responses
                .iter()
                .find(|r| r.id == entity)
                .ok_or("Carrier release response not found")?
                .clone();
            if response.decision != "release"
                || !response.no_custody
                || response.application.is_some()
            {
                return Err(
                    "Carrier release evidence is not awaiting application or already applied"
                        .into(),
                );
            }
            self.apply_recorded_release(
                &response,
                input.expected_load_version,
                actor,
                "operations",
            )?;
            self.record(&response.load_id,operation,"Operations applied retained authenticated carrier evidence after current custody, finance, deadline and handoff checks");
            return Ok(json!(self
                .release_responses
                .iter()
                .find(|r| r.id == entity)
                .ok_or("Carrier release response not found")?));
        }
        if operation != "portal.release.respond" {
            return Err("Unsupported release portal operation".into());
        }
        let input: TrustedResponse =
            serde_json::from_value(body).map_err(|_| "Invalid carrier release response fields")?;
        if input.subject != actor {
            return Err("Carrier identity does not match authenticated subject".into());
        }
        let p = self
            .release_publication_for(entity, &input.carrier_id)?
            .clone();
        if !self.release_current(&p)? {
            return Err(
                "Release request changed; refresh the exact current assignment before responding"
                    .into(),
            );
        }
        let r = input.request;
        let l = self
            .loads
            .iter()
            .find(|l| l.id == p.load_id)
            .ok_or("Load not found")?;
        if l.version != r.expected_load_version {
            return Err("Load changed; refresh before responding to the release request".into());
        }
        if r.document_sha256 != p.sha256 {
            return Err("Release document changed; inspect the exact published request".into());
        }
        if self
            .release_responses
            .iter()
            .any(|d| d.publication_id == entity)
        {
            return Err("Carrier already responded to this publication".into());
        }
        for (v, label) in [
            (&r.contact, "Carrier contact"),
            (&r.reference, "Carrier response reference"),
            (&r.note, "Carrier release findings"),
        ] {
            domain::required(v, label)?;
        }
        if !["release", "retain"].contains(&r.decision.as_str()) {
            return Err("Carrier release decision requires release or retain".into());
        }
        if r.decision == "release"
            && (r.no_custody != Some(true) || p.snapshot.respond_by <= Utc::now())
        {
            return Err("Release requires explicit no-pickup/no-custody confirmation before the response/application deadline".into());
        }
        if r.decision == "retain" && r.no_custody.is_some() {
            return Err("Retaining the assignment does not assert a no-custody release".into());
        }
        let hold = if r.decision == "release" {
            self.release_hold(&p)?
        } else {
            None
        };
        let mut response = ReleaseResponse {
            id: domain::id("release-response"),
            publication_id: p.id,
            release_id: p.release_id.clone(),
            load_id: p.load_id.clone(),
            tender_id: p.tender_id,
            carrier_id: p.carrier_id,
            document_sha256: p.sha256,
            decision: r.decision,
            contact: r.contact.trim().into(),
            reference: r.reference.trim().into(),
            note: r.note.trim().into(),
            no_custody: r.no_custody.unwrap_or(false),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
            outcome: if hold.is_some() {
                "awaiting_operations"
            } else {
                "applied"
            }
            .into(),
            initial_hold: hold.map(str::to_owned),
            evidence_sha256: String::new(),
            application: None,
        };
        response.evidence_sha256 = digest(&response.proof())?;
        self.release_responses.push(response.clone());
        if hold.is_none() {
            if response.decision == "release" {
                self.apply_recorded_release(&response, r.expected_load_version, actor, "native")?;
            } else {
                self.apply_tender_operation("tender.release.retain",&p.release_id,json!({"expected_load_version":r.expected_load_version,"carrier_contact":response.contact,"confirmation_reference":response.reference,"note":response.note}),actor)?;
                self.release_responses
                    .last_mut()
                    .ok_or("Internal storage error")?
                    .application = Some(ReleaseApplication {
                    applied_at: Utc::now(),
                    applied_by: actor.into(),
                    mode: "native".into(),
                });
            }
        } else {
            let l = self
                .loads
                .iter_mut()
                .find(|l| l.id == response.load_id)
                .ok_or("Load not found")?;
            l.version = l.version.checked_add(1).ok_or("Load version exhausted")?;
            l.updated_at = Utc::now();
        }
        self.record(&p.load_id,operation,if hold.is_some(){"Authenticated carrier release acknowledgement retained; operations must reconcile current holds before application"}else{"Authenticated carrier response applied to exact published release request; no financial obligation or carrier acceptance invented"});
        if let Some(e) = self.events.last_mut() {
            e.details = Some(
                json!({"response_id":response.id,"publication_id":response.publication_id,"outcome":response.outcome}),
            );
        }
        response = self
            .release_responses
            .iter()
            .find(|d| d.id == response.id)
            .ok_or("Internal storage error")?
            .clone();
        Ok(json!(response))
    }
}
