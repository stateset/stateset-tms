//! Compare complete logical responses with the preceding independent projection.
use super::*;
use crate::domain;

fn tenant() -> Tenant {
    Tenant(
        "owned-direct-response".into(),
        Principal {
            tenant_id: "owned-direct-response".into(),
            subject: "fixture:staff".into(),
            role: crate::auth::Role::Admin,
            counterparty_id: None,
        },
    )
}
fn strip_clock(mut value: Value) -> Value {
    value["operations"]
        .as_object_mut()
        .expect("operations")
        .remove("generated_at");
    value
}

#[test]
fn indexed_pod_readiness_retains_assignment_conflict_and_global_supersession_rules() {
    let mut load = domain::demo().loads.remove(0);
    load.status = domain::Status::Delivered;
    load.tender_id = Some("owned-current-tender".into());
    load.intermediate_stops.clear();
    load.pod_document_id = None;
    load.pod_reference = Some("OWNED-POD".into());
    load.pod_review = Some(crate::documents::Review {
        decision: "accepted".into(),
        note: "Owned synthetic POD review".into(),
        reviewed_by: "fixture:reviewer".into(),
        reviewed_at: chrono::Utc::now(),
        reference: "OWNED-POD".into(),
    });
    let event = |id: &str, stop: &str, kind: &str, occurred: &str| -> crate::stops::StopEvent {
        serde_json::from_value(json!({"id":id,"load_id":load.id,"tender_id":load.tender_id,
            "appointment_id":null,"stop":stop,"kind":kind,"occurred_at":occurred,
            "local_time":occurred.trim_end_matches('Z'),"timezone":"UTC","disambiguation":"unique",
            "reference":"Owned gate","note":"Owned synthetic actual evidence","recorded_at":occurred,
            "recorded_by":"fixture:gate","supersedes_id":null})).expect("typed gate")
    };
    let original = domain::Workspace {
        stop_events: vec![
            event(
                "owned-pickup",
                "pickup",
                "departure",
                "2026-01-01T00:00:00Z",
            ),
            event(
                "owned-delivery",
                "delivery",
                "arrival",
                "2026-01-02T00:00:00Z",
            ),
        ],
        loads: vec![load.clone()],
        ..Default::default()
    };
    assert!(
        original.delivery_evidence_ready(&load),
        "positive complete fixture"
    );
    for (scenario, ready) in [
        ("valid", true),
        ("wrong-tender", true),
        ("wrong-load", true),
        ("cross-load-supersession", true),
        ("conflict", false),
        ("rejected-pod", false),
    ] {
        let mut workspace = original.clone();
        if scenario == "rejected-pod" {
            workspace.loads[0]
                .pod_review
                .as_mut()
                .expect("review")
                .decision = "rejected".into();
        } else if scenario != "valid" {
            let mut late = event(
                "owned-late-pickup",
                "pickup",
                "departure",
                "2035-01-01T00:00:00Z",
            );
            if scenario == "wrong-tender" {
                late.tender_id = "previous-tender".into();
            }
            if scenario == "wrong-load" {
                late.load_id = "another-load".into();
            }
            workspace.stop_events.push(late);
            if scenario == "cross-load-supersession" {
                let mut replacement = event(
                    "owned-other-replacement",
                    "pickup",
                    "departure",
                    "2026-01-01T00:00:00Z",
                );
                replacement.load_id = "another-load".into();
                replacement.supersedes_id = Some("owned-late-pickup".into());
                workspace.stop_events.push(replacement);
            }
        }
        let bytes = serde_json::to_vec(&workspace).expect("originals");
        let gates = crate::stops::CurrentStopEvents::new(&workspace.stop_events);
        let current = &workspace.loads[0];
        assert_eq!(
            workspace.delivery_evidence_ready(current),
            ready,
            "{scenario}"
        );
        assert_eq!(
            workspace.delivery_evidence_ready_with_gates(current, gates.for_load(current)),
            ready,
            "{scenario}"
        );
        assert_eq!(
            serde_json::to_vec(&workspace).expect("unchanged originals"),
            bytes
        );
    }
}

#[test]
#[ignore = "requires owned FINANCIAL_WORKSPACE_FIXTURE; attribution only"]
fn profile_complete_financial_response_without_changing_its_content() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .expect("owned phase subscriber");
    let raw = std::fs::read(std::env::var("FINANCIAL_WORKSPACE_FIXTURE").expect("owned fixture"))
        .expect("fixture bytes");
    let workspace: domain::Workspace = serde_json::from_slice(&raw).expect("typed fixture");
    let expected = strip_clock(
        workspace_view_with_history(
            workspace.clone(),
            tenant(),
            false,
            true,
            json!({}),
            WorkspaceProjection {
                deferred_history: false,
                invoice_review_required: true,
                defer_approval: false,
            },
        )
        .expect("independent complete projection"),
    );
    for iteration in 0..8 {
        let value = workspace.clone();
        let started = std::time::Instant::now();
        let bytes = workspace_response::encode(
            value,
            tenant(),
            false,
            true,
            json!({}),
            WorkspaceProjection {
                deferred_history: false,
                invoice_review_required: true,
                defer_approval: false,
            },
        )
        .expect("complete response");
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let actual: Value = serde_json::from_slice(&bytes).expect("complete JSON");
        assert_eq!(strip_clock(actual), expected);
        println!(
            "FINANCIAL_RESPONSE_PROFILE {}",
            json!({"iteration":iteration,"elapsed_ms":elapsed,"response_bytes":bytes.len(),"loads":workspace.loads.len(),"invoices":workspace.invoices.len(),"complete_reference_equal":true})
        );
    }
}

#[test]
fn indexed_services_keep_complete_staff_responses_for_every_history_mode() {
    for corrupt in [false, true] {
        let mut services = crate::itinerary::service_read_fixture(4);
        if corrupt {
            services.stop_services[0].note.push_str("tampered original");
        }
        // Keep the finance fixture's referenced loads while adding independent
        // route histories. The route-only fixture deliberately replaces loads.
        let mut w = crate::finance_read_tests::fixture();
        w.loads.extend(services.loads);
        w.stop_services.extend(services.stop_services);
        for deferred_history in [false, true] {
            for defer_approval in [false, true] {
                let projection = || WorkspaceProjection {
                    deferred_history,
                    invoice_review_required: true,
                    defer_approval,
                };
                let reference = workspace_view_with_history(
                    w.clone(),
                    tenant(),
                    false,
                    true,
                    json!({}),
                    projection(),
                )
                .expect("original complete projection");
                let encoded = workspace_response::encode(
                    w.clone(),
                    tenant(),
                    false,
                    true,
                    json!({}),
                    projection(),
                )
                .expect("indexed complete projection");
                let current: Value = serde_json::from_slice(&encoded).expect("JSON");
                assert_eq!(strip_clock(reference), strip_clock(current));
            }
        }
    }
}

#[test]
fn indexed_finance_keeps_complete_staff_responses_for_every_history_mode() {
    let w = crate::finance_read_tests::fixture();
    for deferred_history in [false, true] {
        for defer_approval in [false, true] {
            let reference = workspace_view_with_history(
                w.clone(),
                tenant(),
                false,
                true,
                json!({}),
                WorkspaceProjection {
                    deferred_history,
                    invoice_review_required: true,
                    defer_approval,
                },
            )
            .expect("original complete projection");
            let encoded = workspace_response::encode(
                w.clone(),
                tenant(),
                false,
                true,
                json!({}),
                WorkspaceProjection {
                    deferred_history,
                    invoice_review_required: true,
                    defer_approval,
                },
            )
            .expect("indexed complete projection");
            let current: Value = serde_json::from_slice(&encoded).expect("JSON");
            assert_eq!(strip_clock(reference), strip_clock(current));
        }
    }
}

#[test]
fn direct_workspace_retains_complete_and_deferred_history_approval_contracts_and_private_roots() {
    let mut w = domain::demo();
    // Private command recovery records must never leak by flattening Workspace.
    let mut raw = json!(w);
    raw["receipts"] = json!({"private-command":{"fingerprint":"original-signature","response":{"private":"must not be exposed"}}});
    w = serde_json::from_value(raw).expect("typed original");
    // Construct opaque metadata directly: Value-to-Value deserialization treats
    // the reserved-looking key as a marker before the encoder is exercised.
    w.loads[0].source_order = Some(
        json!({"literal/雪":{"$serde_json::private::RawValue":"retain","large":u64::MAX,"negative":i64::MIN},"content":"metadata with \"quotes\" and 🚚"}),
    );

    for deferred_history in [false, true] {
        for defer_approval in [false, true] {
            let reference = workspace_view_with_history(
                w.clone(),
                tenant(),
                false,
                true,
                json!({"owned":"integration-summary"}),
                WorkspaceProjection {
                    deferred_history,
                    invoice_review_required: true,
                    defer_approval,
                },
            )
            .expect("preceding projection");
            let encoded = workspace_response::encode(
                w.clone(),
                tenant(),
                false,
                true,
                json!({"owned":"integration-summary"}),
                WorkspaceProjection {
                    deferred_history,
                    invoice_review_required: true,
                    defer_approval,
                },
            )
            .expect("direct encoding");
            // Flatten only the existing public Load. Reject duplicate keys,
            // including future collisions with derived fields, before comparing.
            stateset_freight_storage_codec::canonical(
                std::str::from_utf8(&encoded).expect("UTF-8"),
            )
            .expect("unique JSON keys throughout the public response");
            // Compare every raw member without asking serde_json::Value to
            // interpret reserved-looking opaque keys inside source metadata.
            let current: std::collections::BTreeMap<String, Box<serde_json::value::RawValue>> =
                serde_json::from_slice(&encoded).expect("complete raw JSON");
            let mut comparable: std::collections::BTreeMap<
                String,
                Box<serde_json::value::RawValue>,
            > = serde_json::from_slice(&encoded).expect("complete raw JSON");
            let mut operations: std::collections::BTreeMap<
                String,
                Box<serde_json::value::RawValue>,
            > = serde_json::from_str(comparable["operations"].get()).expect("operations");
            operations.remove("generated_at");
            comparable.insert(
                "operations".into(),
                serde_json::value::to_raw_value(&operations).expect("operations bytes"),
            );
            assert_eq!(
                stateset_freight_storage_codec::canonical(&strip_clock(reference).to_string())
                    .expect("legacy canonical bytes"),
                stateset_freight_storage_codec::canonical(
                    &serde_json::to_string(&comparable).expect("raw response")
                )
                .expect("candidate canonical bytes")
            );
            assert!(!current.contains_key("receipts"));
            assert!(!current.contains_key("operations_cases"));
            assert!(!current.contains_key("operations_policy"));
            assert!(!current.contains_key("_freight_format"));
            assert_eq!(!current.contains_key("events"), deferred_history);
            assert_eq!(current.contains_key("history"), deferred_history);
        }
    }
}

#[test]
fn direct_encoding_preserves_failure_instead_of_emitting_partial_success() {
    let mut w = domain::demo();
    w.invoices[0].load_id = "missing-original-load".into();
    let old = workspace_view_with_history(
        w.clone(),
        tenant(),
        false,
        true,
        json!({}),
        WorkspaceProjection {
            deferred_history: false,
            invoice_review_required: true,
            defer_approval: false,
        },
    );
    let direct = workspace_response::encode(
        w,
        tenant(),
        false,
        true,
        json!({}),
        WorkspaceProjection {
            deferred_history: false,
            invoice_review_required: true,
            defer_approval: false,
        },
    );
    assert!(old.is_err());
    assert_eq!(old.err(), direct.err());
}

#[test]
fn borrowed_loads_keep_booked_quote_bindings_and_omit_missing_bindings() {
    let mut w = domain::demo();
    let mut plan = crate::quotes::QuotePlan::from_load(&w.loads[0]);
    plan.origin.date = (chrono::Utc::now() + chrono::Duration::days(2)).date_naive();
    plan.destination.date = (chrono::Utc::now() + chrono::Duration::days(3)).date_naive();
    let expiry = chrono::Utc::now() + chrono::Duration::hours(12);
    let q=w.apply_quote("quote.create","",json!({"reference":"BORROWED-QUOTE","plan":plan,"sell_cents":150001,"valid_until":expiry,"terms":"Owned original terms"}),"fixture:operator").expect("draft");
    let id = q["id"].as_str().expect("quote").to_owned();
    let q=w.apply_quote("quote.offer",&id,json!({"expected_version":q["version"],"carrier_id":"ca4","amount_cents":125001,"valid_until":expiry,"contact":"Owned carrier","reference":"OWNED-OFFER","note":"Synthetic original price"}),"fixture:operator").expect("offer");
    let offer = q["revisions"][0]["offers"][0]["id"].clone();
    let q = w
        .apply_quote(
            "quote.issue",
            &id,
            json!({"expected_version":q["version"],"offer_id":offer}),
            "fixture:operator",
        )
        .expect("issue");
    let q=w.apply_quote("quote.decision",&id,json!({"expected_version":q["version"],"decision":"accept","contact":"Owned customer","reference":"OWNED-ACCEPTANCE","note":"Synthetic customer decision"}),"fixture:operator").expect("accept");
    w.apply_quote(
        "quote.book",
        &id,
        json!({"expected_version":q["version"]}),
        "fixture:operator",
    )
    .expect("book");
    let quote = w.quotes.iter().find(|q| q.id == id).expect("booked quote");
    let booked = quote.booked_load_id.clone().expect("booked load");
    let reference = workspace_view_with_history(
        w.clone(),
        tenant(),
        false,
        true,
        json!({}),
        WorkspaceProjection {
            deferred_history: false,
            invoice_review_required: true,
            defer_approval: false,
        },
    )
    .expect("legacy projection");
    let encoded = workspace_response::encode(
        w,
        tenant(),
        false,
        true,
        json!({}),
        WorkspaceProjection {
            deferred_history: false,
            invoice_review_required: true,
            defer_approval: false,
        },
    )
    .expect("borrowed projection");
    stateset_freight_storage_codec::canonical(std::str::from_utf8(&encoded).expect("UTF-8"))
        .expect("unique keys");
    let actual: Value = serde_json::from_slice(&encoded).expect("complete response");
    assert_eq!(strip_clock(actual.clone()), strip_clock(reference));
    let loads = actual["loads"].as_array().expect("loads");
    let quoted = loads
        .iter()
        .find(|l| l["id"] == booked)
        .expect("booked load");
    assert_eq!(quoted["quote_binding"]["id"], id);
    assert_eq!(quoted["quote_binding"]["carrier_id"], "ca4");
    assert!(loads
        .iter()
        .filter(|l| l["id"] != booked)
        .all(|l| l.get("quote_binding").is_none()));
}
