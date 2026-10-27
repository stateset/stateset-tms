#[path = "support/canonical.rs"]
mod canonical;
use axum::{body::Body, http::Request, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use stateset_freight::{
    api::{self, App},
    auth::{Credential, Principal, Role},
    domain::{self, Workspace},
    integrations::Integrations,
    movement::source_sha256,
    store::Store,
};
use std::sync::Arc;
use tower::ServiceExt;
async fn read(s: &Store) -> Workspace {
    s.read("demo").await.expect("read")
}
async fn op(s: &Store, operation: &str, id: &str, body: Value) -> Value {
    s.mutate_authorized(
        ("demo", "ops:movement"),
        &domain::id("key"),
        operation,
        id,
        body,
    )
    .await
    .expect(operation)["data"]
        .clone()
}
async fn version(s: &Store, id: &str) -> u64 {
    read(s)
        .await
        .loads
        .iter()
        .find(|l| l.id == id)
        .expect("load")
        .version
}
fn router(s: Store) -> Router {
    let keys = [
        ("ops", Role::Operations, None),
        ("finance", Role::Finance, None),
        ("viewer", Role::Viewer, None),
        ("carrier", Role::Carrier, Some("ca4")),
        ("other", Role::Carrier, Some("ca1")),
        ("customer", Role::Customer, Some("c1")),
    ]
    .into_iter()
    .map(|(key, role, cp)| {
        (
            key.into(),
            Credential::Scoped(Principal {
                tenant_id: "demo".into(),
                subject: format!("actor:{key}"),
                role,
                counterparty_id: cp.map(str::to_owned),
            }),
        )
    })
    .collect();
    api::router(App {
        pdf_renderer: Default::default(),
        store: s,
        identity: Arc::new(None),
        keys: Arc::new(keys),
        demo: true,
        integrations: Arc::new(Integrations::default()),
    })
}
async fn call(r: &Router, who: &str, path: &str, body: Value, key: &str) -> (u16, Value) {
    let response = r
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/{path}"))
                .header("authorization", format!("Bearer {who}"))
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}
async fn report(s: &Store, r: &Router, id: &str, offer: &Value, kind: &str, minutes: i64) -> Value {
    let(status,result)=call(r,"carrier",&format!("portal/offers/{}/tracking",offer["id"].as_str().expect("offer")),json!({"expected_load_version":version(s,id).await,"kind":kind,"observed_at":Utc::now()-Duration::minutes(minutes),"location":format!("Actual {kind} location"),"reference":format!("ORIGINAL-CARRIER-{kind}"),"note":"Original native driver evidence"}),&domain::id("key")).await;
    assert_eq!(status, 200, "{result}");
    result["data"].clone()
}
async fn setup(extra: bool) -> (Store, Router, String, Value, String) {
    let s = Store::new(None).await.expect("store");
    s.seed_demo().await.expect("seed");
    let day = (Utc::now() + Duration::days(1)).date_naive();
    let stop = |facility, city, state, postal| json!({"facility":facility,"city":city,"state":state,"date":day,"address_lines":["100 Synthetic Route Street"],"postal_code":postal,"country":"US"});
    let l=op(&s,"load.create","",json!({"reference":domain::id("MOVE")[..25],"customer_id":"c1","mode":"FTL","equipment":"Dry van","commodity":"Movement evidence freight","weight_lbs":1000,"miles":173,"buy_cents":125001,"sell_cents":150001,"origin":stop("Portland origin","Portland","OR","97201"),"destination":stop("Seattle receiver","Seattle","WA","98101"),"intermediate_stops":if extra{vec![json!({"id":"stop_tacoma","kind":"delivery","stop":stop("Tacoma receiver","Tacoma","WA","98402")})]}else{vec![]}})).await;
    let id = l["id"].as_str().expect("load").to_string();
    op(
        &s,
        "load.command",
        &id,
        json!({"action":"tender","expected_version":1,"carrier_id":"ca4"}),
    )
    .await;
    let rate=op(&s,"rate.issue",&id,json!({"expected_load_version":version(&s,&id).await,"terms":"Actual inclusive original carrier agreement","reason":"Initial reviewed confirmation"})).await;
    let p = op(
        &s,
        "portal.publish",
        &id,
        json!({"expected_load_version":version(&s,&id).await,"rate_confirmation_id":rate["id"]}),
    )
    .await;
    let r = router(s.clone());
    let(status,result)=call(&r,"carrier",&format!("portal/offers/{}/response",p["id"].as_str().expect("offer")),json!({"expected_load_version":version(&s,&id).await,"decision":"accept","rate_confirmation_id":p["rate_confirmation_id"],"rate_sha256":p["rate_sha256"],"reference":"Actual native acceptance","note":"Carrier reviewed exact route and price"}),"accept").await;
    assert_eq!(status, 200, "{result}");
    let source = report(&s, &r, &id, &p, "pickup", 20).await;
    let sid = source["id"].as_str().expect("source").into();
    (s, r, id, p, sid)
}
fn correction(w: &Workspace, id: &str, source: &str, minutes: i64) -> Value {
    let source = w
        .carrier_submissions
        .iter()
        .find(|s| s.id == source)
        .expect("source");
    json!({"expected_load_version":w.loads.iter().find(|l|l.id==id).expect("load").version,"source_sha256":source_sha256(source),"expected_previous_id":w.movement_corrections.iter().rev().find(|c|c.source_id==source.id).map(|c|&c.id),"observed_at":Utc::now()-Duration::minutes(minutes),"contact":"PRIVATE actual carrier supervisor","reference":"PRIVATE external corrected movement reference","note":"PRIVATE reviewed original driver and facility evidence for actual time"})
}
fn physical(w: &Workspace, id: &str) -> Value {
    let l = w.loads.iter().find(|l| l.id == id).expect("load");
    json!({"status":l.status,"location":l.location,"watermark":l.last_tracking_at,"tender":l.tender_id,"carrier":l.carrier_id,"buy":l.buy_cents,"sell":l.sell_cents,"source":l.sync_order_id,"driver":l.driver_id,"equipment":l.equipment_id})
}
async fn fails(s: &Store, key: &str, id: &str, body: Value, contains: &str) {
    let before = json!(read(s).await);
    let error = s
        .mutate_authorized(
            ("demo", "ops:movement"),
            key,
            "movement.time.correct",
            id,
            body,
        )
        .await
        .expect_err("must reject");
    assert!(error.contains(contains), "{error}");
    assert_eq!(json!(read(s).await), before);
}
async fn delivered() -> (Store, Router, String, Value, String, String) {
    let (s, r, id, p, source) = setup(false).await;
    let d = report(&s, &r, &id, &p, "delivery", 5).await;
    op(&s,"load.command",&id,json!({"action":"attach_pod","expected_version":version(&s,&id).await,"pod_reference":"Actual original consolidated signed receiver evidence"})).await;
    s.mutate_authorized(("demo","finance:reviewer"),"review-pod","pod.review",&id,json!({"expected_version":version(&s,&id).await,"decision":"accepted","note":"Original external receiver evidence inspected"})).await.expect("review");
    (
        s,
        r,
        id,
        p,
        source,
        d["id"].as_str().expect("delivery").into(),
    )
}

async fn prepare_movement_settlement(s: &Store, load_id: &str) -> Value {
    let invoice = op(s, "invoice.create", "", json!({"load_id":load_id,"number":domain::id("PAYOUT"),"amount_cents":125001,"currency":"USD"})).await;
    op(
        s,
        "invoice.approve",
        invoice["id"].as_str().expect("invoice"),
        json!({}),
    )
    .await;
    op(s, "settlement.create", load_id, json!({})).await
}

fn payout_body(settlement: &Value, cleared_on: chrono::NaiveDate) -> Value {
    json!({"direction":"payout","currency":"USD","account":"Operating USD","external_reference":"early-actual-bank-wire","amount_cents":125001,"cleared_on":cleared_on,"note":"Actual cleared bank statement reconciled to completed transport","allocations":[{"target_id":settlement["id"],"expected_version":settlement["version"],"amount_cents":125001}]})
}

#[tokio::test]
async fn payout_uses_verified_actual_pickup_before_plan_and_replays_without_duplicate_posting() {
    let (s, r, id, _, pickup, _) = delivered().await;
    let settlement = prepare_movement_settlement(&s, &id).await;
    let before = read(&s).await;
    let load = before.loads.iter().find(|l| l.id == id).expect("load");
    let source = before
        .carrier_submissions
        .iter()
        .find(|v| v.id == pickup)
        .expect("pickup");
    let day = before
        .effective_movement_time(source)
        .expect("verified")
        .expect("actual")
        .date_naive();
    assert!(day < load.origin.date, "Actual transport precedes the plan");
    let body = payout_body(&settlement, day);
    let (status, first) = call(&r, "finance", "cash", body.clone(), "early-actual-payout").await;
    assert_eq!(
        status, 200,
        "Verified early transport must reconcile: {first}"
    );
    let paid = read(&s).await;
    assert_eq!(paid.cash_entries.last().expect("cash").cleared_on, day);
    assert_eq!(
        paid.cash_entries.last().expect("cash").recorded_by,
        "actor:finance"
    );
    assert_eq!(
        paid.settlement_balance(settlement["id"].as_str().expect("id"))
            .expect("balance"),
        0
    );
    assert_eq!(
        json!(paid.carrier_submissions),
        json!(before.carrier_submissions)
    );
    assert_eq!(
        json!(paid.loads),
        json!(before.loads),
        "Reconciliation does not rewrite the plan or actual movement"
    );
    let original = json!(paid);
    let (status, replay) = call(&r, "finance", "cash", body, "early-actual-payout").await;
    assert_eq!(status, 200, "{replay}");
    assert_eq!(replay["data"], first["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        json!(read(&s).await),
        original,
        "No duplicate posting, receipt or audit"
    );
}

#[tokio::test]
async fn payout_respects_actual_pickup_after_plan_and_ignores_unrelated_or_stale_reports() {
    let (s, _, id, _, pickup, _) = delivered().await;
    let settlement = prepare_movement_settlement(&s, &id).await;
    let original = read(&s).await;
    let day = original
        .carrier_submissions
        .iter()
        .find(|v| v.id == pickup)
        .expect("pickup")
        .observed_at
        .expect("actual")
        .date_naive();
    for case in [
        "actual-after-plan",
        "retired-tender",
        "different-carrier",
        "stale-ignored",
        "location-only",
        "missing-time",
        "future-time",
        "chronology-conflict",
    ] {
        let mut workspace = original.clone();
        let load = workspace
            .loads
            .iter_mut()
            .find(|l| l.id == id)
            .expect("load");
        if case == "actual-after-plan" {
            load.origin.date = day - Duration::days(2);
        }
        let source = workspace
            .carrier_submissions
            .iter_mut()
            .find(|v| v.id == pickup)
            .expect("pickup");
        match case {
            "retired-tender" => source.tender_id = "retired-assignment".into(),
            "different-carrier" => source.carrier_id = "ca1".into(),
            "stale-ignored" => source.outcome = "stale_ignored".into(),
            "location-only" => source.kind = "location".into(),
            "missing-time" => source.observed_at = None,
            "future-time" => source.observed_at = Some(Utc::now() + Duration::days(1)),
            "chronology-conflict" => source.observed_at = Some(Utc::now() - Duration::minutes(1)),
            _ => {}
        }
        let input = payout_body(
            &settlement,
            if case == "actual-after-plan" {
                day - Duration::days(1)
            } else {
                day
            },
        );
        let before = json!(workspace);
        workspace
            .apply_finance("cash.record", "", input, "finance:checker")
            .expect_err(case);
        assert_eq!(
            json!(workspace),
            before,
            "Rejected {case} leaves no cash, posting or version changes"
        );
    }
}

#[tokio::test]
async fn payout_uses_reviewed_pickup_day_and_rejects_corrupt_correction_proof() {
    let (s, _, id, _, pickup, _) = delivered().await;
    let original = read(&s).await;
    let corrected = read(&s)
        .await
        .carrier_submissions
        .iter()
        .find(|source| source.id == pickup)
        .expect("pickup")
        .observed_at
        .expect("actual time")
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .expect("noon")
        .and_utc()
        - Duration::days(2);
    let mut body = correction(&original, &id, &pickup, 15);
    body["observed_at"] = json!(corrected);
    op(&s, "movement.time.correct", &pickup, body).await;
    let settlement = prepare_movement_settlement(&s, &id).await;
    let reviewed = read(&s).await;
    let body = payout_body(&settlement, corrected.date_naive());
    let mut corrupt = reviewed.clone();
    corrupt
        .movement_corrections
        .last_mut()
        .expect("correction")
        .note
        .push_str(" altered");
    let before = json!(corrupt);
    assert!(corrupt
        .apply_finance("cash.record", "", body.clone(), "finance:checker")
        .is_err());
    assert_eq!(json!(corrupt), before);
    let mut first = reviewed.clone();
    first
        .apply_finance("cash.record", "", body.clone(), "finance:checker")
        .expect("Effective corrected date permits actual early cleared wire");
    assert_eq!(
        json!(first.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(
        json!(first.movement_corrections),
        json!(reviewed.movement_corrections)
    );
    // A later repeated pickup report does not move the original custody date.
    let mut repeated = reviewed.clone();
    let mut source = repeated
        .carrier_submissions
        .iter()
        .find(|v| v.id == pickup)
        .expect("source")
        .clone();
    source.id = domain::id("repeat-pickup");
    source.observed_at = original
        .carrier_submissions
        .iter()
        .find(|v| v.id == pickup)
        .expect("original pickup")
        .observed_at;
    repeated.carrier_submissions.push(source);
    repeated
        .apply_finance("cash.record", "", body, "finance:checker")
        .expect("First effective pickup remains the lower bound");
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_actual_pickup_payout_serializes_retains_review_and_replays_after_evidence_corruption(
) {
    let (s, _, id, _, pickup, _) = delivered().await;
    let corrected = read(&s)
        .await
        .carrier_submissions
        .iter()
        .find(|source| source.id == pickup)
        .expect("pickup")
        .observed_at
        .expect("actual time")
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .expect("noon")
        .and_utc()
        - Duration::days(2);
    let mut body = correction(&read(&s).await, &id, &pickup, 15);
    body["observed_at"] = json!(corrected);
    op(&s, "movement.time.correct", &pickup, body).await;
    let settlement = prepare_movement_settlement(&s, &id).await;
    let original = read(&s).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("owned PostgreSQL URL");
    let a = Store::new(Some(&url)).await.expect("first writer");
    let b = Store::new(Some(&url)).await.expect("second writer");
    let pool = sqlx::PgPool::connect(&url).await.expect("fixture pool");
    let tenant = domain::id("actual-payout-pg");
    canonical::put_workspace(&pool, &tenant, &original)
        .await
        .expect("owned original");
    let mut body = payout_body(&settlement, corrected.date_naive());
    body["amount_cents"] = json!(50000);
    body["allocations"][0]["amount_cents"] = json!(50000);
    let (first, second) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "finance:first"),
            "actual-payout-first",
            "cash.record",
            "",
            body.clone()
        ),
        b.mutate_authorized(
            (&tenant, "finance:second"),
            "actual-payout-second",
            "cash.record",
            "",
            body.clone()
        )
    );
    assert_ne!(
        first.is_ok(),
        second.is_ok(),
        "Versioned allocation commits only once"
    );
    let (key, actor, successful) = if let Ok(value) = first {
        ("actual-payout-first", "finance:first", value)
    } else {
        (
            "actual-payout-second",
            "finance:second",
            second.expect("winning allocation"),
        )
    };
    let reconnect = Store::new(Some(&url)).await.expect("reconnected store");
    let retained = reconnect.read(&tenant).await.expect("durable workspace");
    assert_eq!(retained.cash_entries.len(), original.cash_entries.len() + 1);
    assert_eq!(
        retained.cash_entries.last().expect("cash").recorded_by,
        actor
    );
    assert_eq!(
        retained
            .settlement_balance(settlement["id"].as_str().expect("id"))
            .expect("balance"),
        75001
    );
    assert_eq!(
        json!(retained.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(
        json!(retained.movement_corrections),
        json!(original.movement_corrections)
    );
    let note = &original.movement_corrections[0].note;
    canonical::set(
        &pool,
        &tenant,
        &["movement_corrections", "0", "note"],
        json!("Corrupted retained review"),
    )
    .await
    .expect("owned corruption fixture");
    let corrupted = json!(reconnect.read(&tenant).await.expect("read damaged proof"));
    let replay = reconnect
        .mutate_authorized((&tenant, actor), key, "cash.record", "", body.clone())
        .await
        .expect("Original committed receipt remains authoritative");
    assert_eq!(replay["data"], successful["data"]);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        json!(reconnect.read(&tenant).await.expect("unchanged replay")),
        corrupted
    );
    body["external_reference"] = json!("remaining-actual-payout");
    body["amount_cents"] = json!(75001);
    body["allocations"][0]["amount_cents"] = json!(75001);
    body["allocations"][0]["expected_version"] = json!(2);
    reconnect
        .mutate_authorized(
            (&tenant, actor),
            "remaining-after-review-repair",
            "cash.record",
            "",
            body.clone(),
        )
        .await
        .expect_err("New payout refuses corrupt movement review");
    assert_eq!(
        json!(reconnect.read(&tenant).await.expect("atomic refusal")),
        corrupted
    );
    canonical::set(
        &pool,
        &tenant,
        &["movement_corrections", "0", "note"],
        json!(note),
    )
    .await
    .expect("restore exact reviewed proof");
    reconnect
        .mutate_authorized(
            (&tenant, actor),
            "remaining-after-review-repair",
            "cash.record",
            "",
            body,
        )
        .await
        .expect("Failed key is reusable after exact proof restoration");
    assert_eq!(
        reconnect
            .read(&tenant)
            .await
            .expect("final state")
            .settlement_balance(settlement["id"].as_str().expect("id"))
            .expect("balance"),
        0
    );
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("remove owned tenant");
    pool.close().await;
}

#[tokio::test]
async fn exact_native_source_correction_retains_carrier_provenance_and_uses_effective_time_without_rewinding_state(
) {
    let (s, r, id, p, source) = setup(true).await;
    let original = read(&s).await;
    let before = physical(&original, &id);
    let (_, html, _) = stateset_freight::rate_confirmations::statement(
        &original,
        p["rate_confirmation_id"].as_str().expect("rate"),
    )
    .expect("rate");
    let body = correction(&original, &id, &source, 15);
    let (status, first) = call(
        &r,
        "ops",
        &format!("carrier-submissions/{source}/time-corrections"),
        body.clone(),
        "original-time-correction",
    )
    .await;
    assert_eq!(status, 200, "{first}");
    let current = read(&s).await;
    assert_eq!(physical(&current, &id), before);
    assert_eq!(
        json!(current.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(first["data"]["recorded_by"], "actor:ops");
    assert_eq!(first["data"]["source_sha256"], body["source_sha256"]);
    assert_eq!(json!(current.journal), json!(original.journal));
    assert_eq!(json!(current.invoices), json!(original.invoices));
    assert_eq!(json!(current.cash_entries), json!(original.cash_entries));
    let service = |v, minutes| json!({"expected_load_version":v,"stop_id":"stop_tacoma","observed_at":Utc::now()-Duration::minutes(minutes),"contact":"Actual receiver","reference":"Actual service receipt","note":"Actual intermediate unloading evidence"});
    let before_failure = json!(current);
    assert!(s
        .mutate(
            "demo",
            "before-corrected-pickup",
            "itinerary.service.record",
            &id,
            service(version(&s, &id).await, 17)
        )
        .await
        .expect_err("effective boundary")
        .contains("known carrier pickup"));
    assert_eq!(json!(read(&s).await), before_failure);
    let served = op(
        &s,
        "itinerary.service.record",
        &id,
        service(version(&s, &id).await, 10),
    )
    .await;
    let second = op(
        &s,
        "movement.time.correct",
        &source,
        correction(&read(&s).await, &id, &source, 18),
    )
    .await;
    assert_eq!(second["supersedes_id"], first["data"]["id"]);
    op(
        &s,
        "itinerary.service.correct",
        served["id"].as_str().expect("service"),
        service(version(&s, &id).await, 17),
    )
    .await;
    let replay = call(
        &r,
        "ops",
        &format!("carrier-submissions/{source}/time-corrections"),
        body,
        "original-time-correction",
    )
    .await;
    assert_eq!(replay.0, 200);
    assert_eq!(replay.1["replayed"], true);
    assert_eq!(replay.1["data"], first["data"]);
    let current = read(&s).await;
    assert_eq!(current.movement_corrections.len(), 2);
    assert_eq!(json!(current.movement_corrections[0]), first["data"]);
    assert_eq!(physical(&current, &id), before);
    assert_eq!(
        json!(current.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(
        stateset_freight::rate_confirmations::statement(
            &current,
            p["rate_confirmation_id"].as_str().expect("rate")
        )
        .expect("same rate")
        .1,
        html
    );
    let principal = Principal {
        tenant_id: "demo".into(),
        subject: "carrier:own".into(),
        role: Role::Carrier,
        counterparty_id: Some("ca4".into()),
    };
    let public = current.partner_workspace(&principal).expect("public");
    let offer = public["offers"]
        .as_array()
        .expect("offers")
        .iter()
        .find(|o| o["id"] == p["id"])
        .expect("offer");
    assert!(!offer["reporting"]["execution_timeline"]
        .to_string()
        .contains("PRIVATE"));
    let fact = &offer["reporting"]["execution_timeline"]["stops"][0]["facts"][0];
    assert_eq!(fact["corrected"], true);
    assert_eq!(
        fact["original_observed_at"],
        json!(original.carrier_submissions[0].observed_at)
    );
    assert_eq!(fact["observed_at"], second["observed_at"]);
}

#[tokio::test]
async fn correction_authorization_source_review_and_full_chronology_fail_atomically() {
    let (s, r, id, p, source) = setup(false).await;
    let current = read(&s).await;
    let body = correction(&current, &id, &source, 19);
    let path = format!("carrier-submissions/{source}/time-corrections");
    for who in ["carrier", "other", "customer", "viewer", "finance"] {
        let before = json!(read(&s).await);
        let (status, _) = call(&r, who, &path, body.clone(), &domain::id("key")).await;
        assert_eq!(status, 403, "{who}");
        assert_eq!(json!(read(&s).await), before);
    }
    let mut forged = body.clone();
    forged["recorded_by"] = json!("carrier:forged");
    assert_eq!(call(&r, "ops", &path, forged, "forged-actor").await.0, 422);
    let mut changed = body.clone();
    changed["source_sha256"] = json!("0".repeat(64));
    fails(&s, "changed-source", &source, changed, "evidence changed").await;
    let mut future = body;
    future["observed_at"] = json!(Utc::now() + Duration::hours(1));
    fails(&s, "future", &source, future, "future").await;
    let gate=op(&s,"stop.event.record",&id,json!({"expected_load_version":version(&s,&id).await,"stop":"pickup","kind":"arrival","local_time":(Utc::now()-Duration::minutes(21)).format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":"Actual origin gate evidence","note":"Original confirmed actual arrival"})).await;
    assert_eq!(gate["kind"], "arrival");
    fails(
        &s,
        "before-arrival",
        &source,
        correction(&read(&s).await, &id, &source, 25),
        "chronology",
    )
    .await;
    let first = op(
        &s,
        "movement.time.correct",
        &source,
        correction(&read(&s).await, &id, &source, 19),
    )
    .await;
    let mut stale = correction(&read(&s).await, &id, &source, 18);
    stale["expected_previous_id"] = Value::Null;
    fails(
        &s,
        "wrong-predecessor",
        &source,
        stale,
        "correction changed",
    )
    .await;
    let location = report(&s, &r, &id, &p, "location", 10).await;
    fails(
        &s,
        "location-not-milestone",
        location["id"].as_str().expect("location"),
        correction(
            &read(&s).await,
            &id,
            location["id"].as_str().expect("id"),
            9,
        ),
        "Only applied native",
    )
    .await;
    let old = report(&s, &r, &id, &p, "pickup", 15).await;
    assert_eq!(old["outcome"], "stale_ignored");
    fails(
        &s,
        "stale-not-applied",
        old["id"].as_str().expect("id"),
        correction(&read(&s).await, &id, old["id"].as_str().expect("id"), 16),
        "Only applied native",
    )
    .await;
    assert_eq!(read(&s).await.movement_corrections.len(), 1);
    assert!(first["evidence_sha256"].as_str().expect("hash").len() == 64);
}

#[tokio::test]
async fn reviewed_native_delivery_time_repairs_legacy_conflict_and_corrupted_proof_fails_closed() {
    let (s, _, id, _, _, delivery) = delivered().await;
    op(&s,"stop.event.record",&id,json!({"expected_load_version":version(&s,&id).await,"stop":"delivery","kind":"arrival","local_time":(Utc::now()-Duration::minutes(6)).format("%Y-%m-%dT%H:%M").to_string(),"timezone":"UTC","reference":"Actual destination gate receipt","note":"Original receiver arrival evidence"})).await;
    let invoice = op(
        &s,
        "invoice.create",
        "",
        json!({"load_id":id,"number":"MOVEMENT-125001","amount_cents":125001,"currency":"USD"}),
    )
    .await;
    let mut legacy = read(&s).await;
    let source = legacy
        .carrier_submissions
        .iter_mut()
        .find(|s| s.id == delivery)
        .expect("source");
    // A pre-chronology deployment accepted this native delivery time before
    // the actual arrival. Preserve that original report during explicit repair.
    source.observed_at = Some(Utc::now() - Duration::minutes(8));
    let source = json!(source);
    let load = legacy.loads.iter().find(|l| l.id == id).expect("load");
    let physical_before = physical(&legacy, &id);
    let pod_before = json!({"review":load.pod_review,"reference":load.pod_reference,"document":load.pod_document_id});
    assert!(!legacy.execution_consistent(load));
    assert!(!legacy.delivery_evidence_ready(load));
    assert!(legacy
        .apply(
            "invoice.approve",
            invoice["id"].as_str().expect("invoice"),
            json!({})
        )
        .is_err());
    let body = correction(&legacy, &id, &delivery, 5);
    legacy
        .apply_movement(&delivery, body, "ops:reviewer")
        .expect("explicit source-bound repair");
    let body = correction(&legacy, &id, &delivery, 4);
    legacy
        .apply_movement(&delivery, body, "ops:second-reviewer")
        .expect("second retained correction");
    let load = legacy.loads.iter().find(|l| l.id == id).expect("load");
    assert!(legacy.execution_consistent(load));
    assert!(legacy.delivery_evidence_ready(load));
    assert_eq!(physical(&legacy, &id), physical_before);
    assert_eq!(
        json!({"review":load.pod_review,"reference":load.pod_reference,"document":load.pod_document_id}),
        pod_before
    );
    assert_eq!(
        json!(legacy
            .carrier_submissions
            .iter()
            .find(|s| s.id == delivery)
            .expect("original")),
        source
    );
    for variant in 0..6 {
        let mut bad = legacy.clone();
        match variant {
            0 => bad.movement_corrections[0].note.push_str(" CORRUPTION"),
            1 => bad
                .carrier_submissions
                .iter_mut()
                .find(|s| s.id == delivery)
                .expect("source")
                .note
                .push_str(" CORRUPTION"),
            2 => bad.movement_corrections[0]
                .tender_id
                .push_str(" CORRUPTION"),
            3 => bad.movement_corrections[1].supersedes_id = None,
            4 => bad.movement_corrections[1]
                .source_id
                .push_str(" CORRUPTION"),
            _ => bad.movement_corrections.swap(0, 1),
        }
        let load = bad.loads.iter().find(|l| l.id == id).expect("load");
        assert!(!bad.execution_consistent(load), "variant {variant}");
        assert!(!bad.delivery_evidence_ready(load));
        let timeline = bad.execution_timeline(load, false);
        assert_eq!(timeline["evidence_valid"], false);
        assert!(!timeline.to_string().contains("PRIVATE"));
        assert!(bad
            .movement_sources(load)
            .as_array()
            .expect("sources")
            .iter()
            .all(|s| s["can_correct"] == false));
        let body = correction(&bad, &id, &delivery, 3);
        let before = json!(bad);
        assert_eq!(
            bad.apply_movement(&delivery, body, "ops:reviewer")
                .expect_err("corrupt proof"),
            "Internal storage error"
        );
        assert_eq!(json!(bad), before);
        assert!(bad
            .apply(
                "invoice.approve",
                invoice["id"].as_str().expect("invoice"),
                json!({})
            )
            .is_err());
        assert_eq!(json!(bad), before);
    }
    legacy
        .apply(
            "invoice.approve",
            invoice["id"].as_str().expect("invoice"),
            json!({}),
        )
        .expect("valid evidence permits finance");
    assert_eq!(
        legacy.invoices.last().expect("invoice").amount_cents,
        125001
    );
    assert_eq!(legacy.invoices.last().expect("invoice").status, "approved");
}

#[tokio::test]
async fn native_movement_corrections_stop_at_financial_locks_and_retired_assignments() {
    for lock in [
        "approved-invoice",
        "prepared-settlement",
        "draft-customer-bill",
    ] {
        let (s, _, id, _, _, delivery) = delivered().await;
        if lock == "draft-customer-bill" {
            op(&s,"bill.create","",json!({"load_id":id,"number":"MOVEMENT-CUSTOMER","due_date":Utc::now().date_naive()})).await;
        } else {
            let invoice=op(&s,"invoice.create","",json!({"load_id":id,"number":"MOVEMENT-CARRIER","amount_cents":125001,"currency":"USD"})).await;
            op(
                &s,
                "invoice.approve",
                invoice["id"].as_str().expect("invoice"),
                json!({}),
            )
            .await;
            if lock == "prepared-settlement" {
                op(&s, "settlement.create", &id, json!({})).await;
            }
        }
        let current = read(&s).await;
        let load = current.loads.iter().find(|l| l.id == id).expect("load");
        assert!(current
            .movement_sources(load)
            .as_array()
            .expect("sources")
            .iter()
            .all(|s| s["locked"] == true && s["can_correct"] == false));
        fails(
            &s,
            "locked-evidence",
            &delivery,
            correction(&current, &id, &delivery, 4),
            "locked by financial records",
        )
        .await;
    }
    let (s, _, id, _, pickup) = setup(false).await;
    let mut retired = read(&s).await;
    let load = retired.loads.iter_mut().find(|l| l.id == id).expect("load");
    load.tender_id = Some("replacement-tender".into());
    load.carrier_id = Some("ca1".into());
    let load = load.clone();
    assert_eq!(retired.movement_sources(&load), json!([]));
    let body = correction(&retired, &id, &pickup, 19);
    let before = json!(retired);
    assert!(retired
        .apply_movement(&pickup, body, "ops:reviewer")
        .expect_err("historical assignment")
        .contains("current physical carrier assignment"));
    assert_eq!(json!(retired), before);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_native_time_corrections_serialize_and_retain_originals_with_atomic_integrity_rollback(
) {
    let (s, _, id, offer, source) = setup(false).await;
    let mut original = read(&s).await;
    original
        .loads
        .iter_mut()
        .find(|l| l.id == id)
        .expect("load")
        .sync_order_id = Some(uuid::Uuid::new_v4().to_string());
    let url = std::env::var("TEST_DATABASE_URL").expect("owned database");
    let a = Store::new(Some(&url)).await.expect("first");
    let b = Store::new(Some(&url)).await.expect("independent second");
    let pool = sqlx::PgPool::connect(&url).await.expect("pool");
    let tenant = domain::id("movement-pg");
    let mut seed = json!(original);
    seed["_freight_format"] = json!(stateset_freight::store::STORAGE_FORMAT);
    canonical::put_workspace(&pool, &tenant, &seed)
        .await
        .expect("owned tenant");
    let body = correction(&original, &id, &source, 19);
    let mut competing = body.clone();
    competing["observed_at"] = json!(Utc::now() - Duration::minutes(18));
    let (x, y) = tokio::join!(
        a.mutate_authorized(
            (&tenant, "ops:first"),
            "competing-a",
            "movement.time.correct",
            &source,
            body
        ),
        b.mutate_authorized(
            (&tenant, "ops:second"),
            "competing-b",
            "movement.time.correct",
            &source,
            competing
        )
    );
    assert_ne!(
        x.is_ok(),
        y.is_ok(),
        "one exact-version reviewed correction commits"
    );
    let current = b.read(&tenant).await.expect("read");
    assert_eq!(current.movement_corrections.len(), 1);
    assert_eq!(
        json!(current.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(physical(&current, &id), physical(&original, &id));
    assert_eq!(current.outbox.len(), original.outbox.len() + 1);
    assert_eq!(current.receipts.len(), original.receipts.len() + 1);
    assert_eq!(current.events.len(), original.events.len() + 1);
    let intact: Value = sqlx::query_scalar(
        "SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id=$1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("exact original stored bytes");
    let mut corrupt = intact.clone();
    corrupt["movement_corrections"][0]["note"] = json!("CORRUPTED retained movement evidence");
    canonical::put_workspace(&pool, &tenant, &corrupt)
        .await
        .expect("owned corruption fixture");
    let repair = correction(&current, &id, &source, 17);
    assert_eq!(
        b.mutate_authorized(
            (&tenant, "ops:reviewer"),
            "retry-after-restoration",
            "movement.time.correct",
            &source,
            repair.clone()
        )
        .await
        .expect_err("integrity failure"),
        "Internal storage error"
    );
    let unchanged: Value = sqlx::query_scalar(
        "SELECT freight_workspace_originals(tenant_id) FROM freight_workspaces WHERE tenant_id=$1",
    )
    .bind(&tenant)
    .fetch_one(&pool)
    .await
    .expect("read failed state");
    assert_eq!(
        unchanged, corrupt,
        "failed proof commits no receipt, audit, load version or outbox"
    );
    canonical::put_workspace(&pool, &tenant, &intact)
        .await
        .expect("restore exact retained evidence");
    let saved = b
        .mutate_authorized(
            (&tenant, "ops:reviewer"),
            "retry-after-restoration",
            "movement.time.correct",
            &source,
            repair.clone(),
        )
        .await
        .expect("same failed key reusable");
    let reconnect = Store::new(Some(&url)).await.expect("reconnect");
    let replay = reconnect
        .mutate_authorized(
            (&tenant, "ops:reviewer"),
            "retry-after-restoration",
            "movement.time.correct",
            &source,
            repair,
        )
        .await
        .expect("retained replay");
    assert_eq!(saved["data"], replay["data"]);
    assert_eq!(replay["replayed"], true);
    let retained = reconnect.read(&tenant).await.expect("retained history");
    assert_eq!(retained.movement_corrections.len(), 2);
    assert_eq!(
        json!(retained.carrier_submissions),
        json!(original.carrier_submissions)
    );
    assert_eq!(physical(&retained, &id), physical(&original, &id));
    let milestones = &retained.outbox.last().expect("tracking").payload["trackingData"]
        ["freight_actual_milestones"];
    assert_eq!(
        milestones,
        &json!([{"kind":"pickup","observed_at":saved["data"]["observed_at"],"corrected":true}])
    );
    let native = reconnect.mutate_authorized((&tenant,"carrier:actual"),"later-native-pickup","portal.tracking",offer["id"].as_str().expect("offer"),json!({"carrier_id":"ca4","subject":"carrier:actual","request":{"expected_load_version":retained.loads.iter().find(|l|l.id==id).expect("load").version,"kind":"pickup","observed_at":Utc::now()-Duration::minutes(10),"location":"Original pickup location retained","reference":"Later actual native pickup","note":"Actual carrier evidence after review"}})).await.expect("new native report");
    let after_native = reconnect.read(&tenant).await.expect("native state");
    let milestones = after_native
        .outbox
        .last()
        .expect("same-transaction snapshot")
        .payload["trackingData"]["freight_actual_milestones"]
        .as_array()
        .expect("milestones");
    assert_eq!(milestones.len(), 2);
    assert_eq!(
        milestones[1],
        json!({"kind":"pickup","observed_at":native["data"]["observed_at"],"corrected":false})
    );
    assert!(retained
        .events
        .iter()
        .any(|e| e.kind == "movement.time.correct" && e.actor == "ops:reviewer"));
    assert!(reconnect
        .read(&domain::id("unrelated-movement"))
        .await
        .expect("other tenant")
        .movement_corrections
        .is_empty());
    sqlx::query("DELETE FROM freight_workspaces WHERE tenant_id=$1")
        .bind(&tenant)
        .execute(&pool)
        .await
        .expect("remove owned tenant");
    pool.close().await;
}
