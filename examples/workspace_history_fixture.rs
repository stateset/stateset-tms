//! Frozen typed history for full-workspace HTTP comparisons; not a carrier certification.
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use stateset_freight::{
    carrier_portal::CarrierSubmission,
    documents::sha256,
    domain::{self, Event, Workspace},
    operations::CaseEvent,
    stops::StopEvent,
    tenders::TenderAttempt,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let count: usize = args.next().expect("shipment count").parse().expect("count");
    let prefix = args.next().expect("unique synthetic tenant prefix");
    assert!((1..=5000).contains(&count));
    assert!(prefix
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    let now: DateTime<Utc> = args
        .next()
        .map(|v| v.parse().expect("fixture clock"))
        .unwrap_or_else(Utc::now);
    let histories: usize = args.next().expect("history depth").parse().expect("depth");
    assert!((1..=32).contains(&histories));
    let mut demo = domain::demo();
    for carrier in &mut demo.carriers {
        carrier.insurance_expires = (now + Duration::days(365)).date_naive();
    }
    let template = demo.loads[0].clone();
    let carrier = demo
        .carriers
        .iter()
        .find(|c| c.id == "ca1")
        .expect("carrier")
        .clone();
    let mut w = Workspace {
        customers: demo.customers,
        carriers: demo.carriers,
        ..Default::default()
    };
    for i in 0..count {
        let mut l = template.clone();
        l.id = format!("{prefix}-{i:05}");
        l.reference = format!("{prefix}-{i:05}");
        l.tender_id = Some(format!("tender-{prefix}-{i:05}"));
        l.driver_id = None;
        l.equipment_id = None;
        l.created_at = now - Duration::days(2);
        l.updated_at = now - Duration::hours(6);
        l.last_tracking_at = Some(now - Duration::hours(6));
        l.origin.date = (now - Duration::days(1)).date_naive();
        l.destination.date = (now + Duration::days(1)).date_naive();
        l.exception = (i % 10 == 0).then(|| "Synthetic facility closure under review".into());
        let tender = l.tender_id.clone().expect("tender");
        w.tenders.push(TenderAttempt {
            id: tender.clone(),
            load_id: l.id.clone(),
            sequence: 1,
            shipment_reference: l.reference.clone(),
            carrier_id: carrier.id.clone(),
            carrier: carrier.clone(),
            plan: l.clone(),
            status: "accepted".into(),
            created_at: now - Duration::days(2),
            created_by: "synthetic:fixture".into(),
            updated_at: now - Duration::hours(8),
            legacy: true,
        });
        for (kind, minutes) in [("arrival", 480), ("departure", 450)] {
            let at = now - Duration::minutes(minutes);
            w.stop_events.push(StopEvent {
                id: format!("gate-{}-{kind}", l.id),
                load_id: l.id.clone(),
                tender_id: tender.clone(),
                appointment_id: None,
                stop: "pickup".into(),
                kind: kind.into(),
                occurred_at: at,
                local_time: at.format("%Y-%m-%dT%H:%M:%S").to_string(),
                timezone: "UTC".into(),
                disambiguation: "reject".into(),
                reference: "Synthetic original gate receipt".into(),
                note: "Synthetic performance fixture, no live freight".into(),
                recorded_at: at,
                recorded_by: "synthetic:facility".into(),
                supersedes_id: None,
            });
        }
        w.carrier_submissions.push(CarrierSubmission {
            id: format!("pickup-{}", l.id),
            offer_id: format!("legacy-offer-{}", l.id),
            load_id: l.id.clone(),
            tender_id: tender,
            carrier_id: carrier.id.clone(),
            kind: "pickup".into(),
            reference: "Synthetic retained pickup observation".into(),
            note: "Synthetic performance fixture, not a real carrier assertion".into(),
            observed_at: Some(now - Duration::minutes(465)),
            location: Some("Portland, OR".into()),
            outcome: "applied".into(),
            document_id: None,
            document_sha256: None,
            recorded_at: now - Duration::minutes(464),
            recorded_by: "synthetic:carrier".into(),
        });
        for n in 0..20 {
            w.events.push(Event {id:format!("observation-{}-{n:03}",l.id),entity_id:l.id.clone(),kind:if n==19 && l.exception.is_some(){"load.exception"}else{"load.location"}.into(),message:format!("Synthetic archived observation {n}; original location and carrier contact retained"),occurred_at:now-Duration::minutes(400-n),actor:"synthetic:fixture".into(),details:None});
        }
        // Keep complete arrivals closed so live dwell minutes are not a
        // volatile field in the before/after response comparison.
        let original_arrival = w.stop_events[w.stop_events.len() - 2].clone();
        for n in 0..histories {
            let mut historical = original_arrival.clone();
            historical.id = format!("old-gate-{}-{n:03}", l.id);
            historical.tender_id = format!("earlier-tender-{}-{n:03}", l.id);
            historical.supersedes_id = None;
            w.stop_events.push(historical);
        }
        let mut prior = format!("gate-{}-arrival", l.id);
        for n in 0..histories {
            let mut corrected = original_arrival.clone();
            corrected.id = format!("corrected-gate-{}-{n:03}", l.id);
            corrected.occurred_at = now - Duration::minutes(480) + Duration::seconds(n as i64 + 1);
            corrected.local_time = corrected
                .occurred_at
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string();
            corrected.recorded_at = now - Duration::minutes(440) + Duration::seconds(n as i64 + 1);
            corrected.supersedes_id = Some(prior);
            prior = corrected.id.clone();
            w.stop_events.push(corrected);
        }
        if i % 2 == 0 {
            for (kind, minutes) in [("arrival", 30), ("departure", 20)] {
                let mut event = w.stop_events.last().expect("gate").clone();
                event.id = format!("delivery-{}-{kind}", l.id);
                event.stop = "delivery".into();
                event.kind = kind.into();
                event.occurred_at = now - Duration::minutes(minutes);
                event.local_time = event.occurred_at.format("%Y-%m-%dT%H:%M:%S").to_string();
                event.recorded_at = event.occurred_at;
                event.supersedes_id = None;
                w.stop_events.push(event);
            }
        }
        for n in 0..=histories {
            let start = now - Duration::hours(2);
            let end = now - Duration::hours(1);
            let appointment = serde_json::from_value(json!({
                "id":format!("window-{}-{n:03}",l.id),"load_id":l.id,"stop":"delivery",
                "status":if n == histories {"active"} else {"cancelled"},
                "starts_at":start,"ends_at":end,"timezone":"UTC",
                "start_local":start.format("%Y-%m-%dT%H:%M:%S").to_string(),
                "end_local":end.format("%Y-%m-%dT%H:%M:%S").to_string(),
                "disambiguation":"reject","stop_snapshot":l.destination,
                "contact":"Synthetic facility contact","confirmation_reference":"Original window record",
                "note":"Synthetic retained appointment; not a custody or delivery assertion",
                "recorded_at":start-Duration::hours(1),"recorded_by":"synthetic:facility",
                "replaces_id":null,"cancellation":null
            })).expect("typed appointment");
            w.appointments.push(appointment);
        }
        w.loads.push(l);
    }
    w.reconcile_operations(now);
    for c in w.operations_cases.iter_mut() {
        for n in 0..8 {
            c.history.push(CaseEvent {
                action: "note".into(),
                actor: "synthetic:dispatcher".into(),
                note: format!("Synthetic original follow-up {n}; source evidence remains retained"),
                at: now,
                cycle: 1,
                source: None,
            });
        }
        c.version = c.history.len() as u64;
        let mut value = json!(c);
        value
            .as_object_mut()
            .expect("case")
            .remove("evidence_sha256");
        c.evidence_sha256 = sha256(value.to_string().as_bytes());
    }
    w.verify_operations_cases().expect("retained integrity");
    assert!(!w.operations_need_reconciliation(now));
    assert!(w.loads.iter().all(|l| w.execution_consistent(l)));
    println!(
        "{}",
        json!({"generated_at":now,"scope":"synthetic typed records; not a carrier, finance, or capacity certification","counts":{"history_depth":histories,"appointments":w.appointments.len(),"loads":w.loads.len(),"events":w.events.len(),"gates":w.stop_events.len(),"native_pickups":w.carrier_submissions.len(),"cases":w.operations_cases.len(),"case_events":w.operations_cases.iter().map(|c|c.history.len()).sum::<usize>()},"workspace":w})
    );
}
