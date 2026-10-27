use super::*;
use crate::store::mutation_execution_tests::WorkGate;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_catalog_http_progresses_while_actual_snapshot_cpu_is_held() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    let warm = f.get("/v1/loads?limit=1").await;
    assert_eq!(warm.status(), 200);
    let expected = warm.json::<Value>().await.expect("catalog");
    let before = f.physical().await;
    let gate = Arc::new(WorkGate::default());
    let held = gate.clone();
    let store = f.store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        store
            .read_project("demo", move |w| {
                entered.send(()).expect("entered");
                held.hold();
                Ok(json!({"loads":w.loads.len()}))
            })
            .await
    });
    observed
        .await
        .expect("real SQL snapshot committed; CPU still holds snapshot permit");
    let result = tokio::time::timeout(Duration::from_millis(500), f.get("/v1/loads?limit=1")).await;
    let still_held = !worker.is_finished();
    gate.release();
    worker.await.expect("worker task").expect("worker");
    assert!(still_held);
    let response = result.expect("catalog must complete before CPU release");
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.expect("catalog"), expected);
    assert_eq!(f.physical().await, before);
    assert_eq!(f.get("/ready").await.status(), 200);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_abandoned_catalog_retains_lane_through_sql_timeout_and_actual_rollback() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    assert_eq!(f.get("/v1/loads?limit=1").await.status(), 200);
    let before = f.physical().await;
    let mut blocker = f.observer.begin().await.expect("external blocker");
    sqlx::query("LOCK freight_load_catalog IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("only owned catalog locked");
    let store = f.store.clone();
    let caller = tokio::spawn(async move {
        store
            .load_catalog(
                "demo",
                crate::load_catalog::CatalogQuery {
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE '%freight_ensure_load_catalog%')").bind(&f.app).fetch_one(&f.admin).await.expect("real blocked catalog query");
        if blocked {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual query must enter SQL before caller abort"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    caller.abort();
    assert!(caller.await.expect_err("caller abandoned").is_cancelled());
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            f.store.catalog_admission.acquire("other")
        )
        .await
        .is_err(),
        "abandoned caller cannot release the real SQL lane"
    );
    assert_eq!(
        f.store.catalog_admission.pending(),
        0,
        "cancelled queue reservation reclaimed"
    );
    let spare = tokio::time::timeout(
        Duration::from_millis(100),
        f.store.pool.as_ref().expect("pool").acquire(),
    )
    .await
    .expect("second connection remains free")
    .expect("connection");
    drop(spare);
    assert_eq!(f.get("/ready").await.status(), 200);
    // Default owned lock budget is200ms: it must roll back while blocker remains held.
    f.idle().await;
    let lane = tokio::time::timeout(
        Duration::from_millis(250),
        f.store.catalog_admission.acquire("after-rollback"),
    )
    .await
    .expect("real rollback frees catalog lane")
    .expect("lane");
    drop(lane);
    assert_eq!(f.physical().await, before);
    blocker.rollback().await.expect("release external lock");
    assert_eq!(f.get("/v1/loads?limit=1").await.status(), 200);
    assert_eq!(f.physical().await, before);
    f.clean().await;
}
