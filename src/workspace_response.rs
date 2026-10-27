//! Borrowed serialization of the complete staff response. Root fields are
//! explicitly whitelisted; never flatten the private Workspace into HTTP.
use super::{Tenant, WorkspaceProjection};
use crate::domain::Workspace;
use serde::{ser::SerializeMap, Serialize, Serializer};
use serde_json::{json, Value};

#[derive(Serialize)]
struct LoadResponse<'a> {
    // Load is already the public shipment representation. Borrowing it avoids
    // rebuilding its original fields as a Value tree, then serializing again.
    #[serde(flatten)]
    original: &'a crate::domain::Load,
    pod_ready: bool,
    tender_control: Value,
    stop_execution: Value,
    itinerary: Value,
    execution_timeline: Value,
    movement_sources: Value,
    route_attention: Value,
    commercial: Value,
    source_control: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    quote_binding: Option<Value>,
}
struct WorkspaceResponse<'a> {
    data: &'a Workspace,
    tenant: Tenant,
    demo: bool,
    durable: bool,
    integrations: Value,
    projection: WorkspaceProjection,
    loads: Vec<LoadResponse<'a>>,
    bills: Vec<Value>,
    settlements: Vec<Value>,
    finance: Value,
    invoices: Vec<Value>,
    quotes: Vec<Value>,
    rate_confirmations: Vec<Value>,
    customer_quote_publications: Vec<Value>,
    release_publications: Vec<Value>,
    carrier_contracts: Vec<Value>,
    operations: Value,
}

pub(super) fn encode(
    data: Workspace,
    tenant: Tenant,
    demo: bool,
    durable: bool,
    integrations: Value,
    projection: WorkspaceProjection,
) -> Result<Vec<u8>, String> {
    let profile = crate::read_profile::ReadProfile::new(&tenant.0);
    let started = std::time::Instant::now();
    let movement = crate::movement::MovementReadIndex::new(&data);
    profile.stage("staff_movement_index", started);
    let started = std::time::Instant::now();
    let gates = crate::stops::CurrentStopEvents::new(&data.stop_events);
    let appointments = crate::stops::ActiveAppointments::new(&data.appointments);
    let loads = data.loads.iter().map(|load| {
        let current_gates = gates.for_load(load);
        let mut response = LoadResponse {
            original:load,
            pod_ready:movement.delivery_ready(load,current_gates),
            tender_control:data.tender_controls(load),
            stop_execution:data.stop_execution_with_completion(load,chrono::Utc::now(),current_gates,|stop| appointments.for_stop(&load.id,stop),|stop| movement.services().stop_complete(load,stop)),
            itinerary:movement.services().view(load,true),
            execution_timeline:movement.timeline(load,true,current_gates),
            movement_sources:movement.source_views(load),
            route_attention:if movement.verify(load).is_err() {
                json!("Movement correction evidence cannot be verified; review original records")
            } else if !movement.consistent(load,current_gates) {
                json!("Execution times conflict; review original gate, service, and carrier evidence")
            } else if load.status == crate::domain::Status::Delivered && !movement.services().route_ready(load) {
                json!("Final delivery reported; intermediate service confirmations remain outstanding")
            } else { Value::Null },
            commercial:data.commercial_summary(load)?,
            source_control:data.source_summary(load),
            quote_binding:None,
        };
        response.quote_binding = data.quotes.iter().find(|q| q.active_booking(&load.id)).map(|quote| {
            let offer=quote.procured_offer()?;
            Ok::<_, String>(json!({"id":quote.id,"reference":quote.reference,"carrier_id":offer.carrier.id,"valid_until":quote.procurement_expiry()?}))
        }).transpose()?;
        Ok(response)
    }).collect::<Result<Vec<_>,String>>()?;
    drop(gates);
    drop(appointments);
    profile.stage("staff_loads", started);
    let started = std::time::Instant::now();
    let financial = crate::finance::FinanceReadIndex::new(&data);
    let bills = data
        .bills
        .iter()
        .map(|bill| {
            let mut value = json!(bill);
            value["totals"] = json!(financial.bill_totals(bill)?);
            value["activity_date"] = json!(financial.customer_activity_date(bill));
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let settlements = data
        .settlements
        .iter()
        .map(|settlement| {
            let balance = financial.settlement_balance(&settlement.id)?;
            let mut value = json!(settlement);
            value["balance_cents"] = json!(balance);
            value["total_amount_cents"] = json!(financial.settlement_total(settlement)?);
            value["credit_balance_cents"] = json!(crate::finance::credit_balance(balance)?);
            value["credit_application_cents"] =
                json!(financial.carrier_credit_effect(&settlement.id)?);
            value["credit_application_earliest_date"] =
                json!(financial.carrier_credit_earliest_date(&settlement.id)?);
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let finance = financial.summary()?;
    drop(financial);
    profile.stage("staff_finance", started);
    let started = std::time::Instant::now();
    let invoice_reviews = crate::invoice_review::ReviewIndex::new(&data);
    let invoices = data
        .invoices
        .iter()
        .map(|invoice| {
            let mut value = json!(invoice);
            value["approval_control"] = invoice_reviews.summary_with_details(
                invoice,
                &tenant.1.subject,
                projection.invoice_review_required,
                !projection.defer_approval,
            )?;
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    drop(invoice_reviews);
    profile.stage("staff_invoice_reviews", started);
    let started = std::time::Instant::now();
    let quotes: Vec<Value> = data
        .quotes
        .iter()
        .map(|quote| {
            let mut v = json!(quote);
            v["effective_status"] = json!(quote.effective_status());
            v
        })
        .collect();
    let rate_confirmations = data
        .rate_confirmations
        .iter()
        .map(|doc| {
            let mut value = json!(doc);
            value.as_object_mut().map(|o| o.remove("html"));
            value["current"] = json!(data.rate_confirmation_current(doc));
            value["basis_changed"] = json!(data.rate_confirmation_basis_changed(doc));
            value
        })
        .collect();
    let customer_quote_publications = data
        .customer_quote_publications
        .iter()
        .map(|p| {
            let mut value = json!(p);
            value.as_object_mut().map(|o| o.remove("html"));
            value
        })
        .collect();
    let release_publications = data
        .release_publications
        .iter()
        .map(|p| {
            let mut value = json!(p);
            value.as_object_mut().map(|o| o.remove("html"));
            value
        })
        .collect();
    let carrier_contracts = data
        .carrier_contracts
        .iter()
        .map(|c| {
            let mut value = json!(c);
            if let Some(revisions) = value["revisions"].as_array_mut() {
                for revision in revisions {
                    revision.as_object_mut().map(|o| o.remove("html"));
                }
            }
            value
        })
        .collect();
    profile.stage("staff_publications", started);
    let started = std::time::Instant::now();
    let operations = movement.operations_summary(chrono::Utc::now());
    drop(movement);
    profile.stage("staff_operations", started);
    let response = WorkspaceResponse {
        data: &data,
        tenant,
        demo,
        durable,
        integrations,
        projection,
        loads,
        bills,
        settlements,
        finance,
        invoices,
        quotes,
        rate_confirmations,
        customer_quote_publications,
        release_publications,
        carrier_contracts,
        operations,
    };
    let started = std::time::Instant::now();
    let result = serde_json::to_vec(&response).map_err(|_| "Internal storage error".into());
    profile.stage("staff_serialization", started);
    let started = std::time::Instant::now();
    drop(response);
    drop(data);
    profile.stage("staff_dispose", started);
    result
}
impl Serialize for WorkspaceResponse<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("access",&(json!({"role":self.tenant.1.role,"subject":self.tenant.1.subject,"capabilities":self.tenant.1.role.capabilities()})))?;
        map.serialize_entry("billing_profile", &(self.data.billing_profile))?;
        map.serialize_entry("bills", &(self.bills))?;
        map.serialize_entry("cancellation_fees", &(self.data.cancellation_fees))?;
        map.serialize_entry("carrier_adjustments", &(self.data.carrier_adjustments))?;
        map.serialize_entry("carrier_contracts", &(self.carrier_contracts))?;
        map.serialize_entry(
            "carrier_credit_applications",
            &(self.data.carrier_credit_applications),
        )?;
        map.serialize_entry("carrier_documents", &(self.data.carrier_documents))?;
        map.serialize_entry("carrier_submissions", &(self.data.carrier_submissions))?;
        map.serialize_entry("carriers", &(self.data.carriers))?;
        map.serialize_entry("cash_entries", &(self.data.cash_entries))?;
        map.serialize_entry("charges", &(self.data.charges))?;
        map.serialize_entry("credits", &(self.data.credits))?;
        map.serialize_entry(
            "customer_quote_decisions",
            &(self.data.customer_quote_decisions),
        )?;
        map.serialize_entry(
            "customer_quote_publications",
            &(self.customer_quote_publications),
        )?;
        map.serialize_entry("customers", &(self.data.customers))?;
        map.serialize_entry("document_shares", &(self.data.document_shares))?;
        map.serialize_entry("documents", &(self.data.documents))?;
        map.serialize_entry("drivers", &(self.data.drivers))?;
        map.serialize_entry("equipment", &(self.data.equipment))?;
        map.serialize_entry("finance", &(self.finance))?;
        map.serialize_entry("inbox", &(self.data.inbox))?;
        map.serialize_entry("integrations", &(self.integrations))?;
        map.serialize_entry("invoices", &(self.invoices))?;
        map.serialize_entry("journal", &(self.data.journal))?;
        map.serialize_entry("loads", &(self.loads))?;
        map.serialize_entry(
            "mode",
            &(if self.demo && self.tenant.0 == "demo" {
                "demo"
            } else {
                "live"
            }),
        )?;
        map.serialize_entry("movement_corrections", &(self.data.movement_corrections))?;
        map.serialize_entry("operations", &(self.operations))?;
        map.serialize_entry("outbox", &(self.data.outbox))?;
        map.serialize_entry("portal_decisions", &(self.data.portal_decisions))?;
        map.serialize_entry("portal_offers", &(self.data.portal_offers))?;
        map.serialize_entry("quotes", &(self.quotes))?;
        map.serialize_entry("rate_confirmations", &(self.rate_confirmations))?;
        map.serialize_entry("release_publications", &(self.release_publications))?;
        map.serialize_entry("release_responses", &(self.data.release_responses))?;
        map.serialize_entry("releases", &(self.data.releases))?;
        map.serialize_entry("settlements", &(self.settlements))?;
        map.serialize_entry("source_checks", &(self.data.source_checks))?;
        map.serialize_entry("source_reviews", &(self.data.source_reviews))?;
        map.serialize_entry("stop_services", &(self.data.stop_services))?;
        map.serialize_entry(
            "storage",
            &(if self.durable {
                "postgresql"
            } else {
                "ephemeral"
            }),
        )?;
        map.serialize_entry("tenant_id", &(self.tenant.0))?;
        map.serialize_entry("tender_incidents", &(self.data.tender_incidents))?;
        map.serialize_entry("tenders", &(self.data.tenders))?;
        if self.projection.deferred_history {
            map.serialize_entry("history",&json!({"mode":"deferred","events":self.data.events.len(),"appointments":self.data.appointments.len(),"gates":self.data.stop_events.len()}))?;
        } else {
            map.serialize_entry("events", &self.data.events)?;
            map.serialize_entry("appointments", &self.data.appointments)?;
            map.serialize_entry("stop_events", &self.data.stop_events)?;
        }
        map.end()
    }
}
