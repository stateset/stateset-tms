use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::Sha256;
use stateset_freight::{
    api::{self, App},
    domain::Status,
    integrations::{Connection, Integrations},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tower::ServiceExt;

const SECRET: &str = "inbox-test-callback-secret-at-least-32-chars";

#[tokio::test]
async fn additional_signed_210_waits_for_finance_classification_and_preserves_receipt_after_posting(
) {
    let store = seeded().await;
    let w = store.read("demo").await.expect("read");
    let load = w.loads[5].clone();
    let carrier = w
        .carriers
        .iter()
        .find(|v| Some(&v.id) == load.carrier_id.as_ref())
        .expect("carrier")
        .clone();
    let primary = w
        .invoices
        .iter()
        .find(|v| v.load_id == load.id)
        .expect("primary")
        .id
        .clone();
    store
        .mutate(
            "demo",
            "approve-primary",
            "invoice.approve",
            &primary,
            json!({}),
        )
        .await
        .expect("primary");
    let mut note = notification("signed-additional-210", "210");
    note["partner_id"] = json!(carrier.partner_id);
    let payload = detail(
        &note,
        json!({"shipment_id":load.reference,"carrier_scac":carrier.scac,"invoice_number":"SIGNED-CREDIT-210","net_amount":"-12.51","currency":"USD"}),
    );
    let (url, server) = serve(Router::new().route(
        "/v1/documents/signed-additional-210",
        get(move || {
            let payload = payload.clone();
            async move { Json(payload) }
        }),
    ))
    .await;
    let integrations = integration(&url);
    let app = router(store.clone(), integrations.clone());
    assert_eq!(
        callback(&app, "signed-extra-delivery", &note, true).await.0,
        StatusCode::OK
    );
    integrations
        .process_inbound_one(&store, "demo")
        .await
        .expect("process");
    let w = store.read("demo").await.expect("read");
    assert_eq!(w.inbox[0].status, "finance_review");
    assert_eq!(w.carrier_documents[0].status, "pending");
    assert_eq!(w.invoices[0].amount_cents, 118000);
    assert_eq!(w.journal.len(), 1);
    assert_eq!(
        retry(&app, &w.inbox[0].id, "operator").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let source = w.carrier_documents[0].id.clone();
    let inbox = w.inbox[0].clone();
    let draft=store.mutate_authorized(("demo","finance:reviewer"),"draft-signed","payable.create",&load.id,json!({"expected_load_version":w.loads[5].version,"invoice_id":primary,"kind":"credit_memo","number":"SIGNED-CREDIT-210","document_amount_cents":1251,"issued_on":Utc::now().date_naive(),"description":"Actual carrier credit verified by finance","evidence_reference":"SIGNED-CREDIT-210/line-1","carrier_document_id":source})).await.expect("draft")["data"].clone();
    let current = store.read("demo").await.expect("read");
    store.mutate_authorized(("demo","finance:reviewer"),"post-signed","payable.review",draft["id"].as_str().expect("id"),json!({"expected_load_version":current.loads[5].version,"expected_version":1,"decision":"approved","counterparty_contact":"Actual carrier accounting","confirmation_reference":"External case 15","note":"Carrier confirmed the negative canonical document is an actual credit memo"})).await.expect("post");
    let w = store.read("demo").await.expect("read");
    assert_eq!(w.inbox[0].status, "finance_reviewed");
    assert_eq!(w.inbox[0].original_body, inbox.original_body);
    assert_eq!(w.inbox[0].history[0].outcome, "finance_review");
    assert_eq!(w.carrier_documents[0].status, "posted");
    assert_eq!(w.journal.len(), 2);
    let old = w.carrier_adjustments[0].clone();
    store.mutate_authorized(("demo","finance:reviewer"),"reverse-signed-credit","payable.reverse",&old.id,json!({"expected_load_version":w.loads[5].version,"expected_version":old.version,"reversed_on":Utc::now().date_naive(),"counterparty_contact":"Carrier credit desk","confirmation_reference":"Carrier corrected its confirmation","note":"Retain the original review while correcting its financial classification record"})).await.expect("reverse");
    let current = store.read("demo").await.expect("read");
    let corrected=store.mutate_authorized(("demo","finance:reviewer"),"correct-signed-credit","payable.create",&load.id,json!({"expected_load_version":current.loads[5].version,"invoice_id":old.invoice_id,"kind":old.kind,"number":old.number,"document_amount_cents":old.document_amount_cents,"issued_on":old.issued_on,"description":old.description,"evidence_reference":old.evidence_reference,"carrier_document_id":old.carrier_document_id,"corrects_id":old.id})).await.expect("corrected draft")["data"].clone();
    let current = store.read("demo").await.expect("read");
    assert_eq!(current.inbox[0].status, "finance_review");
    store.mutate_authorized(("demo","finance:reviewer"),"reapprove-signed-credit","payable.review",corrected["id"].as_str().expect("id"),json!({"expected_load_version":current.loads[5].version,"expected_version":1,"decision":"approved","counterparty_contact":"Carrier credit desk","confirmation_reference":"Actual revised confirmation","note":"Reconfirmed the original signed credit amount with retained correction history"})).await.expect("reapprove");
    let current = store.read("demo").await.expect("read");
    assert_eq!(current.inbox[0].status, "finance_reviewed");
    assert_eq!(current.inbox[0].original_body, inbox.original_body);
    assert_eq!(current.journal.len(), 4);
    server.abort();
}
async fn seeded() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("demo");
    s
}
fn integration(url: &str) -> Integrations {
    Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(url.into()),
                edi_token: Some("edi-test-token".into()),
                edi_tenant_id: Some("edi-tenant".into()),
                callback_keys: BTreeMap::from([("1".into(), SECRET.into())]),
                ..Connection::default()
            },
        )]),
    }
}
fn router(store: Store, integrations: Integrations) -> Router {
    api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store,
        integrations: Arc::new(integrations),
        keys: Arc::new(BTreeMap::from([
            ("operator".into(), "demo".into()),
            ("other".into(), "other-tenant".into()),
        ])),
        demo: true,
    })
}
fn notification(id: &str, set: &str) -> Value {
    json!({"type":"document.recorded","tenant_id":"edi-tenant","document_id":id,"partner_id":"atfl","transaction_set":set,"direction":"inbound","status":"accepted"})
}
fn detail(note: &Value, parsed: Value) -> Value {
    json!({"success":true,"data":{"document":{"id":note["document_id"],"partner_id":note["partner_id"],"transaction_set":note["transaction_set"],"direction":"inbound","status":"accepted","processed_at":Utc::now()},"parsed":parsed}})
}
async fn callback(app: &Router, delivery: &str, note: &Value, valid: bool) -> (StatusCode, Value) {
    let body = note.to_string();
    let timestamp = Utc::now().timestamp().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).expect("mac");
    mac.update(format!("{timestamp}.{delivery}.").as_bytes());
    mac.update(body.as_bytes());
    let signature = if valid {
        format!("v1={}", hex::encode(mac.finalize().into_bytes()))
    } else {
        "v1=0000".into()
    };
    let request = Request::builder()
        .method("POST")
        .uri("/webhooks/edi/demo")
        .header("content-type", "application/json")
        .header("x-stateset-key-version", "1")
        .header("x-stateset-event-id", delivery)
        .header("x-stateset-timestamp", timestamp)
        .header("x-stateset-signature", signature)
        .body(Body::from(body))
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}
async fn retry(app: &Router, id: &str, token: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/inbox/{id}/retry"))
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", uuid::Uuid::new_v4().to_string())
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    (
        format!("http://{address}"),
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") }),
    )
}

#[tokio::test]
async fn callback_is_retained_before_upstream_io_and_recoverable_after_outage() {
    let calls = Arc::new(AtomicUsize::new(0));
    let online = Arc::new(AtomicBool::new(false));
    let note = notification("doc-outage", "990");
    let response = detail(
        &note,
        json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":"A"}),
    );
    let (count, available) = (calls.clone(), online.clone());
    let mock = Router::new().route(
        "/v1/documents/doc-outage",
        get(move || {
            let count = count.clone();
            let available = available.clone();
            let response = response.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                if available.load(Ordering::SeqCst) {
                    (StatusCode::OK, Json(response))
                } else {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"offline"})),
                    )
                }
            }
        }),
    );
    let (url, server) = serve(mock).await;
    let integrations = integration(&url);
    let s = seeded().await;
    let app = router(s.clone(), integrations.clone());
    assert_eq!(
        callback(&app, "invalid", &note, false).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(s.read("demo").await.expect("read").inbox.is_empty());
    let (status, response) = callback(&app, "delivery-1", &note, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(response["action"], "queued");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let inbox_id = response["inbox_id"].as_str().expect("inbox id");
    let duplicate = callback(&app, "second-subscription", &note, true).await;
    assert_eq!(duplicate.1["replayed"], true);
    assert_eq!(duplicate.1["inbox_id"], inbox_id);
    let queued = s.read("demo").await.expect("read");
    assert_eq!(queued.inbox.len(), 1);
    assert_eq!(queued.inbox[0].original_body, note.to_string());
    assert_eq!(queued.inbox[0].delivery_ids.len(), 2);
    let proof = queued.inbox[0]
        .authentication
        .as_ref()
        .expect("original signature retained");
    stateset_freight::integrations::verify_callback(
        SECRET,
        &proof.timestamp,
        &proof.delivery_id,
        &proof.signature,
        queued.inbox[0].original_body.as_bytes(),
    )
    .expect("independently verifiable stored signature");
    let mut conflict = note.clone();
    conflict["partner_id"] = json!("another-carrier");
    assert_eq!(
        callback(&app, "delivery-1", &conflict, true).await.0,
        StatusCode::CONFLICT
    );
    integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("failed attempt retained");
    let failed = s.read("demo").await.expect("read");
    assert_eq!(failed.inbox[0].status, "retry");
    assert_eq!(failed.inbox[0].total_attempts, 1);
    let failure_case = failed
        .operations_cases
        .iter()
        .find(|case| case.source.kind == "inbound_failure" && case.source.subject_id == inbox_id)
        .expect("the first actual EDI failure must create an owned-work candidate");
    assert_eq!(failure_case.status, "open");
    let failure_id = failure_case.id.clone();
    assert_eq!(retry(&app, inbox_id, "other").await, StatusCode::NOT_FOUND);
    online.store(true, Ordering::SeqCst);
    assert_eq!(retry(&app, inbox_id, "operator").await, StatusCode::OK);
    assert_eq!(
        s.read("demo")
            .await
            .expect("read")
            .operations_cases
            .iter()
            .find(|case| case.id == failure_id)
            .expect("same failure after retry request")
            .status,
        "open"
    );
    integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("recover");
    let recovered = s.read("demo").await.expect("read");
    assert_eq!(recovered.inbox[0].status, "applied");
    assert_eq!(recovered.inbox[0].total_attempts, 2);
    assert_eq!(recovered.inbox[0].history.len(), 2);
    assert_eq!(recovered.inbox[0].history[0].outcome, "failed");
    assert_eq!(recovered.inbox[0].history[1].outcome, "applied");
    assert!(recovered.inbox[0].last_error.is_none());
    assert_eq!(
        recovered
            .operations_cases
            .iter()
            .find(|case| case.id == failure_id)
            .expect("retained failure history")
            .status,
        "resolved"
    );
    assert_eq!(recovered.loads[2].status, Status::Accepted);
    assert_eq!(
        callback(&app, "delivery-1", &note, true).await.1["status"],
        "applied"
    );
    assert!(!integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("nothing to process"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(recovered
        .events
        .iter()
        .filter(|v| v.kind == "inbox.applied")
        .all(|v| v.actor == "stateset-edi"));
    server.abort();
}

#[tokio::test]
async fn combined_214_is_atomic_and_retries_after_missing_990_is_reconciled() {
    let note = notification("combined-214", "214");
    let response = detail(
        &note,
        json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","events":[{"status_code":"AF","city":"Denver","state":"CO","date":"2026-10-15","time":"1200"},{"status_code":"D1","city":"Phoenix","state":"AZ","date":"2026-10-16","time":"1400"}]}),
    );
    let mock = Router::new().route(
        "/v1/documents/combined-214",
        get(move || {
            let response = response.clone();
            async move { Json(response) }
        }),
    );
    let (url, server) = serve(mock).await;
    let s = seeded().await;
    let integrations = integration(&url);
    let app = router(s.clone(), integrations.clone());
    let original = s.read("demo").await.expect("read");
    let (_, received) = callback(&app, "batch-delivery", &note, true).await;
    let id = received["inbox_id"].as_str().expect("id");
    integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("attempt");
    let failed = s.read("demo").await.expect("read");
    assert_eq!(json!(failed.loads), json!(original.loads));
    assert_eq!(failed.inbox[0].status, "retry");
    assert!(failed.inbox[0].normalized_event.is_some());
    assert!(!failed
        .events
        .iter()
        .any(|v| v.kind == "tracking.observation"));
    s.mutate(
        "demo",
        "carrier-accepted",
        "load.command",
        "load_3",
        json!({"action":"accept","expected_version":1}),
    )
    .await
    .expect("missing acceptance reconciled");
    assert_eq!(retry(&app, id, "operator").await, StatusCode::OK);
    integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("recovered batch");
    let done = s.read("demo").await.expect("read");
    assert_eq!(done.loads[2].status, Status::Delivered);
    assert_eq!(done.loads[2].version, 3);
    assert_eq!(done.loads[2].location, "Phoenix, AZ");
    assert!(done.loads[2].pod_reference.is_none());
    let observations: Vec<_> = done
        .events
        .iter()
        .filter(|v| v.kind == "tracking.observation")
        .collect();
    assert_eq!(observations.len(), 2);
    assert_eq!(
        observations[0].details.as_ref().expect("details")["observation"]["status_code"],
        "AF"
    );
    assert_eq!(
        observations[1].details.as_ref().expect("details")["observation"]["time"],
        "1400"
    );
    server.abort();
}

#[tokio::test]
async fn a_bad_later_214_observation_does_not_commit_an_earlier_pickup() {
    let s = seeded().await;
    s.mutate(
        "demo",
        "accept",
        "load.command",
        "load_3",
        json!({"action":"accept","expected_version":1}),
    )
    .await
    .expect("accept");
    let before = s.read("demo").await.expect("read");
    let result=s.mutate("demo","bad-batch","edi.event","",json!({"event_id":"bad-batch","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":"FR-1050","partner_id":"atfl","latest_status":"ZZ","events":[{"status_code":"AF"},{"status_code":"ZZ"}]}})).await;
    assert!(result.is_err());
    assert_eq!(json!(s.read("demo").await.expect("read")), json!(before));
}

#[tokio::test]
async fn delayed_lifecycle_evidence_recovers_without_lowering_the_tracking_watermark() {
    let s = seeded().await;
    s.mutate(
        "demo",
        "accept-late",
        "load.command",
        "load_3",
        json!({"action":"accept","expected_version":1}),
    )
    .await
    .expect("accept");
    let newest = Utc::now();
    let tracking = |id: &str, code: &str, hours: i64, city: &str| {
        json!({
            "event_id":id,"event_type":"edi.shipment_status.received",
            "occurred_at":newest-chrono::Duration::hours(hours),
            "payload":{"shipment_id":"FR-1050","partner_id":"atfl","latest_status":code,
                "events":[{"status_code":code,"city":city,"state":"AZ"}]}
        })
    };
    s.mutate(
        "demo",
        "appointment",
        "edi.event",
        "",
        tracking("appointment", "AG", 0, "Phoenix"),
    )
    .await
    .expect("appointment confirmed without pickup");
    // Delivery initially fails because receipt order cannot substitute for pickup evidence.
    assert!(s
        .mutate(
            "demo",
            "delayed-delivery",
            "edi.event",
            "",
            tracking("delivery", "D1", 1, "Phoenix")
        )
        .await
        .is_err());
    s.mutate(
        "demo",
        "delayed-pickup",
        "edi.event",
        "",
        tracking("pickup", "AF", 2, "Origin"),
    )
    .await
    .expect("delayed pickup advances lifecycle");
    s.mutate(
        "demo",
        "delayed-delivery",
        "edi.event",
        "",
        tracking("delivery", "D1", 1, "Phoenix"),
    )
    .await
    .expect("retry now has pickup evidence");
    let delivered = s.read("demo").await.expect("read");
    let load = &delivered.loads[2];
    assert_eq!(load.status, Status::Delivered);
    assert_eq!(load.location, "Phoenix, AZ");
    assert_eq!(load.last_tracking_at, Some(newest));
    assert!(load.pod_reference.is_none());
    assert_eq!(
        delivered
            .events
            .iter()
            .filter(|v| v.kind == "tracking.late_progress")
            .count(),
        2
    );
    s.mutate(
        "demo",
        "old-location",
        "edi.event",
        "",
        tracking("old-location", "X6", 1, "Old location"),
    )
    .await
    .expect("old location acknowledged");
    assert_eq!(
        json!(s.read("demo").await.expect("read").loads),
        json!(delivered.loads)
    );
}

#[tokio::test]
async fn older_workspace_snapshots_load_with_an_empty_inbox() {
    let mut previous = json!(stateset_freight::domain::demo());
    previous.as_object_mut().expect("object").remove("inbox");
    let restored: stateset_freight::domain::Workspace =
        serde_json::from_value(previous).expect("backward compatible snapshot");
    assert!(restored.inbox.is_empty());
    assert_eq!(restored.loads.len(), 8);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_retains_inbound_work_and_deduplicates_across_instances() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let a = Store::new(Some(&url)).await.expect("connect");
    let b = Store::new(Some(&url)).await.expect("second instance");
    let tenant = uuid::Uuid::new_v4().to_string();
    let note = notification("persisted-doc", "990");
    let (first, second) = tokio::join!(
        a.receive_document(
            &tenant,
            "delivery",
            "1",
            note.clone(),
            note.to_string(),
            None
        ),
        b.receive_document(
            &tenant,
            "delivery",
            "1",
            note.clone(),
            note.to_string(),
            None
        )
    );
    assert_eq!(
        first.expect("first")["inbox_id"],
        second.expect("second")["inbox_id"]
    );
    drop(a);
    drop(b);
    let restarted = Store::new(Some(&url)).await.expect("restart");
    let saved = restarted.read(&tenant).await.expect("read");
    assert_eq!(saved.inbox.len(), 1);
    assert_eq!(saved.inbox[0].status, "pending");
    assert_eq!(saved.inbox[0].original_body, note.to_string());
    assert!(restarted
        .read("different-tenant")
        .await
        .expect("read")
        .inbox
        .is_empty());
}

#[tokio::test]
async fn historical_signed_movement_is_held_for_carrier_reconciliation_instead_of_applied() {
    let s = seeded().await;
    let release=s.mutate("demo","release","tender.release.request","load_4",json!({"expected_load_version":1,"disposition":"replan","reason":"Release old carrier commitment"})).await.expect("request");
    s.mutate_authorized(("demo","operator:release"),"confirm","tender.release.confirm",release["data"]["id"].as_str().expect("id"),json!({"expected_load_version":2,"carrier_contact":"Old carrier dispatch","confirmation_reference":"RELEASE-ORIGINAL","note":"Confirmed release before pickup"})).await.expect("confirm");
    s.mutate(
        "demo",
        "new-tender",
        "load.command",
        "load_4",
        json!({"action":"tender","expected_version":3,"carrier_id":"ca4"}),
    )
    .await
    .expect("replacement");
    let mut note = notification("historical-pickup", "214");
    note["partner_id"] = json!("brex");
    let response = detail(
        &note,
        json!({"shipment_id":"FR-1051","carrier_scac":"BREX","events":[{"status_code":"AF"}]}),
    );
    let upstream = Router::new().route(
        "/v1/documents/historical-pickup",
        get(move || {
            let response = response.clone();
            async move { Json(response) }
        }),
    );
    let (url, server) = serve(upstream).await;
    let integrations = integration(&url);
    let app = router(s.clone(), integrations.clone());
    assert_eq!(
        callback(&app, "historical-delivery", &note, true).await.0,
        StatusCode::OK
    );
    integrations
        .process_inbound_one(&s, "demo")
        .await
        .expect("processing");
    let held = s.read("demo").await.expect("read");
    assert_eq!(held.inbox[0].status, "reconciliation_required");
    assert_eq!(held.loads[3].status, Status::Tendered);
    assert_eq!(held.tender_incidents.len(), 1);
    assert_eq!(
        retry(&app, &held.inbox[0].id, "operator").await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    s.mutate_authorized(("demo","operator:resolver"),"resolve","tender.incident.resolve",&held.tender_incidents[0].id,json!({"expected_load_version":held.loads[3].version,"carrier_contact":"Old carrier supervisor","confirmation_reference":"CORRECTION-123","note":"Old pickup scan confirmed erroneous; new assignment is correct"})).await.expect("reconcile");
    let resolved = s.read("demo").await.expect("read");
    assert_eq!(resolved.inbox[0].status, "reconciled");
    assert_eq!(
        resolved.inbox[0].history[0].outcome,
        "reconciliation_required"
    );
    assert_eq!(resolved.inbox[0].original_body, note.to_string());
    assert_eq!(resolved.loads[3].status, Status::Tendered);
    server.abort();
}
