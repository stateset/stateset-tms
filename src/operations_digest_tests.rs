use super::*;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

fn legacy_source(source: &Signal) -> Vec<u8> {
    json!(source).to_string().into_bytes()
}
fn legacy_evidence(case: &OperationsCase) -> Vec<u8> {
    let mut value = json!(case);
    value
        .as_object_mut()
        .expect("case")
        .remove("evidence_sha256");
    value.to_string().into_bytes()
}
fn historical() -> OperationsCase {
    let at: DateTime<Utc> = "2026-10-19T03:45:30.439093068Z".parse().expect("instant");
    let source = Signal {
        id: "owned-original-case".into(),
        load_id: Some("owned-load".into()),
        kind: "tracking_stale".into(),
        subject_id: "owned-source".into(),
        title: "Shipment 東京 🚚".into(),
        summary: "Retain \"quotes\", slash\\, newline\n and \0".into(),
        next_action: "Review the actual original".into(),
        priority: 2,
        source: json!({"unknown/雪":{"retained":[u64::MAX,i64::MIN,true,null]},"precise":1.25}),
        deadline: Some(at),
    };
    let source_sha256 = crate::documents::sha256(&legacy_source(&source));
    let mut case = OperationsCase {
        id: source.id.clone(),
        version: 4,
        cycle: 2,
        status: "claimed".into(),
        source: source.clone(),
        source_sha256,
        owner: Some("dispatcher:雪".into()),
        follow_up_at: Some(at + Duration::minutes(15)),
        deferred_until: None,
        first_detected_at: at - Duration::days(1),
        updated_at: at,
        resolved_at: None,
        first_claimed_at: Some(at - Duration::hours(1)),
        history: vec![
            CaseEvent {
                action: "detected".into(),
                actor: "system:operations".into(),
                note: "original".into(),
                at: at - Duration::days(1),
                cycle: 1,
                source: Some(source),
            },
            CaseEvent {
                action: "note".into(),
                actor: "operator:東京".into(),
                note: "literal \0\n\t\\\"".into(),
                at,
                cycle: 2,
                source: None,
            },
        ],
        evidence_sha256: String::new(),
    };
    case.evidence_sha256 = crate::documents::sha256(&legacy_evidence(&case));
    case
}

#[test]
fn complete_borrowed_bytes_match_the_independent_sorted_value_seals() {
    let original = historical();
    let values = [
        Value::Null,
        json!(false),
        json!([]),
        json!({}),
        serde_json::from_str("12345678901234567890123456789.123456789000").expect("exact number"),
        json!({"$serde_json::private::RawValue":"literal, not JSON syntax", "ordinary":[true,null,"雪"]}),
        json!({"$serde_json::private::Number":"literal", "nested":{"unknown":"retain"}}),
    ];
    for value in values {
        for absent in [false, true] {
            let mut case = original.clone();
            case.source.source = value.clone();
            case.version = u64::MAX;
            case.cycle = u64::MAX;
            if absent {
                case.owner = None;
                case.source.load_id = None;
                case.source.deadline = None;
                case.follow_up_at = None;
                case.first_claimed_at = None;
                case.history.clear();
            } else {
                case.resolved_at = Some(case.updated_at);
                case.deferred_until = Some(case.updated_at);
                case.history[0].source = Some(case.source.clone());
                case.history[1].source = Some(case.source.clone());
            }
            case.source_sha256 = crate::documents::sha256(&legacy_source(&case.source));
            let source_bytes = legacy_source(&case.source);
            let evidence_bytes = legacy_evidence(&case);
            assert_eq!(
                serde_json::to_vec(&Source(&case.source)).expect("borrowed source"),
                source_bytes
            );
            assert_eq!(
                serde_json::to_vec(&Evidence(&case)).expect("borrowed case"),
                evidence_bytes
            );
            assert_eq!(
                source(&case.source),
                crate::documents::sha256(&source_bytes)
            );
            assert_eq!(evidence(&case), crate::documents::sha256(&evidence_bytes));
        }
    }
}

#[test]
fn historical_seals_roundtrip_and_new_history_keep_the_existing_seal_format() {
    let case = historical();
    case.verify()
        .expect("original seal made by preceding serializer");
    let mut workspace = crate::domain::demo();
    workspace.operations_cases.clear();
    workspace.operations_cases.push(case.clone());
    let original = json!(workspace);
    let frame = crate::canonical_storage::encode(&workspace).expect("complete original frame");
    let raw = stateset_freight_storage_codec::unpack(frame).expect("complete codec validation");
    let mut restored =
        crate::read_json::decode(&raw, crate::canonical_storage::FORMAT).expect("typed original");
    assert_eq!(
        json!(restored),
        original,
        "no original field or historical seal changes"
    );
    restored
        .verify_operations_cases()
        .expect("historical seals remain valid");
    let changed = &mut restored.operations_cases[0];
    changed
        .record(
            "note",
            "operator:雪",
            "Retained actual note",
            case.updated_at + Duration::seconds(1),
            true,
        )
        .expect("new history");
    assert_eq!(
        changed.evidence_sha256,
        crate::documents::sha256(&legacy_evidence(changed))
    );
    changed
        .verify()
        .expect("new seal is also readable by the preceding algorithm");
}

#[test]
fn tampering_in_original_identity_money_ownership_timestamps_and_history_still_refuses() {
    let original = historical();
    for fault in [
        "id",
        "version",
        "cycle",
        "status",
        "owner",
        "follow_up",
        "defer",
        "first_detected",
        "updated",
        "resolved",
        "first_claimed",
        "source_hash",
        "source_id",
        "source_money",
        "source_priority",
        "history_note",
        "history_actor",
        "history_time",
        "history_cycle",
        "history_source",
        "history_order",
        "seal",
    ] {
        let mut c = original.clone();
        match fault {
            "id" => c.id.push('!'),
            "version" => c.version += 1,
            "cycle" => c.cycle += 1,
            "status" => c.status.push('!'),
            "owner" => c.owner = None,
            "follow_up" => c.follow_up_at = None,
            "defer" => c.deferred_until = Some(c.updated_at),
            "first_detected" => c.first_detected_at += Duration::seconds(1),
            "updated" => c.updated_at += Duration::nanoseconds(1),
            "resolved" => c.resolved_at = Some(c.updated_at),
            "first_claimed" => c.first_claimed_at = None,
            "source_hash" => c.source_sha256.push('!'),
            "source_id" => c.source.id.push('!'),
            "source_money" => c.source.source["extra_money"] = json!(10001),
            "source_priority" => c.source.priority += 1,
            "history_note" => c.history[0].note.push('!'),
            "history_actor" => c.history[0].actor.push('!'),
            "history_time" => c.history[0].at += Duration::nanoseconds(1),
            "history_cycle" => c.history[0].cycle += 1,
            "history_source" => c.history[0].source = None,
            "history_order" => c.history.reverse(),
            "seal" => c.evidence_sha256.push('!'),
            _ => unreachable!(),
        }
        assert_eq!(
            c.evidence_digest(),
            crate::documents::sha256(&legacy_evidence(&c)),
            "{fault}"
        );
        assert!(c.verify().is_err(), "{fault}");
    }
}

#[test]
fn existing_numeric_serialization_unknown_members_and_source_boundaries_keep_their_hashes() {
    let original = historical();
    let mut hashes = std::collections::BTreeMap::new();
    for raw in [
        "1",
        "1.0",
        "1.000",
        "-0.0",
        "0",
        "{\"unknown\":1}",
        "{\"unknown\":1,\"extra\":null}",
    ] {
        let mut signal = original.source.clone();
        signal.source = serde_json::from_str(raw).expect("literal source");
        let hash = source(&signal);
        let bytes = legacy_source(&signal);
        assert_eq!(hash, crate::documents::sha256(&bytes));
        // Keep native model normalization: some serde_json configurations
        // represent 1.0 and 1.000 identically. The retained codec originals are
        // a different boundary; this must not change existing source seals.
        if let Some(previous) = hashes.get(&bytes) {
            assert_eq!(previous, &hash);
        } else {
            assert!(!hashes.values().any(|previous| previous == &hash));
        }
        hashes.insert(bytes, hash);
    }
    let mut a = original.source.clone();
    a.title = "a".into();
    a.summary = "bc".into();
    let mut b = a.clone();
    b.title = "ab".into();
    b.summary = "c".into();
    assert_ne!(source(&a), source(&b));
}
