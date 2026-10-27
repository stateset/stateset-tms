use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request, StatusCode},
    routing::get,
    Json, Router,
};
use http_body_util::BodyExt;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    identity::{ClerkConfig, ClerkIdentity},
    integrations::Integrations,
    store::Store,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::RwLock;
use tower::ServiceExt;

const SECRET: &str = "sk_test_owned_fixture_only_at_least_32_characters";
const ORIGIN: &str = "https://freight.example.test";
const STATIC: &str = "static-as-jwt.workspace-credential-explicit.signature";
#[derive(Clone)]
struct Provider {
    status: String,
    role: String,
    member: bool,
    active_org: String,
    keys: Vec<Value>,
    outage: bool,
    key_reads: u64,
}
async fn jwks(
    State(state): State<Arc<RwLock<Provider>>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    assert!(
        headers.get("authorization").is_none(),
        "BAPI secret must not be sent to issuer JWKS"
    );
    let mut p = state.write().await;
    p.key_reads += 1;
    (
        if p.outage {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        },
        Json(json!({"keys":p.keys})),
    )
}
async fn session(
    State(state): State<Arc<RwLock<Provider>>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    assert_eq!(
        headers
            .get("authorization")
            .expect("private BAPI authorization"),
        format!("Bearer {SECRET}").as_str()
    );
    let p = state.read().await;
    (
        if p.outage {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        },
        Json(
            json!({"id":id,"user_id":"user_alice","status":p.status,"last_active_organization_id":p.active_org,"actor":null,"expire_at":chrono::Utc::now().timestamp_millis()+3600000}),
        ),
    )
}
async fn members(
    State(state): State<Arc<RwLock<Provider>>>,
    Path(org): Path<String>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    assert_eq!(
        headers
            .get("authorization")
            .expect("private BAPI authorization"),
        format!("Bearer {SECRET}").as_str()
    );
    assert_eq!(query.get("user_id").map(String::as_str), Some("user_alice"));
    let p = state.read().await;
    let data = if p.member {
        vec![
            json!({"organization":{"id":org},"public_user_data":{"user_id":"user_alice"},"role":p.role}),
        ]
    } else {
        vec![]
    };
    (
        if p.outage {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::OK
        },
        Json(json!({"total_count":data.len(),"data":data})),
    )
}
fn config(issuer: &str) -> ClerkConfig {
    serde_json::from_value(json!({"issuer":issuer,"secret_key":SECRET,"fixture_api_url":issuer,"authorized_parties":[ORIGIN],
        "organizations":{"org_alpha":{"tenant_id":"demo","roles":{"org:admin":"viewer","org:operations":"operations","org:finance":"finance"}},"org_beta":{"tenant_id":"other","roles":{"org:operations":"operations"}}}})).expect("config")
}
struct Fixture {
    router: Router,
    store: Store,
    provider: Arc<RwLock<Provider>>,
    issuer: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture() -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let issuer = format!("http://{}", listener.local_addr().expect("address"));
    let provider = Arc::new(RwLock::new(Provider {
        status: "active".into(),
        role: "org:operations".into(),
        member: true,
        active_org: "org_alpha".into(),
        keys: vec![
            serde_json::from_str(include_str!("fixtures/identity/key-a.json")).expect("JWK"),
        ],
        outage: false,
        key_reads: 0,
    }));
    let upstream = Router::new()
        .route("/.well-known/jwks.json", get(jwks))
        .route("/sessions/{id}", get(session))
        .route("/organizations/{org}/memberships", get(members))
        .with_state(provider.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, upstream)
            .await
            .expect("fixture server");
    });
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let router = api::router(App {
        pdf_renderer: Default::default(),
        store: store.clone(),
        identity: Arc::new(Some(
            ClerkIdentity::new(config(&issuer), true).expect("identity"),
        )),
        keys: Arc::new(BTreeMap::from([(
            STATIC.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: "service:explicit".into(),
                role: Role::Admin,
                counterparty_id: None,
            }),
        )])),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    });
    Fixture {
        router,
        store,
        provider,
        issuer,
        task,
    }
}
fn claims(issuer: &str) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({"iss":issuer,"azp":ORIGIN,"sub":"user_alice","sid":"sess_alice","iat":now,"nbf":now-5,"exp":now+60,"v":2,
        "o":{"id":"org_alpha","rol":"admin","per":"all"},"tenant_id":"forged","role":"admin"})
}
fn token(body: &Value, key: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(format!("fixture-{key}"));
    encode(
        &header,
        body,
        &EncodingKey::from_rsa_der(if key == "a" {
            include_bytes!("fixtures/identity/key-a.der")
        } else {
            include_bytes!("fixtures/identity/key-b.der")
        }),
    )
    .expect("sign")
}
async fn call(
    f: &Fixture,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
    org: Option<&str>,
    clerk_only: bool,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header(
            "idempotency-key",
            if path == "/v1/customers" {
                "staff-action-key"
            } else {
                "financial-action-key"
            },
        )
        .header("x-freight-role", "admin")
        .header("x-tenant-id", "forged");
    if let Some(org) = org {
        request = request.header("x-freight-organization-id", org);
    }
    if clerk_only {
        request = request.header("x-freight-identity-mode", "clerk");
    }
    let response = f
        .router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("JSON"))
}

#[tokio::test]
async fn clerk_roles_tenants_audit_and_revocation_come_from_live_membership_not_claims_or_replay() {
    let f = fixture().await;
    let jwt = token(&claims(&f.issuer), "a");
    let (status, w) = call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w["tenant_id"], "demo");
    assert_eq!(w["access"]["role"], "operations");
    assert_eq!(w["access"]["subject"], "clerk:user_alice");
    assert_eq!(
        call(&f, &jwt, "POST", "/v1/bills", json!({}), None, false)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "JWT admin claim must not grant finance"
    );
    let customer = json!({"name":"Verified staff customer","contact":"ap@example.test"});
    assert_eq!(
        call(
            &f,
            &jwt,
            "POST",
            "/v1/customers",
            customer.clone(),
            Some("org_alpha"),
            false
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.store
            .read("demo")
            .await
            .expect("audit")
            .events
            .last()
            .expect("event")
            .actor,
        "clerk:user_alice"
    );
    f.provider.write().await.role = "org:admin".into();
    assert_eq!(
        call(
            &f,
            &jwt,
            "POST",
            "/v1/customers",
            customer.clone(),
            None,
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "downgrade applies before idempotency replay"
    );
    f.provider.write().await.role = "org:finance".into();
    assert_eq!(
        call(&f, &jwt, "POST", "/v1/customers", customer, None, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f, &jwt, "POST", "/v1/bills", json!({}), None, false)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "explicit finance grant reaches financial validation"
    );
    f.provider.write().await.member = false;
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.provider.write().await.member = true;
    f.provider.write().await.status = "revoked".into();
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    {
        let mut p = f.provider.write().await;
        p.status = "active".into();
        p.role = "org:operations".into();
        p.active_org = "org_beta".into();
    }
    let mut beta = claims(&f.issuer);
    beta["o"]["id"] = json!("org_beta");
    let beta = token(&beta, "a");
    let (status, w) = call(
        &f,
        &beta,
        "GET",
        "/v1/workspace",
        json!({}),
        Some("org_beta"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w["tenant_id"], "other");
    assert_eq!(w["customers"], json!([]));
    assert_eq!(
        call(
            &f,
            &beta,
            "POST",
            "/v1/customers",
            json!({"name":"Stale UI","contact":"test@example.test"}),
            Some("org_alpha"),
            false
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "old active-org token is rejected after switching"
    );
}

#[tokio::test]
async fn clerk_rejects_forged_wrong_origin_expired_pending_impersonated_and_unscoped_tokens_without_mutation(
) {
    let f = fixture().await;
    let base = claims(&f.issuer);
    let now = chrono::Utc::now().timestamp();
    let before = json!(f.store.read("demo").await.expect("read"));
    for (field, value) in [
        ("iss", json!("https://other-issuer.example")),
        ("azp", json!("https://other-app.example")),
        ("exp", json!(now - 1)),
        ("nbf", json!(now + 30)),
        ("iat", json!(i64::MIN)),
        ("iat", json!(now + 30)),
        ("exp", json!(now + 3600)),
        ("v", json!(1)),
        ("sts", json!("pending")),
        ("act", json!({"sub":"user_impersonator"})),
        ("sid", json!("../sessions/admin")),
        ("aud", json!("another-audience")),
        ("o", json!({"id":"org_unknown"})),
        ("o", Value::Null),
    ] {
        let mut bad = base.clone();
        bad[field] = value;
        let status = call(
            &f,
            &token(&bad, "a"),
            "POST",
            "/v1/customers",
            json!({"name":"Not permitted","contact":"test@example.test"}),
            None,
            false,
        )
        .await
        .0;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{field}: {status}"
        );
    }
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("fixture-a".into());
    let forged =
        encode(&header, &base, &EncodingKey::from_secret(SECRET.as_bytes())).expect("HMAC test");
    assert_eq!(
        call(&f, &forged, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut bad = token(&base, "b");
    let parts: Vec<_> = bad.split('.').collect();
    let correct = token(&base, "a");
    bad = format!(
        "{}.{}.{}",
        correct.split('.').next().expect("header"),
        parts[1],
        parts[2]
    );
    assert_eq!(
        call(&f, &bad, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json!(f.store.read("demo").await.expect("unchanged")),
        before
    );
    assert_eq!(
        call(&f, STATIC, "GET", "/v1/workspace", json!({}), None, true)
            .await
            .0,
        StatusCode::UNAUTHORIZED,
        "SSO gateway cannot fall back to an administrator API credential"
    );
    assert_eq!(
        call(&f, STATIC, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::OK,
        "explicit service credentials remain compatible"
    );
}

#[tokio::test]
async fn clerk_key_rotation_and_provider_outages_fail_closed_without_stale_membership_grants() {
    let f = fixture().await;
    let jwt = token(&claims(&f.issuer), "a");
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.provider.read().await.key_reads, 1);
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        f.provider.read().await.key_reads,
        1,
        "JWKS is cached, membership/session grants are not"
    );
    f.provider.write().await.keys =
        vec![
            serde_json::from_str(include_str!("fixtures/identity/key-b.json"))
                .expect("rotated public key"),
        ];
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let rotated = token(&claims(&f.issuer), "b");
    assert_eq!(
        call(&f, &rotated, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.provider.read().await.key_reads, 2);
    assert_eq!(
        call(&f, &jwt, "GET", "/v1/workspace", json!({}), None, false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    f.provider.write().await.outage = true;
    let before = json!(f.store.read("demo").await.expect("before"));
    assert_eq!(
        call(
            &f,
            &rotated,
            "POST",
            "/v1/customers",
            json!({"name":"Offline grant denied","contact":"test@example.test"}),
            None,
            false
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(json!(f.store.read("demo").await.expect("after")), before);
    f.provider.write().await.outage = false;
    let mut audience_config = config(&f.issuer);
    audience_config.audience = Some("freight".into());
    let verifier = ClerkIdentity::new(audience_config, true).expect("audience config");
    let mut audience_claims = claims(&f.issuer);
    audience_claims["aud"] = json!(["freight"]);
    assert!(verifier.verify(&token(&audience_claims, "b")).await.is_ok());
    audience_claims["aud"] = json!(["other-application"]);
    assert!(verifier
        .verify(&token(&audience_claims, "b"))
        .await
        .is_err());
}

#[test]
fn clerk_configuration_rejects_ambiguous_or_insecure_identity_sources() {
    assert!(ClerkIdentity::new(config("http://127.0.0.1:9999"), false).is_err());
    for (field, value) in [
        ("issuer", json!("https://issuer.example/path")),
        (
            "authorized_parties",
            json!(["https://freight.example.test/path"]),
        ),
        ("organizations", json!({})),
        ("secret_key", json!("short")),
        ("arbitrary_issuer", json!("forged")),
    ] {
        let mut input=serde_json::to_value(json!({"issuer":"https://issuer.example","secret_key":SECRET,"authorized_parties":[ORIGIN],"organizations":{"org_alpha":{"tenant_id":"demo","roles":{"org:staff":"operations"}}}})).expect("JSON");
        input[field] = value;
        assert!(
            serde_json::from_value::<ClerkConfig>(input)
                .ok()
                .and_then(|c| ClerkIdentity::new(c, false).ok())
                .is_none(),
            "{field}"
        );
    }
}

#[tokio::test]
async fn signed_partner_membership_fixes_counterparty_scope_and_live_revocation_never_grants_staff_access(
) {
    let mut f = fixture().await;
    let mut cfg = config(&f.issuer);
    let grant = cfg.organizations.get_mut("org_alpha").expect("grant");
    grant.roles = BTreeMap::from([("org:carrier".into(), Role::Carrier)]);
    grant.counterparty_id = Some("ca4".into());
    f.router = api::router(App {
        pdf_renderer: Default::default(),
        store: f.store.clone(),
        identity: Arc::new(Some(
            ClerkIdentity::new(cfg, true).expect("partner identity"),
        )),
        keys: Arc::new(BTreeMap::new()),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    });
    f.provider.write().await.role = "org:carrier".into();
    let mut forged = claims(&f.issuer);
    forged["counterparty_id"] = json!("ca1");
    forged["role"] = json!("admin");
    let jwt = token(&forged, "a");
    let (status, view) = call(
        &f,
        &jwt,
        "GET",
        "/v1/portal",
        json!({}),
        Some("org_alpha"),
        true,
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["name"], "Blue Ridge Express");
    assert_eq!(view["access"]["subject"], "clerk:user_alice");
    assert_eq!(
        call(
            &f,
            &jwt,
            "GET",
            "/v1/workspace",
            json!({}),
            Some("org_alpha"),
            true
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &f,
            &jwt,
            "GET",
            "/v1/portal",
            json!({}),
            Some("org_beta"),
            true
        )
        .await
        .0,
        403
    );
    f.provider.write().await.role = "org:operations".into();
    assert_eq!(
        call(
            &f,
            &jwt,
            "GET",
            "/v1/portal",
            json!({}),
            Some("org_alpha"),
            true
        )
        .await
        .0,
        403
    );
    f.provider.write().await.role = "org:carrier".into();
    f.provider.write().await.member = false;
    assert_eq!(
        call(
            &f,
            &jwt,
            "GET",
            "/v1/portal",
            json!({}),
            Some("org_alpha"),
            true
        )
        .await
        .0,
        403
    );
    f.provider.write().await.member = true;
    f.provider.write().await.status = "revoked".into();
    assert_eq!(
        call(
            &f,
            &jwt,
            "GET",
            "/v1/portal",
            json!({}),
            Some("org_alpha"),
            true
        )
        .await
        .0,
        401
    );
}
#[test]
fn clerk_partner_organizations_cannot_mix_staff_and_partner_roles_or_omit_scope() {
    let mut cfg = config("http://127.0.0.1:9911");
    let grant = cfg.organizations.get_mut("org_alpha").expect("grant");
    grant.roles = BTreeMap::from([("org:carrier".into(), Role::Carrier)]);
    assert!(ClerkIdentity::new(cfg, true).is_err());
    let mut cfg = config("http://127.0.0.1:9911");
    let grant = cfg.organizations.get_mut("org_alpha").expect("grant");
    grant.counterparty_id = Some("ca4".into());
    assert!(ClerkIdentity::new(cfg, true).is_err());
}
