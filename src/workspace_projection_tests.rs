//! Compare the actual staff workspace projection with the original single-load
//! selectors. Closed observations avoid time-dependent live dwell assertions.
use super::{workspace_view, Tenant};
use crate::{
    auth::{Principal, Role},
    domain::{self, Status, Workspace},
    stops::{Appointment, StopEvent},
};
use chrono::Utc;
use serde_json::{json, Value};

fn fixture() -> Workspace {
    let template = domain::demo().loads.remove(0);
    let mut w = Workspace::default();
    for id in ["a", "b", "unassigned"] {
        let mut load = template.clone();
        load.id = id.into();
        load.reference = id.into();
        load.tender_id = (id != "unassigned").then(|| "shared_reference".into());
        load.status = Status::InTransit;
        load.exception = None;
        w.loads.push(load);
    }
    let gate = |load: &str,
                id: &str,
                tender: &str,
                stop: &str,
                kind: &str,
                at: &str,
                replaces: Option<&str>|
     -> StopEvent {
        serde_json::from_value(json!({"id":id,"load_id":load,"tender_id":tender,"stop":stop,"kind":kind,"appointment_id":null,
            "occurred_at":at,"local_time":"2010-01-02T10:00","timezone":"UTC","disambiguation":"reject",
            "reference":id,"note":"Original owned synthetic ledger","recorded_at":at,"recorded_by":"test:facility","supersedes_id":replaces})).expect("gate")
    };
    w.stop_events = vec![
        gate(
            "a",
            "a_original",
            "shared_reference",
            "pickup",
            "arrival",
            "2010-01-01T09:00:00Z",
            None,
        ),
        gate(
            "a",
            "a_corrected",
            "shared_reference",
            "pickup",
            "arrival",
            "2010-01-01T10:00:00Z",
            Some("a_original"),
        ),
        gate(
            "a",
            "a_departure",
            "shared_reference",
            "pickup",
            "departure",
            "2010-01-01T11:00:00Z",
            None,
        ),
        gate(
            "a",
            "a_arrival",
            "shared_reference",
            "delivery",
            "arrival",
            "2010-01-02T10:00:00Z",
            None,
        ),
        gate(
            "a",
            "a_del_departure",
            "shared_reference",
            "delivery",
            "departure",
            "2010-01-02T11:00:00Z",
            None,
        ),
        gate(
            "b",
            "b_arrival",
            "shared_reference",
            "delivery",
            "arrival",
            "2010-01-02T10:00:00Z",
            None,
        ),
        gate(
            "b",
            "b_departure",
            "shared_reference",
            "delivery",
            "departure",
            "2010-01-02T11:00:00Z",
            None,
        ),
        gate(
            "a",
            "a_prior_arrival",
            "prior",
            "delivery",
            "arrival",
            "2009-01-02T10:00:00Z",
            None,
        ),
        gate(
            "a",
            "a_prior_departure",
            "prior",
            "delivery",
            "departure",
            "2009-01-02T11:00:00Z",
            None,
        ),
        gate(
            "unassigned",
            "unassigned_old",
            "shared_reference",
            "delivery",
            "arrival",
            "2009-01-02T10:00:00Z",
            None,
        ),
    ];
    let window = |load: &str, id: &str, status: &str, year: &str| -> Appointment {
        serde_json::from_value(json!({"id":id,"load_id":load,"stop":"delivery","status":status,
            "starts_at":format!("{year}-01-02T09:00:00Z"),"ends_at":format!("{year}-01-02T10:00:00Z"),"timezone":"UTC",
            "start_local":format!("{year}-01-02T09:00"),"end_local":format!("{year}-01-02T10:00"),"disambiguation":"reject",
            "contact":"Owned facility","confirmation_reference":id,"note":"Original appointment",
            "recorded_at":"2009-01-01T00:00:00Z","recorded_by":"test:facility","replaces_id":null,"cancellation":null})).expect("window")
    };
    w.appointments = vec![
        window("a", "a_cancelled", "cancelled", "2010"),
        window("b", "b_first", "active", "2010"),
        window("a", "a_first", "active", "2010"),
        // Deliberately duplicate active source rows: preserve the old first-match
        // rule rather than silently preferring the last or latest date.
        window("a", "a_second", "active", "2090"),
        window("unassigned", "unassigned_first", "active", "2010"),
    ];
    w
}
fn view(w: &Workspace) -> Value {
    workspace_view(
        w.clone(),
        Tenant(
            "owned".into(),
            Principal {
                tenant_id: "owned".into(),
                subject: "test:operator".into(),
                role: Role::Operations,
                counterparty_id: None,
            },
        ),
        false,
        false,
        json!({}),
    )
    .expect("workspace projection")
}
fn load<'a>(response: &'a Value, id: &str) -> &'a Value {
    response["loads"]
        .as_array()
        .expect("loads")
        .iter()
        .find(|l| l["id"] == id)
        .expect("load")
}
#[test]
fn workspace_keeps_original_selector_order_tender_scope_and_retained_rows() {
    let w = fixture();
    let response = view(&w);
    assert_eq!(response["stop_events"], json!(w.stop_events));
    assert_eq!(response["appointments"], json!(w.appointments));
    for original in &w.loads {
        let projected = load(&response, &original.id);
        assert_eq!(
            projected["stop_execution"],
            w.stop_execution(original, Utc::now())
        );
        assert_eq!(
            projected["execution_timeline"],
            w.execution_timeline(original, true)
        );
    }
    assert_eq!(
        load(&response, "a")["stop_execution"]["pickup"]["arrival"]["id"],
        "a_corrected"
    );
    assert_eq!(
        load(&response, "a")["stop_execution"]["delivery"]["appointment"]["id"],
        "a_first"
    );
    assert_eq!(
        load(&response, "b")["stop_execution"]["delivery"]["arrival"]["id"],
        "b_arrival"
    );
    assert!(load(&response, "unassigned")["stop_execution"]["delivery"]["arrival"].is_null());
    assert_eq!(
        load(&response, "unassigned")["stop_execution"]["has_overdue"],
        true
    );
}
#[test]
fn workspace_rebuilds_from_new_gate_appointment_and_tender_snapshots() {
    let mut w = fixture();
    let first = view(&w);
    let mut correction = w
        .stop_events
        .iter()
        .find(|g| g.id == "a_arrival")
        .expect("arrival")
        .clone();
    correction.id = "a_arrival_revised".into();
    correction.supersedes_id = Some("a_arrival".into());
    w.stop_events.push(correction);
    w.appointments
        .iter_mut()
        .find(|a| a.id == "a_first")
        .expect("window")
        .status = "cancelled".into();
    let second = view(&w);
    assert_eq!(
        load(&second, "a")["stop_execution"]["delivery"]["arrival"]["id"],
        "a_arrival_revised"
    );
    assert_eq!(
        load(&second, "a")["stop_execution"]["delivery"]["appointment"]["id"],
        "a_second"
    );
    assert_eq!(
        load(&first, "a")["stop_execution"]["delivery"]["arrival"]["id"],
        "a_arrival"
    );
    assert_eq!(load(&first, "b"), load(&second, "b"));
    w.loads[0].tender_id = Some("prior".into());
    assert_eq!(
        load(&view(&w), "a")["stop_execution"]["delivery"]["arrival"]["id"],
        "a_prior_arrival"
    );
    w.loads[0].tender_id = None;
    assert!(load(&view(&w), "a")["stop_execution"]["delivery"]["arrival"].is_null());
    assert_eq!(
        load(&view(&w), "unassigned")["stop_execution"]["has_overdue"],
        true
    );
    w.loads[2].status = Status::Cancelled;
    assert_eq!(
        load(&view(&w), "unassigned")["stop_execution"]["has_overdue"],
        false
    );
}
#[test]
fn workspace_retains_chronology_and_unverifiable_movement_holds() {
    let mut w = fixture();
    w.stop_events
        .iter_mut()
        .find(|g| g.id == "a_corrected")
        .expect("current gate")
        .occurred_at = "2010-01-03T10:00:00Z".parse().expect("time");
    let conflicting = view(&w);
    let projected = load(&conflicting, "a");
    assert_eq!(projected["execution_timeline"]["evidence_valid"], true);
    assert_eq!(projected["execution_timeline"]["consistent"], false);
    assert_eq!(
        projected["route_attention"],
        "Execution times conflict; review original gate, service, and carrier evidence"
    );
    w.movement_corrections.push(serde_json::from_value(json!({"id":"damaged","load_id":"a","tender_id":"shared_reference","carrier_id":"ca1",
        "source_id":"missing-original","source_sha256":"invalid","supersedes_id":null,"observed_at":"2010-01-01T10:00:00Z",
        "contact":"Original contact","reference":"Original receipt","note":"Retained damaged proof","recorded_at":"2010-01-01T10:01:00Z",
        "recorded_by":"test:operator","evidence_sha256":"invalid"})).expect("retained correction"));
    let damaged = view(&w);
    assert_eq!(
        load(&damaged, "a")["route_attention"],
        "Movement correction evidence cannot be verified; review original records"
    );
    assert_eq!(
        load(&damaged, "a")["execution_timeline"]["evidence_valid"],
        false
    );
    assert_eq!(
        load(&damaged, "b")["execution_timeline"]["consistent"],
        true
    );
    assert_eq!(
        damaged["movement_corrections"],
        json!(w.movement_corrections)
    );
}
