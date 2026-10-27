use crate::{
    actions::{Kind, Owner, Pending},
    demo,
    model::{App, BalanceFilter, QueueDirection, View},
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
fn open(app: &mut App) {
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
}
fn read(app: &mut App) {
    app.needs_fetch = false;
    app.finish(app.generation, Ok(demo::snapshot(&app.request())));
}
fn app(view: View) -> App {
    let mut app = App {
        view,
        ..App::default()
    };
    read(&mut app);
    app
}
fn choose(app: &mut App, query: &str, expected: Command) {
    open(app);
    app.paste(query);
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1, "ambiguous {query}");
    assert_eq!(choices[0].command, expected);
    key(app, KeyCode::Enter);
    assert!(app.palette.is_none());
}
fn screen(
    app: &mut App,
    width: u16,
    height: u16,
    no_color: bool,
    light: bool,
) -> (String, (u16, u16)) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
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
    let cursor = terminal.get_cursor_position().expect("cursor");
    let text = terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (text, (cursor.x, cursor.y))
}

#[test]
fn palette_search_combines_case_insensitive_words_and_workspace_aliases_without_reads() {
    let mut app = app(View::Loads);
    let generation = app.generation;
    let updated = app.updated;
    open(&mut app);
    app.paste("QUEUES go");
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].command, Command::Workspace(View::Integrations));
    assert_eq!(app.generation, generation);
    assert_eq!(app.updated, updated);
    assert!(!app.needs_fetch);
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.view, View::Loads);
    assert!(app.palette.is_none());
}

#[test]
fn direct_status_choice_resets_only_current_page_and_keeps_search_and_other_workspace_memory() {
    let mut app = app(View::Loads);
    app.query = "FRT".into();
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    assert_eq!(app.cursors.len(), 2);
    let generation = app.generation;
    choose(&mut app, "status transit", Command::Status(5));
    assert_eq!(app.status_index, 5);
    assert_eq!(app.query, "FRT");
    assert_eq!(app.cursors.len(), 1);
    assert!(app.rows.is_empty());
    assert!(app.needs_fetch);
    assert_eq!(app.generation, generation + 1);
    read(&mut app);
    assert!(app.rows.iter().all(|r| r.status == "in_transit"));
    choose(&mut app, "go invoice", Command::Workspace(View::Billing));
    read(&mut app);
    choose(&mut app, "go catalog", Command::Workspace(View::Loads));
    read(&mut app);
    assert_eq!(app.status_index, 5);
    assert_eq!(app.query, "FRT");
}

#[test]
fn direct_balance_and_queue_choices_reuse_current_snapshot_and_remain_contextual() {
    let mut app = app(View::Billing);
    let generation = app.generation;
    choose(
        &mut app,
        "balance overdue",
        Command::Balance(BalanceFilter::Overdue),
    );
    assert_eq!(app.current().expect("invoice").id, "bill-demo-2");
    assert_eq!(app.generation, generation);
    assert!(!app.needs_fetch);
    choose(&mut app, "clear filters", Command::ClearFilters);
    assert_eq!(app.balance_filter, BalanceFilter::All);
    choose(&mut app, "go payables", Command::Workspace(View::Payables));
    read(&mut app);
    choose(
        &mut app,
        "balance carrier",
        Command::Balance(BalanceFilter::Credits),
    );
    assert_eq!(app.current().expect("credit").id, "settle-demo-3");
    open(&mut app);
    app.paste("overdue");
    assert!(app.palette_choices().is_empty());
    key(&mut app, KeyCode::Esc);
    choose(
        &mut app,
        "go queues",
        Command::Workspace(View::Integrations),
    );
    read(&mut app);
    let generation = app.generation;
    choose(&mut app, "states attention", Command::Attention(true));
    choose(
        &mut app,
        "direction inbound",
        Command::Direction(QueueDirection::Inbound),
    );
    assert!(app.visible.is_empty());
    assert_eq!(app.generation, generation);
    assert!(!app.needs_fetch);
}

#[test]
fn unavailable_commands_cannot_open_details_or_erase_filters_and_empty_results_are_inert() {
    let mut app = app(View::Loads);
    open(&mut app);
    app.paste("related shipment");
    assert!(!app.palette_choices()[0].enabled);
    let generation = app.generation;
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_some());
    assert_eq!(app.generation, generation);
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste("NO-SUCH-COMMAND");
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_some());
    assert!(!app.needs_fetch);
    assert!(app.write_ready.is_none());
}

#[test]
fn palette_navigation_scrolls_and_is_bounded_after_a_query_changes_or_a_resize() {
    let mut app = app(View::Loads);
    open(&mut app);
    key(&mut app, KeyCode::End);
    let count = app.palette_choices().len();
    assert_eq!(app.palette.as_ref().expect("palette").selected, count - 1);
    let (text, _) = screen(&mut app, 52, 16, false, false);
    assert!(text.contains("Shipment status: cancelled"));
    assert!(app.palette.as_ref().expect("palette").list.offset() > 0);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.palette.as_ref().expect("palette").selected, count - 1);
    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.palette.as_ref().expect("palette").selected, count - 9);
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.palette.as_ref().expect("palette").selected, 0);
    app.paste("cancelled");
    let (text, _) = screen(&mut app, 120, 30, false, false);
    assert!(text.contains("1/1"));
    assert_eq!(app.palette.as_ref().expect("palette").list.offset(), 0);
}

#[test]
fn palette_text_is_grapheme_safe_byte_bounded_and_paste_or_repeats_cannot_execute_shortcuts() {
    let mut app = app(View::Loads);
    open(&mut app);
    app.paste("qacxy\u{001b}\u{202e}e\u{0301}👩‍🚀");
    assert_eq!(
        app.palette.as_ref().expect("palette").query,
        "qacxye\u{0301}👩‍🚀"
    );
    assert!(!app.quit && app.action_dialog.is_none() && app.write_ready.is_none());
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.palette.as_ref().expect("palette").query, "qacxy");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste(&"é".repeat(99));
    app.paste("👩‍🚀suffix");
    assert_eq!(app.palette.as_ref().expect("palette").query.len(), 198);
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste("go queues");
    app.key(KeyEvent::new_with_kind(
        KeyCode::Enter,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    ));
    assert_eq!(app.view, View::Loads);
    assert!(app.palette.is_some());
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.view, View::Integrations);
}

#[test]
fn palette_cannot_replace_an_active_editor_inspector_or_action_review() {
    let mut app = app(View::Loads);
    key(&mut app, KeyCode::Char('/'));
    open(&mut app);
    assert!(app.searching && app.palette.is_none());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    open(&mut app);
    assert!(app.details && app.palette.is_none());
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('a'));
    open(&mut app);
    assert!(app.action_dialog.is_some() && app.palette.is_none());
    key(&mut app, KeyCode::Esc);
    open(&mut app);
    open(&mut app);
    assert!(app.palette.is_none());
}

#[test]
fn palette_pauses_scheduled_reads_and_owner_changes_clear_query_and_selection() {
    for owner in ["denied", "tenant", "subject"] {
        let mut app = app(View::Loads);
        app.refresh_interval = Some(Duration::from_secs(5));
        open(&mut app);
        app.paste("private query");
        app.tick(Instant::now() + Duration::from_secs(6));
        assert!(!app.needs_fetch);
        assert_eq!(app.refresh_pause(), Some("inspecting"));
        if owner == "denied" {
            app.clear_access();
        } else {
            let mut snapshot = demo::snapshot(&app.request());
            if owner == "tenant" {
                snapshot.identity["tenant_id"] = json!("other");
            } else {
                snapshot.identity["access"]["subject"] = json!("other");
            }
            app.finish(app.generation, Ok(snapshot));
        }
        assert!(app.palette.is_none());
        assert!(app.rows.is_empty());
    }
}

#[test]
fn command_palette_is_read_only_and_keeps_original_save_bytes_and_quit_warning() {
    let mut app = app(View::Loads);
    let load = demo::snapshot(&app.request()).data["loads"][0].clone();
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
    app.write_notice = Some("Original remains uncertain".into());
    let original = serde_json::to_value(app.pending.as_ref().expect("pending")).expect("original");
    open(&mut app);
    app.paste("caxy");
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_some());
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("pending")).expect("retained"),
        original
    );
    assert!(app.write_ready.is_none());
    assert!(app.action_dialog.is_none());
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original remains uncertain")
    );
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.palette.is_none());
    assert!(app.quit_confirm && !app.quit);
    key(&mut app, KeyCode::Esc);
    assert!(app.pending.is_some() && app.write_uncertain);
}

#[test]
fn command_palette_renders_all_palettes_and_keeps_long_unicode_input_caret_visible() {
    for (width, height) in [(52, 16), (77, 24), (120, 30)] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = app(View::Loads);
            open(&mut app);
            app.paste(&"界".repeat(66));
            let (text, (x, y)) = screen(&mut app, width, height, no_color, light);
            assert!(text.contains("Commands") && text.contains("No commands match"));
            assert!(text.contains("Ctrl+U clears search"));
            assert!(text.contains('‹') && text.contains('▏'));
            assert!(x < width && y < height);
            let line = text.lines().nth(y as usize).expect("caret line");
            assert!(line.contains('▏'));
            assert!(text.contains("0/0"));
        }
    }
}

#[test]
fn inspection_search_read_and_related_palette_commands_use_existing_navigation() {
    let mut app = app(View::Billing);
    choose(&mut app, "find query", Command::Search);
    assert!(app.searching);
    key(&mut app, KeyCode::Esc);
    choose(&mut app, "inspect selected", Command::Details);
    assert!(app.details);
    key(&mut app, KeyCode::Esc);
    choose(&mut app, "inspect connection", Command::Connection);
    assert!(app.connection);
    key(&mut app, KeyCode::Esc);
    choose(&mut app, "keyboard help", Command::Help);
    assert!(app.help);
    key(&mut app, KeyCode::Esc);
    choose(&mut app, "toggle automatic", Command::AutoReads);
    assert_eq!(app.refresh_interval, Some(Duration::from_secs(30)));
    choose(&mut app, "toggle automatic", Command::AutoReads);
    assert!(app.refresh_interval.is_none());
    let generation = app.generation;
    choose(&mut app, "refresh first", Command::Refresh);
    assert!(app.needs_fetch);
    assert_eq!(app.generation, generation + 1);
    read(&mut app);
    choose(&mut app, "related shipment", Command::Related);
    assert_eq!(app.view, View::Loads);
    read(&mut app);
    assert_eq!(app.current().expect("related").id, "load-demo-0003");
    choose(&mut app, "return source", Command::Return);
    read(&mut app);
    assert_eq!(app.view, View::Billing);
    assert_eq!(app.current().expect("bill").id, "bill-demo-1");
    assert!(app.write_ready.is_none());
}

#[test]
fn selecting_active_choices_preserves_snapshot_selection_and_viewport() {
    for (view, query, command) in [
        (View::Loads, "status all", Command::Status(0)),
        (
            View::Billing,
            "balance all",
            Command::Balance(BalanceFilter::All),
        ),
        (View::Integrations, "states all", Command::Attention(false)),
        (
            View::Integrations,
            "direction all",
            Command::Direction(QueueDirection::All),
        ),
    ] {
        let mut app = app(view);
        app.selected = 1;
        *app.table.offset_mut() = 1;
        let updated = app.updated;
        let generation = app.generation;
        let id = app.current().expect("row").selection_key();
        choose(&mut app, query, command);
        assert_eq!(app.current().expect("retained").selection_key(), id);
        assert_eq!(app.selected, 1);
        assert_eq!(app.table.offset(), 1);
        assert_eq!(app.generation, generation);
        assert_eq!(app.updated, updated);
        assert!(!app.needs_fetch);
    }
}

#[test]
fn a_fresh_response_revalidates_palette_availability_before_enter() {
    let mut app = app(View::Loads);
    open(&mut app);
    app.paste("inspect selected");
    assert!(app.palette_choices()[0].enabled);
    let mut response = demo::snapshot(&app.request());
    response.data["loads"] = json!([]);
    app.finish(app.generation, Ok(response));
    assert!(!app.palette_choices()[0].enabled);
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_some());
    assert!(!app.details && !app.needs_fetch);
    key(&mut app, KeyCode::Esc);
    let mut response = demo::snapshot(&app.request());
    response.data["loads"] = json!([]);
    app.finish(app.generation, Ok(response));
    open(&mut app);
    let (text, _) = screen(&mut app, 52, 16, false, false);
    assert!(text.contains("(unavailable)"));
}
