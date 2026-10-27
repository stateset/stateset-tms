use crate::{
    actions::*,
    client, demo,
    model::App,
    tests::{response, server},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}
fn app() -> App {
    let mut app = App {
        needs_fetch: false,
        action_viewport_ready: true,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app
}
fn pending(app: &App, kind: Kind, input: &str) -> Pending {
    Pending::new(
        app.current()
            .and_then(|r| r.target.clone())
            .expect("target"),
        Owner::from_identity(&app.identity).expect("owner"),
        kind,
        input,
    )
    .expect("pending")
}
fn submit(pending: Pending, checking: bool) -> WriteJob {
    WriteJob::Submit { pending, checking }
}
fn identity(pending: &Pending) -> Value {
    json!({"tenant_id":pending.owner.tenant,"access":{"subject":pending.owner.subject,"capabilities":["workspace.read","operations.write","saves.recover"]}})
}
fn saved(pending: &Pending) -> Value {
    let body: Value = serde_json::from_slice(&pending.body).expect("body");
    json!({"id":pending.target.id,"version":pending.target.version+1,"location":body["location"],"exception":body["note"]})
}
fn execute(job: &WriteJob, responses: Vec<String>) -> (Outcome, Vec<String>) {
    let (origin, handle) = server(responses);
    let api = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("client");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(crate::actions::execute(&api, &origin, job));
    (result, handle.join().expect("server"))
}

#[test]
fn action_requires_role_fresh_view_valid_target_and_eligible_state() {
    let mut a = app();
    a.identity["access"]["capabilities"] = json!(["workspace.read"]);
    a.open_action();
    assert!(a.action_dialog.is_none());
    let mut a = app();
    a.error = Some("Stale".into());
    a.open_action();
    assert!(a.action_dialog.is_none());
    let mut target = app()
        .current()
        .and_then(|r| r.target.clone())
        .expect("target");
    target.status = "delivered".into();
    assert!(!Kind::Location.allowed(&target));
    assert!(!Kind::Exception.allowed(&target));
    assert!(Kind::Resolve.allowed(&target));
    assert!(Target::from_load(
        &json!({"id":"../wrong","reference":"X","version":1,"status":"in_transit"})
    )
    .is_none());
    assert!(Target::from_load(
        &json!({"id":"safe","reference":"X","version":u64::MAX,"status":"in_transit"})
    )
    .is_none());
}
#[test]
fn form_pins_reviewed_target_and_only_explicit_confirmation_prepares_a_write() {
    let mut a = app();
    a.key(key('a'));
    a.key(key('1'));
    a.paste("  Portland, OR  ");
    a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(a.write_ready.is_none());
    assert!(a.pending.is_none());
    let reviewed = a.action_dialog.as_ref().expect("review").target.version;
    let mut newer = demo::snapshot(&a.request());
    newer.data["loads"][0]["version"] = json!(99);
    a.apply(newer);
    a.key(KeyEvent {
        kind: KeyEventKind::Repeat,
        ..key('y')
    });
    assert!(a.pending.is_none());
    a.key(key('y'));
    let p = a.pending.as_ref().expect("original");
    assert_eq!(p.target.version, reviewed);
    let value: Value = serde_json::from_slice(&p.body).expect("body");
    assert_eq!(
        value,
        json!({"action":"location","expected_version":reviewed,"location":"Portland, OR"})
    );
}
#[test]
fn hidden_or_overflowing_review_cannot_submit() {
    let mut a = app();
    a.key(key('a'));
    a.key(key('1'));
    a.paste("Portland");
    a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for (width, height) in [(52, 16), (80, 24)] {
        if width == 80 {
            let d = a.action_dialog.as_mut().expect("dialog");
            d.target.reference = "長".repeat(500);
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|f| ui::render(f, &mut a, &ui::Theme::new(false, false), true, "offline"))
            .expect("render");
        assert!(!a.action_viewport_ready);
        a.key(key('y'));
        assert!(a.pending.is_none());
    }
}
#[test]
fn uncertain_original_survives_refresh_blocks_new_writes_and_reuses_exact_bytes() {
    let mut a = app();
    let original = pending(&a, Kind::Exception, "Actual carrier delay");
    a.pending = Some(original.clone());
    a.finish_write(false, Outcome::Uncertain("Lost response".into()));
    a.apply(demo::snapshot(&a.request()));
    a.open_action();
    assert!(a.action_dialog.is_none());
    a.key(key('c'));
    let job = a.write_ready.as_ref().expect("explicit original check");
    assert!(job.recovery());
    assert_eq!(job.pending().key, original.key);
    assert_eq!(job.pending().body, original.body);
    a.finish_write(true, Outcome::Rejected("Conflict".into()));
    assert!(a.pending.is_some());
    assert!(a.write_uncertain);
}
#[test]
fn confirmed_save_with_failed_refresh_never_retries_the_mutation() {
    let mut a = app();
    a.pending = Some(pending(&a, Kind::Location, "Actual location"));
    a.finish_write(false, Outcome::Completed);
    assert!(a.pending.is_none());
    assert!(a.needs_fetch);
    a.finish(
        a.generation,
        Err(client::Failure {
            message: "Refresh unavailable".into(),
            denied: false,
        }),
    );
    a.key(key('r'));
    assert!(a.needs_fetch);
    assert!(a.write_ready.is_none());
    assert!(a
        .write_notice
        .as_ref()
        .expect("notice")
        .contains("Save confirmed"));
}
#[test]
fn uncertain_stop_retries_the_resolution_instead_of_reapplying_business_intent() {
    let mut a = app();
    a.pending = Some(pending(&a, Kind::Exception, "Actual incident"));
    a.write_uncertain = true;
    a.key(key('x'));
    a.paste("Stop this original request");
    a.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    a.key(key('y'));
    assert!(matches!(a.write_ready.take(), Some(WriteJob::Stop { .. })));
    a.finish_write(true, Outcome::Uncertain("Lost stop response".into()));
    a.key(key('c'));
    match a.write_ready.take().expect("resolution check") {
        WriteJob::Stop { note, .. } => assert_eq!(note, "Stop this original request"),
        WriteJob::Submit { .. } => panic!("stop intent must never become a business replay"),
    }
}
#[test]
fn access_closure_clears_private_recovery_and_blocks_blind_replacement() {
    let mut a = app();
    a.pending = Some(pending(&a, Kind::Exception, "Private actual incident"));
    a.finish_write(true, Outcome::Denied);
    assert!(a.pending.is_none());
    assert!(a.rows.is_empty());
    assert!(a.identity.is_null());
    assert!(a.write_lost);
    a.apply(demo::snapshot(&a.request()));
    a.open_action();
    assert!(a.action_dialog.is_none());
    a.key(key('q'));
    assert!(!a.quit);
    assert!(a.quit_confirm);
    a.key(key('Q'));
    assert!(a.quit);
}
#[test]
fn demo_receipts_complete_once_and_stops_fence_late_originals() {
    let a = app();
    let original = pending(&a, Kind::Exception, "Actual synthetic delay");
    let mut state = demo::State::default();
    assert!(matches!(
        state.execute(&submit(original.clone(), false)),
        Outcome::Completed
    ));
    assert!(matches!(
        state.execute(&submit(original.clone(), true)),
        Outcome::Completed
    ));
    assert!(matches!(
        state.execute(&WriteJob::Stop {
            pending: original.clone(),
            note: "Resolve original".into()
        }),
        Outcome::Completed
    ));
    let data = state.snapshot(&a.request()).data;
    assert_eq!(data["loads"][0]["version"], original.target.version + 1);
    assert_eq!(data["loads"][0]["exception"], "Actual synthetic delay");
    let mut state = demo::State::default();
    assert!(matches!(
        state.execute(&WriteJob::Stop {
            pending: original.clone(),
            note: "Stop pending original".into()
        }),
        Outcome::Stopped
    ));
    assert!(matches!(
        state.execute(&submit(original.clone(), true)),
        Outcome::Stopped
    ));
    assert_eq!(
        state.snapshot(&a.request()).data["loads"][0]["version"],
        original.target.version
    );
}
#[test]
fn real_http_write_rechecks_owner_and_sends_only_the_reviewed_command() {
    let p = pending(&app(), Kind::Location, "Actual reported location");
    let job = submit(p.clone(), false);
    let (result, requests) = execute(
        &job,
        vec![
            response(identity(&p)),
            response(json!({"data":saved(&p),"replayed":false})),
        ],
    );
    assert!(matches!(result, Outcome::Completed));
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with(&format!("POST /v1/loads/{}/commands ", p.target.id)));
    assert!(requests[1]
        .to_lowercase()
        .contains(&format!("idempotency-key: {}", p.key)));
    assert_eq!(
        requests[1]
            .split_once("\r\n\r\n")
            .expect("body")
            .1
            .as_bytes(),
        p.body
    );
    let mut wrong = identity(&p);
    wrong["access"]["subject"] = json!("different-operator");
    let (result, requests) = execute(&job, vec![response(wrong)]);
    assert!(matches!(result, Outcome::Denied));
    assert_eq!(requests.len(), 1);
}
#[test]
fn lost_http_acknowledgement_is_uncertain_and_explicit_check_reuses_request() {
    let p = pending(&app(), Kind::Exception, "Actual incident");
    let (result, first) = execute(
        &submit(p.clone(), false),
        vec![response(identity(&p)), String::new()],
    );
    assert!(matches!(result, Outcome::Uncertain(_)));
    let (result, second) = execute(
        &submit(p.clone(), true),
        vec![
            response(identity(&p)),
            response(json!({"data":saved(&p),"replayed":true})),
        ],
    );
    assert!(matches!(result, Outcome::Completed));
    // Each owned server has a different ephemeral Host port. The original
    // request method/path, idempotency key and exact body must stay identical.
    assert_eq!(first[1].lines().next(), second[1].lines().next());
    let original_body = first[1].split_once("\r\n\r\n").expect("first body").1;
    assert_eq!(
        original_body,
        second[1].split_once("\r\n\r\n").expect("check body").1
    );
    for request in [&first[1], &second[1]] {
        assert!(request
            .to_lowercase()
            .contains(&format!("idempotency-key: {}", p.key)));
    }
}
#[test]
fn real_http_resolution_binds_original_key_path_body_and_checks_result() {
    let p = pending(&app(), Kind::Exception, "Actual incident");
    let job = WriteJob::Stop {
        pending: p.clone(),
        note: "Resolve this original request".into(),
    };
    let (result, requests) = execute(
        &job,
        vec![
            response(identity(&p)),
            response(json!({"data":{"request_key":p.key,"outcome":"stopped"},"replayed":false})),
        ],
    );
    assert!(matches!(result, Outcome::Stopped));
    let body: Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").expect("body").1).expect("json");
    assert_eq!(body["path"], p.path());
    assert_eq!(body["request_key"], p.key);
    assert_eq!(
        body["body"],
        serde_json::from_slice::<Value>(&p.body).expect("original")
    );
    let (result, _) = execute(
        &job,
        vec![
            response(identity(&p)),
            response(json!({"data":{"request_key":"wrong","outcome":"stopped"},"replayed":true})),
        ],
    );
    assert!(matches!(result, Outcome::Uncertain(_)));
}
#[test]
fn invalid_or_unrepresentable_inputs_are_refused_before_creating_requests() {
    let a = app();
    let p = pending(&a, Kind::Location, "Portland");
    for input in [
        "".into(),
        "東".repeat(70),
        "text\x1b[2J".into(),
        "text\u{202e}".into(),
    ] {
        assert!(Pending::new(p.target.clone(), p.owner.clone(), Kind::Location, &input).is_err());
    }
    assert!(!p.validates_result(&json!({"id":p.target.id,"version":999,"location":"Portland"})));
}
