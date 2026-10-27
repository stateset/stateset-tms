#[path = "support/canonical.rs"]
mod canonical;
#[path = "support/invoice_review.rs"]
mod invoice_review;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Status, Workspace},
    integrations::{Connection, Integrations},
    store::Store,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tower::ServiceExt;
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn w(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "ops:route"),
        &domain::id("key"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
async fn version(s: &Store, id: &str) -> u64 {
    w(s).await
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version
}
fn plan(reference: &str) -> Value {
    let day = (Utc::now() + Duration::days(1)).date_naive();
    let stop = |facility, city, state, postal| json!({"facility":facility,"city":city,"state":state,"date":day,"address_lines":["100 Synthetic Route Street"],"postal_code":postal,"country":"US","instructions":"Actual route instructions <script>escape</script>"});
    json!({"reference":reference,"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Four stop consolidated freight","weight_lbs":1000,"miles":200,"buy_cents":125001,"sell_cents":150001,"origin":stop("Portland pickup","Portland","OR","97201"),"destination":stop("Seattle final delivery","Seattle","WA","98101"),"intermediate_stops":[{"id":"stop_tacoma","kind":"delivery","stop":stop("Tacoma receiver","Tacoma","WA","98402")},{"id":"stop_kent","kind":"pickup","stop":stop("Kent consolidation","Kent","WA","98030")}]})
}
fn router(s: Store) -> Router {
    let keys = [
        ("ops", Role::Operations, None),
        ("finance", Role::Finance, None),
        ("viewer", Role::Viewer, None),
        ("carrier", Role::Carrier, Some("ca4")),
        ("other", Role::Carrier, Some("ca1")),
        ("customer", Role::Customer, Some("c1")),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.to_string(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("actor:{key}"),
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
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
    )
}
async fn publication(s: &Store, id: &str) -> Value {
    let previous = w(s)
        .await
        .portal_offers
        .iter()
        .rev()
        .find(|p| p.load_id == id)
        .map(|p| p.id.clone());
    let prior_rate = w(s)
        .await
        .rate_confirmations
        .iter()
        .rev()
        .find(|p| p.load_id == id)
        .map(|p| p.id.clone());
    let rate=op(s,"rate.issue",id,json!({"expected_load_version":version(s,id).await,"expected_previous_id":prior_rate,"terms":"Actual inclusive four-stop carrier rate; every intermediate service required","reason":"Actual full route agreement"})).await;
    op(
        s,
        "portal.publish",
        id,
        json!({"expected_load_version":version(s,id).await,"rate_confirmation_id":rate["id"],"expected_previous_id":previous}),
    )
    .await
}
async fn tendered() -> (Store, Router, String, Value) {
    let s = setup().await;
    let l = op(&s, "load.create", "", plan("FOUR-STOP")).await;
    let id = l["id"].as_str().expect("id").to_string();
    op(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","carrier_id":"ca4","expected_version":1}),
    )
    .await;
    let p = publication(&s, &id).await;
    let r = router(s.clone());
    (s, r, id, p)
}
async fn accept(s: &Store, r: &Router, id: &str, p: &Value) {
    let (status,result)=call(r,"carrier","POST",&format!("portal/offers/{}/response",p["id"].as_str().expect("offer")),json!({"expected_load_version":version(s,id).await,"decision":"accept","rate_confirmation_id":p["rate_confirmation_id"],"rate_sha256":p["rate_sha256"],"reference":"Actual full route agreement","note":"Carrier reviewed every intermediate stop and full compensation"}),"accept").await;
    assert_eq!(status, 200, "{result}");
}
async fn pickup(s: &Store, r: &Router, id: &str, p: &Value) -> chrono::DateTime<Utc> {
    let at = Utc::now() - Duration::minutes(20);
    let (status,result)=call(r,"carrier","POST",&format!("portal/offers/{}/tracking",p["id"].as_str().expect("offer")),json!({"expected_load_version":version(s,id).await,"kind":"pickup","observed_at":at,"location":"Portland OR","reference":"Actual origin pickup","note":"Freight loaded; no intermediate service invented"}),"pickup").await;
    assert_eq!(status, 200, "{result}");
    at
}
fn service(version: u64, stop: &str, at: chrono::DateTime<Utc>) -> Value {
    json!({"expected_load_version":version,"stop_id":stop,"observed_at":at,"contact":"Actual receiving or shipping lead","reference":format!("ACTUAL-{stop}"),"note":"PRIVATE actual intermediate service evidence inspected"})
}
fn gate_at(version: u64, stop: &str, kind: &str, at: chrono::DateTime<Utc>) -> Value {
    json!({"expected_load_version":version,"stop":stop,"kind":kind,"local_time":at.format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":format!("PRIVATE-GATE-{stop}-{kind}"),"note":"Original actual gate observation"})
}
async fn reject_unchanged(s: &Store, key: &str, operation: &str, id: &str, body: Value) {
    let before = json!(w(s).await);
    let error = s
        .mutate_authorized(("demo", "ops:route"), key, operation, id, body)
        .await
        .expect_err("chronology must reject");
    assert!(error.contains("chronology"), "{error}");
    assert_eq!(
        json!(w(s).await),
        before,
        "No evidence, load version, queue, audit or receipt may change"
    );
}

#[tokio::test]
async fn shared_execution_order_rejects_cross_stop_gate_service_conflicts_and_preserves_failed_key_for_repair(
) {
    let (s, r, id, p) = tendered().await;
    accept(&s, &r, &id, &p).await;
    pickup(&s, &r, &id, &p).await;
    let now = Utc::now();
    let arrival = op(
        &s,
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "stop_kent",
            "arrival",
            now - Duration::minutes(5),
        ),
    )
    .await;
    let body = service(
        version(&s, &id).await,
        "stop_tacoma",
        now - Duration::minutes(4),
    );
    reject_unchanged(
        &s,
        "after-later-arrival",
        "itinerary.service.record",
        &id,
        body,
    )
    .await;
    op(&s,"stop.event.correct",arrival["id"].as_str().expect("arrival"),json!({"expected_load_version":version(&s,&id).await,"local_time":(now-Duration::minutes(3)).format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":"Actual corrected Kent gate time","note":"Retain original record"})).await;
    s.mutate_authorized(
        ("demo", "ops:route"),
        "after-later-arrival",
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_tacoma",
            now - Duration::minutes(4),
        ),
    )
    .await
    .expect("failed key remains reusable after reviewed source correction");
    op(
        &s,
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "pickup",
            "arrival",
            now - Duration::minutes(21),
        ),
    )
    .await;
    reject_unchanged(
        &s,
        "departure-after-stop-service",
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "pickup",
            "departure",
            now - Duration::minutes(3),
        ),
    )
    .await;
    let departure = op(
        &s,
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "pickup",
            "departure",
            now - Duration::minutes(18),
        ),
    )
    .await;
    reject_unchanged(&s,"bad-departure-correction","stop.event.correct",departure["id"].as_str().expect("departure"),json!({"expected_load_version":version(&s,&id).await,"local_time":(now-Duration::minutes(3)).format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":"Conflicting proposed correction","note":"Cannot depart origin after intermediate delivery"})).await;
    let current = w(&s).await;
    let load = current.loads.iter().find(|l| l.id == id).expect("load");
    let timeline = current.execution_timeline(load, true);
    assert_eq!(timeline["consistent"], true);
    assert_eq!(timeline["record_count"], 5);
    assert_eq!(
        current
            .stop_events
            .iter()
            .filter(|g| g.stop == "stop_kent")
            .count(),
        2
    );
    let (_, carrier) = call(&r, "carrier", "GET", "portal", json!({}), "timeline").await;
    let offer = carrier["offers"]
        .as_array()
        .expect("offers")
        .iter()
        .find(|o| o["id"] == p["id"])
        .expect("offer");
    let public = &offer["reporting"]["execution_timeline"];
    assert_eq!(public["record_count"], 5);
    assert!(!public.to_string().contains("PRIVATE-GATE"));
    assert!(!public.to_string().contains("ops:route"));
    assert!(timeline.to_string().contains("PRIVATE-GATE"));
    let (_, customer) = call(
        &r,
        "customer",
        "GET",
        "portal",
        json!({}),
        "customer-timeline",
    )
    .await;
    let shipment = customer["shipments"]
        .as_array()
        .expect("shipments")
        .iter()
        .find(|l| l["id"] == id)
        .expect("shipment");
    assert_eq!(shipment["execution_timeline"], *public);
}

#[tokio::test]
async fn native_pickup_delivery_respect_gate_and_service_times_while_stale_reports_remain_nonbinding_history(
) {
    let (s, r, id, p) = tendered().await;
    accept(&s, &r, &id, &p).await;
    let now = Utc::now();
    op(
        &s,
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "pickup",
            "arrival",
            now - Duration::minutes(10),
        ),
    )
    .await;
    let report = |v, kind: &str, minutes| json!({"expected_load_version":v,"kind":kind,"observed_at":now-Duration::minutes(minutes),"location":"Actual route facility","reference":format!("ACTUAL-{kind}-{minutes}"),"note":"Original carrier driver evidence"});
    let path = format!(
        "portal/offers/{}/tracking",
        p["id"].as_str().expect("offer")
    );
    let before = json!(w(&s).await);
    let (status, result) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "pickup", 15),
        "pickup-before-arrival",
    )
    .await;
    assert_eq!(status, 422, "{result}");
    assert_eq!(json!(w(&s).await), before);
    let (status, result) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "pickup", 9),
        "actual-pickup",
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let first = op(
        &s,
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_tacoma",
            now - Duration::minutes(7),
        ),
    )
    .await;
    op(
        &s,
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_kent",
            now - Duration::minutes(5),
        ),
    )
    .await;
    op(
        &s,
        "stop.event.record",
        &id,
        gate_at(
            version(&s, &id).await,
            "delivery",
            "arrival",
            now - Duration::minutes(3),
        ),
    )
    .await;
    let before = json!(w(&s).await);
    let (status, result) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "delivery", 4),
        "delivery-before-gate",
    )
    .await;
    assert_eq!(status, 422, "{result}");
    assert_eq!(json!(w(&s).await), before);
    let (status, result) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "delivery", 2),
        "actual-delivery",
    )
    .await;
    assert_eq!(status, 200, "{result}");
    let before = json!(w(&s).await);
    let (status, result) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "pickup", 1),
        "new-conflicting-pickup",
    )
    .await;
    assert_eq!(status, 422, "{result}");
    assert_eq!(json!(w(&s).await), before);
    let (status, ignored) = call(
        &r,
        "carrier",
        "POST",
        &path,
        report(version(&s, &id).await, "pickup", 4),
        "old-pickup-retained",
    )
    .await;
    assert_eq!(status, 200, "{ignored}");
    assert_eq!(ignored["data"]["outcome"], "stale_ignored");
    op(
        &s,
        "itinerary.service.correct",
        first["id"].as_str().expect("service"),
        service(
            version(&s, &id).await,
            "stop_tacoma",
            now - Duration::minutes(6),
        ),
    )
    .await;
    let current = w(&s).await;
    let load = current.loads.iter().find(|l| l.id == id).expect("load");
    assert!(current.execution_consistent(load));
    assert_eq!(current.execution_timeline(load, true)["record_count"], 6);
    assert_eq!(current.carrier_submissions.len(), 3);
}
#[tokio::test]
async fn route_plans_validate_order_identity_and_addresses_without_applying_two_stop_contract_prices(
) {
    let s = setup().await;
    for kind in ["order", "duplicate", "kind", "count"] {
        let mut body = plan(&format!("BAD-{kind}"));
        match kind {
            "order" => {
                body["intermediate_stops"][0]["stop"]["date"] = json!(Utc::now().date_naive())
            }
            "duplicate" => body["intermediate_stops"][1]["id"] = json!("stop_tacoma"),
            "kind" => body["intermediate_stops"][0]["kind"] = json!("skip"),
            _ => {
                body["intermediate_stops"] = json!((0..19)
                    .map(|i| {
                        let mut v = body["intermediate_stops"][0].clone();
                        v["id"] = json!(format!("stop_{i}"));
                        v
                    })
                    .collect::<Vec<_>>())
            }
        };
        assert!(
            s.mutate("demo", &domain::id("key"), "load.create", "", body)
                .await
                .is_err(),
            "{kind}"
        );
    }
    let mut body = plan("MISSING-ADDRESS");
    body["intermediate_stops"][0]["stop"]["address_lines"] = json!([]);
    let l = op(&s, "load.create", "", body).await;
    assert!(s
        .mutate(
            "demo",
            "missing-address",
            "load.command",
            l["id"].as_str().expect("id"),
            json!({"action":"tender","carrier_id":"ca4","expected_version":1})
        )
        .await
        .is_err());
    let body = plan("FULL-QUOTE");
    let mut p = body;
    p.as_object_mut().expect("object").remove("reference");
    p.as_object_mut().expect("object").remove("buy_cents");
    p.as_object_mut().expect("object").remove("sell_cents");
    let q=op(&s,"quote.create","",json!({"reference":"MULTI-QUOTE","plan":p,"sell_cents":150001,"valid_until":Utc::now()+Duration::hours(12),"terms":"Explicit full-route customer agreement"})).await;
    let workspace = w(&s).await;
    let (_, html) =
        stateset_freight::quotes::statement(&workspace, q["id"].as_str().expect("q"), None)
            .expect("quote document");
    assert!(html.contains("Tacoma receiver") && html.contains("Kent consolidation"));
    assert!(!html.contains("<script>escape"));
    let contract=op(&s,"contract.create","",json!({"number":"TWO-ENDPOINT-ONLY","carrier_id":"ca4","rate":{"customer_id":"c1","origin":{"country":"US","state":"OR","postal_code":"97201"},"destination":{"country":"US","state":"WA","postal_code":"98101"},"equipment":"Dry van","max_weight_lbs":45000,"max_miles":200,"effective_from":Utc::now().date_naive(),"effective_through":(Utc::now()+Duration::days(30)).date_naive(),"amount_cents":125001,"terms":"Explicit two-stop lane price"},"contact":"Actual carrier contracting lead","reference":"Actual lane agreement","note":"Two endpoints only","reason":"Original carrier agreement"})).await;
    let current = w(&s).await;
    let candidates = current
        .contract_candidates(q["id"].as_str().expect("quote"))
        .expect("candidates");
    assert_eq!(candidates["rates"][0]["eligible"], false);
    assert!(candidates["rates"][0]["reasons"]
        .to_string()
        .contains("intermediate"));
    assert!(s.mutate("demo","no-two-stop-price","quote.contract-offer",q["id"].as_str().expect("quote"),json!({"expected_version":q["version"],"agreement_id":contract["id"],"expected_rate_version":contract["version"],"revision_sha256":contract["revisions"][0]["basis_sha256"],"valid_until":Utc::now()+Duration::hours(12),"contact":"Actual capacity lead","reference":"Actual capacity confirmation","note":"This price does not cover extra stops"})).await.expect_err("scope").contains("not applicable"));
}

async fn historical_timeline_fixture() -> (Workspace, String, String, String, String) {
    let (s, r, id, p) = tendered().await;
    accept(&s, &r, &id, &p).await;
    pickup(&s, &r, &id, &p).await;
    let now = Utc::now();
    for (stop, minutes) in [("stop_tacoma", 10), ("stop_kent", 8)] {
        op(
            &s,
            "itinerary.service.record",
            &id,
            service(
                version(&s, &id).await,
                stop,
                now - Duration::minutes(minutes),
            ),
        )
        .await;
    }
    for (stop, kind, minutes) in [
        ("pickup", "arrival", 21),
        ("pickup", "departure", 18),
        ("stop_kent", "arrival", 9),
        ("delivery", "arrival", 7),
    ] {
        op(
            &s,
            "stop.event.record",
            &id,
            gate_at(
                version(&s, &id).await,
                stop,
                kind,
                now - Duration::minutes(minutes),
            ),
        )
        .await;
    }
    let(status,result)=call(&r,"carrier","POST",&format!("portal/offers/{}/tracking",p["id"].as_str().expect("offer")),json!({"expected_load_version":version(&s,&id).await,"kind":"delivery","observed_at":now-Duration::minutes(5),"location":"Seattle WA","reference":"Actual delivery","note":"Actual final receiver handoff"}),"fixture-delivery").await;
    assert_eq!(status, 200, "{result}");
    op(&s,"load.command",&id,json!({"action":"attach_pod","expected_version":version(&s,&id).await,"pod_reference":"Actual consolidated all-stop signed evidence"})).await;
    s.mutate_authorized(("demo","finance:reviewer"),"fixture-review","pod.review",&id,json!({"expected_version":version(&s,&id).await,"decision":"accepted","note":"Consolidated full route receiver evidence inspected"})).await.expect("review");
    let invoice = op(
        &s,
        "invoice.create",
        "",
        json!({"load_id":id,"number":"EXECUTION-125001","amount_cents":125001,"currency":"USD"}),
    )
    .await;
    let mut current = w(&s).await;
    let mut ids = Vec::new();
    // These cross-stop combinations were accepted by the previous independent
    // gate/service validators. Retain that historical evidence for repair.
    for (stop, minutes) in [("stop_kent", 12), ("delivery", 9)] {
        let event = current
            .stop_events
            .iter_mut()
            .find(|g| g.load_id == id && g.stop == stop)
            .expect("gate");
        event.local_time = (now - Duration::minutes(minutes))
            .format("%Y-%m-%dT%H:%M")
            .to_string();
        event.occurred_at =
            stateset_freight::stops::local_instant(&event.local_time, "UTC", "reject")
                .expect("time");
        ids.push(event.id.clone());
        for audit in &mut current.events {
            if audit.details.as_ref().is_some_and(|d| d["id"] == event.id) {
                audit.details = Some(json!(event));
            }
        }
    }
    (
        current,
        id,
        invoice["id"].as_str().expect("invoice").into(),
        ids[0].clone(),
        ids[1].clone(),
    )
}
fn repair_body(w: &Workspace, load: &str, gate: &str, minutes: i64) -> Value {
    let original = w.stop_events.iter().find(|g| g.id == gate).expect("gate");
    // Use original own-stop service as a stable baseline, independent of test
    // execution duration or the clock at correction entry.
    let at = if original.stop == "stop_kent" {
        w.current_services(w.loads.iter().find(|l| l.id == load).expect("load"))
            .into_iter()
            .find(|s| s.stop_id == "stop_kent")
            .expect("service")
            .observed_at
    } else {
        w.carrier_submissions
            .iter()
            .find(|s| s.load_id == load && s.kind == "delivery")
            .expect("delivery")
            .observed_at
            .expect("time")
    };
    json!({"expected_load_version":w.loads.iter().find(|l|l.id==load).expect("load").version,"local_time":(at-Duration::minutes(minutes)).format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":"Actual original gate time corrected","note":"Original record and other unresolved findings retained"})
}
#[tokio::test]
async fn independent_historical_conflicts_can_be_repaired_one_at_a_time_without_releasing_finance_early(
) {
    let (mut current, id, invoice, kent, delivery) = historical_timeline_fixture().await;
    let original = current.stop_events.clone();
    let load = current.loads.iter().find(|l| l.id == id).expect("load");
    assert!(current.route_ready(load));
    assert!(!current.execution_consistent(load));
    assert_eq!(
        current.execution_timeline(load, true)["conflicts"]
            .as_array()
            .expect("conflicts")
            .len(),
        2
    );
    assert!(!current.delivery_evidence_ready(load));
    assert!(current
        .apply("invoice.approve", &invoice, json!({}))
        .is_err());
    assert!(current.journal.is_empty());
    current
        .apply_stop_operation(
            "stop.event.correct",
            &kent,
            repair_body(&current, &id, &kent, 1),
            "ops:repair",
        )
        .expect("one independent repair");
    assert!(!current.execution_consistent(current.loads.iter().find(|l| l.id == id).expect("load")));
    assert!(current
        .apply("invoice.approve", &invoice, json!({}))
        .is_err());
    current
        .apply_stop_operation(
            "stop.event.correct",
            &delivery,
            repair_body(&current, &id, &delivery, 2),
            "ops:repair",
        )
        .expect("second actual repair");
    assert!(current.execution_consistent(current.loads.iter().find(|l| l.id == id).expect("load")));
    assert_eq!(
        json!(&current.stop_events[..original.len()]),
        json!(original)
    );
    current
        .apply("invoice.approve", &invoice, json!({}))
        .expect("actual reviewed route now qualifies");
    assert_eq!(
        current
            .invoices
            .iter()
            .find(|i| i.id == invoice)
            .expect("invoice")
            .amount_cents,
        125001
    );
}
#[tokio::test]
async fn native_route_documents_acceptance_and_non_bypassable_edi_hold_preserve_actual_transport_boundary(
) {
    let (s, r, id, p) = tendered().await;
    let workspace = w(&s).await;
    let load = workspace.loads.iter().find(|l| l.id == id).expect("load");
    let job = workspace
        .outbox
        .iter()
        .find(|j| j.load_id == id && j.destination == "stateset-edi")
        .expect("job");
    assert_eq!(job.status, "native_only");
    assert_eq!(job.total_attempts, 0);
    assert!(workspace.events.iter().any(|e| e.entity_id == id
        && e.message
            .contains("full-route native carrier coordination required")));
    assert!(!workspace.can_withdraw_tender(load));
    let (_, html, _) = stateset_freight::rate_confirmations::statement(
        &workspace,
        p["rate_confirmation_id"].as_str().expect("rate"),
    )
    .expect("statement");
    assert!(
        html.contains("Tacoma receiver")
            && html.contains("Kent consolidation")
            && html.contains("&lt;script&gt;escape")
    );
    assert!(s
        .mutate("demo", "no-edi-retry", "outbox.retry", &job.id, json!({}))
        .await
        .is_err());
    let tender = workspace
        .tenders
        .iter()
        .find(|t| Some(&t.id) == load.tender_id.as_ref())
        .expect("tender");
    assert!(s.mutate("demo","partial-990","edi.event","",json!({"event_id":"PARTIAL-990","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":tender.shipment_reference,"partner_id":tender.carrier.partner_id,"accepted":true}})).await.expect_err("partial acceptance").contains("route"));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let server = axum::Router::new().fallback(move || {
        let c = counter.clone();
        async move {
            c.fetch_add(1, Ordering::SeqCst);
            "unexpected"
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("addr");
    let task = tokio::spawn(async move { axum::serve(listener, server).await.expect("server") });
    let config = Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(format!("http://{address}")),
                edi_token: Some("owned-fixture".into()),
                ..Default::default()
            },
        )]),
    };
    assert!(!config.dispatch_one(&s, "demo").await.expect("no dispatch"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
    accept(&s, &r, &id, &p).await;
    assert_eq!(
        w(&s)
            .await
            .loads
            .iter()
            .find(|l| l.id == id)
            .expect("load")
            .status,
        Status::Accepted
    );
}
#[tokio::test]
async fn scoped_native_intermediate_services_require_actual_pickup_order_times_and_named_evidence_before_delivery(
) {
    let (s, r, id, p) = tendered().await;
    let path = format!(
        "portal/offers/{}/stop-services",
        p["id"].as_str().expect("id")
    );
    let now = Utc::now() - Duration::minutes(5);
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            service(version(&s, &id).await, "stop_tacoma", now),
            "before-pickup"
        )
        .await
        .0,
        422
    );
    assert_eq!(
        call(
            &r,
            "other",
            "POST",
            &path,
            service(version(&s, &id).await, "stop_tacoma", now),
            "other"
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &r,
            "customer",
            "POST",
            &path,
            service(version(&s, &id).await, "stop_tacoma", now),
            "customer"
        )
        .await
        .0,
        403
    );
    accept(&s, &r, &id, &p).await;
    let pickup = pickup(&s, &r, &id, &p).await;
    let before = json!(w(&s).await);
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            service(version(&s, &id).await, "stop_kent", now),
            "out-of-order"
        )
        .await
        .0,
        422
    );
    assert_eq!(json!(w(&s).await), before);
    assert_eq!(
        call(
            &r,
            "carrier",
            "POST",
            &path,
            service(
                version(&s, &id).await,
                "stop_tacoma",
                pickup - Duration::seconds(1)
            ),
            "before-known-pickup"
        )
        .await
        .0,
        422
    );
    let body = service(version(&s, &id).await, "stop_tacoma", now);
    let (status, first) = call(
        &r,
        "carrier",
        "POST",
        &path,
        body.clone(),
        "native-service-once",
    )
    .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["data"]["recorded_by"], "actor:carrier");
    assert_eq!(
        call(&r, "carrier", "POST", &path, body, "native-service-once")
            .await
            .1["replayed"],
        true
    );
    let (status, last) = call(
        &r,
        "carrier",
        "POST",
        &path,
        service(
            version(&s, &id).await,
            "stop_kent",
            now + Duration::minutes(1),
        ),
        "native-service-two",
    )
    .await;
    assert_eq!(status, 200, "{last}");
    assert!(w(&s)
        .await
        .route_ready(w(&s).await.loads.iter().find(|l| l.id == id).expect("load")));
    let (status,delivered)=call(&r,"carrier","POST",&format!("portal/offers/{}/tracking",p["id"].as_str().expect("id")),json!({"expected_load_version":version(&s,&id).await,"kind":"delivery","observed_at":Utc::now()-Duration::seconds(1),"location":"Seattle WA","reference":"Actual final delivery","note":"All intermediate services confirmed separately"}),"final-delivery").await;
    assert_eq!(status, 200, "{delivered}");
    let workspace = w(&s).await;
    assert!(workspace.journal.is_empty());
    let l = workspace.loads.iter().find(|l| l.id == id).expect("load");
    assert_eq!(l.status, Status::Delivered);
    assert!(
        !workspace.delivery_evidence_ready(l),
        "Still needs finance-reviewed consolidated POD"
    );
}

#[tokio::test]
async fn final_edi_delivery_retains_missing_intermediate_attention_and_finance_requires_every_verified_service_and_reviewed_pod(
) {
    let (s, r, id, p) = tendered().await;
    accept(&s, &r, &id, &p).await;
    pickup(&s, &r, &id, &p).await;
    let workspace = w(&s).await;
    let tender = workspace
        .tenders
        .iter()
        .find(|t| t.load_id == id)
        .expect("tender");
    op(&s,"edi.event","",json!({"event_id":"ACTUAL-FINAL-D1","event_type":"edi.shipment_status.received","occurred_at":Utc::now()-Duration::minutes(3),"payload":{"shipment_id":tender.shipment_reference,"partner_id":tender.carrier.partner_id,"latest_status":"D1"}})).await;
    op(&s,"load.command",&id,json!({"action":"attach_pod","expected_version":version(&s,&id).await,"pod_reference":"Actual consolidated signed receiver manifest covering full route"})).await;
    s.mutate_authorized(("demo","finance:reviewer"),"review-consolidated","pod.review",&id,json!({"expected_version":version(&s,&id).await,"decision":"accepted","note":"Actual consolidated receiver evidence inspected for all ordered stops"})).await.expect("review");
    let invoice = op(
        &s,
        "invoice.create",
        "",
        json!({"load_id":id,"number":"FOUR-STOP-INVOICE","amount_cents":125001,"currency":"USD"}),
    )
    .await;
    let iid = invoice["id"].as_str().expect("invoice");
    assert!(s
        .mutate(
            "demo",
            "no-early-approval",
            "invoice.approve",
            iid,
            json!({})
        )
        .await
        .is_err());
    let current = w(&s).await;
    let load = current.loads.iter().find(|l| l.id == id).expect("load");
    assert_eq!(load.status, Status::Delivered);
    assert!(!current.route_ready(load));
    assert!(current.journal.is_empty());
    let first = op(
        &s,
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_tacoma",
            Utc::now() - Duration::minutes(5),
        ),
    )
    .await;
    assert!(s
        .mutate("demo", "still-missing", "invoice.approve", iid, json!({}))
        .await
        .is_err());
    op(
        &s,
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_kent",
            Utc::now() - Duration::minutes(4),
        ),
    )
    .await;
    let original = w(&s).await.stop_services[0].clone();
    let correction = op(
        &s,
        "itinerary.service.correct",
        first["id"].as_str().expect("service"),
        service(
            version(&s, &id).await,
            "stop_tacoma",
            Utc::now() - Duration::minutes(6),
        ),
    )
    .await;
    let current = w(&s).await;
    assert_eq!(json!(current.stop_services[0]), json!(original));
    assert_eq!(correction["supersedes_id"], first["id"]);
    let mut corrupt = current.clone();
    corrupt.stop_services.last_mut().expect("service").note = "changed proof".into();
    assert!(
        !corrupt.delivery_evidence_ready(corrupt.loads.iter().find(|l| l.id == id).expect("load"))
    );
    assert!(corrupt.apply("invoice.approve", iid, json!({})).is_err());
    assert!(corrupt.journal.is_empty());
    let mut corrupt_original = current.clone();
    corrupt_original.stop_services[0].note = "changed superseded original proof".into();
    assert!(
        !corrupt_original.delivery_evidence_ready(
            corrupt_original
                .loads
                .iter()
                .find(|l| l.id == id)
                .expect("load")
        ),
        "Corrected service still depends on intact original evidence"
    );
    assert!(corrupt_original
        .apply("invoice.approve", iid, json!({}))
        .is_err());
    op(&s, "invoice.approve", iid, json!({})).await;
    let approved = w(&s).await;
    assert_eq!(
        approved
            .invoices
            .iter()
            .find(|i| i.id == iid)
            .expect("invoice")
            .amount_cents,
        125001
    );
    assert!(s
        .mutate(
            "demo",
            "no-rewrite-after-post",
            "itinerary.service.correct",
            correction["id"].as_str().expect("correction"),
            service(
                version(&s, &id).await,
                "stop_tacoma",
                Utc::now() - Duration::minutes(7)
            )
        )
        .await
        .expect_err("locked")
        .contains("locked"));
}

#[tokio::test]
async fn intermediate_appointments_gate_chronology_and_corrections_preserve_exact_facility_baseline(
) {
    let (s, r, id, _) = tendered().await;
    let day = w(&s)
        .await
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .origin
        .date;
    let appointment = |version, key: &str, hour: u32| json!({"expected_load_version":version,"stop":key,"start_local":format!("{day}T{hour:02}:00"),"end_local":format!("{day}T{hour:02}:30"),"timezone":"UTC","contact":"Actual facility scheduling lead","confirmation_reference":format!("FACILITY-{key}"),"note":"Original facility confirmed window"});
    let a = op(
        &s,
        "stop.appointment.book",
        &id,
        appointment(version(&s, &id).await, "stop_tacoma", 10),
    )
    .await;
    op(
        &s,
        "stop.appointment.book",
        &id,
        appointment(version(&s, &id).await, "stop_kent", 11),
    )
    .await;
    assert!(s
        .mutate(
            "demo",
            "crossed-window",
            "stop.appointment.book",
            &id,
            appointment(version(&s, &id).await, "stop_kent", 9)
        )
        .await
        .is_err());
    assert_eq!(a["stop_snapshot"]["facility"], "Tacoma receiver");
    let current = w(&s).await;
    let old = current.loads.iter().find(|l| l.id == id).expect("load");
    let mut altered = old.clone();
    altered.intermediate_stops.swap(0, 1);
    assert!(current.check_stop_revision(old, &altered).is_err());
    let p = publication(&s, &id).await;
    let (_, html, _) = stateset_freight::rate_confirmations::statement(
        &w(&s).await,
        p["rate_confirmation_id"].as_str().expect("rate"),
    )
    .expect("full route document");
    for text in [
        "10:00",
        "11:00",
        "Actual facility scheduling lead",
        "Stop 2 · delivery",
        "Stop 3 · pickup",
    ] {
        assert!(
            html.contains(text),
            "Missing full route document detail: {text}"
        );
    }
    accept(&s, &r, &id, &p).await;
    pickup(&s, &r, &id, &p).await;
    let base = Utc::now() - Duration::minutes(15);
    let local = |t: chrono::DateTime<Utc>| t.format("%Y-%m-%dT%H:%M").to_string();
    let gate = |version, key: &str, kind: &str, time| json!({"expected_load_version":version,"stop":key,"kind":kind,"local_time":local(time),"timezone":"UTC","reference":format!("ACTUAL-{key}-{kind}"),"note":"Actual gate evidence"});
    let arrival = op(
        &s,
        "stop.event.record",
        &id,
        gate(version(&s, &id).await, "stop_tacoma", "arrival", base),
    )
    .await;
    assert!(s.mutate("demo","no-gate-hide","stop.appointment.cancel",a["id"].as_str().expect("appointment"),json!({"expected_load_version":version(&s,&id).await,"reason":"Cannot hide observed arrival baseline"})).await.is_err());
    op(
        &s,
        "itinerary.service.record",
        &id,
        service(
            version(&s, &id).await,
            "stop_tacoma",
            base + Duration::minutes(1),
        ),
    )
    .await;
    op(
        &s,
        "stop.event.record",
        &id,
        gate(
            version(&s, &id).await,
            "stop_tacoma",
            "departure",
            base + Duration::minutes(2),
        ),
    )
    .await;
    let impossible = gate(
        version(&s, &id).await,
        "stop_kent",
        "arrival",
        base - Duration::minutes(1),
    );
    assert!(s
        .mutate(
            "demo",
            "reverse-gates",
            "stop.event.record",
            &id,
            impossible
        )
        .await
        .is_err());
    let correction = json!({"expected_load_version":version(&s,&id).await,"local_time":local(base-Duration::minutes(1)),"timezone":"UTC","reference":"Actual corrected arrival","note":"Original remains readable"});
    op(
        &s,
        "stop.event.correct",
        arrival["id"].as_str().expect("arrival"),
        correction,
    )
    .await;
    let current = w(&s).await;
    assert_eq!(current.stop_events[0].id, arrival["id"]);
    assert!(
        current.stop_execution(
            current.loads.iter().find(|l| l.id == id).expect("load"),
            Utc::now()
        )["stop_tacoma"]["dwell_complete"]
            == true
    );
}

#[tokio::test]
async fn native_route_retirement_retains_old_document_without_replacement_route_or_execution_leakage(
) {
    let (s, r, id, p) = tendered().await;
    let rate = p["rate_confirmation_id"].as_str().expect("rate");
    let before = call(
        &r,
        "carrier",
        "GET",
        &format!("portal/rate-confirmations/{rate}/statement"),
        json!({}),
        "original",
    )
    .await;
    let (status,result)=call(&r,"carrier","POST",&format!("portal/offers/{}/response",p["id"].as_str().expect("id")),json!({"expected_load_version":version(&s,&id).await,"decision":"decline","rate_confirmation_id":p["rate_confirmation_id"],"rate_sha256":p["rate_sha256"],"reference":"Actual carrier cannot serve all stops","note":"No custody; native declined full route"}),"decline-route").await;
    assert_eq!(status, 200, "{result}");
    let mut next = plan("FOUR-STOP");
    next["intermediate_stops"][0]["stop"]["facility"] = json!("PRIVATE REPLACEMENT ROUTE FACILITY");
    op(
        &s,
        "load.revise",
        &id,
        json!({"expected_version":version(&s,&id).await,"plan":next}),
    )
    .await;
    op(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":version(&s,&id).await,"carrier_id":"ca1"}),
    )
    .await;
    let (status, portal) = call(&r, "carrier", "GET", "portal", json!({}), "history").await;
    assert_eq!(status, 200);
    assert!(!portal.to_string().contains("PRIVATE REPLACEMENT"));
    let offer = portal["offers"]
        .as_array()
        .expect("offers")
        .iter()
        .find(|o| o["id"] == p["id"])
        .expect("old offer");
    assert!(offer["load_version"].is_null());
    assert!(offer["reporting"]["itinerary"].is_null());
    assert!(
        offer["plan"]["intermediate_stops"]
            .as_array()
            .expect("old route")
            .len()
            == 2
    );
    assert_eq!(
        call(
            &r,
            "carrier",
            "GET",
            &format!("portal/rate-confirmations/{rate}/statement"),
            json!({}),
            "retained"
        )
        .await,
        before
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_execution_repair_races_hold_legacy_final_tracking_and_release_only_consistent_snapshots(
) {
    let (template, id, invoice, kent, delivery) = historical_timeline_fixture().await;
    let writes = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let captured = writes.clone();
    let mock=Router::new().route("/v1/tenants/sync-execution/orders/{id}",axum::routing::get(|axum::extract::Path(id):axum::extract::Path<String>,headers:axum::http::HeaderMap|async move {assert_eq!(headers["x-stateset-api-key"],"execution-fixture");axum::Json(json!({"data":{"id":id,"tenantId":"sync-execution","orderNumber":"PO-EXECUTION","status":"pending","lineItems":[]}}))})).route("/v1/tenants/sync-execution/orders/{id}/tracking",axum::routing::post(move |headers:axum::http::HeaderMap,axum::Json(body):axum::Json<Value>|{let captured=captured.clone();async move{assert_eq!(headers["x-stateset-api-key"],"execution-fixture");captured.lock().await.push(body);axum::Json(json!({"data":{"ok":true}}))}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("owned fixture");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, mock).await.expect("fixture") });
    let url = std::env::var("TEST_DATABASE_URL").expect("owned DB");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    for legacy in [true, false] {
        let tenant = domain::id("execution-pg");
        let order = uuid::Uuid::new_v4().to_string();
        let mut seed = template.clone();
        let load = seed.loads.iter_mut().find(|l| l.id == id).expect("load");
        load.sync_order_id = Some(order.clone());
        load.order_number = Some("PO-EXECUTION".into());
        load.source_order = Some(
            json!({"id":order,"tenantId":"sync-execution","orderNumber":"PO-EXECUTION","status":"pending","lineItems":[]}),
        );
        let load = load.clone();
        seed.enqueue_sync(&load);
        let job = seed.outbox.last_mut().expect("tracking");
        let job_id = job.id.clone();
        assert_eq!(
            job.payload["trackingData"]["freight_execution_consistent"],
            false
        );
        if legacy {
            job.payload["trackingData"]
                .as_object_mut()
                .expect("tracking")
                .remove("freight_execution_consistent");
        }
        let mut data = json!(seed);
        data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
        canonical::put_workspace(&pool, &tenant, &data)
            .await
            .expect("owned historical fixture");
        let config = Integrations {
            tenants: BTreeMap::from([(
                tenant.clone(),
                Connection {
                    sync_url: Some(format!("http://{address}")),
                    sync_api_key: Some("execution-fixture".into()),
                    sync_tenant_id: Some("sync-execution".into()),
                    ..Default::default()
                },
            )]),
        };
        let before = writes.lock().await.len();
        config
            .dispatch_one(&a, &tenant)
            .await
            .expect("hold without external IO");
        assert_eq!(writes.lock().await.len(), before);
        let current = b.read(&tenant).await.expect("read");
        let job = current.outbox.iter().find(|j| j.id == job_id).expect("job");
        assert_eq!(job.status, "local_only");
        assert_eq!(job.total_attempts, 0);
        assert!(job
            .last_error
            .as_ref()
            .expect("reason")
            .contains("conflicting execution"));
        assert!(b
            .mutate(
                &tenant,
                "no-held-payload-replay",
                "outbox.retry",
                &job_id,
                json!({})
            )
            .await
            .is_err());
        let gate_body = repair_body(&current, &id, &kent, 1);
        let original_service = current
            .current_services(current.loads.iter().find(|l| l.id == id).expect("load"))
            .into_iter()
            .find(|s| s.stop_id == "stop_tacoma")
            .expect("service");
        let service_id = original_service.id.clone();
        let service_body = service(
            current
                .loads
                .iter()
                .find(|l| l.id == id)
                .expect("load")
                .version,
            "stop_tacoma",
            original_service.observed_at - Duration::minutes(3),
        );
        let (x, y) = tokio::join!(
            a.mutate_authorized(
                (&tenant, "ops:a"),
                "gate-repair",
                "stop.event.correct",
                &kent,
                gate_body
            ),
            b.mutate_authorized(
                (&tenant, "ops:b"),
                "service-repair",
                "itinerary.service.correct",
                &service_id,
                service_body
            )
        );
        assert_ne!(
            x.is_ok(),
            y.is_ok(),
            "one current-version repair commits across independent instances"
        );
        let current = a.read(&tenant).await.expect("partial repair");
        assert!(
            !current.execution_consistent(current.loads.iter().find(|l| l.id == id).expect("load"))
        );
        assert!(a
            .mutate(
                &tenant,
                "no-early-finance",
                "invoice.approve",
                &invoice,
                json!({})
            )
            .await
            .is_err());
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "final-repair",
            "stop.event.correct",
            &delivery,
            repair_body(&current, &id, &delivery, 2),
        )
        .await
        .expect("remaining independent repair");
        for _ in 0..5 {
            config
                .dispatch_one(&b, &tenant)
                .await
                .expect("ordered actual HTTP dispatch");
        }
        let captured = writes.lock().await;
        assert_eq!(
            captured.len(),
            before + 1,
            "only the final consistent snapshot reaches Sync"
        );
        assert_eq!(
            captured.last().expect("write")["trackingData"]["freight_execution_consistent"],
            true
        );
        assert!(!captured
            .last()
            .expect("write")
            .to_string()
            .contains("PRIVATE-GATE"));
        drop(captured);
        invoice_review::approve(&a, &tenant, "approval-after-repair", &invoice)
            .await
            .expect("all evidence now valid");
        let reconnect = Store::new(Some(&url)).await.expect("reconnect");
        let current = reconnect.read(&tenant).await.expect("retained");
        assert!(
            current.execution_consistent(current.loads.iter().find(|l| l.id == id).expect("load"))
        );
        assert_eq!(
            json!(&current.stop_events[..template.stop_events.len()]),
            json!(template.stop_events)
        );
        assert!(current
            .events
            .iter()
            .any(|e| e.actor == "ops:a" || e.actor == "ops:b"));
        assert!(reconnect
            .read(&domain::id("unrelated-execution"))
            .await
            .expect("other tenant")
            .stop_events
            .is_empty());
        sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
            .bind(&tenant)
            .execute(&pool)
            .await
            .expect("remove owned tenant");
    }
    pool.close().await;
    server.abort();
    let _ = server.await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_route_service_races_integrity_rollback_reconnect_and_forced_edi_status_remain_safe(
) {
    let (memory, r, id, p) = tendered().await;
    accept(&memory, &r, &id, &p).await;
    pickup(&memory, &r, &id, &p).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned DB");
    let a = Store::new(Some(&url)).await.expect("first writer");
    let b = Store::new(Some(&url)).await.expect("second writer");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("route-pg");
    let mut data = json!(w(&memory).await);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    let edi = data["outbox"]
        .as_array_mut()
        .expect("outbox")
        .iter_mut()
        .find(|j| j["destination"] == "stateset-edi")
        .expect("native job");
    edi["status"] = json!("pending_handoff");
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned fixture");
    let config = Integrations {
        tenants: BTreeMap::from([(
            tenant.clone(),
            Connection {
                edi_url: Some("http://127.0.0.1:1".into()),
                edi_token: Some("owned".into()),
                ..Default::default()
            },
        )]),
    };
    assert!(!config
        .dispatch_one(&a, &tenant)
        .await
        .expect("forced pending must not send partial route"));
    let version = data["loads"]
        .as_array()
        .expect("loads")
        .iter()
        .find(|l| l["id"] == id)
        .expect("load")["version"]
        .as_u64()
        .expect("version");
    let body = service(version, "stop_tacoma", Utc::now() - Duration::minutes(10));
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "serve-a",
            "itinerary.service.record",
            &id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "ops:b"),
            "serve-b",
            "itinerary.service.record",
            &id,
            body
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let current = a.read(&tenant).await.expect("read");
    assert_eq!(current.stop_services.len(), 1);
    let original = current.stop_services[0].clone();
    let v = current
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version;
    let correction = service(v, "stop_tacoma", Utc::now() - Duration::minutes(11));
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "correct-a",
            "itinerary.service.correct",
            &original.id,
            correction.clone()
        ),
        b.mutate_authorized(
            (&tenant, "ops:b"),
            "correct-b",
            "itinerary.service.correct",
            &original.id,
            correction
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    let current = b.read(&tenant).await.expect("read");
    assert_eq!(current.stop_services.len(), 2);
    assert_eq!(json!(current.stop_services[0]), json!(original));
    let mut good = json!(current);
    good["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    let mut corrupt = good.clone();
    corrupt["stop_services"][1]["note"] = json!("changed actual proof");
    canonical::put_workspace(&pool, &tenant, &corrupt)
        .await
        .expect("corrupt owned proof");
    let v = current
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version;
    let second = service(v, "stop_kent", Utc::now() - Duration::minutes(9));
    assert_eq!(
        a.mutate_authorized(
            (&tenant, "ops:a"),
            "retry-after-restore",
            "itinerary.service.record",
            &id,
            second.clone()
        )
        .await
        .expect_err("integrity"),
        "Internal storage error"
    );
    let retained: Value = sqlx::query_scalar(
        "SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id=$1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("persisted");
    assert_eq!(
        retained, corrupt,
        "failed mutation must preserve all records and idempotency state"
    );
    canonical::put_workspace(&pool, &tenant, &good)
        .await
        .expect("restore exact original");
    a.mutate_authorized(
        (&tenant, "ops:a"),
        "retry-after-restore",
        "itinerary.service.record",
        &id,
        second.clone(),
    )
    .await
    .expect("failed key remains available");
    let replay = b
        .mutate_authorized(
            (&tenant, "ops:a"),
            "retry-after-restore",
            "itinerary.service.record",
            &id,
            second,
        )
        .await
        .expect("stable replay");
    assert_eq!(replay["data"]["recorded_by"], "ops:a");
    let reconnect = Store::new(Some(&url)).await.expect("reconnect");
    let current = reconnect.read(&tenant).await.expect("read");
    assert_eq!(current.stop_services.len(), 3);
    assert!(current.route_ready(current.loads.iter().find(|l| l.id == id).expect("load")));
    assert!(current.journal.is_empty());
    assert!(reconnect
        .read(&domain::id("unrelated-route"))
        .await
        .expect("other tenant")
        .stop_services
        .is_empty());
    assert!(current
        .outbox
        .iter()
        .filter(|j| j.destination == "stateset-edi" && j.load_id == id)
        .all(|j| j.total_attempts == 0));
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("remove owned tenant");
    pool.close().await;
}
