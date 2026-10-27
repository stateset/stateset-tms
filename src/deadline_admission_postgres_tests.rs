use super::*;

async fn post(f: &Fixture, key: &str, body: &Value) -> reqwest::Response {
    f.client
        .post(format!("{}/v1/customers", f.origin))
        .bearer_auth("owned-budget")
        .header("idempotency-key", key)
        .json(body)
        .send()
        .await
        .expect("owned HTTP")
}

async fn deadline_and_write(cancel: bool) {
    let table = "freight_tender_deadlines";
    let cold = "UPDATE freight_tender_deadlines SET ready=false WHERE tenant_id='demo'";
    // Admission ownership is the subject here. The finite production budget
    // lets the held query survive the independent receipt/readiness probes;
    // separate budget tests keep their intentionally short timeout settings.
    let f = Fixture::new_with_workspace_budget(Some(1), DatabaseBudget::default()).await;
    let original_body =
        json!({"name":"Original before held index","contact":"original@example.test"});
    let original = post(&f, "index-known-original", &original_body)
        .await
        .json::<Value>()
        .await
        .expect("known original");
    assert_eq!(original["replayed"], false);
    sqlx::query(cold)
        .execute(&f.observer)
        .await
        .expect("only derived cache made cold");
    let before = f.physical().await;
    let key = i64::from_be_bytes(
        uuid::Uuid::new_v4().as_bytes()[..8]
            .try_into()
            .expect("eight UUID bytes for the owned advisory key"),
    );
    // The trigger pauses the real cache builder only AFTER it owns the tenant
    // row. A table lock would stop its preliminary ready check too early.
    sqlx::query(&format!(
        "CREATE FUNCTION hold_owned_index() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({key}); RETURN NEW; END $$;"
    ))
    .execute(&f.observer)
    .await
    .expect("owned trigger function");
    sqlx::query(&format!(
        "CREATE TRIGGER hold_owned_index BEFORE INSERT OR UPDATE ON {table} FOR EACH ROW EXECUTE FUNCTION hold_owned_index()"
    ))
    .execute(&f.observer)
    .await
    .expect("owned derived-index trigger");
    let mut blocker = f.observer.begin().await.expect("owned advisory blocker");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *blocker)
        .await
        .expect("hold actual builder");
    let held_at = tokio::time::Instant::now();
    let store = f.store.clone();
    let mut reader = Some(tokio::spawn(async move {
        store.tender_expiry_may_be_due("demo").await
    }));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND wait_event='advisory')",
        )
        .bind(&f.app)
        .fetch_one(&f.admin)
        .await
        .expect("actual index SQL observed");
        if held {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "index must reach its real insert trigger"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let locked = sqlx::query(
        "SELECT tenant_id FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE NOWAIT",
    )
    .fetch_optional(&f.observer)
    .await
    .expect_err("real builder owns tenant row");
    assert_eq!(
        locked.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("55P03")
    );
    let owned_after_cancel = if cancel {
        let reader = reader.take().expect("owned expiry future");
        reader.abort();
        assert!(reader.await.expect_err("cancelled caller").is_cancelled());
        f.store.write_turns.requests() == 1
    } else {
        true
    };
    let client = f.client.clone();
    let url = format!("{}/v1/customers", f.origin);
    let body = json!({"name":"Applied once after index","contact":"after-index@example.test"});
    let send_body = body.clone();
    let mut writer = tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("owned-budget")
            .header("idempotency-key", "index-next-original")
            .json(&send_body)
            .send()
            .await
    });
    let early = tokio::time::timeout(Duration::from_millis(40), &mut writer).await;
    let queued_before_release = early.is_err();
    let original_before_release = post(&f, "index-known-original", &original_body).await;
    let original_status = original_before_release.status();
    let original_before_release = original_before_release
        .json::<Value>()
        .await
        .expect("replay JSON");
    let ready = f.get("/ready").await.status();
    let unchanged_before_release = f.physical().await == before;
    // Always release and join before asserting the regression, including the
    // old-source control whose write refuses immediately under its 1ms budget.
    let held_for_ms = held_at.elapsed().as_millis();
    blocker.rollback().await.expect("release actual builder");
    let preflight = match reader {
        Some(reader) => reader.await.expect("expiry task"),
        None => Ok(false),
    };
    let preflight_error = preflight.as_ref().err().cloned();
    let preflight_ok = preflight.is_ok();
    let write = match early {
        Ok(result) => result.expect("writer task").expect("writer HTTP"),
        Err(_) => writer.await.expect("writer task").expect("writer HTTP"),
    };
    let write_status = write.status();
    let result = write.json::<Value>().await.expect("write JSON");
    // Save status before owned cleanup so expected-failure controls leave no
    // open external locks, pending tasks, or fixture schema behind.
    let committed = f.physical().await;
    let replay = if write_status == 200 && result["replayed"] == false {
        let replay = post(&f, "index-next-original", &body).await;
        let status = replay.status();
        Some((
            status,
            replay.json::<Value>().await.expect("new original replay"),
        ))
    } else {
        None
    };
    let replay_preserved = f.physical().await == committed;
    println!(
        "deadline-admission-observations {}",
        json!({"cancelled":cancel,"held_for_ms":held_for_ms,"queued_before_release":queued_before_release,"owned_after_cancel":owned_after_cancel,"original_status":original_status.as_u16(),"ready":ready.as_u16(),"unchanged_before_release":unchanged_before_release,"preflight_ok":preflight_ok,"preflight_error":preflight_error,"write_status":write_status.as_u16(),"write_result":result,"replay_preserved":replay_preserved})
    );
    f.clean().await;
    assert!(
        queued_before_release,
        "same-store write must queue before SQL rather than fail behind a derived-index row lock"
    );
    assert_eq!(original_status, 200);
    assert_eq!(
        original_before_release,
        json!({"data":original["data"],"replayed":true})
    );
    assert_eq!(ready, 200);
    assert!(unchanged_before_release);
    assert!(preflight_ok, "actual expiry preflight completes");
    assert!(
        owned_after_cancel,
        "cancelled expiry query retains the local turn until actual SQL rollback"
    );
    assert_eq!(write_status, 200);
    assert_eq!(result["replayed"], false);
    let (replay_status, replay) = replay.expect("only proven commitment is replayed");
    assert_eq!(replay_status, 200);
    assert_eq!(replay, json!({"data":result["data"],"replayed":true}));
    assert!(replay_preserved);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cold_deadline_row_lock_queues_local_write_without_hiding_original_receipts() {
    deadline_and_write(false).await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_cancelled_deadline_keeps_local_turn_until_actual_rollback() {
    deadline_and_write(true).await;
}
