//! A replacement customer agreement is a separate quote until explicit booking.
//! The prior agreement, source evidence, and carrier attempts remain historical.
use crate::{
    domain::{self, Load, Result, Status, Workspace},
    quotes::{Quote, QuoteEvidence, QuotePlan},
    source::{SourceDecision, SourceReview, SOURCE_UNAVAILABLE},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AmendmentTarget {
    pub quote_id: String,
    pub quote_reference: String,
    pub quote_version: u64,
    pub offer_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Supersession {
    pub quote_id: String,
    pub quote_reference: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceConsent {
    pub digest: String,
    pub snapshot: Value,
    pub authorize_transport: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Proposal {
    pub expected_version: u64,
    pub expected_load_version: u64,
    pub reference: String,
    pub plan: QuotePlan,
    pub sell_cents: i64,
    pub valid_until: DateTime<Utc>,
    pub terms: String,
    pub reason: String,
}
fn bump(version: u64) -> Result<u64> {
    version
        .checked_add(1)
        .ok_or_else(|| "Agreement version exhausted".into())
}
impl Quote {
    pub fn active_booking(&self, load: &str) -> bool {
        self.booked_load_id.as_deref() == Some(load)
            && self.superseded_by.is_none()
            && self.status == "booked"
    }
}
impl Workspace {
    pub(crate) fn amendment_basis(
        &self,
        parent_id: &str,
        expected_parent: u64,
        expected_load: u64,
        plan: &QuotePlan,
    ) -> Result<(AmendmentTarget, Load)> {
        let parent = self
            .quotes
            .iter()
            .find(|v| v.id == parent_id)
            .ok_or("Original quote not found")?;
        let load = self
            .loads
            .iter()
            .find(|v| Some(&v.id) == parent.booked_load_id.as_ref())
            .ok_or("Booked shipment not found")?;
        if !parent.active_booking(&load.id)
            || parent.version != expected_parent
            || load.version != expected_load
        {
            return Err(
                "Original agreement or shipment changed; refresh before proposing an amendment"
                    .into(),
            );
        }
        if matches!(
            load.status,
            Status::InTransit | Status::Delivered | Status::Cancelled
        ) {
            return Err("Base-plan amendments require pre-pickup freight; use the delivery, supplement, or cancellation workflows for closed/in-custody transport".into());
        }
        if plan.customer_id != load.customer_id {
            return Err(
                "An amendment cannot transfer the customer agreement to another customer".into(),
            );
        }
        // Activation re-prices the booked shipment in place, so the plan must
        // keep its currency; a different currency is a new agreement.
        if domain::currency_code(&plan.currency)? != load.currency {
            return Err("An amendment must keep the booked shipment currency".into());
        }
        Ok((
            AmendmentTarget {
                quote_id: parent.id.clone(),
                quote_reference: parent.reference.clone(),
                quote_version: parent.version,
                offer_id: parent.procured_offer()?.id.clone(),
            },
            load.clone(),
        ))
    }
    pub(crate) fn make_amendment(
        &mut self,
        parent: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let input: Proposal =
            serde_json::from_value(body).map_err(|_| "Invalid amendment proposal")?;
        let (target, load) = self.amendment_basis(
            parent,
            input.expected_version,
            input.expected_load_version,
            &input.plan,
        )?;
        if self
            .quotes
            .iter()
            .any(|q| q.reference.eq_ignore_ascii_case(input.reference.trim()))
        {
            return Err("Quote reference already exists".into());
        }
        let reference = input.reference.trim().to_owned();
        let mut revision = self.quote_revision(
            crate::quotes::Draft {
                planned_load_id: None,
                expected_load_version: None,
                reference: reference.clone(),
                plan: input.plan,
                sell_cents: input.sell_cents,
                valid_until: input.valid_until,
                terms: input.terms,
            },
            input.reason,
            actor,
            1,
        )?;
        revision.source_load = Some(load.clone());
        revision.amendment_of = Some(target);
        let quote = Quote {
            id: domain::id("quote"),
            reference,
            status: "draft".into(),
            version: 1,
            revisions: vec![revision],
            booking_source: None,
            booked_load_id: None,
            booked_at: None,
            booked_by: None,
            reconfirmations: vec![],
            procurements: vec![],
            superseded_by: None,
        };
        let value = json!(quote);
        self.record(
            &quote.id,
            "quote.amend",
            "Replacement agreement drafted; current customer agreement remains effective",
        );
        self.record(
            &load.id,
            "quote.amend",
            format!(
                "Amendment {} proposed against {}; no shipment, rate, or carrier changes",
                quote.reference, parent
            ),
        );
        self.quotes.push(quote);
        Ok(value)
    }
    pub(crate) fn source_consent(
        &self,
        load: &Load,
        digest: Option<String>,
        authorize: Option<bool>,
    ) -> Result<Option<SourceConsent>> {
        if load.sync_order_id.is_none() {
            if digest.is_some() || authorize.is_some() {
                return Err("Source consent fields require an imported amendment shipment".into());
            }
            return Ok(None);
        }
        let check = self.source_check(&load.id).ok_or(SOURCE_UNAVAILABLE)?;
        if check.last_error.is_some() || check.valid_until.is_none_or(|v| v <= Utc::now()) {
            return Err(SOURCE_UNAVAILABLE.into());
        }
        if digest.as_deref() != Some(check.digest.as_str()) {
            return Err("Sync source changed; refresh before accepting the amendment".into());
        }
        if crate::source::needs_explicit_authorization(&check.snapshot) && authorize != Some(true) {
            return Err("Explicit customer transport authorization is required for this amendment's cancelled, returned, or unknown source status".into());
        }
        Ok(Some(SourceConsent {
            digest: check.digest.clone(),
            snapshot: check.snapshot.clone(),
            authorize_transport: authorize.unwrap_or(false),
        }))
    }
    pub(crate) fn verify_amendment_source(&self, quote: &Quote, load: &Load) -> Result<()> {
        if load.sync_order_id.is_none() {
            return Ok(());
        }
        let check = self.source_check(&load.id).ok_or(SOURCE_UNAVAILABLE)?;
        if check.last_error.is_some() || check.valid_until.is_none_or(|v| v <= Utc::now()) {
            return Err(SOURCE_UNAVAILABLE.into());
        }
        let consent = quote.current()?.source_consent.as_ref().ok_or(
            "Record customer amendment acceptance against the canonical Sync source first",
        )?;
        if consent.digest != check.digest {
            return Err("Sync source changed; obtain a new customer amendment acceptance".into());
        }
        if crate::source::needs_explicit_authorization(&check.snapshot)
            && !consent.authorize_transport
        {
            return Err(
                "Explicit customer source authorization is missing from the accepted amendment"
                    .into(),
            );
        }
        if quote.current()?.decision_kind.as_deref() != Some("accept")
            || quote.current()?.decision.is_none()
        {
            return Err("Customer amendment acceptance is required".into());
        }
        Ok(())
    }
    pub(crate) fn record_amended_source(
        &mut self,
        quote: &Quote,
        load: &Load,
        proof: &QuoteEvidence,
    ) -> Result<()> {
        if load.sync_order_id.is_none() {
            return Ok(());
        }
        let check = self
            .source_check(&load.id)
            .ok_or(SOURCE_UNAVAILABLE)?
            .clone();
        let prior = check
            .active_review_id
            .as_ref()
            .and_then(|id| self.source_reviews.iter().find(|v| &v.id == id))
            .cloned();
        let baseline = prior
            .as_ref()
            .map(|v| v.baseline.clone())
            .or_else(|| {
                check
                    .authorized_review_id
                    .as_ref()
                    .and_then(|id| self.source_reviews.iter().find(|v| &v.id == id))
                    .map(|v| v.snapshot.clone())
            })
            .or_else(|| load.source_order.clone())
            .ok_or("Original source evidence not found")?;
        let id = domain::id("source-review");
        let mut fields = prior
            .as_ref()
            .map_or_else(Vec::new, |v| v.changed_fields.clone());
        if quote.current()?.source_load.as_ref().is_some_and(|old| {
            json!(QuotePlan::from_load(old)) != json!(QuotePlan::from_load(load))
        }) {
            fields.push("freight_plan".into());
        }
        fields.push("commercial_amendment".into());
        fields.sort();
        fields.dedup();
        if let Some(prior) = prior {
            let old = self
                .source_reviews
                .iter_mut()
                .find(|v| v.id == prior.id)
                .ok_or("Source review not found")?;
            old.status = "superseded".into();
            old.superseded_by_id = Some(id.clone());
            old.version = bump(old.version)?;
        }
        let plan = json!(QuotePlan::from_load(load));
        self.source_reviews.push(SourceReview {
            id: id.clone(),
            load_id: load.id.clone(),
            order_id: check.order_id.clone(),
            baseline,
            snapshot: check.snapshot.clone(),
            digest: check.digest.clone(),
            changed_fields: fields,
            plan_snapshot: plan.clone(),
            observed_at: check.checked_at.unwrap_or_else(Utc::now),
            observed_by: check.checked_by.clone(),
            status: "resolved".into(),
            version: 1,
            superseded_by_id: None,
            decision: Some(SourceDecision {
                quote_id: Some(quote.id.clone()),
                decision: if crate::source::needs_explicit_authorization(&check.snapshot) {
                    "authorize_transport"
                } else {
                    "confirm_plan"
                }
                .into(),
                contact: proof.contact.clone(),
                reference: proof.reference.clone(),
                note: proof.note.clone(),
                recorded_at: proof.recorded_at,
                recorded_by: proof.recorded_by.clone(),
                plan_snapshot: plan,
            }),
        });
        let check = self
            .source_checks
            .iter_mut()
            .find(|v| v.load_id == load.id)
            .ok_or(SOURCE_UNAVAILABLE)?;
        check.active_review_id = None;
        check.authorized_review_id = Some(id);
        check.version = bump(check.version)?;
        self.record(&load.id,"source.amendment",format!("Customer amendment {} authorizes this exact freight plan and verified source; source order unchanged",quote.reference));
        Ok(())
    }
    pub(crate) fn book_amendment(
        &mut self,
        quote: &mut Quote,
        expected: Option<u64>,
        actor: &str,
    ) -> Result<(String, u64)> {
        let revision = quote.current()?.clone();
        let target = revision
            .amendment_of
            .as_ref()
            .ok_or("Amendment target not found")?;
        let baseline = revision
            .source_load
            .as_ref()
            .ok_or("Amendment baseline shipment not found")?;
        let parent_index = self
            .quotes
            .iter()
            .position(|v| v.id == target.quote_id)
            .ok_or("Original quote not found")?;
        let parent = self.quotes[parent_index].clone();
        let load_index = self
            .loads
            .iter()
            .position(|v| v.id == baseline.id)
            .ok_or("Booked shipment not found")?;
        let load = self.loads[load_index].clone();
        if !parent.active_booking(&load.id)
            || parent.version != target.quote_version
            || parent.procured_offer()?.id != target.offer_id
        {
            return Err("Original agreement changed; revise the amendment against the current agreement before accepting again".into());
        }
        if expected != Some(load.version) {
            return Err("Shipment changed; refresh before activating the amendment".into());
        }
        if load.status != Status::Planned
            || load.carrier_id.is_some()
            || load.tender_id.is_some()
            || load.driver_id.is_some()
            || load.equipment_id.is_some()
            || self.release_pending(&load)
            || self.unresolved_tender_incidents(&load.id)
        {
            return Err("Safely withdraw or obtain confirmed carrier release before activating the amendment; shipment must be planned and unassigned".into());
        }
        if json!(QuotePlan::from_load(&load)) != json!(QuotePlan::from_load(baseline))
            || load.buy_cents != baseline.buy_cents
            || load.sell_cents != baseline.sell_cents
            || load.customer_id != revision.plan.customer_id
        {
            return Err("Freight agreement changed; revise and obtain customer acceptance against the current agreement".into());
        }
        if self
            .charges
            .iter()
            .any(|v| v.load_id == load.id && matches!(v.status.as_str(), "proposed" | "approved"))
            || self.invoices.iter().any(|v| v.load_id == load.id)
            || self.settlements.iter().any(|v| v.load_id == load.id)
            || self
                .bills
                .iter()
                .any(|v| v.load_id == load.id && v.status != "void")
            || self
                .carrier_documents
                .iter()
                .any(|v| v.load_id == load.id && v.status == "pending")
        {
            return Err("Reconcile charge agreements and financial documents before replacing the base agreement".into());
        }
        self.verify_amendment_source(quote, &load)?;
        let offer = revision
            .offers
            .iter()
            .find(|v| Some(&v.id) == revision.selected_offer_id.as_ref())
            .ok_or("Selected offer not found")?;
        let mut validation = self.clone();
        validation.loads.remove(load_index);
        revision
            .plan
            .load(&load.reference, offer.amount_cents, revision.sell_cents)
            .validate(&validation)?;
        let mut revised = load.clone();
        revised.customer_id = revision.plan.customer_id.clone();
        revised.mode = revision.plan.mode.clone();
        revised.equipment = revision.plan.equipment.clone();
        revised.commodity = revision.plan.commodity.clone();
        revised.weight_lbs = revision.plan.weight_lbs;
        revised.miles = revision.plan.miles;
        revised.origin = revision.plan.origin.clone();
        revised.intermediate_stops = revision.plan.intermediate_stops.clone();
        revised.destination = revision.plan.destination.clone();
        revised.buy_cents = offer.amount_cents;
        revised.sell_cents = revision.sell_cents;
        revised.version = bump(load.version)?;
        revised.updated_at = Utc::now();
        self.check_stop_revision(&load, &revised)?;
        let proof = revision
            .decision
            .as_ref()
            .ok_or("Customer amendment acceptance is required")?;
        self.record_amended_source(quote, &revised, proof)?;
        self.loads[load_index] = revised.clone();
        let parent = &mut self.quotes[parent_index];
        let parent_reference = parent.reference.clone();
        parent.superseded_by = Some(Supersession {
            quote_id: quote.id.clone(),
            quote_reference: quote.reference.clone(),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
        });
        parent.status = "superseded".into();
        parent.version = bump(parent.version)?;
        quote.booking_source = Some(load);
        self.record(&revised.id,"quote.amendment_activated",format!("Customer amendment {} replaced {}; original agreements, source evidence, and carrier tenders retained",quote.reference,parent_reference));
        Ok((revised.id, revised.version))
    }
}
