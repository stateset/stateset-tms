use super::*;
use crate::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain,
    integrations::Integrations,
    store::Store,
};
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use sqlx::{Connection, PgConnection, PgPool};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

fn nodes<'a>(node: &'a Value, output: &mut Vec<&'a Value>) {
    output.push(node);
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            nodes(child, output);
        }
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_filtered_catalog_plans_preserve_pages_and_avoid_unrelated_rows() {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = PgPool::connect(&base).await.expect("admin");
    let schema = format!("catalog_plans_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={schema},pg_catalog -cplan_cache_mode=force_generic_plan"),
    );
    let store = Store::new(Some(url.as_str())).await.expect("store");
    let pool = PgPool::connect(url.as_str()).await.expect("observer");
    let mut workspace = domain::demo();
    let template = workspace.loads[0].clone();
    workspace.loads = (0..1000)
        .map(|i| {
            let mut load = template.clone();
            load.id = format!("catalog-plan-{i:05}");
            load.reference = load.id.clone();
            load.customer_id = if i >= 950 { "c2" } else { "c1" }.into();
            load.carrier_id = Some(if i >= 950 { "ca2" } else { "ca1" }.into());
            load.status = if i >= 950 {
                Status::Planned
            } else {
                Status::Delivered
            };
            load.location = if i >= 950 {
                "Literal 100%_ AUSTIN"
            } else {
                "Archived location"
            }
            .into();
            load
        })
        .collect();
    let mut raw = json!(workspace);
    raw["_freight_format"] = json!(crate::store::STORAGE_FORMAT);
    crate::storage_fixture::put(&pool, "catalog", &raw)
        .await
        .expect("source");
    let mut other = raw.clone();
    for load in other["loads"].as_array_mut().expect("other loads") {
        load["reference"] = json!("OTHER TENANT PRIVATE REFERENCE");
    }
    crate::storage_fixture::put(&pool, "other", &other)
        .await
        .expect("other source");
    sqlx::raw_sql("ANALYZE freight_load_catalog; ANALYZE freight_load_search;")
        .execute(&pool)
        .await
        .expect("statistics");
    let before: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT tenant_id,xmin::text,updated_at::text,data::text FROM freight_workspaces ORDER BY tenant_id"
    ).fetch_all(&pool).await.expect("original source");
    let mut plans = Vec::new();
    let mut failures = Vec::new();
    let mut parity_checks = 0;
    for mask in 0..8 {
        let query = CatalogQuery {
            status: (mask & 1 != 0).then_some(Status::Planned),
            customer_id: (mask & 2 != 0).then(|| "c2".into()),
            carrier_id: (mask & 4 != 0).then(|| "ca2".into()),
            ..Default::default()
        };
        let selection = query.clone().selection("catalog").expect("selection");
        // Exact expected pages derive from the retained typed source through
        // the independent in-memory implementation, including scoped cursors.
        let mut page_query = query.clone();
        for _ in 0..2 {
            let expected = page_query
                .clone()
                .selection("catalog")
                .expect("selection")
                .memory_page(&workspace)
                .expect("memory page");
            let actual = store
                .load_catalog("catalog", page_query.clone())
                .await
                .expect("stored page");
            assert_eq!(actual, expected, "filter mask {mask}");
            parity_checks += 1;
            page_query.cursor = actual["next_cursor"].as_str().map(str::to_owned);
        }
        for mode in ["force_generic_plan", "force_custom_plan", "auto"] {
            for (version, statement) in [
                ("preceding", PAGE_QUERY),
                ("current", selection.page_query()),
            ] {
                // Separate connections give both versions fresh statement
                // caches. PREPARE the real SELECT, not an EXPLAIN wrapper.
                let mut conn = PgConnection::connect(url.as_str())
                    .await
                    .expect("plan connection");
                sqlx::raw_sql(&format!("SET plan_cache_mode={mode}; PREPARE catalog(text,text,text,text,text,text,bigint) AS {statement}"))
                    .execute(&mut conn).await.expect("prepare exact page statement");
                let arguments = format!(
                    "'catalog','',{},{},{},'',26",
                    if mask & 1 != 0 { "'planned'" } else { "NULL" },
                    if mask & 2 != 0 { "'c2'" } else { "NULL" },
                    if mask & 4 != 0 { "'ca2'" } else { "NULL" }
                );
                if mode == "auto" {
                    for _ in 0..6 {
                        sqlx::raw_sql(&format!("EXECUTE catalog({arguments});"))
                            .execute(&mut conn)
                            .await
                            .expect("warm actual statement");
                    }
                }
                let plan: Value = sqlx::query_scalar(&format!(
                    "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) EXECUTE catalog({arguments})"
                ))
                .fetch_one(&mut conn)
                .await
                .expect("explain actual prepared execution");
                let prepared: Value = sqlx::query_scalar(
                    "SELECT to_jsonb(p) FROM pg_prepared_statements p WHERE name='catalog'",
                )
                .fetch_one(&mut conn)
                .await
                .expect("prepared statement accounting");
                let mut all = Vec::new();
                nodes(&plan[0]["Plan"], &mut all);
                let removed: u64 = all
                    .iter()
                    .filter(|n| n["Relation Name"] == "freight_load_search")
                    .map(|n| {
                        n["Rows Removed by Filter"].as_u64().unwrap_or(0)
                            * n["Actual Loops"].as_u64().unwrap_or(1)
                    })
                    .sum();
                let indexes: Vec<_> = all
                    .iter()
                    .filter_map(|n| n["Index Name"].as_str())
                    .collect();
                assert!(!all
                    .iter()
                    .any(|n| n["Relation Name"] == "freight_workspaces"));
                assert!(plan[0]["Plan"]["Actual Rows"].as_u64().expect("rows") <= 26);
                if version == "current" && mask != 0 {
                    let selective_index = (mask & 1 != 0
                        && indexes.contains(&"freight_load_search_status"))
                        || (mask & 2 != 0 && indexes.contains(&"freight_load_search_customer"))
                        || (mask & 4 != 0 && indexes.contains(&"freight_load_search_carrier"));
                    if removed != 0 || !selective_index {
                        failures.push(format!("{mode} mask {mask}: removed {removed} unrelated rows; indexes {indexes:?}"));
                    }
                }
                plans.push(json!({"mask":mask,"mode":mode,"version":version,"sql":statement,
                    "arguments":arguments,"plan":plan,"prepared_statement":prepared,"unrelated_rows_removed":removed}));
                conn.close().await.expect("close plan connection");
            }
        }
        // Search remains literal and every value remains a bound parameter,
        // even when the selected SQL shape has no optional filter OR clause.
        for search in ["100%_ austin", "' OR TRUE --"] {
            let mut literal = query.clone();
            literal.q = Some(search.into());
            assert_eq!(
                store
                    .load_catalog("catalog", literal.clone())
                    .await
                    .expect("literal page"),
                literal
                    .selection("catalog")
                    .expect("selection")
                    .memory_page(&workspace)
                    .expect("memory literal")
            );
            parity_checks += 1;
        }
    }
    let hostile = CatalogQuery {
        customer_id: Some("c2' OR TRUE --".into()),
        ..Default::default()
    };
    assert_eq!(
        store
            .load_catalog("catalog", hostile.clone())
            .await
            .expect("bound identity"),
        hostile
            .selection("catalog")
            .expect("selection")
            .memory_page(&workspace)
            .expect("memory identity")
    );
    assert!(store
        .load_catalog("missing", CatalogQuery::default())
        .await
        .expect("missing tenant")["loads"]
        .as_array()
        .expect("rows")
        .is_empty());
    let keys = [("viewer", Role::Viewer), ("carrier", Role::Carrier)]
        .into_iter()
        .map(|(key, role)| {
            (
                key.to_string(),
                Credential::Scoped(Principal {
                    tenant_id: "catalog".into(),
                    subject: format!("test:{key}"),
                    role,
                    counterparty_id: (key == "carrier").then(|| "ca2".into()),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let app = api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store: store.clone(),
        keys: Arc::new(keys),
        demo: false,
        integrations: Arc::new(Integrations::default()),
    });
    for (key, expected_status) in [("viewer", 200), ("carrier", 403), ("unknown", 401)] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/loads?status=planned&customer_id=c2&carrier_id=ca2&q=100%25_&limit=7")
                    .header("authorization", format!("Bearer {key}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status().as_u16(), expected_status);
        if expected_status == 200 {
            let body: Value = serde_json::from_slice(
                &response
                    .into_body()
                    .collect()
                    .await
                    .expect("body")
                    .to_bytes(),
            )
            .expect("JSON");
            let expected = CatalogQuery {
                status: Some(Status::Planned),
                customer_id: Some("c2".into()),
                carrier_id: Some("ca2".into()),
                q: Some("100%_".into()),
                limit: Some(7),
                ..Default::default()
            }
            .selection("catalog")
            .expect("selection")
            .memory_page(&workspace)
            .expect("memory HTTP page");
            assert_eq!(
                body, expected,
                "HTTP decoding and actual PostgreSQL bindings"
            );
        }
    }
    drop(app);
    let after: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT tenant_id,xmin::text,updated_at::text,data::text FROM freight_workspaces ORDER BY tenant_id"
    ).fetch_all(&pool).await.expect("retained source");
    assert_eq!(
        before, after,
        "catalog reads must not rewrite source records"
    );
    println!(
        "CATALOG_FILTER_PLANS={}",
        json!({"rows_per_tenant":1000,"matching_rows_per_filtered_tenant":50,
        "tenant_count":2,"memory_page_parity_checks":parity_checks,"http_checks":3,"plans":plans,"plan_failures":failures,
        "fixture_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(&raw).expect("fixture JSON")))})
    );
    drop(store);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("clean schema");
    admin.close().await;
    assert!(
        failures.is_empty(),
        "selective catalog filters regressed: {failures:?}"
    );
}
