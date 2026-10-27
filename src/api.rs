use crate::{
    auth::{route_capability, Credential, Principal},
    documents::{Upload, MAX_DOCUMENT_BYTES},
    integrations::{verify_callback, Integrations},
    store::Store,
};
use axum::{
    extract::{DefaultBodyLimit, MatchedPath, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct App {
    pub pdf_renderer: Arc<crate::pdf_renderer::PdfRenderer>,
    pub identity: Arc<Option<crate::identity::ClerkIdentity>>,
    pub store: Store,
    pub keys: Arc<BTreeMap<String, Credential>>,
    pub demo: bool,
    pub integrations: Arc<Integrations>,
}
#[derive(Clone)]
struct Tenant(String, Principal);
struct Error(StatusCode, String);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let busy_code = if self.1 == crate::store::READ_BUSY {
            Some("read_capacity_exceeded")
        } else if self.1 == crate::store::WORKSPACE_BUSY {
            Some("workspace_capacity_exceeded")
        } else if self.1 == crate::store::WRITE_BUSY {
            Some("write_capacity_exceeded")
        } else if self.1 == crate::store::DATABASE_BUSY
            || self.1 == crate::store::DATABASE_UNAVAILABLE
        {
            Some("database_temporarily_unavailable")
        } else {
            None
        };
        if let Some(code) = busy_code {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "1"), ("cache-control", "no-store")],
                Json(json!({"error":{"code":code,"message":self.1}})),
            )
                .into_response();
        }
        if self.1 == crate::save_recovery::REQUEST_STOPPED {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error":{"code":"request_stopped","message":self.1}})),
            )
                .into_response();
        }
        (self.0, Json(json!({"error":{"message":self.1}}))).into_response()
    }
}
impl From<String> for Error {
    fn from(message: String) -> Self {
        let status = if message == crate::sharing::FINANCE_REQUIRED
            || message == crate::save_recovery::RECOVERY_FORBIDDEN
            || message == crate::invoice_review::INDEPENDENT_REVIEW
            || message == crate::invoice_review::NAMED_REVIEWER
        {
            StatusCode::FORBIDDEN
        } else if message == crate::source::SOURCE_HOLD {
            StatusCode::CONFLICT
        } else if message == crate::source::SOURCE_UNAVAILABLE
            || message == crate::store::INCOMPATIBLE_STORAGE
            || message == crate::store::READ_BUSY
            || message == crate::store::WRITE_BUSY
            || message == crate::store::WORKSPACE_BUSY
            || message == crate::store::DATABASE_BUSY
            || message == crate::store::DATABASE_UNAVAILABLE
        {
            StatusCode::SERVICE_UNAVAILABLE
        } else if message == "Internal storage error" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else if message.ends_with("not found") || message == "Shipment not found" {
            StatusCode::NOT_FOUND
        } else if message.contains("already")
            || message.contains("changed;")
            || message.contains("older than")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        Self(status, message)
    }
}
pub fn router(app: App) -> Router {
    router_with_observability(app, crate::observability::Observability::default())
}

pub fn router_with_observability(
    app: App,
    observation: crate::observability::Observability,
) -> Router {
    let protected = Router::new()
        .route("/v1/history", get(history_page))
        .route("/v1/save-recovery/resolve", post(resolve_save))
        .route("/v1/access", get(access))
        .route("/v1/console", get(console_page))
        .route("/v1/operations", get(operations_board))
        .route("/v1/operations/issues/{id}", get(operations_case))
        .route("/v1/operations/policy", post(operations_policy))
        .route("/v1/operations/reconcile", post(operations_reconcile))
        .route(
            "/v1/operations/issues/{id}/actions",
            post(operations_action),
        )
        .route("/v1/loads/{id}/stop-services", post(record_stop_service))
        .route("/v1/stop-services/{id}/correct", post(correct_stop_service))
        .route(
            "/v1/carrier-submissions/{id}/time-corrections",
            post(correct_movement_time),
        )
        .route(
            "/v1/portal/offers/{id}/stop-services",
            post(partner_stop_service),
        )
        .route("/v1/carrier-contracts", post(create_carrier_contract))
        .route(
            "/v1/carrier-contracts/{id}/revise",
            post(revise_carrier_contract),
        )
        .route(
            "/v1/carrier-contracts/{id}/withdraw",
            post(withdraw_carrier_contract),
        )
        .route(
            "/v1/carrier-contracts/{id}/statement",
            get(carrier_contract_statement),
        )
        .route("/v1/quotes/{id}/contract-rates", get(contract_candidates))
        .route("/v1/quotes/{id}/contract-offers", post(contract_offer))
        .route(
            "/v1/quotes/{id}/contract-reprocure",
            post(contract_reprocure),
        )
        .route("/v1/portal", get(partner_workspace))
        .route(
            "/v1/release-publications/{id}/statement",
            get(staff_release_statement),
        )
        .route(
            "/v1/release-requests/{id}/portal-publications",
            post(publish_release_request),
        )
        .route(
            "/v1/release-responses/{id}/apply",
            post(apply_release_response),
        )
        .route(
            "/v1/portal/releases/{id}/response",
            post(carrier_release_response),
        )
        .route(
            "/v1/portal/releases/{id}/statement",
            get(carrier_release_statement),
        )
        .route("/v1/documents/{id}/shares", post(share_document))
        .route(
            "/v1/document-shares/{id}/revoke",
            post(revoke_document_share),
        )
        .route(
            "/v1/portal/files/{id}/content",
            get(shared_document_content),
        )
        .route(
            "/v1/quotes/{id}/portal-publications",
            post(publish_customer_quote),
        )
        .route(
            "/v1/portal/customer-quotes/{id}/response",
            post(customer_quote_response),
        )
        .route(
            "/v1/portal/customer-quotes/{id}/statement",
            get(customer_quote_statement),
        )
        .route(
            "/v1/portal/customer-quotes/{id}/source/refresh",
            post(customer_quote_source),
        )
        .route("/v1/loads/{id}/portal-offers", post(publish_portal_offer))
        .route("/v1/portal/offers/{id}/response", post(partner_response))
        .route("/v1/portal/offers/{id}/tracking", post(partner_tracking))
        .route(
            "/v1/portal/offers/{id}/pod",
            post(partner_pod).layer(DefaultBodyLimit::max(MAX_DOCUMENT_BYTES)),
        )
        .route(
            "/v1/portal/submissions/{id}/content",
            get(partner_submission_content),
        )
        .route(
            "/v1/portal/rate-confirmations/{id}/statement",
            get(partner_rate_statement),
        )
        .route(
            "/v1/portal/bills/{id}/statement",
            get(partner_bill_statement),
        )
        .route(
            "/v1/loads/{id}/rate-confirmations",
            post(issue_rate_confirmation),
        )
        .route(
            "/v1/rate-confirmations/{id}/handoffs",
            post(record_rate_handoff),
        )
        .route(
            "/v1/rate-confirmations/{id}/handoffs/resolve",
            post(resolve_rate_handoff),
        )
        .route(
            "/v1/rate-confirmations/{id}/handoffs/retract",
            post(retract_rate_handoff),
        )
        .route("/v1/rate-confirmations/{id}/statement", get(rate_statement))
        .route("/v1/loads/{id}/source/refresh", post(refresh_source))
        .route("/v1/source-reviews/{id}/resolve", post(resolve_source))
        .route("/v1/quotes/{id}/amendments", post(create_amendment))
        .route("/v1/quotes", post(create_quote))
        .route("/v1/quotes/{id}/revise", post(revise_quote))
        .route("/v1/quotes/{id}/offers", post(record_quote_offer))
        .route(
            "/v1/quotes/{id}/offers/withdraw",
            post(withdraw_quote_offer),
        )
        .route("/v1/quotes/{id}/issue", post(issue_quote))
        .route("/v1/quotes/{id}/decision", post(decide_quote))
        .route("/v1/quotes/{id}/book", post(book_quote))
        .route("/v1/quotes/{id}/reconfirm", post(reconfirm_quote))
        .route("/v1/quotes/{id}/reprocure", post(reprocure_quote))
        .route("/v1/quotes/{id}/statement", get(quote_statement))
        .route(
            "/v1/loads/{id}/cancellation-fees",
            post(propose_cancellation),
        )
        .route(
            "/v1/cancellation-fees/{id}/review",
            post(review_cancellation),
        )
        .route(
            "/v1/cancellation-fees/{id}/reverse",
            post(reverse_cancellation),
        )
        .route("/v1/workspace", get(workspace))
        .route("/v1/bills", post(create_bill))
        .route("/v1/finance/profile", post(update_billing_profile))
        .route("/v1/bills/{id}/plan", post(revise_bill))
        .route("/v1/bills/{id}/supplements", post(create_supplement))
        .route("/v1/bills/{id}/issue", post(issue_bill))
        .route("/v1/bills/{id}/void", post(void_bill))
        .route("/v1/bills/{id}/credits", post(create_credit))
        .route("/v1/credits/{id}/reverse", post(reverse_credit))
        .route("/v1/cash", post(record_cash))
        .route(
            "/v1/carrier-credit-applications",
            post(apply_carrier_credit),
        )
        .route(
            "/v1/carrier-credit-applications/{id}/reverse",
            post(reverse_carrier_credit),
        )
        .route("/v1/cash/{id}/reverse", post(reverse_cash))
        .route("/v1/bills/{id}/statement", get(bill_statement))
        .route("/v1/finance/journal.csv", get(journal_csv))
        .route(
            "/v1/loads/{id}/documents",
            get(document_preview::list)
                .post(upload_document)
                .layer(DefaultBodyLimit::max(MAX_DOCUMENT_BYTES)),
        )
        .route("/v1/documents/{id}/content", get(document_content))
        .route("/v1/documents/{id}/preview", get(document_preview::preview))
        .route("/v1/documents/{id}/review", post(review_document))
        .route("/v1/loads/{id}/pod/review", post(review_pod))
        .route("/v1/loads", get(load_catalog).post(create_load))
        .route(
            "/v1/loads/{id}/carrier-adjustments",
            post(create_carrier_adjustment),
        )
        .route(
            "/v1/carrier-adjustments/{id}/review",
            post(review_carrier_adjustment),
        )
        .route(
            "/v1/carrier-adjustments/{id}/reverse",
            post(reverse_carrier_adjustment),
        )
        .route(
            "/v1/carrier-documents/{id}/dismiss",
            post(dismiss_carrier_document),
        )
        .route(
            "/v1/carrier-documents/{id}/apply-revision",
            post(apply_carrier_revision),
        )
        .route("/v1/loads/{id}/charges", post(propose_charge))
        .route("/v1/charges/{id}/review", post(review_charge))
        .route("/v1/charges/{id}/void", post(void_charge))
        .route("/v1/loads/{id}/appointments", post(book_appointment))
        .route("/v1/appointments/{id}/cancel", post(cancel_appointment))
        .route("/v1/loads/{id}/stop-events", post(record_stop_event))
        .route("/v1/stop-events/{id}/correct", post(correct_stop_event))
        .route("/v1/loads/{id}/release-requests", post(request_release))
        .route("/v1/release-requests/{id}/confirm", post(confirm_release))
        .route("/v1/release-requests/{id}/retain", post(retain_carrier))
        .route(
            "/v1/tender-incidents/{id}/resolve",
            post(resolve_tender_incident),
        )
        .route("/v1/loads/{id}/commands", post(command))
        .route("/v1/loads/{id}/plan", post(revise_plan))
        .route("/v1/carriers/{id}", post(update_carrier))
        .route("/v1/customers/{id}", post(update_customer))
        .route("/v1/drivers/{id}", post(update_driver))
        .route("/v1/equipment/{id}", post(update_equipment))
        .route("/v1/carriers", post(create_carrier))
        .route("/v1/customers", post(create_customer))
        .route("/v1/drivers", post(create_driver))
        .route("/v1/equipment", post(create_equipment))
        .route("/v1/invoices", post(create_invoice))
        .route(
            "/v1/invoices/{id}/approval-evidence",
            get(invoice_approval_evidence),
        )
        .route("/v1/invoices/{id}/approve", post(approve_invoice))
        .route("/v1/invoices/{id}/entry-review", post(review_invoice_entry))
        .route("/v1/loads/{id}/settlement", post(settlement))
        .route("/v1/events", post(edi_event))
        .route("/v1/invoices/{id}/resolve", post(resolve_invoice))
        .route("/v1/outbox/{id}/retry", post(retry_outbox))
        .route("/v1/inbox/{id}/retry", post(retry_inbox))
        .route("/v1/commerce/orders", get(commerce_orders))
        .route("/v1/commerce/orders/{id}", get(commerce_order))
        .route("/v1/commerce/import", post(commerce_import))
        .route("/v1/loads/{id}/commerce-shipments", get(commerce_shipments))
        .route(
            "/v1/loads/{id}/commerce-tracking",
            get(commerce_tracking_preview).post(commerce_tracking),
        )
        .route(
            "/v1/outbox/{id}/commerce-review",
            get(commerce_review).post(commerce_reconcile),
        )
        .route("/v1/sync/orders", get(sync_orders))
        .route("/v1/sync/import", post(import_order))
        .route_layer(middleware::from_fn_with_state(app.clone(), authorize));
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(health))
        .route("/live", get(crate::observability::live))
        .route("/metrics", get(crate::observability::metrics))
        .route("/webhooks/edi/{tenant}", post(edi_callback))
        .merge(protected)
        .with_state(app)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(TraceLayer::new_for_http().make_span_with(crate::observability::request_span))
        .layer(middleware::from_fn_with_state(
            observation.shutdown.clone(),
            crate::lifecycle::admission,
        ))
        .layer(middleware::from_fn_with_state(
            observation.clone(),
            crate::observability::observe,
        ))
        .layer(Extension(observation))
}
async fn authorize(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let clerk_only = request
        .headers()
        .get("x-freight-identity-mode")
        .is_some_and(|v| v == "clerk");
    let mut tenant = token.filter(|_| !clerk_only).and_then(|token| {
        let incoming = Sha256::digest(token.as_bytes());
        app.keys
            .iter()
            .find(|(key, _)| {
                let expected = Sha256::digest(key.as_bytes());
                incoming
                    .iter()
                    .zip(expected.iter())
                    .fold(0u8, |a, (b, c)| a | (b ^ c))
                    == 0
            })
            .map(|(_, credential)| {
                let actor = format!("credential:{:.12}", format!("{:x}", incoming));
                credential.principal(&actor)
            })
    });
    if tenant.is_none() {
        if let (Some(identity), Some(token)) = (app.identity.as_ref(), token) {
            match identity.verify(token).await {
                Ok(identity) => {
                    if request
                        .headers()
                        .get("x-freight-organization-id")
                        .is_some_and(|v| v.to_str().ok() != Some(identity.organization_id.as_str()))
                    {
                        return Error(
                            StatusCode::FORBIDDEN,
                            "Organization changed; refresh before continuing".into(),
                        )
                        .into_response();
                    }
                    tenant = Some(identity.principal);
                }
                Err(error) => {
                    use crate::identity::IdentityError::*;
                    let (code,message)=match error {
                        Unauthenticated=>(StatusCode::UNAUTHORIZED,"A current staff session is required"),
                        Forbidden=>(StatusCode::FORBIDDEN,"This staff membership has no access to the selected freight organization"),
                        Unavailable=>(StatusCode::SERVICE_UNAVAILABLE,"Staff identity verification is temporarily unavailable"),
                    };
                    return Error(code, message.into()).into_response();
                }
            }
        }
    }
    match tenant {
        Some(principal) => {
            if !principal.valid_scope() {
                return Error(StatusCode::FORBIDDEN, "Invalid identity scope".into())
                    .into_response();
            }
            let capability = request
                .extensions()
                .get::<MatchedPath>()
                .and_then(|route| route_capability(request.method().as_str(), route.as_str()));
            if !capability.is_some_and(|v| principal.role.allows(v)) {
                return Error(
                    StatusCode::FORBIDDEN,
                    "Your workspace role does not permit this operation".into(),
                )
                .into_response();
            }
            request
                .extensions_mut()
                .insert(Tenant(principal.tenant_id.clone(), principal));
            next.run(request).await
        }
        None => Error(
            StatusCode::UNAUTHORIZED,
            "A valid freight API credential is required".into(),
        )
        .into_response(),
    }
}
async fn health(
    State(app): State<App>,
    Extension(observation): Extension<crate::observability::Observability>,
) -> Response {
    let healthy = crate::observability::ready(&observation.shutdown, app.store.healthy()).await;
    (
        if healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        [("cache-control", "no-store")],
        Json(json!({"status":if healthy {"ok"} else {"unavailable"},"service":"stateset-freight"})),
    )
        .into_response()
}
fn encoded_json(bytes: Vec<u8>) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        bytes,
    )
        .into_response()
}
async fn workspace(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<WorkspaceQuery>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    let defer_approval = match headers.get("x-freight-approval-details") {
        None => false,
        Some(value) if value == "complete" => false,
        Some(value) if value == "deferred" => true,
        Some(_) => {
            return Err(Error(
                StatusCode::BAD_REQUEST,
                "Approval details mode must be complete or deferred".into(),
            ))
        }
    };
    let tenant_id = tenant.0.clone();
    let integrations = app.integrations.summary(&tenant.0);
    let demo = app.demo;
    let durable = app.store.durable();
    let invoice_review_required = app.store.invoice_review_required(&tenant_id);
    let bytes = app
        .store
        .read_encode(&tenant_id, move |data| {
            workspace_response::encode(
                data,
                tenant,
                demo,
                durable,
                integrations,
                WorkspaceProjection {
                    deferred_history: query.history == Some(HistoryMode::Deferred),
                    invoice_review_required,
                    defer_approval,
                },
            )
        })
        .await?;
    Ok((
        [
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::VARY, "X-Freight-Approval-Details"),
        ],
        encoded_json(bytes),
    )
        .into_response())
}
async fn invoice_approval_evidence(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let tenant_id = tenant.0.clone();
    let bytes = app
        .store
        .approval_project(&tenant_id, &id, move |mut value| {
            value["tenant_id"] = json!(tenant.0);
            value["access"] = json!({"subject":tenant.1.subject,"role":tenant.1.role});
            Ok(value)
        })
        .await?;
    if bytes.len() > 64 * 1024 {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Approval record is too large to display".into(),
        ));
    }
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        encoded_json(bytes),
    )
        .into_response())
}
#[derive(serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum HistoryMode {
    Deferred,
    Complete,
}
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceQuery {
    history: Option<HistoryMode>,
}
async fn history_page(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<crate::history::Query>,
) -> Result<Response, Error> {
    let tenant_id = tenant.0.clone();
    let bytes = app
        .store
        .history_project(&tenant_id, query, move |mut value| {
            value["tenant_id"] = json!(tenant.0);
            value["access"] = json!({"subject":tenant.1.subject,"role":tenant.1.role});
            serde_json::to_vec(&value).map_err(|_| "Internal storage error".to_string())
        })
        .await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        encoded_json(bytes),
    )
        .into_response())
}
async fn load_catalog(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<crate::load_catalog::CatalogQuery>,
) -> Result<Json<Value>, Error> {
    Ok(Json(app.store.load_catalog(&tenant.0, query).await?))
}
async fn console_page(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<crate::console_catalog::Query>,
) -> Result<Response, Error> {
    let data = app.store.console_page(&tenant.0, &tenant.1, query).await?;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(data),
    )
        .into_response())
}

async fn access(Extension(tenant): Extension<Tenant>) -> Json<Value> {
    Json(
        json!({"tenant_id":tenant.0,"features":{"operations_save_recovery":true,"console_catalog":true},"access":{"subject":tenant.1.subject,"role":tenant.1.role,"capabilities":tenant.1.role.capabilities()}}),
    )
}
#[cfg(test)]
fn workspace_view(
    data: crate::domain::Workspace,
    tenant: Tenant,
    demo: bool,
    durable: bool,
    integrations: Value,
) -> Result<Value, String> {
    let required = !(demo && tenant.0 == "demo");
    let bytes = workspace_response::encode(
        data,
        tenant,
        demo,
        durable,
        integrations,
        WorkspaceProjection {
            deferred_history: false,
            invoice_review_required: required,
            defer_approval: false,
        },
    )?;
    serde_json::from_slice(&bytes).map_err(|_| "Internal storage error".into())
}
struct WorkspaceProjection {
    deferred_history: bool,
    invoice_review_required: bool,
    defer_approval: bool,
}
#[cfg(test)]
fn workspace_view_with_history(
    data: crate::domain::Workspace,
    tenant: Tenant,
    demo: bool,
    durable: bool,
    integrations: Value,
    projection: WorkspaceProjection,
) -> Result<Value, String> {
    let deferred = projection.deferred_history;
    let gates = crate::stops::CurrentStopEvents::new(&data.stop_events);
    let appointments = crate::stops::ActiveAppointments::new(&data.appointments);
    let loads: Vec<Value> = data
        .loads
        .iter()
        .map(|load| {
            let current_gates = gates.for_load(load);
            let mut value = json!(load);
            value["pod_ready"] = json!(data.delivery_evidence_ready(load));
            value["tender_control"] = data.tender_controls(load);
            value["stop_execution"] = data.stop_execution_with_sources(load, chrono::Utc::now(), current_gates, |stop| appointments.for_stop(&load.id, stop));
            value["itinerary"] = data.itinerary_view(load,true);
            value["execution_timeline"] = data.execution_timeline_with_gates(load,true,current_gates);
            value["movement_sources"] = data.movement_sources(load);
            value["route_attention"] = if data.verify_movement_corrections(load).is_err() {
                json!("Movement correction evidence cannot be verified; review original records")
            } else if !data.execution_consistent_with_gates(load, current_gates) {
                json!("Execution times conflict; review original gate, service, and carrier evidence")
            } else if load.status == crate::domain::Status::Delivered && !data.route_ready(load) {
                json!("Final delivery reported; intermediate service confirmations remain outstanding")
            } else { Value::Null };
            value["commercial"] = data.commercial_summary(load)?;
            value["source_control"]=data.source_summary(load);
            if let Some(quote)=data.quotes.iter().find(|v|v.active_booking(&load.id)) {
                let offer=quote.procured_offer()?;
                value["quote_binding"]=json!({"id":quote.id,"reference":quote.reference,"carrier_id":offer.carrier.id,"valid_until":quote.procurement_expiry()?});
            }
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    drop(gates);
    drop(appointments);
    let bills = data
        .bills
        .iter()
        .map(|bill| {
            let mut value = json!(bill);
            value["totals"] = json!(data.bill_totals(bill)?);
            value["activity_date"] = json!(data.customer_activity_date(bill));
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let settlements = data
        .settlements
        .iter()
        .map(|settlement| {
            let mut value = json!(settlement);
            value["balance_cents"] = json!(data.settlement_balance(&settlement.id)?);
            value["total_amount_cents"] = json!(data.settlement_total(settlement)?);
            value["credit_balance_cents"] = json!(crate::finance::credit_balance(
                data.settlement_balance(&settlement.id)?
            )?);
            value["credit_application_cents"] = json!(data.carrier_credit_effect(&settlement.id)?);
            value["credit_application_earliest_date"] =
                json!(data.carrier_credit_earliest_date(&settlement.id)?);
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    let finance = data.finance_summary()?;
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
    let quotes: Vec<Value> = data
        .quotes
        .iter()
        .map(|quote| {
            let mut v = json!(quote);
            v["effective_status"] = json!(quote.effective_status());
            v
        })
        .collect();
    let mut response = json!({"tenant_id":tenant.0,"mode":if demo && tenant.0=="demo" {"demo"} else {"live"},"storage":if durable {"postgresql"} else {"ephemeral"},
        "loads":loads,"customers":data.customers,"carriers":data.carriers,"drivers":data.drivers,"equipment":data.equipment,"invoices":invoices,"settlements":settlements,"outbox":data.outbox,"inbox":data.inbox,"documents":data.documents,
        "bills":bills,"credits":data.credits,"cash_entries":data.cash_entries,"journal":data.journal,"finance":finance,"billing_profile":data.billing_profile,
        "portal_offers":data.portal_offers,"portal_decisions":data.portal_decisions,"carrier_submissions":data.carrier_submissions,
        "rate_confirmations":data.rate_confirmations.iter().map(|doc| {let mut v=json!(doc); v.as_object_mut().map(|o|o.remove("html")); v["current"]=json!(data.rate_confirmation_current(doc)); v["basis_changed"]=json!(data.rate_confirmation_basis_changed(doc));v}).collect::<Vec<_>>(),
        "tenders":data.tenders,"releases":data.releases,"tender_incidents":data.tender_incidents,
        "charges":data.charges,
        "carrier_adjustments":data.carrier_adjustments,"carrier_documents":data.carrier_documents,
        "carrier_credit_applications":data.carrier_credit_applications,
        "cancellation_fees":data.cancellation_fees,
        "quotes":quotes,
        "source_checks":data.source_checks,"source_reviews":data.source_reviews,
        "access":{"role":tenant.1.role,"subject":tenant.1.subject,"capabilities":tenant.1.role.capabilities()},
        "integrations":integrations});
    if deferred {
        response["history"] = json!({"mode":"deferred","events":data.events.len(),"appointments":data.appointments.len(),"gates":data.stop_events.len()});
    } else {
        response["events"] = json!(data.events);
        response["appointments"] = json!(data.appointments);
        response["stop_events"] = json!(data.stop_events);
    }
    response["customer_quote_publications"] = json!(data
        .customer_quote_publications
        .iter()
        .map(|p| {
            let mut v = json!(p);
            v.as_object_mut().map(|o| o.remove("html"));
            v
        })
        .collect::<Vec<_>>());
    response["customer_quote_decisions"] = json!(data.customer_quote_decisions);
    response["document_shares"] = json!(data.document_shares);
    response["release_publications"] = json!(data
        .release_publications
        .iter()
        .map(|p| {
            let mut v = json!(p);
            v.as_object_mut().map(|o| o.remove("html"));
            v
        })
        .collect::<Vec<_>>());
    response["release_responses"] = json!(data.release_responses);
    response["stop_services"] = json!(data.stop_services);
    response["movement_corrections"] = json!(data.movement_corrections);
    response["operations"] = crate::operations::preceding_summary(&data, chrono::Utc::now());
    response["carrier_contracts"] = json!(data
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
        .collect::<Vec<_>>());
    Ok(response)
}
async fn refresh_source(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    if body != json!({}) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Source refresh accepts an empty object; canonical data is fetched from Sync".into(),
        ));
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    Ok(Json(
        app.integrations
            .refresh_source(&app.store, &tenant.0, &id, &tenant.1.subject, Some(key))
            .await?,
    ))
}
async fn resolve_save(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Json(input): Json<crate::save_recovery::ResolveSave>,
) -> Result<Json<Value>, Error> {
    Ok(Json(app.store.resolve_save(&tenant.1, input).await?))
}
async fn mutate(
    app: App,
    tenant: Tenant,
    headers: HeaderMap,
    operation: &str,
    entity: &str,
    body: Value,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    let mut source_grant = None;
    if crate::source::requires_source_snapshot(operation, &body) {
        if let Some(response) = app
            .store
            .replay(&tenant.0, key, operation, entity, &body)
            .await?
        {
            return Ok(Json(response));
        }
        let workspace = app.store.read(&tenant.0).await?;
        if let Some(load) = workspace.commitment_source(operation, entity, &body) {
            if operation == "load.command" {
                serde_json::from_value::<crate::domain::Command>(body.clone()).map_err(|_| {
                    Error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Invalid load command".into(),
                    )
                })?;
            }
            let observation = app
                .integrations
                .refresh_source(&app.store, &tenant.0, &load.id, &tenant.1.subject, None)
                .await?;
            source_grant = Some(crate::source::SourceGrant {
                load_id: load.id.clone(),
                version: observation["data"]["version"].as_u64().ok_or_else(|| {
                    Error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Invalid source observation".into(),
                    )
                })?,
            });
        }
    }
    Ok(Json(
        app.store
            .mutate_guarded(
                (&tenant.0, &tenant.1.subject),
                (key, operation, entity, body),
                source_grant,
            )
            .await?,
    ))
}
macro_rules! create_handler {
    ($name:ident,$operation:literal) => {
        async fn $name(
            State(app): State<App>,
            Extension(tenant): Extension<Tenant>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Result<Json<Value>, Error> {
            mutate(app, tenant, headers, $operation, "", body).await
        }
    };
}
create_handler!(create_load, "load.create");
create_handler!(create_quote, "quote.create");
create_handler!(create_carrier_contract, "contract.create");
create_handler!(create_carrier, "carrier.create");
create_handler!(create_customer, "customer.create");
create_handler!(create_driver, "driver.create");
create_handler!(create_equipment, "equipment.create");
create_handler!(create_invoice, "invoice.create");
create_handler!(create_bill, "bill.create");
create_handler!(record_cash, "cash.record");
create_handler!(apply_carrier_credit, "carrier_credit.apply");
create_handler!(update_billing_profile, "bill.profile");
macro_rules! update_handler {
    ($name:ident,$operation:literal) => {
        async fn $name(
            State(app): State<App>,
            Extension(tenant): Extension<Tenant>,
            Path(id): Path<String>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Result<Json<Value>, Error> {
            mutate(app, tenant, headers, $operation, &id, body).await
        }
    };
}
update_handler!(revise_plan, "load.revise");
update_handler!(resolve_source, "source.resolve");
update_handler!(create_amendment, "quote.amend");
update_handler!(revise_quote, "quote.revise");
update_handler!(record_quote_offer, "quote.offer");
update_handler!(withdraw_quote_offer, "quote.offer.withdraw");
update_handler!(issue_quote, "quote.issue");
update_handler!(decide_quote, "quote.decision");
update_handler!(book_quote, "quote.book");
update_handler!(reconfirm_quote, "quote.reconfirm");
update_handler!(reprocure_quote, "quote.reprocure");
update_handler!(revise_carrier_contract, "contract.revise");
update_handler!(withdraw_carrier_contract, "contract.withdraw");
update_handler!(contract_offer, "quote.contract-offer");
update_handler!(contract_reprocure, "quote.contract-reprocure");
update_handler!(propose_cancellation, "cancellation.propose");
update_handler!(review_cancellation, "cancellation.review");
update_handler!(reverse_cancellation, "cancellation.reverse");
update_handler!(update_carrier, "carrier.update");
update_handler!(update_customer, "customer.update");
update_handler!(update_driver, "driver.update");
update_handler!(update_equipment, "equipment.update");
update_handler!(retry_inbox, "inbox.retry");
update_handler!(review_document, "document.review");
update_handler!(review_pod, "pod.review");
update_handler!(revise_bill, "bill.revise");
update_handler!(create_supplement, "bill.supplement");
update_handler!(issue_bill, "bill.issue");
update_handler!(void_bill, "bill.void");
update_handler!(issue_rate_confirmation, "rate.issue");
update_handler!(publish_portal_offer, "portal.publish");
update_handler!(publish_release_request, "portal.release.publish");
update_handler!(apply_release_response, "portal.release.apply");
update_handler!(publish_customer_quote, "portal.quote.publish");
update_handler!(record_rate_handoff, "rate.handoff");
update_handler!(retract_rate_handoff, "rate.handoff.retract");
update_handler!(resolve_rate_handoff, "rate.handoff.resolve");
update_handler!(create_credit, "credit.create");
update_handler!(reverse_credit, "credit.reverse");
update_handler!(reverse_cash, "cash.reverse");
update_handler!(reverse_carrier_credit, "carrier_credit.reverse");
update_handler!(request_release, "tender.release.request");
update_handler!(confirm_release, "tender.release.confirm");
update_handler!(retain_carrier, "tender.release.retain");
update_handler!(resolve_tender_incident, "tender.incident.resolve");
update_handler!(book_appointment, "stop.appointment.book");
update_handler!(propose_charge, "charge.propose");
update_handler!(create_carrier_adjustment, "payable.create");
update_handler!(review_carrier_adjustment, "payable.review");
update_handler!(reverse_carrier_adjustment, "payable.reverse");
update_handler!(dismiss_carrier_document, "payable.document.dismiss");
update_handler!(apply_carrier_revision, "payable.document.apply_revision");
update_handler!(review_charge, "charge.review");
update_handler!(void_charge, "charge.void");
update_handler!(cancel_appointment, "stop.appointment.cancel");
update_handler!(record_stop_event, "stop.event.record");
update_handler!(record_stop_service, "itinerary.service.record");
update_handler!(correct_stop_service, "itinerary.service.correct");
update_handler!(correct_movement_time, "movement.time.correct");
create_handler!(operations_policy, "operations.policy");
create_handler!(operations_reconcile, "operations.reconcile");
async fn operations_board(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<crate::operations::QueueQuery>,
) -> Result<Response, Error> {
    let bytes = app
        .store
        .read_project(&tenant.0, move |workspace| {
            workspace.operations_page(query, &tenant.1, chrono::Utc::now())
        })
        .await?;
    Ok(encoded_json(bytes))
}
async fn operations_case(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let bytes = app
        .store
        .read_project(&tenant.0, move |workspace| {
            workspace.operations_case(&id, chrono::Utc::now())
        })
        .await?;
    Ok(encoded_json(bytes))
}
async fn operations_action(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::operations::CaseInput = serde_json::from_value(body).map_err(|_| {
        Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid operations action fields".into(),
        )
    })?;
    if request.action == "force_release" && tenant.1.role != crate::auth::Role::Admin {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Administrator required to release another owner".into(),
        ));
    }
    let trusted = crate::operations::trusted_action(&tenant.1, request);
    mutate(app, tenant, headers, "operations.case.action", &id, trusted).await
}
update_handler!(correct_stop_event, "stop.event.correct");

#[derive(serde::Deserialize)]
struct QuoteDocumentQuery {
    revision: Option<u64>,
}
async fn contract_candidates(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    Ok(Json(
        app.store.read(&tenant.0).await?.contract_candidates(&id)?,
    ))
}
async fn carrier_contract_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(query): Query<QuoteDocumentQuery>,
) -> Result<Response, Error> {
    let w = app.store.read(&tenant.0).await?;
    let c = w
        .carrier_contracts
        .iter()
        .find(|c| c.id == id)
        .ok_or("Carrier contract not found".to_string())?;
    let r = if let Some(revision) = query.revision {
        c.revisions
            .iter()
            .find(|r| r.revision == revision)
            .ok_or("Contract revision not found".to_string())?
    } else {
        c.current()?
    };
    r.verify()?;
    if r.agreement_id != c.id {
        return Err("Internal storage error".to_string().into());
    }
    Ok(partner_attachment(
        format!("carrier-contract-{}-{}", c.id, r.revision),
        r.html.clone(),
        Some(r.sha256.clone()),
    ))
}
async fn quote_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(query): Query<QuoteDocumentQuery>,
) -> Result<Response, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    let (name, html) = crate::quotes::statement(&workspace, &id, query.revision)?;
    Ok((
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            (
                "content-disposition",
                format!("attachment; filename=\"{name}.html\""),
            ),
            ("cache-control", "private, no-store".into()),
            ("x-content-type-options", "nosniff".into()),
            (
                "content-security-policy",
                "default-src 'none'; style-src 'unsafe-inline'".into(),
            ),
        ],
        html,
    )
        .into_response())
}
async fn partner_workspace(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
) -> Result<Json<Value>, Error> {
    let w = app.store.read(&tenant.0).await?;
    let mut value = w.partner_workspace(&tenant.1)?;
    value["files"] = json!(w.shared_file_views(&tenant.1)?);
    if tenant.1.role == crate::auth::Role::Carrier {
        value["release_requests"] = json!(w.release_portal_views(
            tenant
                .1
                .counterparty_id
                .as_deref()
                .ok_or("Carrier scope required".to_owned())?
        )?);
    }
    value["mode"] = json!(if app.demo && tenant.0 == "demo" {
        "demo"
    } else {
        "live"
    });
    Ok(Json(value))
}
async fn partner_response(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::portal::ResponseInput = serde_json::from_value(body).map_err(|_| {
        Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid carrier response fields".into(),
        )
    })?;
    let cp = tenant
        .1
        .counterparty_id
        .clone()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Carrier scope required".into()))?;
    app.store
        .read(&tenant.0)
        .await?
        .portal_offer_for(&id, &cp)?;
    let trusted = json!({"carrier_id":cp,"subject":tenant.1.subject,"request":request});
    mutate(app, tenant, headers, "portal.respond", &id, trusted).await
}
async fn partner_stop_service(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::itinerary::ServiceInput = serde_json::from_value(body).map_err(|_| {
        Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid carrier stop service fields".into(),
        )
    })?;
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Carrier scope required".into()))?;
    app.store.read(&tenant.0).await?.portal_offer_for(&id, cp)?;
    let body = json!({"carrier_id":cp,"subject":tenant.1.subject,"request":request});
    mutate(app, tenant, headers, "portal.itinerary.service", &id, body).await
}
async fn carrier_release_response(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::release_portal::ResponseInput =
        serde_json::from_value(body).map_err(|_| {
            Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid carrier release response fields".into(),
            )
        })?;
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Carrier scope required".into()))?;
    app.store
        .read(&tenant.0)
        .await?
        .release_publication_for(&id, cp)?;
    let body = json!({"carrier_id":cp,"subject":tenant.1.subject,"request":request});
    mutate(app, tenant, headers, "portal.release.respond", &id, body).await
}
async fn carrier_release_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    if tenant.1.role != crate::auth::Role::Carrier {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Carrier release publication not found".into(),
        ));
    }
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Carrier scope required".into()))?;
    let w = app.store.read(&tenant.0).await?;
    let p = w.release_publication_for(&id, cp)?;
    p.verify()?;
    Ok(partner_attachment(
        format!("carrier-release-{}", p.snapshot.shipment_reference),
        p.html.clone(),
        Some(p.sha256.clone()),
    ))
}
async fn staff_release_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let w = app.store.read(&tenant.0).await?;
    let p = w
        .release_publications
        .iter()
        .find(|p| p.id == id)
        .ok_or("Carrier release publication not found".to_owned())?;
    p.verify()?;
    Ok(partner_attachment(
        format!("carrier-release-{}", p.snapshot.shipment_reference),
        p.html.clone(),
        Some(p.sha256.clone()),
    ))
}
async fn share_document(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::sharing::CreateShare = serde_json::from_value(body).map_err(|_| {
        Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid document sharing fields".into(),
        )
    })?;
    let body = json!({"role":tenant.1.role,"request":request});
    mutate(app, tenant, headers, "share.create", &id, body).await
}
async fn revoke_document_share(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::sharing::RevokeShare = serde_json::from_value(body).map_err(|_| {
        Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid document share revocation fields".into(),
        )
    })?;
    let body = json!({"role":tenant.1.role,"request":request});
    mutate(app, tenant, headers, "share.revoke", &id, body).await
}
async fn shared_document_content(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let (document, bytes) = app.store.shared_document_content(&tenant.1, &id).await?;
    document_attachment(document, bytes)
}
async fn customer_quote_response(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::customer_portal::CustomerResponse =
        serde_json::from_value(body).map_err(|_| {
            Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid customer quote response fields".into(),
            )
        })?;
    let cp = tenant
        .1
        .counterparty_id
        .clone()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Customer scope required".into()))?;
    app.store
        .read(&tenant.0)
        .await?
        .customer_publication_for(&id, &cp)?;
    let body = json!({"customer_id":cp,"subject":tenant.1.subject,"request":request});
    mutate(app, tenant, headers, "portal.quote.respond", &id, body).await
}
async fn customer_quote_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Customer scope required".into()))?;
    if tenant.1.role != crate::auth::Role::Customer {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Customer quote publication not found".into(),
        ));
    }
    let w = app.store.read(&tenant.0).await?;
    let p = w.customer_publication_for(&id, cp)?;
    p.verify()?;
    Ok(partner_attachment(
        format!("customer-quote-{}-r{}", p.snapshot.reference, p.revision),
        p.html.clone(),
        Some(p.sha256.clone()),
    ))
}
async fn customer_quote_source(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    if headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .is_none()
    {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "Idempotency-Key (1–200 characters) required".into(),
        ));
    }
    if body != json!({}) {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Source verification accepts only an empty object".into(),
        ));
    }
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Customer scope required".into()))?;
    let w = app.store.read(&tenant.0).await?;
    let p = w.customer_publication_for(&id, cp)?;
    p.verify()?;
    if !w.customer_publication_current(p) {
        return Err(Error(
            StatusCode::CONFLICT,
            "Customer publication changed; refresh before source verification".into(),
        ));
    }
    let q = w
        .quotes
        .iter()
        .find(|q| q.id == p.quote_id)
        .ok_or("Quote not found".to_owned())?;
    if q.status != "issued" {
        return Err(Error(
            StatusCode::CONFLICT,
            "Quote changed; source verification requires an outstanding issued revision".into(),
        ));
    }
    let load = q
        .current()?
        .source_load
        .as_ref()
        .filter(|l| l.sync_order_id.is_some() && l.customer_id == cp)
        .ok_or("Quote source not found".to_owned())?;
    app.integrations
        .refresh_source(&app.store, &tenant.0, &load.id, &tenant.1.subject, None)
        .await?;
    let w = app.store.read(&tenant.0).await?;
    Ok(Json(
        json!({"data":w.customer_quote_view(w.customer_publication_for(&id,cp)?)?}),
    ))
}
async fn partner_tracking(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let request: crate::carrier_portal::TrackingInput =
        serde_json::from_value(body).map_err(|_| {
            Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Invalid carrier tracking fields".into(),
            )
        })?;
    let cp = tenant
        .1
        .counterparty_id
        .clone()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Carrier scope required".into()))?;
    app.store
        .read(&tenant.0)
        .await?
        .portal_offer_for(&id, &cp)?;
    let trusted = json!({"carrier_id":cp,"subject":tenant.1.subject,"request":request});
    mutate(app, tenant, headers, "portal.tracking", &id, trusted).await
}
async fn partner_pod(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(input): Query<crate::carrier_portal::PodInput>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    Ok(Json(
        app.store
            .submit_carrier_pod(&tenant.1, key, &id, input, bytes.to_vec(), content_type)
            .await?,
    ))
}
async fn partner_submission_content(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let cp = tenant
        .1
        .counterparty_id
        .as_deref()
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Partner scope required".into()))?;
    let w = app.store.read(&tenant.0).await?;
    if tenant.1.role != crate::auth::Role::Carrier {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Carrier submission not found".into(),
        ));
    }
    let submission = w.carrier_submission_for(&id, cp)?;
    let docid = submission
        .document_id
        .as_deref()
        .ok_or("Carrier submission file not found".to_owned())?;
    let (document, bytes) = app.store.document_content(&tenant.0, docid).await?;
    if document.load_id != submission.load_id
        || submission.document_sha256.as_deref() != Some(&document.sha256)
    {
        return Err("Internal storage error".to_owned().into());
    }
    document_attachment(document, bytes)
}
fn partner_attachment(name: String, html: String, digest: Option<String>) -> Response {
    let mut headers = HeaderMap::new();
    for (k, v) in [
        ("content-type", "text/html; charset=utf-8".to_owned()),
        (
            "content-disposition",
            format!("attachment; filename=\"{name}.html\""),
        ),
        ("cache-control", "private, no-store".into()),
        ("x-content-type-options", "nosniff".into()),
        (
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'".into(),
        ),
    ] {
        if let Ok(v) = axum::http::HeaderValue::from_str(&v) {
            headers.insert(axum::http::HeaderName::from_static(k), v);
        }
    }
    if let Some(hash) = digest {
        if let Ok(v) = axum::http::HeaderValue::from_str(&hash) {
            headers.insert("x-content-sha256", v);
        }
    }
    (headers, html).into_response()
}
async fn partner_rate_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let w = app.store.read(&tenant.0).await?;
    if tenant.1.role != crate::auth::Role::Carrier
        || !w.portal_offers.iter().any(|o| {
            o.rate_confirmation_id == id && Some(&o.carrier_id) == tenant.1.counterparty_id.as_ref()
        })
    {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Published carrier document not found".into(),
        ));
    }
    let (name, html, hash) = crate::rate_confirmations::statement(&w, &id)?;
    Ok(partner_attachment(name, html, Some(hash)))
}
async fn partner_bill_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let w = app.store.read(&tenant.0).await?;
    if tenant.1.role != crate::auth::Role::Customer
        || !w.bills.iter().any(|b| {
            b.id == id
                && Some(&b.customer_id) == tenant.1.counterparty_id.as_ref()
                && b.issued_at.is_some()
        })
    {
        return Err(Error(
            StatusCode::NOT_FOUND,
            "Customer invoice not found".into(),
        ));
    }
    let (name, html) = crate::finance::statement(&w, &id)?;
    Ok(partner_attachment(name, html, None))
}
async fn rate_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    let (name, html, digest) = crate::rate_confirmations::statement(&workspace, &id)?;
    Ok((
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            (
                "content-disposition",
                format!("attachment; filename=\"{name}.html\""),
            ),
            ("cache-control", "private, no-store".into()),
            ("x-content-type-options", "nosniff".into()),
            (
                "content-security-policy",
                "default-src 'none'; style-src 'unsafe-inline'".into(),
            ),
            ("x-content-sha256", digest),
        ],
        html,
    )
        .into_response())
}
async fn bill_statement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    let (name, html) = crate::finance::statement(&workspace, &id)?;
    Ok((
        [
            ("content-type", "text/html; charset=utf-8".to_owned()),
            (
                "content-disposition",
                format!("attachment; filename=\"{name}.html\""),
            ),
            ("cache-control", "private, no-store".into()),
            ("x-content-type-options", "nosniff".into()),
            (
                "content-security-policy",
                "default-src 'none'; style-src 'unsafe-inline'".into(),
            ),
        ],
        html,
    )
        .into_response())
}
async fn journal_csv(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
) -> Result<Response, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    Ok((
        [
            ("content-type", "text/csv; charset=utf-8"),
            (
                "content-disposition",
                "attachment; filename=\"freight-journal.csv\"",
            ),
            ("cache-control", "private, no-store"),
            ("x-content-type-options", "nosniff"),
        ],
        crate::finance::journal_csv(&workspace),
    )
        .into_response())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadQuery {
    kind: String,
    filename: String,
}
async fn upload_document(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    Ok(Json(
        app.store
            .upload_document(
                (&tenant.0, &tenant.1.subject),
                key,
                &id,
                Upload {
                    kind: query.kind,
                    filename: query.filename,
                    content_type: content_type.into(),
                    bytes: bytes.to_vec(),
                },
            )
            .await?,
    ))
}
async fn document_content(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let (document, bytes) = app.store.document_content(&tenant.0, &id).await?;
    document_attachment(document, bytes)
}
fn document_attachment(
    document: crate::documents::Document,
    bytes: Vec<u8>,
) -> Result<Response, Error> {
    // Always download untrusted evidence as an attachment, never inline HTML.
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        "content-type",
        document
            .content_type
            .parse()
            .map_err(|_| "Internal storage error".to_owned())?,
    );
    headers.insert(
        "cache-control",
        "private, no-store"
            .parse()
            .map_err(|_| "Internal storage error".to_owned())?,
    );
    headers.insert(
        "x-content-type-options",
        "nosniff"
            .parse()
            .map_err(|_| "Internal storage error".to_owned())?,
    );
    headers.insert(
        "x-content-sha256",
        document
            .sha256
            .parse()
            .map_err(|_| "Internal storage error".to_owned())?,
    );
    let encoded: String = document
        .filename
        .as_bytes()
        .iter()
        .map(|v| format!("%{v:02X}"))
        .collect();
    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"shipment-document\"; filename*=UTF-8''{encoded}")
            .parse()
            .map_err(|_| "Internal storage error".to_owned())?,
    );
    Ok(response)
}
async fn command(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    mutate(app, tenant, headers, "load.command", &id, body).await
}
async fn approve_invoice(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, Error> {
    mutate(
        app,
        tenant,
        headers,
        "invoice.approve",
        &id,
        body.map(|v| v.0).unwrap_or_else(|| json!({})),
    )
    .await
}
async fn review_invoice_entry(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    mutate(app, tenant, headers, "invoice.entry_review", &id, body).await
}
async fn settlement(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, Error> {
    mutate(app, tenant, headers, "settlement.create", &id, json!({})).await
}
async fn edi_event(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    mut headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    if body
        .get("payload")
        .and_then(|v| v.get("tenant_id"))
        .and_then(Value::as_str)
        .is_some_and(|v| v != tenant.0)
    {
        return Err(Error(
            StatusCode::FORBIDDEN,
            "Event tenant does not match credential".into(),
        ));
    }
    let event_id = body
        .get("event_id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 150)
        .ok_or_else(|| Error(StatusCode::BAD_REQUEST, "Event ID required".into()))?;
    headers.insert(
        "idempotency-key",
        format!("edi:{event_id}")
            .parse()
            .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid event ID".into()))?,
    );
    mutate(app, tenant, headers, "edi.event", "", body).await
}

async fn resolve_invoice(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    mutate(app, tenant, headers, "invoice.resolve", &id, body).await
}
async fn retry_outbox(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, Error> {
    mutate(app, tenant, headers, "outbox.retry", &id, json!({})).await
}
async fn sync_orders(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, Error> {
    let page = query
        .get("page")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1);
    app.integrations
        .orders(&tenant.0, page)
        .await
        .map(Json)
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))
}
async fn import_order(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    if let Some(response) = app
        .store
        .replay(&tenant.0, key, "sync.import", "", &body)
        .await?
    {
        return Ok(Json(response));
    }
    let order_id = body["order_id"]
        .as_str()
        .ok_or_else(|| Error(StatusCode::BAD_REQUEST, "order_id required".into()))?;
    let order = app
        .integrations
        .order(&tenant.0, order_id)
        .await
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))?;
    mutate(
        app,
        tenant,
        headers,
        "sync.import",
        "",
        json!({"order":order,"load":body["load"]}),
    )
    .await
}
async fn edi_callback(
    State(app): State<App>,
    Path(tenant): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, Error> {
    let unauthorized = |message: String| Error(StatusCode::UNAUTHORIZED, message);
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| unauthorized(format!("Missing {name}")))
    };
    let connection = app
        .integrations
        .tenants
        .get(&tenant)
        .ok_or_else(|| unauthorized("Callback authentication failed".into()))?;
    let secret = connection
        .callback_keys
        .get(header("x-stateset-key-version")?)
        .ok_or_else(|| unauthorized("Unknown callback key version".into()))?;
    let callback_id = header("x-stateset-event-id")?;
    if callback_id.is_empty() || callback_id.len() > 150 {
        return Err(unauthorized("Invalid callback ID".into()));
    }
    verify_callback(
        secret,
        header("x-stateset-timestamp")?,
        callback_id,
        header("x-stateset-signature")?,
        &body,
    )
    .map_err(unauthorized)?;
    let notification: Value = serde_json::from_slice(&body)
        .map_err(|_| Error(StatusCode::BAD_REQUEST, "Invalid callback JSON".into()))?;
    let supported = app
        .integrations
        .validate_notification(&tenant, &notification)
        .map_err(|e| Error(StatusCode::BAD_REQUEST, e))?;
    if !supported {
        return Ok(Json(json!({"received":true,"action":"ignored"})));
    }
    Ok(Json(
        app.store
            .receive_document(
                &tenant,
                callback_id,
                header("x-stateset-key-version")?,
                notification,
                String::from_utf8(body.to_vec()).map_err(|_| {
                    Error(
                        StatusCode::BAD_REQUEST,
                        "Callback body must be UTF-8".into(),
                    )
                })?,
                Some(crate::inbox::CallbackAuthentication {
                    delivery_id: callback_id.into(),
                    key_version: header("x-stateset-key-version")?.into(),
                    timestamp: header("x-stateset-timestamp")?.into(),
                    signature: header("x-stateset-signature")?.into(),
                }),
            )
            .await?,
    ))
}

#[cfg(test)]
#[path = "workspace_projection_tests.rs"]
mod workspace_projection_tests;

#[path = "workspace_response.rs"]
mod workspace_response;

#[cfg(test)]
#[path = "direct_workspace_tests.rs"]
mod direct_workspace_tests;

#[path = "document_preview.rs"]
mod document_preview;

async fn commerce_orders(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, Error> {
    app.integrations
        .tenants
        .get(&tenant.0)
        .filter(|c| c.commerce_url.is_some())
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_GATEWAY,
                "iCommerce is not configured for this freight tenant".into(),
            )
        })?;
    sync_orders(State(app), Extension(tenant), Query(query)).await
}
async fn commerce_order(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let c = app
        .integrations
        .tenants
        .get(&tenant.0)
        .filter(|c| c.commerce_url.is_some())
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_GATEWAY,
                "iCommerce is not configured for this freight tenant".into(),
            )
        })?;
    crate::commerce::order(c, &id)
        .await
        .map(|data| Json(json!({"data":data})))
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))
}
async fn commerce_import(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    if !body.is_object()
        || body.as_object().is_some_and(|v| v.len() != 2)
        || body.get("order_id").is_none()
        || body.get("load").is_none()
    {
        return Err(Error(StatusCode::UNPROCESSABLE_ENTITY,"iCommerce import accepts only order_id and load; source and identity are fetched by the server".into()));
    }
    // Replay remains available even if the connection is unavailable after a lost acknowledgement.
    if let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) {
        if let Some(receipt) = app
            .store
            .replay(&tenant.0, key, "sync.import", "", &body)
            .await?
        {
            return Ok(Json(receipt));
        }
    }
    app.integrations
        .tenants
        .get(&tenant.0)
        .filter(|c| c.commerce_url.is_some())
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_GATEWAY,
                "iCommerce is not configured for this freight tenant".into(),
            )
        })?;
    import_order(State(app), Extension(tenant), headers, Json(body)).await
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CommerceShipmentsQuery {
    #[serde(default = "first_commerce_page")]
    page: u32,
}
fn first_commerce_page() -> u32 {
    1
}
async fn commerce_shipments(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    query: Result<Query<CommerceShipmentsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Value>, Error> {
    let Query(query) = query.map_err(|_| {
        Error(
            StatusCode::BAD_REQUEST,
            "Only one positive shipment page is accepted; order identity comes from the load"
                .into(),
        )
    })?;
    if query
        .page
        .checked_sub(1)
        .and_then(|n| n.checked_mul(crate::commerce::SHIPMENT_PAGE_SIZE))
        .is_none()
    {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "Shipment page is outside supported bounds".into(),
        ));
    }
    let workspace = app.store.read(&tenant.0).await?;
    let load = workspace
        .loads
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "Load not found".into()))?;
    let _order = crate::commerce::candidate_order(load)
        .map_err(|e| Error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let c = app
        .integrations
        .tenants
        .get(&tenant.0)
        .ok_or_else(|| Error(StatusCode::BAD_GATEWAY, "iCommerce not configured".into()))?;
    crate::commerce::shipment_candidates(&workspace, load, c, query.page)
        .await
        .map(|data| Json(json!({"data":data})))
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))
}
async fn commerce_tracking_preview(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, Error> {
    let shipment = query
        .get("shipment_id")
        .ok_or_else(|| Error(StatusCode::BAD_REQUEST, "shipment_id required".into()))?;
    let workspace = app.store.read(&tenant.0).await?;
    let load = workspace
        .loads
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "Load not found".into()))?;
    let c = app
        .integrations
        .tenants
        .get(&tenant.0)
        .ok_or_else(|| Error(StatusCode::BAD_GATEWAY, "iCommerce not configured".into()))?;
    let native = crate::commerce::shipment(c, shipment)
        .await
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))?;
    crate::commerce::guard_link(&workspace, &id, native.id)
        .map_err(|e| Error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let plan = crate::commerce::tracking_plan(load, &native)
        .map_err(|e| Error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(
        json!({"data":{"load_id":id,"expected_load_version":load.version,"shipment":native,"plan":plan,"up_to_date":plan.is_none()}}),
    ))
}
async fn commerce_tracking(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    if let Some(receipt) = app
        .store
        .replay(&tenant.0, key, "commerce.tracking", &id, &body)
        .await?
    {
        return Ok(Json(receipt));
    }
    let request: crate::commerce::TrackingRequest =
        serde_json::from_value(body.clone()).map_err(|_| {
            Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Expected load version, shipment UUID and expected shipment version are required"
                    .into(),
            )
        })?;
    let c = app
        .integrations
        .tenants
        .get(&tenant.0)
        .ok_or_else(|| Error(StatusCode::BAD_GATEWAY, "iCommerce not configured".into()))?;
    let native = crate::commerce::shipment(c, &request.shipment_id.to_string())
        .await
        .map_err(|e| Error(StatusCode::BAD_GATEWAY, e))?;
    if native.version != request.expected_shipment_version {
        return Err(Error(
            StatusCode::CONFLICT,
            "Native shipment changed; review again".into(),
        ));
    }
    let binding = crate::commerce::binding(c).map_err(|e| Error(StatusCode::BAD_GATEWAY, e))?;
    mutate(
        app,
        tenant,
        headers,
        "commerce.tracking",
        &id,
        json!({"request":body,"verified_shipment":native,"binding":binding}),
    )
    .await
}

async fn commerce_review(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let workspace = app.store.read(&tenant.0).await?;
    let connection = app
        .integrations
        .tenants
        .get(&tenant.0)
        .ok_or_else(|| Error(StatusCode::BAD_GATEWAY, "iCommerce not configured".into()))?;
    crate::commerce::review_preview(&workspace, &id, connection)
        .await
        .map(|data| Json(json!({"data":data})))
        .map_err(|e| Error(StatusCode::UNPROCESSABLE_ENTITY, e))
}
async fn commerce_reconcile(
    State(app): State<App>,
    Extension(tenant): Extension<Tenant>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Error> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty() && k.len() <= 200)
        .ok_or_else(|| {
            Error(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key (1–200 characters) required".into(),
            )
        })?;
    if let Some(receipt) = app
        .store
        .replay(&tenant.0, key, "commerce.reconcile", &id, &body)
        .await?
    {
        return Ok(Json(receipt));
    }
    let request:crate::commerce::ReconcileRequest=serde_json::from_value(body.clone()).map_err(|_|Error(StatusCode::UNPROCESSABLE_ENTITY,"Expected load/native versions, original attempt counter, contact, reference and note are required".into()))?;
    let workspace = app.store.read(&tenant.0).await?;
    let connection = app
        .integrations
        .tenants
        .get(&tenant.0)
        .ok_or_else(|| Error(StatusCode::BAD_GATEWAY, "iCommerce not configured".into()))?;
    let preview = crate::commerce::review_preview(&workspace, &id, connection)
        .await
        .map_err(|e| Error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    if preview["shipment"]["version"] != request.expected_shipment_version
        || preview["expected_load_version"] != request.expected_load_version
        || preview["expected_total_attempts"] != request.expected_total_attempts
    {
        return Err(Error(
            StatusCode::CONFLICT,
            "Shipment, load or original handoff attempts changed; preview again".into(),
        ));
    }
    let configured =
        crate::commerce::binding(connection).map_err(|e| Error(StatusCode::BAD_GATEWAY, e))?;
    mutate(
        app,
        tenant,
        headers,
        "commerce.reconcile",
        &id,
        json!({"request":body,"verified_shipment":preview["shipment"],"binding":configured}),
    )
    .await
}
