use crate::{
    actions::{Kind, Owner, Pending, WriteJob},
    config::Config,
    demo,
    model::{App, View},
    setup::Wizard,
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::time::{Duration, Instant};
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn app(view: View) -> App {
    let mut app = App {
        view,
        needs_fetch: false,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app
}
fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    t.draw(|f| {
        ui::render(
            f,
            app,
            &ui::Theme::new(false, false),
            false,
            "https://fixture.example",
        )
    })
    .expect("render");
    t.backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
fn pending(app: &App) -> Pending {
    Pending::new(
        app.current()
            .and_then(|r| r.target.clone())
            .expect("target"),
        Owner::from_identity(&app.identity).expect("owner"),
        Kind::Location,
        "Test location",
    )
    .expect("pending")
}
fn config() -> Config {
    let mut c = Config::parse(["--demo".into()])
        .expect("config")
        .expect("not help");
    c.demo = false;
    c
}
fn read_demo(a: &mut App) {
    a.needs_fetch = false;
    a.finish(a.generation, Ok(demo::snapshot(&a.request())));
}
fn search(a: &mut App, query: &str) {
    a.key(key(KeyCode::Char('/')));
    // Replace the draft as a user would before entering a different search.
    while !a.draft.is_empty() {
        a.key(key(KeyCode::Backspace));
    }
    a.paste(query);
    a.key(key(KeyCode::Enter));
}
#[test]
fn returning_to_a_workspace_restores_its_page_and_record_from_a_fresh_read() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('n')));
    read_demo(&mut a);
    a.selected = 10;
    screen(&mut a, 77, 24);
    let id = a.current().expect("selected load").id.clone();
    let cursor = a.request().cursor;
    let offset = a.table.offset();
    assert!(offset > 0);
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('1')));
    assert_eq!(a.cursors.len(), 2);
    assert_eq!(a.request().cursor, cursor);
    assert_eq!(a.table.offset(), offset);
    screen(&mut a, 77, 24);
    assert_eq!(
        a.table.offset(),
        offset,
        "loading must preserve the viewport"
    );
    assert!(a.rows.is_empty(), "old private snapshots are not cached");
    assert!(a.updated.is_none());
    assert!(a.needs_fetch);
    a.open_action();
    assert!(a.action_dialog.is_none(), "return requires a fresh read");
    // A reordered fresh response must restore the identity, not the old row number.
    let mut snapshot = demo::snapshot(&a.request());
    snapshot.data["loads"]
        .as_array_mut()
        .expect("loads")
        .reverse();
    a.needs_fetch = false;
    a.finish(a.generation, Ok(snapshot));
    assert_eq!(a.current().expect("restored load").id, id);
    assert_eq!(a.selected, 4);
    assert!(a.pending.is_none());
    assert!(a.write_ready.is_none());
}
#[test]
fn workspace_filters_are_independent_and_switching_while_loading_keeps_the_anchor() {
    let mut a = app(View::Loads);
    search(&mut a, "Dallas");
    a.key(key(KeyCode::Char('s')));
    read_demo(&mut a);
    a.selected = 1;
    let id = a.current().expect("filtered load").id.clone();
    a.key(key(KeyCode::Char('2')));
    search(&mut a, "POD");
    read_demo(&mut a);
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    search(&mut a, "INV-2026");
    a.key(key(KeyCode::Char('1')));
    assert_eq!(a.request().search, "Dallas");
    assert_eq!(a.request().status, "planned");
    // Leave again before the response; no empty table may erase the saved anchor.
    let old_generation = a.generation;
    let old_response = demo::snapshot(&a.request());
    a.key(key(KeyCode::Char('2')));
    assert_eq!(a.request().search, "POD");
    a.finish(old_generation, Ok(old_response));
    assert!(a.rows.is_empty());
    a.key(key(KeyCode::Char('3')));
    assert_eq!(a.query, "INV-2026");
    a.key(key(KeyCode::Char('1')));
    read_demo(&mut a);
    assert_eq!(a.current().expect("same filtered load").id, id);
    assert!(a.rows.iter().all(|r| r.status == "planned"));
}
#[test]
fn selecting_the_active_workspace_keeps_the_current_snapshot_and_selection() {
    let mut a = app(View::Loads);
    a.selected = 7;
    let generation = a.generation;
    let updated = a.updated;
    let id = a.current().expect("load").id.clone();
    a.key(key(KeyCode::Char('1')));
    assert_eq!(a.generation, generation);
    assert_eq!(a.updated, updated);
    assert_eq!(a.current().expect("same load").id, id);
    assert!(!a.needs_fetch);
}
#[test]
fn a_missing_remembered_record_selects_the_first_fresh_result() {
    let mut a = app(View::Loads);
    a.selected = 30;
    let id = a.current().expect("load").id.clone();
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('1')));
    let mut snapshot = demo::snapshot(&a.request());
    snapshot.data["loads"]
        .as_array_mut()
        .expect("loads")
        .retain(|v| v["id"] != id);
    a.needs_fetch = false;
    a.finish(a.generation, Ok(snapshot));
    assert_eq!(a.selected, 0);
    assert_ne!(a.current().expect("fresh load").id, id);
    assert!(screen(&mut a, 52, 24).contains("FRT-10421"));
}
#[test]
fn a_failed_return_read_keeps_the_anchor_for_an_explicit_retry() {
    let mut a = app(View::Loads);
    a.selected = 12;
    let id = a.current().expect("load").id.clone();
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('1')));
    a.needs_fetch = false;
    a.finish(
        a.generation,
        Err(crate::client::Failure {
            message: "Unavailable".into(),
            denied: false,
        }),
    );
    assert!(a.rows.is_empty());
    assert!(!a.needs_fetch, "failed reads do not retry automatically");
    a.key(key(KeyCode::Char('3')));
    a.key(key(KeyCode::Char('1')));
    read_demo(&mut a);
    assert_eq!(a.current().expect("restored load").id, id);
    assert!(a.write_ready.is_none());
}
#[test]
fn explicit_refresh_restarts_only_the_active_workspaces_page() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('n')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    search(&mut a, "INV-2026");
    a.key(key(KeyCode::Char('1')));
    a.key(key(KeyCode::Char('r')));
    assert!(a.request().cursor.is_none());
    assert_eq!(a.table.offset(), 0);
    assert_eq!(a.selected, 0);
    read_demo(&mut a);
    assert_eq!(a.current().expect("first page").cells[0], "FRT-10421");
    a.key(key(KeyCode::Char('3')));
    assert_eq!(a.query, "INV-2026");
    a.key(key(KeyCode::Char('1')));
    assert!(a.request().cursor.is_none(), "old page must not reappear");
}
#[test]
fn denial_clears_all_workspace_navigation_even_from_an_obsolete_read() {
    let mut a = app(View::Loads);
    search(&mut a, "Dallas");
    read_demo(&mut a);
    let generation = a.generation;
    a.key(key(KeyCode::Char('3')));
    read_demo(&mut a);
    search(&mut a, "INV-2026");
    a.finish(
        generation,
        Err(crate::client::Failure {
            message: "Denied".into(),
            denied: true,
        }),
    );
    assert!(a.rows.is_empty());
    assert!(a.query.is_empty());
    assert!(a.draft.is_empty());
    assert!(a.identity.is_null());
    a.key(key(KeyCode::Char('1')));
    assert!(a.request().search.is_empty());
    assert!(a.request().status.is_empty());
    assert!(a.request().cursor.is_none());
    a.key(key(KeyCode::Char('3')));
    assert!(a.query.is_empty());
}
#[test]
fn changing_tenant_or_operator_discards_navigation_and_rereads_without_old_filters() {
    for field in ["tenant", "operator"] {
        let mut a = app(View::Loads);
        search(&mut a, "Dallas");
        read_demo(&mut a);
        a.key(key(KeyCode::Char('3')));
        read_demo(&mut a);
        search(&mut a, "INV-2026");
        let mut snapshot = demo::snapshot(&a.request());
        if field == "tenant" {
            snapshot.identity["tenant_id"] = "different-tenant".into();
        } else {
            snapshot.identity["access"]["subject"] = "different-operator".into();
        }
        let generation = a.generation;
        a.needs_fetch = false;
        a.finish(generation, Ok(snapshot));
        assert!(a.needs_fetch);
        assert_ne!(a.generation, generation);
        assert!(a.rows.is_empty(), "old-filter response is discarded");
        assert!(a.finance.is_null());
        assert!(a.query.is_empty());
        assert!(!a.denied);
        a.key(key(KeyCode::Char('1')));
        assert!(a.request().search.is_empty());
        assert!(a.request().cursor.is_none());
        assert!(a.write_ready.is_none());
    }
}
#[test]
fn compact_controls_fit_and_describe_the_current_workspace() {
    for width in [52, 77, 80] {
        for view in View::ALL {
            let mut a = app(view);
            let text = screen(&mut a, width, 24);
            let footer = text.lines().rev().take(2).collect::<Vec<_>>();
            assert!(footer[0].contains("? q") || footer[0].contains("q quit"));
            assert!(footer[1].contains("Enter details"));
            assert!(text.contains("1/"));
            if view == View::Loads {
                assert!(footer[0].contains("a actions"));
            } else {
                assert!(!footer[0].contains("a actions"));
            }
        }
        let mut a = app(View::Loads);
        a.pending = Some(pending(&a));
        let text = screen(&mut a, width, 24);
        assert!(text
            .lines()
            .last()
            .expect("footer")
            .contains("x stop review"));
        search(&mut a, "Dallas");
        a.key(key(KeyCode::Char('/')));
        assert!(screen(&mut a, width, 24)
            .lines()
            .last()
            .expect("footer")
            .contains("Esc cancels"));
    }
}
#[test]
fn record_scrolling_stops_at_the_last_wrapped_line_and_supports_page_shortcuts() {
    for width in [52, 77, 120] {
        let mut a = app(View::Loads);
        a.rows[0]
            .details
            .push(("Long note".into(), "東 West ".repeat(100)));
        let id = a.current().expect("load").id.clone();
        a.key(key(KeyCode::Enter));
        let top = screen(&mut a, width, 24);
        assert!(top.contains("Home/End"));
        assert!(a.detail_max > a.detail_page);
        a.key(key(KeyCode::PageDown));
        assert_eq!(a.detail_scroll, a.detail_page);
        a.key(key(KeyCode::PageUp));
        assert_eq!(a.detail_scroll, 0);
        a.key(key(KeyCode::End));
        let bottom = screen(&mut a, width, 24);
        assert!(
            bottom.contains(&id),
            "last identity must be visible at {width}"
        );
        let end = a.detail_scroll;
        for _ in 0..300 {
            a.key(key(KeyCode::Down));
        }
        assert_eq!(a.detail_scroll, end);
        assert!(screen(&mut a, width, 24).contains(&id));
        a.key(key(KeyCode::Home));
        assert_eq!(a.detail_scroll, 0);
        assert!(screen(&mut a, width, 24).contains("Reference"));
        assert_eq!(a.current().expect("same record").id, id);
    }
}
#[test]
fn resizing_an_inspector_recalculates_wrapping_and_clamps_the_scroll() {
    let mut a = app(View::Loads);
    a.rows[0]
        .details
        .push(("Note".into(), "Wrapped shipment evidence ".repeat(40)));
    let id = a.current().expect("load").id.clone();
    a.key(key(KeyCode::Enter));
    screen(&mut a, 52, 16);
    a.key(key(KeyCode::End));
    let narrow_end = a.detail_scroll;
    let wide = screen(&mut a, 180, 50);
    assert!(a.detail_scroll < narrow_end);
    assert_eq!(a.detail_scroll, a.detail_max);
    assert!(
        wide.contains(&id),
        "a larger window still shows the final record ID"
    );
    screen(&mut a, 52, 16);
    a.key(key(KeyCode::End));
    assert!(screen(&mut a, 52, 16).contains(&id));
    a.key(key(KeyCode::Esc));
    assert!(!a.details);
}
#[test]
fn details_browse_only_adjacent_filtered_records_without_fetching_or_writing() {
    let mut a = app(View::Loads);
    search(&mut a, "Dallas");
    read_demo(&mut a);
    let first = a.current().expect("first").id.clone();
    let second = a.rows[a.visible[1]].id.clone();
    a.key(key(KeyCode::Enter));
    screen(&mut a, 77, 24);
    a.key(key(KeyCode::End));
    assert!(a.detail_scroll > 0);
    a.key(key(KeyCode::Left));
    assert!(a.detail_scroll > 0, "the first boundary keeps its scroll");
    a.key(key(KeyCode::Right));
    assert!(a.details);
    assert_eq!(a.current().expect("second").id, second);
    assert_eq!(a.detail_scroll, 0);
    assert!(screen(&mut a, 77, 24).contains("Details 2/"));
    a.key(key(KeyCode::Char('h')));
    assert_eq!(a.current().expect("first again").id, first);
    for _ in 0..100 {
        a.key(key(KeyCode::Char('l')));
    }
    assert_eq!(a.selected, a.visible.len() - 1);
    assert!(a.current().expect("last filtered load").cells[2].contains("Dallas"));
    assert!(!a.needs_fetch);
    assert!(a.write_ready.is_none());
    a.key(key(KeyCode::Char('a')));
    assert!(a.action_dialog.is_none());
    a.key(key(KeyCode::Esc));
    assert_eq!(
        a.selected,
        a.visible.len() - 1,
        "closing retains inspected selection"
    );
}
#[test]
fn help_and_connection_share_bounded_scrolling_and_visible_progress() {
    for panel in ['?', 'i'] {
        let mut a = app(View::Loads);
        a.recovery_error = Some("An owned storage diagnostic. ".repeat(100));
        a.key(key(KeyCode::Char(panel)));
        let top = screen(&mut a, 52, 16);
        assert!(top.contains("PgUp/Dn"));
        assert!(a.detail_max > 0);
        a.key(key(KeyCode::PageDown));
        assert!(a.detail_scroll > 0);
        a.key(key(KeyCode::End));
        let end = a.detail_scroll;
        let bottom = screen(&mut a, 52, 16);
        assert!(bottom.contains(if panel == '?' {
            "--no-color"
        } else {
            "Esc close"
        }));
        for _ in 0..300 {
            a.key(key(KeyCode::Down));
        }
        assert_eq!(a.detail_scroll, end);
        a.key(key(KeyCode::Char('g')));
        assert_eq!(a.detail_scroll, 0);
        a.key(key(KeyCode::Char('G')));
        assert_eq!(a.detail_scroll, end);
        a.key(key(KeyCode::Esc));
        assert!(!a.help && !a.connection);
    }
}
#[test]
fn search_edits_in_the_middle_and_moves_or_deletes_whole_unicode_graphemes() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('/')));
    a.paste("abe\u{301}👩\u{200d}🚀東z");
    a.key(key(KeyCode::Home));
    a.key(key(KeyCode::Right));
    a.key(key(KeyCode::Right));
    a.paste("XY");
    assert_eq!(a.draft, "abXYe\u{301}👩\u{200d}🚀東z");
    a.key(key(KeyCode::Left));
    a.key(key(KeyCode::Backspace));
    a.key(key(KeyCode::Delete));
    assert_eq!(a.draft, "abe\u{301}👩\u{200d}🚀東z");
    a.key(key(KeyCode::Right));
    assert_eq!(&a.draft[..a.draft_cursor], "abe\u{301}");
    a.key(key(KeyCode::Delete));
    assert_eq!(a.draft, "abe\u{301}東z");
    a.key(key(KeyCode::Backspace));
    assert_eq!(a.draft, "ab東z");
    a.key(key(KeyCode::End));
    a.key(key(KeyCode::Delete));
    assert_eq!(a.draft, "ab東z");
    assert!(!a.needs_fetch, "draft editing never reads or writes");
    assert!(a.write_ready.is_none());
}
#[test]
fn bounded_search_paste_keeps_complete_graphemes_and_normalizes_joined_boundaries() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('/')));
    a.paste(&"a".repeat(195));
    a.paste("👩\u{200d}🚀");
    assert_eq!(
        a.draft.len(),
        195,
        "oversized grapheme is not partially pasted"
    );
    a.key(key(KeyCode::Home));
    a.paste("東");
    assert_eq!(a.draft.len(), 198);
    assert_eq!(a.draft_cursor, 3);
    a.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert!(a.draft.is_empty());
    assert_eq!(a.draft_cursor, 0);
    a.paste("\u{301}z");
    a.key(key(KeyCode::Home));
    a.paste("e");
    assert_eq!(&a.draft[..a.draft_cursor], "e\u{301}");
    a.key(key(KeyCode::Backspace));
    assert_eq!(a.draft, "z");
}
#[test]
fn canceled_or_unchanged_search_preserves_the_current_page_and_record() {
    let mut a = app(View::Loads);
    search(&mut a, "FRT");
    read_demo(&mut a);
    a.key(key(KeyCode::Char('n')));
    read_demo(&mut a);
    a.selected = 4;
    let id = a.current().expect("load").id.clone();
    let generation = a.generation;
    let cursor = a.request().cursor;
    a.key(key(KeyCode::Char('/')));
    a.key(key(KeyCode::Enter));
    assert_eq!(a.generation, generation);
    assert_eq!(a.request().cursor, cursor);
    assert_eq!(a.current().expect("same load").id, id);
    a.key(key(KeyCode::Char('/')));
    a.key(key(KeyCode::Home));
    a.paste("canceled ");
    a.key(key(KeyCode::Esc));
    assert_eq!(a.query, "FRT");
    assert_eq!(a.current().expect("same load").id, id);
    assert!(!a.needs_fetch);
}
#[test]
fn typed_search_strips_the_same_terminal_and_direction_controls_as_paste() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('/')));
    let raw = "hello\u{202e}world\u{009b}\u{2066}";
    for c in raw.chars() {
        a.key(key(KeyCode::Char(c)));
    }
    assert_eq!(a.draft, crate::model::clean(raw));
    a.paste("\x1b\rq");
    assert!(a.draft.ends_with('q'));
    assert!(!a.quit);
    assert!(a.write_ready.is_none());
}
#[test]
fn long_search_keeps_the_caret_and_edit_context_visible_in_compact_windows() {
    for width in [52, 77, 120] {
        let mut a = app(View::Loads);
        a.key(key(KeyCode::Char('/')));
        a.paste(&format!(
            "START{}{}END",
            "東e\u{301}".repeat(10),
            "x".repeat(125)
        ));
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("terminal");
        for end in [true, false, true] {
            a.key(key(if end { KeyCode::End } else { KeyCode::Home }));
            terminal
                .draw(|f| ui::render(f, &mut a, &ui::Theme::new(false, false), true, "offline"))
                .expect("render");
            let position = terminal.get_cursor_position().expect("cursor");
            assert!(position.x < width, "cursor clipped at {width}");
            let line = terminal
                .backend()
                .buffer()
                .content
                .chunks(width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
                .find(|line| line.contains('▏'))
                .expect("visible caret");
            if end {
                assert!(line.contains("END"));
                assert!(line.contains('‹'));
                assert!(!line.contains("START"));
            } else {
                assert_eq!(position.x, 3);
                assert!(line.contains("START"));
                assert!(line.contains('›'));
            }
        }
    }
}
#[test]
fn clearing_paged_filters_returns_to_page_one_and_fences_old_replies() {
    let mut a = app(View::Loads);
    search(&mut a, "FRT");
    read_demo(&mut a);
    a.key(key(KeyCode::Char('n')));
    read_demo(&mut a);
    let generation = a.generation;
    let old_response = demo::snapshot(&a.request());
    a.key(key(KeyCode::Backspace));
    assert!(a.query.is_empty());
    assert_eq!(a.cursors, vec![None]);
    assert!(a.rows.is_empty());
    assert!(a.needs_fetch);
    a.finish(generation, Ok(old_response));
    assert!(a.rows.is_empty());
    read_demo(&mut a);
    assert_eq!(a.current().expect("first load").cells[0], "FRT-10421");
    a.key(key(KeyCode::Char('s')));
    a.key(key(KeyCode::Backspace));
    assert!(a.request().status.is_empty());
    assert_eq!(a.selected, 0);
    assert!(a.write_ready.is_none());
}
#[test]
fn clearing_local_filters_reuses_the_snapshot_and_keeps_other_workspace_context() {
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut a = app(view);
        let count = a.visible.len();
        let updated = a.updated;
        search(&mut a, "missing-result-canary");
        assert!(a.visible.is_empty());
        let empty = screen(&mut a, 52, 24);
        assert!(empty.contains("Backspace clears filters"));
        a.key(key(KeyCode::Backspace));
        assert_eq!(a.visible.len(), count);
        assert_eq!(a.updated, updated);
        assert!(!a.needs_fetch);
        search(&mut a, "retained-query");
        a.key(key(KeyCode::Char('1')));
        read_demo(&mut a);
        a.key(key(KeyCode::Backspace));
        assert!(!a.needs_fetch, "clearing no filters is a no-op");
        a.key(key(KeyCode::Char((b'1' + view.index() as u8) as char)));
        assert_eq!(a.query, "retained-query");
    }
}
#[test]
fn clearing_filters_keeps_an_uncertain_original_immutable_and_status_visible() {
    let mut a = app(View::Loads);
    a.pending = Some(pending(&a));
    a.write_uncertain = true;
    let original = a.pending.as_ref().expect("original").body.clone();
    search(&mut a, &"a".repeat(200));
    a.key(key(KeyCode::Char('s')));
    let text = screen(&mut a, 52, 24);
    assert!(
        text.contains("[planned]"),
        "status cannot hide behind a long query"
    );
    assert!(text
        .lines()
        .last()
        .expect("footer")
        .contains("c check original"));
    a.key(key(KeyCode::Backspace));
    assert_eq!(
        a.pending.as_ref().expect("original retained").body,
        original
    );
    assert!(a.write_uncertain);
    assert!(a.write_ready.is_none());
    assert!(a.query.is_empty());
    assert!(a.request().status.is_empty());
}
#[test]
fn opening_a_related_shipment_requires_a_fresh_exact_read_and_returns_to_the_issue() {
    let mut a = app(View::Operations);
    a.selected = 1;
    let issue = a.current().expect("issue").id.clone();
    let linked = a
        .current()
        .expect("issue")
        .related_load
        .clone()
        .expect("link");
    a.key(key(KeyCode::Enter));
    a.key(key(KeyCode::Char('o')));
    assert_eq!(a.view, View::Loads);
    assert!(!a.details);
    assert!(a.rows.is_empty());
    assert!(a.updated.is_none());
    assert_eq!(a.request().search, linked);
    assert!(a.request().status.is_empty());
    assert!(a.request().cursor.is_none());
    a.open_action();
    assert!(
        a.action_dialog.is_none(),
        "linking never skips the fresh read"
    );
    read_demo(&mut a);
    assert_eq!(a.rows.len(), 1);
    assert_eq!(a.current().expect("linked load").id, linked);
    let text = screen(&mut a, 52, 24);
    assert!(text.contains("Shipment from Operations"));
    assert!(text.lines().last().expect("footer").contains("b return"));
    a.key(key(KeyCode::Enter));
    a.key(key(KeyCode::Char('b')));
    assert_eq!(a.view, View::Operations);
    assert!(a.rows.is_empty(), "return also reads fresh data");
    read_demo(&mut a);
    assert_eq!(a.current().expect("same issue").id, issue);
    assert!(a.pending.is_none());
    assert!(a.write_ready.is_none());
}
#[test]
fn related_links_from_finance_and_outbox_never_treat_an_inbox_document_as_a_load() {
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut a = app(view);
        let expected = a
            .current()
            .expect("record")
            .related_load
            .clone()
            .expect("link");
        a.key(key(KeyCode::Char('o')));
        read_demo(&mut a);
        assert_eq!(a.current().expect("load").id, expected);
        a.key(key(KeyCode::Char('b')));
        assert_eq!(a.view, view);
    }
    let mut a = app(View::Integrations);
    a.selected = 2;
    assert!(a.current().expect("inbox").related_load.is_none());
    let generation = a.generation;
    a.key(key(KeyCode::Char('o')));
    assert_eq!(a.view, View::Integrations);
    assert_eq!(a.generation, generation);
    assert!(!a.needs_fetch);
}
#[test]
fn related_links_reject_malformed_raw_identities_instead_of_cleaning_them_into_links() {
    for id in [
        serde_json::json!(null),
        serde_json::json!(42),
        serde_json::json!(""),
        serde_json::json!("../load"),
        serde_json::json!("load\u{202e}id"),
        serde_json::json!("a".repeat(201)),
    ] {
        let data = serde_json::json!({"bills":[{"id":"invoice","load_id":id}]});
        assert!(crate::model::records(View::Billing, &data)[0]
            .related_load
            .is_none());
    }
}
#[test]
fn linked_reads_never_select_substring_normalized_or_duplicate_identity_matches() {
    let mut a = app(View::Operations);
    a.key(key(KeyCode::Char('o')));
    let id = a.focused_load.clone().expect("link");
    let mut response = demo::snapshot(&a.request());
    let exact = response.data["loads"][0].clone();
    for ids in [
        vec![format!("{id}-other")],
        vec![format!("{id}\u{202e}")],
        vec![id.clone(), id.clone()],
    ] {
        let rows = ids
            .into_iter()
            .map(|id| {
                let mut row = exact.clone();
                row["id"] = id.into();
                row
            })
            .collect::<Vec<_>>();
        response.data["loads"] = serde_json::json!(rows);
        a.needs_fetch = false;
        a.finish(
            a.generation,
            Ok(crate::client::Snapshot {
                identity: response.identity.clone(),
                data: response.data.clone(),
            }),
        );
        assert!(a.rows.is_empty());
        assert!(a.current().is_none());
        a.open_action();
        assert!(a.action_dialog.is_none());
    }
}
#[test]
fn related_lookup_can_page_explicitly_without_falling_back_to_another_load() {
    let mut a = app(View::Operations);
    a.key(key(KeyCode::Char('o')));
    let id = a.focused_load.clone().expect("link");
    let mut response = demo::snapshot(&a.request());
    response.data["loads"] = serde_json::json!([]);
    response.data["next_cursor"] = "owned-next-page".into();
    a.needs_fetch = false;
    a.finish(a.generation, Ok(response));
    assert!(a.current().is_none());
    assert!(
        !a.needs_fetch,
        "lookup never automatically scans more pages"
    );
    assert!(screen(&mut a, 52, 24).contains("n checks the next page"));
    a.key(key(KeyCode::Char('n')));
    assert_eq!(a.request().cursor.as_deref(), Some("owned-next-page"));
    assert_eq!(a.request().search, id);
    let mut response = demo::snapshot(&a.request());
    response.data["loads"] = demo::snapshot(&App::default().request()).data["loads"]
        .as_array()
        .expect("loads")
        .iter()
        .filter(|row| row["id"] == id)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    a.needs_fetch = false;
    a.finish(a.generation, Ok(response));
    assert_eq!(a.current().expect("exact load on next page").id, id);
}
#[test]
fn source_workspace_search_cursor_and_selection_survive_a_related_lookup() {
    let mut a = app(View::Operations);
    search(&mut a, "POD");
    read_demo(&mut a);
    let issue = a.current().expect("filtered issue").id.clone();
    // An opaque cursor is retained exactly; the fake response can still use its first page.
    a.cursors.push(Some("owned-operations-cursor".into()));
    a.key(key(KeyCode::Char('o')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('b')));
    assert_eq!(a.query, "POD");
    assert_eq!(
        a.request().cursor.as_deref(),
        Some("owned-operations-cursor")
    );
    read_demo(&mut a);
    assert_eq!(a.current().expect("same issue").id, issue);
    assert!(a.focused_load.is_none());
    assert!(a.related_from.is_none());
}
#[test]
fn editing_or_clearing_a_link_search_returns_to_normal_catalog_behavior() {
    for clear in [true, false] {
        let mut a = app(View::Operations);
        a.key(key(KeyCode::Char('o')));
        read_demo(&mut a);
        if clear {
            a.key(key(KeyCode::Backspace));
        } else {
            search(&mut a, "Dallas");
        }
        assert!(a.focused_load.is_none());
        assert!(a.related_from.is_none());
        read_demo(&mut a);
        assert!(a.rows.len() > 1);
        let generation = a.generation;
        a.key(key(KeyCode::Char('b')));
        assert_eq!(a.generation, generation);
        assert_eq!(a.view, View::Loads);
    }
}
#[test]
fn linked_navigation_preserves_uncertain_originals_and_clears_on_access_or_owner_change() {
    let mut a = app(View::Loads);
    a.pending = Some(pending(&a));
    a.write_uncertain = true;
    a.write_notice = Some("Original outcome is uncertain.".into());
    let original = a.pending.as_ref().expect("pending").body.clone();
    a.key(key(KeyCode::Char('2')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('o')));
    read_demo(&mut a);
    assert_eq!(a.pending.as_ref().expect("pending retained").body, original);
    assert!(a.write_uncertain);
    assert_eq!(
        a.write_notice.as_deref(),
        Some("Original outcome is uncertain.")
    );
    assert!(a.write_ready.is_none());
    a.key(key(KeyCode::Char('b')));
    read_demo(&mut a);
    a.key(key(KeyCode::Char('o')));
    let mut response = demo::snapshot(&a.request());
    response.identity["access"]["subject"] = "another-operator".into();
    a.finish(a.generation, Ok(response));
    assert!(a.focused_load.is_none());
    assert!(a.related_from.is_none());
    assert!(a.query.is_empty());
    let mut denied = app(View::Operations);
    denied.key(key(KeyCode::Char('o')));
    read_demo(&mut denied);
    assert!(denied.focused_load.is_some());
    denied.clear_access();
    assert!(denied.focused_load.is_none());
    denied.key(key(KeyCode::Char('2')));
    denied.key(key(KeyCode::Char('1')));
    assert!(denied.related_from.is_none());
    assert!(denied.query.is_empty());
}
#[test]
fn compact_rows_keep_routes_carriers_and_financial_amounts_in_the_right_pane() {
    for width in [52, 77, 80, 120, 180] {
        let mut a = app(View::Loads);
        let expected = a.rows[0].cells.clone();
        let text = screen(&mut a, width, 40);
        for value in [&expected[0], &expected[2], &expected[3]] {
            assert!(text.contains(value), "missing {value} at {width}");
        }
        for view in [View::Billing, View::Payables] {
            let mut a = app(view);
            let expected = a.rows[0].cells.clone();
            let text = screen(&mut a, width, 40);
            for value in [&expected[3], &expected[4]] {
                assert!(
                    text.contains(value),
                    "missing money {value} in {view:?} at {width}"
                );
            }
        }
    }
}
#[test]
fn compact_financial_values_preserve_both_integer_limits_and_unknown_amounts() {
    for width in [52, 77, 80] {
        let mut a = app(View::Billing);
        a.rows[0].cells[3] = crate::model::money(&serde_json::json!(i64::MIN));
        a.rows[0].cells[4] = crate::model::money(&serde_json::json!(i64::MAX));
        let text = screen(&mut a, width, 24);
        assert!(
            text.contains("-92233720368547758.08"),
            "debit clipped at {width}"
        );
        assert!(
            text.contains("92233720368547758.07"),
            "credit clipped at {width}"
        );
        a.rows[0].cells[4] = "—".into();
        assert!(screen(&mut a, width, 24).contains('—'));
    }
}
#[test]
fn stale_snapshot_is_prominent_and_blocks_new_actions_until_a_successful_read() {
    let mut a = app(View::Loads);
    a.updated = Some(Instant::now() - Duration::from_secs(61));
    let text = screen(&mut a, 77, 40);
    assert!(text.contains("STALE"));
    a.open_action();
    assert!(a.action_dialog.is_none());
    assert!(a
        .write_notice
        .as_deref()
        .expect("notice")
        .contains("refresh"));
    a.apply(demo::snapshot(&a.request()));
    assert!(!a.stale());
    a.open_action();
    assert!(a.action_dialog.is_some());
}
#[test]
fn scheduled_reads_preserve_page_filters_and_selection_without_queuing_commands() {
    let mut a = app(View::Loads);
    a.key(key(KeyCode::Char('n')));
    a.needs_fetch = false;
    a.apply(demo::snapshot(&a.request()));
    a.selected = 3;
    a.refresh_interval = Some(Duration::from_secs(30));
    let cursor = a.request().cursor;
    let generation = a.generation;
    a.tick(Instant::now() + Duration::from_secs(29));
    assert!(!a.needs_fetch);
    a.tick(Instant::now() + Duration::from_secs(31));
    assert!(a.needs_fetch);
    assert_eq!(a.generation, generation + 1);
    assert_eq!(a.selected, 3);
    assert_eq!(a.request().cursor, cursor);
    assert!(a.write_ready.is_none());
    assert!(a.pending.is_none());
    a.tick(Instant::now() + Duration::from_secs(100));
    assert_eq!(a.generation, generation + 1);
}
#[test]
fn automatic_reads_pause_for_editing_inspection_errors_and_every_pending_save() {
    for state in 0..10 {
        let mut a = app(View::Loads);
        a.refresh_interval = Some(Duration::from_secs(5));
        match state {
            0 => a.searching = true,
            1 => a.details = true,
            2 => a.help = true,
            3 => a.connection = true,
            4 => a.error = Some("Offline".into()),
            5 => a.denied = true,
            6 => a.write_busy = true,
            7 => {
                a.pending = Some(pending(&a));
                a.write_uncertain = true;
            }
            8 => {
                a.open_action();
            }
            _ => {
                a.write_ready = Some(WriteJob::Submit {
                    pending: pending(&a),
                    checking: false,
                })
            }
        }
        let generation = a.generation;
        a.tick(Instant::now() + Duration::from_secs(100));
        assert!(!a.needs_fetch, "state {state}");
        assert_eq!(a.generation, generation, "state {state}");
    }
    let mut a = app(View::Loads);
    let original = pending(&a);
    a.pending = Some(original.clone());
    a.write_uncertain = true;
    a.refresh_interval = Some(Duration::from_secs(5));
    a.updated = Some(Instant::now() - Duration::from_secs(100));
    a.tick(Instant::now());
    assert_eq!(a.pending.as_ref().expect("retained").body, original.body);
    assert!(a.write_ready.is_none());
    // Explicit recovery still works when the read snapshot ages; it uses the frozen original.
    a.key(key(KeyCode::Char('c')));
    assert!(a.write_ready.is_some());
}
#[test]
fn read_error_pauses_automatic_retry_and_manual_recovery_restarts_scheduling() {
    let mut a = app(View::Loads);
    a.refresh_interval = Some(Duration::from_secs(5));
    a.finish(
        a.generation,
        Err(crate::client::Failure {
            message: "Offline".into(),
            denied: false,
        }),
    );
    a.tick(Instant::now() + Duration::from_secs(100));
    assert!(!a.needs_fetch);
    a.key(key(KeyCode::Char('r')));
    assert!(a.needs_fetch);
    a.needs_fetch = false;
    a.finish(a.generation, Ok(demo::snapshot(&a.request())));
    a.tick(Instant::now() + Duration::from_secs(6));
    assert!(a.needs_fetch);
}
#[test]
fn connection_panel_shows_complete_recovery_errors_and_help_can_scroll() {
    let mut a = app(View::Loads);
    a.recovery_error = Some("Storage is locked; close the competing terminal and refresh".into());
    a.key(key(KeyCode::Char('i')));
    assert!(a.connection);
    let text = screen(&mut a, 77, 40);
    assert!(text.contains("Storage is locked"));
    assert!(text.contains("competing terminal"));
    a.key(key(KeyCode::Down));
    assert_eq!(
        a.detail_scroll, 0,
        "fitting content cannot scroll into blank space"
    );
    a.key(key(KeyCode::Esc));
    assert!(!a.connection);
    a.key(key(KeyCode::Char('?')));
    assert_eq!(a.detail_scroll, 0);
    screen(&mut a, 77, 24);
    a.key(key(KeyCode::Down));
    assert_eq!(a.detail_scroll, 1);
    a.key(key(KeyCode::Char('a')));
    assert!(a.action_dialog.is_none());
}
#[test]
fn refresh_cli_is_bounded_and_setup_is_explicit_for_live_sessions() {
    for value in ["-1", "1", "4", "3601", "invalid"] {
        assert!(Config::parse(["--demo", "--refresh", value].map(String::from)).is_err());
    }
    let c = Config::parse(["--demo", "--refresh", "5", "--stale-after", "10"].map(String::from))
        .expect("config")
        .expect("not help");
    assert_eq!(c.refresh_interval, Some(Duration::from_secs(5)));
    assert_eq!(c.stale_after, Duration::from_secs(10));
    for flag in ["--demo", "--snapshot"] {
        assert!(Config::parse(["--connect", flag].map(String::from)).is_err());
    }
    assert!(config().needs_connection());
}
fn wizard_screen(w: &mut Wizard, width: u16, height: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    t.draw(|f| w.render(f, &ui::Theme::new(false, false)))
        .expect("render");
    t.backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect()
}
#[test]
fn wizard_masks_secrets_and_requires_an_explicit_fully_visible_destination_review() {
    let mut c = config();
    let mut w = Wizard::new(&c, false);
    w.key(key(KeyCode::Enter));
    w.paste("PRIVATE-STAFF-TOKEN-CANARY");
    w.key(key(KeyCode::Enter));
    w.paste(&"a1".repeat(32));
    w.key(key(KeyCode::Enter));
    assert_eq!(w.step, 3);
    let text = wizard_screen(&mut w, 77, 40);
    assert!(!text.contains("PRIVATE-STAFF-TOKEN-CANARY"));
    assert!(!text.contains(&"a1".repeat(32)));
    assert!(text.contains("127.0.0.1:8090"));
    w.paste("y");
    assert!(!w.key(KeyEvent::new_with_kind(
        KeyCode::Char('y'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat
    )));
    wizard_screen(&mut w, 40, 10);
    assert!(!w.key(key(KeyCode::Char('y'))));
    wizard_screen(&mut w, 77, 40);
    assert!(w.key(key(KeyCode::Char('y'))));
    w.apply(&mut c).expect("apply");
    let value = c
        .headers
        .get(reqwest::header::AUTHORIZATION)
        .expect("credential");
    assert!(value.is_sensitive());
    assert_eq!(value, "Bearer PRIVATE-STAFF-TOKEN-CANARY");
    assert_eq!(c.recovery_key.as_deref(), Some("a1".repeat(32).as_str()));
    assert!(!c.needs_connection());
}
#[test]
fn wizard_cannot_forward_an_existing_credential_to_a_changed_api_origin() {
    let mut c = config();
    c.headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_static("Bearer owned-test"),
    );
    let mut w = Wizard::new(&c, false);
    w.fields[0] = "https://different.example/".into();
    w.key(key(KeyCode::Enter));
    w.key(key(KeyCode::Enter));
    assert_eq!(w.step, 1);
    assert!(w.error.is_some());
    w.paste("explicit-new-credential");
    w.key(key(KeyCode::Enter));
    w.key(key(KeyCode::Enter));
    wizard_screen(&mut w, 77, 40);
    assert!(w.key(key(KeyCode::Char('y'))));
    w.apply(&mut c).expect("apply");
    assert_eq!(c.origin.as_str(), "https://different.example/");
    assert_eq!(
        c.headers[reqwest::header::AUTHORIZATION],
        "Bearer explicit-new-credential"
    );
}
#[test]
fn wizard_validates_hidden_fields_without_echoing_invalid_values_or_interpreting_paste() {
    let mut w = Wizard::new(&config(), false);
    w.key(key(KeyCode::Enter));
    w.paste("q");
    assert!(!w.cancelled);
    w.paste("\u{1b}PRIVATE-BAD-TOKEN");
    assert!(!w.error.as_deref().expect("error").contains("PRIVATE"));
    w.key(key(KeyCode::Enter));
    w.paste("NOT-A-VALID-RECOVERY-KEY");
    w.key(key(KeyCode::Enter));
    assert_eq!(w.step, 2);
    let text = wizard_screen(&mut w, 77, 40);
    assert!(!text.contains("NOT-A-VALID-RECOVERY-KEY"));
    w.key(key(KeyCode::Esc));
    assert!(w.cancelled);
}
