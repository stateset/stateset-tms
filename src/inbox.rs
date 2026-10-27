//! Persist authenticated EDI notifications before acknowledging delivery.
//! External I/O happens outside tenant transactions; lease ownership fences
//! completion and domain changes commit with the final inbox outcome.
use crate::{
    domain::{id, Receipt, Workspace},
    integrations::Integrations,
    store::Store,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub document_id: String,
    pub delivery_ids: Vec<String>,
    pub key_version: String,
    pub notification: Value,
    pub original_body: String,
    #[serde(default)]
    pub authentication: Option<CallbackAuthentication>,
    pub fingerprint: String,
    pub status: String,
    pub attempts: u32,
    #[serde(default)]
    pub total_attempts: u64,
    #[serde(default)]
    pub history: Vec<InboxAttempt>,
    pub received_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub lease: Option<String>,
    pub leased_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub normalized_event: Option<Value>,
    pub result: Option<Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallbackAuthentication {
    pub delivery_id: String,
    pub key_version: String,
    pub timestamp: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxAttempt {
    pub number: u64,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub outcome: String,
    pub error: Option<String>,
}

impl Store {
    pub async fn receive_document(
        &self,
        tenant: &str,
        delivery_id: &str,
        key_version: &str,
        notification: Value,
        original_body: String,
        authentication: Option<CallbackAuthentication>,
    ) -> Result<Value, String> {
        crate::domain::required(delivery_id, "Callback delivery ID")?;
        crate::domain::required(key_version, "Callback key version")?;
        if serde_json::from_str::<Value>(&original_body)
            .map_err(|_| "Invalid original callback JSON")?
            != notification
        {
            return Err("Original callback body does not match notification".into());
        }
        let document_id = notification["document_id"]
            .as_str()
            .ok_or("Document ID required")?
            .to_owned();
        let fingerprint = digest(&notification);
        let owned = (delivery_id.to_owned(), key_version.to_owned());
        self.update(tenant, move |w| {
            let (delivery_id, key_version) = (owned.0.as_str(), owned.1.as_str());
            if w.inbox.iter().any(|v| v.document_id != document_id && v.delivery_ids.iter().any(|id| id == delivery_id)) {
                return Err("Callback delivery ID already belongs to another document".into());
            }
            if let Some(item) = w.inbox.iter_mut().find(|v| v.document_id == document_id) {
                if item.fingerprint != fingerprint { return Err("EDI document notification already received with different contents".into()); }
                if !item.delivery_ids.iter().any(|id| id == delivery_id) {
                    if item.delivery_ids.len() >= 100 {return Err("Too many callback delivery identities for one document".into());}
                    item.delivery_ids.push(delivery_id.into());
                }
                return Ok(json!({"received":true,"action":"queued","inbox_id":item.id,"status":item.status,"replayed":true}));
            }
            let now = Utc::now();
            let item = InboxItem { id:id("inbox"), document_id:document_id.clone(), delivery_ids:vec![delivery_id.into()],key_version:key_version.into(),
                notification,original_body,authentication,fingerprint,status:"pending".into(),attempts:0,total_attempts:0,history:Vec::new(),received_at:now,updated_at:now,
                next_attempt_at:None,lease:None,leased_until:None,last_error:None,normalized_event:None,result:None };
            let response = json!({"received":true,"action":"queued","inbox_id":item.id,"status":item.status,"replayed":false});
            w.record(&item.id,"inbox.received",format!("EDI document {document_id} received for durable processing"));
            if let Some(event)=w.events.last_mut() {event.actor="stateset-edi".into();}
            w.inbox.push(item);
            Ok(response)
        }).await
    }
}

impl Integrations {
    pub async fn process_inbound_one(&self, store: &Store, tenant: &str) -> Result<bool, String> {
        if !store.integration_due(tenant, true).await? {
            return Ok(false);
        }
        let claimed = store
            .update_if(tenant, move |w| {
                let now = Utc::now();
                let Some(item) = w.inbox.iter_mut().find(|v| due(v, now)) else {
                    return Ok(None);
                };
                if item.status == "processing" {
                    remember_attempt(
                        item,
                        "lease_expired",
                        Some("Worker lease expired before completion; processing reclaimed".into()),
                    );
                }
                item.status = "processing".into();
                item.attempts = item.attempts.saturating_add(1);
                item.total_attempts = item.total_attempts.saturating_add(1);
                item.lease = Some(id("lease"));
                item.leased_until = Some(now + chrono::Duration::seconds(60));
                item.updated_at = now;
                Ok(Some(json!(item)))
            })
            .await?;
        let Some(claimed) = claimed else {
            return Ok(false);
        };
        let job: InboxItem = serde_json::from_value(claimed).map_err(|e| e.to_string())?;
        let fetched = self.callback_event(tenant, &job.notification).await;
        store
            .update_if(tenant, move |w| {
                let index = w
                    .inbox
                    .iter()
                    .position(|v| v.id == job.id)
                    .ok_or("Inbox item not found")?;
                if w.inbox[index].lease != job.lease || w.inbox[index].status != "processing" {
                    return Ok(None);
                }
                match fetched {
                    Ok(Some(event)) => {
                        // Applying a batch may fail after an earlier status changed
                        // the candidate. Discard all such changes on any failure.
                        let mut candidate = w.clone();
                        let start = candidate.events.len();
                        let result = apply_event(&mut candidate, &event);
                        match result {
                            Ok(result) => {
                                let outcome = if result["data"]["held"] == true {
                                    "reconciliation_required"
                                } else if result["data"]["finance_review"] == true {
                                    "finance_review"
                                } else {
                                    "applied"
                                };
                                let item = &mut candidate.inbox[index];
                                remember_attempt(item, outcome, None);
                                item.status = outcome.into();
                                item.last_error = None;
                                item.result = Some(result);
                                item.normalized_event = Some(event);
                                clear_lease(item);
                                candidate.record(
                                    &job.id,
                                    &format!("inbox.{outcome}"),
                                    format!("EDI document {}: {outcome}", job.document_id),
                                );
                                for entry in &mut candidate.events[start..] {
                                    entry.actor = "stateset-edi".into();
                                }
                                *w = candidate;
                            }
                            Err(error) => {
                                w.inbox[index].normalized_event = Some(event);
                                fail(w, index, error);
                            }
                        }
                    }
                    Ok(None) => {
                        let item = &mut w.inbox[index];
                        remember_attempt(item, "ignored", None);
                        item.status = "ignored".into();
                        item.last_error = None;
                        clear_lease(item);
                    }
                    Err(error) => fail(w, index, error),
                }
                Ok(Some(json!({"status":w.inbox[index].status})))
            })
            .await?;
        Ok(true)
    }
}

pub(crate) fn due(item: &InboxItem, now: DateTime<Utc>) -> bool {
    (matches!(item.status.as_str(), "pending" | "retry")
        && item.next_attempt_at.is_none_or(|t| t <= now))
        || (item.status == "processing" && item.leased_until.is_some_and(|t| t <= now))
}

fn apply_event(workspace: &mut Workspace, event: &Value) -> Result<Value, String> {
    let key = format!(
        "edi:{}",
        event["event_id"]
            .as_str()
            .ok_or("Normalized event ID required")?
    );
    // Same receipt namespace and fingerprint as authenticated /v1/events.
    let fingerprint = crate::store::fingerprint("edi.event", "", event);
    if let Some(receipt) = workspace.receipts.get(&key) {
        if receipt.fingerprint != fingerprint {
            return Err("EDI event already applied with different contents".into());
        }
        let response = receipt.checked_response(&fingerprint)?;
        return Ok(json!({"data":response,"replayed":true}));
    }
    let result = workspace.apply("edi.event", "", event.clone())?;
    workspace.receipts.insert(
        key,
        Receipt {
            fingerprint,
            response: result.clone(),
            stopped: None,
        },
    );
    Ok(json!({"data":result,"replayed":false}))
}
fn clear_lease(item: &mut InboxItem) {
    item.lease = None;
    item.leased_until = None;
    item.next_attempt_at = None;
    item.updated_at = Utc::now();
}
fn fail(workspace: &mut Workspace, index: usize, error: String) {
    let item = &mut workspace.inbox[index];
    remember_attempt(item, "failed", Some(error.clone()));
    item.status = if item.attempts >= 8 { "held" } else { "retry" }.into();
    item.last_error = Some(error);
    item.lease = None;
    item.leased_until = None;
    item.updated_at = Utc::now();
    item.next_attempt_at =
        Some(Utc::now() + chrono::Duration::seconds(5 * 2i64.pow(item.attempts.min(10))));
    let entity = item.id.clone();
    let message = format!("EDI document {}: {}", item.document_id, item.status);
    workspace.record(&entity, "inbox.failed", message);
    if let Some(event) = workspace.events.last_mut() {
        event.actor = "stateset-edi".into();
    }
}
fn remember_attempt(item: &mut InboxItem, outcome: &str, error: Option<String>) {
    item.history.push(InboxAttempt {
        number: item.total_attempts,
        started_at: item.updated_at,
        finished_at: Utc::now(),
        outcome: outcome.into(),
        error,
    });
}
fn digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::Connection;
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    #[tokio::test]
    async fn expired_inbox_lease_cannot_overwrite_a_new_worker_result() {
        let store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        let note = json!({"type":"document.recorded","tenant_id":"demo","document_id":"lease-doc","partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted"});
        store
            .receive_document(
                "demo",
                "delivery",
                "1",
                note.clone(),
                note.to_string(),
                None,
            )
            .await
            .expect("enqueue");
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let (entered, unblock, count) = (started.clone(), release.clone(), calls.clone());
        let server=axum::Router::new().route("/v1/documents/lease-doc",axum::routing::get(move || {
            let (entered,unblock,count)=(entered.clone(),unblock.clone(),count.clone());
            async move {
                let first=count.fetch_add(1,Ordering::SeqCst)==0;
                if first {entered.notify_one();unblock.notified().await;}
                axum::Json(json!({"data":{"document":{"id":"lease-doc","partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted","processed_at":Utc::now()},"parsed":{"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":if first {"A"} else {"D"}}}}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listen");
        let address = listener.local_addr().expect("address");
        let server =
            tokio::spawn(async move { axum::serve(listener, server).await.expect("serve") });
        let integrations = Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    edi_url: Some(format!("http://{address}")),
                    edi_token: Some("token".into()),
                    edi_tenant_id: Some("demo".into()),
                    ..Connection::default()
                },
            )]),
        };
        let (old_store, old_integration) = (store.clone(), integrations.clone());
        let old = tokio::spawn(async move {
            old_integration
                .process_inbound_one(&old_store, "demo")
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
            .await
            .expect("first fetch started");
        store
            .update("demo", |w| {
                w.inbox[0].leased_until = Some(Utc::now() - chrono::Duration::seconds(1));
                Ok(Value::Null)
            })
            .await
            .expect("simulate crash lease expiry");
        integrations
            .process_inbound_one(&store, "demo")
            .await
            .expect("new worker");
        release.notify_one();
        old.await
            .expect("old worker finished")
            .expect("old result fenced");
        let w = store.read("demo").await.expect("read");
        assert_eq!(w.loads[2].status, crate::domain::Status::Planned);
        assert_eq!(w.inbox[0].status, "applied");
        assert_eq!(w.inbox[0].attempts, 2);
        assert_eq!(
            w.events.iter().filter(|v| v.kind == "load.decline").count(),
            1
        );
        assert_eq!(
            w.events.iter().filter(|v| v.kind == "load.accept").count(),
            0
        );
        server.abort();
    }
}
