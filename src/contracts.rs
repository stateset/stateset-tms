//! Reusable carrier pricing, with immutable terms and separately confirmed capacity.
use crate::{
    domain::{self, Carrier, Customer, Result, Stop, Workspace},
    quotes::{QuoteEvidence, QuotePlan},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LanePoint {
    pub country: String,
    pub state: String,
    pub postal_code: String,
}
impl LanePoint {
    fn normalize(&mut self) -> Result<()> {
        self.country = self.country.trim().to_ascii_uppercase();
        self.state = self.state.trim().to_ascii_uppercase();
        self.postal_code = self
            .postal_code
            .trim()
            .to_ascii_uppercase()
            .replace([' ', '-'], "");
        let postal = self.postal_code.as_bytes();
        if self.state.len() != 2
            || !self.state.bytes().all(|c| c.is_ascii_uppercase())
            || !match self.country.as_str() {
                "US" => [5, 9].contains(&postal.len()) && postal.iter().all(u8::is_ascii_digit),
                "CA" => {
                    postal.len() == 6
                        && postal.iter().enumerate().all(|(i, c)| {
                            if i % 2 == 0 {
                                c.is_ascii_uppercase()
                            } else {
                                c.is_ascii_digit()
                            }
                        })
                }
                _ => false,
            }
        {
            return Err("Contract lanes require US ZIP5/ZIP9 or Canadian postal codes, two-letter country and state/province".into());
        }
        Ok(())
    }
    fn matches(&self, stop: &Stop) -> bool {
        let mut p = Self {
            country: stop.details.country.clone(),
            state: stop.state.clone(),
            postal_code: stop.details.postal_code.clone(),
        };
        p.normalize().is_ok() && &p == self
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateTerms {
    pub customer_id: Option<String>,
    pub origin: LanePoint,
    pub destination: LanePoint,
    pub equipment: String,
    pub max_weight_lbs: u32,
    pub max_miles: u32,
    pub effective_from: NaiveDate,
    pub effective_through: NaiveDate,
    pub amount_cents: i64,
    #[serde(
        default = "domain::default_currency",
        skip_serializing_if = "domain::is_default_currency"
    )]
    pub currency: String,
    pub terms: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractRevision {
    pub agreement_id: String,
    pub revision: u64,
    pub number: String,
    pub carrier: Carrier,
    pub customer: Option<Customer>,
    pub rate: RateTerms,
    pub evidence: QuoteEvidence,
    pub reason: String,
    pub basis_sha256: String,
    pub html: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CarrierContract {
    pub id: String,
    pub number: String,
    pub carrier_id: String,
    pub version: u64,
    pub revisions: Vec<ContractRevision>,
    pub withdrawal: Option<QuoteEvidence>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateSource {
    pub agreement_id: String,
    pub revision: u64,
    pub basis_sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RateInput {
    expected_version: Option<u64>,
    number: String,
    carrier_id: String,
    rate: RateTerms,
    contact: String,
    reference: String,
    note: String,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Withdraw {
    expected_version: u64,
    contact: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInput {
    expected_version: u64,
    agreement_id: String,
    expected_rate_version: u64,
    revision_sha256: String,
    expected_load_version: Option<u64>,
    reason: Option<String>,
    valid_until: DateTime<Utc>,
    contact: String,
    reference: String,
    note: String,
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| "Invalid carrier contract fields".into())
}
fn check(actual: u64, expected: u64) -> Result<()> {
    if actual != expected {
        Err("Carrier contract changed; refresh and review current terms".into())
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
    for (v, name) in [
        (&contact, "Actual carrier contact"),
        (&reference, "Original agreement reference"),
        (&note, "Agreement findings"),
    ] {
        domain::required(v, name)?;
    }
    Ok(QuoteEvidence {
        contact: contact.trim().into(),
        reference: reference.trim().into(),
        note: note.trim().into(),
        recorded_at: Utc::now(),
        recorded_by: actor.into(),
    })
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn render(r: &ContractRevision) -> String {
    let lane = |p: &LanePoint| escape(&format!("{} {} {}", p.country, p.state, p.postal_code));
    let price = stateset_freight_commerce::parse_currency(&r.rate.currency).map_or_else(
        |_| stateset_freight_commerce::format_usd(r.rate.amount_cents),
        |currency| stateset_freight_commerce::format_money(r.rate.amount_cents, currency),
    );
    format!("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Recorded carrier agreement {}</title><style>body{{font:16px system-ui;max-width:850px;margin:32px auto;padding:24px;color:#18332d}}p{{line-height:1.7;white-space:pre-wrap}}</style><h1>Recorded carrier lane agreement</h1><p>{} · Revision {}</p><p>Carrier: {} · {}\nCustomer scope: {}</p><p>Origin: {}\nDestination: {}\nFTL · {} · Maximum {} lbs / {} miles\nPickup and delivery within {} through {} inclusive</p><h2>Inclusive price: {} {}</h2><p>{}</p><h2>Staff-attested source evidence</h2><p>Contact: {}\nReference: {}\nFindings: {}\nRecorded: {} by {}\nReason: {}</p><p>This record retains reviewed external agreement terms. It is not a carrier signature, capacity reservation, tender acceptance, shipment, invoice approval, or payment. Each use requires fresh carrier capacity confirmation. Later revisions or withdrawal do not rewrite a confirmed offer or booked shipment.</p></html>",escape(&r.number),escape(&r.number),r.revision,escape(&r.carrier.name),escape(&r.carrier.scac),escape(r.customer.as_ref().map_or("All customers",|c|&c.name)),lane(&r.rate.origin),lane(&r.rate.destination),escape(&r.rate.equipment),r.rate.max_weight_lbs,r.rate.max_miles,r.rate.effective_from,r.rate.effective_through,escape(&r.rate.currency),price,escape(&r.rate.terms),escape(&r.evidence.contact),escape(&r.evidence.reference),escape(&r.evidence.note),r.evidence.recorded_at.to_rfc3339(),escape(&r.evidence.recorded_by),escape(&r.reason))
}
impl ContractRevision {
    fn basis(&self) -> Value {
        json!({"agreement_id":self.agreement_id,"revision":self.revision,"number":self.number,"carrier":self.carrier,"customer":self.customer,"rate":self.rate,"evidence":self.evidence,"reason":self.reason})
    }
    fn digest(&self) -> String {
        crate::documents::sha256(self.basis().to_string().as_bytes())
    }
    pub fn verify(&self) -> Result<()> {
        if self.digest() != self.basis_sha256
            || render(self) != self.html
            || crate::documents::sha256(self.html.as_bytes()) != self.sha256
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
    fn plan_reasons(&self, p: &QuotePlan) -> Vec<&'static str> {
        let mut reasons = vec![];
        if !p.intermediate_stops.is_empty() {
            reasons.push("This two-stop lane agreement does not price intermediate stops; obtain an explicit full-route carrier offer");
        }
        if p.mode != "FTL" {
            reasons.push("Full truckload only");
        }
        if self.rate.equipment != p.equipment {
            reasons.push("Equipment does not match");
        }
        if self
            .rate
            .customer_id
            .as_ref()
            .is_some_and(|id| id != &p.customer_id)
        {
            reasons.push("Customer scope does not match");
        }
        if !self.rate.origin.matches(&p.origin) || !self.rate.destination.matches(&p.destination) {
            reasons.push("Exact directed postal lane does not match");
        }
        if p.origin.date < self.rate.effective_from
            || p.destination.date > self.rate.effective_through
            || p.origin.date > p.destination.date
        {
            reasons.push("Pickup and delivery are outside the agreement period");
        }
        if p.weight_lbs > self.rate.max_weight_lbs || p.miles > self.rate.max_miles {
            reasons.push("Shipment exceeds agreed weight or mileage");
        }
        if self.rate.currency != p.currency {
            reasons.push("Currency does not match");
        }
        reasons
    }
}
impl CarrierContract {
    pub fn current(&self) -> Result<&ContractRevision> {
        let r = self.revisions.last().ok_or("Internal storage error")?;
        if r.agreement_id != self.id || r.number != self.number || r.carrier.id != self.carrier_id {
            return Err("Internal storage error".into());
        }
        Ok(r)
    }
}
impl Workspace {
    pub fn contract_candidates(&self, id: &str) -> Result<Value> {
        let q = self
            .quotes
            .iter()
            .find(|q| q.id == id)
            .ok_or("Quote not found")?;
        let plan = &q.current()?.plan;
        let rates=self.carrier_contracts.iter().map(|c| {
            let r=c.current()?;r.verify()?;
            let mut reasons=r.plan_reasons(plan);
            if c.withdrawal.is_some() {reasons.push("Agreement withdrawn for new sourcing");}
            let replacement=q.status=="booked" && self.loads.iter().any(|l|Some(&l.id)==q.booked_load_id.as_ref() && l.status==crate::domain::Status::Planned && l.carrier_id.is_none() && l.tender_id.is_none());
            if q.status!="draft" && !replacement {reasons.push("Requires a draft quote or safely unassigned booked shipment");}
            if r.rate.effective_through<Utc::now().date_naive() {reasons.push("Agreement expired");}
            let carrier=self.carriers.iter().find(|v|v.id==c.carrier_id);
            if carrier.is_none_or(|v|v.scac!=r.carrier.scac || v.partner_id!=r.carrier.partner_id || v.equipment!=plan.equipment || v.insurance_expires<plan.destination.date.max(Utc::now().date_naive())) {reasons.push("Current carrier identity, equipment or insurance does not qualify");}
            Ok(json!({"id":c.id,"number":c.number,"version":c.version,"revision":r.revision,"basis_sha256":r.basis_sha256,"document_sha256":r.sha256,"carrier_name":r.carrier.name,"amount_cents":r.rate.amount_cents,"rate":r.rate,"eligible":reasons.is_empty(),"reasons":reasons}))
        }).collect::<Result<Vec<_>>>()?;
        let load = q
            .booked_load_id
            .as_ref()
            .and_then(|id| self.loads.iter().find(|l| &l.id == id));
        Ok(
            json!({"quote_id":q.id,"quote_version":q.version,"kind":if q.status=="booked" {"replacement"} else {"offer"},"load_version":load.map(|l|l.version),"rates":rates}),
        )
    }
    pub(crate) fn verify_contract_source(
        &self,
        source: &RateSource,
        p: &QuotePlan,
        carrier: &Carrier,
        amount: i64,
    ) -> Result<()> {
        let c = self
            .carrier_contracts
            .iter()
            .find(|c| c.id == source.agreement_id)
            .ok_or("Internal storage error")?;
        let r = c
            .revisions
            .iter()
            .find(|r| r.revision == source.revision)
            .ok_or("Internal storage error")?;
        r.verify()?;
        if r.basis_sha256 != source.basis_sha256
            || r.agreement_id != c.id
            || c.carrier_id != carrier.id
            || r.carrier.scac != carrier.scac
            || r.carrier.partner_id != carrier.partner_id
            || r.rate.amount_cents != amount
            || !r.plan_reasons(p).is_empty()
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
    pub fn apply_contract(
        &mut self,
        operation: &str,
        id: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "contract.withdraw" {
            let input: Withdraw = decode(body)?;
            let c = self
                .carrier_contracts
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or("Carrier contract not found")?;
            check(c.version, input.expected_version)?;
            c.current()?.verify()?;
            if c.withdrawal.is_some() {
                return Err("Carrier contract already withdrawn".into());
            }
            c.withdrawal = Some(evidence(input.contact, input.reference, input.note, actor)?);
            c.version = c
                .version
                .checked_add(1)
                .ok_or("Carrier contract version exhausted")?;
            let result = json!(c);
            self.record(id,operation,"Carrier agreement withdrawn for new sourcing; original revisions and confirmed offers retained");
            return Ok(result);
        }
        if !["contract.create", "contract.revise"].contains(&operation) {
            return Err("Unsupported carrier contract operation".into());
        }
        let mut input: RateInput = decode(body)?;
        domain::required(&input.number, "Agreement number")?;
        domain::required(&input.reason, "Agreement revision reason")?;
        let carrier = self
            .carriers
            .iter()
            .find(|c| c.id == input.carrier_id)
            .ok_or("Carrier not found")?
            .clone();
        let customer = if let Some(id) = &input.rate.customer_id {
            Some(
                self.customers
                    .iter()
                    .find(|c| &c.id == id)
                    .ok_or("Customer not found")?
                    .clone(),
            )
        } else {
            None
        };
        input.rate.origin.normalize()?;
        input.rate.destination.normalize()?;
        input.rate.currency = domain::money_in(input.rate.amount_cents, &input.rate.currency)?;
        if input.rate.amount_cents == 0
            || input.rate.max_weight_lbs == 0
            || input.rate.max_weight_lbs > 80000
            || input.rate.max_miles == 0
            || input.rate.max_miles > 10000
            || !["Dry van", "Reefer", "Flatbed"].contains(&input.rate.equipment.as_str())
            || carrier.equipment != input.rate.equipment
        {
            return Err("Contract requires positive inclusive minor units, supported equipment, weight 1–80000 lbs and distance 1–10000 miles".into());
        }
        if input.rate.effective_from > input.rate.effective_through
            || input.rate.effective_through < Utc::now().date_naive()
            || input.rate.effective_through > Utc::now().date_naive() + chrono::Duration::days(1096)
        {
            return Err(
                "Agreement period must end today or later and within three years, after its start"
                    .into(),
            );
        }
        if input.rate.terms.trim().is_empty() || input.rate.terms.len() > 2000 {
            return Err("Inclusive rate terms must contain 1–2000 characters".into());
        }
        input.rate.terms = input.rate.terms.trim().into();
        let index = self.carrier_contracts.iter().position(|c| c.id == id);
        let revision = if operation == "contract.revise" {
            let c = &self.carrier_contracts[index.ok_or("Carrier contract not found")?];
            check(
                c.version,
                input
                    .expected_version
                    .ok_or("Expected carrier contract version required")?,
            )?;
            c.current()?.verify()?;
            if c.withdrawal.is_some() {
                return Err(
                    "Withdrawn carrier contract cannot be revised; record a new agreement".into(),
                );
            }
            if c.carrier_id != input.carrier_id || c.number != input.number.trim() {
                return Err("Agreement carrier and number are fixed; record a new agreement for a different identity".into());
            }
            c.current()?
                .revision
                .checked_add(1)
                .ok_or("Contract revision exhausted")?
        } else {
            if input.expected_version.is_some() {
                return Err("New agreement must omit expected_version".into());
            }
            if self.carrier_contracts.iter().any(|c| {
                c.carrier_id == input.carrier_id
                    && c.number.eq_ignore_ascii_case(input.number.trim())
            }) {
                return Err(
                    "Carrier agreement number already exists; use its revision history".into(),
                );
            }
            1
        };
        let contract_id = if operation == "contract.create" {
            domain::id("contract")
        } else {
            id.to_string()
        };
        let mut r = ContractRevision {
            agreement_id: contract_id.clone(),
            revision,
            number: input.number.trim().into(),
            carrier,
            customer,
            rate: input.rate,
            evidence: evidence(input.contact, input.reference, input.note, actor)?,
            reason: input.reason.trim().into(),
            basis_sha256: String::new(),
            html: String::new(),
            sha256: String::new(),
        };
        r.basis_sha256 = r.digest();
        r.html = render(&r);
        r.sha256 = crate::documents::sha256(r.html.as_bytes());
        r.verify()?;
        let c = if operation == "contract.create" {
            self.carrier_contracts.push(CarrierContract {
                id: contract_id,
                number: r.number.clone(),
                carrier_id: input.carrier_id,
                version: 1,
                revisions: vec![r],
                withdrawal: None,
            });
            self.carrier_contracts
                .last()
                .ok_or("Internal storage error")?
        } else {
            let c = &mut self.carrier_contracts[index.ok_or("Carrier contract not found")?];
            c.revisions.push(r);
            c.version = c
                .version
                .checked_add(1)
                .ok_or("Contract version exhausted")?;
            c
        };
        let result = json!(c);
        let id = c.id.clone();
        self.record(&id,operation,"Reviewed inclusive carrier lane rate recorded; no capacity, shipment, financial obligation or external delivery invented");
        Ok(result)
    }
    pub fn apply_contract_offer(
        &mut self,
        operation: &str,
        id: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let input: SourceInput = decode(body)?;
        let q = self
            .quotes
            .iter()
            .find(|q| q.id == id)
            .ok_or("Quote not found")?;
        if q.version != input.expected_version {
            return Err("Quote changed; refresh before sourcing".into());
        }
        let c = self
            .carrier_contracts
            .iter()
            .find(|c| c.id == input.agreement_id)
            .ok_or("Carrier contract not found")?;
        check(c.version, input.expected_rate_version)?;
        let r = c.current()?;
        r.verify()?;
        if r.basis_sha256 != input.revision_sha256 {
            return Err("Carrier contract terms changed; inspect the original revision".into());
        }
        if c.withdrawal.is_some()
            || r.rate.effective_through < Utc::now().date_naive()
            || !r.plan_reasons(&q.current()?.plan).is_empty()
        {
            return Err("Carrier contract is not applicable to this exact shipment".into());
        }
        let source = RateSource {
            agreement_id: c.id.clone(),
            revision: r.revision,
            basis_sha256: r.basis_sha256.clone(),
        };
        if self
            .carriers
            .iter()
            .find(|v| v.id == c.carrier_id)
            .is_none_or(|v| v.scac != r.carrier.scac || v.partner_id != r.carrier.partner_id)
        {
            return Err(
                "Carrier identity changed; revise the agreement before new sourcing".into(),
            );
        }
        let mut body = json!({"expected_version":input.expected_version,"carrier_id":c.carrier_id,"amount_cents":r.rate.amount_cents,"valid_until":input.valid_until,"contact":input.contact,"reference":input.reference,"note":input.note});
        let replacement = operation == "quote.contract-reprocure";
        if replacement {
            body["expected_load_version"] = json!(input
                .expected_load_version
                .ok_or("Replacement load version required")?);
            body["reason"] = json!(input.reason.ok_or("Replacement reason required")?);
        } else if input.expected_load_version.is_some() || input.reason.is_some() {
            return Err("Replacement fields cannot be supplied for a draft quote offer".into());
        }
        self.apply_quote(
            if replacement {
                "quote.reprocure"
            } else {
                "quote.offer"
            },
            id,
            body,
            actor,
        )?;
        let q = self
            .quotes
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or("Quote not found")?;
        let offer = if replacement {
            &mut q
                .procurements
                .last_mut()
                .ok_or("Internal storage error")?
                .offer
        } else {
            q.revisions
                .last_mut()
                .ok_or("Internal storage error")?
                .offers
                .last_mut()
                .ok_or("Internal storage error")?
        };
        offer.rate_source = Some(source);
        let response = json!(q);
        self.record(id,operation,"Exact contract revision price sourced with fresh carrier capacity confirmation; original agreement retained");
        Ok(response)
    }
}
