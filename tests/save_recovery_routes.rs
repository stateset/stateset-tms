use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    integrations::Integrations,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;

#[tokio::test]
async fn every_supported_original_route_uses_its_actual_api_receipt_fingerprint() {
    let store = Store::new(None).await.expect("owned store");
    let router = api::router(App {
        pdf_renderer: Default::default(),
        store: store.clone(),
        keys: Arc::new(BTreeMap::from([("key".into(), "tenant".into())])),
        demo: false,
        identity: Arc::new(None),
        integrations: Arc::new(Integrations::default()),
    });
    for path in [
        "loads",
        "loads/id/plan",
        "loads/id/commands",
        "loads/id/settlement",
        "loads/id/pod/review",
        "sync/import",
        "commerce/import",
        "loads/id/commerce-tracking",
        "outbox/id/commerce-review",
        "invoices",
        "invoices/id/resolve",
        "invoices/id/approve",
        "invoices/id/entry-review",
        "customers",
        "customers/id",
        "carriers",
        "carriers/id",
        "drivers",
        "drivers/id",
        "equipment",
        "equipment/id",
        "inbox/id/retry",
        "outbox/id/retry",
    ] {
        let key = uuid::Uuid::new_v4().to_string();
        let original = if matches!(path, "sync/import" | "commerce/import") {
            json!({"order_id":"00000000-0000-4000-8000-000000000001","load":{}})
        } else {
            json!({})
        };
        let resolution = json!({"request_key":key,"path":path,"body":original,"note":"Exact original route binding qualification"});
        for (uri, body, expected) in [
            ("/v1/save-recovery/resolve".to_string(), resolution, 200),
            (format!("/v1/{path}"), original, 409),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(&uri)
                        .header("authorization", "Bearer key")
                        .header("content-type", "application/json")
                        .header("idempotency-key", &key)
                        .body(Body::from(body.to_string()))
                        .expect("request"),
                )
                .await
                .expect("actual handler");
            let status = response.status().as_u16();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            let result: Value = serde_json::from_slice(&bytes).expect("JSON result");
            assert_eq!(status, expected, "{path}: {result}");
            if status == 409 {
                assert_eq!(
                    result["error"]["code"], "request_stopped",
                    "{path}: {result}"
                );
            }
        }
    }
    let after = store.read("tenant").await.expect("source");
    assert_eq!(after.receipts.len(), 23);
    assert_eq!(after.events.len(), 23);
    assert!(after
        .events
        .iter()
        .all(|event| event.kind == "request.stopped"));
    assert!(
        after.customers.is_empty()
            && after.loads.is_empty()
            && after.invoices.is_empty()
            && after.outbox.is_empty()
    );
}
