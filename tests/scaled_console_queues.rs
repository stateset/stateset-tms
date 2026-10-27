//! The full mixed fixture must qualify; clearing queues hides ingestion costs.
#[path = "support/canonical.rs"]
mod canonical;
use serde_json::{json, Value};
use sqlx::PgPool;
use stateset_freight::{
    auth::{Principal, Role},
    console_catalog::Query,
    domain,
    store::Store,
};
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_mixed_financial_and_queue_ingestion_preserves_originals_and_bounded_console_pages(
) {
    if std::env::var_os("FREIGHT_PROFILE_CONSOLE").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("stateset_freight::store::console_store=debug")
            .with_ansi(false)
            .try_init();
    }
    let base = std::env::var("TEST_DATABASE_URL").expect("owned disposable database");
    let admin = PgPool::connect(&base).await.expect("admin");
    let schema = format!("scaled_console_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("migrations");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET statement_timeout='30s'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url.as_str())
        .await
        .expect("bounded fixture pool");
    let mut workspace = domain::demo();
    workspace.bills.clear();
    workspace.settlements.clear();
    workspace.outbox.clear();
    workspace.inbox.clear();
    workspace.cash_entries.clear();
    workspace.credits.clear();
    workspace.carrier_credit_applications.clear();
    workspace.carrier_adjustments.clear();
    let load = workspace
        .loads
        .iter()
        .find(|load| load.id == "load_6")
        .expect("load")
        .clone();
    for n in 0..5000 {
        workspace.bills.push(
            serde_json::from_value(json!({
                "id":format!("bill-{n:05}"),"number":format!("INV-{n:05}"),
                "load_id":load.id,"customer_id":load.customer_id,
                "snapshot":{"customer":workspace.customers[0],"load_reference":load.reference,
                    "origin":load.origin,"destination":load.destination,"commodity":load.commodity,
                    "amount_cents":150000,"currency":"USD","source_load_version":1},
                "due_date":"2010-01-02","status":"issued","version":1,
                "created_at":"2010-01-01T00:00:00Z","created_by":"fixture:finance",
                "issued_at":"2010-01-01T00:00:00Z"
            }))
            .expect("bill"),
        );
        workspace.settlements.push(
            serde_json::from_value(json!({"id":format!("settlement-{n:05}"),
            "load_id":load.id,"carrier_id":load.carrier_id,"amount_cents":118000,
            "status":"prepared","version":1}))
            .expect("settlement"),
        );
        workspace.outbox.push(
            serde_json::from_value(json!({"id":format!("queue-{n:05}"),
            "load_id":load.id,"destination":"stateset-edi","status":"pending_handoff",
            "method":"POST","path":"/owned","payload":{"private":"PRIVATE-QUEUE-PAYLOAD"},
            "next_attempt_at":"2027-01-01T00:00:00Z"}))
            .expect("outbound"),
        );
        workspace.inbox.push(
            serde_json::from_value(json!({"id":format!("queue-{n:05}"),
            "document_id":format!("doc-{n}"),"delivery_ids":[],"key_version":"owned",
            "notification":{},"original_body":"PRIVATE-QUEUE-PAYLOAD","fingerprint":"owned",
            "status":"held","attempts":0,"total_attempts":0,"history":[],
            "received_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}))
            .expect("inbound"),
        );
    }
    let start = Instant::now();
    canonical::put_workspace(&pool, "scaled", &workspace)
        .await
        .expect("complete mixed ingestion");
    let ingestion_ms = start.elapsed().as_millis();
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "mixed ingestion exceeded its predeclared 30s diagnostic budget"
    );
    let originals = canonical::physical(&pool, "scaled")
        .await
        .expect("original physical state");
    let actor = Principal {
        tenant_id: "scaled".into(),
        subject: "fixture:viewer".into(),
        role: Role::Viewer,
        counterparty_id: None,
    };
    let mut reads = Vec::new();
    for (view, collection, expected) in [
        ("billing", "bills", 5000),
        ("payables", "settlements", 5000),
        ("integrations", "outbox", 10000),
    ] {
        let start = Instant::now();
        let query: Query = serde_json::from_value(json!({"view":view,"limit":50})).expect("query");
        let page = store
            .console_page("scaled", &actor, query)
            .await
            .expect("bounded page");
        assert_eq!(page["total"], expected);
        let rows = page[collection].as_array().expect("rows").len()
            + if view == "integrations" {
                page["inbox"].as_array().expect("inbox").len()
            } else {
                0
            };
        assert_eq!(rows, 50);
        assert!(!page.to_string().contains("PRIVATE-QUEUE-PAYLOAD"));
        reads.push(json!({"view":view,"elapsed_ms":start.elapsed().as_millis(),"total":expected,"page_rows":rows}));
    }
    let search: Query =
        serde_json::from_value(json!({"view":"billing","q":"INV-04999","limit":50}))
            .expect("search");
    let page = store
        .console_page("scaled", &actor, search)
        .await
        .expect("literal search");
    assert_eq!(page["total"], 1);
    assert_eq!(page["bills"][0]["id"], "bill-04999");
    assert_eq!(
        canonical::physical(&pool, "scaled")
            .await
            .expect("after read projection"),
        originals
    );
    let result: Value = json!({"records_per_collection":5000,"ingestion_ms":ingestion_ms,"reads":reads,"originals_preserved":true,"production_capacity_qualified":false});
    println!("PASS complete 20000-record mixed console fixture: {result}");
    pool.close().await;
    drop(store);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
