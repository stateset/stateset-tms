use chrono::{Duration, Utc};
use serde_json::json;
use stateset_freight::{
    domain::{self, Status},
    store::Store,
};
#[path = "support/canonical.rs"]
mod canonical;

#[test]
fn expiry_is_an_once_only_exception_and_a_late_carrier_acceptance_remains_applicable() {
    let now = Utc::now();
    let mut w = domain::demo();
    let l = &mut w.loads[2];
    l.exception = None;
    l.tender_expires_at = Some(now - Duration::minutes(1));
    let reference = l.reference.clone();
    let carrier_id = l.carrier_id.clone();
    let partner = w
        .carriers
        .iter()
        .find(|c| Some(&c.id) == carrier_id.as_ref())
        .expect("carrier")
        .partner_id
        .clone();
    let before = json!(w);
    assert!(w.tender_expiry_due(now));
    assert_eq!(w.expire_tenders(now).expect("expiry"), 1);
    assert_eq!(w.loads[2].status, Status::Tendered);
    assert_eq!(w.loads[2].version, 2);
    assert_eq!(w.loads[2].carrier_id, carrier_id);
    assert_eq!(json!(w.outbox), before["outbox"]);
    for (i, l) in w.loads.iter().enumerate().filter(|(i, _)| *i != 2) {
        assert_eq!(json!(l), before["loads"][i]);
    }
    assert_eq!(w.events.last().expect("audit").kind, "tender.overdue");
    let expired = json!(w);
    assert_eq!(w.expire_tenders(now).expect("repeat"), 0);
    assert_eq!(json!(w), expired);
    w.apply("edi.event","",json!({"event_id":"late-990-after-expiry","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":reference,"partner_id":partner,"accepted":true}})).expect("late actual carrier response");
    assert_eq!(w.loads[2].status, Status::Accepted);
    assert_eq!(w.loads[2].carrier_id, carrier_id);
    assert_eq!(
        w.events
            .iter()
            .filter(|e| e.kind == "tender.overdue")
            .count(),
        1
    );
}

#[test]
fn expiry_respects_exact_deadlines_existing_exceptions_other_states_and_atomic_overflow() {
    let now = Utc::now();
    let mut w = domain::demo();
    for l in &mut w.loads {
        l.tender_expires_at = None;
    }
    let template = w.loads[2].clone();
    w.loads.clear();
    for (i, (status, deadline, exception)) in [
        (Status::Tendered, now, None),
        (Status::Tendered, now + Duration::seconds(1), None),
        (
            Status::Tendered,
            now - Duration::minutes(1),
            Some("Actual carrier coordination remains open".into()),
        ),
        (Status::Accepted, now - Duration::minutes(1), None),
        (Status::InTransit, now - Duration::minutes(1), None),
        (Status::Cancelled, now - Duration::minutes(1), None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut l = template.clone();
        l.id = format!("excluded-{i}");
        l.status = status;
        l.tender_expires_at = Some(deadline);
        l.exception = exception;
        w.loads.push(l);
    }
    let before = json!(w);
    assert!(!w.tender_expiry_due(now));
    assert_eq!(w.expire_tenders(now).expect("quiet"), 0);
    assert_eq!(json!(w), before);
    let mut due = template;
    due.id = "due".into();
    due.exception = None;
    due.tender_expires_at = Some(now - Duration::minutes(1));
    w.loads.push(due.clone());
    due.id = "exhausted".into();
    due.version = u64::MAX;
    w.loads.push(due);
    let before = json!(w);
    assert!(w
        .expire_tenders(now)
        .expect_err("exhausted")
        .contains("version exhausted"));
    assert_eq!(json!(w), before);
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL"]
async fn postgres_competing_expiry_and_idle_checks_commit_one_physical_update_and_retain_late_acceptance(
) {
    let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
    let admin = sqlx::PgPool::connect(&base).await.expect("admin");
    let schema = format!("expiry_test_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("schema");
    let app = format!("expiry-{}", uuid::Uuid::new_v4().simple());
    let mut url = reqwest::Url::parse(&base).expect("url");
    url.query_pairs_mut()
        .append_pair("options", &format!("-csearch_path={schema}"))
        .append_pair("application_name", &app);
    let a = Store::new(Some(url.as_str())).await.expect("first store");
    let b = Store::new(Some(url.as_str())).await.expect("second store");
    let pool = sqlx::PgPool::connect(url.as_str())
        .await
        .expect("test pool");
    a.seed_demo().await.expect("seed");
    sqlx::raw_sql("CREATE TABLE expiry_write_trace (id bigserial PRIMARY KEY); CREATE FUNCTION trace_expiry_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO expiry_write_trace DEFAULT VALUES; RETURN NEW; END $$; CREATE TRIGGER expiry_trace AFTER UPDATE OF data ON freight_workspaces FOR EACH ROW EXECUTE FUNCTION trace_expiry_write()") .execute(&pool).await.expect("source header update observer");
    canonical::set(
        &pool,
        "demo",
        &["loads", "2", "tender_expires_at"],
        json!(Utc::now() - Duration::minutes(1)),
    )
    .await
    .expect("due fixture");
    sqlx::query("TRUNCATE expiry_write_trace")
        .execute(&pool)
        .await
        .expect("reset observer");
    let mut lock = pool.begin().await.expect("blocker");
    sqlx::query("SELECT data FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
        .fetch_one(&mut *lock)
        .await
        .expect("hold row");
    let first = tokio::spawn(async move { a.expire_tenders("demo").await });
    let second = tokio::spawn(async move { b.expire_tenders("demo").await });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let blocked:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'").bind(&app).fetch_one(&pool).await.expect("blocked writers");
        if blocked >= 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "both sweeps must reach the tenant lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    lock.commit().await.expect("release blocker");
    let (x, y) = (
        first.await.expect("first task").expect("first expiry"),
        second.await.expect("second task").expect("second expiry"),
    );
    assert_eq!(x + y, 1);
    let store = Store::new(Some(url.as_str())).await.expect("reconnect");
    let before = store.read("demo").await.expect("retained");
    assert_eq!(before.loads[2].status, Status::Tendered);
    assert_eq!(before.loads[2].version, 2);
    assert_eq!(
        before
            .events
            .iter()
            .filter(|e| e.kind == "tender.overdue")
            .count(),
        1
    );
    let physical = canonical::physical(&pool, "demo")
        .await
        .expect("expired originals");
    assert_eq!(store.expire_tenders("demo").await.expect("idle sweep"), 0);
    assert_eq!(
        canonical::physical(&pool, "demo")
            .await
            .expect("idle originals"),
        physical
    );
    let writes: i64 = sqlx::query_scalar("SELECT count(*) FROM expiry_write_trace")
        .fetch_one(&pool)
        .await
        .expect("write count");
    assert_eq!(
        writes, 1,
        "losing and idle sweeps must not commit another source header"
    );
    let carrier = before
        .carriers
        .iter()
        .find(|c| Some(&c.id) == before.loads[2].carrier_id.as_ref())
        .expect("carrier");
    store.mutate("demo","late-990","edi.event","",json!({"event_id":"pg-late-990","event_type":"edi.load_tender_response.received","occurred_at":Utc::now(),"payload":{"shipment_id":before.loads[2].reference,"partner_id":carrier.partner_id,"accepted":true}})).await.expect("late acceptance");
    assert_eq!(
        store.read("demo").await.expect("accepted").loads[2].status,
        Status::Accepted
    );
    drop(store);
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("cleanup");
    admin.close().await;
}
