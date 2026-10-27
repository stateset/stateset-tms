#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    documents::Upload,
    domain::{self, Workspace},
    integrations::Integrations,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
const PDF: &[u8] = b"%PDF-1.7\nActual synthetic receiver and shipment evidence\n%%EOF";
fn principal(who: &str) -> Principal {
    let (role, cp) = match who {
        "customer" => (Role::Customer, Some("c1")),
        "other_customer" => (Role::Customer, Some("c3")),
        "carrier" => (Role::Carrier, Some("ca4")),
        "other_carrier" => (Role::Carrier, Some("ca1")),
        "operations" => (Role::Operations, None),
        "finance" => (Role::Finance, None),
        "viewer" => (Role::Viewer, None),
        "integration" => (Role::Integration, None),
        _ => (Role::Admin, None),
    };
    Principal {
        tenant_id: "demo".into(),
        subject: format!("actor:{who}"),
        role,
        counterparty_id: cp.map(str::to_owned),
    }
}
fn router(s: Store) -> Router {
    let keys = [
        "customer",
        "other_customer",
        "carrier",
        "other_carrier",
        "operations",
        "finance",
        "viewer",
        "integration",
        "admin",
    ]
    .into_iter()
    .map(|k| (k.into(), Credential::Scoped(principal(k))))
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store: s,
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(
    r: &Router,
    who: &str,
    method: &str,
    path: &str,
    body: Value,
    key: &str,
) -> (u16, Value) {
    let r = r
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {who}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .header("x-freight-role", "admin")
                .header("x-tenant-id", "other")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = r.status().as_u16();
    let bytes = r.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "staff:operations"),
        &domain::id("key"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn version(s: &Store, id: &str) -> u64 {
    read(s)
        .await
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version
}
async fn file(s: &Store, lid: &str, kind: &str) -> String {
    s.upload_document(
        ("demo", "staff:files"),
        &domain::id("upload"),
        lid,
        Upload {
            kind: kind.into(),
            filename: format!("actual-{kind}-evidence.pdf"),
            content_type: "application/pdf".into(),
            bytes: PDF.to_vec(),
        },
    )
    .await
    .expect("upload")["data"]["id"]
        .as_str()
        .expect("document")
        .into()
}
async fn setup() -> (Store, Router, String, String) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let date = (Utc::now() + Duration::days(1)).date_naive();
    let l=op(&s,"load.create","",json!({"reference":domain::id("SHARED-FREIGHT"),"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Actual shared shipment","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":date,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":date,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":125001,"sell_cents":150001})).await;
    let lid = l["id"].as_str().expect("load").to_owned();
    let doc = file(&s, &lid, "pod").await;
    (s.clone(), router(s), lid, doc)
}
async fn input(s: &Store, lid: &str, audience: &str) -> Value {
    json!({"expected_load_version":version(s,lid).await,"expected_document_version":1,"audience":audience,"message":"Original shipment evidence for the named recipient","review_note":"PRIVATE-STAFF-CONTENT-REVIEW checked original bytes and filename","acknowledge_content":true})
}
#[tokio::test]
async fn explicit_sharing_exposes_only_named_counterparty_and_exact_bytes_with_authorization_audit_and_reversible_access(
) {
    let (s, r, lid, doc) = setup().await;
    let path = format!("documents/{doc}/shares");
    let body = input(&s, &lid, "customer").await;
    let before = json!(read(&s).await);
    for who in ["customer", "carrier", "viewer", "integration"] {
        assert_eq!(
            call(&r, who, "POST", &path, body.clone(), "denied").await.0,
            403
        );
    }
    let mut noack = body.clone();
    noack["acknowledge_content"] = json!(false);
    assert_eq!(
        call(&r, "operations", "POST", &path, noack, "no-review")
            .await
            .0,
        422
    );
    for field in ["counterparty_id", "shared_by", "role", "sha256"] {
        let mut forged = body.clone();
        forged[field] = json!("forged");
        assert_eq!(
            call(&r, "operations", "POST", &path, forged, "forged")
                .await
                .0,
            422
        );
    }
    for field in ["expected_load_version", "expected_document_version"] {
        let mut stale = body.clone();
        stale[field] = json!(0);
        assert_eq!(
            call(&r, "operations", "POST", &path, stale, "stale-version")
                .await
                .0,
            409
        );
    }
    assert_eq!(json!(read(&s).await), before);
    let result = call(&r, "operations", "POST", &path, body.clone(), "share-once").await;
    assert_eq!(result.0, 200, "{result:?}");
    let share = &result.1["data"];
    let sid = share["id"].as_str().expect("share");
    let download = format!("portal/files/{sid}/content");
    assert_eq!(
        call(&r, "operations", "POST", &path, body, "share-once")
            .await
            .1["replayed"],
        true
    );
    assert_eq!(read(&s).await.document_shares.len(), 1);
    let view = call(&r, "customer", "GET", "portal", json!({}), "view")
        .await
        .1;
    assert_eq!(view["files"][0]["available"], true);
    for private in [
        "PRIVATE-STAFF",
        "shared_by",
        "download_authorizations",
        "buy_cents",
        "150001",
        "document_id",
    ] {
        assert!(!view.to_string().contains(private), "{private}");
    }
    for who in ["other_customer", "carrier", "other_carrier"] {
        assert_eq!(
            call(&r, who, "GET", &download, json!({}), "wrong-recipient")
                .await
                .0,
            404
        );
    }
    let downloadresult = call(&r, "customer", "GET", &download, json!({}), "download").await;
    assert_eq!(downloadresult.0, 200);
    assert_eq!(downloadresult.1, String::from_utf8_lossy(PDF).to_string());
    let w = read(&s).await;
    let auth = &w.document_shares[0].download_authorizations;
    assert_eq!(auth.len(), 1);
    assert_eq!(auth[0].authorized_by, "actor:customer");
    assert!(w
        .events
        .iter()
        .any(|e| e.kind == "document.download_authorized" && e.actor == "actor:customer"));
    let revoked = call(
        &r,
        "operations",
        "POST",
        &format!("document-shares/{sid}/revoke"),
        json!({"expected_version":1,"note":"PRIVATE-REVOKE wrong disclosure decision corrected"}),
        "revoke",
    )
    .await;
    assert_eq!(revoked.0, 200);
    let before = json!(read(&s).await);
    assert_eq!(
        call(&r, "customer", "GET", &download, json!({}), "revoked")
            .await
            .0,
        404
    );
    assert_eq!(json!(read(&s).await), before);
    let view = call(&r, "customer", "GET", "portal", json!({}), "revoked-view")
        .await
        .1;
    assert_eq!(view["files"][0]["status"], "revoked");
    assert!(!view.to_string().contains("PRIVATE-REVOKE"));
    assert_eq!(
        s.document_content("demo", &doc)
            .await
            .expect("retained file")
            .1,
        PDF
    );
    assert!(read(&s).await.journal.is_empty());
    let mut new = input(&s, &lid, "customer").await;
    new["expected_previous_id"] = json!(sid);
    let republished = call(&r, "finance", "POST", &path, new, "republish").await;
    assert_eq!(republished.0, 200, "{republished:?}");
    assert_ne!(republished.1["data"]["id"], sid);
    assert_eq!(
        read(&s).await.document_shares[0]
            .download_authorizations
            .len(),
        1
    );
}
#[tokio::test]
async fn carrier_file_publication_survives_revocation_as_handoff_evidence_and_historical_carriers_never_gain_new_execution(
) {
    let (s, r, lid, doc) = setup().await;
    op(
        &s,
        "load.command",
        &lid,
        json!({"action":"tender","expected_version":version(&s,&lid).await,"carrier_id":"ca4"}),
    )
    .await;
    let w = read(&s).await;
    let original = w.loads.iter().find(|l| l.id == lid).expect("load");
    let tender = original.tender_id.clone().expect("tender");
    assert!(w.can_withdraw_tender(original));
    let result = call(
        &r,
        "operations",
        "POST",
        &format!("documents/{doc}/shares"),
        input(&s, &lid, "carrier").await,
        "carrier-file",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    let sid = result.1["data"]["id"].as_str().expect("share");
    assert_eq!(
        read(&s)
            .await
            .outbox
            .iter()
            .filter(|j| j.load_id == lid)
            .map(|j| j.total_attempts)
            .sum::<u64>(),
        0
    );
    assert!(!read(&s).await.can_withdraw_tender(
        read(&s)
            .await
            .loads
            .iter()
            .find(|l| l.id == lid)
            .expect("load")
    ));
    assert_eq!(
        call(
            &r,
            "operations",
            "POST",
            &format!("document-shares/{sid}/revoke"),
            json!({"expected_version":1,"note":"Actual file disclosure retracted"}),
            "revoke"
        )
        .await
        .0,
        200
    );
    assert!(s
        .mutate(
            "demo",
            "no-unsent",
            "load.command",
            &lid,
            json!({"action":"withdraw_tender","expected_version":version(&s,&lid).await})
        )
        .await
        .is_err());
    let release=op(&s,"tender.release.request",&lid,json!({"expected_load_version":version(&s,&lid).await,"disposition":"replan","reason":"Carrier received file access; actual release required"})).await;
    op(&s,"tender.release.confirm",release["id"].as_str().expect("release"),json!({"expected_load_version":version(&s,&lid).await,"carrier_contact":"Actual original carrier lead","confirmation_reference":"Actual no-pickup release","note":"Original carrier confirms release of this exact tender"})).await;
    op(
        &s,
        "load.command",
        &lid,
        json!({"action":"tender","expected_version":version(&s,&lid).await,"carrier_id":"ca1"}),
    )
    .await;
    let w = read(&s).await;
    assert!(w.can_withdraw_tender(w.loads.iter().find(|l| l.id == lid).expect("new tender")));
    let mut historical = input(&s, &lid, "carrier").await;
    historical["carrier_tender_id"] = json!(tender);
    historical["expected_previous_id"] = json!(sid);
    let result = call(
        &r,
        "finance",
        "POST",
        &format!("documents/{doc}/shares"),
        historical,
        "historical-file",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    let historicalid = result.1["data"]["id"].as_str().expect("historical share");
    let view = call(&r, "carrier", "GET", "portal", json!({}), "old-carrier")
        .await
        .1;
    assert!(!view.to_string().contains("load_version"));
    assert!(!view.to_string().contains("latest_tracking"));
    assert_eq!(view["files"].as_array().expect("files").len(), 2);
    assert_eq!(
        call(
            &r,
            "carrier",
            "GET",
            &format!("portal/files/{historicalid}/content"),
            json!({}),
            "own-original"
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            "other_carrier",
            "GET",
            "portal",
            json!({}),
            "new-carrier"
        )
        .await
        .1["files"],
        json!([])
    );
    let w = read(&s).await;
    assert!(w.can_withdraw_tender(w.loads.iter().find(|l| l.id == lid).expect("new tender")));
}
#[tokio::test]
async fn carrier_financial_files_require_current_finance_authority_and_never_use_customer_sharing()
{
    let (s, r, lid, _) = setup().await;
    op(
        &s,
        "load.command",
        &lid,
        json!({"action":"tender","expected_version":version(&s,&lid).await,"carrier_id":"ca4"}),
    )
    .await;
    for kind in ["carrier_invoice", "rate_confirmation"] {
        let doc = file(&s, &lid, kind).await;
        let path = format!("documents/{doc}/shares");
        assert_eq!(
            call(
                &r,
                "operations",
                "POST",
                &path,
                input(&s, &lid, "carrier").await,
                "ops-financial"
            )
            .await
            .0,
            403
        );
        assert_eq!(
            call(
                &r,
                "finance",
                "POST",
                &path,
                input(&s, &lid, "customer").await,
                "customer-financial"
            )
            .await
            .0,
            422
        );
        let result = call(
            &r,
            "finance",
            "POST",
            &path,
            input(&s, &lid, "carrier").await,
            &domain::id("finance"),
        )
        .await;
        assert_eq!(result.0, 200, "{result:?}");
        assert_eq!(result.1["data"]["shared_role"], "finance");
    }
    assert!(call(&r, "customer", "GET", "portal", json!({}), "customer")
        .await
        .1["files"]
        .as_array()
        .expect("files")
        .is_empty());
}
#[tokio::test]
async fn review_rejection_holds_shared_access_without_losing_grants_and_legacy_carrier_scope_is_adopted_explicitly(
) {
    let (s, r, lid, doc) = setup().await;
    let result = call(
        &r,
        "operations",
        "POST",
        &format!("documents/{doc}/shares"),
        input(&s, &lid, "customer").await,
        "shared",
    )
    .await;
    assert_eq!(result.0, 200);
    let sid = result.1["data"]["id"].as_str().expect("share");
    op(&s,"document.review",&doc,json!({"expected_version":1,"decision":"rejected","note":"PRIVATE-FINANCE uncertain receiver signature"})).await;
    let before = json!(read(&s).await);
    assert_eq!(
        call(
            &r,
            "customer",
            "GET",
            &format!("portal/files/{sid}/content"),
            json!({}),
            "held"
        )
        .await
        .0,
        404
    );
    assert_eq!(json!(read(&s).await), before);
    let view = call(&r, "customer", "GET", "portal", json!({}), "view")
        .await
        .1;
    assert_eq!(view["files"][0]["status"], "held");
    assert!(!view.to_string().contains("PRIVATE-FINANCE"));
    let doc = file(&s, "load_7", "bill_of_lading").await;
    let before = version(&s, "load_7").await;
    let result = call(
        &r,
        "finance",
        "POST",
        &format!("documents/{doc}/shares"),
        input(&s, "load_7", "carrier").await,
        "legacy-share",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    assert!(result.1["data"]["tender_id"].is_string());
    assert_eq!(version(&s, "load_7").await, before + 1);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_shared_file_grants_check_original_bytes_and_serialize_access_with_revocation_without_false_receipts(
) {
    let (memory, _, lid, doc) = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("sharing-pg");
    let mut data = json!(read(&memory).await);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned tenant");
    sqlx::query(
        "INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)",
    )
    .bind(&tenant)
    .bind(&doc)
    .bind(b"corrupted original file".as_slice())
    .execute(&pool)
    .await
    .expect("owned corruption");
    let body = json!({"role":"operations","request":input(&memory,&lid,"customer").await});
    let before = json!(a.read(&tenant).await.expect("before"));
    assert!(a
        .mutate_authorized(
            (&tenant, "staff:share"),
            "share-a",
            "share.create",
            &doc,
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("restore exact bytes");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "staff:share"),
            "share-a",
            "share.create",
            &doc,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "staff:share"),
            "share-b",
            "share.create",
            &doc,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let w = a.read(&tenant).await.expect("read");
    let sid = w.document_shares[0].id.clone();
    let mut customer = principal("customer");
    customer.tenant_id = tenant.clone();
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"altered bytes during download".as_slice())
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("corrupt after sharing");
    let before = json!(a.read(&tenant).await.expect("before access"));
    assert!(a.shared_document_content(&customer, &sid).await.is_err());
    assert_eq!(
        json!(a.read(&tenant).await.expect("rollback access and audit")),
        before
    );
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(PDF)
    .bind(&tenant)
    .bind(&doc)
    .execute(&pool)
    .await
    .expect("restore");
    let mut other = principal("other_customer");
    other.tenant_id = tenant.clone();
    assert!(a.shared_document_content(&other, &sid).await.is_err());
    let(download,revoked)=tokio::join!(a.shared_document_content(&customer,&sid),b.mutate_authorized((&tenant,"staff:revoke"),"revoke","share.revoke",&sid,json!({"role":"operations","request":{"expected_version":1,"note":"Actual recipient disclosure revoked"}})));
    assert!(revoked.is_ok());
    let authorized = download.is_ok();
    if let Ok((_, bytes)) = download {
        assert_eq!(bytes, PDF);
    }
    assert!(a.shared_document_content(&customer, &sid).await.is_err());
    drop((a, b));
    let reopened = Store::new(Some(&url)).await.expect("reconnect");
    let w = reopened.read(&tenant).await.expect("read");
    let share = &w.document_shares[0];
    assert_eq!(share.download_authorizations.len(), usize::from(authorized));
    if let Some(access) = share.download_authorizations.first() {
        assert!(access.authorized_at <= share.revocation.as_ref().expect("revoked").revoked_at);
        assert_eq!(access.sha256, stateset_freight::documents::sha256(PDF));
    }
    assert!(w.journal.is_empty());
    assert!(reopened
        .read("other-sharing-tenant")
        .await
        .expect("other tenant")
        .document_shares
        .is_empty());
    assert_eq!(
        reopened
            .document_content(&tenant, &doc)
            .await
            .expect("retained original")
            .1,
        PDF
    );
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup file");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup tenant");
    pool.close().await;
}
