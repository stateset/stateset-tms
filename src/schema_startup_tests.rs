use super::*;

#[test]
fn schema_configuration_requires_an_explicit_supported_policy() {
    assert_eq!(
        schema_mode(Err(std::env::VarError::NotPresent)).expect("default"),
        SchemaMode::Apply
    );
    assert_eq!(
        schema_mode(Ok("apply".into())).expect("apply"),
        SchemaMode::Apply
    );
    assert_eq!(
        schema_mode(Ok("verify".into())).expect("verify"),
        SchemaMode::Verify
    );
    for value in ["", "auto", "true", "1", "VERIFY", " verify", "verify "] {
        assert!(schema_mode(Ok(value.into())).is_err(), "accepted {value}");
    }
    assert!(schema_mode(Err(std::env::VarError::NotUnicode("invalid".into()))).is_err());
}

#[tokio::test]
async fn schema_verification_requires_a_database() {
    let result =
        Store::connect_with_schema_mode(None, DatabaseBudget::default(), SchemaMode::Verify).await;
    assert!(result
        .err()
        .expect("durable schema required")
        .to_string()
        .contains("requires DATABASE_URL"));
}

struct Fixture {
    admin: PgPool,
    schema: String,
    admin_url: String,
    role: String,
    role_url: String,
    application: String,
}
impl Fixture {
    async fn new(install: bool) -> Self {
        let base = std::env::var("TEST_DATABASE_URL")
            .expect("disposable database with CREATE ROLE rights");
        let admin = PgPool::connect(&base).await.expect("admin");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let schema = format!("schema_startup_{suffix}");
        let role = format!("schema_app_{suffix}");
        let password = uuid::Uuid::new_v4().simple().to_string();
        let application = format!("schema-verify-{suffix}");
        eprintln!("Owned schema startup fixture: schema={schema} role={role}");
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("owned schema");
        let mut url = reqwest::Url::parse(&base).expect("URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema},public"));
        let admin_url = url.to_string();
        if install {
            Store::migrate(&admin_url)
                .await
                .expect("administrative schema application");
        }
        sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT"))
            .execute(&admin).await.expect("owned role");
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO {role}"))
            .execute(&admin)
            .await
            .expect("schema access");
        if install {
            sqlx::query(&format!(
                "GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO {role}"
            ))
            .execute(&admin)
            .await
            .expect("read grants");
            let tables: Vec<String> = sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname=$1 AND tablename NOT IN ('freight_schema_migrations','freight_storage_format')")
                .bind(&schema).fetch_all(&admin).await.expect("business tables");
            for table in tables {
                assert!(table
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_'));
                sqlx::query(&format!(
                    "GRANT INSERT,UPDATE,DELETE ON {schema}.{table} TO {role}"
                ))
                .execute(&admin)
                .await
                .expect("business DML");
            }
            sqlx::query(&format!(
                "GRANT UPDATE(singleton) ON {schema}.freight_storage_format TO {role}"
            ))
            .execute(&admin)
            .await
            .expect("format share lock without version update rights");
            sqlx::query(&format!(
                "GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO {role}"
            ))
            .execute(&admin)
            .await
            .expect("sequence access");
        }
        url.set_username(&role).expect("username");
        url.set_password(Some(&password)).expect("password");
        url.query_pairs_mut()
            .append_pair("application_name", &application);
        Self {
            admin,
            schema,
            admin_url,
            role,
            role_url: url.to_string(),
            application,
        }
    }
    async fn verify(&self) -> Result<Store, sqlx::Error> {
        Store::connect_with_schema_mode(
            Some(&self.role_url),
            DatabaseBudget::default(),
            SchemaMode::Verify,
        )
        .await
    }
    async fn metadata(&self) -> Value {
        sqlx::query_scalar(&format!("SELECT jsonb_build_object('history',(SELECT jsonb_agg(to_jsonb(m) ORDER BY version) FROM {s}.freight_schema_migrations m),'format',(SELECT jsonb_agg(to_jsonb(f)) FROM {s}.freight_storage_format f),'tables',(SELECT jsonb_agg(tablename ORDER BY tablename) FROM pg_tables WHERE schemaname='{s}'))", s=self.schema))
            .fetch_one(&self.admin).await.expect("exact metadata witness")
    }
    async fn close(self) {
        sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename=$1 AND pid<>pg_backend_pid()")
            .bind(&self.role).execute(&self.admin).await.expect("only owned role connections");
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .expect("owned schema cleanup");
        sqlx::query(&format!("DROP ROLE {}", self.role))
            .execute(&self.admin)
            .await
            .expect("owned role cleanup");
        self.admin.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL and CREATE ROLE rights"]
async fn verification_of_missing_schema_creates_no_objects_and_admin_migration_creates_no_tenants()
{
    let f = Fixture::new(false).await;
    let result = f.verify().await;
    assert!(result.is_err(), "missing migration history must fail");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname=$1")
        .bind(&f.schema)
        .fetch_one(&f.admin)
        .await
        .expect("inventory");
    assert_eq!(count, 0, "verification creates no schema objects");
    Store::migrate(&f.admin_url)
        .await
        .expect("explicit schema installation");
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {}.freight_workspaces",
        f.schema
    ))
    .fetch_one(&f.admin)
    .await
    .expect("tenant count");
    assert_eq!(
        count, 0,
        "schema-only application creates no business workspace"
    );
    let before = f.metadata().await;
    Store::migrate(&f.admin_url)
        .await
        .expect("repeat migration");
    assert_eq!(
        f.metadata().await,
        before,
        "repeat retains original migration history"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL and CREATE ROLE rights"]
async fn restricted_schema_verification_retains_real_writes_originals_and_restart_without_ddl() {
    let f = Fixture::new(true).await;
    let store = f.verify().await.expect("non-owner verified startup");
    let pool = store.pool.as_ref().expect("durable");
    let identity: (String, String, bool, bool) = sqlx::query_as("SELECT current_user::text,session_user::text,rolsuper,rolcreaterole FROM pg_roles WHERE rolname=current_user")
        .fetch_one(pool).await.expect("actual login");
    assert_eq!(identity, (f.role.clone(), f.role.clone(), false, false));
    for statement in [
        format!("CREATE TABLE {}.must_not_exist(id integer)", f.schema),
        "UPDATE freight_schema_migrations SET checksum=checksum".into(),
        "UPDATE freight_storage_format SET version=version".into(),
        "ALTER TABLE freight_workspaces DISABLE TRIGGER freight_storage_format_guard".into(),
    ] {
        let err = sqlx::raw_sql(&statement)
            .execute(pool)
            .await
            .expect_err("privilege refusal");
        assert_eq!(
            err.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501")
        );
    }
    let before = f.metadata().await;
    let body = json!({"name":"Original restricted role customer","contact":"owned@example.test"});
    let original = store
        .mutate(
            "role-tenant",
            "role-original",
            "customer.create",
            "",
            body.clone(),
        )
        .await
        .expect("business write");
    assert_eq!(
        store
            .read("role-tenant")
            .await
            .expect("read")
            .customers
            .len(),
        1
    );
    pool.close().await;
    let restarted = f.verify().await.expect("restricted restart");
    let replay = restarted
        .mutate(
            "role-tenant",
            "role-original",
            "customer.create",
            "",
            body.clone(),
        )
        .await
        .expect("exact original");
    assert_eq!(replay["data"], original["data"]);
    assert_eq!(replay["replayed"], true);
    assert!(restarted
        .mutate(
            "role-tenant",
            "role-original",
            "customer.create",
            "",
            json!({"name":"Changed original","contact":"owned@example.test"})
        )
        .await
        .is_err());
    assert_eq!(
        restarted
            .read("role-tenant")
            .await
            .expect("retained read")
            .customers
            .len(),
        1
    );
    assert_eq!(
        f.metadata().await,
        before,
        "runtime never changes schema metadata"
    );
    restarted.pool.as_ref().expect("pool").close().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL and CREATE ROLE rights"]
async fn verified_startup_refuses_incomplete_modified_future_and_wrong_format_without_repair() {
    let f = Fixture::new(true).await;
    let valid = f.verify().await.expect("initial restricted verification");
    valid.pool.as_ref().expect("pool").close().await;
    let (version, checksum): (i32, String) = sqlx::query_as(&format!(
        "SELECT version,checksum FROM {}.freight_schema_migrations ORDER BY version DESC LIMIT 1",
        f.schema
    ))
    .fetch_one(&f.admin)
    .await
    .expect("last original migration");
    sqlx::query(&format!(
        "DELETE FROM {}.freight_schema_migrations WHERE version=$1",
        f.schema
    ))
    .bind(version)
    .execute(&f.admin)
    .await
    .expect("pending migration fixture");
    let before = f.metadata().await;
    assert!(f
        .verify()
        .await
        .err()
        .expect("incomplete history")
        .to_string()
        .contains("schema is incomplete"));
    assert_eq!(f.metadata().await, before);
    sqlx::query(&format!(
        "INSERT INTO {}.freight_schema_migrations(version,checksum) VALUES($1,$2)",
        f.schema
    ))
    .bind(version)
    .bind(checksum)
    .execute(&f.admin)
    .await
    .expect("restore history");
    sqlx::query(&format!(
        "DELETE FROM {}.freight_schema_migrations WHERE version=7",
        f.schema
    ))
    .execute(&f.admin)
    .await
    .expect("gapped history fixture");
    let before = f.metadata().await;
    assert!(f
        .verify()
        .await
        .err()
        .expect("gapped history")
        .to_string()
        .contains("Unsupported or modified freight migration history"));
    assert_eq!(f.metadata().await, before);
    sqlx::query(&format!(
        "INSERT INTO {}.freight_schema_migrations(version,checksum) VALUES(7,$1)",
        f.schema
    ))
    .bind(format!("{:x}", Sha256::digest(MIGRATIONS[6].as_bytes())))
    .execute(&f.admin)
    .await
    .expect("restore original gap entry");
    for change in [
        format!(
            "UPDATE {}.freight_schema_migrations SET checksum=repeat('f',64) WHERE version=1",
            f.schema
        ),
        format!(
            "INSERT INTO {}.freight_schema_migrations(version,checksum) VALUES(32,repeat('f',64))",
            f.schema
        ),
    ] {
        sqlx::raw_sql(&change)
            .execute(&f.admin)
            .await
            .expect("history refusal fixture");
        let before = f.metadata().await;
        assert!(f
            .verify()
            .await
            .err()
            .expect("unsupported history")
            .to_string()
            .contains("Unsupported or modified freight migration history"));
        assert_eq!(f.metadata().await, before);
        sqlx::query(&format!(
            "DELETE FROM {}.freight_schema_migrations WHERE version>31",
            f.schema
        ))
        .execute(&f.admin)
        .await
        .expect("remove future fixture");
        sqlx::query(&format!(
            "UPDATE {}.freight_schema_migrations SET checksum=$1 WHERE version=1",
            f.schema
        ))
        .bind(format!("{:x}", Sha256::digest(MIGRATIONS[0].as_bytes())))
        .execute(&f.admin)
        .await
        .expect("restore original checksum");
    }
    sqlx::query(&format!(
        "UPDATE {}.freight_storage_format SET version=$1",
        f.schema
    ))
    .bind(STORAGE_FORMAT + 1)
    .execute(&f.admin)
    .await
    .expect("future format fixture");
    let before = f.metadata().await;
    assert!(f
        .verify()
        .await
        .err()
        .expect("future format")
        .to_string()
        .contains(INCOMPATIBLE_STORAGE));
    assert_eq!(f.metadata().await, before);
    sqlx::query(&format!("DELETE FROM {}.freight_storage_format", f.schema))
        .execute(&f.admin)
        .await
        .expect("missing format marker fixture");
    let before = f.metadata().await;
    assert!(f
        .verify()
        .await
        .err()
        .expect("missing format marker")
        .to_string()
        .contains(INCOMPATIBLE_STORAGE));
    assert_eq!(f.metadata().await, before);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL and CREATE ROLE rights"]
async fn schema_verification_waits_for_migration_lock_and_checks_the_committed_history() {
    let f = Fixture::new(true).await;
    let mut tx = f.admin.begin().await.expect("migration transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(785432019651)")
        .execute(&mut *tx)
        .await
        .expect("migration fence");
    let url = f.role_url.clone();
    let mut verification = Box::pin(async move {
        Store::connect_with_schema_mode(Some(&url), DatabaseBudget::default(), SchemaMode::Verify)
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let observed_waiter = async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE a.application_name=$1 AND l.locktype='advisory' AND NOT l.granted)")
            .bind(&f.application).fetch_one(&f.admin).await.expect("actual lock waiter");
            if waiting {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "verification never waited on migration fence"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        _ = &mut verification => panic!("verification completed before migration released its fence"),
        _ = observed_waiter => {}
    }
    sqlx::query(&format!(
        "INSERT INTO {}.freight_schema_migrations(version,checksum) VALUES(32,repeat('f',64))",
        f.schema
    ))
    .execute(&mut *tx)
    .await
    .expect("future release history");
    tx.commit().await.expect("migration commit");
    assert!(verification
        .await
        .err()
        .expect("future release refused")
        .to_string()
        .contains("Unsupported or modified freight migration history"));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL and CREATE ROLE rights"]
async fn verification_cannot_fall_back_to_another_installed_schema() {
    let mut f = Fixture::new(false).await;
    let fallback = format!("schema_fallback_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {fallback}"))
        .execute(&f.admin)
        .await
        .expect("owned fallback schema");
    fn with_path(url: &str, path: &str) -> String {
        let mut url = reqwest::Url::parse(url).expect("URL");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != "options")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(pairs)
            .append_pair("options", &format!("-csearch_path={path}"));
        url.to_string()
    }
    let admin_url = with_path(&f.admin_url, &format!("{fallback},public"));
    Store::migrate(&admin_url)
        .await
        .expect("install distinct fallback");
    sqlx::raw_sql(&format!("GRANT USAGE ON SCHEMA {fallback} TO {role}; GRANT SELECT ON ALL TABLES IN SCHEMA {fallback} TO {role}; GRANT UPDATE(singleton) ON {fallback}.freight_storage_format TO {role}",role=f.role))
        .execute(&f.admin).await.expect("fallback read rights");
    f.role_url = with_path(&f.role_url, &format!("{},{fallback},public", f.schema));
    let control = PgPool::connect(&f.role_url)
        .await
        .expect("restricted login");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM freight_schema_migrations")
        .fetch_one(&control)
        .await
        .expect("PostgreSQL would resolve fallback history");
    assert_eq!(count, 31);
    control.close().await;
    let result = f.verify().await;
    let message = match result {
        Ok(store) => {
            store.pool.as_ref().expect("pool").close().await;
            None
        }
        Err(error) => Some(error.to_string()),
    };
    sqlx::query(&format!("DROP SCHEMA {fallback} CASCADE"))
        .execute(&f.admin)
        .await
        .expect("fallback cleanup");
    f.close().await;
    assert!(
        message.is_some_and(|value| value.contains("current schema")),
        "verification must refuse an installed fallback schema"
    );
}
