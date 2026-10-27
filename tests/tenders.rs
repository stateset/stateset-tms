#[path = "support/canonical.rs"]
mod canonical;
use axum::{routing::post, Json, Router};
use chrono::Utc;
use serde_json::{json, Value};
use stateset_freight::{
    domain::{self, Status},
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
async fn seeded() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    s
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Result<Value, String> {
    s.mutate_authorized(
        ("demo", "operator:test"),
        &domain::id("request"),
        operation,
        id,
        body,
    )
    .await
    .map(|v| v["data"].clone())
}
async fn command(s: &Store, id: &str, action: &str, extra: Value) -> Result<Value, String> {
    let w = s.read("demo").await.expect("read");
    let load = w.loads.iter().find(|v| v.id == id).expect("load");
    let mut body = json!({"action":action,"expected_version":load.version});
    if let Some(fields) = extra.as_object() {
        for (k, v) in fields {
            body[k] = v.clone();
        }
    }
    op(s, "load.command", id, body).await
}
async fn confirmation(s: &Store, release: &str, operation: &str) -> Result<Value, String> {
    let w = s.read("demo").await.expect("read");
    let r = w
        .releases
        .iter()
        .find(|v| v.id == release)
        .expect("release");
    let load = w.loads.iter().find(|v| v.id == r.load_id).expect("load");
    op(s,operation,release,json!({"expected_load_version":load.version,"carrier_contact":"Named carrier dispatcher","confirmation_reference":"Recorded external carrier confirmation","note":"Confirmed this specific tender commitment through the carrier channel"})).await
}
fn tracking(reference: &str, id: &str, partner: &str) -> Value {
    json!({"event_id":id,"event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":reference,"partner_id":partner,"latest_status":"AF","events":[{"status_code":"AF"}]}})
}

#[tokio::test]
async fn proven_unsent_tender_can_be_withdrawn_and_same_carrier_retender_has_a_new_identity() {
    let s = seeded().await;
    command(&s, "load_5", "tender", json!({"carrier_id":"ca2"}))
        .await
        .expect("tender");
    let first = s.read("demo").await.expect("read").tenders[0].clone();
    assert_ne!(first.shipment_reference, "FR-1052");
    assert_eq!(first.created_by, "operator:test");
    command(&s, "load_5", "withdraw_tender", json!({}))
        .await
        .expect("withdraw before any claim");
    let withdrawn = s.read("demo").await.expect("read");
    assert_eq!(withdrawn.loads[4].status, Status::Planned);
    assert_eq!(withdrawn.tenders[0].status, "withdrawn");
    assert_eq!(withdrawn.outbox[0].status, "superseded");
    command(&s, "load_5", "tender", json!({"carrier_id":"ca2"}))
        .await
        .expect("new tender");
    let before = s.read("demo").await.expect("read");
    let second = &before.tenders[1];
    let mut legacy = json!(before);
    legacy["outbox"][1]
        .as_object_mut()
        .expect("job")
        .remove("transmission");
    let migrated: domain::Workspace = serde_json::from_value(legacy).expect("legacy snapshot");
    assert!(
        !migrated.can_withdraw_tender(&migrated.loads[4]),
        "missing transmission history never proves no handoff"
    );
    assert_ne!(first.shipment_reference, second.shipment_reference);
    let old = json!({"event_id":"old-accept","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":first.shipment_reference,"partner_id":"pclg","accepted":true}});
    let result = op(&s, "edi.event", "", old)
        .await
        .expect("old response retained");
    assert_eq!(result["ignored"], true);
    let after = s.read("demo").await.expect("read");
    assert_eq!(json!(after.loads), json!(before.loads));
    assert_eq!(after.outbox[1].status, "pending_handoff");
    assert!(op(&s,"edi.event","",json!({"event_id":"ambiguous","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":"FR-1052","partner_id":"pclg","accepted":true}})).await.is_err());
    op(&s,"edi.event","",json!({"event_id":"new-accept","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":second.shipment_reference,"partner_id":"pclg","accepted":true}})).await.expect("current response");
    assert_eq!(
        s.read("demo").await.expect("read").loads[4].status,
        Status::Accepted
    );
}

#[tokio::test]
async fn uncertain_handoff_survives_retry_reset_and_release_pauses_new_transmissions() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let upstream = Router::new().route(
        "/v1/edi/outbound/204/pclg",
        post(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"uncertain upstream failure"})),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listen");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.expect("serve") });
    let integration = Integrations {
        tenants: BTreeMap::from([(
            "demo".into(),
            Connection {
                edi_url: Some(format!("http://{address}")),
                edi_token: Some("test".into()),
                edi_tenant_id: Some("demo".into()),
                ..Connection::default()
            },
        )]),
    };
    let s = seeded().await;
    command(&s, "load_5", "tender", json!({"carrier_id":"ca2"}))
        .await
        .expect("tender");
    integration
        .dispatch_one(&s, "demo")
        .await
        .expect("failed handoff recorded");
    let job = s.read("demo").await.expect("read").outbox[0].clone();
    assert_eq!(job.transmission, "attempted");
    assert_eq!(job.total_attempts, 1);
    let failed = s.read("demo").await.expect("read");
    let failure_case = failed
        .operations_cases
        .iter()
        .find(|case| case.source.kind == "outbound_failure" && case.source.subject_id == job.id)
        .expect("the first actual outbound failure must create an owned-work candidate");
    assert_eq!(failure_case.status, "open");
    let failure_id = failure_case.id.clone();
    op(&s, "outbox.retry", &job.id, json!({}))
        .await
        .expect("operator retry reset");
    assert_eq!(
        s.read("demo")
            .await
            .expect("read")
            .operations_cases
            .iter()
            .find(|case| case.id == failure_id)
            .expect("same issue after retry request")
            .status,
        "open"
    );
    assert!(command(&s, "load_5", "cancel", json!({})).await.is_err());
    assert!(command(&s, "load_5", "withdraw_tender", json!({}))
        .await
        .is_err());
    let current = s.read("demo").await.expect("read").loads[4].clone();
    let release=op(&s,"tender.release.request","load_5",json!({"expected_load_version":current.version,"disposition":"replan","reason":"Change carrier before pickup"})).await.expect("release request");
    assert!(!integration.dispatch_one(&s, "demo").await.expect("paused"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(op(&s, "outbox.retry", &job.id, json!({})).await.is_err());
    let id = release["id"].as_str().expect("id");
    confirmation(&s, id, "tender.release.confirm")
        .await
        .expect("actual carrier release recorded");
    let after = s.read("demo").await.expect("read");
    assert_eq!(after.loads[4].status, Status::Planned);
    assert_eq!(after.tenders[0].status, "released");
    assert_eq!(after.outbox[0].status, "superseded");
    assert_eq!(
        after
            .operations_cases
            .iter()
            .find(|case| case.id == failure_id)
            .expect("retained superseded handoff history")
            .status,
        "resolved"
    );
    assert_eq!(
        after.releases[0]
            .confirmation
            .as_ref()
            .expect("proof")
            .recorded_by,
        "operator:test"
    );
    server.abort();
}

#[tokio::test]
async fn legacy_acceptance_requires_release_and_late_movement_creates_a_custody_hold() {
    let s = seeded().await;
    let unit = op(
        &s,
        "equipment.create",
        "",
        json!({"unit":"RELEASE-UNIT","kind":"Dry van","plate":"TEST 001"}),
    )
    .await
    .expect("unit");
    assert!(command(&s, "load_4", "cancel", json!({})).await.is_err());
    let release=op(&s,"tender.release.request","load_4",json!({"expected_load_version":1,"disposition":"replan","reason":"Carrier requested substitution before pickup"})).await.expect("release");
    assert!(command(
        &s,
        "load_4",
        "dispatch",
        json!({"driver_id":"d3","equipment_id":unit["id"]})
    )
    .await
    .is_err());
    confirmation(
        &s,
        release["id"].as_str().expect("id"),
        "tender.release.confirm",
    )
    .await
    .expect("release");
    command(&s, "load_4", "tender", json!({"carrier_id":"ca4"}))
        .await
        .expect("same carrier new offer");
    let before = s.read("demo").await.expect("read");
    let result = op(
        &s,
        "edi.event",
        "",
        tracking("FR-1051", "old-pickup", "brex"),
    )
    .await
    .expect("custody conflict retained");
    assert_eq!(result["held"], true);
    let held = s.read("demo").await.expect("read");
    assert_eq!(json!(held.loads), json!(before.loads));
    assert!(held.unresolved_tender_incidents("load_4"));
    command(&s, "load_4", "accept", json!({}))
        .await
        .expect("new carrier accepted");
    assert!(command(
        &s,
        "load_4",
        "dispatch",
        json!({"driver_id":"d3","equipment_id":unit["id"]})
    )
    .await
    .is_err());
    let current = s.read("demo").await.expect("read").loads[3].clone();
    op(&s,"tender.incident.resolve",result["incident_id"].as_str().expect("incident"),json!({"expected_load_version":current.version,"carrier_contact":"Carrier supervisor","confirmation_reference":"CASE-123","note":"Carrier confirmed old pickup scan was erroneous; current tender remains correct"})).await.expect("external reconciliation");
    command(
        &s,
        "load_4",
        "dispatch",
        json!({"driver_id":"d3","equipment_id":unit["id"]}),
    )
    .await
    .expect("dispatch after proof");
    let after = s.read("demo").await.expect("read");
    assert_eq!(after.tenders.len(), 2);
    assert_eq!(after.tender_incidents[0].status, "resolved");
    assert_eq!(after.tenders[0].carrier.scac, "BREX");
}

#[tokio::test]
async fn pickup_during_release_records_truth_and_prevents_cancellation() {
    let s = seeded().await;
    let release=op(&s,"tender.release.request","load_4",json!({"expected_load_version":1,"disposition":"cancel","reason":"Customer requested cancellation"})).await.expect("request");
    op(
        &s,
        "edi.event",
        "",
        tracking("FR-1051", "pickup-during-release", "brex"),
    )
    .await
    .expect("actual movement retained");
    let after = s.read("demo").await.expect("read");
    assert_eq!(after.loads[3].status, Status::InTransit);
    assert_eq!(after.releases[0].status, "blocked_by_movement");
    assert_eq!(after.tender_incidents.len(), 1);
    assert!(confirmation(
        &s,
        release["id"].as_str().expect("id"),
        "tender.release.confirm"
    )
    .await
    .is_err());
    assert!(command(&s, "load_4", "cancel", json!({})).await.is_err());
}

#[tokio::test]
async fn retained_commitment_stale_confirmations_and_financial_guards_are_explicit() {
    let s = seeded().await;
    let release=op(&s,"tender.release.request","load_4",json!({"expected_load_version":1,"disposition":"cancel","reason":"Investigate cancellation request"})).await.expect("request");
    let id = release["id"].as_str().expect("id");
    let before = json!(s.read("demo").await.expect("read"));
    assert!(op(&s,"tender.release.confirm",id,json!({"expected_load_version":1,"carrier_contact":"Dispatcher","confirmation_reference":"REF","note":"Stale confirmation"})).await.is_err());
    assert_eq!(json!(s.read("demo").await.expect("read")), before);
    confirmation(&s, id, "tender.release.retain")
        .await
        .expect("carrier stays committed");
    assert_eq!(
        s.read("demo").await.expect("read").loads[3].status,
        Status::Accepted
    );
    assert!(confirmation(&s, id, "tender.release.confirm")
        .await
        .is_err());
    op(
        &s,
        "invoice.create",
        "",
        json!({"number":"PREPICKUP","load_id":"load_4","amount_cents":142000,"currency":"USD"}),
    )
    .await
    .expect("received external charge");
    let current = s.read("demo").await.expect("read").loads[3].clone();
    assert!(op(&s,"tender.release.request","load_4",json!({"expected_load_version":current.version,"disposition":"replan","reason":"Cannot detach the outstanding invoice"})).await.is_err());
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_release_decisions_serialize_across_instances_and_preserve_history() {
    let url = std::env::var("TEST_DATABASE_URL").expect("url");
    let a = Store::new(Some(&url)).await.expect("connect");
    let b = Store::new(Some(&url)).await.expect("second");
    let tenant = domain::id("release-test");
    let pool = sqlx::PgPool::connect(&url).await.expect("sql");
    canonical::put_workspace(&pool, &tenant, &json!(domain::demo()))
        .await
        .expect("fixture");
    let request=a.mutate_authorized((&tenant,"operator:a"),"release","tender.release.request","load_4",json!({"expected_load_version":1,"disposition":"cancel","reason":"External carrier release"})).await.expect("request");
    let id = request["data"]["id"].as_str().expect("id");
    let body = json!({"expected_load_version":2,"carrier_contact":"Carrier dispatch","confirmation_reference":"RELEASE-123","note":"Confirmed cancellation before pickup"});
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "operator:a"),
            "confirm-a",
            "tender.release.confirm",
            id,
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "operator:b"),
            "confirm-b",
            "tender.release.retain",
            id,
            body
        )
    );
    assert_ne!(first.is_ok(), second.is_ok());
    drop(a);
    drop(b);
    let restart = Store::new(Some(&url)).await.expect("restart");
    let saved = restart.read(&tenant).await.expect("read");
    assert_eq!(saved.releases.len(), 1);
    assert!(saved.releases[0].confirmation.is_some());
    assert_eq!(saved.receipts.len(), 2);
    assert_eq!(saved.tenders.len(), 1);
    assert!(restart
        .read("other-release-tenant")
        .await
        .expect("read")
        .releases
        .is_empty());
}

#[tokio::test]
async fn a_released_carriers_invoice_does_not_become_the_new_carriers_liability() {
    let s = seeded().await;
    let release = op(
        &s,
        "tender.release.request",
        "load_4",
        json!({"expected_load_version":1,"disposition":"replan","reason":"Change original offer"}),
    )
    .await
    .expect("release");
    confirmation(
        &s,
        release["id"].as_str().expect("id"),
        "tender.release.confirm",
    )
    .await
    .expect("confirm");
    command(&s, "load_4", "tender", json!({"carrier_id":"ca1"}))
        .await
        .expect("different carrier");
    command(&s, "load_4", "accept", json!({}))
        .await
        .expect("accept");
    let w = s.read("demo").await.expect("read");
    let reference = &w.tenders[1].shipment_reference;
    op(&s,"edi.event","",json!({"event_id":"delivery-current","event_type":"edi.shipment_status.received","occurred_at":Utc::now(),"payload":{"shipment_id":reference,"partner_id":"smtt","latest_status":"D1","events":[{"status_code":"AF"},{"status_code":"D1"}]}})).await.expect("delivery");
    command(
        &s,
        "load_4",
        "attach_pod",
        json!({"pod_reference":"Actual current carrier delivery proof"}),
    )
    .await
    .expect("pod");
    let load = s.read("demo").await.expect("read").loads[3].clone();
    op(&s,"pod.review","load_4",json!({"expected_version":load.version,"decision":"accepted","note":"Reviewed actual current carrier delivery"})).await.expect("review");
    let before = s.read("demo").await.expect("read");
    assert!(before.delivery_evidence_ready(&before.loads[3]));
    let result=op(&s,"edi.event","",json!({"event_id":"prior-invoice","event_type":"edi.freight_invoice.received","occurred_at":Utc::now(),"payload":{"shipment_id":"FR-1051","partner_id":"brex","invoice_number":"OLD-CARRIER","net_amount":"12.34","currency":"USD"}})).await.expect("conflict retained");
    assert_eq!(result["held"], true);
    let held = s.read("demo").await.expect("read");
    assert_eq!(held.invoices.len(), before.invoices.len());
    assert_eq!(held.journal.len(), before.journal.len());
    assert!(!held.delivery_evidence_ready(&held.loads[3]));
}
