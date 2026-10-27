//! Gate history must remain scoped to its original load and tender while the
//! control tower discovers elapsed appointments and execution conflicts.
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use stateset_freight::{
    domain::{self, Status, Workspace},
    stops::{Appointment, StopEvent},
};
fn fixture() -> (Workspace, DateTime<Utc>) {
    let now = "2026-10-16T12:00:00Z"
        .parse::<DateTime<Utc>>()
        .expect("clock");
    let base = domain::demo().loads.remove(0);
    let mut w = Workspace::default();
    for name in ["owned_a", "owned_b"] {
        let mut load = base.clone();
        load.id = name.into();
        load.reference = name.into();
        load.status = Status::InTransit;
        load.tender_id = Some("owned_shared_reference".into());
        load.exception = None;
        load.last_tracking_at = Some(now);
        w.loads.push(load);
        let appointment: Appointment = serde_json::from_value(json!({
            "id":format!("appointment_{name}"),"load_id":name,"stop":"delivery","status":"active",
            "starts_at":now-Duration::hours(3),"ends_at":now-Duration::hours(1),"timezone":"UTC",
            "start_local":"2026-10-16T09:00","end_local":"2026-10-16T11:00","disambiguation":"reject",
            "contact":"Owned facility contact","confirmation_reference":"Owned original window",
            "note":"Original synthetic window; actual arrival is separate","recorded_at":now-Duration::hours(4),
            "recorded_by":"user:owned-dispatcher","replaces_id":null,"cancellation":null
        })).expect("appointment");
        w.appointments.push(appointment);
    }
    (w, now)
}
fn gate(
    load: &str,
    id: &str,
    tender: &str,
    stop: &str,
    kind: &str,
    at: DateTime<Utc>,
    supersedes: Option<&str>,
) -> StopEvent {
    serde_json::from_value(json!({
        "id":id,"load_id":load,"tender_id":tender,"appointment_id":null,"stop":stop,"kind":kind,
        "occurred_at":at,"local_time":at.format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC",
        "disambiguation":"reject","reference":format!("Owned gate receipt {id}"),"note":"Original retained gate evidence",
        "recorded_at":at+Duration::minutes(1),"recorded_by":"user:owned-dispatcher","supersedes_id":supersedes
    })).expect("gate")
}
fn arrivals(w: &Workspace, now: DateTime<Utc>) -> Vec<String> {
    let before = json!(w);
    let mut ids = w
        .operations_signals(now)
        .into_iter()
        .filter(|s| s.kind == "arrival_unrecorded")
        .map(|s| s.load_id.expect("load"))
        .collect::<Vec<_>>();
    ids.sort();
    assert_eq!(json!(w), before, "source reads must not mutate originals");
    ids
}
#[test]
fn elapsed_windows_use_current_arrivals_for_the_exact_load_and_tender_only() {
    let (mut w, now) = fixture();
    let tender = "owned_shared_reference";
    // The same tender reference deliberately appears on another load. Neither
    // that arrival nor an older carrier's arrival can complete owned_a's gate.
    w.stop_events.extend([
        gate(
            "owned_b",
            "b_arrived",
            tender,
            "delivery",
            "arrival",
            now - Duration::hours(2),
            None,
        ),
        gate(
            "owned_a",
            "a_old_carrier",
            "owned_prior_tender",
            "delivery",
            "arrival",
            now - Duration::hours(2),
            None,
        ),
        gate(
            "owned_a",
            "a_original",
            tender,
            "pickup",
            "arrival",
            now - Duration::hours(4),
            None,
        ),
        gate(
            "owned_a",
            "a_corrected",
            tender,
            "pickup",
            "arrival",
            now - Duration::minutes(230),
            Some("a_original"),
        ),
        gate(
            "owned_a",
            "a_departure",
            tender,
            "pickup",
            "departure",
            now - Duration::hours(3),
            None,
        ),
    ]);
    let mut cancelled = w.appointments[0].clone();
    cancelled.id = "owned_cancelled_window".into();
    cancelled.status = "cancelled".into();
    w.appointments.push(cancelled);
    assert_eq!(arrivals(&w, now), vec!["owned_a"]);
    w.loads[1].tender_id = None;
    assert_eq!(arrivals(&w, now), vec!["owned_a", "owned_b"]);
    w.loads[1].tender_id = Some(tender.into());
    let selected = w
        .current_stop_events(&w.loads[0])
        .iter()
        .map(|s| s.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec!["a_corrected", "a_departure"]);
    w.stop_events.push(gate(
        "owned_a",
        "a_arrived",
        tender,
        "delivery",
        "arrival",
        now - Duration::hours(2),
        None,
    ));
    assert!(arrivals(&w, now).is_empty());
}
#[test]
fn execution_conflicts_keep_corrected_current_gate_identity_and_exclude_historical_records() {
    let (mut w, now) = fixture();
    let tender = "owned_shared_reference";
    w.stop_events.extend([
        gate(
            "owned_a",
            "a_original",
            tender,
            "pickup",
            "arrival",
            now - Duration::hours(4),
            None,
        ),
        gate(
            "owned_a",
            "a_corrected",
            tender,
            "pickup",
            "arrival",
            now - Duration::hours(1),
            Some("a_original"),
        ),
        gate(
            "owned_a",
            "a_departure",
            tender,
            "pickup",
            "departure",
            now - Duration::hours(3),
            None,
        ),
        gate(
            "owned_a",
            "old_delivery",
            "owned_prior_tender",
            "delivery",
            "arrival",
            now - Duration::hours(5),
            None,
        ),
        gate(
            "owned_b",
            "other_load",
            tender,
            "delivery",
            "arrival",
            now - Duration::hours(5),
            None,
        ),
    ]);
    let before = json!(w);
    let source = w
        .operations_signals(now)
        .into_iter()
        .find(|s| s.kind == "execution_conflict" && s.load_id.as_deref() == Some("owned_a"))
        .expect("conflict")
        .source;
    let conflicts = source["conflicts"].as_array().expect("conflicts");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["before"]["id"], "a_corrected");
    assert_eq!(conflicts[0]["after"]["id"], "a_departure");
    assert_eq!(source["record_count"], 2);
    assert_eq!(source, w.execution_timeline(&w.loads[0], false));
    assert_eq!(json!(w), before);
}
#[test]
fn each_source_projection_rebuilds_from_its_own_clock_and_snapshot() {
    let (mut w, now) = fixture();
    w.appointments[0].ends_at = now + Duration::minutes(1);
    w.appointments[1].status = "cancelled".into();
    assert!(arrivals(&w, now).is_empty());
    assert_eq!(arrivals(&w, now + Duration::minutes(2)), vec!["owned_a"]);
    w.stop_events.push(gate(
        "owned_a",
        "owned_arrival",
        "owned_shared_reference",
        "delivery",
        "arrival",
        now,
        None,
    ));
    assert!(arrivals(&w, now + Duration::minutes(2)).is_empty());
    w.loads[0].tender_id = Some("owned_replacement_tender".into());
    assert_eq!(arrivals(&w, now + Duration::minutes(2)), vec!["owned_a"]);
    w.loads[0].tender_id = Some("owned_shared_reference".into());
    assert!(arrivals(&w, now + Duration::minutes(2)).is_empty());
    w.stop_events.clear();
    assert_eq!(arrivals(&w, now + Duration::minutes(2)), vec!["owned_a"]);
    w.appointments[0].status = "cancelled".into();
    assert!(arrivals(&w, now + Duration::minutes(2)).is_empty());
}
