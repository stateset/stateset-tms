use serde_json::{json, Value};
use stateset_freight::{documents::Upload, store::Store};
#[path = "support/canonical.rs"]
mod canonical;

const PDF: &[u8] = b"%PDF-1.4\noriginal carrier proof\n%%EOF";
fn upload() -> Upload {
    Upload {
        kind: "pod".into(),
        filename: "original.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: PDF.to_vec(),
    }
}
async fn row(pool: &sqlx::PgPool) -> Value {
    let mut snapshot = canonical::physical(pool, "demo")
        .await
        .expect("physical and logical originals");
    snapshot["block_write_count"] = json!(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM receipt_block_trace"
    )
    .fetch_one(pool)
    .await
    .expect("every block INSERT/UPDATE/DELETE"));
    snapshot
}
async fn writes(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM receipt_write_trace")
        .fetch_one(pool)
        .await
        .expect("actual UPDATE trigger count")
}

async fn exercise(store: &Store, observer: Option<&sqlx::PgPool>) {
    let body =
        json!({"action":"location","expected_version":1,"location":"Original observed position"});
    let original = store
        .mutate_authorized(
            ("demo", "original-operator"),
            "observation",
            "load.command",
            "load_1",
            body.clone(),
        )
        .await
        .expect("first command");
    let original_state = json!(store.read("demo").await.expect("original state"));
    let physical = if let Some(pool) = observer {
        Some(row(pool).await)
    } else {
        None
    };
    let write_count = if let Some(pool) = observer {
        writes(pool).await
    } else {
        0
    };
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (store, body) = (store.clone(), body.clone());
        tasks.spawn(async move {
            store
                .mutate_authorized(
                    ("demo", "retrying-operator"),
                    "observation",
                    "load.command",
                    "load_1",
                    body,
                )
                .await
        });
    }
    while let Some(result) = tasks.join_next().await {
        let replay = result
            .expect("retry task")
            .expect("stale original version may replay");
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["data"], original["data"]);
    }
    assert_eq!(
        json!(store.read("demo").await.expect("after retries")),
        original_state
    );
    if let Some(pool) = observer {
        assert_eq!(
            writes(pool).await,
            write_count,
            "verified receipt retries must not issue UPDATE"
        );
        assert_eq!(
            Some(row(pool).await),
            physical,
            "row version, location, timestamp and every retained field stay original"
        );
    }
    assert!(store
        .mutate(
            "demo",
            "observation",
            "load.command",
            "load_1",
            json!({"action":"location","expected_version":1,"location":"Different request"})
        )
        .await
        .is_err());
    assert!(store
        .mutate(
            "demo",
            "fresh-stale-command",
            "load.command",
            "load_1",
            body
        )
        .await
        .is_err());

    let document = store
        .upload_document(("demo", "original-operator"), "proof", "load_7", upload())
        .await
        .expect("original bytes");
    let before_upload_retry = json!(store.read("demo").await.expect("uploaded"));
    let physical = if let Some(pool) = observer {
        Some(row(pool).await)
    } else {
        None
    };
    let write_count = if let Some(pool) = observer {
        writes(pool).await
    } else {
        0
    };
    let replay = store
        .upload_document(("demo", "retrying-operator"), "proof", "load_7", upload())
        .await
        .expect("upload receipt");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], document["data"]);
    assert_eq!(
        json!(store.read("demo").await.expect("upload retry")),
        before_upload_retry
    );
    let id = document["data"]["id"]
        .as_str()
        .expect("original document id");
    assert_eq!(
        store
            .document_content("demo", id)
            .await
            .expect("original file")
            .1,
        PDF
    );
    if let Some(pool) = observer {
        assert_eq!(
            writes(pool).await,
            write_count,
            "upload retries do not rewrite metadata"
        );
        assert_eq!(Some(row(pool).await), physical);
        let blobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM freight_document_content WHERE tenant_id='demo'",
        )
        .fetch_one(pool)
        .await
        .expect("blob count");
        assert_eq!(
            blobs, 1,
            "fresh candidate upload id must not create an orphan blob"
        );
    }

    // A duplicate document with a new delivery identity has new evidence even
    // though its wire response reports replay. It must still commit that identity.
    let notification = json!({"document_id":"retained-edi-document","type":"document.recorded"});
    let original_body = format!("  {}\n", notification);
    store
        .receive_document(
            "demo",
            "first-delivery",
            "v1",
            notification.clone(),
            original_body.clone(),
            None,
        )
        .await
        .expect("first callback");
    let write_count = if let Some(pool) = observer {
        writes(pool).await
    } else {
        0
    };
    let duplicate = store
        .receive_document(
            "demo",
            "second-delivery",
            "v1",
            notification.clone(),
            notification.to_string(),
            None,
        )
        .await
        .expect("new delivery identity");
    assert_eq!(duplicate["replayed"], true);
    let retained = store.read("demo").await.expect("retained callback");
    assert_eq!(retained.inbox.len(), 1);
    assert_eq!(
        retained.inbox[0].delivery_ids,
        ["first-delivery", "second-delivery"]
    );
    assert_eq!(retained.inbox[0].original_body, original_body);
    if let Some(pool) = observer {
        assert_eq!(
            writes(pool).await,
            write_count + 1,
            "new callback identity commits exactly one new source header"
        );
    }

    let draft = store.mutate("demo", "draft-bill", "bill.create", "", json!({"load_id":"load_6","number":"RECEIPT-BILL","due_date":chrono::Utc::now().date_naive()})).await.expect("draft bill");
    let bill_id = draft["data"]["id"].as_str().expect("bill id");
    let issue = json!({"expected_version":1});
    let posted = store
        .mutate_authorized(
            ("demo", "original-finance"),
            "posted-bill",
            "bill.issue",
            bill_id,
            issue.clone(),
        )
        .await
        .expect("post bill");
    let before_finance_retry = json!(store.read("demo").await.expect("posted subledger"));
    let physical = if let Some(pool) = observer {
        Some(row(pool).await)
    } else {
        None
    };
    let replay = store
        .mutate_authorized(
            ("demo", "retrying-finance"),
            "posted-bill",
            "bill.issue",
            bill_id,
            issue,
        )
        .await
        .expect("posted receipt");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], posted["data"]);
    assert_eq!(
        json!(store.read("demo").await.expect("financial retry")),
        before_finance_retry,
        "original journal, cents, actor and receipt remain exact"
    );
    if let Some(pool) = observer {
        assert_eq!(
            Some(row(pool).await),
            physical,
            "financial replay does not rewrite any original"
        );
    }

    let day = chrono::Utc::now().date_naive();
    let source = json!({"id":"00000000-0000-4000-8000-000000000981","orderNumber":"RECEIPT-SOURCE","status":"pending"});
    let plan = json!({"reference":"RECEIPT-IMPORT","customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Original goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":123456,"sell_cents":150000});
    let imported = store
        .mutate(
            "demo",
            "import",
            "sync.import",
            "",
            json!({"order":source,"load":plan}),
        )
        .await
        .expect("Sync import");
    let load_id = imported["data"]["id"].as_str().expect("imported load");
    let check_version = store
        .read("demo")
        .await
        .expect("source")
        .source_check(load_id)
        .expect("initial source check")
        .version;
    let first_observation = store
        .record_source(
            ("demo", "original-observer"),
            (load_id, source.clone(), check_version, chrono::Utc::now()),
            Some("source-observation"),
        )
        .await
        .expect("canonical source observation");
    let before_source_retry = json!(store.read("demo").await.expect("retained source"));
    let physical = if let Some(pool) = observer {
        Some(row(pool).await)
    } else {
        None
    };
    let mut changed = source.clone();
    changed["status"] = json!("cancelled");
    let replay = store
        .record_source(
            ("demo", "retrying-observer"),
            (load_id, changed.clone(), check_version, chrono::Utc::now()),
            Some("source-observation"),
        )
        .await
        .expect("original observation receipt");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], first_observation["data"]);
    let replay = store
        .mutate(
            "demo",
            "import",
            "sync.import",
            "",
            json!({"order":changed,"load":plan}),
        )
        .await
        .expect("original import receipt after upstream change");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], imported["data"]);
    assert_eq!(
        json!(store.read("demo").await.expect("source retries")),
        before_source_retry
    );
    if let Some(pool) = observer {
        assert_eq!(
            Some(row(pool).await),
            physical,
            "Sync and canonical-observation retries retain physical originals"
        );
    }
    assert!(store
        .read("other-tenant")
        .await
        .expect("isolated tenant")
        .receipts
        .is_empty());
}

#[tokio::test]
async fn receipt_retries_preserve_original_state_and_callback_delivery_evidence() {
    let store = Store::new(None).await.expect("memory store");
    store.seed_demo().await.expect("seed");
    exercise(&store, None).await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_receipt_retries_avoid_physical_updates_without_discarding_callback_evidence() {
    let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("receipt_test_{}", uuid::Uuid::new_v4().simple());
    eprintln!("Owned receipt regression schema: {schema}");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let store = Store::new(Some(url.as_str())).await.expect("durable store");
    store.seed_demo().await.expect("seed");
    let pool = sqlx::PgPool::connect(url.as_str()).await.expect("observer");
    sqlx::raw_sql("CREATE TABLE receipt_write_trace (id bigserial PRIMARY KEY); CREATE FUNCTION trace_receipt_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO receipt_write_trace DEFAULT VALUES; RETURN NEW; END $$; CREATE TRIGGER receipt_trace AFTER UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION trace_receipt_write(); CREATE TABLE receipt_block_trace (id bigserial PRIMARY KEY); CREATE FUNCTION trace_block_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO receipt_block_trace DEFAULT VALUES; RETURN NEW; END $$; CREATE TRIGGER receipt_block_trace AFTER INSERT OR UPDATE OR DELETE ON freight_workspace_blocks FOR EACH ROW EXECUTE FUNCTION trace_block_write()")
        .execute(&pool).await.expect("physical UPDATE observer");
    exercise(&store, Some(&pool)).await;
    drop(store);
    let restarted = Store::new(Some(url.as_str())).await.expect("restart");
    let retained = restarted.read("demo").await.expect("durable originals");
    assert_eq!(retained.loads[0].version, 2);
    assert_eq!(retained.loads[0].location, "Original observed position");
    assert_eq!(retained.inbox[0].delivery_ids.len(), 2);
    assert_eq!(retained.documents[0].uploaded_by, "original-operator");
    drop(restarted);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned schema cleanup");
    admin.close().await;
}
