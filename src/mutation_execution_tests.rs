use super::*;
use crate::{api, integrations::Integrations};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{mpsc, Condvar, Mutex},
    time::Duration,
};

#[derive(Default)]
pub(super) struct WorkGate {
    released: Mutex<bool>,
    changed: Condvar,
}
impl WorkGate {
    pub(super) fn hold(&self) {
        let released = self.released.lock().expect("gate");
        let (_released, timeout) = self
            .changed
            .wait_timeout_while(released, Duration::from_secs(8), |value| !*value)
            .expect("bounded work");
        assert!(!timeout.timed_out(), "controller must release owned work");
    }
    pub(super) fn release(&self) {
        *self.released.lock().expect("gate") = true;
        self.changed.notify_all();
    }
}

#[tokio::test]
async fn cancelled_memory_apply_keeps_admission_until_work_ends_and_discards_candidate() {
    let store = Store::new(None).await.expect("memory store");
    store.seed_demo().await.expect("owned fixture");
    let original = json!(store.read("demo").await.expect("original"));
    let gate = Arc::new(WorkGate::default());
    let worker_gate = gate.clone();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let worker_store = store.clone();
    let writer = tokio::spawn(async move {
        worker_store
            .update("demo", move |workspace| {
                workspace.customers[0].name = "Must never replace the original".into();
                entered.send(()).expect("actual apply entered");
                worker_gate.hold();
                Ok(Value::Null)
            })
            .await
    });
    observed.await.expect("actual apply observed");
    writer.abort();
    let cancelled = writer
        .await
        .expect_err("caller cancelled independently of CPU work");
    assert!(cancelled.is_cancelled());
    assert!(
        store.memory.try_write().is_err(),
        "work still owns write admission"
    );
    gate.release();
    let retained = tokio::time::timeout(Duration::from_secs(3), store.read("demo"))
        .await
        .expect("real work releases admission")
        .expect("original retained");
    assert_eq!(json!(retained), original);
    store
        .update("demo", |workspace| {
            workspace.customers[0].name = "Explicit later update".into();
            Ok(Value::Null)
        })
        .await
        .expect("subsequent writer progresses");
    assert_eq!(
        store.read("demo").await.expect("later source").customers[0].name,
        "Explicit later update"
    );
}

fn probe(address: SocketAddr, path: &str) -> Result<u16, String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    let mut response = [0; 1024];
    let count = stream
        .read(&mut response)
        .map_err(|error| error.to_string())?;
    std::str::from_utf8(&response[..count])
        .map_err(|error| error.to_string())?
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "missing HTTP status".to_string())?
        .parse()
        .map_err(|error: std::num::ParseIntError| error.to_string())
}

#[test]
fn held_mutation_keeps_http_probes_available_with_one_async_worker() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("one async worker");
    runtime.block_on(async {
        let store = Store::new(None).await.expect("memory store");
        store.seed_demo().await.expect("owned fixture");
        let app = api::router(api::App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: store.clone(),
            demo: false,
            integrations: Arc::new(Integrations::default()),
            keys: Arc::new(BTreeMap::from([("owned-mutation".into(), "demo".into())])),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let ready = tokio::task::spawn_blocking(move || probe(address, "/ready"))
            .await
            .expect("probe thread");
        assert_eq!(ready, Ok(200));
        let gate = Arc::new(WorkGate::default());
        let controller_gate = gate.clone();
        let writer_gate = gate.clone();
        let (entered, observed) = mpsc::channel();
        let controller = std::thread::spawn(move || {
            let entry = observed.recv_timeout(Duration::from_secs(5));
            let probes = if entry.is_ok() {
                (probe(address, "/live"), probe(address, "/ready"))
            } else {
                (
                    Err("mutation not observed".into()),
                    Err("mutation not observed".into()),
                )
            };
            controller_gate.release();
            probes
        });
        let writer_store = store.clone();
        let writer = tokio::spawn(async move {
            writer_store
                .update("demo", move |workspace| {
                    workspace.customers[0].name = "Original held mutation".into();
                    entered.send(()).expect("observe actual apply");
                    writer_gate.hold();
                    Ok(json!({"applied":true}))
                })
                .await
        });
        let probes = tokio::task::spawn_blocking(move || controller.join().expect("controller"))
            .await
            .expect("controller join");
        let response = writer.await.expect("writer task").expect("write");
        let saved = store.read("demo").await.expect("retained write");
        server.abort();
        let _ = server.await;
        assert_eq!(response, json!({"applied":true}));
        assert_eq!(saved.customers[0].name, "Original held mutation");
        assert_eq!(probes.0, Ok(200), "liveness while actual mutation is held");
        assert_eq!(probes.1, Ok(200), "readiness while actual mutation is held");
    });
}
