//! Partner access uses explicit projections and server-owned counterparty grants.
//! Publishing is an actual communication channel, separate from EDI transport.
use crate::{
    auth::{Principal, Role},
    domain::{self, Load, Result, Status, Stop, Workspace},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalOffer {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub rate_confirmation_id: String,
    pub rate_sha256: String,
    pub supersedes_id: Option<String>,
    pub published_at: DateTime<Utc>,
    pub published_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PortalDecision {
    pub id: String,
    pub offer_id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub decision: String,
    pub reference: String,
    pub note: String,
    pub rate_confirmation_id: String,
    pub rate_sha256: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    expected_load_version: u64,
    rate_confirmation_id: String,
    expected_previous_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseInput {
    pub expected_load_version: u64,
    pub decision: String,
    pub reference: String,
    pub note: String,
    pub rate_confirmation_id: String,
    pub rate_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedResponse {
    carrier_id: String,
    subject: String,
    request: ResponseInput,
}
#[derive(Serialize)]
struct PartnerStop {
    facility: String,
    city: String,
    state: String,
    date: NaiveDate,
    address_lines: Vec<String>,
    postal_code: String,
    country: String,
    contact: String,
    phone: String,
    instructions: String,
}
impl From<&Stop> for PartnerStop {
    fn from(s: &Stop) -> Self {
        Self {
            facility: s.facility.clone(),
            city: s.city.clone(),
            state: s.state.clone(),
            date: s.date,
            address_lines: s.details.address_lines.clone(),
            postal_code: s.details.postal_code.clone(),
            country: s.details.country.clone(),
            contact: s.details.contact.clone(),
            phone: s.details.phone.clone(),
            instructions: s.details.instructions.clone(),
        }
    }
}
#[derive(Serialize)]
struct PartnerWaypoint {
    id: String,
    kind: String,
    stop: PartnerStop,
}
impl From<&crate::itinerary::IntermediateStop> for PartnerWaypoint {
    fn from(s: &crate::itinerary::IntermediateStop) -> Self {
        Self {
            id: s.id.clone(),
            kind: s.kind.clone(),
            stop: (&s.stop).into(),
        }
    }
}
#[derive(Serialize)]
struct PartnerPlan {
    intermediate_stops: Vec<PartnerWaypoint>,
    reference: String,
    equipment: String,
    commodity: String,
    weight_lbs: u32,
    origin: PartnerStop,
    destination: PartnerStop,
}
impl From<&Load> for PartnerPlan {
    fn from(l: &Load) -> Self {
        Self {
            intermediate_stops: l
                .intermediate_stops
                .iter()
                .map(PartnerWaypoint::from)
                .collect(),
            reference: l.reference.clone(),
            equipment: l.equipment.clone(),
            commodity: l.commodity.clone(),
            weight_lbs: l.weight_lbs,
            origin: (&l.origin).into(),
            destination: (&l.destination).into(),
        }
    }
}
#[derive(Serialize)]
struct PartnerWindow {
    stop: String,
    start_local: String,
    end_local: String,
    timezone: String,
}
#[derive(Serialize)]
struct CarrierOfferView {
    id: String,
    load_id: String,
    tender_id: String,
    shipment_reference: String,
    plan: PartnerPlan,
    rate_confirmation_id: String,
    rate_sha256: String,
    rate_revision: u64,
    amount_cents: i64,
    currency: String,
    terms: String,
    windows: Vec<PartnerWindow>,
    published_at: DateTime<Utc>,
    is_current: bool,
    status: Value,
    load_version: Option<u64>,
    can_accept: bool,
    can_decline: bool,
    awaiting_updated_confirmation: bool,
    location: Option<String>,
    last_tracking_at: Option<DateTime<Utc>>,
    decision: Option<Value>,
    reporting: Value,
}
impl Workspace {
    fn portal_carrier_qualified(&self, load: &Load) -> bool {
        self.carriers
            .iter()
            .find(|c| Some(&c.id) == load.carrier_id.as_ref())
            .is_some_and(|c| {
                c.equipment == load.equipment
                    && c.insurance_expires >= self.coverage_date(load).max(Utc::now().date_naive())
            })
    }
    pub fn portal_offer_for(&self, id: &str, carrier: &str) -> Result<&PortalOffer> {
        self.portal_offers
            .iter()
            .find(|v| v.id == id && v.carrier_id == carrier)
            .ok_or("Portal offer not found".into())
    }
    pub fn portal_published(&self, tender: &str) -> bool {
        self.portal_offers.iter().any(|v| v.tender_id == tender)
    }
    fn latest_portal_offer(&self, tender: &str) -> Option<&PortalOffer> {
        self.portal_offers
            .iter()
            .rev()
            .find(|v| v.tender_id == tender)
    }
    pub(crate) fn portal_current(&self, offer: &PortalOffer) -> bool {
        self.loads.iter().any(|l| {
            l.id == offer.load_id
                && l.tender_id.as_deref() == Some(&offer.tender_id)
                && l.carrier_id.as_deref() == Some(&offer.carrier_id)
        }) && self
            .latest_portal_offer(&offer.tender_id)
            .is_some_and(|v| v.id == offer.id)
    }
    pub fn apply_portal(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "portal.itinerary.service" {
            return self.apply_itinerary_portal(entity, body, actor);
        }
        if operation == "portal.tracking" {
            return self.apply_carrier_tracking(entity, body, actor);
        }
        if operation.starts_with("portal.quote.") {
            return self.apply_customer_quote(operation, entity, body, actor);
        }
        if operation.starts_with("portal.release.") {
            return self.apply_release_portal(operation, entity, body, actor);
        }
        if operation == "portal.publish" {
            let input: Publish =
                serde_json::from_value(body).map_err(|_| "Invalid portal publication fields")?;
            let index = self
                .loads
                .iter()
                .position(|l| l.id == entity)
                .ok_or("Load not found")?;
            let load = self.loads[index].clone();
            if load.version != input.expected_load_version {
                return Err("Load changed; refresh before continuing".into());
            }
            if !matches!(
                load.status,
                Status::Tendered | Status::Accepted | Status::Dispatched
            ) {
                return Err(
                    "Publish a carrier offer against its current tender before pickup".into(),
                );
            }
            self.guard_source(&load)?;
            if !self.portal_carrier_qualified(&load) {
                return Err(
                    "Carrier qualification changed; contact operations before publication".into(),
                );
            }
            if self.release_pending(&load)
                || self.unresolved_tender_incidents(entity)
                || self.pending_charges(entity, "buy")
            {
                return Err("Resolve carrier release, custody, and proposed charges before portal publication".into());
            }
            let doc = self
                .rate_confirmations
                .iter()
                .find(|d| {
                    d.id == input.rate_confirmation_id
                        && d.load_id == entity
                        && Some(&d.tender_id) == load.tender_id.as_ref()
                })
                .ok_or("Current carrier rate confirmation not found")?;
            if !self.rate_confirmation_current(doc) || self.rate_confirmation_basis_changed(doc) {
                return Err(
                    "Carrier rate confirmation changed; issue its current basis before publication"
                        .into(),
                );
            }
            crate::rate_confirmations::statement(self, &doc.id)?;
            let prior = self.latest_portal_offer(&doc.tender_id);
            if prior.map(|v| v.id.as_str()) != input.expected_previous_id.as_deref() {
                return Err("Portal offer changed; refresh its preceding publication".into());
            }
            if self
                .portal_offers
                .iter()
                .any(|v| v.rate_confirmation_id == doc.id)
            {
                return Err("This rate confirmation is already published".into());
            }
            let offer = PortalOffer {
                id: domain::id("offer"),
                load_id: entity.into(),
                tender_id: doc.tender_id.clone(),
                carrier_id: doc.snapshot.carrier.id.clone(),
                rate_confirmation_id: doc.id.clone(),
                rate_sha256: doc.sha256.clone(),
                supersedes_id: prior.map(|v| v.id.clone()),
                published_at: Utc::now(),
                published_by: actor.into(),
            };
            let result = json!(offer);
            self.portal_offers.push(offer);
            self.bump_portal_load(entity)?;
            self.record(entity,operation,"Carrier offer published to scoped partner portal; no unsent withdrawal or automatic acceptance");
            Ok(result)
        } else if operation == "portal.respond" {
            let input: TrustedResponse =
                serde_json::from_value(body).map_err(|_| "Invalid partner response fields")?;
            if input.subject != actor {
                return Err("Partner identity does not match authenticated subject".into());
            }
            let offer = self.portal_offer_for(entity, &input.carrier_id)?.clone();
            if !self.portal_current(&offer) {
                return Err(
                    "Portal offer changed; tender or publication retired, refresh before responding".into(),
                );
            }
            let l = self
                .loads
                .iter()
                .find(|l| l.id == offer.load_id)
                .ok_or("Load not found")?;
            if l.version != input.request.expected_load_version {
                return Err("Load changed; refresh before responding".into());
            }
            if l.status != Status::Tendered {
                return Err("Carrier responses require an outstanding tender; use operations coordination for an accepted assignment".into());
            }
            if !["accept", "decline"].contains(&input.request.decision.as_str()) {
                return Err("Carrier decision requires accept or decline".into());
            }
            domain::required(&input.request.reference, "Carrier response reference")?;
            domain::required(&input.request.note, "Carrier response findings")?;
            if input.request.rate_confirmation_id != offer.rate_confirmation_id
                || input.request.rate_sha256 != offer.rate_sha256
            {
                return Err("Published rate confirmation changed; inspect its exact revision before responding".into());
            }
            let doc = self
                .rate_confirmations
                .iter()
                .find(|d| d.id == offer.rate_confirmation_id)
                .ok_or("Rate confirmation not found")?;
            crate::rate_confirmations::statement(self, &doc.id)?;
            if input.request.decision == "accept" {
                self.guard_source(l)?;
                if !self.portal_carrier_qualified(l) {
                    return Err(
                        "Carrier qualification changed; contact operations before accepting".into(),
                    );
                }
                if !self.rate_confirmation_current(doc)
                    || self.rate_confirmation_basis_changed(doc)
                    || self.release_pending(l)
                    || self.unresolved_tender_incidents(&l.id)
                    || self.pending_charges(&l.id, "buy")
                {
                    return Err("Carrier terms or coordination changed; inspect a current published confirmation before accepting".into());
                }
            }
            self.apply("load.command",&offer.load_id,json!({"action":input.request.decision,"expected_version":input.request.expected_load_version}))?;
            let decision = PortalDecision {
                id: domain::id("partner-decision"),
                offer_id: offer.id,
                load_id: offer.load_id.clone(),
                tender_id: offer.tender_id,
                carrier_id: offer.carrier_id,
                decision: input.request.decision,
                reference: input.request.reference.trim().into(),
                note: input.request.note.trim().into(),
                rate_confirmation_id: offer.rate_confirmation_id,
                rate_sha256: offer.rate_sha256,
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
            };
            self.portal_decisions.push(decision.clone());
            self.record(&offer.load_id,operation,"Authenticated carrier portal decision recorded against exact published tender/rate revision");
            Ok(json!(decision))
        } else {
            Err("Unsupported portal operation".into())
        }
    }
    fn bump_portal_load(&mut self, id: &str) -> Result<()> {
        let l = self
            .loads
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or("Load not found")?;
        l.version = l.version.checked_add(1).ok_or("Load version exhausted")?;
        l.updated_at = Utc::now();
        Ok(())
    }
    pub fn partner_workspace(&self, p: &Principal) -> Result<Value> {
        let cp = p
            .counterparty_id
            .as_deref()
            .filter(|_| p.valid_scope())
            .ok_or("Partner scope is required")?;
        let access =
            json!({"role":p.role,"subject":p.subject,"capabilities":p.role.capabilities()});
        match p.role {
            Role::Carrier => {
                let carrier = self
                    .carriers
                    .iter()
                    .find(|c| c.id == cp)
                    .ok_or("Partner carrier not found")?;
                let offers=self.portal_offers.iter().filter(|o|o.carrier_id==cp).map(|o| {
                    let d=self.rate_confirmations.iter().find(|d|d.id==o.rate_confirmation_id&&d.snapshot.carrier.id==cp).ok_or("Internal storage error")?;
                    let is_current=self.portal_current(o);let l=self.loads.iter().find(|l|l.id==o.load_id).filter(|_|is_current);
                    let plan=PartnerPlan {intermediate_stops:d.snapshot.intermediate_stops.iter().map(PartnerWaypoint::from).collect(),reference:d.snapshot.load_reference.clone(),equipment:d.snapshot.equipment.clone(),commodity:d.snapshot.commodity.clone(),weight_lbs:d.snapshot.weight_lbs,origin:(&d.snapshot.origin).into(),destination:(&d.snapshot.destination).into()};
                    let changed=!self.rate_confirmation_current(d)||self.rate_confirmation_basis_changed(d);
                    let can_decline=l.is_some_and(|l|l.status==Status::Tendered);
                    let can_accept=can_decline&&!changed&&l.is_some_and(|l|self.portal_carrier_qualified(l)&&self.guard_source(l).as_ref().err().is_none_or(|v|v!=crate::source::SOURCE_HOLD)&&!self.release_pending(l)&&!self.unresolved_tender_incidents(&l.id)&&!self.pending_charges(&l.id,"buy"));
                    Ok(CarrierOfferView {id:o.id.clone(),load_id:o.load_id.clone(),tender_id:o.tender_id.clone(),shipment_reference:d.snapshot.shipment_reference.clone(),plan,rate_confirmation_id:d.id.clone(),rate_sha256:d.sha256.clone(),rate_revision:d.revision,amount_cents:d.snapshot.buy.total_amount_cents,currency:d.snapshot.currency.clone(),terms:d.snapshot.terms.clone(),windows:d.snapshot.windows.iter().map(|w|PartnerWindow{stop:w.stop.clone(),start_local:w.start_local.clone(),end_local:w.end_local.clone(),timezone:w.timezone.clone()}).collect(),published_at:o.published_at,is_current,status:l.map_or_else(||json!("historical"),|l|json!(l.status)),load_version:l.map(|l|l.version),can_accept,can_decline,awaiting_updated_confirmation:changed,location:l.map(|l|l.location.clone()),last_tracking_at:l.and_then(|l|l.last_tracking_at),reporting:self.carrier_reporting(o,is_current),decision:self.portal_decisions.iter().find(|d|d.offer_id==o.id).map(|d|json!({"decision":d.decision,"reference":d.reference,"note":d.note,"recorded_at":d.recorded_at,"recorded_by":d.recorded_by}))})
                }).collect::<Result<Vec<_>>>()?;
                Ok(json!({"kind":"carrier","name":carrier.name,"access":access,"offers":offers}))
            }
            Role::Customer => {
                let customer = self
                    .customers
                    .iter()
                    .find(|c| c.id == cp)
                    .ok_or("Partner customer not found")?;
                let shipments=self.loads.iter().filter(|l|l.customer_id==cp).map(|l|json!({"id":l.id,"plan":PartnerPlan::from(l),"execution_timeline":self.execution_timeline(l,false),"status":l.status,"location":l.location,"last_tracking_at":l.last_tracking_at,"windows":self.appointments.iter().filter(|a|a.load_id==l.id&&a.status=="active").map(|a|PartnerWindow{stop:a.stop.clone(),start_local:a.start_local.clone(),end_local:a.end_local.clone(),timezone:a.timezone.clone()}).collect::<Vec<_>>()})).collect::<Vec<_>>();
                let bills=self.bills.iter().filter(|b|b.customer_id==cp&&b.issued_at.is_some()).map(|b| {let t=self.bill_totals(b)?;Ok(json!({"id":b.id,"load_id":b.load_id,"load_reference":b.snapshot.load_reference,"number":b.number,"status":b.status,"amount_cents":b.snapshot.amount_cents,"currency":b.snapshot.currency,"due_date":b.due_date,"balance_cents":t.balance_cents,"refunded_cents":t.refunded_cents,"refund_due_cents":t.refund_due_cents}))}).collect::<Result<Vec<_>>>()?;
                Ok(
                    json!({"kind":"customer","name":customer.name,"access":access,"shipments":shipments,"bills":bills,"quotes":self.customer_quote_views(cp)?}),
                )
            }
            _ => Err("Partner scope is required".into()),
        }
    }
}
