use super::*;
use crate::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Status},
    integrations::Integrations,
    store::Store,
};
use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    routing::get,
    Json, Router,
};
use http_body_util::BodyExt;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::RwLock;
use tower::ServiceExt;
const ORDER: &str = "00000000-0000-4000-8000-000000000001";
const SHIPMENT: &str = "00000000-0000-4000-8000-000000000005";
fn native_order() -> Value {
    json!({"id":ORDER,"order_number":"COMMERCE-1","customer_id":"00000000-0000-4000-8000-000000000002","status":"confirmed","total_amount":"19.99","currency":"USD","payment_status":"paid","fulfillment_status":"unfulfilled","items":[{"id":"00000000-0000-4000-8000-000000000003","product_id":"00000000-0000-4000-8000-000000000004","sku":"SKU-1","name":"Goods","quantity":2,"shipped_quantity":0,"unit_price":"9.99","total":"19.98"}],"created_at":"2026-10-19T00:00:00Z","updated_at":"2026-10-19T01:00:00Z"})
}
fn native_shipment() -> Value {
    json!({"id":SHIPMENT,"order_id":ORDER,"shipment_number":"NATIVE-1","status":"shipped","version":3,"items":[{"product_id":"00000000-0000-4000-8000-000000000004","sku":"SKU-1","quantity":2}],"tracking_number":"REAL-CARRIER-TRACKING","shipping_cost":"15.00"})
}
fn freight_plan() -> Value {
    let load = &domain::demo().loads[0];
    json!({"reference":"NATIVE-IMPORT","customer_id":load.customer_id,"mode":load.mode,"equipment":load.equipment,"commodity":"Operator freight plan","weight_lbs":1000,"miles":173,"origin":load.origin,"destination":load.destination,"buy_cents":123456,"sell_cents":150000})
}
fn auth(headers: &HeaderMap) {
    assert_eq!(
        headers["authorization"],
        "Bearer owned-native-credential-at-least-32-characters"
    );
    assert_eq!(headers["x-tenant-id"], "native-tenant");
}
struct Fixture {
    router: Router,
    store: Store,
    integrations: Integrations,
    connection: Connection,
    order: Arc<RwLock<Value>>,
    shipment: Arc<RwLock<Value>>,
    shipment_page: Arc<RwLock<Option<Value>>>,
    shipment_queries: Arc<RwLock<Vec<BTreeMap<String, String>>>>,
    fail: Arc<AtomicBool>,
    writes: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        Self::new_at(None).await
    }
    async fn new_at(database: Option<&str>) -> Self {
        let order = Arc::new(RwLock::new(native_order()));
        let shipment = Arc::new(RwLock::new(native_shipment()));
        let shipment_page = Arc::new(RwLock::new(None::<Value>));
        let shipment_queries = Arc::new(RwLock::new(Vec::<BTreeMap<String, String>>::new()));
        let fail = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let source = order.clone();
        let read_count = reads.clone();
        let list = order.clone();
        let native = shipment.clone();
        let patch_native = shipment.clone();
        let listed_native = shipment.clone();
        let listed_page = shipment_page.clone();
        let listed_queries = shipment_queries.clone();
        let patch_count = writes.clone();
        let patch_fail = fail.clone();
        let peer=Router::new().route(&format!("/api/v1/orders/{ORDER}"),get(move |headers:HeaderMap|{let source=source.clone();let reads=read_count.clone();async move {auth(&headers);reads.fetch_add(1,Ordering::SeqCst);Json(source.read().await.clone())}}))
            .route("/api/v1/orders",get(move |headers:HeaderMap|{let list=list.clone();async move {auth(&headers);Json(json!({"orders":[list.read().await.clone()],"total":2,"limit":100,"offset":0,"has_more":true,"next_cursor":"native-cursor"}))}}))
            .route("/api/v1/shipments",get(move |headers:HeaderMap,axum::extract::Query(query):axum::extract::Query<BTreeMap<String,String>>| {
                let native=listed_native.clone();let page=listed_page.clone();let queries=listed_queries.clone();
                async move {auth(&headers);queries.write().await.push(query.clone());
                    if let Some(raw)=page.read().await.clone(){return Json(raw);}
                    let offset=query["offset"].parse::<u32>().expect("offset");
                    Json(json!({"shipments":if offset==0{vec![native.read().await.clone()]}else{vec![]},"total":1,"limit":25,"offset":offset,"has_more":false}))
                }
            }))
            .route(&format!("/api/v1/shipments/{SHIPMENT}"),get(move |headers:HeaderMap|{let native=native.clone();async move {auth(&headers);Json(native.read().await.clone())}}).patch(move |headers:HeaderMap,Json(body):Json<Value>|{let native=patch_native.clone();let writes=patch_count.clone();let fail=patch_fail.clone();async move {auth(&headers);writes.fetch_add(1,Ordering::SeqCst);assert_eq!(body.as_object().expect("object").len(),2);let mut s=native.write().await;if body["expected_version"]!=s["version"] {return (StatusCode::CONFLICT,Json(json!({"error":"version"})));}s["version"]=json!(s["version"].as_i64().expect("version")+1);s["status"]=body["status"].clone();if fail.load(Ordering::SeqCst){(StatusCode::BAD_GATEWAY,Json(json!({"private":"never echo upstream response"})))}else{(StatusCode::OK,Json(s.clone()))}}}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, peer).await.expect("owned peer");
        });
        let connection = Connection {
            commerce_url: Some(origin),
            commerce_token: Some("owned-native-credential-at-least-32-characters".into()),
            commerce_tenant_id: Some("native-tenant".into()),
            ..Default::default()
        };
        let integrations = Integrations {
            tenants: BTreeMap::from([("demo".into(), connection.clone())]),
        };
        let store = Store::new(database).await.expect("owned store");
        store.seed_demo().await.expect("seed");
        let keys = [
            ("ops", Role::Operations, "demo"),
            ("viewer", Role::Viewer, "demo"),
            ("integration", Role::Integration, "demo"),
            ("other", Role::Operations, "other"),
        ]
        .into_iter()
        .map(|(key, role, tenant)| {
            (
                key.into(),
                Credential::Scoped(Principal {
                    tenant_id: tenant.into(),
                    subject: format!("user:{key}"),
                    role,
                    counterparty_id: None,
                }),
            )
        })
        .collect();
        let router = api::router(App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: store.clone(),
            keys: Arc::new(keys),
            demo: true,
            integrations: Arc::new(integrations.clone()),
        });
        Self {
            router,
            store,
            integrations,
            connection,
            order,
            shipment,
            shipment_page,
            shipment_queries,
            fail,
            writes,
            reads,
            server,
        }
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        key: &str,
        role: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {role}"))
            .header("idempotency-key", key)
            .header("content-type", "application/json")
            .body(if method == "GET" {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .expect("request");
        let response = self
            .router
            .clone()
            .oneshot(request)
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
    async fn import(&self) -> String {
        let (status, body) = self
            .call(
                "POST",
                "/v1/commerce/import",
                "native-import",
                "ops",
                json!({"order_id":ORDER,"load":freight_plan()}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["data"]["id"].as_str().expect("load").into()
    }
    async fn physical(&self, id: &str, status: Status) {
        let id = id.to_string();
        self.store
            .update("demo", move |w| {
                let l = w
                    .loads
                    .iter_mut()
                    .find(|l| l.id == id)
                    .expect("fixture load");
                l.status = status;
                l.version += 1;
                Ok(Value::Null)
            })
            .await
            .expect("physical fixture only");
    }
    async fn queue(&self, id: &str, key: &str) -> Value {
        let (_, preview) = self
            .call(
                "GET",
                &format!("/v1/loads/{id}/commerce-tracking?shipment_id={SHIPMENT}"),
                "",
                "ops",
                Value::Null,
            )
            .await;
        let (status,queued)=self.call("POST",&format!("/v1/loads/{id}/commerce-tracking"),key,"ops",json!({"expected_load_version":preview["data"]["expected_load_version"],"shipment_id":SHIPMENT,"expected_shipment_version":preview["data"]["shipment"]["version"]})).await;
        assert_eq!(status, StatusCode::OK, "{queued}");
        queued
    }
}
impl Fixture {
    async fn advanced_handoff(&self) -> (String, String) {
        let id = self.import().await;
        self.physical(&id, Status::InTransit).await;
        let queued = self.queue(&id, "advanced-native").await;
        let job = queued["data"]["job"]["id"]
            .as_str()
            .expect("job")
            .to_owned();
        self.fail.store(true, Ordering::SeqCst);
        assert!(self
            .integrations
            .dispatch_one(&self.store, "demo")
            .await
            .expect("uncertain attempt"));
        self.shipment.write().await["version"] = json!(7);
        self.shipment.write().await["status"] = json!("out_for_delivery");
        (id, job)
    }
    async fn review_body(&self, job: &str) -> Value {
        let (status, p) = self
            .call(
                "GET",
                &format!("/v1/outbox/{job}/commerce-review"),
                "",
                "ops",
                Value::Null,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{p}");
        json!({"expected_load_version":p["data"]["expected_load_version"],"expected_total_attempts":p["data"]["expected_total_attempts"],"expected_shipment_version":p["data"]["shipment"]["version"],"contact":"Native shipping operator","reference":"CASE-EXTERNAL-1","note":"Reviewed external shipment advancement and retained original uncertainty"})
    }
}
#[tokio::test]
async fn reviewed_external_advancement_retires_only_local_intent_and_replays_offline() {
    let f = Fixture::new().await;
    let (id, job) = f.advanced_handoff().await;
    let before = f.store.read("demo").await.expect("before");
    let original = before
        .outbox
        .iter()
        .find(|j| j.id == job)
        .expect("original");
    let body = f.review_body(&job).await;
    let path = format!("/v1/outbox/{job}/commerce-review");
    let (status, receipt) = f
        .call("POST", &path, "review-original", "ops", body.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["data"]["status"], "external_reviewed");
    assert_eq!(receipt["data"]["review"]["reviewed_by"], "user:ops");
    assert_eq!(
        receipt["data"]["review"]["original_patch_outcome"],
        "unknown"
    );
    let after = f.store.read("demo").await.expect("after");
    let reviewed = after.outbox.iter().find(|j| j.id == job).expect("reviewed");
    assert_eq!(reviewed.payload, original.payload);
    assert_eq!(reviewed.total_attempts, original.total_attempts);
    assert_eq!(reviewed.attempts, original.attempts);
    assert_eq!(reviewed.transmission, original.transmission);
    assert_eq!(reviewed.path, original.path);
    assert_eq!(reviewed.method, original.method);
    let review_evidence: Value =
        serde_json::from_str(reviewed.response.as_ref().expect("review evidence")).expect("record");
    assert_eq!(
        review_evidence["original_error"],
        json!(original.last_error)
    );
    assert_eq!(
        review_evidence["original_response"],
        json!(original.response)
    );
    assert_eq!(
        after
            .loads
            .iter()
            .find(|l| l.id == id)
            .expect("load")
            .status,
        Status::InTransit
    );
    assert!(after.events.iter().any(|e| e.kind == "commerce.reconcile"
        && e.actor == "user:ops"
        && e.details.as_ref().is_some_and(|d| d["job_id"] == job)));
    assert!(!f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("no retry"));
    assert!(f
        .call(
            "POST",
            &format!("/v1/outbox/{job}/retry"),
            "retry-retired",
            "ops",
            json!({})
        )
        .await
        .0
        .is_client_error());
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    f.physical(&id, Status::Delivered).await;
    let next = f.queue(&id, "new-reviewed-step").await;
    assert_ne!(next["data"]["job"]["id"], job);
    assert_eq!(
        next["data"]["job"]["payload"]["body"]["expected_version"],
        7
    );
    assert_eq!(
        next["data"]["job"]["payload"]["body"]["status"],
        "delivered"
    );
    f.server.abort();
    let (status, replayed) = f
        .call("POST", &path, "review-original", "ops", body.clone())
        .await;
    assert_eq!(status, StatusCode::OK, "{replayed}");
    assert_eq!(replayed["data"], receipt["data"]);
    assert_eq!(replayed["replayed"], true);
    // Caller objects cannot impersonate the server's observation wrapper.
    assert_eq!(
        f.call(
            "POST",
            &path,
            "review-original",
            "ops",
            json!({"request":body})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.call("POST", "/v1/save-recovery/resolve", "", "ops", json!({"request_key":"review-original","path":format!("outbox/{job}/commerce-review"),"body":{"request":body},"note":"Different original object"})).await.0, StatusCode::CONFLICT);
    let mut changed = body;
    changed["note"] = json!("Different decision");
    assert_eq!(
        f.call("POST", &path, "review-original", "ops", changed)
            .await
            .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn external_review_refuses_stale_versions_permissions_manifests_and_unfenced_writes() {
    let f = Fixture::new().await;
    let (id, job) = f.advanced_handoff().await;
    let path = format!("/v1/outbox/{job}/commerce-review");
    let body = f.review_body(&job).await;
    for role in ["viewer", "integration", "other"] {
        let expected = if role == "other" {
            StatusCode::BAD_GATEWAY
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(
            f.call("GET", &path, "", role, Value::Null).await.0,
            expected
        );
        assert_eq!(
            f.call("POST", &path, "forbidden", role, body.clone())
                .await
                .0,
            expected
        );
    }
    for field in [
        "expected_load_version",
        "expected_total_attempts",
        "expected_shipment_version",
    ] {
        let mut stale = body.clone();
        stale[field] = json!(999);
        assert_eq!(
            f.call("POST", &path, field, "ops", stale).await.0,
            StatusCode::CONFLICT
        );
    }
    assert_eq!(
        f.call("POST", &path, "", "ops", body.clone()).await.0,
        StatusCode::BAD_REQUEST
    );
    for (field, value) in [
        ("shipment_id", json!(SHIPMENT)),
        ("note", json!("\n")),
        ("contact", json!("")),
        ("reference", json!("x".repeat(201))),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert_eq!(
            f.call("POST", &path, "invalid", "ops", invalid).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for (field, value) in [
        ("version", json!(3)),
        ("order_id", json!("00000000-0000-4000-8000-000000000099")),
        ("items", json!([])),
    ] {
        let saved = f.shipment.read().await.clone();
        f.shipment.write().await[field] = value;
        assert_eq!(
            f.call("GET", &path, "", "ops", Value::Null).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            f.call("POST", &path, "invalid-native", "ops", body.clone())
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        *f.shipment.write().await = saved;
    }
    // A retained preview cannot retire a job after another explicit attempt.
    f.store
        .update("demo", {
            let job = job.clone();
            move |w| {
                w.outbox
                    .iter_mut()
                    .find(|j| j.id == job)
                    .expect("job")
                    .total_attempts += 1;
                Ok(Value::Null)
            }
        })
        .await
        .expect("attempt changed");
    assert_eq!(
        f.call("POST", &path, "stale-attempt", "ops", body.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    let w = f.store.read("demo").await.expect("source");
    let mut changed = f.connection.clone();
    changed.commerce_tenant_id = Some("wrong-native-tenant".into());
    assert!(review_preview(&w, &job, &changed).await.is_err());
    let mut leased = w.clone();
    leased
        .outbox
        .iter_mut()
        .find(|j| j.id == job)
        .expect("job")
        .lease = Some("inflight".into());
    assert!(review_preview(&leased, &job, &f.connection).await.is_err());
    let mut wrong_job = w.clone();
    wrong_job
        .outbox
        .iter_mut()
        .find(|j| j.id == job)
        .expect("job")
        .destination = "stateset-sync-server".into();
    assert!(review_preview(&wrong_job, &job, &f.connection)
        .await
        .is_err());
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        w.loads.iter().find(|l| l.id == id).expect("load").status,
        Status::InTransit
    );
}
#[tokio::test]
async fn external_review_recovery_fences_late_requests_without_native_access() {
    let f = Fixture::new().await;
    let (_, job) = f.advanced_handoff().await;
    let body = f.review_body(&job).await;
    let path = format!("outbox/{job}/commerce-review");
    let resolution = json!({"request_key":"stopped-review","path":path,"body":body,"note":"Operator withdrew local review before commit"});
    assert_eq!(
        f.call(
            "POST",
            "/v1/save-recovery/resolve",
            "",
            "integration",
            resolution.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.call("POST", "/v1/save-recovery/resolve", "", "ops", resolution)
            .await
            .0,
        StatusCode::OK
    );
    f.server.abort();
    let (status, result) = f
        .call(
            "POST",
            &format!("/v1/{path}"),
            "stopped-review",
            "ops",
            body,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{result}");
    assert_eq!(result["error"]["code"], "request_stopped");
    assert_eq!(
        f.store
            .read("demo")
            .await
            .expect("still held")
            .outbox
            .iter()
            .find(|j| j.id == job)
            .expect("job")
            .status,
        "held"
    );
}

#[tokio::test]
async fn retiring_external_intent_preserves_source_holds_and_completed_resolution() {
    let f = Fixture::new().await;
    let (id, job) = f.advanced_handoff().await;
    f.order.write().await["status"] = json!("cancelled");
    f.integrations
        .refresh_source(&f.store, "demo", &id, "user:ops", None)
        .await
        .expect("observe changed source");
    let held = f.store.read("demo").await.expect("source held");
    assert!(held
        .source_check(&id)
        .expect("source check")
        .active_review_id
        .is_some());
    let body = f.review_body(&job).await;
    let path = format!("outbox/{job}/commerce-review");
    let (status, receipt) = f
        .call(
            "POST",
            &format!("/v1/{path}"),
            "review-held-source",
            "ops",
            body.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    f.server.abort();
    let (status,resolution)=f.call("POST","/v1/save-recovery/resolve","","ops",json!({"request_key":"review-held-source","path":path,"body":body,"note":"Recover original completed local review"})).await;
    assert_eq!(status, StatusCode::OK, "{resolution}");
    assert_eq!(resolution["data"]["outcome"], "completed");
    assert_eq!(resolution["data"]["result"], receipt["data"]);
    let after = f.store.read("demo").await.expect("source still held");
    assert!(after
        .guard_source(after.loads.iter().find(|l| l.id == id).expect("load"))
        .is_err());
    assert_eq!(after.source_reviews.len(), held.source_reviews.len());
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn durable_external_review_serializes_competing_decisions_and_preserves_originals() {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("commerce_review_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let f = Fixture::new_at(Some(url.as_str())).await;
    let (_, job) = f.advanced_handoff().await;
    let body = f.review_body(&job).await;
    let before = f.store.read("demo").await.expect("before");
    let original = before
        .outbox
        .iter()
        .find(|j| j.id == job)
        .expect("original");
    let second = Store::new(Some(url.as_str()))
        .await
        .expect("second process");
    let verified = json!({"request":body,"verified_shipment":f.shipment.read().await.clone(),"binding":binding(&f.connection).expect("binding")});
    let (a, b) = tokio::join!(
        f.store.mutate(
            "demo",
            "review-a",
            "commerce.reconcile",
            &job,
            verified.clone()
        ),
        second.mutate("demo", "review-b", "commerce.reconcile", &job, verified)
    );
    assert_ne!(a.is_ok(), b.is_ok(), "one competing decision commits");
    let (key, result) = if let Ok(result) = a {
        ("review-a", result)
    } else {
        ("review-b", b.expect("winner"))
    };
    let restarted = Store::new(Some(url.as_str())).await.expect("reconnect");
    let state = restarted.read("demo").await.expect("durable review");
    let reviewed = state.outbox.iter().find(|j| j.id == job).expect("reviewed");
    assert_eq!(reviewed.status, "external_reviewed");
    assert_eq!(reviewed.payload, original.payload);
    assert_eq!(reviewed.total_attempts, original.total_attempts);
    assert_eq!(reviewed.transmission, original.transmission);
    assert_eq!(
        serde_json::from_str::<Value>(reviewed.response.as_ref().expect("review evidence"))
            .expect("json")["original_patch_outcome"],
        "unknown"
    );
    let replay = restarted
        .replay("demo", key, "commerce.reconcile", &job, &body)
        .await
        .expect("replay")
        .expect("original receipt");
    assert_eq!(replay["data"], result["data"]);
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    drop(state);
    drop(before);
    drop(f);
    drop(second);
    drop(restarted);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned schema cleanup");
    admin.close().await;
}
#[tokio::test]
async fn native_import_is_tenant_scoped_exact_and_replayable_without_source() {
    let f = Fixture::new().await;
    let (status, page) = f
        .call("GET", "/v1/commerce/orders?page=1", "", "ops", Value::Null)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"][0]["sourceSystem"], "icommerce");
    for role in ["viewer", "other"] {
        let (status, _) = f
            .call("GET", "/v1/commerce/orders", "", role, Value::Null)
            .await;
        assert!(status.is_client_error() || status == StatusCode::BAD_GATEWAY);
    }
    let id = f.import().await;
    let w = f.store.read("demo").await.expect("workspace");
    let load = w.loads.iter().find(|l| l.id == id).expect("load");
    assert_eq!(load.sell_cents, 150000);
    assert_eq!(
        load.source_order.as_ref().expect("source")["totalAmount"],
        "19.99"
    );
    f.order.write().await["total_amount"] = json!("bad");
    let reads = f.reads.load(Ordering::SeqCst);
    let (status, replay) = f
        .call(
            "POST",
            "/v1/commerce/import",
            "native-import",
            "ops",
            json!({"order_id":ORDER,"load":freight_plan()}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["data"]["id"], id);
    assert_eq!(replay["replayed"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), reads);
    let mut spoof = json!({"order_id":ORDER,"load":freight_plan()});
    spoof["source_order"] = native_order();
    assert_eq!(
        f.call("POST", "/v1/commerce/import", "spoof", "ops", spoof)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}
#[tokio::test]
async fn native_single_step_handoff_preserves_manifest_and_never_routes_through_sync() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::Delivered).await;
    for (key, next) in [
        ("native-step-1", "in_transit"),
        ("native-step-2", "out_for_delivery"),
        ("native-step-3", "delivered"),
    ] {
        let queued = f.queue(&id, key).await;
        assert_eq!(queued["data"]["plan"]["next_status"], next);
        assert!(f
            .integrations
            .dispatch_one(&f.store, "demo")
            .await
            .expect("dispatch"));
        assert_eq!(f.shipment.read().await["status"], next);
    }
    let w = f.store.read("demo").await.expect("state");
    assert_eq!(w.outbox.iter().filter(|j| j.load_id == id).count(), 3);
    assert!(w
        .outbox
        .iter()
        .filter(|j| j.load_id == id)
        .all(|j| j.destination == "stateset-icommerce" && j.status == "synced"));
    assert_eq!(
        f.shipment.read().await["tracking_number"],
        "REAL-CARRIER-TRACKING"
    );
    assert_eq!(f.shipment.read().await["shipping_cost"], "15.00");
    assert_eq!(f.writes.load(Ordering::SeqCst), 3);
    let mut w = w.clone();
    let mut different: Shipment =
        serde_json::from_value(native_shipment()).expect("different native fixture");
    different.id = ShipmentId::new();
    assert!(guard_link(&w, &id, different.id).is_err());
    let request = json!({"expected_load_version":w.loads.iter().find(|l|l.id==id).expect("load").version,"shipment_id":different.id,"expected_shipment_version":different.version});
    assert!(w.apply("commerce.tracking",&id,json!({"request":request,"verified_shipment":different,"binding":binding(&f.connection).expect("binding")})).expect_err("original native binding").contains("retained native shipment identity"));
}
#[tokio::test]
async fn uncertain_patch_is_held_then_explicit_original_retry_observes_convergence() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::InTransit).await;
    let queued = f.queue(&id, "uncertain").await;
    let job = queued["data"]["job"]["id"].as_str().expect("job");
    f.fail.store(true, Ordering::SeqCst);
    assert!(f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("attempt"));
    let w = f.store.read("demo").await.expect("held");
    let held = w.outbox.iter().find(|j| j.id == job).expect("job");
    assert_eq!(held.status, "held");
    assert!(!held.last_error.as_ref().expect("error").contains("private"));
    let original = held.payload.clone();
    assert!(!f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("no automatic retry"));
    assert_eq!(
        f.call(
            "POST",
            &format!("/v1/outbox/{job}/retry"),
            "explicit-retry",
            "ops",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("original retry"));
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    let w = f.store.read("demo").await.expect("converged");
    let done = w.outbox.iter().find(|j| j.id == job).expect("job");
    assert_eq!(done.status, "synced");
    assert_eq!(done.payload, original);
    assert!(done
        .response
        .as_ref()
        .expect("response")
        .contains("receipt is unavailable"));
}
#[tokio::test]
async fn changed_manifest_version_connection_and_source_hold_prevent_writeback() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::InTransit).await;
    f.shipment.write().await["items"][0]["quantity"] = json!(1);
    assert_eq!(
        f.call(
            "GET",
            &format!("/v1/loads/{id}/commerce-tracking?shipment_id={SHIPMENT}"),
            "",
            "ops",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    *f.shipment.write().await = native_shipment();
    let queued = f.queue(&id, "freeze").await;
    let job: Outbox = serde_json::from_value(queued["data"]["job"].clone()).expect("job");
    let mut changed = f.connection.clone();
    changed.commerce_tenant_id = Some("different".into());
    assert!(send_tracking(&changed, &job)
        .await
        .expect_err("binding")
        .contains("retargeted"));
    f.shipment.write().await["version"] = json!(9);
    assert!(send_tracking(&f.connection, &job)
        .await
        .expect_err("version")
        .contains("rebased"));
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    f.order.write().await["status"] = json!("cancelled");
    assert!(!f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("source hold"));
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    assert!(f
        .store
        .read("demo")
        .await
        .expect("source review")
        .source_check(&id)
        .expect("check")
        .active_review_id
        .is_some());
}
#[test]
fn native_origin_and_money_guards_reject_unchecked_boundaries() {
    for origin in [
        "http://commerce.example.com",
        "https://user:secret@example.com",
        "https://example.com/api",
        "https://example.com/?tenant=wrong",
    ] {
        assert!(validate_origin(origin).is_err());
    }
    assert!(validate_origin("http://127.0.0.1:8000").is_ok());
    assert!(validate_origin("https://commerce.example.com").is_ok());
}

fn shipment_page(rows: Vec<Value>, offset: u32, more: bool) -> Value {
    json!({"shipments":rows,"total":27,"limit":25,"offset":offset,"has_more":more})
}
#[tokio::test]
async fn discovery_is_read_only_order_scoped_and_explains_native_eligibility_and_active_handoffs() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::Delivered).await;
    let mut partial = native_shipment();
    partial["id"] = json!(ShipmentId::new());
    partial["items"][0]["quantity"] = json!(1);
    let mut packing = native_shipment();
    packing["id"] = json!(ShipmentId::new());
    packing["status"] = json!("pending");
    let mut current = native_shipment();
    current["id"] = json!(ShipmentId::new());
    current["status"] = json!("delivered");
    *f.shipment_page.write().await = Some(shipment_page(
        vec![native_shipment(), partial, packing, current],
        0,
        false,
    ));
    let before = f.store.read("demo").await.expect("before read");
    let (status, page) = f
        .call(
            "GET",
            &format!("/v1/loads/{id}/commerce-shipments"),
            "",
            "ops",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let data = &page["data"];
    assert_eq!(data["order_id"], ORDER);
    assert_eq!(data["load_id"], id);
    assert_eq!(data["items"][0]["state"], "ready_for_review");
    assert!(data["items"][0]["shipment"]
        .get("tracking_number")
        .is_none());
    assert!(data["items"][0]["shipment"].get("shipping_cost").is_none());
    assert_eq!(data["items"][0]["plan"]["next_status"], "in_transit");
    assert_eq!(data["items"][1]["state"], "blocked");
    assert!(data["items"][1]["reason"]
        .as_str()
        .expect("reason")
        .contains("partial"));
    assert_eq!(data["items"][2]["state"], "blocked");
    assert_eq!(data["items"][3]["state"], "up_to_date");
    assert_eq!(
        f.shipment_queries.read().await[0],
        BTreeMap::from([
            ("order_id".into(), ORDER.into()),
            ("limit".into(), "25".into()),
            ("offset".into(), "0".into())
        ])
    );
    assert_eq!(
        serde_json::to_value(&before).expect("before"),
        serde_json::to_value(f.store.read("demo").await.expect("after")).expect("after")
    );
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    let queued = f.queue(&id, "selected-native").await;
    let job = &queued["data"]["job"]["id"];
    let (status, page) = f
        .call(
            "GET",
            &format!("/v1/loads/{id}/commerce-shipments"),
            "",
            "ops",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["data"]["active_handoffs"][0]["id"], *job);
    assert!(page["data"]["items"]
        .as_array()
        .expect("rows")
        .iter()
        .all(|row| row["state"] == "blocked"));
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn discovery_refuses_foreign_malformed_pages_and_caller_filters_before_exposing_records() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::InTransit).await;
    let path = format!("/v1/loads/{id}/commerce-shipments");
    for role in ["viewer", "integration", "other"] {
        let (status, _) = f.call("GET", &path, "", role, Value::Null).await;
        assert_eq!(
            status,
            if role == "other" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    for query in [
        "page=0",
        "page=-1",
        "page=4294967295",
        "page=x",
        "page=1&page=2",
        "order_id=other",
        "tenant=other",
        "url=https://invalid.test",
        "offset=25",
    ] {
        assert_eq!(
            f.call("GET", &format!("{path}?{query}"), "", "ops", Value::Null)
                .await
                .0,
            StatusCode::BAD_REQUEST,
            "{query}"
        );
    }
    let ordinary = f.store.read("demo").await.expect("workspace").loads[0]
        .id
        .clone();
    assert_eq!(
        f.call(
            "GET",
            &format!("/v1/loads/{ordinary}/commerce-shipments"),
            "",
            "ops",
            Value::Null
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(f.shipment_queries.read().await.is_empty());
    let good = shipment_page(vec![native_shipment()], 0, false);
    let mut foreign = good.clone();
    foreign["shipments"][0]["order_id"] = json!(OrderId::new());
    foreign["shipments"][0]["shipment_number"] = json!("PRIVATE-CROSS-ORDER-CANARY");
    let mut duplicate = good.clone();
    duplicate["shipments"] = json!([native_shipment(), native_shipment()]);
    let mut oversized = good.clone();
    oversized["shipments"][0]["shipment_number"] = json!("PRIVATE-OVERSIZED-CANARY".repeat(100000));
    let mut absent = good.clone();
    absent.as_object_mut().expect("object").remove("has_more");
    let mut wrong_limit = good.clone();
    wrong_limit["limit"] = json!(100);
    let mut wrong_offset = good.clone();
    wrong_offset["offset"] = json!(25);
    let mut incomplete = good.clone();
    incomplete["has_more"] = json!(true);
    let mut controls = good.clone();
    controls["shipments"][0]["status"] = json!("shipped\n");
    let mut invalid_version = good.clone();
    invalid_version["shipments"][0]["version"] = json!(-1);
    let too_many = shipment_page(vec![native_shipment(); 26], 0, false);
    for invalid in [
        foreign,
        duplicate,
        oversized,
        absent,
        wrong_limit,
        wrong_offset,
        incomplete,
        controls,
        invalid_version,
        too_many,
    ] {
        *f.shipment_page.write().await = Some(invalid);
        let (status, body) = f.call("GET", &path, "", "ops", Value::Null).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
        assert!(!body.to_string().contains("PRIVATE-"));
        assert!(body.get("data").is_none());
    }
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn discovery_pages_are_bounded_and_selection_cannot_reuse_an_old_native_version() {
    let f = Fixture::new().await;
    let id = f.import().await;
    f.physical(&id, Status::Delivered).await;
    let mut rows = vec![native_shipment()];
    for n in 1..25 {
        let mut s = native_shipment();
        s["id"] = json!(format!("00000000-0000-4000-8000-{:012x}", 500 + n));
        rows.push(s);
    }
    *f.shipment_page.write().await = Some(shipment_page(rows, 0, true));
    let (status, listed) = f
        .call(
            "GET",
            &format!("/v1/loads/{id}/commerce-shipments?page=1"),
            "",
            "ops",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["data"]["items"].as_array().expect("page").len(), 25);
    assert_eq!(listed["data"]["has_more"], true);
    f.shipment.write().await["version"] = json!(4);
    f.shipment.write().await["status"] = json!("in_transit");
    let (status, fresh) = f
        .call(
            "GET",
            &format!("/v1/loads/{id}/commerce-tracking?shipment_id={SHIPMENT}"),
            "",
            "ops",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    assert_eq!(fresh["data"]["shipment"]["version"], 4);
    assert_eq!(fresh["data"]["plan"]["next_status"], "out_for_delivery");
    let stale = json!({"expected_load_version":listed["data"]["expected_load_version"],"shipment_id":SHIPMENT,"expected_shipment_version":3});
    assert_eq!(
        f.call(
            "POST",
            &format!("/v1/loads/{id}/commerce-tracking"),
            "stale-list-selection",
            "ops",
            stale
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
    let queued = f.queue(&id, "fresh-selected-step").await;
    assert_eq!(
        queued["data"]["job"]["payload"]["body"],
        json!({"expected_version":4,"status":"out_for_delivery"})
    );
    *f.shipment_page.write().await = Some(shipment_page(vec![native_shipment()], 25, false));
    let (status, page) = f
        .call(
            "GET",
            &format!("/v1/loads/{id}/commerce-shipments?page=2"),
            "",
            "ops",
            Value::Null,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["data"]["page"], 2);
    assert_eq!(page["data"]["has_more"], false);
    assert_eq!(
        f.shipment_queries.read().await.last().expect("query")["offset"],
        "25"
    );
    assert_eq!(f.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn durable_native_queue_replays_and_recovers_original_intent_after_reconnect() {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned disposable database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("commerce_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let f = Fixture::new_at(Some(url.as_str())).await;
    let id = f.import().await;
    f.physical(&id, Status::InTransit).await;
    let queued = f.queue(&id, "durable-native").await;
    let job = queued["data"]["job"]["id"].as_str().expect("job");
    f.fail.store(true, Ordering::SeqCst);
    assert!(f
        .integrations
        .dispatch_one(&f.store, "demo")
        .await
        .expect("uncertain native PATCH"));
    let restarted = Store::new(Some(url.as_str())).await.expect("reconnect");
    let second = Store::new(Some(url.as_str()))
        .await
        .expect("independent worker");
    let saved = restarted.read("demo").await.expect("saved");
    let original = saved
        .outbox
        .iter()
        .find(|v| v.id == job)
        .expect("native job");
    assert_eq!(original.status, "held");
    let request = json!({"expected_load_version":saved.loads.iter().find(|v|v.id==id).expect("load").version,"shipment_id":SHIPMENT,"expected_shipment_version":3});
    let replay = restarted
        .replay("demo", "durable-native", "commerce.tracking", &id, &request)
        .await
        .expect("receipt")
        .expect("original receipt");
    assert_eq!(replay["data"], queued["data"]);
    restarted
        .mutate("demo", "retry-durable", "outbox.retry", job, json!({}))
        .await
        .expect("explicit retry");
    let (a, b) = tokio::join!(
        f.integrations.dispatch_one(&restarted, "demo"),
        f.integrations.dispatch_one(&second, "demo")
    );
    assert_ne!(a.expect("worker a"), b.expect("worker b"));
    assert_eq!(f.writes.load(Ordering::SeqCst), 1);
    let final_state = second.read("demo").await.expect("final");
    let final_job = final_state
        .outbox
        .iter()
        .find(|v| v.id == job)
        .expect("native job");
    assert_eq!(final_job.status, "synced");
    assert_eq!(final_job.payload, original.payload);
    drop(final_state);
    drop(saved);
    drop(f);
    drop(second);
    drop(restarted);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("clean owned schema");
    admin.close().await;
}
