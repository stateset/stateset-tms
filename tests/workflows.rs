use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::Sha256;
use stateset_freight::{
    api::{self, App},
    domain::{self, Status},
    integrations::{verify_callback, Connection, Integrations},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tower::ServiceExt;

async fn store() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
fn app(store: Store, integrations: Integrations) -> Router {
    api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store,
        keys: Arc::new(BTreeMap::from([
            ("key-a".into(), "demo".into()),
            ("key-b".into(), "other".into()),
        ])),
        demo: true,
        integrations: Arc::new(integrations),
    })
}
async fn call(
    router: &Router,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {key}"))
        .header("idempotency-key", domain::id("req"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}
fn plan(reference: &str) -> Value {
    let today = Utc::now().date_naive();
    json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":24000,"miles":180,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":today,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":today,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":123456,"sell_cents":150000})
}
async fn command(s: &Store, id: &str, action: &str, extra: Value) -> Result<Value, String> {
    let w = s.read("demo").await.expect("read");
    let version = w.loads.iter().find(|v| v.id == id).expect("load").version;
    let mut body = json!({"action":action,"expected_version":version});
    if let Some(fields) = extra.as_object() {
        for (k, v) in fields {
            body[k] = v.clone();
        }
    }
    s.mutate("demo", &domain::id("key"), "load.command", id, body)
        .await
}

#[test]
fn decimal_money_has_no_rounding_or_overflow() {
    for (value, expected) in [("1234.56", 123456), ("0.01", 1), ("15.1", 1510), ("0", 0)] {
        assert_eq!(domain::decimal_cents(value).expect("amount"), expected);
    }
    for value in [
        "-1",
        "1.001",
        "NaN",
        "1e5",
        "9223372036854775807",
        "1000000.01",
        ".01",
    ] {
        assert!(domain::decimal_cents(value).is_err(), "{value}");
    }
}

#[tokio::test]
async fn import_replay_retains_original_source_snapshot_and_operator_attribution() {
    let s = store().await;
    let source = json!({"id":"00000000-0000-4000-8000-000000000999","orderNumber":"PO-ORIGINAL","status":"pending"});
    let input = plan("FR-REPLAY");
    let first = s
        .mutate_authorized(
            ("demo", "credential:operator-one"),
            "import-key",
            "sync.import",
            "",
            json!({"order":source,"load":input}),
        )
        .await
        .expect("import");
    let second=s.mutate("demo","import-key","sync.import","",json!({"order":{"id":source["id"],"orderNumber":"PO-CHANGED","status":"shipped"},"load":input})).await.expect("replay");
    assert_eq!(second["replayed"], true);
    assert_eq!(first["data"], second["data"]);
    assert!(s
        .replay(
            "demo",
            "import-key",
            "sync.import",
            "",
            &json!({"order_id":source["id"],"load":input})
        )
        .await
        .expect("offline receipt")
        .is_some());
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.events.last().expect("audit event").actor,
        "credential:operator-one"
    );
    assert_eq!(
        w.loads
            .last()
            .expect("load")
            .source_order
            .as_ref()
            .expect("source")["orderNumber"],
        "PO-ORIGINAL"
    );
}

#[tokio::test]
async fn planning_revisions_and_qualification_renewals_preserve_identity_and_reject_stale_edits() {
    let s = store().await;
    let w = s.read("demo").await.expect("read");
    let original = w.loads.iter().find(|v| v.id == "load_8").expect("load");
    let changed = plan("FR-REVISED");
    let result = s
        .mutate(
            "demo",
            "revise",
            "load.revise",
            &original.id,
            json!({"expected_version":original.version,"plan":changed}),
        )
        .await
        .expect("revise");
    assert_eq!(result["data"]["id"], original.id);
    assert_eq!(result["data"]["version"], original.version + 1);
    assert!(s
        .mutate(
            "demo",
            "stale-plan",
            "load.revise",
            &original.id,
            json!({"expected_version":original.version,"plan":changed})
        )
        .await
        .is_err());
    assert!(s
        .mutate(
            "demo",
            "contracted-plan",
            "load.revise",
            "load_1",
            json!({"expected_version":1,"plan":changed})
        )
        .await
        .is_err());
    let carrier = &w.carriers[0];
    let mut renewed = json!(carrier);
    renewed["insurance_expires"] =
        json!((Utc::now().date_naive() + chrono::Duration::days(730)).to_string());
    let result = s
        .mutate(
            "demo",
            "renew",
            "carrier.update",
            &carrier.id,
            json!({"expected":carrier,"data":renewed}),
        )
        .await
        .expect("renew");
    assert_eq!(result["data"]["id"], carrier.id);
    assert!(s
        .mutate(
            "demo",
            "stale-renew",
            "carrier.update",
            &carrier.id,
            json!({"expected":carrier,"data":renewed})
        )
        .await
        .is_err());
}
#[tokio::test]
async fn authentication_and_tenant_isolation() {
    let router = app(store().await, Integrations::default());
    assert_eq!(
        call(&router, "GET", "/v1/workspace", "invalid", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (_, a) = call(&router, "GET", "/v1/workspace", "key-a", json!({})).await;
    let (_, b) = call(&router, "GET", "/v1/workspace", "key-b", json!({})).await;
    assert_eq!(a["loads"].as_array().expect("loads").len(), 8);
    assert_eq!(b["loads"], json!([]));
    assert!(a.get("receipts").is_none());
    let (status, _) = call(
        &router,
        "POST",
        "/v1/loads/load_1/commands",
        "key-b",
        json!({"action":"deliver","expected_version":1,"pod_reference":"fake"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn idempotency_conflict_and_failed_mutation_are_atomic() {
    let s = store().await;
    let body = plan("FR-IDEMPOTENT");
    let a = s
        .mutate("demo", "same", "load.create", "", body.clone())
        .await
        .expect("create");
    let b = s
        .mutate("demo", "same", "load.create", "", body)
        .await
        .expect("replay");
    assert_eq!(a["data"]["id"], b["data"]["id"]);
    assert_eq!(b["replayed"], true);
    assert!(s
        .mutate("demo", "same", "load.create", "", plan("FR-DIFFERENT"))
        .await
        .is_err());
    let before = json!(s.read("demo").await.expect("read"));
    assert!(
        command(&s, "load_3", "deliver", json!({"pod_reference":"bad"}))
            .await
            .is_err()
    );
    assert_eq!(before, json!(s.read("demo").await.expect("read")));
}
#[tokio::test]
async fn stale_updates_and_double_dispatch_are_rejected() {
    let s = store().await;
    let (a, b) = tokio::join!(
        s.mutate(
            "demo",
            "a",
            "load.command",
            "load_3",
            json!({"action":"accept","expected_version":1})
        ),
        s.mutate(
            "demo",
            "b",
            "load.command",
            "load_3",
            json!({"action":"decline","expected_version":1})
        )
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(command(
        &s,
        "load_4",
        "dispatch",
        json!({"driver_id":"d1","equipment_id":"e1"})
    )
    .await
    .expect_err("assignment conflict")
    .contains("active load"));
}
#[tokio::test]
async fn insurance_and_equipment_qualification_gate_tendering() {
    let s = store().await;
    assert!(command(&s, "load_5", "tender", json!({"carrier_id":"ca1"}))
        .await
        .expect_err("mismatch")
        .contains("equipment"));
    let response=s.mutate("demo","carrier","carrier.create","",json!({"name":"Expired","scac":"EXPD","partner_id":"expired","mc_number":"MC-1","insurance_expires":"2000-01-01","equipment":"Reefer"})).await.expect("carrier");
    assert!(command(
        &s,
        "load_5",
        "tender",
        json!({"carrier_id":response["data"]["id"]})
    )
    .await
    .expect_err("insurance")
    .contains("insurance"));
}
#[tokio::test]
async fn invoice_correction_approval_and_settlement_require_evidence() {
    let s = store().await;
    let w = s.read("demo").await.expect("read");
    let bad = w
        .invoices
        .iter()
        .find(|v| v.status == "exception")
        .expect("exception");
    assert!(s
        .mutate("demo", "bad-approve", "invoice.approve", &bad.id, json!({}))
        .await
        .is_err());
    s.mutate("demo","resolve","invoice.resolve",&bad.id,json!({"expected_amount_cents":108000,"amount_cents":98000,"note":"Corrected invoice received from carrier"})).await.expect("resolve");
    s.mutate("demo", "approve", "invoice.approve", &bad.id, json!({}))
        .await
        .expect("approve");
    let prepared = s
        .mutate("demo", "settle", "settlement.create", "load_7", json!({}))
        .await
        .expect("settlement");
    assert_eq!(prepared["data"]["status"], "prepared");
    assert_eq!(prepared["data"]["amount_cents"], 98000);
    assert!(s
        .mutate(
            "demo",
            "settle-again",
            "settlement.create",
            "load_7",
            json!({})
        )
        .await
        .is_err());
}
#[test]
fn signed_callback_binds_timestamp_id_and_exact_bytes() {
    let timestamp = Utc::now().timestamp().to_string();
    let body = b"{ \"test\": true }";
    let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").expect("mac");
    mac.update(format!("{timestamp}.callback-id.").as_bytes());
    mac.update(body);
    let signature = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
    assert!(verify_callback("secret", &timestamp, "callback-id", &signature, body).is_ok());
    assert!(verify_callback("secret", &timestamp, "different-id", &signature, body).is_err());
    assert!(verify_callback("secret", &timestamp, "callback-id", &signature, b"{}").is_err());
    assert!(verify_callback("secret", "0", "callback-id", &signature, body).is_err());
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    (
        format!("http://{address}"),
        tokio::spawn(async move { axum::serve(listener, router).await.expect("server") }),
    )
}
fn connection(base: &str) -> Integrations {
    Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(base.into()),
                edi_token: Some("edi-key".into()),
                edi_tenant_id: Some("upstream-tenant".into()),
                callback_keys: BTreeMap::from([(
                    "1".into(),
                    "test-callback-secret-at-least-32-characters".into(),
                )]),
                sync_url: Some(base.into()),
                sync_api_key: Some("sync-key".into()),
                sync_tenant_id: Some("sync-tenant".into()),
                ..Default::default()
            },
        )]),
    }
}
#[tokio::test]
async fn edi_handoff_retries_with_same_id_and_valid_204_contract() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mock = Router::new().route(
        "/v1/edi/outbound/204/pclg",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                let count = count.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer edi-key");
                    assert_eq!(headers["x-tenant-id"], "upstream-tenant");
                    assert!(headers["idempotency-key"]
                        .to_str()
                        .expect("key")
                        .starts_with("outbox_"));
                    assert!(!body["orders"].as_array().expect("orders").is_empty());
                    assert_eq!(body["ship_from"]["role"], "SF");
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        (StatusCode::SERVICE_UNAVAILABLE, "temporary failure")
                    } else {
                        (StatusCode::OK, "ISA*mock X12 interchange")
                    }
                }
            },
        ),
    );
    let (url, server) = serve(mock).await;
    let s = store().await;
    let integrations = connection(&url);
    command(&s, "load_5", "tender", json!({"carrier_id":"ca2"}))
        .await
        .expect("tender");
    integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("dispatch");
    let job = s.read("demo").await.expect("read").outbox[0].clone();
    assert_eq!(job.status, "retry");
    s.mutate("demo", "retry", "outbox.retry", &job.id, json!({}))
        .await
        .expect("retry");
    integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("dispatch");
    let retried = s.read("demo").await.expect("read").outbox[0].clone();
    assert_eq!(retried.id, job.id);
    assert_eq!(retried.status, "gateway_accepted");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}
#[tokio::test]
async fn complete_signed_document_callback_lifecycle_and_invoice_audit() {
    let document = Arc::new(tokio::sync::RwLock::new(Value::Null));
    let copy = document.clone();
    let mock = Router::new().route(
        "/v1/documents/{id}",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let copy = copy.clone();
            async move {
                assert_eq!(headers["x-tenant-id"], "upstream-tenant");
                assert_eq!(headers["authorization"], "Bearer edi-key");
                axum::Json(json!({"success":true,"data":copy.read().await.clone()}))
            }
        }),
    );
    let (base, server) = serve(mock).await;
    let s = store().await;
    let integrations = connection(&base);
    let router = app(s.clone(), integrations.clone());
    for (number, set, parsed) in [
        (
            1,
            "990",
            json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":"A"}),
        ),
        (
            2,
            "214",
            json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","events":[{"status_code":"AF","city":"Denver","state":"CO"}]}),
        ),
        (
            3,
            "214",
            json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","events":[{"status_code":"D1","city":"Phoenix","state":"AZ"}]}),
        ),
        (
            4,
            "210",
            json!({"shipment_id":"FR-1050","carrier_scac":"ATFL","invoice_number":"INV-NEW","net_amount":"2750.00","currency":"USD"}),
        ),
    ] {
        let doc_id = format!("doc-{number}");
        let record = json!({"id":doc_id,"partner_id":"atfl","transaction_set":set,"direction":"inbound","status":"accepted","processed_at":Utc::now()});
        *document.write().await = json!({"document":record,"parsed":parsed});
        let notification = json!({"type":"document.recorded","tenant_id":"upstream-tenant","document_id":doc_id,"partner_id":"atfl","transaction_set":set,"direction":"inbound","status":"accepted"});
        let body = notification.to_string();
        let timestamp = Utc::now().timestamp().to_string();
        let event_id = format!("callback-{number}");
        let mut mac =
            Hmac::<Sha256>::new_from_slice(b"test-callback-secret-at-least-32-characters")
                .expect("mac");
        mac.update(format!("{timestamp}.{event_id}.").as_bytes());
        mac.update(body.as_bytes());
        let signature = format!("v1={}", hex::encode(mac.finalize().into_bytes()));
        for repeat in 0..2 {
            let req = Request::builder()
                .method("POST")
                .uri("/webhooks/edi/demo")
                .header("content-type", "application/json")
                .header("x-stateset-key-version", "1")
                .header("x-stateset-event-id", &event_id)
                .header("x-stateset-timestamp", &timestamp)
                .header("x-stateset-signature", &signature)
                .body(Body::from(body.clone()))
                .expect("request");
            let response = router.clone().oneshot(req).await.expect("response");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            if repeat == 1 {
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes).expect("json")["replayed"],
                    true
                );
            }
        }
        assert!(integrations
            .process_inbound_one(&s, "demo")
            .await
            .expect("process inbox"));
    }
    let w = s.read("demo").await.expect("read");
    assert!(w.inbox.iter().all(|v| v.status == "applied"));
    let load = w.loads.iter().find(|v| v.id == "load_3").expect("load");
    assert_eq!(load.status, Status::Delivered);
    assert!(load.driver_id.is_none());
    assert!(load.pod_reference.is_none());
    let invoice = w
        .invoices
        .iter()
        .find(|v| v.number == "INV-NEW")
        .expect("invoice");
    assert_eq!(invoice.amount_cents, 275000);
    assert!(s
        .mutate(
            "demo",
            "approve-early",
            "invoice.approve",
            &invoice.id,
            json!({})
        )
        .await
        .is_err());
    command(
        &s,
        "load_3",
        "attach_pod",
        json!({"pod_reference":"signed-POD-123"}),
    )
    .await
    .expect("pod");
    let load = s.read("demo").await.expect("read").loads[2].clone();
    s.mutate_authorized(("demo","finance:reviewer"),"review-pod","pod.review",&load.id,
        json!({"expected_version":load.version,"decision":"accepted","note":"Inspected signed external delivery evidence against the shipment"}))
        .await.expect("reviewed POD");
    s.mutate(
        "demo",
        "approve-final",
        "invoice.approve",
        &invoice.id,
        json!({}),
    )
    .await
    .expect("approve");
    s.mutate(
        "demo",
        "settlement-final",
        "settlement.create",
        "load_3",
        json!({}),
    )
    .await
    .expect("settle");
    server.abort();
}
#[tokio::test]
async fn sync_order_import_and_tracking_writeback_use_actual_http_shapes() {
    let order_id = "00000000-0000-4000-8000-000000000123";
    let writes = Arc::new(AtomicUsize::new(0));
    let copy = writes.clone();
    let mock=Router::new().route("/v1/tenants/sync-tenant/orders/{id}",axum::routing::get(move |headers:axum::http::HeaderMap|async move {assert_eq!(headers["x-stateset-api-key"],"sync-key");axum::Json(json!({"data":{"id":order_id,"orderNumber":"PO-123","status":"pending","lineItems":[]}}))}))
        .route("/v1/tenants/sync-tenant/orders/{id}/tracking",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<Value>|{let copy=copy.clone();async move {assert_eq!(headers["x-stateset-api-key"],"sync-key");assert_eq!(body["trackingData"]["trackingNumber"],"FR-IMPORT");assert_eq!(body["trackingData"]["freight_status"],"in_transit");if copy.load(Ordering::SeqCst)>0 {assert!(body["trackingData"]["freight_stops"]["pickup"]["arrival_at"].is_string());assert!(body["trackingData"]["freight_stops"]["pickup"].get("reference").is_none());}copy.fetch_add(1,Ordering::SeqCst);axum::Json(json!({"data":{"ok":true}}))}}));
    let (base, server) = serve(mock).await;
    let s = store().await;
    let integrations = connection(&base);
    let router = app(s.clone(), integrations.clone());
    let (status, response) = call(
        &router,
        "POST",
        "/v1/sync/import",
        "key-a",
        json!({"order_id":order_id,"load":plan("FR-IMPORT")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let load_id = response["data"]["id"].as_str().expect("load");
    command(&s, load_id, "tender", json!({"carrier_id":"ca4"}))
        .await
        .expect("tender");
    command(&s, load_id, "accept", json!({}))
        .await
        .expect("accept");
    // External pickup evidence does not claim an internal driver or unit.
    let shipment = s.read("demo").await.expect("read").tenders[0]
        .shipment_reference
        .clone();
    s.mutate("demo","pickup","edi.event","",json!({"event_id":"external-pickup","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":shipment,"partner_id":"brex","carrier_scac":"BREX","latest_status":"AF","events":[{"status_code":"AF"}]}})).await.expect("pickup");
    // EDI and sync are independent destinations; a queued EDI handoff does not block sync.
    integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("edi attempt");
    integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("sync attempt");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    let load = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == load_id)
        .expect("load");
    let observed = (Utc::now() - chrono::Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M")
        .to_string();
    let event=s.mutate_authorized(("demo","user:gate-recorder"),"gate-with-sync","stop.event.record",load_id,json!({"expected_load_version":load.version,"stop":"pickup","kind":"arrival","local_time":observed,"timezone":"UTC","reference":"Gate ledger entry","note":"Confirmed against the actual gate ledger"})).await.expect("gate evidence");
    let w = s.read("demo").await.expect("read");
    assert_eq!(
        w.outbox.last().expect("sync job").payload["trackingData"]["freight_stops"]["pickup"]
            ["arrival_at"],
        event["data"]["occurred_at"]
    );
    integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("gate tracking written through real HTTP shape");
    assert_eq!(writes.load(Ordering::SeqCst), 2);
    assert!(s
        .read("demo")
        .await
        .expect("read")
        .outbox
        .iter()
        .any(|v| v.status == "synced"));
    server.abort();
}
#[tokio::test]
async fn wrong_partner_and_old_tracking_do_not_change_a_load() {
    let s = store().await;
    let before = json!(s.read("demo").await.expect("read"));
    assert!(s.mutate("demo","wrong","edi.event","",json!({"event_id":"bad","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":"FR-1048","partner_id":"atfl","latest_status":"D1"}})).await.is_err());
    assert_eq!(before, json!(s.read("demo").await.expect("read")));
    let event = |at: chrono::DateTime<Utc>| json!({"event_id":domain::id("evt"),"event_type":"edi.shipment_status.received","occurred_at":at,"payload":{"shipment_id":"FR-1048","partner_id":"smtt","latest_status":"X6","events":[{"status_code":"X6","city":"Tacoma"}]}});
    s.mutate("demo", "new", "edi.event", "", event(Utc::now()))
        .await
        .expect("current");
    assert!(s
        .mutate(
            "demo",
            "old",
            "edi.event",
            "",
            event(Utc::now() - chrono::Duration::hours(1))
        )
        .await
        .is_ok());
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL; run with --ignored"]
async fn postgres_persistence_concurrency_and_tenant_isolation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL required");
    let a = Store::new(Some(&url)).await.expect("connect");
    a.seed_demo().await.expect("seed");
    let tenant = domain::id("test");
    a.mutate(
        &tenant,
        "customer",
        "customer.create",
        "",
        json!({"name":"Persistent customer","contact":"ops@example.test"}),
    )
    .await
    .expect("persist");
    let b = Store::new(Some(&url)).await.expect("reconnect");
    assert_eq!(b.read(&tenant).await.expect("read").customers.len(), 1);
    assert!(b
        .read("never-provisioned")
        .await
        .expect("read")
        .customers
        .is_empty());
    let (first, second) = tokio::join!(
        a.mutate(
            &tenant,
            "same",
            "customer.create",
            "",
            json!({"name":"Concurrent","contact":""})
        ),
        b.mutate(
            &tenant,
            "same",
            "customer.create",
            "",
            json!({"name":"Concurrent","contact":""})
        )
    );
    let first = first.expect("first");
    let second = second.expect("second");
    assert_eq!(first["data"]["id"], second["data"]["id"]);
    assert_eq!(b.read(&tenant).await.expect("read").customers.len(), 2);
}
