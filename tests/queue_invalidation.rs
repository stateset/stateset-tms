//! Physical trigger observations; original queue bytes remain authoritative.
#[path = "support/canonical.rs"]
mod canonical;
use serde_json::json;
use sqlx::PgPool;
use stateset_freight::{domain, store::Store};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_block_invalidation_avoids_queue_reconstruction_and_header_writes_restore_exact_hints(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned disposable database");
    let admin = PgPool::connect(&base).await.expect("admin");
    let schema = format!("queue_invalidation_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("migrations");
    let pool = PgPool::connect(url.as_str()).await.expect("observer");
    // Restore the former trigger selection without changing function semantics
    // for its no-argument invocation. Observe real scheduler calls explicitly.
    sqlx::raw_sql("DROP TRIGGER IF EXISTS freight_integration_hint_invalidation ON freight_workspaces; DROP TRIGGER freight_integration_wakeups ON freight_workspaces; CREATE TRIGGER freight_integration_wakeups BEFORE INSERT OR UPDATE OF data,inbox_due_at,edi_due_at,sync_due_at ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION freight_refresh_integration_wakeups(); CREATE TABLE owned_queue_calls(inbound boolean,destination text); ALTER FUNCTION freight_queue_due(jsonb,boolean,text) RENAME TO owned_original_queue_due; CREATE FUNCTION freight_queue_due(queue jsonb,inbound boolean,destination text) RETURNS timestamptz LANGUAGE plpgsql AS $$ BEGIN INSERT INTO owned_queue_calls VALUES(inbound,destination); RETURN owned_original_queue_due(queue,inbound,destination); END $$;")
        .execute(&pool).await.expect("owned trace and predecessor");
    let mut workspace = domain::demo();
    workspace.outbox.clear();
    workspace.inbox.clear();
    for n in 0..130 {
        workspace.outbox.push(serde_json::from_value(json!({"id":format!("out-{n}"),"load_id":"load_1","destination":"stateset-edi","status":"pending_handoff","method":"POST","path":"/owned","payload":{"owned":true},"next_attempt_at":"2027-01-01T00:00:00Z"})).expect("outbound"));
        workspace.inbox.push(serde_json::from_value(json!({"id":format!("in-{n}"),"document_id":format!("doc-{n}"),"delivery_ids":[],"key_version":"owned","notification":{},"original_body":"owned synthetic body","fingerprint":"owned","status":"held","attempts":0,"total_attempts":0,"history":[],"received_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"})).expect("inbound"));
    }
    canonical::put_workspace(&pool, "before", &workspace)
        .await
        .expect("predecessor seed");
    let old_calls: i64 = sqlx::query_scalar("SELECT count(*) FROM owned_queue_calls")
        .fetch_one(&pool)
        .await
        .expect("old count");
    let physical = canonical::physical(&pool, "before")
        .await
        .expect("original physical records");
    sqlx::raw_sql(include_str!("../migrations/031_queue_invalidation.sql"))
        .execute(&pool)
        .await
        .expect("migration");
    assert_eq!(
        canonical::physical(&pool, "before")
            .await
            .expect("after migration"),
        physical
    );
    sqlx::query("TRUNCATE owned_queue_calls")
        .execute(&pool)
        .await
        .expect("reset trace");
    canonical::put_workspace(&pool, "after", &workspace)
        .await
        .expect("current seed");
    let new_calls: i64 = sqlx::query_scalar("SELECT count(*) FROM owned_queue_calls")
        .fetch_one(&pool)
        .await
        .expect("new count");
    assert!(old_calls > new_calls, "old={old_calls} new={new_calls}");
    assert_eq!(
        new_calls, 3,
        "Only the completed header reconstructs the three deadline projections"
    );
    let source = canonical::source(&pool, "after").await.expect("source");
    assert_eq!(source["outbox"], json!(workspace.outbox));
    assert_eq!(source["inbox"], json!(workspace.inbox));
    let expected=sqlx::query_scalar::<_,serde_json::Value>("SELECT jsonb_build_array(inbox_due_at,edi_due_at,sync_due_at) FROM freight_workspaces WHERE tenant_id='after'").fetch_one(&pool).await.expect("exact hints");
    assert!(expected[0].is_null());
    assert!(expected[1].as_str().expect("due").starts_with("2027-01-01"));
    // An attempt to hide real work by modifying a hint still recomputes it.
    sqlx::query("UPDATE freight_workspaces SET edi_due_at=NULL WHERE tenant_id='after'")
        .execute(&pool)
        .await
        .expect("hint guard");
    let hints=sqlx::query_scalar::<_,serde_json::Value>("SELECT jsonb_build_array(inbox_due_at,edi_due_at,sync_due_at) FROM freight_workspaces WHERE tenant_id='after'").fetch_one(&pool).await.expect("guarded hints");
    assert_eq!(hints, expected);
    sqlx::query("TRUNCATE owned_queue_calls")
        .execute(&pool)
        .await
        .expect("trace reset");
    sqlx::query("UPDATE freight_workspace_blocks SET payload=payload WHERE tenant_id='after' AND root='outbox'").execute(&pool).await.expect("same-cardinality block update");
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM owned_queue_calls")
        .fetch_one(&pool)
        .await
        .expect("block calls");
    assert_eq!(
        calls, 0,
        "block invalidation never reconstructs canonical queues"
    );
    let due: bool=sqlx::query_scalar("SELECT inbox_due_at='-infinity' AND edi_due_at='-infinity' AND sync_due_at='-infinity' FROM freight_workspaces WHERE tenant_id='after'").fetch_one(&pool).await.expect("conservative hints");
    assert!(due);
    // This must not take the invalidation fast path even with identical JSON.
    sqlx::query("UPDATE freight_workspaces SET data=data WHERE tenant_id='after'")
        .execute(&pool)
        .await
        .expect("final header");
    let hints=sqlx::query_scalar::<_,serde_json::Value>("SELECT jsonb_build_array(inbox_due_at,edi_due_at,sync_due_at) FROM freight_workspaces WHERE tenant_id='after'").fetch_one(&pool).await.expect("restored hints");
    assert_eq!(hints, expected);
    assert_eq!(
        canonical::source(&pool, "after")
            .await
            .expect("preserved queues"),
        source
    );
    println!("PASS queue invalidation: predecessor scheduler calls={old_calls}, current calls={new_calls}; block-only calls=0; migration preserves originals and header restores exact hints");
    pool.close().await;
    drop(store);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
