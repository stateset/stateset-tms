use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{self, Credential, Principal, Role},
    integrations::Integrations,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

async fn setup() -> (Store, Router) {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let keys = [
        ("viewer", Role::Viewer),
        ("operations", Role::Operations),
        ("finance", Role::Finance),
        ("integration", Role::Integration),
        ("admin", Role::Admin),
    ]
    .into_iter()
    .map(|(name, role)| {
        (
            name.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("user:{name}"),
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
async fn call(
    router: &Router,
    role: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {role}"))
                .header("idempotency-key", uuid::Uuid::new_v4().to_string())
                // These browser-supplied claims must never change the principal.
                .header("x-freight-role", "admin")
                .header("x-freight-subject", "spoofed")
                .header("x-tenant-id", "other-tenant")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}

#[tokio::test]
async fn every_protected_route_enforces_role_before_mutation_or_upstream_io() {
    let (store, router) = setup().await;
    // Independent expected policy, covering every registered protected route.
    let operations = ["operations", "admin"];
    let finance = ["finance", "admin"];
    let integration = ["integration", "admin"];
    let retries = ["operations", "integration", "admin"];
    let uploads = ["operations", "finance", "admin"];
    let readers = ["viewer", "operations", "finance", "integration", "admin"];
    let recovery = ["operations", "finance", "integration", "admin"];
    let routes: Vec<(&str, &str, &[&str])> = vec![
        ("POST", "/v1/save-recovery/resolve", &recovery),
        ("GET", "/v1/operations", &readers),
        ("GET", "/v1/loads", &readers),
        ("GET", "/v1/access", &readers),
        ("GET", "/v1/loads/missing/documents", &readers),
        ("GET", "/v1/documents/missing/preview", &readers),
        ("GET", "/v1/operations/issues/missing", &readers),
        ("POST", "/v1/operations/policy", &["admin"]),
        ("POST", "/v1/operations/reconcile", &operations),
        ("POST", "/v1/operations/issues/missing/actions", &operations),
        ("POST", "/v1/loads/missing/stop-services", &operations),
        ("POST", "/v1/stop-services/missing/correct", &operations),
        (
            "POST",
            "/v1/carrier-submissions/missing/time-corrections",
            &operations,
        ),
        ("POST", "/v1/portal/offers/missing/stop-services", &[]),
        ("POST", "/v1/carrier-contracts", &operations),
        ("POST", "/v1/carrier-contracts/missing/revise", &operations),
        (
            "POST",
            "/v1/carrier-contracts/missing/withdraw",
            &operations,
        ),
        ("GET", "/v1/carrier-contracts/missing/statement", &readers),
        ("GET", "/v1/quotes/missing/contract-rates", &readers),
        ("POST", "/v1/quotes/missing/contract-offers", &operations),
        ("POST", "/v1/quotes/missing/contract-reprocure", &operations),
        ("POST", "/v1/loads/missing/rate-confirmations", &operations),
        (
            "POST",
            "/v1/rate-confirmations/missing/handoffs",
            &operations,
        ),
        (
            "POST",
            "/v1/rate-confirmations/missing/handoffs/resolve",
            &operations,
        ),
        (
            "POST",
            "/v1/rate-confirmations/missing/handoffs/retract",
            &operations,
        ),
        ("GET", "/v1/rate-confirmations/missing/statement", &readers),
        ("POST", "/v1/loads/missing/portal-offers", &operations),
        ("GET", "/v1/portal", &[]),
        (
            "POST",
            "/v1/release-requests/missing/portal-publications",
            &operations,
        ),
        ("POST", "/v1/release-responses/missing/apply", &operations),
        ("POST", "/v1/portal/releases/missing/response", &[]),
        ("GET", "/v1/portal/releases/missing/statement", &[]),
        (
            "GET",
            "/v1/release-publications/missing/statement",
            &readers,
        ),
        ("GET", "/v1/portal/files/missing/content", &[]),
        ("POST", "/v1/documents/missing/shares", &uploads),
        ("POST", "/v1/document-shares/missing/revoke", &uploads),
        (
            "POST",
            "/v1/quotes/missing/portal-publications",
            &operations,
        ),
        ("GET", "/v1/portal/customer-quotes/missing/statement", &[]),
        ("POST", "/v1/portal/customer-quotes/missing/response", &[]),
        (
            "POST",
            "/v1/portal/customer-quotes/missing/source/refresh",
            &[],
        ),
        (
            "GET",
            "/v1/portal/rate-confirmations/missing/statement",
            &[],
        ),
        ("GET", "/v1/portal/bills/missing/statement", &[]),
        ("POST", "/v1/portal/offers/missing/response", &[]),
        ("POST", "/v1/portal/offers/missing/tracking", &[]),
        ("POST", "/v1/portal/offers/missing/pod", &[]),
        ("GET", "/v1/portal/submissions/missing/content", &[]),
        ("POST", "/v1/loads/missing/source/refresh", &operations),
        ("POST", "/v1/source-reviews/missing/resolve", &operations),
        ("POST", "/v1/quotes", &operations),
        ("POST", "/v1/quotes/missing/revise", &operations),
        ("POST", "/v1/quotes/missing/amendments", &operations),
        ("POST", "/v1/quotes/missing/offers", &operations),
        ("POST", "/v1/quotes/missing/offers/withdraw", &operations),
        ("POST", "/v1/quotes/missing/issue", &operations),
        ("POST", "/v1/quotes/missing/decision", &operations),
        ("POST", "/v1/quotes/missing/book", &operations),
        ("POST", "/v1/quotes/missing/reconfirm", &operations),
        ("POST", "/v1/quotes/missing/reprocure", &operations),
        ("GET", "/v1/quotes/missing/statement", &readers),
        ("POST", "/v1/loads/missing/cancellation-fees", &uploads),
        ("POST", "/v1/cancellation-fees/missing/review", &finance),
        ("POST", "/v1/cancellation-fees/missing/reverse", &finance),
        ("POST", "/v1/loads/missing/carrier-adjustments", &finance),
        ("POST", "/v1/carrier-adjustments/missing/review", &finance),
        ("POST", "/v1/carrier-adjustments/missing/reverse", &finance),
        ("POST", "/v1/carrier-documents/missing/dismiss", &finance),
        (
            "POST",
            "/v1/carrier-documents/missing/apply-revision",
            &finance,
        ),
        ("POST", "/v1/loads/missing/charges", &uploads),
        ("POST", "/v1/charges/missing/review", &finance),
        ("POST", "/v1/charges/missing/void", &finance),
        ("GET", "/v1/sync/orders", &operations),
        ("POST", "/v1/loads/missing/release-requests", &operations),
        ("POST", "/v1/loads/missing/appointments", &operations),
        ("POST", "/v1/appointments/missing/cancel", &operations),
        ("POST", "/v1/loads/missing/stop-events", &operations),
        ("POST", "/v1/stop-events/missing/correct", &operations),
        ("POST", "/v1/release-requests/missing/confirm", &operations),
        ("POST", "/v1/release-requests/missing/retain", &operations),
        ("POST", "/v1/tender-incidents/missing/resolve", &operations),
        ("GET", "/v1/bills/missing/statement", &readers),
        ("GET", "/v1/finance/journal.csv", &readers),
        ("POST", "/v1/bills", &finance),
        ("POST", "/v1/finance/profile", &finance),
        ("POST", "/v1/bills/missing/plan", &finance),
        ("POST", "/v1/bills/missing/supplements", &finance),
        ("POST", "/v1/bills/missing/issue", &finance),
        ("POST", "/v1/bills/missing/void", &finance),
        ("POST", "/v1/bills/missing/credits", &finance),
        ("POST", "/v1/credits/missing/reverse", &finance),
        ("POST", "/v1/cash", &finance),
        ("POST", "/v1/carrier-credit-applications", &finance),
        (
            "POST",
            "/v1/carrier-credit-applications/missing/reverse",
            &finance,
        ),
        ("POST", "/v1/cash/missing/reverse", &finance),
        ("GET", "/v1/documents/missing/content", &readers),
        (
            "POST",
            "/v1/loads/missing/documents?kind=pod&filename=evidence.pdf",
            &uploads,
        ),
        ("POST", "/v1/documents/missing/review", &finance),
        ("POST", "/v1/loads/missing/pod/review", &finance),
        ("POST", "/v1/loads", &operations),
        ("POST", "/v1/loads/missing/commands", &operations),
        ("POST", "/v1/loads/missing/plan", &operations),
        ("POST", "/v1/carriers", &operations),
        ("POST", "/v1/carriers/missing", &operations),
        ("POST", "/v1/customers", &operations),
        ("POST", "/v1/customers/missing", &operations),
        ("POST", "/v1/drivers", &operations),
        ("POST", "/v1/drivers/missing", &operations),
        ("POST", "/v1/equipment", &operations),
        ("POST", "/v1/equipment/missing", &operations),
        ("POST", "/v1/sync/import", &operations),
        ("POST", "/v1/invoices", &finance),
        ("POST", "/v1/invoices/missing/approve", &finance),
        ("POST", "/v1/invoices/missing/entry-review", &finance),
        ("POST", "/v1/invoices/missing/resolve", &finance),
        ("POST", "/v1/loads/missing/settlement", &finance),
        ("POST", "/v1/events", &integration),
        ("POST", "/v1/inbox/missing/retry", &retries),
        ("POST", "/v1/outbox/missing/retry", &retries),
    ];
    let before = json!(store.read("demo").await.expect("read"));
    for role in ["viewer", "operations", "finance", "integration", "admin"] {
        let (status, workspace) = call(&router, role, "GET", "/v1/workspace", json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(workspace["access"]["role"], role);
        assert_eq!(workspace["access"]["subject"], format!("user:{role}"));
        assert_eq!(workspace["tenant_id"], "demo");
        for (method, path, allowed) in &routes {
            let body = if *path == "/v1/operations/reconcile" {
                json!({"unexpected":true})
            } else {
                json!({})
            };
            let (status, _) = call(&router, role, method, path, body).await;
            if allowed.contains(&role) {
                assert_ne!(status, StatusCode::FORBIDDEN, "{role} {method} {path}");
                if *method == "GET"
                    && matches!(
                        *path,
                        "/v1/finance/journal.csv" | "/v1/operations" | "/v1/loads" | "/v1/access"
                    )
                {
                    assert_eq!(status, StatusCode::OK);
                } else {
                    assert_ne!(
                        status,
                        StatusCode::OK,
                        "malformed input must not mutate {path}"
                    );
                }
            } else {
                assert_eq!(status, StatusCode::FORBIDDEN, "{role} {method} {path}");
            }
        }
    }
    assert_eq!(json!(store.read("demo").await.expect("read")), before);
}

#[tokio::test]
async fn named_subject_is_audited_and_operations_cannot_approve_or_prepare_settlement() {
    let (store, router) = setup().await;
    let (status, created) = call(
        &router,
        "operations",
        "POST",
        "/v1/customers",
        json!({"name":"Scoped customer","contact":"operations@example.test"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let workspace = store.read("demo").await.expect("read");
    let entity = created["data"]["id"].as_str().expect("customer");
    assert_eq!(
        workspace
            .events
            .iter()
            .find(|v| v.entity_id == entity)
            .expect("audit")
            .actor,
        "user:operations"
    );
    let invoice = workspace
        .invoices
        .iter()
        .find(|v| v.status == "matched")
        .expect("matched demo invoice");
    let approve = format!("/v1/invoices/{}/approve", invoice.id);
    assert_eq!(
        call(&router, "operations", "POST", &approve, json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "finance", "POST", &approve, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let settlement = format!("/v1/loads/{}/settlement", invoice.load_id);
    assert_eq!(
        call(&router, "operations", "POST", &settlement, json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&router, "finance", "POST", &settlement, json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let done = store.read("demo").await.expect("read");
    assert_eq!(
        done.settlements.last().expect("settlement").status,
        "prepared"
    );
    let finance_events: Vec<_> = done
        .events
        .iter()
        .filter(|v| v.kind == "invoice.approve" || v.kind == "settlement.create")
        .collect();
    assert_eq!(finance_events.len(), 2);
    assert!(finance_events.iter().all(|v| v.actor == "user:finance"));
    assert!(store
        .read("other-tenant")
        .await
        .expect("read")
        .customers
        .is_empty());
}

#[test]
fn credential_configuration_is_explicit_backward_compatible_and_secret_safe() {
    let key = "a-test-credential-at-least-32-characters";
    let config = json!({key:{"tenant_id":"tenant-a","subject":"user:alice","role":"finance"}});
    let parsed = auth::credentials(&config.to_string()).expect("scoped config");
    assert_eq!(parsed[key].principal("fallback").role, Role::Finance);
    let legacy = auth::credentials(&json!({key:"tenant-a"}).to_string()).expect("legacy");
    assert_eq!(legacy[key].principal("fallback").role, Role::Admin);
    for principal in [
        json!({"tenant_id":"tenant-a","subject":"alice","role":"financ"}),
        json!({"tenant_id":"tenant-a","subject":"alice"}),
        json!({"tenant_id":"tenant-a","subject":" ","role":"admin"}),
        json!({"tenant_id":"../../tenant","subject":"alice","role":"admin"}),
        json!({"tenant_id":"tenant-a","subject":"alice","role":"admin","roles":["admin"]}),
    ] {
        let err =
            auth::credentials(&json!({key:principal}).to_string()).expect_err("invalid config");
        assert!(!err.contains(key));
    }
    assert!(auth::route_capability("POST", "/v1/future-sensitive-endpoint").is_none());
    for token in [
        format!("{key}\n"),
        format!("{key}\u{0000}"),
        format!("{key}é"),
    ] {
        assert!(auth::credentials(&json!({token: "tenant-a"}).to_string()).is_err());
    }
}
