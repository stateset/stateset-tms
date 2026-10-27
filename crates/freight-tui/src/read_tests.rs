use crate::{
    actions::{Kind, Owner, Pending},
    client::Failure,
    demo,
    model::{App, BalanceFilter, QueueDirection, View},
    operations::Scope,
    palette::Command,
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;
use std::time::{Duration, Instant};
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}
fn retry(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
}
fn read(app: &mut App) {
    app.needs_fetch = false;
    app.finish(app.generation, Ok(demo::snapshot(&app.request())));
}
fn loaded() -> App {
    let mut app = App::default();
    read(&mut app);
    app
}
fn second_page() -> App {
    let mut app = loaded();
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    app.selected = 3;
    *app.table.offset_mut() = 2;
    app
}
fn screen(app: &mut App, width: u16, no_color: bool, light: bool) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).expect("terminal");
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
fn current_page_read_preserves_cursor_selection_viewport_and_restores_identity_after_reorder() {
    let mut app = second_page();
    let id = app.current().expect("load").id.clone();
    let cursor = app.request().cursor;
    let updated = app.updated;
    let generation = app.generation;
    retry(&mut app);
    assert_eq!(app.request().cursor, cursor);
    assert_eq!(app.selected, 3);
    assert_eq!(app.table.offset(), 2);
    assert_eq!(app.updated, updated);
    assert_eq!(app.rows.len(), 15);
    assert_eq!(app.generation, generation + 1);
    assert!(app.needs_fetch);
    let mut response = demo::snapshot(&app.request());
    response.data["loads"]
        .as_array_mut()
        .expect("loads")
        .reverse();
    app.needs_fetch = false;
    app.finish(app.generation, Ok(response));
    assert_eq!(app.current().expect("same load").id, id);
    assert_eq!(app.request().cursor, cursor);
    assert!(app.write_ready.is_none());
}
#[test]
fn failed_return_read_keeps_its_page_and_selection_anchor_through_explicit_retry() {
    let mut app = second_page();
    let id = app.current().expect("load").id.clone();
    key(&mut app, KeyCode::Char('3'));
    read(&mut app);
    key(&mut app, KeyCode::Char('1'));
    app.needs_fetch = false;
    app.finish(
        app.generation,
        Err(Failure {
            message: "Owned offline diagnostic".into(),
            denied: false,
        }),
    );
    assert!(app.rows.is_empty());
    assert_eq!(app.selection_anchor.as_deref(), Some(id.as_str()));
    app.refresh_interval = Some(Duration::from_secs(5));
    app.tick(Instant::now() + Duration::from_secs(6));
    assert!(!app.needs_fetch);
    retry(&mut app);
    assert_eq!(app.request().cursor.as_deref(), Some("50"));
    read(&mut app);
    assert_eq!(app.current().expect("restored").id, id);
    assert!(app.error.is_none());
}
#[test]
fn current_read_coalesces_queued_or_running_reads_and_ignores_key_repeat() {
    let mut app = second_page();
    let generation = app.generation;
    retry(&mut app);
    retry(&mut app);
    assert_eq!(app.generation, generation + 1);
    app.loading = Some(app.generation);
    app.needs_fetch = false;
    retry(&mut app);
    assert_eq!(app.generation, generation + 1);
    read(&mut app);
    app.key(KeyEvent::new_with_kind(
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
        KeyEventKind::Repeat,
    ));
    assert_eq!(app.generation, generation + 1);
    assert!(!app.needs_fetch);
}
#[test]
fn retry_keeps_work_scope_local_filters_and_linked_shipment_context() {
    for view in [View::Operations, View::Billing, View::Integrations] {
        let mut app = App {
            view,
            ..App::default()
        };
        read(&mut app);
        match view {
            View::Operations => {
                app.set_operations_scope(Scope::Mine);
                read(&mut app);
                app.cursors.push(Some("OWNED-CURSOR".into()));
            }
            View::Billing => app.set_balance_filter(BalanceFilter::Overdue),
            _ => {
                app.queue_attention = true;
                app.queue_direction = QueueDirection::Outbound;
                app.filter();
            }
        }
        let query = app.query.clone();
        let scope = app.operations_scope;
        let balance = app.balance_filter;
        let direction = app.queue_direction;
        let cursor = app.request().cursor;
        retry(&mut app);
        assert_eq!(app.query, query);
        assert_eq!(app.operations_scope, scope);
        assert_eq!(app.balance_filter, balance);
        assert_eq!(app.queue_direction, direction);
        assert_eq!(app.request().cursor, cursor);
        assert!(app.needs_fetch);
    }
    let mut app = App {
        view: View::Operations,
        ..App::default()
    };
    read(&mut app);
    key(&mut app, KeyCode::Char('o'));
    read(&mut app);
    let id = app.current().expect("linked").id.clone();
    retry(&mut app);
    read(&mut app);
    assert_eq!(app.focused_load.as_deref(), Some(id.as_str()));
    assert_eq!(app.related_from, Some(View::Operations));
    assert_eq!(app.current().expect("same linked").id, id);
}
#[test]
fn connection_refresh_controls_keep_panel_open_and_distinguish_current_and_first_page() {
    let mut app = second_page();
    key(&mut app, KeyCode::Char('i'));
    app.detail_scroll = 7;
    retry(&mut app);
    assert!(app.connection);
    assert_eq!(app.detail_scroll, 7);
    assert_eq!(app.request().cursor.as_deref(), Some("50"));
    assert_eq!(app.selected, 3);
    read(&mut app);
    key(&mut app, KeyCode::Char('R'));
    assert_eq!(app.refresh_interval, Some(Duration::from_secs(30)));
    assert!(app.connection);
    assert_eq!(app.refresh_pause(), Some("inspecting"));
    key(&mut app, KeyCode::Char('r'));
    assert!(app.connection);
    assert!(app.request().cursor.is_none());
    assert_eq!(app.selected, 0);
    assert_eq!(app.detail_scroll, 0);
    assert!(app.rows.is_empty());
    read(&mut app);
    assert_eq!(app.current().expect("first").cells[0], "FRT-10421");
    key(&mut app, KeyCode::Char('R'));
    assert!(app.refresh_interval.is_none());
}
#[test]
fn palette_exposes_current_page_refresh_without_restarting_the_cursor() {
    let mut app = second_page();
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste("refresh current");
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].command, Command::RefreshPage);
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_none());
    assert_eq!(app.request().cursor.as_deref(), Some("50"));
    assert_eq!(app.selected, 3);
    assert!(app.needs_fetch);
}
#[test]
fn read_shortcut_never_escapes_text_editing_inspection_or_an_action_review() {
    let mut app = loaded();
    let generation = app.generation;
    app.paste("r");
    assert_eq!(app.generation, generation);
    for opener in [
        KeyCode::Char('/'),
        KeyCode::Enter,
        KeyCode::Char('?'),
        KeyCode::Char('a'),
    ] {
        key(&mut app, opener);
        retry(&mut app);
        assert_eq!(app.generation, generation);
        assert!(!app.needs_fetch);
        key(&mut app, KeyCode::Esc);
    }
}
#[test]
fn current_page_failure_stays_stale_and_denial_or_changed_owner_clears_private_context() {
    let mut app = second_page();
    retry(&mut app);
    app.needs_fetch = false;
    app.finish(
        app.generation,
        Err(Failure {
            message: "Owned timeout".into(),
            denied: false,
        }),
    );
    assert!(app.stale());
    assert_eq!(app.current().expect("retained").cells[0], "FRT-10474");
    assert_eq!(app.request().cursor.as_deref(), Some("50"));
    assert!(!app.needs_fetch);
    for outcome in ["denied", "tenant", "subject"] {
        let mut app = second_page();
        app.query = "private query".into();
        retry(&mut app);
        app.needs_fetch = false;
        if outcome == "denied" {
            app.finish(
                app.generation,
                Err(Failure {
                    message: "Access denied".into(),
                    denied: true,
                }),
            );
        } else {
            let mut response = demo::snapshot(&app.request());
            if outcome == "tenant" {
                response.identity["tenant_id"] = json!("other");
            } else {
                response.identity["access"]["subject"] = json!("other");
            }
            app.finish(app.generation, Ok(response));
        }
        assert!(app.rows.is_empty());
        assert!(app.query.is_empty());
        assert!(app.request().cursor.is_none());
    }
}
#[test]
fn reads_keep_complete_uncertain_original_and_retained_stop_decision() {
    let mut app = second_page();
    let target = app.current().expect("load").target.clone().expect("target");
    app.pending = Some(
        Pending::new(
            target,
            Owner::from_identity(&app.identity).expect("owner"),
            Kind::Exception,
            "Owned observation",
        )
        .expect("pending"),
    );
    app.stopping_note = Some("Retained stop decision".into());
    app.write_uncertain = true;
    app.write_notice = Some("Original remains uncertain".into());
    let original = serde_json::to_value(app.pending.as_ref().expect("pending")).expect("original");
    retry(&mut app);
    read(&mut app);
    key(&mut app, KeyCode::Char('i'));
    retry(&mut app);
    read(&mut app);
    key(&mut app, KeyCode::Char('r'));
    read(&mut app);
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("retained"),
        original
    );
    assert_eq!(app.stopping_note.as_deref(), Some("Retained stop decision"));
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original remains uncertain")
    );
    assert!(app.write_uncertain && app.write_ready.is_none());
}
#[test]
fn compact_error_hints_and_connection_read_target_are_visible_in_all_palettes() {
    for width in [52, 77, 120] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = second_page();
            app.error = Some("Owned offline diagnostic".into());
            let text = screen(&mut app, width, no_color, light);
            let footer = text.lines().last().expect("footer");
            assert!(
                footer.contains("Ctrl+R retry page")
                    && footer.contains("r first page")
                    && footer.contains("? q")
            );
            key(&mut app, KeyCode::Char('i'));
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("Read target: Loads · page 2"));
            assert!(text.contains("Read status: Idle"));
            key(&mut app, KeyCode::End);
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("Ctrl+R rereads this page"));
            assert!(text.contains("R toggles automatic reads"));
        }
    }
}
