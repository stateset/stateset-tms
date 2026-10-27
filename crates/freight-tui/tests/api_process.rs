//! Explicit opt-in qualification of both real executables on an owned API.
//! FREIGHT_TUI_TEST_DATABASE_URL selects an explicitly disposable PostgreSQL DB.
#![cfg(all(feature = "api-process-tests", unix))]
use serde_json::json;
use std::{fs, time::Duration};
use tokio::process::Command;

async fn fixture_command(
    client: &reqwest::Client,
    origin: &str,
    token: &str,
    path: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let response = client
        .post(format!("{origin}{path}"))
        .bearer_auth(token)
        .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
        .json(&body)
        .send()
        .await
        .expect("owned fixture command");
    let status = response.status();
    let response: serde_json::Value = response.json().await.expect("fixture response");
    assert!(status.is_success(), "Owned fixture {path}: {response}");
    response["data"].clone()
}

#[tokio::test]
async fn actual_tui_reads_all_five_views_and_denies_wrong_credentials_without_secret_output() {
    let binary = env!("CARGO_BIN_EXE_freight-tui");
    let api_binary = std::env::var("FREIGHT_TUI_TEST_API_BINARY").expect(
        "Build the Freight API and set FREIGHT_TUI_TEST_API_BINARY to its absolute executable path",
    );
    let directory = std::env::temp_dir().join(format!(
        "freight-tui-api-process-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir(&directory).expect("owned directory");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("private directory");
    fs::write(directory.join(".env"), "# owned empty fixture\n").expect("dotenv");
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("owned port");
    let address = reservation.local_addr().expect("address");
    drop(reservation);
    let token = "owned-tui-operator-credential-at-least-32-characters";
    let viewer = "owned-tui-viewer-credential-at-least-32-characters";
    let fixture = "owned-tui-fixture-credential-at-least-32-characters";
    let keys = json!({(token):{"tenant_id":"demo","subject":"user:owned-operator","role":"operations"},(viewer):{"tenant_id":"demo","subject":"user:owned-viewer","role":"viewer"},(fixture):{"tenant_id":"demo","subject":"fixture:console","role":"admin"}});
    let log = fs::File::create(directory.join("api.log")).expect("log");
    let mut api_command = Command::new(api_binary);
    api_command.env_clear();
    if let Ok(database) = std::env::var("FREIGHT_TUI_TEST_DATABASE_URL") {
        assert!(
            !database.trim().is_empty(),
            "Disposable TUI database URL is empty"
        );
        api_command.env("DATABASE_URL", database);
        println!("Owned API/TUI backend: PostgreSQL");
    } else {
        println!("Owned API/TUI backend: memory");
    }
    let mut api = api_command
        .current_dir(&directory)
        .env("FREIGHT_DEMO", "true")
        .env("FREIGHT_BIND_ADDR", address.to_string())
        .env("FREIGHT_API_KEYS_JSON", keys.to_string())
        .env("FREIGHT_INTEGRATIONS_JSON", "{}")
        .env("TOKIO_WORKER_THREADS", "2")
        .stdout(log.try_clone().expect("log clone"))
        .stderr(log)
        .kill_on_drop(true)
        .spawn()
        .expect("real API process");
    let origin = format!("http://{address}");
    let client = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                api.try_wait().expect("process").is_none(),
                "API exited: {}",
                fs::read_to_string(directory.join("api.log")).expect("log")
            );
            if client
                .get(format!("{origin}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("API ready");
    // The default demo has empty financial views. Use actual owned API commands
    // so these reads exercise nonempty console publication on both backends.
    let workspace: serde_json::Value = client
        .get(format!("{origin}/v1/workspace"))
        .bearer_auth(fixture)
        .send()
        .await
        .expect("fixture workspace")
        .json()
        .await
        .expect("workspace JSON");
    let invoice = workspace["invoices"]
        .as_array()
        .expect("invoices")
        .iter()
        .find(|v| v["status"] == "matched")
        .expect("matched synthetic invoice");
    let load_id = invoice["load_id"].as_str().expect("invoice load");
    fixture_command(
        &client,
        &origin,
        fixture,
        &format!(
            "/v1/invoices/{}/approve",
            invoice["id"].as_str().expect("invoice ID")
        ),
        json!({}),
    )
    .await;
    fixture_command(
        &client,
        &origin,
        fixture,
        &format!("/v1/loads/{load_id}/settlement"),
        json!({}),
    )
    .await;
    let bill = fixture_command(
        &client,
        &origin,
        fixture,
        "/v1/bills",
        json!({"load_id":load_id,"number":"TUI-BILL","due_date":"2027-12-31"}),
    )
    .await;
    fixture_command(
        &client,
        &origin,
        fixture,
        &format!("/v1/bills/{}/issue", bill["id"].as_str().expect("bill ID")),
        json!({"expected_version":bill["version"]}),
    )
    .await;
    let planned = workspace["loads"]
        .as_array()
        .expect("loads")
        .iter()
        .find(|v| v["status"] == "planned")
        .expect("planned synthetic load");
    let planned_id = planned["id"].as_str().expect("planned ID");
    let carrier = workspace["carriers"]
        .as_array()
        .expect("carriers")
        .iter()
        .find(|v| v["equipment"] == planned["equipment"])
        .expect("compatible carrier");
    fixture_command(
        &client,
        &origin,
        fixture,
        &format!("/v1/loads/{planned_id}/commands"),
        json!({"action":"tender","expected_version":planned["version"],"carrier_id":carrier["id"]}),
    )
    .await;
    println!("Owned API/TUI fixture: issued customer bill, prepared settlement and pending test tender; external integrations disabled");
    for role in [token, viewer] {
        for view in ["loads", "operations", "billing", "payables", "integrations"] {
            let output = tokio::time::timeout(
                Duration::from_secs(15),
                Command::new(binary)
                    .kill_on_drop(true)
                    .env_clear()
                    .current_dir(&directory)
                    .env("FREIGHT_TUI_TOKEN", role)
                    .args(["--api", &origin, "--snapshot", "--view", view, "--no-color"])
                    .output(),
            )
            .await
            .expect("bounded TUI process")
            .expect("real TUI");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{view}: {stdout} {stderr}");
            assert!(stdout.contains("API SNAPSHOT · demo"));
            assert!(!stdout.contains('\u{1b}'));
            assert!(!stdout.contains(token));
            assert!(!stderr.contains(token));
            assert!(!stdout.contains(viewer));
            assert!(!stdout.contains(fixture));
            match view {
                "billing" => assert!(stdout.contains("TUI-BILL"), "nonempty billing: {stdout}"),
                "payables" => assert!(stdout.contains(load_id), "nonempty payables: {stdout}"),
                "integrations" => assert!(stdout.contains(planned_id), "nonempty queues: {stdout}"),
                _ => {}
            }
        }
    }
    for role in [token, viewer] {
        for (scope, title) in [
            ("actionable", "Actionable"),
            ("active", "All active"),
            ("mine", "Assigned to me"),
            ("unowned", "Unowned"),
            ("deferred", "Deferred"),
            ("resolved", "Source inactive"),
            ("all", "All issues"),
        ] {
            let output = tokio::time::timeout(
                Duration::from_secs(15),
                Command::new(binary)
                    .kill_on_drop(true)
                    .env_clear()
                    .current_dir(&directory)
                    .env("FREIGHT_TUI_TOKEN", role)
                    .args([
                        "--api",
                        &origin,
                        "--snapshot",
                        "--view",
                        "operations",
                        "--operations-scope",
                        scope,
                        "--no-color",
                    ])
                    .output(),
            )
            .await
            .expect("bounded scoped read")
            .expect("scoped TUI process");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{scope}: {stdout} {stderr}");
            assert!(stdout.contains(&format!("Operations: {title}")));
            assert!(!stdout.contains(token) && !stdout.contains(viewer));
            assert!(!stderr.contains(token) && !stderr.contains(viewer));
            assert!(!stdout.contains('\u{1b}'));
        }
    }
    // A filtered read slower than the former 500ms sleep must finish before
    // the driver opens an action. Both cases still execute the real TUI/API.
    for delay_ms in [0, 1250] {
        let work_directory = directory.join(format!("work-read-delay-{delay_ms}"));
        fs::create_dir(&work_directory).expect("owned work diagnostics");
        fs::set_permissions(&work_directory, fs::Permissions::from_mode(0o700))
            .expect("private work diagnostics");
        let work = tokio::time::timeout(
            Duration::from_secs(90),
            Command::new("python3")
                .kill_on_drop(true)
                .env_clear()
                .env("FREIGHT_TUI_TOKEN", token)
                .env("FREIGHT_TUI_TEST_READ_DELAY_MS", delay_ms.to_string())
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scripts/check-tui-work-live.py"
                ))
                .args([
                    binary,
                    &origin,
                    work_directory.to_str().expect("owned path"),
                ])
                .output(),
        )
        .await
        .expect("bounded live work acceptance")
        .expect("owned PTY helper");
        assert!(
            work.status.success(),
            "Live TUI work failed: {} {} (diagnostics {})",
            String::from_utf8_lossy(&work.stdout),
            String::from_utf8_lossy(&work.stderr),
            work_directory.display()
        );
        println!("{}", String::from_utf8_lossy(&work.stdout));
    }
    let denied = Command::new(binary)
        .kill_on_drop(true)
        .env_clear()
        .current_dir(&directory)
        .env("FREIGHT_TUI_TOKEN", "wrong-owned-credential")
        .args(["--api", &origin, "--snapshot"])
        .output()
        .await
        .expect("denied snapshot");
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("Access denied"));
    api.kill().await.expect("stop owned API");
    api.wait().await.expect("reap API");
    println!(
        "Owned API/TUI process diagnostics retained at {}",
        directory.display()
    );
}
