//! Explicit customer publication and authenticated decisions over frozen terms.
use crate::{
    domain::{self, Result, Workspace},
    quotes::{Quote, QuotePlan},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishedIssuer {
    pub name: String,
    pub address: String,
    pub contact: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishedQuote {
    pub reference: String,
    pub revision: u64,
    pub plan: QuotePlan,
    pub customer_name: String,
    pub customer_contact: String,
    pub issuer: PublishedIssuer,
    pub amount_cents: i64,
    pub currency: String,
    pub valid_until: DateTime<Utc>,
    pub terms: String,
    pub amendment_of: Option<String>,
    pub shipment_reference: Option<String>,
    pub order_number: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuotePublication {
    pub id: String,
    pub quote_id: String,
    pub customer_id: String,
    pub revision: u64,
    pub snapshot: PublishedQuote,
    pub basis_sha256: String,
    pub html: String,
    pub sha256: String,
    pub supersedes_id: Option<String>,
    pub published_at: DateTime<Utc>,
    pub published_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomerDecision {
    pub id: String,
    pub publication_id: String,
    pub quote_id: String,
    pub customer_id: String,
    pub revision: u64,
    pub document_sha256: String,
    pub decision: String,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub source: Option<Value>,
    pub authorize_transport: bool,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    expected_version: u64,
    expected_previous_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomerResponse {
    pub expected_version: u64,
    pub revision: u64,
    pub document_sha256: String,
    pub decision: String,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub source_digest: Option<String>,
    pub authorize_transport: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedResponse {
    customer_id: String,
    subject: String,
    request: CustomerResponse,
}
fn digest(value: &impl Serialize) -> Result<String> {
    Ok(crate::documents::sha256(
        &serde_json::to_vec(value).map_err(|_| "Internal storage error")?,
    ))
}
fn snapshot(q: &Quote) -> Result<PublishedQuote> {
    let r = q.current()?;
    let issuer = r
        .issuer
        .as_ref()
        .ok_or("Issued customer quote requires an issuer")?;
    Ok(PublishedQuote {
        reference: q.reference.clone(),
        revision: r.revision,
        plan: r.plan.clone(),
        customer_name: r.customer.name.clone(),
        customer_contact: r.customer.contact.clone(),
        issuer: PublishedIssuer {
            name: issuer.legal_name.clone(),
            address: issuer.address.clone(),
            contact: issuer.contact.clone(),
        },
        amount_cents: r.sell_cents,
        currency: r.plan.currency.clone(),
        valid_until: r.valid_until,
        terms: r.terms.clone(),
        amendment_of: r.amendment_of.as_ref().map(|t| t.quote_reference.clone()),
        shipment_reference: r.source_load.as_ref().map(|l| l.reference.clone()),
        order_number: r.source_load.as_ref().and_then(|l| l.order_number.clone()),
    })
}
impl QuotePublication {
    pub fn verify(&self) -> Result<()> {
        if crate::documents::sha256(self.html.as_bytes()) != self.sha256
            || digest(&self.snapshot)? != self.basis_sha256
            || self.snapshot.revision != self.revision
            || self.snapshot.plan.customer_id != self.customer_id
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
}
impl Workspace {
    pub fn commitment_quote(&self, operation: &str, entity: &str) -> Option<&Quote> {
        let id = if operation == "portal.quote.respond" {
            self.customer_quote_publications
                .iter()
                .find(|p| p.id == entity)?
                .quote_id
                .as_str()
        } else {
            entity
        };
        self.quotes.iter().find(|q| q.id == id)
    }
    pub fn customer_publication_for(&self, id: &str, customer: &str) -> Result<&QuotePublication> {
        self.customer_quote_publications
            .iter()
            .find(|p| p.id == id && p.customer_id == customer)
            .ok_or("Customer quote publication not found".into())
    }
    pub fn customer_publication_current(&self, p: &QuotePublication) -> bool {
        self.quotes
            .iter()
            .find(|q| q.id == p.quote_id)
            .and_then(|q| q.current().ok())
            .is_some_and(|r| r.revision == p.revision && r.plan.customer_id == p.customer_id)
            && self
                .customer_quote_publications
                .iter()
                .rev()
                .find(|v| v.quote_id == p.quote_id)
                .is_some_and(|v| v.id == p.id)
    }
    fn customer_quote_ready(&self, q: &Quote) -> Result<()> {
        let r = q.current()?;
        if q.status != "issued"
            || q.booked_load_id.is_some()
            || q.superseded_by.is_some()
            || r.issued_at.is_none()
        {
            return Err("Customer quote changed; inspect its current issued revision".into());
        }
        if r.valid_until <= Utc::now() {
            return Err("Quote expired; request current transportation terms".into());
        }
        let offer = r
            .offers
            .iter()
            .find(|o| Some(&o.id) == r.selected_offer_id.as_ref())
            .ok_or("Selected offer not found")?;
        self.quoted_carrier(r, offer)?;
        if offer.valid_until < r.valid_until || offer.valid_until <= Utc::now() {
            return Err("Transportation offer expired; request current terms".into());
        }
        if let Some(source) = &r.source_load {
            let load = self
                .loads
                .iter()
                .find(|l| l.id == source.id)
                .ok_or("Source shipment not found")?;
            if let Some(target) = &r.amendment_of {
                self.amendment_basis(
                    &target.quote_id,
                    target.quote_version,
                    load.version,
                    &r.plan,
                )?;
                if load.sync_order_id.is_some() {
                    let check = self
                        .source_check(&load.id)
                        .ok_or(crate::source::SOURCE_UNAVAILABLE)?;
                    if check.last_error.is_some()
                        || check.valid_until.is_none_or(|t| t <= Utc::now())
                    {
                        return Err(crate::source::SOURCE_UNAVAILABLE.into());
                    }
                }
            } else {
                self.quote_source(&source.id, Some(source.version))?;
                if json!(QuotePlan::from_load(load)) != json!(r.plan) {
                    return Err("Source shipment changed; request an updated quote".into());
                }
                self.guard_source(load)?;
            }
        }
        Ok(())
    }
    fn customer_source_context(&self, p: &QuotePublication) -> Option<Value> {
        if !self.customer_publication_current(p) {
            return None;
        }
        let q = self.quotes.iter().find(|q| q.id == p.quote_id)?;
        let source = q.current().ok()?.source_load.as_ref()?;
        let load = self.loads.iter().find(|l| {
            l.id == source.id && l.customer_id == p.customer_id && l.sync_order_id.is_some()
        })?;
        let check = self.source_check(&load.id);
        // No raw commerce JSON, metadata, line pricing, internal tags or errors.
        Some(
            json!({"order_id":load.sync_order_id,"order_number":load.order_number,
            "status":check.map(|c|crate::source::status(&c.snapshot)),"digest":check.map(|c|&c.digest),"checked_at":check.and_then(|c|c.checked_at),
            "available":check.is_some_and(|c|c.last_error.is_none()&&c.valid_until.is_some_and(|t|t>Utc::now())),
            "requires_authorization":q.current().ok()?.amendment_of.is_some() && check.is_some_and(|c|crate::source::needs_explicit_authorization(&c.snapshot))}),
        )
    }
    pub fn customer_quote_view(&self, p: &QuotePublication) -> Result<Value> {
        p.verify()?;
        let current = self.customer_publication_current(p);
        let q = self
            .quotes
            .iter()
            .find(|q| q.id == p.quote_id)
            .ok_or("Internal storage error")?;
        let basis_matches = current && digest(&snapshot(q)?)? == p.basis_sha256;
        let issued = current
            && q.status == "issued"
            && q.booked_load_id.is_none()
            && q.superseded_by.is_none();
        let source = self.customer_source_context(p);
        Ok(
            json!({"id":p.id,"quote_id":p.quote_id,"snapshot":p.snapshot,"document_sha256":p.sha256,"published_at":p.published_at,
            "is_current":current,"status":if current {q.effective_status()} else {"historical"},"version":if current {Some(q.version)} else {None},
            "can_accept":current&&basis_matches&&self.customer_quote_ready(q).is_ok(),"can_decline":issued,"can_withdraw":current&&q.booked_load_id.is_none()&&["issued","accepted","declined"].contains(&q.status.as_str()),
            "source":source,"can_refresh_source":issued&&source.is_some(),
            "decisions":self.customer_quote_decisions.iter().filter(|d|d.publication_id==p.id&&d.customer_id==p.customer_id).map(|d|json!({"decision":d.decision,"contact":d.contact,"reference":d.reference,"note":d.note,"source":d.source,"authorize_transport":d.authorize_transport,"recorded_at":d.recorded_at,"recorded_by":d.recorded_by})).collect::<Vec<_>>()}),
        )
    }
    pub fn customer_quote_views(&self, customer: &str) -> Result<Vec<Value>> {
        self.customer_quote_publications
            .iter()
            .filter(|p| p.customer_id == customer)
            .map(|p| self.customer_quote_view(p))
            .collect()
    }
    pub fn apply_customer_quote(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "portal.quote.publish" {
            let input: Publish = serde_json::from_value(body)
                .map_err(|_| "Invalid customer quote publication fields")?;
            let q = self
                .quotes
                .iter()
                .find(|q| q.id == entity)
                .ok_or("Quote not found")?;
            if q.version != input.expected_version {
                return Err("Quote changed; refresh before publishing".into());
            }
            self.customer_quote_ready(q)?;
            let prior = self
                .customer_quote_publications
                .iter()
                .rev()
                .find(|p| p.quote_id == entity);
            if prior.map(|p| p.id.as_str()) != input.expected_previous_id.as_deref() {
                return Err("Customer publication changed; refresh before publishing".into());
            }
            if prior.is_some_and(|p| p.revision == q.current().map_or(0, |r| r.revision)) {
                return Err("Customer quote revision already published".into());
            }
            let snap = snapshot(q)?;
            let (_, html) = crate::quotes::statement(self, entity, Some(snap.revision))?;
            let p = QuotePublication {
                id: domain::id("customer-publication"),
                quote_id: q.id.clone(),
                customer_id: snap.plan.customer_id.clone(),
                revision: snap.revision,
                basis_sha256: digest(&snap)?,
                snapshot: snap,
                sha256: crate::documents::sha256(html.as_bytes()),
                html,
                supersedes_id: prior.map(|p| p.id.clone()),
                published_at: Utc::now(),
                published_by: actor.into(),
            };
            p.verify()?;
            let q = self
                .quotes
                .iter_mut()
                .find(|q| q.id == entity)
                .ok_or("Quote not found")?;
            q.version = q.version.checked_add(1).ok_or("Quote version exhausted")?;
            self.customer_quote_publications.push(p.clone());
            self.record(entity,operation,"Exact issued customer quote published to its scoped partner account; no acceptance or booking created");
            let mut result = json!(p);
            result.as_object_mut().map(|o| o.remove("html"));
            return Ok(result);
        }
        if operation != "portal.quote.respond" {
            return Err("Unsupported customer quote portal operation".into());
        }
        let input: TrustedResponse =
            serde_json::from_value(body).map_err(|_| "Invalid customer quote response fields")?;
        if input.subject != actor {
            return Err("Customer identity does not match authenticated subject".into());
        }
        let p = self
            .customer_publication_for(entity, &input.customer_id)?
            .clone();
        p.verify()?;
        if !self.customer_publication_current(&p) {
            return Err("Customer publication changed; inspect its current revision".into());
        }
        let q = self
            .quotes
            .iter()
            .find(|q| q.id == p.quote_id)
            .ok_or("Quote not found")?;
        let r = input.request;
        if q.version != r.expected_version {
            return Err("Quote changed; refresh before responding".into());
        }
        if r.revision != p.revision
            || r.document_sha256 != p.sha256
            || digest(&snapshot(q)?)? != p.basis_sha256
        {
            return Err(
                "Published customer terms changed; inspect their exact document before responding"
                    .into(),
            );
        }
        if !["accept", "decline", "withdraw"].contains(&r.decision.as_str()) {
            return Err("Customer decision requires accept, decline, or withdraw".into());
        }
        if r.decision == "accept" {
            self.customer_quote_ready(q)?;
        }
        if (r.decision != "accept"
            || q.current()?.amendment_of.is_none()
            || q.current()?
                .source_load
                .as_ref()
                .is_none_or(|l| l.sync_order_id.is_none()))
            && (r.source_digest.is_some() || r.authorize_transport.is_some())
        {
            return Err(
                "Source authorization fields apply only to acceptance of an imported amendment"
                    .into(),
            );
        }
        let source = if r.decision == "accept" {
            self.customer_source_context(&p)
        } else {
            None
        };
        // Reuse quote evidence, validity, selected-offer qualification, withdrawal,
        // and exact canonical amendment consent; keep the result private to staff.
        self.apply_quote("quote.decision",&p.quote_id,json!({"expected_version":r.expected_version,"decision":r.decision,"contact":r.contact,"reference":r.reference,"note":r.note,"source_digest":r.source_digest,"authorize_transport":r.authorize_transport}),actor)?;
        let decision = CustomerDecision {
            id: domain::id("customer-decision"),
            publication_id: p.id,
            quote_id: p.quote_id.clone(),
            customer_id: p.customer_id,
            revision: p.revision,
            document_sha256: p.sha256,
            decision: r.decision,
            contact: r.contact.trim().into(),
            reference: r.reference.trim().into(),
            note: r.note.trim().into(),
            source,
            authorize_transport: r.authorize_transport.unwrap_or(false),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
        };
        self.customer_quote_decisions.push(decision.clone());
        self.record(&p.quote_id,operation,"Authenticated customer decision recorded against the exact published revision; booking and carrier custody remain separate");
        Ok(json!(decision))
    }
}
