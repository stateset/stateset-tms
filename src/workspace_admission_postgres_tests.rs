use super::*;
use crate::store::mutation_execution_tests::WorkGate;

async fn post(f: &Fixture, key: &str, body: &Value) -> reqwest::Response {
    f.client
        .post(format!("{}/v1/customers", f.origin))
        .bearer_auth("owned-budget")
        .header("idempotency-key", key)
        .json(body)
        .send()
        .await
        .expect("HTTP")
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_shared_projection_queue_leaves_pool_and_protected_originals_available() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    let body =
        json!({"name":"Original before shared projection","contact":"original@example.test"});
    let original = f
        .store
        .mutate(
            "demo",
            "shared-original",
            "customer.create",
            "",
            body.clone(),
        )
        .await
        .expect("original");
    let before = f.physical().await;
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let reader_store = f.store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        reader_store
            .read_project("demo", move |workspace| {
                entered.send(()).expect("projection observed");
                worker_gate.hold();
                Ok(json!({"loads":workspace.loads.len()}))
            })
            .await
    });
    observed
        .await
        .expect("actual SQL/decode completed; projection held");
    reader.abort();
    assert!(reader.await.expect_err("caller cancelled").is_cancelled());
    let writer_store = f.store.clone();
    let writer = tokio::spawn(async move {
        writer_store
            .update("unrelated", |workspace| {
                workspace.record("owned", "test", "Only after projection ends");
                Ok(Value::Null)
            })
            .await
    });
    let admission = f.store.workspace_admission.as_ref().expect("configured");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while admission.pending() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "writer reached actual shared queue"
        );
        tokio::task::yield_now().await;
    }
    let pool = f.store.pool.as_ref().expect("pool");
    let one = tokio::time::timeout(Duration::from_millis(250), pool.acquire())
        .await
        .expect("first remains free")
        .expect("connection");
    let two = tokio::time::timeout(Duration::from_millis(250), pool.acquire())
        .await
        .expect("second remains free")
        .expect("connection");
    drop((one, two));
    assert_eq!(f.get("/live").await.status(), 200);
    assert_eq!(f.get("/ready").await.status(), 200);
    let replay = post(&f, "shared-original", &body).await;
    assert_eq!(replay.status(), 200);
    let replay = replay.json::<Value>().await.expect("JSON");
    assert_eq!(replay, json!({"data":original["data"],"replayed":true}));
    assert_eq!(f.physical().await, before);
    let rejected = post(
        &f,
        "must-not-bypass-full-shared-queue",
        &json!({"name":"Must not apply","contact":"refused@example.test"}),
    )
    .await;
    assert_eq!(rejected.status(), 503);
    assert_eq!(rejected.headers()["retry-after"], "1");
    assert_eq!(rejected.headers()["cache-control"], "no-store");
    assert_eq!(
        rejected.json::<Value>().await.expect("busy JSON"),
        json!({"error":{"code":"workspace_capacity_exceeded","message":WORKSPACE_BUSY}})
    );
    assert_eq!(f.physical().await, before);
    assert!(!writer.is_finished());
    gate.release();
    writer
        .await
        .expect("writer")
        .expect("progresses after actual projection");
    assert_eq!(
        f.store
            .read("unrelated")
            .await
            .expect("retained")
            .events
            .len(),
        1
    );
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_shared_write_refuses_external_row_lock_without_holding_unrelated_work() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    let original_body =
        json!({"name":"Known original during external lock","contact":"original@example.test"});
    let original = f
        .store
        .mutate(
            "demo",
            "shared-known",
            "customer.create",
            "",
            original_body.clone(),
        )
        .await
        .expect("original");
    let before = f.physical().await;
    let mut blocker = f.observer.begin().await.expect("owned blocker");
    sqlx::query("SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .fetch_one(&mut *blocker)
        .await
        .expect("actual external row lock");
    let body =
        json!({"name":"Explicit retry after reviewed lock refusal","contact":"retry@example.test"});
    Fixture::busy(post(&f, "shared-lock-refused", &body).await).await;
    assert_eq!(f.physical().await, before);
    assert!(f
        .store
        .replay("demo", "shared-lock-refused", "customer.create", "", &body)
        .await
        .expect("selected original")
        .is_none());
    let unrelated = f
        .store
        .read_project("unrelated", |workspace| {
            Ok(json!({"loads":workspace.loads.len()}))
        })
        .await
        .expect("unrelated read while original lock stays held");
    assert_eq!(
        serde_json::from_slice::<Value>(&unrelated).expect("JSON"),
        json!({"loads":0})
    );
    assert_eq!(f.get("/ready").await.status(), 200);
    let replay = post(&f, "shared-known", &original_body).await;
    assert_eq!(replay.status(), 200);
    assert_eq!(
        replay.json::<Value>().await.expect("JSON"),
        json!({"data":original["data"],"replayed":true})
    );
    assert_eq!(f.physical().await, before);
    blocker.rollback().await.expect("owned lock released");
    let accepted = post(&f, "shared-lock-refused", &body).await;
    assert_eq!(accepted.status(), 200);
    let accepted = accepted.json::<Value>().await.expect("JSON");
    assert_eq!(accepted["replayed"], false);
    let committed = f.physical().await;
    let replay = post(&f, "shared-lock-refused", &body).await;
    assert_eq!(replay.status(), 200);
    assert_eq!(
        replay.json::<Value>().await.expect("JSON"),
        json!({"data":accepted["data"],"replayed":true})
    );
    assert_eq!(f.physical().await, committed);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cancelled_apply_retains_shared_work_through_cpu_and_real_rollback() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    let before = f.physical().await;
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let store = f.store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        store
            .update("demo", move |workspace| {
                workspace.customers[0].name = "Must roll back".into();
                entered.send(()).expect("actual apply observed");
                worker_gate.hold();
                Ok(Value::Null)
            })
            .await
    });
    observed
        .await
        .expect("actual apply entered with tenant lock");
    writer.abort();
    assert!(writer.await.expect_err("caller cancelled").is_cancelled());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), f.store.workspace_work("probe"))
            .await
            .is_err(),
        "actual worker still holds shared work"
    );
    assert_eq!(f.get("/ready").await.status(), 200);
    let mut probe = f.observer.begin().await.expect("independent lock probe");
    let error = sqlx::query(
        "SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE NOWAIT",
    )
    .fetch_one(&mut *probe)
    .await
    .expect_err("caller cancellation does not release real lock");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03")
    );
    probe.rollback().await.expect("probe rollback");
    gate.release();
    f.idle().await;
    assert_eq!(f.physical().await, before);
    let later = f
        .store
        .workspace_work("probe")
        .await
        .expect("work releases after real rollback");
    drop(later);
    f.clean().await;
}
