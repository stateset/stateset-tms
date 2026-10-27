#[path = "support/canonical.rs"]
mod canonical;
#[path = "support/invoice_review.rs"]
mod invoice_review;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    documents::{self, Upload},
    integrations::Integrations,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";
async fn setup() -> (Store, Router) {
    let store = Store::new(None).await.expect("store");
    setup_store(store).await
}
async fn setup_store(store: Store) -> (Store, Router) {
    store.seed_demo().await.expect("seed");
    let keys = [
        ("ops", "demo", Role::Operations),
        ("finance", "demo", Role::Finance),
        ("viewer", "demo", Role::Viewer),
        ("other", "other", Role::Admin),
    ]
    .into_iter()
    .map(|(key, tenant, role)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: tenant.into(),
                subject: format!("user:{key}"),
                role,
                counterparty_id: None,
            }),
        )
    })
    .collect::<BTreeMap<_, _>>();
    let router = api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store: store.clone(),
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    });
    (store, router)
}
fn upload(bytes: &[u8]) -> Upload {
    Upload {
        kind: "pod".into(),
        filename: "receipt.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: bytes.to_vec(),
    }
}
async fn request(
    router: &Router,
    key: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
    kind: &str,
    idempotency: &str,
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", kind)
                .header("idempotency-key", idempotency)
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response")
}
async fn json_request(router: &Router, key: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = request(
        router,
        key,
        "POST",
        path,
        body.to_string().into_bytes(),
        "application/json",
        &uuid::Uuid::new_v4().to_string(),
    )
    .await;
    let code = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (code, serde_json::from_slice(&bytes).expect("json"))
}
async fn uploaded(store: &Store, load: &str, key: &str) -> String {
    store
        .upload_document(("demo", "user:ops"), key, load, upload(PDF))
        .await
        .expect("upload")["data"]["id"]
        .as_str()
        .expect("id")
        .into()
}

#[tokio::test]
async fn binary_upload_download_is_exact_scoped_and_checked_before_use() {
    let (store, router) = setup().await;
    let path = "/v1/loads/load_7/documents?kind=pod&filename=receipt.pdf";
    for key in ["viewer", "invalid"] {
        let expected = if key == "viewer" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(
            request(
                &router,
                key,
                "POST",
                path,
                PDF.to_vec(),
                "application/pdf",
                "denied"
            )
            .await
            .status(),
            expected
        );
    }
    assert!(store.read("demo").await.expect("read").documents.is_empty());
    let response = request(
        &router,
        "ops",
        "POST",
        path,
        PDF.to_vec(),
        "application/pdf",
        "upload",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    let id = result["data"]["id"].as_str().expect("id");
    let route = format!("/v1/documents/{id}/content");
    assert_eq!(
        request(
            &router,
            "other",
            "GET",
            &route,
            vec![],
            "application/json",
            "read"
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let response = request(
        &router,
        "viewer",
        "GET",
        &route,
        vec![],
        "application/json",
        "read",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-content-sha256"],
        documents::sha256(PDF)
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(response.headers()["content-disposition"]
        .to_str()
        .expect("header")
        .starts_with("attachment;"));
    assert_eq!(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes()[..],
        PDF
    );
    let before = json!(store.read("demo").await.expect("read"));
    for (filename, kind, bytes) in [
        ("evil.pdf", "text/html", b"<html>executable</html>".to_vec()),
        ("../receipt.pdf", "application/pdf", PDF.to_vec()),
        ("receipt.pdf", "application/pdf", b"fake pdf".to_vec()),
    ] {
        let response = request(
            &router,
            "ops",
            "POST",
            &format!("/v1/loads/load_7/documents?kind=pod&filename={filename}"),
            bytes,
            kind,
            "invalid",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
}

#[tokio::test]
async fn actual_file_evidence_requires_finance_review_and_locks_after_approval() {
    let (store, router) = setup().await;
    let id = uploaded(&store, "load_7", "pod").await;
    let attach = "/v1/loads/load_7/commands".to_owned();
    assert_eq!(
        json_request(
            &router,
            "ops",
            &attach,
            json!({"action":"attach_pod","document_id":id,"expected_version":1})
        )
        .await
        .0,
        StatusCode::OK
    );
    let workspace = store.read("demo").await.expect("read");
    let invoice = workspace
        .invoices
        .iter()
        .find(|v| v.load_id == "load_7")
        .expect("invoice");
    let invoice_id = invoice.id.clone();
    store.mutate("demo","correct","invoice.resolve",&invoice_id,json!({"expected_amount_cents":108000,"amount_cents":98000,"note":"Carrier supplied corrected original invoice"})).await.expect("correction");
    let approve = format!("/v1/invoices/{invoice_id}/approve");
    assert_eq!(
        json_request(&router, "finance", &approve, json!({}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let review = format!("/v1/documents/{id}/review");
    let body = json!({"expected_version":1,"decision":"accepted","note":"Inspected receiver signature and delivery identity on the stored scan"});
    assert_eq!(
        json_request(&router, "ops", &review, body.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_request(&router, "finance", &review, body).await.0,
        StatusCode::OK
    );
    assert_eq!(
        json_request(&router, "finance", &approve, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let before = json!(store.read("demo").await.expect("read"));
    assert_ne!(json_request(&router,"finance",&review,json!({"expected_version":2,"decision":"rejected","note":"Attempt to invalidate approved evidence"})).await.0,StatusCode::OK);
    assert_ne!(
        json_request(
            &router,
            "ops",
            &attach,
            json!({"action":"attach_pod","pod_reference":"replacement","expected_version":2})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
    assert_eq!(
        json_request(&router, "finance", "/v1/loads/load_7/settlement", json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let done = store.read("demo").await.expect("read");
    assert_eq!(done.documents[0].reviews[0].reviewed_by, "user:finance");
    assert!(done.delivery_evidence_ready(&done.loads[6]));
    assert_eq!(done.settlements[0].status, "prepared");
}

#[tokio::test]
async fn upload_and_review_replays_do_not_duplicate_bytes_evidence_or_audit() {
    let (store, _) = setup().await;
    let (a, b) = tokio::join!(
        store.upload_document(("demo", "ops-a"), "same", "load_7", upload(PDF)),
        store.upload_document(("demo", "ops-b"), "same", "load_7", upload(PDF))
    );
    let id = a.expect("first")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert_eq!(b.expect("second")["data"]["id"], id);
    assert_eq!(store.read("demo").await.expect("read").documents.len(), 1);
    let modified = b"%PDF-1.4\nchanged content\n%%EOF";
    assert!(store
        .upload_document(("demo", "ops"), "same", "load_7", upload(modified))
        .await
        .is_err());
    let body =
        json!({"expected_version":1,"decision":"rejected","note":"Missing receiver signature"});
    store
        .mutate_authorized(
            ("demo", "finance"),
            "review",
            "document.review",
            &id,
            body.clone(),
        )
        .await
        .expect("review");
    let before = json!(store.read("demo").await.expect("read"));
    let replay = store
        .mutate_authorized(("demo", "finance"), "review", "document.review", &id, body)
        .await
        .expect("replay");
    assert_eq!(replay["replayed"], true);
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
    assert!(store
        .mutate(
            "demo",
            "stale",
            "document.review",
            &id,
            json!({"expected_version":1,"decision":"accepted","note":"Stale review"})
        )
        .await
        .is_err());
    assert!(store
        .mutate(
            "demo",
            "wrong-load",
            "load.command",
            "load_6",
            json!({"action":"attach_pod","expected_version":1,"document_id":id})
        )
        .await
        .is_err());
    assert_eq!(
        store
            .document_content("demo", &id)
            .await
            .expect("unchanged bytes")
            .1,
        PDF
    );
}

#[tokio::test]
async fn uploads_over_default_json_limit_work_but_over_document_limit_fail() {
    let (_, router) = setup().await;
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.resize(1024 * 1024 + 100, b' ');
    bytes.extend_from_slice(b"\n%%EOF");
    let route = "/v1/loads/load_7/documents?kind=pod&filename=large.pdf";
    assert_eq!(
        request(
            &router,
            "ops",
            "POST",
            route,
            bytes,
            "application/pdf",
            "large"
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &router,
            "ops",
            "POST",
            route,
            vec![b'x'; documents::MAX_DOCUMENT_BYTES + 1],
            "application/pdf",
            "too-big"
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn external_evidence_review_is_bound_to_the_exact_current_reference() {
    let (store, router) = setup().await;
    let attach = "/v1/loads/load_7/commands";
    json_request(
        &router,
        "ops",
        attach,
        json!({"action":"attach_pod","pod_reference":"external-A","expected_version":1}),
    )
    .await;
    let review = "/v1/loads/load_7/pod/review";
    let body = json!({"expected_version":2,"decision":"accepted","note":"Retrieved external file A and checked receiver confirmation"});
    assert_eq!(
        json_request(&router, "ops", review, body.clone()).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_request(&router, "finance", review, body).await.0,
        StatusCode::OK
    );
    let ready = store.read("demo").await.expect("read");
    assert!(ready.delivery_evidence_ready(&ready.loads[6]));
    assert_eq!(
        json_request(
            &router,
            "ops",
            attach,
            json!({"action":"attach_pod","pod_reference":"external-B","expected_version":3})
        )
        .await
        .0,
        StatusCode::OK
    );
    let replaced = store.read("demo").await.expect("read");
    assert!(!replaced.delivery_evidence_ready(&replaced.loads[6]));
    assert!(replaced.loads[6].pod_review.is_none());
    assert_eq!(json_request(&router,"finance",review,json!({"expected_version":4,"decision":"accepted","note":"Client cannot claim reviewer","reviewed_by":"admin"})).await.0,StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_commits_document_bytes_metadata_and_receipts_together_across_instances() {
    let url = std::env::var("TEST_DATABASE_URL").expect("url");
    let a = Store::new(Some(&url)).await.expect("connect");
    let b = Store::new(Some(&url)).await.expect("second");
    let tenant = uuid::Uuid::new_v4().to_string();
    a.mutate(
        &tenant,
        "customer",
        "customer.create",
        "",
        json!({"name":"Test customer","contact":""}),
    )
    .await
    .expect("customer");
    // A fresh tenant has no load, so a failed upload must leave no binary orphan.
    assert!(a
        .upload_document((&tenant, "ops"), "missing", "missing", upload(PDF))
        .await
        .is_err());
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM freight_document_content WHERE tenant_id=$1")
            .bind(&tenant)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 0);
    canonical::put_workspace(&pool, &tenant, &json!(stateset_freight::domain::demo()))
        .await
        .expect("isolated synthetic workspace");
    let (first, second) = tokio::join!(
        a.upload_document((&tenant, "ops-a"), "same", "load_7", upload(PDF)),
        b.upload_document((&tenant, "ops-b"), "same", "load_7", upload(PDF))
    );
    let id = first.expect("first")["data"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert_eq!(second.expect("second")["data"]["id"], id);
    drop(a);
    drop(b);
    let restarted = Store::new(Some(&url)).await.expect("restart");
    let (document, bytes) = restarted
        .document_content(&tenant, &id)
        .await
        .expect("persisted");
    assert_eq!(bytes, PDF);
    assert_eq!(document.sha256, documents::sha256(PDF));
    assert!(restarted
        .document_content("other-tenant", &id)
        .await
        .is_err());
    restarted
        .mutate(
            &tenant,
            "attach",
            "load.command",
            "load_7",
            json!({"action":"attach_pod","document_id":id,"expected_version":1}),
        )
        .await
        .expect("attach");
    restarted.mutate_authorized((&tenant,"finance"),"review","document.review",&id,json!({"expected_version":1,"decision":"accepted","note":"Inspected original receiver evidence"})).await.expect("review");
    let invoice = restarted
        .read(&tenant)
        .await
        .expect("read")
        .invoices
        .into_iter()
        .find(|v| v.load_id == "load_7")
        .expect("invoice");
    restarted.mutate(&tenant,"correct","invoice.resolve",&invoice.id,json!({"expected_amount_cents":108000,"amount_cents":98000,"note":"Revised carrier invoice"})).await.expect("correct");
    let bill=restarted.mutate(&tenant,"bill","bill.create","",json!({"load_id":"load_7","number":"BILL-CORRUPTION","due_date":chrono::Utc::now().date_naive()})).await.expect("bill");
    let bill_id = bill["data"]["id"].as_str().expect("bill id");
    // Independent corruption injection must fail the download integrity check.
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt".to_vec())
    .bind(&tenant)
    .bind(&id)
    .execute(&pool)
    .await
    .expect("corrupt test-owned file");
    assert!(restarted.document_content(&tenant, &id).await.is_err());
    let invoice_approval = invoice_review::body(&restarted, &tenant, &invoice.id).await;
    let before = json!(restarted.read(&tenant).await.expect("read"));
    assert!(restarted
        .mutate(
            &tenant,
            "bill-issue",
            "bill.issue",
            bill_id,
            json!({"expected_version":1})
        )
        .await
        .is_err());
    assert!(restarted
        .mutate_authorized(
            (&tenant, invoice_review::CHECKER),
            "approve",
            "invoice.approve",
            &invoice.id,
            invoice_approval.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(restarted.read(&tenant).await.expect("read")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&id)
    .execute(&pool)
    .await
    .expect("restore test-owned file");
    assert_eq!(
        restarted
            .document_content(&tenant, &id)
            .await
            .expect("recovered original bytes")
            .1,
        PDF
    );
    restarted
        .mutate_authorized(
            (&tenant, invoice_review::CHECKER),
            "approve",
            "invoice.approve",
            &invoice.id,
            invoice_approval,
        )
        .await
        .expect("approval after integrity restored");
    restarted
        .mutate(
            &tenant,
            "bill-issue",
            "bill.issue",
            bill_id,
            json!({"expected_version":1}),
        )
        .await
        .expect("customer issue after integrity restored");
}

static PREVIEW_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn evidence_catalog_and_rgb_preview_are_tenant_scoped_integrity_bound_and_read_only() {
    let _guard = PREVIEW_TEST_LOCK.lock().await;
    let (store, router) = setup().await;
    let load_id = store.read("demo").await.expect("workspace").loads[0]
        .id
        .clone();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(32, 16, image::Rgb([12, 96, 130]))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("png");
    let original = bytes.into_inner();
    let uploaded = store
        .upload_document(
            ("demo", "fixture:operator"),
            "owned-preview",
            &load_id,
            Upload {
                kind: "pod".into(),
                filename: "owned.png".into(),
                content_type: "image/png".into(),
                bytes: original.clone(),
            },
        )
        .await
        .expect("upload");
    let id = uploaded["data"]["id"].as_str().expect("document");
    let before =
        serde_json::to_value(store.read("demo").await.expect("workspace")).expect("original state");
    let catalog_path = format!("/v1/loads/{load_id}/documents");
    let preview_path = format!("/v1/documents/{id}/preview");
    for path in [&catalog_path, &preview_path] {
        assert_eq!(
            request(
                &router,
                "missing",
                "GET",
                path,
                vec![],
                "application/json",
                ""
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &router,
                "other",
                "GET",
                path,
                vec![],
                "application/json",
                ""
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    let catalog = request(
        &router,
        "viewer",
        "GET",
        &catalog_path,
        vec![],
        "application/json",
        "",
    )
    .await;
    assert_eq!(catalog.status(), StatusCode::OK);
    assert_eq!(catalog.headers()["cache-control"], "private, no-store");
    let catalog: Value = serde_json::from_slice(
        &catalog
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(catalog["total"], 1);
    assert_eq!(catalog["documents"][0]["id"], id);
    assert_eq!(catalog["documents"][0]["status"], "pending_review");
    assert!(catalog["documents"][0].get("reviews").is_none());
    let response = request(
        &router,
        "viewer",
        "GET",
        &preview_path,
        vec![],
        "application/json",
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "application/vnd.stateset.rgb8");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["x-source-sha256"], documents::sha256(&original));
    assert_eq!(headers["x-load-id"], load_id);
    assert_eq!(headers["x-document-id"], id);
    let pixels = response
        .into_body()
        .collect()
        .await
        .expect("pixels")
        .to_bytes();
    assert_eq!(headers["x-content-sha256"], documents::sha256(&pixels));
    assert_eq!(headers["x-preview-width"], "32");
    assert_eq!(headers["x-preview-height"], "16");
    assert_eq!(headers["x-preview-page"], "1");
    assert_eq!(headers["x-preview-pages"], "1");
    assert_eq!(pixels.as_ref(), [12, 96, 130].repeat(32 * 16));
    assert_eq!(
        request(
            &router,
            "viewer",
            "GET",
            &format!("{preview_path}?page=2"),
            vec![],
            "application/json",
            ""
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let after =
        serde_json::to_value(store.read("demo").await.expect("workspace")).expect("after state");
    assert_eq!(
        before, after,
        "preview does not approve evidence, record events or change originals"
    );
    assert_eq!(
        store
            .document_content("demo", id)
            .await
            .expect("retained")
            .1,
        original
    );
}

#[tokio::test]
async fn invalid_pdf_and_corrupt_images_have_safe_preview_errors() {
    let _guard = PREVIEW_TEST_LOCK.lock().await;
    let (store, router) = setup().await;
    let load_id = store.read("demo").await.expect("workspace").loads[0]
        .id
        .clone();
    let mut fake_png = b"\x89PNG\r\n\x1a\n".to_vec();
    fake_png.extend_from_slice(b"\x00\x00\x00\x00IEND\xaeB`\x82");
    for (n, upload) in [
        upload(PDF),
        Upload {
            kind: "pod".into(),
            filename: "corrupt.png".into(),
            content_type: "image/png".into(),
            bytes: fake_png,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let value = store
            .upload_document(
                ("demo", "fixture:operator"),
                &format!("owned-preview-invalid-{n}"),
                &load_id,
                upload,
            )
            .await
            .expect("retained original");
        let id = value["data"]["id"].as_str().expect("id");
        let response = request(
            &router,
            "viewer",
            "GET",
            &format!("/v1/documents/{id}/preview"),
            vec![],
            "application/json",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn pdf_pages_are_bounded_source_bound_tenant_scoped_and_preserve_originals() {
    let _guard = PREVIEW_TEST_LOCK.lock().await;
    let (store, router) = setup().await;
    qualify_pdf_pages(store, router).await;
}

#[tokio::test]
#[cfg(target_os = "linux")]
#[ignore = "requires disposable TEST_DATABASE_URL and Poppler"]
async fn postgres_pdf_pages_preserve_exact_originals_and_tenant_boundaries() {
    let _guard = PREVIEW_TEST_LOCK.lock().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable TEST_DATABASE_URL");
    let (store, router) =
        setup_store(Store::new(Some(&url)).await.expect("PostgreSQL store")).await;
    qualify_pdf_pages(store, router).await;
}

#[cfg(target_os = "linux")]
async fn qualify_pdf_pages(store: Store, router: Router) {
    let load = store.read("demo").await.expect("workspace").loads[0]
        .id
        .clone();
    let original = include_bytes!("fixtures/documents/preview-two-pages.pdf");
    let value = store
        .upload_document(
            ("demo", "fixture:operator"),
            "owned-pdf-pages",
            &load,
            upload(original),
        )
        .await
        .expect("original PDF");
    let id = value["data"]["id"].as_str().expect("id");
    let path = format!("/v1/documents/{id}/preview");
    let before = serde_json::to_value(store.read("demo").await.expect("before")).expect("state");
    for (key, status) in [
        ("missing", StatusCode::UNAUTHORIZED),
        ("other", StatusCode::NOT_FOUND),
    ] {
        assert_eq!(
            request(&router, key, "GET", &path, vec![], "application/json", "")
                .await
                .status(),
            status
        );
    }
    for (query, page, dimensions, color) in [
        ("", "1", (720, 960), [255, 0, 0]),
        ("?page=2", "2", (960, 720), [0, 0, 255]),
        ("?page=1", "1", (720, 960), [255, 0, 0]),
    ] {
        let response = request(
            &router,
            "viewer",
            "GET",
            &format!("{path}{query}"),
            vec![],
            "application/json",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
        let headers = response.headers().clone();
        assert_eq!(headers["x-preview-page"], page);
        assert_eq!(headers["x-preview-pages"], "2");
        assert_eq!(headers["x-preview-width"], dimensions.0.to_string());
        assert_eq!(headers["x-preview-height"], dimensions.1.to_string());
        assert_eq!(headers["x-source-sha256"], documents::sha256(original));
        assert_eq!(headers["x-document-id"], id);
        assert_eq!(headers["x-load-id"], load);
        assert_eq!(headers["cache-control"], "private, no-store");
        let pixels = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        assert_eq!(headers["x-content-sha256"], documents::sha256(&pixels));
        // Independent fixture expectation: every byte, not the renderer's own digest alone.
        assert_eq!(pixels.as_ref(), color.repeat(dimensions.0 * dimensions.1));
    }
    for (query, status) in [
        ("?page=0", StatusCode::UNPROCESSABLE_ENTITY),
        ("?page=3", StatusCode::UNPROCESSABLE_ENTITY),
        ("?page=1001", StatusCode::UNPROCESSABLE_ENTITY),
        ("?page=-1", StatusCode::BAD_REQUEST),
        ("?page=1&page=2", StatusCode::BAD_REQUEST),
        ("?page=no", StatusCode::BAD_REQUEST),
        ("?url=https://refused.invalid", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            request(
                &router,
                "viewer",
                "GET",
                &format!("{path}{query}"),
                vec![],
                "application/json",
                ""
            )
            .await
            .status(),
            status,
            "{query}"
        );
    }
    let after = serde_json::to_value(store.read("demo").await.expect("after")).expect("state");
    assert_eq!(
        before, after,
        "Viewing never mutates reviews, receipts or originals"
    );
    assert_eq!(
        store
            .document_content("demo", id)
            .await
            .expect("original")
            .1,
        original
    );
    let encrypted = include_bytes!("fixtures/documents/preview-encrypted.pdf");
    let value = store
        .upload_document(
            ("demo", "fixture:operator"),
            "owned-encrypted-pdf",
            &load,
            upload(encrypted),
        )
        .await
        .expect("encrypted original");
    let id = value["data"]["id"].as_str().expect("id");
    let response = request(
        &router,
        "viewer",
        "GET",
        &format!("/v1/documents/{id}/preview"),
        vec![],
        "application/json",
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert!(!String::from_utf8_lossy(&error).contains("synthetic-fixture-password"));
    assert_eq!(
        store
            .document_content("demo", id)
            .await
            .expect("retained encrypted original")
            .1,
        encrypted
    );

    // Exercise standard-font text as well as solid colors: missing runtime fonts
    // must not silently produce blank POD previews while the color tests pass.
    let text = include_bytes!("fixtures/documents/preview-text.pdf");
    let value = store
        .upload_document(
            ("demo", "fixture:operator"),
            "owned-text-pdf",
            &load,
            upload(text),
        )
        .await
        .expect("text PDF");
    let id = value["data"]["id"].as_str().expect("id");
    let response = request(
        &router,
        "viewer",
        "GET",
        &format!("/v1/documents/{id}/preview"),
        vec![],
        "application/json",
        "",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["x-preview-width"], "960");
    let height = headers["x-preview-height"]
        .to_str()
        .expect("height")
        .parse::<usize>()
        .expect("height");
    // 960 * 320 / 500 = 614.4: Poppler versions use floor or ceil here.
    assert!(
        [614, 615].contains(&height),
        "source aspect rounds within one pixel"
    );
    assert_eq!(headers["x-preview-page"], "1");
    assert_eq!(headers["x-preview-pages"], "1");
    assert_eq!(headers["x-source-sha256"], documents::sha256(text));
    let rgb = response
        .into_body()
        .collect()
        .await
        .expect("text pixels")
        .to_bytes();
    assert_eq!(rgb.len(), 960 * height * 3);
    assert_eq!(headers["x-content-sha256"], documents::sha256(&rgb));
    let mut ink = 0;
    let mut white = 0;
    let mut bands = 0;
    let mut previous_row = None;
    let mut last_row = 0;
    for (y, row) in rgb.chunks_exact(960 * 3).enumerate() {
        let mut dark = 0;
        for pixel in row.chunks_exact(3) {
            if pixel.iter().all(|v| *v < 80) {
                dark += 1;
            }
            if pixel.iter().all(|v| *v > 245) {
                white += 1;
            }
        }
        ink += dark;
        if dark >= 3 {
            if previous_row != y.checked_sub(1) || previous_row.is_none() {
                bands += 1;
            }
            previous_row = Some(y);
            last_row = y;
        }
    }
    assert!((2500..100000).contains(&ink), "visible text ink");
    assert!(white > 960 * height * 85 / 100, "white paper background");
    assert!(bands >= 5 && last_row > 250, "all five text lines render");
    assert_eq!(
        store
            .document_content("demo", id)
            .await
            .expect("text original")
            .1,
        text
    );
    assert_eq!(
        store
            .read("demo")
            .await
            .expect("workspace")
            .documents
            .iter()
            .find(|d| d.id == id)
            .expect("text document")
            .status(),
        "pending_review"
    );
}
