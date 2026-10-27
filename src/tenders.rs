//! Tender identity, proven-unsent withdrawal, and externally confirmed carrier
//! release. The current EDI gateway emits originals only; no cancel is fabricated.
use crate::domain::{self, Carrier, EdiEvent, Load, Outbox, Result, Status, Workspace};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TenderAttempt {
    pub id: String,
    pub load_id: String,
    pub sequence: u64,
    pub shipment_reference: String,
    pub carrier_id: String,
    pub carrier: Carrier,
    pub plan: Load,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: DateTime<Utc>,
    pub legacy: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierRelease {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub disposition: String,
    pub reason: String,
    pub status: String,
    pub requested_at: DateTime<Utc>,
    pub requested_by: String,
    pub confirmation: Option<CarrierConfirmation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierConfirmation {
    pub carrier_contact: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TenderIncident {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub event: EdiEvent,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub resolution: Option<CarrierConfirmation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseInput {
    expected_load_version: u64,
    disposition: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmationInput {
    expected_load_version: u64,
    carrier_contact: String,
    confirmation_reference: String,
    note: String,
}
pub enum TenderEvent {
    Current(Box<Load>),
    Historical(Value),
}

impl Workspace {
    pub fn tender_expiry_due(&self, now: DateTime<Utc>) -> bool {
        self.loads.iter().any(|l| {
            l.status == Status::Tendered
                && l.tender_expires_at.is_some_and(|at| at < now)
                && l.exception.is_none()
        })
    }
    pub fn expire_tenders(&mut self, now: DateTime<Utc>) -> Result<usize> {
        let due = self
            .loads
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.status == Status::Tendered
                    && l.tender_expires_at.is_some_and(|at| at < now)
                    && l.exception.is_none()
            })
            .map(|(i, l)| {
                l.version
                    .checked_add(1)
                    .map(|version| (i, version))
                    .ok_or("Shipment version exhausted".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        for (index, version) in &due {
            let l = &mut self.loads[*index];
            l.version = *version;
            l.updated_at = now;
            l.exception =
                Some("Tender response overdue; contact carrier before retendering".into());
            let id = l.id.clone();
            self.record(&id, "tender.overdue", "Carrier response SLA exceeded");
        }
        Ok(due.len())
    }
}
impl crate::store::Store {
    pub async fn expire_tenders(&self, tenant: &str) -> Result<usize> {
        if !self.tender_expiry_may_be_due(tenant).await? {
            return Ok(0);
        }
        if !self.read(tenant).await?.tender_expiry_due(Utc::now()) {
            return Ok(0);
        }
        let result = self
            .update_if(tenant, |w| {
                let expired = w.expire_tenders(Utc::now())?;
                Ok((expired > 0).then(|| json!(expired)))
            })
            .await?;
        Ok(result.and_then(|v| v.as_u64()).unwrap_or(0) as usize)
    }
}

impl Workspace {
    pub fn release_pending(&self, load: &Load) -> bool {
        self.releases
            .iter()
            .any(|v| v.status == "pending" && Some(&v.tender_id) == load.tender_id.as_ref())
    }
    pub fn unresolved_tender_incidents(&self, load_id: &str) -> bool {
        self.tender_incidents
            .iter()
            .any(|v| v.load_id == load_id && v.status == "open")
            || self.unresolved_rate_handoffs(load_id)
    }
    pub fn can_withdraw_tender(&self, load: &Load) -> bool {
        if load.tender_id.as_deref().is_some_and(|id| {
            self.portal_published(id)
                || self.carrier_file_shared(id)
                || self.release_request_published(id)
        }) {
            return false;
        }
        if load.status != Status::Tendered
            || self.release_pending(load)
            || self.rate_confirmations.iter().any(|v| {
                Some(&v.tender_id) == load.tender_id.as_ref()
                    && v.handoffs.iter().any(|h| h.retraction.is_none())
            })
        {
            return false;
        }
        let Some(tender) = self
            .tenders
            .iter()
            .find(|v| Some(&v.id) == load.tender_id.as_ref() && !v.legacy && v.status == "offered")
        else {
            return false;
        };
        let jobs: Vec<_> = self
            .outbox
            .iter()
            .filter(|v| {
                v.tender_id.as_deref() == Some(&tender.id) && v.destination == "stateset-edi"
            })
            .collect();
        !jobs.is_empty()
            && jobs.iter().all(|v| {
                v.transmission == "not_started"
                    && v.attempts == 0
                    && v.total_attempts == 0
                    && ["pending_handoff", "native_only"].contains(&v.status.as_str())
                    && v.response.is_none()
            })
    }
    pub fn tender_controls(&self, load: &Load) -> Value {
        json!({"can_withdraw":self.can_withdraw_tender(load),"release_pending":self.release_pending(load),"open_incidents":self.tender_incidents.iter().filter(|v|v.load_id==load.id && v.status=="open").count() + self.rate_confirmations.iter().filter(|v|v.load_id==load.id).flat_map(|v|&v.handoffs).filter(|h|h.coordination_required && h.retraction.is_none() && h.resolution.is_none()).count()})
    }
    pub fn adopt_tender(&mut self, load: &mut Load) -> Result<()> {
        if load.tender_id.is_some() || load.carrier_id.is_none() {
            return Ok(());
        }
        let carrier = self
            .carriers
            .iter()
            .find(|v| Some(&v.id) == load.carrier_id.as_ref())
            .ok_or("Carrier not found")?
            .clone();
        let id = domain::id("tender");
        let now = Utc::now();
        let status = match load.status {
            Status::Tendered => "offered",
            Status::Accepted | Status::Dispatched => "accepted",
            Status::InTransit => "in_transit",
            Status::Delivered => "delivered",
            Status::Cancelled => "closed",
            Status::Planned => return Err("Planned load cannot retain a carrier assignment".into()),
        };
        let attempt = TenderAttempt {
            id: id.clone(),
            load_id: load.id.clone(),
            sequence: self.tenders.iter().filter(|v| v.load_id == load.id).count() as u64 + 1,
            shipment_reference: load.reference.clone(),
            carrier_id: carrier.id.clone(),
            carrier,
            plan: load.clone(),
            status: status.into(),
            created_at: now,
            updated_at: now,
            created_by: "legacy_snapshot".into(),
            legacy: true,
        };
        for job in &mut self.outbox {
            if job.load_id == load.id
                && job.destination == "stateset-edi"
                && job.tender_id.is_none()
            {
                job.tender_id = Some(id.clone());
            }
        }
        self.tenders.push(attempt);
        load.tender_id = Some(id);
        self.record(&load.id,"tender.legacy_adopted","Existing carrier assignment retained with legacy shipment identity; transmission history is uncertain");
        Ok(())
    }
    pub fn prepare_tender(&mut self, load: &mut Load, carrier: Carrier) -> Result<String> {
        self.guard_source(load)?;
        self.verify_quoted_tender(load, &carrier)?;
        if self.has_approved_charges(&load.id) {
            return Err("Reconcile approved charges before re-tendering".into());
        }
        if load.status != Status::Planned {
            return Err("Only planned loads can be tendered".into());
        }
        crate::stops::validate_stop(&load.origin, true)?;
        crate::stops::validate_stop(&load.destination, true)?;
        crate::itinerary::validate_route(
            &load.origin,
            &load.intermediate_stops,
            &load.destination,
            true,
        )?;
        if self.unresolved_tender_incidents(&load.id) {
            return Err("Resolve carrier tender incidents before reassigning this load".into());
        }
        if self.invoices.iter().any(|v| v.load_id == load.id)
            || self
                .bills
                .iter()
                .any(|v| v.load_id == load.id && v.status != "void")
        {
            return Err("Reconcile existing financial documents before re-tendering".into());
        }
        if carrier.insurance_expires < self.coverage_date(load).max(Utc::now().date_naive()) {
            return Err("Carrier insurance must cover delivery and appointment dates".into());
        }
        if carrier.equipment != load.equipment {
            return Err("Carrier equipment does not match load requirements".into());
        }
        let id = domain::id("tender");
        let shipment = format!(
            "SF-{}",
            uuid::Uuid::new_v4().simple().to_string()[..24].to_ascii_uppercase()
        );
        let now = Utc::now();
        let attempt = TenderAttempt {
            id: id.clone(),
            load_id: load.id.clone(),
            sequence: self.tenders.iter().filter(|v| v.load_id == load.id).count() as u64 + 1,
            shipment_reference: shipment.clone(),
            carrier_id: carrier.id.clone(),
            carrier: carrier.clone(),
            plan: load.clone(),
            status: "offered".into(),
            created_at: now,
            created_by: "system".into(),
            updated_at: now,
            legacy: false,
        };
        self.outbox.push(Outbox{id:domain::id("outbox"),load_id:load.id.clone(),destination:"stateset-edi".into(),status:"pending_handoff".into(),method:"POST".into(),path:format!("/v1/edi/outbound/204/{}",carrier.partner_id),attempts:0,total_attempts:0,next_attempt_at:None,leased_until:None,lease:None,last_error:None,response:None,tender_id:Some(id.clone()),transmission:"not_started".into(),payload:json!({"shipment_id":shipment,"carrier_scac":carrier.scac,"payment_method":"PP","pickup_date":load.origin.date,"bol_number":load.reference,"weight":load.weight_lbs.to_string(),"weight_uom":"L","ship_from":crate::stops::party(&load.origin,"SF"),"ship_to":crate::stops::party(&load.destination,"ST"),"orders":[{"po_number":load.order_number.as_deref().unwrap_or(&load.reference),"quantity":"1","weight":load.weight_lbs.to_string()}]})});
        if !load.intermediate_stops.is_empty() {
            let job = self.outbox.last_mut().ok_or("Internal storage error")?;
            job.status = "native_only".into();
            job.last_error=Some("Current EDI 204 cannot encode intermediate stops. Publish the full native carrier document and obtain actual acceptance.".into());
        }
        self.tenders.push(attempt);
        load.tender_id = Some(id);
        load.carrier_id = Some(carrier.id);
        load.status = Status::Tendered;
        load.tender_expires_at = Some(now + chrono::Duration::hours(2));
        let coordination = if load.intermediate_stops.is_empty() {
            "EDI handoff pending"
        } else {
            "full-route native carrier coordination required"
        };
        Ok(format!(
            "Tender prepared for {}; carrier shipment identity {shipment}; {coordination}",
            carrier.name
        ))
    }
    pub fn finish_tender_state(&mut self, load: &Load, status: &str) {
        if let Some(attempt) = self
            .tenders
            .iter_mut()
            .find(|v| Some(&v.id) == load.tender_id.as_ref())
        {
            attempt.status = status.into();
            attempt.updated_at = Utc::now();
        }
    }
    pub fn close_declined_release(&mut self, load: &Load) {
        for release in &mut self.releases {
            if Some(&release.tender_id) == load.tender_id.as_ref() && release.status == "pending" {
                release.status = "closed_by_decline".into();
            }
        }
    }
    pub fn withdraw_tender(&mut self, load: &mut Load, cancel: bool) -> Result<()> {
        if self.has_approved_charges(&load.id) {
            return Err(
                "Reconcile approved charges before withdrawing the carrier assignment".into(),
            );
        }
        if !self.can_withdraw_tender(load) {
            return Err("Carrier commitment or transmission is uncertain; request and confirm carrier release before cancellation or re-tendering".into());
        }
        self.finish_tender_state(load, "withdrawn");
        self.suppress_tender_jobs(load.tender_id.as_deref());
        clear_assignment(load, cancel);
        Ok(())
    }
    pub(crate) fn suppress_tender_jobs(&mut self, tender: Option<&str>) {
        let Some(tender) = tender else {
            return;
        };
        for job in &mut self.outbox {
            if job.destination == "stateset-edi" && job.tender_id.as_deref() == Some(tender) {
                job.status = "superseded".into();
                job.lease = None;
                job.leased_until = None;
                job.next_attempt_at = None;
            }
        }
    }
    pub fn block_release_after_pickup(&mut self, load: &Load, event: &EdiEvent) {
        let pending: Vec<_> = self
            .releases
            .iter()
            .filter(|v| Some(&v.tender_id) == load.tender_id.as_ref() && v.status == "pending")
            .map(|v| v.id.clone())
            .collect();
        if pending.is_empty() {
            return;
        }
        for release in &mut self.releases {
            if pending.contains(&release.id) {
                release.status = "blocked_by_movement".into();
            }
        }
        if let Some(id) = &load.tender_id {
            self.record_tender_incident(load, id, event);
        }
    }
    fn record_tender_incident(&mut self, load: &Load, tender_id: &str, event: &EdiEvent) -> String {
        if let Some(incident) = self
            .tender_incidents
            .iter()
            .find(|v| v.event.event_id == event.event_id)
        {
            return incident.id.clone();
        }
        let incident = TenderIncident {
            id: domain::id("incident"),
            load_id: load.id.clone(),
            tender_id: tender_id.into(),
            event: event.clone(),
            status: "open".into(),
            created_at: Utc::now(),
            resolution: None,
        };
        self.record(&load.id,"tender.incident",format!("Carrier event {} conflicts with release/assignment history; operational and financial hold requires reconciliation",event.event_id));
        if let Some(entry) = self.events.last_mut() {
            entry.details =
                Some(json!({"incident_id":incident.id,"tender_id":tender_id,"event":event}));
        }
        let id = incident.id.clone();
        self.tender_incidents.push(incident);
        id
    }
    pub fn correlate_tender_event(&mut self, event: &EdiEvent) -> Result<TenderEvent> {
        let reference = event.payload["shipment_id"]
            .as_str()
            .ok_or("shipment_id required")?;
        let attempt = if let Some(attempt) = self
            .tenders
            .iter()
            .find(|v| v.shipment_reference == reference)
        {
            attempt.clone()
        } else {
            let index = self
                .loads
                .iter()
                .position(|v| v.reference == reference)
                .ok_or("Shipment not found")?;
            let mut load = self.loads[index].clone();
            if load.tender_id.is_some() {
                return Err("EDI shipment identity does not match the current tender; carrier must echo the 204 shipment_id".into());
            }
            self.adopt_tender(&mut load)?;
            let attempt = self
                .tenders
                .iter()
                .find(|v| Some(&v.id) == load.tender_id.as_ref())
                .ok_or("Shipment has no carrier")?
                .clone();
            self.loads[index] = load;
            attempt
        };
        if event.payload["partner_id"].as_str() != Some(&attempt.carrier.partner_id) {
            return Err("Event partner does not match the tendered carrier".into());
        }
        if event
            .payload
            .get("carrier_scac")
            .and_then(Value::as_str)
            .is_some_and(|v| v != attempt.carrier.scac)
        {
            return Err("SCAC does not match carrier".into());
        }
        let load = self
            .loads
            .iter()
            .find(|v| v.id == attempt.load_id)
            .ok_or("Load not found")?
            .clone();
        if load.tender_id.as_deref() == Some(&attempt.id)
            && !["released", "withdrawn", "declined", "closed"].contains(&attempt.status.as_str())
        {
            return Ok(TenderEvent::Current(Box::new(load)));
        }
        let movement = event.event_type == "edi.shipment_status.received"
            && event.payload["events"]
                .as_array()
                .map_or(event.payload["latest_status"] != "AG", |v| {
                    v.iter().any(|v| v["status_code"] != "AG")
                });
        if movement || event.event_type == "edi.freight_invoice.received" {
            let incident = self.record_tender_incident(&load, &attempt.id, event);
            Ok(TenderEvent::Historical(
                json!({"held":true,"reason":"historical_carrier_conflict","load_id":load.id,"tender_id":attempt.id,"incident_id":incident}),
            ))
        } else {
            self.record(
                &load.id,
                "tender.stale_response",
                format!(
                    "Response {} retained for historical tender {}; current assignment unchanged",
                    event.event_id, attempt.shipment_reference
                ),
            );
            Ok(TenderEvent::Historical(
                json!({"ignored":true,"reason":"historical_tender","load_id":load.id,"tender_id":attempt.id}),
            ))
        }
    }
    pub fn apply_tender_operation(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "tender.release.request" {
            if self.has_approved_charges(entity) {
                return Err("Reconcile approved charges before releasing the carrier".into());
            }
            let input: ReleaseInput = decode(body)?;
            domain::required(&input.reason, "Release reason")?;
            if !["replan", "cancel"].contains(&input.disposition.as_str()) {
                return Err("Release disposition must be replan or cancel".into());
            }
            let index = self
                .loads
                .iter()
                .position(|v| v.id == entity)
                .ok_or("Load not found")?;
            let mut load = self.loads[index].clone();
            check_version(&load, input.expected_load_version)?;
            if !matches!(
                load.status,
                Status::Tendered | Status::Accepted | Status::Dispatched
            ) {
                return Err("Carrier release requires a pre-pickup assignment".into());
            }
            self.adopt_tender(&mut load)?;
            if self.release_pending(&load) {
                return Err("Carrier release already pending".into());
            }
            if self.can_withdraw_tender(&load) {
                return Err(
                    "Tender has not been handed off; withdraw the unsent tender instead".into(),
                );
            }
            if self.invoices.iter().any(|v| v.load_id == entity)
                || self
                    .bills
                    .iter()
                    .any(|v| v.load_id == entity && v.status != "void")
            {
                return Err("Reconcile financial documents before releasing the carrier".into());
            }
            let release = CarrierRelease {
                id: domain::id("release"),
                load_id: load.id.clone(),
                tender_id: load.tender_id.clone().ok_or("Tender required")?,
                disposition: input.disposition,
                reason: input.reason,
                status: "pending".into(),
                requested_at: Utc::now(),
                requested_by: actor.into(),
                confirmation: None,
            };
            bump(&mut load)?;
            self.loads[index] = load.clone();
            self.releases.push(release.clone());
            self.record(&load.id,operation,format!("Carrier release requested for {}; external confirmation required before reassignment",release.disposition));
            return Ok(json!(release));
        }
        let input: ConfirmationInput = decode(body)?;
        domain::required(&input.carrier_contact, "Carrier contact")?;
        domain::required(
            &input.confirmation_reference,
            "Carrier confirmation reference",
        )?;
        domain::required(&input.note, "Carrier coordination note")?;
        let confirmation = CarrierConfirmation {
            carrier_contact: input.carrier_contact,
            reference: input.confirmation_reference,
            note: input.note,
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
        };
        if operation == "tender.incident.resolve" {
            let index = self
                .tender_incidents
                .iter()
                .position(|v| v.id == entity)
                .ok_or("Tender incident not found")?;
            let incident = self.tender_incidents[index].clone();
            if incident.status != "open" {
                return Err("Tender incident already resolved".into());
            }
            let load = self
                .loads
                .iter_mut()
                .find(|v| v.id == incident.load_id)
                .ok_or("Load not found")?;
            check_version(load, input.expected_load_version)?;
            bump(load)?;
            self.tender_incidents[index].status = "resolved".into();
            self.tender_incidents[index].resolution = Some(confirmation);
            for item in &mut self.inbox {
                if item.status == "reconciliation_required"
                    && item
                        .normalized_event
                        .as_ref()
                        .is_some_and(|event| event["event_id"] == incident.event.event_id)
                {
                    item.status = "reconciled".into();
                    item.updated_at = Utc::now();
                }
            }
            self.record(&incident.load_id,operation,"Carrier conflict resolved through recorded external coordination; assignment retained");
            return Ok(json!(self.tender_incidents[index]));
        }
        if !["tender.release.confirm", "tender.release.retain"].contains(&operation) {
            return Err("Unsupported tender operation".into());
        }
        let release_index = self
            .releases
            .iter()
            .position(|v| v.id == entity)
            .ok_or("Carrier release not found")?;
        let release = self.releases[release_index].clone();
        if release.status != "pending" {
            return Err("Carrier release is no longer pending".into());
        }
        let index = self
            .loads
            .iter()
            .position(|v| v.id == release.load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        check_version(&load, input.expected_load_version)?;
        if load.tender_id.as_deref() != Some(&release.tender_id)
            || !matches!(
                load.status,
                Status::Tendered | Status::Accepted | Status::Dispatched
            )
        {
            return Err(
                "Carrier assignment or movement changed; reconcile before recording release".into(),
            );
        }
        if operation == "tender.release.confirm" {
            if self.has_approved_charges(&load.id) {
                return Err("Reconcile approved charges before releasing the carrier".into());
            }
            if self.unresolved_tender_incidents(&load.id) {
                return Err("Resolve carrier incidents before confirming release".into());
            }
            if self.invoices.iter().any(|v| v.load_id == load.id)
                || self
                    .bills
                    .iter()
                    .any(|v| v.load_id == load.id && v.status != "void")
            {
                return Err("Reconcile financial documents before releasing the carrier".into());
            }
            for job in &mut self.outbox {
                if job.tender_id.as_deref() == Some(&release.tender_id)
                    && job.status == "sending"
                    && job.leased_until.is_some_and(|v| v <= Utc::now())
                {
                    job.status = "held".into();
                    job.lease = None;
                    job.leased_until = None;
                    job.last_error=Some("Expired handoff lease fenced during carrier-confirmed release; prior transmission remains uncertain".into());
                }
            }
            if self.outbox.iter().any(|v| {
                v.tender_id.as_deref() == Some(&release.tender_id) && v.status == "sending"
            }) {
                return Err("Tender handoff is still sending; wait for its result or lease recovery before confirming release".into());
            }
            self.finish_tender_state(&load, "released");
            self.suppress_tender_jobs(Some(&release.tender_id));
            clear_assignment(&mut load, release.disposition == "cancel");
            self.releases[release_index].status = "confirmed".into();
        } else {
            self.releases[release_index].status = "retained".into();
        }
        bump(&mut load)?;
        self.loads[index] = load.clone();
        self.releases[release_index].confirmation = Some(confirmation);
        self.enqueue_sync(&load);
        self.record(
            &load.id,
            operation,
            format!(
                "Carrier release {} with contact and confirmation evidence",
                self.releases[release_index].status
            ),
        );
        Ok(json!(self.releases[release_index]))
    }
}
fn clear_assignment(load: &mut Load, cancel: bool) {
    load.status = if cancel {
        Status::Cancelled
    } else {
        Status::Planned
    };
    load.carrier_id = None;
    load.tender_id = None;
    load.driver_id = None;
    load.equipment_id = None;
    load.tender_expires_at = None;
    load.last_tracking_at = None;
    load.location = if cancel {
        "Cancelled before pickup"
    } else {
        "Awaiting pickup"
    }
    .into();
    if load.exception.as_deref()
        == Some("Tender response overdue; contact carrier before retendering")
    {
        load.exception = None;
    }
}
fn bump(load: &mut Load) -> Result<()> {
    load.version = load
        .version
        .checked_add(1)
        .ok_or("Load version exhausted")?;
    load.updated_at = Utc::now();
    Ok(())
}
fn check_version(load: &Load, expected: u64) -> Result<()> {
    if load.version != expected {
        Err("Load changed; refresh before continuing".into())
    } else {
        Ok(())
    }
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid carrier-release request fields".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        integrations::{Connection, Integrations},
        store::Store,
    };
    use std::{collections::BTreeMap, sync::Arc};
    #[tokio::test]
    async fn release_waits_for_live_handoff_and_fences_expired_worker_completion() {
        let store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        store
            .mutate(
                "demo",
                "tender",
                "load.command",
                "load_5",
                json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}),
            )
            .await
            .expect("tender");
        let entered = Arc::new(tokio::sync::Notify::new());
        let finish = Arc::new(tokio::sync::Notify::new());
        let (signal, unblock) = (entered.clone(), finish.clone());
        let server = axum::Router::new().route(
            "/v1/edi/outbound/204/pclg",
            axum::routing::post(move || {
                let (signal, unblock) = (signal.clone(), unblock.clone());
                async move {
                    signal.notify_one();
                    unblock.notified().await;
                    "ISA*LEASE-RESULT"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let address = listener.local_addr().expect("address");
        let server =
            tokio::spawn(async move { axum::serve(listener, server).await.expect("serve") });
        let integration = Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    edi_url: Some(format!("http://{address}")),
                    edi_token: Some("test".into()),
                    edi_tenant_id: Some("demo".into()),
                    ..Connection::default()
                },
            )]),
        };
        let (copy, connection) = (store.clone(), integration.clone());
        let worker = tokio::spawn(async move { connection.dispatch_one(&copy, "demo").await });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("live send");
        let release=store.mutate("demo","release","tender.release.request","load_5",json!({"expected_load_version":2,"disposition":"replan","reason":"Carrier release during uncertain handoff"})).await.expect("request");
        let id = release["data"]["id"].as_str().expect("id");
        let body = json!({"expected_load_version":3,"carrier_contact":"Carrier dispatcher","confirmation_reference":"Confirmed exact tender release","note":"Carrier confirmed no pickup commitment remains"});
        assert!(store
            .mutate(
                "demo",
                "confirm",
                "tender.release.confirm",
                id,
                body.clone()
            )
            .await
            .is_err());
        store
            .update("demo", |w| {
                w.outbox[0].leased_until = Some(Utc::now() - chrono::Duration::seconds(1));
                Ok(Value::Null)
            })
            .await
            .expect("simulate expired worker");
        store
            .mutate("demo", "confirm", "tender.release.confirm", id, body)
            .await
            .expect("confirmed release recovers expired lease without resending");
        store
            .mutate(
                "demo",
                "new-tender",
                "load.command",
                "load_5",
                json!({"action":"tender","expected_version":4,"carrier_id":"ca2"}),
            )
            .await
            .expect("replacement");
        finish.notify_one();
        worker
            .await
            .expect("worker task")
            .expect("old result fenced");
        let after = store.read("demo").await.expect("read");
        assert_eq!(after.outbox[0].status, "superseded");
        assert_eq!(after.outbox[1].status, "pending_handoff");
        assert_eq!(after.loads[4].status, Status::Tendered);
        assert!(after.outbox[0].response.is_none());
        assert_eq!(after.outbox[0].total_attempts, 1);
        server.abort();
    }
}
