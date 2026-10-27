use super::*;

async fn physical(
    pool: &PgPool,
) -> (
    Vec<(String, String, String, String)>,
    Vec<(String, String, String, String, String)>,
) {
    (
        sqlx::query_as("SELECT tenant_id,xmin::text,ctid::text,md5(data::text) FROM freight_workspaces ORDER BY tenant_id")
            .fetch_all(pool).await.expect("exact header tuples"),
        sqlx::query_as("SELECT tenant_id,root,key,xmin::text,ctid::text||':'||md5(payload::text) FROM freight_workspace_blocks ORDER BY tenant_id,root,key")
            .fetch_all(pool).await.expect("exact block tuples"),
    )
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cached_reads_fetch_current_blocks_keep_originals_and_never_certify_corrupt_writes(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned PostgreSQL");
    let admin = PgPool::connect(&base).await.expect("admin");
    let schema = format!("validated_read_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("owned schema");
    let mut url = reqwest::Url::parse(&base).expect("URL");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let store = Store::new(Some(url.as_str())).await.expect("real store");
    let observer = PgPool::connect(url.as_str())
        .await
        .expect("independent observer");
    let mut workspace = domain::demo();
    let mut load = workspace.loads[0].clone();
    load.source_order = Some(
        json!({"literal/雪":"owned opaque metadata ".repeat(7000),"large":u64::MAX,"negative":i64::MIN}),
    );
    workspace.loads = (0..64)
        .map(|n| {
            let mut load = load.clone();
            load.id = format!("owned-cache-{n}");
            load
        })
        .collect();
    let other = workspace.clone();
    store
        .update("owned-a", move |w| {
            *w = workspace;
            Ok(Value::Null)
        })
        .await
        .expect("owned originals");
    store
        .update("owned-b", move |w| {
            *w = other;
            w.loads[0].location = "Other tenant original".into();
            Ok(Value::Null)
        })
        .await
        .expect("other tenant");
    let before = physical(&observer).await;
    let original = json!(store.read("owned-a").await.expect("first complete read"));
    let hits = store.read_cache.hits();
    let clone = store.clone();
    assert_eq!(
        json!(clone.read("owned-a").await.expect("shared cached source")),
        original
    );
    assert_eq!(store.read_cache.hits(), hits + 1, "actual Store cache hit");
    let other = store.read("owned-b").await.expect("different tenant");
    assert_eq!(other.loads[0].location, "Other tenant original");
    assert_eq!(
        store.read_cache.hits(),
        hits + 1,
        "another tenant cannot hit the same source"
    );
    assert_eq!(
        physical(&observer).await,
        before,
        "reads never rewrite originals or receipts"
    );
    assert_eq!(
        json!(store.read("owned-a").await.expect("warm original again")),
        original
    );

    let header: (String, String, String) = sqlx::query_as(
        "SELECT data::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='owned-a'",
    )
    .fetch_one(&observer)
    .await
    .expect("header");
    let key:String=sqlx::query_scalar("SELECT key FROM freight_workspace_blocks WHERE tenant_id='owned-a' AND root='loads' ORDER BY key LIMIT 1").fetch_one(&observer).await.expect("first load block");
    let hits = store.read_cache.hits();
    sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,0,payload,location}',to_jsonb($2::text),false) WHERE tenant_id='owned-a' AND root='loads' AND key=$1")
        .bind(&key).bind("Changed block with unchanged header").execute(&observer).await.expect("block-only external change");
    let current = store
        .read("owned-a")
        .await
        .expect("fetch current block, never old cache");
    assert_eq!(store.read_cache.hits(), hits, "changed block must miss");
    assert!(current
        .loads
        .iter()
        .any(|load| load.location == "Changed block with unchanged header"));
    let current_header: (String, String, String) = sqlx::query_as(
        "SELECT data::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='owned-a'",
    )
    .fetch_one(&observer)
    .await
    .expect("same header");
    assert_eq!(
        current_header.0, header.0,
        "unchanged header data may not substitute for complete block hashes"
    );
    let payload:String=sqlx::query_scalar("SELECT payload::text FROM freight_workspace_blocks WHERE tenant_id='owned-a' AND root='loads' AND key=$1").bind(&key).fetch_one(&observer).await.expect("current original block");
    let before = physical(&observer).await;
    assert_eq!(
        json!(store.read("owned-a").await.expect("new cached original")),
        json!(current)
    );
    assert_eq!(physical(&observer).await, before);

    sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,0,position}','0'::jsonb,false) WHERE tenant_id='owned-a' AND root='loads' AND key=$1").bind(&key).execute(&observer).await.expect("invalid record identity inside valid block envelope");
    let corrupt = physical(&observer).await;
    assert!(
        store.read("owned-a").await.is_err(),
        "warm cache cannot hide corruption"
    );
    let entered = Arc::new(AtomicBool::new(false));
    let observed = entered.clone();
    assert!(
        store
            .update("owned-a", move |_| {
                observed.store(true, Ordering::SeqCst);
                Ok(Value::Null)
            })
            .await
            .is_err(),
        "mutations never trust a cached read source"
    );
    assert!(
        !entered.load(Ordering::SeqCst),
        "bad source fails before application"
    );
    assert_eq!(
        physical(&observer).await,
        corrupt,
        "failed read/write retains every corrupt original for recovery"
    );
    sqlx::query("UPDATE freight_workspace_blocks SET payload=$2::jsonb WHERE tenant_id='owned-a' AND root='loads' AND key=$1").bind(&key).bind(payload).execute(&observer).await.expect("restore exact owned bytes");
    let restored = physical(&observer).await;
    let hits = store.read_cache.hits();
    assert_eq!(
        json!(store
            .read("owned-a")
            .await
            .expect("same bytes on a new tuple")),
        json!(current)
    );
    assert_eq!(
        store.read_cache.hits(),
        hits + 1,
        "fresh tuple metadata does not prevent exact-content reuse"
    );
    assert_eq!(physical(&observer).await, restored);
    store.pool.as_ref().expect("store pool").close().await;
    observer.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned schema removed");
    admin.close().await;
}
