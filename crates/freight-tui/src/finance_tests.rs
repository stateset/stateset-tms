use crate::{
    actions::{Kind, Owner, Pending},
    client::Snapshot,
    demo,
    model::{App, BalanceFilter, View},
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
    let cases = [
        ("current", json!(1), json!(0)),
        ("late", json!(i64::MAX), json!(3)),
        ("credit-min", json!(i64::MIN), json!(90)),
        ("credit-one", json!(-1), json!(90)),
        ("zero", json!(0), json!(90)),
        ("missing", Value::Null, json!(90)),
        ("string", json!("12"), json!(90)),
        ("fraction", json!(1.5), json!(90)),
        ("overflow", json!(u64::MAX), json!(90)),
        ("no-age", json!(42), Value::Null),
        ("bad-age", json!(43), json!("9")),
        ("negative-age", json!(44), json!(-1)),
    ];
    let bills = cases.iter().map(|(id, balance, days)| json!({
        "id":id,"number":format!("INV-{id}"),"load_id":"load-demo-0003",
        "status":"void","customer_id":"Owned customer","due_date":"1999-01-01",
        "snapshot":{"amount_cents":100},"totals":{"balance_cents":balance,"overdue_days":days}
    })).collect::<Vec<_>>();
    let settlements = cases
        .iter()
        .map(|(id, balance, _)| {
            json!({
                "id":id,"load_id":"load-demo-0003","carrier_id":"Owned carrier",
                "status":"prepared","total_amount_cents":100,"balance_cents":balance
            })
        })
        .collect::<Vec<_>>();
    json!({"bills":bills,"settlements":settlements,"finance":{
        "receivable_cents":10000,"overdue_cents":5000,"prepared_payable_cents":3000
    }})
}
fn read(app: &mut App, data: Value) {
    let identity = demo::snapshot(&app.request()).identity;
    app.needs_fetch = false;
    app.finish(app.generation, Ok(Snapshot { identity, data }));
}
fn app(view: View) -> App {
    let mut app = App {
        view,
        needs_fetch: false,
        ..App::default()
    };
    read(&mut app, data());
    app
}
fn ids(app: &App) -> Vec<&str> {
    app.visible
        .iter()
        .map(|i| app.rows[*i].id.as_str())
        .collect()
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
fn billing_filters_use_exact_signed_cents_and_reported_overdue_days_not_status_or_due_text() {
    let mut app = app(View::Billing);
    let generation = app.generation;
    let updated = app.updated;
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(
        ids(&app),
        ["current", "late", "no-age", "bad-age", "negative-age"]
    );
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.balance_filter, BalanceFilter::Overdue);
    assert_eq!(ids(&app), ["late"]);
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(ids(&app), ["credit-min", "credit-one"]);
    assert_eq!(
        app.current().expect("credit").cells[4],
        "-92233720368547758.08"
    );
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(ids(&app), ["zero"]);
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(ids(&app), ["missing", "string", "fraction", "overflow"]);
    assert!(app.visible.iter().all(|i| app.rows[*i].cells[4] == "—"));
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.balance_filter, BalanceFilter::All);
    assert_eq!(app.visible.len(), 12);
    assert_eq!(app.generation, generation);
    assert_eq!(app.updated, updated);
    assert!(!app.needs_fetch);
    assert!(app.write_ready.is_none());
}

#[test]
fn payables_cycle_skips_unsupported_aging_and_never_confuses_unknown_or_credit_with_zero() {
    let mut app = app(View::Payables);
    for (expected, count) in [
        (BalanceFilter::Open, 5),
        (BalanceFilter::Credits, 2),
        (BalanceFilter::Zero, 1),
        (BalanceFilter::Unknown, 4),
        (BalanceFilter::All, 12),
    ] {
        key(&mut app, KeyCode::Char('f'));
        assert_eq!(app.balance_filter, expected);
        assert_eq!(app.visible.len(), count);
    }
    assert_eq!(
        BalanceFilter::Credits.title(View::Payables),
        "Carrier credits"
    );
    assert_eq!(BalanceFilter::Credits.title(View::Billing), "Refunds due");
}

#[test]
fn balance_filter_search_and_clear_combine_locally_and_keep_workspace_totals() {
    let mut app = app(View::Billing);
    let finance = app.finance.clone();
    let generation = app.generation;
    key(&mut app, KeyCode::Char('f'));
    search(&mut app, "no-age");
    assert_eq!(ids(&app), ["no-age"]);
    key(&mut app, KeyCode::Char('f'));
    assert!(app.visible.is_empty());
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.visible.len(), 12);
    assert_eq!(app.balance_filter, BalanceFilter::All);
    assert!(app.query.is_empty());
    assert!(!app.has_filters());
    assert_eq!(app.finance, finance);
    assert_eq!(app.generation, generation);
    assert!(!app.needs_fetch);
}

#[test]
fn financial_workspaces_and_linked_shipment_return_retain_independent_filters_and_selection() {
    let mut app = app(View::Billing);
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('f'));
    search(&mut app, "Owned customer");
    key(&mut app, KeyCode::Char('4'));
    read(&mut app, data());
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('f'));
    app.selected = 1;
    key(&mut app, KeyCode::Char('3'));
    read(&mut app, data());
    assert_eq!(app.balance_filter, BalanceFilter::Overdue);
    assert_eq!(app.current().expect("invoice").id, "late");
    assert_eq!(app.query, "Owned customer");
    key(&mut app, KeyCode::Char('o'));
    assert_eq!(app.view, View::Loads);
    let response = demo::snapshot(&app.request()).data;
    read(&mut app, response);
    assert_eq!(app.current().expect("shipment").id, "load-demo-0003");
    key(&mut app, KeyCode::Char('b'));
    read(&mut app, data());
    assert_eq!(app.balance_filter, BalanceFilter::Overdue);
    assert_eq!(app.current().expect("invoice").id, "late");
    key(&mut app, KeyCode::Char('4'));
    read(&mut app, data());
    assert_eq!(app.balance_filter, BalanceFilter::Credits);
    assert_eq!(app.current().expect("payable").id, "credit-one");
    key(&mut app, KeyCode::Backspace);
    key(&mut app, KeyCode::Char('3'));
    read(&mut app, data());
    assert_eq!(app.balance_filter, BalanceFilter::Overdue);
}

#[test]
fn inflight_and_scheduled_finance_reads_keep_filters_but_reclassify_changed_balances() {
    let mut app = app(View::Billing);
    key(&mut app, KeyCode::Char('r'));
    app.loading = Some(app.generation);
    app.needs_fetch = false;
    app.selected = 1;
    key(&mut app, KeyCode::Char('f'));
    key(&mut app, KeyCode::Char('f'));
    let mut response = data();
    response["bills"].as_array_mut().expect("bills").reverse();
    read(&mut app, response.clone());
    assert_eq!(app.current().expect("retained").id, "late");
    app.refresh_interval = Some(Duration::from_secs(5));
    app.tick(Instant::now() + Duration::from_secs(6));
    assert!(app.needs_fetch);
    response["bills"]
        .as_array_mut()
        .expect("bills")
        .iter_mut()
        .find(|v| v["id"] == "late")
        .expect("late")["totals"]["balance_cents"] = json!(0);
    read(&mut app, response);
    assert_eq!(app.balance_filter, BalanceFilter::Overdue);
    assert!(app.visible.is_empty());
    assert!(app.write_ready.is_none());
}

#[test]
fn denial_and_tenant_or_subject_changes_clear_active_and_cached_finance_filters() {
    for changed in ["denied", "tenant", "subject"] {
        let mut app = app(View::Billing);
        key(&mut app, KeyCode::Char('f'));
        key(&mut app, KeyCode::Char('4'));
        read(&mut app, data());
        key(&mut app, KeyCode::Char('f'));
        if changed == "denied" {
            app.clear_access();
        } else {
            let mut identity = demo::snapshot(&app.request()).identity;
            if changed == "tenant" {
                identity["tenant_id"] = json!("other-tenant");
            } else {
                identity["access"]["subject"] = json!("other-subject");
            }
            app.finish(
                app.generation,
                Ok(Snapshot {
                    identity,
                    data: data(),
                }),
            );
        }
        assert_eq!(app.balance_filter, BalanceFilter::All);
        assert!(app.rows.is_empty());
        key(&mut app, KeyCode::Char('3'));
        assert_eq!(app.balance_filter, BalanceFilter::All);
    }
}

#[test]
fn finance_shortcut_is_text_in_search_and_does_not_escape_inspection_or_paste() {
    let mut app = app(View::Billing);
    app.paste("f");
    assert_eq!(app.balance_filter, BalanceFilter::All);
    key(&mut app, KeyCode::Char('/'));
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.draft, "f");
    assert_eq!(app.balance_filter, BalanceFilter::All);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Char('f'));
    assert!(app.details);
    assert_eq!(app.balance_filter, BalanceFilter::All);
    key(&mut app, KeyCode::Esc);
    key(&mut app, KeyCode::Char('1'));
    key(&mut app, KeyCode::Char('f'));
    assert_eq!(app.balance_filter, BalanceFilter::All);
}

#[test]
fn financial_filter_summary_and_controls_fit_compact_widths_and_all_palettes() {
    for view in [View::Billing, View::Payables] {
        for width in [52, 77, 120] {
            for (no_color, light) in [(false, false), (false, true), (true, false)] {
                let mut app = app(view);
                app.balance_filter = BalanceFilter::Unknown;
                app.filter();
                search(&mut app, &"z".repeat(200));
                let text = screen(&mut app, width, no_color, light);
                assert!(text.contains("[Unknown balances]"));
                assert!(text.contains("Backspace clears filters"));
                let footer = text.lines().last().expect("footer");
                assert!(footer.contains("f balance filter") && footer.contains("? q"));
                key(&mut app, KeyCode::Backspace);
                key(&mut app, KeyCode::Char('f'));
                let text = screen(&mut app, width, no_color, light);
                assert!(text.contains("Backspace clear  o shipment"));
            }
        }
    }
}

#[test]
fn finance_filters_preserve_the_entire_uncertain_original_and_recovery_controls() {
    let mut app = app(View::Billing);
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
    search(&mut app, "late");
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("retained"),
        original
    );
    let text = screen(&mut app, 52, false, false);
    assert!(text.contains("c check original") && text.contains("x stop review"));
    assert_eq!(
        app.write_notice.as_deref(),
        Some("Original outcome remains uncertain")
    );
    assert!(app.write_uncertain);
    assert!(app.write_ready.is_none());
    assert!(!app.needs_fetch);
}
