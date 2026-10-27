use crate::{
    actions::{Kind, Owner, Pending},
    client::Snapshot,
    demo,
    model::{App, View},
    palette::Command,
    parties::{LoadParties, Party},
    ui,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;

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
fn choose(app: &mut App, query: &str, expected: Command) {
    app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    app.paste(query);
    let choices = app.palette_choices();
    assert_eq!(choices.len(), 1, "ambiguous {query}");
    assert_eq!(choices[0].command, expected);
    assert!(choices[0].enabled);
    key(app, KeyCode::Enter);
    assert!(app.palette.is_none());
}
#[test]
fn exact_parties_preserve_unicode_spaces_and_url_punctuation_without_normalizing() {
    let customer = "  Réseau & Sons/West?x=1#2+%  ";
    let carrier = "🚛-carrier";
    let parties = LoadParties::from_load(&json!({"customer_id":customer,"carrier_id":carrier}));
    assert_eq!(parties.get(Party::Customer), Some(customer));
    assert_eq!(parties.get(Party::Carrier), Some(carrier));
    let mut app = loaded();
    app.load_parties = parties;
    app.query = "Chicago".into();
    app.status_index = 1;
    app.cursors.push(Some("opaque+cursor&other=1".into()));
    let url = crate::client::endpoint(
        &"http://localhost:8090".parse().expect("owned fixture"),
        &app.request(),
    );
    let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(pairs["customer_id"], customer);
    assert_eq!(pairs["carrier_id"], carrier);
    assert_eq!(pairs["q"], "Chicago");
    assert_eq!(pairs["status"], crate::model::STATUSES[1]);
    assert_eq!(pairs["cursor"], "opaque+cursor&other=1");
    assert_eq!(pairs.len(), 6);
    assert!(url.fragment().is_none());
}
#[test]
fn missing_malformed_and_display_changed_parties_never_become_filter_identities() {
    for value in [
        json!(null),
        json!(1),
        json!(false),
        json!([]),
        json!(""),
        json!("  "),
        json!("cust\nmer"),
        json!("carrier\u{1b}[2J"),
        json!("a\u{202e}b"),
        json!("a".repeat(201)),
        json!("é".repeat(101)),
    ] {
        let mut app = loaded();
        app.rows[0].load_parties =
            LoadParties::from_load(&json!({"customer_id":value,"carrier_id":value}));
        for party in Party::ALL {
            assert!(!app.can_filter_selected_party(party));
            let generation = app.generation;
            app.filter_selected_party(party);
            assert!(!app.load_parties.any());
            assert_eq!(app.generation, generation);
        }
    }
    let boundary = "é".repeat(100);
    assert_eq!(
        LoadParties::from_load(&json!({"customer_id":boundary})).get(Party::Customer),
        Some(boundary.as_str())
    );
}
#[test]
fn selected_customer_filters_the_entire_catalog_and_combines_with_carrier_search_and_status() {
    let mut app = loaded();
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    app.query = "Los Angeles".into();
    app.status_index = 5; // in_transit, matching some synthetic even-numbered loads
    let generation = app.generation;
    choose(
        &mut app,
        "selected customer",
        Command::SelectedParty(Party::Customer),
    );
    assert_eq!(app.generation, generation + 1);
    assert!(app.rows.is_empty() && app.needs_fetch && app.updated.is_none());
    assert!(app.request().cursor.is_none() && app.next_cursor.is_none());
    assert_eq!(app.page_positions.len(), 1);
    assert_eq!(app.query, "Los Angeles");
    assert_eq!(app.status_index, 5);
    read(&mut app);
    assert!(!app.rows.is_empty());
    assert_eq!(app.current().expect("owned fixture").id, "load-demo-0000");
    assert!(app.rows.iter().all(|r| r.load_parties.get(Party::Customer)
        == Some("Acme Distribution (sample)")
        && r.status == "in_transit"));
    choose(
        &mut app,
        "selected carrier",
        Command::SelectedParty(Party::Carrier),
    );
    read(&mut app);
    assert!(app.load_parties.get(Party::Customer).is_some());
    assert_eq!(
        app.load_parties.get(Party::Carrier),
        Some("Pioneer Transport (sample)")
    );
    let generation = app.generation;
    choose(
        &mut app,
        "selected customer",
        Command::SelectedParty(Party::Customer),
    );
    assert_eq!(app.generation, generation); // already active: keep the current read/selection
    choose(
        &mut app,
        "clear customer filter",
        Command::ClearParty(Party::Customer),
    );
    assert!(app.load_parties.get(Party::Customer).is_none());
    assert!(app.load_parties.get(Party::Carrier).is_some());
    assert_eq!(app.query, "Los Angeles");
    assert_eq!(app.status_index, 5);
    read(&mut app);
    choose(
        &mut app,
        "clear carrier filter",
        Command::ClearParty(Party::Carrier),
    );
    assert!(!app.load_parties.any());
}
#[test]
fn customer_scope_is_global_in_demo_and_clear_is_available_after_empty_results() {
    let mut app = loaded();
    app.filter_selected_party(Party::Customer);
    read(&mut app);
    assert_eq!(app.rows.len(), 33); // includes eight matches beyond the old first page
    assert_eq!(app.rows.last().expect("owned fixture").id, "load-demo-0064");
    app.query = "no-matches".into();
    read(&mut app);
    assert!(app.rows.is_empty() && app.has_filters());
    choose(
        &mut app,
        "clear customer filter",
        Command::ClearParty(Party::Customer),
    );
    assert!(!app.load_parties.any());
    assert_eq!(app.query, "no-matches");
    read(&mut app);
    key(&mut app, KeyCode::Backspace);
    read(&mut app);
    assert!(!app.has_filters());
    assert_eq!(app.rows.len(), 50);
}
#[test]
fn party_filters_survive_workspace_navigation_and_current_page_reads_but_never_leak_to_other_endpoints(
) {
    let mut app = loaded();
    app.filter_selected_party(Party::Carrier);
    read(&mut app);
    key(&mut app, KeyCode::Char('n'));
    read(&mut app);
    app.selected = 3;
    for view in [
        View::Operations,
        View::Billing,
        View::Payables,
        View::Integrations,
    ] {
        key(
            &mut app,
            KeyCode::Char(char::from(b'1' + view.index() as u8)),
        );
        read(&mut app);
        assert!(!app.load_parties.any());
        // Even an accidentally populated request cannot forward party fields elsewhere.
        let mut request = app.request();
        request.load_parties =
            LoadParties::from_load(&json!({"customer_id":"c", "carrier_id":"k"}));
        let url = crate::client::endpoint(
            &"http://localhost:8090".parse().expect("owned fixture"),
            &request,
        );
        assert!(!url
            .query_pairs()
            .any(|(k, _)| k == "customer_id" || k == "carrier_id"));
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(!app.palette_choices().iter().any(|c| matches!(
            c.command,
            Command::SelectedParty(_) | Command::ClearParty(_)
        )));
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('1'));
        read(&mut app);
        assert_eq!(app.request().cursor.as_deref(), Some("50"));
        assert_eq!(app.current().expect("owned fixture").id, "load-demo-0053");
        assert!(app.load_parties.get(Party::Carrier).is_some());
        app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        read(&mut app);
        assert_eq!(app.current().expect("owned fixture").id, "load-demo-0053");
    }
}
#[test]
fn a_linked_lookup_clears_old_party_restrictions_before_finding_the_exact_load() {
    let mut app = loaded();
    app.filter_selected_party(Party::Customer);
    read(&mut app);
    key(&mut app, KeyCode::Char('2'));
    read(&mut app);
    app.selected = 1; // different customer's load
    key(&mut app, KeyCode::Char('o'));
    assert!(!app.load_parties.any());
    assert_eq!(app.query, "load-demo-0001");
    read(&mut app);
    assert_eq!(app.current().expect("owned fixture").id, "load-demo-0001");
    key(&mut app, KeyCode::Char('b'));
    read(&mut app);
    assert_eq!(app.current().expect("owned fixture").id, "issue-demo-1");
}
#[test]
fn owner_change_and_access_denial_erase_active_and_remembered_party_scopes() {
    for denied in [false, true] {
        let mut app = loaded();
        app.filter_selected_party(Party::Customer);
        read(&mut app);
        key(&mut app, KeyCode::Char('3'));
        read(&mut app);
        if denied {
            app.clear_access();
        } else {
            let mut response = demo::snapshot(&app.request());
            response.identity["access"]["subject"] = json!("new-operator");
            app.finish(app.generation, Ok(response));
        }
        assert!(!app.load_parties.any());
        key(&mut app, KeyCode::Char('1'));
        assert!(!app.load_parties.any());
    }
}
#[test]
fn fresh_availability_is_checked_after_a_palette_read_and_during_loading() {
    let mut app = loaded();
    for loading in [true, false] {
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        app.paste("selected carrier");
        if loading {
            app.loading = Some(app.generation);
        } else {
            app.rows[0].load_parties = LoadParties::default();
        }
        let generation = app.generation;
        key(&mut app, KeyCode::Enter);
        assert!(app.palette.is_some());
        assert!(!app.load_parties.any());
        assert_eq!(app.generation, generation);
        key(&mut app, KeyCode::Esc);
        app.loading = None;
    }
    app.clear_access();
    assert!(!app.can_filter_selected_party(Party::Customer));
    app.filter_selected_party(Party::Customer);
    assert!(!app.load_parties.any());
}
#[test]
fn obsolete_unfiltered_responses_cannot_replace_a_new_party_context() {
    let mut app = loaded();
    let old_generation = app.generation;
    let old_response = demo::snapshot(&app.request());
    app.filter_selected_party(Party::Customer);
    app.finish(old_generation, Ok(old_response));
    assert!(app.rows.is_empty() && app.needs_fetch);
    read(&mut app);
    assert_eq!(app.rows.len(), 33);
}
#[test]
fn customer_and_carrier_reads_do_not_change_a_retained_original_or_stop_decision() {
    let mut app = loaded();
    app.pending = Some(
        Pending::new(
            app.current()
                .expect("owned fixture")
                .target
                .clone()
                .expect("owned fixture"),
            Owner::from_identity(&app.identity).expect("owned fixture"),
            Kind::Location,
            "Owned location",
        )
        .expect("owned fixture"),
    );
    app.stopping_note = Some("Retained stop decision".into());
    app.write_uncertain = true;
    app.write_notice = Some("Original outcome uncertain".into());
    let original = serde_json::to_value(&app.pending).expect("owned fixture");
    for party in Party::ALL {
        choose(
            &mut app,
            &format!("selected {}", party.title()),
            Command::SelectedParty(party),
        );
        read(&mut app);
    }
    key(&mut app, KeyCode::Backspace);
    read(&mut app);
    assert_eq!(
        serde_json::to_value(&app.pending).expect("owned fixture"),
        original
    );
    assert_eq!(app.stopping_note.as_deref(), Some("Retained stop decision"));
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original outcome uncertain")
    );
    assert!(app.write_uncertain && app.write_ready.is_none());
}
#[test]
fn compact_headers_and_connection_show_active_scopes_in_every_palette() {
    for width in [52, 77, 120] {
        for (no_color, light) in [(false, false), (false, true), (true, false)] {
            let mut app = loaded();
            app.filter_selected_party(Party::Customer);
            read(&mut app);
            app.filter_selected_party(Party::Carrier);
            read(&mut app);
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).expect("owned fixture");
            let screen = |terminal: &mut Terminal<TestBackend>, app: &mut App| {
                terminal
                    .draw(|f| ui::render(f, app, &ui::Theme::new(no_color, light), true, "offline"))
                    .expect("owned fixture");
                terminal
                    .backend()
                    .buffer()
                    .content
                    .chunks(width as usize)
                    .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let text = screen(&mut terminal, &mut app);
            assert!(text.contains("[customer + carrier]"));
            assert!(text.contains("33 shown"));
            key(&mut app, KeyCode::Char('i'));
            let text = screen(&mut terminal, &mut app);
            assert!(text.contains("Customer filter: Acme Distribution (sample)"));
            assert!(text.contains("Carrier filter: Pioneer Transport (sample)"));
        }
    }
}
#[test]
fn raw_party_identity_is_taken_from_load_fields_and_never_from_cleaned_details() {
    let mut app = loaded();
    let mut response = demo::snapshot(&app.request());
    response.data["loads"][0]["customer_id"] = json!("unsafe\u{202e}customer");
    response.data["loads"][0]["carrier_id"] = json!("exact-carrier");
    app.finish(
        app.generation,
        Ok(Snapshot {
            identity: response.identity,
            data: response.data,
        }),
    );
    assert!(app
        .current()
        .expect("owned fixture")
        .details
        .iter()
        .any(|(k, v)| k == "Customer" && v == "unsafecustomer"));
    assert!(!app.can_filter_selected_party(Party::Customer));
    app.filter_selected_party(Party::Carrier);
    assert_eq!(app.load_parties.get(Party::Carrier), Some("exact-carrier"));
}
