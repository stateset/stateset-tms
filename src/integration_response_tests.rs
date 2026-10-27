use super::*;
use crate::domain::Status;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{Mutex, Notify},
};

// Deliberately independent of the implementation constant. Exercise actual
// framing and incomplete bodies rather than mocking reqwest's decoder.
const LIMIT: usize = 2_000_000;
const ORDER: &str = "00000000-0000-4000-8000-000000000901";
#[derive(Clone, Copy)]
enum Wire {
    Fixed,
    Chunked,
    CloseDelimited,
    OpenChunked,
    AnnouncedOnly,
    Truncated,
}
struct Reply {
    body: Vec<u8>,
    wire: Wire,
    status: u16,
}
impl Reply {
    fn new(body: Vec<u8>, wire: Wire) -> Self {
        Self {
            body,
            wire,
            status: 200,
        }
    }
}
#[derive(Debug)]
struct Request {
    headers: String,
    body: Vec<u8>,
}
struct Partner {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    release: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Partner {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Partner {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let saved = requests.clone();
        let release = Arc::new(Notify::new());
        let gate = release.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                // The host can probe newly opened loopback ports. Only a
                // request carrying this fixture's credentials consumes a
                // scripted reply; empty connections and health checks do not.
                let (mut stream, mut raw, header_end, headers) = loop {
                    let (mut stream, _) = listener.accept().await.expect("request");
                    let mut raw = Vec::new();
                    let header_end = loop {
                        let mut buffer = [0; 8192];
                        let count = stream.read(&mut buffer).await.unwrap_or(0);
                        if count == 0 {
                            break None;
                        }
                        raw.extend_from_slice(&buffer[..count]);
                        if let Some(end) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                            break Some(end + 4);
                        }
                        assert!(raw.len() < 65536, "fixture headers");
                    };
                    let Some(header_end) = header_end else {
                        continue;
                    };
                    let headers = String::from_utf8(raw[..header_end].to_vec()).expect("headers");
                    let owned = headers.lines().filter_map(|line| line.split_once(':')).any(
                        |(name, value)| {
                            name.eq_ignore_ascii_case("authorization")
                                && value.trim() == "Bearer owned-edi-key"
                                || name.eq_ignore_ascii_case("x-stateset-api-key")
                                    && value.trim() == "owned-sync-key"
                        },
                    );
                    if !owned {
                        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").await;
                        continue;
                    }
                    break (stream, raw, header_end, headers);
                };
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("request length"))
                    })
                    .unwrap_or(0);
                while raw.len() - header_end < length {
                    let mut buffer = [0; 8192];
                    let count = stream.read(&mut buffer).await.expect("request body");
                    assert_ne!(count, 0);
                    raw.extend_from_slice(&buffer[..count]);
                }
                saved.lock().await.push(Request {
                    headers,
                    body: raw[header_end..].to_vec(),
                });
                let framing = match reply.wire {
                    Wire::Chunked | Wire::OpenChunked => "Transfer-Encoding: chunked\r\n".into(),
                    Wire::CloseDelimited => String::new(),
                    Wire::Truncated => format!("Content-Length: {}\r\n", reply.body.len() + 100),
                    Wire::AnnouncedOnly => format!("Content-Length: {}\r\n", LIMIT + 1),
                    Wire::Fixed => format!("Content-Length: {}\r\n", reply.body.len()),
                };
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 {} Fixture\r\nConnection: close\r\n{}\r\n",
                            reply.status, framing
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("response headers");
                if matches!(reply.wire, Wire::AnnouncedOnly) {
                    gate.notified().await;
                    continue;
                }
                for chunk in reply.body.chunks(16384) {
                    if matches!(reply.wire, Wire::Chunked | Wire::OpenChunked) {
                        if stream
                            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if stream.write_all(chunk).await.is_err() {
                            break;
                        }
                        if stream.write_all(b"\r\n").await.is_err() {
                            break;
                        }
                    } else if stream.write_all(chunk).await.is_err() {
                        break;
                    }
                }
                if matches!(reply.wire, Wire::OpenChunked) {
                    gate.notified().await;
                }
                if matches!(reply.wire, Wire::Chunked | Wire::OpenChunked) {
                    let _ = stream.write_all(b"0\r\n\r\n").await;
                }
            }
        });
        Self {
            url,
            requests,
            release,
            task,
        }
    }
    fn integrations(&self) -> Integrations {
        Integrations {
            tenants: BTreeMap::from([(
                "demo".into(),
                Connection {
                    edi_url: Some(self.url.clone()),
                    edi_token: Some("owned-edi-key".into()),
                    edi_tenant_id: Some("edi-tenant".into()),
                    sync_url: Some(self.url.clone()),
                    sync_api_key: Some("owned-sync-key".into()),
                    sync_tenant_id: Some("sync-tenant".into()),
                    ..Connection::default()
                },
            )]),
        }
    }
}
#[tokio::test]
async fn unrelated_health_probes_and_empty_connections_do_not_consume_partner_replies() {
    let partner = Partner::new(vec![Reply::new(valid_body(1), Wire::Fixed)]).await;
    let address = partner.url.trim_start_matches("http://");
    let mut empty = tokio::net::TcpStream::connect(address)
        .await
        .expect("empty probe");
    empty.shutdown().await.expect("empty probe closes");
    drop(empty);
    for host in ["127.0.0.1", "localhost"] {
        let mut probe = tokio::net::TcpStream::connect(address)
            .await
            .expect("health probe");
        probe
            .write_all(
                format!("GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Go-http-client/1.1\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .expect("probe headers");
        let mut response = Vec::new();
        probe
            .read_to_end(&mut response)
            .await
            .expect("probe refusal");
        assert!(response.starts_with(b"HTTP/1.1 401 Unauthorized\r\n"));
        assert!(partner.requests.lock().await.is_empty());
        assert!(
            !partner.task.is_finished(),
            "health probe must not consume the owned response"
        );
    }
    let original = fetch(&partner.integrations(), 1)
        .await
        .expect("actual owned request");
    assert_eq!(original["id"], ORDER);
    assert_eq!(partner.requests.lock().await.len(), 1);
}

fn notification() -> Value {
    json!({"type":"document.recorded","tenant_id":"edi-tenant","document_id":"owned-doc","partner_id":"atfl","transaction_set":"990","direction":"inbound","status":"accepted"})
}
fn document() -> Value {
    let note = notification();
    json!({"data":{"document":{"id":note["document_id"],"partner_id":note["partner_id"],"transaction_set":"990","direction":"inbound","status":"accepted","processed_at":Utc::now()},"parsed":{"shipment_id":"FR-1050","carrier_scac":"ATFL","action_code":"A"}}})
}
fn job(destination: &str) -> Outbox {
    serde_json::from_value(json!({"id":"owned-original-key","load_id":"load_1","destination":destination,"status":"pending_handoff","method":"POST","path":"/owned-handoff","payload":{"trackingData":{"freight_status":"in_transit"},"original":"retain this exact request"}})).expect("job")
}
async fn fetch(integrations: &Integrations, kind: usize) -> Result<Value, String> {
    match kind {
        0 => integrations.orders("demo", 1).await,
        1 => integrations.order_snapshot("demo", ORDER).await,
        2 => integrations
            .callback_event("demo", &notification())
            .await
            .map(|v| v.expect("event")),
        _ => unreachable!(),
    }
}
fn valid_body(kind: usize) -> Vec<u8> {
    serde_json::to_vec(&match kind {
        0 => json!({"data":{"items":[]}}),
        1 => json!({"data":{"id":ORDER,"tenantId":"sync-tenant"}}),
        2 => document(),
        _ => unreachable!(),
    })
    .expect("body")
}

#[tokio::test]
async fn connector_reads_reject_oversized_streams_before_the_partner_finishes() {
    for kind in 0..3 {
        for wire in [Wire::OpenChunked, Wire::AnnouncedOnly] {
            let partner = Partner::new(vec![Reply::new(vec![b' '; LIMIT + 1], wire)]).await;
            let result =
                tokio::time::timeout(Duration::from_secs(2), fetch(&partner.integrations(), kind))
                    .await
                    .expect("reject while partner still holds body open");
            assert_eq!(
                result.expect_err("oversized response"),
                if kind == 2 {
                    "EDI detail response exceeds limit"
                } else {
                    "Sync order response exceeds limit"
                }
            );
            assert!(
                !partner.task.is_finished(),
                "the fixture must still be withholding completion"
            );
            partner.release.notify_one();
        }
    }
}

#[tokio::test]
async fn connector_reads_accept_exact_limit_for_all_http_body_framings() {
    for kind in 0..3 {
        for wire in [Wire::Fixed, Wire::Chunked, Wire::CloseDelimited] {
            let mut body = valid_body(kind);
            body.resize(LIMIT, b' ');
            let partner = Partner::new(vec![Reply::new(body, wire)]).await;
            let value = fetch(&partner.integrations(), kind)
                .await
                .expect("exact limit is allowed");
            assert!(value.is_object());
            let requests = partner.requests.lock().await;
            assert_eq!(requests.len(), 1);
            let headers = requests[0].headers.to_ascii_lowercase();
            if kind == 2 {
                assert!(headers.starts_with("get /v1/documents/owned-doc "));
                assert!(headers.contains("authorization: bearer owned-edi-key\r\n"));
                assert!(headers.contains("x-tenant-id: edi-tenant\r\n"));
            } else {
                assert!(headers.starts_with("get /v1/tenants/sync-tenant/orders"));
                assert!(headers.contains("x-stateset-api-key: owned-sync-key\r\n"));
            }
        }
    }
}

#[tokio::test]
async fn connector_reads_reject_oversized_close_delimited_and_incomplete_bodies() {
    for kind in 0..3 {
        let partner = Partner::new(vec![Reply::new(
            vec![b' '; LIMIT + 1],
            Wire::CloseDelimited,
        )])
        .await;
        assert!(fetch(&partner.integrations(), kind)
            .await
            .expect_err("close-delimited cap")
            .ends_with("response exceeds limit"));
        let partner = Partner::new(vec![Reply::new(valid_body(kind), Wire::Truncated)]).await;
        assert!(fetch(&partner.integrations(), kind)
            .await
            .expect_err("truncated body")
            .ends_with("response unreadable"));
        let partner = Partner::new(vec![Reply::new(vec![0xff], Wire::Chunked)]).await;
        assert!(fetch(&partner.integrations(), kind)
            .await
            .expect_err("invalid UTF-8 JSON")
            .starts_with("Invalid "));
    }
}

#[tokio::test]
async fn connector_http_failures_do_not_wait_for_or_retain_error_bodies() {
    for kind in 0..5 {
        let mut reply = Reply::new(Vec::new(), Wire::AnnouncedOnly);
        reply.status = 503;
        let partner = Partner::new(vec![reply]).await;
        let integrations = partner.integrations();
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            if kind < 3 {
                fetch(&integrations, kind).await.map(|_| ())
            } else {
                send(
                    &integrations.tenants["demo"],
                    &job(if kind == 3 {
                        "stateset-edi"
                    } else {
                        "stateset-sync-server"
                    }),
                )
                .await
                .map(|_| ())
            }
        })
        .await
        .expect("classify status before reading held error body");
        assert!(result
            .expect_err("upstream unavailable")
            .ends_with("HTTP 503"));
        assert!(!partner.task.is_finished());
    }
}

#[tokio::test]
async fn outbound_acknowledgements_enforce_stream_limits_and_preserve_exact_text() {
    for destination in ["stateset-edi", "stateset-sync-server"] {
        for wire in [Wire::OpenChunked, Wire::AnnouncedOnly] {
            let partner = Partner::new(vec![Reply::new(vec![b' '; LIMIT + 1], wire)]).await;
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                send(&partner.integrations().tenants["demo"], &job(destination)),
            )
            .await
            .expect("reject before end of body");
            assert_eq!(
                result.expect_err("oversized acknowledgement"),
                "Integration response exceeds limit"
            );
            assert!(!partner.task.is_finished());
        }
        for wire in [Wire::Fixed, Wire::Chunked, Wire::CloseDelimited] {
            let mut body = if destination == "stateset-edi" {
                b"ISA*original interchange~".to_vec()
            } else {
                br#"{"data":{"ok":true}}"#.to_vec()
            };
            body.resize(LIMIT, b' ');
            let partner = Partner::new(vec![Reply::new(body.clone(), wire)]).await;
            let result = send(&partner.integrations().tenants["demo"], &job(destination))
                .await
                .expect("bounded original acknowledgement");
            if destination == "stateset-edi" {
                assert_eq!(result.as_bytes(), body);
            } else {
                assert_eq!(result, "Tracking update confirmed");
            }
        }
        let partner = Partner::new(vec![Reply::new(
            if destination == "stateset-edi" {
                b"ISA*original~".to_vec()
            } else {
                br#"{"data":{"ok":true}}"#.to_vec()
            },
            Wire::Truncated,
        )])
        .await;
        assert_eq!(
            send(&partner.integrations().tenants["demo"], &job(destination))
                .await
                .expect_err("incomplete acknowledgement"),
            "Integration response unreadable"
        );
        let partner = Partner::new(vec![Reply::new(
            vec![b' '; LIMIT + 1],
            Wire::CloseDelimited,
        )])
        .await;
        assert_eq!(
            send(&partner.integrations().tenants["demo"], &job(destination))
                .await
                .expect_err("close-delimited acknowledgement cap"),
            "Integration response exceeds limit"
        );
    }
}

#[tokio::test]
async fn acknowledgements_reject_invalid_bytes_instead_of_replacing_them() {
    for destination in ["stateset-edi", "stateset-sync-server"] {
        let body = if destination == "stateset-edi" {
            b"ISA*\xff~".to_vec()
        } else {
            b"{\"data\":{\"ok\":true},\"invalid\":\"\xff\"}".to_vec()
        };
        let partner = Partner::new(vec![Reply::new(body, Wire::Chunked)]).await;
        let error = send(&partner.integrations().tenants["demo"], &job(destination))
            .await
            .expect_err("never repair invalid bytes into an accepted acknowledgement");
        assert_eq!(
            error,
            if destination == "stateset-edi" {
                "EDI response is not UTF-8"
            } else {
                "Sync response is not JSON"
            }
        );
    }
}

async fn outbound_retry(store: &Store) {
    for destination in ["stateset-edi", "stateset-sync-server"] {
        let accepted = if destination == "stateset-edi" {
            b"ISA*original accepted bytes~".to_vec()
        } else {
            br#"{"data":{"ok":true}}"#.to_vec()
        };
        let partner = Partner::new(vec![
            Reply::new(vec![b' '; LIMIT + 1], Wire::Chunked),
            Reply::new(accepted.clone(), Wire::Chunked),
        ])
        .await;
        let tenant = destination;
        let integrations = Integrations {
            tenants: BTreeMap::from([(
                tenant.into(),
                partner.integrations().tenants["demo"].clone(),
            )]),
        };
        let original = job(destination);
        let seeded = original.clone();
        store
            .update(tenant, move |w| {
                *w = crate::domain::demo();
                w.outbox = vec![seeded];
                Ok(Value::Null)
            })
            .await
            .expect("owned original pending work");
        let before = store.read(tenant).await.expect("before");
        assert!(integrations
            .dispatch_one(store, tenant)
            .await
            .expect("failed attempt recorded"));
        let failed = store.read(tenant).await.expect("failed");
        let item = &failed.outbox[0];
        assert_eq!(item.status, "retry");
        assert_eq!(
            item.last_error.as_deref(),
            Some("Integration response exceeds limit")
        );
        assert_eq!(item.total_attempts, 1);
        assert_eq!(item.transmission, "attempted");
        assert_eq!(item.payload, original.payload);
        assert_eq!(item.path, original.path);
        assert!(item.response.is_none() && item.lease.is_none() && item.next_attempt_at.is_some());
        assert_eq!(json!(failed.loads), json!(before.loads));
        assert_eq!(json!(failed.journal), json!(before.journal));
        store
            .mutate(
                tenant,
                "operator-retry",
                "outbox.retry",
                &original.id,
                json!({}),
            )
            .await
            .expect("explicit retry");
        assert!(integrations
            .dispatch_one(store, tenant)
            .await
            .expect("original request recovered"));
        let recovered = store.read(tenant).await.expect("recovered");
        let item = &recovered.outbox[0];
        assert_eq!(
            item.status,
            if destination == "stateset-edi" {
                "gateway_accepted"
            } else {
                "synced"
            },
            "handoff recovery failed: error={:?}, observed_requests={}, partner_finished={}",
            item.last_error,
            partner.requests.lock().await.len(),
            partner.task.is_finished()
        );
        assert_eq!(item.total_attempts, 2);
        assert_eq!(item.id, original.id);
        assert_eq!(item.payload, original.payload);
        assert!(item.last_error.is_none());
        assert_eq!(
            item.response.as_deref(),
            Some(if destination == "stateset-edi" {
                std::str::from_utf8(&accepted).expect("original")
            } else {
                "Tracking update confirmed"
            })
        );
        assert!(!integrations
            .dispatch_one(store, tenant)
            .await
            .expect("completed work sleeps"));
        let requests = partner.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].headers, requests[1].headers,
            "same target, tenant, credentials and original key"
        );
        assert_eq!(
            requests[0].body, requests[1].body,
            "exact request bytes reused"
        );
        assert!(requests[0]
            .headers
            .to_ascii_lowercase()
            .contains("idempotency-key: owned-original-key\r\n"));
    }
}

#[tokio::test]
async fn oversized_handoffs_retry_with_original_keys_and_payloads() {
    outbound_retry(&Store::new(None).await.expect("memory")).await;
}

#[tokio::test]
async fn oversized_edi_detail_retains_callback_evidence_until_a_valid_retry() {
    inbound_retry(&Store::new(None).await.expect("memory")).await;
}

async fn inbound_retry(store: &Store) {
    let partner = Partner::new(vec![
        Reply::new(vec![b' '; LIMIT + 1], Wire::Chunked),
        Reply::new(
            serde_json::to_vec(&document()).expect("detail"),
            Wire::Chunked,
        ),
    ])
    .await;
    let integrations = partner.integrations();
    store.seed_demo().await.expect("seed");
    let note = notification();
    store
        .receive_document(
            "demo",
            "owned-delivery",
            "1",
            note.clone(),
            note.to_string(),
            None,
        )
        .await
        .expect("original callback retained");
    let before = store.read("demo").await.expect("before");
    assert!(integrations
        .process_inbound_one(store, "demo")
        .await
        .expect("attempt retained"));
    let failed = store.read("demo").await.expect("failed");
    let item = &failed.inbox[0];
    assert_eq!(item.status, "retry");
    assert_eq!(
        item.last_error.as_deref(),
        Some("EDI detail response exceeds limit")
    );
    assert_eq!(item.original_body, before.inbox[0].original_body);
    assert_eq!(item.fingerprint, before.inbox[0].fingerprint);
    assert_eq!(item.delivery_ids, before.inbox[0].delivery_ids);
    assert!(item.normalized_event.is_none() && item.result.is_none());
    assert_eq!(json!(failed.loads), json!(before.loads));
    assert_eq!(json!(failed.journal), json!(before.journal));
    assert_eq!(json!(failed.receipts), json!(before.receipts));
    store
        .mutate("demo", "owned-retry", "inbox.retry", &item.id, json!({}))
        .await
        .expect("operator retry");
    assert!(integrations
        .process_inbound_one(store, "demo")
        .await
        .expect("valid detail applied"));
    let recovered = store.read("demo").await.expect("recovered");
    assert_eq!(recovered.inbox[0].status, "applied");
    assert_eq!(recovered.inbox[0].total_attempts, 2);
    assert_eq!(
        recovered.inbox[0].original_body,
        before.inbox[0].original_body
    );
    assert_eq!(recovered.inbox[0].history[0].outcome, "failed");
    assert_eq!(recovered.inbox[0].history[1].outcome, "applied");
    assert_eq!(recovered.loads[2].status, Status::Accepted);
    assert!(!integrations
        .process_inbound_one(store, "demo")
        .await
        .expect("no duplicate application"));
    assert_eq!(partner.requests.lock().await.len(), 2);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_oversized_handoffs_recover_original_work_and_survive_reconnect() {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned PostgreSQL");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("response_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema},pg_catalog"));
    let store = Store::new(Some(url.as_str())).await.expect("store");
    outbound_retry(&store).await;
    inbound_retry(&store).await;
    drop(store);
    let restarted = Store::new(Some(url.as_str())).await.expect("reconnect");
    for tenant in ["stateset-edi", "stateset-sync-server"] {
        let workspace = restarted.read(tenant).await.expect("restored work");
        assert_eq!(workspace.outbox.len(), 1);
        assert_eq!(workspace.outbox[0].id, "owned-original-key");
        assert_eq!(workspace.outbox[0].total_attempts, 2);
        assert_eq!(
            workspace.outbox[0].status,
            if tenant == "stateset-edi" {
                "gateway_accepted"
            } else {
                "synced"
            }
        );
    }
    let recovered = restarted.read("demo").await.expect("retained callback");
    assert_eq!(recovered.inbox.len(), 1);
    assert_eq!(recovered.inbox[0].status, "applied");
    assert_eq!(recovered.inbox[0].total_attempts, 2);
    assert_eq!(recovered.inbox[0].original_body, notification().to_string());
    assert_eq!(recovered.inbox[0].history.len(), 2);
    assert_eq!(recovered.loads[2].status, Status::Accepted);
    drop(restarted);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
