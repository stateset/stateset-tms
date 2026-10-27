use super::*;
use crate::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    integrations::Integrations,
};
use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use tower::ServiceExt;

const TENANT: &str = "invoice-review-real";
async fn fixture() -> Store {
    let store = Store::new(None).await.expect("owned store");
    store
        .memory
        .write()
        .await
        .insert(TENANT.into(), domain::demo());
    store
}
fn router(store: &Store) -> Router {
    router_for_tenant(store, TENANT)
}
fn router_for_tenant(store: &Store, tenant: &str) -> Router {
    let keys = [
        ("maker", "staff:maker", Role::Finance),
        ("maker-rotated", "staff:maker", Role::Finance),
        ("maker-admin", "staff:maker", Role::Admin),
        ("checker", "staff:checker", Role::Finance),
        ("viewer", "staff:viewer", Role::Viewer),
        ("carrier", "staff:carrier", Role::Carrier),
        ("customer", "staff:customer", Role::Customer),
    ]
    .into_iter()
    .map(|(key, subject, role)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: tenant.into(),
                subject: subject.into(),
                role,
                counterparty_id: match role {
                    Role::Carrier => Some("ca4".into()),
                    Role::Customer => Some("c1".into()),
                    _ => None,
                },
            }),
        )
    })
    .chain([("legacy".into(), Credential::Legacy(tenant.into()))])
    .chain([(
        "other-tenant".into(),
        Credential::Scoped(Principal {
            tenant_id: "other-invoice-workspace".into(),
            subject: "staff:other".into(),
            role: Role::Viewer,
            counterparty_id: None,
        }),
    )])
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        store: store.clone(),
        keys: Arc::new(keys),
        demo: false,
        identity: Arc::new(None),
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(
    router: &Router,
    token: &str,
    path: &str,
    key: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(if body.is_some() { "POST" } else { "GET" })
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .header("x-freight-subject", "staff:spoofed")
                .header("x-freight-role", "admin")
                .body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .expect("request"),
        )
        .await
        .expect("actual handler");
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&body).expect("JSON"))
}
fn review(revision: &str) -> Value {
    json!({"expected_revision":revision,"evidence_reference":"Actual carrier invoice INV-1180","note":"Checked invoice number, carrier, amount, and agreed charges against the source"})
}
fn approval(revision: &str) -> Value {
    json!({"expected_revision":revision,"note":"Independently verified carrier invoice, delivery evidence, and agreed charges"})
}
async fn current(store: &Store, tenant: &str) -> (String, String) {
    let w = store.read(tenant).await.expect("workspace");
    let invoice = &w.invoices[0];
    let summary = crate::invoice_review::ReviewIndex::new(&w)
        .summary(invoice, "staff:checker", true)
        .expect("control");
    (
        invoice.id.clone(),
        summary["revision"].as_str().expect("revision").into(),
    )
}
async fn apply(
    store: &Store,
    actor: &str,
    key: &str,
    operation: &str,
    id: &str,
    body: Value,
) -> Value {
    store
        .mutate_authorized((TENANT, actor), key, operation, id, body)
        .await
        .expect("authorized operation")
}
async fn unchanged_failure(
    store: &Store,
    actor: &str,
    operation: &str,
    id: &str,
    body: Value,
    contains: &str,
) {
    let before = json!(store.read(TENANT).await.expect("before"));
    let error = store
        .mutate_authorized(
            (TENANT, actor),
            &domain::id("declined"),
            operation,
            id,
            body,
        )
        .await
        .expect_err("must decline");
    assert!(error.contains(contains), "{error}");
    assert_eq!(before, json!(store.read(TENANT).await.expect("after")));
}

#[tokio::test]
async fn invoice_review_requires_distinct_named_subject_even_after_key_rotation_and_admin_grant() {
    let store = fixture().await;
    let router = router(&store);
    let (id, revision) = current(&store, TENANT).await;
    let path = format!("invoices/{id}/entry-review");
    for token in ["viewer", "legacy"] {
        let before = json!(store.read(TENANT).await.expect("before"));
        let response = call(
            &router,
            token,
            &path,
            "denied-review",
            Some(review(&revision)),
        )
        .await;
        assert_eq!(response.0, 403, "{response:?}");
        assert_eq!(before, json!(store.read(TENANT).await.expect("after")));
    }
    assert_eq!(
        call(
            &router,
            "maker",
            &path,
            "entry-review",
            Some(review(&revision))
        )
        .await
        .0,
        200
    );
    for history in ["workspace", "workspace?history=deferred"] {
        let response = call(&router, "checker", history, "read", None).await;
        assert_eq!(response.0, 200, "{response:?}");
        assert_eq!(
            response.1["invoices"][0]["approval_control"]["state"],
            "ready_for_approval"
        );
        assert_eq!(
            response.1["invoices"][0]["approval_control"]["revision"],
            revision
        );
    }
    let approve = format!("invoices/{id}/approve");
    for token in ["maker", "maker-rotated", "maker-admin", "legacy", "viewer"] {
        let before = json!(store.read(TENANT).await.expect("before"));
        let response = call(
            &router,
            token,
            &approve,
            "denied-approval",
            Some(approval(&revision)),
        )
        .await;
        assert_eq!(response.0, 403, "{token}: {response:?}");
        assert_eq!(before, json!(store.read(TENANT).await.expect("after")));
    }
    let approved = call(
        &router,
        "checker",
        &approve,
        "independent-approval",
        Some(approval(&revision)),
    )
    .await;
    assert_eq!(approved.0, 200, "{approved:?}");
    assert_eq!(approved.1["data"]["status"], "approved");
    let after = json!(store.read(TENANT).await.expect("approved"));
    let replay = call(
        &router,
        "maker-admin",
        &approve,
        "independent-approval",
        Some(approval(&revision)),
    )
    .await;
    assert_eq!(replay.0, 200, "{replay:?}");
    assert_eq!(replay.1["data"], approved.1["data"]);
    assert_eq!(replay.1["replayed"], true);
    assert_eq!(after, json!(store.read(TENANT).await.expect("replayed")));
    let w = store.read(TENANT).await.expect("audit");
    let event = w
        .events
        .iter()
        .find(|e| e.kind == "invoice.approve")
        .expect("approval event");
    assert_eq!(event.actor, "staff:checker");
    assert_eq!(
        event.details.as_ref().expect("reviewed decision")["revision"],
        revision
    );
    assert!(w.journal.iter().all(|e| e.actor == "staff:checker"));
}

#[tokio::test]
async fn approved_invoice_projection_retains_its_original_review_after_later_evidence() {
    let store = fixture().await;
    let router = router(&store);
    let (id, revision) = current(&store, TENANT).await;
    let reviewed = call(
        &router,
        "maker",
        &format!("invoices/{id}/entry-review"),
        "recorded-review",
        Some(review(&revision)),
    )
    .await;
    assert_eq!(reviewed.0, 200, "{reviewed:?}");
    let approved = call(
        &router,
        "checker",
        &format!("invoices/{id}/approve"),
        "recorded-approval",
        Some(approval(&revision)),
    )
    .await;
    assert_eq!(approved.0, 200, "{approved:?}");
    let before_reads = json!(store.read(TENANT).await.expect("approved source"));
    let mut original_evidence = Value::Null;
    for path in ["workspace", "workspace?history=deferred"] {
        let response = call(&router, "checker", path, "audit-read", None).await;
        assert_eq!(response.0, 200, "{response:?}");
        let control = &response.1["invoices"][0]["approval_control"];
        assert_eq!(control["state"], "approved");
        assert_eq!(
            control["entry_reviewed_by"], "staff:maker",
            "Approving must not hide the entry reviewer of the committed decision"
        );
        assert_ne!(control["revision"], revision);
        assert_eq!(control["can_approve"], false);
        assert_eq!(control["can_review_entry"], false);
        let evidence = &control["approval_evidence"];
        assert_eq!(evidence["revision"], revision);
        assert_eq!(evidence["entry_review_id"], reviewed.1["data"]["review_id"]);
        assert_eq!(evidence["entry_reviewed_by"], "staff:maker");
        assert_eq!(evidence["approved_by"], "staff:checker");
        assert_eq!(evidence["entry_review_note"], review(&revision)["note"]);
        assert_eq!(evidence["approval_note"], approval(&revision)["note"]);
        assert_eq!(
            evidence["evidence_reference"],
            review(&revision)["evidence_reference"]
        );
        assert!(evidence["approved_at"].is_string());
        assert!(evidence["entry_reviewed_at"].is_string());
        assert!(evidence["approval_event_id"].is_string());
        if original_evidence.is_null() {
            original_evidence = evidence.clone();
        } else {
            assert_eq!(*evidence, original_evidence);
        }
    }
    assert_eq!(
        before_reads,
        json!(store.read(TENANT).await.expect("after reads"))
    );
    let load_version = before_reads["loads"][5]["version"].clone();
    let changed = call(
        &router,
        "maker",
        "loads/load_6/pod/review",
        "later-receiver-inspection",
        Some(json!({"expected_version":load_version,"decision":"accepted","note":"Later inspection retains the original receiver evidence"})),
    )
    .await;
    assert_eq!(changed.0, 200, "{changed:?}");
    for path in ["workspace", "workspace?history=deferred"] {
        let response = call(&router, "checker", path, "later-audit-read", None).await;
        assert_eq!(response.0, 200, "{response:?}");
        assert_eq!(
            response.1["invoices"][0]["approval_control"]["approval_evidence"],
            original_evidence
        );
    }
    let after_change = json!(store.read(TENANT).await.expect("changed source"));
    let replay = call(
        &router,
        "maker-admin",
        &format!("invoices/{id}/approve"),
        "recorded-approval",
        Some(approval(&revision)),
    )
    .await;
    assert_eq!(replay.0, 200, "{replay:?}");
    assert_eq!(replay.1["data"], approved.1["data"]);
    assert_eq!(replay.1["replayed"], true);
    assert_eq!(
        after_change,
        json!(store.read(TENANT).await.expect("after replay"))
    );
}

#[tokio::test]
async fn invoice_approval_projection_never_invents_a_missing_or_ambiguous_review_link() {
    let store = fixture().await;
    let (id, revision) = current(&store, TENANT).await;
    apply(
        &store,
        "staff:maker",
        "entry-review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "approval",
        "invoice.approve",
        &id,
        approval(&revision),
    )
    .await;
    let saved = store.read(TENANT).await.expect("recorded source");
    let original = saved
        .invoice_approval_control(&id, "staff:viewer")
        .expect("recorded control")["approval_evidence"]
        .clone();
    assert!(!original.is_null());
    let review_index = saved
        .events
        .iter()
        .position(|e| e.kind == "invoice.entry_reviewed")
        .expect("entry review");
    let decision_index = saved
        .events
        .iter()
        .position(|e| e.kind == "invoice.approve")
        .expect("approval");
    let mut later = saved.clone();
    let mut later_review = later.events[review_index].clone();
    later_review.id = "later-unlinked-review".into();
    later_review.actor = "staff:later-reviewer".into();
    later.events.push(later_review);
    assert_eq!(
        later
            .invoice_approval_control(&id, "staff:viewer")
            .expect("later source")["approval_evidence"],
        original,
        "The decision links its original review, not the last matching review"
    );
    for case in [
        "legacy",
        "missing review",
        "other invoice",
        "different revision",
        "missing review note",
        "missing approval note",
        "unnamed reviewer",
        "unnamed approver",
        "same subject",
        "duplicate review",
        "duplicate decision",
    ] {
        let mut altered = saved.clone();
        match case {
            "legacy" => altered.events[decision_index].details = None,
            "missing review" => {
                altered.events[decision_index]
                    .details
                    .as_mut()
                    .expect("details")["entry_review_id"] = json!("missing-review")
            }
            "other invoice" => {
                altered.events[review_index]
                    .details
                    .as_mut()
                    .expect("details")["invoice_id"] = json!("another-invoice")
            }
            "different revision" => {
                altered.events[review_index]
                    .details
                    .as_mut()
                    .expect("details")["revision"] = json!("0".repeat(64))
            }
            "missing review note" => {
                altered.events[review_index]
                    .details
                    .as_mut()
                    .expect("details")["note"] = Value::Null
            }
            "missing approval note" => {
                altered.events[decision_index]
                    .details
                    .as_mut()
                    .expect("details")["note"] = Value::Null
            }
            "unnamed reviewer" => altered.events[review_index].actor = "system".into(),
            "unnamed approver" => altered.events[decision_index].actor = "system".into(),
            "same subject" => altered.events[decision_index].actor = "staff:maker".into(),
            "duplicate review" => altered.events.push(altered.events[review_index].clone()),
            "duplicate decision" => altered.events.push(altered.events[decision_index].clone()),
            _ => unreachable!("owned case"),
        }
        let before = json!(altered);
        let control = altered
            .invoice_approval_control(&id, "staff:viewer")
            .expect("historical decision remains readable");
        assert_eq!(control["state"], "approved", "{case}");
        assert!(control["approval_evidence"].is_null(), "{case}: {control}");
        assert!(control["entry_reviewed_by"].is_null(), "{case}: {control}");
        assert_eq!(
            before,
            json!(altered),
            "{case}: reading must not repair historical evidence"
        );
    }
}

#[tokio::test]
async fn compact_approval_summaries_defer_only_findings_and_selected_reads_keep_exact_scope() {
    let store = fixture().await;
    let router = router(&store);
    let (id, revision) = current(&store, TENANT).await;
    let mut input = review(&revision);
    input["note"] = json!("Actual checked invoice evidence. ".repeat(100)[..2000]);
    input["evidence_reference"] =
        json!("Carrier original evidence reference. ".repeat(100)[..2000]);
    apply(
        &store,
        "staff:maker",
        "entry-review",
        "invoice.entry_review",
        &id,
        input.clone(),
    )
    .await;
    let mut decision = approval(&revision);
    decision["note"] = json!("Independent finance and delivery review. ".repeat(100)[..2000]);
    apply(
        &store,
        "staff:checker",
        "approval",
        "invoice.approve",
        &id,
        decision.clone(),
    )
    .await;
    let before = json!(store.read(TENANT).await.expect("original source"));
    let full = call(
        &router,
        "viewer",
        "workspace?history=deferred",
        "full",
        None,
    )
    .await;
    assert_eq!(full.0, 200);
    let original = full.1["invoices"][0]["approval_control"]["approval_evidence"].clone();
    assert_eq!(original["entry_review_note"], input["note"]);
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/workspace?history=deferred")
                .header("authorization", "Bearer viewer")
                .header("x-freight-approval-details", "deferred")
                .body(Body::empty())
                .expect("compact request"),
        )
        .await
        .expect("actual handler");
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["vary"], "X-Freight-Approval-Details");
    let compact: Value = serde_json::from_slice(
        &response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("compact JSON");
    let mut expected = full.1.clone();
    let proof = &mut expected["invoices"][0]["approval_control"]["approval_evidence"];
    for field in ["entry_review_note", "approval_note", "evidence_reference"] {
        proof.as_object_mut().expect("proof").remove(field);
    }
    assert_eq!(
        compact["invoices"], expected["invoices"],
        "Opt-in must change only the three deferred invoice fields"
    );
    assert!(
        serde_json::to_vec(&full.1).expect("full bytes").len()
            - serde_json::to_vec(&compact).expect("compact bytes").len()
            >= 6000
    );
    let path = format!("invoices/{id}/approval-evidence");
    for token in ["viewer", "maker", "checker", "maker-admin", "legacy"] {
        let selected = call(&router, token, &path, "detail", None).await;
        assert_eq!(selected.0, 200, "{selected:?}");
        assert_eq!(selected.1["invoice_id"], id);
        assert_eq!(selected.1["tenant_id"], TENANT);
        assert_eq!(selected.1["approval_evidence"], original);
        if token == "viewer" {
            assert_eq!(selected.1["access"]["subject"], "staff:viewer");
        }
    }
    for token in ["carrier", "customer"] {
        assert_eq!(call(&router, token, &path, "denied", None).await.0, 403);
    }
    assert_eq!(
        call(&router, "other-tenant", &path, "other", None).await.0,
        404
    );
    assert_eq!(
        call(
            &router,
            "viewer",
            "invoices/missing/approval-evidence",
            "missing",
            None
        )
        .await
        .0,
        404
    );
    let legacy = before["invoices"][1]["id"].as_str().expect("other invoice");
    let missing = call(
        &router,
        "viewer",
        &format!("invoices/{legacy}/approval-evidence"),
        "unapproved",
        None,
    )
    .await;
    assert_eq!(missing.0, 200);
    assert!(missing.1["approval_evidence"].is_null());
    let unauth = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/{path}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("unauth");
    assert_eq!(unauth.status(), 401);
    let invalid = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/workspace")
                .header("authorization", "Bearer viewer")
                .header("x-freight-approval-details", "unknown")
                .body(Body::empty())
                .expect("invalid mode"),
        )
        .await
        .expect("invalid response");
    assert_eq!(invalid.status(), 400);
    assert_eq!(
        before,
        json!(store.read(TENANT).await.expect("after reads"))
    );
}

#[tokio::test]
async fn selected_approval_read_refuses_ambiguous_invoices_and_oversized_records() {
    let store = fixture().await;
    let (id, revision) = current(&store, TENANT).await;
    apply(
        &store,
        "staff:maker",
        "review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "approve",
        "invoice.approve",
        &id,
        approval(&revision),
    )
    .await;
    let saved = store.read(TENANT).await.expect("source");
    {
        let mut memory = store.memory.write().await;
        let workspace = memory.get_mut(TENANT).expect("workspace");
        workspace.invoices.push(workspace.invoices[0].clone());
    }
    let path = format!("invoices/{id}/approval-evidence");
    assert_eq!(
        call(&router(&store), "viewer", &path, "duplicate", None)
            .await
            .0,
        500
    );
    {
        let mut memory = store.memory.write().await;
        let mut oversized = saved.clone();
        oversized
            .events
            .iter_mut()
            .find(|e| e.kind == "invoice.approve")
            .expect("decision")
            .id = "a".repeat(70000);
        memory.insert(TENANT.into(), oversized);
    }
    let before = json!(store.read(TENANT).await.expect("oversized"));
    let response = call(&router(&store), "viewer", &path, "oversized", None).await;
    assert_eq!(response.0, 422, "{response:?}");
    assert!(response.1.get("approval_evidence").is_none());
    assert_eq!(before, json!(store.read(TENANT).await.expect("unchanged")));
}

#[tokio::test]
async fn missing_review_stale_review_and_same_amount_correction_cannot_post_or_leave_receipts() {
    let store = fixture().await;
    let (id, old) = current(&store, TENANT).await;
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&old),
        "entry review",
    )
    .await;
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        json!({}),
        "expected_revision",
    )
    .await;
    apply(
        &store,
        "staff:maker",
        "review-before-correction",
        "invoice.entry_review",
        &id,
        review(&old),
    )
    .await;
    // An exception corrected back to the same amount is still a new source
    // decision. Its appended event must invalidate the old review (ABA).
    store
        .memory
        .write()
        .await
        .get_mut(TENANT)
        .expect("fixture")
        .invoices[0]
        .status = "exception".into();
    apply(&store,"staff:checker","correction","invoice.resolve",&id,json!({"expected_amount_cents":118000,"amount_cents":118000,"note":"Actual carrier invoice correction confirmed against source"})).await;
    let (_, new) = current(&store, TENANT).await;
    assert_ne!(old, new);
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&old),
        "changed;",
    )
    .await;
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&new),
        "entry review",
    )
    .await;
    apply(
        &store,
        "staff:maker",
        "review-after-correction",
        "invoice.entry_review",
        &id,
        review(&new),
    )
    .await;
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&new),
        "different named",
    )
    .await;
    apply(
        &store,
        "staff:third-checker",
        "third-party-approval",
        "invoice.approve",
        &id,
        approval(&new),
    )
    .await;
}

#[tokio::test]
async fn invoice_entry_actor_cannot_approve_after_another_staff_member_reviews_it() {
    let store = fixture().await;
    store
        .memory
        .write()
        .await
        .get_mut(TENANT)
        .expect("fixture")
        .invoices
        .clear();
    let created = apply(
        &store,
        "staff:maker",
        "invoice-entry",
        "invoice.create",
        "",
        json!({"number":"ACTUAL-1180","load_id":"load_6","amount_cents":118000,"currency":"USD"}),
    )
    .await;
    let (id, revision) = current(&store, TENANT).await;
    assert_eq!(created["data"]["id"], id);
    apply(
        &store,
        "staff:entry-verifier",
        "review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    unchanged_failure(
        &store,
        "staff:maker",
        "invoice.approve",
        &id,
        approval(&revision),
        "different named",
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "approval",
        "invoice.approve",
        &id,
        approval(&revision),
    )
    .await;
}

#[tokio::test]
async fn demo_name_alone_and_seeding_other_tenants_never_disable_invoice_review() {
    let store = fixture().await;
    assert!(store.invoice_review_required("demo"));
    store.seed_demo().await.expect("explicit synthetic seed");
    assert!(!store.invoice_review_required("demo"));
    assert!(store.invoice_review_required(TENANT));
    let (id, revision) = current(&store, TENANT).await;
    unchanged_failure(
        &store,
        "system",
        "invoice.approve",
        &id,
        approval(&revision),
        "named staff",
    )
    .await;
    unchanged_failure(
        &store,
        "credential:rotated-key",
        "invoice.entry_review",
        &id,
        review(&revision),
        "named staff",
    )
    .await;
}

#[tokio::test]
async fn requesting_demo_mode_cannot_exempt_a_preexisting_unverified_tenant() {
    let store = Store::new(None).await.expect("owned store");
    let mut existing = domain::demo();
    existing.customers[0].name = "Owned preexisting business workspace".into();
    store.memory.write().await.insert("demo".into(), existing);
    let api = router_for_tenant(&store, "demo");
    let (id, revision) = current(&store, "demo").await;
    assert_eq!(
        call(
            &api,
            "maker",
            &format!("invoices/{id}/entry-review"),
            "actual-entry-review",
            Some(review(&revision))
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &api,
            "maker-admin",
            &format!("invoices/{id}/approve"),
            "self-before-demo",
            Some(approval(&revision))
        )
        .await
        .0,
        403
    );
    let before = json!(store.read("demo").await.expect("original"));
    let seed = store.seed_demo().await;
    let (status, response) = call(
        &api,
        "maker-admin",
        &format!("invoices/{id}/approve"),
        "self-after-demo",
        Some(approval(&revision)),
    )
    .await;
    assert_eq!(status,403,"Requesting demo mode must not permit self-approval in an existing unverified workspace: {response}");
    assert!(
        seed.is_err(),
        "Existing unverified data cannot acquire synthetic provenance"
    );
    assert!(store.invoice_review_required("demo"));
    assert_eq!(json!(store.read("demo").await.expect("retained")), before);
}

#[tokio::test]
async fn an_actual_memory_demo_can_be_reopened_without_resetting_its_records() {
    let store = Store::new(None).await.expect("store");
    store.seed_demo().await.expect("actual synthetic creation");
    store
        .mutate_authorized(
            ("demo", "staff:demo"),
            "owned-demo-customer",
            "customer.create",
            "",
            json!({"name":"Owned later demo customer","contact":"owned@example.test"}),
        )
        .await
        .expect("demo work");
    let before = json!(store.read("demo").await.expect("retained"));
    store
        .seed_demo()
        .await
        .expect("reopen actual synthetic seed");
    assert!(!store.invoice_review_required("demo"));
    assert_eq!(
        json!(store.read("demo").await.expect("same source")),
        before
    );
}

struct DemoDatabase {
    admin: PgPool,
    pool: PgPool,
    url: String,
    schema: String,
}
impl DemoDatabase {
    async fn new() -> Self {
        let base = std::env::var("TEST_DATABASE_URL").expect("owned PostgreSQL URL");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("demo_provenance_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("owned schema");
        let mut url = reqwest::Url::parse(&base).expect("URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        Store::new(Some(url.as_str()))
            .await
            .expect("current schema");
        let pool = PgPool::connect(url.as_str()).await.expect("observer");
        Self {
            admin,
            pool,
            url: url.to_string(),
            schema,
        }
    }
    async fn clean(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("owned cleanup");
        self.admin.close().await;
    }
}

async fn approved_root_fixture(db: &DemoDatabase) -> (Store, String, Value) {
    let store = Store::new(Some(&db.url)).await.expect("owned Store");
    crate::storage_fixture::put_workspace(&db.pool, TENANT, &domain::demo())
        .await
        .expect("original fixture");
    let (id, revision) = current(&store, TENANT).await;
    apply(
        &store,
        "staff:maker",
        "root-review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "root-approval",
        "invoice.approve",
        &id,
        approval(&revision),
    )
    .await;
    let expected = store
        .read(TENANT)
        .await
        .expect("source")
        .invoice_approval_evidence(&id)
        .expect("reference");
    (store, id, expected)
}
async fn root_physical(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT jsonb_build_object('header',(SELECT jsonb_build_array(data::text,updated_at::text,history_revision,xmin::text,ctid::text) FROM freight_workspaces WHERE tenant_id=$1),'blocks',(SELECT jsonb_agg(jsonb_build_array(root,key,payload::text,xmin::text,ctid::text) ORDER BY root,key) FROM freight_workspace_blocks WHERE tenant_id=$1))")
        .bind(TENANT).fetch_one(pool).await.expect("physical originals")
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_approval_roots_leave_unrelated_originals_unread_and_never_weaken_writes() {
    let db = DemoDatabase::new().await;
    let (store, id, expected) = approved_root_fixture(&db).await;
    let mut source: Value = sqlx::query_scalar("SELECT freight_workspace_originals($1)")
        .bind(TENANT)
        .fetch_one(&db.pool)
        .await
        .expect("raw source");
    source["receipts"] = json!({"unrelated":{"unsupported_original":"x".repeat(500000)}});
    crate::storage_fixture::put(&db.pool, TENANT, &source)
        .await
        .expect("unrelated invalid typed receipt");
    let before = root_physical(&db.pool).await;
    assert!(store
        .read(TENANT)
        .await
        .expect_err("complete read still refuses")
        .contains("Internal storage error"));
    let response = call(
        &router(&store),
        "viewer",
        &format!("invoices/{id}/approval-evidence"),
        "scoped-root",
        None,
    )
    .await;
    assert_eq!(response.0, 200, "{response:?}");
    assert_eq!(
        response.1["approval_evidence"],
        expected["approval_evidence"]
    );
    let full_bytes:i64=sqlx::query_scalar("SELECT sum(octet_length(payload::text))::bigint FROM freight_workspace_blocks WHERE tenant_id=$1").bind(TENANT).fetch_one(&db.pool).await.expect("all originals size");
    let mut tx = db.pool.begin().await.expect("snapshot");
    let selected = canonical::approval_snapshot(&mut tx, TENANT)
        .await
        .expect("selected source")
        .expect("tenant");
    assert!(
        selected.volume().2 < full_bytes as usize / 10,
        "Unrelated receipt must not be transferred"
    );
    assert_eq!(
        selected
            .decode()
            .expect("native selected validation")
            .evidence(&id)
            .expect("record"),
        expected
    );
    tx.rollback().await.expect("read end");
    let error = store
        .mutate_authorized(
            (TENANT, "staff:maker"),
            "must-refuse",
            "invoice.entry_review",
            &id,
            review("invalid"),
        )
        .await
        .expect_err("writes still decode full source");
    assert!(error.contains("Internal storage error"), "{error}");
    assert_eq!(before, root_physical(&db.pool).await);
    drop(store);
    db.clean().await;
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_approval_roots_refuse_selected_corruption_and_future_format_without_rewrites() {
    let db = DemoDatabase::new().await;
    let (store, id, expected) = approved_root_fixture(&db).await;
    let path = format!("invoices/{id}/approval-evidence");
    for root in canonical::APPROVAL_ROOTS {
        let (key,payload):(String,Value)=sqlx::query_as("SELECT key,payload FROM freight_workspace_blocks WHERE tenant_id=$1 AND root=$2 ORDER BY key LIMIT 1").bind(TENANT).bind(root).fetch_one(&db.pool).await.expect("selected original");
        sqlx::query("UPDATE freight_workspace_blocks SET payload=jsonb_set(payload,'{items,0,position}','0'::jsonb) WHERE tenant_id=$1 AND root=$2 AND key=$3").bind(TENANT).bind(root).bind(&key).execute(&db.pool).await.expect("bad ordinal fixture");
        let before = root_physical(&db.pool).await;
        assert_eq!(
            call(&router(&store), "viewer", &path, "corrupt", None)
                .await
                .0,
            500,
            "{root}"
        );
        assert_eq!(before, root_physical(&db.pool).await);
        sqlx::query("UPDATE freight_workspace_blocks SET payload=$4 WHERE tenant_id=$1 AND root=$2 AND key=$3").bind(TENANT).bind(root).bind(&key).bind(&payload).execute(&db.pool).await.expect("exact repair");
    }
    sqlx::query("UPDATE freight_storage_format SET version=22 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("future format fixture");
    let before = root_physical(&db.pool).await;
    assert_eq!(
        call(&router(&store), "viewer", &path, "future", None)
            .await
            .0,
        503
    );
    assert_eq!(before, root_physical(&db.pool).await);
    sqlx::query("UPDATE freight_storage_format SET version=21 WHERE singleton")
        .execute(&db.pool)
        .await
        .expect("exact format repair");
    let response = call(&router(&store), "viewer", &path, "repaired", None).await;
    assert_eq!(response.0, 200);
    assert_eq!(
        response.1["approval_evidence"],
        expected["approval_evidence"]
    );
    drop(store);
    db.clean().await;
}
async fn wait_for_approval_block_read(pool: &PgPool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%root=ANY%')").fetch_one(pool).await.expect("owned read observation");
        if waiting {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "Read never reached the blocked original query"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_approval_roots_hold_one_snapshot_and_admission_after_http_cancellation() {
    let db = DemoDatabase::new().await;
    let (store, id, expected) = approved_root_fixture(&db).await;
    let saved = store.read(TENANT).await.expect("saved");
    let path = format!("invoices/{id}/approval-evidence");
    let mut writer = db.pool.begin().await.expect("writer");
    sqlx::query("LOCK TABLE freight_workspace_blocks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *writer)
        .await
        .expect("owned barrier");
    let app = router(&store);
    let request_path = path.clone();
    let reader =
        tokio::spawn(async move { call(&app, "viewer", &request_path, "repeatable", None).await });
    wait_for_approval_block_read(&db.pool).await;
    let mut replacement = json!(domain::demo());
    replacement["_freight_format"] = json!(STORAGE_FORMAT);
    crate::storage_fixture::put_tx(&mut writer, TENANT, &replacement)
        .await
        .expect("new source generation");
    writer.commit().await.expect("replacement commit");
    let response = reader.await.expect("reader");
    assert_eq!(response.0, 200, "{response:?}");
    assert_eq!(
        response.1["approval_evidence"], expected["approval_evidence"],
        "Header and selected originals must remain in the old snapshot"
    );
    assert_eq!(
        call(&router(&store), "viewer", &path, "new generation", None)
            .await
            .0,
        404
    );
    crate::storage_fixture::put_workspace(&db.pool, TENANT, &saved)
        .await
        .expect("restore owned source");
    let before = root_physical(&db.pool).await;
    let slots = store.read_slots.available_permits();
    assert!(slots > 0);
    let mut barrier = db.pool.begin().await.expect("barrier");
    sqlx::query("LOCK TABLE freight_workspace_blocks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *barrier)
        .await
        .expect("hold originals");
    let app = router(&store);
    let reader = tokio::spawn(async move { call(&app, "viewer", &path, "cancelled", None).await });
    wait_for_approval_block_read(&db.pool).await;
    reader.abort();
    assert!(reader.await.expect_err("cancelled caller").is_cancelled());
    assert_eq!(
        store.read_slots.available_permits(),
        slots - 1,
        "An admitted SQL task retains its slot after its caller disappears"
    );
    wait_for_approval_block_read(&db.pool).await;
    barrier.rollback().await.expect("release original barrier");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while store.read_slots.available_permits() != slots {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("admitted job finishes and returns slot");
    assert_eq!(before, root_physical(&db.pool).await);
    drop(store);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_unverified_demo_seed_refuses_without_changing_existing_reviews_or_originals() {
    let db = DemoDatabase::new().await;
    let store = Store::new(Some(&db.url)).await.expect("store");
    crate::storage_fixture::put_workspace(&db.pool, "demo", &domain::demo())
        .await
        .expect("existing unverified data");
    let api = router_for_tenant(&store, "demo");
    let (id, revision) = current(&store, "demo").await;
    assert_eq!(
        call(
            &api,
            "maker",
            &format!("invoices/{id}/entry-review"),
            "review-before-demo",
            Some(review(&revision))
        )
        .await
        .0,
        200
    );
    let before = crate::storage_fixture::physical(&db.pool, "demo")
        .await
        .expect("before");
    assert_eq!(
        store.seed_demo().await.expect_err("must refuse takeover"),
        DEMO_PROVENANCE_REQUIRED
    );
    assert!(store.invoice_review_required("demo"));
    assert_eq!(
        call(
            &api,
            "maker-admin",
            &format!("invoices/{id}/approve"),
            "self-after-demo",
            Some(approval(&revision))
        )
        .await
        .0,
        403
    );
    let mode: bool =
        sqlx::query_scalar("SELECT synthetic_demo FROM freight_workspaces WHERE tenant_id='demo'")
            .fetch_one(&db.pool)
            .await
            .expect("mode");
    assert!(!mode);
    assert_eq!(
        crate::storage_fixture::physical(&db.pool, "demo")
            .await
            .expect("after"),
        before
    );
    drop(api);
    drop(store);
    db.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_actual_demo_provenance_survives_restart_and_is_rechecked_for_new_approvals() {
    let db = DemoDatabase::new().await;
    let a = Store::new(Some(&db.url)).await.expect("first");
    let b = Store::new(Some(&db.url)).await.expect("second");
    let (first, second) = tokio::join!(a.seed_demo(), b.seed_demo());
    first.expect("first seed");
    second.expect("concurrent seed");
    let before = crate::storage_fixture::physical(&db.pool, "demo")
        .await
        .expect("original");
    let restarted = Store::new(Some(&db.url)).await.expect("restarted");
    assert!(
        restarted.invoice_review_required("demo"),
        "Outside explicit demo mode, a prior synthetic seed still requires independent finance"
    );
    restarted
        .seed_demo()
        .await
        .expect("verified synthetic reopen");
    assert!(!restarted.invoice_review_required("demo"));
    assert_eq!(
        crate::storage_fixture::physical(&db.pool, "demo")
            .await
            .expect("same source"),
        before
    );
    let (id, _) = current(&restarted, "demo").await;
    let original = restarted
        .mutate_authorized(
            ("demo", "staff:demo"),
            "actual-demo-original",
            "invoice.approve",
            &id,
            json!({}),
        )
        .await
        .expect("one-person synthetic demonstration");
    sqlx::query("UPDATE freight_workspaces SET synthetic_demo=false WHERE tenant_id='demo'")
        .execute(&db.pool)
        .await
        .expect("owned provenance damage");
    let damaged = crate::storage_fixture::physical(&db.pool, "demo")
        .await
        .expect("after damage");
    let replay = restarted
        .mutate_authorized(
            ("demo", "staff:demo"),
            "actual-demo-original",
            "invoice.approve",
            &id,
            json!({}),
        )
        .await
        .expect("committed original remains authoritative");
    assert!(replay["replayed"] == true);
    assert_eq!(replay["data"], original["data"]);
    assert_eq!(
        crate::storage_fixture::physical(&db.pool, "demo")
            .await
            .expect("replay unchanged"),
        damaged
    );
    let workspace = restarted.read("demo").await.expect("read");
    let load = workspace
        .loads
        .iter()
        .find(|l| l.id == "load_7")
        .expect("delivered target");
    let next_invoice = workspace
        .invoices
        .iter()
        .find(|v| v.load_id == load.id)
        .expect("second original");
    let next = next_invoice.id.as_str();
    restarted.mutate_authorized(("demo","staff:demo"),"resolve-second-demo-invoice","invoice.resolve",next,json!({"expected_amount_cents":next_invoice.amount_cents,"amount_cents":load.buy_cents,"note":"Reviewed actual owned synthetic invoice correction against agreed carrier amount"})).await.expect("match second retained invoice");
    let before = crate::storage_fixture::physical(&db.pool, "demo")
        .await
        .expect("before refused approval");
    assert_eq!(
        restarted
            .mutate_authorized(
                ("demo", "staff:demo"),
                "new-after-provenance-damage",
                "invoice.approve",
                next,
                json!({})
            )
            .await
            .expect_err("cached demo flag is insufficient"),
        DEMO_PROVENANCE_REQUIRED
    );
    assert_eq!(
        crate::storage_fixture::physical(&db.pool, "demo")
            .await
            .expect("atomic refusal"),
        before
    );
    sqlx::query("UPDATE freight_workspaces SET synthetic_demo=true WHERE tenant_id='demo'")
        .execute(&db.pool)
        .await
        .expect("restore exact provenance");
    restarted
        .mutate_authorized(
            ("demo", "staff:demo"),
            "new-after-provenance-damage",
            "invoice.approve",
            next,
            json!({}),
        )
        .await
        .expect("failed key reuses only after proof restoration");
    crate::storage_fixture::put_workspace(&db.pool, "other-tenant", &domain::demo())
        .await
        .expect("other tenant");
    let error = sqlx::query(
        "UPDATE freight_workspaces SET synthetic_demo=true WHERE tenant_id='other-tenant'",
    )
    .execute(&db.pool)
    .await
    .expect_err("scope guard");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23514")
    );
    drop(restarted);
    drop(a);
    drop(b);
    db.clean().await;
}

#[tokio::test]
async fn reviewed_save_resolution_binds_exact_revision_and_body_and_stops_late_approval() {
    let store = fixture().await;
    let router = router(&store);
    let (id, revision) = current(&store, TENANT).await;
    let path = format!("invoices/{id}/approve");
    let body = approval(&revision);
    let resolved=call(&router,"checker","save-recovery/resolve","resolve",Some(json!({"request_key":"late-approval","path":path,"body":body,"note":"Original independent approval was abandoned before application"}))).await;
    assert_eq!(resolved.0, 200, "{resolved:?}");
    let response = call(&router, "checker", &path, "late-approval", Some(body)).await;
    assert_eq!(response.0, 409, "{response:?}");
    assert_eq!(response.1["error"]["code"], "request_stopped");
    let conflict = call(
        &router,
        "checker",
        &path,
        "late-approval",
        Some(approval("another-revision")),
    )
    .await;
    assert_eq!(conflict.0, 409);
    let w = store.read(TENANT).await.expect("unposted");
    assert!(w.journal.is_empty());
    assert_eq!(w.invoices[0].status, "matched");
}

#[tokio::test]
async fn canonical_invoice_revision_requires_fresh_review_and_excludes_its_classifying_actor() {
    let store = fixture().await;
    let source = store.read(TENANT).await.expect("source assignment");
    let load = source
        .loads
        .iter()
        .find(|v| v.id == "load_6")
        .expect("load");
    let carrier = source
        .carriers
        .iter()
        .find(|v| Some(&v.id) == load.carrier_id.as_ref())
        .expect("assigned carrier");
    let (id, original_revision) = current(&store, TENANT).await;
    apply(
        &store,
        "staff:maker",
        "original-review",
        "invoice.entry_review",
        &id,
        review(&original_revision),
    )
    .await;
    apply(&store,"stateset-edi","edi-invoice-revision","edi.event","",json!({"event_id":"actual-replacement-210","event_type":"edi.freight_invoice.received","occurred_at":chrono::Utc::now(),"payload":{"shipment_id":load.reference,"partner_id":carrier.partner_id,"carrier_scac":carrier.scac,"invoice_number":"ACTUAL-REPLACEMENT-210","net_amount":"1180.00","currency":"USD"}})).await;
    let w = store.read(TENANT).await.expect("canonical document");
    let document = w.carrier_documents.last().expect("additional document");
    let load = w.loads.iter().find(|v| v.id == "load_6").expect("load");
    apply(&store,"staff:classifier","apply-invoice-revision","payable.document.apply_revision",&document.id,json!({"expected_version":document.version,"expected_load_version":load.version,"note":"Verified actual replacement carrier invoice against signed source"})).await;
    let (_, revision) = current(&store, TENANT).await;
    assert_ne!(original_revision, revision);
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&original_revision),
        "changed;",
    )
    .await;
    apply(
        &store,
        "staff:maker",
        "new-source-review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    unchanged_failure(
        &store,
        "staff:classifier",
        "invoice.approve",
        &id,
        approval(&revision),
        "different named",
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "independent-approval",
        "invoice.approve",
        &id,
        approval(&revision),
    )
    .await;
}

#[tokio::test]
async fn malformed_review_evidence_and_forged_subjects_are_rejected_without_posting() {
    let store = fixture().await;
    let (id, revision) = current(&store, TENANT).await;
    for body in [
        json!({}),
        json!({"expected_revision":revision,"note":"short","evidence_reference":"short"}),
        json!({"expected_revision":revision,"note":"Actual source inspected","evidence_reference":"ACTUAL-INV-1180","actor":"staff:checker"}),
    ] {
        unchanged_failure(
            &store,
            "staff:maker",
            "invoice.entry_review",
            &id,
            body,
            "review",
        )
        .await;
    }
    apply(
        &store,
        "staff:maker",
        "entry-review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    let mut guard = store.memory.write().await;
    let w = guard.get_mut(TENANT).expect("owned corrupt fixture");
    w.events
        .iter_mut()
        .find(|e| e.kind == "invoice.entry_reviewed")
        .expect("review")
        .details
        .as_mut()
        .expect("details")["note"] = Value::Null;
    drop(guard);
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&revision),
        "entry review",
    )
    .await;
}

#[tokio::test]
async fn original_legacy_approval_receipt_replays_without_reinterpreting_historical_decision() {
    let store = fixture().await;
    let (id, _) = current(&store, TENANT).await;
    let mut guard = store.memory.write().await;
    let w = guard.get_mut(TENANT).expect("fixture");
    let original = w
        .apply("invoice.approve", &id, json!({}))
        .expect("historical decision");
    w.receipts.insert(
        "historical-approval".into(),
        Receipt {
            fingerprint: fingerprint("invoice.approve", &id, &json!({})),
            response: original.clone(),
            stopped: None,
        },
    );
    drop(guard);
    let before = json!(store.read(TENANT).await.expect("before"));
    let replay = apply(
        &store,
        "credential:historical-key",
        "historical-approval",
        "invoice.approve",
        &id,
        json!({}),
    )
    .await;
    assert_eq!(replay["data"], original);
    assert_eq!(replay["replayed"], true);
    assert_eq!(before, json!(store.read(TENANT).await.expect("after")));
}

#[tokio::test]
async fn stored_pod_review_changes_invalidate_invoice_review_even_without_a_load_version_change() {
    let store = fixture().await;
    let (id, _) = current(&store, TENANT).await;
    let document = store
        .upload_document(
            (TENANT, "staff:maker"),
            "actual-pod",
            "load_6",
            crate::documents::Upload {
                kind: "pod".into(),
                filename: "receiver.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: b"%PDF-1.4\nOwned receiver evidence\n%%EOF\n".to_vec(),
            },
        )
        .await
        .expect("owned POD")["data"]
        .clone();
    let w = store.read(TENANT).await.expect("source");
    let load = w.loads.iter().find(|v| v.id == "load_6").expect("load");
    apply(
        &store,
        "staff:maker",
        "attach-pod",
        "load.command",
        "load_6",
        json!({"action":"attach_pod","expected_version":load.version,"document_id":document["id"]}),
    )
    .await;
    let doc_id = document["id"].as_str().expect("document");
    apply(&store,"staff:maker","inspect-pod","document.review",doc_id,json!({"expected_version":1,"decision":"accepted","note":"Inspected actual receiver proof of delivery"})).await;
    let (_, revision) = current(&store, TENANT).await;
    apply(
        &store,
        "staff:maker",
        "entry-review",
        "invoice.entry_review",
        &id,
        review(&revision),
    )
    .await;
    let before = store.read(TENANT).await.expect("reviewed").loads[5].version;
    apply(&store,"staff:maker","reinspect-pod","document.review",doc_id,json!({"expected_version":2,"decision":"accepted","note":"Recorded a new inspection of the receiver evidence"})).await;
    assert_eq!(
        store.read(TENANT).await.expect("new review").loads[5].version,
        before
    );
    unchanged_failure(
        &store,
        "staff:checker",
        "invoice.approve",
        &id,
        approval(&revision),
        "changed;",
    )
    .await;
    let (_, fresh) = current(&store, TENANT).await;
    assert_ne!(revision, fresh);
    apply(
        &store,
        "staff:maker",
        "fresh-entry-review",
        "invoice.entry_review",
        &id,
        review(&fresh),
    )
    .await;
    apply(
        &store,
        "staff:checker",
        "approval",
        "invoice.approve",
        &id,
        approval(&fresh),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn postgres_independent_invoice_approval_serializes_competing_checkers_and_preserves_original_replay(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("owned PostgreSQL");
    let admin = PgPool::connect(&base).await.expect("admin");
    let schema = format!("invoice_review_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("owned schema");
    let mut url = reqwest::Url::parse(&base).expect("URL");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"));
    let first = Store::new(Some(url.as_str()))
        .await
        .expect("first instance");
    let second = Store::new(Some(url.as_str()))
        .await
        .expect("second instance");
    let pool = PgPool::connect(url.as_str()).await.expect("observer");
    // Historical originals without named entry provenance need an explicit
    // current review, including a real tenant whose name happens to be demo.
    crate::storage_fixture::put_workspace(&pool, "demo", &domain::demo())
        .await
        .expect("legacy source");
    let (id, revision) = current(&first, "demo").await;
    assert!(first.invoice_review_required("demo"));
    first
        .mutate_authorized(
            ("demo", "staff:maker"),
            "entry-review",
            "invoice.entry_review",
            &id,
            review(&revision),
        )
        .await
        .expect("entry review");
    let (a, b) = tokio::join!(
        first.mutate_authorized(
            ("demo", "staff:checker-a"),
            "approval-a",
            "invoice.approve",
            &id,
            approval(&revision)
        ),
        second.mutate_authorized(
            ("demo", "staff:checker-b"),
            "approval-b",
            "invoice.approve",
            &id,
            approval(&revision)
        )
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "{a:?} {b:?}"
    );
    let (key, approved) = if let Ok(value) = a {
        ("approval-a", value)
    } else {
        ("approval-b", b.expect("winner"))
    };
    let before:(Value,String,String,String)=sqlx::query_as("SELECT freight_workspace_originals(tenant_id),updated_at::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&pool).await.expect("original");
    let replay = second
        .mutate_authorized(
            ("demo", "staff:maker"),
            key,
            "invoice.approve",
            &id,
            approval(&revision),
        )
        .await
        .expect("original receipt wins");
    assert_eq!(replay["data"], approved["data"]);
    assert_eq!(replay["replayed"], true);
    let after:(Value,String,String,String)=sqlx::query_as("SELECT freight_workspace_originals(tenant_id),updated_at::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&pool).await.expect("unchanged");
    assert_eq!(before, after);
    let saved = second.read("demo").await.expect("durable");
    assert_eq!(saved.journal.len(), 1);
    assert_eq!(
        saved
            .events
            .iter()
            .filter(|v| v.kind == "invoice.approve")
            .count(),
        1
    );
    assert!(saved
        .events
        .iter()
        .any(|e| e.kind == "invoice.received" && e.actor == "system"));
    drop(first);
    drop(second);
    let reconnected = Store::new(Some(url.as_str())).await.expect("fresh Store");
    let router = router_for_tenant(&reconnected, "demo");
    for path in ["workspace", "workspace?history=deferred"] {
        let response = call(&router, "viewer", path, "retained-approval-audit", None).await;
        assert_eq!(response.0, 200, "{response:?}");
        let evidence = &response.1["invoices"][0]["approval_control"]["approval_evidence"];
        assert_eq!(evidence["revision"], revision);
        assert_eq!(evidence["entry_reviewed_by"], "staff:maker");
        assert_eq!(
            evidence["approved_by"],
            saved
                .events
                .iter()
                .find(|e| e.kind == "invoice.approve")
                .expect("decision")
                .actor
        );
    }
    let after_reads:(Value,String,String,String)=sqlx::query_as("SELECT freight_workspace_originals(tenant_id),updated_at::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&pool).await.expect("read-only audit");
    let detail = call(
        &router,
        "viewer",
        &format!("invoices/{id}/approval-evidence"),
        "retained-detail",
        None,
    )
    .await;
    assert_eq!(detail.0, 200, "{detail:?}");
    assert_eq!(detail.1["approval_evidence"]["revision"], revision);
    assert_eq!(
        detail.1["approval_evidence"]["entry_reviewed_by"],
        "staff:maker"
    );
    let after_detail:(Value,String,String,String)=sqlx::query_as("SELECT freight_workspace_originals(tenant_id),updated_at::text,xmin::text,ctid::text FROM freight_workspaces WHERE tenant_id='demo'").fetch_one(&pool).await.expect("selected read-only audit");
    assert_eq!(after_reads, after_detail);
    assert_eq!(before, after_reads);
    drop(reconnected);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("owned cleanup");
    admin.close().await;
}
