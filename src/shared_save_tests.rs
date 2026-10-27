use super::*;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

fn body() -> Value {
    json!({"action":"location","expected_version":1,"location":"Original shared observation"})
}
async fn save(store: &Store, key: &str) -> Result<Value, String> {
    store
        .mutate_authorized(
            ("demo", "original-operator"),
            key,
            "load.command",
            "load_1",
            body(),
        )
        .await
}
#[tokio::test]
async fn memory_shared_saves_and_originals_remain_independent_of_snapshot_capacity() {
    let mut store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let original = save(&store, "original").await.expect("apply");
    store.read_slots = Arc::new(Semaphore::new(0));
    store.read_slots.close();
    assert_eq!(
        save(&store, "original")
            .await
            .expect("original despite unavailable reads"),
        json!({"data":original["data"],"replayed":true})
    );
    let mut current = body();
    current["expected_version"] = json!(2);
    store
        .mutate("demo", "next", "load.command", "load_1", current)
        .await
        .expect("new write despite unavailable reads");
    assert!(
        store
            .replay("demo", "original", "load.command", "load_1", &body())
            .await
            .is_err(),
        "mandatory read probe retains its budget"
    );
    let memory = store.memory.read().await;
    let w = memory.get("demo").expect("source");
    assert_eq!(
        w.loads
            .iter()
            .find(|l| l.id == "load_1")
            .expect("load")
            .version,
        3
    );
    assert_eq!(w.receipts.len(), 2);
}

struct Database {
    admin: PgPool,
    pool: PgPool,
    store: Store,
    schema: String,
    app: String,
    url: String,
}
impl Database {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("shared_save_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        let app = schema.clone();
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},pg_catalog"))
            .append_pair("application_name", &app);
        let pool = PgPool::connect(url.as_str()).await.expect("observer");
        let store = Store::new(Some(url.as_str())).await.expect("store");
        store.seed_demo().await.expect("seed");
        Self {
            admin,
            pool,
            store,
            schema,
            app,
            url: url.to_string(),
        }
    }
    async fn physical(&self) -> (String, String, String, i64) {
        sqlx::query_as("SELECT xmin::text,updated_at::text,md5(data::text),history_revision FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&self.pool).await.expect("source tuple")
    }
    async fn waiting(&self, contains: &str) {
        tokio::time::timeout(Duration::from_secs(3),async {
            loop {
                let count:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND position($2 in query)>0 AND query NOT LIKE '%pg_stat_activity%'").bind(&self.app).bind(contains).fetch_one(&self.pool).await.expect("actual waiting query");
                if count>0 {break;}
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("query reaches its intended lock");
    }
    async fn clean(self) {
        self.store.pool.as_ref().expect("pool").close().await;
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("cleanup");
        self.admin.close().await;
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_shared_original_skips_tenant_lock_even_without_read_capacity() {
    let mut db = Database::new().await;
    let original = save(&db.store, "literal/\"雪{key}")
        .await
        .expect("original");
    let before = db.physical().await;
    db.store.read_slots = Arc::new(Semaphore::new(0));
    let mut owner = db.pool.begin().await.expect("writer");
    sqlx::query("SELECT data FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .execute(&mut *owner)
        .await
        .expect("source held by another writer");
    let replay = tokio::time::timeout(
        Duration::from_millis(750),
        save(&db.store, "literal/\"雪{key}"),
    )
    .await
    .expect("original must not wait for tenant lock or read queue")
    .expect("original");
    assert_eq!(replay, json!({"data":original["data"],"replayed":true}));
    let plan: Value = sqlx::query_scalar(&format!(
        "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {}",
        include_str!("conditional_save.sql")
    ))
    .bind("demo")
    .bind("literal/\"雪{key}")
    .bind(STORAGE_FORMAT.to_string())
    .fetch_one(&db.pool)
    .await
    .expect("actual hit plan while row held");
    fn locks(plan: &Value) -> Vec<i64> {
        let mut out = vec![];
        if plan["Node Type"] == "LockRows" {
            out.push(plan["Actual Loops"].as_i64().expect("loops"));
        }
        if let Some(children) = plan["Plans"].as_array() {
            for p in children {
                out.extend(locks(p));
            }
        }
        out
    }
    assert_eq!(locks(&plan[0]["Plan"]), vec![0]);
    if let Ok(path) = std::env::var("FREIGHT_CONDITIONAL_SAVE_PLAN_EVIDENCE") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"result":"pass","plan":plan,"locking_subplan_loops":0,"read_capacity_available":0,"source_unchanged":db.physical().await==before})).expect("json")).expect("evidence");
    }
    assert_eq!(db.physical().await, before);
    owner.rollback().await.expect("release writer");
    db.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_shared_miss_rechecks_a_winner_committed_after_its_statement_snapshot() {
    let db = Database::new().await;
    // Separate application stores retain the actual PostgreSQL snapshot race;
    // same-store writers now queue before taking another database connection.
    let contender = Store::new(Some(&db.url))
        .await
        .expect("independent contender");
    let contender_pool = contender.pool.as_ref().expect("contender pool").clone();
    let lock_key = uuid::Uuid::new_v4().as_u128() as i64;
    sqlx::raw_sql(&format!("CREATE TABLE shared_save_updates(n integer); CREATE FUNCTION hold_shared_application() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF freight_canonical_receipt(NEW.tenant_id,NEW.data,'racing') IS NOT NULL THEN PERFORM pg_advisory_xact_lock({lock_key}); INSERT INTO shared_save_updates VALUES(1); END IF; RETURN NEW; END $$; CREATE TRIGGER shared_save_gate BEFORE UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION hold_shared_application();")).execute(&db.pool).await.expect("owned application barrier");
    let mut owner = db.pool.begin().await.expect("barrier");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key)
        .execute(&mut *owner)
        .await
        .expect("hold application before commit");
    let store = db.store.clone();
    let first = tokio::spawn(async move { save(&store, "racing").await });
    db.waiting("UPDATE freight_workspaces SET data=$2").await;
    let second = tokio::spawn(async move { save(&contender, "racing").await });
    db.waiting("WITH original AS MATERIALIZED").await;
    owner
        .rollback()
        .await
        .expect("let real first application commit");
    let first = first.await.expect("first task").expect("first command");
    let second = second
        .await
        .expect("second task")
        .expect("retry from an earlier miss");
    assert_eq!(second, json!({"data":first["data"],"replayed":true}));
    let updates: i64 = sqlx::query_scalar("SELECT count(*) FROM shared_save_updates")
        .fetch_one(&db.pool)
        .await
        .expect("actual update count");
    assert_eq!(updates, 1);
    let w = db.store.read("demo").await.expect("current source");
    assert_eq!(
        w.loads
            .iter()
            .find(|l| l.id == "load_1")
            .expect("load")
            .version,
        2
    );
    assert_eq!(w.receipts.len(), 1);
    contender_pool.close().await;
    db.clean().await;
}
#[test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
fn postgres_abandoned_shared_decode_keeps_its_transaction_until_the_worker_finishes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let db=Database::new().await;
        let before=db.physical().await;
        let (entered,ready)=tokio::sync::oneshot::channel();
        let (release,held)=std::sync::mpsc::channel();
        let occupying=tokio::task::spawn_blocking(move||{let _=entered.send(());held.recv_timeout(Duration::from_secs(10)).expect("release blocking pool");});
        ready.await.expect("blocking worker occupied");
        let applied=Arc::new(AtomicBool::new(false));
        let flag=applied.clone();let store=db.store.clone();
        let caller=tokio::spawn(async move {
            // This is the same shared-command path; observe whether business
            // application can start before the real decoder is allowed to run.
            store.update_receipted_original("demo",None,None,Some(("cancelled","owned-signature")),move|w| {
                flag.store(true,Ordering::SeqCst);
                w.loads.clear();
                Ok(json!({"data":{"unexpected":true},"replayed":false}))
            }).await
        });
        tokio::time::timeout(Duration::from_secs(3),async {
            loop {
                let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND state='idle in transaction' AND query LIKE 'SELECT root,key,payload::text%' AND query NOT LIKE '%pg_stat_activity%'").bind(&db.app).fetch_one(&db.pool).await.expect("source query completed");
                if n>0{break;}
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("candidate query completes before decode");
        assert!(db.store.healthy().await);assert!(db.store.healthy().await);
        assert!(!applied.load(Ordering::SeqCst));
        caller.abort();let _=caller.await;
        let mut upgrade=db.pool.begin().await.expect("format upgrade probe");
        sqlx::query("SET LOCAL lock_timeout='150ms'").execute(&mut *upgrade).await.expect("finite observer wait");
        let error=sqlx::query("UPDATE freight_storage_format SET version=version WHERE singleton").execute(&mut *upgrade).await.expect_err("abandoned queued decoder still owns the format fence");
        assert_eq!(error.as_database_error().and_then(|e|e.code()).as_deref(),Some("55P03"));
        upgrade.rollback().await.expect("observer rollback");
        release.send(()).expect("finish worker");occupying.await.expect("occupying job");
        tokio::time::timeout(Duration::from_secs(3),sqlx::query("UPDATE freight_storage_format SET version=version WHERE singleton").execute(&db.pool)).await.expect("format fence released after actual work ends").expect("format update");
        assert!(!applied.load(Ordering::SeqCst));assert_eq!(db.physical().await,before);
        let original=save(&db.store,"cancelled").await.expect("exact key can apply after cancelled pre-application decode");
        assert_eq!(original["data"]["version"],2);
        db.clean().await;
    });
}

#[test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
fn postgres_abandoned_preparation_keeps_locks_and_rolls_back_source_receipt_and_blob() {
    const PDF: &[u8] = b"%PDF-1.4\noriginal prepared proof\n%%EOF";
    fn compound(w: &mut Workspace) -> Result<Value, String> {
        let signature = fingerprint("load.command", "load_1", &body());
        let response = apply(
            w,
            "prepared-original",
            &signature,
            "load.command",
            "load_1",
            body(),
            ("operator", true),
        )?;
        if response["replayed"] != true {
            w.documents.push(crate::documents::Document {
                id: "prepared-proof".into(),
                load_id: "load_1".into(),
                kind: "pod".into(),
                filename: "original.pdf".into(),
                content_type: "application/pdf".into(),
                size_bytes: PDF.len(),
                sha256: crate::documents::sha256(PDF),
                uploaded_at: chrono::Utc::now(),
                uploaded_by: "operator".into(),
                version: 1,
                reviews: vec![],
            });
        }
        Ok(response)
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let db = Database::new().await;
        let before = db.physical().await;
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, held) = std::sync::mpsc::channel();
        let (staged, applied) = tokio::sync::oneshot::channel();
        let store = db.store.clone();
        let caller = tokio::spawn(async move {
            store
                .update_receipted(
                    "demo",
                    Some(("prepared-proof".into(), PDF.to_vec())),
                    None,
                    move |w| {
                        let response = compound(w)?;
                        // Application and preparation share this owned job.
                        // Hold the real candidate after staging its changes,
                        // rather than queueing an unrelated spare worker.
                        let _ = staged.send(());
                        let _ = entered.send(());
                        held.recv_timeout(Duration::from_secs(10))
                            .expect("release actual preparation work");
                        Ok(response)
                    },
                )
                .await
        });
        applied
            .await
            .expect("real business changes and receipt staged");
        ready.await.expect("preparation worker occupied");
        assert!(
            !caller.is_finished(),
            "save must wait for real worker preparation"
        );
        assert!(db.store.healthy().await);
        caller.abort();
        let _ = caller.await;
        let mut upgrade = db.pool.begin().await.expect("upgrade probe");
        sqlx::query("SET LOCAL lock_timeout='150ms'")
            .execute(&mut *upgrade)
            .await
            .expect("finite probe");
        let error =
            sqlx::query("UPDATE freight_storage_format SET version=version WHERE singleton")
                .execute(&mut *upgrade)
                .await
                .expect_err("abandoned preparation still owns format fence");
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("55P03")
        );
        upgrade.rollback().await.expect("probe rollback");
        let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_document_content")
            .fetch_one(&db.pool)
            .await
            .expect("no early file bytes");
        assert_eq!(blobs, 0);
        release.send(()).expect("finish real preparation");
        tokio::time::timeout(
            Duration::from_secs(3),
            sqlx::query("UPDATE freight_storage_format SET version=version WHERE singleton")
                .execute(&db.pool),
        )
        .await
        .expect("format fence released after worker completion")
        .expect("format probe succeeds");
        assert_eq!(
            db.physical().await,
            before,
            "cancelled prepared changes must all roll back"
        );
        let first = db
            .store
            .update_receipted(
                "demo",
                Some(("prepared-proof".into(), PDF.to_vec())),
                None,
                compound,
            )
            .await
            .expect("exact original can apply after cancellation");
        let committed = db.physical().await;
        let replay = db
            .store
            .update_receipted(
                "demo",
                Some(("orphan-candidate".into(), PDF.to_vec())),
                None,
                compound,
            )
            .await
            .expect("exact result replays");
        assert_eq!(replay, json!({"data":first["data"],"replayed":true}));
        assert_eq!(db.physical().await, committed);
        let blobs: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_document_content")
            .fetch_one(&db.pool)
            .await
            .expect("one original file");
        assert_eq!(blobs, 1);
        assert_eq!(
            db.store
                .document_content("demo", "prepared-proof")
                .await
                .expect("retained original bytes")
                .1,
            PDF
        );
        db.clean().await;
    });
}
