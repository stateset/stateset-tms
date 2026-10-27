#[path = "support/canonical.rs"]
mod canonical;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Status, Workspace},
    integrations::{Connection, Integrations},
    source::{self, SourceGrant},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::RwLock;
use tower::ServiceExt;
fn plan(reference: &str) -> Value {
    let day = (Utc::now() + Duration::days(2)).date_naive();
    json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Original freight","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":123456,"sell_cents":150000})
}
fn order() -> Value {
    json!({"id":uuid::Uuid::new_v4().to_string(),"orderNumber":"ORIGINAL-PO","status":"pending","updatedAt":"2026-10-01T00:00:00Z","lineItems":[{"sku":"SKU-1","quantity":2}],"locationId":"DC-1","fulfillmentStatus":"unfulfilled","tags":["b","a"],"metadata":{"retain":"raw original"}})
}
fn workspace() -> (Workspace, String, Value) {
    let mut w = domain::demo();
    let order = order();
    let load = w
        .apply(
            "sync.import",
            "",
            json!({"order":order,"load":plan("SOURCE-ORIGINAL")}),
        )
        .expect("import");
    (w, load["id"].as_str().expect("load id").into(), order)
}
fn observe(w: &mut Workspace, load: &str, order: Value) -> SourceGrant {
    let v = w.source_check(load).map_or(0, |v| v.version);
    w.observe_source(load, order, v, Utc::now(), "user:source-reader")
        .expect("canonical observation")
}
fn decision(w: &Workspace, load: &str, kind: &str) -> (String, Value) {
    let check = w.source_check(load).expect("check");
    let review = w
        .source_reviews
        .iter()
        .find(|v| Some(&v.id) == check.active_review_id.as_ref())
        .expect("review");
    (
        review.id.clone(),
        json!({"expected_load_version":w.loads.iter().find(|v|v.id==load).expect("load").version,"expected_version":review.version,"expected_digest":review.digest,"decision":kind,"contact":"Actual customer transport lead","reference":"ORIGINAL-EMAIL-123","note":"Customer reviewed the current freight plan and this source status"}),
    )
}
fn resolve(w: &mut Workspace, load: &str, kind: &str) {
    let (id, body) = decision(w, load, kind);
    w.resolve_source(&id, body, "user:ops")
        .expect("review decision");
}
#[test]
fn source_projection_preserves_raw_evidence_and_detects_business_fields_without_poll_churn() {
    let (mut w, id, original) = workspace();
    let version = w.loads.last().expect("load").version;
    let mut benign = original.clone();
    benign["updatedAt"] = json!("2026-10-02T00:00:00Z");
    benign["tracking"] = json!({"tracking_number":"other"});
    benign["status"] = json!("processing");
    benign["tags"] = json!(["a", "b", "a"]);
    benign["netsuiteLastError"] = json!("temporary integration telemetry");
    observe(&mut w, &id, benign.clone());
    assert!(w.source_reviews.is_empty());
    assert_eq!(w.loads.last().expect("load").version, version);
    assert_eq!(w.source_check(&id).expect("check").snapshot, benign);
    for field in [
        "lineItems",
        "locationId",
        "fulfillmentStatus",
        "unknownFutureBusinessField",
    ] {
        let mut changed = benign.clone();
        changed[field] = json!("changed");
        observe(&mut w, &id, changed.clone());
        let review = w.source_reviews.last().expect("review");
        assert!(review.changed_fields.contains(&field.to_string()));
        let count = w.source_reviews.len();
        let version = w.loads.last().expect("load").version;
        observe(&mut w, &id, changed);
        assert_eq!(w.source_reviews.len(), count);
        assert_eq!(w.loads.last().expect("load").version, version);
        assert_eq!(
            w.guard_source(w.loads.last().expect("load")),
            Err(source::SOURCE_HOLD.into())
        );
    }
    assert_eq!(
        w.loads.last().expect("load").source_order.as_ref(),
        Some(&original)
    );
    assert!(w.journal.is_empty());
    assert!(w
        .source_reviews
        .iter()
        .take(3)
        .all(|v| v.status == "superseded" && v.superseded_by_id.is_some()));
}
#[test]
fn source_reversion_and_stale_decisions_cannot_silently_clear_a_hold() {
    let (mut w, id, original) = workspace();
    let mut changed = original.clone();
    changed["lineItems"][0]["quantity"] = json!(9);
    observe(&mut w, &id, changed);
    let (old, body) = decision(&w, &id, "confirm_plan");
    observe(&mut w, &id, original.clone());
    assert!(w.resolve_source(&old, body, "ops").is_err());
    assert_eq!(w.source_reviews[0].status, "superseded");
    assert!(w
        .source_reviews
        .last()
        .expect("review")
        .changed_fields
        .contains(&"upstream_reverted".into()));
    resolve(&mut w, &id, "confirm_plan");
    observe(&mut w, &id, original);
    assert!(w.guard_source(w.loads.last().expect("load")).is_ok());
    // Customer consent is bound to the actual freight plan, not reusable for a new destination.
    w.loads
        .last_mut()
        .expect("load")
        .destination
        .details
        .address_lines = vec!["99 Different Dock".into()];
    assert_eq!(
        w.guard_source(w.loads.last().expect("load")),
        Err(source::SOURCE_HOLD.into())
    );
    assert_eq!(
        w.source_summary(w.loads.last().expect("load"))["held"],
        true
    );
    let snapshot = w.source_check(&id).expect("check").snapshot.clone();
    observe(&mut w, &id, snapshot);
    assert!(w
        .source_reviews
        .last()
        .expect("review")
        .changed_fields
        .contains(&"freight_plan".into()));
}
#[test]
fn cancelled_source_requires_explicit_customer_consent_and_never_authorizes_unsafe_writeback() {
    let (mut w, id, original) = workspace();
    let financial = json!({"journal":w.journal,"invoices":w.invoices});
    let mut cancelled = original.clone();
    cancelled["status"] = json!("cancelled");
    observe(&mut w, &id, cancelled.clone());
    for kind in ["confirm_plan", "cancel_transport"] {
        let (r, b) = decision(&w, &id, kind);
        assert!(w.resolve_source(&r, b, "ops").is_err());
    }
    resolve(&mut w, &id, "authorize_transport");
    observe(&mut w, &id, cancelled);
    assert!(w.guard_source(w.loads.last().expect("load")).is_ok());
    assert_eq!(
        w.source_summary(w.loads.last().expect("load"))["writeback_safe"],
        false
    );
    assert_eq!(
        w.loads.last().expect("load").source_order.as_ref(),
        Some(&original)
    );
    assert_eq!(w.loads.last().expect("load").buy_cents, 123456);
    assert_eq!(
        json!({"journal":w.journal,"invoices":w.invoices}),
        financial
    );
    for status in [
        "cancelled",
        "canceled",
        "returned",
        "fulfilled",
        "delivered",
        "unknown",
    ] {
        assert!(!source::tracking_status_safe(&json!({"status":status})));
    }
}
#[test]
fn stale_or_failed_source_observations_do_not_overwrite_newer_checks_and_expiry_fails_closed() {
    let (mut w, id, original) = workspace();
    let grant = observe(&mut w, &id, original.clone());
    let mut newer = original.clone();
    newer["updatedAt"] = json!("2026-10-03T00:00:00Z");
    observe(&mut w, &id, newer.clone());
    assert!(w.check_source_grant(&grant).is_err());
    assert!(w
        .observe_source(
            &id,
            original.clone(),
            grant.version,
            Utc::now(),
            "old request"
        )
        .is_err());
    assert!(w.fail_source(&id, grant.version, "late failure").is_err());
    let current = w.source_check(&id).expect("check").version;
    for invalid in [json!(null), json!("not a timestamp")] {
        let mut malformed = newer.clone();
        malformed["updatedAt"] = invalid;
        assert!(w
            .observe_source(&id, malformed, current, Utc::now(), "malformed revision")
            .is_err());
    }
    let mut missing = newer.clone();
    missing.as_object_mut().expect("object").remove("updatedAt");
    assert!(w
        .observe_source(&id, missing, current, Utc::now(), "missing revision")
        .is_err());
    assert!(w
        .observe_source(&id, original, current, Utc::now(), "old replica")
        .is_err());
    assert_eq!(w.source_check(&id).expect("check").snapshot, newer);
    w.fail_source(&id, current, "Sync returned HTTP 503")
        .expect("failure");
    assert_eq!(
        w.guard_source(w.loads.last().expect("load")),
        Err(source::SOURCE_UNAVAILABLE.into())
    );
    observe(&mut w, &id, newer);
    w.source_checks.last_mut().expect("check").valid_until =
        Some(Utc::now() - Duration::seconds(1));
    assert_eq!(
        w.guard_source(w.loads.last().expect("load")),
        Err(source::SOURCE_UNAVAILABLE.into())
    );
}
struct Fixture {
    snapshot: Arc<RwLock<Value>>,
    fail: Arc<AtomicBool>,
    reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
    edi: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
    integrations: Integrations,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new(order: Value) -> Self {
        let snapshot = Arc::new(RwLock::new(order));
        let fail = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let edi = Arc::new(AtomicUsize::new(0));
        let router = Router::new()
            .route(
                "/v1/tenants/source-tenant/orders/{id}",
                get({
                    let (s, f, r) = (snapshot.clone(), fail.clone(), reads.clone());
                    move |headers: axum::http::HeaderMap| {
                        let (s, f, r) = (s.clone(), f.clone(), r.clone());
                        async move {
                            assert_eq!(headers["x-stateset-api-key"], "owned-source-key");
                            r.fetch_add(1, Ordering::SeqCst);
                            if f.load(Ordering::SeqCst) {
                                return StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            Json(json!({"data":s.read().await.clone()})).into_response()
                        }
                    }
                }),
            )
            .route(
                "/v1/tenants/source-tenant/orders/{id}/tracking",
                post({
                    let (s, n) = (snapshot.clone(), writes.clone());
                    move |Json(body): Json<Value>| {
                        let (s, n) = (s.clone(), n.clone());
                        async move {
                            n.fetch_add(1, Ordering::SeqCst);
                            let mut s = s.write().await;
                            s["status"] = json!("shipped");
                            s["tracking"] = body["trackingData"].clone();
                            Json(json!({"data":{"ok":true}}))
                        }
                    }
                }),
            )
            .route(
                "/v1/edi/outbound/204/{partner}",
                post({
                    let n = edi.clone();
                    move || {
                        let n = n.clone();
                        async move {
                            n.fetch_add(1, Ordering::SeqCst);
                            "ISA*TEST OWNED INTERCHANGE"
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned port");
        let base = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("fixture");
        });
        let integrations = Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    sync_url: Some(base.clone()),
                    sync_api_key: Some("owned-source-key".into()),
                    sync_tenant_id: Some("source-tenant".into()),
                    edi_url: Some(base),
                    edi_token: Some("owned-edi".into()),
                    edi_tenant_id: Some("demo".into()),
                    callback_keys: BTreeMap::new(),
                    ..Default::default()
                },
            )]),
        };
        Self {
            snapshot,
            fail,
            reads,
            writes,
            edi,
            server,
            integrations,
        }
    }
    fn app(&self, s: Store) -> Router {
        api::router(App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: s,
            keys: Arc::new(BTreeMap::from([
                (
                    "carrier".into(),
                    Credential::Scoped(Principal {
                        tenant_id: "demo".into(),
                        subject: "carrier:source".into(),
                        role: Role::Carrier,
                        counterparty_id: Some("ca4".into()),
                    }),
                ),
                (
                    "ops".into(),
                    Credential::Scoped(Principal {
                        tenant_id: "demo".into(),
                        subject: "user:ops".into(),
                        role: Role::Operations,
                        counterparty_id: None,
                    }),
                ),
                (
                    "viewer".into(),
                    Credential::Scoped(Principal {
                        tenant_id: "demo".into(),
                        subject: "user:viewer".into(),
                        role: Role::Viewer,
                        counterparty_id: None,
                    }),
                ),
            ])),
            demo: true,
            integrations: Arc::new(self.integrations.clone()),
        })
    }
}
async fn setup() -> (Store, String, Value) {
    let order = order();
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let load = s
        .mutate(
            "demo",
            "import",
            "sync.import",
            "",
            json!({"order":order,"load":plan("SOURCE-ORIGINAL")}),
        )
        .await
        .expect("import")["data"]
        .clone();
    (s, load["id"].as_str().expect("id").into(), order)
}
async fn call(
    router: &Router,
    path: &str,
    key: &str,
    credential: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::post(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {credential}"))
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}
async fn action(s: &Store, id: &str, action: &str, extra: Value) -> Value {
    let w = s.read("demo").await.expect("read");
    let mut b = json!({"action":action,"expected_version":w.loads.iter().find(|v|v.id==id).expect("load").version});
    for (k, v) in extra.as_object().expect("extra") {
        b[k] = v.clone();
    }
    s.mutate("demo", &domain::id("key"), "load.command", id, b)
        .await
        .expect("action")["data"]
        .clone()
}
#[tokio::test]
async fn api_preflight_persists_cancel_hold_before_rejecting_tender_and_allows_same_key_after_resolution(
) {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order.clone()).await;
    let router = fixture.app(s.clone());
    fixture.snapshot.write().await["status"] = json!("cancelled");
    let body = json!({"action":"tender","carrier_id":"ca4","expected_version":1});
    let (status, result) = call(
        &router,
        &format!("loads/{id}/commands"),
        "same-tender-key",
        "ops",
        body.clone(),
    )
    .await;
    assert_eq!(status, 409, "{result}");
    let w = s.read("demo").await.expect("read");
    assert!(w
        .source_check(&id)
        .expect("check")
        .active_review_id
        .is_some());
    assert!(w.tenders.is_empty());
    assert!(w.outbox.is_empty());
    assert!(!w.receipts.contains_key("same-tender-key"));
    assert_eq!(
        w.loads.last().expect("load").source_order.as_ref(),
        Some(&order)
    );
    let (r, b) = decision(&w, &id, "authorize_transport");
    let (status, result) = call(
        &router,
        &format!("source-reviews/{r}/resolve"),
        "source-decision",
        "ops",
        b,
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let w = s.read("demo").await.expect("read");
    let mut body = body;
    body["expected_version"] = json!(w.loads.last().expect("load").version);
    let (status, result) = call(
        &router,
        &format!("loads/{id}/commands"),
        "same-tender-key",
        "ops",
        body.clone(),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    fixture.fail.store(true, Ordering::SeqCst);
    let reads = fixture.reads.load(Ordering::SeqCst);
    let (status, replay) = call(
        &router,
        &format!("loads/{id}/commands"),
        "same-tender-key",
        "ops",
        body,
    )
    .await;
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["replayed"], true);
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads);
}
#[tokio::test]
async fn refresh_permissions_replay_and_outage_cannot_accept_client_forged_snapshots() {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order).await;
    let router = fixture.app(s.clone());
    let path = format!("loads/{id}/source/refresh");
    assert_eq!(
        call(&router, &path, "denied", "viewer", json!({})).await.0,
        403
    );
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        call(&router, &path, "forged", "ops", json!({"status":"pending"}))
            .await
            .0,
        422
    );
    assert_eq!(fixture.reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        call(&router, &path, "refresh-once", "ops", json!({}))
            .await
            .0,
        200
    );
    let reads = fixture.reads.load(Ordering::SeqCst);
    fixture.fail.store(true, Ordering::SeqCst);
    let (status, replay) = call(&router, &path, "refresh-once", "ops", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(replay["replayed"], true);
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads);
    assert_eq!(
        call(&router, &path, "refresh-once", "viewer", json!({}))
            .await
            .0,
        403
    );
    let body = json!({"action":"tender","carrier_id":"ca4","expected_version":1});
    assert_eq!(
        call(
            &router,
            &format!("loads/{id}/commands"),
            "outage-tender",
            "ops",
            body
        )
        .await
        .0,
        503
    );
    let w = s.read("demo").await.expect("read");
    assert!(w.tenders.is_empty());
    assert!(w.source_check(&id).expect("check").last_error.is_some());
    assert!(w.source_reviews.is_empty());
}
#[tokio::test]
async fn queued_tender_rechecks_source_without_losing_unsent_proof_or_starving_other_loads() {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order).await;
    action(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    let local = s
        .mutate("demo", "local", "load.create", "", plan("LOCAL-OTHER"))
        .await
        .expect("local")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    action(&s, &local, "tender", json!({"carrier_id":"ca4"})).await;
    fixture.snapshot.write().await["status"] = json!("cancelled");
    assert!(fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("dispatch other"));
    assert_eq!(fixture.edi.load(Ordering::SeqCst), 1);
    let w = s.read("demo").await.expect("read");
    let job = w.outbox.iter().find(|v| v.load_id == id).expect("job");
    assert_eq!(job.transmission, "not_started");
    assert_eq!(job.attempts, 0);
    assert_eq!(
        w.loads.iter().find(|v| v.id == id).expect("load").status,
        Status::Tendered
    );
    // Safe withdrawal remains possible; a source hold cannot fabricate a carrier release.
    action(
        &s,
        &id,
        "withdraw_tender",
        json!({"note":"Customer cancelled before transmission"}),
    )
    .await;
}
#[tokio::test]
async fn physical_facts_continue_during_source_hold_but_unsafe_tracking_never_changes_canonical_status(
) {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order).await;
    action(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    action(&s, &id, "accept", json!({})).await;
    fixture.snapshot.write().await["status"] = json!("cancelled");
    fixture
        .integrations
        .refresh_source(&s, "demo", &id, "ops", None)
        .await
        .expect("observe cancellation");
    let w = s.read("demo").await.expect("read");
    let shipment = &w.tenders[0].shipment_reference;
    s.mutate("demo","physical-pickup","edi.event","",json!({"event_id":"source-held-pickup","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":shipment,"partner_id":"brex","carrier_scac":"BREX","latest_status":"AF","events":[{"status_code":"AF"}]}})).await.expect("physical pickup");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("hold writeback");
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.loads.last().expect("load").status, Status::InTransit);
    let sync = w
        .outbox
        .iter()
        .find(|v| v.destination == "stateset-sync-server")
        .expect("tracking");
    assert_eq!(sync.status, "held");
    assert_eq!(sync.total_attempts, 0);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.snapshot.read().await["status"], "cancelled");
    // Explicit continued transport consent still cannot overwrite upstream cancellation.
    let (r, b) = decision(&w, &id, "authorize_transport");
    s.mutate("demo", "consent", "source.resolve", &r, b)
        .await
        .expect("consent");
    s.mutate("demo", "retry-unsafe", "outbox.retry", &sync.id, json!({}))
        .await
        .expect("retry queued");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("hand off the independently queued EDI job");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("recheck tracking retry");
    let retried = s.read("demo").await.expect("read");
    assert_eq!(
        retried
            .outbox
            .iter()
            .find(|v| v.id == sync.id)
            .expect("tracking job")
            .status,
        "held"
    );
    for status in ["returned", "fulfilled", "delivered", "unknown"] {
        fixture.snapshot.write().await["status"] = json!(status);
        s.mutate(
            "demo",
            &domain::id("retry-status"),
            "outbox.retry",
            &sync.id,
            json!({}),
        )
        .await
        .expect("retry for changed canonical state");
        fixture
            .integrations
            .dispatch_one(&s, "demo")
            .await
            .expect("guard unsafe source status");
        let current = s.read("demo").await.expect("read");
        let job = current
            .outbox
            .iter()
            .find(|v| v.id == sync.id)
            .expect("tracking job");
        assert_eq!(job.status, "held");
        assert_eq!(job.total_attempts, 0);
        assert_eq!(fixture.snapshot.read().await["status"], status);
    }
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn own_tracking_shipped_echo_is_benign_but_completed_fulfillment_still_requires_review() {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order).await;
    action(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    action(&s, &id, "accept", json!({})).await;
    let w = s.read("demo").await.expect("read");
    s.mutate("demo","physical","edi.event","",json!({"event_id":"pickup","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":w.tenders[0].shipment_reference,"partner_id":"brex","carrier_scac":"BREX","latest_status":"AF","events":[{"status_code":"AF"}]}})).await.expect("pickup");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("edi");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("tracking");
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.snapshot.read().await["status"], "shipped");
    fixture
        .integrations
        .refresh_source(&s, "demo", &id, "ops", None)
        .await
        .expect("echo");
    assert!(s
        .read("demo")
        .await
        .expect("read")
        .source_reviews
        .is_empty());
    fixture.snapshot.write().await["fulfillmentStatus"] = json!("fulfilled");
    fixture
        .integrations
        .refresh_source(&s, "demo", &id, "ops", None)
        .await
        .expect("business change");
    assert_eq!(s.read("demo").await.expect("read").source_reviews.len(), 1);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_source_observations_and_customer_decisions_are_fenced_atomic_and_durable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first store");
    let b = Store::new(Some(&url)).await.expect("second store");
    let tenant = domain::id("source-review-pg");
    let (w, id, original) = workspace();
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let mut data = json!(w);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned tenant");
    let mut cancelled = original.clone();
    cancelled["status"] = json!("cancelled");
    let (first, second) = tokio::join!(
        a.record_source(
            (&tenant, "poll:first"),
            (&id, cancelled.clone(), 1, Utc::now()),
            None
        ),
        b.record_source(
            (&tenant, "poll:second"),
            (&id, cancelled.clone(), 1, Utc::now()),
            None
        )
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let current = a.read(&tenant).await.expect("read");
    assert_eq!(current.source_reviews.len(), 1);
    let (review, body) = decision(&current, &id, "authorize_transport");
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:first"),
            "decision-a",
            "source.resolve",
            &review,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "ops:second"),
            "decision-b",
            "source.resolve",
            &review,
            body
        )
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|v| *v)
            .count(),
        1
    );
    let stale_grant = SourceGrant {
        load_id: id.clone(),
        version: current.source_check(&id).expect("check").version,
    };
    let current = a.read(&tenant).await.expect("read");
    let input = json!({"action":"tender","carrier_id":"ca4","expected_version":current.loads.last().expect("load").version});
    assert!(b
        .mutate_guarded(
            (&tenant, "ops:stale"),
            ("stale-action", "load.command", &id, input),
            Some(stale_grant)
        )
        .await
        .is_err());
    let restarted = Store::new(Some(&url)).await.expect("reconnect");
    let recovered = restarted.read(&tenant).await.expect("read");
    assert_eq!(json!(recovered), json!(current));
    assert_eq!(
        recovered.loads.last().expect("load").source_order.as_ref(),
        Some(&original)
    );
    assert!(recovered.outbox.is_empty());
    assert!(recovered.journal.is_empty());
    assert_eq!(recovered.source_reviews[0].status, "resolved");
    assert!(!restarted
        .read("unrelated-source-tenant")
        .await
        .expect("isolated")
        .source_checks
        .iter()
        .any(|v| v.load_id == id));
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
}

#[tokio::test]
async fn imported_quote_booking_fetches_canonical_source_before_applying_accepted_rates() {
    let (s, id, original) = setup().await;
    let fixture = Fixture::new(original.clone()).await;
    let router = fixture.app(s.clone());
    let plan = plan("SOURCE-ORIGINAL");
    let q=s.mutate("demo","quote","quote.create","",json!({"reference":"SOURCE-QUOTE-GUARD","planned_load_id":id,"expected_load_version":1,"plan":{
        "customer_id":plan["customer_id"],"mode":plan["mode"],"equipment":plan["equipment"],"commodity":plan["commodity"],"weight_lbs":plan["weight_lbs"],"miles":plan["miles"],"origin":plan["origin"],"destination":plan["destination"]},"sell_cents":160001,"valid_until":Utc::now()+Duration::hours(4),"terms":"Inclusive quote based on the existing source plan"})).await.expect("draft")["data"].clone();
    let qid = q["id"].as_str().expect("quote");
    let q=s.mutate("demo","offer","quote.offer",qid,json!({"expected_version":q["version"],"carrier_id":"ca4","amount_cents":130001,"valid_until":Utc::now()+Duration::hours(4),"contact":"Actual carrier dispatch","reference":"CARRIER-OFFER","note":"Original offer correspondence"})).await.expect("offer")["data"].clone();
    let q=s.mutate("demo","issue","quote.issue",qid,json!({"expected_version":q["version"],"offer_id":q["revisions"][0]["offers"][0]["id"]})).await.expect("issue")["data"].clone();
    let q=s.mutate("demo","accept-quote","quote.decision",qid,json!({"expected_version":q["version"],"decision":"accept","contact":"Actual customer transport lead","reference":"CUSTOMER-ACCEPTANCE","note":"Original customer quote acceptance"})).await.expect("accept quote")["data"].clone();
    let before = s.read("demo").await.expect("before");
    fixture.snapshot.write().await["lineItems"][0]["quantity"] = json!(20);
    let body = json!({"expected_version":q["version"],"expected_load_version":1});
    let (status, result) = call(
        &router,
        &format!("quotes/{qid}/book"),
        "source-quote-book",
        "ops",
        body,
    )
    .await;
    assert_eq!(status, 409, "{result}");
    let held = s.read("demo").await.expect("hold");
    assert_eq!(json!(held.quotes), json!(before.quotes));
    assert_eq!(held.loads.last().expect("load").buy_cents, 123456);
    assert_eq!(
        held.loads.last().expect("load").source_order.as_ref(),
        Some(&original)
    );
    assert!(!held.receipts.contains_key("source-quote-book"));
    assert!(held.outbox.is_empty());
    assert!(held.journal.is_empty());
    let (review, body) = decision(&held, &id, "confirm_plan");
    s.mutate("demo", "confirm-source", "source.resolve", &review, body)
        .await
        .expect("confirm current freight plan");
    let w = s.read("demo").await.expect("read");
    let body = json!({"expected_version":q["version"],"expected_load_version":w.loads.last().expect("load").version});
    let (status, result) = call(
        &router,
        &format!("quotes/{qid}/book"),
        "source-quote-book",
        "ops",
        body,
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let w = s.read("demo").await.expect("booked");
    let load = w.loads.last().expect("load");
    assert_eq!(load.buy_cents, 130001);
    assert_eq!(load.sell_cents, 160001);
    assert_eq!(load.source_order.as_ref(), Some(&original));
    assert_eq!(w.loads.len(), before.loads.len());
    assert!(w.guard_source(load).is_ok());
    let parent = w.quotes.iter().find(|v| v.id == qid).expect("parent");
    let mut changed_plan = json!(stateset_freight::quotes::QuotePlan::from_load(load));
    changed_plan["destination"]["city"] = json!("Tacoma");
    changed_plan["destination"]["address_lines"] = json!(["99 Amended Dock", "Gate B"]);
    let(status,amended)=call(&router,&format!("quotes/{qid}/amendments"),"source-amend-proposal","ops",json!({"expected_version":parent.version,"expected_load_version":load.version,"reference":"SOURCE-AMENDMENT","plan":changed_plan,"sell_cents":170001,"valid_until":Utc::now()+Duration::hours(4),"terms":"Inclusive amended freight agreement","reason":"Customer amended the actual receiving dock"})).await;
    assert_eq!(status, 200, "{amended}");
    let child = amended["data"]["id"].as_str().expect("child");
    let offer=s.mutate("demo","amend-offer","quote.offer",child,json!({"expected_version":1,"carrier_id":"ca1","amount_cents":140001,"valid_until":Utc::now()+Duration::hours(4),"contact":"Actual replacement dispatch","reference":"SOURCE-AMEND-OFFER","note":"Actual inclusive carrier offer"})).await.expect("offer")["data"].clone();
    let issued=s.mutate("demo","amend-issue","quote.issue",child,json!({"expected_version":offer["version"],"offer_id":offer["revisions"][0]["offers"][0]["id"]})).await.expect("issue")["data"].clone();
    fixture.snapshot.write().await["status"] = json!("cancelled");
    let mut consent = json!({"expected_version":issued["version"],"decision":"accept","contact":"Verified customer transport lead","reference":"ACTUAL-AMENDED-SOURCE-CONSENT","note":"Customer explicitly approves amended transport despite cancelled source order","source_digest":w.source_check(&id).expect("check").digest,"authorize_transport":false});
    assert_eq!(
        call(
            &router,
            &format!("quotes/{child}/decision"),
            "amend-accept",
            "ops",
            consent.clone()
        )
        .await
        .0,
        409
    );
    let latest = s.read("demo").await.expect("latest");
    consent["source_digest"] = json!(latest.source_check(&id).expect("check").digest);
    assert_eq!(
        call(
            &router,
            &format!("quotes/{child}/decision"),
            "amend-accept",
            "ops",
            consent.clone()
        )
        .await
        .0,
        422
    );
    consent["authorize_transport"] = json!(true);
    let (status, accepted) = call(
        &router,
        &format!("quotes/{child}/decision"),
        "amend-accept",
        "ops",
        consent,
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    fixture.snapshot.write().await["lineItems"][0]["quantity"] = json!(21);
    let latest = s.read("demo").await.expect("latest");
    let book = json!({"expected_version":accepted["data"]["version"],"expected_load_version":latest.loads.last().expect("load").version});
    assert_eq!(
        call(
            &router,
            &format!("quotes/{child}/book"),
            "amend-book",
            "ops",
            book
        )
        .await
        .0,
        409
    );
    let held = s.read("demo").await.expect("held");
    assert!(held
        .quotes
        .iter()
        .find(|v| v.id == qid)
        .expect("original")
        .active_booking(&id));
    assert_eq!(held.loads.last().expect("load").buy_cents, 130001);
    let withdrawn=s.mutate("demo","withdraw-amendment","quote.decision",child,json!({"expected_version":accepted["data"]["version"],"decision":"withdraw","contact":"Verified customer transport lead","reference":"REVIEW-NEW-SOURCE","note":"Customer withdraws stale source consent to review the changed item quantity"})).await.expect("withdraw")["data"].clone();
    let revised=s.mutate("demo","revise-amendment","quote.revise",child,json!({"expected_version":withdrawn["version"],"expected_parent_version":held.quotes.iter().find(|v|v.id==qid).expect("parent").version,"expected_load_version":held.loads.last().expect("load").version,"plan":changed_plan,"sell_cents":170001,"valid_until":Utc::now()+Duration::hours(4),"terms":"Inclusive amended agreement after current source review","reason":"Customer reviewed current source quantity"})).await.expect("revise")["data"].clone();
    let offer=s.mutate("demo","offer-amendment-v2","quote.offer",child,json!({"expected_version":revised["version"],"carrier_id":"ca1","amount_cents":140001,"valid_until":Utc::now()+Duration::hours(4),"contact":"Actual replacement dispatch","reference":"SOURCE-AMEND-OFFER-V2","note":"Actual reconfirmed inclusive carrier offer"})).await.expect("offer")["data"].clone();
    let offer_id = offer["revisions"]
        .as_array()
        .expect("revisions")
        .last()
        .expect("revision")["offers"][0]["id"]
        .clone();
    let issued = s
        .mutate(
            "demo",
            "issue-amendment-v2",
            "quote.issue",
            child,
            json!({"expected_version":offer["version"],"offer_id":offer_id}),
        )
        .await
        .expect("issue")["data"]
        .clone();
    let latest = s.read("demo").await.expect("latest");
    let consent = json!({"expected_version":issued["version"],"decision":"accept","contact":"Verified customer transport lead","reference":"ACTUAL-AMENDED-SOURCE-CONSENT-V2","note":"Customer explicitly approves current amended plan and cancelled commerce source","source_digest":latest.source_check(&id).expect("check").digest,"authorize_transport":true});
    let (status, accepted) = call(
        &router,
        &format!("quotes/{child}/decision"),
        "amend-accept-v2",
        "ops",
        consent,
    )
    .await;
    assert_eq!(status, 200, "{accepted}");
    let latest = s.read("demo").await.expect("latest");
    let(status,activated)=call(&router,&format!("quotes/{child}/book"),"amend-book","ops",json!({"expected_version":accepted["data"]["version"],"expected_load_version":latest.loads.last().expect("load").version})).await;
    assert_eq!(status, 200, "{activated}");
    let latest = s.read("demo").await.expect("latest");
    let load = latest.loads.last().expect("load");
    assert!(latest.guard_source(load).is_ok());
    assert_eq!(load.destination.city, "Tacoma");
    assert_eq!(load.source_order.as_ref(), Some(&original));
    assert_eq!(load.sell_cents, 170001);
    assert!(latest.journal.is_empty());
    assert!(latest.outbox.is_empty());
    assert_eq!(
        latest
            .source_reviews
            .last()
            .expect("source proof")
            .decision
            .as_ref()
            .expect("decision")
            .quote_id
            .as_deref(),
        Some(child)
    );
    assert_eq!(
        latest
            .quotes
            .iter()
            .filter(|q| q.active_booking(&id))
            .count(),
        1
    );
    assert_eq!(fixture.snapshot.read().await["status"], "cancelled");
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pre_pickup_tracking_remains_local_without_blocking_later_physical_sync_writeback() {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order).await;
    action(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    action(&s, &id, "withdraw_tender", json!({})).await;
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("retain planning fact");
    let w = s.read("demo").await.expect("read");
    let retained = w
        .outbox
        .iter()
        .find(|v| v.load_id == id && v.destination == "stateset-sync-server")
        .expect("withdrawal tracking");
    let retained_id = retained.id.clone();
    let payload = retained.payload.clone();
    assert_eq!(retained.status, "local_only");
    assert_eq!(retained.transmission, "not_started");
    assert_eq!(retained.total_attempts, 0);
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.snapshot.read().await["status"], "pending");
    assert!(s
        .mutate(
            "demo",
            "unsafe-planning-retry",
            "outbox.retry",
            &retained_id,
            json!({})
        )
        .await
        .is_err());
    action(&s, &id, "tender", json!({"carrier_id":"ca4"})).await;
    action(&s, &id, "accept", json!({})).await;
    let w = s.read("demo").await.expect("read");
    let tender = w
        .tenders
        .iter()
        .find(|v| Some(&v.id) == w.loads.last().expect("load").tender_id.as_ref())
        .expect("new tender");
    s.mutate("demo","actual-pickup-after-replanning","edi.event","",json!({"event_id":"actual-pickup-after-replanning","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":tender.shipment_reference,"partner_id":"brex","carrier_scac":"BREX","latest_status":"AF","events":[{"status_code":"AF"}]}})).await.expect("actual pickup");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("new EDI tender");
    fixture
        .integrations
        .dispatch_one(&s, "demo")
        .await
        .expect("physical pickup tracking");
    assert_eq!(fixture.writes.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.snapshot.read().await["status"], "shipped");
    fixture
        .integrations
        .refresh_source(&s, "demo", &id, "ops", None)
        .await
        .expect("verify legitimate shipped echo");
    let w = s.read("demo").await.expect("read");
    let retained = w
        .outbox
        .iter()
        .find(|v| v.id == retained_id)
        .expect("original fact");
    assert_eq!(retained.status, "local_only");
    assert_eq!(retained.payload, payload);
    assert_eq!(retained.total_attempts, 0);
    assert!(w.source_reviews.is_empty());
    assert_eq!(w.loads.last().expect("load").status, Status::InTransit);
}

#[tokio::test]
async fn carrier_rate_confirmation_checks_canonical_sync_before_issuance_and_replays_without_remote_io(
) {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order.clone()).await;
    let router = fixture.app(s.clone());
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let (status, result) = call(
        &router,
        &format!("loads/{id}/commands"),
        "rate-source-tender",
        "ops",
        json!({"action":"tender","expected_version":l.version,"carrier_id":"ca4"}),
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let body = json!({"expected_load_version":l.version,"terms":"Actual agreed carrier transport terms","reason":"Issue exact source-bound carrier confirmation"});
    let reads = fixture.reads.load(Ordering::SeqCst);
    let path = format!("loads/{id}/rate-confirmations");
    assert_eq!(
        call(&router, &path, "rate-viewer", "viewer", body.clone())
            .await
            .0,
        403
    );
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads);
    let (status, result) = call(&router, &path, "rate-source-issue", "ops", body.clone()).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads + 1);
    let doc = result["data"].clone();
    fixture.snapshot.write().await["status"] = json!("cancelled");
    let (status, result) = call(&router, &path, "rate-source-issue", "ops", body).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["replayed"], true);
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads + 1);
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let (status,result)=call(&router,&path,"rate-source-held","ops",json!({"expected_load_version":l.version,"expected_previous_id":doc["id"],"terms":"Do not issue against a cancelled commerce order","reason":"Requires fresh source verification"})).await;
    assert_eq!(status, 409, "{result}");
    let w = s.read("demo").await.expect("read");
    assert_eq!(w.rate_confirmations.len(), 1);
    assert!(w
        .source_check(&id)
        .expect("check")
        .active_review_id
        .is_some());
    assert!(!w.receipts.contains_key("rate-source-held"));
    assert_eq!(
        w.loads
            .iter()
            .find(|v| v.id == id)
            .expect("load")
            .source_order
            .as_ref(),
        Some(&order)
    );
}

#[tokio::test]
async fn carrier_portal_publication_and_native_acceptance_fetch_canonical_source_but_decline_stays_available_during_cancellation(
) {
    let (s, id, order) = setup().await;
    let fixture = Fixture::new(order.clone()).await;
    let router = fixture.app(s.clone());
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    assert_eq!(
        call(
            &router,
            &format!("loads/{id}/commands"),
            "portal-source-tender",
            "ops",
            json!({"action":"tender","expected_version":l.version,"carrier_id":"ca4"})
        )
        .await
        .0,
        200
    );
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let (status,doc)=call(&router,&format!("loads/{id}/rate-confirmations"),"portal-source-rate","ops",json!({"expected_load_version":l.version,"terms":"Actual carrier-facing terms","reason":"Exact source-bound partner offer"})).await;
    assert_eq!(status, 200, "{doc}");
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let reads = fixture.reads.load(Ordering::SeqCst);
    let (status, pubbed) = call(
        &router,
        &format!("loads/{id}/portal-offers"),
        "portal-source-publish",
        "ops",
        json!({"expected_load_version":l.version,"rate_confirmation_id":doc["data"]["id"]}),
    )
    .await;
    assert_eq!(status, 200, "{pubbed}");
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads + 1);
    fixture.snapshot.write().await["status"] = json!("cancelled");
    let l = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load");
    let path = format!(
        "portal/offers/{}/response",
        pubbed["data"]["id"].as_str().expect("offer")
    );
    let mut input = json!({"expected_load_version":l.version,"decision":"accept","reference":"Actual carrier acceptance attempt","note":"Carrier reviewed published terms","rate_confirmation_id":doc["data"]["id"],"rate_sha256":doc["data"]["sha256"]});
    let (status, result) = call(
        &router,
        &path,
        "portal-native-accept",
        "carrier",
        input.clone(),
    )
    .await;
    assert_eq!(status, 409, "{result}");
    let w = s.read("demo").await.expect("read");
    assert!(w
        .source_check(&id)
        .expect("source")
        .active_review_id
        .is_some());
    assert!(w.portal_decisions.is_empty());
    assert!(!w.receipts.contains_key("portal-native-accept"));
    input["decision"] = json!("decline");
    input["expected_load_version"] =
        json!(w.loads.iter().find(|v| v.id == id).expect("load").version);
    let reads = fixture.reads.load(Ordering::SeqCst);
    let (status, result) = call(&router, &path, "portal-native-decline", "carrier", input).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(fixture.reads.load(Ordering::SeqCst), reads);
    assert_eq!(fixture.snapshot.read().await["status"], "cancelled");
}
