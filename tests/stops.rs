use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{domain::Status, stops::local_instant, store::Store};

async fn setup() -> (Store, String, String) {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("seed");
    let pickup = (Utc::now() - Duration::days(2))
        .format("%Y-%m-%d")
        .to_string();
    let delivery = (Utc::now() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let result=store.mutate("demo","create","load.create","",json!({
        "reference":"FR-GATES","customer_id":"c1","mode":"FTL","equipment":"Flatbed","commodity":"Steel","weight_lbs":20000,"miles":400,"buy_cents":120000,"sell_cents":150000,
        "origin":{"facility":"Origin dock","city":"Portland","state":"OR","date":pickup,"address_lines":["15 Dock Street","Building 2"],"postal_code":"97201","country":"US","contact":"Shipping desk","phone":"+1 503 555 0100","instructions":"Use gate 2"},
        "destination":{"facility":"Delivery dock","city":"Seattle","state":"WA","date":delivery,"address_lines":["25 Receiving Ave"],"postal_code":"98101","country":"US"}
    })).await.expect("plan");
    (
        store,
        result["data"]["id"].as_str().expect("id").into(),
        pickup,
    )
}
async fn command(store: &Store, id: &str, action: &str, mut body: Value) {
    let w = store.read("demo").await.expect("workspace");
    let load = w.loads.iter().find(|v| v.id == id).expect("load");
    body["action"] = json!(action);
    body["expected_version"] = json!(load.version);
    store
        .mutate(
            "demo",
            &format!("{action}-{}", load.version),
            "load.command",
            id,
            body,
        )
        .await
        .expect(action);
}
async fn operation(store: &Store, id: &str, op: &str, entity: &str, mut body: Value) -> Value {
    let w = store.read("demo").await.expect("workspace");
    let load = w.loads.iter().find(|v| v.id == id).expect("load");
    body["expected_load_version"] = json!(load.version);
    store
        .mutate_authorized(
            ("demo", "user:dispatcher"),
            &format!("{op}-{}", load.version),
            op,
            entity,
            body,
        )
        .await
        .expect(op)["data"]
        .clone()
}
fn appointment(stop: &str, date: &str, start: &str, end: &str) -> Value {
    json!({"stop":stop,"start_local":format!("{date}T{start}"),"end_local":format!("{date}T{end}"),"timezone":"UTC","contact":"Dock scheduler","confirmation_reference":"CONF-100","note":"Window confirmed directly with facility"})
}
fn gate(stop: &str, kind: &str, date: &str, time: &str) -> Value {
    json!({"stop":stop,"kind":kind,"local_time":format!("{date}T{time}"),"timezone":"UTC","reference":"Gate ledger row 15","note":"Confirmed against facility gate ledger"})
}
async fn fails_unchanged(
    store: &Store,
    key: &str,
    op: &str,
    entity: &str,
    body: Value,
    contains: &str,
) {
    let before = json!(store.read("demo").await.expect("before"));
    let error = store
        .mutate("demo", key, op, entity, body)
        .await
        .expect_err("must reject");
    assert!(error.contains(contains), "{error}");
    assert_eq!(json!(store.read("demo").await.expect("after")), before);
}
#[test]
fn zones_resolve_dst_boundaries_without_guessing_ambiguous_or_nonexistent_local_times() {
    assert_eq!(
        local_instant("2026-10-15T09:00", "America/Los_Angeles", "reject")
            .expect("PDT")
            .to_rfc3339(),
        "2026-10-15T16:00:00+00:00"
    );
    assert!(
        local_instant("2026-03-08T02:30", "America/Los_Angeles", "earlier")
            .expect_err("gap")
            .contains("does not exist")
    );
    assert!(
        local_instant("2026-11-01T01:30", "America/Los_Angeles", "reject")
            .expect_err("ambiguous")
            .contains("twice")
    );
    assert_eq!(
        local_instant("2026-11-01T01:30", "America/Los_Angeles", "earlier")
            .expect("first")
            .to_rfc3339(),
        "2026-11-01T08:30:00+00:00"
    );
    assert_eq!(
        local_instant("2026-11-01T01:30", "America/Los_Angeles", "later")
            .expect("second")
            .to_rfc3339(),
        "2026-11-01T09:30:00+00:00"
    );
    assert!(local_instant("2026-10-15T09:00", "PST8NONSENSE", "reject").is_err());
    assert!(local_instant("2026-10-15T09:00Z", "UTC", "reject").is_err());
    assert!(local_instant("2026-10-15T09:00", "UTC", "guess").is_err());
}
#[tokio::test]
async fn appointments_and_gate_evidence_measure_lateness_and_dwell_without_inventing_delivery_or_charges(
) {
    let (store, id, date) = setup().await;
    let appt = operation(
        &store,
        &id,
        "stop.appointment.book",
        &id,
        appointment("pickup", &date, "08:00", "09:00"),
    )
    .await;
    assert_eq!(appt["recorded_by"], "user:dispatcher");
    command(&store, &id, "tender", json!({"carrier_id":"ca3"})).await;
    command(&store, &id, "accept", json!({})).await;
    operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("pickup", "arrival", &date, "09:10"),
    )
    .await;
    let before = store.read("demo").await.expect("workspace");
    let load = before.loads.iter().find(|v| v.id == id).expect("load");
    assert_eq!(load.status, Status::Accepted);
    let m = before.stop_execution(load, Utc::now());
    assert_eq!(m["pickup"]["late_minutes"], 10);
    assert_eq!(m["pickup"]["arrival_status"], "late");
    assert_eq!(m["pickup"]["arrival"]["appointment_id"], appt["id"]);
    let mut departure = gate("pickup", "departure", &date, "09:30");
    departure["expected_load_version"] = json!(load.version);
    fails_unchanged(
        &store,
        "departure-before-custody",
        "stop.event.record",
        &id,
        departure,
        "Reconcile carrier acceptance",
    )
    .await;
    command(
        &store,
        &id,
        "dispatch",
        json!({"driver_id":"d3","equipment_id":"e3"}),
    )
    .await;
    command(&store, &id, "pickup", json!({})).await;
    let missing = store.read("demo").await.expect("workspace");
    let l = missing.loads.iter().find(|v| v.id == id).expect("load");
    assert!(
        missing.stop_execution(l, Utc::now())["pickup"]["dwell_minutes"].is_null(),
        "a missing departure is not an endless dwell bill"
    );
    operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("pickup", "departure", &date, "09:30"),
    )
    .await;
    let delivery = (Utc::now() - Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    operation(
        &store,
        &id,
        "stop.appointment.book",
        &id,
        appointment("delivery", &delivery, "11:00", "12:00"),
    )
    .await;
    let arrival = operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("delivery", "arrival", &delivery, "12:15"),
    )
    .await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    assert_eq!(l.status, Status::InTransit);
    assert!(l.pod_reference.is_none());
    assert_eq!(
        w.stop_execution(l, Utc::now())["pickup"]["dwell_minutes"],
        20
    );
    command(
        &store,
        &id,
        "deliver",
        json!({"pod_reference":"external-pod-GATES"}),
    )
    .await;
    operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("delivery", "departure", &delivery, "12:45"),
    )
    .await;
    let correction=operation(&store,&id,"stop.event.correct",arrival["id"].as_str().expect("id"),json!({"local_time":format!("{delivery}T12:05"),"timezone":"UTC","reference":"Corrected gate ledger row 15","note":"Facility corrected arrival clock by 10 minutes"})).await;
    assert_eq!(correction["supersedes_id"], arrival["id"]);
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    let m = w.stop_execution(l, Utc::now());
    assert_eq!(m["delivery"]["late_minutes"], 5);
    assert_eq!(m["delivery"]["dwell_minutes"], 40);
    assert_eq!(m["delivery"]["dwell_complete"], true);
    fails_unchanged(&store,"bad-chronology-correction","stop.event.correct",correction["id"].as_str().expect("correction id"),json!({"expected_load_version":l.version,"local_time":format!("{date}T09:00"),"timezone":"UTC","reference":"Bad correction","note":"Would put delivery arrival before pickup departure"}),"Gate times must follow").await;
    assert!(!w.delivery_evidence_ready(l));
    assert_eq!(w.stop_events.len(), 5);
    assert_eq!(
        json!(w
            .stop_events
            .iter()
            .find(|v| Some(v.id.as_str()) == arrival["id"].as_str())
            .expect("original")),
        arrival
    );
    assert!(
        !w.invoices.iter().any(|v| v.load_id == id) && w.bills.is_empty() && w.journal.is_empty()
    );
}
#[tokio::test]
async fn booking_history_and_arrival_baseline_cannot_be_rewritten_by_replanning_or_stale_requests()
{
    let (store, id, date) = setup().await;
    let first = operation(
        &store,
        &id,
        "stop.appointment.book",
        &id,
        appointment("pickup", &date, "08:00", "09:00"),
    )
    .await;
    let mut next = appointment("pickup", &date, "09:00", "10:00");
    next["confirmation_reference"] = json!("CONF-101");
    let second = operation(&store, &id, "stop.appointment.book", &id, next).await;
    assert_eq!(second["replaces_id"], first["id"]);
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    assert_eq!(w.appointments[0].status, "superseded");
    assert_eq!(w.stop_execution(l, Utc::now())["pickup"]["overdue"], true);
    let mut stale = appointment("pickup", &date, "10:00", "11:00");
    stale["expected_load_version"] = json!(1);
    fails_unchanged(
        &store,
        "stale-booking",
        "stop.appointment.book",
        &id,
        stale,
        "Load changed;",
    )
    .await;
    let mut plan = json!(l);
    let fields = [
        "reference",
        "customer_id",
        "mode",
        "equipment",
        "commodity",
        "weight_lbs",
        "miles",
        "origin",
        "destination",
        "buy_cents",
        "sell_cents",
    ];
    plan.as_object_mut()
        .expect("plan")
        .retain(|key, _| fields.contains(&key.as_str()));
    plan["origin"]["facility"] = json!("Changed dock");
    fails_unchanged(
        &store,
        "bad-replan",
        "load.revise",
        &id,
        json!({"expected_version":l.version,"plan":plan}),
        "Cancel the booked appointment",
    )
    .await;
    command(&store, &id, "tender", json!({"carrier_id":"ca3"})).await;
    command(&store, &id, "accept", json!({})).await;
    let event = operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("pickup", "arrival", &date, "09:30"),
    )
    .await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    let mut body = appointment("pickup", &date, "09:30", "10:30");
    body["expected_load_version"] = json!(l.version);
    fails_unchanged(
        &store,
        "rewrite-baseline",
        "stop.appointment.book",
        &id,
        body,
        "Arrival already recorded",
    )
    .await;
    fails_unchanged(
        &store,
        "cancel-baseline",
        "stop.appointment.cancel",
        second["id"].as_str().expect("id"),
        json!({"expected_load_version":l.version,"reason":"Try to hide lateness"}),
        "Arrival already recorded",
    )
    .await;
    let mut duplicate = gate("pickup", "arrival", &date, "09:35");
    duplicate["expected_load_version"] = json!(l.version);
    fails_unchanged(
        &store,
        "duplicate-gate",
        "stop.event.record",
        &id,
        duplicate,
        "already recorded",
    )
    .await;
    let mut future = gate("delivery", "arrival", "2099-01-01", "09:00");
    future["expected_load_version"] = json!(l.version);
    fails_unchanged(
        &store,
        "premature-delivery",
        "stop.event.record",
        &id,
        future,
        "Reconcile",
    )
    .await;
    assert_eq!(event["recorded_by"], "user:dispatcher");
    let mut future = gate("pickup", "departure", "2099-01-01", "09:00");
    command(
        &store,
        &id,
        "dispatch",
        json!({"driver_id":"d3","equipment_id":"e3"}),
    )
    .await;
    command(&store, &id, "pickup", json!({})).await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    future["expected_load_version"] = json!(l.version);
    fails_unchanged(
        &store,
        "future-gate-time",
        "stop.event.record",
        &id,
        future,
        "cannot be in the future",
    )
    .await;
}
#[tokio::test]
async fn retired_carrier_gate_evidence_stays_historical_and_sync_gets_only_current_operational_times(
) {
    let (store, id, date) = setup().await;
    command(&store, &id, "tender", json!({"carrier_id":"ca3"})).await;
    command(&store, &id, "accept", json!({})).await;
    let event = operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("pickup", "arrival", &date, "09:00"),
    )
    .await;
    let release = operation(
        &store,
        &id,
        "tender.release.request",
        &id,
        json!({"disposition":"replan","reason":"Carrier at gate confirms it has no cargo"}),
    )
    .await;
    operation(&store,&id,"tender.release.confirm",release["id"].as_str().expect("release"),json!({"carrier_contact":"Dispatcher and gate","confirmation_reference":"No-custody-105","note":"Carrier left empty and released the assignment"})).await;
    command(&store, &id, "tender", json!({"carrier_id":"ca3"})).await;
    command(&store, &id, "accept", json!({})).await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    assert!(w.stop_execution(l, Utc::now())["pickup"]["arrival"].is_null());
    assert!(w.stop_tracking(l)["pickup"]["arrival_at"].is_null());
    assert_eq!(json!(w.stop_events[0]), event);
    operation(
        &store,
        &id,
        "stop.event.record",
        &id,
        gate("pickup", "arrival", &date, "10:00"),
    )
    .await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    let tracking = w.stop_tracking(l);
    assert!(tracking["pickup"]["arrival_at"].is_string());
    assert!(
        !tracking.to_string().contains("Gate ledger")
            && !tracking.to_string().contains("user:dispatcher")
    );
}
#[tokio::test]
async fn stop_validation_rejects_unusable_wire_addresses_and_legacy_plans_need_addresses_before_tender(
) {
    let (store, id, _) = setup().await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    let mut invalid = l.origin.clone();
    invalid.details.address_lines = vec!["25 Street~B2*INJECT".into()];
    assert!(stateset_freight::stops::validate_stop(&invalid, true).is_err());
    invalid.details.address_lines = vec![];
    assert!(stateset_freight::stops::validate_stop(&invalid, true).is_err());
    assert!(
        stateset_freight::stops::validate_stop(&invalid, false).is_ok(),
        "draft can be completed later"
    );
    let mut legacy = json!(l.origin);
    legacy
        .as_object_mut()
        .expect("stop")
        .remove("address_lines");
    legacy.as_object_mut().expect("stop").remove("postal_code");
    let legacy: stateset_freight::domain::Stop =
        serde_json::from_value(legacy).expect("read old stop");
    assert!(legacy.details.address_lines.is_empty());
    let mut plan = json!(l);
    let fields = [
        "reference",
        "customer_id",
        "mode",
        "equipment",
        "commodity",
        "weight_lbs",
        "miles",
        "origin",
        "destination",
        "buy_cents",
        "sell_cents",
    ];
    plan.as_object_mut()
        .expect("plan")
        .retain(|key, _| fields.contains(&key.as_str()));
    plan["origin"] = json!(legacy);
    store
        .mutate(
            "demo",
            "legacy-plan",
            "load.revise",
            &id,
            json!({"expected_version":l.version,"plan":plan}),
        )
        .await
        .expect("legacy draft remains readable/editable");
    fails_unchanged(
        &store,
        "legacy-tender",
        "load.command",
        &id,
        json!({"action":"tender","carrier_id":"ca3","expected_version":2}),
        "street address",
    )
    .await;
}
#[tokio::test]
async fn overnight_appointments_extend_carrier_and_driver_qualification_through_the_final_local_day(
) {
    let (store, id, _) = setup().await;
    let today = Utc::now().date_naive();
    let tomorrow = today + Duration::days(1);
    let yesterday = (today - Duration::days(1)).to_string();
    let mut a = appointment("delivery", &yesterday, "23:00", "23:30");
    a["end_local"] = json!(format!("{tomorrow}T00:00"));
    operation(&store, &id, "stop.appointment.book", &id, a).await;
    let w = store.read("demo").await.expect("workspace");
    let original = json!(w.carriers.iter().find(|v| v.id == "ca3").expect("carrier"));
    let mut data = original.clone();
    data["insurance_expires"] = json!(today);
    store
        .mutate(
            "demo",
            "policy-current-day",
            "carrier.update",
            "ca3",
            json!({"expected":original,"data":data}),
        )
        .await
        .expect("record policy date");
    fails_unchanged(
        &store,
        "uncovered-tender",
        "load.command",
        &id,
        json!({"action":"tender","carrier_id":"ca3","expected_version":2}),
        "appointment dates",
    )
    .await;
    let w = store.read("demo").await.expect("workspace");
    let original = json!(w.carriers.iter().find(|v| v.id == "ca3").expect("carrier"));
    let mut data = original.clone();
    data["insurance_expires"] = json!(tomorrow);
    store
        .mutate(
            "demo",
            "policy-through-window",
            "carrier.update",
            "ca3",
            json!({"expected":original,"data":data}),
        )
        .await
        .expect("renew policy");
    command(&store, &id, "tender", json!({"carrier_id":"ca3"})).await;
    command(&store, &id, "accept", json!({})).await;
    let w = store.read("demo").await.expect("workspace");
    let original = json!(w.drivers.iter().find(|v| v.id == "d3").expect("driver"));
    let mut data = original.clone();
    data["license_expires"] = json!(today);
    store
        .mutate(
            "demo",
            "license-current-day",
            "driver.update",
            "d3",
            json!({"expected":original,"data":data}),
        )
        .await
        .expect("record license");
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    fails_unchanged(&store,"uncovered-dispatch","load.command",&id,json!({"action":"dispatch","driver_id":"d3","equipment_id":"e3","expected_version":l.version}),"appointment dates").await;
    let original = json!(w.drivers.iter().find(|v| v.id == "d3").expect("driver"));
    let mut data = original.clone();
    data["license_expires"] = json!(tomorrow);
    store
        .mutate(
            "demo",
            "license-through-window",
            "driver.update",
            "d3",
            json!({"expected":original,"data":data}),
        )
        .await
        .expect("renew license");
    command(
        &store,
        &id,
        "dispatch",
        json!({"driver_id":"d3","equipment_id":"e3"}),
    )
    .await;
    let w = store.read("demo").await.expect("workspace");
    let l = w.loads.iter().find(|v| v.id == id).expect("load");
    assert_eq!(w.coverage_date(l), tomorrow);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_appointment_writers_serialize_and_retain_proof_across_reconnect() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    let tenant = format!("stop-{}", uuid::Uuid::new_v4());
    let one = Store::new(Some(&url)).await.expect("one");
    let two = Store::new(Some(&url)).await.expect("two");
    let customer = one
        .mutate(
            &tenant,
            "customer",
            "customer.create",
            "",
            json!({"name":"Appointments test","contact":"ops"}),
        )
        .await
        .expect("customer");
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let plan = json!({"reference":"PG-STOPS","customer_id":customer["data"]["id"],"mode":"FTL","equipment":"Dry van","commodity":"Fixtures","weight_lbs":1,"miles":1,"buy_cents":1,"sell_cents":2,
        "origin":{"facility":"Dock A","city":"Portland","state":"OR","date":date},"destination":{"facility":"Dock B","city":"Seattle","state":"WA","date":date}});
    let load = one
        .mutate(&tenant, "plan", "load.create", "", plan)
        .await
        .expect("load");
    let id = load["data"]["id"].as_str().expect("id");
    let mut a = appointment("pickup", &date, "08:00", "09:00");
    a["expected_load_version"] = json!(1);
    let mut b = appointment("pickup", &date, "10:00", "11:00");
    b["expected_load_version"] = json!(1);
    let (x, y) = tokio::join!(
        one.mutate_authorized(
            (&tenant, "user:first"),
            "first",
            "stop.appointment.book",
            id,
            a
        ),
        two.mutate_authorized(
            (&tenant, "user:second"),
            "second",
            "stop.appointment.book",
            id,
            b
        )
    );
    assert_ne!(x.is_ok(), y.is_ok());
    drop(one);
    drop(two);
    let reopened = Store::new(Some(&url)).await.expect("reopen");
    let w = reopened.read(&tenant).await.expect("workspace");
    assert_eq!(w.appointments.len(), 1);
    assert_eq!(w.loads[0].version, 2);
    assert!(w.appointments[0].recorded_by.starts_with("user:"));
    assert_eq!(w.appointments[0].confirmation_reference, "CONF-100");
    assert_eq!(w.receipts.len(), 3);
    assert!(reopened
        .read("different-appointment-tenant")
        .await
        .expect("isolated")
        .appointments
        .is_empty());
}
