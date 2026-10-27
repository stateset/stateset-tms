//! Native iCommerce HTTP adapter. Connection/tenant/credential are operator configuration.
use crate::domain::{id, Load, Outbox, Workspace};
use crate::integrations::Connection;
use reqwest::{header, Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use stateset_freight_commerce::{OrderId, OrderPage, Shipment, ShipmentId, TrackingPlan};
use std::time::Duration;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
pub fn validate_origin(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|_| "Invalid iCommerce origin")?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !(url.scheme() == "https"
            || url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
    {
        return Err("iCommerce needs an HTTPS origin without path/credentials; HTTP is permitted only on loopback".into());
    }
    Ok(url)
}
fn client(c: &Connection) -> Result<Client, String> {
    let mut headers = header::HeaderMap::new();
    let mut auth = header::HeaderValue::from_str(&format!(
        "Bearer {}",
        c.commerce_token
            .as_deref()
            .ok_or("iCommerce credential missing")?
    ))
    .map_err(|_| "Invalid iCommerce credential")?;
    auth.set_sensitive(true);
    headers.insert(header::AUTHORIZATION, auth);
    headers.insert(
        "x-tenant-id",
        header::HeaderValue::from_str(
            c.commerce_tenant_id
                .as_deref()
                .ok_or("iCommerce tenant missing")?,
        )
        .map_err(|_| "Invalid iCommerce tenant")?,
    );
    Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "Cannot initialize iCommerce verification client".into())
}
async fn read(c: &Connection, path: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let mut url = validate_origin(
        c.commerce_url
            .as_deref()
            .ok_or("iCommerce connection not configured")?,
    )?;
    url.set_path(path);
    let response = client(c)?
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|_| "iCommerce request failed")?;
    if !response.status().is_success() {
        return Err(format!(
            "iCommerce returned HTTP {}",
            response.status().as_u16()
        ));
    }
    response_value(response).await
}
async fn response_value(mut response: reqwest::Response) -> Result<Value, String> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err("iCommerce response exceeds the bounded adapter limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "iCommerce response interrupted")?
    {
        if chunk.len() > MAX_RESPONSE.saturating_sub(bytes.len()) {
            return Err("iCommerce response exceeds the bounded adapter limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "iCommerce returned invalid JSON".into())
}
pub async fn order(c: &Connection, id: &str) -> Result<Value, String> {
    let id: OrderId = id
        .parse()
        .map_err(|_| "iCommerce order ID must be a UUID")?;
    let value = read(c, &format!("/api/v1/orders/{id}"), &[]).await?;
    stateset_freight_commerce::order_snapshot(value, id)
}
pub async fn orders(c: &Connection, page: u32) -> Result<Value, String> {
    let page = page.max(1);
    let offset = page
        .checked_sub(1)
        .and_then(|n| n.checked_mul(100))
        .ok_or("iCommerce page offset exhausted")?;
    let value = read(
        c,
        "/api/v1/orders",
        &[("limit", "100".into()), ("offset", offset.to_string())],
    )
    .await?;
    serde_json::from_value::<OrderPage>(value)
        .map_err(|_| "Invalid iCommerce order page")?
        .normalized(page)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    origin: String,
    tenant_id: String,
}
pub fn binding(c: &Connection) -> Result<Binding, String> {
    Ok(Binding {
        origin: validate_origin(
            c.commerce_url
                .as_deref()
                .ok_or("iCommerce not configured")?,
        )?
        .to_string(),
        tenant_id: c
            .commerce_tenant_id
            .clone()
            .ok_or("iCommerce tenant missing")?,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingRequest {
    pub expected_load_version: u64,
    pub shipment_id: ShipmentId,
    pub expected_shipment_version: i32,
}
pub async fn shipment(c: &Connection, raw: &str) -> Result<Shipment, String> {
    let id: ShipmentId = raw
        .parse()
        .map_err(|_| "Native shipment ID must be a UUID")?;
    let value = read(c, &format!("/api/v1/shipments/{id}"), &[]).await?;
    let shipment: Shipment =
        serde_json::from_value(value).map_err(|_| "Invalid native shipment response")?;
    if shipment.id != id {
        return Err("Native shipment identity mismatch".into());
    }
    Ok(shipment)
}
pub fn tracking_plan(load: &Load, shipment: &Shipment) -> Result<Option<TrackingPlan>, String> {
    let source = load
        .source_order
        .as_ref()
        .ok_or("Load has no retained commerce order")?;
    if load.sync_order_id.as_deref() != source["id"].as_str() {
        return Err("Load source identity mismatch".into());
    }
    let status = serde_json::to_value(&load.status).map_err(|_| "Invalid freight status")?;
    TrackingPlan::new(
        shipment,
        source,
        status.as_str().ok_or("Invalid freight status")?,
    )
}
pub fn guard_link(w: &Workspace, load_id: &str, shipment_id: ShipmentId) -> Result<(), String> {
    let shipment_id = shipment_id.to_string();
    if w.outbox.iter().any(|j| {
        j.load_id == load_id
            && j.destination == "stateset-icommerce"
            && j.payload["plan"]["shipment_id"].as_str() != Some(shipment_id.as_str())
    }) {
        return Err("This load already has a retained native shipment identity; a different shipment cannot reuse its physical execution".into());
    }
    Ok(())
}
pub fn enqueue(w: &mut Workspace, load_id: &str, body: Value) -> Result<Value, String> {
    let request: TrackingRequest =
        serde_json::from_value(body["request"].clone()).map_err(|_| "Invalid tracking review")?;
    let native: Shipment = serde_json::from_value(body["verified_shipment"].clone())
        .map_err(|_| "Verified native shipment missing")?;
    let binding: Binding = serde_json::from_value(body["binding"].clone())
        .map_err(|_| "Verified connection binding missing")?;
    validate_origin(&binding.origin)?;
    guard_link(w, load_id, native.id)?;
    let load = w
        .loads
        .iter()
        .find(|v| v.id == load_id)
        .ok_or("Load not found")?;
    if load.version != request.expected_load_version
        || native.version != request.expected_shipment_version
        || native.id != request.shipment_id
    {
        return Err("Shipment or load changed; review again".into());
    }
    w.guard_source(load)?;
    let plan = tracking_plan(load, &native)?
        .ok_or("Native shipment already reflects this freight state")?;
    let canonical = w
        .source_check(load_id)
        .ok_or(crate::source::SOURCE_UNAVAILABLE)?;
    let status = serde_json::to_value(&load.status).map_err(|_| "Invalid freight status")?;
    let current_plan = TrackingPlan::new(
        &native,
        &canonical.snapshot,
        status.as_str().ok_or("Invalid freight status")?,
    )?
    .ok_or("Native shipment already reflects freight state")?;
    if current_plan.body() != plan.body() {
        return Err("Current canonical order no longer matches this shipment review".into());
    }
    if w.outbox.iter().any(|v| {
        v.load_id == load_id
            && v.destination == "stateset-icommerce"
            && matches!(
                v.status.as_str(),
                "pending_handoff" | "sending" | "retry" | "held"
            )
    }) {
        return Err(
            "Resolve the existing native shipment handoff before creating another review".into(),
        );
    }
    let job = Outbox {
        id: id("outbox"),
        load_id: load_id.into(),
        destination: "stateset-icommerce".into(),
        status: "pending_handoff".into(),
        method: "PATCH".into(),
        path: format!("/api/v1/shipments/{}", plan.shipment_id),
        payload: json!({"body":plan.body(),"plan":plan,"binding":binding,"verified_shipment":native}),
        attempts: 0,
        next_attempt_at: None,
        leased_until: None,
        lease: None,
        last_error: None,
        response: None,
        tender_id: None,
        transmission: "not_started".into(),
        total_attempts: 0,
    };
    let result = json!({"job":job,"plan":plan});
    w.outbox.push(job);
    w.record(
        load_id,
        "commerce.tracking",
        "Explicit native shipment lifecycle step queued with original version and manifest",
    );
    Ok(result)
}
/// Recheck the current canonical manifest under the tenant claim transaction.
pub(crate) fn authorize_handoff(w: &Workspace, job: &Outbox) -> Result<(), String> {
    let load = w
        .loads
        .iter()
        .find(|l| l.id == job.load_id)
        .ok_or("Load not found")?;
    w.guard_source(load)?;
    let native: Shipment = serde_json::from_value(job.payload["verified_shipment"].clone())
        .map_err(|_| "Original native shipment missing")?;
    let plan = tracking_plan(load, &native)?.ok_or("Native handoff no longer requires a step")?;
    let snapshot = &w
        .source_check(&load.id)
        .ok_or(crate::source::SOURCE_UNAVAILABLE)?
        .snapshot;
    let status = serde_json::to_value(&load.status).map_err(|_| "Invalid freight status")?;
    let canonical = TrackingPlan::new(
        &native,
        snapshot,
        status.as_str().ok_or("Invalid freight status")?,
    )?
    .ok_or("Native handoff no longer requires a step")?;
    if canonical.body() != job.payload["body"] || plan.body() != job.payload["body"] {
        return Err(
            "Current source or freight state no longer authorizes the frozen native handoff".into(),
        );
    }
    Ok(())
}
fn frozen_intent(job: &Outbox) -> Result<(Binding, Shipment, TrackingPlan), String> {
    let saved: Binding = serde_json::from_value(job.payload["binding"].clone())
        .map_err(|_| "Native handoff binding missing")?;
    let original: Shipment = serde_json::from_value(job.payload["verified_shipment"].clone())
        .map_err(|_| "Original manifest missing")?;
    let plan: TrackingPlan = serde_json::from_value(job.payload["plan"].clone())
        .map_err(|_| "Native handoff plan missing")?;
    if job.method != "PATCH"
        || job.path != format!("/api/v1/shipments/{}", plan.shipment_id)
        || job.payload["body"] != plan.body()
        || plan.shipment_id != original.id
        || plan.order_id != original.order_id
        || plan.expected_version != original.version
        || plan.from_status != original.status
    {
        return Err("Native handoff intent is inconsistent".into());
    }
    if plan.expected_version < 0
        || plan.expected_version.checked_add(1).is_none()
        || !matches!(
            (
                plan.from_status.as_str(),
                plan.next_status.as_str(),
                plan.target_status.as_str()
            ),
            (
                "shipped" | "failed",
                "in_transit",
                "in_transit" | "delivered"
            ) | ("in_transit", "out_for_delivery", "delivered")
                | ("out_for_delivery", "delivered", "delivered")
        )
    {
        return Err("Native handoff lifecycle plan invalid".into());
    }
    Ok((saved, original, plan))
}
fn same_manifest(a: &Shipment, b: &Shipment) -> bool {
    let manifest = |s: &Shipment| {
        let mut rows = s
            .items
            .iter()
            .map(|i| (i.product_id, i.sku.clone(), i.quantity))
            .collect::<Vec<_>>();
        rows.sort();
        rows
    };
    manifest(a) == manifest(b)
}
/// Every attempt checks the frozen manifest and version. There is no native
/// PATCH receipt: an exact observed result is described as convergence only.
pub async fn send_tracking(c: &Connection, job: &Outbox) -> Result<String, String> {
    let (saved, original, plan) = frozen_intent(job)?;
    if saved != binding(c)? {
        return Err("Native connection changed; original handoff cannot be retargeted".into());
    }
    let current = shipment(c, &plan.shipment_id.to_string()).await?;
    if !same_manifest(&current, &original) {
        return Err("Native shipment manifest changed; retain original handoff for review".into());
    }
    if plan.confirms(&current) {
        return Ok("Observed native shipment at the exact original next version and status; original PATCH receipt is unavailable".into());
    }
    if current.order_id != original.order_id
        || current.version != plan.expected_version
        || current.status != plan.from_status
    {
        return Err("Native shipment changed; original version will not be rebased".into());
    }
    let mut url = validate_origin(&saved.origin)?;
    url.set_path(&job.path);
    let response=client(c)?.patch(url).json(&job.payload["body"]).send().await.map_err(|_|"Native PATCH outcome is uncertain; explicit review and retry of the original version are required")?;
    if !response.status().is_success() {
        return Err(format!(
            "Native PATCH returned HTTP {}; review original handoff",
            response.status().as_u16()
        ));
    }
    let updated: Shipment = serde_json::from_value(response_value(response).await?)
        .map_err(|_| "Native PATCH response invalid; outcome requires review")?;
    if !plan.confirms(&updated) || !same_manifest(&updated, &original) {
        return Err(
            "Native PATCH did not confirm original identity, version, status and manifest".into(),
        );
    }
    Ok("Native shipment PATCH confirmed at the original next version".into())
}

#[cfg(test)]
#[path = "commerce_tests.rs"]
mod tests;

#[path = "commerce_review.rs"]
mod review;
pub use review::{reconcile, review_preview, ReconcileRequest};

#[path = "commerce_discovery.rs"]
mod discovery;
pub use discovery::{candidate_order, shipment_candidates, SHIPMENT_PAGE_SIZE};
