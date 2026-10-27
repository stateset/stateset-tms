use super::mutation_execution_tests::WorkGate;
use super::*;
use std::time::Duration;

fn admission(milliseconds: u64) -> Arc<WorkspaceAdmission> {
    Arc::new(WorkspaceAdmission::new(
        1,
        crate::read_admission::ReadLimits {
            timeout: Duration::from_millis(milliseconds),
            ..Default::default()
        },
    ))
}

#[tokio::test]
async fn cancelled_projection_keeps_shared_work_until_real_projection_ends_then_writer_progresses()
{
    let mut store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("fixture");
    store.workspace_admission = Some(admission(2000));
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let reader_store = store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        reader_store
            .read_project("demo", move |workspace| {
                entered.send(()).expect("observed");
                worker_gate.hold();
                Ok(json!({"loads":workspace.loads.len()}))
            })
            .await
    });
    observed.await.expect("real projection entered");
    reader.abort();
    assert!(reader.await.expect_err("abandoned caller").is_cancelled());
    let writer_store = store.clone();
    let (changed, mut change_observed) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        writer_store
            .update("other", move |workspace| {
                changed.send(()).expect("writer observed");
                workspace.record("other", "owned.test", "Only after the real projection ends");
                Ok(Value::Null)
            })
            .await
    });
    let shared = store.workspace_admission.as_ref().expect("configured");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while shared.pending() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "writer must reach shared queue"
        );
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        change_observed.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        !writer.is_finished(),
        "aborted HTTP awaiter did not free processing work"
    );
    gate.release();
    writer
        .await
        .expect("writer task")
        .expect("writer progresses");
    change_observed
        .await
        .expect("new work entered after release");
    assert_eq!(store.read("other").await.expect("original").events.len(), 1);
}

#[tokio::test]
async fn cancelled_memory_apply_keeps_shared_work_and_discards_uncommitted_candidate() {
    let mut store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("fixture");
    let original = json!(store.read("demo").await.expect("before"));
    store.workspace_admission = Some(admission(30));
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let worker_store = store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        worker_store
            .update("demo", move |workspace| {
                workspace.customers[0].name = "Must not commit".into();
                entered.send(()).expect("observed");
                worker_gate.hold();
                Ok(Value::Null)
            })
            .await
    });
    observed.await.expect("actual application entered");
    writer.abort();
    assert!(writer.await.expect_err("cancelled").is_cancelled());
    let busy = store.workspace_work("unrelated").await.err();
    assert_eq!(busy.as_deref(), Some(WORKSPACE_BUSY));
    gate.release();
    assert_eq!(
        json!(store.read("demo").await.expect("unchanged original")),
        original
    );
}

#[tokio::test]
async fn cancelled_direct_encoding_keeps_actual_worker_owned_until_failure_disposes_it() {
    let mut store = Store::new(None).await.expect("memory");
    store.seed_demo().await.expect("fixture");
    let original = json!(store.read("demo").await.expect("original"));
    store.workspace_admission = Some(admission(30));
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let reader_store = store.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        reader_store
            .read_encode("demo", move |_workspace| {
                entered.send(()).expect("worker observed");
                worker_gate.hold();
                Err("Owned direct encoder failure".into())
            })
            .await
    });
    observed.await.expect("actual encoder entered");
    reader.abort();
    assert!(reader.await.expect_err("abandoned caller").is_cancelled());
    assert_eq!(
        store.workspace_work("other").await.err().as_deref(),
        Some(WORKSPACE_BUSY)
    );
    gate.release();
    let mut released = false;
    for _ in 0..20 {
        if store.workspace_work("other").await.is_ok() {
            released = true;
            break;
        }
    }
    assert!(
        released,
        "Encoder failure must dispose the real worker lease"
    );
    assert_eq!(
        json!(store.read("demo").await.expect("retained original")),
        original
    );
}
