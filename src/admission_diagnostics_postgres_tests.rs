use super::*;
use crate::store::admission_diagnostics_tests::{router, scrape};
use crate::store::mutation_execution_tests::WorkGate;

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_metrics_do_not_acquire_connections_and_remain_readable_after_pool_close() {
    let f = Fixture::new().await;
    let app = router(f.store.clone());
    let before = f.physical().await;
    let pool = f.store.pool.as_ref().expect("real PostgreSQL pool");
    let mut leases = Vec::new();
    for _ in 0..pool.options().get_max_connections() {
        leases.push(pool.acquire().await.expect("actual connection lease"));
    }
    assert_eq!(pool.size(), 2);
    assert_eq!(pool.num_idle(), 0);
    let observed = tokio::time::timeout(Duration::from_millis(200), scrape(&app)).await;
    drop(leases);
    let text =
        observed.expect("metrics cannot wait for the exhausted 250ms database acquisition budget");
    assert!(text.contains("freight_database_pool_configured 1\n"));
    assert!(text.contains("freight_database_pool_connections 2\n"));
    assert!(text.contains("freight_database_pool_idle 0\n"));
    assert!(text.contains("freight_database_pool_limit 2\n"));
    assert!(text.contains("freight_database_pool_closed 0\n"));
    assert!(!text.contains(&f.schema));
    assert!(!text.contains(&f.url));
    assert_eq!(f.physical().await, before);
    pool.close().await;
    let closed = scrape(&app).await;
    assert!(closed.contains("freight_database_pool_configured 1\n"));
    assert!(closed.contains("freight_database_pool_closed 1\n"));
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_metrics_retain_actual_write_and_work_ownership_after_caller_abort() {
    let f = Fixture::new_with_workspace(Some(1)).await;
    let app = router(f.store.clone());
    let before = f.physical().await;
    let body = json!({"name":"Cancelled metrics ownership candidate","contact":"owned-metrics@example.test"});
    let signature = fingerprint("customer.create", "", &body);
    let apply_signature = signature.clone();
    let gate = Arc::new(WorkGate::default());
    struct Release(Arc<WorkGate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release();
        }
    }
    let _release = Release(gate.clone());
    let worker_gate = gate.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let store = f.store.clone();
    let caller = tokio::spawn(async move {
        store
            .update_receipted_original(
                "demo",
                None,
                None,
                Some(("metrics-cancelled", &signature)),
                move |workspace| {
                    let response = apply(
                        workspace,
                        "metrics-cancelled",
                        &apply_signature,
                        "customer.create",
                        "",
                        body,
                        ("owned-metrics", false),
                    )?;
                    entered.send(()).expect("actual mutation observed");
                    worker_gate.hold();
                    Ok(response)
                },
            )
            .await
    });
    observed
        .await
        .expect("actual locked SQL snapshot decoded and owned CPU work entered");
    caller.abort();
    assert!(caller.await.expect_err("caller aborted").is_cancelled());
    let locked = sqlx::query(
        "SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE NOWAIT",
    )
    .fetch_optional(&f.observer)
    .await
    .expect_err("actual owner retains tenant row");
    assert_eq!(
        locked
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("55P03")
    );
    let text = tokio::time::timeout(Duration::from_millis(200), scrape(&app))
        .await
        .expect("metrics do not acquire SQL or processing work");
    gate.release();
    f.idle().await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.store.write_turns.requests() != 0
            || f.store
                .workspace_admission
                .as_ref()
                .expect("shared work")
                .diagnostics()
                .available
                != 1
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real rollback releases both owners");
    assert!(text.contains("freight_write_admission_requests 1\n"));
    assert!(text.contains("freight_admission_available_permits{lane=\"workspace\"} 0\n"));
    let returned = scrape(&app).await;
    assert!(returned.contains("freight_write_admission_requests 0\n"));
    assert!(returned.contains("freight_admission_available_permits{lane=\"workspace\"} 1\n"));
    assert_eq!(
        f.physical().await,
        before,
        "observation and abandoned apply do not change originals, receipts or physical tuples"
    );
    f.clean().await;
}
