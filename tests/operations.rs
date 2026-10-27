#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Status, Workspace},
    integrations::Integrations,
    operations::QueueQuery,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;

async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
fn principal(subject: &str) -> Principal {
    Principal {
        tenant_id: "demo".into(),
        subject: subject.into(),
        role: Role::Operations,
        counterparty_id: None,
    }
}
fn issue(w: &Workspace, load: &str, kind: &str) -> Value {
    w.operations_board(Utc::now())["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .find(|r| r["signal"]["load_id"] == load && r["signal"]["kind"] == kind)
        .expect("issue")
        .clone()
}
fn input(row: &Value, action: &str) -> Value {
    json!({"expected_version":row["case"]["version"].as_u64().unwrap_or(0),"expected_source_active":row["source_active"],"source_sha256":row["source_sha256"],"action":action,"note":"Reviewed actual source and next operational step","follow_up_at":if action=="claim"{json!(Utc::now()+Duration::minutes(90))}else{Value::Null},"defer_until":if action=="snooze"{json!(Utc::now()+Duration::minutes(15))}else{Value::Null}})
}
fn trusted(actor: &str, body: Value) -> Value {
    json!({"subject":actor,"is_admin":actor=="user:admin","request":body})
}

#[tokio::test]
async fn corrupt_cleared_source_identity_cannot_readdress_retained_history_or_restore_trust() {
    let s = setup().await;
    let original = issue(&read(&s).await, "load_8", "shipment_exception");
    let id = original["id"].as_str().expect("case id");
    exception(&s, None).await;
    let mut w = read(&s).await;
    let c = w
        .operations_cases
        .iter_mut()
        .find(|c| c.id == id)
        .expect("retained case");
    c.source.id = "untrusted-replacement-address".into();
    assert!(w.verify_operations_cases().is_err());
    let before = json!(w);
    let detail = w
        .operations_case(id, Utc::now())
        .expect("original case address");
    assert_eq!(detail["id"], id);
    assert_eq!(detail["case"]["id"], id);
    assert_eq!(detail["source_active"], false);
    assert_eq!(detail["evidence_valid"], false);
    assert!(detail["owner"].is_null());
    assert!(w
        .operations_case("untrusted-replacement-address", Utc::now())
        .is_err());
    let page = w
        .operations_page(
            QueueQuery {
                view: Some("resolved".into()),
                ..Default::default()
            },
            &principal("user:ops1"),
            Utc::now(),
        )
        .expect("history index");
    let row = page["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .find(|r| r["id"] == id)
        .expect("original address remains discoverable");
    assert!(row["case"].is_null());
    assert_eq!(row["evidence_valid"], false);
    assert_eq!(
        json!(w),
        before,
        "reads cannot repair or discard damaged evidence"
    );
}
async fn action(
    s: &Store,
    actor: &str,
    key: &str,
    row: &Value,
    body: Value,
) -> Result<Value, String> {
    s.mutate_authorized(
        ("demo", actor),
        key,
        "operations.case.action",
        row["id"].as_str().expect("id"),
        trusted(actor, body),
    )
    .await
}
async fn exception(s: &Store, reason: Option<&str>) {
    let w = read(s).await;
    let l = w.loads.iter().find(|l| l.id == "load_8").expect("load");
    s.mutate_authorized(("demo","user:ops1"),&domain::id("key"),"load.command","load_8",json!({"action":if reason.is_some(){"exception"}else{"resolve"},"expected_version":l.version,"note":reason})).await.expect("source mutation");
}
async fn setup() -> Store {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    exception(
        &s,
        Some("Actual weather closure requires carrier coordination"),
    )
    .await;
    s
}
fn business(w: &Workspace) -> Value {
    json!({"loads":w.loads,"tenders":w.tenders,"rates":w.rate_confirmations,"sources":w.source_checks,"reviews":w.source_reviews,"invoices":w.invoices,"bills":w.bills,"settlements":w.settlements,"cash":w.cash_entries,"journal":w.journal,"outbox":w.outbox,"submissions":w.carrier_submissions,"documents":w.documents})
}
async fn rejects(s: &Store, row: &Value, body: Value, actor: &str, contains: &str) {
    let before = json!(read(s).await);
    let error = action(s, actor, &domain::id("failed"), row, body)
        .await
        .expect_err("reject");
    assert!(error.contains(contains), "{error}");
    assert_eq!(json!(read(s).await), before);
}
fn router(s: Store) -> Router {
    let keys = [
        ("ops1", Role::Operations, None),
        ("ops2", Role::Operations, None),
        ("admin", Role::Admin, None),
        ("finance", Role::Finance, None),
        ("viewer", Role::Viewer, None),
        ("carrier", Role::Carrier, Some("ca4")),
        ("customer", Role::Customer, Some("c1")),
    ]
    .into_iter()
    .map(|(k, role, cp)| {
        (
            k.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("user:{k}"),
                role,
                counterparty_id: cp.map(str::to_owned),
            }),
        )
    })
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        store: s,
        identity: Arc::new(None),
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(
    r: &Router,
    who: &str,
    method: &str,
    path: &str,
    body: Value,
    key: &str,
) -> (u16, Value) {
    let response = r
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {who}"))
                .header("idempotency-key", key)
                .header("content-type", "application/json")
                .header("x-freight-subject", "forged-admin")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}

#[test]
fn source_detection_uses_actual_tracking_and_stop_evidence_without_inventing_eta_or_pod() {
    let mut w = domain::demo();
    w.loads.retain(|l| l.id == "load_1");
    let now = Utc::now();
    w.loads[0].exception = None;
    w.loads[0].last_tracking_at = Some(now - Duration::minutes(300));
    let before = business(&w);
    let signals = w.operations_signals(now);
    let stale = signals
        .iter()
        .find(|s| s.kind == "tracking_stale")
        .expect("stale");
    assert_eq!(
        stale.source["last_tracking_at"],
        json!(now - Duration::minutes(300))
    );
    assert_eq!(stale.deadline, Some(now - Duration::minutes(60)));
    assert!(!w
        .operations_signals(now - Duration::minutes(61))
        .iter()
        .any(|s| s.kind == "tracking_stale"));
    assert_eq!(business(&w), before);
    w.loads[0].status = Status::Delivered;
    w.loads[0].pod_reference = None;
    w.loads[0].pod_review = None;
    let stop = w.loads[0].destination.clone();
    w.loads[0]
        .intermediate_stops
        .push(stateset_freight::itinerary::IntermediateStop {
            id: "stop_actual_receiver".into(),
            kind: "delivery".into(),
            stop,
        });
    let before = business(&w);
    let signals = w.operations_signals(now);
    assert!(signals
        .iter()
        .any(|s| s.kind == "incomplete_route" && s.priority == 0));
    assert!(signals
        .iter()
        .any(|s| s.kind == "pod_needed" && s.priority == 2));
    assert!(!signals.iter().any(|s| s.kind == "tracking_stale"));
    assert_eq!(business(&w), before);
    w.loads[0].status = Status::Cancelled;
    assert!(!w.operations_signals(now).iter().any(|s| [
        "tracking_stale",
        "pod_needed",
        "incomplete_route"
    ]
    .contains(&s.kind.as_str())));
}

#[tokio::test]
async fn named_ownership_followups_deferral_and_replay_preserve_business_state_and_failed_keys() {
    let s = setup().await;
    let original = read(&s).await;
    let row = issue(&original, "load_8", "shipment_exception");
    let body = input(&row, "claim");
    let claimed = action(&s, "user:ops1", "first-claim", &row, body.clone())
        .await
        .expect("claim");
    assert_eq!(claimed["data"]["owner"], "user:ops1");
    assert_eq!(
        claimed["data"]["history"]
            .as_array()
            .expect("history")
            .last()
            .expect("event")["actor"],
        "user:ops1"
    );
    assert_eq!(business(&read(&s).await), business(&original));
    let current = issue(&read(&s).await, "load_8", "shipment_exception");
    rejects(
        &s,
        &current,
        input(&current, "claim"),
        "user:ops2",
        "already has an owner",
    )
    .await;
    rejects(
        &s,
        &current,
        input(&current, "release"),
        "user:ops2",
        "current owner",
    )
    .await;
    rejects(
        &s,
        &current,
        input(&current, "resolve"),
        "user:ops1",
        "underlying source",
    )
    .await;
    let deferred = action(
        &s,
        "user:ops1",
        "defer",
        &current,
        input(&current, "snooze"),
    )
    .await
    .expect("defer");
    assert!(!deferred["data"]["deferred_until"].is_null());
    assert_eq!(
        issue(&read(&s).await, "load_8", "shipment_exception")["deferred"],
        true
    );
    let current = issue(&read(&s).await, "load_8", "shipment_exception");
    let mut bad = input(&current, "snooze");
    bad["defer_until"] = json!(Utc::now() + Duration::days(2));
    rejects(&s, &current, bad, "user:ops1", "within 24 hours").await;
    let original_state = json!(read(&s).await);
    let replay = action(&s, "user:ops1", "first-claim", &row, body)
        .await
        .expect("original receipt");
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["data"], claimed["data"]);
    assert_eq!(json!(read(&s).await), original_state);
    let mut expired = read(&s).await;
    let at = Utc::now() + Duration::minutes(16);
    assert!(expired.reconcile_operations(at) > 0);
    let expired_row = issue(&expired, "load_8", "shipment_exception");
    assert!(expired_row["case"]["deferred_until"].is_null());
    assert_eq!(expired_row["case"]["owner"], "user:ops1");
    assert!(!expired.operations_need_reconciliation(at));
}

#[tokio::test]
async fn actual_source_resolution_and_recurrence_retain_cycles_and_first_response_denominators() {
    let s = setup().await;
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    action(&s, "user:ops1", "claim-cycle-1", &row, input(&row, "claim"))
        .await
        .expect("claim");
    let before = read(&s).await;
    let first = before
        .operations_cases
        .iter()
        .find(|c| c.id == row["id"])
        .expect("case")
        .clone();
    exception(&s, None).await;
    let cleared = read(&s).await;
    let old = cleared
        .operations_cases
        .iter()
        .find(|c| c.id == first.id)
        .expect("old");
    assert_eq!(old.status, "resolved");
    assert_eq!(old.owner.as_deref(), Some("user:ops1"));
    assert_eq!(
        json!(&old.history[..first.history.len()]),
        json!(first.history)
    );
    assert_eq!(old.history.last().expect("clear").action, "source_cleared");
    exception(
        &s,
        Some("Actual second facility disruption requires new handling"),
    )
    .await;
    let current = read(&s).await;
    let reopened = issue(&current, "load_8", "shipment_exception");
    assert_eq!(reopened["id"], row["id"]);
    assert_eq!(reopened["case"]["cycle"], 2);
    assert!(reopened["owner"].is_null());
    assert!(reopened["case"]["follow_up_at"].is_null());
    assert_eq!(
        current
            .operations_cases
            .iter()
            .find(|c| c.id == first.id)
            .expect("case")
            .history
            .last()
            .expect("event")
            .action,
        "reopened"
    );
    let metrics = &current.operations_board(Utc::now())["metrics"];
    assert!(
        metrics["detected_cycles"].as_u64().expect("cycles")
            > metrics["acknowledged_cycles"].as_u64().expect("claimed")
    );
    assert!(!metrics["median_first_response_seconds"].is_null());
    let before = json!(current);
    let mut stale = input(&row, "claim");
    stale["expected_version"] = reopened["case"]["version"].clone();
    rejects(&s, &reopened, stale, "user:ops2", "source changed").await;
    assert_eq!(json!(read(&s).await), before);
}

#[tokio::test]
async fn http_policy_permissions_actor_binding_and_partner_privacy_are_enforced_before_replay() {
    let s = setup().await;
    let r = router(s.clone());
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    let path = format!(
        "operations/issues/{}/actions",
        row["id"].as_str().expect("id")
    );
    let body = input(&row, "claim");
    for who in ["viewer", "finance", "carrier", "customer"] {
        let before = json!(read(&s).await);
        assert_eq!(
            call(&r, who, "POST", &path, body.clone(), "forbidden")
                .await
                .0,
            403
        );
        assert_eq!(json!(read(&s).await), before);
    }
    let mut forged = body.clone();
    forged["is_admin"] = json!(true);
    assert_eq!(
        call(&r, "ops1", "POST", &path, forged, "forged").await.0,
        422
    );
    let (status, result) = call(&r, "ops1", "POST", &path, body.clone(), "same-logical-key").await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["data"]["owner"], "user:ops1");
    assert_eq!(
        call(&r, "ops2", "POST", &path, body, "same-logical-key")
            .await
            .0,
        409,
        "another subject cannot replay a human assertion"
    );
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    assert_eq!(
        call(
            &r,
            "ops2",
            "POST",
            &path,
            input(&row, "force_release"),
            "unauthorized-override"
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &r,
            "admin",
            "POST",
            &path,
            input(&row, "force_release"),
            "actual-admin"
        )
        .await
        .0,
        200
    );
    let policy = json!({"expected_version":0,"tracking_stale_minutes":120,"note":"Actual operating follow-up agreement"});
    assert_eq!(
        call(
            &r,
            "ops1",
            "POST",
            "operations/policy",
            policy.clone(),
            "policy-role"
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &r,
            "admin",
            "POST",
            "operations/policy",
            policy,
            "policy-first"
        )
        .await
        .0,
        200
    );
    let before = json!(read(&s).await);
    assert_eq!(
        call(
            &r,
            "admin",
            "POST",
            "operations/policy",
            json!({"expected_version":0,"tracking_stale_minutes":60,"note":"Stale setting review"}),
            "policy-stale"
        )
        .await
        .0,
        409
    );
    assert_eq!(json!(read(&s).await), before);
    let detail = format!("operations/issues/{}", row["id"].as_str().expect("id"));
    for who in ["carrier", "customer"] {
        assert_eq!(
            call(&r, who, "GET", &detail, Value::Null, "read-denied")
                .await
                .0,
            403
        );
        let (status, view) = call(&r, who, "GET", "portal", Value::Null, "portal").await;
        assert_eq!(status, 200, "{view}");
        assert!(view.get("operations").is_none());
        assert!(!view
            .to_string()
            .contains("Actual operating follow-up agreement"));
    }
    let (status, view) = call(&r, "viewer", "GET", &detail, Value::Null, "read-history").await;
    assert_eq!(status, 200, "{view}");
    assert!(view["case"]["history"]
        .as_array()
        .expect("history")
        .iter()
        .any(|e| e["action"] == "force_release" && e["actor"] == "user:admin"));
}

#[test]
fn queue_pagination_binds_scope_filters_and_order_without_returning_full_history() {
    let mut w = domain::demo();
    let template = w.loads[7].clone();
    w.loads.clear();
    for i in 0..57 {
        let mut l = template.clone();
        l.id = format!("queue-{i}");
        l.reference = format!("QUEUE-{i:03}");
        l.exception = Some("Actual queue qualification issue".into());
        w.loads.push(l);
    }
    w.reconcile_operations(Utc::now());
    let p = principal("user:ops1");
    let first = w
        .operations_page(
            QueueQuery {
                limit: Some(25),
                ..Default::default()
            },
            &p,
            Utc::now(),
        )
        .expect("page");
    assert_eq!(first["total"], 57);
    assert_eq!(first["issues"].as_array().expect("issues").len(), 25);
    assert!(first["issues"][0]["case"].get("history").is_none());
    let cursor = first["next_cursor"].as_str().expect("cursor").to_string();
    let next = w
        .operations_page(
            QueueQuery {
                limit: Some(25),
                cursor: Some(cursor.clone()),
                ..Default::default()
            },
            &p,
            Utc::now(),
        )
        .expect("next");
    let ids = first["issues"]
        .as_array()
        .expect("first")
        .iter()
        .map(|r| r["id"].clone())
        .collect::<Vec<_>>();
    assert!(next["issues"]
        .as_array()
        .expect("second")
        .iter()
        .all(|r| !ids.contains(&r["id"])));
    let mut other = p.clone();
    other.tenant_id = "different-tenant".into();
    assert!(w
        .operations_page(
            QueueQuery {
                limit: Some(25),
                cursor: Some(cursor.clone()),
                ..Default::default()
            },
            &other,
            Utc::now()
        )
        .expect_err("tenant-bound cursor")
        .contains("Queue changed"));
    assert!(w
        .operations_page(
            QueueQuery {
                limit: Some(25),
                view: Some("mine".into()),
                cursor: Some(cursor.clone()),
                ..Default::default()
            },
            &p,
            Utc::now()
        )
        .is_err());
    let filtered = w
        .operations_page(
            QueueQuery {
                search: Some("QUEUE-001".into()),
                ..Default::default()
            },
            &p,
            Utc::now(),
        )
        .expect("search");
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["issues"][0]["load_reference"], "QUEUE-001");
    w.loads[0].exception = None;
    assert!(w
        .operations_page(
            QueueQuery {
                limit: Some(25),
                cursor: Some(cursor),
                ..Default::default()
            },
            &p,
            Utc::now()
        )
        .is_err());
    for limit in [0, 101] {
        assert!(w
            .operations_page(
                QueueQuery {
                    limit: Some(limit),
                    ..Default::default()
                },
                &p,
                Utc::now()
            )
            .is_err());
    }
}

#[test]
fn critical_holds_retry_followthrough_and_corrupt_index_do_not_rewrite_physical_truth() {
    let mut w = domain::demo();
    let load = w.loads[5].id.clone();
    let stop = w.loads[5].destination.clone();
    w.loads[5]
        .intermediate_stops
        .push(stateset_freight::itinerary::IntermediateStop {
            id: "missing_actual_stop".into(),
            kind: "delivery".into(),
            stop,
        });
    w.reconcile_operations(Utc::now());
    let row = issue(&w, &load, "incomplete_route");
    let before = business(&w);
    let claim = trusted("user:ops1", input(&row, "claim"));
    w.apply_operations(
        "operations.case.action",
        row["id"].as_str().expect("id"),
        claim,
        "user:ops1",
    )
    .expect("critical ownership");
    let row = issue(&w, &load, "incomplete_route");
    let mut candidate = w.clone();
    assert!(candidate
        .apply_operations(
            "operations.case.action",
            row["id"].as_str().expect("id"),
            trusted("user:ops1", input(&row, "snooze")),
            "user:ops1"
        )
        .expect_err("critical")
        .contains("cannot be deferred"));
    assert_eq!(business(&w), before);
    let job:domain::Outbox=serde_json::from_value(json!({"id":"held-real-job","load_id":load,"destination":"stateset-edi","status":"held","method":"POST","path":"/actual/fixture","payload":{},"total_attempts":8})).expect("job");
    w.outbox.push(job);
    w.reconcile_operations(Utc::now());
    let id = issue(&w, &load, "outbound_failure")["id"].clone();
    w.outbox.last_mut().expect("job").status = "sending".into();
    w.reconcile_operations(Utc::now());
    assert_eq!(
        w.operations_cases
            .iter()
            .find(|c| c.id == id)
            .expect("retry case")
            .status,
        "open"
    );
    w.outbox.last_mut().expect("job").status = "gateway_accepted".into();
    w.reconcile_operations(Utc::now());
    assert_eq!(
        w.operations_cases
            .iter()
            .find(|c| c.id == id)
            .expect("recovered condition")
            .status,
        "resolved"
    );
    w.operations_cases[0].history[0]
        .note
        .push_str(" CORRUPTION");
    assert!(w.verify_operations_cases().is_err());
    assert_eq!(w.operations_board(Utc::now())["evidence_valid"], false);
    let case_records = json!(w.operations_cases);
    assert_eq!(w.reconcile_operations(Utc::now()), 0);
    assert_eq!(json!(w.operations_cases), case_records);
    let id = "load_8";
    let version = w.loads.iter().find(|l| l.id == id).expect("load").version;
    w.apply("load.command",id,json!({"action":"exception","expected_version":version,"note":"Actual physical operating report still retained"})).expect("corrupt work index cannot block source truth");
    assert_eq!(
        w.loads
            .iter()
            .find(|l| l.id == id)
            .expect("load")
            .exception
            .as_deref(),
        Some("Actual physical operating report still retained")
    );
}

fn integration_failure_fixture(outbound: bool, destination: &str) -> Workspace {
    let mut w = domain::demo();
    if outbound {
        w.outbox.push(serde_json::from_value(json!({
            "id":"first-failure-job", "load_id":"load_5", "destination":destination,
            "status":"retry", "method":"POST", "path":"/fixture-only", "payload":{"original":"keep"},
            "attempts":1, "total_attempts":1, "last_error":"Upstream returned HTTP 503",
            "next_attempt_at":Utc::now()+Duration::seconds(10)
        })).expect("outbound fixture"));
    } else {
        w.inbox.push(serde_json::from_value(json!({
            "id":"first-failure-job", "document_id":"first-failure-document", "delivery_ids":["original-delivery"],
            "key_version":"v1", "notification":{"document_id":"first-failure-document"},
            "original_body":"{\"original\":true}", "fingerprint":"retained-fixture-fingerprint",
            "status":"retry", "attempts":1, "total_attempts":1, "history":[],
            "received_at":Utc::now(), "updated_at":Utc::now(),
            "last_error":"EDI document returned HTTP 503", "next_attempt_at":Utc::now()+Duration::seconds(10)
        })).expect("inbound fixture"));
    }
    w
}

fn set_failure_state(w: &mut Workspace, outbound: bool, status: &str, error: Option<&str>) {
    if outbound {
        let job = w.outbox.last_mut().expect("outbound");
        job.status = status.into();
        job.last_error = error.map(str::to_owned);
    } else {
        let job = w.inbox.last_mut().expect("inbound");
        job.status = status.into();
        job.last_error = error.map(str::to_owned);
    }
}

#[test]
fn first_failure_detection_requires_current_recovery_evidence_and_preserves_originals() {
    for (outbound, destination, active, terminal) in [
        (
            true,
            "stateset-edi",
            vec!["pending_handoff", "sending", "retry"],
            vec!["gateway_accepted", "superseded", "native_only"],
        ),
        (
            true,
            "stateset-sync-server",
            vec!["pending_handoff", "sending", "retry"],
            vec!["synced", "superseded", "local_only"],
        ),
        (
            false,
            "stateset-edi",
            vec!["pending", "processing", "retry"],
            vec![
                "applied",
                "ignored",
                "finance_review",
                "reconciliation_required",
            ],
        ),
    ] {
        for status in active
            .iter()
            .chain(terminal.iter())
            .chain(std::iter::once(&"held"))
        {
            for error in [None, Some("retained first failure")] {
                let mut w = integration_failure_fixture(outbound, destination);
                set_failure_state(&mut w, outbound, status, error);
                let original = json!({"business":business(&w),"inbox":w.inbox});
                w.reconcile_operations(Utc::now());
                let detected = w
                    .operations_cases
                    .iter()
                    .any(|case| case.source.subject_id == "first-failure-job");
                assert_eq!(
                    detected,
                    *status == "held" || active.contains(status) && error.is_some(),
                    "{destination} {status} {error:?}"
                );
                assert_eq!(json!({"business":business(&w),"inbox":w.inbox}), original);
                w.verify_operations_cases().expect("valid retained cases");
            }
        }
    }
}

#[test]
fn first_failure_ownership_survives_retries_and_clears_only_with_source_recovery() {
    for (outbound, destination, pending, processing, terminal) in [
        (
            true,
            "stateset-edi",
            "pending_handoff",
            "sending",
            "gateway_accepted",
        ),
        (
            true,
            "stateset-sync-server",
            "pending_handoff",
            "sending",
            "synced",
        ),
        (false, "stateset-edi", "pending", "processing", "applied"),
    ] {
        let mut w = integration_failure_fixture(outbound, destination);
        w.reconcile_operations(Utc::now());
        let id = w
            .operations_cases
            .iter()
            .find(|case| case.source.subject_id == "first-failure-job")
            .expect("first failure")
            .id
            .clone();
        let row = w.operations_case(&id, Utc::now()).expect("row");
        w.apply_operations(
            "operations.case.action",
            &id,
            trusted("user:ops1", input(&row, "claim")),
            "user:ops1",
        )
        .expect("claim first failure");
        let owned = w
            .operations_cases
            .iter()
            .find(|c| c.id == id)
            .expect("owned")
            .clone();
        for status in [pending, processing, "retry"] {
            set_failure_state(&mut w, outbound, status, None);
            let original = json!({"business":business(&w),"inbox":w.inbox});
            w.reconcile_operations(Utc::now());
            let current = w
                .operations_cases
                .iter()
                .find(|c| c.id == id)
                .expect("same case");
            assert_eq!(current.status, "working");
            assert_eq!(current.owner, owned.owner);
            assert_eq!(current.follow_up_at, owned.follow_up_at);
            assert_eq!(current.first_detected_at, owned.first_detected_at);
            assert_eq!(current.cycle, 1);
            assert_eq!(json!({"business":business(&w),"inbox":w.inbox}), original);
        }
        set_failure_state(&mut w, outbound, terminal, None);
        w.reconcile_operations(Utc::now());
        assert_eq!(
            w.operations_cases
                .iter()
                .find(|c| c.id == id)
                .expect("history")
                .status,
            "resolved"
        );
        set_failure_state(&mut w, outbound, pending, None);
        w.reconcile_operations(Utc::now());
        assert_eq!(
            w.operations_cases
                .iter()
                .find(|c| c.id == id)
                .expect("no invented recurrence")
                .status,
            "resolved"
        );
        set_failure_state(&mut w, outbound, "retry", Some("new retained failure"));
        w.reconcile_operations(Utc::now());
        let current = w
            .operations_cases
            .iter()
            .find(|c| c.id == id)
            .expect("recurrence");
        assert_eq!(current.cycle, 2);
        assert_eq!(current.status, "open");
        assert!(current.owner.is_none() && current.follow_up_at.is_none());
        w.verify_operations_cases()
            .expect("verified original history");
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_first_failures_reconcile_and_retain_ownership_across_reconnect() {
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let store = Store::new(Some(&url)).await.expect("store");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    for (outbound, destination, terminal) in [
        (true, "stateset-edi", "gateway_accepted"),
        (true, "stateset-sync-server", "synced"),
        (false, "stateset-edi", "applied"),
    ] {
        let tenant = domain::id("first-failure-pg");
        let seed = integration_failure_fixture(outbound, destination);
        canonical::put_workspace(&pool, &tenant, &seed)
            .await
            .expect("owned fixture");
        store
            .reconcile_operations(&tenant)
            .await
            .expect("reconcile retained failure");
        let reconnect = Store::new(Some(&url)).await.expect("independent reader");
        let failed = reconnect.read(&tenant).await.expect("committed failure");
        let case = failed
            .operations_cases
            .iter()
            .find(|c| c.source.subject_id == "first-failure-job")
            .expect("retained source has a durable issue");
        assert_eq!(case.status, "open");
        let id = case.id.clone();
        let original = json!({"outbox":failed.outbox,"inbox":failed.inbox});
        let row = failed.operations_case(&id, Utc::now()).expect("review");
        reconnect
            .mutate_authorized(
                (&tenant, "user:ops1"),
                "claim-first-failure",
                "operations.case.action",
                &id,
                trusted("user:ops1", input(&row, "claim")),
            )
            .await
            .expect("named responsibility");
        let claimed = store.read(&tenant).await.expect("durable claim");
        assert_eq!(
            json!({"outbox":claimed.outbox,"inbox":claimed.inbox}),
            original
        );
        let mut source_recovered = claimed;
        set_failure_state(&mut source_recovered, outbound, terminal, None);
        canonical::put_workspace(&pool, &tenant, &source_recovered)
            .await
            .expect("owned recovery fixture retains case history");
        store
            .reconcile_operations(&tenant)
            .await
            .expect("reconcile recovered source");
        let restored = reconnect.read(&tenant).await.expect("recovered");
        let history = restored
            .operations_cases
            .iter()
            .find(|c| c.id == id)
            .expect("history retained");
        assert_eq!(history.status, "resolved");
        assert!(history
            .history
            .iter()
            .any(|event| event.action == "claim" && event.actor == "user:ops1"));
        restored
            .verify_operations_cases()
            .expect("integrity after reconnect");
        assert!(!reconnect
            .read(&domain::id("other-first-failure-tenant"))
            .await
            .expect("other tenant")
            .operations_cases
            .iter()
            .any(|c| c.id == id));
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_case_claims_serialize_with_source_changes_and_retain_history_replay_and_isolation(
) {
    let memory = setup().await;
    let seed = read(&memory).await;
    let row = issue(&seed, "load_8", "shipment_exception");
    let source = business(&seed);
    let url = std::env::var("TEST_DATABASE_URL").expect("database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("operations-pg");
    let mut data = json!(seed);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned tenant");
    let id = row["id"].as_str().expect("id");
    let first = trusted("user:first", input(&row, "claim"));
    let second = trusted("user:second", input(&row, "claim"));
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "user:first"),
            "claim-a",
            "operations.case.action",
            id,
            first
        ),
        b.mutate_authorized(
            (&tenant, "user:second"),
            "claim-b",
            "operations.case.action",
            id,
            second
        )
    );
    assert_ne!(
        x.is_ok(),
        y.is_ok(),
        "one exact-version claim across independent instances"
    );
    let current = a.read(&tenant).await.expect("read");
    assert_eq!(business(&current), source);
    let case = current
        .operations_cases
        .iter()
        .find(|c| c.id == id)
        .expect("case");
    let owner = case.owner.as_deref().expect("owner").to_string();
    let row = issue(&current, "load_8", "shipment_exception");
    let note = trusted(&owner, input(&row, "note"));
    let intact: Value = sqlx::query_scalar(
        "SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id=$1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("stored");
    let mut corrupt = intact.clone();
    let index = current
        .operations_cases
        .iter()
        .position(|c| c.id == id)
        .expect("case index");
    corrupt["operations_cases"][index]["history"][0]["note"] =
        json!("CORRUPT retained triage proof");
    canonical::put_workspace(&pool, &tenant, &corrupt)
        .await
        .expect("owned corruption");
    assert_eq!(
        b.mutate_authorized(
            (&tenant, &owner),
            "retry-after-restoration",
            "operations.case.action",
            id,
            note.clone()
        )
        .await
        .expect_err("corruption"),
        "Internal storage error"
    );
    let after: Value = sqlx::query_scalar(
        "SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id=$1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("unchanged");
    assert_eq!(after, corrupt);
    canonical::put_workspace(&pool, &tenant, &intact)
        .await
        .expect("restore exact bytes");
    let saved = b
        .mutate_authorized(
            (&tenant, &owner),
            "retry-after-restoration",
            "operations.case.action",
            id,
            note.clone(),
        )
        .await
        .expect("failed key reusable");
    let reconnect = Store::new(Some(&url)).await.expect("reconnect");
    let replay = reconnect
        .mutate_authorized(
            (&tenant, &owner),
            "retry-after-restoration",
            "operations.case.action",
            id,
            note,
        )
        .await
        .expect("replay");
    assert_eq!(replay["data"], saved["data"]);
    assert_eq!(replay["replayed"], true);
    let current = reconnect.read(&tenant).await.expect("retained");
    assert_eq!(business(&current), source);
    assert!(reconnect
        .read(&domain::id("other-operations"))
        .await
        .expect("other tenant")
        .operations_cases
        .is_empty());
    let row = issue(&current, "load_8", "shipment_exception");
    let body = trusted(&owner, input(&row, "snooze"));
    let version = current
        .loads
        .iter()
        .find(|l| l.id == "load_8")
        .expect("load")
        .version;
    let (triage, cleared) = tokio::join!(
        a.mutate_authorized(
            (&tenant, &owner),
            "race-defer",
            "operations.case.action",
            id,
            body
        ),
        b.mutate_authorized(
            (&tenant, "user:ops"),
            "actual-source-clear",
            "load.command",
            "load_8",
            json!({"action":"resolve","expected_version":version})
        )
    );
    cleared.expect("actual source can clear under triage race");
    let final_state = reconnect.read(&tenant).await.expect("final");
    let c = final_state
        .operations_cases
        .iter()
        .find(|c| c.id == id)
        .expect("case");
    assert_eq!(c.status, "resolved");
    assert!(c.deferred_until.is_none());
    if triage.is_ok() {
        assert!(c.history.iter().any(|e| e.action == "snooze"));
    }
    assert_eq!(c.history.last().expect("clear").action, "source_cleared");
    let mut corrupt = json!(final_state);
    corrupt["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    corrupt["operations_cases"][index]["history"][0]["note"] =
        json!("CORRUPT index must not suppress real tracking");
    canonical::put_workspace(&pool, &tenant, &corrupt)
        .await
        .expect("owned index corruption");
    let load = final_state
        .loads
        .iter()
        .find(|l| l.id == "load_1")
        .expect("in transit");
    reconnect.mutate_authorized((&tenant,"user:actual-tracking"),"physical-through-index-failure","load.command","load_1",json!({"action":"location","expected_version":load.version,"location":"Actual carrier observation during work-index incident"})).await.expect("physical facts remain available");
    let reported = reconnect.read(&tenant).await.expect("actual retained");
    assert_eq!(
        reported
            .loads
            .iter()
            .find(|l| l.id == "load_1")
            .expect("load")
            .location,
        "Actual carrier observation during work-index incident"
    );
    assert_eq!(
        json!(reported.operations_cases),
        corrupt["operations_cases"]
    );
    assert_eq!(
        reported.operations_board(Utc::now())["evidence_valid"],
        false
    );
    let malformed_index = json!({"retain_original_work":corrupt["operations_cases"],"unsupported_structure":"requires original-record recovery"});
    let malformed_policy = json!({"version":"damaged","tracking_stale_minutes":"unknown","retain_original_policy_proof":"must not disappear"});
    let mut malformed = json!(reported);
    malformed["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    malformed["operations_cases"] = malformed_index.clone();
    malformed["operations_policy"] = malformed_policy.clone();
    canonical::put_workspace(&pool, &tenant, &malformed)
        .await
        .expect("owned malformed projection");
    let version = reported
        .loads
        .iter()
        .find(|l| l.id == "load_1")
        .expect("load")
        .version;
    reconnect.mutate_authorized((&tenant,"user:actual-tracking"),"physical-through-malformed-index","load.command","load_1",json!({"action":"location","expected_version":version,"location":"New actual observation while malformed work data is quarantined"})).await.expect("malformed work and policy cannot suppress actual tracking");
    let preserved = reconnect
        .read(&tenant)
        .await
        .expect("source workspace remains readable");
    assert_eq!(
        preserved
            .loads
            .iter()
            .find(|l| l.id == "load_1")
            .expect("load")
            .location,
        "New actual observation while malformed work data is quarantined"
    );
    assert_eq!(json!(preserved)["operations_cases"], malformed_index);
    assert_eq!(json!(preserved)["operations_policy"], malformed_policy);
    assert_eq!(
        preserved.operations_board(Utc::now())["metrics"]["median_first_response_seconds"],
        Value::Null
    );
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("remove owned tenant");
    pool.close().await;
}

#[test]
fn malformed_work_records_and_future_fields_are_quarantined_without_losing_original_values() {
    let mut seed = domain::demo();
    seed.reconcile_operations(Utc::now());
    let intact = json!(seed);
    for shape in [
        json!(null),
        json!({"original":"unreadable work container"}),
        json!([{"id":"incomplete-original-case","future_work_proof":[1,2,3]}]),
        {
            let mut future = intact["operations_cases"].clone();
            future[0]["future_proof"] = json!({"must_retain":1234567});
            future
        },
    ] {
        let mut raw = intact.clone();
        raw["operations_cases"] = shape.clone();
        let mut w: Workspace = serde_json::from_value(raw).expect("source data remains decodable");
        assert!(w.verify_operations_cases().is_err());
        let version = w.loads[7].version;
        w.apply("load.command","load_8",json!({"action":"exception","expected_version":version,"note":"Actual source report retained while work evidence needs recovery"})).expect("source mutation");
        assert_eq!(w.reconcile_operations(Utc::now()), 0);
        assert_eq!(json!(w)["operations_cases"], shape);
        assert_eq!(w.operations_board(Utc::now())["evidence_valid"], false);
    }
    let mut raw = intact;
    let original =
        json!({"version":3,"tracking_stale_minutes":240,"future_policy_proof":{"retain":true}});
    raw["operations_policy"] = original.clone();
    let mut w: Workspace = serde_json::from_value(raw).expect("unknown work policy preserved");
    assert!(w.verify_operations_cases().is_err());
    assert_eq!(w.reconcile_operations(Utc::now()), 0);
    assert_eq!(json!(w)["operations_policy"], original);
}

#[test]
fn clock_skew_preserves_actual_claims_without_fabricating_zero_second_response_times() {
    let mut w = domain::demo();
    w.loads.retain(|l| l.id == "load_8");
    w.loads[0].exception =
        Some("Actual condition observed by a worker whose clock is ahead".into());
    w.reconcile_operations(Utc::now() + Duration::seconds(60));
    let row = issue(&w, "load_8", "shipment_exception");
    w.apply_operations(
        "operations.case.action",
        row["id"].as_str().expect("id"),
        trusted("user:ops1", input(&row, "claim")),
        "user:ops1",
    )
    .expect("ownership remains valid despite server clock skew");
    let board = w.operations_board(Utc::now());
    assert_eq!(board["metrics"]["detected_cycles"], 1);
    assert_eq!(board["metrics"]["acknowledged_cycles"], 1);
    assert_eq!(board["metrics"]["response_time_samples"], 0);
    assert_eq!(board["metrics"]["response_time_unavailable_cycles"], 1);
    assert!(board["metrics"]["median_first_response_seconds"].is_null());
    assert_eq!(board["issues"][0]["owner"], "user:ops1");
    assert!(w.verify_operations_cases().is_ok());
}

#[test]
fn inactive_source_review_cannot_attach_to_an_identical_recurring_condition_before_the_clock_worker_runs(
) {
    let mut w = domain::demo();
    w.loads.retain(|l| l.id == "load_1");
    w.loads[0].exception = None;
    let now = Utc::now();
    w.loads[0].last_tracking_at = Some(now - Duration::minutes(300));
    w.reconcile_operations(now);
    w.reconcile_operations(now - Duration::hours(2));
    let board = w.operations_board(now - Duration::hours(2));
    let old = board["issues"][0].clone();
    assert_eq!(old["source_active"], false);
    assert_eq!(old["case"]["status"], "resolved");
    let live = issue(&w, "load_1", "tracking_stale");
    assert_eq!(live["source_active"], true);
    assert_eq!(
        live["source_sha256"], old["source_sha256"],
        "same facts can recur when the clock passes the threshold again"
    );
    assert_eq!(live["case"]["version"], old["case"]["version"]);
    let before = json!(w);
    assert!(w
        .apply_operations(
            "operations.case.action",
            old["id"].as_str().expect("id"),
            trusted("user:ops1", input(&old, "note")),
            "user:ops1"
        )
        .expect_err("old inactive review")
        .contains("source state changed"));
    assert_eq!(json!(w), before);
    w.reconcile_operations(Utc::now());
    let current = issue(&w, "load_1", "tracking_stale");
    assert_eq!(current["case"]["cycle"], 2);
    w.apply_operations(
        "operations.case.action",
        current["id"].as_str().expect("id"),
        trusted("user:ops1", input(&current, "note")),
        "user:ops1",
    )
    .expect("fresh active source review");
}

#[tokio::test]
async fn interrupted_work_resolution_uses_the_same_authenticated_and_normalized_body_as_application(
) {
    let s = setup().await;
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    let before = business(&read(&s).await);
    let r = router(s.clone());
    let path = format!(
        "operations/issues/{}/actions",
        row["id"].as_str().expect("case")
    );
    let mut body = input(&row, "claim");
    body.as_object_mut().expect("input").remove("defer_until"); // omitted option is normalized by CaseInput
    let (status, saved) = call(&r, "ops1", "POST", &path, body.clone(), "work-original").await;
    assert_eq!(status, 200, "{saved}");
    let resolution = json!({"request_key":"work-original","path":path,"body":body,"note":"Check the exact interrupted original"});
    let (status, completed) = call(
        &r,
        "ops1",
        "POST",
        "save-recovery/resolve",
        resolution.clone(),
        "unused",
    )
    .await;
    assert_eq!(status, 200, "{completed}");
    assert_eq!(completed["data"]["outcome"], "completed");
    assert_eq!(completed["data"]["result"], saved["data"]);
    let after = json!(read(&s).await);
    for who in ["ops2", "admin"] {
        let (status, denied) = call(
            &r,
            who,
            "POST",
            "save-recovery/resolve",
            resolution.clone(),
            "unused",
        )
        .await;
        assert_eq!(status, 409, "{who}: {denied}");
    }
    assert_eq!(json!(read(&s).await), after);
    assert_eq!(business(&read(&s).await), before);
}

#[tokio::test]
async fn stopped_work_original_cannot_later_claim_or_replace_the_reviewer_with_a_forged_wrapper() {
    let s = setup().await;
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    let r = router(s.clone());
    let body = input(&row, "claim");
    let path = format!(
        "operations/issues/{}/actions",
        row["id"].as_str().expect("case")
    );
    let resolution = json!({"request_key":"stopped-work","path":path,"body":body,"note":"Keep the uncertain original from applying"});
    let (status, stopped) = call(
        &r,
        "ops1",
        "POST",
        "save-recovery/resolve",
        resolution.clone(),
        "unused",
    )
    .await;
    assert_eq!(status, 200, "{stopped}");
    assert_eq!(stopped["data"]["outcome"], "stopped");
    let after = json!(read(&s).await);
    let (status, result) = call(&r, "ops1", "POST", &path, body.clone(), "stopped-work").await;
    assert_eq!(status, 409, "{result}");
    assert_eq!(result["error"]["code"], "request_stopped");
    let mut forged = resolution;
    forged["request_key"] = json!("forged-work");
    forged["body"] = json!({"request":body,"is_admin":true,"subject":"user:admin"});
    assert_eq!(
        call(
            &r,
            "ops1",
            "POST",
            "save-recovery/resolve",
            forged,
            "unused"
        )
        .await
        .0,
        422
    );
    assert_eq!(json!(read(&s).await), after);
}

#[tokio::test]
async fn work_stop_resolution_respects_route_roles_and_admin_only_force_release() {
    let s = setup().await;
    let row = issue(&read(&s).await, "load_8", "shipment_exception");
    let r = router(s.clone());
    let path = format!(
        "operations/issues/{}/actions",
        row["id"].as_str().expect("case")
    );
    let before = json!(read(&s).await);
    for who in ["viewer", "finance", "carrier", "customer"] {
        let resolution = json!({"request_key":"role-test","path":path,"body":input(&row,"claim"),"note":"Attempt with unauthorized role"});
        assert_eq!(
            call(
                &r,
                who,
                "POST",
                "save-recovery/resolve",
                resolution,
                "unused"
            )
            .await
            .0,
            403,
            "{who}"
        );
    }
    let resolution = json!({"request_key":"force-work","path":path,"body":input(&row,"force_release"),"note":"No elevated original"});
    assert_eq!(
        call(
            &r,
            "ops1",
            "POST",
            "save-recovery/resolve",
            resolution,
            "unused"
        )
        .await
        .0,
        403
    );
    assert_eq!(json!(read(&s).await), before);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_work_originals_resolve_completed_stopped_and_racing_actions_across_reconnects() {
    use stateset_freight::save_recovery::{ResolveSave, REQUEST_STOPPED};
    let seed = read(&setup().await).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable database");
    let a = Store::new(Some(&url)).await.expect("first store");
    let b = Store::new(Some(&url)).await.expect("second store");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("work-originals-pg");
    let mut data = json!(seed);
    data["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &data)
        .await
        .expect("owned seed");
    let row = issue(
        &a.read(&tenant).await.expect("read"),
        "load_8",
        "shipment_exception",
    );
    let id = row["id"].as_str().expect("id");
    let path = format!("operations/issues/{id}/actions");
    let mut principal = principal("user:ops1");
    principal.tenant_id = tenant.clone();
    let resolution = |key: &str, body: Value| ResolveSave {
        request_key: key.into(),
        path: path.clone(),
        body,
        note: "Resolve exact owned original".into(),
    };
    let body = input(&row, "claim");
    let saved = a
        .mutate_authorized(
            (&tenant, &principal.subject),
            "pg-work-original",
            "operations.case.action",
            id,
            trusted(&principal.subject, body.clone()),
        )
        .await
        .expect("claim");
    let reconnect = Store::new(Some(&url)).await.expect("reconnect");
    let completed = reconnect
        .resolve_save(&principal, resolution("pg-work-original", body))
        .await
        .expect("resolve completed");
    assert_eq!(completed["data"]["outcome"], "completed");
    assert_eq!(completed["data"]["result"], saved["data"]);
    let row = issue(
        &a.read(&tenant).await.expect("claimed"),
        "load_8",
        "shipment_exception",
    );
    let body = input(&row, "release");
    assert_eq!(
        b.resolve_save(&principal, resolution("pg-work-stop", body.clone()))
            .await
            .expect("stop")["data"]["outcome"],
        "stopped"
    );
    assert_eq!(
        reconnect
            .mutate_authorized(
                (&tenant, &principal.subject),
                "pg-work-stop",
                "operations.case.action",
                id,
                trusted(&principal.subject, body)
            )
            .await
            .expect_err("cannot apply stopped release"),
        REQUEST_STOPPED
    );
    let before = a.read(&tenant).await.expect("before race");
    let row = issue(&before, "load_8", "shipment_exception");
    let body = input(&row, "note");
    let (applied, resolved) = tokio::join!(
        a.mutate_authorized(
            (&tenant, &principal.subject),
            "pg-work-race",
            "operations.case.action",
            id,
            trusted(&principal.subject, body.clone())
        ),
        b.resolve_save(&principal, resolution("pg-work-race", body))
    );
    let resolved = resolved.expect("one consistent resolution");
    if let Ok(applied) = applied {
        assert_eq!(resolved["data"]["outcome"], "completed");
        assert_eq!(resolved["data"]["result"], applied["data"]);
    } else {
        assert_eq!(resolved["data"]["outcome"], "stopped");
    }
    assert_eq!(
        business(&reconnect.read(&tenant).await.expect("retained truth")),
        business(&before)
    );
}
