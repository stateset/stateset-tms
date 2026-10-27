#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    history::Query,
    integrations::Integrations,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;
fn query(v: Value) -> Query {
    serde_json::from_value(v).expect("query")
}
async fn operation(store: &Store, id: &str, op: &str, entity: &str, mut body: Value) -> Value {
    let w = store.read("demo").await.expect("workspace");
    body["expected_load_version"] =
        json!(w.loads.iter().find(|l| l.id == id).expect("load").version);
    store
        .mutate_authorized(
            ("demo", "test:dispatcher"),
            &uuid::Uuid::new_v4().to_string(),
            op,
            entity,
            body,
        )
        .await
        .expect(op)["data"]
        .clone()
}
async fn setup(store: &Store) -> (String, String) {
    store.seed_demo().await.expect("seed");
    let day = (chrono::Utc::now() - chrono::Duration::days(2))
        .date_naive()
        .to_string();
    let r=store.mutate("demo","owned-history-plan","load.create","",json!({"reference":"OWNED-HISTORY","customer_id":"c1","mode":"FTL","equipment":"Flatbed","commodity":"Original goods","weight_lbs":1000,"miles":100,"buy_cents":100000,"sell_cents":125000,
        "origin":{"facility":"Original dock","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},
        "destination":{"facility":"Original receiving","city":"Seattle","state":"WA","date":day,"address_lines":["25 Dock Street"],"postal_code":"98101","country":"US"}})).await.expect("plan");
    let id = r["data"]["id"].as_str().expect("id").to_string();
    for n in 0..9 {
        operation(store,&id,"stop.appointment.book",&id,json!({"stop":"pickup","start_local":format!("{day}T08:00"),"end_local":format!("{day}T09:00"),"timezone":"UTC","contact":"Original facility contact","confirmation_reference":format!("WINDOW-{n}"),"note":"Actual owned facility authorization"})).await;
    }
    for (action, extra) in [
        ("tender", json!({"carrier_id":"ca3"})),
        ("accept", json!({})),
    ] {
        let w = store.read("demo").await.expect("load");
        let mut body = extra;
        body["action"] = json!(action);
        body["expected_version"] =
            json!(w.loads.iter().find(|l| l.id == id).expect("load").version);
        store
            .mutate(
                "demo",
                &format!("history-{action}"),
                "load.command",
                &id,
                body,
            )
            .await
            .expect(action);
    }
    let mut gate=operation(store,&id,"stop.event.record",&id,json!({"stop":"pickup","kind":"arrival","local_time":format!("{day}T09:10"),"timezone":"UTC","reference":"Original gate receipt","note":"Original observed arrival"})).await;
    for minute in 11..17 {
        gate=operation(store,&id,"stop.event.correct",gate["id"].as_str().expect("gate"),json!({"local_time":format!("{day}T09:{minute}"),"timezone":"UTC","reference":format!("Correction receipt {minute}"),"note":"Actual correction; original retained"})).await;
    }
    (id, day)
}
async fn pages(store: &Store, kind: &str, id: Option<&str>, limit: usize) -> Vec<Value> {
    let mut cursor = Value::Null;
    let mut records = vec![];
    loop {
        let page = store
            .history_page(
                "demo",
                query(json!({"kind":kind,"load_id":id,"limit":limit,"cursor":cursor})),
            )
            .await
            .expect("history page");
        let items = page["items"].as_array().expect("items");
        assert!(items.len() <= limit);
        records.extend(items.iter().map(|i| i["record"].clone()));
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    records
}
#[tokio::test]
async fn original_histories_page_in_source_order_without_losing_corrections_or_inactive_windows() {
    let store = Store::new(None).await.expect("store");
    let (id, _) = setup(&store).await;
    let before = store.read("demo").await.expect("originals");
    for (kind, expected) in [
        (
            "appointments",
            json!(before
                .appointments
                .iter()
                .filter(|v| v.load_id == id)
                .rev()
                .collect::<Vec<_>>()),
        ),
        (
            "gates",
            json!(before
                .stop_events
                .iter()
                .filter(|v| v.load_id == id)
                .rev()
                .collect::<Vec<_>>()),
        ),
        (
            "events",
            json!(before
                .events
                .iter()
                .filter(|v| v.entity_id == id)
                .rev()
                .collect::<Vec<_>>()),
        ),
    ] {
        assert_eq!(
            json!(pages(&store, kind, Some(&id), 3).await),
            expected,
            "{kind}"
        );
    }
    assert_eq!(
        json!(store.read("demo").await.expect("after")),
        json!(before)
    );
}
#[tokio::test]
async fn append_window_and_scope_fences_keep_new_events_out_until_explicit_refresh() {
    let store = Store::new(None).await.expect("store");
    let (id, day) = setup(&store).await;
    let before = store.read("demo").await.expect("before");
    let first = store
        .history_page("demo", query(json!({"kind":"events","limit":3})))
        .await
        .expect("first");
    let cursor = first["next_cursor"].clone();
    operation(&store,&id,"stop.event.correct",before.stop_events.last().expect("gate").id.as_str(),json!({"local_time":format!("{day}T09:18"),"timezone":"UTC","reference":"Later corrected receipt","note":"New original after page capture"})).await;
    let mut actual = first["items"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|i| i["record"].clone())
        .collect::<Vec<_>>();
    let mut next = cursor.clone();
    loop {
        let page = store
            .history_page(
                "demo",
                query(json!({"kind":"events","limit":3,"cursor":next})),
            )
            .await
            .expect("older window");
        assert_eq!(page["has_new_records"], true);
        actual.extend(
            page["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|i| i["record"].clone()),
        );
        next = page["next_cursor"].clone();
        if next.is_null() {
            break;
        }
    }
    assert_eq!(
        json!(actual),
        json!(before.events.iter().rev().collect::<Vec<_>>())
    );
    for (tenant, value) in [
        ("other", json!({"kind":"events","cursor":cursor})),
        (
            "demo",
            json!({"kind":"events","load_id":id,"cursor":cursor}),
        ),
        ("demo", json!({"kind":"gates","load_id":id,"cursor":cursor})),
    ] {
        assert!(store.history_page(tenant, query(value)).await.is_err());
    }
    for invalid in [
        json!({"kind":"events","limit":0}),
        json!({"kind":"events","limit":101}),
        json!({"kind":"gates"}),
        json!({"kind":"events","cursor":"bad"}),
    ] {
        assert!(store.history_page("demo", query(invalid)).await.is_err());
    }
}
#[tokio::test]
async fn shipment_history_cursor_refuses_a_new_review_version_and_retains_all_originals() {
    let store = Store::new(None).await.expect("store");
    let (id, day) = setup(&store).await;
    let first = store
        .history_page(
            "demo",
            query(json!({"kind":"appointments","load_id":id,"limit":2})),
        )
        .await
        .expect("first");
    let w = store.read("demo").await.expect("original");
    operation(&store,&id,"stop.event.correct",&w.stop_events.last().expect("gate").id,json!({"local_time":format!("{day}T09:19"),"timezone":"UTC","reference":"Fresh gate ledger","note":"Actual later review"})).await;
    assert_eq!(store.history_page("demo",query(json!({"kind":"appointments","load_id":id,"limit":2,"cursor":first["next_cursor"]}))).await.expect_err("changed version"),stateset_freight::history::CHANGED);
    assert_eq!(pages(&store, "appointments", Some(&id), 2).await.len(), 9);
}
fn router(store: Store) -> axum::Router {
    let keys = [
        ("viewer", Role::Viewer, "demo", None),
        ("other", Role::Viewer, "other", None),
        ("carrier", Role::Carrier, "demo", Some("ca3")),
        ("customer", Role::Customer, "demo", Some("c1")),
    ]
    .into_iter()
    .map(|(key, role, tenant, counterparty)| {
        (
            key.to_string(),
            Credential::Scoped(Principal {
                tenant_id: tenant.into(),
                subject: format!("test:{key}"),
                role,
                counterparty_id: counterparty.map(str::to_string),
            }),
        )
    })
    .collect::<BTreeMap<_, _>>();
    api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store,
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    })
}
async fn get(app: axum::Router, path: &str, key: &str) -> (u16, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("x-tenant-id", "demo")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if status == 400 {
        json!({"rejection":String::from_utf8_lossy(&bytes)})
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, value)
}
#[tokio::test]
async fn deferred_workspace_preserves_current_views_and_never_claims_missing_history_is_empty() {
    let store = Store::new(None).await.expect("store");
    setup(&store).await;
    let app = router(store);
    let (status, mut full) = get(app.clone(), "/v1/workspace", "viewer").await;
    assert_eq!(status, 200);
    let (status, mut compact) = get(app.clone(), "/v1/workspace?history=deferred", "viewer").await;
    assert_eq!(status, 200);
    for (field, count) in [
        ("events", "events"),
        ("appointments", "appointments"),
        ("stop_events", "gates"),
    ] {
        assert!(compact.get(field).is_none());
        assert_eq!(
            compact["history"][count],
            full[field].as_array().expect("full originals").len()
        );
        full.as_object_mut().expect("object").remove(field);
    }
    assert_eq!(compact["history"]["mode"], "deferred");
    compact.as_object_mut().expect("object").remove("history");
    full["operations"]
        .as_object_mut()
        .expect("ops")
        .remove("generated_at");
    compact["operations"]
        .as_object_mut()
        .expect("ops")
        .remove("generated_at");
    assert_eq!(compact, full);
    assert_eq!(
        get(app, "/v1/workspace?history=typo", "viewer").await.0,
        400
    );
}
#[tokio::test]
async fn history_requires_staff_access_and_returns_the_verified_owner_not_a_tenant_header() {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let app = router(store);
    for (key, status) in [
        ("viewer", 200),
        ("other", 200),
        ("carrier", 403),
        ("customer", 403),
        ("bad", 401),
    ] {
        let (actual, body) = get(app.clone(), "/v1/history?kind=events&limit=2", key).await;
        assert_eq!(actual, status, "{key}");
        if status == 200 {
            assert_eq!(body["access"]["subject"], format!("test:{key}"));
            if key == "other" {
                assert_eq!(body["tenant_id"], "other");
                assert_eq!(body["items"], json!([]));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_history_has_memory_parity_bounded_rows_append_windows_and_format_fences() {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("history_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("store");
    let (id, day) = setup(&store).await;
    let pool = sqlx::PgPool::connect(url.as_str()).await.expect("pool");
    let before = store.read("demo").await.expect("source");
    for (kind, expected) in [
        (
            "events",
            json!(before.events.iter().rev().collect::<Vec<_>>()),
        ),
        (
            "appointments",
            json!(before
                .appointments
                .iter()
                .filter(|a| a.load_id == id)
                .rev()
                .collect::<Vec<_>>()),
        ),
        (
            "gates",
            json!(before
                .stop_events
                .iter()
                .filter(|a| a.load_id == id)
                .rev()
                .collect::<Vec<_>>()),
        ),
    ] {
        assert_eq!(
            json!(
                pages(
                    &store,
                    kind,
                    if kind == "events" { None } else { Some(&id) },
                    2
                )
                .await
            ),
            expected,
            "{kind}"
        );
    }
    let original: String =
        sqlx::query_scalar("SELECT md5(freight_workspace_originals(tenant_id)::text) FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&pool)
            .await
            .expect("physical");
    let first = store
        .history_page("demo", query(json!({"kind":"events","limit":2})))
        .await
        .expect("page");
    operation(&store,&id,"stop.event.correct",&before.stop_events.last().expect("gate").id,json!({"local_time":format!("{day}T09:20"),"timezone":"UTC","reference":"New actual ledger","note":"Preserve captured prefix"})).await;
    let next = store
        .history_page(
            "demo",
            query(json!({"kind":"events","limit":2,"cursor":first["next_cursor"]})),
        )
        .await
        .expect("captured page");
    assert_eq!(next["has_new_records"], true);
    assert_eq!(next["total"], first["total"]);
    assert!(store
        .history_page(
            "other",
            query(json!({"kind":"events","cursor":first["next_cursor"]}))
        )
        .await
        .is_err());
    assert_ne!(
        original,
        sqlx::query_scalar::<_, String>(
            "SELECT md5(freight_workspace_originals(tenant_id)::text) FROM freight_workspaces WHERE tenant_id='demo'"
        )
        .fetch_one(&pool)
        .await
        .expect("changed source")
    );
    canonical::corrupt_marker(&pool, "demo", "22")
        .await
        .expect("owned future marker");
    assert_eq!(
        store
            .history_page("demo", query(json!({"kind":"events"})))
            .await
            .expect_err("fence"),
        stateset_freight::store::INCOMPATIBLE_STORAGE
    );
    canonical::corrupt_marker(&pool, "demo", "21")
        .await
        .expect("restore marker");
    canonical::set(&pool, "demo", &["stop_events"], Value::Null)
        .await
        .expect("malformed collection");
    assert_eq!(
        store
            .history_page("demo", query(json!({"kind":"gates","load_id":id})))
            .await
            .expect_err("malformed source"),
        "Internal storage error"
    );
    pool.close().await;
    drop(store);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned cleanup");
    admin.close().await;
}
