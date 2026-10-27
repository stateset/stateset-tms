use crate::{
    actions::{Kind, Owner, Pending},
    client::Snapshot,
    demo,
    model::{App, QueueDirection, View},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn data() -> Value {
    json!({
        "outbox": [
            {"id":"shared-id","load_id":"load-demo-0001","destination":"stateset-edi","status":"retry","attempts":2,"total_attempts":10,"transmission":"unknown","leased_until":"2026-10-19T18:00:00Z","last_error":"Owned timeout diagnostic\u{001b}\u{202e}","payload":{"token":"PRIVATE-PAYLOAD-CANARY"},"response":"PRIVATE-RESPONSE-CANARY"},
            {"id":"held-native","load_id":"load-demo-0002","destination":"stateset-icommerce","status":"held","attempts":1,"total_attempts":3,"transmission":"attempted"},
            {"id":"accepted","status":"gateway_accepted","transmission":"unknown"},
            {"id":"reviewed","status":"external_reviewed","transmission":"unknown","last_error":"Retained historical error"},
            {"id":"future","status":"future_state","transmission":"unknown"},
            {"id":"sending","status":"sending","transmission":"attempted"}
        ],
        "inbox": [
            {"id":"shared-id","document_id":"EDI-IN-001","status":"retry","attempts":1,"total_attempts":4},
            {"id":"applied","document_id":"EDI-IN-002","status":"applied"},
            {"id":"ignored","document_id":"EDI-IN-003","status":"ignored"},
            {"id":"processing","document_id":"EDI-IN-004","status":"processing"}
        ]
    })
}
fn snapshot(app: &App, data: Value) -> Snapshot {
    Snapshot {
        identity: demo::snapshot(&app.request()).identity,
        data,
    }
}
fn read(app: &mut App, data: Value) {
    let snapshot = snapshot(app, data);
    app.needs_fetch = false;
    app.finish(app.generation, Ok(snapshot));
}
fn app() -> App {
    let mut app = App {
        view: View::Integrations,
        needs_fetch: false,
        ..App::default()
    };
    read(&mut app, data());
    app
}
fn search(app: &mut App, query: &str) {
    key(app, KeyCode::Char('/'));
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste(query);
    key(app, KeyCode::Enter);
}
fn screen(app: &mut App, width: u16, no_color: bool, light: bool) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("terminal");
    terminal
        .draw(|frame| {
            ui::render(
                frame,
                app,
                &ui::Theme::new(no_color, light),
                true,
                "offline",
            )
        })
        .expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn attention_direction_and_search_combine_locally_without_reclassifying_completed_outcomes() {
    let mut app = app();
    let generation = app.generation;
    let updated = app.updated;
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.visible.len(), 3);
    assert!(app
        .visible
        .iter()
        .all(|i| matches!(app.rows[*i].status.as_str(), "retry" | "held" | "failed")));
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.visible.len(), 2);
    search(&mut app, "stateset-icommerce");
    assert_eq!(app.visible.len(), 1);
    assert_eq!(app.current().expect("native hold").id, "held-native");
    key(&mut app, KeyCode::Char('d'));
    assert!(app.visible.is_empty());
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.visible.len(), 10);
    assert_eq!(app.queue_direction, QueueDirection::All);
    assert!(!app.queue_attention);
    assert!(app.query.is_empty());
    assert_eq!(app.generation, generation);
    assert_eq!(app.updated, updated);
    assert!(!app.needs_fetch);
    assert!(app.write_ready.is_none());
}

#[test]
fn an_inbound_record_with_the_same_raw_id_keeps_its_selection_across_refresh_and_workspace_return()
{
    let mut app = app();
    app.selected = 6;
    assert_eq!(
        app.current().expect("inbound").direction,
        Some(QueueDirection::Inbound)
    );
    assert_eq!(app.current().expect("inbound").id, "shared-id");
    let mut response = data();
    response["outbox"].as_array_mut().expect("outbox").reverse();
    read(&mut app, response.clone());
    assert_eq!(app.current().expect("same inbound").cells[0], "EDI-IN-001");
    key(&mut app, KeyCode::Char('3'));
    app.needs_fetch = false;
    app.apply(demo::snapshot(&app.request()));
    key(&mut app, KeyCode::Char('5'));
    read(&mut app, response);
    assert_eq!(
        app.current().expect("restored inbound").direction,
        Some(QueueDirection::Inbound)
    );
    assert_eq!(
        app.current().expect("restored inbound").cells[0],
        "EDI-IN-001"
    );
}

#[test]
fn queue_filters_and_selected_handoff_survive_linked_shipment_navigation() {
    let mut app = app();
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    search(&mut app, "stateset-icommerce");
    key(&mut app, KeyCode::Char('o'));
    assert_eq!(app.view, View::Loads);
    app.needs_fetch = false;
    app.apply(demo::snapshot(&app.request()));
    assert_eq!(app.current().expect("shipment").id, "load-demo-0002");
    key(&mut app, KeyCode::Char('b'));
    read(&mut app, data());
    assert!(app.queue_attention);
    assert_eq!(app.queue_direction, QueueDirection::Outbound);
    assert_eq!(app.query, "stateset-icommerce");
    assert_eq!(app.current().expect("same handoff").id, "held-native");
    assert!(app.write_ready.is_none());
}

#[test]
fn access_denial_and_a_changed_tenant_or_subject_clear_queue_filters_and_navigation() {
    for changed in ["denied", "tenant", "subject"] {
        let mut app = app();
        key(&mut app, KeyCode::Char('f'));
        key(&mut app, KeyCode::Char('d'));
        search(&mut app, "held");
        if changed == "denied" {
            app.clear_access();
        } else {
            let mut response = snapshot(&app, data());
            if changed == "tenant" {
                response.identity["tenant_id"] = "other-tenant".into();
            } else {
                response.identity["access"]["subject"] = "other-subject".into();
            }
            app.finish(app.generation, Ok(response));
        }
        assert!(!app.queue_attention);
        assert_eq!(app.queue_direction, QueueDirection::All);
        assert!(app.query.is_empty());
        key(&mut app, KeyCode::Char('3'));
        key(&mut app, KeyCode::Char('5'));
        assert!(!app.queue_attention);
        assert_eq!(app.queue_direction, QueueDirection::All);
    }
}

#[test]
fn a_pending_or_scheduled_read_uses_the_current_local_filters_and_retains_scoped_selection() {
    let mut app = app();
    app.selected = 1;
    key(&mut app, KeyCode::Char('r'));
    // Selection can change while the retained snapshot is being refreshed.
    app.selected = 1;
    let generation = app.generation;
    app.loading = Some(generation);
    app.needs_fetch = false;
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    let mut response = data();
    response["outbox"].as_array_mut().expect("outbox").reverse();
    read(&mut app, response);
    assert_eq!(app.visible.len(), 2);
    assert_eq!(app.queue_direction, QueueDirection::Outbound);
    assert_eq!(
        app.current().expect("same handoff after read").id,
        "held-native"
    );
    app.refresh_interval = Some(Duration::from_secs(5));
    app.tick(Instant::now() + Duration::from_secs(6));
    assert!(app.needs_fetch);
    assert_eq!(app.generation, generation + 1);
    read(&mut app, data());
    assert!(app.queue_attention);
    assert_eq!(app.queue_direction, QueueDirection::Outbound);
    assert_eq!(app.visible.len(), 2);
    assert_eq!(
        app.current().expect("same handoff after scheduled read").id,
        "held-native"
    );
    assert!(app.write_ready.is_none());
}

#[test]
fn queue_filter_shortcuts_do_not_escape_search_paste_or_inspection() {
    let mut app = app();
    app.paste("fdq");
    assert!(!app.queue_attention && !app.quit);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.draft, "fd");
    assert!(!app.queue_attention);
    assert_eq!(app.queue_direction, QueueDirection::All);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    assert!(app.details);
    assert!(!app.queue_attention);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('1'));
    let generation = app.generation;
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    assert_eq!(app.generation, generation);
    assert!(!app.queue_attention);
}

#[test]
fn queue_details_distinguish_cycle_attempts_total_attempts_transmission_and_lease_without_payloads()
{
    let app = app();
    let row = app.current().expect("retry");
    for (key, expected) in [
        ("Direction", "Outbound"),
        ("Total attempts", "10"),
        ("Attempts this cycle", "2"),
        ("Transmission", "unknown"),
        ("Lease until", "2026-10-19T18:00:00Z"),
    ] {
        assert!(
            row.details.iter().any(|(k, v)| k == key && v == expected),
            "missing {key}"
        );
    }
    assert!(row
        .details
        .iter()
        .any(|(k, v)| k == "Recovery" && v.contains("read-only")));
    let details = format!("{:?}", row.details);
    assert!(!details.contains("PRIVATE-PAYLOAD-CANARY"));
    assert!(!details.contains("PRIVATE-RESPONSE-CANARY"));
    assert!(!details.contains('\u{202e}'));
    assert!(!row.details.iter().any(|(_, v)| v.contains('\u{1b}')));
    let applied = app
        .rows
        .iter()
        .find(|row| row.id == "applied")
        .expect("applied record");
    assert!(applied
        .details
        .iter()
        .any(|(k, v)| k == "Total attempts" && v == "—"));
    assert!(applied
        .details
        .iter()
        .any(|(k, v)| k == "Attempts this cycle" && v == "—"));
}

#[test]
fn compact_queue_filters_and_empty_result_recovery_fit_in_all_palettes() {
    for width in [52, 77, 120] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = app();
            key(&mut app, KeyCode::Char('f'));
            key(&mut app, KeyCode::Char('d'));
            key(&mut app, KeyCode::Char('d'));
            search(&mut app, &"z".repeat(200));
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("[Needs attention · Inbound]"));
            assert!(text.contains("Backspace clears filters"));
            let footer = text.lines().last().expect("footer");
            assert!(
                footer.contains("f attention")
                    && footer.contains("d direction")
                    && footer.contains("? q")
            );
        }
    }
}

#[test]
fn queue_filtering_and_clear_preserve_the_uncertain_original_body_key_and_warning() {
    let mut app = app();
    let load = demo::snapshot(&App::default().request()).data["loads"][0].clone();
    app.pending = Some(
        Pending::new(
            crate::actions::Target::from_load(&load).expect("target"),
            Owner::from_identity(&app.identity).expect("owner"),
            Kind::Location,
            "Owned location",
        )
        .expect("pending"),
    );
    app.write_uncertain = true;
    app.write_notice = Some("Original outcome remains uncertain".into());
    let original = serde_json::to_value(app.pending.as_ref().expect("pending")).expect("original");
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('d'));
    search(&mut app, "stateset-icommerce");
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("retained"),
        original
    );
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original outcome remains uncertain")
    );
    assert!(app.write_uncertain);
    assert!(app.write_ready.is_none());
    assert!(!app.needs_fetch);
}
