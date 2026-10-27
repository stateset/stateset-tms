use crate::{
    actions::{Kind, Owner, Pending},
    client::{Failure, Snapshot},
    demo,
    model::{App, View},
    palette::Command,
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::{json, Value};
fn key(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
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
fn select(app: &mut App, index: usize, offset: usize) {
    app.selected = index;
    *app.table.offset_mut() = offset;
}
fn two_pages() -> App {
    let mut app = loaded();
    select(&mut app, 49, 40);
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    select(&mut app, 3, 2);
    app
}
fn apply_data(app: &mut App, data: Value) {
    let identity = demo::snapshot(&app.request()).identity;
    app.needs_fetch = false;
    app.finish(app.generation, Ok(Snapshot { identity, data }));
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
fn revisiting_each_page_reads_fresh_data_and_restores_its_row_and_viewport() {
    let mut app = two_pages();
    let generation = app.generation;
    key(&mut app, KeyCode::Char('p'));
    assert!(
        app.rows.is_empty() && app.visible.is_empty() && app.updated.is_none() && app.needs_fetch
    );
    assert_eq!(app.selection_anchor.as_deref(), Some("load-demo-0049"));
    assert_eq!(app.table.offset(), 40);
    read(&mut app);
    assert_eq!(app.current().expect("page one").id, "load-demo-0049");
    assert_eq!(app.selected, 49);
    assert_eq!(app.table.offset(), 40);
    key(&mut app, KeyCode::Char('n'));
    assert!(app.rows.is_empty() && app.needs_fetch);
    read(&mut app);
    assert_eq!(app.current().expect("page two").id, "load-demo-0053");
    assert_eq!(app.selected, 3);
    assert_eq!(app.table.offset(), 2);
    assert_eq!(app.generation, generation + 2);
}
#[test]
fn reordered_or_removed_records_restore_identity_or_fall_back_to_first_result() {
    let mut app = two_pages();
    key(&mut app, KeyCode::Char('p'));
    let mut response = demo::snapshot(&app.request()).data;
    response["loads"].as_array_mut().expect("loads").reverse();
    apply_data(&mut app, response);
    assert_eq!(app.current().expect("same identity").id, "load-demo-0049");
    assert_eq!(app.selected, 0);
    key(&mut app, KeyCode::Char('n'));
    let mut response = demo::snapshot(&app.request()).data;
    response["loads"]
        .as_array_mut()
        .expect("loads")
        .retain(|r| r["id"] != "load-demo-0053");
    apply_data(&mut app, response);
    assert_eq!(app.selected, 0);
    assert_eq!(app.current().expect("fallback").id, "load-demo-0050");
}
#[test]
fn a_changed_next_cursor_invalidates_forward_positions_instead_of_resurrecting_old_rows() {
    let mut app = two_pages();
    key(&mut app, KeyCode::Char('p'));
    let mut response = demo::snapshot(&app.request()).data;
    response["next_cursor"] = json!("NEW-OPAQUE-BRANCH");
    apply_data(&mut app, response);
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.request().cursor.as_deref(), Some("NEW-OPAQUE-BRANCH"));
    assert_eq!(app.selected, 0);
    assert!(app.selection_anchor.is_none());
    let response = demo::snapshot(
        &App {
            cursors: vec![None, Some("50".into())],
            ..App::default()
        }
        .request(),
    )
    .data;
    apply_data(&mut app, response);
    assert_eq!(app.current().expect("new branch").id, "load-demo-0050");
    assert!(app.write_ready.is_none());
}
#[test]
fn first_page_restart_and_changed_search_clear_the_entire_page_position_history() {
    for search in [false, true] {
        let mut app = two_pages();
        if search {
            key(&mut app, KeyCode::Char('/'));
            app.paste("FRT");
            key(&mut app, KeyCode::Enter);
        } else {
            key(&mut app, KeyCode::Char('r'));
        }
        read(&mut app);
        assert_eq!(app.selected, 0);
        key(&mut app, KeyCode::Char('n'));
        read(&mut app);
        assert_eq!(app.selected, 0);
        assert_eq!(app.current().expect("new context").id, "load-demo-0050");
    }
}
#[test]
fn workspaces_and_current_page_retry_preserve_the_other_pages_position() {
    let mut app = two_pages();
    key(&mut app, KeyCode::Char('3'));
    read(&mut app);
    key(&mut app, KeyCode::Char('1'));
    read(&mut app);
    assert_eq!(
        app.current().expect("returned page two").id,
        "load-demo-0053"
    );
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    read(&mut app);
    key(&mut app, KeyCode::Char('p'));
    read(&mut app);
    assert_eq!(
        app.current().expect("retained page one").id,
        "load-demo-0049"
    );
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    assert_eq!(
        app.current().expect("retained page two").id,
        "load-demo-0053"
    );
}
#[test]
fn failed_page_returns_keep_their_anchor_and_can_be_retried_without_cached_records() {
    let mut app = two_pages();
    key(&mut app, KeyCode::Char('p'));
    app.needs_fetch = false;
    app.finish(
        app.generation,
        Err(Failure {
            message: "Owned unavailable diagnostic".into(),
            denied: false,
        }),
    );
    assert!(app.rows.is_empty());
    assert_eq!(app.selection_anchor.as_deref(), Some("load-demo-0049"));
    assert!(!app.needs_fetch);
    app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    read(&mut app);
    assert_eq!(
        app.current().expect("restored after retry").id,
        "load-demo-0049"
    );
}
#[test]
fn denial_and_changed_owners_clear_positions_for_every_visited_page() {
    for changed in ["denied", "tenant", "subject"] {
        let mut app = two_pages();
        let mut identity = demo::snapshot(&app.request()).identity;
        if changed == "denied" {
            app.clear_access();
        } else {
            if changed == "tenant" {
                identity["tenant_id"] = json!("other");
            } else {
                identity["access"]["subject"] = json!("other");
            }
            let response = Snapshot {
                identity: identity.clone(),
                data: demo::snapshot(&app.request()).data,
            };
            app.finish(app.generation, Ok(response));
        }
        for code in [None, Some('n'), Some('p')] {
            if let Some(code) = code {
                key(&mut app, KeyCode::Char(code));
            }
            app.needs_fetch = false;
            app.finish(
                app.generation,
                Ok(Snapshot {
                    identity: identity.clone(),
                    data: demo::snapshot(&app.request()).data,
                }),
            );
            assert_eq!(app.selected, 0);
        }
    }
}
#[test]
fn palette_page_choices_follow_live_availability_and_start_a_fresh_read() {
    let mut app = loaded();
    select(&mut app, 49, 40);
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste("next page");
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].command, Command::NextPage);
    assert!(choices[0].enabled);
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_none() && app.needs_fetch && app.rows.is_empty());
    read(&mut app);
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste("previous page");
    assert_eq!(app.palette_choices()[0].command, Command::PreviousPage);
    app.loading = Some(app.generation);
    key(&mut app, KeyCode::Enter);
    assert!(app.palette.is_some());
    app.loading = None;
    key(&mut app, KeyCode::Enter);
    read(&mut app);
    assert_eq!(app.current().expect("retained row").id, "load-demo-0049");
    key(&mut app, KeyCode::Char('3'));
    read(&mut app);
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste("next page");
    assert!(app.palette_choices().is_empty());
}
#[test]
fn restored_positions_render_selected_record_after_a_window_resize_in_all_palettes() {
    for width in [52, 77, 120] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = two_pages();
            key(&mut app, KeyCode::Char('p'));
            read(&mut app);
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("FRT-10470"));
            assert!(text.contains("50/50"));
            key(&mut app, KeyCode::Char('n'));
            read(&mut app);
            let text = screen(&mut app, width, no_color, light);
            assert!(text.contains("FRT-10474"));
            assert!(text.contains("4/15"));
        }
    }
}
#[test]
fn page_changes_preserve_the_complete_uncertain_original_and_stop_decision() {
    let mut app = loaded();
    let target = app.current().expect("load").target.clone().expect("target");
    app.pending = Some(
        Pending::new(
            target,
            Owner::from_identity(&app.identity).expect("owner"),
            Kind::Location,
            "Owned location",
        )
        .expect("pending"),
    );
    app.stopping_note = Some("Retained stop note".into());
    app.write_uncertain = true;
    app.write_notice = Some("Original outcome uncertain".into());
    let original = serde_json::to_value(app.pending.as_ref().expect("pending")).expect("original");
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    select(&mut app, 3, 2);
    key(&mut app, KeyCode::Char('p'));
    read(&mut app);
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("retained"),
        original
    );
    assert_eq!(app.stopping_note.as_deref(), Some("Retained stop note"));
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original outcome uncertain")
    );
    assert!(app.write_uncertain && app.write_ready.is_none());
}

#[test]
fn operations_page_positions_survive_a_linked_shipment_lookup_and_return() {
    fn page(start: usize, next: Option<&str>) -> Value {
        json!({"issues":(start..start+2).map(|i|json!({"id":format!("work-{i}"),"load_reference":format!("FRT-{}",10421+i),"owner":"demo:terminal","source_active":true,"deferred":false,"evidence_valid":true,"signal":{"load_id":format!("load-demo-{i:04}"),"priority":1,"title":"Work item","summary":"Owned work page","next_action":"Inspect source","deadline":null}})).collect::<Vec<_>>(),"next_cursor":next})
    }
    let mut app = App {
        view: View::Operations,
        operations_scope: crate::operations::Scope::Mine,
        query: "Work".into(),
        ..App::default()
    };
    apply_data(&mut app, page(0, Some("OWNED-WORK-PAGE-2")));
    select(&mut app, 1, 0);
    key(&mut app, KeyCode::Char('n'));
    apply_data(&mut app, page(50, None));
    select(&mut app, 1, 0);
    key(&mut app, KeyCode::Char('o'));
    read(&mut app);
    assert_eq!(
        app.current().expect("exact related shipment").id,
        "load-demo-0051"
    );
    key(&mut app, KeyCode::Char('b'));
    assert_eq!(app.query, "Work");
    assert_eq!(app.operations_scope, crate::operations::Scope::Mine);
    assert_eq!(app.request().cursor.as_deref(), Some("OWNED-WORK-PAGE-2"));
    assert!(app.rows.is_empty() && app.needs_fetch);
    apply_data(&mut app, page(50, None));
    assert_eq!(app.current().expect("source row").id, "work-51");
    key(&mut app, KeyCode::Char('p'));
    apply_data(&mut app, page(0, Some("OWNED-WORK-PAGE-2")));
    assert_eq!(app.current().expect("first work page").id, "work-1");
}
#[test]
fn an_obsolete_page_reply_cannot_replace_a_new_destination_or_its_saved_anchor() {
    let mut app = loaded();
    select(&mut app, 49, 40);
    key(&mut app, KeyCode::Char('n'));
    let generation = app.generation;
    let response = demo::snapshot(&app.request());
    key(&mut app, KeyCode::Char('p'));
    app.finish(generation, Ok(response));
    assert!(app.rows.is_empty());
    assert_eq!(app.selection_anchor.as_deref(), Some("load-demo-0049"));
    assert!(app.request().cursor.is_none());
    read(&mut app);
    assert_eq!(
        app.current().expect("fresh destination").id,
        "load-demo-0049"
    );
}
