#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Workspace},
    integrations::Integrations,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
fn agreement(amount: i64) -> Value {
    json!({"number":"REVIEWED-PDX-SEA","carrier_id":"ca4","rate":{"customer_id":"c1","origin":{"country":"us","state":"or","postal_code":"97201"},"destination":{"country":"US","state":"WA","postal_code":"98101"},"equipment":"Dry van","max_weight_lbs":45000,"max_miles":200,"effective_from":Utc::now().date_naive(),"effective_through":(Utc::now()+Duration::days(30)).date_naive(),"amount_cents":amount,"terms":"Inclusive fuel and linehaul <script>private contract terms</script>; no additional charges without separate agreement"},"contact":"Carrier contracting lead","reference":"PRIVATE-CONTRACT-ORIGINAL","note":"PRIVATE actual external agreement inspected","reason":"Original reviewed lane agreement"})
}
fn draft(reference: &str) -> Value {
    let day = (Utc::now() + Duration::days(2)).date_naive();
    json!({"reference":reference,"plan":{"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Consumer goods","weight_lbs":1000,"miles":173,"origin":{"facility":"Portland DC","city":"Portland","state":"OR","date":day,"address_lines":["15 Dock Street"],"postal_code":"97201","country":"US"},"destination":{"facility":"Seattle DC","city":"Seattle","state":"WA","date":day,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"}},"sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Customer inclusive quote terms"})
}
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "ops:contracts"),
        &domain::id("key"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
fn source(c: &Value, q: &Value) -> Value {
    json!({"expected_version":q["version"],"agreement_id":c["id"],"expected_rate_version":c["version"],"revision_sha256":c["revisions"].as_array().expect("revisions").last().expect("revision")["basis_sha256"],"valid_until":Utc::now()+Duration::hours(24),"contact":"Actual carrier capacity lead","reference":"FRESH-CAPACITY-REFERENCE","note":"Driver capacity and the exact inclusive contract price confirmed for this plan"})
}
fn revise(c: &Value, amount: i64) -> Value {
    let mut b = agreement(amount);
    b["expected_version"] = c["version"].clone();
    b["reference"] = json!("REVISED-CONTRACT-SOURCE");
    b["reason"] = json!("Actual new carrier commercial agreement");
    b
}
async fn book(s: &Store, q: &Value) -> Value {
    let id = q["id"].as_str().expect("id");
    let offer = q["revisions"][0]["offers"][0]["id"].clone();
    let q = op(
        s,
        "quote.issue",
        id,
        json!({"expected_version":q["version"],"offer_id":offer}),
    )
    .await;
    let q=op(s,"quote.decision",id,json!({"expected_version":q["version"],"decision":"accept","contact":"Customer lead","reference":"Actual customer acceptance","note":"Exact issued terms accepted"})).await;
    op(
        s,
        "quote.book",
        id,
        json!({"expected_version":q["version"]}),
    )
    .await
}
fn router(s: Store) -> Router {
    let keys = [
        ("ops", Role::Operations, None),
        ("viewer", Role::Viewer, None),
        ("finance", Role::Finance, None),
        ("carrier", Role::Carrier, Some("ca4")),
        ("customer", Role::Customer, Some("c1")),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.to_string(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("test:{key}"),
                role,
                counterparty_id: cp.map(str::to_owned),
            }),
        )
    })
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        store: s,
        identity: Arc::new(None),
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
) -> (u16, Vec<u8>, axum::http::HeaderMap) {
    let response = r
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {who}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, bytes, headers)
}
#[tokio::test]
async fn exact_contract_price_and_fresh_capacity_book_without_mutating_old_terms_after_revision_or_withdrawal(
) {
    let s = setup().await;
    let before = read(&s).await;
    let c = op(&s, "contract.create", "", agreement(125001)).await;
    let cid = c["id"].as_str().expect("id");
    let q = op(&s, "quote.create", "", draft("CONTRACT-EXACT")).await;
    let qid = q["id"].as_str().expect("id");
    let candidates = read(&s).await.contract_candidates(qid).expect("candidates");
    assert_eq!(candidates["rates"][0]["eligible"], true);
    assert_eq!(read(&s).await.loads.len(), before.loads.len());
    assert_eq!(read(&s).await.outbox.len(), before.outbox.len());
    let body = source(&c, &q);
    let result = s
        .mutate_authorized(
            ("demo", "ops:contracts"),
            "capacity-once",
            "quote.contract-offer",
            qid,
            body.clone(),
        )
        .await
        .expect("sourced");
    assert_eq!(
        s.mutate_authorized(
            ("demo", "ops:contracts"),
            "capacity-once",
            "quote.contract-offer",
            qid,
            body
        )
        .await
        .expect("replay")["replayed"],
        true
    );
    let sourced = result["data"].clone();
    let o = &sourced["revisions"][0]["offers"][0];
    assert_eq!(o["amount_cents"], 125001);
    assert_eq!(o["evidence"]["reference"], "FRESH-CAPACITY-REFERENCE");
    assert_eq!(o["rate_source"]["revision"], 1);
    assert_eq!(o["rate_source"]["agreement_id"], cid);
    let original = c["revisions"][0].clone();
    let c = op(&s, "contract.revise", cid, revise(&c, 130001)).await;
    assert_eq!(c["revisions"][0], original);
    let withdrawn=op(&s,"contract.withdraw",cid,json!({"expected_version":c["version"],"contact":"Carrier contracting lead","reference":"Actual withdrawal","note":"No further use; confirmed shipments keep their rates"})).await;
    let booked = book(&s, &sourced).await;
    let lid = booked["booked_load_id"].as_str().expect("load");
    let w = read(&s).await;
    let l = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!((l.buy_cents, l.sell_cents), (125001, 150001));
    assert!(w.journal.is_empty());
    let tender = op(
        &s,
        "load.command",
        lid,
        json!({"action":"tender","expected_version":l.version,"carrier_id":"ca4"}),
    )
    .await;
    assert_eq!(tender["buy_cents"], 125001);
    assert_eq!(
        read(&s)
            .await
            .quotes
            .iter()
            .find(|q| q.id == qid)
            .expect("quote")
            .current()
            .expect("rev")
            .offers
            .len(),
        1
    );
    let q2 = op(&s, "quote.create", "", draft("CONTRACT-WITHDRAWN")).await;
    assert!(s
        .mutate(
            "demo",
            "no-new-use",
            "quote.contract-offer",
            q2["id"].as_str().expect("id"),
            source(&withdrawn, &q2)
        )
        .await
        .is_err());
}
#[tokio::test]
async fn applicability_uses_exact_directed_postal_scope_dates_limits_and_live_qualification() {
    let s = setup().await;
    op(&s, "contract.create", "", agreement(125001)).await;
    let q = op(&s, "quote.create", "", draft("CONTRACT-MATCH")).await;
    let id = q["id"].as_str().expect("id");
    let baseline = read(&s).await;
    let cases = [
        ("customer", json!("c2")),
        ("mode", json!("LTL")),
        ("equipment", json!("Reefer")),
        ("postal", json!("972019999")),
        ("weight", json!(45001)),
        ("miles", json!(201)),
        (
            "period",
            json!((Utc::now() + Duration::days(31)).date_naive()),
        ),
    ];
    for (kind, value) in cases {
        let mut w = baseline.clone();
        let p = &mut w
            .quotes
            .iter_mut()
            .find(|q| q.id == id)
            .expect("quote")
            .revisions[0]
            .plan;
        match kind {
            "customer" => p.customer_id = value.as_str().expect("customer").into(),
            "mode" => p.mode = value.as_str().expect("mode").into(),
            "equipment" => p.equipment = value.as_str().expect("equipment").into(),
            "postal" => p.origin.details.postal_code = value.as_str().expect("zip").into(),
            "weight" => p.weight_lbs = 45001,
            "miles" => p.miles = 201,
            _ => p.destination.date = serde_json::from_value(value).expect("date"),
        };
        let v = w.contract_candidates(id).expect("evaluate");
        assert_eq!(v["rates"][0]["eligible"], false, "{kind}");
        assert!(!v["rates"][0]["reasons"]
            .as_array()
            .expect("reasons")
            .is_empty());
    }
    let mut w = baseline.clone();
    let p = &mut w.quotes[0].revisions[0].plan;
    std::mem::swap(&mut p.origin, &mut p.destination);
    assert_eq!(
        w.contract_candidates(id).expect("reverse")["rates"][0]["eligible"],
        false
    );
    let mut w = baseline.clone();
    w.carriers
        .iter_mut()
        .find(|c| c.id == "ca4")
        .expect("carrier")
        .insurance_expires = Utc::now().date_naive();
    assert_eq!(
        w.contract_candidates(id).expect("insurance")["rates"][0]["eligible"],
        false
    );
    let mut invalid = agreement(0);
    assert!(s
        .mutate("demo", "zero", "contract.create", "", invalid.clone())
        .await
        .is_err());
    invalid = agreement(125001);
    invalid["rate"]["origin"]["postal_code"] = json!("*");
    assert!(s
        .mutate("demo", "wildcard", "contract.create", "", invalid)
        .await
        .is_err());
    let mut canadian = agreement(125001);
    canadian["number"] = json!("CA-NORMALIZED");
    canadian["rate"]["origin"] = json!({"country":"ca","state":"bc","postal_code":"v6b 1a1"});
    let c = op(&s, "contract.create", "", canadian).await;
    assert_eq!(c["revisions"][0]["rate"]["origin"]["postal_code"], "V6B1A1");
}
#[tokio::test]
async fn sourcing_rejects_forged_price_identity_stale_versions_and_missing_capacity_evidence() {
    let s = setup().await;
    let c = op(&s, "contract.create", "", agreement(125001)).await;
    let cid = c["id"].as_str().expect("id");
    let q = op(&s, "quote.create", "", draft("CONTRACT-GUARDS")).await;
    let qid = q["id"].as_str().expect("id");
    for (field, value) in [
        ("amount_cents", json!(1)),
        ("carrier_id", json!("ca1")),
        ("recorded_by", json!("forged")),
        ("contact", json!("")),
        ("revision_sha256", json!("bad")),
    ] {
        let mut body = source(&c, &q);
        body[field] = value;
        assert!(
            s.mutate(
                "demo",
                &domain::id("key"),
                "quote.contract-offer",
                qid,
                body
            )
            .await
            .is_err(),
            "{field}"
        );
    }
    let c2 = op(&s, "contract.revise", cid, revise(&c, 130001)).await;
    assert!(s
        .mutate(
            "demo",
            "stale-rate",
            "quote.contract-offer",
            qid,
            source(&c, &q)
        )
        .await
        .expect_err("stale")
        .contains("changed;"));
    let mut w = read(&s).await;
    w.carriers
        .iter_mut()
        .find(|c| c.id == "ca4")
        .expect("carrier")
        .scac = "NEWC".into();
    assert!(w
        .apply_quote("quote.contract-offer", qid, source(&c2, &q), "ops")
        .expect_err("identity")
        .contains("identity changed"));
    let w = read(&s).await;
    assert_eq!(w.quotes[0].version, 1);
    assert!(w.quotes[0].revisions[0].offers.is_empty());
    assert!(w.journal.is_empty());
}
#[tokio::test]
async fn original_contract_hash_and_source_price_fail_closed_before_quote_or_tender_commitment() {
    let s = setup().await;
    let c = op(&s, "contract.create", "", agreement(125001)).await;
    let q = op(&s, "quote.create", "", draft("CONTRACT-INTEGRITY")).await;
    let qid = q["id"].as_str().expect("id");
    let q = op(&s, "quote.contract-offer", qid, source(&c, &q)).await;
    let w = read(&s).await;
    for kind in ["html", "terms", "source", "price"] {
        let mut bad = w.clone();
        match kind {
            "html" => bad.carrier_contracts[0].revisions[0]
                .html
                .push_str("changed"),
            "terms" => bad.carrier_contracts[0].revisions[0].rate.amount_cents = 1,
            "source" => {
                bad.quotes[0].revisions[0].offers[0]
                    .rate_source
                    .as_mut()
                    .expect("source")
                    .basis_sha256 = "bad".into()
            }
            _ => bad.quotes[0].revisions[0].offers[0].amount_cents = 1,
        };
        assert_eq!(bad.apply_quote("quote.issue",qid,json!({"expected_version":q["version"],"offer_id":q["revisions"][0]["offers"][0]["id"]}),"ops").expect_err("integrity"),"Internal storage error", "{kind}");
    }
    let booked = book(&s, &q).await;
    let mut w = read(&s).await;
    w.carrier_contracts[0].revisions[0].evidence.note = "changed original proof".into();
    let lid = booked["booked_load_id"].as_str().expect("load");
    let l = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert!(w
        .clone()
        .apply(
            "load.command",
            lid,
            json!({"action":"tender","expected_version":l.version,"carrier_id":"ca4"})
        )
        .is_err());
}
#[tokio::test]
async fn contract_replacement_preserves_customer_agreement_and_requires_safe_assignment_and_current_load_version(
) {
    let s = setup().await;
    let c = op(&s, "contract.create", "", agreement(125001)).await;
    let q = op(&s, "quote.create", "", draft("CONTRACT-REPLACE")).await;
    let qid = q["id"].as_str().expect("id");
    let q = op(&s, "quote.contract-offer", qid, source(&c, &q)).await;
    let booked = book(&s, &q).await;
    let lid = booked["booked_load_id"].as_str().expect("load");
    let l = read(&s)
        .await
        .loads
        .iter()
        .find(|l| l.id == lid)
        .expect("load")
        .clone();
    let tender = op(
        &s,
        "load.command",
        lid,
        json!({"action":"tender","expected_version":l.version,"carrier_id":"ca4"}),
    )
    .await;
    let c = op(
        &s,
        "contract.revise",
        c["id"].as_str().expect("id"),
        revise(&c, 130001),
    )
    .await;
    let mut b = source(&c, &booked);
    b["expected_load_version"] = tender["version"].clone();
    b["reason"] = json!("Original capacity withdrawn; current replacement confirmed");
    b["reference"] = json!("FRESH-REPLACEMENT-CAPACITY");
    assert!(s
        .mutate(
            "demo",
            "unsafe-replacement",
            "quote.contract-reprocure",
            qid,
            b.clone()
        )
        .await
        .is_err());
    let l = op(
        &s,
        "load.command",
        lid,
        json!({"action":"withdraw_tender","expected_version":tender["version"]}),
    )
    .await;
    let w = read(&s).await;
    let current = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!(current.status, domain::Status::Planned);
    b["expected_load_version"] = json!(current.version);
    let q = op(&s, "quote.contract-reprocure", qid, b).await;
    let w = read(&s).await;
    let load = w.loads.iter().find(|l| l.id == lid).expect("load");
    assert_eq!((load.buy_cents, load.sell_cents), (130001, 150001));
    assert_eq!(q["procurements"].as_array().expect("awards").len(), 2);
    assert_eq!(q["procurements"][0]["offer"]["amount_cents"], 125001);
    assert_eq!(q["procurements"][1]["offer"]["rate_source"]["revision"], 2);
    assert!(w.journal.is_empty());
    let _ = l;
}
#[tokio::test]
async fn contract_http_documents_preserve_exact_bytes_and_staff_only_scope_without_portal_leakage()
{
    let s = setup().await;
    let r = router(s.clone());
    let c = op(&s, "contract.create", "", agreement(125001)).await;
    let cid = c["id"].as_str().expect("id");
    let path = format!("carrier-contracts/{cid}/statement?revision=1");
    let (status, bytes, headers) = call(&r, "viewer", "GET", &path, json!({}), "read").await;
    assert_eq!(status, 200);
    assert_eq!(
        headers["x-content-sha256"],
        stateset_freight::documents::sha256(&bytes)
    );
    let html = String::from_utf8(bytes.clone()).expect("html");
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert_eq!(headers["cache-control"], "private, no-store");
    for who in ["carrier", "customer"] {
        assert_eq!(
            call(&r, who, "GET", &path, json!({}), "denied").await.0,
            403
        );
        let (_, bytes, _) = call(&r, who, "GET", "portal", json!({}), "portal").await;
        let body = String::from_utf8(bytes).expect("body");
        assert!(!body.contains("PRIVATE-CONTRACT"));
        assert!(!body.contains(cid));
    }
    assert_eq!(
        call(
            &r,
            "finance",
            "POST",
            "carrier-contracts",
            agreement(125001),
            "denied-finance"
        )
        .await
        .0,
        403
    );
    let c2 = op(&s, "contract.revise", cid, revise(&c, 130001)).await;
    assert_eq!(
        call(&r, "viewer", "GET", &path, json!({}), "retained")
            .await
            .1,
        bytes
    );
    assert_ne!(c2["revisions"][1]["sha256"], c["revisions"][0]["sha256"]);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_contract_races_integrity_rollback_reconnect_and_tenant_isolation_preserve_original_selection(
) {
    let memory = setup().await;
    let c = op(&memory, "contract.create", "", agreement(125001)).await;
    let cid = c["id"].as_str().expect("contract");
    let q = op(&memory, "quote.create", "", draft("CONTRACT-PG")).await;
    let qid = q["id"].as_str().expect("quote");
    let url = std::env::var("TEST_DATABASE_URL").expect("owned DB");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    for mode in ["duplicate", "revision", "withdraw", "corruption"] {
        let tenant = domain::id("contract-pg");
        let mut data = json!(read(&memory).await);
        data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
        canonical::put_workspace(&pool, &tenant, &data)
            .await
            .expect("owned fixture");
        if mode == "duplicate" {
            let (x, y) = tokio::join!(
                a.mutate_authorized(
                    (&tenant, "ops:a"),
                    "use-a",
                    "quote.contract-offer",
                    qid,
                    source(&c, &q)
                ),
                b.mutate_authorized(
                    (&tenant, "ops:b"),
                    "use-b",
                    "quote.contract-offer",
                    qid,
                    source(&c, &q)
                )
            );
            assert_eq!(usize::from(x.is_ok()) + usize::from(y.is_ok()), 1);
            let w = a.read(&tenant).await.expect("read");
            assert_eq!(w.quotes[0].revisions[0].offers.len(), 1);
            assert_eq!(w.quotes[0].version, 2);
        } else if mode == "revision" || mode == "withdraw" {
            let operation = if mode == "revision" {
                "contract.revise"
            } else {
                "contract.withdraw"
            };
            let body = if mode == "revision" {
                revise(&c, 130001)
            } else {
                json!({"expected_version":1,"contact":"Carrier agreement lead","reference":"Actual withdrawal","note":"No future sourcing"})
            };
            let (change, usage) = tokio::join!(
                a.mutate_authorized((&tenant, "ops:a"), "change", operation, cid, body),
                b.mutate_authorized(
                    (&tenant, "ops:b"),
                    "source",
                    "quote.contract-offer",
                    qid,
                    source(&c, &q)
                )
            );
            change.expect("agreement change commits");
            let w = a.read(&tenant).await.expect("read");
            assert_eq!(w.carrier_contracts[0].version, 2);
            if usage.is_ok() {
                let offer = &w.quotes[0].revisions[0].offers[0];
                assert_eq!(offer.amount_cents, 125001);
                assert_eq!(offer.rate_source.as_ref().expect("source").revision, 1);
            } else {
                assert!(w.quotes[0].revisions[0].offers.is_empty());
            }
            let mut stale = source(&c, &q);
            stale["expected_version"] = json!(w.quotes[0].version);
            assert!(a
                .mutate_authorized(
                    (&tenant, "ops:a"),
                    "after-change",
                    "quote.contract-offer",
                    qid,
                    stale
                )
                .await
                .is_err());
        } else {
            data["carrier_contracts"][0]["revisions"][0]["html"] = json!("corrupt original bytes");
            canonical::put_workspace(&pool, &tenant, &data)
                .await
                .expect("corrupt owned bytes");
            let before = json!(a.read(&tenant).await.expect("read"));
            assert_eq!(
                a.mutate_authorized(
                    (&tenant, "ops:a"),
                    "failed-key",
                    "quote.contract-offer",
                    qid,
                    source(&c, &q)
                )
                .await
                .expect_err("corrupt"),
                "Internal storage error"
            );
            assert_eq!(json!(b.read(&tenant).await.expect("read")), before);
            data["carrier_contracts"][0]["revisions"][0]["html"] =
                c["revisions"][0]["html"].clone();
            canonical::put_workspace(&pool, &tenant, &data)
                .await
                .expect("restore");
            let sourced = a
                .mutate_authorized(
                    (&tenant, "ops:a"),
                    "failed-key",
                    "quote.contract-offer",
                    qid,
                    source(&c, &q),
                )
                .await
                .expect("restored key");
            let offerid = sourced["data"]["revisions"][0]["offers"][0]["id"].clone();
            let original = a.read(&tenant).await.expect("original");
            let mut corrupt = json!(original);
            corrupt["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
            corrupt["carrier_contracts"][0]["revisions"][0]["evidence"]["note"] =
                json!("changed actual source findings");
            canonical::put_workspace(&pool, &tenant, &corrupt)
                .await
                .expect("corrupt proof");
            let issue = json!({"expected_version":2,"offer_id":offerid});
            assert!(a
                .mutate_authorized(
                    (&tenant, "ops:a"),
                    "issue-retry",
                    "quote.issue",
                    qid,
                    issue.clone()
                )
                .await
                .is_err());
            let mut restore = json!(original);
            restore["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
            canonical::put_workspace(&pool, &tenant, &restore)
                .await
                .expect("restore proof");
            a.mutate_authorized((&tenant, "ops:a"), "issue-retry", "quote.issue", qid, issue)
                .await
                .expect("retry after restoration");
            let reconnect = Store::new(Some(&url)).await.expect("reconnect");
            let w = reconnect.read(&tenant).await.expect("read");
            assert_eq!(w.quotes[0].status, "issued");
            assert_eq!(
                w.carrier_contracts[0].current().expect("rev").html,
                c["revisions"][0]["html"].as_str().expect("HTML")
            );
            assert_eq!(
                w.quotes[0].revisions[0].offers[0].evidence.recorded_by,
                "ops:a"
            );
            assert!(w.journal.is_empty());
            assert!(reconnect
                .read("unrelated-contract-tenant")
                .await
                .expect("other")
                .carrier_contracts
                .is_empty());
        }
        sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .expect("owned cleanup");
    }
}
