use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    integrations::Integrations,
    save_recovery::{ResolveSave, REQUEST_STOPPED},
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;
fn principal(tenant: &str, role: Role) -> Principal {
    Principal {
        tenant_id: tenant.into(),
        subject: format!("verified:{role:?}"),
        role,
        counterparty_id: match role {
            Role::Carrier => Some("ca4".into()),
            Role::Customer => Some("c1".into()),
            _ => None,
        },
    }
}
fn input(key: &str, path: &str, body: Value) -> ResolveSave {
    ResolveSave {
        request_key: key.into(),
        path: path.into(),
        body,
        note: "Operator resolved the interrupted original save".into(),
    }
}
fn body() -> Value {
    json!({"name":"Owned original customer","contact":"owned@example.test"})
}
fn router(store: Store) -> Router {
    let keys = [
        ("ops", Role::Operations),
        ("finance", Role::Finance),
        ("integration", Role::Integration),
        ("viewer", Role::Viewer),
        ("admin", Role::Admin),
        ("carrier", Role::Carrier),
        ("customer", Role::Customer),
    ]
    .into_iter()
    .map(|(key, role)| (key.into(), Credential::Scoped(principal("tenant", role))))
    .chain([(
        "other".into(),
        Credential::Scoped(principal("other", Role::Operations)),
    )])
    .collect::<BTreeMap<_, _>>();
    api::router(App {
        pdf_renderer: Default::default(),
        store,
        keys: Arc::new(keys),
        demo: false,
        identity: Arc::new(None),
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(router: &Router, token: &str, path: &str, key: &str, body: Value) -> (u16, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .header("x-tenant-id", "forged")
                .header("x-freight-role", "admin")
                .body(Body::from(body.to_string()))
                .expect("owned save-resolution fixture"),
        )
        .await
        .expect("owned save-resolution fixture");
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("owned save-resolution fixture")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
#[tokio::test]
async fn stopped_original_never_applies_and_resolution_replays_original_decision() {
    let store = Store::new(None)
        .await
        .expect("owned save-resolution fixture");
    let ops = principal("tenant", Role::Operations);
    let stopped = store
        .resolve_save(&ops, input("original", "customers", body()))
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(stopped["data"]["outcome"], "stopped");
    assert_eq!(stopped["data"]["cancellation"]["stopped_by"], ops.subject);
    let before = json!(store
        .read("tenant")
        .await
        .expect("owned save-resolution fixture"));
    for _ in 0..8 {
        assert_eq!(
            store
                .mutate("tenant", "original", "customer.create", "", body())
                .await
                .expect_err("original request must be rejected"),
            REQUEST_STOPPED
        );
    }
    let mut repeated = input("original", "customers", body());
    repeated.note = "A later note must not overwrite original resolution".into();
    let result = store
        .resolve_save(&ops, repeated)
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(result["data"], stopped["data"]);
    assert_eq!(result["replayed"], true);
    assert_eq!(
        json!(store
            .read("tenant")
            .await
            .expect("owned save-resolution fixture")),
        before
    );
    assert!(store
        .resolve_save(
            &ops,
            input("original", "customers", json!({"name":"Different input"}))
        )
        .await
        .expect_err("original request must be rejected")
        .contains("already used"));
    let r = router(store.clone());
    let result = call(&r, "ops", "/v1/customers", "original", body()).await;
    assert_eq!(result.0, 409);
    assert_eq!(result.1["error"]["code"], "request_stopped");
    assert!(store
        .read("tenant")
        .await
        .expect("owned save-resolution fixture")
        .customers
        .is_empty());
    let fresh = store
        .mutate("tenant", "new-decision", "customer.create", "", body())
        .await
        .expect("owned save-resolution fixture");
    assert!(!fresh["data"]["id"]
        .as_str()
        .expect("owned save-resolution fixture")
        .is_empty());
    let other = call(&r, "other", "/v1/customers", "original", body()).await;
    assert_eq!(other.0, 200);
    assert_eq!(
        store
            .read("tenant")
            .await
            .expect("owned save-resolution fixture")
            .customers
            .len(),
        1
    );
    assert_eq!(
        store
            .read("other")
            .await
            .expect("owned save-resolution fixture")
            .customers
            .len(),
        1
    );
}
#[tokio::test]
async fn completed_original_is_retained_instead_of_stopped() {
    let store = Store::new(None)
        .await
        .expect("owned save-resolution fixture");
    let ops = principal("tenant", Role::Operations);
    let applied = store
        .mutate_authorized(
            ("tenant", &ops.subject),
            "original",
            "customer.create",
            "",
            body(),
        )
        .await
        .expect("owned save-resolution fixture");
    let before = json!(store
        .read("tenant")
        .await
        .expect("owned save-resolution fixture"));
    let result = store
        .resolve_save(&ops, input("original", "customers", body()))
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(result["data"]["outcome"], "completed");
    assert_eq!(result["data"]["result"], applied["data"]);
    assert_eq!(
        json!(store
            .read("tenant")
            .await
            .expect("owned save-resolution fixture")),
        before
    );
    let replay = store
        .mutate("tenant", "original", "customer.create", "", body())
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(replay["data"], applied["data"]);
    assert_eq!(replay["replayed"], true);
}
#[tokio::test]
async fn recovery_authorization_tracks_original_route_and_rejects_forged_envelopes() {
    let store = Store::new(None)
        .await
        .expect("owned save-resolution fixture");
    let r = router(store.clone());
    for (path, allowed) in [
        ("customers", vec!["ops", "admin"]),
        ("invoices", vec!["finance", "admin"]),
        ("outbox/job/retry", vec!["ops", "integration", "admin"]),
    ] {
        for token in [
            "ops",
            "finance",
            "integration",
            "viewer",
            "admin",
            "carrier",
            "customer",
        ] {
            let key = uuid::Uuid::new_v4().to_string();
            let result=call(&r,token,"/v1/save-recovery/resolve","resolution",json!({"request_key":key,"path":path,"body":{},"note":"Original request resolution"})).await;
            assert_eq!(
                result.0,
                if allowed.contains(&token) { 200 } else { 403 },
                "{token} {path} {result:?}"
            );
        }
    }
    let before = json!(store
        .read("tenant")
        .await
        .expect("owned save-resolution fixture"));
    for path in [
        "/customers",
        "customers/../invoices",
        "customers?x=1",
        "customers/%63",
        "customers/x/",
        "documents/id/content",
        "portal/offers/id/response",
        "events",
        "save-recovery/resolve",
    ] {
        assert_eq!(
            call(
                &r,
                "admin",
                "/v1/save-recovery/resolve",
                "resolution",
                json!({"request_key":"invalid","path":path,"body":{},"note":"Invalid path denied"})
            )
            .await
            .0,
            422,
            "{path}"
        );
    }
    for key in ["", "bad\nkey", "clé"] {
        assert_eq!(
            call(
                &r,
                "ops",
                "/v1/save-recovery/resolve",
                "resolution",
                json!({"request_key":key,"path":"customers","body":{},"note":"Bad key denied"})
            )
            .await
            .0,
            422
        );
    }
    assert_eq!(call(&r,"ops","/v1/save-recovery/resolve","resolution",json!({"request_key":"forged","path":"customers","body":{},"note":"No forged actor","actor":"admin","tenant_id":"other"})).await.0,422);
    assert_eq!(call(&r,"admin","/v1/save-recovery/resolve","resolution",json!({"request_key":"nonempty","path":"invoices/id/approve","body":{"ignored":"would not enter original fingerprint"},"note":"Invalid original envelope"})).await.0,422);
    assert_eq!(
        json!(store
            .read("tenant")
            .await
            .expect("owned save-resolution fixture")),
        before
    );
}
async fn race(a: Store, b: Store, tenant: &str) {
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let mut handles = Vec::new();
    for i in 0..16 {
        let store = if i % 2 == 0 { a.clone() } else { b.clone() };
        let barrier = barrier.clone();
        let tenant = tenant.to_string();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            if i % 2 == 0 {
                store
                    .resolve_save(
                        &principal(&tenant, Role::Operations),
                        input("racing", "customers", body()),
                    )
                    .await
                    .map(|v| {
                        v["data"]["outcome"]
                            .as_str()
                            .expect("owned save-resolution fixture")
                            .to_owned()
                    })
            } else {
                store
                    .mutate(&tenant, "racing", "customer.create", "", body())
                    .await
                    .map(|_| "completed".into())
            }
        }));
    }
    let mut resolved = None;
    for h in handles {
        match h.await.expect("owned save-resolution fixture") {
            Ok(value) => {
                if let Some(previous) = &resolved {
                    assert_eq!(previous, &value);
                } else {
                    resolved = Some(value);
                }
            }
            Err(error) => assert_eq!(error, REQUEST_STOPPED),
        }
    }
    let w = a.read(tenant).await.expect("owned save-resolution fixture");
    let stopped = w.receipts["racing"].stopped.is_some();
    assert_eq!(w.customers.len(), if stopped { 0 } else { 1 });
    assert_eq!(
        resolved.as_deref(),
        Some(if stopped { "stopped" } else { "completed" })
    );
    assert_eq!(
        w.events
            .iter()
            .filter(|e| e.kind == "request.stopped")
            .count(),
        usize::from(stopped)
    );
}
#[tokio::test]
async fn competing_originals_and_resolvers_have_one_consistent_outcome() {
    let store = Store::new(None)
        .await
        .expect("owned save-resolution fixture");
    for i in 0..16 {
        race(store.clone(), store.clone(), &format!("race-{i}")).await;
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_resolution_races_survive_reconnect_without_replay_updates() {
    use sqlx::PgPool;
    let base = std::env::var("TEST_DATABASE_URL").expect("owned save-resolution fixture");
    let admin = PgPool::connect(&base)
        .await
        .expect("owned save-resolution fixture");
    let schema = format!("freight_resolution_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("owned save-resolution fixture");
    let mut url = reqwest::Url::parse(&base).expect("owned save-resolution fixture");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let url = url.to_string();
    let a = Store::new(Some(&url))
        .await
        .expect("owned save-resolution fixture");
    let b = Store::new(Some(&url))
        .await
        .expect("owned save-resolution fixture");
    let pool = PgPool::connect(&url)
        .await
        .expect("owned save-resolution fixture");
    let ops = principal("stopped", Role::Operations);
    let original = a
        .resolve_save(&ops, input("original", "customers", body()))
        .await
        .expect("owned save-resolution fixture");
    let raw: Value =
        sqlx::query_scalar("SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id='stopped'")
            .fetch_one(&pool)
            .await
            .expect("owned save-resolution fixture");
    let row: (String,String,String)=sqlx::query_as("SELECT xmin::text,ctid::text,updated_at::text FROM freight_workspaces WHERE tenant_id='stopped'").fetch_one(&pool).await.expect("owned save-resolution fixture");
    for _ in 0..8 {
        assert_eq!(
            b.resolve_save(&ops, input("original", "customers", body()))
                .await
                .expect("owned save-resolution fixture")["data"],
            original["data"]
        );
        assert_eq!(
            b.mutate("stopped", "original", "customer.create", "", body())
                .await
                .expect_err("original request must be rejected"),
            REQUEST_STOPPED
        );
    }
    let after: Value =
        sqlx::query_scalar("SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id='stopped'")
            .fetch_one(&pool)
            .await
            .expect("owned save-resolution fixture");
    let position: (String,String,String)=sqlx::query_as("SELECT xmin::text,ctid::text,updated_at::text FROM freight_workspaces WHERE tenant_id='stopped'").fetch_one(&pool).await.expect("owned save-resolution fixture");
    assert_eq!(after, raw);
    assert_eq!(position, row);
    let done = principal("completed", Role::Operations);
    let applied = a
        .mutate("completed", "original", "customer.create", "", body())
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(
        b.resolve_save(&done, input("original", "customers", body()))
            .await
            .expect("owned save-resolution fixture")["data"]["result"],
        applied["data"]
    );
    for i in 0..8 {
        race(a.clone(), b.clone(), &format!("race-{i}")).await;
    }
    drop(a);
    drop(b);
    let restored = Store::new(Some(&url))
        .await
        .expect("owned save-resolution fixture");
    assert_eq!(
        restored
            .resolve_save(&ops, input("original", "customers", body()))
            .await
            .expect("owned save-resolution fixture")["data"],
        original["data"]
    );
    assert_eq!(
        restored
            .mutate("stopped", "original", "customer.create", "", body())
            .await
            .expect_err("original request must be rejected"),
        REQUEST_STOPPED
    );
    assert_eq!(
        restored
            .mutate("completed", "original", "customer.create", "", body())
            .await
            .expect("owned save-resolution fixture")["data"],
        applied["data"]
    );
    let mut previous_release = Value::Null;
    if let Ok(binary) = std::env::var("FREIGHT_TEST_PREVIOUS_BINARY") {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::process::Command::new(binary)
                .env_clear()
                .env("DATABASE_URL", &url)
                .env("FREIGHT_DEMO", "false")
                .env(
                    "FREIGHT_API_KEYS_JSON",
                    r#"{"owned-resolution-check-token-at-least-32-chars":"stopped"}"#,
                )
                .env("FREIGHT_BIND_ADDR", "127.0.0.1:0")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .expect("obsolete binary must refuse startup")
        .expect("owned save-resolution fixture");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            stderr.contains("migration history") || stderr.contains("format"),
            "{stderr}"
        );
        previous_release =
            json!({"accepted":false,"stderr":stderr,"exit_code":output.status.code()});
    }
    if let Ok(path) = std::env::var("FREIGHT_SAVE_RESOLUTION_PROOF") {
        let outcomes:Vec<(String,bool)>=sqlx::query_as("SELECT tenant_id,freight_canonical_receipt(tenant_id,data,'racing')->'stopped' IS NOT NULL FROM freight_workspaces WHERE tenant_id LIKE 'race-%' ORDER BY tenant_id").fetch_all(&pool).await.expect("owned save-resolution fixture");
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"result":"pass","storage_format":18,"migration_count":25,"physical_before":row,"physical_after":position,"stopped_receipt":raw["receipts"]["original"],"same_raw_workspace_after_replays":raw==after,"reconnected_stop":original["data"],"completed_original":applied["data"],"racing_outcomes":outcomes,"previous_release":previous_release})).expect("owned save-resolution fixture")).expect("owned save-resolution fixture");
    }
    drop(restored);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned save-resolution fixture");
    admin.close().await;
}
