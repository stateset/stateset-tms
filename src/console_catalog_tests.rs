use super::*;
use crate::{auth::Role, finance_read_tests::fixture};
fn principal() -> Principal {
    Principal {
        tenant_id: "demo".into(),
        subject: "user:dispatcher".into(),
        role: Role::Viewer,
        counterparty_id: None,
    }
}
fn query(v: Value) -> Query {
    serde_json::from_value(v).expect("query")
}
fn page(w: &Workspace, v: Value) -> Value {
    w.console_page("demo", &principal(), query(v))
        .expect("page")
}
fn large() -> Workspace {
    let mut w = fixture();
    let bill = w.bills[0].clone();
    let settlement = w.settlements[0].clone();
    w.bills.clear();
    w.settlements.clear();
    w.cash_entries.clear();
    w.credits.clear();
    w.carrier_credit_applications.clear();
    for n in 0..123 {
        let mut b = bill.clone();
        b.id = format!("bill-{n:03}");
        b.number = format!("INV-{n:03}");
        b.snapshot.amount_cents = if n == 122 { 7 } else { 150000 };
        w.bills.push(b);
        let mut s = settlement.clone();
        s.id = format!("settlement-{n:03}");
        w.settlements.push(s);
    }
    w
}
#[test]
fn financial_pages_cover_all_records_once_with_workspace_totals_and_no_mutation() {
    let w = large();
    let before = serde_json::to_vec(&w).expect("original");
    let summary = FinanceReadIndex::new(&w).summary().expect("summary");
    for (view, key) in [("billing", "bills"), ("payables", "settlements")] {
        let mut cursor = Value::Null;
        let mut ids = Vec::new();
        let mut lengths = Vec::new();
        loop {
            let p = page(&w, json!({"view":view,"cursor":cursor}));
            let rows = p[key].as_array().expect("rows");
            lengths.push(rows.len());
            assert_eq!(p["total"], 123);
            assert_eq!(
                p["finance"]["receivable_cents"],
                summary["receivable_cents"]
            );
            assert_eq!(p["totals_scope"], "workspace");
            ids.extend(
                rows.iter()
                    .map(|r| r["id"].as_str().expect("id").to_owned()),
            );
            cursor = p["next_cursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        assert_eq!(lengths, vec![50, 50, 23]);
        assert!(ids.windows(2).all(|p| p[0] < p[1]));
    }
    assert_eq!(before, serde_json::to_vec(&w).expect("unchanged"));
}
#[test]
fn filters_search_entire_catalog_and_preserve_exact_finance_totals() {
    let mut w = large();
    let total = page(&w, json!({"view":"billing"}))["finance"].clone();
    let p = page(&w, json!({"view":"billing","q":"  inv-122 "}));
    assert_eq!(p["bills"][0]["id"], "bill-122");
    assert_eq!(p["finance"], total);
    assert_eq!(page(&w, json!({"view":"billing","q":"0.07"}))["total"], 1);
    assert_eq!(
        page(&w, json!({"view":"billing","q":"customer_id"}))["total"],
        0
    );
    w.bills[1].status = "void".into();
    w.bills[2].snapshot.amount_cents = -1;
    for (balance, count) in [
        ("open", 121),
        ("overdue", 121),
        ("zero", 1),
        ("credits", 1),
        ("unknown", 0),
    ] {
        assert_eq!(
            page(&w, json!({"view":"billing","balance":balance}))["total"],
            count,
            "{balance}"
        );
    }
    let indexed = FinanceReadIndex::new(&w);
    for row in page(&w, json!({"view":"billing","limit":100}))["bills"]
        .as_array()
        .expect("rows")
    {
        let b = w.bills.iter().find(|b| b.id == row["id"]).expect("bill");
        assert_eq!(
            row["totals"],
            json!(indexed.bill_totals(b).expect("totals"))
        );
        assert!(row["snapshot"].get("customer").is_none());
    }
}
#[test]
fn cursor_is_bound_to_actor_tenant_filters_page_size_and_matching_identities() {
    let mut w = large();
    let first = page(&w, json!({"view":"billing"}));
    let cursor = &first["next_cursor"];
    for extra in [
        json!({"q":"INV"}),
        json!({"balance":"open"}),
        json!({"limit":10}),
        json!({"view":"payables"}),
    ] {
        let mut q = json!({"view":"billing","cursor":cursor});
        for (k, v) in extra.as_object().expect("extra") {
            q[k] = v.clone();
        }
        assert!(w.console_page("demo", &principal(), query(q)).is_err());
    }
    assert!(w
        .console_page(
            "other",
            &principal(),
            query(json!({"view":"billing","cursor":cursor}))
        )
        .is_err());
    let mut actor = principal();
    actor.subject = "user:someone-else".into();
    assert!(w
        .console_page(
            "demo",
            &actor,
            query(json!({"view":"billing","cursor":cursor}))
        )
        .is_err());
    // Financial changes that preserve membership produce fresh values on the next page.
    w.bills[50].snapshot.amount_cents = 42;
    assert_eq!(
        page(&w, json!({"view":"billing","cursor":cursor}))["bills"][0]["snapshot"]["amount_cents"],
        42
    );
    w.bills.remove(0);
    assert!(w
        .console_page(
            "demo",
            &principal(),
            query(json!({"view":"billing","cursor":cursor}))
        )
        .is_err());
    let decoded: Value =
        serde_json::from_slice(&hex::decode(cursor.as_str().expect("cursor")).expect("hex"))
            .expect("json");
    let mut altered = decoded;
    altered["after"] = json!("not-present");
    assert!(w.console_page("demo",&principal(),query(json!({"view":"billing","cursor":hex::encode(serde_json::to_vec(&altered).expect("bytes"))}))).is_err());
}
#[test]
fn queues_share_one_bound_with_direction_qualified_ids_and_private_payloads_omitted() {
    let mut w = fixture();
    let out: crate::domain::Outbox = serde_json::from_value(json!({"id":"fixture","load_id":"load_6","destination":"owned-fixture","status":"pending","method":"POST","path":"/owned","payload":{}})).expect("outbox");
    w.outbox.clear();
    w.inbox.clear();
    for n in 0..61 {
        let mut o = out.clone();
        o.id = format!("same-{n:03}");
        o.payload = json!({"private":"PAYLOAD-CANARY"});
        o.response = Some("RESPONSE-CANARY".into());
        o.lease = Some("LEASE-CANARY".into());
        o.status = if n == 60 { "failed" } else { "sent" }.into();
        w.outbox.push(o);
        w.inbox.push(serde_json::from_value(json!({"id":format!("same-{n:03}"),"document_id":format!("DOC-{n:03}"),"delivery_ids":[],"key_version":"KEY-CANARY","notification":{},"original_body":"BODY-CANARY","fingerprint":"fingerprint","status":"held","attempts":0,"total_attempts":0,"history":[],"received_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"})).expect("inbox"));
    }
    let mut cursor = Value::Null;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let p = page(&w, json!({"view":"integrations","cursor":cursor}));
        let mut count = 0;
        for key in ["outbox", "inbox"] {
            for row in p[key].as_array().expect("rows") {
                count += 1;
                assert!(seen.insert(format!("{key}:{}", row["id"])));
            }
        }
        assert!(count <= 50);
        let raw = p.to_string();
        for private in [
            "PAYLOAD-CANARY",
            "RESPONSE-CANARY",
            "LEASE-CANARY",
            "KEY-CANARY",
            "BODY-CANARY",
        ] {
            assert!(!raw.contains(private));
        }
        cursor = p["next_cursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen.len(), 122);
    assert_eq!(
        page(&w, json!({"view":"integrations","q":"EDI callback"}))["total"],
        61
    );
    assert_eq!(
        page(&w, json!({"view":"integrations","attention":true}))["total"],
        62
    );
    let p = page(
        &w,
        json!({"view":"integrations","direction":"outbound","attention":true}),
    );
    assert_eq!(p["total"], 1);
    assert_eq!(p["outbox"][0]["id"], "same-060");
    assert_eq!(
        page(
            &w,
            json!({"view":"integrations","direction":"inbound","q":"DOC-060"})
        )["total"],
        1
    );
}
#[test]
fn strict_queries_fail_closed_including_overflow_and_duplicate_ids() {
    let mut w = large();
    for extra in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"q":"x".repeat(201)}),
        json!({"cursor":"not-hex"}),
        json!({"cursor":"a".repeat(2049)}),
        json!({"attention":false}),
        json!({"direction":"all"}),
    ] {
        let mut q = json!({"view":"billing"});
        for (k, v) in extra.as_object().expect("extra") {
            q[k] = v.clone();
        }
        assert!(w.console_page("demo", &principal(), query(q)).is_err());
    }
    for q in [
        json!({"view":"missing"}),
        json!({"view":"billing","tenant_id":"other"}),
        json!({"view":"billing","balance":"missing"}),
    ] {
        assert!(serde_json::from_value::<Query>(q).is_err());
    }
    assert!(w
        .console_page(
            "demo",
            &principal(),
            query(json!({"view":"payables","balance":"overdue"}))
        )
        .is_err());
    assert!(w
        .console_page(
            "demo",
            &principal(),
            query(json!({"view":"integrations","balance":"all"}))
        )
        .is_err());
    w.bills.push(w.bills[0].clone());
    assert!(w
        .console_page("demo", &principal(), query(json!({"view":"billing"})))
        .is_err());
    w.bills.pop();
    w.bills[0].snapshot.amount_cents = i64::MAX;
    assert!(w
        .console_page("demo", &principal(), query(json!({"view":"billing"})))
        .is_err());
}
#[tokio::test]
async fn http_catalog_enforces_roles_tenant_scope_strict_queries_and_no_store() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::{collections::BTreeMap, sync::Arc};
    use tower::ServiceExt;
    let store = crate::store::Store::new(None).await.expect("store");
    store
        .update("demo", |w| {
            *w = large();
            Ok(json!({}))
        })
        .await
        .expect("seed");
    let keys = [
        ("viewer", Role::Viewer, "demo", None),
        ("other", Role::Viewer, "other", None),
        ("carrier", Role::Carrier, "demo", Some("ca1")),
        ("customer", Role::Customer, "demo", Some("c1")),
    ]
    .into_iter()
    .map(|(key, role, tenant, party)| {
        (
            key.into(),
            crate::auth::Credential::Scoped(Principal {
                tenant_id: tenant.into(),
                subject: format!("user:{key}"),
                role,
                counterparty_id: party.map(str::to_owned),
            }),
        )
    })
    .collect::<BTreeMap<_, _>>();
    let app = crate::api::router(crate::api::App {
        pdf_renderer: Default::default(),
        identity: Arc::new(None),
        store,
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(crate::integrations::Integrations::default()),
    });
    for (key, status) in [
        ("viewer", 200),
        ("other", 200),
        ("carrier", 403),
        ("customer", 403),
        ("unknown", 401),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/console?view=billing&limit=2")
                    .header("authorization", format!("Bearer {key}"))
                    .header("x-tenant-id", "demo")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), status);
        if status == 200 {
            assert_eq!(response.headers()["cache-control"], "no-store");
            let data: Value = serde_json::from_slice(
                &response
                    .into_body()
                    .collect()
                    .await
                    .expect("body")
                    .to_bytes(),
            )
            .expect("json");
            assert_eq!(
                data["tenant_id"],
                if key == "other" { "other" } else { "demo" }
            );
            assert_eq!(
                data["bills"].as_array().expect("rows").len(),
                if key == "other" { 0 } else { 2 }
            );
        }
    }
    for path in [
        "/v1/console",
        "/v1/console?view=billing&tenant_id=other",
        "/v1/console?view=payables&balance=overdue",
        "/v1/console?view=billing&limit=101",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", "Bearer viewer")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(response.status().is_client_error(), "{path}");
    }
}
