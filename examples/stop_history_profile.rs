//! Deterministic synthetic projection profile; not a production capacity test.
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use stateset_freight::{
    auth::{Principal, Role},
    documents::sha256,
    domain::{self, Event, Status, Workspace},
    operations::{CaseEvent, OperationsCase, QueueQuery},
};
use std::{collections::BTreeSet, hint::black_box, time::Instant};

fn hash(value: &Value) -> String {
    sha256(value.to_string().as_bytes())
}
fn gate(
    load: &stateset_freight::domain::Load,
    id: &str,
    tender: &str,
    stop: &str,
    kind: &str,
    at: DateTime<Utc>,
    supersedes_id: Option<String>,
) -> stateset_freight::stops::StopEvent {
    stateset_freight::stops::StopEvent {
        id: id.into(),
        load_id: load.id.clone(),
        tender_id: tender.into(),
        appointment_id: None,
        stop: stop.into(),
        kind: kind.into(),
        occurred_at: at,
        local_time: at.format("%Y-%m-%dT%H:%M").to_string(),
        timezone: "UTC".into(),
        disambiguation: "reject".into(),
        reference: format!("Owned gate ledger {id}"),
        note: "Retained actual synthetic gate observation; no custody or service assertion".into(),
        recorded_at: at + Duration::minutes(1),
        recorded_by: "user:dispatcher".into(),
        supersedes_id,
    }
}
fn appointment(
    load: &stateset_freight::domain::Load,
    id: &str,
    stop: &str,
    now: DateTime<Utc>,
) -> stateset_freight::stops::Appointment {
    let plan = if stop == "pickup" {
        &load.origin
    } else {
        &load.destination
    };
    let starts = now - Duration::hours(3);
    let ends = now - Duration::hours(1);
    stateset_freight::stops::Appointment {
        stop_snapshot: Some(plan.clone()),
        id: id.into(),
        load_id: load.id.clone(),
        stop: stop.into(),
        status: "active".into(),
        starts_at: starts,
        ends_at: ends,
        timezone: "UTC".into(),
        start_local: starts.format("%Y-%m-%dT%H:%M").to_string(),
        end_local: ends.format("%Y-%m-%dT%H:%M").to_string(),
        disambiguation: "reject".into(),
        contact: "Owned facility dispatcher".into(),
        confirmation_reference: format!("Owned window {id}"),
        note: "Facility-confirmed synthetic appointment; arrival remains independent".into(),
        recorded_at: starts - Duration::hours(1),
        recorded_by: "user:dispatcher".into(),
        replaces_id: None,
        cancellation: None,
    }
}
fn fixture(count: usize, histories: usize, notes: usize, now: DateTime<Utc>) -> Workspace {
    let template = domain::demo().loads.remove(0);
    let mut w = Workspace::default();
    for i in 0..count {
        let mut l = template.clone();
        l.id = format!("profile_{i:06}");
        l.reference = format!("PROFILE-{i:06}");
        l.tender_id = Some(format!("tender_{i:06}"));
        l.status = Status::InTransit;
        l.created_at = now - Duration::days(2);
        l.updated_at = now - Duration::hours(6);
        l.origin.date = (now - Duration::days(1)).date_naive();
        l.destination.date = now.date_naive();
        l.last_tracking_at = Some(now - Duration::hours(6));
        l.tender_expires_at = None;
        l.exception = (i % 10 == 0).then(|| "Actual facility closure under investigation".into());
        l.pod_document_id = None;
        l.pod_reference = None;
        l.pod_review = None;
        l.sync_order_id = None;
        l.order_number = None;
        l.source_order = None;
        if l.exception.is_some() {
            w.events.push(Event {
                id: format!("event_{i:06}"),
                entity_id: l.id.clone(),
                kind: "load.exception".into(),
                message: "Actual facility closure under investigation".into(),
                actor: "user:dispatcher".into(),
                occurred_at: now - Duration::hours(1),
                details: None,
            });
        }
        // Retain multiple carrier attempts and explicit gate corrections.
        // Historical gates must not satisfy this tender's arrival condition.
        for revision in 0..histories {
            for (phase, minutes) in [("arrival", 0), ("departure", 20)] {
                w.stop_events.push(gate(
                    &l,
                    &format!("old_{i}_{revision}_{phase}"),
                    &format!("old_tender_{i}_{revision}"),
                    "pickup",
                    phase,
                    now - Duration::hours(4) + Duration::minutes(minutes),
                    None,
                ));
            }
            let mut appointment = appointment(
                &l,
                &format!("old_appointment_{i}_{revision}"),
                "delivery",
                now,
            );
            appointment.status = "cancelled".into();
            appointment.cancellation = Some(stateset_freight::stops::AppointmentCancellation {
                reason: "Owned earlier operating window replaced".into(),
                recorded_at: now - Duration::hours(2),
                recorded_by: "user:dispatcher".into(),
            });
            w.appointments.push(appointment);
        }
        for stop in ["pickup", "delivery"] {
            w.appointments.push(appointment(
                &l,
                &format!("appointment_{i}_{stop}"),
                stop,
                now,
            ));
        }
        let tender = l.tender_id.as_deref().expect("tender");
        let mut prior = None;
        for revision in 0..3 {
            let id = format!("current_{i}_pickup_{revision}");
            w.stop_events.push(gate(
                &l,
                &id,
                tender,
                "pickup",
                "arrival",
                now - Duration::hours(4) + Duration::minutes(revision),
                prior,
            ));
            prior = Some(id);
        }
        w.stop_events.push(gate(
            &l,
            &format!("current_{i}_departure"),
            tender,
            "pickup",
            "departure",
            now - Duration::hours(3),
            None,
        ));
        if i % 2 == 0 {
            w.stop_events.push(gate(
                &l,
                &format!("current_{i}_delivery"),
                tender,
                "delivery",
                "arrival",
                now - Duration::minutes(90),
                None,
            ));
        }
        w.loads.push(l);
    }
    let original = w.operations_signals(now);
    for (i, l) in w.loads.iter_mut().enumerate() {
        if i % 5 != 0 {
            l.last_tracking_at = Some(now);
        }
    }
    let active: BTreeSet<_> = w
        .operations_signals(now)
        .into_iter()
        .map(|s| s.id)
        .collect();
    for (i, source) in original.into_iter().enumerate() {
        let closed = !active.contains(&source.id);
        let owned = i % 3 != 0;
        let detected = now - Duration::hours(1);
        let mut history = vec![CaseEvent {
            action: "detected".into(),
            actor: "system:operations".into(),
            note: "Current source condition detected".into(),
            at: detected,
            cycle: 1,
            source: Some(source.clone()),
        }];
        if owned {
            history.push(CaseEvent {
                action: "claim".into(),
                actor: "user:dispatcher".into(),
                note: "Actual facility contact will review the original arrival evidence".into(),
                at: detected + Duration::minutes(5),
                cycle: 1,
                source: None,
            });
        }
        for n in 0..notes {
            history.push(CaseEvent {
                action: "note".into(),
                actor: "user:dispatcher".into(),
                note: format!("Retained synthetic follow-up {n}: facility and carrier contacted; original evidence remains under review"),
                at: detected + Duration::seconds(400 + n as i64),
                cycle: 1,
                source: None,
            });
        }
        if closed {
            history.push(CaseEvent {
                action: "source_cleared".into(),
                actor: "system:operations".into(),
                note: "The original source condition no longer applies".into(),
                at: now - Duration::minutes(5),
                cycle: 1,
                source: None,
            });
        }
        let mut c = OperationsCase {
            id: source.id.clone(),
            version: history.len() as u64,
            cycle: 1,
            status: if closed {
                "resolved"
            } else if owned {
                "working"
            } else {
                "open"
            }
            .into(),
            source_sha256: hash(&json!(source)),
            source,
            owner: owned.then(|| "user:dispatcher".into()),
            follow_up_at: owned.then_some(now + Duration::hours(1)),
            deferred_until: (!closed && owned && i % 2 == 0).then_some(now + Duration::minutes(30)),
            first_detected_at: detected,
            updated_at: history.last().expect("history").at,
            resolved_at: closed.then_some(now - Duration::minutes(5)),
            first_claimed_at: owned.then_some(detected + Duration::minutes(5)),
            history,
            evidence_sha256: String::new(),
        };
        let mut value = json!(c);
        value
            .as_object_mut()
            .expect("case")
            .remove("evidence_sha256");
        c.evidence_sha256 = hash(&value);
        w.operations_cases.push(c);
    }
    w.verify_operations_cases().expect("fixture integrity");
    assert!(!w.operations_need_reconciliation(now));
    w
}
fn measure(mut f: impl FnMut() -> Value, iterations: usize) -> Value {
    let expected = f();
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        let value = black_box(f());
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(
            value, expected,
            "projection changed during fixed-clock samples"
        );
    }
    samples.sort_by(f64::total_cmp);
    json!({"samples_ms":samples,"median_ms":samples[iterations/2],"min_ms":samples[0],"max_ms":samples[iterations-1],"result_sha256":hash(&expected)})
}
fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse().expect("iterations"))
        .unwrap_or(7);
    assert!((3..=100).contains(&iterations));
    let now: DateTime<Utc> = "2026-10-16T12:00:00Z".parse().expect("fixed clock");
    let principal = Principal {
        tenant_id: "profile".into(),
        subject: "user:dispatcher".into(),
        role: Role::Operations,
        counterparty_id: None,
    };
    let mut results = vec![];
    for (loads, histories, notes) in [(100, 4, 20), (1000, 4, 20), (5000, 4, 20), (1000, 32, 20)] {
        eprintln!("PROFILE {loads} shipments, {histories} historical carrier attempts and {notes} retained notes per case");
        let mut w = fixture(loads, histories, notes, now);
        let original = hash(&json!(w));
        let mut measurements = json!({});
        measurements["signals"] = measure(|| json!(w.operations_signals(now)), iterations);
        measurements["reconciliation_check"] =
            measure(|| json!(w.operations_need_reconciliation(now)), iterations);
        measurements["board_json"] = measure(
            || {
                let bytes = serde_json::to_vec(&w.operations_board(now)).expect("board JSON");
                json!({"bytes":bytes.len(),"sha256":sha256(&bytes)})
            },
            iterations,
        );
        measurements["first_page_json"] = measure(
            || {
                let v = w
                    .operations_page(QueueQuery::default(), &principal, now)
                    .expect("page");
                let bytes = serde_json::to_vec(&v).expect("page JSON");
                json!({"bytes":bytes.len(),"sha256":sha256(&bytes)})
            },
            iterations,
        );
        measurements["summary_json"] = measure(
            || {
                let bytes = serde_json::to_vec(&w.operations_summary(now)).expect("summary JSON");
                json!({"bytes":bytes.len(),"sha256":sha256(&bytes)})
            },
            iterations,
        );
        measurements["reconcile_noop"] = measure(|| json!(w.reconcile_operations(now)), iterations);
        assert_eq!(
            hash(&json!(w)),
            original,
            "profiling mutated retained records"
        );
        results.push(json!({"loads":loads,"historical_attempts_per_load":histories,"appointments":w.appointments.len(),"gate_events":w.stop_events.len(),"notes_per_case":notes,"cases":w.operations_cases.len(),"history_events":w.operations_cases.iter().map(|c|c.history.len()).sum::<usize>(),"workspace_json_bytes":serde_json::to_vec(&w).expect("workspace").len(),"fixture_sha256":original,"measurements":measurements}));
    }
    println!(
        "{}",
        json!({"schema":1,"optimized":!cfg!(debug_assertions),"fixed_at":now,"iterations":iterations,"scope":"Synthetic in-process operations projections with retained appointments, historical tender gates and correction chains; excludes PostgreSQL, HTTP, browser, native carrier traffic, financial histories and deployment capacity", "results":results})
    );
}
