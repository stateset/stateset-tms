#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Status, Workspace},
    integrations::Integrations,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "staff:operations"),
        &domain::id("key"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn version(s: &Store, id: &str) -> u64 {
    read(s)
        .await
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version
}
fn router(s: Store) -> Router {
    let keys = [
        ("carrier", Role::Carrier, Some("ca4")),
        ("second", Role::Carrier, Some("ca4")),
        ("other", Role::Carrier, Some("ca1")),
        ("customer", Role::Customer, Some("c1")),
        ("operations", Role::Operations, None),
        ("finance", Role::Finance, None),
        ("viewer", Role::Viewer, None),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("actor:{key}"),
                role,
                counterparty_id: cp.map(str::to_owned),
            }),
        )
    })
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store: s,
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(
    r: &Router,
    who: &str,
    method: &str,
    path: &str,
    body: Value,
    key: &str,
) -> (u16, Value) {
    let r = r
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {who}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .header("x-freight-role", "admin")
                .header("x-tenant-id", "other")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = r.status().as_u16();
    let bytes = r.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}
async fn setup(disposition: &str) -> (Store, Router, String, String) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let date = (Utc::now() + Duration::days(1)).date_naive();
    let l=op(&s,"load.create","",json!({"reference":domain::id("NATIVE-RELEASE"),"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Actual freight","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":date,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":date,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":125001,"sell_cents":150001})).await;
    let lid = l["id"].as_str().expect("load").to_owned();
    op(
        &s,
        "load.command",
        &lid,
        json!({"action":"tender","expected_version":1,"carrier_id":"ca4"}),
    )
    .await;
    op(
        &s,
        "load.command",
        &lid,
        json!({"action":"accept","expected_version":version(&s,&lid).await}),
    )
    .await;
    let release=op(&s,"tender.release.request",&lid,json!({"expected_load_version":version(&s,&lid).await,"disposition":disposition,"reason":"PRIVATE-STAFF-RELEASE-REASON"})).await;
    let rid = release["id"].as_str().expect("release").to_owned();
    (s.clone(), router(s), lid, rid)
}
async fn publish(s: &Store, lid: &str, rid: &str, previous: Option<&str>) -> Value {
    op(s,"portal.release.publish",rid,json!({"expected_load_version":version(s,lid).await,"expected_previous_id":previous,"message":"Actual request for release of this carrier assignment <script>escaped</script>","respond_by":Utc::now()+Duration::hours(24)})).await
}
async fn response(s: &Store, lid: &str, p: &Value, decision: &str) -> Value {
    let mut v = json!({"expected_load_version":version(s,lid).await,"document_sha256":p["sha256"],"decision":decision,"contact":"Actual authorized carrier dispatcher","reference":"Native carrier release decision","note":"Carrier checked driver and confirmed this exact assignment's custody"});
    if decision == "release" {
        v["no_custody"] = json!(true);
    }
    v
}
#[tokio::test]
async fn native_release_is_exact_scoped_once_and_cancels_only_the_published_assignment_without_financial_effects(
) {
    let (s, r, lid, rid) = setup("cancel").await;
    assert_eq!(
        call(&r, "carrier", "GET", "portal", json!({}), "private")
            .await
            .1["release_requests"],
        json!([])
    );
    let p = publish(&s, &lid, &rid, None).await;
    let pid = p["id"].as_str().expect("publication");
    let path = format!("portal/releases/{pid}/response");
    let doc = format!("portal/releases/{pid}/statement");
    let view = call(&r, "carrier", "GET", "portal", json!({}), "view")
        .await
        .1;
    for secret in [
        "PRIVATE-STAFF",
        "125001",
        "150001",
        "customer_id",
        "requested_by",
        "source_order",
    ] {
        assert!(!view.to_string().contains(secret), "{secret}");
    }
    let original = call(&r, "carrier", "GET", &doc, json!({}), "document").await;
    assert_eq!(original.0, 200);
    assert_eq!(
        stateset_freight::documents::sha256(original.1.as_str().expect("html").as_bytes()),
        p["sha256"]
    );
    assert!(original
        .1
        .as_str()
        .expect("html")
        .contains("&lt;script&gt;"));
    for who in ["other", "customer"] {
        assert_eq!(
            call(&r, who, "GET", &doc, json!({}), "wrong-party").await.0,
            404
        );
    }
    let good = response(&s, &lid, &p, "release").await;
    let before = json!(read(&s).await);
    for who in ["other", "customer", "operations", "finance"] {
        assert_eq!(
            call(&r, who, "POST", &path, good.clone(), "deny").await.0,
            if who == "other" { 404 } else { 403 }
        );
    }
    for (field, value) in [
        ("carrier_id", json!("ca1")),
        ("subject", json!("forged")),
        ("document_sha256", json!("changed")),
        ("no_custody", json!(false)),
    ] {
        let mut forged = good.clone();
        forged[field] = value;
        assert_eq!(
            call(&r, "carrier", "POST", &path, forged, "forged").await.0,
            if field == "document_sha256" { 409 } else { 422 }
        );
    }
    assert_eq!(json!(read(&s).await), before);
    let result = call(&r, "carrier", "POST", &path, good.clone(), "response-once").await;
    assert_eq!(result.0, 200, "{result:?}");
    assert_eq!(result.1["data"]["recorded_by"], "actor:carrier");
    assert_eq!(result.1["data"]["outcome"], "applied");
    let after = json!(read(&s).await);
    assert_eq!(
        call(&r, "carrier", "POST", &path, good.clone(), "response-once")
            .await
            .1["replayed"],
        true
    );
    assert_eq!(
        call(&r, "second", "POST", &path, good, "response-once")
            .await
            .0,
        409
    );
    assert_eq!(json!(read(&s).await), after);
    let w = read(&s).await;
    let l = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!(l.status, Status::Cancelled);
    assert!(l.carrier_id.is_none());
    assert!(w.journal.is_empty());
    let proof = &w.release_responses[0];
    assert!(proof.no_custody);
    let release = w.releases.iter().find(|v| v.id == rid).expect("release");
    assert_eq!(
        release.confirmation.as_ref().expect("proof").recorded_by,
        "actor:carrier"
    );
    assert_eq!(
        release.confirmation.as_ref().expect("proof").recorded_at,
        proof.recorded_at
    );
    assert_eq!(
        call(&r, "carrier", "GET", &doc, json!({}), "original-document")
            .await
            .1,
        original.1
    );
}
#[tokio::test]
async fn superseded_release_request_and_expired_authorization_cannot_release_but_actual_retention_remains_available(
) {
    let (s, r, lid, rid) = setup("replan").await;
    let p = publish(&s, &lid, &rid, None).await;
    let pid = p["id"].as_str().expect("publication");
    let next=op(&s,"portal.release.publish",&rid,json!({"expected_load_version":version(&s,&lid).await,"expected_previous_id":pid,"message":"Actual updated request with a short explicit deadline","respond_by":Utc::now()+Duration::seconds(1)})).await;
    let nid = next["id"].as_str().expect("new pub");
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &format!("portal/releases/{pid}/response"),
            response(&s, &lid, &p, "release").await,
            "old-publication"
        )
        .await
        .0,
        409
    );
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &format!("portal/releases/{nid}/response"),
            response(&s, &lid, &next, "release").await,
            "expired"
        )
        .await
        .0,
        422
    );
    let retained = call(
        &r,
        "carrier",
        "POST",
        &format!("portal/releases/{nid}/response"),
        response(&s, &lid, &next, "retain").await,
        "retain",
    )
    .await;
    assert_eq!(retained.0, 200, "{retained:?}");
    let w = read(&s).await;
    assert_eq!(
        w.loads.iter().find(|l| l.id == lid).expect("load").status,
        Status::Accepted
    );
    assert_eq!(
        w.releases
            .iter()
            .find(|v| v.id == rid)
            .expect("release")
            .status,
        "retained"
    );
    assert!(!w.release_responses[0].no_custody);
    assert_eq!(
        w.release_portal_views("ca4").expect("view")[0]["load_version"],
        Value::Null
    );
    assert!(w.journal.is_empty());
}
#[tokio::test]
async fn actual_pickup_blocks_native_release_and_retains_the_publication_without_erasing_movement()
{
    let (s, r, lid, rid) = setup("replan").await;
    let p = publish(&s, &lid, &rid, None).await;
    let body = response(&s, &lid, &p, "release").await;
    let w = read(&s).await;
    let t = w.tenders.iter().find(|t| t.load_id == lid).expect("tender");
    op(&s,"edi.event","",json!({"event_id":"actual-pickup-during-native-release","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":t.shipment_reference,"partner_id":t.carrier.partner_id,"carrier_scac":t.carrier.scac,"latest_status":"AF"}})).await;
    let before = json!(read(&s).await);
    let path = format!(
        "portal/releases/{}/response",
        p["id"].as_str().expect("pub")
    );
    assert_eq!(
        call(&r, "carrier", "POST", &path, body, "stale-response")
            .await
            .0,
        409
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            response(&s, &lid, &p, "release").await,
            "no-custody-contradiction"
        )
        .await
        .0,
        409
    );
    assert_eq!(json!(read(&s).await), before);
    let w = read(&s).await;
    assert_eq!(
        w.loads.iter().find(|l| l.id == lid).expect("load").status,
        Status::InTransit
    );
    assert_eq!(
        w.releases
            .iter()
            .find(|r| r.id == rid)
            .expect("release")
            .status,
        "blocked_by_movement"
    );
    assert!(w.release_responses.is_empty());
    assert!(w
        .tender_incidents
        .iter()
        .any(|i| i.load_id == lid && i.status == "open"));
}

#[tokio::test]
async fn native_acknowledgement_during_live_handoff_is_retained_and_operations_applies_the_original_proof_after_completion(
) {
    use axum::{routing::post, Json};
    use stateset_freight::integrations::Connection;
    use std::collections::BTreeMap;
    let (s, r, lid, initial) = setup("replan").await;
    op(&s,"tender.release.retain",&initial,json!({"expected_load_version":version(&s,&lid).await,"carrier_contact":"Actual initial carrier contact","confirmation_reference":"Actual initial retention","note":"Carrier retained original assignment before live handoff test"})).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let (e, f) = (entered.clone(), finish.clone());
    let upstream = Router::new().route(
        "/v1/edi/outbound/204/brex",
        post(move || {
            let (e, f) = (e.clone(), f.clone());
            async move {
                e.notify_one();
                f.notified().await;
                Json(json!({"owned":"actual in-flight synthetic HTTP acknowledgement"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.expect("serve") });
    let integrations = Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(format!("http://{address}")),
                edi_token: Some("owned-fixture-token".into()),
                edi_tenant_id: Some("demo".into()),
                ..Connection::default()
            },
        )]),
    };
    let ss = s.clone();
    let worker = tokio::spawn(async move { integrations.dispatch_one(&ss, "demo").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("actual HTTP request entered fixture");
    let release=op(&s,"tender.release.request",&lid,json!({"expected_load_version":version(&s,&lid).await,"disposition":"replan","reason":"PRIVATE-LEASE-RELEASE-REQUEST"})).await;
    let rid = release["id"].as_str().expect("release");
    let p = publish(&s, &lid, rid, None).await;
    let result = call(
        &r,
        "carrier",
        "POST",
        &format!(
            "portal/releases/{}/response",
            p["id"].as_str().expect("pub")
        ),
        response(&s, &lid, &p, "release").await,
        "native-during-lease",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    let proof = &result.1["data"];
    assert_eq!(proof["outcome"], "awaiting_operations");
    assert_eq!(proof["initial_hold"], "handoff_in_progress");
    let responseid = proof["id"].as_str().expect("response");
    let w = read(&s).await;
    let source = w.release_responses[0].clone();
    assert!(source.application.is_none());
    assert!(w
        .loads
        .iter()
        .find(|l| l.id == lid)
        .expect("load")
        .carrier_id
        .is_some());
    let applypath = format!("release-responses/{responseid}/apply");
    let denied = call(
        &r,
        "operations",
        "POST",
        &applypath,
        json!({"expected_load_version":version(&s,&lid).await}),
        "apply-live",
    )
    .await;
    assert_eq!(denied.0, 422, "{denied:?}");
    assert!(read(&s).await.release_responses[0].application.is_none());
    finish.notify_one();
    worker
        .await
        .expect("worker")
        .expect("actual result persisted");
    let applied = call(
        &r,
        "operations",
        "POST",
        &applypath,
        json!({"expected_load_version":version(&s,&lid).await}),
        "apply-after-handoff",
    )
    .await;
    assert_eq!(applied.0, 200, "{applied:?}");
    let w = read(&s).await;
    let l = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!(l.status, Status::Planned);
    assert!(l.carrier_id.is_none());
    let response = &w.release_responses[0];
    assert_eq!(response.evidence_sha256, source.evidence_sha256);
    assert_eq!(response.recorded_at, source.recorded_at);
    assert_eq!(response.recorded_by, "actor:carrier");
    let app = response.application.as_ref().expect("applied");
    assert_eq!(app.applied_by, "actor:operations");
    assert_eq!(app.mode, "operations");
    let confirmation = w
        .releases
        .iter()
        .find(|r| r.id == rid)
        .expect("release")
        .confirmation
        .as_ref()
        .expect("confirmation");
    assert_eq!(confirmation.recorded_by, "actor:carrier");
    assert_eq!(confirmation.recorded_at, source.recorded_at);
    assert!(w.journal.is_empty());
    let view = call(&r, "carrier", "GET", "portal", json!({}), "history")
        .await
        .1;
    assert!(!view.to_string().contains("PRIVATE-LEASE"));
    assert!(!view.to_string().contains("applied_by"));
    assert!(view["release_requests"][0]["load_version"].is_null());
    server.abort();
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_native_release_serializes_decisions_and_pickup_and_checks_immutable_pending_proof_before_application(
) {
    let (memory, _, lid, rid) = setup("replan").await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let fixture = read(&memory).await;
    for mode in ["decision", "pending", "movement"] {
        let tenant = domain::id("native-release-pg");
        let mut data = json!(fixture);
        data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
        if mode == "pending" {
            let j = &mut data["outbox"][0];
            j["status"] = json!("sending");
            j["lease"] = json!("owned-inflight-lease");
            j["leased_until"] = json!(Utc::now() + Duration::minutes(2));
            j["attempts"] = json!(1);
            j["total_attempts"] = json!(1);
            j["transmission"] = json!("attempted");
        }
        canonical::put_workspace(&pool, &tenant, &data)
            .await
            .expect("owned tenant");
        let body = json!({"expected_load_version":fixture.loads.iter().find(|l|l.id==lid).expect("load").version,"message":"Exact authenticated carrier release request","respond_by":Utc::now()+Duration::hours(24)});
        let (left, right) = tokio::join!(
            a.mutate_authorized(
                (&tenant, "staff:a"),
                "publish-a",
                "portal.release.publish",
                &rid,
                body.clone()
            ),
            b.mutate_authorized(
                (&tenant, "staff:b"),
                "publish-b",
                "portal.release.publish",
                &rid,
                body
            )
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let w = a.read(&tenant).await.expect("read");
        let p = w.release_publications[0].clone();
        let load = w.loads.iter().find(|l| l.id == lid).expect("load");
        let native = json!({"carrier_id":"ca4","subject":"actor:carrier","request":{"expected_load_version":load.version,"document_sha256":p.sha256,"decision":"release","no_custody":true,"contact":"Actual carrier dispatcher","reference":"Actual authenticated no-custody release","note":"Carrier verified driver and confirms no pickup/custody for this exact assignment"}});
        if mode == "decision" {
            canonical::set(
                &pool,
                &tenant,
                &["release_publications", "0", "html"],
                json!("corrupt original release request"),
            )
            .await
            .expect("corrupt owned bytes");
            let before = json!(a.read(&tenant).await.expect("corrupt"));
            assert!(a
                .mutate_authorized(
                    (&tenant, "actor:carrier"),
                    "native-a",
                    "portal.release.respond",
                    &p.id,
                    native.clone()
                )
                .await
                .is_err());
            assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
            canonical::set(
                &pool,
                &tenant,
                &["release_publications", "0", "html"],
                json!(p.html),
            )
            .await
            .expect("restore exact document");
            let mut retained = native.clone();
            retained["request"]["decision"] = json!("retain");
            retained["request"]
                .as_object_mut()
                .expect("request")
                .remove("no_custody");
            let (left, right) = tokio::join!(
                a.mutate_authorized(
                    (&tenant, "actor:carrier"),
                    "native-a",
                    "portal.release.respond",
                    &p.id,
                    native
                ),
                b.mutate_authorized(
                    (&tenant, "actor:carrier"),
                    "native-b",
                    "portal.release.respond",
                    &p.id,
                    retained
                )
            );
            assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
            let reopen = Store::new(Some(&url)).await.expect("reconnect");
            let w = reopen.read(&tenant).await.expect("retained");
            assert_eq!(w.release_responses.len(), 1);
            assert!(w.release_responses[0].application.is_some());
            assert_eq!(w.release_publications[0].html, p.html);
            assert_eq!(w.release_responses[0].recorded_by, "actor:carrier");
            assert!(w.journal.is_empty());
        } else if mode == "pending" {
            let result = a
                .mutate_authorized(
                    (&tenant, "actor:carrier"),
                    "native-a",
                    "portal.release.respond",
                    &p.id,
                    native,
                )
                .await
                .expect("acknowledged");
            assert_eq!(result["data"]["outcome"], "awaiting_operations");
            let saved = a.read(&tenant).await.expect("saved");
            let proof = saved.release_responses[0].clone();
            let version = saved
                .loads
                .iter()
                .find(|l| l.id == lid)
                .expect("load")
                .version;
            canonical::set(
                &pool,
                &tenant,
                &["release_responses", "0", "note"],
                json!("corrupted retained no-custody evidence"),
            )
            .await
            .expect("corrupt owned proof");
            let before = json!(a.read(&tenant).await.expect("before"));
            assert!(a
                .mutate_authorized(
                    (&tenant, "staff:operations"),
                    "apply",
                    "portal.release.apply",
                    &proof.id,
                    json!({"expected_load_version":version})
                )
                .await
                .is_err());
            assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
            let mut restored = json!(saved);
            restored["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
            restored["outbox"][0]["leased_until"] = json!(Utc::now() - Duration::seconds(1));
            canonical::put_workspace(&pool, &tenant, &restored)
                .await
                .expect("restore proof and expire owned lease");
            a.mutate_authorized(
                (&tenant, "staff:operations"),
                "apply",
                "portal.release.apply",
                &proof.id,
                json!({"expected_load_version":version}),
            )
            .await
            .expect("apply original proof after recovery");
            let w = a.read(&tenant).await.expect("applied");
            let result = &w.release_responses[0];
            assert_eq!(result.evidence_sha256, proof.evidence_sha256);
            assert_eq!(result.recorded_at, proof.recorded_at);
            assert_eq!(
                result.application.as_ref().expect("application").applied_by,
                "staff:operations"
            );
            assert_eq!(
                w.releases
                    .iter()
                    .find(|r| r.id == rid)
                    .expect("release")
                    .confirmation
                    .as_ref()
                    .expect("confirmation")
                    .recorded_by,
                "actor:carrier"
            );
            assert_eq!(w.outbox[0].total_attempts, 1);
            assert!(w.outbox[0].lease.is_none());
            assert!(w
                .loads
                .iter()
                .find(|l| l.id == lid)
                .expect("load")
                .carrier_id
                .is_none());
        } else {
            let t = w
                .tenders
                .iter()
                .find(|t| t.id == p.tender_id)
                .expect("tender");
            let pickup = json!({"event_id":"concurrent-real-pickup","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":t.shipment_reference,"partner_id":t.carrier.partner_id,"carrier_scac":t.carrier.scac,"latest_status":"AF"}});
            let (release, pickup) = tokio::join!(
                a.mutate_authorized(
                    (&tenant, "actor:carrier"),
                    "native-a",
                    "portal.release.respond",
                    &p.id,
                    native
                ),
                b.mutate_authorized(
                    (&tenant, "integration:carrier"),
                    "pickup",
                    "edi.event",
                    "",
                    pickup
                )
            );
            assert!(pickup.is_ok());
            let w = a.read(&tenant).await.expect("state");
            let l = w.loads.iter().find(|l| l.id == lid).expect("load");
            assert!(w
                .tender_incidents
                .iter()
                .any(|i| i.load_id == lid && i.status == "open"));
            if release.is_ok() {
                assert_eq!(l.status, Status::Planned);
                assert_eq!(w.release_responses.len(), 1);
            } else {
                assert_eq!(l.status, Status::InTransit);
                assert!(w.release_responses.is_empty());
                assert_eq!(
                    w.releases
                        .iter()
                        .find(|r| r.id == rid)
                        .expect("release")
                        .status,
                    "blocked_by_movement"
                );
            }
        }
        assert!(a
            .read("other-native-release-tenant")
            .await
            .expect("other")
            .release_publications
            .is_empty());
        sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .expect("cleanup owned tenant");
    }
    pool.close().await;
}

#[tokio::test]
async fn financial_records_arriving_after_publication_retain_carrier_evidence_without_clearing_the_assignment(
) {
    let (s, r, lid, rid) = setup("cancel").await;
    let p = publish(&s, &lid, &rid, None).await;
    op(&s,"invoice.create","",json!({"load_id":lid,"number":"ACTUAL-PRE-PICKUP-CARRIER-INVOICE","amount_cents":125001,"currency":"USD"})).await;
    let result = call(
        &r,
        "carrier",
        "POST",
        &format!(
            "portal/releases/{}/response",
            p["id"].as_str().expect("pub")
        ),
        response(&s, &lid, &p, "release").await,
        "financial-hold-ack",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    assert_eq!(result.1["data"]["outcome"], "awaiting_operations");
    assert_eq!(result.1["data"]["initial_hold"], "financial_review");
    let w = read(&s).await;
    let l = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!(l.status, Status::Accepted);
    assert!(l.carrier_id.is_some());
    assert!(w
        .releases
        .iter()
        .find(|r| r.id == rid)
        .expect("release")
        .confirmation
        .is_none());
    assert!(w.journal.is_empty());
    let response = &w.release_responses[0];
    assert_eq!(
        call(
            &r,
            "operations",
            "POST",
            &format!("release-responses/{}/apply", response.id),
            json!({"expected_load_version":l.version}),
            "financial-hold-apply"
        )
        .await
        .0,
        422
    );
    let view = call(&r, "carrier", "GET", "portal", json!({}), "pending-view")
        .await
        .1;
    assert_eq!(view["release_requests"][0]["application_pending"], true);
    assert!(!view.to_string().contains("125001"));
    assert!(!view
        .to_string()
        .contains("ACTUAL-PRE-PICKUP-CARRIER-INVOICE"));
}
