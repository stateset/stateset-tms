//! Native carrier evidence remains bound to its published tender and principal.
use crate::{
    auth::{Principal, Role},
    documents::{Document, Upload},
    domain::{self, EdiEvent, Load, Receipt, Result, Status, Workspace},
    portal::PortalOffer,
    store::Store,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierSubmission {
    pub id: String,
    pub offer_id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub kind: String,
    pub reference: String,
    pub note: String,
    pub observed_at: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub outcome: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingInput {
    pub expected_load_version: u64,
    pub kind: String,
    pub observed_at: DateTime<Utc>,
    pub location: String,
    pub reference: String,
    pub note: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PodInput {
    pub expected_load_version: u64,
    pub filename: String,
    pub reference: String,
    pub note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedTracking {
    carrier_id: String,
    subject: String,
    request: TrackingInput,
}
impl Workspace {
    fn reporting_load(
        &self,
        offer: &str,
        carrier: &str,
        version: u64,
    ) -> Result<(PortalOffer, Load)> {
        let offer = self.portal_offer_for(offer, carrier)?;
        if !self.portal_current(offer) {
            return Err(
                "Carrier assignment changed; refresh before reporting shipment evidence".into(),
            );
        }
        let load = self
            .loads
            .iter()
            .find(|l| l.id == offer.load_id)
            .ok_or("Load not found")?;
        if load.version != version {
            return Err("Load changed; refresh before reporting shipment evidence".into());
        }
        Ok((offer.clone(), load.clone()))
    }
    pub fn apply_carrier_tracking(
        &mut self,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let input: TrustedTracking =
            serde_json::from_value(body).map_err(|_| "Invalid carrier tracking fields")?;
        if input.subject != actor {
            return Err("Carrier identity does not match authenticated subject".into());
        }
        let r = input.request;
        let (offer, load) =
            self.reporting_load(entity, &input.carrier_id, r.expected_load_version)?;
        for (value, name) in [
            (&r.reference, "Carrier evidence reference"),
            (&r.note, "Carrier evidence note"),
            (&r.location, "Observed location"),
        ] {
            domain::required(value, name)?;
        }
        if r.observed_at > Utc::now() {
            return Err("Observation time cannot be in the future".into());
        }
        if r.kind == "delivery" && !self.route_ready(&load) {
            return Err("Confirm every intermediate stop before final carrier delivery".into());
        }
        let code = match r.kind.as_str() {
            "pickup"
                if matches!(
                    load.status,
                    Status::Accepted | Status::Dispatched | Status::InTransit | Status::Delivered
                ) =>
            {
                "AF"
            }
            "location" if load.status == Status::InTransit => "X6",
            "delivery" if matches!(load.status, Status::InTransit | Status::Delivered) => "D1",
            _ => {
                return Err(
                    "Report pickup after acceptance, location in transit, or delivery after pickup"
                        .into(),
                )
            }
        };
        let mut submission = CarrierSubmission {
            id: domain::id("carrier-evidence"),
            offer_id: offer.id,
            load_id: load.id.clone(),
            tender_id: offer.tender_id,
            carrier_id: offer.carrier_id,
            kind: r.kind,
            reference: r.reference.trim().into(),
            note: r.note.trim().into(),
            observed_at: Some(r.observed_at),
            location: Some(r.location.trim().into()),
            outcome: "applied".into(),
            document_id: None,
            document_sha256: None,
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
        };
        let advances = submission.kind == "pickup"
            && matches!(load.status, Status::Accepted | Status::Dispatched)
            || submission.kind == "delivery" && load.status == Status::InTransit;
        let stale = load.last_tracking_at.is_some_and(|v| r.observed_at <= v);
        if advances || !stale {
            if let Some(fact) = crate::execution::ExecutionFact::carrier(&submission) {
                self.validate_execution_fact(&load, None, fact)?;
            }
        }
        // Reuse physical progression, stale-watermark and release-after-pickup rules.
        // A source hold cannot erase actual movement; the Sync worker independently
        // holds unsafe writeback instead of marking a cancelled source shipped.
        let event = EdiEvent {
            event_id: submission.id.clone(),
            event_type: "carrier.portal.tracking".into(),
            occurred_at: r.observed_at,
            payload: json!({"load_id":load.id,"tender_id":submission.tender_id,"latest_status":code,"events":[{"status_code":code,"city":submission.location}]}),
        };
        let result = self.apply_tracking_facts(&load, &event)?;
        if result["ignored"] == true {
            submission.outcome = "stale_ignored".into();
        }
        self.record(
            &load.id,
            "portal.tracking",
            format!(
                "Carrier reported {}: {} ({})",
                submission.kind, submission.reference, submission.outcome
            ),
        );
        if let Some(e) = self.events.last_mut() {
            e.details = Some(json!(submission));
        }
        self.carrier_submissions.push(submission.clone());
        if result["ignored"] != true {
            let current = self
                .loads
                .iter()
                .find(|l| l.id == load.id)
                .ok_or("Load not found")?
                .clone();
            // Capture the authenticated actual source in the same immutable
            // snapshot as its physical progression, never a later side effect.
            self.enqueue_sync(&current);
        }
        Ok(json!(submission))
    }
    fn pod_status<'a>(&'a self, load: &'a Load) -> &'a str {
        if let Some(id) = &load.pod_document_id {
            self.documents
                .iter()
                .find(|d| &d.id == id)
                .map_or("unavailable", Document::status)
        } else if load.pod_reference.is_some() {
            load.pod_review
                .as_ref()
                .map_or("unreviewed", |r| r.decision.as_str())
        } else {
            "missing"
        }
    }
    fn carrier_can_submit_pod(&self, load: &Load) -> bool {
        load.status == Status::Delivered
            && !self.evidence_locked(&load.id)
            && matches!(self.pod_status(load), "missing" | "rejected")
    }
    pub fn carrier_submission_for(&self, id: &str, carrier: &str) -> Result<&CarrierSubmission> {
        self.carrier_submissions
            .iter()
            .find(|s| s.id == id && s.carrier_id == carrier)
            .ok_or("Carrier submission not found".into())
    }
    pub(crate) fn carrier_reporting(&self, offer: &PortalOffer, current: bool) -> Value {
        let load = self
            .loads
            .iter()
            .find(|l| l.id == offer.load_id)
            .filter(|_| current);
        let kinds: Vec<&str> = match load.map(|l| &l.status) {
            Some(Status::Accepted | Status::Dispatched) => vec!["pickup"],
            Some(Status::InTransit) => {
                if load.is_some_and(|l| self.route_ready(l)) {
                    vec!["location", "delivery"]
                } else {
                    vec!["location"]
                }
            }
            _ => vec![],
        };
        let submissions = self.carrier_submissions.iter().filter(|s| s.tender_id == offer.tender_id && s.carrier_id == offer.carrier_id).map(|s| {
            let doc = s.document_id.as_ref().and_then(|id| self.documents.iter().find(|d| &d.id == id));
            json!({"id":s.id,"kind":s.kind,"reference":s.reference,"note":s.note,"observed_at":s.observed_at,"effective_observed_at":self.effective_movement_time(s).ok().flatten(),"time_corrected":self.current_movement_correction(s).ok().flatten().is_some(),"location":s.location,"outcome":s.outcome,"recorded_at":s.recorded_at,"recorded_by":s.recorded_by,
                "document":doc.map(|d|json!({"filename":d.filename,"content_type":d.content_type,"size_bytes":d.size_bytes,"sha256":d.sha256,"status":d.status()}))})
        }).collect::<Vec<_>>();
        json!({"execution_timeline":load.map(|l|self.execution_timeline(l,false)),"itinerary":load.map(|l|self.itinerary_view(l,false)),"stop_services":self.stop_services.iter().filter(|s|s.tender_id==offer.tender_id && s.offer_id.as_ref().is_some_and(|id|self.portal_offers.iter().any(|o|&o.id==id && o.carrier_id==offer.carrier_id))).map(|s|json!({"id":s.id,"stop_id":s.stop_id,"observed_at":s.observed_at,"reference":s.reference,"note":s.note,"recorded_at":s.recorded_at,"recorded_by":s.recorded_by,"supersedes_id":s.supersedes_id})).collect::<Vec<_>>(),"tracking_kinds":kinds,"can_submit_pod":load.is_some_and(|l| self.carrier_can_submit_pod(l)),"pod_status":load.map(|l| self.pod_status(l)),"submissions":submissions})
    }
}
impl Store {
    pub async fn submit_carrier_pod(
        &self,
        principal: &Principal,
        key: &str,
        offer_id: &str,
        input: PodInput,
        bytes: Vec<u8>,
        content_type: String,
    ) -> Result<Value> {
        if principal.role != Role::Carrier || !principal.valid_scope() {
            return Err("Carrier scope required".into());
        }
        let carrier = principal
            .counterparty_id
            .as_deref()
            .ok_or("Carrier scope required")?;
        domain::required(&input.reference, "Carrier evidence reference")?;
        domain::required(&input.note, "Carrier evidence note")?;
        let upload = Upload {
            kind: "pod".into(),
            filename: input.filename.clone(),
            content_type,
            bytes,
        };
        let document = upload.document("", &principal.subject)?;
        let fingerprint = crate::store::fingerprint(
            "portal.pod",
            offer_id,
            &json!({"carrier_id":carrier,"subject":principal.subject,"request":input,"content_type":document.content_type,"size_bytes":document.size_bytes,"sha256":document.sha256}),
        );
        let blob_id = document.id.clone();
        let owned = (
            principal.clone(),
            key.to_owned(),
            offer_id.to_owned(),
            carrier.to_owned(),
        );
        self.update_receipted(&principal.tenant_id,Some((blob_id,upload.bytes)),None,move |w| {
            let (principal, key, offer_id, carrier) = (&owned.0, owned.1.as_str(), owned.2.as_str(), owned.3.as_str());
            // Scope is checked even for replay; the receipt binds the authenticated
            // subject and exact bytes. Historical replay returns only own evidence.
            w.portal_offer_for(offer_id,carrier)?;
            if let Some(receipt) = w.receipts.get(key) {
                let response=receipt.checked_response(&fingerprint)?;
                return Ok(json!({"data":response,"replayed":true}));
            }
            let (offer,load) = w.reporting_load(offer_id,carrier,input.expected_load_version)?;
            if !w.carrier_can_submit_pod(&load) { return Err("Carrier POD requires delivery and missing or rejected evidence; contact operations before replacing pending, accepted, or financially locked proof".into()); }
            let mut document = document;
            document.load_id = load.id.clone();
            let submission = CarrierSubmission {id:domain::id("carrier-evidence"),offer_id:offer.id,load_id:load.id.clone(),tender_id:offer.tender_id,carrier_id:offer.carrier_id,kind:"pod".into(),reference:input.reference.trim().into(),note:input.note.trim().into(),observed_at:None,location:None,outcome:"attached_unreviewed".into(),document_id:Some(document.id.clone()),document_sha256:Some(document.sha256.clone()),recorded_at:Utc::now(),recorded_by:principal.subject.clone()};
            w.documents.push(document.clone());
            let start = w.events.len();
            w.apply("load.command",&load.id,json!({"action":"attach_pod","expected_version":load.version,"document_id":document.id}))?;
            w.record(&load.id,"portal.pod",format!("Carrier submitted POD {}: {}; finance review required",document.filename,submission.reference));
            if let Some(e) = w.events.last_mut() {e.details=Some(json!(submission));}
            for e in &mut w.events[start..] { e.actor=principal.subject.clone(); }
            w.carrier_submissions.push(submission.clone());
            let result=json!(submission);
            w.receipts.insert(key.into(),Receipt {fingerprint,response:result.clone(),stopped:None});
            Ok(json!({"data":result,"replayed":false}))
        }).await
    }
}
