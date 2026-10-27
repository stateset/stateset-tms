use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use serde_json::json;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn demo_app() -> App {
    let mut app = App {
        needs_fetch: false,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app
}

#[test]
fn origin_validation_keeps_credentials_on_the_configured_api() {
    for url in [
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com/path",
        "https://example.com?token=secret",
        "https://example.com/#fragment",
        "file:///tmp/demo",
        "http://127.0.0.1.evil.test",
    ] {
        assert!(config::validate_origin(url).is_err(), "{url}");
    }
    for url in [
        "https://freight.example.com",
        "http://127.0.0.1:8090",
        "http://localhost:8090",
        "http://[::1]:8090",
    ] {
        assert!(config::validate_origin(url).is_ok(), "{url}");
    }
}

#[test]
fn command_line_views_are_explicit_and_credential_flags_are_not_accepted() {
    for (name, view) in [
        ("loads", model::View::Loads),
        ("operations", model::View::Operations),
        ("billing", model::View::Billing),
        ("payables", model::View::Payables),
        ("integrations", model::View::Integrations),
    ] {
        let config = Config::parse(
            [
                "--demo",
                "--snapshot",
                "--api",
                "http://127.0.0.1:8090",
                "--view",
                name,
            ]
            .map(String::from),
        )
        .expect("configuration")
        .expect("not help");
        assert_eq!(config.view, view);
        assert!(config.headers.get(reqwest::header::AUTHORIZATION).is_none());
    }
    let error = Config::parse(["--token", "do-not-echo-this"].map(String::from))
        .err()
        .expect("credential flag refused");
    assert!(!error.contains("do-not-echo-this"));
}
#[test]
fn search_and_status_use_encoded_server_filters_and_reset_cursors() {
    let mut app = demo_app();
    app.key(key(KeyCode::Char('n')));
    assert_eq!(app.cursors.len(), 2);
    app.key(key(KeyCode::Char('/')));
    app.paste("Phoenix &q=other");
    app.key(key(KeyCode::Enter));
    assert_eq!(app.cursors, vec![None]);
    assert!(app.visible.is_empty());
    let url = client::endpoint(
        &config::validate_origin("http://localhost:8090").expect("origin"),
        &app.request(),
    );
    let pairs = url.query_pairs().collect::<Vec<_>>();
    assert_eq!(pairs.iter().filter(|(k, _)| k == "q").count(), 1);
    assert!(pairs
        .iter()
        .any(|(k, v)| k == "q" && v == "Phoenix &q=other"));
    app.key(key(KeyCode::Char('s')));
    assert_eq!(app.request().status, "planned");
}
#[test]
fn delayed_reads_cannot_replace_a_different_view_and_denial_clears_records() {
    let mut app = demo_app();
    let old = app.generation;
    let snapshot = demo::snapshot(&app.request());
    app.key(key(KeyCode::Char('3')));
    app.finish(old, Ok(snapshot));
    assert_eq!(app.view, model::View::Billing);
    assert!(app.rows.is_empty());
    app.apply(demo::snapshot(&app.request()));
    assert!(!app.rows.is_empty());
    app.finish(
        old,
        Err(client::Failure {
            message: "Denied".into(),
            denied: true,
        }),
    );
    assert!(app.rows.is_empty());
    assert!(app.visible.is_empty());
    assert!(app.identity.is_null());
    assert!(app.finance.is_null());
    assert!(!app.needs_fetch);
}
#[test]
fn transport_failure_retains_snapshot_and_requires_explicit_refresh() {
    let mut app = demo_app();
    let count = app.rows.len();
    app.finish(
        app.generation,
        Err(client::Failure {
            message: "Unavailable".into(),
            denied: false,
        }),
    );
    assert_eq!(app.rows.len(), count);
    assert_eq!(app.error.as_deref(), Some("Unavailable"));
    assert!(!app.needs_fetch);
    app.key(key(KeyCode::Char('r')));
    assert!(app.needs_fetch);
}
#[test]
fn demo_pages_are_bounded_and_status_searches_combine() {
    let mut app = demo_app();
    assert_eq!(app.rows.len(), 50);
    let first = app.rows[0].id.clone();
    app.key(key(KeyCode::Char('n')));
    app.apply(demo::snapshot(&app.request()));
    assert_eq!(app.rows.len(), 15);
    assert!(app.next_cursor.is_none());
    app.key(key(KeyCode::Char('p')));
    app.apply(demo::snapshot(&app.request()));
    assert_eq!(app.rows[0].id, first);
    app.key(key(KeyCode::Char('/')));
    app.paste("Dallas");
    app.key(key(KeyCode::Enter));
    app.key(key(KeyCode::Char('s')));
    app.apply(demo::snapshot(&app.request()));
    assert!(!app.rows.is_empty());
    assert!(app
        .rows
        .iter()
        .all(|r| r.status == "planned" && r.cells.iter().any(|v| v.contains("Dallas"))));
}
#[test]
fn unicode_search_is_byte_bounded_and_paste_cannot_trigger_shortcuts() {
    let mut app = demo_app();
    app.paste("q");
    assert!(!app.quit);
    app.key(key(KeyCode::Char('/')));
    app.paste(&"東".repeat(100));
    assert!(app.draft.len() <= 200);
    app.key(key(KeyCode::Backspace));
    assert!(app.draft.is_char_boundary(app.draft.len()));
    app.key(key(KeyCode::Esc));
    assert_eq!(app.query, "");
    assert!(!app.quit);
}
#[test]
fn terminal_control_sequences_are_never_rendered_as_controls() {
    let raw = "hello\x1b]52;c;data\x07\u{009b}2J\u{202e}world\r\n\t";
    let cleaned = model::clean(raw);
    assert!(!cleaned.chars().any(char::is_control));
    assert!(!cleaned.contains('\u{202e}'));
    assert!(cleaned.contains("hello"));
    assert_eq!(model::clean(&"a".repeat(5000)).len(), 4096);
}
#[test]
fn signed_money_is_exact_at_both_integer_limits_and_absence_is_unknown() {
    assert_eq!(model::money(&json!(i64::MIN)), "-92233720368547758.08");
    assert_eq!(model::money(&json!(i64::MAX)), "92233720368547758.07");
    assert_eq!(model::money(&json!(-1)), "-0.01");
    assert_eq!(model::money(&json!(null)), "—");
    assert_eq!(model::money_in(&json!(150001), &json!(null)), "1500.01");
    assert_eq!(model::money_in(&json!(150001), &json!("USD")), "1500.01");
    assert_eq!(
        model::money_in(&json!(150000), &json!("EUR")),
        "EUR 1500.00"
    );
    assert_eq!(model::money_in(&json!(150000), &json!("JPY")), "JPY 150000");
    assert_eq!(model::money_in(&json!(-1), &json!("BHD")), "BHD -0.001");
    assert_eq!(model::money_in(&json!(null), &json!("EUR")), "—");
    let usd_only = json!({"currencies":[{"currency":"USD","receivable_cents":100}]});
    assert_eq!(model::currency_totals(&usd_only, "receivable_cents"), None);
    let mixed = json!({"currencies":[
        {"currency":"EUR","receivable_cents":150000},
        {"currency":"JPY","receivable_cents":0},
        {"currency":"USD","receivable_cents":-1}]});
    assert_eq!(
        model::currency_totals(&mixed, "receivable_cents").as_deref(),
        Some("-0.01 · EUR 1500.00")
    );
    assert_eq!(model::currency_totals(&json!({}), "receivable_cents"), None);
}
#[test]
fn operation_details_follow_the_real_nested_signal_shape() {
    let data = json!({"issues":[{"id":"issue-1","load_reference":"FRT-1","owner":"dispatcher","source_active":true,"evidence_valid":true,"signal":{"title":"POD missing","priority":1,"summary":"Inspect delivery","next_action":"Obtain original POD","load_id":"load-1"}}]});
    let rows = model::records(model::View::Operations, &data);
    assert_eq!(rows[0].cells[1], "P1");
    assert_eq!(rows[0].cells[2], "POD missing");
    assert!(rows[0]
        .details
        .iter()
        .any(|(k, v)| k == "Next action" && v == "Obtain original POD"));
}
#[test]
fn all_views_render_at_small_and_wide_sizes_in_both_themes() {
    for (width, height) in [(40, 10), (52, 16), (80, 24), (120, 40), (180, 50)] {
        for view in model::View::ALL {
            let mut app = App {
                view,
                ..App::default()
            };
            app.needs_fetch = false;
            app.apply(demo::snapshot(&app.request()));
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            for (no_color, light) in [(false, false), (false, true), (true, false)] {
                terminal
                    .draw(|f| {
                        ui::render(
                            f,
                            &mut app,
                            &ui::Theme::new(no_color, light),
                            true,
                            "offline",
                        )
                    })
                    .expect("render");
                if view == model::View::Loads && width >= 52 && height >= 16 {
                    let visible = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>();
                    assert!(
                        visible.contains("FRT-10421"),
                        "shipment row must remain visible at {width}x{height}"
                    );
                }
                app.details = true;
                terminal
                    .draw(|f| {
                        ui::render(
                            f,
                            &mut app,
                            &ui::Theme::new(no_color, light),
                            true,
                            "offline",
                        )
                    })
                    .expect("details");
                app.details = false;
            }
        }
    }
}

pub(crate) fn server(responses: Vec<String>) -> (reqwest::Url, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let origin = config::validate_origin(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ))
    .expect("origin");
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let (mut stream, _) = listener.accept().expect("accept");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("timeout");
                let mut buffer = vec![];
                let mut chunk = [0u8; 1024];
                while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < 16384 {
                    let n = stream.read(&mut chunk).expect("request");
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&buffer[..end]);
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.split_once(':').and_then(|(key, value)| {
                                if key.eq_ignore_ascii_case("content-length") {
                                    value.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                        })
                        .unwrap_or(0);
                    assert!(length <= 16 * 1024, "owned fixture body limit");
                    while buffer.len() < end + 4 + length {
                        let n = stream.read(&mut chunk).expect("body");
                        assert!(n > 0, "complete request body");
                        buffer.extend_from_slice(&chunk[..n]);
                    }
                }
                stream.write_all(response.as_bytes()).expect("response");
                String::from_utf8(buffer).expect("HTTP header")
            })
            .collect()
    });
    (origin, handle)
}
pub(crate) fn response(value: serde_json::Value) -> String {
    let body = value.to_string();
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())
}
fn fetch_mock(
    responses: Vec<String>,
    view: model::View,
) -> (Result<client::Snapshot, client::Failure>, Vec<String>) {
    let (origin, server) = server(responses);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_static("Bearer synthetic-terminal-test"),
    );
    let config = Config {
        graphics: graphics::Mode::Auto,
        connect: false,
        recovery_key: None,
        refresh_interval: None,
        stale_after: Duration::from_secs(60),
        demo: false,
        snapshot: false,
        no_color: true,
        light: false,
        view,
        operations_scope: crate::operations::Scope::Actionable,
        origin,
        headers,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let app = App {
        view,
        ..App::default()
    };
    let result = runtime.block_on(client::fetch(
        &config.client().expect("client"),
        &config.origin,
        &app.request(),
    ));
    (result, server.join().expect("server"))
}
fn identity() -> serde_json::Value {
    json!({"tenant_id":"test","access":{"role":"viewer","capabilities":["workspace.read"]}})
}
#[test]
fn actual_http_client_verifies_access_then_fetches_a_bounded_load_page() {
    let (result, requests) = fetch_mock(
        vec![
            response(identity()),
            response(json!({"loads":[],"next_cursor":null})),
        ],
        model::View::Loads,
    );
    assert!(result.is_ok());
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/access "));
    assert!(requests[1].starts_with("GET /v1/loads?limit=50 "));
    assert!(requests.iter().all(|r| r
        .to_lowercase()
        .contains("authorization: bearer synthetic-terminal-test")));
}
#[test]
fn actual_http_client_refuses_redirects_and_does_not_echo_error_bodies() {
    let (result,requests)=fetch_mock(vec!["HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()],model::View::Loads);
    assert!(result
        .err()
        .expect("redirect refused")
        .message
        .contains("redirect refused"));
    assert_eq!(requests.len(), 1);
    let (result,_)=fetch_mock(vec!["HTTP/1.1 401 Unauthorized\r\nContent-Length: 12\r\nConnection: close\r\n\r\nsecret-token".into()],model::View::Loads);
    let error = result.err().expect("denied");
    assert!(error.denied);
    assert!(!error.message.contains("secret-token"));
}
#[test]
fn actual_http_client_refuses_cross_tenant_and_oversized_responses() {
    let (result, _) = fetch_mock(
        vec![
            response(identity()),
            response(json!({"tenant_id":"other","bills":[]})),
        ],
        model::View::Billing,
    );
    assert!(result.err().expect("identity mismatch").denied);
    let (result, _) = fetch_mock(
        vec!["HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\nConnection: close\r\n\r\n".into()],
        model::View::Loads,
    );
    assert!(result.err().expect("oversized").message.contains("32 MiB"));
}

#[test]
fn related_shipment_lookup_verifies_access_and_uses_the_bounded_catalog_over_http() {
    let mut app = App {
        view: model::View::Operations,
        needs_fetch: false,
        ..App::default()
    };
    app.apply(demo::snapshot(&app.request()));
    app.selected = 1;
    app.key(key(KeyCode::Char('o')));
    let mut snapshot = demo::snapshot(&app.request());
    let mut other = snapshot.data["loads"][0].clone();
    other["id"] = "load-demo-0001-other".into();
    snapshot.data["loads"]
        .as_array_mut()
        .expect("loads")
        .insert(0, other);
    let (origin, worker) = server(vec![response(snapshot.identity), response(snapshot.data)]);
    let config = Config::parse(["--demo", "--api", origin.as_str()].map(String::from))
        .expect("config")
        .expect("not help");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(client::fetch(
        &config.client().expect("client"),
        &origin,
        &app.request(),
    ));
    app.needs_fetch = false;
    app.finish(app.generation, result);
    let requests = worker.join().expect("worker");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/access "));
    assert!(requests[1].starts_with("GET /v1/loads?limit=50&q=load-demo-0001 "));
    assert_eq!(app.rows.len(), 1);
    assert_eq!(
        app.current().expect("exact linked shipment").id,
        "load-demo-0001"
    );
    assert!(app.write_ready.is_none());
    assert!(app.pending.is_none());
}

#[test]
fn operations_scopes_are_access_checked_server_queries_with_no_caller_identity_override() {
    for scope in crate::operations::Scope::ALL {
        let app = App {
            view: model::View::Operations,
            operations_scope: scope,
            query: "POD &view=all".into(),
            ..App::default()
        };
        let mut request = app.request();
        request.cursor = Some("owned.cursor+data".into());
        let snapshot = demo::snapshot(&request);
        let (origin, worker) = server(vec![response(snapshot.identity), response(snapshot.data)]);
        let config = Config::parse(["--demo", "--api", origin.as_str()].map(String::from))
            .expect("config")
            .expect("not help");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime
            .block_on(client::fetch(
                &config.client().expect("client"),
                &origin,
                &request,
            ))
            .expect("read");
        let requests = worker.join().expect("worker");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /v1/access "));
        let path = requests[1].split_whitespace().nth(1).expect("request path");
        let url = reqwest::Url::parse(&format!("http://localhost{path}")).expect("URL");
        assert_eq!(url.path(), "/v1/operations");
        let query = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query.len(), 4);
        assert_eq!(query["view"], scope.code());
        assert_eq!(query["limit"], "50");
        assert_eq!(query["search"], "POD &view=all");
        assert_eq!(query["cursor"], "owned.cursor+data");
        assert!(!query.contains_key("owner") && !query.contains_key("subject"));
    }
    for view in [
        model::View::Loads,
        model::View::Billing,
        model::View::Payables,
        model::View::Integrations,
    ] {
        let app = App {
            view,
            operations_scope: crate::operations::Scope::Mine,
            ..App::default()
        };
        let url = client::endpoint(
            &config::validate_origin("http://127.0.0.1:8090").expect("origin"),
            &app.request(),
        );
        assert!(!url.query_pairs().any(|(key, _)| key == "view"));
    }
}

#[test]
fn explicit_current_page_retry_rechecks_access_and_recovers_the_same_cursor_using_only_gets() {
    let mut app = demo_app();
    app.key(key(KeyCode::Char('n')));
    app.needs_fetch = false;
    app.apply(demo::snapshot(&app.request()));
    app.selected = 3;
    let id = app.current().expect("selected load").id.clone();
    let snapshot = demo::snapshot(&app.request());
    let (origin, worker) = server(vec![
        response(snapshot.identity.clone()),
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        response(snapshot.identity),
        response(snapshot.data),
    ]);
    let config = Config::parse(["--demo", "--api", origin.as_str()].map(String::from))
        .expect("config")
        .expect("not help");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let api = config.client().expect("client");
    for attempt in 0..2 {
        app.key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        app.needs_fetch = false;
        let result = runtime.block_on(client::fetch(&api, &origin, &app.request()));
        app.finish(app.generation, result);
        assert_eq!(app.request().cursor.as_deref(), Some("50"));
        assert_eq!(app.current().expect("same load").id, id);
        if attempt == 0 {
            assert!(app.error.is_some() && app.stale());
        } else {
            assert!(app.error.is_none() && !app.stale());
        }
    }
    let requests = worker.join().expect("worker");
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert!(request.starts_with("GET "));
        assert!(!request.to_lowercase().contains("idempotency-key"));
    }
    assert!(
        requests[0].starts_with("GET /v1/access ") && requests[2].starts_with("GET /v1/access ")
    );
    for index in [1, 3] {
        assert!(requests[index].starts_with("GET /v1/loads?limit=50&cursor=50 "));
    }
    assert!(app.write_ready.is_none() && app.pending.is_none());
}

#[test]
fn returning_to_a_saved_page_rechecks_access_and_displays_fresh_server_fields() {
    let mut app = demo_app();
    app.selected = 49;
    let identity = demo::snapshot(&app.request()).identity;
    let second = demo::snapshot(
        &App {
            cursors: vec![None, Some("50".into())],
            ..App::default()
        }
        .request(),
    )
    .data;
    let mut first = demo::snapshot(&app.request()).data;
    first["loads"][49]["location"] = json!("Fresh server observation");
    first["loads"][49]["version"] = json!(99);
    let (origin, worker) = server(vec![
        response(identity.clone()),
        response(second),
        response(identity),
        response(first),
    ]);
    let config = Config::parse(["--demo", "--api", origin.as_str()].map(String::from))
        .expect("config")
        .expect("not help");
    let api = config.client().expect("client");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    for code in ['n', 'p'] {
        app.key(key(KeyCode::Char(code)));
        assert!(app.rows.is_empty() && app.needs_fetch);
        app.needs_fetch = false;
        let result = runtime.block_on(client::fetch(&api, &origin, &app.request()));
        app.finish(app.generation, result);
    }
    let row = app.current().expect("saved row from fresh data");
    assert_eq!(row.id, "load-demo-0049");
    assert!(row
        .details
        .iter()
        .any(|(k, v)| k == "Location" && v == "Fresh server observation"));
    assert_eq!(row.target.as_ref().expect("fresh version").version, 99);
    let requests = worker.join().expect("worker");
    assert_eq!(requests.len(), 4);
    assert!(
        requests[0].starts_with("GET /v1/access ") && requests[2].starts_with("GET /v1/access ")
    );
    assert!(requests[1].starts_with("GET /v1/loads?limit=50&cursor=50 "));
    assert!(requests[3].starts_with("GET /v1/loads?limit=50 "));
    assert!(requests.iter().all(|r| r.starts_with("GET ")));
    assert!(app.write_ready.is_none());
}

#[test]
fn party_discovery_rechecks_access_and_sends_exact_combined_filters_over_http() {
    use crate::parties::{LoadParties, Party};
    let customer = " Réseau & Sons/West?x=1#2+% ";
    let carrier = "carrier/one&two";
    let mut app = demo_app();
    app.rows[0].load_parties =
        LoadParties::from_load(&json!({"customer_id":customer,"carrier_id":carrier}));
    app.query = "Los Angeles".into();
    app.status_index = 5;
    app.filter_selected_party(Party::Customer);
    let mut first = demo::snapshot(&App::default().request());
    first.data["loads"] = json!([{"id":"owned-party-load","reference":"FRT-PARTY","customer_id":customer,"carrier_id":carrier,"status":"in_transit","version":1}]);
    first.data["next_cursor"] = serde_json::Value::Null;
    let (origin, worker) = server(vec![
        response(first.identity.clone()),
        response(first.data.clone()),
        response(first.identity),
        response(first.data),
    ]);
    let config = Config::parse(["--demo", "--api", origin.as_str()].map(String::from))
        .expect("owned HTTP fixture")
        .expect("owned HTTP fixture");
    let api = config.client().expect("owned HTTP fixture");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("owned HTTP fixture");
    for attempt in 0..2 {
        if attempt == 1 {
            app.filter_selected_party(Party::Carrier);
        }
        app.needs_fetch = false;
        let result = runtime.block_on(client::fetch(&api, &origin, &app.request()));
        app.finish(app.generation, result);
        assert_eq!(
            app.current().expect("owned HTTP fixture").id,
            "owned-party-load"
        );
    }
    let requests = worker.join().expect("owned HTTP fixture");
    assert_eq!(requests.len(), 4);
    for (index, request) in requests.iter().enumerate() {
        assert!(request.starts_with("GET "));
        assert!(!request.to_lowercase().contains("idempotency-key"));
        if index % 2 == 0 {
            assert!(request.starts_with("GET /v1/access "));
        } else {
            let path = request
                .split_whitespace()
                .nth(1)
                .expect("owned HTTP fixture");
            let url = reqwest::Url::parse(&format!("http://localhost{path}"))
                .expect("owned HTTP fixture");
            assert_eq!(url.path(), "/v1/loads");
            let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
            assert_eq!(pairs["limit"], "50");
            assert_eq!(pairs["q"], "Los Angeles");
            assert_eq!(pairs["status"], "in_transit");
            assert_eq!(pairs["customer_id"], customer);
            assert!(!pairs.contains_key("cursor"));
            if index == 3 {
                assert_eq!(pairs["carrier_id"], carrier);
            } else {
                assert!(!pairs.contains_key("carrier_id"));
            }
        }
    }
    assert!(app.write_ready.is_none() && app.pending.is_none());
}
