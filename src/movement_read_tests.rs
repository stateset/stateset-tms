//! Compare indexed selectors with the preceding whole-workspace scans.
use super::*;
use crate::movement::{source_sha256, MovementCorrection};
use chrono::Duration;

fn fixture() -> Workspace {
    let mut w = crate::domain::demo();
    let now = Utc::now();
    let base = w.loads[0].clone();
    w.loads = (0..4)
        .map(|n| {
            let mut l = base.clone();
            l.id = format!("owned/雪-{n}");
            l.tender_id = Some(format!("assignment-{n}"));
            l.carrier_id = Some("owned-carrier".into());
            l.status = Status::InTransit;
            l.last_tracking_at = Some(now - Duration::hours(8));
            l
        })
        .collect();
    w.loads[1].status = Status::Delivered;
    w.loads[2].status = Status::Delivered;
    let intermediate = w.loads[2].origin.clone();
    w.loads[2]
        .intermediate_stops
        .push(crate::itinerary::IntermediateStop {
            id: "stop_owned".into(),
            kind: "delivery".into(),
            stop: intermediate,
        });
    w.loads[3].status = Status::Cancelled;
    w.carrier_submissions.clear();
    for (n, l) in w.loads.iter().enumerate() {
        for (kind, minutes) in [("pickup", 180), ("delivery", 60), ("pod", 30)] {
            w.carrier_submissions.push(CarrierSubmission {
                id: format!("native-{n}-{kind}"),
                offer_id: format!("offer-{n}"),
                load_id: l.id.clone(),
                tender_id: l.tender_id.clone().expect("assignment"),
                carrier_id: l.carrier_id.clone().expect("carrier"),
                kind: kind.into(),
                reference: "Owned actual report".into(),
                note: "Original quoted \"雪\" evidence".into(),
                observed_at: Some(now - Duration::minutes(minutes)),
                location: Some("Owned facility".into()),
                outcome: "applied".into(),
                document_id: None,
                document_sha256: None,
                recorded_at: now,
                recorded_by: "fixture:carrier".into(),
            });
        }
    }
    let mut stale = w.carrier_submissions[0].clone();
    stale.id = "prior-assignment".into();
    stale.tender_id = "replaced-assignment".into();
    w.carrier_submissions.push(stale);
    let mut rejected = w.carrier_submissions[0].clone();
    rejected.id = "unapplied-native".into();
    rejected.outcome = "held".into();
    w.carrier_submissions.push(rejected);
    let mut carrier = w.carrier_submissions[0].clone();
    carrier.id = "other-carrier".into();
    carrier.carrier_id = "other-carrier".into();
    w.carrier_submissions.push(carrier);
    let source = w.carrier_submissions[0].clone();
    let mut prior = None;
    for (n, minutes) in [(0, 160), (1, 140)] {
        let mut c = MovementCorrection {
            id: format!("correction-{n}"),
            load_id: source.load_id.clone(),
            tender_id: source.tender_id.clone(),
            carrier_id: source.carrier_id.clone(),
            source_id: source.id.clone(),
            source_sha256: source_sha256(&source),
            supersedes_id: prior,
            observed_at: now - Duration::minutes(minutes),
            contact: "Owned supervisor".into(),
            reference: "Actual corrected reference".into(),
            note: "Original correction evidence".into(),
            recorded_at: now + Duration::seconds(n),
            recorded_by: "fixture:reviewer".into(),
            evidence_sha256: String::new(),
        };
        c.evidence_sha256 = c.digest();
        prior = Some(c.id.clone());
        w.movement_corrections.push(c);
    }
    w.invoices[0].load_id = w.loads[0].id.clone();
    w.invoices[0].status = "approved".into();
    w.settlements.push(crate::domain::Settlement {
        id: "owned-settlement".into(),
        load_id: w.loads[1].id.clone(),
        carrier_id: "owned-carrier".into(),
        amount_cents: 101,
        status: "open".into(),
        version: 1,
        created_at: Some(now),
    });
    for (n, status) in [(2, "draft"), (3, "void")] {
        let l = &w.loads[n];
        w.bills.push(crate::finance::CustomerBill {
            id: format!("bill-{n}"),
            number: format!("OWNED-{n}"),
            load_id: l.id.clone(),
            customer_id: l.customer_id.clone(),
            snapshot: crate::finance::BillingSnapshot {
                customer: w.customers[0].clone(),
                load_reference: l.reference.clone(),
                origin: l.origin.clone(),
                destination: l.destination.clone(),
                commodity: l.commodity.clone(),
                amount_cents: 101,
                currency: "USD".into(),
                source_load_version: l.version,
                issuer: None,
                agreement: None,
                intermediate_stops: l.intermediate_stops.clone(),
            },
            due_date: now.date_naive(),
            status: status.into(),
            version: 1,
            created_at: now,
            created_by: "fixture:finance".into(),
            issued_at: None,
            void_reason: None,
            supplement: None,
        });
    }
    w.reconcile_operations(now);
    w
}

// Exact preceding chain loop, retained independently of the extracted kernel.
fn preceding_chain<'a>(
    w: &'a Workspace,
    source: &CarrierSubmission,
) -> Result<Option<&'a MovementCorrection>> {
    let mut last: Option<&MovementCorrection> = None;
    let mut ids = BTreeSet::new();
    for c in w
        .movement_corrections
        .iter()
        .filter(|c| c.source_id == source.id)
    {
        c.verify()?;
        if c.load_id != source.load_id
            || c.tender_id != source.tender_id
            || c.carrier_id != source.carrier_id
            || c.source_sha256 != source_sha256(source)
            || c.supersedes_id.as_deref() != last.map(|c| c.id.as_str())
            || !ids.insert(c.id.as_str())
        {
            return Err("Internal storage error".into());
        }
        last = Some(c);
    }
    Ok(last)
}
fn preceding_sources(w: &Workspace, load: &Load) -> Value {
    let locked = w.execution_evidence_locked(&load.id);
    let valid = w.verify_movement_corrections(load).is_ok();
    let physical = matches!(load.status, Status::InTransit | Status::Delivered);
    json!(w.carrier_submissions.iter().filter(|s|s.load_id==load.id && Some(&s.tender_id)==load.tender_id.as_ref() && Some(&s.carrier_id)==load.carrier_id.as_ref() && s.outcome=="applied" && ["pickup","delivery"].contains(&s.kind.as_str())).map(|s| {
        let latest=preceding_chain(w,s);
        json!({"source":s,"source_sha256":source_sha256(s),"effective_observed_at":latest.as_ref().ok().and_then(|c|c.map(|c|c.observed_at)).or(s.observed_at),"evidence_valid":valid && latest.is_ok(),"can_correct":physical && !locked && valid && latest.is_ok(),"locked":locked,"corrections":w.movement_corrections.iter().filter(|c|c.source_id==s.id).collect::<Vec<_>>()})
    }).collect::<Vec<_>>())
}
fn check(w: &Workspace) {
    let before = serde_json::to_vec(w).expect("original model");
    let now = Utc::now();
    let index = MovementReadIndex::new(w);
    let gates = crate::stops::CurrentStopEvents::new(&w.stop_events);
    for source in &w.carrier_submissions {
        assert_eq!(
            index.current(source).map(|c| c.map(|c| json!(c))),
            preceding_chain(w, source).map(|c| c.map(|c| json!(c)))
        );
    }
    for load in &w.loads {
        let current = gates.for_load(load);
        assert_eq!(index.verify(load), w.verify_movement_corrections(load));
        assert_eq!(index.source_views(load), preceding_sources(w, load));
        assert_eq!(
            index.consistent(load, current),
            w.execution_consistent_with_gates(load, current)
        );
        assert_eq!(
            index.delivery_ready(load, current),
            w.delivery_evidence_ready_with_gates(load, current)
        );
        for private in [true, false] {
            assert_eq!(
                index.timeline(load, private, current),
                w.execution_timeline_with_gates(load, private, current)
            );
        }
    }
    let preceding = crate::operations::preceding_summary(w, now);
    assert_eq!(index.operations_summary(now), preceding);
    assert_eq!(w.operations_summary(now), preceding);
    assert_eq!(
        json!(w.operations_signals(now)),
        json!(crate::operations::preceding_signals(w, now))
    );
    assert_eq!(serde_json::to_vec(w).expect("unchanged original"), before);
}

#[test]
fn indexed_movement_keeps_complete_sources_route_views_lock_rules_and_original_order() {
    let mut w = fixture();
    check(&w);
    let index = MovementReadIndex::new(&w);
    for (n, locked) in [(0, true), (1, true), (2, true), (3, false)] {
        let rows = index.source_views(&w.loads[n]);
        assert!(rows
            .as_array()
            .expect("sources")
            .iter()
            .all(|row| row["locked"] == locked));
    }
    drop(index);
    w.loads[0].tender_id = None;
    w.loads[1].carrier_id = None;
    w.carrier_submissions[3].observed_at = None;
    check(&w);
}

#[test]
fn correction_corruption_forks_cross_assignment_links_and_orphans_never_hide_in_an_index() {
    let original = fixture();
    for fault in [
        "seal",
        "source_hash",
        "supersedes",
        "duplicate",
        "reorder",
        "foreign_load",
        "foreign_assignment",
        "foreign_carrier",
        "orphan",
        "source_changed",
    ] {
        let mut w = original.clone();
        match fault {
            "seal" => w.movement_corrections[0].evidence_sha256 = "damaged".into(),
            "source_hash" => {
                let c = &mut w.movement_corrections[0];
                c.source_sha256 = "changed".into();
                c.evidence_sha256 = c.digest();
            }
            "supersedes" => {
                let c = &mut w.movement_corrections[1];
                c.supersedes_id = None;
                c.evidence_sha256 = c.digest();
            }
            "duplicate" => w
                .movement_corrections
                .push(w.movement_corrections[0].clone()),
            "reorder" => w.movement_corrections.swap(0, 1),
            "foreign_load" => {
                let c = &mut w.movement_corrections[0];
                c.load_id = "foreign-load".into();
                c.evidence_sha256 = c.digest();
            }
            "foreign_assignment" => {
                let c = &mut w.movement_corrections[0];
                c.tender_id = "inactive-assignment".into();
                c.evidence_sha256 = c.digest();
            }
            "foreign_carrier" => {
                let c = &mut w.movement_corrections[0];
                c.carrier_id = "foreign-carrier".into();
                c.evidence_sha256 = c.digest();
            }
            "orphan" => {
                let c = &mut w.movement_corrections[0];
                c.source_id = "absent-source".into();
                c.evidence_sha256 = c.digest();
            }
            "source_changed" => w.carrier_submissions[0].note.push_str(" changed original"),
            _ => unreachable!(),
        }
        assert!(
            MovementReadIndex::new(&w).verify(&w.loads[0]).is_err(),
            "{fault}"
        );
        check(&w);
    }
}

#[test]
fn duplicate_source_ids_retain_first_matching_record_and_do_not_collapse_foreign_evidence() {
    let mut w = fixture();
    let mut held = w.carrier_submissions[0].clone();
    held.outcome = "held".into();
    w.carrier_submissions.insert(0, held);
    assert!(MovementReadIndex::new(&w).verify(&w.loads[0]).is_ok());
    check(&w);
    let mut duplicate = w.carrier_submissions[4].clone();
    duplicate.id = w.carrier_submissions[1].id.clone();
    w.carrier_submissions.push(duplicate);
    assert!(MovementReadIndex::new(&w).verify(&w.loads[1]).is_err());
    check(&w);
}

#[test]
fn validity_is_lazy_and_reused_only_in_the_same_immutable_read() {
    let mut w = fixture();
    let index = MovementReadIndex::new(&w);
    assert_eq!(index.checks.get(), 0);
    for _ in 0..3 {
        assert!(index.verify(&w.loads[0]).is_ok());
    }
    assert_eq!(index.checks.get(), 1);
    drop(index);
    w.movement_corrections[0].note.push_str(" unsealed change");
    let next = MovementReadIndex::new(&w);
    assert!(next.verify(&w.loads[0]).is_err());
    assert!(next.verify(&w.loads[0]).is_err());
    assert_eq!(next.checks.get(), 1);
    drop(next);
    let mut unused = w.movement_corrections.remove(0);
    unused.load_id = "unused".into();
    unused.tender_id = "unused".into();
    unused.source_id = "unused".into();
    w.movement_corrections.clear();
    w.movement_corrections.push(unused);
    let index = MovementReadIndex::new(&w);
    assert_eq!(index.checks.get(), 0);
    assert!(
        index.verify(&w.loads[0]).is_ok(),
        "unused historical corruption is not globally applied"
    );
    check(&w);
}

#[test]
fn operations_quarantine_and_time_sensitive_summaries_keep_the_existing_contract() {
    let original = fixture();
    for cases in [
        json!({"future_evidence":"retained"}),
        json!([{"unsupported":"original"}]),
    ] {
        let mut raw = json!(original);
        raw["operations_cases"] = cases;
        let w: Workspace = serde_json::from_value(raw).expect("quarantine original");
        check(&w);
    }
    let mut raw = json!(original);
    raw["operations_policy"] = json!({"version":2,"tracking_stale_minutes":0});
    check(&serde_json::from_value(raw).expect("retained invalid policy"));
}
