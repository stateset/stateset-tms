use crate::{
    client::Snapshot,
    config::Config,
    demo,
    model::{App, View},
    operations::Scope,
    palette::Command,
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;
use std::time::{Duration, Instant};
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn read(app: &mut App) {
    app.needs_fetch = false;
    app.finish(app.generation, Ok(demo::snapshot(&app.request())));
}
fn operations_app() -> App {
    let mut app = App {
        view: View::Operations,
        ..App::default()
    };
    read(&mut app);
    app
}
fn screen(app: &mut App, width: u16, no_color: bool, light: bool) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("terminal");
    terminal
        .draw(|f| ui::render(f, app, &ui::Theme::new(no_color, light), true, "offline"))
        .expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn work_scope_changes_request_a_fresh_first_page_keep_search_and_fence_old_scope_responses() {
    let mut app = operations_app();
    app.query = "evidence".into();
    app.cursors.push(Some("OLD-CURSOR".into()));
    app.next_cursor = Some("NEXT".into());
    app.selected = 2;
    let old = demo::snapshot(&app.request());
    let generation = app.generation;
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.operations_scope, Scope::Active);
    assert_eq!(app.query, "evidence");
    assert_eq!(app.cursors, vec![None]);
    assert!(app.next_cursor.is_none());
    assert_eq!(app.selected, 0);
    assert!(app.rows.is_empty());
    assert!(app.updated.is_none());
    assert!(app.needs_fetch);
    assert_eq!(app.generation, generation + 1);
    app.finish(generation, Ok(old));
    assert!(app.rows.is_empty());
    assert_eq!(app.operations_scope, Scope::Active);
    read(&mut app);
    assert_eq!(app.current().expect("searched issue").id, "issue-demo-1");
    assert_eq!(app.request().operations_scope, Scope::Active);
    assert!(app.write_ready.is_none());
}
#[test]
fn demo_scopes_follow_api_ownership_and_deferral_rules_without_changing_default_order() {
    let mut app = operations_app();
    for (scope, ids) in [
        (
            Scope::Actionable,
            vec!["issue-demo-0", "issue-demo-1", "issue-demo-2"],
        ),
        (
            Scope::Active,
            vec![
                "issue-demo-0",
                "issue-demo-1",
                "issue-demo-2",
                "issue-demo-3",
            ],
        ),
        (Scope::Mine, vec!["issue-demo-2", "issue-demo-3"]),
        (Scope::Unowned, vec!["issue-demo-1"]),
        (Scope::Deferred, vec!["issue-demo-3"]),
        (Scope::Inactive, vec!["issue-demo-4"]),
        (
            Scope::All,
            vec![
                "issue-demo-0",
                "issue-demo-1",
                "issue-demo-2",
                "issue-demo-3",
                "issue-demo-4",
            ],
        ),
    ] {
        app.set_operations_scope(scope);
        read(&mut app);
        assert_eq!(
            app.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ids,
            "{scope:?}"
        );
        key(&mut app, KeyCode::Char('f'));
    }
    assert_eq!(app.operations_scope, Scope::Actionable);
}
#[test]
fn current_scope_selection_is_a_noop_and_palette_scope_choices_are_contextual() {
    let mut app = operations_app();
    app.selected = 1;
    *app.table.offset_mut() = 1;
    let generation = app.generation;
    let updated = app.updated;
    app.set_operations_scope(Scope::Actionable);
    assert_eq!(app.selected, 1);
    assert_eq!(app.table.offset(), 1);
    assert_eq!(app.generation, generation);
    assert_eq!(app.updated, updated);
    assert!(!app.needs_fetch);
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste("work mine");
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].command, Command::WorkScope(Scope::Mine));
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_none());
    assert_eq!(app.operations_scope, Scope::Mine);
    assert!(app.needs_fetch);
    key(&mut app, KeyCode::Char('3'));
    app.set_operations_scope(Scope::All);
    assert_eq!(app.operations_scope, Scope::Actionable);
}
#[test]
fn scope_page_selection_and_search_return_after_switching_or_opening_a_related_shipment() {
    let mut app = operations_app();
    app.set_operations_scope(Scope::Mine);
    read(&mut app);
    app.selected = 1;
    key(&mut app, KeyCode::Char('o'));
    read(&mut app);
    assert_eq!(app.current().expect("load").id, "load-demo-0003");
    key(&mut app, KeyCode::Char('b'));
    read(&mut app);
    assert_eq!(app.operations_scope, Scope::Mine);
    assert_eq!(app.current().expect("retained").id, "issue-demo-3");
    app.query = "carrier".into();
    app.cursors.push(Some("OWNED-SECOND-PAGE".into()));
    key(&mut app, KeyCode::Char('3'));
    read(&mut app);
    key(&mut app, KeyCode::Char('2'));
    assert_eq!(app.operations_scope, Scope::Mine);
    assert_eq!(app.query, "carrier");
    assert_eq!(
        app.cursors.last().and_then(|s| s.as_deref()),
        Some("OWNED-SECOND-PAGE")
    );
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.operations_scope, Scope::Actionable);
    assert!(app.query.is_empty());
    assert_eq!(app.cursors, vec![None]);
    assert!(app.needs_fetch);
}
#[test]
fn refreshed_current_scope_preserves_selection_while_owner_changes_clear_cached_scope() {
    let mut app = operations_app();
    app.set_operations_scope(Scope::Mine);
    read(&mut app);
    app.selected = 1;
    app.refresh_interval = Some(Duration::from_secs(5));
    app.tick(Instant::now() + Duration::from_secs(6));
    assert!(app.needs_fetch);
    read(&mut app);
    assert_eq!(app.current().expect("same").id, "issue-demo-3");
    assert_eq!(app.operations_scope, Scope::Mine);
    for changed in ["denied", "tenant", "subject"] {
        let mut app = operations_app();
        app.set_operations_scope(Scope::Mine);
        read(&mut app);
        key(&mut app, KeyCode::Char('3'));
        read(&mut app);
        if changed == "denied" {
            app.clear_access();
        } else {
            let mut response = demo::snapshot(&app.request());
            if changed == "tenant" {
                response.identity["tenant_id"] = json!("other");
            } else {
                response.identity["access"]["subject"] = json!("other");
            }
            app.finish(app.generation, Ok(response));
        }
        key(&mut app, KeyCode::Char('2'));
        assert_eq!(app.operations_scope, Scope::Actionable);
        assert!(app.rows.is_empty());
    }
}
#[test]
fn operation_inspection_separates_source_deadline_work_follow_up_and_inactive_condition() {
    let mut app = operations_app();
    let row = app.current().expect("owned issue");
    assert_eq!(row.cells[4], "2026-10-19T18:00:00Z");
    for (label, value) in [
        ("Source deadline", "2026-10-19T18:00:00Z"),
        ("Follow-up at", "2026-10-20T12:00:00Z"),
        ("Work status", "working"),
        ("Work version", "2"),
        ("Deferred until", "—"),
        ("Deferred", "false"),
    ] {
        assert!(
            row.details.iter().any(|(k, v)| k == label && v == value),
            "missing {label}"
        );
    }
    app.set_operations_scope(Scope::Inactive);
    read(&mut app);
    let row = app.current().expect("inactive");
    assert_eq!(row.status, "inactive");
    assert!(!row.needs_attention());
    assert!(row
        .details
        .iter()
        .any(|(k, v)| k == "Work status" && v == "working"));
    app.set_operations_scope(Scope::Deferred);
    read(&mut app);
    let row = app.current().expect("deferred");
    assert_eq!(row.status, "deferred");
    assert!(!row.needs_attention());
    let mut response = demo::snapshot(&app.request());
    response.data["issues"][0]["case"] = json!({"status":"claimed","notes":"PRIVATE-NOTE-CANARY","history":["PRIVATE-HISTORY-CANARY"]});
    app.finish(app.generation, Ok(response));
    let details = format!("{:?}", app.current().expect("issue").details);
    assert!(!details.contains("CANARY"));
    assert!(details.contains("Follow-up at"));
}
#[test]
fn work_scope_shortcuts_cannot_escape_search_details_or_paste() {
    let mut app = operations_app();
    app.paste("f");
    assert_eq!(app.operations_scope, Scope::Actionable);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.draft, "f");
    assert_eq!(app.operations_scope, Scope::Actionable);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('f'));
    assert!(app.details);
    assert_eq!(app.operations_scope, Scope::Actionable);
}
#[test]
fn scope_headers_and_clear_controls_fit_compact_widths_in_all_palettes() {
    for width in [52, 77, 120] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = operations_app();
            app.set_operations_scope(Scope::Mine);
            read(&mut app);
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("[Assigned to me]"));
            assert!(text.contains("f work view"));
            assert!(text.contains("Backspace clear  o shipment"));
            app.query = "z".repeat(200);
            let identity = demo::snapshot(&app.request()).identity;
            app.needs_fetch = false;
            app.finish(
                app.generation,
                Ok(Snapshot {
                    identity,
                    data: json!({"issues":[]}),
                }),
            );
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("[Assigned to me]"));
            assert!(text.contains("Backspace clears filters"));
        }
    }
}
#[test]
fn operations_scope_cli_is_explicit_and_never_echoes_unknown_values() {
    for scope in Scope::ALL {
        let config = Config::parse(
            [
                "--demo",
                "--view",
                "operations",
                "--operations-scope",
                scope.code(),
                "--snapshot",
            ]
            .map(String::from),
        )
        .expect("config")
        .expect("not help");
        assert_eq!(config.operations_scope, scope);
    }
    for args in [
        vec!["--demo", "--operations-scope", "mine"],
        vec![
            "--demo",
            "--view",
            "operations",
            "--operations-scope",
            "PRIVATE-INVALID-CANARY",
        ],
        vec!["--demo", "--view", "operations", "--operations-scope"],
    ] {
        let error = Config::parse(args.into_iter().map(String::from))
            .err()
            .expect("refused");
        assert!(!error.contains("CANARY"));
    }
    assert_eq!(Scope::Inactive.code(), "resolved");
    assert_eq!(Scope::Inactive.title(), "Source inactive");
}
#[test]
fn scope_changes_preserve_uncertain_original_bytes_stop_decision_and_warning() {
    use crate::actions::{Kind, Owner, Pending};
    let mut app = operations_app();
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
    app.stopping_note = Some("Retained stop decision".into());
    app.write_uncertain = true;
    app.write_notice = Some("Original outcome uncertain".into());
    let original = serde_json::to_value(app.pending.as_ref().expect("original")).expect("original");
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("retained"),
        original
    );
    assert_eq!(app.stopping_note.as_deref(), Some("Retained stop decision"));
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original outcome uncertain")
    );
    assert!(app.write_uncertain && app.write_ready.is_none());
}
