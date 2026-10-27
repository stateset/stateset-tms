use crate::{
    client::{self, Snapshot},
    config, demo,
    model::{App, BalanceFilter, QueueDirection, View},
    tests::{response, server},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{json, Value};
fn key(app: &mut App, c: KeyCode) {
    app.key(KeyEvent::new(c, KeyModifiers::NONE));
}
fn identity(app: &App) -> Value {
    let mut id = demo::snapshot(&app.request()).identity;
    id["features"]["console_catalog"] = json!(true);
    id
}
fn data(view: View, start: usize) -> Value {
    let rows=(start..start+50).map(|n|match view {
        View::Billing=>json!({"id":format!("bill-{n:03}"),"number":format!("INV-{n:03}"),"load_id":"load-demo-0003","snapshot":{"amount_cents":100},"totals":{"balance_cents":100}}),
        View::Payables=>json!({"id":format!("settlement-{n:03}"),"load_id":"load-demo-0003","total_amount_cents":100,"balance_cents":100}),
        _=>json!({"id":format!("queue-{n:03}"),"load_id":"load-demo-0003","status":"retry","total_attempts":1}),
    }).collect::<Vec<_>>();
    let mut d = json!({"tenant_id":"synthetic-demo","view":match view{View::Billing=>"billing",View::Payables=>"payables",_=>"integrations"},"limit":50,"total":150,"totals_scope":"workspace","bills":[],"settlements":[],"outbox":[],"inbox":[],"next_cursor":format!("opaque-{}",start+50),"finance":{"receivable_cents":12345}});
    d[match view {
        View::Billing => "bills",
        View::Payables => "settlements",
        _ => "outbox",
    }] = json!(rows);
    d
}
fn apply(app: &mut App, start: usize) {
    let snapshot = Snapshot {
        identity: identity(app),
        data: data(app.view, start),
    };
    app.needs_fetch = false;
    app.finish(app.generation, Ok(snapshot));
}
fn loaded(view: View) -> App {
    let mut app = App {
        view,
        ..App::default()
    };
    apply(&mut app, 0);
    app
}
#[test]
fn financial_and_queue_pages_restore_identity_viewport_and_current_page_retry() {
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut app = loaded(view);
        assert!(app.paged());
        app.selected = 31;
        *app.table.offset_mut() = 20;
        let id = app.current().expect("row").selection_key();
        key(&mut app, KeyCode::Char('n'));
        assert_eq!(app.request().cursor.as_deref(), Some("opaque-50"));
        assert!(app.rows.is_empty());
        apply(&mut app, 50);
        app.selected = 4;
        key(&mut app, KeyCode::Char('p'));
        apply(&mut app, 0);
        assert_eq!(app.current().expect("restored").selection_key(), id);
        assert_eq!(app.table.offset(), 20);
        key(&mut app, KeyCode::Char('n'));
        apply(&mut app, 50);
        assert_eq!(app.selected, 4);
        app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(app.request().cursor.as_deref(), Some("opaque-50"));
        apply(&mut app, 50);
        assert_eq!(app.selected, 4);
        assert!(app.pending.is_none() && app.write_ready.is_none());
    }
}
#[test]
fn financial_and_queue_filters_invalidate_pages_and_encode_only_their_scope() {
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut app = loaded(view);
        key(&mut app, KeyCode::Char('n'));
        apply(&mut app, 50);
        let before = app.generation;
        key(&mut app, KeyCode::Char('f'));
        assert!(app.needs_fetch && app.generation > before);
        assert!(app.rows.is_empty());
        assert!(app.request().cursor.is_none());
        if view == View::Integrations {
            assert!(app.queue_attention);
            key(&mut app, KeyCode::Char('d'));
            assert_eq!(app.queue_direction, QueueDirection::Outbound);
        } else {
            assert_eq!(app.balance_filter, BalanceFilter::Open);
        }
        let url = client::endpoint(
            &config::validate_origin("http://localhost:8090").expect("origin"),
            &app.request(),
        );
        assert_eq!(url.path(), "/v1/console");
        let q = url
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(q["limit"], "50");
        assert!(!q.contains_key("history") && !q.contains_key("status"));
        if view == View::Integrations {
            assert_eq!(q["direction"], "outbound");
            assert_eq!(q["attention"], "true");
            assert!(!q.contains_key("balance"));
        } else {
            assert_eq!(q["balance"], "open");
            assert!(!q.contains_key("direction"));
        }
    }
}
#[test]
fn server_search_does_not_locally_hide_matching_projected_rows_and_backspace_refetches() {
    let mut app = loaded(View::Billing);
    key(&mut app, KeyCode::Char('/'));
    app.paste(" Zürich &q=private ");
    key(&mut app, KeyCode::Enter);
    assert!(app.needs_fetch && app.rows.is_empty());
    assert_eq!(app.query, "Zürich &q=private");
    apply(&mut app, 100);
    assert_eq!(app.visible.len(), 50);
    assert_eq!(app.finance["receivable_cents"], 12345);
    let url = client::endpoint(
        &config::validate_origin("http://localhost:8090").expect("origin"),
        &app.request(),
    );
    assert_eq!(url.query_pairs().filter(|(k, _)| k == "q").count(), 1);
    assert!(url.query_pairs().any(|(k, v)| k == "q" && v == app.query));
    key(&mut app, KeyCode::Backspace);
    assert!(app.query.is_empty() && app.needs_fetch && app.rows.is_empty());
}
#[test]
fn api_upgrade_and_rollback_clear_cached_cursors_but_preserve_filters() {
    let mut app = loaded(View::Billing);
    app.query = "invoice".into();
    key(&mut app, KeyCode::Char('n'));
    apply(&mut app, 50);
    key(&mut app, KeyCode::Char('4'));
    apply(&mut app, 0);
    app.balance_filter = BalanceFilter::Credits;
    key(&mut app, KeyCode::Char('n'));
    apply(&mut app, 50);
    let mut id = identity(&app);
    id["features"]["console_catalog"] = json!(false);
    app.apply(Snapshot {
        identity: id.clone(),
        data: data(app.view, 50),
    });
    assert!(!app.catalog());
    assert!(app.request().cursor.is_none() && app.rows.is_empty() && app.needs_fetch);
    assert_eq!(app.balance_filter, BalanceFilter::Credits);
    key(&mut app, KeyCode::Char('3'));
    assert!(app.request().cursor.is_none());
    assert_eq!(app.query, "invoice");
    app.apply(Snapshot {
        identity: id,
        data: data(app.view, 0),
    });
    app.apply(Snapshot {
        identity: identity(&app),
        data: data(app.view, 0),
    });
    assert!(app.catalog() && app.needs_fetch && app.rows.is_empty());
    assert_eq!(app.query, "invoice");
}
#[test]
fn old_api_and_demo_keep_small_local_financial_and_queue_filters() {
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut app = App {
            view,
            ..App::default()
        };
        app.apply(demo::snapshot(&app.request()));
        let generation = app.generation;
        key(&mut app, KeyCode::Char('f'));
        assert_eq!(app.generation, generation);
        assert!(!app.paged());
        let url = client::endpoint(
            &config::validate_origin("http://localhost:8090").expect("origin"),
            &app.request(),
        );
        assert_eq!(url.path(), "/v1/workspace");
    }
}
#[test]
fn fresh_access_selects_catalog_on_first_read_and_drops_incompatible_cursor_on_rollback() {
    for (enabled, first) in [(true, true), (true, false), (false, false)] {
        let mut app = loaded(View::Billing);
        app.cursors = vec![None, Some("old-cursor".into())];
        app.identity["features"]["console_catalog"] = json!(!enabled);
        if first {
            app.identity = Value::Null;
        }
        let mut id = identity(&app);
        id["features"]["console_catalog"] = json!(enabled);
        let (origin, worker) = server(vec![response(id), response(data(app.view, 0))]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let api = reqwest::Client::new();
        runtime
            .block_on(client::fetch(&api, &origin, &app.request()))
            .expect("read");
        let requests = worker.join().expect("requests");
        assert!(requests[0].starts_with("GET /v1/access "));
        assert!(requests[1].starts_with(if enabled {
            "GET /v1/console?"
        } else {
            "GET /v1/workspace?"
        }));
        assert!(!requests[1].contains("cursor="));
        assert!(requests.iter().all(|r| r.starts_with("GET ")));
    }
}
#[test]
fn catalog_rejects_unbounded_wrong_view_and_missing_arrays_before_showing_rows() {
    for change in [
        json!({"limit":100}),
        json!({"view":"payables"}),
        json!({"total":1}),
        json!({"totals_scope":"page"}),
        json!({"inbox":null}),
        json!({"next_cursor":""}),
        json!({"bills":vec![json!({"id":"too-many"});51]}),
    ] {
        let app = loaded(View::Billing);
        let mut d = data(app.view, 0);
        for (k, v) in change.as_object().expect("change") {
            d[k] = v.clone();
        }
        let (origin, worker) = server(vec![response(identity(&app)), response(d)]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let failure = runtime
            .block_on(client::fetch(
                &reqwest::Client::new(),
                &origin,
                &app.request(),
            ))
            .err()
            .expect("malformed page refused");
        assert!(!failure.denied && failure.message.contains("invalid console page"));
        worker.join().expect("worker");
    }
}
#[test]
fn compact_catalog_shows_global_totals_scope_and_page_commands() {
    use ratatui::{backend::TestBackend, Terminal};
    for view in [View::Billing, View::Payables, View::Integrations] {
        let mut app = loaded(view);
        let mut terminal = Terminal::new(TestBackend::new(77, 30)).expect("terminal");
        terminal
            .draw(|frame| {
                crate::ui::render(
                    frame,
                    &mut app,
                    &crate::ui::Theme::new(true, false),
                    false,
                    "http://localhost:8090",
                )
            })
            .expect("draw");
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(screen.contains("n/p pages") && screen.contains("150 matching"));
        if view.financial() {
            assert!(screen.contains("All receivable USD"));
        }
    }
}

#[test]
fn feature_transition_keeps_an_uncertain_original_and_its_retained_stop_decision() {
    use crate::actions::{Kind, Owner, Pending};
    let mut app = App::default();
    app.apply(demo::snapshot(&app.request()));
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
    let original = serde_json::to_value(app.pending.as_ref().expect("original")).expect("bytes");
    key(&mut app, KeyCode::Char('3'));
    app.apply(Snapshot {
        identity: identity(&app),
        data: data(View::Billing, 0),
    });
    assert!(
        app.catalog() && app.request().cursor.is_none() && app.rows.is_empty() && app.needs_fetch
    );
    assert_eq!(
        serde_json::to_value(app.pending.as_ref().expect("retained")).expect("bytes"),
        original
    );
    assert_eq!(app.stopping_note.as_deref(), Some("Retained stop decision"));
    assert!(app.write_uncertain && app.write_ready.is_none());
}

#[test]
fn console_feature_changes_keep_load_and_operations_page_requests_compatible() {
    for view in [View::Loads, View::Operations] {
        let mut app = App {
            view,
            ..App::default()
        };
        app.apply(demo::snapshot(&app.request()));
        app.cursors = vec![None, Some("50".into())];
        let mut snapshot = demo::snapshot(&app.request());
        snapshot.identity["features"]["console_catalog"] = json!(true);
        let (origin, worker) = server(vec![response(snapshot.identity), response(snapshot.data)]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let snapshot = runtime
            .block_on(client::fetch(
                &reqwest::Client::new(),
                &origin,
                &app.request(),
            ))
            .expect("read");
        app.apply(snapshot);
        assert_eq!(app.request().cursor.as_deref(), Some("50"));
        let requests = worker.join().expect("worker");
        assert!(requests[1].contains("cursor=50"));
    }
}
