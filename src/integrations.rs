//! Adapters follow the checked-in EDI and sync-server HTTP contracts.
pub use crate::integration_worker::{worker, WorkerConfig};
use crate::{
    domain::{id, Outbox},
    store::Store,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::{collections::BTreeMap, time::Duration};

// Process-local scheduling only. Durable claims and ordering remain in Store.
#[derive(Clone, Default)]
pub(crate) struct DispatchCursor(Option<String>);
const DISPATCH_CANDIDATES_PER_TURN: usize = 4;

#[cfg(test)]
#[path = "dispatch_turn_tests.rs"]
mod dispatch_turn_tests;
#[cfg(test)]
#[path = "integration_response_tests.rs"]
mod response_tests;

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub commerce_url: Option<String>,
    pub commerce_token: Option<String>,
    pub commerce_tenant_id: Option<String>,
    pub edi_url: Option<String>,
    pub edi_token: Option<String>,
    pub edi_tenant_id: Option<String>,
    // Version -> secret allows old queued callbacks to finish after rotation.
    #[serde(default)]
    pub callback_keys: BTreeMap<String, String>,
    pub sync_url: Option<String>,
    pub sync_api_key: Option<String>,
    pub sync_tenant_id: Option<String>,
}
#[derive(Clone, Default)]
pub struct Integrations {
    pub tenants: BTreeMap<String, Connection>,
}
impl Integrations {
    pub fn from_env() -> Result<Self, String> {
        let tenants: BTreeMap<String, Connection> = serde_json::from_str(
            &std::env::var("FREIGHT_INTEGRATIONS_JSON").unwrap_or_else(|_| "{}".into()),
        )
        .map_err(|e| e.to_string())?;
        for connection in tenants.values() {
            for base in [
                &connection.edi_url,
                &connection.sync_url,
                &connection.commerce_url,
            ]
            .into_iter()
            .flatten()
            {
                let url = reqwest::Url::parse(base).map_err(|_| "Invalid integration URL")?;
                if !["http", "https"].contains(&url.scheme())
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    return Err(
                        "Integration URLs must be HTTP(S) without credentials, query, or fragment"
                            .into(),
                    );
                }
                if url.scheme() == "http"
                    && !matches!(
                        url.host_str(),
                        Some("localhost" | "127.0.0.1" | "[::1]" | "edi" | "sync-server")
                    )
                {
                    return Err("Remote integrations require HTTPS".into());
                }
            }
            if connection.callback_keys.values().any(|v| v.len() < 32) {
                return Err("Callback secrets require at least 32 characters".into());
            }
            if connection.edi_url.is_some() != connection.edi_token.is_some()
                || connection.sync_url.is_some() != connection.sync_api_key.is_some()
            {
                return Err("Integration URL and credential must be configured together".into());
            }
            if connection.commerce_url.is_some() != connection.commerce_token.is_some()
                || connection.commerce_url.is_some() != connection.commerce_tenant_id.is_some()
                || connection.commerce_url.is_some() && connection.sync_url.is_some()
            {
                return Err("Configure exactly one order provider per freight tenant; iCommerce requires URL, credential and tenant together".into());
            }
            if let Some(raw) = &connection.commerce_url {
                crate::commerce::validate_origin(raw)?;
            }
            if connection.commerce_token.as_ref().is_some_and(|v| {
                v.len() < 32
                    || v.len() > 16384
                    || reqwest::header::HeaderValue::from_str(&format!("Bearer {v}")).is_err()
            }) {
                return Err("Invalid iCommerce credential".into());
            }
            if connection.sync_url.is_some() && connection.sync_tenant_id.is_none() {
                return Err("Sync tenant ID required".into());
            }
            if connection.edi_url.is_some() && connection.edi_tenant_id.is_none() {
                return Err("EDI tenant ID required".into());
            }
            for tenant in [
                &connection.edi_tenant_id,
                &connection.sync_tenant_id,
                &connection.commerce_tenant_id,
            ]
            .into_iter()
            .flatten()
            {
                if !tenant
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
                    || tenant.is_empty()
                    || tenant.len() > 100
                {
                    return Err("Invalid upstream tenant ID".into());
                }
            }
        }
        Ok(Self { tenants })
    }
    pub fn summary(&self, tenant: &str) -> Value {
        let connection = self.tenants.get(tenant);
        json!([
            {"name":"StateSet EDI","detail":"204 tenders · signed document callbacks · 990 / 214 / 210","status":if connection.is_some_and(|v|v.edi_url.is_some()) {"configured"} else {"not_configured"}},
            {"name":"StateSet Sync","detail":"Order import · shipment tracking writeback","status":if connection.is_some_and(|v|v.sync_url.is_some()) {"configured"} else {"not_configured"}},
            {"name":"StateSet iCommerce","detail":"Typed order import · explicit versioned shipment tracking","status":if connection.is_some_and(|v|v.commerce_url.is_some()) {"configured"} else {"not_configured"}}
        ])
    }
    pub async fn orders(&self, tenant: &str, page: u32) -> Result<Value, String> {
        if self
            .tenants
            .get(tenant)
            .is_some_and(|c| c.commerce_url.is_some())
        {
            return crate::commerce::orders(
                self.tenants
                    .get(tenant)
                    .ok_or("iCommerce connection missing")?,
                page,
            )
            .await;
        }
        let c = self
            .tenants
            .get(tenant)
            .ok_or("Sync integration is not configured")?;
        let base = c
            .sync_url
            .as_deref()
            .ok_or("Sync integration is not configured")?;
        let response = client()?
            .get(format!(
                "{}/v1/tenants/{}/orders",
                base.trim_end_matches('/'),
                c.sync_tenant_id.as_deref().ok_or("Sync tenant required")?
            ))
            .header(
                "x-stateset-api-key",
                c.sync_api_key
                    .as_deref()
                    .ok_or("Sync credential required")?,
            )
            .query(&[
                ("page", page.max(1).to_string()),
                ("page_size", "100".into()),
            ])
            .send()
            .await
            .map_err(|_| "Sync request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "Sync orders returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let bytes = response_bytes(response, "Sync order").await?;
        let result: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid sync order response")?;
        if !result["data"]["items"].is_array() {
            return Err("Sync order response missing data.items".into());
        }
        Ok(result["data"].clone())
    }
    pub async fn order(&self, tenant: &str, order_id: &str) -> Result<Value, String> {
        let order = self.order_snapshot(tenant, order_id).await?;
        if order["sourceSystem"] == "icommerce" && order["importable"] != true {
            return Err("iCommerce order is held, fulfilled, partially shipped or not eligible for a new forward load".into());
        }
        if crate::source::needs_explicit_authorization(&order) {
            return Err("Cancelled, returned, or unknown orders cannot be imported as new forward shipments".into());
        }
        Ok(order)
    }
    pub async fn order_snapshot(&self, tenant: &str, order_id: &str) -> Result<Value, String> {
        if let Some(connection) = self
            .tenants
            .get(tenant)
            .filter(|c| c.commerce_url.is_some())
        {
            return crate::commerce::order(connection, order_id).await;
        }
        uuid::Uuid::parse_str(order_id).map_err(|_| "Order ID must be a UUID")?;
        let c = self
            .tenants
            .get(tenant)
            .ok_or("Sync integration is not configured")?;
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Sync verification client unavailable")?
            .get(format!(
                "{}/v1/tenants/{}/orders/{order_id}",
                c.sync_url
                    .as_deref()
                    .ok_or("Sync URL required")?
                    .trim_end_matches('/'),
                c.sync_tenant_id.as_deref().ok_or("Sync tenant required")?
            ))
            .header(
                "x-stateset-api-key",
                c.sync_api_key
                    .as_deref()
                    .ok_or("Sync credential required")?,
            )
            .send()
            .await
            .map_err(|_| "Sync request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "Sync order returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let bytes = response_bytes(response, "Sync order").await?;
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid sync order response")?;
        if response["data"]["id"].as_str() != Some(order_id) {
            return Err("Sync order identity mismatch".into());
        }
        if response["data"]["tenantId"]
            .as_str()
            .is_some_and(|v| Some(v) != c.sync_tenant_id.as_deref())
        {
            return Err("Sync order tenant mismatch".into());
        }
        Ok(response["data"].clone())
    }
    pub async fn refresh_source(
        &self,
        store: &Store,
        tenant: &str,
        load: &str,
        actor: &str,
        key: Option<&str>,
    ) -> Result<Value, String> {
        if let Some(key) = key {
            if let Some(replay) = store
                .replay(tenant, key, "source.refresh", load, &json!({}))
                .await?
            {
                return Ok(replay);
            }
        }
        let workspace = store.read(tenant).await?;
        let record = workspace
            .loads
            .iter()
            .find(|v| v.id == load)
            .ok_or("Load not found")?;
        let order = record
            .sync_order_id
            .as_deref()
            .ok_or("Shipment is not imported from Sync")?;
        let native = record
            .source_order
            .as_ref()
            .is_some_and(|v| v["sourceSystem"] == "icommerce");
        let configured_native = self
            .tenants
            .get(tenant)
            .is_some_and(|c| c.commerce_url.is_some());
        if native != configured_native {
            return Err("Order provider changed; restore the original source connection before refreshing or committing this load".into());
        }
        let version = workspace.source_check(load).map_or(0, |v| v.version);
        let requested_at = chrono::Utc::now();
        let result = match self.order_snapshot(tenant, order).await {
            Ok(snapshot) => {
                store
                    .record_source(
                        (tenant, actor),
                        (load, snapshot, version, requested_at),
                        key,
                    )
                    .await
            }
            Err(error) => {
                let _ = store
                    .record_source_error(tenant, load, version, &error)
                    .await;
                return Err(crate::source::SOURCE_UNAVAILABLE.into());
            }
        };
        if let Err(error) = &result {
            let _ = store
                .record_source_error(tenant, load, version, error)
                .await;
            if error.starts_with("Invalid canonical Sync")
                || error.starts_with("Sync source revision timestamp")
                || error.starts_with("Sync returned an older source")
            {
                return Err(crate::source::SOURCE_UNAVAILABLE.into());
            }
        }
        result
    }
    pub fn validate_notification(
        &self,
        tenant: &str,
        notification: &Value,
    ) -> Result<bool, String> {
        if notification["type"] != "document.recorded" {
            return Err("Unsupported callback type".into());
        }
        let c = self
            .tenants
            .get(tenant)
            .ok_or("EDI integration is not configured")?;
        if notification["tenant_id"].as_str() != c.edi_tenant_id.as_deref() {
            return Err("Callback tenant mismatch".into());
        }
        if notification["direction"] != "inbound" || notification["status"] != "accepted" {
            return Ok(false);
        }
        if !matches!(
            notification["transaction_set"].as_str(),
            Some("990" | "214" | "210")
        ) {
            return Ok(false);
        }
        let doc = notification["document_id"]
            .as_str()
            .ok_or("Document ID required")?;
        if doc.is_empty()
            || doc.len() > 150
            || !doc
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"-_".contains(&v))
        {
            return Err("Invalid document ID".into());
        }
        crate::domain::required(
            notification["partner_id"]
                .as_str()
                .ok_or("Partner ID required")?,
            "Partner ID",
        )?;
        Ok(true)
    }
    pub async fn callback_event(
        &self,
        tenant: &str,
        notification: &Value,
    ) -> Result<Option<Value>, String> {
        if !self.validate_notification(tenant, notification)? {
            return Ok(None);
        }
        let c = self
            .tenants
            .get(tenant)
            .ok_or("EDI integration is not configured")?;
        let doc = notification["document_id"]
            .as_str()
            .ok_or("Document ID required")?;
        let response = client()?
            .get(format!(
                "{}/v1/documents/{doc}",
                c.edi_url
                    .as_deref()
                    .ok_or("EDI URL required")?
                    .trim_end_matches('/')
            ))
            .bearer_auth(c.edi_token.as_deref().ok_or("EDI credential required")?)
            .header(
                "x-tenant-id",
                c.edi_tenant_id.as_deref().ok_or("EDI tenant required")?,
            )
            .send()
            .await
            .map_err(|_| "EDI detail request failed")?;
        if !response.status().is_success() {
            return Err(format!(
                "EDI document returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let bytes = response_bytes(response, "EDI detail").await?;
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid EDI detail response")?;
        normalize_document(notification, &response["data"])
    }
    pub async fn dispatch_one(&self, store: &Store, tenant: &str) -> Result<bool, String> {
        self.dispatch(
            store,
            tenant,
            &mut DispatchCursor::default(),
            usize::MAX,
            &crate::lifecycle::Shutdown::default(),
        )
        .await
    }
    pub(crate) async fn dispatch_turn(
        &self,
        store: &Store,
        tenant: &str,
        cursor: &mut DispatchCursor,
        shutdown: &crate::lifecycle::Shutdown,
    ) -> Result<bool, String> {
        self.dispatch(
            store,
            tenant,
            cursor,
            DISPATCH_CANDIDATES_PER_TURN,
            shutdown,
        )
        .await
    }
    async fn dispatch(
        &self,
        store: &Store,
        tenant: &str,
        cursor: &mut DispatchCursor,
        limit: usize,
        shutdown: &crate::lifecycle::Shutdown,
    ) -> Result<bool, String> {
        if shutdown.requested() {
            return Ok(false);
        }
        let Some(c) = self.tenants.get(tenant) else {
            return Ok(false);
        };
        if !store.integration_due(tenant, false).await? {
            return Ok(false);
        }
        if shutdown.requested() {
            return Ok(false);
        }
        // Restore coordination holds before considering expired leases. A source
        // check never marks a tender transmitted: withdrawal proof remains intact.
        store.update_if(tenant, |w| {
            let now=Utc::now();
            let mut changed=false;
            let paused:Vec<_>=w.releases.iter().filter(|v|v.status=="pending").map(|v|v.tender_id.clone()).collect();
            let held:Vec<_>=w.tender_incidents.iter().filter(|v|v.status=="open").map(|v|v.load_id.clone()).collect();
            for job in &mut w.outbox {
                if job.destination=="stateset-icommerce" && job.status=="sending" && job.leased_until.is_some_and(|v|v<=now) {
                    job.status="held".into(); job.lease=None; job.leased_until=None;
                    job.last_error=Some("Native shipment handoff lease expired; original PATCH outcome requires explicit review".into()); changed=true;
                }
                if job.destination=="stateset-edi" && (job.tender_id.as_ref().is_some_and(|id|paused.contains(id)) || held.contains(&job.load_id)) && job.status=="sending" && job.leased_until.is_some_and(|v|v<=now) {
                    job.status="held".into();job.lease=None;job.leased_until=None;
                    job.last_error=Some("Carrier coordination hold after expired handoff lease; prior transmission outcome is uncertain".into());
                    changed=true;
                }
            }
            Ok(changed.then_some(Value::Null))
        }).await?;
        let workspace = store.read(tenant).await?;
        let start = cursor
            .0
            .as_ref()
            .and_then(|id| workspace.outbox.iter().position(|job| &job.id == id))
            .map_or(0, |index| index + 1);
        let candidates: Vec<_> = workspace
            .outbox
            .iter()
            .cycle()
            .skip(start)
            .take(workspace.outbox.len())
            .filter(|job| handoff_eligible(&workspace, c, &job.id))
            .take(limit)
            .cloned()
            .collect();
        let mut selected = None;
        for candidate in candidates {
            if shutdown.requested() {
                break;
            }
            // Advance even on source holds or a lost claim. The next turn
            // resumes after this original job identity, not an unstable index
            // into the currently eligible subset.
            cursor.0 = Some(candidate.id.clone());
            // Sync's tracking mutation sets the commerce order SHIPPED. A
            // withdrawal/decline/gate fact before pickup must stay local. This
            // is terminal for this payload, so a later physical update can
            // proceed without replaying an older, untransmitted planning fact.
            if sync_local_hold(&workspace, &candidate).is_some() {
                let c = c.clone();
                store
                    .update_if(tenant, move |w| {
                        if !handoff_eligible(w, &c, &candidate.id) {
                            return Ok(None);
                        }
                        let Some((reason, activity)) = sync_local_hold(w, &candidate) else {
                            return Ok(None);
                        };
                        let job = w
                            .outbox
                            .iter_mut()
                            .find(|v| v.id == candidate.id)
                            .ok_or("Outbox item missing")?;
                        job.status = "local_only".into();
                        job.lease = None;
                        job.leased_until = None;
                        job.last_error = Some(reason.into());
                        w.record(&candidate.load_id, "integration.local_only", activity);
                        Ok(Some(Value::Null))
                    })
                    .await?;
                continue;
            }
            if candidate.destination == "stateset-edi"
                && workspace
                    .source_check(&candidate.load_id)
                    .is_some_and(|v| v.active_review_id.is_some())
            {
                continue;
            }
            let imported = workspace
                .loads
                .iter()
                .any(|v| v.id == candidate.load_id && v.sync_order_id.is_some());
            let grant = if imported {
                // No tenant lock is held while fetching the canonical source.
                match self
                    .refresh_source(store, tenant, &candidate.load_id, "system", None)
                    .await
                {
                    Ok(value) => Some(crate::source::SourceGrant {
                        load_id: candidate.load_id.clone(),
                        version: value["data"]["version"]
                            .as_u64()
                            .ok_or("Invalid source observation")?,
                    }),
                    Err(error) => {
                        tracing::warn!(%tenant,%error,load_id=%candidate.load_id,"Source verification deferred handoff");
                        continue;
                    }
                }
            } else {
                None
            };
            let claim_connection = c.clone();
            let claim_shutdown = shutdown.clone();
            let claimed=store.update_if(tenant,move |w| {
                let c = &claim_connection;
                let shutdown = &claim_shutdown;
                if shutdown.requested() {return Ok(None);}
                if !handoff_eligible(w,c,&candidate.id) {return Ok(None);}
                if let Some(grant)=&grant {
                    let check=w.source_check(&grant.load_id).ok_or(crate::source::SOURCE_UNAVAILABLE)?;
                    if check.version!=grant.version || check.last_error.is_some() || check.valid_until.is_none_or(|v|v<=Utc::now()) {return Ok(None);}
                    if matches!(candidate.destination.as_str(),"stateset-edi"|"stateset-icommerce") {
                        if w.check_source_grant(grant).is_err() {return Ok(None);}
                    } else if !crate::source::tracking_status_safe(&check.snapshot) || !matches!(candidate.payload["trackingData"]["freight_status"].as_str(),Some("in_transit"|"delivered")) {
                        let item=w.outbox.iter_mut().find(|v|v.id==candidate.id).ok_or("Outbox item missing")?;
                        item.status="held".into();item.lease=None;item.leased_until=None;
                        item.last_error=Some("Sync source or freight state forbids tracking writeback: upstream endpoint sets order SHIPPED".into());
                        w.record(&candidate.load_id,"integration.source_hold","Tracking retained locally; canonical source status must not be overwritten");
                        return Ok(Some(Value::Null));
                    }
                }
                if candidate.destination=="stateset-icommerce" {
                    if let Err(error)=crate::commerce::authorize_handoff(w,&candidate) {
                        let job=w.outbox.iter_mut().find(|v|v.id==candidate.id).ok_or("Outbox item missing")?;
                        job.status="held".into();job.last_error=Some(error);job.lease=None;job.leased_until=None;
                        return Ok(Some(Value::Null));
                    }
                }
                let job=w.outbox.iter_mut().find(|v|v.id==candidate.id).ok_or("Outbox item missing")?;
                job.status="sending".into();job.attempts=job.attempts.saturating_add(1);job.total_attempts=job.total_attempts.saturating_add(1);
                job.transmission="attempted".into();job.lease=Some(id("lease"));job.leased_until=Some(Utc::now()+chrono::Duration::seconds(60));
                Ok(Some(json!(job)))
            }).await?;
            if let Some(claimed) = claimed.filter(|value| !value.is_null()) {
                selected =
                    Some(serde_json::from_value::<Outbox>(claimed).map_err(|e| e.to_string())?);
                break;
            }
        }
        let Some(job) = selected else {
            return Ok(false);
        };
        let result = send(c, &job).await;
        store
            .update_if(tenant, move |w| {
                let item = w
                    .outbox
                    .iter_mut()
                    .find(|v| v.id == job.id)
                    .ok_or("Outbox item missing")?;
                if item.lease != job.lease || item.status != "sending" {
                    return Ok(None);
                }
                item.lease = None;
                item.leased_until = None;
                match &result {
                    Ok(response) => {
                        item.status = if job.destination == "stateset-edi" {
                            "gateway_accepted"
                        } else {
                            "synced"
                        }
                        .into();
                        item.last_error = None;
                        item.response = Some(response.clone());
                    }
                    Err(error) => {
                        item.status =
                            if job.destination == "stateset-icommerce" || item.attempts >= 8 {
                                "held"
                            } else {
                                "retry"
                            }
                            .into();
                        item.last_error = Some(error.clone());
                        item.next_attempt_at = Some(
                            Utc::now()
                                + chrono::Duration::seconds(5i64 * 2i64.pow(item.attempts.min(10))),
                        );
                    }
                }
                let status = item.status.clone();
                w.record(
                    &job.load_id,
                    "integration.handoff",
                    format!("{}: {}", job.destination, status),
                );
                Ok(Some(json!({"status":status})))
            })
            .await?;
        Ok(true)
    }
}
fn sync_local_hold(
    w: &crate::domain::Workspace,
    job: &Outbox,
) -> Option<(&'static str, &'static str)> {
    if job.destination != "stateset-sync-server" {
        return None;
    }
    let tracking = &job.payload["trackingData"];
    if tracking["freight_status"] == "delivered" {
        if tracking["freight_execution_consistent"] == false
            || w.loads
                .iter()
                .find(|l| l.id == job.load_id)
                .is_some_and(|l| !w.execution_consistent(l))
        {
            return Some(("Final delivery tracking retained locally until conflicting execution times are corrected; original observations and any previous transmission remain recorded", "Conflicting gate, service or native movement times hold final tracking; a later consistent snapshot can proceed"));
        }
        if tracking["freight_route_complete"] == false {
            return Some(("Final delivery tracking retained locally until all intermediate services are confirmed; original observation and any previous transmission remain recorded", "Final delivery fact retained locally while intermediate service evidence is missing; later complete route tracking can proceed"));
        }
    }
    if matches!(
        tracking["freight_status"].as_str(),
        Some("planned" | "tendered" | "accepted" | "dispatched")
    ) {
        return Some(("Pre-pickup tracking retained locally: Sync tracking sets the commerce order SHIPPED; any previous transmission attempt remains recorded", "Pre-pickup fact retained locally; later physical tracking can proceed without marking an unshipped commerce order SHIPPED"));
    }
    None
}
pub(crate) fn due(job: &Outbox, now: chrono::DateTime<Utc>) -> bool {
    matches!(
        job.destination.as_str(),
        "stateset-edi" | "stateset-sync-server" | "stateset-icommerce"
    ) && (matches!(job.status.as_str(), "pending_handoff" | "retry")
        && job.next_attempt_at.is_none_or(|v| v <= now)
        || job.status == "sending" && job.leased_until.is_some_and(|v| v <= now))
}
fn handoff_eligible(w: &crate::domain::Workspace, c: &Connection, id: &str) -> bool {
    let Some(index) = w.outbox.iter().position(|v| v.id == id) else {
        return false;
    };
    let job = &w.outbox[index];
    let now = Utc::now();
    let configured = match job.destination.as_str() {
        "stateset-edi" => c.edi_url.is_some(),
        "stateset-sync-server" => c.sync_url.is_some(),
        "stateset-icommerce" => c.commerce_url.is_some(),
        _ => false,
    };
    configured
        && !(job.destination == "stateset-edi"
            && w.tenders.iter().any(|t| {
                Some(&t.id) == job.tender_id.as_ref() && !t.plan.intermediate_stops.is_empty()
            }))
        && due(job, now)
        && !(job.destination == "stateset-edi"
            && (w
                .releases
                .iter()
                .any(|v| v.status == "pending" && Some(&v.tender_id) == job.tender_id.as_ref())
                || w.tender_incidents
                    .iter()
                    .any(|v| v.status == "open" && v.load_id == job.load_id)))
        && !w.outbox[..index].iter().any(|prior| {
            prior.load_id == job.load_id
                && prior.destination == job.destination
                && matches!(
                    prior.status.as_str(),
                    "pending_handoff" | "retry" | "sending" | "held"
                )
        })
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Integration HTTP client unavailable".into())
}
// Content-Length is only an early rejection hint. Chunked/close-delimited
// partners must obey the same byte budget before decoding or retaining a body.
const RESPONSE_LIMIT: usize = 2_000_000;
async fn response_bytes(mut response: reqwest::Response, source: &str) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|size| size > RESPONSE_LIMIT as u64)
    {
        return Err(format!("{source} response exceeds limit"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| format!("{source} response unreadable"))?
    {
        if chunk.len() > RESPONSE_LIMIT - bytes.len() {
            return Err(format!("{source} response exceeds limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn send(c: &Connection, job: &Outbox) -> Result<String, String> {
    if job.destination == "stateset-icommerce" {
        return crate::commerce::send_tracking(c, job).await;
    }
    let request = if job.destination == "stateset-edi" {
        client()?
            .post(format!(
                "{}{}",
                c.edi_url
                    .as_deref()
                    .ok_or("EDI URL required")?
                    .trim_end_matches('/'),
                job.path
            ))
            .bearer_auth(c.edi_token.as_deref().ok_or("EDI credential required")?)
            .header(
                "x-tenant-id",
                c.edi_tenant_id.as_deref().ok_or("EDI tenant required")?,
            )
    } else {
        client()?
            .post(format!(
                "{}/v1/tenants/{}{}",
                c.sync_url
                    .as_deref()
                    .ok_or("Sync URL required")?
                    .trim_end_matches('/'),
                c.sync_tenant_id.as_deref().ok_or("Sync tenant required")?,
                job.path
            ))
            .header(
                "x-stateset-api-key",
                c.sync_api_key
                    .as_deref()
                    .ok_or("Sync credential required")?,
            )
    };
    let response = request
        .header("Idempotency-Key", &job.id)
        .json(&job.payload)
        .send()
        .await
        .map_err(|_| {
            "Integration request failed or timed out; retry will reuse the original key"
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("Upstream returned HTTP {}", status.as_u16()));
    }
    let bytes = response_bytes(response, "Integration").await?;
    if job.destination == "stateset-edi" {
        let body = String::from_utf8(bytes).map_err(|_| "EDI response is not UTF-8")?;
        if !body.starts_with("ISA") {
            return Err("EDI response is not an X12 interchange".into());
        }
        Ok(body)
    } else {
        let parsed: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Sync response is not JSON")?;
        if parsed["data"]["ok"] != true {
            return Err("Sync response did not confirm update".into());
        }
        Ok("Tracking update confirmed".into())
    }
}
pub fn verify_callback(
    secret: &str,
    timestamp: &str,
    event_id: &str,
    signature: &str,
    body: &[u8],
) -> Result<(), String> {
    let time: i64 = timestamp
        .parse()
        .map_err(|_| "Invalid callback timestamp")?;
    if Utc::now().timestamp().abs_diff(time) > 300 {
        return Err("Callback outside five minute replay window".into());
    }
    let signature = signature
        .strip_prefix("v1=")
        .ok_or("Invalid callback signature version")?;
    let bytes = hex::decode(signature).map_err(|_| "Invalid callback signature encoding")?;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| "Invalid callback secret")?;
    mac.update(format!("{timestamp}.{event_id}.").as_bytes());
    mac.update(body);
    mac.verify_slice(&bytes)
        .map_err(|_| "Invalid callback signature".into())
}
pub fn normalize_document(notification: &Value, detail: &Value) -> Result<Option<Value>, String> {
    let doc = &detail["document"];
    for field in ["partner_id", "transaction_set", "direction", "status"] {
        if doc[field] != notification[field] {
            return Err(format!("EDI document {field} does not match callback"));
        }
    }
    if doc["id"] != notification["document_id"] {
        return Err("EDI document ID mismatch".into());
    }
    let mut payload = detail["parsed"].clone();
    if !payload.is_object() {
        return Err("EDI detail has no canonical parsed payload".into());
    }
    payload["partner_id"] = doc["partner_id"].clone();
    let kind = match doc["transaction_set"].as_str() {
        Some("990") => {
            let action = payload["action_code"]
                .as_str()
                .ok_or("990 action required")?;
            if !["A", "D"].contains(&action) {
                return Err("Unsupported 990 action".into());
            }
            payload["accepted"] = json!(action == "A");
            "edi.load_tender_response.received"
        }
        Some("214") => {
            payload["latest_status"] = payload["events"]
                .as_array()
                .and_then(|v| v.last())
                .map(|v| v["status_code"].clone())
                .ok_or("214 events required")?;
            "edi.shipment_status.received"
        }
        Some("210") => "edi.freight_invoice.received",
        _ => return Ok(None),
    };
    Ok(Some(
        json!({"event_id":format!("edi-doc-{}",doc["id"].as_str().ok_or("Document ID required")?),"event_type":kind,"occurred_at":doc["processed_at"],"payload":payload}),
    ))
}
