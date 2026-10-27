use super::mutation_execution_tests::WorkGate;
use super::*;
use std::time::Duration;

#[tokio::test]
async fn catalog_progresses_before_real_snapshot_worker_is_released() {
    let mut store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("fixture");
    store.read_slots = Arc::new(Semaphore::new(1));
    store.read_turns = Arc::new(crate::read_admission::TenantReadTurns::new(
        crate::read_admission::ReadLimits {
            timeout: Duration::from_millis(80),
            ..Default::default()
        },
    ));
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let reader_store = store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        reader_store
            .read_project("demo", move |workspace| {
                entered.send(()).expect("worker entered");
                worker_gate.hold();
                Ok(json!({"loads":workspace.loads.len()}))
            })
            .await
    });
    observed
        .await
        .expect("actual snapshot materialized; CPU projection held");
    let result = tokio::time::timeout(
        Duration::from_millis(250),
        store.load_catalog(
            "demo",
            crate::load_catalog::CatalogQuery {
                limit: Some(1),
                ..Default::default()
            },
        ),
    )
    .await;
    let held = !reader.is_finished();
    // Always release the real worker before an assertion can fail in the control.
    gate.release();
    reader
        .await
        .expect("reader task")
        .expect("reader completes");
    assert!(held, "snapshot must still be held at observation");
    let page = result
        .expect("catalog must respond before release")
        .expect("catalog must not use the held snapshot queue");
    assert_eq!(page["loads"].as_array().expect("loads").len(), 1);
    assert_eq!(page["loads"][0]["id"], "load_1");
    assert!(page.get("invoices").is_none());
}
