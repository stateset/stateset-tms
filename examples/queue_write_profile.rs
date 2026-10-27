//! Owned diagnostic workload for canonical queue writes; not a capacity claim.
#[path = "../tests/support/canonical.rs"]
mod canonical;
use serde_json::json;
use sqlx::PgPool;
use stateset_freight::{domain, store::Store};
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("TEST_DATABASE_URL")
        .map_err(|_| "Set TEST_DATABASE_URL to an owned disposable PostgreSQL database")?;
    let admin = PgPool::connect(&base).await?;
    let schema = format!("queue_profile_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await?;
    let mut url = reqwest::Url::parse(&base)?;
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"))
        .append_pair("application_name", &schema);
    let store = Store::new(Some(url.as_str())).await?;
    // Match the serving connection's default statement budget on every connection.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET statement_timeout='30s'")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("SET track_functions='pl'")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect(url.as_str())
        .await?;
    let version: String = sqlx::query_scalar("SHOW server_version")
        .fetch_one(&pool)
        .await?;
    let mut results = Vec::new();
    let mut failed = false;
    for count in [123, 1000, 5000] {
        let mut workspace = domain::demo();
        workspace.outbox.clear();
        workspace.inbox.clear();
        for n in 0..count {
            workspace.outbox.push(serde_json::from_value(json!({
                "id":format!("out-{n}"),"load_id":"load_1","destination":"stateset-edi",
                "status":"pending_handoff","method":"POST","path":"/owned",
                "payload":{"owned":true},"next_attempt_at":"2027-01-01T00:00:00Z"
            }))?);
            workspace.inbox.push(serde_json::from_value(json!({
                "id":format!("in-{n}"),"document_id":format!("doc-{n}"),"delivery_ids":[],
                "key_version":"owned","notification":{},"original_body":"owned synthetic body",
                "fingerprint":"owned","status":"held","attempts":0,"total_attempts":0,
                "history":[],"received_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"
            }))?);
        }
        let tenant = format!("queue-{count}");
        let start = Instant::now();
        let outcome = canonical::put_workspace(&pool, &tenant, &workspace).await;
        let millis = start.elapsed().as_millis();
        let passed = outcome.is_ok();
        // Error codes convey the failure without connection credentials or bodies.
        let error_code = outcome
            .as_ref()
            .err()
            .and_then(sqlx::Error::as_database_error)
            .and_then(|error| error.code())
            .map(|code| code.into_owned());
        results.push(json!({"records_per_direction":count,"elapsed_ms":millis,
                            "passed":passed,"database_error_code":error_code}));
        if !passed {
            failed = true;
            break;
        }
        let actual = canonical::source(&pool, &tenant).await?;
        assert_eq!(actual["outbox"], json!(workspace.outbox));
        assert_eq!(actual["inbox"], json!(workspace.inbox));
        let hints: (bool, bool) = sqlx::query_as("SELECT inbox_due_at IS NULL,edi_due_at='2027-01-01T00:00:00Z'::timestamptz FROM freight_workspaces WHERE tenant_id=$1")
            .bind(&tenant).fetch_one(&pool).await?;
        assert_eq!(hints, (true, true));
    }
    pool.close().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let functions: serde_json::Value = sqlx::query_scalar("SELECT coalesce(jsonb_agg(jsonb_build_object('function',funcname,'calls',calls,'total_ms',total_time,'self_ms',self_time) ORDER BY funcname),'[]'::jsonb) FROM pg_stat_user_functions WHERE schemaname=$1")
        .bind(&schema).fetch_one(&admin).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"schema":schema,"postgres_version":version,
        "statement_budget_ms":30000,"results":results,"capacity_qualified":false,"function_profile":functions}))?
    );
    drop(store);
    if failed {
        return Err("Queue write profile failed; owned schema retained for inspection".into());
    }
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await?;
    admin.close().await;
    Ok(())
}
