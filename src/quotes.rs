//! Human-confirmed spot quotes and procurement evidence. Booking creates one
//! planned load; a price offer never establishes carrier acceptance or custody.
use crate::domain::{self, Carrier, Customer, Load, NewLoad, Result, Status, Stop, Workspace};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotePlan {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub customer_id: String,
    pub mode: String,
    pub equipment: String,
    pub commodity: String,
    pub weight_lbs: u32,
    pub miles: u32,
    pub origin: Stop,
    pub destination: Stop,
    #[serde(
        default = "domain::default_currency",
        skip_serializing_if = "domain::is_default_currency"
    )]
    pub currency: String,
}
impl QuotePlan {
    pub fn from_load(load: &Load) -> Self {
        Self {
            intermediate_stops: load.intermediate_stops.clone(),
            customer_id: load.customer_id.clone(),
            mode: load.mode.clone(),
            equipment: load.equipment.clone(),
            commodity: load.commodity.clone(),
            weight_lbs: load.weight_lbs,
            miles: load.miles,
            origin: load.origin.clone(),
            destination: load.destination.clone(),
            currency: load.currency.clone(),
        }
    }
    pub(crate) fn load(&self, reference: &str, buy: i64, sell: i64) -> NewLoad {
        NewLoad {
            intermediate_stops: self.intermediate_stops.clone(),
            reference: reference.into(),
            customer_id: self.customer_id.clone(),
            mode: self.mode.clone(),
            equipment: self.equipment.clone(),
            commodity: self.commodity.clone(),
            weight_lbs: self.weight_lbs,
            miles: self.miles,
            origin: self.origin.clone(),
            destination: self.destination.clone(),
            buy_cents: buy,
            sell_cents: sell,
            currency: self.currency.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuoteEvidence {
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierOffer {
    #[serde(default)]
    pub rate_source: Option<crate::contracts::RateSource>,
    pub id: String,
    pub carrier: Carrier,
    pub amount_cents: i64,
    pub valid_until: DateTime<Utc>,
    pub evidence: QuoteEvidence,
    pub withdrawn: Option<QuoteEvidence>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuoteRevision {
    #[serde(default)]
    pub amendment_of: Option<crate::amendments::AmendmentTarget>,
    #[serde(default)]
    pub source_consent: Option<crate::amendments::SourceConsent>,
    #[serde(default)]
    pub source_load: Option<Load>,
    #[serde(default)]
    pub issuer: Option<crate::finance::BillingProfile>,
    pub revision: u64,
    pub plan: QuotePlan,
    pub customer: Customer,
    pub sell_cents: i64,
    pub valid_until: DateTime<Utc>,
    pub terms: String,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub reason: String,
    pub offers: Vec<CarrierOffer>,
    pub selected_offer_id: Option<String>,
    pub issued_at: Option<DateTime<Utc>>,
    pub issued_by: Option<String>,
    pub decision: Option<QuoteEvidence>,
    pub decision_kind: Option<String>,
    pub decision_history: Vec<(String, QuoteEvidence)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuoteReconfirmation {
    #[serde(default)]
    pub offer_id: String,
    pub valid_until: DateTime<Utc>,
    pub evidence: QuoteEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcurementAward {
    pub offer: CarrierOffer,
    pub previous_offer_id: Option<String>,
    pub load_version: u64,
    pub reason: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quote {
    #[serde(default)]
    pub superseded_by: Option<crate::amendments::Supersession>,
    #[serde(default)]
    pub booking_source: Option<Load>,
    pub id: String,
    pub reference: String,
    pub status: String,
    pub version: u64,
    pub revisions: Vec<QuoteRevision>,
    pub booked_load_id: Option<String>,
    pub booked_at: Option<DateTime<Utc>>,
    pub booked_by: Option<String>,
    pub reconfirmations: Vec<QuoteReconfirmation>,
    #[serde(default)]
    pub procurements: Vec<ProcurementAward>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    pub planned_load_id: Option<String>,
    pub expected_load_version: Option<u64>,
    pub reference: String,
    pub plan: QuotePlan,
    pub sell_cents: i64,
    pub valid_until: DateTime<Utc>,
    pub terms: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    expected_parent_version: Option<u64>,
    expected_load_version: Option<u64>,
    expected_version: u64,
    plan: QuotePlan,
    sell_cents: i64,
    valid_until: DateTime<Utc>,
    terms: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OfferInput {
    expected_version: u64,
    carrier_id: String,
    amount_cents: i64,
    valid_until: DateTime<Utc>,
    contact: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WithdrawOffer {
    expected_version: u64,
    offer_id: String,
    contact: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    expected_version: u64,
    offer_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    source_digest: Option<String>,
    authorize_transport: Option<bool>,
    expected_version: u64,
    decision: String,
    contact: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Book {
    expected_load_version: Option<u64>,
    expected_version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reconfirm {
    expected_version: u64,
    expected_load_version: u64,
    valid_until: DateTime<Utc>,
    contact: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reprocure {
    expected_version: u64,
    expected_load_version: u64,
    carrier_id: String,
    amount_cents: i64,
    valid_until: DateTime<Utc>,
    contact: String,
    reference: String,
    note: String,
    reason: String,
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| "Invalid quote request".into())
}
fn check(actual: u64, expected: u64) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err("Quote changed; refresh before continuing".into())
    }
}
fn future(date: DateTime<Utc>) -> Result<()> {
    let now = Utc::now();
    if date <= now || date > now + chrono::Duration::days(365) {
        Err("Quote/offer validity must be in the future and within 365 days".into())
    } else {
        Ok(())
    }
}
fn evidence(
    contact: String,
    reference: String,
    note: String,
    actor: &str,
) -> Result<QuoteEvidence> {
    for (v, label) in [
        (&contact, "Actual counterparty contact"),
        (&reference, "External confirmation reference"),
        (&note, "Evidence note"),
    ] {
        domain::required(v, label)?;
    }
    Ok(QuoteEvidence {
        contact: contact.trim().into(),
        reference: reference.trim().into(),
        note: note.trim().into(),
        recorded_at: Utc::now(),
        recorded_by: actor.into(),
    })
}
fn positive(value: i64) -> Result<()> {
    domain::money(value)?;
    if value == 0 {
        Err("Quote and offer amounts must be positive minor units".into())
    } else {
        Ok(())
    }
}
impl Quote {
    pub fn procured_offer(&self) -> Result<&CarrierOffer> {
        if let Some(award) = self.procurements.last() {
            return Ok(&award.offer);
        }
        let revision = self.current()?;
        revision
            .offers
            .iter()
            .find(|v| Some(&v.id) == revision.selected_offer_id.as_ref())
            .ok_or_else(|| "Selected offer not found".into())
    }
    pub fn procurement_expiry(&self) -> Result<DateTime<Utc>> {
        let offer = self.procured_offer()?;
        Ok(self
            .reconfirmations
            .iter()
            .rev()
            .find(|v| v.offer_id == offer.id)
            .map_or(offer.valid_until, |v| v.valid_until))
    }
    pub fn current(&self) -> Result<&QuoteRevision> {
        self.revisions
            .last()
            .ok_or_else(|| "Quote revision not found".into())
    }
    pub fn effective_status(&self) -> &str {
        if !["booked", "superseded", "declined", "withdrawn"].contains(&self.status.as_str())
            && self
                .revisions
                .last()
                .is_some_and(|v| v.valid_until <= Utc::now())
        {
            "expired"
        } else {
            &self.status
        }
    }
}
impl Workspace {
    pub(crate) fn quote_source(&self, id: &str, expected: Option<u64>) -> Result<Load> {
        let load = self
            .loads
            .iter()
            .find(|v| v.id == id)
            .ok_or("Planned source load not found")?;
        if Some(load.version) != expected {
            return Err("Source shipment changed; refresh before quoting or booking".into());
        }
        if load.status != Status::Planned || load.carrier_id.is_some() || load.tender_id.is_some() {
            return Err("Quote source requires a planned unassigned shipment".into());
        }
        domain::currency_code(&load.currency)?;
        if self
            .quotes
            .iter()
            .any(|v| v.booked_load_id.as_deref() == Some(id))
        {
            return Err("Source shipment already belongs to a booked quote".into());
        }
        if self.unresolved_tender_incidents(id)
            || self.has_approved_charges(id)
            || self.invoices.iter().any(|v| v.load_id == id)
            || self.settlements.iter().any(|v| v.load_id == id)
            || self
                .bills
                .iter()
                .any(|v| v.load_id == id && v.status != "void")
            || self.outbox.iter().any(|v| {
                v.load_id == id && v.destination == "stateset-edi" && v.status == "sending"
            })
        {
            return Err("Resolve carrier custody and financial commitments before quoting this planned shipment".into());
        }
        Ok(load.clone())
    }
    pub(crate) fn quote_revision(
        &self,
        input: Draft,
        reason: String,
        actor: &str,
        revision: u64,
    ) -> Result<QuoteRevision> {
        let Draft {
            planned_load_id,
            expected_load_version,
            reference,
            mut plan,
            sell_cents: sell,
            valid_until: until,
            terms,
        } = input;
        plan.currency = domain::currency_code(&plan.currency)?;
        let source_load = if let Some(id) = planned_load_id {
            let load = self.quote_source(&id, expected_load_version)?;
            if json!(QuotePlan::from_load(&load)) != json!(plan) {
                return Err("Quote plan must match the existing source shipment; revise that plan explicitly before quoting".into());
            }
            Some(load)
        } else {
            if expected_load_version.is_some() {
                return Err("Source load version requires an existing planned shipment".into());
            }
            None
        };
        plan.load(&reference, 0, sell).validate(self)?;
        crate::itinerary::validate_route(
            &plan.origin,
            &plan.intermediate_stops,
            &plan.destination,
            true,
        )?;
        for stop in [&plan.origin, &plan.destination] {
            crate::stops::validate_stop(stop, true)?;
        }
        if plan.origin.date < Utc::now().date_naive() {
            return Err("A new quote requires a current or future pickup date".into());
        }
        positive(sell)?;
        future(until)?;
        if terms.trim().is_empty() || terms.len() > 2000 {
            return Err("Quote terms must contain 1–2,000 characters".into());
        }
        domain::required(&reason, "Revision reason")?;
        let customer = self
            .customers
            .iter()
            .find(|v| v.id == plan.customer_id)
            .ok_or("Customer not found")?
            .clone();
        Ok(QuoteRevision {
            amendment_of: None,
            source_consent: None,
            source_load,
            issuer: self.billing_profile.clone(),
            revision,
            plan,
            customer,
            sell_cents: sell,
            valid_until: until,
            terms: terms.trim().into(),
            created_at: Utc::now(),
            created_by: actor.into(),
            reason,
            offers: vec![],
            selected_offer_id: None,
            issued_at: None,
            issued_by: None,
            decision: None,
            decision_kind: None,
            decision_history: vec![],
        })
    }
    pub(crate) fn quoted_carrier(
        &self,
        revision: &QuoteRevision,
        offer: &CarrierOffer,
    ) -> Result<()> {
        if let Some(source) = &offer.rate_source {
            self.verify_contract_source(
                source,
                &revision.plan,
                &offer.carrier,
                offer.amount_cents,
            )?;
        }
        let current = self
            .carriers
            .iter()
            .find(|v| v.id == offer.carrier.id)
            .ok_or("Carrier not found")?;
        if current.scac != offer.carrier.scac || current.partner_id != offer.carrier.partner_id {
            return Err("Carrier identity changed; obtain a new carrier offer".into());
        }
        if current.equipment != revision.plan.equipment
            || current.insurance_expires
                < revision.plan.destination.date.max(Utc::now().date_naive())
        {
            return Err(
                "Carrier equipment and insurance must qualify through quoted delivery".into(),
            );
        }
        if offer.withdrawn.is_some() {
            return Err("Selected carrier offer was withdrawn; obtain a new offer".into());
        }
        Ok(())
    }
    pub fn verify_quoted_tender(&self, load: &Load, carrier: &Carrier) -> Result<()> {
        let Some(quote) = self.quotes.iter().find(|v| v.active_booking(&load.id)) else {
            return Ok(());
        };
        let revision = quote.current()?;
        let offer = quote.procured_offer()?;
        self.quoted_carrier(revision, offer)?;
        let expiry = quote.procurement_expiry()?;
        if expiry <= Utc::now() {
            return Err(
                "Carrier offer expired; record actual same-rate reconfirmation before tendering"
                    .into(),
            );
        }
        if carrier.id != offer.carrier.id
            || load.buy_cents != offer.amount_cents
            || load.sell_cents != revision.sell_cents
            || json!(QuotePlan::from_load(load)) != json!(revision.plan)
        {
            return Err("Tender carrier and base rates must match the accepted quote".into());
        }
        Ok(())
    }
    pub fn apply_quote(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if matches!(
            operation,
            "quote.contract-offer" | "quote.contract-reprocure"
        ) {
            return self.apply_contract_offer(operation, entity, body, actor);
        }
        if operation == "quote.amend" {
            return self.make_amendment(entity, body, actor);
        }
        if operation == "quote.create" {
            let input: Draft = decode(body)?;
            if self
                .quotes
                .iter()
                .any(|v| v.reference.eq_ignore_ascii_case(input.reference.trim()))
            {
                return Err("Quote reference already exists".into());
            }
            let reference = input.reference.trim().to_owned();
            let revision = self.quote_revision(input, "Initial quote".into(), actor, 1)?;
            let quote = Quote {
                superseded_by: None,
                booking_source: None,
                id: domain::id("quote"),
                reference,
                status: "draft".into(),
                version: 1,
                revisions: vec![revision],
                booked_load_id: None,
                booked_at: None,
                booked_by: None,
                reconfirmations: vec![],
                procurements: vec![],
            };
            let response = json!(quote);
            self.record(
                &quote.id,
                operation,
                "Draft quote recorded; no shipment or carrier commitment created",
            );
            self.quotes.push(quote);
            return Ok(response);
        }
        let index = self
            .quotes
            .iter()
            .position(|v| v.id == entity)
            .ok_or("Quote not found")?;
        let mut quote = self.quotes[index].clone();
        let current = quote.current()?.clone();
        if quote.superseded_by.is_some() {
            return Err("Quote already superseded; use its current replacement agreement".into());
        }
        match operation {
            "quote.revise" => {
                let input: Revision = decode(body)?;
                check(quote.version, input.expected_version)?;
                if !["draft", "issued", "declined", "withdrawn"].contains(&quote.status.as_str()) {
                    return Err("Withdraw an accepted unbooked quote with customer confirmation before revising; booked quotes retain their agreement".into());
                }
                let basis = if let Some(target) = &current.amendment_of {
                    Some(
                        self.amendment_basis(
                            &target.quote_id,
                            input
                                .expected_parent_version
                                .ok_or("Current original quote version is required")?,
                            input
                                .expected_load_version
                                .ok_or("Current shipment version is required")?,
                            &input.plan,
                        )?,
                    )
                } else {
                    if input.expected_parent_version.is_some() {
                        return Err("Original quote version applies only to an amendment".into());
                    }
                    None
                };
                let mut revision = self.quote_revision(
                    Draft {
                        planned_load_id: if basis.is_some() {
                            None
                        } else {
                            current.source_load.as_ref().map(|v| v.id.clone())
                        },
                        expected_load_version: if basis.is_some() {
                            None
                        } else {
                            input.expected_load_version
                        },
                        reference: quote.reference.clone(),
                        plan: input.plan,
                        sell_cents: input.sell_cents,
                        valid_until: input.valid_until,
                        terms: input.terms,
                    },
                    input.reason,
                    actor,
                    current
                        .revision
                        .checked_add(1)
                        .ok_or("Quote revision exhausted")?,
                )?;
                if let Some((target, load)) = basis {
                    revision.amendment_of = Some(target);
                    revision.source_load = Some(load);
                }
                quote.revisions.push(revision);
                quote.status = "draft".into();
            }
            "quote.offer" => {
                let input: OfferInput = decode(body)?;
                check(quote.version, input.expected_version)?;
                if quote.status != "draft" {
                    return Err(
                        "Carrier offers can be recorded only against the current draft revision"
                            .into(),
                    );
                }
                positive(input.amount_cents)?;
                future(input.valid_until)?;
                let carrier = self
                    .carriers
                    .iter()
                    .find(|v| v.id == input.carrier_id)
                    .ok_or("Carrier not found")?
                    .clone();
                let offer = CarrierOffer {
                    rate_source: None,
                    id: domain::id("offer"),
                    carrier,
                    amount_cents: input.amount_cents,
                    valid_until: input.valid_until,
                    evidence: evidence(input.contact, input.reference, input.note, actor)?,
                    withdrawn: None,
                };
                self.quoted_carrier(&current, &offer)?;
                if current.offers.iter().any(|v| {
                    v.carrier.id == offer.carrier.id
                        && v.evidence
                            .reference
                            .eq_ignore_ascii_case(&offer.evidence.reference)
                }) {
                    return Err("Carrier offer reference already recorded for this revision".into());
                }
                quote
                    .revisions
                    .last_mut()
                    .ok_or("Quote revision not found")?
                    .offers
                    .push(offer);
            }
            "quote.offer.withdraw" => {
                let input: WithdrawOffer = decode(body)?;
                check(quote.version, input.expected_version)?;
                if quote.booked_load_id.is_some() {
                    return Err("A booked shipment retains its carrier agreement; coordinate through shipment release and exceptions".into());
                }
                let proof = evidence(input.contact, input.reference, input.note, actor)?;
                let offer = quote
                    .revisions
                    .last_mut()
                    .ok_or("Quote revision not found")?
                    .offers
                    .iter_mut()
                    .find(|v| v.id == input.offer_id)
                    .ok_or("Carrier offer not found")?;
                if offer.withdrawn.is_some() {
                    return Err("Carrier offer already withdrawn".into());
                }
                offer.withdrawn = Some(proof);
            }
            "quote.issue" => {
                let input: Issue = decode(body)?;
                check(quote.version, input.expected_version)?;
                if quote.status != "draft" {
                    return Err("Only a draft quote can be issued".into());
                }
                future(current.valid_until)?;
                let offer = current
                    .offers
                    .iter()
                    .find(|v| v.id == input.offer_id)
                    .ok_or("Carrier offer not found")?;
                self.quoted_carrier(&current, offer)?;
                if offer.valid_until < current.valid_until {
                    return Err(
                        "Customer quote expiry cannot exceed the selected carrier offer expiry"
                            .into(),
                    );
                }
                let rev = quote
                    .revisions
                    .last_mut()
                    .ok_or("Quote revision not found")?;
                rev.issuer = Some(self.billing_profile.clone().ok_or(
                    "Configure the billing issuer identity before issuing a customer quote",
                )?);
                rev.selected_offer_id = Some(input.offer_id);
                rev.issued_at = Some(Utc::now());
                rev.issued_by = Some(actor.into());
                quote.status = "issued".into();
            }
            "quote.decision" => {
                let input: Decision = decode(body)?;
                check(quote.version, input.expected_version)?;
                let proof = evidence(input.contact, input.reference, input.note, actor)?;
                if quote.booked_load_id.is_some() {
                    return Err("Booked quote decisions remain immutable; coordinate through the booked shipment".into());
                }
                if input.decision == "withdraw"
                    && ["issued", "accepted", "declined"].contains(&quote.status.as_str())
                {
                    quote.status = "withdrawn".into();
                } else if ["accept", "decline"].contains(&input.decision.as_str())
                    && quote.status == "issued"
                {
                    if input.decision == "accept" {
                        future(current.valid_until)?;
                        let offer = current
                            .offers
                            .iter()
                            .find(|v| Some(&v.id) == current.selected_offer_id.as_ref())
                            .ok_or("Selected offer not found")?;
                        self.quoted_carrier(&current, offer)?;
                        if current.amendment_of.is_some() {
                            let load_id = &current
                                .source_load
                                .as_ref()
                                .ok_or("Amendment source shipment missing")?
                                .id;
                            let load = self
                                .loads
                                .iter()
                                .find(|v| &v.id == load_id)
                                .ok_or("Booked shipment not found")?;
                            quote
                                .revisions
                                .last_mut()
                                .ok_or("Quote revision not found")?
                                .source_consent = self.source_consent(
                                load,
                                input.source_digest.clone(),
                                input.authorize_transport,
                            )?;
                        } else if input.source_digest.is_some()
                            || input.authorize_transport.is_some()
                        {
                            return Err("Source consent fields apply only to an imported commercial amendment".into());
                        }
                    }
                    quote.status = if input.decision == "accept" {
                        "accepted"
                    } else {
                        "declined"
                    }
                    .into();
                } else {
                    return Err("Quote decision is not permitted in its current state".into());
                }
                let rev = quote
                    .revisions
                    .last_mut()
                    .ok_or("Quote revision not found")?;
                rev.decision_history
                    .push((input.decision.clone(), proof.clone()));
                rev.decision = Some(proof);
                rev.decision_kind = Some(input.decision);
            }
            "quote.book" => {
                let input: Book = decode(body)?;
                check(quote.version, input.expected_version)?;
                if quote.status != "accepted" || quote.booked_load_id.is_some() {
                    return Err("Only an accepted unbooked quote can create a shipment".into());
                }
                future(current.valid_until)?;
                let offer = current
                    .offers
                    .iter()
                    .find(|v| Some(&v.id) == current.selected_offer_id.as_ref())
                    .ok_or("Selected offer not found")?;
                self.quoted_carrier(&current, offer)?;
                future(offer.valid_until)?;
                if current.plan.origin.date < Utc::now().date_naive() {
                    return Err("Pickup date passed; obtain an updated customer quote".into());
                }
                let (booked_id, booked_version) = if current.amendment_of.is_some() {
                    self.book_amendment(&mut quote, input.expected_load_version, actor)?
                } else if let Some(source) = &current.source_load {
                    let load = self.quote_source(&source.id, input.expected_load_version)?;
                    self.guard_source(&load)?;
                    if json!(QuotePlan::from_load(&load)) != json!(current.plan) {
                        return Err("Source shipment plan changed from the accepted quote; obtain the correct revision".into());
                    }
                    let load_index = self
                        .loads
                        .iter()
                        .position(|v| v.id == load.id)
                        .ok_or("Source load not found")?;
                    let active = &mut self.loads[load_index];
                    active.buy_cents = offer.amount_cents;
                    active.sell_cents = current.sell_cents;
                    active.version = active
                        .version
                        .checked_add(1)
                        .ok_or("Load version exhausted")?;
                    active.updated_at = Utc::now();
                    let version = active.version;
                    self.record(&load.id,operation,format!("Accepted quote {} booked on the existing shipment; original commerce identity and snapshot retained",quote.reference));
                    quote.booking_source = Some(load.clone());
                    (load.id, version)
                } else {
                    if input.expected_load_version.is_some() {
                        return Err(
                            "Standalone quote booking cannot select a source load version".into(),
                        );
                    }
                    let plan =
                        current
                            .plan
                            .load(&quote.reference, offer.amount_cents, current.sell_cents);
                    let created=self.apply("load.create","",json!({"reference":plan.reference,"customer_id":plan.customer_id,"mode":plan.mode,"equipment":plan.equipment,"commodity":plan.commodity,"weight_lbs":plan.weight_lbs,"miles":plan.miles,"origin":plan.origin,"destination":plan.destination,"intermediate_stops":plan.intermediate_stops,"buy_cents":plan.buy_cents,"sell_cents":plan.sell_cents,"currency":plan.currency}))?;
                    (
                        created["id"]
                            .as_str()
                            .ok_or("Booked load not found")?
                            .to_owned(),
                        1,
                    )
                };
                quote.booked_load_id = Some(booked_id);
                quote.booked_at = Some(Utc::now());
                quote.booked_by = Some(actor.into());
                quote.procurements.push(ProcurementAward {
                    offer: offer.clone(),
                    previous_offer_id: current.amendment_of.as_ref().map(|v| v.offer_id.clone()),
                    load_version: booked_version,
                    reason: if current.amendment_of.is_some() {
                        "Activated customer amendment with new carrier procurement"
                    } else {
                        "Initial booking from accepted quote"
                    }
                    .into(),
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                });
                quote.status = "booked".into();
            }
            "quote.reconfirm" => {
                let input: Reconfirm = decode(body)?;
                check(quote.version, input.expected_version)?;
                let load = self
                    .loads
                    .iter()
                    .find(|v| Some(&v.id) == quote.booked_load_id.as_ref())
                    .ok_or("Booked load not found")?;
                if load.status != Status::Planned || load.version != input.expected_load_version {
                    return Err(
                        "Same-rate reconfirmation requires the unchanged planned shipment".into(),
                    );
                }
                let offer = quote.procured_offer()?;
                self.quoted_carrier(&current, offer)?;
                let offer_id = offer.id.clone();
                future(input.valid_until)?;
                let proof = evidence(input.contact, input.reference, input.note, actor)?;
                if quote
                    .reconfirmations
                    .iter()
                    .any(|v| v.evidence.reference.eq_ignore_ascii_case(&proof.reference))
                {
                    return Err("Carrier reconfirmation reference already recorded".into());
                }
                quote.reconfirmations.push(QuoteReconfirmation {
                    offer_id,
                    valid_until: input.valid_until,
                    evidence: proof,
                });
            }
            "quote.reprocure" => {
                let input: Reprocure = decode(body)?;
                check(quote.version, input.expected_version)?;
                let load_index = self
                    .loads
                    .iter()
                    .position(|v| Some(&v.id) == quote.booked_load_id.as_ref())
                    .ok_or("Booked load not found")?;
                let load = &self.loads[load_index];
                self.guard_source(load)?;
                if quote.status != "booked"
                    || load.status != Status::Planned
                    || load.version != input.expected_load_version
                    || load.carrier_id.is_some()
                    || load.tender_id.is_some()
                {
                    return Err("Replacement procurement requires the unchanged planned shipment after safe carrier withdrawal/release".into());
                }
                if self.unresolved_tender_incidents(&load.id)
                    || self.has_approved_charges(&load.id)
                    || self.invoices.iter().any(|v| v.load_id == load.id)
                    || self
                        .bills
                        .iter()
                        .any(|v| v.load_id == load.id && v.status != "void")
                {
                    return Err("Resolve custody, approved charges, and financial documents before replacement procurement".into());
                }
                domain::required(&input.reason, "Replacement procurement reason")?;
                positive(input.amount_cents)?;
                future(input.valid_until)?;
                let carrier = self
                    .carriers
                    .iter()
                    .find(|v| v.id == input.carrier_id)
                    .ok_or("Carrier not found")?
                    .clone();
                let offer = CarrierOffer {
                    rate_source: None,
                    id: domain::id("offer"),
                    carrier,
                    amount_cents: input.amount_cents,
                    valid_until: input.valid_until,
                    evidence: evidence(input.contact, input.reference, input.note, actor)?,
                    withdrawn: None,
                };
                self.quoted_carrier(&current, &offer)?;
                if quote.procurements.iter().any(|v| {
                    v.offer.carrier.id == offer.carrier.id
                        && v.offer
                            .evidence
                            .reference
                            .eq_ignore_ascii_case(&offer.evidence.reference)
                }) {
                    return Err("Procurement confirmation reference already recorded".into());
                }
                let award = ProcurementAward {
                    offer,
                    previous_offer_id: Some(quote.procured_offer()?.id.clone()),
                    load_version: load.version,
                    reason: input.reason,
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                };
                let load_id = load.id.clone();
                let current_load = &mut self.loads[load_index];
                current_load.buy_cents = award.offer.amount_cents;
                current_load.version = current_load
                    .version
                    .checked_add(1)
                    .ok_or("Load version exhausted")?;
                current_load.updated_at = Utc::now();
                self.record(&load_id,operation,format!("Replacement carrier procurement confirmed at {} cents; accepted customer sell agreement retained",award.offer.amount_cents));
                quote.procurements.push(award);
            }
            _ => return Err("Unsupported quote operation".into()),
        }
        quote.version = quote
            .version
            .checked_add(1)
            .ok_or("Quote version exhausted")?;
        self.record(
            &quote.id,
            operation,
            format!(
                "Quote {} revision {} · {}; original evidence retained",
                quote.reference,
                quote.current()?.revision,
                quote.status
            ),
        );
        let response = json!(quote);
        self.quotes[index] = quote;
        Ok(response)
    }
}

pub fn statement(
    workspace: &Workspace,
    id: &str,
    revision: Option<u64>,
) -> Result<(String, String)> {
    let quote = workspace
        .quotes
        .iter()
        .find(|v| v.id == id)
        .ok_or("Quote not found")?;
    let rev = if let Some(number) = revision {
        quote
            .revisions
            .iter()
            .find(|v| v.revision == number)
            .ok_or("Quote revision not found")?
    } else {
        quote.current()?
    };
    fn html(v: &str) -> String {
        v.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
    let status = if rev.issued_at.is_none() {
        "DRAFT · NOT ISSUED"
    } else if rev.revision != quote.current()?.revision {
        "HISTORICAL QUOTE REVISION"
    } else {
        quote.effective_status()
    };
    let party = |stop: &Stop| {
        html(&format!(
            "{} · {} · {}, {} {} {}",
            stop.facility,
            stop.details.address_lines.join(", "),
            stop.city,
            stop.state,
            stop.details.postal_code,
            stop.details.country
        ))
    };
    let issuer = rev
        .issuer
        .as_ref()
        .map(|v| {
            format!(
                "<h2>{}</h2><p>{}<br>{}</p>",
                html(&v.legal_name),
                html(&v.address),
                html(&v.contact)
            )
        })
        .unwrap_or_else(|| "<p>Issuer identity is not configured.</p>".into());
    let source_info = rev
        .source_load
        .as_ref()
        .map(|v| {
            format!(
                "<p>Shipment {}{}</p>",
                html(&v.reference),
                v.order_number
                    .as_ref()
                    .map(|n| format!(" · Customer order {}", html(n)))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_default();
    let amendment_info=rev.amendment_of.as_ref().map(|v|format!("<p>Customer amendment of quote {}. {}</p>",html(&v.quote_reference),if quote.booked_load_id.is_some(){"This amendment has been activated on the existing shipment. Previous agreements remain historical."}else{"The existing agreement remains effective until this amendment is explicitly booked after safe carrier coordination."})).unwrap_or_default();
    let currency = stateset_freight_commerce::parse_currency(&rev.plan.currency)?;
    let price = stateset_freight_commerce::format_money(rev.sell_cents, currency);
    let result=format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Freight quote {reference}</title><style>body{{font:16px system-ui,sans-serif;max-width:820px;margin:48px auto;padding:24px;color:#18332d}}h1{{font-size:28px}}p{{line-height:1.7;white-space:pre-wrap}}.total{{font-size:24px}}@media print{{body{{margin:0}}}}</style><h1>Freight quote {reference}</h1>{issuer}{amendment_info}<p>Revision {revision} · {status}</p><h2>{customer}</h2><p>{contact}</p><h2>Shipment</h2>{source_info}<p>Pickup: {pickup}\n{origin}\nDelivery: {delivery}\n{destination}\n{mode} · {equipment} · {weight} lb · {miles} miles\n{commodity}</p><p class=\"total\">Quoted inclusive price: {currency} {price}</p><p>Acceptance and booking deadline: {validity} (UTC)</p><h2>Terms</h2><p>{terms}</p><p>Carrier assignment, shipment execution, and invoicing follow separate workflows. This document does not confirm carrier acceptance, delivery, invoice issuance, or payment.</p></html>",amendment_info=amendment_info,source_info=source_info,issuer=issuer,reference=html(&quote.reference),revision=rev.revision,status=html(status),customer=html(&rev.customer.name),contact=html(&rev.customer.contact),pickup=rev.plan.origin.date,delivery=rev.plan.destination.date,origin=party(&rev.plan.origin),destination=party(&rev.plan.destination),mode=html(&rev.plan.mode),equipment=html(&rev.plan.equipment),weight=rev.plan.weight_lbs,miles=rev.plan.miles,commodity=html(&rev.plan.commodity),currency=currency,price=price,validity=rev.valid_until.to_rfc3339(),terms=html(&rev.terms));
    let result = if rev.plan.intermediate_stops.is_empty() {
        result
    } else {
        result.replace(
            "</html>",
            &format!(
                "{}</html>",
                crate::itinerary::route_document(&rev.plan.intermediate_stops)
            ),
        )
    };
    Ok((
        format!("quote-{}-r{}", quote.reference, rev.revision),
        result,
    ))
}
