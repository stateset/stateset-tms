use super::*;
use crate::itinerary::IntermediateStop;
use chrono::{Duration, Utc};

pub(crate) fn fixture(count: usize) -> Workspace {
    let mut w = crate::domain::demo();
    let mut prototype = w.loads[0].clone();
    prototype.status = Status::Delivered;
    prototype.intermediate_stops = vec![IntermediateStop {
        id: "stop_middle".into(),
        kind: "delivery".into(),
        stop: prototype.origin.clone(),
    }];
    w.loads.clear();
    w.stop_services.clear();
    let now = Utc::now();
    for n in 0..count {
        let mut load = prototype.clone();
        load.id = format!("load-{n}");
        load.tender_id = Some(format!("tender-{n}"));
        let mut prior = None;
        for step in 0..4 {
            let mut service = StopService {
                id: format!("service-{n}-{step}"),
                load_id: load.id.clone(),
                tender_id: load.tender_id.clone().expect("tender"),
                stop_id: "stop_middle".into(),
                stop: load.intermediate_stops[0].clone(),
                observed_at: now - Duration::hours(1),
                contact: "Synthetic facility".into(),
                reference: "Original reference 雪".into(),
                note: "Retained evidence".into(),
                recorded_at: now + Duration::seconds(step),
                recorded_by: "fixture:operator".into(),
                offer_id: None,
                supersedes_id: prior,
                evidence_sha256: String::new(),
            };
            service.evidence_sha256 = service.digest();
            prior = Some(service.id.clone());
            w.stop_services.push(service);
        }
        w.loads.push(load);
    }
    w
}

fn compare(w: &Workspace) {
    let index = ServiceReadIndex::new(w);
    for load in &w.loads {
        assert_eq!(index.route_ready(load), w.route_ready(load), "{}", load.id);
        assert_eq!(json!(index.current(load)), json!(w.current_services(load)));
        for private in [false, true] {
            assert_eq!(
                index.view(load, private),
                w.itinerary_view(load, private),
                "{}",
                load.id
            );
        }
        for key in ["pickup", "delivery", "stop_middle", "stop_absent"] {
            assert_eq!(index.stop_complete(load, key), w.stop_complete(load, key));
        }
    }
}

#[test]
fn original_order_privacy_and_complete_correction_chains_match_scans() {
    let w = fixture(6);
    compare(&w);
    let mut no_assignment = w.clone();
    no_assignment.loads[0].tender_id = None;
    compare(&no_assignment);
    let mut empty = w.clone();
    empty.loads[0].intermediate_stops.clear();
    compare(&empty);
}

#[test]
fn corruption_duplicates_or_missing_predecessors_never_become_ready() {
    for kind in 0..5 {
        let mut w = fixture(3);
        match kind {
            0 => w.stop_services[0].note.push_str("tampered"),
            1 => w.stop_services.push(w.stop_services[0].clone()),
            2 => {
                w.stop_services.remove(0);
            }
            3 => {
                w.stop_services[3].stop.stop.facility = "Changed".into();
                w.stop_services[3].evidence_sha256 = w.stop_services[3].digest();
            }
            _ => {
                w.stop_services[3].supersedes_id = Some(w.stop_services[3].id.clone());
                w.stop_services[3].evidence_sha256 = w.stop_services[3].digest();
            }
        }
        compare(&w);
        let index = ServiceReadIndex::new(&w);
        assert!(!index.route_ready(&w.loads[0]));
        assert!(
            index.route_ready(&w.loads[1]),
            "unrelated assignment remains usable"
        );
    }
}

#[test]
fn foreign_supersession_still_removes_the_original_globally() {
    let mut w = fixture(2);
    w.stop_services[7].supersedes_id = Some(w.stop_services[3].id.clone());
    w.stop_services[7].evidence_sha256 = w.stop_services[7].digest();
    compare(&w);
    assert!(!ServiceReadIndex::new(&w).route_ready(&w.loads[0]));
}

#[test]
fn duplicate_route_ids_and_alternate_loads_cannot_reuse_validity() {
    let mut w = fixture(2);
    let mut duplicate = w.loads[0].intermediate_stops[0].clone();
    duplicate.stop.facility = "Different facility".into();
    w.loads[0].intermediate_stops.push(duplicate);
    compare(&w);
    assert!(!ServiceReadIndex::new(&w).route_ready(&w.loads[0]));

    let w = fixture(1);
    let index = ServiceReadIndex::new(&w);
    assert!(index.route_ready(&w.loads[0]));
    let mut alternate = w.loads[0].clone();
    alternate.intermediate_stops[0].stop.facility = "Different plan, same ID".into();
    assert!(!index.route_ready(&alternate));
    assert!(index.route_ready(&w.loads[0]));
}

#[test]
fn repeated_views_validate_each_borrowed_assignment_once() {
    let w = fixture(100);
    let index = ServiceReadIndex::new(&w);
    for load in &w.loads {
        for _ in 0..3 {
            assert!(index.route_ready(load));
            assert!(index.stop_complete(load, "stop_middle"));
            assert_eq!(index.view(load, true), w.itinerary_view(load, true));
        }
    }
    assert_eq!(index.checks.get(), w.loads.len());
    let mut changed = w.clone();
    changed.stop_services[0]
        .note
        .push_str("changed after preceding read");
    assert!(!ServiceReadIndex::new(&changed).route_ready(&changed.loads[0]));
}

#[test]
fn scaled_route_projection_matches_originals() {
    use std::{hint::black_box, time::Instant};
    // Ordinary regression remains small; explicitly request the larger paired
    // diagnostic without weakening any equality or original-retention checks.
    let sizes = if std::env::var("FREIGHT_PROFILE_ROUTE_SCALE").as_deref() == Ok("1") {
        [100, 1000]
    } else {
        [10, 100]
    };
    for count in sizes {
        let w = fixture(count);
        let original = serde_json::to_vec(&w).expect("original workspace");
        let project = |indexed: bool| {
            let index = indexed.then(|| ServiceReadIndex::new(&w));
            let result = w
                .loads
                .iter()
                .map(|load| match &index {
                    Some(index) => index.view(load, true),
                    None => w.itinerary_view(load, true),
                })
                .collect::<Vec<_>>();
            serde_json::to_vec(&result).expect("complete response")
        };
        let expected = project(false);
        let mut legacy_ms = Vec::new();
        let mut indexed_ms = Vec::new();
        for sample in 0..3 {
            for indexed in if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                let actual = black_box(project(indexed));
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(actual, expected);
                if indexed {
                    indexed_ms.push(ms);
                } else {
                    legacy_ms.push(ms);
                }
            }
        }
        assert_eq!(
            serde_json::to_vec(&w).expect("retained originals"),
            original
        );
        println!(
            "route_projection_profile={}",
            json!({
                "loads":count, "service_records":w.stop_services.len(),
                "legacy_ms":legacy_ms, "indexed_ms":indexed_ms,
                "scope":"Complete itinerary projection and encoding, including index construction; excludes SQL, full-workspace projection and HTTP"
            })
        );
    }
}
