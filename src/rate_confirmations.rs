//! Carrier-facing rate records, frozen on the exact tender attempt. Issuance,
//! external handoff evidence, carrier acceptance, and payment remain distinct.
use crate::{
    charges::ChargeBasis,
    domain::{self, Carrier, Load, Result, Status, Stop, Workspace},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateIssuer {
    pub legal_name: String,
    pub address: String,
    pub contact: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateWindow {
    pub stop: String,
    pub start_local: String,
    pub end_local: String,
    pub timezone: String,
    pub contact: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateSnapshot {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub issuer: RateIssuer,
    pub carrier: Carrier,
    pub load_reference: String,
    pub shipment_reference: String,
    pub order_number: Option<String>,
    pub origin: Stop,
    pub destination: Stop,
    pub equipment: String,
    pub commodity: String,
    pub weight_lbs: u32,
    pub currency: String,
    pub buy: ChargeBasis,
    pub windows: Vec<RateWindow>,
    pub terms: String,
    pub source_load_version: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateConfirmation {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub revision: u64,
    pub supersedes_id: Option<String>,
    pub snapshot: RateSnapshot,
    pub html: String,
    pub sha256: String,
    pub issued_at: DateTime<Utc>,
    pub issued_by: String,
    pub reason: String,
    pub version: u64,
    pub handoffs: Vec<RateHandoff>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateHandoff {
    pub id: String,
    pub sent_at: DateTime<Utc>,
    pub carrier_contact: String,
    pub channel: String,
    pub reference: String,
    pub note: String,
    pub document_id: Option<String>,
    pub document_sha256: Option<String>,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub retraction: Option<HandoffRetraction>,
    pub coordination_required: bool,
    pub resolution: Option<crate::tenders::CarrierConfirmation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffRetraction {
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    expected_load_version: u64,
    expected_previous_id: Option<String>,
    terms: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Handoff {
    expected_load_version: u64,
    expected_version: u64,
    sent_at: DateTime<Utc>,
    carrier_contact: String,
    channel: String,
    reference: String,
    note: String,
    document_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retract {
    expected_load_version: u64,
    expected_version: u64,
    handoff_id: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    expected_version: u64,
    expected_load_version: u64,
    handoff_id: String,
    carrier_contact: String,
    confirmation_reference: String,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|_| "Invalid rate confirmation fields".into())
}
fn version(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        return Err("Rate confirmation changed; refresh before continuing".into());
    }
    Ok(())
}
impl Workspace {
    pub fn unresolved_rate_handoffs(&self, load: &str) -> bool {
        self.rate_confirmations
            .iter()
            .filter(|v| v.load_id == load)
            .any(|v| {
                v.handoffs.iter().any(|h| {
                    h.coordination_required && h.retraction.is_none() && h.resolution.is_none()
                })
            })
    }
    fn bump_rate_load(&mut self, load: &str, expected: u64) -> Result<()> {
        let l = self
            .loads
            .iter_mut()
            .find(|v| v.id == load)
            .ok_or("Load not found")?;
        if l.version != expected {
            return Err("Load changed; refresh before continuing".into());
        }
        l.version = l.version.checked_add(1).ok_or("Load version exhausted")?;
        l.updated_at = Utc::now();
        Ok(())
    }
    pub fn rate_confirmation_current(&self, doc: &RateConfirmation) -> bool {
        self.loads.iter().any(|l| {
            l.id == doc.load_id
                && l.tender_id.as_deref() == Some(&doc.tender_id)
                && l.status != Status::Cancelled
        }) && !self
            .rate_confirmations
            .iter()
            .any(|v| v.supersedes_id.as_deref() == Some(&doc.id))
    }
    pub fn rate_confirmation_basis_changed(&self, doc: &RateConfirmation) -> bool {
        self.loads
            .iter()
            .find(|v| v.id == doc.load_id)
            .is_none_or(|load| {
                let Ok(mut current) = self.rate_snapshot(load, &doc.snapshot.terms) else {
                    return true;
                };
                current.source_load_version = doc.snapshot.source_load_version;
                json!(current) != json!(doc.snapshot)
            })
    }
    fn rate_snapshot(&self, load: &Load, terms: &str) -> Result<RateSnapshot> {
        let tender = self
            .tenders
            .iter()
            .find(|t| {
                Some(&t.id) == load.tender_id.as_ref()
                    && t.load_id == load.id
                    && Some(&t.carrier_id) == load.carrier_id.as_ref()
            })
            .ok_or("Current carrier tender required")?;
        let profile = self.billing_profile.as_ref().ok_or("Configure the issuing legal identity in Freight audit before issuing a rate confirmation")?;
        domain::required(&profile.legal_name, "Issuing legal name")?;
        domain::required(&profile.address, "Issuing address")?;
        domain::required(&profile.contact, "Issuing contact")?;
        Ok(RateSnapshot {
            intermediate_stops: load.intermediate_stops.clone(),
            issuer: RateIssuer {
                legal_name: profile.legal_name.clone(),
                address: profile.address.clone(),
                contact: profile.contact.clone(),
            },
            carrier: tender.carrier.clone(),
            load_reference: load.reference.clone(),
            shipment_reference: tender.shipment_reference.clone(),
            order_number: load.order_number.clone(),
            origin: load.origin.clone(),
            destination: load.destination.clone(),
            equipment: load.equipment.clone(),
            commodity: load.commodity.clone(),
            weight_lbs: load.weight_lbs,
            currency: load.currency.clone(),
            buy: self.charge_basis(load, "buy")?,
            windows: self
                .appointments
                .iter()
                .filter(|v| v.load_id == load.id && v.status == "active")
                .map(|v| RateWindow {
                    stop: v.stop.clone(),
                    start_local: v.start_local.clone(),
                    end_local: v.end_local.clone(),
                    timezone: v.timezone.clone(),
                    contact: v.contact.clone(),
                })
                .collect(),
            terms: terms.trim().into(),
            source_load_version: load.version,
        })
    }
    pub fn apply_rate_confirmation(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "rate.issue" {
            let input: Issue = decode(body)?;
            domain::required(&input.reason, "Issuance/revision reason")?;
            if input.terms.trim().is_empty()
                || input.terms.len() > 4000
                || input.terms.chars().any(|v| v.is_control() && v != '\n')
            {
                return Err(
                    "Carrier terms require 1–4000 characters without invalid controls".into(),
                );
            }
            let index = self
                .loads
                .iter()
                .position(|l| l.id == entity)
                .ok_or("Load not found")?;
            let mut load = self.loads[index].clone();
            if load.version != input.expected_load_version {
                return Err("Load changed; refresh before continuing".into());
            }
            if !matches!(
                load.status,
                Status::Tendered | Status::Accepted | Status::Dispatched
            ) {
                return Err(
                    "Issue rate confirmations against a current carrier tender before pickup"
                        .into(),
                );
            }
            self.guard_source(&load)?;
            if self.release_pending(&load) || self.unresolved_tender_incidents(&load.id) {
                return Err(
                    "Resolve carrier release and custody holds before issuing rate confirmation"
                        .into(),
                );
            }
            if self.pending_charges(&load.id, "buy") {
                return Err(
                    "Resolve proposed carrier charges before issuing rate confirmation".into(),
                );
            }
            crate::stops::validate_stop(&load.origin, true)?;
            crate::stops::validate_stop(&load.destination, true)?;
            self.adopt_tender(&mut load)?;
            let snapshot = self.rate_snapshot(&load, &input.terms)?;
            let tender = self
                .tenders
                .iter()
                .find(|t| Some(&t.id) == load.tender_id.as_ref())
                .ok_or("Current tender required")?;
            let prior = self
                .rate_confirmations
                .iter()
                .rev()
                .find(|v| v.tender_id == tender.id);
            if input.expected_previous_id.as_deref() != prior.map(|v| v.id.as_str()) {
                return Err(
                    "Rate confirmation changed; select its latest revision before continuing"
                        .into(),
                );
            }
            let now = Utc::now();
            let mut doc = RateConfirmation {
                id: domain::id("rate"),
                load_id: load.id.clone(),
                tender_id: tender.id.clone(),
                revision: prior.map_or(Ok(1), |v| {
                    v.revision.checked_add(1).ok_or("Rate revision exhausted")
                })?,
                supersedes_id: prior.map(|v| v.id.clone()),
                snapshot,
                html: String::new(),
                sha256: String::new(),
                issued_at: now,
                issued_by: actor.into(),
                reason: input.reason.trim().into(),
                version: 1,
                handoffs: vec![],
            };
            doc.html = render(&doc);
            doc.sha256 = hex::encode(Sha256::digest(doc.html.as_bytes()));
            self.loads[index] = load.clone();
            self.loads[index].version = load
                .version
                .checked_add(1)
                .ok_or("Load version exhausted")?;
            self.loads[index].updated_at = now;
            let result = json!(doc);
            self.rate_confirmations.push(doc);
            self.record(entity,operation,"Carrier rate confirmation issued; external delivery and acceptance remain separate");
            return Ok(result);
        }
        let index = self
            .rate_confirmations
            .iter()
            .position(|v| v.id == entity)
            .ok_or("Rate confirmation not found")?;
        if operation == "rate.handoff" {
            statement(self, entity)?;
            let input: Handoff = decode(body)?;
            let doc = &self.rate_confirmations[index];
            version(doc.version, input.expected_version)?;
            for (v, label) in [
                (&input.carrier_contact, "Actual carrier contact"),
                (&input.reference, "External handoff reference"),
                (&input.note, "Handoff findings"),
            ] {
                domain::required(v, label)?;
            }
            if !["email", "portal", "other"].contains(&input.channel.as_str()) {
                return Err("Handoff channel requires email, portal, or other".into());
            }
            if input.sent_at < doc.issued_at || input.sent_at > Utc::now() {
                return Err(
                    "Actual handoff time must follow issuance and cannot be in the future".into(),
                );
            }
            if doc.handoffs.iter().any(|v| {
                v.retraction.is_none()
                    && v.channel == input.channel
                    && v.reference
                        .trim()
                        .eq_ignore_ascii_case(input.reference.trim())
            }) {
                return Err("This handoff reference is already recorded".into());
            }
            let digest = if let Some(id) = &input.document_id {
                Some(
                    self.documents
                        .iter()
                        .find(|v| {
                            &v.id == id && v.load_id == doc.load_id && v.status() != "rejected"
                        })
                        .ok_or("Handoff evidence must belong to this shipment and not be rejected")?
                        .sha256
                        .clone(),
                )
            } else {
                None
            };
            let coordination_required = !self.rate_confirmation_current(doc)
                || self.rate_confirmation_basis_changed(doc)
                || self
                    .loads
                    .iter()
                    .find(|v| v.id == doc.load_id)
                    .is_some_and(|l| self.release_pending(l));
            let load = doc.load_id.clone();
            self.bump_rate_load(&load, input.expected_load_version)?;
            self.rate_confirmations[index].handoffs.push(RateHandoff {
                id: domain::id("handoff"),
                sent_at: input.sent_at,
                carrier_contact: input.carrier_contact.trim().into(),
                channel: input.channel,
                reference: input.reference.trim().into(),
                note: input.note.trim().into(),
                document_id: input.document_id,
                document_sha256: digest,
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
                retraction: None,
                coordination_required,
                resolution: None,
            });
            self.rate_confirmations[index].version = self.rate_confirmations[index]
                .version
                .checked_add(1)
                .ok_or("Rate version exhausted")?;
            self.record(
                &load,
                operation,
                "Recorded actual external rate-confirmation handoff; carrier acceptance unchanged",
            );
        } else if operation == "rate.handoff.retract" {
            let input: Retract = decode(body)?;
            domain::required(&input.note, "Handoff correction reason")?;
            let load_id = self.rate_confirmations[index].load_id.clone();
            self.bump_rate_load(&load_id, input.expected_load_version)?;
            let doc = &mut self.rate_confirmations[index];
            version(doc.version, input.expected_version)?;
            let h = doc
                .handoffs
                .iter_mut()
                .find(|v| v.id == input.handoff_id)
                .ok_or("Handoff not found")?;
            if h.retraction.is_some() {
                return Err("Handoff evidence already retracted".into());
            }
            h.retraction = Some(HandoffRetraction {
                note: input.note.trim().into(),
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
            });
            doc.version = doc.version.checked_add(1).ok_or("Rate version exhausted")?;
            let load = doc.load_id.clone();
            self.record(
                &load,
                operation,
                "Erroneous handoff evidence retracted; original evidence retained",
            );
        } else if operation == "rate.handoff.resolve" {
            let input: Resolve = decode(body)?;
            for (v, label) in [
                (&input.carrier_contact, "Actual carrier contact"),
                (
                    &input.confirmation_reference,
                    "Carrier coordination reference",
                ),
                (&input.note, "Coordination findings"),
            ] {
                domain::required(v, label)?;
            }
            let load = self.rate_confirmations[index].load_id.clone();
            self.bump_rate_load(&load, input.expected_load_version)?;
            let doc = &mut self.rate_confirmations[index];
            version(doc.version, input.expected_version)?;
            let h = doc
                .handoffs
                .iter_mut()
                .find(|v| v.id == input.handoff_id)
                .ok_or("Handoff not found")?;
            if !h.coordination_required || h.retraction.is_some() || h.resolution.is_some() {
                return Err("No unresolved carrier handoff coordination".into());
            }
            h.resolution = Some(crate::tenders::CarrierConfirmation {
                carrier_contact: input.carrier_contact.trim().into(),
                reference: input.confirmation_reference.trim().into(),
                note: input.note.trim().into(),
                recorded_at: Utc::now(),
                recorded_by: actor.into(),
            });
            doc.version = doc.version.checked_add(1).ok_or("Rate version exhausted")?;
            self.record(&load,operation,"Carrier rate handoff coordination resolved with actual external confirmation; ownership retained");
        } else {
            return Err("Unsupported rate confirmation operation".into());
        }
        Ok(json!(self.rate_confirmations[index]))
    }
    pub(crate) fn rate_documents<'a>(
        &'a self,
        action: &str,
        response: &Value,
    ) -> Result<Vec<&'a crate::documents::Document>> {
        let doc = self
            .rate_confirmations
            .iter()
            .find(|v| Some(v.id.as_str()) == response["data"]["id"].as_str())
            .ok_or("Internal storage error")?;
        let mut ids = Vec::new();
        if action == "rate.issue" {
            for charge in self
                .charges
                .iter()
                .filter(|v| v.load_id == doc.load_id && v.side == "buy" && v.status == "approved")
            {
                if let Some(id) = &charge.document_id {
                    ids.push((id, &charge.document_sha256));
                }
            }
        } else if action == "rate.handoff" {
            let h = doc.handoffs.last().ok_or("Internal storage error")?;
            if let Some(id) = &h.document_id {
                ids.push((id, &h.document_sha256));
            }
        }
        ids.into_iter()
            .map(|(id, hash)| {
                let d = self
                    .documents
                    .iter()
                    .find(|v| &v.id == id)
                    .ok_or("Internal storage error")?;
                if hash.as_ref() != Some(&d.sha256) {
                    return Err("Internal storage error".into());
                }
                Ok(d)
            })
            .collect()
    }
}
pub fn statement(workspace: &Workspace, id: &str) -> Result<(String, String, String)> {
    let doc = workspace
        .rate_confirmations
        .iter()
        .find(|v| v.id == id)
        .ok_or("Rate confirmation not found")?;
    if hex::encode(Sha256::digest(doc.html.as_bytes())) != doc.sha256 {
        return Err("Internal storage error".into());
    }
    Ok((
        format!("rate-confirmation-{}-r{}", doc.id, doc.revision),
        doc.html.clone(),
        doc.sha256.clone(),
    ))
}
fn esc(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn usd(v: i64) -> String {
    format!("{}.{:02}", v / 100, v % 100)
}
fn money(v: i64, currency: &str) -> String {
    stateset_freight_commerce::parse_currency(currency).map_or_else(
        |_| usd(v),
        |code| stateset_freight_commerce::format_money(v, code),
    )
}
fn stop_html(s: &Stop, windows: &[RateWindow], kind: &str, title: &str) -> String {
    let windows = windows
        .iter()
        .filter(|v| v.stop == kind)
        .map(|v| {
            format!(
                "<p>Appointment: {} – {} ({})<br>Facility contact: {}</p>",
                esc(&v.start_local),
                esc(&v.end_local),
                esc(&v.timezone),
                esc(&v.contact)
            )
        })
        .collect::<String>();
    format!("<h2>{}</h2><p>{}<br>{}<br>{}, {} {} {}<br>Planned date: {}</p><p>Contact: {} · {}</p><p class=lines>{}</p>{windows}",esc(title),esc(&s.facility),s.details.address_lines.iter().map(|v|esc(v)).collect::<Vec<_>>().join("<br>"),esc(&s.city),esc(&s.state),esc(&s.details.postal_code),esc(&s.details.country),s.date,esc(&s.details.contact),esc(&s.details.phone),esc(&s.details.instructions))
}
fn render(doc: &RateConfirmation) -> String {
    let s = &doc.snapshot;
    let charges = s
        .buy
        .charges
        .iter()
        .map(|c| {
            format!(
                "<tr><td>{}</td><td>{} {}</td></tr>",
                esc(&c.description),
                esc(&s.currency),
                money(c.amount_cents, &s.currency)
            )
        })
        .collect::<String>();
    let mut route = stop_html(&s.origin, &s.windows, "pickup", "Pickup");
    if !s.intermediate_stops.is_empty() {
        route.push_str(
            "<p>Confirm actual service at every intermediate stop in this required order.</p>",
        );
        for (i, stop) in s.intermediate_stops.iter().enumerate() {
            route.push_str(&stop_html(
                &stop.stop,
                &s.windows,
                &stop.id,
                &format!("Stop {} · {}", i + 2, stop.kind),
            ));
        }
    }
    format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Carrier rate confirmation {}</title><style>body{{font:15px system-ui,sans-serif;color:#18332d;max-width:820px;margin:32px auto;padding:24px;overflow-wrap:anywhere}}h1{{font-size:26px}}table{{width:100%;border-collapse:collapse}}td,th{{text-align:left;padding:12px;border-bottom:1px solid #dbe6e0}}td:last-child{{text-align:right}}.lines{{white-space:pre-wrap}}.muted{{color:#526860}}@media print{{body{{margin:0}}}}</style><body><p>STATESET FREIGHT · CARRIER RATE CONFIRMATION · revision {}</p><h1>{}</h1><p>Document: {}<br>Issued: {}<br>Carrier shipment ID: {}<br>Order: {}</p><h2>Issued by</h2><p class="lines">{}<br>{}<br>{}</p><h2>Carrier</h2><p>{} · SCAC {} · MC {}</p><p>Equipment: {} · Commodity: {} · Weight: {} lb</p>{}{}<h2>Carrier compensation</h2><table><tr><td>Base freight</td><td>{} {}</td></tr>{}<tr><th>Total inclusive {}</th><td>{} {}</td></tr></table><h2>Carrier terms</h2><p class="lines">{}</p><p class="muted">This is the rate record issued for this tender attempt. Verify its current revision before performing transport. Issuance does not send this document, establish carrier acceptance, confirm pickup/delivery, or initiate payment. External handoff and carrier acceptance are recorded separately.</p></body></html>"#,
        esc(&s.load_reference),
        doc.revision,
        esc(&s.load_reference),
        esc(&doc.id),
        doc.issued_at.to_rfc3339(),
        esc(&s.shipment_reference),
        esc(s.order_number.as_deref().unwrap_or("Not supplied")),
        esc(&s.issuer.legal_name),
        esc(&s.issuer.address),
        esc(&s.issuer.contact),
        esc(&s.carrier.name),
        esc(&s.carrier.scac),
        esc(&s.carrier.mc_number),
        esc(&s.equipment),
        esc(&s.commodity),
        s.weight_lbs,
        route,
        stop_html(&s.destination, &s.windows, "delivery", "Delivery"),
        esc(&s.currency),
        money(s.buy.base_amount_cents, &s.currency),
        charges,
        esc(&s.currency),
        esc(&s.currency),
        money(s.buy.total_amount_cents, &s.currency),
        esc(&s.terms)
    )
}
