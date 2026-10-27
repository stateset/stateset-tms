//! Ordered intermediate stops with explicit, current-tender service evidence.
use crate::domain::{self, Load, Result, Status, Stop, Workspace};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntermediateStop {
    pub id: String,
    pub kind: String,
    pub stop: Stop,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StopService {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub stop_id: String,
    pub stop: IntermediateStop,
    pub observed_at: DateTime<Utc>,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub offer_id: Option<String>,
    pub supersedes_id: Option<String>,
    pub evidence_sha256: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceInput {
    pub expected_load_version: u64,
    pub stop_id: String,
    pub observed_at: DateTime<Utc>,
    pub contact: String,
    pub reference: String,
    pub note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedService {
    carrier_id: String,
    subject: String,
    request: ServiceInput,
}
impl StopService {
    fn basis(&self) -> Value {
        json!({"id":self.id,"load_id":self.load_id,"tender_id":self.tender_id,"stop_id":self.stop_id,"stop":self.stop,"observed_at":self.observed_at,"contact":self.contact,"reference":self.reference,"note":self.note,"recorded_at":self.recorded_at,"recorded_by":self.recorded_by,"offer_id":self.offer_id,"supersedes_id":self.supersedes_id})
    }
    fn digest(&self) -> String {
        crate::documents::sha256(self.basis().to_string().as_bytes())
    }
    pub fn verify(&self) -> Result<()> {
        if self.digest() != self.evidence_sha256 {
            Err("Internal storage error".into())
        } else {
            Ok(())
        }
    }
}
pub fn validate_route(
    origin: &Stop,
    extra: &[IntermediateStop],
    destination: &Stop,
    tender: bool,
) -> Result<()> {
    if extra.len() > 18 {
        return Err(
            "A shipment supports at most twenty stops including origin and final delivery".into(),
        );
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut date = origin.date;
    for v in extra {
        if !v.id.starts_with("stop_")
            || v.id.len() <= 5
            || v.id.len() > 100
            || !v
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || !ids.insert(&v.id)
        {
            return Err("Intermediate stop IDs must be unique stop_ identifiers with ASCII letters, digits, hyphens or underscores".into());
        }
        if !["pickup", "delivery"].contains(&v.kind.as_str()) {
            return Err("Intermediate stop kind must be pickup or delivery".into());
        }
        crate::stops::validate_stop(&v.stop, tender)?;
        if v.stop.date < date || v.stop.date > destination.date {
            return Err(
                "Route dates must follow origin through final delivery in stop order".into(),
            );
        }
        date = v.stop.date;
    }
    Ok(())
}
pub fn route_stops(load: &Load) -> Vec<(&str, &str, &Stop)> {
    let mut stops = vec![("pickup", "pickup", &load.origin)];
    stops.extend(
        load.intermediate_stops
            .iter()
            .map(|s| (s.id.as_str(), s.kind.as_str(), &s.stop)),
    );
    stops.push(("delivery", "delivery", &load.destination));
    stops
}
fn verify_services(load: &Load, history: &[&StopService]) -> Result<()> {
    let mut ids = std::collections::BTreeSet::new();
    let mut replaced = std::collections::BTreeSet::new();
    for service in history {
        service.verify()?;
        if !ids.insert(service.id.as_str())
            || !load
                .intermediate_stops
                .iter()
                .any(|s| s.id == service.stop_id && json!(s) == json!(service.stop))
        {
            return Err("Internal storage error".into());
        }
        if let Some(prior) = &service.supersedes_id {
            if !replaced.insert(prior.as_str())
                || !history.iter().any(|old| {
                    old.id == *prior
                        && old.id != service.id
                        && old.stop_id == service.stop_id
                        && old.recorded_at < service.recorded_at
                })
            {
                return Err("Internal storage error".into());
            }
        }
    }
    Ok(())
}
#[path = "itinerary_read.rs"]
mod read;
#[cfg(test)]
pub(crate) use read::tests::fixture as service_read_fixture;
pub(crate) use read::ServiceReadIndex;

impl Workspace {
    fn verify_stop_services(&self, load: &Load) -> Result<()> {
        let history: Vec<_> = self
            .stop_services
            .iter()
            .filter(|s| s.load_id == load.id && Some(&s.tender_id) == load.tender_id.as_ref())
            .collect();
        verify_services(load, &history)
    }
    pub fn current_services<'a>(&'a self, load: &Load) -> Vec<&'a StopService> {
        let replaced: std::collections::BTreeSet<&str> = self
            .stop_services
            .iter()
            .filter_map(|s| s.supersedes_id.as_deref())
            .collect();
        self.stop_services
            .iter()
            .filter(|s| {
                s.load_id == load.id
                    && Some(&s.tender_id) == load.tender_id.as_ref()
                    && !replaced.contains(s.id.as_str())
            })
            .collect()
    }
    pub fn route_ready(&self, load: &Load) -> bool {
        if self.verify_stop_services(load).is_err() {
            return false;
        }
        let services = self.current_services(load);
        load.intermediate_stops.iter().all(|s| {
            services
                .iter()
                .filter(|e| e.stop_id == s.id && e.verify().is_ok() && json!(e.stop) == json!(s))
                .count()
                == 1
        })
    }
    pub fn stop_complete(&self, load: &Load, key: &str) -> bool {
        match key {
            "pickup" => matches!(load.status, Status::InTransit | Status::Delivered),
            "delivery" => load.status == Status::Delivered,
            _ => {
                self.verify_stop_services(load).is_ok()
                    && self.current_services(load).iter().any(|s| {
                        s.stop_id == key
                            && s.verify().is_ok()
                            && load
                                .intermediate_stops
                                .iter()
                                .any(|r| r.id == key && json!(s.stop) == json!(r))
                    })
            }
        }
    }
    pub fn itinerary_view(&self, load: &Load, private: bool) -> Value {
        let services = self.current_services(load);
        json!({"requires_native_handoff":!load.intermediate_stops.is_empty(),"all_intermediate_stops_confirmed":self.route_ready(load),"stops":route_stops(load).iter().enumerate().map(|(index,(id,kind,stop))|{
            let service=services.iter().find(|s|s.stop_id==*id);
            json!({"id":id,"sequence":index+1,"kind":kind,"stop":stop,"completed":self.stop_complete(load,id),"service":service.map(|s|if private {json!(s)} else {json!({"observed_at":s.observed_at})})})
        }).collect::<Vec<_>>()})
    }
    pub fn apply_itinerary_portal(
        &mut self,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let input: TrustedService =
            serde_json::from_value(body).map_err(|_| "Invalid carrier stop service fields")?;
        if actor != input.subject {
            return Err("Carrier identity does not match authenticated subject".into());
        }
        let offer = self.portal_offer_for(entity, &input.carrier_id)?.clone();
        if !self.portal_current(&offer) {
            return Err(
                "Carrier assignment changed; refresh before reporting stop evidence".into(),
            );
        }
        let load = self
            .loads
            .iter()
            .find(|l| l.id == offer.load_id)
            .ok_or("Load not found")?;
        if load.tender_id.as_deref() != Some(&offer.tender_id)
            || !matches!(load.status, Status::InTransit | Status::Delivered)
        {
            return Err("Current picked-up carrier assignment required for stop service".into());
        }
        let result = self.record_stop_service(
            &offer.load_id,
            input.request,
            None,
            Some(entity.into()),
            actor,
        )?;
        // Only the carrier's own submitted evidence is returned, never the load aggregate.
        Ok(result)
    }
    pub fn apply_itinerary(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let input: ServiceInput =
            serde_json::from_value(body).map_err(|_| "Invalid stop service fields")?;
        if operation == "itinerary.service.record" {
            return self.record_stop_service(entity, input, None, None, actor);
        }
        if operation != "itinerary.service.correct" {
            return Err("Unsupported itinerary operation".into());
        }
        let old = self
            .stop_services
            .iter()
            .find(|s| s.id == entity)
            .ok_or("Stop service not found")?
            .clone();
        old.verify()?;
        if old.stop_id != input.stop_id {
            return Err("Correction must retain the original stop identity".into());
        }
        self.record_stop_service(&old.load_id, input, Some(old.id), None, actor)
    }
    fn record_stop_service(
        &mut self,
        load_id: &str,
        input: ServiceInput,
        supersedes: Option<String>,
        offer_id: Option<String>,
        actor: &str,
    ) -> Result<Value> {
        let index = self
            .loads
            .iter()
            .position(|l| l.id == load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        if load.version != input.expected_load_version {
            return Err("Load changed; refresh before confirming stop service".into());
        }
        if !matches!(load.status, Status::InTransit | Status::Delivered) {
            return Err(
                "Intermediate service requires physical pickup on the current assignment".into(),
            );
        }
        if self.execution_evidence_locked(load_id) {
            return Err("Stop service evidence is locked by posted financial records".into());
        }
        let tender = self
            .tenders
            .iter()
            .find(|t| {
                Some(&t.id) == load.tender_id.as_ref()
                    && Some(&t.carrier_id) == load.carrier_id.as_ref()
            })
            .ok_or("Current carrier tender required")?;
        let (position, stop) = load
            .intermediate_stops
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == input.stop_id)
            .ok_or("Intermediate stop not found")?;
        if json!(load.intermediate_stops) != json!(tender.plan.intermediate_stops) {
            return Err(
                "Route changed; reconcile exact carrier plan before recording service".into(),
            );
        }
        for (v, name) in [
            (&input.contact, "Actual stop contact"),
            (&input.reference, "Stop service reference"),
            (&input.note, "Stop service findings"),
        ] {
            domain::required(v, name)?;
        }
        if input.observed_at > Utc::now() {
            return Err("Actual stop service cannot be in the future".into());
        }
        self.verify_stop_services(&load)?;
        let current = self.current_services(&load);
        for s in &current {
            s.verify()?;
        }
        let previous = current.iter().find(|s| s.stop_id == input.stop_id);
        if previous.map(|s| s.id.as_str()) != supersedes.as_deref() {
            return Err(
                "Stop service already recorded or changed; use its current explicit correction"
                    .into(),
            );
        }
        // A missing earlier confirmation remains missing; never silently serve it.
        for prior in &load.intermediate_stops[..position] {
            if !current.iter().any(|s| s.stop_id == prior.id) {
                return Err("Confirm earlier intermediate stops before this stop".into());
            }
        }
        for existing in &current {
            if existing.stop_id == input.stop_id {
                continue;
            }
            let p = load
                .intermediate_stops
                .iter()
                .position(|s| s.id == existing.stop_id)
                .ok_or("Internal storage error")?;
            if (p < position && existing.observed_at > input.observed_at)
                || (p > position && existing.observed_at < input.observed_at)
            {
                return Err("Stop service times must follow the ordered route".into());
            }
        }
        for s in self.carrier_submissions.iter().filter(|s| {
            s.load_id == load.id
                && Some(&s.tender_id) == load.tender_id.as_ref()
                && s.outcome == "applied"
        }) {
            if let Some(time) = self.effective_movement_time(s)? {
                if (s.kind == "pickup" && input.observed_at < time)
                    || (s.kind == "delivery" && input.observed_at > time)
                {
                    return Err(
                        "Stop service time conflicts with known carrier pickup or final delivery"
                            .into(),
                    );
                }
            }
        }
        for gate in self
            .current_stop_events(&load)
            .iter()
            .filter(|g| g.stop == input.stop_id)
        {
            if (gate.kind == "arrival" && gate.occurred_at > input.observed_at)
                || (gate.kind == "departure" && gate.occurred_at < input.observed_at)
            {
                return Err(
                    "Stop service must fall between known gate arrival and departure".into(),
                );
            }
        }
        let mut service = StopService {
            id: domain::id("stop-service"),
            load_id: load.id.clone(),
            tender_id: tender.id.clone(),
            stop_id: input.stop_id,
            stop: stop.clone(),
            observed_at: input.observed_at,
            contact: input.contact.trim().into(),
            reference: input.reference.trim().into(),
            note: input.note.trim().into(),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
            offer_id,
            supersedes_id: supersedes,
            evidence_sha256: String::new(),
        };
        service.evidence_sha256 = service.digest();
        service.verify()?;
        self.validate_execution_fact(
            &load,
            service.supersedes_id.as_deref(),
            crate::execution::ExecutionFact::service(&service),
        )?;
        self.stop_services.push(service.clone());
        load.version = load
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        load.updated_at = Utc::now();
        self.loads[index] = load.clone();
        self.enqueue_sync(&load);
        self.record(&load.id,"itinerary.service","Actual intermediate stop service recorded; original tender, prices and evidence retained");
        if let Some(event) = self.events.last_mut() {
            event.details = Some(
                json!({"service_id":service.id,"stop_id":service.stop_id,"supersedes_id":service.supersedes_id}),
            );
        }
        Ok(json!(service))
    }
}

pub fn route_document(stops: &[IntermediateStop]) -> String {
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
    if stops.is_empty() {
        return String::new();
    }
    let mut text=String::from("<h2>Intermediate stops in required order</h2><p>Confirm service at every intermediate stop. End-to-end status alone does not establish intermediate service.</p>");
    for (i, s) in stops.iter().enumerate() {
        text.push_str(&format!(
            "<h3>Stop {} · {}</h3><p>{}<br>{}<br>{}, {} {} {}<br>Planned date: {}</p><p>Contact: {} · {}</p><p>{}</p>",
            i + 2,
            esc(&s.kind),
            esc(&s.stop.facility),
            s.stop
                .details
                .address_lines
                .iter()
                .map(|v| esc(v))
                .collect::<Vec<_>>()
                .join("<br>"),
            esc(&s.stop.city),
            esc(&s.stop.state),
            esc(&s.stop.details.postal_code),
            esc(&s.stop.details.country),
            s.stop.date,
            esc(&s.stop.details.contact),
            esc(&s.stop.details.phone),
            esc(&s.stop.details.instructions)
        ));
    }
    text
}
