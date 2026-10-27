//! Actual executables, OS signals, unfinished HTTP and durable lease recovery.
#![cfg(unix)]
use axum::{extract::Path, http::HeaderMap, routing::get, Json, Router};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use stateset_freight::{domain::Status, store::Store};
use std::{
    fs::{self, File},
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    sync::{Mutex, Semaphore},
};

const SECRET: &str = "owned-shutdown-callback-secret-at-least-32-characters";

struct Process {
    child: Child,
    directory: PathBuf,
    url: String,
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
impl Process {
    fn start(integrations: Value, timeout_ms: &str, database: Option<&str>) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "freight-shutdown-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir(&directory).expect("owned directory");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("private directory");
        fs::write(directory.join(".env"), "# owned empty environment\n").expect("dotenv");
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("owned port");
        let address = reservation.local_addr().expect("address");
        drop(reservation);
        let log = File::create(directory.join("process.log")).expect("log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_stateset-freight"));
        command
            .env_clear()
            .current_dir(&directory)
            .env("FREIGHT_DEMO", "true")
            .env("FREIGHT_BIND_ADDR", address.to_string())
            .env("FREIGHT_API_KEYS_JSON", "{}")
            .env("FREIGHT_INTEGRATIONS_JSON", integrations.to_string())
            .env("FREIGHT_INTEGRATION_CONCURRENCY", "1")
            .env("FREIGHT_SHUTDOWN_TIMEOUT_MS", timeout_ms)
            .env("RUST_LOG", "stateset_freight=info")
            .env("TOKIO_WORKER_THREADS", "2")
            .stdout(Stdio::from(log.try_clone().expect("log clone")))
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        if let Some(database) = database {
            command.env("DATABASE_URL", database);
        }
        let child = command.spawn().expect("actual Freight executable");
        Self {
            child,
            directory,
            url: format!("http://{address}"),
        }
    }
    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("process.log")).expect("process log")
    }
    async fn ready(&mut self) {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client");
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    self.child.try_wait().expect("status").is_none(),
                    "Premature exit: {}",
                    self.log()
                );
                if client
                    .get(format!("{}/health", self.url))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("Freight ready: {}", self.log()));
    }
    async fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().expect("live pid").to_string())
            .status()
            .await
            .expect("signal");
        assert!(status.success());
    }
    async fn draining(&mut self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !self.log().contains("Freight drain started") {
                assert!(
                    self.child.try_wait().expect("status").is_none(),
                    "Exit before drain: {}",
                    self.log()
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("observed process drain");
    }
    async fn wait(&mut self) -> ExitStatus {
        tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .unwrap_or_else(|_| panic!("Bounded process exit: {}", self.log()))
            .expect("exit status")
    }
}

struct Peer {
    url: String,
    calls: Arc<Mutex<Vec<String>>>,
    release: Arc<Semaphore>,
    document_gates: Arc<Mutex<Vec<Arc<Semaphore>>>>,
    entered: Arc<Semaphore>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new() -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Semaphore::new(0));
        let entered = Arc::new(Semaphore::new(0));
        let document_gates = Arc::new(Mutex::new(Vec::new()));
        let documents = get({
            let calls = calls.clone();
            let document_gates = document_gates.clone();
            let entered = entered.clone();
            move |Path(document): Path<String>, headers: HeaderMap| {
                let calls = calls.clone();
                let document_gates = document_gates.clone();
                let entered = entered.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer owned-edi-key");
                    assert_eq!(headers["x-tenant-id"], "owned-edi-tenant");
                    let release = Arc::new(Semaphore::new(0));
                    document_gates.lock().await.push(release.clone());
                    calls.lock().await.push(document.clone());
                    entered.add_permits(1);
                    release.acquire().await.expect("release document").forget();
                    Json(
                        json!({"data":{"document":{"id":document,"partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted","processed_at":Utc::now()},"parsed":{"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":"A"}}}),
                    )
                }
            }
        });
        let orders = get({
            let calls = calls.clone();
            let release = release.clone();
            let entered = entered.clone();
            move |headers: HeaderMap| {
                let calls = calls.clone();
                let release = release.clone();
                let entered = entered.clone();
                async move {
                    assert_eq!(headers["x-stateset-api-key"], "owned-sync-key");
                    calls.lock().await.push("orders".into());
                    entered.add_permits(1);
                    release.acquire().await.expect("release orders").forget();
                    Json(json!({"data":{"items":[{"id":"original-order-marker"}],"page":1}}))
                }
            }
        });
        let router = Router::new()
            .route("/v1/documents/{document}", documents)
            .route("/v1/tenants/owned-sync-tenant/orders", orders);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("owned peer");
        let url = format!("http://{}", listener.local_addr().expect("peer address"));
        let task = tokio::spawn(async move { axum::serve(listener, router).await.expect("peer") });
        Self {
            url,
            calls,
            release,
            document_gates,
            entered,
            task,
        }
    }
    fn integrations(&self) -> Value {
        json!({"demo":{"edi_url":self.url,"edi_token":"owned-edi-key","edi_tenant_id":"owned-edi-tenant","callback_keys":{"1":SECRET},"sync_url":self.url,"sync_api_key":"owned-sync-key","sync_tenant_id":"owned-sync-tenant"}})
    }
    async fn request_entered(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.entered.acquire())
            .await
            .expect("request reached actual peer")
            .expect("peer entered")
            .forget();
    }
}

#[tokio::test]
async fn actual_process_handles_sigterm_and_sigint_with_successful_idle_drain() {
    for signal in ["TERM", "INT"] {
        let mut process = Process::start(json!({}), "3000", None);
        process.ready().await;
        process.signal(signal).await;
        assert!(process.wait().await.success(), "{}", process.log());
        assert!(process.log().contains("Freight drain completed"));
    }
}

#[tokio::test]
async fn actual_process_finishes_admitted_sync_http_request_during_sigterm_drain() {
    let peer = Peer::new().await;
    let mut process = Process::start(peer.integrations(), "3000", None);
    process.ready().await;
    let url = format!("{}/v1/sync/orders", process.url);
    let request = tokio::spawn(async move {
        reqwest::Client::new()
            .get(url)
            .bearer_auth("freight-local-demo")
            .send()
            .await
            .expect("admitted response")
            .json::<Value>()
            .await
            .expect("original response")
    });
    peer.request_entered().await;
    process.signal("TERM").await;
    process.draining().await;
    assert!(process.child.try_wait().expect("status").is_none());
    assert!(!request.is_finished());
    let later = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .expect("client")
        .get(format!("{}/health", process.url))
        .send()
        .await;
    assert!(
        later.is_err()
            || later
                .as_ref()
                .is_ok_and(|response| response.status().as_u16() == 503),
        "draining process cannot advertise readiness"
    );
    peer.release.add_permits(1);
    let result = request.await.expect("original request task");
    assert_eq!(result["items"][0]["id"], "original-order-marker");
    assert!(process.wait().await.success(), "{}", process.log());
    assert!(process.log().contains("Freight drain completed"));
    assert_eq!(peer.calls.lock().await.as_slice(), ["orders"]);
}

#[tokio::test]
async fn actual_process_exits_nonzero_at_deadline_with_an_unfinished_http_response() {
    let peer = Peer::new().await;
    let mut process = Process::start(peer.integrations(), "100", None);
    process.ready().await;
    let url = format!("{}/v1/sync/orders", process.url);
    let request = tokio::spawn(async move {
        reqwest::Client::new()
            .get(url)
            .bearer_auth("freight-local-demo")
            .send()
            .await
    });
    peer.request_entered().await;
    process.signal("TERM").await;
    assert_eq!(process.wait().await.code(), Some(1), "{}", process.log());
    assert!(process.log().contains("Freight drain deadline exceeded"));
    assert!(!process.log().contains("Freight drain completed"));
    assert!(
        request.await.expect("request task").is_err(),
        "unfinished response cannot become a successful response"
    );
    peer.release.add_permits(1);
}

#[tokio::test]
async fn actual_process_cancels_startup_when_sigterm_arrives_during_database_connection() {
    let stalled = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned stalled database");
    let database = format!(
        "postgres://owned@{}/owned",
        stalled.local_addr().expect("address")
    );
    let mut process = Process::start(json!({}), "100", Some(&database));
    let (_connection, _) = tokio::time::timeout(Duration::from_secs(5), stalled.accept())
        .await
        .expect("database connection started")
        .expect("connection");
    process.signal("TERM").await;
    assert!(process.wait().await.success(), "{}", process.log());
    assert!(process
        .log()
        .contains("Freight startup cancelled before services started"));
    assert!(!process.log().contains("StateSet Freight listening"));
}

#[tokio::test]
async fn actual_process_rejects_invalid_drain_configuration_before_opening_storage() {
    for raw in ["", "0", "300001", " 1", "+1", "1.5"] {
        let mut process = Process::start(json!({}), raw, Some("deliberately-invalid-database-url"));
        assert_eq!(process.wait().await.code(), Some(1));
        assert!(
            process
                .log()
                .contains("FREIGHT_SHUTDOWN_TIMEOUT_MS must be an integer from 1 to 300000"),
            "{}",
            process.log()
        );
    }
}

async fn callback(process: &Process) {
    let note = json!({"type":"document.recorded","tenant_id":"owned-edi-tenant","document_id":"original-990","partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted"}).to_string();
    let timestamp = Utc::now().timestamp().to_string();
    let delivery = "original-shutdown-delivery";
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).expect("HMAC");
    mac.update(format!("{timestamp}.{delivery}.{note}").as_bytes());
    let response = reqwest::Client::new()
        .post(format!("{}/webhooks/edi/demo", process.url))
        .header("content-type", "application/json")
        .header("x-stateset-timestamp", timestamp)
        .header("x-stateset-event-id", delivery)
        .header("x-stateset-key-version", "1")
        .header(
            "x-stateset-signature",
            format!("v1={}", hex::encode(mac.finalize().into_bytes())),
        )
        .body(note)
        .send()
        .await
        .expect("callback");
    assert_eq!(
        response.status().as_u16(),
        200,
        "callback is durably acknowledged"
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL; waits for the real 60-second durable lease"]
async fn postgres_forced_process_drain_retains_original_callback_and_restart_waits_for_lease_then_applies_once(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("shutdown_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut database = reqwest::Url::parse(&base).expect("url");
    database
        .query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let peer = Peer::new().await;
    let mut process = Process::start(peer.integrations(), "100", Some(database.as_str()));
    process.ready().await;
    let store = Store::new(Some(database.as_str()))
        .await
        .expect("independent persisted inspection");
    callback(&process).await;
    peer.request_entered().await;
    let original = store.read("demo").await.expect("original lease").inbox[0].clone();
    assert_eq!(original.status, "processing");
    assert_eq!(original.total_attempts, 1);
    assert!(original.lease.is_some());
    assert!(original.authentication.is_some());
    process.signal("TERM").await;
    assert_eq!(process.wait().await.code(), Some(1), "{}", process.log());
    let retained = store.read("demo").await.expect("retained lease").inbox[0].clone();
    assert_eq!(
        json!(retained),
        json!(original),
        "forced shutdown must not invent a partner failure or clear the lease"
    );
    let mut restarted = Process::start(peer.integrations(), "3000", Some(database.as_str()));
    restarted.ready().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        peer.calls.lock().await.len(),
        1,
        "new process cannot reclaim an unexpired lease"
    );
    assert_eq!(
        json!(store.read("demo").await.expect("unexpired").inbox[0]),
        json!(original)
    );
    // No fixture clock or SQL edit: the actual persisted 60-second lease expires.
    tokio::time::timeout(Duration::from_secs(70), peer.entered.acquire())
        .await
        .expect("real lease recovery request")
        .expect("recovery")
        .forget();
    let reclaimed = store.read("demo").await.expect("reclaimed").inbox[0].clone();
    assert!(Utc::now() >= original.leased_until.expect("original expiry"));
    assert_eq!(reclaimed.id, original.id);
    assert_eq!(reclaimed.original_body, original.original_body);
    assert_eq!(reclaimed.delivery_ids, original.delivery_ids);
    assert_ne!(reclaimed.lease, original.lease);
    assert_eq!(reclaimed.total_attempts, 2);
    assert_eq!(reclaimed.history[0].outcome, "lease_expired");
    assert_eq!(reclaimed.status, "processing");
    // Releasing the dead process's response must not release the new request
    // or complete the new lease. The gates belong to exact HTTP attempts.
    peer.document_gates.lock().await[0].add_permits(1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        json!(store.read("demo").await.expect("still held retry").inbox[0]),
        json!(reclaimed)
    );
    restarted.signal("TERM").await;
    restarted.draining().await;
    assert!(
        restarted.child.try_wait().expect("status").is_none(),
        "graceful drain waits for the original retry response"
    );
    peer.document_gates.lock().await[1].add_permits(1);
    assert!(restarted.wait().await.success(), "{}", restarted.log());
    let workspace = store.read("demo").await.expect("completed");
    let item = &workspace.inbox[0];
    assert_eq!(item.status, "applied");
    assert_eq!(item.id, original.id);
    assert_eq!(item.original_body, original.original_body);
    assert_eq!(json!(item.authentication), json!(original.authentication));
    assert_eq!(item.delivery_ids, original.delivery_ids);
    assert_eq!(item.total_attempts, 2);
    assert_eq!(
        item.history
            .iter()
            .filter(|h| h.outcome == "applied")
            .count(),
        1
    );
    assert!(item.lease.is_none());
    assert_eq!(workspace.loads[2].status, Status::Accepted);
    assert_eq!(
        peer.calls.lock().await.as_slice(),
        ["original-990", "original-990"]
    );
    drop(workspace);
    drop(store);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
