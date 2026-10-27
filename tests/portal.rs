#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{self, Capability, Credential, Principal, Role},
    domain::{self, Load},
    integrations::Integrations,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
async fn run(s: &Store, op: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(("demo", "staff:ops"), &domain::id("key"), op, id, body)
        .await
        .expect(op)["data"]
        .clone()
}
async fn load(s: &Store, id: &str) -> Load {
    s.read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|v| v.id == id)
        .expect("load")
}
fn router(s: Store, tenant: &str) -> Router {
    let keys = [
        ("carrier", Role::Carrier, Some("ca4")),
        ("other_carrier", Role::Carrier, Some("ca1")),
        ("second_carrier", Role::Carrier, Some("ca4")),
        ("customer", Role::Customer, Some("c1")),
        ("other_customer", Role::Customer, Some("c3")),
        ("staff", Role::Admin, None),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: tenant.into(),
                subject: format!("partner:{key}"),
                role,
                counterparty_id: cp.map(str::to_owned),
            }),
        )
    })
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
    let result = r
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
    let status = result.status().as_u16();
    let bytes = result.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}
async fn setup() -> (Store, Router, String, Value) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let day = (Utc::now() + Duration::days(1)).date_naive();
    let l=run(&s,"load.create","",json!({"reference":domain::id("PORTAL-SHIPMENT"),"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Public operational goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"},"buy_cents":125001,"sell_cents":150001})).await;
    let id = l["id"].as_str().expect("id").to_owned();
    run(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":1,"carrier_id":"ca4"}),
    )
    .await;
    let rate=run(&s,"rate.issue",&id,json!({"expected_load_version":load(&s,&id).await.version,"terms":"Actual carrier-facing transport terms","reason":"PRIVATE-STAFF-REASON-DO-NOT-PUBLISH"})).await;
    (s.clone(), router(s, "demo"), id, rate)
}
async fn publish(s: &Store, id: &str, rate: &Value, prior: Option<&str>) -> Value {
    run(s,"portal.publish",id,json!({"expected_load_version":load(s,id).await.version,"rate_confirmation_id":rate["id"],"expected_previous_id":prior})).await
}
async fn response(s: &Store, id: &str, rate: &Value, decision: &str) -> Value {
    json!({"expected_load_version":load(s,id).await.version,"rate_confirmation_id":rate["id"],"rate_sha256":rate["sha256"],"decision":decision,"reference":"Carrier native response reference","note":"Authorized carrier lead reviewed this exact published rate revision"})
}
#[test]
fn partner_credentials_require_exact_server_owned_scope_and_cannot_gain_staff_capabilities() {
    let token = "p".repeat(40);
    for body in [
        json!({"tenant_id":"demo","subject":"carrier:user","role":"carrier"}),
        json!({"tenant_id":"demo","subject":"carrier:user","role":"carrier","counterparty_id":"../ca4"}),
        json!({"tenant_id":"demo","subject":"user","role":"admin","counterparty_id":"ca4"}),
    ] {
        assert!(auth::credentials(&json!({token.clone():body}).to_string()).is_err());
    }
    assert!(auth::credentials(&json!({token:{"tenant_id":"demo","subject":"carrier:user","role":"carrier","counterparty_id":"ca4"}}).to_string()).is_ok());
    for role in [Role::Carrier, Role::Customer] {
        assert!(!role.allows(Capability::Read));
        assert!(!role.allows(Capability::FinanceWrite));
        assert!(!role.allows(Capability::OperationsWrite));
        assert!(role.allows(Capability::PortalRead));
    }
    assert!(Role::Carrier.allows(Capability::CarrierRespond));
    assert!(!Role::Customer.allows(Capability::CarrierRespond));
    assert!(!Role::Admin.allows(Capability::PortalRead));
}
#[tokio::test]
async fn partner_projections_hide_unpublished_offers_other_parties_and_staff_roots_even_when_claim_headers_are_forged(
) {
    let (s, r, id, rate) = setup().await;
    assert_eq!(
        call(&r, "carrier", "GET", "portal", json!({}), "read")
            .await
            .1["offers"],
        json!([])
    );
    let download = format!(
        "portal/rate-confirmations/{}/statement",
        rate["id"].as_str().expect("id")
    );
    assert_eq!(
        call(&r, "carrier", "GET", &download, json!({}), "hidden")
            .await
            .0,
        404
    );
    publish(&s, &id, &rate, None).await;
    let (status, view) = call(&r, "carrier", "GET", "portal", json!({}), "read").await;
    assert_eq!(status, 200);
    let text = view.to_string();
    assert_eq!(view["offers"][0]["amount_cents"], 125001);
    assert_eq!(view["offers"].as_array().expect("offers").len(), 1);
    for private in [
        "150001",
        "sell_cents",
        "customer_id",
        "source_order",
        "PRIVATE-STAFF-REASON",
        "billing_profile",
        "receipts",
        "outbox",
        "partner_id",
        "payment_instructions",
        "tenders",
    ] {
        assert!(!text.contains(private), "{private}");
    }
    assert_eq!(
        call(&r, "other_carrier", "GET", "portal", json!({}), "other")
            .await
            .1["offers"],
        json!([])
    );
    assert_eq!(
        call(
            &r,
            "other_carrier",
            "GET",
            &download,
            json!({}),
            "other-doc"
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(&r, "customer", "GET", &download, json!({}), "wrong-kind")
            .await
            .0,
        404
    );
    for who in ["carrier", "customer"] {
        for (method, path) in [
            ("GET", "workspace"),
            ("GET", "finance/journal.csv"),
            ("GET", "sync/orders"),
            ("GET", "documents/missing/content"),
            ("POST", "loads"),
            ("POST", "events"),
            ("POST", "cash"),
            ("POST", "loads/missing/commands"),
            ("GET", "rate-confirmations/missing/statement"),
        ] {
            assert_eq!(
                call(&r, who, method, path, json!({}), "deny").await.0,
                403,
                "{who} {path}"
            );
        }
    }
    assert_eq!(
        call(&r, "staff", "GET", "portal", json!({}), "staff-not-partner")
            .await
            .0,
        403
    );
    let (status, html) = call(&r, "carrier", "GET", &download, json!({}), "doc").await;
    assert_eq!(status, 200);
    assert_eq!(html, rate["html"]);
}
#[tokio::test]
async fn published_offer_is_a_commitment_and_native_acceptance_binds_actor_rate_and_version_without_repeating_or_sending_edi(
) {
    let (s, r, id, rate) = setup().await;
    let offer = publish(&s, &id, &rate, None).await;
    let oid = offer["id"].as_str().expect("id");
    let path = format!("portal/offers/{oid}/response");
    let w = s.read("demo").await.expect("read");
    assert!(!w.can_withdraw_tender(&load(&s, &id).await));
    let before = json!(w);
    let good = response(&s, &id, &rate, "accept").await;
    let mut expired = s.read("demo").await.expect("read");
    expired
        .carriers
        .iter_mut()
        .find(|v| v.id == "ca4")
        .expect("carrier")
        .insurance_expires = Utc::now().date_naive() - Duration::days(1);
    let native = json!({"carrier_id":"ca4","subject":"partner:carrier","request":good});
    assert!(expired
        .apply_portal("portal.respond", oid, native, "partner:carrier")
        .expect_err("expired qualification")
        .contains("qualification changed"));

    for (field, value) in [
        ("carrier_id", json!("ca1")),
        ("subject", json!("forged")),
        ("amount_cents", json!(1)),
        ("status", json!("accepted")),
    ] {
        let mut forged = good.clone();
        forged[field] = value;
        assert_eq!(
            call(&r, "carrier", "POST", &path, forged, "forged").await.0,
            422
        );
    }
    let mut stale = good.clone();
    stale["expected_load_version"] = json!(1);
    assert_eq!(
        call(&r, "carrier", "POST", &path, stale, "stale").await.0,
        409
    );
    let mut changed = good.clone();
    changed["rate_sha256"] = json!("forged-digest");
    assert_eq!(
        call(&r, "carrier", "POST", &path, changed, "wrong-rate")
            .await
            .0,
        409
    );
    assert_eq!(
        call(
            &r,
            "other_carrier",
            "POST",
            &path,
            good.clone(),
            "wrong-owner"
        )
        .await
        .0,
        404
    );
    assert_eq!(json!(s.read("demo").await.expect("rollback")), before);
    let (status, result) = call(&r, "carrier", "POST", &path, good.clone(), "native-once").await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["data"]["recorded_by"], "partner:carrier");
    assert_eq!(
        call(&r, "carrier", "POST", &path, good, "native-once")
            .await
            .1["replayed"],
        true
    );
    let w = s.read("demo").await.expect("read");
    assert_eq!(load(&s, &id).await.status, domain::Status::Accepted);
    assert_eq!(w.portal_decisions.len(), 1);
    assert_eq!(w.portal_decisions[0].rate_sha256, rate["sha256"]);
    assert_eq!(json!(w.outbox), before["outbox"]);
    assert!(w.journal.is_empty());
    assert_eq!(
        call(
            &router(s, "other-tenant"),
            "carrier",
            "GET",
            "portal",
            json!({}),
            "isolated"
        )
        .await
        .0,
        404
    );
}
#[tokio::test]
async fn superseded_publications_and_unpublished_rate_changes_cannot_be_accepted_and_decline_retires_only_the_exact_tender(
) {
    let (s, r, id, rate) = setup().await;
    let first = publish(&s, &id, &rate, None).await;
    let next=run(&s,"rate.issue",&id,json!({"expected_load_version":load(&s,&id).await.version,"expected_previous_id":rate["id"],"terms":"Actual changed carrier terms","reason":"Unpublished revision stays private until publication"})).await;
    let path = format!(
        "portal/offers/{}/response",
        first["id"].as_str().expect("id")
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            response(&s, &id, &rate, "accept").await,
            "not-current"
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "GET",
            &format!(
                "portal/rate-confirmations/{}/statement",
                next["id"].as_str().expect("id")
            ),
            json!({}),
            "hidden-revision"
        )
        .await
        .0,
        404
    );
    let second = publish(&s, &id, &next, first["id"].as_str()).await;
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            response(&s, &id, &rate, "accept").await,
            "retired-pub"
        )
        .await
        .0,
        409
    );
    let path = format!(
        "portal/offers/{}/response",
        second["id"].as_str().expect("id")
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            response(&s, &id, &next, "decline").await,
            "decline-once"
        )
        .await
        .0,
        200
    );
    let l = load(&s, &id).await;
    assert_eq!(l.status, domain::Status::Planned);
    assert!(l.carrier_id.is_none());
    run(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":l.version,"carrier_id":"ca1"}),
    )
    .await;
    let view = call(&r, "carrier", "GET", "portal", json!({}), "history")
        .await
        .1;
    assert!(view["offers"]
        .as_array()
        .expect("offers")
        .iter()
        .all(|o| o["is_current"] == false
            && o["location"].is_null()
            && o["load_version"].is_null()));
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            response(&s, &id, &next, "accept").await,
            "retired-tender"
        )
        .await
        .0,
        409
    );
    let w = s.read("demo").await.expect("read");
    assert!(w
        .outbox
        .iter()
        .filter(|o| o.tender_id.as_deref() == rate["tender_id"].as_str())
        .all(|o| o.status == "superseded"));
}
#[tokio::test]
async fn customer_portal_exposes_only_own_shipments_and_issued_invoices_without_buy_costs_proof_or_draft_downloads(
) {
    let (s, r, _, _) = setup().await;
    let l = load(&s, "load_6").await;
    if !s
        .read("demo")
        .await
        .expect("read")
        .delivery_evidence_ready(&l)
    {
        run(&s,"pod.review",&l.id,json!({"expected_version":l.version,"decision":"accepted","note":"Finance verified the actual externally held receiver evidence"})).await;
    }
    let bill=run(&s,"bill.create","",json!({"load_id":"load_6","number":"CUSTOMER-PORTAL-INVOICE","due_date":Utc::now().date_naive()})).await;
    let bid = bill["id"].as_str().expect("id");
    let path = format!("portal/bills/{bid}/statement");
    assert_eq!(
        call(
            &r,
            "customer",
            "GET",
            "portal",
            json!({}),
            "own-draft-hidden"
        )
        .await
        .1["bills"],
        json!([])
    );
    assert_eq!(
        call(&r, "customer", "GET", &path, json!({}), "no-draft")
            .await
            .0,
        404
    );
    run(&s, "bill.issue", bid, json!({"expected_version":1})).await;
    let (status, view) = call(&r, "customer", "GET", "portal", json!({}), "customer").await;
    assert_eq!(status, 200);
    assert_eq!(view["bills"][0]["amount_cents"], 149000);
    let own = s
        .read("demo")
        .await
        .expect("read")
        .loads
        .into_iter()
        .filter(|l| l.customer_id == "c1")
        .count();
    assert_eq!(view["shipments"].as_array().expect("loads").len(), own);
    for private in [
        "buy_cents",
        "margin",
        "carrier_id",
        "source_order",
        "PRIVATE-STAFF",
        "journal",
        "credentials",
        "receipts",
    ] {
        assert!(!view.to_string().contains(private), "{private}");
    }
    let (status, html) = call(&r, "customer", "GET", &path, json!({}), "own-invoice").await;
    assert_eq!(status, 200);
    assert!(html.as_str().expect("html").contains("1490.00"));
    assert!(!html.as_str().expect("html").contains("1180.00"));
    assert_eq!(
        call(
            &r,
            "other_customer",
            "GET",
            &path,
            json!({}),
            "other-customer"
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(&r, "carrier", "GET", &path, json!({}), "carrier-no-bill")
            .await
            .0,
        404
    );
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            "portal/offers/missing/response",
            json!({}),
            "customer-no-write"
        )
        .await
        .0,
        403
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_portal_publications_and_native_responses_preserve_evidence_and_serialize_competing_decisions(
) {
    let (memory, _, id, rate) = setup().await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("portal-pg");
    let mut data = json!(memory.read("demo").await.expect("memory"));
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("tenant");
    let l = a
        .read(&tenant)
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|l| l.id == id)
        .expect("load");
    let body = json!({"expected_load_version":l.version,"rate_confirmation_id":rate["id"]});
    let before = json!(a.read(&tenant).await.expect("before"));
    canonical::set(
        &pool,
        &tenant,
        &["rate_confirmations", "0", "html"],
        json!("corrupted rate bytes"),
    )
    .await
    .expect("corrupt rate bytes");
    let corrupt = json!(a.read(&tenant).await.expect("corrupt"));
    assert!(a
        .mutate_authorized(
            (&tenant, "staff:a"),
            "publish-once",
            "portal.publish",
            &id,
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), corrupt);
    canonical::put_workspace(&pool, &tenant, &before)
        .await
        .expect("restore");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "staff:a"),
            "publish-once",
            "portal.publish",
            &id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "staff:b"),
            "publish-competing",
            "portal.publish",
            &id,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let w = a.read(&tenant).await.expect("read");
    let offer = &w.portal_offers[0];
    let oid = offer.id.clone();
    let l = w.loads.iter().find(|l| l.id == id).expect("load");
    assert!(!w.can_withdraw_tender(l));
    let input = json!({"expected_load_version":l.version,"decision":"accept","reference":"Actual native carrier response","note":"Carrier account lead reviewed exact published bytes","rate_confirmation_id":rate["id"],"rate_sha256":rate["sha256"]});
    let body = json!({"carrier_id":"ca4","subject":"partner:carrier","request":input});
    let before = json!(w);
    canonical::set(
        &pool,
        &tenant,
        &["rate_confirmations", "0", "html"],
        json!("corrupt before acceptance"),
    )
    .await
    .expect("corrupt");
    let corrupt = json!(a.read(&tenant).await.expect("corrupt"));
    assert!(a
        .mutate_authorized(
            (&tenant, "partner:carrier"),
            "native-once",
            "portal.respond",
            &oid,
            body.clone()
        )
        .await
        .is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), corrupt);
    canonical::put_workspace(&pool, &tenant, &before)
        .await
        .expect("restore");
    let mut decline = body.clone();
    decline["request"]["decision"] = json!("decline");
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "partner:carrier"),
            "native-once",
            "portal.respond",
            &oid,
            body
        ),
        b.mutate_authorized(
            (&tenant, "partner:carrier"),
            "native-competing",
            "portal.respond",
            &oid,
            decline
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    drop(a);
    drop(b);
    let reopened = Store::new(Some(&url)).await.expect("reconnect");
    let w = reopened.read(&tenant).await.expect("retained");
    assert_eq!(w.portal_offers.len(), 1);
    assert_eq!(w.portal_decisions.len(), 1);
    assert_eq!(w.portal_decisions[0].rate_sha256, rate["sha256"]);
    assert_eq!(w.portal_decisions[0].recorded_by, "partner:carrier");
    assert!(w.journal.is_empty());
    let l = w.loads.iter().find(|l| l.id == id).expect("load");
    assert_eq!(
        l.status,
        if w.portal_decisions[0].decision == "accept" {
            domain::Status::Accepted
        } else {
            domain::Status::Planned
        }
    );
    assert_eq!(
        stateset_freight::rate_confirmations::statement(&w, rate["id"].as_str().expect("id"))
            .expect("exact rate")
            .1,
        rate["html"]
    );
    let p = Principal {
        tenant_id: tenant.clone(),
        subject: "partner:carrier".into(),
        role: Role::Carrier,
        counterparty_id: Some("ca4".into()),
    };
    assert_eq!(
        w.partner_workspace(&p).expect("scoped view")["offers"]
            .as_array()
            .expect("offers")
            .len(),
        1
    );
    assert!(reopened
        .read("other-portal-tenant")
        .await
        .expect("isolated")
        .portal_offers
        .is_empty());
    // A previously accepted carrier can receive a later confirmation containing an
    // approved addition. Publishing must still inspect the actual agreement bytes.
    run(
        &memory,
        "load.command",
        &id,
        json!({"action":"accept","expected_version":load(&memory,&id).await.version}),
    )
    .await;
    let bytes = b"%PDF-1.7\nActual synthetic approved carrier addition evidence\n%%EOF";
    let up = memory
        .upload_document(
            ("demo", "staff:ops"),
            "portal-charge-file",
            &id,
            stateset_freight::documents::Upload {
                kind: "other".into(),
                filename: "carrier-addition.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: bytes.to_vec(),
            },
        )
        .await
        .expect("upload");
    let proof = up["data"]["id"].as_str().expect("proof");
    let charge=run(&memory,"charge.propose",&id,json!({"expected_load_version":load(&memory,&id).await.version,"side":"buy","kind":"other","description":"Actual additional carrier loading service","amount_cents":1234,"evidence_reference":"Actual carrier addition correspondence","document_id":proof})).await;
    run(&memory,"charge.review",charge["id"].as_str().expect("charge"),json!({"expected_load_version":load(&memory,&id).await.version,"expected_version":1,"decision":"approved","counterparty_contact":"Actual carrier accounting","confirmation_reference":"Actual approved addition confirmation","note":"Finance inspected actual external agreement"})).await;
    let updated=run(&memory,"rate.issue",&id,json!({"expected_load_version":load(&memory,&id).await.version,"expected_previous_id":rate["id"],"terms":"Actual revised carrier agreement including loading addition","reason":"Publish newly approved carrier service"})).await;
    let proof_tenant = domain::id("portal-proof-pg");
    let mut data = json!(memory.read("demo").await.expect("memory"));
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &proof_tenant, &data)
        .await
        .expect("proof tenant");
    sqlx::query(
        "INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)",
    )
    .bind(&proof_tenant)
    .bind(proof)
    .bind(b"corrupt agreement".as_slice())
    .execute(&pool)
    .await
    .expect("corrupt stored agreement");
    let request = json!({"expected_load_version":load(&memory,&id).await.version,"rate_confirmation_id":updated["id"]});
    let before = json!(reopened.read(&proof_tenant).await.expect("before"));
    assert!(reopened
        .mutate_authorized(
            (&proof_tenant, "staff:ops"),
            "publish-proof-once",
            "portal.publish",
            &id,
            request.clone()
        )
        .await
        .is_err());
    assert_eq!(
        json!(reopened.read(&proof_tenant).await.expect("rollback")),
        before
    );
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(bytes.as_slice())
    .bind(&proof_tenant)
    .bind(proof)
    .execute(&pool)
    .await
    .expect("restore agreement");
    reopened
        .mutate_authorized(
            (&proof_tenant, "staff:ops"),
            "publish-proof-once",
            "portal.publish",
            &id,
            request,
        )
        .await
        .expect("same failed key after evidence restored");
    assert_eq!(
        reopened
            .read(&proof_tenant)
            .await
            .expect("retained")
            .portal_offers
            .len(),
        1
    );
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&proof_tenant)
        .execute(&pool)
        .await
        .expect("clean proof bytes");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&proof_tenant)
        .execute(&pool)
        .await
        .expect("clean proof tenant");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
}

const CARRIER_POD: &[u8] = b"%PDF-1.7\nOriginal synthetic receiver evidence from carrier\n%%EOF";
async fn accept_offer(s: &Store, r: &Router, id: &str, rate: &Value) -> String {
    let offer = publish(s, id, rate, None).await;
    let oid = offer["id"].as_str().expect("offer").to_owned();
    let result = call(
        r,
        "carrier",
        "POST",
        &format!("portal/offers/{oid}/response"),
        response(s, id, rate, "accept").await,
        "native-accept",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    oid
}
async fn observation(s: &Store, id: &str, kind: &str, at: chrono::DateTime<Utc>) -> Value {
    json!({"expected_load_version":load(s,id).await.version,"kind":kind,"observed_at":at,"location":"Seattle, WA","reference":format!("Actual-{kind}-evidence"),"note":"Carrier dispatch inspected actual physical receiver/driver evidence"})
}
async fn carrier_move(
    s: &Store,
    r: &Router,
    id: &str,
    oid: &str,
    kind: &str,
    at: chrono::DateTime<Utc>,
) -> Value {
    let result = call(
        r,
        "carrier",
        "POST",
        &format!("portal/offers/{oid}/tracking"),
        observation(s, id, kind, at).await,
        &domain::id("move"),
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    result.1["data"].clone()
}
async fn pod_call(
    r: &Router,
    who: &str,
    oid: &str,
    query: Value,
    bytes: &[u8],
    key: &str,
) -> (u16, Value) {
    let mut url =
        reqwest::Url::parse(&format!("http://owned/v1/portal/offers/{oid}/pod")).expect("url");
    for (k, v) in query.as_object().expect("query") {
        url.query_pairs_mut().append_pair(
            k,
            &v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string()),
        );
    }
    let response = r
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{}?{}", url.path(), url.query().expect("query")))
                .header("authorization", format!("Bearer {who}"))
                .header("idempotency-key", key)
                .header("content-type", "application/pdf")
                .body(Body::from(bytes.to_vec()))
                .expect("upload"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}
async fn pod_query(s: &Store, id: &str) -> Value {
    json!({"expected_load_version":load(s,id).await.version,"filename":"receiver-proof.pdf","reference":"Actual carrier receiver proof","note":"Carrier lead supplied actual original receiver evidence"})
}
#[tokio::test]
async fn native_carrier_tracking_preserves_actor_and_stale_evidence_and_blocks_release_without_granting_staff_access(
) {
    let (s, r, id, rate) = setup().await;
    let oid = accept_offer(&s, &r, &id, &rate).await;
    let path = format!("portal/offers/{oid}/tracking");
    let at = Utc::now() - Duration::minutes(10);
    let good = observation(&s, &id, "pickup", at).await;
    let before = json!(s.read("demo").await.expect("before"));
    for who in ["customer", "staff", "other_carrier"] {
        assert_eq!(
            call(&r, who, "POST", &path, good.clone(), "deny").await.0,
            if who == "other_carrier" { 404 } else { 403 }
        );
    }
    let mut forged = good.clone();
    forged["carrier_id"] = json!("ca1");
    assert_eq!(
        call(&r, "carrier", "POST", &path, forged, "forged").await.0,
        422
    );
    let mut future = good.clone();
    future["observed_at"] = json!(Utc::now() + Duration::hours(1));
    assert_eq!(
        call(&r, "carrier", "POST", &path, future, "future").await.0,
        422
    );
    let mut premature = good.clone();
    premature["kind"] = json!("delivery");
    assert_eq!(
        call(&r, "carrier", "POST", &path, premature, "premature")
            .await
            .0,
        422
    );
    assert_eq!(json!(s.read("demo").await.expect("rollback")), before);
    let release=run(&s,"tender.release.request",&id,json!({"expected_load_version":load(&s,&id).await.version,"disposition":"replan","reason":"Actual coordination before carrier pickup report"})).await;
    let input = observation(&s, &id, "pickup", at).await;
    let result = call(&r, "carrier", "POST", &path, input.clone(), "pickup-once").await;
    assert_eq!(result.0, 200, "{result:?}");
    assert_eq!(result.1["data"]["recorded_by"], "partner:carrier");
    assert!(!result.1.to_string().contains("sell_cents"));
    let after = json!(s.read("demo").await.expect("after"));
    assert_eq!(
        call(&r, "carrier", "POST", &path, input.clone(), "pickup-once")
            .await
            .1["replayed"],
        true
    );
    assert_eq!(
        call(&r, "second_carrier", "POST", &path, input, "pickup-once")
            .await
            .0,
        409
    );
    assert_eq!(json!(s.read("demo").await.expect("unchanged")), after);
    let w = s.read("demo").await.expect("read");
    assert_eq!(load(&s, &id).await.status, domain::Status::InTransit);
    assert_ne!(
        w.releases
            .iter()
            .find(|v| v.id == release["id"])
            .expect("release")
            .status,
        "pending"
    );
    let later = at + Duration::minutes(3);
    carrier_move(&s, &r, &id, &oid, "location", later).await;
    let version = load(&s, &id).await.version;
    let stale = carrier_move(&s, &r, &id, &oid, "location", at + Duration::minutes(1)).await;
    assert_eq!(stale["outcome"], "stale_ignored");
    assert_eq!(load(&s, &id).await.version, version);
    assert_eq!(load(&s, &id).await.last_tracking_at, Some(later));
    carrier_move(&s, &r, &id, &oid, "delivery", at + Duration::minutes(4)).await;
    let l = load(&s, &id).await;
    assert_eq!(l.status, domain::Status::Delivered);
    assert!(l.pod_reference.is_none());
    let w = s.read("demo").await.expect("read");
    assert!(w.journal.is_empty());
    assert_eq!(w.carrier_submissions.len(), 4);
    assert!(w
        .events
        .iter()
        .filter(|e| e.kind == "portal.tracking")
        .all(|e| e.actor == "partner:carrier"));
}
#[tokio::test]
async fn carrier_pod_commits_original_bytes_attachment_and_receipt_together_and_keeps_finance_review_private(
) {
    let (s, r, id, rate) = setup().await;
    let oid = accept_offer(&s, &r, &id, &rate).await;
    let query = pod_query(&s, &id).await;
    assert_eq!(
        pod_call(&r, "carrier", &oid, query, CARRIER_POD, "not-delivered")
            .await
            .0,
        422
    );
    let at = Utc::now() - Duration::minutes(5);
    carrier_move(&s, &r, &id, &oid, "pickup", at).await;
    carrier_move(&s, &r, &id, &oid, "delivery", at + Duration::minutes(1)).await;
    let query = pod_query(&s, &id).await;
    let before = json!(s.read("demo").await.expect("before"));
    assert_eq!(
        pod_call(
            &r,
            "other_carrier",
            &oid,
            query.clone(),
            CARRIER_POD,
            "wrong-party"
        )
        .await
        .0,
        404
    );
    assert_eq!(
        pod_call(&r, "customer", &oid, query.clone(), CARRIER_POD, "customer")
            .await
            .0,
        403
    );
    let mut stale = query.clone();
    stale["expected_load_version"] = json!(1);
    assert_eq!(
        pod_call(&r, "carrier", &oid, stale, CARRIER_POD, "stale")
            .await
            .0,
        409
    );
    assert_eq!(
        pod_call(&r, "carrier", &oid, query.clone(), b"fake PDF", "bad-file")
            .await
            .0,
        422
    );
    assert_eq!(json!(s.read("demo").await.expect("rollback")), before);
    let result = pod_call(&r, "carrier", &oid, query.clone(), CARRIER_POD, "pod-once").await;
    assert_eq!(result.0, 200, "{result:?}");
    let report = &result.1["data"];
    let docid = report["document_id"].as_str().expect("doc");
    let sid = report["id"].as_str().expect("submission");
    assert_eq!(load(&s, &id).await.pod_document_id.as_deref(), Some(docid));
    assert_eq!(
        s.document_content("demo", docid).await.expect("bytes").1,
        CARRIER_POD
    );
    assert!(!s
        .read("demo")
        .await
        .expect("read")
        .delivery_evidence_ready(&load(&s, &id).await));
    let after = json!(s.read("demo").await.expect("after"));
    assert_eq!(
        pod_call(&r, "carrier", &oid, query.clone(), CARRIER_POD, "pod-once")
            .await
            .1["replayed"],
        true
    );
    let changed = b"%PDF-1.7\nChanged receiver proof\n%%EOF";
    assert_eq!(
        pod_call(&r, "carrier", &oid, query, changed, "pod-once")
            .await
            .0,
        409
    );
    assert_eq!(
        json!(s.read("demo").await.expect("replay unchanged")),
        after
    );
    let download = format!("portal/submissions/{sid}/content");
    let (status, bytes) = call(&r, "carrier", "GET", &download, json!({}), "download").await;
    assert_eq!(status, 200);
    assert_eq!(bytes, String::from_utf8_lossy(CARRIER_POD).to_string());
    for who in ["other_carrier", "customer"] {
        assert_eq!(
            call(&r, who, "GET", &download, json!({}), "private")
                .await
                .0,
            404
        );
    }
    assert_eq!(
        pod_call(
            &r,
            "carrier",
            &oid,
            pod_query(&s, &id).await,
            changed,
            "replace-pending"
        )
        .await
        .0,
        422
    );
    let invoice=run(&s,"invoice.create","",json!({"load_id":id,"number":"CARRIER-NATIVE-INVOICE","amount_cents":125001,"currency":"USD"})).await;
    assert!(s
        .mutate(
            "demo",
            "early-approve",
            "invoice.approve",
            invoice["id"].as_str().expect("invoice"),
            json!({})
        )
        .await
        .is_err());
    run(&s,"document.review",docid,json!({"expected_version":1,"decision":"rejected","note":"PRIVATE-FINANCE-NOTE incorrect receiver signature"})).await;
    let result = pod_call(
        &r,
        "carrier",
        &oid,
        pod_query(&s, &id).await,
        changed,
        "replacement",
    )
    .await;
    assert_eq!(result.0, 200, "{result:?}");
    let newdoc = result.1["data"]["document_id"]
        .as_str()
        .expect("replacement doc");
    assert_ne!(newdoc, docid);
    run(&s,"document.review",newdoc,json!({"expected_version":1,"decision":"accepted","note":"PRIVATE-FINANCE-ACCEPT staff inspected original file"})).await;
    let view = call(&r, "carrier", "GET", "portal", json!({}), "view")
        .await
        .1;
    assert!(!view.to_string().contains("PRIVATE-FINANCE"));
    assert_eq!(view["offers"][0]["reporting"]["pod_status"], "accepted");
    assert_eq!(view["offers"][0]["reporting"]["can_submit_pod"], false);
    assert_eq!(
        pod_call(
            &r,
            "carrier",
            &oid,
            pod_query(&s, &id).await,
            CARRIER_POD,
            "replace-accepted"
        )
        .await
        .0,
        422
    );
    run(
        &s,
        "invoice.approve",
        invoice["id"].as_str().expect("invoice"),
        json!({}),
    )
    .await;
    assert_eq!(
        s.document_content("demo", docid)
            .await
            .expect("original retained")
            .1,
        CARRIER_POD
    );
}
#[tokio::test]
async fn carrier_reporting_requires_latest_publication_and_cannot_move_an_outstanding_or_retired_tender(
) {
    let (s, r, id, rate) = setup().await;
    let offer = publish(&s, &id, &rate, None).await;
    let oid = offer["id"].as_str().expect("offer");
    let at = Utc::now() - Duration::minutes(1);
    let path = format!("portal/offers/{oid}/tracking");
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            observation(&s, &id, "pickup", at).await,
            "unaccepted"
        )
        .await
        .0,
        422
    );
    let next=run(&s,"rate.issue",&id,json!({"expected_load_version":load(&s,&id).await.version,"expected_previous_id":rate["id"],"terms":"Actual revised carrier terms","reason":"Actual change confirmed with carrier"})).await;
    let publication = publish(&s, &id, &next, Some(oid)).await;
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            observation(&s, &id, "pickup", at).await,
            "superseded"
        )
        .await
        .0,
        409
    );
    let newoid = publication["id"].as_str().expect("new offer");
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &format!("portal/offers/{newoid}/response"),
            response(&s, &id, &next, "decline").await,
            "declined"
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &format!("portal/offers/{newoid}/tracking"),
            observation(&s, &id, "pickup", at).await,
            "retired"
        )
        .await
        .0,
        409
    );
    assert!(s
        .read("demo")
        .await
        .expect("read")
        .carrier_submissions
        .is_empty());
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_carrier_evidence_serializes_competing_reports_and_rolls_back_file_attachment_receipt_on_commit_failure(
) {
    use stateset_freight::carrier_portal::PodInput;
    let (memory, r, id, rate) = setup().await;
    let oid = accept_offer(&memory, &r, &id, &rate).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("carrier-pg");
    let mut data = json!(memory.read("demo").await.expect("fixture"));
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("tenant");
    let at = Utc::now() - Duration::minutes(3);
    let input = observation(&memory, &id, "pickup", at).await;
    let body = json!({"carrier_id":"ca4","subject":"partner:carrier","request":input});
    let (left, right) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "partner:carrier"),
            "pickup-a",
            "portal.tracking",
            &oid,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "partner:carrier"),
            "pickup-b",
            "portal.tracking",
            &oid,
            body
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let w = a.read(&tenant).await.expect("read");
    let l = w.loads.iter().find(|l| l.id == id).expect("load");
    let input = json!({"expected_load_version":l.version,"kind":"delivery","observed_at":at+Duration::minutes(1),"location":"Seattle, WA","reference":"Actual native delivery","note":"Actual receiver evidence supplied by carrier"});
    a.mutate_authorized(
        (&tenant, "partner:carrier"),
        "delivery",
        "portal.tracking",
        &oid,
        json!({"carrier_id":"ca4","subject":"partner:carrier","request":input}),
    )
    .await
    .expect("delivery");
    let before = json!(a.read(&tenant).await.expect("before"));
    let l = a
        .read(&tenant)
        .await
        .expect("read")
        .loads
        .into_iter()
        .find(|l| l.id == id)
        .expect("load");
    let query = json!({"expected_load_version":l.version,"filename":"actual-carrier-pod.pdf","reference":"Actual carrier POD confirmation","note":"Actual carrier supplied receiver file"});
    let p = Principal {
        tenant_id: tenant.clone(),
        subject: "partner:carrier".into(),
        role: Role::Carrier,
        counterparty_id: Some("ca4".into()),
    };
    let trigger = format!("carrier_atomic_{}", uuid::Uuid::new_v4().simple());
    sqlx::raw_sql(&format!("CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned injected aggregate commit failure'; END $$; CREATE TRIGGER {trigger} BEFORE UPDATE ON freight_workspaces FOR EACH ROW WHEN (NEW.tenant_id='{tenant}') EXECUTE FUNCTION {trigger}();")).execute(&pool).await.expect("owned failure injection");
    let attempt = a
        .submit_carrier_pod(
            &p,
            "pod-a",
            &oid,
            serde_json::from_value::<PodInput>(query.clone()).expect("query"),
            CARRIER_POD.to_vec(),
            "application/pdf".into(),
        )
        .await;
    assert!(attempt.is_err());
    assert_eq!(json!(a.read(&tenant).await.expect("rollback")), before);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM freight_document_content WHERE tenant_id=$1")
            .bind(&tenant)
            .fetch_one(&pool)
            .await
            .expect("blob count");
    assert_eq!(
        count, 0,
        "file insertion rolls back when aggregate/receipt commit fails"
    );
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {trigger} ON freight_workspaces; DROP FUNCTION {trigger}();"
    ))
    .execute(&pool)
    .await
    .expect("remove owned injection");
    let (left, right) = tokio::join!(
        a.submit_carrier_pod(
            &p,
            "pod-a",
            &oid,
            serde_json::from_value(query.clone()).expect("query"),
            CARRIER_POD.to_vec(),
            "application/pdf".into()
        ),
        b.submit_carrier_pod(
            &p,
            "pod-b",
            &oid,
            serde_json::from_value(query).expect("query"),
            CARRIER_POD.to_vec(),
            "application/pdf".into()
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    drop((a, b));
    let reopened = Store::new(Some(&url)).await.expect("reconnect");
    let w = reopened.read(&tenant).await.expect("retained");
    assert_eq!(w.carrier_submissions.len(), 3);
    let doc = w
        .carrier_submissions
        .iter()
        .find_map(|s| s.document_id.as_deref())
        .expect("document");
    let l = w.loads.iter().find(|l| l.id == id).expect("load");
    assert_eq!(l.pod_document_id.as_deref(), Some(doc));
    assert!(!w.delivery_evidence_ready(l));
    assert!(w.journal.is_empty());
    assert_eq!(
        reopened
            .document_content(&tenant, doc)
            .await
            .expect("exact bytes")
            .1,
        CARRIER_POD
    );
    let r = router(reopened.clone(), &tenant);
    sqlx::query(
        "UPDATE freight_document_content SET content=$1 WHERE tenant_id=$2 AND document_id=$3",
    )
    .bind(b"corrupt stored native evidence".as_slice())
    .bind(&tenant)
    .bind(doc)
    .execute(&pool)
    .await
    .expect("owned corruption");
    let sid = &w
        .carrier_submissions
        .iter()
        .find(|s| s.document_id.as_deref() == Some(doc))
        .expect("submission")
        .id;
    assert_eq!(
        call(
            &r,
            "carrier",
            "GET",
            &format!("portal/submissions/{sid}/content"),
            json!({}),
            "corrupt"
        )
        .await
        .0,
        500
    );
    assert!(reopened
        .read("other-carrier-pg")
        .await
        .expect("isolated")
        .carrier_submissions
        .is_empty());
    sqlx::query("DELETE FROM freight_document_content WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup owned file");
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("cleanup");
    pool.close().await;
}
