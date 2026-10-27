#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::Connection;
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain,
    integrations::Integrations,
    load_catalog::CatalogQuery,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

fn query(value: Value) -> CatalogQuery {
    serde_json::from_value(value).expect("query")
}

#[tokio::test]
async fn catalog_pages_are_bounded_filter_bound_and_tenant_scoped() {
    let store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("seed");
    let before = store.read("demo").await.expect("original");
    let mut expected: Vec<_> = before.loads.iter().map(|load| load.id.clone()).collect();
    expected.sort();
    let mut actual = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = store
            .load_catalog("demo", query(json!({"limit":2,"cursor":cursor})))
            .await
            .expect("page");
        assert!(page["loads"].as_array().expect("rows").len() <= 2);
        for row in page["loads"].as_array().expect("rows") {
            actual.push(row["id"].as_str().expect("id").to_string());
            for field in [
                "source_order",
                "buy_cents",
                "sell_cents",
                "documents",
                "events",
                "receipts",
                "journal",
            ] {
                assert!(
                    row.get(field).is_none(),
                    "catalog must not expose unrelated history or incomplete finance totals"
                );
            }
        }
        if cursor.is_null() {
            let token = page["next_cursor"].as_str().expect("more than two loads");
            assert!(store
                .load_catalog("other", query(json!({"cursor":token})))
                .await
                .is_err());
            assert!(store
                .load_catalog("demo", query(json!({"cursor":token,"status":"planned"})))
                .await
                .is_err());
            assert!(store
                .load_catalog("demo", query(json!({"cursor":token,"q":"Portland"})))
                .await
                .is_err());
        }
        cursor = page["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(actual, expected);
    for invalid in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"cursor":"malformed"}),
        json!({"q":"x".repeat(201)}),
        json!({"customer_id":""}),
    ] {
        assert!(store.load_catalog("demo", query(invalid)).await.is_err());
    }
    assert!(store
        .load_catalog("other", CatalogQuery::default())
        .await
        .expect("empty tenant")["loads"]
        .as_array()
        .expect("rows")
        .is_empty());
    assert_eq!(
        json!(store.read("demo").await.expect("unchanged")),
        json!(before)
    );
}

#[tokio::test]
async fn catalog_search_is_literal_and_tracks_committed_commands_and_original_replays() {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let load = store.read("demo").await.expect("state").loads[0].clone();
    let body = json!({"action":"location","expected_version":load.version,"location":"Literal 100%_ AUSTIN"});
    store
        .mutate(
            "demo",
            "catalog-location",
            "load.command",
            &load.id,
            body.clone(),
        )
        .await
        .expect("original command");
    let selection = json!({"q":"  100%_ austin  ","status":load.status,"customer_id":load.customer_id,"carrier_id":load.carrier_id});
    let page = store
        .load_catalog("demo", query(selection.clone()))
        .await
        .expect("literal match");
    assert_eq!(page["loads"].as_array().expect("rows").len(), 1);
    assert_eq!(page["loads"][0]["id"], load.id);
    assert_eq!(page["loads"][0]["version"], load.version + 1);
    store
        .mutate("demo", "catalog-location", "load.command", &load.id, body)
        .await
        .expect("original replay");
    assert_eq!(
        store
            .load_catalog("demo", query(selection))
            .await
            .expect("replayed"),
        page
    );
    assert!(store
        .load_catalog("demo", query(json!({"q":"100anything"})))
        .await
        .expect("literal")["loads"]
        .as_array()
        .expect("rows")
        .is_empty());
}

#[tokio::test]
async fn catalog_and_access_enforce_staff_scope_before_reading_business_data() {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let keys = [
        ("viewer", Role::Viewer, "demo", None),
        ("other", Role::Viewer, "other", None),
        ("carrier", Role::Carrier, "demo", Some("ca4")),
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
    let app = api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store,
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    });
    for (key, expected) in [
        ("viewer", 200),
        ("other", 200),
        ("carrier", 403),
        ("customer", 403),
        ("unknown", 401),
    ] {
        for path in ["/v1/loads?limit=2", "/v1/access"] {
            let response = app
                .clone()
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
            assert_eq!(response.status().as_u16(), expected, "{key} {path}");
            let body: Value = serde_json::from_slice(
                &response
                    .into_body()
                    .collect()
                    .await
                    .expect("body")
                    .to_bytes(),
            )
            .expect("json");
            if expected == 200 && path == "/v1/access" {
                assert_eq!(body["access"]["subject"], format!("test:{key}"));
                assert!(body.get("loads").is_none());
            }
            if key == "other" && path.starts_with("/v1/loads") {
                assert_eq!(body["loads"], json!([]));
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_catalog_backfill_atomic_projection_and_history_independent_reads() {
    use sha2::{Digest, Sha256};
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("catalog_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let pool = sqlx::PgPool::connect(url.as_str()).await.expect("pool");
    sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())").execute(&pool).await.expect("history");
    let mut migrations = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .expect("migrations")
        .map(|p| p.expect("path").path())
        .collect::<Vec<_>>();
    migrations.sort();
    for (index, path) in migrations.iter().take(20).enumerate() {
        let sql = std::fs::read_to_string(path).expect("sql");
        sqlx::raw_sql(&sql)
            .execute(&pool)
            .await
            .expect("previous migration");
        sqlx::query("INSERT INTO freight_schema_migrations VALUES($1,$2,now())")
            .bind(index as i32 + 1)
            .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
            .execute(&pool)
            .await
            .expect("checksum");
    }
    let mut original = json!(domain::demo());
    original["_freight_format"] = json!(17);
    sqlx::query(
        "INSERT INTO freight_workspaces(tenant_id,data,updated_at) VALUES('demo',$1,'2026-01-01T00:00:00Z')",
    )
    .bind(&original)
    .execute(&pool)
    .await
    .expect("original data");
    let before: (String, String, Value) = sqlx::query_as(
        "SELECT xmin::text,updated_at::text,data FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&pool)
    .await
    .expect("before");
    for (index, path) in migrations.iter().enumerate().skip(20).take(2) {
        let sql = std::fs::read_to_string(path).expect("catalog migration");
        sqlx::raw_sql(&sql)
            .execute(&pool)
            .await
            .expect("catalog upgrade");
        sqlx::query("INSERT INTO freight_schema_migrations VALUES($1,$2,now())")
            .bind(index as i32 + 1)
            .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
            .execute(&pool)
            .await
            .expect("catalog history");
    }
    let catalog_only: (String, String, Value) = sqlx::query_as(
        "SELECT xmin::text,updated_at::text,data FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&pool)
    .await
    .expect("catalog-only upgrade");
    assert_eq!(
        catalog_only, before,
        "catalog backfill must not rewrite source records"
    );
    let store = Store::new(Some(url.as_str()))
        .await
        .expect("upgrade existing schema");
    let after: (String, String, Value) = sqlx::query_as(
        "SELECT xmin::text,updated_at::text,freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id='demo'",
    )
    .fetch_one(&pool)
    .await
    .expect("after");
    let mut expected_source = before.2.clone();
    expected_source["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    assert_eq!(
        after.1, before.1,
        "format upgrade preserves original timestamp"
    );
    assert_eq!(
        after.2, expected_source,
        "format upgrade changes only the marker"
    );
    original["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    // Identical retained data including dates, not a second clock-relative demo.
    let expected = store.read("demo").await.expect("authoritative");
    let page = store
        .load_catalog("demo", CatalogQuery::default())
        .await
        .expect("catalog");
    assert_eq!(
        page["loads"].as_array().expect("rows").len(),
        expected.loads.len()
    );
    let all_before: Vec<(String, String, Value)> = sqlx::query_as(
        "SELECT load_id,xmin::text,summary FROM freight_load_search ORDER BY load_id",
    )
    .fetch_all(&pool)
    .await
    .expect("projection");
    let load = &expected.loads[0];
    let body = json!({"action":"location","expected_version":load.version,"location":"Literal 100%_ AUSTIN"});
    store
        .mutate(
            "demo",
            "catalog-location",
            "load.command",
            &load.id,
            body.clone(),
        )
        .await
        .expect("change");
    let all_after: Vec<(String, String, Value)> = sqlx::query_as(
        "SELECT load_id,xmin::text,summary FROM freight_load_search ORDER BY load_id",
    )
    .fetch_all(&pool)
    .await
    .expect("projection");
    for (before, after) in all_before.iter().zip(&all_after) {
        if before.0 == load.id {
            assert_ne!(before.1, after.1);
            assert_eq!(after.2["version"], load.version + 1);
        } else {
            assert_eq!(
                before, after,
                "unrelated catalog rows must not be rewritten"
            );
        }
    }
    let page = store
        .load_catalog("demo", query(json!({"q":"100%_ austin"})))
        .await
        .expect("filtered");
    assert_eq!(page["loads"].as_array().expect("rows").len(), 1);
    // The read path must not select, lock, transfer or decode tenant history.
    let mut locked = pool.begin().await.expect("lock");
    sqlx::query("LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await
        .expect("held history");
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.load_catalog("demo", query(json!({"q":"100%_ austin"})))
        )
        .await
        .expect("independent read")
        .expect("catalog"),
        page
    );
    locked.rollback().await.expect("unlock");
    let mut tx = pool.begin().await.expect("staged transaction");
    let mut uncommitted = canonical::source(&pool, "demo")
        .await
        .expect("committed source");
    uncommitted["loads"][0]["location"] = json!("Rolled back location");
    canonical::put_tx(&mut tx, "demo", &uncommitted)
        .await
        .expect("staged source/index");
    assert_eq!(
        store
            .load_catalog("demo", query(json!({"q":"100%_ austin"})))
            .await
            .expect("outside snapshot"),
        page
    );
    tx.rollback().await.expect("rollback");
    store
        .mutate("demo", "catalog-location", "load.command", &load.id, body)
        .await
        .expect("original replay");
    assert_eq!(
        store
            .load_catalog("demo", query(json!({"q":"100%_ austin"})))
            .await
            .expect("same original"),
        page
    );
    canonical::set(&pool, "demo", &["loads"], json!({}))
        .await
        .expect("retain damaged original");
    assert!(
        store
            .load_catalog("demo", CatalogQuery::default())
            .await
            .is_err(),
        "malformed source cannot become a successful empty page"
    );
    canonical::put(&pool, "demo", &original)
        .await
        .expect("repair source");
    assert_eq!(
        store
            .load_catalog("demo", CatalogQuery::default())
            .await
            .expect("repaired")["loads"]
            .as_array()
            .expect("rows")
            .len(),
        expected.loads.len()
    );
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id='demo'")
        .execute(&pool)
        .await
        .expect("delete tenant");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_load_search")
        .fetch_one(&pool)
        .await
        .expect("cascade");
    assert_eq!(count, 0);
    drop(store);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("clean schema");
    admin.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_incremental_catalog_matches_original_projector_across_source_changes() {
    use sha2::{Digest, Sha256};
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("catalog_delta_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    // Keep this connection alive through the upgrade, exercising a cached old
    // trigger/projector as well as a newly opened application connection.
    let mut old_writer = sqlx::PgConnection::connect(url.as_str())
        .await
        .expect("old writer");
    sqlx::raw_sql("CREATE TABLE freight_schema_migrations(version integer PRIMARY KEY,checksum text NOT NULL,applied_at timestamptz NOT NULL DEFAULT now())")
        .execute(&mut old_writer).await.expect("history");
    let mut migrations = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .expect("migrations")
        .map(|p| p.expect("path").path())
        .collect::<Vec<_>>();
    migrations.sort();
    for (index, path) in migrations.iter().take(21).enumerate() {
        let sql = std::fs::read_to_string(path).expect("sql");
        sqlx::raw_sql(&sql)
            .execute(&mut old_writer)
            .await
            .expect("previous migration");
        sqlx::query("INSERT INTO freight_schema_migrations VALUES($1,$2,now())")
            .bind(index as i32 + 1)
            .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
            .execute(&mut old_writer)
            .await
            .expect("checksum");
    }
    let mut source = json!(domain::demo());
    source["_freight_format"] = json!(17);
    let template = source["loads"][0].clone();
    source["loads"] = json!((0..12)
        .map(|i| {
            let mut load = template.clone();
            load["id"] = json!(format!("shipment-{i:02}"));
            load
        })
        .collect::<Vec<_>>());
    sqlx::query(
        "INSERT INTO freight_workspaces(tenant_id,data) VALUES('actual',$1),('reference',$1)",
    )
    .bind(&source)
    .execute(&mut old_writer)
    .await
    .expect("old source/trigger");
    sqlx::query("UPDATE freight_workspaces SET data=jsonb_set(data,'{loads,0,location}','\"Warm old writer\"') WHERE tenant_id='actual'")
        .execute(&mut old_writer).await.expect("cache old trigger plans");
    let before: Vec<(String, String)> = sqlx::query_as("SELECT load_id,xmin::text FROM freight_load_search WHERE tenant_id='actual' ORDER BY load_id")
        .fetch_all(&mut old_writer).await.expect("physical catalog before upgrade");
    let (a, b) = tokio::join!(
        Store::new(Some(url.as_str())),
        Store::new(Some(url.as_str()))
    );
    let a = a.expect("first upgrader");
    let b = b.expect("second upgrader");
    let after: Vec<(String, String)> = sqlx::query_as("SELECT load_id,xmin::text FROM freight_load_search WHERE tenant_id='actual' ORDER BY load_id")
        .fetch_all(&mut old_writer).await.expect("physical catalog after upgrade");
    assert_eq!(before, after, "upgrade must not rewrite catalog rows");
    // Independent oracle: the released migration 021 projector, without any
    // incremental comparison. Compare every stored projection field, not just
    // counts or values generated by the new implementation itself.
    let released = include_str!("../migrations/021_load_catalog.sql");
    let reference = released[released
        .find("CREATE FUNCTION freight_project_load_catalog")
        .expect("retained fixture structure")
        ..released
            .find("CREATE FUNCTION freight_refresh_load_catalog")
            .expect("retained fixture structure")]
        .replace("freight_project_load_catalog", "freight_catalog_reference");
    sqlx::raw_sql(&reference)
        .execute(&mut old_writer)
        .await
        .expect("released oracle");
    let pool = sqlx::PgPool::connect(url.as_str())
        .await
        .expect("canonical fixture pool");
    source["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    let original = source.clone();
    let mut cases = Vec::new();
    source["loads"][0]["location"] = json!("Literal 100%_ AUSTIN");
    source["loads"][0]["version"] = json!(99);
    cases.push(("one shipment", source.clone(), false));
    source["loads"][1]["order_number"] = json!("Search-only 100%_");
    cases.push(("search-only field", source.clone(), false));
    source["loads"][1]["source_order"] = json!({"private":"original commerce evidence"});
    source["loads"][1]["buy_cents"] = json!(123456789);
    cases.push(("private fields", source.clone(), true));
    source["loads"]
        .as_array_mut()
        .expect("retained fixture structure")
        .reverse();
    cases.push(("reorder", source.clone(), true));
    source["loads"][0]["id"] = json!("é-shipment");
    cases.push(("identity replacement", source.clone(), false));
    source["loads"]
        .as_array_mut()
        .expect("retained fixture structure")
        .pop();
    cases.push(("removal", source.clone(), false));
    let mut added = template.clone();
    added["id"] = json!("new-shipment");
    source["loads"]
        .as_array_mut()
        .expect("retained fixture structure")
        .insert(3, added);
    cases.push(("insertion", source.clone(), false));
    source["loads"][0]
        .as_object_mut()
        .expect("retained fixture structure")
        .remove("carrier_id");
    source["loads"][0]
        .as_object_mut()
        .expect("retained fixture structure")
        .remove("reference");
    source["loads"][0]["origin"] = json!({"city":"Seattle"});
    cases.push(("missing optional fields", source.clone(), false));
    source["loads"][0]["carrier_id"] = Value::Null;
    source["loads"][0]["reference"] = Value::Null;
    cases.push(("explicit nulls", source.clone(), true));
    for (name, loads) in [
        (
            "duplicate identity",
            json!([original["loads"][0], original["loads"][0]]),
        ),
        ("non-array", json!({})),
        ("null collection", Value::Null),
        ("scalar member", json!([original["loads"][0], 42])),
    ] {
        let mut damaged = original.clone();
        damaged["loads"] = loads;
        cases.push((name, damaged, false));
        cases.push((
            "repair must rebuild unchanged shipments",
            original.clone(),
            false,
        ));
    }
    for (name, field, value) in [
        ("numeric identity", "id", json!(7)),
        ("empty identity", "id", json!("")),
        ("oversized UTF-8 identity", "id", json!("é".repeat(101))),
        ("invalid status", "status", json!("unknown")),
        ("missing status", "status", Value::Null),
        ("invalid customer", "customer_id", json!({})),
        ("invalid carrier", "carrier_id", json!({})),
    ] {
        let mut damaged = original.clone();
        damaged["loads"][5][field] = value;
        cases.push((name, damaged, false));
        cases.push((
            "repair must rebuild unchanged shipments",
            original.clone(),
            false,
        ));
    }
    let mut missing = original.clone();
    missing
        .as_object_mut()
        .expect("retained fixture structure")
        .remove("loads");
    cases.push(("missing collection", missing, false));
    let mut empty = original.clone();
    empty["loads"] = json!([]);
    cases.push(("empty collection", empty, false));
    cases.push(("restore after empty", original.clone(), false));
    for (name, data, physically_unchanged) in cases {
        let before: Vec<(String, String)> = sqlx::query_as("SELECT load_id,xmin::text FROM freight_load_search WHERE tenant_id='actual' ORDER BY load_id")
            .fetch_all(&mut old_writer).await.expect("before case");
        let mut tx = pool.begin().await.expect("atomic source fixtures");
        canonical::put_tx(&mut tx, "actual", &data)
            .await
            .expect(name);
        canonical::put_tx(&mut tx, "reference", &data)
            .await
            .expect(name);
        tx.commit().await.expect("both source fixtures");
        sqlx::query("SELECT freight_catalog_reference('reference',$1)")
            .bind(&data)
            .execute(&mut old_writer)
            .await
            .expect("full oracle rebuild");
        let equal: bool = sqlx::query_scalar("SELECT NOT EXISTS ((SELECT valid FROM freight_load_catalog WHERE tenant_id='actual') EXCEPT (SELECT valid FROM freight_load_catalog WHERE tenant_id='reference')) AND NOT EXISTS ((SELECT load_id,status,customer_id,carrier_id,search_text,summary FROM freight_load_search WHERE tenant_id='actual') EXCEPT (SELECT load_id,status,customer_id,carrier_id,search_text,summary FROM freight_load_search WHERE tenant_id='reference')) AND NOT EXISTS ((SELECT load_id,status,customer_id,carrier_id,search_text,summary FROM freight_load_search WHERE tenant_id='reference') EXCEPT (SELECT load_id,status,customer_id,carrier_id,search_text,summary FROM freight_load_search WHERE tenant_id='actual'))")
            .fetch_one(&mut old_writer).await.expect("compare all projection fields");
        assert!(
            equal,
            "incremental projection differs from released full projector: {name}"
        );
        if physically_unchanged {
            let after: Vec<(String, String)> = sqlx::query_as("SELECT load_id,xmin::text FROM freight_load_search WHERE tenant_id='actual' ORDER BY load_id")
                .fetch_all(&mut old_writer).await.expect("after case");
            assert_eq!(before, after, "unrelated summary rewrite: {name}");
        }
    }
    // The public two-argument rebuild remains authoritative even if a catalog
    // row was damaged independently of the source; no previous-source hint.
    sqlx::query("DELETE FROM freight_load_search WHERE tenant_id='actual'")
        .execute(&mut old_writer)
        .await
        .expect("simulate projection damage");
    sqlx::query("SELECT freight_project_load_catalog('actual',freight_canonical_selected(tenant_id,data,ARRAY['loads'])) FROM freight_workspaces WHERE tenant_id='actual'")
        .execute(&mut old_writer).await.expect("explicit rebuild");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM freight_load_search WHERE tenant_id='actual'")
            .fetch_one(&mut old_writer)
            .await
            .expect("rebuilt rows");
    assert_eq!(count, 12);
    drop(a);
    drop(b);
    drop(old_writer);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("clean schema");
    admin.close().await;
}
