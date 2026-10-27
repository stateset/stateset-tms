use crate::{
    actions::{self, Kind, Outcome, Owner, Pending, Stage, Target, WriteJob},
    demo,
    model::{App, View},
    operations::Scope,
    tests::{response, server},
    ui,
    work_actions::WorkKind,
};
use chrono::{Duration, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn app() -> App {
    let mut app = App {
        view: View::Operations,
        operations_scope: Scope::All,
        needs_fetch: false,
        action_viewport_ready: true,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app.selected = 1;
    app
}
fn target(app: &App) -> Target {
    app.current()
        .and_then(|r| r.target.clone())
        .expect("trusted work target")
}
fn owner(app: &App) -> Owner {
    Owner::from_identity(&app.identity).expect("owner")
}
fn time() -> String {
    (Utc::now() + Duration::hours(2)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
fn pending(app: &App, kind: WorkKind) -> Pending {
    Pending::new_work(
        target(app),
        owner(app),
        kind,
        "Actual carrier coordination",
        &time(),
    )
    .expect("work pending")
}
fn submit(pending: Pending, checking: bool) -> WriteJob {
    WriteJob::Submit { pending, checking }
}
fn result(p: &Pending) -> Value {
    let body: Value = serde_json::from_slice(&p.body).expect("body");
    json!({"id":p.target.id,"version":if p.target.version==0{2}else{p.target.version+1},"source_sha256":p.target.work.as_ref().expect("source").source_sha256,"owner":p.owner.subject,"status":"working","follow_up_at":body["follow_up_at"],"deferred_until":null,
        "history":[{"action":"detected","actor":"system:operations"},{"action":"claim","actor":p.owner.subject,"note":body["note"]}]})
}
fn execute(job: &WriteJob, responses: Vec<String>) -> (Outcome, Vec<String>) {
    let (origin, worker) = server(responses);
    let api = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("HTTP client");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let out = rt.block_on(actions::execute(&api, &origin, job));
    (out, worker.join().expect("requests"))
}
fn review(app: &mut App, kind: char, when: Option<&str>) {
    key(app, KeyCode::Char('a'));
    key(app, KeyCode::Char(kind));
    if let Some(time) = when {
        app.paste(time);
        key(app, KeyCode::Enter);
    }
    app.paste("Actual coordination note");
    key(app, KeyCode::Enter);
    assert!(app
        .action_dialog
        .as_ref()
        .is_some_and(|d| d.stage == Stage::Review));
}
#[test]
fn work_review_pins_source_version_actor_note_and_absolute_time_without_sending() {
    let mut app = app();
    review(&mut app, '1', Some("2h"));
    let dialog = app.action_dialog.as_ref().expect("review");
    let original = dialog.target.clone();
    let absolute = dialog.when.clone();
    assert!(absolute.ends_with('Z'));
    assert!(app.pending.is_none() && app.write_ready.is_none());
    app.paste("y");
    assert!(app.write_ready.is_none());
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        app.key(KeyEvent::new(KeyCode::Char('y'), modifiers));
        assert!(app.write_ready.is_none());
    }
    let mut repeated = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
    repeated.kind = KeyEventKind::Repeat;
    app.key(repeated);
    assert!(app.write_ready.is_none());
    // Even a newer response cannot replace the reviewed evidence.
    let mut response = demo::snapshot(&app.request());
    response.data["issues"][1]["case"] = json!({"version":99});
    app.apply(response);
    key(&mut app, KeyCode::Char('y'));
    let p = app.pending.as_ref().expect("confirmed original");
    assert_eq!(p.target, original);
    let body: Value = serde_json::from_slice(&p.body).expect("body");
    assert_eq!(body["follow_up_at"], absolute);
    assert_eq!(body["expected_version"], 0);
    assert_eq!(body["expected_source_active"], true);
    assert_eq!(p.path(), "operations/issues/issue-demo-1/actions");
    assert!(app.write_ready.is_some());
}
#[test]
fn choosing_unavailable_owner_actions_never_edits_or_prepares_a_write() {
    let mut app = app();
    app.selected = 0; // owned by another dispatcher
    key(&mut app, KeyCode::Char('a'));
    for keycode in ['1', '2', '4', '5', '6'] {
        key(&mut app, KeyCode::Char(keycode));
        assert!(app
            .action_dialog
            .as_ref()
            .is_some_and(|d| d.stage == Stage::Choose));
        assert!(app.write_ready.is_none());
    }
    key(&mut app, KeyCode::Char('3'));
    assert!(app
        .action_dialog
        .as_ref()
        .is_some_and(|d| d.stage == Stage::Edit));
    key(&mut app, KeyCode::Esc);
    assert!(app.action_dialog.is_none() && app.pending.is_none());
}
#[test]
fn malformed_or_changed_source_and_case_identity_are_never_actionable() {
    let a = app();
    let base = demo::snapshot(&a.request()).data["issues"][0].clone();
    for (field, bad) in [
        ("id", json!("../wrong")),
        ("source_sha256", json!("a".repeat(63))),
        ("source_sha256", json!("G".repeat(64))),
        ("source_active", json!(null)),
        ("source_changed", json!(true)),
        ("evidence_valid", json!(false)),
        ("owner", json!("changed-owner")),
        ("case", json!({"version":2})),
    ] {
        let mut row = base.clone();
        row[field] = bad;
        assert!(Target::from_issue(&row).is_none(), "{field}");
    }
    let mut row = base.clone();
    row["case"]["id"] = json!("different-case");
    assert!(Target::from_issue(&row).is_none());
    let mut row = base;
    row["case"]["source_sha256"] = json!("f".repeat(64));
    assert!(Target::from_issue(&row).is_none());
    let mut row = demo::snapshot(&a.request()).data["issues"][1].clone();
    row.as_object_mut().expect("issue").remove("owner");
    assert!(Target::from_issue(&row).is_none());
}
#[test]
fn work_times_are_explicit_bounded_and_pinned_to_utc() {
    let a = app();
    let mut t = target(&a);
    let now = Utc
        .with_ymd_and_hms(2026, 10, 20, 12, 0, 0)
        .single()
        .expect("clock");
    assert_eq!(
        WorkKind::Claim.time("2h", &t, now).expect("two hours"),
        "2026-10-20T14:00:00Z"
    );
    assert_eq!(
        WorkKind::Claim
            .time("2026-10-20T08:00:00-07:00", &t, now)
            .expect("offset"),
        "2026-10-20T15:00:00Z"
    );
    for raw in [
        "0m",
        "-1h",
        "8d",
        "99999999999999999h",
        "tomorrow",
        "2026-10-20T13:00:00",
        "2026-10-20T11:59:59Z",
    ] {
        assert!(WorkKind::Claim.time(raw, &t, now).is_err(), "{raw}");
    }
    t.work.as_mut().expect("work").follow_up_at = Some("2026-10-20T13:00:00Z".into());
    assert!(WorkKind::Defer.time("30m", &t, now).is_ok());
    assert!(WorkKind::Defer.time("2h", &t, now).is_err());
    assert!(WorkKind::Defer.time("2d", &t, now).is_err());
}
#[test]
fn a_time_that_expires_during_review_must_be_reviewed_again_before_sending() {
    let mut a = app();
    review(&mut a, '1', Some("1h"));
    a.action_dialog.as_mut().expect("review").when = "2000-01-01T00:00:00Z".into();
    key(&mut a, KeyCode::Char('y'));
    assert!(a.write_ready.is_none() && a.pending.is_none());
    assert!(a
        .action_dialog
        .as_ref()
        .expect("held review")
        .error
        .is_some());
}
#[test]
fn stale_reads_viewer_roles_pending_originals_and_recovery_errors_block_new_work() {
    for condition in 0..5 {
        let mut a = app();
        match condition {
            0 => a.identity["access"]["capabilities"] = json!(["workspace.read"]),
            1 => a.updated = Some(std::time::Instant::now() - std::time::Duration::from_secs(120)),
            2 => a.pending = Some(pending(&a, WorkKind::Claim)),
            3 => a.recovery_error = Some("Owned storage failure".into()),
            _ => a.loading = Some(a.generation),
        };
        assert!(!a.can_open_action());
        key(&mut a, KeyCode::Char('a'));
        assert!(a.action_dialog.is_none());
    }
}
#[test]
fn critical_work_cannot_be_deferred_and_work_does_not_offer_shipment_commands() {
    let a = app();
    let mut t = target(&a);
    t.work.as_mut().expect("work").owner = Some(owner(&a).subject);
    t.work.as_mut().expect("work").priority = 0;
    t.work.as_mut().expect("work").follow_up_at = Some(time());
    assert!(!WorkKind::Defer.allowed(&t, &owner(&a)));
    assert!(Kind::ALL.into_iter().all(|k| !k.allowed(&t)));
    assert_eq!(Kind::choices(&t).len(), 6);
    assert!(Pending::new(t.clone(), owner(&a), Kind::Resolve, "").is_err());
}
#[test]
fn synthetic_work_loop_updates_owned_cases_and_never_shipment_source_data() {
    let mut state = demo::State::default();
    let mut a = app();
    let before = state.snapshot(&a.request()).data["loads"].clone();
    for kind in [
        WorkKind::Claim,
        WorkKind::Note,
        WorkKind::FollowUp,
        WorkKind::Defer,
        WorkKind::Resume,
        WorkKind::Release,
    ] {
        a.needs_fetch = false;
        a.apply(state.snapshot(&a.request()));
        a.selected = 1;
        let when = if kind == WorkKind::Defer {
            (Utc::now() + Duration::minutes(15)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        } else {
            time()
        };
        let p = Pending::new_work(
            target(&a),
            owner(&a),
            kind,
            "Owned actual coordination",
            &when,
        )
        .expect("work action");
        let job = submit(p.clone(), false);
        assert!(matches!(state.execute(&job), Outcome::Completed));
        assert!(matches!(state.execute(&job), Outcome::Completed));
        let response = state.snapshot(&a.request());
        assert!(
            p.validates_result(&response.data["issues"][1]["case"]),
            "{kind:?}"
        );
        assert_eq!(response.data["loads"], before);
    }
    a.apply(state.snapshot(&a.request()));
    a.selected = 1;
    assert!(target(&a).work.as_ref().expect("work").owner.is_none());
}
#[test]
fn acknowledgements_require_the_exact_source_owner_history_and_expected_version() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    let good = result(&p);
    assert!(p.validates_result(&good));
    for (field, bad) in [
        ("id", json!("other")),
        ("version", json!(99)),
        ("source_sha256", json!("f".repeat(64))),
        ("owner", json!("other-operator")),
        ("history", json!([])),
        ("follow_up_at", json!("2000-01-01T00:00:00Z")),
    ] {
        let mut wrong = good.clone();
        wrong[field] = bad;
        assert!(!p.validates_result(&wrong), "{field}");
    }
}
#[test]
fn actual_http_work_submit_checks_access_and_uses_the_pinned_original_once() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    let original = p.clone();
    let (out, requests) = execute(
        &submit(p, false),
        vec![
            response(a.identity),
            response(json!({"data":result(&original),"replayed":false})),
        ],
    );
    assert!(matches!(out, Outcome::Completed));
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/access "));
    assert!(requests[1].starts_with("POST /v1/operations/issues/issue-demo-1/actions "));
    assert!(requests[1]
        .to_lowercase()
        .contains(&format!("idempotency-key: {}", original.key)));
    assert!(requests[1].ends_with(std::str::from_utf8(&original.body).expect("original bytes")));
}
#[test]
fn wrong_identity_refuses_work_before_post_and_an_unknown_result_keeps_the_original() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    let mut identity = a.identity.clone();
    identity["access"]["subject"] = json!("another-operator");
    let (out, requests) = execute(&submit(p.clone(), false), vec![response(identity)]);
    assert!(matches!(out, Outcome::Denied));
    assert_eq!(requests.len(), 1);
    let (out, requests) = execute(
        &submit(p, false),
        vec![
            response(a.identity),
            response(json!({"data":{"id":"wrong"},"replayed":false})),
        ],
    );
    assert!(matches!(out, Outcome::Uncertain(_)));
    assert_eq!(requests.len(), 2);
}
#[test]
fn interrupted_work_can_replay_exact_bytes_or_resolve_the_exact_original() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    for completed in [false, true] {
        let data = if completed {
            json!({"request_key":p.key,"outcome":"completed","result":result(&p)})
        } else {
            json!({"request_key":p.key,"outcome":"stopped"})
        };
        let (out, requests) = execute(
            &WriteJob::Stop {
                pending: p.clone(),
                note: "Retain the exact stop decision".into(),
            },
            vec![
                response(a.identity.clone()),
                response(json!({"data":data,"replayed":true})),
            ],
        );
        assert!(if completed {
            matches!(out, Outcome::Completed)
        } else {
            matches!(out, Outcome::Stopped)
        });
        let body: Value =
            serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).expect("resolver body"))
                .expect("JSON");
        assert_eq!(body["path"], p.path());
        assert_eq!(
            body["body"],
            serde_json::from_slice::<Value>(&p.body).expect("original")
        );
        assert_eq!(body["request_key"], p.key);
    }
    let (out, requests) = execute(
        &submit(p.clone(), true),
        vec![
            response(a.identity),
            response(json!({"data":result(&p),"replayed":true})),
        ],
    );
    assert!(matches!(out, Outcome::Completed));
    assert!(requests[1].ends_with(std::str::from_utf8(&p.body).expect("original bytes")));
}
#[test]
fn compact_action_review_never_confirms_when_its_full_evidence_does_not_fit() {
    for (width, height, expected) in [
        (52, 24, false),
        (72, 22, true),
        (77, 40, true),
        (120, 30, true),
    ] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut a = app();
            review(&mut a, '1', Some("2h"));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal
                .draw(|f| ui::render(f, &mut a, &ui::Theme::new(no_color, light), true, "offline"))
                .expect("review frame");
            assert_eq!(a.action_viewport_ready, expected, "{width}x{height}");
            key(&mut a, KeyCode::Char('y'));
            assert_eq!(a.write_ready.is_some(), expected);
        }
    }
}
#[test]
fn command_palette_opens_review_without_turning_its_query_into_a_write() {
    let mut a = app();
    a.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    a.paste("review selected actions");
    let choices = a.palette_choices();
    assert_eq!(choices.len(), 1);
    assert!(choices[0].enabled);
    key(&mut a, KeyCode::Enter);
    assert!(a
        .action_dialog
        .as_ref()
        .is_some_and(|d| d.stage == Stage::Choose));
    assert!(a.write_ready.is_none());
}
#[test]
fn a_new_issue_without_a_case_uses_zero_version_and_checks_the_detected_plus_action_ack() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    assert_eq!(p.target.version, 0);
    assert!(p.validates_result(&result(&p)));
    let mut wrong = result(&p);
    wrong["version"] = json!(1);
    assert!(!p.validates_result(&wrong));
}

#[test]
fn an_expired_deferral_ack_requires_the_exact_system_resume_before_the_requested_action() {
    let mut a = app();
    let mut t = target(&a);
    t.version = 4;
    t.status = "working".into();
    let work = t.work.as_mut().expect("work");
    work.owner = Some(owner(&a).subject.clone());
    work.deferred_until = Some("2026-10-20T11:00:00Z".into());
    work.follow_up_at = Some(time());
    a.rows[1].target = Some(t);
    let p = pending(&a, WorkKind::FollowUp);
    let mut data = result(&p);
    data["version"] = json!(6);
    data["history"] = json!([{"action":"resumed","actor":"system:operations","at":"2026-10-20T12:00:00Z"},{"action":"note","actor":p.owner.subject,"note":"Actual carrier coordination"}]);
    assert!(p.validates_result(&data));
    for (field, bad) in [
        ("action", json!("source_changed")),
        ("actor", json!("another-operator")),
        ("at", json!("2026-10-20T10:00:00Z")),
    ] {
        let mut forged = data.clone();
        forged["history"][0][field] = bad;
        assert!(!p.validates_result(&forged), "{field}");
    }
}

#[test]
fn work_acknowledgements_with_retained_history_larger_than_a_shipment_response_still_validate() {
    let a = app();
    let p = pending(&a, WorkKind::Claim);
    let mut data = result(&p);
    let mut history = vec![json!({"action":"note","note":"x".repeat(200)}); 400];
    history.extend(data["history"].as_array().expect("latest events").clone());
    data["history"] = json!(history);
    assert!(data.to_string().len() > 64 * 1024);
    let (out, requests) = execute(
        &submit(p, false),
        vec![
            response(a.identity),
            response(json!({"data":data,"replayed":false})),
        ],
    );
    assert!(matches!(out, Outcome::Completed));
    assert_eq!(requests.len(), 2);
}

#[test]
fn old_apis_keep_operations_read_only_and_recovery_never_posts_to_an_unsupported_server() {
    let mut a = app();
    let p = pending(&a, WorkKind::Claim);
    a.identity
        .as_object_mut()
        .expect("identity")
        .remove("features");
    assert!(!a.can_open_action());
    key(&mut a, KeyCode::Char('a'));
    assert!(a.action_dialog.is_none());
    assert!(a
        .write_notice
        .as_deref()
        .is_some_and(|s| s.contains("compatible API")));
    for job in [
        submit(p.clone(), false),
        submit(p.clone(), true),
        WriteJob::Stop {
            pending: p,
            note: "Exact stop decision".into(),
        },
    ] {
        let (out, requests) = execute(&job, vec![response(a.identity.clone())]);
        assert!(matches!(out, Outcome::Rejected(_)));
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /v1/access "));
    }
}
