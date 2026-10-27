use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, String>;
pub fn id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}
pub fn required(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 200 {
        return Err(format!("{field} must contain 1–200 characters"));
    }
    Ok(())
}
pub fn money(value: i64) -> Result<()> {
    let exact = stateset_freight_commerce::usd_from_minor(value);
    stateset_freight_commerce::usd_to_minor(exact)?;
    if !(0..=100_000_000).contains(&value) {
        return Err("Amount must be between 0 and 100,000,000 cents".into());
    }
    Ok(())
}
/// Validate a minor-unit amount in an explicit currency, returning the
/// normalized (uppercase) currency code for storage.
pub fn money_in(value: i64, currency: &str) -> Result<String> {
    let code = stateset_freight_commerce::parse_currency(currency)?;
    let exact = stateset_freight_commerce::from_minor(value, code);
    stateset_freight_commerce::to_minor(exact)?;
    if !(0..=100_000_000).contains(&value) {
        return Err("Amount must be between 0 and 100,000,000 minor units".into());
    }
    Ok(code.to_string())
}
/// Normalize a stored or supplied currency code, rejecting malformed values.
pub fn currency_code(currency: &str) -> Result<String> {
    Ok(stateset_freight_commerce::parse_currency(currency)?.to_string())
}
pub fn default_currency() -> String {
    "USD".into()
}
/// USD serializes as the historical absence of a currency field so stored
/// USD records keep their exact legacy bytes and digests.
pub fn is_default_currency(currency: &String) -> bool {
    currency == "USD"
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Planned,
    Tendered,
    Accepted,
    Dispatched,
    InTransit,
    Delivered,
    Cancelled,
}
impl Status {
    pub fn can_move_to(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Tendered | Self::Cancelled)
                | (
                    Self::Tendered,
                    Self::Accepted | Self::Planned | Self::Cancelled
                )
                | (Self::Accepted, Self::Dispatched | Self::Cancelled)
                | (Self::Dispatched, Self::InTransit)
                | (Self::InTransit, Self::Delivered)
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Customer {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub contact: String,
    #[serde(default)]
    pub billing_address: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Carrier {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub scac: String,
    pub mc_number: String,
    pub insurance_expires: NaiveDate,
    pub equipment: String,
    pub partner_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Driver {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub phone: String,
    pub license_expires: NaiveDate,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Equipment {
    #[serde(default)]
    pub id: String,
    pub unit: String,
    pub kind: String,
    pub plate: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stop {
    pub facility: String,
    pub city: String,
    pub state: String,
    pub date: NaiveDate,
    #[serde(default, flatten)]
    pub details: crate::stops::StopDetails,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Load {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub id: String,
    pub reference: String,
    pub customer_id: String,
    pub mode: String,
    pub equipment: String,
    pub commodity: String,
    pub weight_lbs: u32,
    pub miles: u32,
    pub origin: Stop,
    pub destination: Stop,
    pub status: Status,
    pub carrier_id: Option<String>,
    pub driver_id: Option<String>,
    pub equipment_id: Option<String>,
    pub buy_cents: i64,
    pub sell_cents: i64,
    pub currency: String,
    pub location: String,
    pub exception: Option<String>,
    pub pod_reference: Option<String>,
    #[serde(default)]
    pub pod_document_id: Option<String>,
    #[serde(default)]
    pub pod_review: Option<crate::documents::Review>,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_tracking_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub sync_order_id: Option<String>,
    #[serde(default)]
    pub order_number: Option<String>,
    #[serde(default)]
    pub source_order: Option<Value>,
    #[serde(default)]
    pub tender_expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tender_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invoice {
    pub id: String,
    pub number: String,
    pub load_id: String,
    pub amount_cents: i64,
    pub currency: String,
    pub status: String,
    pub variance_cents: i64,
    #[serde(default)]
    pub agreement: Option<crate::charges::ChargeBasis>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settlement {
    pub id: String,
    pub load_id: String,
    pub carrier_id: String,
    pub amount_cents: i64,
    pub status: String,
    #[serde(default = "initial_version")]
    pub version: u64,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}
fn initial_version() -> u64 {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub entity_id: String,
    pub kind: String,
    pub message: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default = "system_actor")]
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
fn system_actor() -> String {
    "system".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outbox {
    pub id: String,
    pub load_id: String,
    pub destination: String,
    pub status: String,
    pub method: String,
    pub path: String,
    pub payload: Value,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub next_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub leased_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub lease: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub response: Option<String>,
    #[serde(default)]
    pub tender_id: Option<String>,
    #[serde(default = "unknown_transmission")]
    pub transmission: String,
    #[serde(default)]
    pub total_attempts: u64,
}
fn unknown_transmission() -> String {
    "unknown".into()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub loads: Vec<Load>,
    pub customers: Vec<Customer>,
    pub carriers: Vec<Carrier>,
    pub drivers: Vec<Driver>,
    pub equipment: Vec<Equipment>,
    pub invoices: Vec<Invoice>,
    pub settlements: Vec<Settlement>,
    pub events: Vec<Event>,
    pub outbox: Vec<Outbox>,
    pub receipts: BTreeMap<String, Receipt>,
    #[serde(default)]
    pub inbox: Vec<crate::inbox::InboxItem>,
    #[serde(default)]
    pub documents: Vec<crate::documents::Document>,
    #[serde(default)]
    pub bills: Vec<crate::finance::CustomerBill>,
    #[serde(default)]
    pub credits: Vec<crate::finance::CreditNote>,
    #[serde(default)]
    pub cash_entries: Vec<crate::finance::CashEntry>,
    #[serde(default)]
    pub journal: Vec<crate::finance::JournalEntry>,
    #[serde(default)]
    pub billing_profile: Option<crate::finance::BillingProfile>,
    #[serde(default)]
    pub tenders: Vec<crate::tenders::TenderAttempt>,
    #[serde(default)]
    pub releases: Vec<crate::tenders::CarrierRelease>,
    #[serde(default)]
    pub tender_incidents: Vec<crate::tenders::TenderIncident>,
    #[serde(default)]
    pub appointments: Vec<crate::stops::Appointment>,
    #[serde(default)]
    pub stop_events: Vec<crate::stops::StopEvent>,
    #[serde(default)]
    pub charges: Vec<crate::charges::Charge>,
    #[serde(default)]
    pub carrier_adjustments: Vec<crate::payables::CarrierAdjustment>,
    #[serde(default)]
    pub carrier_documents: Vec<crate::payables::CarrierDocument>,
    #[serde(default)]
    pub carrier_credit_applications: Vec<crate::carrier_credits::CarrierCreditApplication>,
    #[serde(default)]
    pub cancellation_fees: Vec<crate::cancellations::CancellationFee>,
    #[serde(default)]
    pub quotes: Vec<crate::quotes::Quote>,
    #[serde(default)]
    pub source_checks: Vec<crate::source::SourceCheck>,
    #[serde(default)]
    pub source_reviews: Vec<crate::source::SourceReview>,
    #[serde(default)]
    pub rate_confirmations: Vec<crate::rate_confirmations::RateConfirmation>,
    #[serde(default)]
    pub portal_offers: Vec<crate::portal::PortalOffer>,
    #[serde(default)]
    pub portal_decisions: Vec<crate::portal::PortalDecision>,
    #[serde(default)]
    pub carrier_submissions: Vec<crate::carrier_portal::CarrierSubmission>,
    #[serde(default)]
    pub movement_corrections: Vec<crate::movement::MovementCorrection>,
    #[serde(default)]
    pub operations_cases: crate::operations::OperationsIndex,
    #[serde(default)]
    pub operations_policy: Option<crate::operations::RetainedPolicy>,
    #[serde(default)]
    pub customer_quote_publications: Vec<crate::customer_portal::QuotePublication>,
    #[serde(default)]
    pub customer_quote_decisions: Vec<crate::customer_portal::CustomerDecision>,
    #[serde(default)]
    pub document_shares: Vec<crate::sharing::DocumentShare>,
    #[serde(default)]
    pub release_publications: Vec<crate::release_portal::ReleasePublication>,
    #[serde(default)]
    pub release_responses: Vec<crate::release_portal::ReleaseResponse>,
    #[serde(default)]
    pub carrier_contracts: Vec<crate::contracts::CarrierContract>,
    #[serde(default)]
    pub stop_services: Vec<crate::itinerary::StopService>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub fingerprint: String,
    pub response: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<crate::save_recovery::StoppedRequest>,
}
impl Workspace {
    pub fn record(&mut self, entity: &str, kind: &str, message: impl Into<String>) {
        self.events.push(Event {
            id: id("evt"),
            entity_id: entity.into(),
            kind: kind.into(),
            message: message.into(),
            occurred_at: Utc::now(),
            actor: system_actor(),
            details: None,
        });
    }
    pub fn apply(&mut self, operation: &str, entity: &str, mut body: Value) -> Result<Value> {
        match operation {
            "load.revise" => {
                if self
                    .quotes
                    .iter()
                    .any(|q| q.booked_load_id.as_deref() == Some(entity))
                {
                    return Err("Quoted shipment plan and base rates are bound to its accepted quote; record an explicit commercial amendment before changing them".into());
                }
                let index = self
                    .loads
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Load not found")?;
                let original = self.loads[index].clone();
                if original.status != Status::Planned {
                    return Err("Only planned loads can be revised; contracted shipments require explicit carrier coordination".into());
                }
                if self.has_approved_charges(entity) {
                    return Err(
                        "Reconcile approved charges before revising the freight plan".into(),
                    );
                }
                if self.invoices.iter().any(|v| v.load_id == entity) {
                    return Err("A load with a carrier invoice cannot be replanned".into());
                }
                if body["expected_version"].as_u64() != Some(original.version) {
                    return Err("Load changed; refresh before continuing".into());
                }
                let mut plan = body.get("plan").ok_or("Revised plan required")?.clone();
                // A plan that omits its currency keeps the load's currency
                // rather than silently re-denominating the rates as USD.
                if let Some(fields) = plan.as_object_mut() {
                    fields
                        .entry("currency")
                        .or_insert_with(|| json!(original.currency));
                }
                let mut candidate = self.clone();
                candidate.loads.remove(index);
                candidate.apply("load.create", "", plan)?;
                let mut revised = candidate.loads.pop().ok_or("Revised load missing")?;
                self.check_stop_revision(&original, &revised)?;
                revised.id = original.id.clone();
                revised.created_at = original.created_at;
                revised.version = original.version + 1;
                revised.sync_order_id = original.sync_order_id;
                revised.order_number = original.order_number;
                revised.source_order = original.source_order;
                self.loads[index] = revised.clone();
                self.record(
                    entity,
                    "load.plan_revised",
                    "Lane, requirements, and rates revised before tender",
                );
                Ok(json!(revised))
            }
            "carrier.update" | "customer.update" | "driver.update" | "equipment.update" => {
                macro_rules! revise {
                    ($items:ident,$create:literal) => {{
                        let index = self
                            .$items
                            .iter()
                            .position(|v| v.id == entity)
                            .ok_or("Record not found")?;
                        if body.get("expected") != Some(&json!(self.$items[index])) {
                            return Err("Record changed; refresh before continuing".into());
                        }
                        let mut candidate = self.clone();
                        candidate.$items.remove(index);
                        candidate.apply(
                            $create,
                            "",
                            body.get("data").ok_or("Updated record required")?.clone(),
                        )?;
                        let mut revised = candidate.$items.pop().ok_or("Updated record missing")?;
                        revised.id = entity.into();
                        self.$items[index] = revised.clone();
                        self.record(entity, operation, "Master data updated");
                        Ok(json!(revised))
                    }};
                }
                match operation {
                    "carrier.update" => {
                        let current = self
                            .carriers
                            .iter()
                            .find(|v| v.id == entity)
                            .ok_or("Carrier not found")?;
                        if (self
                            .loads
                            .iter()
                            .any(|v| v.carrier_id.as_deref() == Some(entity))
                            || self.tenders.iter().any(|v| v.carrier_id == entity))
                            && (body["data"]["scac"] != current.scac
                                || body["data"]["partner_id"] != current.partner_id)
                        {
                            return Err("Carrier SCAC and EDI partner identity cannot change after assignment".into());
                        }
                        revise!(carriers, "carrier.create")
                    }
                    "customer.update" => {
                        if body["data"].get("billing_address").is_none() {
                            let current = self
                                .customers
                                .iter()
                                .find(|v| v.id == entity)
                                .ok_or("Customer not found")?;
                            body["data"]["billing_address"] = json!(current.billing_address);
                        }
                        revise!(customers, "customer.create")
                    }
                    "driver.update" => revise!(drivers, "driver.create"),
                    _ => {
                        let unit = self
                            .equipment
                            .iter()
                            .find(|v| v.id == entity)
                            .ok_or("Equipment not found")?;
                        if self.loads.iter().any(|v| {
                            v.equipment_id.as_deref() == Some(entity)
                                && matches!(v.status, Status::Dispatched | Status::InTransit)
                        }) && body["data"]["kind"] != unit.kind
                        {
                            return Err(
                                "Equipment type cannot change while assigned to an active load"
                                    .into(),
                            );
                        }
                        revise!(equipment, "equipment.create")
                    }
                }
            }
            "load.create" => {
                let input: NewLoad = decode(body)?;
                input.validate(self)?;
                let currency = currency_code(&input.currency)?;
                let now = Utc::now();
                let load = Load {
                    intermediate_stops: input.intermediate_stops.clone(),
                    id: id("load"),
                    reference: input.reference.trim().into(),
                    customer_id: input.customer_id,
                    mode: input.mode,
                    equipment: input.equipment,
                    commodity: input.commodity,
                    weight_lbs: input.weight_lbs,
                    miles: input.miles,
                    origin: input.origin,
                    destination: input.destination,
                    status: Status::Planned,
                    carrier_id: None,
                    driver_id: None,
                    equipment_id: None,
                    buy_cents: input.buy_cents,
                    sell_cents: input.sell_cents,
                    currency,
                    location: "Awaiting pickup".into(),
                    exception: None,
                    pod_reference: None,
                    pod_document_id: None,
                    pod_review: None,
                    version: 1,
                    created_at: now,
                    updated_at: now,
                    last_tracking_at: None,
                    sync_order_id: None,
                    order_number: None,
                    source_order: None,
                    tender_expires_at: None,
                    tender_id: None,
                };
                self.record(&load.id, operation, format!("{} created", load.reference));
                self.loads.push(load.clone());
                Ok(json!(load))
            }
            "commerce.tracking" => crate::commerce::enqueue(self, entity, body),
            "sync.import" => {
                let source = body.get("order").ok_or("Source order required")?.clone();
                let order_id = source["id"].as_str().ok_or("Source order ID required")?;
                uuid::Uuid::parse_str(order_id).map_err(|_| "Invalid source order ID")?;
                if self
                    .loads
                    .iter()
                    .any(|v| v.sync_order_id.as_deref() == Some(order_id))
                {
                    return Err("Sync order already linked to a load".into());
                }
                let mut plan = body.get("load").ok_or("Load plan required")?.clone();
                // Freight may be priced in another currency than the goods, so
                // the order's currency only fills a plan that omits one, and a
                // malformed upstream code falls back to the USD default.
                if let (Some(fields), Some(code)) = (
                    plan.as_object_mut(),
                    source["currency"]
                        .as_str()
                        .and_then(|code| currency_code(code).ok()),
                ) {
                    fields.entry("currency").or_insert_with(|| json!(code));
                }
                let created = self.apply("load.create", "", plan)?;
                let item = self
                    .loads
                    .iter_mut()
                    .find(|v| Some(v.id.as_str()) == created["id"].as_str())
                    .ok_or("Load not found")?;
                item.sync_order_id = Some(order_id.into());
                item.order_number = source["orderNumber"].as_str().map(str::to_owned);
                item.source_order = Some(source.clone());
                let entity = item.id.clone();
                self.observe_source(&entity, source.clone(), 0, Utc::now(), "system")?;
                self.record(
                    &entity,
                    "sync.order_imported",
                    format!("Linked sync order {order_id}; source snapshot retained"),
                );
                Ok(json!(self
                    .loads
                    .iter()
                    .find(|v| v.id == entity)
                    .ok_or("Imported load not found")?))
            }
            "load.command" => self.command(entity, decode(body)?),
            "carrier.create" => {
                let mut item: Carrier = decode(body)?;
                required(&item.name, "Carrier name")?;
                required(&item.mc_number, "MC number")?;
                if !["Dry van", "Reefer", "Flatbed"].contains(&item.equipment.as_str()) {
                    return Err("Invalid carrier equipment type".into());
                }
                required(&item.partner_id, "EDI partner ID")?;
                if !item
                    .partner_id
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
                {
                    return Err("Invalid EDI partner ID".into());
                }
                if !(2..=4).contains(&item.scac.len())
                    || !item.scac.bytes().all(|v| v.is_ascii_uppercase())
                {
                    return Err("SCAC must contain 2–4 uppercase letters".into());
                }
                if self
                    .carriers
                    .iter()
                    .any(|v| v.scac == item.scac || v.partner_id == item.partner_id)
                {
                    return Err("Carrier SCAC or partner already exists".into());
                }
                item.id = id("carrier");
                self.record(&item.id, operation, format!("{} added", item.name));
                self.carriers.push(item.clone());
                Ok(json!(item))
            }
            "customer.create" => {
                let mut item: Customer = decode(body)?;
                required(&item.name, "Customer name")?;
                if item.billing_address.len() > 1000 {
                    return Err("Billing address exceeds 1,000 characters".into());
                }
                item.id = id("customer");
                self.record(&item.id, operation, format!("{} added", item.name));
                self.customers.push(item.clone());
                Ok(json!(item))
            }
            "driver.create" => {
                let mut item: Driver = decode(body)?;
                required(&item.name, "Driver name")?;
                required(&item.phone, "Phone")?;
                item.id = id("driver");
                self.record(&item.id, operation, format!("{} added", item.name));
                self.drivers.push(item.clone());
                Ok(json!(item))
            }
            "equipment.create" => {
                let mut item: Equipment = decode(body)?;
                required(&item.unit, "Unit")?;
                required(&item.plate, "Plate")?;
                if !["Dry van", "Reefer", "Flatbed"].contains(&item.kind.as_str()) {
                    return Err("Invalid equipment type".into());
                }
                if self.equipment.iter().any(|v| v.unit == item.unit) {
                    return Err("Unit already exists".into());
                }
                item.id = id("equipment");
                self.record(&item.id, operation, format!("Unit {} added", item.unit));
                self.equipment.push(item.clone());
                Ok(json!(item))
            }
            "invoice.create" => {
                let input: NewInvoice = decode(body)?;
                self.invoice(input)
            }
            "invoice.approve" => {
                let invoice = self
                    .invoices
                    .iter()
                    .find(|v| v.id == entity)
                    .ok_or("Invoice not found")?;
                if invoice.status != "matched" || invoice.variance_cents != 0 {
                    return Err(
                        "Only matched invoices can be approved; resolve the variance first".into(),
                    );
                }
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == invoice.load_id)
                    .ok_or("Load not found")?;
                let agreement = self.charge_basis(load, "buy")?;
                if self.pending_charges(&load.id, "buy") {
                    return Err(
                        "Review pending carrier charges before approving its invoice".into(),
                    );
                }
                if invoice.amount_cents != agreement.total_amount_cents {
                    return Err(
                        "Carrier invoice does not match the current approved agreement".into(),
                    );
                }
                if !self.delivery_evidence_ready(load) {
                    return Err(
                        "Delivery and reviewed proof of delivery are required before approval"
                            .into(),
                    );
                }
                let invoice = self
                    .invoices
                    .iter_mut()
                    .find(|v| v.id == entity)
                    .ok_or("Invoice not found")?;
                invoice.status = "approved".into();
                invoice.agreement = Some(agreement);
                let result = json!(invoice);
                self.record(entity, operation, "Freight invoice approved for payment");
                self.recognize_carrier_invoice(entity)?;
                Ok(result)
            }
            "settlement.create" => {
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == entity)
                    .ok_or("Load not found")?;
                if !self.delivery_evidence_ready(load) {
                    return Err("Delivered load and reviewed proof of delivery required".into());
                }
                if self.settlements.iter().any(|v| v.load_id == entity) {
                    return Err("Settlement already exists".into());
                }
                if !self
                    .invoices
                    .iter()
                    .any(|v| v.load_id == entity && v.status == "approved")
                {
                    return Err("Approve the carrier invoice before preparing settlement".into());
                }
                let approved = self
                    .invoices
                    .iter()
                    .find(|v| v.load_id == entity && v.status == "approved")
                    .ok_or("Approved invoice not found")?;
                let mut item = Settlement {
                    id: id("settlement"),
                    load_id: entity.into(),
                    carrier_id: load.carrier_id.clone().ok_or("Carrier required")?,
                    amount_cents: approved.amount_cents,
                    status: "prepared".into(),
                    version: 1,
                    created_at: Some(Utc::now()),
                };
                self.record(
                    entity,
                    operation,
                    "Carrier settlement prepared; payment has not been sent",
                );
                self.settlements.push(item.clone());
                self.refresh_carrier_settlement(&item.id)?;
                item = self
                    .settlements
                    .iter()
                    .find(|v| v.id == item.id)
                    .ok_or("Settlement not found")?
                    .clone();
                let invoice_id = self
                    .invoices
                    .iter()
                    .find(|v| v.load_id == entity && v.status == "approved")
                    .ok_or("Approved invoice not found")?
                    .id
                    .clone();
                self.recognize_carrier_invoice(&invoice_id)?;
                Ok(json!(item))
            }
            "invoice.resolve" => {
                let expected = body
                    .get("expected_amount_cents")
                    .and_then(Value::as_i64)
                    .ok_or("expected_amount_cents required")?;
                let amount = body
                    .get("amount_cents")
                    .and_then(Value::as_i64)
                    .ok_or("amount_cents required")?;
                money(amount)?;
                let note = body
                    .get("note")
                    .and_then(Value::as_str)
                    .ok_or("Resolution note required")?;
                required(note, "Resolution note")?;
                let invoice = self
                    .invoices
                    .iter()
                    .find(|v| v.id == entity)
                    .ok_or("Invoice not found")?;
                if invoice.status != "exception" || invoice.amount_cents != expected {
                    return Err("Invoice changed; refresh before resolving".into());
                }
                let load = self
                    .loads
                    .iter()
                    .find(|v| v.id == invoice.load_id)
                    .ok_or("Load not found")?;
                let agreement = self.charge_basis(load, "buy")?;
                let invoice = self
                    .invoices
                    .iter_mut()
                    .find(|v| v.id == entity)
                    .ok_or("Invoice not found")?;
                invoice.amount_cents = amount;
                invoice.variance_cents = amount - agreement.total_amount_cents;
                invoice.agreement = Some(agreement);
                invoice.status = if invoice.variance_cents == 0 {
                    "matched"
                } else {
                    "exception"
                }
                .into();
                let result = json!(invoice);
                self.record(
                    entity,
                    "invoice.corrected",
                    format!("Corrected carrier invoice from {expected} to {amount} cents: {note}"),
                );
                Ok(result)
            }
            "outbox.retry" => {
                if self
                    .outbox
                    .iter()
                    .find(|j| j.id == entity)
                    .is_some_and(|j| {
                        j.destination == "stateset-edi"
                            && self.tenders.iter().any(|t| {
                                Some(&t.id) == j.tender_id.as_ref()
                                    && !t.plan.intermediate_stops.is_empty()
                            })
                    })
                {
                    return Err("The EDI adapter cannot encode intermediate stops; coordinate the full route through the native carrier portal".into());
                }
                if let Some(job) = self.outbox.iter().find(|v| v.id == entity) {
                    if job.destination == "stateset-edi"
                        && (self.releases.iter().any(|v| {
                            v.status == "pending" && Some(&v.tender_id) == job.tender_id.as_ref()
                        }) || self.unresolved_tender_incidents(&job.load_id))
                    {
                        return Err(
                            "Carrier coordination is unresolved; tender retry is on hold".into(),
                        );
                    }
                }
                let item = self
                    .outbox
                    .iter_mut()
                    .find(|v| v.id == entity)
                    .ok_or("Outbox item not found")?;
                if !["held", "retry"].contains(&item.status.as_str()) {
                    return Err("Only failed handoffs can be retried".into());
                }
                item.status = "pending_handoff".into();
                item.attempts = 0;
                item.next_attempt_at = None;
                item.lease = None;
                item.leased_until = None;
                let result = json!(item);
                self.record(entity, operation, "Integration handoff queued for retry");
                Ok(result)
            }
            "inbox.retry" => {
                let item = self
                    .inbox
                    .iter_mut()
                    .find(|v| v.id == entity)
                    .ok_or("Inbox item not found")?;
                if !["held", "retry"].contains(&item.status.as_str()) {
                    return Err("Only failed inbound documents can be retried".into());
                }
                item.status = "pending".into();
                item.attempts = 0;
                item.next_attempt_at = None;
                item.lease = None;
                item.leased_until = None;
                item.updated_at = Utc::now();
                let result = json!(item);
                self.record(entity, operation, "Inbound EDI document queued for retry");
                Ok(result)
            }
            "edi.event" => self.ingest(decode(body)?),
            _ => Err("Unknown operation".into()),
        }
    }
    fn invoice(&mut self, input: NewInvoice) -> Result<Value> {
        required(&input.number, "Invoice number")?;
        money(input.amount_cents)?;
        let load = self
            .loads
            .iter()
            .find(|v| v.id == input.load_id)
            .ok_or("Load not found")?;
        if load.carrier_id.is_none() || load.status == Status::Cancelled {
            return Err("An assigned carrier and open or delivered load are required".into());
        }
        if self.cancellation_invoice_number_exists(
            load.carrier_id.as_deref().unwrap_or(""),
            &input.number,
            None,
        ) {
            return Err("Carrier invoice number already records a cancellation obligation".into());
        }
        if self.invoices.iter().any(|invoice| {
            invoice.number == input.number
                && self.loads.iter().any(|existing| {
                    existing.id == invoice.load_id && existing.carrier_id == load.carrier_id
                })
        }) {
            return Err("Carrier invoice number already exists".into());
        }
        if self.invoices.iter().any(|v| v.load_id == load.id) {
            return Err("A carrier invoice already exists for this load".into());
        }
        if input.currency != load.currency {
            return Err("Invoice currency must match load currency".into());
        }
        let agreement = self.charge_basis(load, "buy")?;
        let variance = input.amount_cents - agreement.total_amount_cents;
        let item = Invoice {
            id: id("invoice"),
            number: input.number,
            load_id: input.load_id,
            amount_cents: input.amount_cents,
            currency: input.currency,
            status: if variance == 0 {
                "matched"
            } else {
                "exception"
            }
            .into(),
            variance_cents: variance,
            agreement: Some(agreement),
        };
        self.record(
            &item.load_id,
            "invoice.received",
            format!("Invoice {}: {}", item.number, item.status),
        );
        self.invoices.push(item.clone());
        Ok(json!(item))
    }
    pub fn enqueue_sync(&mut self, load: &Load) {
        // Native iCommerce tracking requires an explicit, versioned shipment review.
        // Never route its order through Sync's automatic order-shipped mutation.
        if load
            .source_order
            .as_ref()
            .is_some_and(|v| v["sourceSystem"] == "icommerce")
        {
            return;
        }
        if let Some(order_id) = &load.sync_order_id {
            let carrier = self
                .carriers
                .iter()
                .find(|v| Some(&v.id) == load.carrier_id.as_ref());
            self.outbox.push(Outbox {id:id("outbox"),load_id:load.id.clone(),destination:"stateset-sync-server".into(),status:"pending_handoff".into(),method:"POST".into(),path:format!("/orders/{order_id}/tracking"),payload:json!({"trackingData":{"trackingNumber":load.reference,"carrier":carrier.map(|v|v.name.as_str()),"estimatedDelivery":load.destination.date.to_string(),"freight_status":load.status,"location":load.location,"pod_reference":load.pod_reference,"freight_version":load.version,"freight_stops":self.stop_tracking(load),"freight_route_complete":self.route_ready(load),"freight_execution_consistent":self.execution_consistent(load),"freight_actual_milestones":self.movement_tracking(load),"freight_route_stops":crate::itinerary::route_stops(load).iter().map(|(id,kind,s)|json!({"id":id,"kind":kind,"city":s.city,"state":s.state,"date":s.date,"completed":self.stop_complete(load,id)})).collect::<Vec<_>>()}}),attempts:0,next_attempt_at:None,leased_until:None,lease:None,last_error:None,response:None,tender_id:None,transmission:"not_started".into(),total_attempts:0});
        }
    }
    fn command(&mut self, entity: &str, input: Command) -> Result<Value> {
        let index = self
            .loads
            .iter()
            .position(|v| v.id == entity)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        if load.version != input.expected_version {
            return Err("Load changed; refresh before continuing".into());
        }
        let today = Utc::now().date_naive();
        if input.action == "dispatch" {
            self.guard_source(&load)?;
        }
        if load.carrier_id.is_some() {
            self.adopt_tender(&mut load)?;
        }
        if input.action == "dispatch"
            && (self.release_pending(&load) || self.unresolved_tender_incidents(entity))
        {
            return Err(
                "Carrier release or custody conflict is unresolved; dispatch is on hold".into(),
            );
        }
        let message: String;
        match input.action.as_str() {
            "tender" => {
                let carrier = self
                    .carriers
                    .iter()
                    .find(|v| Some(&v.id) == input.carrier_id.as_ref())
                    .ok_or("Choose a carrier")?
                    .clone();
                message = self.prepare_tender(&mut load, carrier)?;
            }
            "accept" => {
                move_to(&mut load, Status::Accepted)?;
                load.tender_expires_at = None;
                if load.exception.as_deref()
                    == Some("Tender response overdue; contact carrier before retendering")
                {
                    load.exception = None;
                }
                message = "Carrier acceptance recorded".into();
                self.finish_tender_state(&load, "accepted");
            }
            "decline" => {
                if self.has_approved_charges(entity) {
                    return Err(
                        "Reconcile approved charges before recording carrier decline".into(),
                    );
                }
                if self.invoices.iter().any(|v| v.load_id == entity) {
                    return Err("Reconcile carrier invoice before recording tender decline".into());
                }
                move_to(&mut load, Status::Planned)?;
                self.finish_tender_state(&load, "declined");
                self.close_declined_release(&load);
                self.suppress_tender_jobs(load.tender_id.as_deref());
                load.carrier_id = None;
                load.tender_id = None;
                load.tender_expires_at = None;
                message = "Carrier declined tender; load returned to planning".into();
                if load.exception.as_deref()
                    == Some("Tender response overdue; contact carrier before retendering")
                {
                    load.exception = None;
                }
            }
            "dispatch" => {
                let driver = self
                    .drivers
                    .iter()
                    .find(|v| Some(&v.id) == input.driver_id.as_ref())
                    .ok_or("Choose a driver")?;
                let unit = self
                    .equipment
                    .iter()
                    .find(|v| Some(&v.id) == input.equipment_id.as_ref())
                    .ok_or("Choose equipment")?;
                if driver.license_expires < self.coverage_date(&load).max(today) {
                    return Err("Driver license must cover delivery and appointment dates".into());
                }
                if unit.kind != load.equipment {
                    return Err("Equipment does not match load requirements".into());
                }
                if self.loads.iter().any(|v| {
                    v.id != load.id
                        && matches!(v.status, Status::Dispatched | Status::InTransit)
                        && (v.driver_id.as_ref() == Some(&driver.id)
                            || v.equipment_id.as_ref() == Some(&unit.id))
                }) {
                    return Err("Driver or equipment is assigned to an active load".into());
                }
                let carrier = self
                    .carriers
                    .iter()
                    .find(|v| Some(&v.id) == load.carrier_id.as_ref())
                    .ok_or("Carrier required")?;
                if carrier.insurance_expires < self.coverage_date(&load).max(today) {
                    return Err(
                        "Carrier insurance must cover delivery and appointment dates".into(),
                    );
                }
                move_to(&mut load, Status::Dispatched)?;
                load.driver_id = Some(driver.id.clone());
                load.equipment_id = Some(unit.id.clone());
                message = format!("Dispatched to {} · unit {}", driver.name, unit.unit);
            }
            "pickup" => {
                move_to(&mut load, Status::InTransit)?;
                load.location = format!("{}, {}", load.origin.city, load.origin.state);
                message = "Pickup confirmed; shipment in transit".into();
                self.finish_tender_state(&load, "in_transit");
                let observed = EdiEvent {
                    event_id: id("manual-pickup"),
                    event_type: "operator.pickup".into(),
                    occurred_at: Utc::now(),
                    payload: json!({"load_id":load.id,"tender_id":load.tender_id}),
                };
                self.block_release_after_pickup(&load, &observed);
            }
            "location" => {
                if load.status != Status::InTransit {
                    return Err("Location updates require an in-transit load".into());
                }
                let location = input.location.ok_or("Location required")?;
                required(&location, "Location")?;
                load.location = location.clone();
                load.last_tracking_at = Some(Utc::now());
                message = format!("Location updated: {location}");
            }
            "deliver" => {
                if !self.route_ready(&load) {
                    return Err("Confirm every intermediate stop before final delivery; missing service cannot be invented".into());
                }
                self.set_delivery_evidence(&mut load, input.pod_reference, input.document_id)?;
                move_to(&mut load, Status::Delivered)?;
                load.exception = None;
                load.location = format!("{}, {}", load.destination.city, load.destination.state);
                message = "Delivery confirmed with proof of delivery".into();
                self.finish_tender_state(&load, "delivered");
            }
            "exception" => {
                if matches!(load.status, Status::Delivered | Status::Cancelled) {
                    return Err("Load is already closed".into());
                }
                let note = input.note.ok_or("Exception note required")?;
                required(&note, "Exception note")?;
                load.exception = Some(note.clone());
                message = format!("Exception: {note}");
            }
            "resolve" => {
                if load.exception.is_none() {
                    return Err("No open exception".into());
                }
                load.exception = None;
                message = "Shipment exception resolved".into();
            }
            "cancel" => {
                if self.has_approved_charges(entity) {
                    return Err("Reconcile approved charges before cancellation".into());
                }
                if self.unresolved_tender_incidents(entity) {
                    return Err("Resolve carrier custody incidents before cancellation".into());
                }
                if self.invoices.iter().any(|v| v.load_id == entity)
                    || self
                        .bills
                        .iter()
                        .any(|v| v.load_id == entity && v.status != "void")
                {
                    return Err("Reconcile financial documents before cancellation".into());
                }
                if load.status == Status::Planned {
                    move_to(&mut load, Status::Cancelled)?;
                } else {
                    self.withdraw_tender(&mut load, true)?;
                }
                message = "Load cancelled".into();
            }
            "withdraw_tender" => {
                self.withdraw_tender(&mut load, false)?;
                message = "Proven-unsent tender withdrawn; load returned to planning".into();
            }
            "attach_pod" => {
                if load.status != Status::Delivered {
                    return Err("POD can only be attached after delivery".into());
                }
                self.set_delivery_evidence(&mut load, input.pod_reference, input.document_id)?;
                message = "Proof of delivery attached; finance review required".into();
            }
            _ => return Err("Unknown load action".into()),
        }
        load.version = load
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        load.updated_at = Utc::now();
        self.loads[index] = load.clone();
        if matches!(
            input.action.as_str(),
            "pickup"
                | "location"
                | "deliver"
                | "attach_pod"
                | "cancel"
                | "withdraw_tender"
                | "decline"
        ) {
            self.enqueue_sync(&load);
        }
        self.record(entity, &format!("load.{}", input.action), message);
        Ok(json!(load))
    }
    fn ingest(&mut self, event: EdiEvent) -> Result<Value> {
        required(&event.event_id, "Event ID")?;
        if ![
            "edi.load_tender_response.received",
            "edi.shipment_status.received",
            "edi.freight_invoice.received",
        ]
        .contains(&event.event_type.as_str())
        {
            return Err("Unsupported EDI event type".into());
        }
        let load = match self.correlate_tender_event(&event)? {
            crate::tenders::TenderEvent::Current(load) => *load,
            crate::tenders::TenderEvent::Historical(result) => return Ok(result),
        };
        let mut cmd = Command {
            action: String::new(),
            expected_version: load.version,
            carrier_id: None,
            driver_id: None,
            equipment_id: None,
            location: None,
            note: None,
            pod_reference: None,
            document_id: None,
        };
        let result = match event.event_type.as_str() {
            "edi.load_tender_response.received" => {
                if !load.intermediate_stops.is_empty()
                    && load.status == Status::Tendered
                    && event.payload["accepted"] == true
                {
                    return Err("A 990 cannot establish acceptance of omitted intermediate stops; obtain actual full-route carrier acceptance through the native document workflow".into());
                }
                cmd.action = match event.payload.get("accepted").and_then(Value::as_bool) {
                    Some(true) => "accept",
                    Some(false) => "decline",
                    None => return Err("accepted must be a boolean".into()),
                }
                .into();
                let result = if cmd.action == "accept"
                    && matches!(
                        load.status,
                        Status::Accepted
                            | Status::Dispatched
                            | Status::InTransit
                            | Status::Delivered
                    ) {
                    json!(load)
                } else {
                    self.command(&load.id, cmd)?
                };
                for job in &mut self.outbox {
                    if job.load_id == load.id
                        && job.destination == "stateset-edi"
                        && job.tender_id == load.tender_id
                        && job.status != "superseded"
                    {
                        job.status = "carrier_responded".into();
                        job.lease = None;
                        job.leased_until = None;
                    }
                }
                result
            }
            "edi.shipment_status.received" => self.apply_tracking(&load, &event)?,
            "edi.freight_invoice.received" => {
                let number = event
                    .payload
                    .get("invoice_number")
                    .and_then(Value::as_str)
                    .ok_or("invoice_number required")?;
                let amount = event
                    .payload
                    .get("net_amount")
                    .ok_or("net_amount required")?;
                let raw = amount
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| amount.to_string());
                let cents = if let Some(unsigned) = raw.strip_prefix('-') {
                    -decimal_cents(unsigned)?
                } else {
                    decimal_cents(&raw)?
                };
                let currency = event
                    .payload
                    .get("currency")
                    .and_then(Value::as_str)
                    .ok_or("currency required")?;
                if cents < 0 || self.invoices.iter().any(|v| v.load_id == load.id) {
                    self.receive_additional_carrier_document(
                        &load, &event, number, cents, currency,
                    )?
                } else {
                    self.invoice(NewInvoice {
                        number: number.into(),
                        load_id: load.id.clone(),
                        amount_cents: cents,
                        currency: currency.into(),
                    })?
                }
            }
            _ => return Err("Unsupported EDI event type".into()),
        };
        self.record(
            &load.id,
            &event.event_type,
            if result["finance_review"]==true {format!("EDI invoice {} retained for finance classification; no additional payable posted",event.event_id)}else{format!("EDI event {} applied", event.event_id)},
        );
        Ok(result)
    }
    pub(crate) fn apply_tracking(&mut self, original: &Load, event: &EdiEvent) -> Result<Value> {
        let result = self.apply_tracking_facts(original, event)?;
        if result["ignored"] != true {
            let load = self
                .loads
                .iter()
                .find(|l| l.id == original.id)
                .ok_or("Load not found")?
                .clone();
            self.enqueue_sync(&load);
        }
        Ok(result)
    }
    pub(crate) fn apply_tracking_facts(
        &mut self,
        original: &Load,
        event: &EdiEvent,
    ) -> Result<Value> {
        let latest = event.payload["latest_status"]
            .as_str()
            .ok_or("latest_status required")?;
        let fallback = vec![json!({"status_code":latest})];
        let statuses = match event.payload.get("events") {
            Some(Value::Array(items)) if !items.is_empty() => items,
            Some(_) => return Err("214 events must be a non-empty array".into()),
            None => &fallback,
        };
        if statuses.last().and_then(|v| v["status_code"].as_str()) != Some(latest) {
            return Err("latest_status does not match final 214 status event".into());
        }
        let mut load = original.clone();
        for observation in statuses {
            let status = observation["status_code"]
                .as_str()
                .ok_or("214 status_code required")?;
            let update_location=match status {
                "AF" if matches!(load.status,Status::Accepted|Status::Dispatched) => {load.status=Status::InTransit;true}
                "AF" if matches!(load.status,Status::InTransit|Status::Delivered) => false,
                "D1" if matches!(load.status,Status::InTransit|Status::Delivered) => {
                    load.status=Status::Delivered;load.exception=None;
                    load.location=format!("{}, {}",load.destination.city,load.destination.state);true
                }
                "X6"|"X1" if load.status==Status::InTransit => true,
                "AG" if matches!(load.status,Status::Accepted|Status::Dispatched|Status::InTransit|Status::Delivered) => false,
                "X6"|"X1" if load.status==Status::Delivered => false,
                _ => return Err(format!("Cannot apply 214 status {status} while shipment is {:?}; reconcile earlier lifecycle evidence and retry",load.status)),
            };
            if update_location {
                if let Some(city) = observation["city"]
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                {
                    required(city, "Tracking city")?;
                    load.location = match observation["state"].as_str().filter(|v| !v.is_empty()) {
                        Some(state) => format!("{city}, {state}"),
                        None => city.into(),
                    };
                }
            }
        }
        let stale = original
            .last_tracking_at
            .is_some_and(|v| event.occurred_at <= v);
        if stale && load.status == original.status {
            self.record(
                &original.id,
                "tracking.stale_ignored",
                format!(
                    "Older tracking event {} retained without changing shipment state",
                    event.event_id
                ),
            );
            return Ok(json!({"ignored":true,"reason":"stale_tracking","load_id":original.id}));
        }
        load.last_tracking_at = Some(
            original
                .last_tracking_at
                .map_or(event.occurred_at, |v| v.max(event.occurred_at)),
        );
        load.version = load
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        load.updated_at = Utc::now();
        let index = self
            .loads
            .iter()
            .position(|v| v.id == load.id)
            .ok_or("Load not found")?;
        self.loads[index] = load.clone();
        if matches!(load.status, Status::InTransit | Status::Delivered) {
            self.finish_tender_state(
                &load,
                if load.status == Status::Delivered {
                    "delivered"
                } else {
                    "in_transit"
                },
            );
            self.block_release_after_pickup(&load, event);
        }
        if stale {
            self.record(
                &load.id,
                "tracking.late_progress",
                format!(
                    "Delayed tracking event {} advanced shipment lifecycle; tracking watermark retained",
                    event.event_id
                ),
            );
        }
        for (index, observation) in statuses.iter().enumerate() {
            self.record(
                &load.id,
                "tracking.observation",
                format!(
                    "Carrier status {} · event {} · observation {}",
                    observation["status_code"].as_str().unwrap_or("unknown"),
                    event.event_id,
                    index + 1
                ),
            );
            if let Some(entry) = self.events.last_mut() {
                entry.details = Some(
                    json!({"event_id":event.event_id,"source_occurred_at":event.occurred_at,"observation":observation}),
                );
            }
        }
        Ok(json!(load))
    }
}
fn move_to(load: &mut Load, status: Status) -> Result<()> {
    if !load.status.can_move_to(&status) {
        return Err(format!(
            "Cannot move from {:?} to {:?}",
            load.status, status
        ));
    }
    load.status = status;
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(body: Value) -> Result<T> {
    serde_json::from_value(body).map_err(|v| format!("Invalid request: {v}"))
}
pub fn decimal_cents(value: &str) -> Result<i64> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() > 2 || parts[0].is_empty() || !parts[0].bytes().all(|v| v.is_ascii_digit()) {
        return Err("Invalid decimal amount".into());
    }
    let fraction = parts.get(1).copied().unwrap_or("");
    if fraction.len() > 2 || !fraction.bytes().all(|v| v.is_ascii_digit()) {
        return Err("Amount must have at most two decimal places".into());
    }
    let dollars: i64 = parts[0].parse().map_err(|_| "Amount too large")?;
    let pennies: i64 = format!("{fraction:0<2}")
        .parse()
        .map_err(|_| "Invalid cents")?;
    let result = dollars
        .checked_mul(100)
        .and_then(|v| v.checked_add(pennies))
        .ok_or("Amount too large")?;
    money(result)?;
    Ok(result)
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLoad {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intermediate_stops: Vec<crate::itinerary::IntermediateStop>,
    pub reference: String,
    pub customer_id: String,
    pub mode: String,
    pub equipment: String,
    pub commodity: String,
    pub weight_lbs: u32,
    pub miles: u32,
    pub origin: Stop,
    pub destination: Stop,
    pub buy_cents: i64,
    pub sell_cents: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
}
impl NewLoad {
    pub(crate) fn validate(&self, workspace: &Workspace) -> Result<()> {
        crate::itinerary::validate_route(
            &self.origin,
            &self.intermediate_stops,
            &self.destination,
            false,
        )?;
        required(&self.reference, "Reference")?;
        required(&self.commodity, "Commodity")?;
        if !self
            .reference
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
        {
            return Err("Reference may contain letters, numbers, hyphens, and underscores".into());
        }
        if workspace
            .loads
            .iter()
            .any(|v| v.reference == self.reference.trim())
        {
            return Err("Load reference already exists".into());
        }
        if !workspace.customers.iter().any(|v| v.id == self.customer_id) {
            return Err("Customer not found".into());
        }
        if !["FTL", "LTL"].contains(&self.mode.as_str()) {
            return Err("Mode must be FTL or LTL".into());
        }
        if !["Dry van", "Reefer", "Flatbed"].contains(&self.equipment.as_str()) {
            return Err("Invalid equipment type".into());
        }
        if !(1..=80_000).contains(&self.weight_lbs) || !(1..=10_000).contains(&self.miles) {
            return Err("Weight or distance outside supported range".into());
        }
        for stop in [&self.origin, &self.destination] {
            crate::stops::validate_stop(stop, false)?;
        }
        if self.destination.date < self.origin.date {
            return Err("Delivery cannot precede pickup".into());
        }
        money_in(self.buy_cents, &self.currency)?;
        money_in(self.sell_cents, &self.currency)?;
        Ok(())
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub action: String,
    pub expected_version: u64,
    pub carrier_id: Option<String>,
    pub driver_id: Option<String>,
    pub equipment_id: Option<String>,
    pub location: Option<String>,
    pub note: Option<String>,
    pub pod_reference: Option<String>,
    pub document_id: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewInvoice {
    pub number: String,
    pub load_id: String,
    pub amount_cents: i64,
    pub currency: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EdiEvent {
    pub event_id: String,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

pub fn demo() -> Workspace {
    let today = Utc::now().date_naive();
    let expiry = today + chrono::Duration::days(365);
    let mut w = Workspace {
        billing_profile: Some(crate::finance::BillingProfile {
            legal_name: "Synthetic Demo Freight LLC".into(),
            address: "200 Sample Road, Demo City, OR 97000, US".into(),
            contact: "billing@example.test".into(),
            payment_instructions: "Synthetic demo only. No bank payment instructions.".into(),
            version: 1,
        }),
        ..Workspace::default()
    };
    for (key, name, contact) in [
        ("c1", "Northstar Retail", "logistics@northstar.example"),
        ("c2", "Evergreen Foods", "shipping@evergreen.example"),
        ("c3", "Meridian Supply", "ops@meridian.example"),
    ] {
        w.customers.push(Customer {
            id: key.into(),
            name: name.into(),
            contact: contact.into(),
            billing_address: "100 Synthetic Avenue, Demo City, OR 97000, US".into(),
        });
    }
    for (key, name, scac, mc, eq) in [
        ("ca1", "Summit Transport", "SMTT", "MC-482190", "Dry van"),
        (
            "ca2",
            "Pacific Coast Logistics",
            "PCLG",
            "MC-731204",
            "Reefer",
        ),
        ("ca3", "Atlas Freight Lines", "ATFL", "MC-593821", "Flatbed"),
        ("ca4", "Blue Ridge Express", "BREX", "MC-329401", "Dry van"),
    ] {
        w.carriers.push(Carrier {
            id: key.into(),
            name: name.into(),
            scac: scac.into(),
            mc_number: mc.into(),
            insurance_expires: expiry,
            equipment: eq.into(),
            partner_id: scac.to_lowercase(),
        });
    }
    for (key, name, phone) in [
        ("d1", "Alex Morgan", "+1 503 555 0104"),
        ("d2", "Jordan Chen", "+1 206 555 0118"),
        ("d3", "Sam Rivera", "+1 415 555 0132"),
    ] {
        w.drivers.push(Driver {
            id: key.into(),
            name: name.into(),
            phone: phone.into(),
            license_expires: expiry,
        });
    }
    for (key, unit, kind, plate) in [
        ("e1", "TRK-104", "Dry van", "OR 48219"),
        ("e2", "TRK-208", "Reefer", "WA 73120"),
        ("e3", "TRK-316", "Flatbed", "CA 59382"),
    ] {
        w.equipment.push(Equipment {
            id: key.into(),
            unit: unit.into(),
            kind: kind.into(),
            plate: plate.into(),
        });
    }
    let lanes = [
        (
            "FR-1048",
            "c1",
            "Portland",
            "OR",
            "Seattle",
            "WA",
            "Dry van",
            "ca1",
            Status::InTransit,
            173,
            125000,
            156000,
        ),
        (
            "FR-1049",
            "c2",
            "Sacramento",
            "CA",
            "Portland",
            "OR",
            "Reefer",
            "ca2",
            Status::InTransit,
            580,
            218000,
            264000,
        ),
        (
            "FR-1050",
            "c3",
            "Denver",
            "CO",
            "Phoenix",
            "AZ",
            "Flatbed",
            "ca3",
            Status::Tendered,
            865,
            275000,
            328000,
        ),
        (
            "FR-1051",
            "c1",
            "Los Angeles",
            "CA",
            "Las Vegas",
            "NV",
            "Dry van",
            "ca4",
            Status::Accepted,
            270,
            142000,
            178000,
        ),
        (
            "FR-1052",
            "c2",
            "Boise",
            "ID",
            "Salt Lake City",
            "UT",
            "Reefer",
            "ca2",
            Status::Planned,
            340,
            164000,
            198000,
        ),
        (
            "FR-1046",
            "c1",
            "Reno",
            "NV",
            "San Francisco",
            "CA",
            "Dry van",
            "ca1",
            Status::Delivered,
            218,
            118000,
            149000,
        ),
        (
            "FR-1045",
            "c3",
            "Dallas",
            "TX",
            "Austin",
            "TX",
            "Flatbed",
            "ca3",
            Status::Delivered,
            195,
            98000,
            124000,
        ),
        (
            "FR-1053",
            "c3",
            "Chicago",
            "IL",
            "Detroit",
            "MI",
            "Dry van",
            "ca4",
            Status::Planned,
            283,
            138000,
            169000,
        ),
    ];
    for (i, (reference, customer, oc, os, dc, ds, eq, carrier, status, miles, buy, sell)) in
        lanes.into_iter().enumerate()
    {
        let now = Utc::now();
        let delivered = status == Status::Delivered;
        let active = status == Status::InTransit;
        let load = Load {
            intermediate_stops: vec![],
            id: format!("load_{}", i + 1),
            reference: reference.into(),
            customer_id: customer.into(),
            mode: "FTL".into(),
            equipment: eq.into(),
            commodity: if customer == "c2" {
                "Packaged foods"
            } else {
                "Consumer goods"
            }
            .into(),
            weight_lbs: if i == 1 { 36000 } else { 24000 },
            miles,
            origin: Stop {
                facility: format!("{oc} distribution center"),
                city: oc.into(),
                state: os.into(),
                date: today,
                details: crate::stops::StopDetails::demo(),
            },
            destination: Stop {
                facility: format!("{dc} receiving"),
                city: dc.into(),
                state: ds.into(),
                date: today + chrono::Duration::days(1),
                details: crate::stops::StopDetails::demo(),
            },
            carrier_id: if status == Status::Planned {
                None
            } else {
                Some(carrier.into())
            },
            driver_id: if active {
                Some(format!("d{}", i + 1))
            } else {
                None
            },
            equipment_id: if active {
                Some(format!("e{}", i + 1))
            } else {
                None
            },
            status,
            buy_cents: buy,
            sell_cents: sell,
            currency: "USD".into(),
            location: if delivered {
                dc.into()
            } else if i == 1 {
                "Medford, OR".into()
            } else {
                oc.into()
            },
            exception: if i == 1 {
                Some("Delivery appointment at risk · weather delay".into())
            } else {
                None
            },
            pod_reference: if delivered {
                Some(format!("POD-{reference}"))
            } else {
                None
            },
            pod_document_id: None,
            pod_review: if delivered {
                Some(crate::documents::Review {
                    decision: "accepted".into(),
                    note: "Synthetic demo delivery evidence".into(),
                    reviewed_by: "demo".into(),
                    reviewed_at: now,
                    reference: format!("POD-{reference}"),
                })
            } else {
                None
            },
            version: 1,
            created_at: now,
            updated_at: now,
            last_tracking_at: None,
            sync_order_id: None,
            order_number: None,
            source_order: None,
            tender_expires_at: None,
            tender_id: None,
        };
        w.record(
            &load.id,
            "load.created",
            format!("{} · {} → {}", reference, oc, dc),
        );
        w.loads.push(load);
    }
    w.invoice(NewInvoice {
        number: "INV-2084".into(),
        load_id: "load_6".into(),
        amount_cents: 118000,
        currency: "USD".into(),
    })
    .expect("valid demo invoice");
    w.invoice(NewInvoice {
        number: "INV-2085".into(),
        load_id: "load_7".into(),
        amount_cents: 108000,
        currency: "USD".into(),
    })
    .expect("valid demo invoice");
    w
}
