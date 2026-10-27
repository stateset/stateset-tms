#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Workspace},
    integrations::Integrations,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "staff:quotes"),
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
        .quotes
        .iter()
        .find(|q| q.id == id)
        .expect("quote")
        .version
}
fn router(s: Store) -> Router {
    router_with(s, Integrations::default())
}
fn router_with(s: Store, integrations: Integrations) -> Router {
    let keys = [
        ("customer", Role::Customer, Some("c1")),
        ("second", Role::Customer, Some("c1")),
        ("other", Role::Customer, Some("c3")),
        ("carrier", Role::Carrier, Some("ca4")),
        ("staff", Role::Admin, None),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("native:{key}"),
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
        integrations: Arc::new(integrations),
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
fn draft(reference: &str) -> Value {
    let day = (Utc::now() + Duration::days(2)).date_naive();
    json!({"reference":reference,"plan":{"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Actual customer shipment","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"}},"sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Exact public inclusive transportation terms <script>escaped</script>"})
}
async fn issue(s: &Store, id: &str) {
    let q=op(s,"quote.offer",id,json!({"expected_version":version(s,id).await,"carrier_id":"ca4","amount_cents":125001,"valid_until":Utc::now()+Duration::hours(24),"contact":"PRIVATE-CARRIER-CONTACT","reference":"PRIVATE-CARRIER-PROOF","note":"PRIVATE-PROCUREMENT-NOTE"})).await;
    op(s,"quote.issue",id,json!({"expected_version":q["version"],"offer_id":q["revisions"].as_array().expect("revisions").last().expect("current")["offers"][0]["id"]})).await;
}
async fn setup() -> (Store, Router, String) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let q = op(&s, "quote.create", "", draft(&domain::id("CUSTOMER-QUOTE"))).await;
    let id = q["id"].as_str().expect("id").to_owned();
    issue(&s, &id).await;
    (s.clone(), router(s), id)
}
async fn publish(s: &Store, id: &str, prior: Option<&str>) -> Value {
    op(
        s,
        "portal.quote.publish",
        id,
        json!({"expected_version":version(s,id).await,"expected_previous_id":prior}),
    )
    .await
}
async fn response(s: &Store, id: &str, p: &Value, decision: &str) -> Value {
    json!({"expected_version":version(s,id).await,"revision":p["revision"],"document_sha256":p["sha256"],"decision":decision,"contact":"Actual customer transport lead","reference":format!("Native customer {decision}"),"note":"Customer reviewed the exact published plan, price, terms and deadline"})
}
#[tokio::test]
async fn published_customer_quote_is_private_exact_immutable_and_native_acceptance_does_not_book_or_leak_procurement(
) {
    let (s, r, id) = setup().await;
    assert_eq!(
        call(&r, "customer", "GET", "portal", json!({}), "read")
            .await
            .1["quotes"],
        json!([])
    );
    let p = publish(&s, &id, None).await;
    let pid = p["id"].as_str().expect("pub");
    let path = format!("portal/customer-quotes/{pid}/response");
    let doc = format!("portal/customer-quotes/{pid}/statement");
    let view = call(&r, "customer", "GET", "portal", json!({}), "view")
        .await
        .1;
    assert_eq!(view["quotes"][0]["snapshot"]["amount_cents"], 150001);
    for secret in [
        "125001",
        "PRIVATE-CARRIER",
        "PRIVATE-PROCUREMENT",
        "offers",
        "selected_offer_id",
        "buy_cents",
        "payment_instructions",
        "published_by",
        "source_load",
    ] {
        assert!(!view.to_string().contains(secret), "{secret}");
    }
    let original = call(&r, "customer", "GET", &doc, json!({}), "doc").await;
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
    for who in ["other", "carrier"] {
        assert_eq!(call(&r, who, "GET", &doc, json!({}), "deny").await.0, 404);
    }
    let good = response(&s, &id, &p, "accept").await;
    let before = json!(read(&s).await);
    for (field, value) in [
        ("customer_id", json!("c3")),
        ("recorded_by", json!("forged")),
        ("amount_cents", json!(1)),
        ("document_sha256", json!("changed")),
        ("revision", json!(999)),
    ] {
        let mut forged = good.clone();
        forged[field] = value;
        assert!(
            call(&r, "customer", "POST", &path, forged, &domain::id("bad"))
                .await
                .0
                >= 400
        );
    }
    for who in ["carrier", "staff", "other"] {
        assert_eq!(
            call(&r, who, "POST", &path, good.clone(), "deny").await.0,
            if who == "other" { 404 } else { 403 }
        );
    }
    assert_eq!(json!(read(&s).await), before);
    let result = call(&r, "customer", "POST", &path, good.clone(), "accept-once").await;
    assert_eq!(result.0, 200, "{result:?}");
    assert_eq!(result.1["data"]["recorded_by"], "native:customer");
    assert!(!result.1.to_string().contains("offers"));
    let after = json!(read(&s).await);
    assert_eq!(
        call(&r, "customer", "POST", &path, good.clone(), "accept-once")
            .await
            .1["replayed"],
        true
    );
    assert_eq!(
        call(&r, "second", "POST", &path, good, "accept-once")
            .await
            .0,
        409
    );
    assert_eq!(json!(read(&s).await), after);
    assert_eq!(
        call(&r, "customer", "GET", &doc, json!({}), "same-document")
            .await
            .1,
        original.1
    );
    let w = read(&s).await;
    let q = w.quotes.iter().find(|q| q.id == id).expect("quote");
    assert_eq!(q.status, "accepted");
    assert!(q.booked_load_id.is_none());
    assert_eq!(w.customer_quote_decisions.len(), 1);
    assert!(w.journal.is_empty());
    let booked = op(&s, "quote.book", &id, json!({"expected_version":q.version})).await;
    let lid = booked["booked_load_id"].as_str().expect("load");
    let w = read(&s).await;
    let l = w.loads.iter().find(|l| l.id == lid).expect("booked");
    assert_eq!((l.buy_cents, l.sell_cents), (125001, 150001));
    assert!(l.carrier_id.is_none());
    assert!(
        call(
            &r,
            "customer",
            "POST",
            &path,
            response(&s, &id, &p, "withdraw").await,
            "booked-withdraw"
        )
        .await
        .0 >= 400
    );
    assert_eq!(
        call(&r, "customer", "GET", &doc, json!({}), "after-booking")
            .await
            .1,
        original.1
    );
}
#[tokio::test]
async fn customer_withdrawal_revision_history_and_changed_capacity_never_reuse_old_acceptance_or_publish_drafts(
) {
    let (s, r, id) = setup().await;
    let p = publish(&s, &id, None).await;
    let pid = p["id"].as_str().expect("pub");
    let path = format!("portal/customer-quotes/{pid}/response");
    let mut expired = read(&s).await;
    expired
        .quotes
        .iter_mut()
        .find(|q| q.id == id)
        .expect("quote")
        .revisions
        .last_mut()
        .expect("revision")
        .valid_until = Utc::now() - Duration::minutes(1);
    let trusted = json!({"customer_id":"c1","subject":"native:customer","request":response(&s,&id,&p,"accept").await});
    assert!(expired
        .apply_customer_quote("portal.quote.respond", pid, trusted, "native:customer")
        .is_err());
    let mut unqualified = read(&s).await;
    unqualified
        .carriers
        .iter_mut()
        .find(|c| c.id == "ca4")
        .expect("carrier")
        .insurance_expires = (Utc::now() - Duration::days(1)).date_naive();
    assert!(
        !unqualified.customer_quote_views("c1").expect("view")[0]["can_accept"]
            .as_bool()
            .expect("bool")
    );
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            &path,
            response(&s, &id, &p, "accept").await,
            "accept"
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            &path,
            response(&s, &id, &p, "withdraw").await,
            "withdraw"
        )
        .await
        .0,
        200
    );
    let mut revision = draft("IGNORED");
    revision
        .as_object_mut()
        .expect("object")
        .remove("reference");
    revision["expected_version"] = json!(version(&s, &id).await);
    revision["reason"] = json!("PRIVATE-REVISED-REASON");
    revision["sell_cents"] = json!(160001);
    op(&s, "quote.revise", &id, revision).await;
    assert!(s
        .mutate(
            "demo",
            "draft-publish",
            "portal.quote.publish",
            &id,
            json!({"expected_version":version(&s,&id).await,"expected_previous_id":pid})
        )
        .await
        .is_err());
    let view = call(&r, "customer", "GET", "portal", json!({}), "historical")
        .await
        .1;
    assert_eq!(view["quotes"][0]["status"], "historical");
    assert!(view["quotes"][0]["version"].is_null());
    assert!(!view.to_string().contains("160001"));
    assert!(!view.to_string().contains("PRIVATE-REVISED"));
    issue(&s, &id).await;
    let next = publish(&s, &id, Some(pid)).await;
    assert_eq!(next["revision"], 2);
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            &path,
            response(&s, &id, &p, "accept").await,
            "stale-pub"
        )
        .await
        .0,
        409
    );
    let view = call(&r, "customer", "GET", "portal", json!({}), "both")
        .await
        .1;
    assert_eq!(view["quotes"].as_array().expect("quotes").len(), 2);
    assert_eq!(
        view["quotes"][0]["decisions"]
            .as_array()
            .expect("history")
            .len(),
        2
    );
}

#[tokio::test]
async fn imported_customer_amendment_requires_fresh_exact_source_consent_without_exposing_raw_orders_or_activating_early(
) {
    use axum::{http::StatusCode, response::IntoResponse, routing::get, Json};
    use stateset_freight::integrations::Connection;
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let order = json!({"id":uuid::Uuid::new_v4().to_string(),"orderNumber":"NATIVE-SOURCE-ORIGINAL","status":"pending","lineItems":[{"sku":"CUSTOMER-SKU","quantity":2}],"metadata":{"PRIVATE-INTERNAL-NOTE":"never publish raw source"}});
    let canonical = Arc::new(tokio::sync::RwLock::new(order.clone()));
    let unavailable = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let (data, down, count) = (canonical.clone(), unavailable.clone(), reads.clone());
    let fixture = Router::new().route(
        "/v1/tenants/native-sync/orders/{id}",
        get(move |headers: axum::http::HeaderMap| {
            let (data, down, count) = (data.clone(), down.clone(), count.clone());
            async move {
                assert_eq!(headers["x-stateset-api-key"], "native-source-key");
                count.fetch_add(1, Ordering::SeqCst);
                if down.load(Ordering::SeqCst) {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"error":"Owned outage"})),
                    )
                        .into_response()
                } else {
                    Json(json!({"data":data.read().await.clone()})).into_response()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned listener");
    let address = listener.local_addr().expect("address");
    let server =
        tokio::spawn(async move { axum::serve(listener, fixture).await.expect("fixture") });
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let r = router_with(
        s.clone(),
        Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    sync_url: Some(format!("http://{address}")),
                    sync_api_key: Some("native-source-key".into()),
                    sync_tenant_id: Some("native-sync".into()),
                    ..Connection::default()
                },
            )]),
        },
    );
    let mut plan = draft("SOURCE")["plan"].clone();
    plan["reference"] = json!("NATIVE-SOURCE-LOAD");
    plan["buy_cents"] = json!(90000);
    plan["sell_cents"] = json!(140000);
    let source = op(&s, "sync.import", "", json!({"order":order,"load":plan})).await;
    let lid = source["id"].as_str().expect("load");
    let mut body = draft("NATIVE-SOURCE-QUOTE");
    body["planned_load_id"] = json!(lid);
    body["expected_load_version"] = source["version"].clone();
    let q = op(&s, "quote.create", "", body).await;
    let qid = q["id"].as_str().expect("quote");
    issue(&s, qid).await;
    let pubresult = call(
        &r,
        "staff",
        "POST",
        &format!("quotes/{qid}/portal-publications"),
        json!({"expected_version":version(&s,qid).await}),
        "source-pub",
    )
    .await;
    assert_eq!(pubresult.0, 200, "{pubresult:?}");
    let p = &pubresult.1["data"];
    let pid = p["id"].as_str().expect("publication");
    let accept = call(
        &r,
        "customer",
        "POST",
        &format!("portal/customer-quotes/{pid}/response"),
        response(&s, qid, p, "accept").await,
        "original-accept",
    )
    .await;
    assert_eq!(accept.0, 200, "{accept:?}");
    let book = call(
        &r,
        "staff",
        "POST",
        &format!("quotes/{qid}/book"),
        json!({"expected_version":version(&s,qid).await,"expected_load_version":source["version"]}),
        "original-book",
    )
    .await;
    assert_eq!(book.0, 200, "{book:?}");
    let load = read(&s)
        .await
        .loads
        .into_iter()
        .find(|l| l.id == lid)
        .expect("load");
    let mut proposal = draft("NATIVE-SOURCE-AMENDMENT");
    proposal["expected_version"] = json!(version(&s, qid).await);
    proposal["expected_load_version"] = json!(load.version);
    proposal["reason"] = json!("PRIVATE-STAFF-AMENDMENT-REASON");
    proposal["sell_cents"] = json!(170001);
    proposal["plan"]["destination"]["city"] = json!("Tacoma");
    let child = op(&s, "quote.amend", qid, proposal).await;
    let cid = child["id"].as_str().expect("child");
    issue(&s, cid).await;
    canonical.write().await["status"] = json!("cancelled");
    let childpub = call(
        &r,
        "staff",
        "POST",
        &format!("quotes/{cid}/portal-publications"),
        json!({"expected_version":version(&s,cid).await}),
        "child-pub",
    )
    .await;
    assert_eq!(childpub.0, 200, "{childpub:?}");
    let p = &childpub.1["data"];
    let pid = p["id"].as_str().expect("pub");
    let path = format!("portal/customer-quotes/{pid}/response");
    let refresh = format!("portal/customer-quotes/{pid}/source/refresh");
    let count = reads.load(Ordering::SeqCst);
    assert_eq!(
        call(&r, "other", "POST", &refresh, json!({}), "crossed-source")
            .await
            .0,
        404
    );
    assert_eq!(reads.load(Ordering::SeqCst), count);
    let view = call(&r, "customer", "GET", "portal", json!({}), "source-view")
        .await
        .1;
    assert!(!view.to_string().contains("PRIVATE-INTERNAL"));
    assert!(!view.to_string().contains("PRIVATE-STAFF"));
    let current = view["quotes"]
        .as_array()
        .expect("quotes")
        .iter()
        .find(|q| q["id"] == pid)
        .expect("quote");
    assert_eq!(current["source"]["status"], "cancelled");
    assert_eq!(current["source"]["requires_authorization"], true);
    let mut good = response(&s, cid, p, "accept").await;
    good["source_digest"] = current["source"]["digest"].clone();
    good["authorize_transport"] = json!(false);
    let denied = call(
        &r,
        "customer",
        "POST",
        &path,
        good.clone(),
        "missing-source-consent",
    )
    .await;
    assert_eq!(denied.0, 422, "{denied:?}");
    good["authorize_transport"] = json!(true);
    canonical.write().await["lineItems"][0]["quantity"] = json!(3);
    let changed = call(
        &r,
        "customer",
        "POST",
        &path,
        good.clone(),
        "changed-source",
    )
    .await;
    assert_eq!(changed.0, 409, "{changed:?}");
    let fresh = call(&r, "customer", "POST", &refresh, json!({}), "fresh-source").await;
    assert_eq!(fresh.0, 200, "{fresh:?}");
    good["source_digest"] = fresh.1["data"]["source"]["digest"].clone();
    unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&r, "customer", "POST", &path, good.clone(), "outage")
            .await
            .0,
        503
    );
    unavailable.store(false, Ordering::SeqCst);
    let accepted = call(&r, "customer", "POST", &path, good, "actual-source-consent").await;
    assert_eq!(accepted.0, 200, "{accepted:?}");
    let w = read(&s).await;
    let load = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!(load.sell_cents, 150001);
    assert_eq!(load.destination.city, "Seattle");
    assert_eq!(
        w.quotes
            .iter()
            .find(|q| q.id == qid)
            .expect("parent")
            .status,
        "booked"
    );
    assert!(
        w.quotes
            .iter()
            .find(|q| q.id == cid)
            .expect("child")
            .current()
            .expect("revision")
            .source_consent
            .as_ref()
            .expect("consent")
            .authorize_transport
    );
    let activated = call(
        &r,
        "staff",
        "POST",
        &format!("quotes/{cid}/book"),
        json!({"expected_version":version(&s,cid).await,"expected_load_version":load.version}),
        "activate",
    )
    .await;
    assert_eq!(activated.0, 200, "{activated:?}");
    let w = read(&s).await;
    let load = w.loads.iter().find(|l| l.id == lid).expect("same source");
    assert_eq!(load.sell_cents, 170001);
    assert_eq!(load.destination.city, "Tacoma");
    assert_eq!(load.sync_order_id, order["id"].as_str().map(str::to_owned));
    assert_eq!(canonical.read().await["status"], "cancelled");
    assert!(w.journal.is_empty());
    server.abort();
}

#[tokio::test]
async fn revised_quote_for_another_customer_keeps_only_original_publication_visible_to_the_prior_customer(
) {
    let (s, r, id) = setup().await;
    let p = publish(&s, &id, None).await;
    let pid = p["id"].as_str().expect("pub");
    let first = call(
        &r,
        "customer",
        "GET",
        &format!("portal/customer-quotes/{pid}/statement"),
        json!({}),
        "original",
    )
    .await
    .1;
    op(&s,"quote.decision",&id,json!({"expected_version":version(&s,&id).await,"decision":"withdraw","contact":"Actual original customer","reference":"Actual original withdrawal","note":"Original customer confirms this quote is no longer required"})).await;
    let mut revision = draft("IGNORED");
    revision.as_object_mut().expect("body").remove("reference");
    revision["plan"]["customer_id"] = json!("c3");
    revision["sell_cents"] = json!(190001);
    revision["expected_version"] = json!(version(&s, &id).await);
    revision["reason"] = json!("PRIVATE-OTHER-CUSTOMER-PLAN");
    op(&s, "quote.revise", &id, revision).await;
    issue(&s, &id).await;
    let next = publish(&s, &id, Some(pid)).await;
    let nextid = next["id"].as_str().expect("next");
    let original = call(&r, "customer", "GET", "portal", json!({}), "original-scope")
        .await
        .1;
    assert_eq!(original["quotes"].as_array().expect("quotes").len(), 1);
    assert_eq!(original["quotes"][0]["status"], "historical");
    assert!(original["quotes"][0]["version"].is_null());
    assert!(!original.to_string().contains("190001"));
    assert_eq!(
        call(
            &r,
            "customer",
            "GET",
            &format!("portal/customer-quotes/{pid}/statement"),
            json!({}),
            "history"
        )
        .await
        .1,
        first
    );
    assert_eq!(
        call(
            &r,
            "customer",
            "GET",
            &format!("portal/customer-quotes/{nextid}/statement"),
            json!({}),
            "cross-document"
        )
        .await
        .0,
        404
    );
    let other = call(&r, "other", "GET", "portal", json!({}), "new-scope")
        .await
        .1;
    assert_eq!(other["quotes"].as_array().expect("quotes").len(), 1);
    assert_eq!(other["quotes"][0]["snapshot"]["amount_cents"], 190001);
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            &format!("portal/customer-quotes/{nextid}/response"),
            response(&s, &id, &next, "accept").await,
            "cross-accept"
        )
        .await
        .0,
        404
    );
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_customer_publication_and_native_decisions_serialize_preserve_exact_bytes_and_roll_back_corruption(
) {
    let (memory, _, id) = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("customer-portal-pg");
    let mut data = json!(read(&memory).await);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned tenant");
    let body = json!({"expected_version":version(&memory,&id).await});
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "staff:a"),
            "publish-a",
            "portal.quote.publish",
            &id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "staff:b"),
            "publish-b",
            "portal.quote.publish",
            &id,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let w = a.read(&tenant).await.expect("read");
    let p = w.customer_quote_publications[0].clone();
    let q = w.quotes.iter().find(|q| q.id == id).expect("quote");
    let native = json!({"customer_id":"c1","subject":"native:customer","request":{"expected_version":q.version,"revision":p.revision,"document_sha256":p.sha256,"decision":"accept","contact":"Actual native customer","reference":"Actual acceptance reference","note":"Customer accepted exact published shipment terms"}});
    canonical::set(
        &pool,
        &tenant,
        &["customer_quote_publications", "0", "html"],
        json!("corrupted customer document"),
    )
    .await
    .expect("owned corruption");
    let corrupt = json!(a.read(&tenant).await.expect("corrupt"));
    assert!(a
        .mutate_authorized(
            (&tenant, "native:customer"),
            "native-a",
            "portal.quote.respond",
            &p.id,
            native.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), corrupt);
    canonical::set(
        &pool,
        &tenant,
        &["customer_quote_publications", "0", "html"],
        json!(p.html),
    )
    .await
    .expect("restore original bytes");
    let mut decline = native.clone();
    decline["request"]["decision"] = json!("decline");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "native:customer"),
            "native-a",
            "portal.quote.respond",
            &p.id,
            native
        ),
        b.mutate_authorized(
            (&tenant, "native:customer"),
            "native-b",
            "portal.quote.respond",
            &p.id,
            decline
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    drop((a, b));
    let reopened = Store::new(Some(&url)).await.expect("reconnect");
    let w = reopened.read(&tenant).await.expect("retained");
    assert_eq!(w.customer_quote_publications.len(), 1);
    assert_eq!(w.customer_quote_decisions.len(), 1);
    assert_eq!(w.customer_quote_decisions[0].recorded_by, "native:customer");
    assert_eq!(w.customer_quote_publications[0].html, p.html);
    assert!(w.journal.is_empty());
    w.customer_quote_publications[0]
        .verify()
        .expect("exact frozen bytes");
    assert_eq!(
        w.quotes
            .iter()
            .find(|q| q.id == id)
            .expect("quote")
            .booked_load_id,
        None
    );
    assert!(reopened
        .read("other-customer-portal-pg")
        .await
        .expect("isolation")
        .customer_quote_publications
        .is_empty());
    canonical::set(
        &pool,
        &tenant,
        &[
            "customer_quote_publications",
            "0",
            "snapshot",
            "amount_cents",
        ],
        json!(1),
    )
    .await
    .expect("owned metadata corruption");
    let corrupt = reopened.read(&tenant).await.expect("corrupt");
    assert!(corrupt.customer_quote_views("c1").is_err());
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
}
