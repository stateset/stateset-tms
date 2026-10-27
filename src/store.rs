use crate::canonical_storage as canonical;
use crate::database_budget::{error as database_error, DatabaseBudget};
pub use crate::database_budget::{DATABASE_BUSY, DATABASE_UNAVAILABLE};
use crate::domain::{self, Receipt, Workspace};
pub use crate::read_admission::READ_BUSY;
pub use crate::workspace_admission::WORKSPACE_BUSY;
use crate::workspace_admission::{WorkLease, WorkspaceAdmission, WorkspaceTransaction};
pub use crate::write_admission::WRITE_BUSY;
use crate::write_admission::{Admission, WriteTransaction, WriteTurns};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
type DocumentBytes = BTreeMap<(String, String), Vec<u8>>;
enum UpdateOutcome {
    Commit(Value),
    Unchanged(Value),
    Declined,
}
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 2;
fn database_configuration_error() -> sqlx::Error {
    sqlx::Error::Protocol("FREIGHT_DATABASE_MAX_CONNECTIONS must be an integer from 1 to 64".into())
}
fn database_max_connections(raw: Option<&str>) -> Result<u32, sqlx::Error> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_DATABASE_MAX_CONNECTIONS);
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(database_configuration_error());
    }
    raw.parse::<u32>()
        .ok()
        .filter(|n| (1..=64).contains(n))
        .ok_or_else(database_configuration_error)
}
fn read_concurrency(raw: Option<&str>) -> Result<usize, sqlx::Error> {
    database_max_connections(Some(raw.unwrap_or("2")))
        .map(|n| n as usize)
        .map_err(|_| {
            sqlx::Error::Protocol("FREIGHT_READ_CONCURRENCY must be an integer from 1 to 64".into())
        })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SchemaMode {
    Apply,
    Verify,
}
fn schema_mode(raw: Result<String, std::env::VarError>) -> Result<SchemaMode, sqlx::Error> {
    match raw {
        Err(std::env::VarError::NotPresent) => Ok(SchemaMode::Apply),
        Ok(value) if value == "apply" => Ok(SchemaMode::Apply),
        Ok(value) if value == "verify" => Ok(SchemaMode::Verify),
        _ => Err(sqlx::Error::Protocol(
            "FREIGHT_MIGRATIONS must be apply or verify".into(),
        )),
    }
}
/// Supported persisted format and transactional writer compatibility fence.
pub const STORAGE_FORMAT: i32 = canonical::FORMAT;
pub const INCOMPATIBLE_STORAGE: &str = "Storage format is incompatible with this API release";
pub const DEMO_PROVENANCE_REQUIRED: &str = "Existing demo tenant has no recorded synthetic seed; use a fresh demo database or start with FREIGHT_DEMO=false";
const MIGRATIONS: [&str; 31] = [
    include_str!("../migrations/001_workspaces.sql"),
    include_str!("../migrations/002_documents.sql"),
    include_str!("../migrations/003_storage_format.sql"),
    include_str!("../migrations/004_cancellation_fees.sql"),
    include_str!("../migrations/005_quotes.sql"),
    include_str!("../migrations/006_source_reviews.sql"),
    include_str!("../migrations/007_quote_amendments.sql"),
    include_str!("../migrations/008_customer_refunds.sql"),
    include_str!("../migrations/009_rate_confirmations.sql"),
    include_str!("../migrations/010_partner_portal.sql"),
    include_str!("../migrations/011_carrier_submissions.sql"),
    include_str!("../migrations/012_customer_quote_portal.sql"),
    include_str!("../migrations/013_document_sharing.sql"),
    include_str!("../migrations/014_carrier_release_portal.sql"),
    include_str!("../migrations/015_carrier_contracts.sql"),
    include_str!("../migrations/016_multi_stop.sql"),
    include_str!("../migrations/017_execution_chronology.sql"),
    include_str!("../migrations/018_movement_corrections.sql"),
    include_str!("../migrations/019_operations_work.sql"),
    include_str!("../migrations/020_integration_wakeups.sql"),
    include_str!("../migrations/021_load_catalog.sql"),
    include_str!("../migrations/022_incremental_load_catalog.sql"),
    include_str!("../migrations/023_save_resolution.sql"),
    include_str!("../migrations/024_indexed_history.sql"),
    include_str!("../migrations/025_tender_deadlines.sql"),
    include_str!("../migrations/026_canonical_blocks.sql"),
    include_str!("../migrations/027_finance_policy_fence.sql"),
    include_str!("../migrations/028_synthetic_demo_provenance.sql"),
    include_str!("../migrations/029_commerce_wakeups.sql"),
    include_str!("../migrations/030_console_index.sql"),
    include_str!("../migrations/031_queue_invalidation.sql"),
];

enum ReadSnapshot {
    Postgres {
        raw: Option<canonical::Snapshot>,
        tx: WorkspaceTransaction,
        cache: Arc<canonical::ReadCache>,
        tenant: String,
    },
    Memory {
        memory: tokio::sync::OwnedRwLockReadGuard<BTreeMap<String, Workspace>>,
        tenant: String,
    },
}
impl ReadSnapshot {
    fn materialize(self, profile: &crate::read_profile::ReadProfile) -> Result<Workspace, String> {
        let started = Instant::now();
        match self {
            Self::Postgres {
                raw,
                tx,
                cache,
                tenant,
            } => {
                let decoded = match raw {
                    Some(raw) => raw.decode_cached(&tenant, &cache),
                    None => Ok(Workspace::default()),
                };
                profile.stage("decode", started);
                let workspace = decoded?;
                // Called only inside a blocking job. The asynchronous runtime
                // continues driving SQL while this worker waits for COMMIT.
                // Keep the format lock through decoding, then return the DB
                // connection before projection, encoding and disposal.
                let commit_started = Instant::now();
                let committed = tokio::runtime::Handle::current()
                    .block_on(tx.commit())
                    .map_err(database_error);
                profile.stage("commit", commit_started);
                committed?;
                Ok(workspace)
            }
            Self::Memory { memory, tenant } => {
                let workspace = memory.get(&tenant).cloned().unwrap_or_default();
                drop(memory);
                profile.stage("decode", started);
                Ok(workspace)
            }
        }
    }
}

#[derive(Clone)]
pub struct Store {
    pool: Option<PgPool>,
    memory: Arc<RwLock<BTreeMap<String, Workspace>>>,
    blobs: Arc<RwLock<DocumentBytes>>,
    read_slots: Arc<Semaphore>,
    catalog_admission: Arc<crate::catalog_admission::CatalogAdmission>,
    read_turns: Arc<crate::read_admission::TenantReadTurns>,
    write_turns: Arc<WriteTurns>,
    synthetic_demo: Arc<AtomicBool>,
    workspace_admission: Option<Arc<WorkspaceAdmission>>,
    read_cache: Arc<canonical::ReadCache>,
}
impl Store {
    pub async fn new(url: Option<&str>) -> Result<Self, sqlx::Error> {
        Self::connect(url, DatabaseBudget::from_env()?).await
    }
    /// Apply and verify the embedded schema using an administrative connection.
    /// This creates no tenant workspace and starts no application services.
    pub async fn migrate(url: &str) -> Result<(), sqlx::Error> {
        let store = Self::connect_with_schema_mode(
            Some(url),
            DatabaseBudget::from_env()?,
            SchemaMode::Apply,
        )
        .await?;
        if let Some(pool) = &store.pool {
            pool.close().await;
        }
        Ok(())
    }
    async fn connect(
        url: Option<&str>,
        database_budget: DatabaseBudget,
    ) -> Result<Self, sqlx::Error> {
        Self::connect_with_schema_mode(
            url,
            database_budget,
            schema_mode(std::env::var("FREIGHT_MIGRATIONS"))?,
        )
        .await
    }
    async fn connect_with_schema_mode(
        url: Option<&str>,
        database_budget: DatabaseBudget,
        schema_mode: SchemaMode,
    ) -> Result<Self, sqlx::Error> {
        if url.is_none() && schema_mode == SchemaMode::Verify {
            return Err(sqlx::Error::Protocol(
                "FREIGHT_MIGRATIONS=verify requires DATABASE_URL".into(),
            ));
        }
        let catalog_admission = Arc::new(crate::catalog_admission::CatalogAdmission::new(
            crate::read_admission::ReadLimits::catalog_from_env().map_err(sqlx::Error::Protocol)?,
        ));
        let write_turns = Arc::new(WriteTurns::new(
            crate::write_admission::WriteLimits::from_env().map_err(sqlx::Error::Protocol)?,
            std::time::Duration::from_millis(database_budget.acquire_ms),
        ));
        let read_turns = Arc::new(crate::read_admission::TenantReadTurns::new(
            crate::read_admission::ReadLimits::from_env().map_err(sqlx::Error::Protocol)?,
        ));
        let configured = match std::env::var("FREIGHT_READ_CONCURRENCY") {
            Ok(raw) => Some(raw),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(sqlx::Error::Protocol(
                    "Invalid FREIGHT_READ_CONCURRENCY".into(),
                ))
            }
        };
        let read_slots = Arc::new(Semaphore::new(read_concurrency(configured.as_deref())?));
        let configured = std::env::var("FREIGHT_WORKSPACE_CONCURRENCY")
            .map(Some)
            .or_else(|error| match error {
                std::env::VarError::NotPresent => Ok(None),
                _ => Err(sqlx::Error::Protocol(
                    "Invalid FREIGHT_WORKSPACE_CONCURRENCY".into(),
                )),
            })?;
        let workspace_jobs = crate::workspace_admission::concurrency(configured.as_deref())
            .map_err(sqlx::Error::Protocol)?;
        let pool = if let Some(url) = url {
            let configured = match std::env::var("FREIGHT_DATABASE_MAX_CONNECTIONS") {
                Ok(raw) => Some(raw),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(database_configuration_error())
                }
            };
            let database_connections = database_max_connections(configured.as_deref())?;
            let pool = database_budget
                .pool_options(database_connections)
                .connect(url)
                .await?;
            let mut migration = pool.begin().await?;
            sqlx::query("SET LOCAL lock_timeout = '10s'")
                .execute(&mut *migration)
                .await?;
            sqlx::query("SET LOCAL statement_timeout = '60s'")
                .execute(&mut *migration)
                .await?;
            // Serialize both schema application and verification with migration
            // jobs; rollback if a newer release has changed the schema/format.
            sqlx::query("SELECT pg_advisory_xact_lock(785432019651)")
                .execute(&mut *migration)
                .await?;
            if schema_mode == SchemaMode::Apply {
                sqlx::raw_sql(
                    "CREATE TABLE IF NOT EXISTS freight_schema_migrations (
                    version integer PRIMARY KEY CHECK (version > 0),
                    checksum text NOT NULL CHECK (length(checksum) = 64),
                    applied_at timestamptz NOT NULL DEFAULT now()
                )",
                )
                .execute(&mut *migration)
                .await?;
            }
            if schema_mode == SchemaMode::Verify {
                // Match the schema in which administrative CREATE would run.
                // A later search_path entry must not supply another schema's
                // valid migration history or format marker.
                for relation in ["freight_schema_migrations", "freight_storage_format"] {
                    let in_current_schema: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=pg_catalog.to_regclass($1) AND n.nspname=pg_catalog.current_schema())",
                    )
                    .bind(relation)
                    .fetch_one(&mut *migration)
                    .await?;
                    if !in_current_schema {
                        return Err(sqlx::Error::Protocol(
                            "Database schema is not fully installed in the current schema; run stateset-freight --migrate with the migration role".into(),
                        ));
                    }
                }
            }
            let applied: Vec<(i32, String)> = sqlx::query_as(
                "SELECT version,checksum FROM freight_schema_migrations ORDER BY version",
            )
            .fetch_all(&mut *migration)
            .await?;
            // An exact prefix prevents downgrade, gaps, and edited historical
            // migrations. A future release appends its next migration here.
            for (index, (version, checksum)) in applied.iter().enumerate() {
                if *version != index as i32 + 1
                    || MIGRATIONS.get(index).is_none_or(|sql| {
                        *checksum != format!("{:x}", Sha256::digest(sql.as_bytes()))
                    })
                {
                    return Err(sqlx::Error::Protocol(
                        "Unsupported or modified freight migration history".into(),
                    ));
                }
            }
            if schema_mode == SchemaMode::Verify && applied.len() != MIGRATIONS.len() {
                return Err(sqlx::Error::Protocol(
                    "Database schema is incomplete; run stateset-freight --migrate with the migration role".into(),
                ));
            }
            for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied.len()) {
                if index == 25 {
                    migration = canonical::migrate(migration, sql).await?;
                } else if index == 26 {
                    migration = canonical::migrate_policy_fence(migration, sql, 19).await?;
                } else if index == 27 {
                    migration = canonical::migrate_policy_fence(migration, sql, 20).await?;
                } else {
                    sqlx::raw_sql(sql).execute(&mut *migration).await?;
                }
                sqlx::query(
                    "INSERT INTO freight_schema_migrations(version,checksum) VALUES($1,$2)",
                )
                .bind(index as i32 + 1)
                .bind(format!("{:x}", Sha256::digest(sql.as_bytes())))
                .execute(&mut *migration)
                .await?;
            }
            let version: Option<i32> = sqlx::query_scalar(
                "SELECT version FROM freight_storage_format WHERE singleton FOR SHARE",
            )
            .fetch_optional(&mut *migration)
            .await?;
            if version != Some(STORAGE_FORMAT) {
                return Err(sqlx::Error::Protocol(INCOMPATIBLE_STORAGE.into()));
            }
            migration.commit().await?;
            Some(pool)
        } else {
            None
        };
        let workspace_admission = workspace_jobs
            .map(|jobs| {
                crate::read_admission::ReadLimits::from_env()
                    .map(|limits| Arc::new(WorkspaceAdmission::new(jobs, limits)))
            })
            .transpose()
            .map_err(sqlx::Error::Protocol)?;
        Ok(Self {
            pool,
            memory: Arc::default(),
            blobs: Arc::default(),
            read_slots,
            catalog_admission,
            read_turns,
            write_turns,
            workspace_admission,
            read_cache: Arc::default(),
            synthetic_demo: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn durable(&self) -> bool {
        self.pool.is_some()
    }
    pub(crate) fn admission_diagnostics(&self) -> crate::admission_diagnostics::AdmissionSnapshot {
        use crate::admission_diagnostics::{AdmissionSnapshot, PoolSnapshot, ReadLane};
        AdmissionSnapshot {
            read: ReadLane::new(self.read_turns.diagnostics(), &self.read_slots),
            catalog: self.catalog_admission.diagnostics(),
            workspace: self
                .workspace_admission
                .as_ref()
                .map(|work| work.diagnostics()),
            write: self.write_turns.diagnostics(),
            pool: self.pool.as_ref().map(|pool| PoolSnapshot {
                connections: pool.size(),
                idle: pool.num_idle(),
                limit: pool.options().get_max_connections(),
                closed: pool.is_closed(),
            }),
        }
    }
    pub async fn tenant_ids(&self) -> Result<Vec<String>, String> {
        if let Some(pool) = &self.pool {
            let mut tx = pool.begin().await.map_err(database_error)?;
            require_format(&mut tx).await?;
            let tenants = sqlx::query_scalar("SELECT tenant_id FROM freight_workspaces")
                .fetch_all(&mut *tx)
                .await
                .map_err(database_error)?;
            tx.commit().await.map_err(database_error)?;
            Ok(tenants)
        } else {
            Ok(self.memory.read().await.keys().cloned().collect())
        }
    }
    pub async fn healthy(&self) -> bool {
        match &self.pool {
            Some(pool) => sqlx::query_scalar::<_, i32>(
                "SELECT version FROM freight_storage_format WHERE singleton",
            )
            .fetch_one(pool)
            .await
            .is_ok_and(|version| version == STORAGE_FORMAT),
            None => true,
        }
    }
    pub async fn seed_demo(&self) -> Result<(), String> {
        if let Some(pool) = &self.pool {
            let work = self.workspace_work("demo").await?;
            let tx = pool.begin().await.map_err(database_error)?;
            let mut tx = WorkspaceTransaction::new(tx, work.clone());
            self.configure_workspace_transaction(&mut tx).await?;
            require_format(&mut tx).await?;
            if canonical::insert_empty(&mut tx, "demo").await? {
                let snapshot = canonical::snapshot(&mut tx, "demo", true)
                    .await?
                    .ok_or("Internal storage error")?;
                let (returned, prepared) = tokio::task::spawn_blocking(move || {
                    let (_, before) = snapshot.decode_mutation()?;
                    let workspace = domain::demo();
                    Ok::<_, String>((tx, canonical::prepare("demo", before, &workspace)?))
                })
                .await
                .map_err(internal)??;
                tx = returned;
                canonical::persist(&mut tx, prepared).await?;
                sqlx::query(
                    "UPDATE freight_workspaces SET synthetic_demo=true WHERE tenant_id='demo'",
                )
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            } else {
                let verified: bool = sqlx::query_scalar("SELECT synthetic_demo FROM freight_workspaces WHERE tenant_id='demo' FOR UPDATE")
                    .fetch_one(&mut *tx).await.map_err(database_error)?;
                if !verified {
                    return Err(DEMO_PROVENANCE_REQUIRED.into());
                }
            }
            tx.commit().await.map_err(database_error)?;
        } else {
            let mut memory = self.memory.write().await;
            let _work = self.workspace_work("demo").await?;
            match memory.entry("demo".into()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(domain::demo());
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    if !self.synthetic_demo.load(Ordering::Acquire) {
                        return Err(DEMO_PROVENANCE_REQUIRED.into());
                    }
                }
            }
        }
        // Only the explicitly seeded synthetic tenant can use the one-person
        // demonstration. No HTTP request or credential can enable this policy.
        self.synthetic_demo.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn invoice_review_required(&self, tenant: &str) -> bool {
        tenant != "demo" || !self.synthetic_demo.load(Ordering::Acquire)
    }
    async fn read_permit(&self, tenant: &str) -> Result<OwnedSemaphorePermit, String> {
        self.read_turns
            .acquire(tenant, self.read_slots.clone())
            .await
    }
    async fn index_transaction(
        &self,
        pool: &PgPool,
        tenant: &str,
        page: Option<Arc<OwnedSemaphorePermit>>,
    ) -> Result<WriteTransaction, String> {
        // Lazy catalog/history preparation can lock the tenant row. Coordinate
        // with local mutations before acquiring a database connection, while
        // keeping indexed pages independent of whole-workspace CPU admission.
        let deadline = self.write_turns.deadline();
        let turn = self.write_turns.request(tenant)?.wait(deadline).await?;
        let transaction = tokio::time::timeout_at(deadline, pool.begin())
            .await
            .map_err(|_| database_error(sqlx::Error::PoolTimedOut))?
            .map_err(database_error)?;
        let mut transaction = WriteTransaction::new(transaction, turn);
        // Indexed pages retain page admission; deadline hints have no page
        // lease. Neither is a whole-workspace processing lease.
        transaction.hold_work(page);
        Ok(transaction)
    }
    async fn workspace_work(&self, tenant: &str) -> Result<WorkLease, String> {
        match &self.workspace_admission {
            Some(admission) => admission.acquire(tenant).await.map(Some),
            None => Ok(None),
        }
    }
    async fn configure_workspace_transaction(
        &self,
        tx: &mut sqlx::PgConnection,
    ) -> Result<(), String> {
        if self.workspace_admission.is_some() {
            // Do not hold global processing admission behind another process's tenant lock. All failed writes still roll back and require explicit retry.
            sqlx::query("SET LOCAL lock_timeout = '1ms'")
                .execute(tx)
                .await
                .map_err(database_error)?;
        }
        Ok(())
    }
    async fn read_snapshot(
        &self,
        tenant: &str,
        profile: &crate::read_profile::ReadProfile,
    ) -> Result<(ReadSnapshot, WorkLease), String> {
        let started = Instant::now();
        let result = if let Some(pool) = &self.pool {
            let work = self.workspace_work(tenant).await?;
            let tx = pool.begin().await.map_err(database_error)?;
            let mut tx = WorkspaceTransaction::new(tx, work.clone());
            self.configure_workspace_transaction(&mut tx).await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            require_format(&mut tx).await?;
            let raw = canonical::snapshot(&mut tx, tenant, false).await?;
            (
                ReadSnapshot::Postgres {
                    raw,
                    tx,
                    cache: self.read_cache.clone(),
                    tenant: tenant.to_string(),
                },
                work,
            )
        } else {
            let memory = self.memory.clone().read_owned().await;
            let work = self.workspace_work(tenant).await?;
            (
                ReadSnapshot::Memory {
                    memory,
                    tenant: tenant.to_string(),
                },
                work,
            )
        };
        profile.stage("snapshot", started);
        Ok(result)
    }
    pub async fn read(&self, tenant: &str) -> Result<Workspace, String> {
        let permit = self.read_permit(tenant).await?;
        let store = self.clone();
        let tenant = tenant.to_string();
        // Once admitted, the SQL/decode task owns the permit too. Abandoning an
        // HTTP awaiter cannot release a slot while PostgreSQL is still reading.
        tokio::spawn(async move {
            let profile = crate::read_profile::ReadProfile::default();
            let (snapshot, work) = store.read_snapshot(&tenant, &profile).await?;
            tokio::task::spawn_blocking(move || {
                let _work = work;
                let _permit = permit;
                snapshot.materialize(&profile)
            })
            .await
            .map_err(internal)?
        })
        .await
        .map_err(internal)?
    }
    pub async fn load_catalog(
        &self,
        tenant: &str,
        query: crate::load_catalog::CatalogQuery,
    ) -> Result<Value, String> {
        let selection = query.selection(tenant)?;
        let permit = self.catalog_admission.acquire(tenant).await?;
        let store = self.clone();
        let tenant = tenant.to_string();
        tokio::spawn(async move {
            let _permit = permit.clone();
            if let Some(pool) = &store.pool {
                let mut attempts = 0;
                let rows: Vec<(bool, Option<String>)> = loop {
                    let mut tx = store
                        .index_transaction(pool, &tenant, Some(permit.clone()))
                        .await?;
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                        .execute(&mut *tx)
                        .await
                        .map_err(database_error)?;
                    require_format(&mut tx).await?;
                    if let Err(error) = sqlx::query("SELECT freight_ensure_load_catalog($1)")
                        .bind(&tenant)
                        .execute(&mut *tx)
                        .await
                    {
                        let conflict = error
                            .as_database_error()
                            .is_some_and(|db| db.code().as_deref() == Some("40001"));
                        tx.rollback().await.map_err(database_error)?;
                        if conflict {
                            attempts += 1;
                            if attempts < 3 {
                                continue;
                            }
                            return Err(DATABASE_BUSY.into());
                        }
                        return Err(database_error(error));
                    }
                    let rows = sqlx::query_as(selection.page_query())
                        .bind(&tenant)
                        .bind(&selection.after)
                        .bind(&selection.status)
                        .bind(&selection.customer_id)
                        .bind(&selection.carrier_id)
                        .bind(&selection.q)
                        .bind((selection.limit + 1) as i64)
                        .fetch_all(&mut *tx)
                        .await
                        .map_err(database_error)?;
                    tx.commit().await.map_err(database_error)?;
                    break rows;
                };
                let mut loads = Vec::with_capacity(rows.len());
                for (valid, raw) in rows {
                    if !valid {
                        return Err("Internal storage error".into());
                    }
                    if let Some(raw) = raw {
                        loads.push(serde_json::from_str(&raw).map_err(internal)?);
                    }
                }
                selection.page(loads)
            } else {
                let memory = store.memory.read().await;
                selection.memory_page(memory.get(&tenant).unwrap_or(&Workspace::default()))
            }
        })
        .await
        .map_err(internal)?
    }
    pub async fn history_page(
        &self,
        tenant: &str,
        query: crate::history::Query,
    ) -> Result<Value, String> {
        self.history_project(tenant, query, Ok).await
    }
    pub(crate) async fn history_project<F, T>(
        &self,
        tenant: &str,
        query: crate::history::Query,
        project: F,
    ) -> Result<T, String>
    where
        F: FnOnce(Value) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let selection = query.selection(tenant)?;
        let permit = Arc::new(self.read_permit(tenant).await?);
        let store = self.clone();
        let tenant = tenant.to_string();
        // The admitted task owns its slot through SQL, parsing and response
        // construction even if its HTTP caller goes away.
        tokio::spawn(async move {
            if let Some(pool) = &store.pool {
                // The ready check and page SELECT must see the same source
                // generation even when a command commits between statements.
                // Cold builders can conflict under repeatable read; restart
                // only those transactions, retaining the admitted read slot.
                let mut attempts = 0;
                let rows = loop {
                    let mut tx = store
                        .index_transaction(pool, &tenant, Some(permit.clone()))
                        .await?;
                    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                        .execute(&mut *tx)
                        .await
                        .map_err(database_error)?;
                    require_format(&mut tx).await?;
                    let rebuilt = sqlx::query("SELECT freight_ensure_history($1)")
                        .bind(&tenant)
                        .execute(&mut *tx)
                        .await;
                    if let Err(error) = rebuilt {
                        let conflict = error
                            .as_database_error()
                            .is_some_and(|db| db.code().as_deref() == Some("40001"));
                        tx.rollback().await.map_err(database_error)?;
                        if conflict {
                            attempts += 1;
                            if attempts < 3 {
                                continue;
                            }
                            return Err(DATABASE_BUSY.into());
                        }
                        return Err(database_error(error));
                    }
                    let rows = sqlx::query(crate::history::PAGE_SQL)
                        .bind(&tenant)
                        .bind(selection.kind.field())
                        .bind(&selection.load_id)
                        .bind(selection.upper())
                        .bind(selection.before())
                        .bind((selection.limit + 1) as i64)
                        .bind(STORAGE_FORMAT.to_string())
                        .fetch_all(&mut *tx)
                        .await
                        .map_err(database_error)?;
                    tx.commit().await.map_err(database_error)?;
                    break rows;
                };
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    project(selection.postgres_page(rows)?)
                })
                .await
                .map_err(internal)?
            } else {
                let memory = store.memory.clone().read_owned().await;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    project(
                        selection
                            .memory_page(memory.get(&tenant).unwrap_or(&Workspace::default()))?,
                    )
                })
                .await
                .map_err(internal)?
            }
        })
        .await
        .map_err(internal)?
    }
    async fn read_process<F, T>(&self, tenant: &str, project: F) -> Result<T, String>
    where
        F: FnOnce(Workspace, &crate::read_profile::ReadProfile) -> Result<T, String>
            + Send
            + 'static,
        T: Send + 'static,
    {
        let profile = crate::read_profile::ReadProfile::new(tenant);
        let admission_started = Instant::now();
        let permit = self.read_permit(tenant).await?;
        profile.stage("admission", admission_started);
        let store = self.clone();
        let tenant = tenant.to_string();
        tokio::spawn(async move {
            let (snapshot, work) = store.read_snapshot(&tenant, &profile).await?;
            let queued = Instant::now();
            tokio::task::spawn_blocking(move || {
                let _work = work;
                let _permit = permit;
                profile.stage("decode_queue", queued);
                let workspace = snapshot.materialize(&profile)?;
                project(workspace, &profile)
            })
            .await
            .map_err(internal)?
        })
        .await
        .map_err(internal)?
    }
    pub(crate) async fn read_project<F>(&self, tenant: &str, project: F) -> Result<Vec<u8>, String>
    where
        F: FnOnce(Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.read_process(tenant, move |workspace, profile| {
            let started = Instant::now();
            let value = project(workspace)?;
            profile.stage("projection", started);
            let started = Instant::now();
            let bytes = serde_json::to_vec(&value).map_err(internal);
            profile.stage("encoding", started);
            let started = Instant::now();
            drop(value);
            profile.stage("response_value_drop", started);
            profile.complete();
            bytes
        })
        .await
    }
    pub(crate) async fn read_encode<F>(&self, tenant: &str, encode: F) -> Result<Vec<u8>, String>
    where
        F: FnOnce(Workspace) -> Result<Vec<u8>, String> + Send + 'static,
    {
        self.read_process(tenant, move |workspace, profile| {
            let started = Instant::now();
            let bytes = encode(workspace);
            profile.stage("projection_encoding", started);
            profile.complete();
            bytes
        })
        .await
    }
    pub(crate) async fn approval_project<F>(
        &self,
        tenant: &str,
        invoice: &str,
        project: F,
    ) -> Result<Vec<u8>, String>
    where
        F: FnOnce(Value) -> Result<Value, String> + Send + 'static,
    {
        let profile = crate::read_profile::ReadProfile::new(tenant);
        let started = Instant::now();
        let permit = self.read_permit(tenant).await?;
        profile.stage("admission", started);
        let store = self.clone();
        let tenant = tenant.to_owned();
        let invoice = invoice.to_owned();
        tokio::spawn(async move {
            let started = Instant::now();
            let memory = if store.pool.is_none() {
                Some(store.memory.clone().read_owned().await)
            } else {
                None
            };
            let work = store.workspace_work(&tenant).await?;
            let snapshot = if let Some(pool) = &store.pool {
                let tx = pool.begin().await.map_err(database_error)?;
                let mut tx = WorkspaceTransaction::new(tx, work.clone());
                store.configure_workspace_transaction(&mut tx).await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
                require_format(&mut tx).await?;
                let source = canonical::approval_snapshot(&mut tx, &tenant).await?;
                Some((source, tx))
            } else {
                None
            };

            profile.stage("approval_snapshot", started);
            let queued = Instant::now();
            tokio::task::spawn_blocking(move || {
                let _work = work;
                let _permit = permit;
                profile.stage("decode_queue", queued);
                let started = Instant::now();
                let evidence = if let Some((source, tx)) = snapshot {
                    let source = source.ok_or("Invoice not found")?;
                    let (header, blocks, payload) = source.volume();
                    profile.approval_volume(header, blocks, payload);
                    let decoded = source.decode()?;
                    profile.stage("approval_decode", started);
                    let evidence = decoded.evidence(&invoice)?;
                    // Keep the format fence and connection through native
                    // validation, including abandonment of the HTTP awaiter.
                    tokio::runtime::Handle::current()
                        .block_on(tx.commit())
                        .map_err(database_error)?;
                    evidence
                } else {
                    let memory = memory.ok_or("Internal storage error")?;
                    let workspace = memory.get(&tenant).ok_or("Invoice not found")?;
                    let evidence = workspace.invoice_approval_evidence(&invoice)?;
                    drop(memory);
                    profile.stage("approval_decode", started);
                    evidence
                };
                let started = Instant::now();
                let value = project(evidence)?;
                profile.stage("projection", started);
                let started = Instant::now();
                let bytes = serde_json::to_vec(&value).map_err(internal);
                profile.stage("encoding", started);
                profile.complete();
                bytes
            })
            .await
            .map_err(internal)?
        })
        .await
        .map_err(internal)?
    }
    pub async fn mutate(
        &self,
        tenant: &str,
        key: &str,
        operation: &str,
        entity: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.mutate_authorized((tenant, "system"), key, operation, entity, body)
            .await
    }
    pub async fn mutate_authorized(
        &self,
        principal: (&str, &str),
        key: &str,
        operation: &str,
        entity: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.mutate_guarded(principal, (key, operation, entity, body), None)
            .await
    }
    pub async fn mutate_guarded(
        &self,
        principal: (&str, &str),
        request: (&str, &str, &str, Value),
        source: Option<crate::source::SourceGrant>,
    ) -> Result<Value, String> {
        let (key, operation, entity, body) = request;
        let (tenant, actor) = principal;
        let invoice_review_required = self.invoice_review_required(tenant);
        // Only trusted mutation assembly unwraps a native observation. Public
        // replay and save resolution fingerprint the exact caller object.
        let request_body = if matches!(operation, "commerce.tracking" | "commerce.reconcile") {
            body.get("request").unwrap_or(&body)
        } else {
            &body
        };
        let fingerprint = fingerprint(operation, entity, request_body);
        let original_signature = fingerprint.clone();
        let owned = (
            key.to_owned(),
            operation.to_owned(),
            entity.to_owned(),
            actor.to_owned(),
        );
        self.update_receipted_original(
            tenant,
            None,
            if matches!(
                operation,
                "invoice.approve"
                    | "settlement.create"
                    | "bill.issue"
                    | "charge.review"
                    | "payable.review"
                    | "carrier_credit.apply"
                    | "cancellation.review"
                    | "cash.record"
                    | "credit.create"
                    | "rate.issue"
                    | "rate.handoff"
                    | "portal.publish"
                    | "portal.respond"
                    | "share.create"
            ) {
                Some(operation)
            } else {
                None
            },
            Some((key, &original_signature)),
            move |workspace| {
                let (key, operation, entity, actor) = (owned.0.as_str(), owned.1.as_str(), owned.2.as_str(), owned.3.as_str());
                if !workspace.receipts.contains_key(key) {
                    if let Some(grant) = &source {
                        if workspace.commitment_source(operation,entity,&body).map(|v|v.id.as_str())!=Some(grant.load_id.as_str()) {return Err("Sync source verification does not match this commitment".into());}
                        if matches!(operation,"quote.book"|"quote.decision"|"portal.quote.publish"|"portal.quote.respond") && workspace.commitment_quote(operation,entity).and_then(|q|q.current().ok()).is_some_and(|r|r.amendment_of.is_some()) {
                            let check=workspace.source_check(&grant.load_id).ok_or(crate::source::SOURCE_UNAVAILABLE)?;
                            if check.version!=grant.version {return Err("Sync source check changed; retry against the current observation".into());}
                            if check.last_error.is_some() || check.valid_until.is_none_or(|v|v<=chrono::Utc::now()) {return Err(crate::source::SOURCE_UNAVAILABLE.into());}
                            if operation=="quote.book" {
                                let quote=workspace.quotes.iter().find(|q|q.id==entity).ok_or("Quote not found")?;
                                let load=workspace.loads.iter().find(|v|v.id==grant.load_id).ok_or("Shipment not found")?;
                                workspace.verify_amendment_source(quote,load)?;
                            }
                        } else {workspace.check_source_grant(grant)?;}
                    }
                }
                let start = workspace.events.len();
                let journal_start = workspace.journal.len();
                let tender_start = workspace.tenders.len();
                let source_start = workspace.source_checks.len();
                let response = apply(workspace, key, &fingerprint, operation, entity, body, (actor, invoice_review_required))?;
                for event in &mut workspace.events[start..] {
                    event.actor = actor.into();
                }
                for entry in &mut workspace.journal[journal_start..] {
                    entry.actor = actor.into();
                }
                for tender in &mut workspace.tenders[tender_start..] {
                    if !tender.legacy {
                        tender.created_by = actor.into();
                    }
                }
                for check in &mut workspace.source_checks[source_start..] {
                    check.checked_by = actor.into();
                }
                Ok(response)
            },
        )
        .await
    }
    pub async fn record_source(
        &self,
        principal: (&str, &str),
        observed: (&str, Value, u64, chrono::DateTime<chrono::Utc>),
        key: Option<&str>,
    ) -> Result<Value, String> {
        let (tenant, actor) = principal;
        let (load, snapshot, expected, requested_at) = observed;
        let signature = fingerprint("source.refresh", load, &json!({}));
        let owned = (load.to_owned(), actor.to_owned(), key.map(str::to_owned));
        self.update_receipted(tenant, None, None, move |w| {
            let (load, actor, key) = (owned.0.as_str(), owned.1.as_str(), owned.2.as_deref());
            if let Some(receipt) = key.and_then(|key| w.receipts.get(key)) {
                let response = receipt.checked_response(&signature)?;
                return Ok(json!({"data":response,"replayed":true}));
            }
            w.observe_source(load, snapshot, expected, requested_at, actor)?;
            let response = json!(w
                .source_check(load)
                .ok_or("Source observation not recorded")?);
            if let Some(key) = key {
                w.receipts.insert(
                    key.into(),
                    Receipt {
                        fingerprint: signature,
                        response: response.clone(),
                        stopped: None,
                    },
                );
            }
            Ok(json!({"data":response,"replayed":false}))
        })
        .await
    }
    pub async fn record_source_error(
        &self,
        tenant: &str,
        load: &str,
        expected: u64,
        message: &str,
    ) -> Result<(), String> {
        let load = load.to_owned();
        let message = message.to_owned();
        self.update(tenant, move |w| {
            w.fail_source(&load, expected, &message)?;
            Ok(Value::Null)
        })
        .await
        .map(|_| ())
    }
    pub async fn replay(
        &self,
        tenant: &str,
        key: &str,
        operation: &str,
        entity: &str,
        body: &Value,
    ) -> Result<Option<Value>, String> {
        let signature = fingerprint(operation, entity, body);
        let permit = self.read_permit(tenant).await?;
        self.replay_admitted(tenant, key, signature, permit).await
    }
    async fn replay_admitted(
        &self,
        tenant: &str,
        key: &str,
        signature: String,
        permit: OwnedSemaphorePermit,
    ) -> Result<Option<Value>, String> {
        let store = self.clone();
        let tenant = tenant.to_string();
        let key = key.to_string();
        // Admission remains owned through SQL, selected-record decoding and
        // result construction even if the original HTTP awaiter disappears.
        tokio::spawn(async move {
            if let Some(pool) = &store.pool {
                let mut tx = pool.begin().await.map_err(database_error)?;
                require_format(&mut tx).await?;
                let row: Option<(bool, bool, Option<String>)> =
                    sqlx::query_as(include_str!("receipt_lookup.sql"))
                        .bind(&tenant)
                        .bind(&key)
                        .bind(STORAGE_FORMAT.to_string())
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(database_error)?;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    let result = match row {
                        None => Ok(None),
                        Some((false, _, _)) => Err(INCOMPATIBLE_STORAGE.into()),
                        Some((true, false, _)) => Err("Internal storage error".into()),
                        Some((true, true, None)) => Ok(None),
                        Some((true, true, Some(raw))) => serde_json::from_str::<Receipt>(&raw)
                            .map_err(internal)
                            .and_then(|receipt| replayed_receipt(&receipt, &signature)),
                    };
                    // This is a read-only transaction. Keep its format lock
                    // through decode/validation and release it before admission.
                    tokio::runtime::Handle::current()
                        .block_on(tx.commit())
                        .map_err(database_error)?;
                    result
                })
                .await
                .map_err(internal)?
            } else {
                let memory = store.memory.clone().read_owned().await;
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    match memory.get(&tenant).and_then(|w| w.receipts.get(&key)) {
                        Some(receipt) => replayed_receipt(receipt, &signature),
                        None => Ok(None),
                    }
                })
                .await
                .map_err(internal)?
            }
        })
        .await
        .map_err(internal)?
    }
    async fn replay_contended(
        &self,
        tenant: &str,
        key: &str,
        signature: &str,
    ) -> Result<Option<Value>, String> {
        let pool = self.pool.as_ref().ok_or("Internal storage error")?;
        let mut tx = pool.begin().await.map_err(database_error)?;
        require_format(&mut tx).await?;
        let row: Option<(bool, bool, Option<String>)> =
            sqlx::query_as(include_str!("receipt_lookup.sql"))
                .bind(tenant)
                .bind(key)
                .bind(STORAGE_FORMAT.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(database_error)?;
        let signature = signature.to_owned();
        tokio::task::spawn_blocking(move || {
            let result = match row {
                None => Ok(None),
                Some((false, _, _)) => Err(INCOMPATIBLE_STORAGE.into()),
                Some((true, false, _)) => Err("Internal storage error".into()),
                Some((true, true, None)) => Ok(None),
                Some((true, true, Some(raw))) => serde_json::from_str::<Receipt>(&raw)
                    .map_err(internal)
                    .and_then(|receipt| replayed_receipt(&receipt, &signature)),
            };
            tokio::runtime::Handle::current()
                .block_on(tx.commit())
                .map_err(database_error)?;
            result
        })
        .await
        .map_err(internal)?
    }
    pub(crate) async fn update<F>(&self, tenant: &str, change: F) -> Result<Value, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.update_binary(tenant, None, change).await
    }
    /// Scheduling is advisory. A positive result must still be claimed under
    /// the tenant lock; a concurrent new job after a negative result is seen on
    /// the next poll. PostgreSQL maintains these small columns with every write.
    pub(crate) async fn integration_due(
        &self,
        tenant: &str,
        inbound: bool,
    ) -> Result<bool, String> {
        if let Some(pool) = &self.pool {
            let mut tx = pool.begin().await.map_err(database_error)?;
            require_format(&mut tx).await?;
            let due: Option<bool> = sqlx::query_scalar(if inbound {
                "SELECT coalesce(inbox_due_at <= $2::text::timestamptz,false) FROM freight_workspaces WHERE tenant_id=$1"
            } else {
                "SELECT coalesce(edi_due_at <= $2::text::timestamptz,false) OR coalesce(sync_due_at <= $2::text::timestamptz,false) FROM freight_workspaces WHERE tenant_id=$1"
            })
            .bind(tenant).bind(chrono::Utc::now().to_rfc3339()).fetch_optional(&mut *tx).await.map_err(database_error)?;
            tx.commit().await.map_err(database_error)?;
            Ok(due.unwrap_or(false))
        } else {
            let memory = self.memory.read().await;
            let Some(w) = memory.get(tenant) else {
                return Ok(false);
            };
            let now = chrono::Utc::now();
            Ok(if inbound {
                w.inbox.iter().any(|item| crate::inbox::due(item, now))
            } else {
                w.outbox
                    .iter()
                    .any(|job| crate::integrations::due(job, now))
            })
        }
    }
    pub(crate) async fn tender_expiry_may_be_due(&self, tenant: &str) -> Result<bool, String> {
        let Some(pool) = &self.pool else {
            return Ok(true);
        };
        // Preparing a stale deadline hint can lock the tenant row too. Join
        // local write admission before SQL without consuming snapshot/CPU work.
        let mut tx = self.index_transaction(pool, tenant, None).await?;
        require_format(&mut tx).await?;
        sqlx::query("SELECT freight_ensure_tender_deadline($1)")
            .bind(tenant)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let due: Option<bool> = sqlx::query_scalar(include_str!("tender_expiry_due.sql"))
            .bind(tenant)
            .bind(chrono::Utc::now().to_rfc3339())
            .fetch_optional(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(due.unwrap_or(false))
    }
    /// A decision made under the tenant lock may decline to commit. All staged
    /// state, including an initially inserted empty tenant, is then discarded.
    pub(crate) async fn update_if<F>(
        &self,
        tenant: &str,
        change: F,
    ) -> Result<Option<Value>, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Option<Value>, String> + Send + 'static,
    {
        self.update_conditionally_checked(tenant, None, None, None, move |w| {
            change(w).map(|result| match result {
                Some(response) => UpdateOutcome::Commit(response),
                None => UpdateOutcome::Declined,
            })
        })
        .await
    }
    pub(crate) async fn read_binary(&self, tenant: &str, id: &str) -> Result<Vec<u8>, String> {
        if let Some(pool) = &self.pool {
            sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id=$1 AND document_id=$2")
                .bind(tenant).bind(id).fetch_optional(pool).await.map_err(database_error)?.ok_or_else(||"Internal storage error".into())
        } else {
            self.blobs
                .read()
                .await
                .get(&(tenant.into(), id.into()))
                .cloned()
                .ok_or_else(|| "Internal storage error".into())
        }
    }
    pub(crate) async fn update_binary<F>(
        &self,
        tenant: &str,
        blob: Option<(String, Vec<u8>)>,
        change: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.update_checked(tenant, blob, None, change).await
    }
    /// Serialize current access policy, original-byte verification, and its audit
    /// against revocation. Bytes are released only after the authorization commits.
    pub(crate) async fn authorized_document<F>(
        &self,
        tenant: &str,
        authorize: F,
    ) -> Result<(crate::documents::Document, Vec<u8>), String>
    where
        F: FnOnce(&mut Workspace) -> Result<crate::documents::Document, String> + Send,
    {
        if let Some(pool) = &self.pool {
            let work = self.workspace_work(tenant).await?;
            let tx = pool.begin().await.map_err(database_error)?;
            let mut tx = WorkspaceTransaction::new(tx, work.clone());
            self.configure_workspace_transaction(&mut tx).await?;
            require_format(&mut tx).await?;
            let snapshot = canonical::snapshot(&mut tx, tenant, true)
                .await?
                .ok_or("Shared file not found")?;
            let (returned, mut workspace, before) = tokio::task::spawn_blocking(move || {
                let (workspace, before) = snapshot.decode_mutation()?;
                Ok::<_, String>((tx, workspace, before))
            })
            .await
            .map_err(internal)??;
            tx = returned;
            let document = authorize(&mut workspace)?;
            let bytes:Option<Vec<u8>>=sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id=$1 AND document_id=$2 FOR SHARE").bind(tenant).bind(&document.id).fetch_optional(&mut *tx).await.map_err(database_error)?;
            verify_document(&document, bytes.as_deref())?;
            let owner = tenant.to_owned();
            let (returned, prepared) = tokio::task::spawn_blocking(move || {
                Ok::<_, String>((tx, canonical::prepare(&owner, before, &workspace)?))
            })
            .await
            .map_err(internal)??;
            tx = returned;
            canonical::persist(&mut tx, prepared).await?;
            tx.commit().await.map_err(database_error)?;
            Ok((document, bytes.ok_or("Internal storage error")?))
        } else {
            let mut memory = self.memory.write().await;
            let _work = self.workspace_work(tenant).await?;
            let mut workspace = memory.get(tenant).cloned().ok_or("Shared file not found")?;
            let document = authorize(&mut workspace)?;
            let blobs = self.blobs.read().await;
            let bytes = blobs.get(&(tenant.into(), document.id.clone()));
            verify_document(&document, bytes.map(Vec::as_slice))?;
            let bytes = bytes.cloned().ok_or("Internal storage error")?;
            memory.insert(tenant.into(), workspace);
            Ok((document, bytes))
        }
    }
    async fn update_checked<F>(
        &self,
        tenant: &str,
        blob: Option<(String, Vec<u8>)>,
        financial_action: Option<&str>,
        change: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.update_conditionally_checked(tenant, blob, financial_action, None, move |w| {
            change(w).map(UpdateOutcome::Commit)
        })
        .await?
        .ok_or_else(|| "Internal storage error".into())
    }
    /// Only for commands whose verified receipt branch makes no state changes.
    /// A response's `replayed` flag alone is not a general no-write contract:
    /// inbound callback redelivery can also retain a new delivery identity.
    pub(crate) async fn update_receipted<F>(
        &self,
        tenant: &str,
        blob: Option<(String, Vec<u8>)>,
        financial_action: Option<&str>,
        change: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.update_receipted_original(tenant, blob, financial_action, None, change)
            .await
    }
    /// Shared commands check their immutable original in the same transaction
    /// used by a miss. Custom policy/file paths keep their existing closures.
    async fn update_receipted_original<F>(
        &self,
        tenant: &str,
        blob: Option<(String, Vec<u8>)>,
        financial_action: Option<&str>,
        original: Option<(&str, &str)>,
        change: F,
    ) -> Result<Value, String>
    where
        F: FnOnce(&mut Workspace) -> Result<Value, String> + Send + 'static,
    {
        self.update_conditionally_checked(tenant, blob, financial_action, original, move |w| {
            change(w).map(|response| {
                if response["replayed"] == true {
                    UpdateOutcome::Unchanged(response)
                } else {
                    UpdateOutcome::Commit(response)
                }
            })
        })
        .await?
        .ok_or_else(|| "Internal storage error".into())
    }
    async fn update_conditionally_checked<F>(
        &self,
        tenant: &str,
        blob: Option<(String, Vec<u8>)>,
        financial_action: Option<&str>,
        original: Option<(&str, &str)>,
        change: F,
    ) -> Result<Option<Value>, String>
    where
        F: FnOnce(&mut Workspace) -> Result<UpdateOutcome, String> + Send + 'static,
    {
        if let Some(pool) = &self.pool {
            let deadline = self.write_turns.deadline();
            let admission = self.write_turns.request(tenant)?;
            let permit = match admission {
                Admission::Active(permit) => permit,
                waiting => {
                    // Only contended commands need another read. Known originals
                    // bypass the write queue and never consume snapshot capacity.
                    if let Some((key, signature)) = original {
                        if let Some(response) =
                            self.replay_contended(tenant, key, signature).await?
                        {
                            return Ok(Some(response));
                        }
                    }
                    waiting.wait(deadline).await?
                }
            };
            if self.workspace_admission.is_some() {
                if let Some((key, signature)) = original {
                    if let Some(response) = self.replay_contended(tenant, key, signature).await? {
                        return Ok(Some(response));
                    }
                }
            }
            let work_lease = self.workspace_work(tenant).await?;
            let transaction = tokio::time::timeout_at(deadline, pool.begin())
                .await
                .map_err(|_| database_error(sqlx::Error::PoolTimedOut))?
                .map_err(database_error)?;
            let mut tx = WriteTransaction::new(transaction, permit);
            tx.hold_work(work_lease.clone());
            self.configure_workspace_transaction(&mut tx).await?;
            require_format(&mut tx).await?;
            let (mut tx, workspace, before) = if let Some((key, signature)) = original {
                let row: Option<(bool, bool, Option<String>, Option<String>)> =
                    sqlx::query_as(include_str!("conditional_save.sql"))
                        .bind(tenant)
                        .bind(key)
                        .bind(STORAGE_FORMAT.to_string())
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(database_error)?;
                if let Some((marker_valid, receipts_valid, Some(raw), _)) = row {
                    let signature = signature.to_owned();
                    // This is a write command's read-only result. Its database
                    // connection bounds worker ownership independently of the
                    // snapshot read queue; no tenant row is locked on a hit.
                    let (tx, response) = tokio::task::spawn_blocking(move || {
                        let response = if !marker_valid {
                            Err(INCOMPATIBLE_STORAGE.into())
                        } else if !receipts_valid {
                            Err("Internal storage error".into())
                        } else {
                            serde_json::from_str::<Receipt>(&raw)
                                .map_err(internal)
                                .and_then(|receipt| replayed_receipt(&receipt, &signature))
                        };
                        (tx, response)
                    })
                    .await
                    .map_err(internal)?;
                    let response = response?;
                    tx.commit().await.map_err(database_error)?;
                    return Ok(response);
                }
                let raw = match row {
                    Some((_, _, None, Some(raw))) => raw,
                    None => {
                        // An absent tenant can race another first application.
                        // Insert and lock in the original order, then recheck
                        // the complete latest workspace before applying.
                        canonical::insert_empty(&mut tx, tenant).await?;
                        sqlx::query_scalar::<_, String>(
                            "SELECT data::text FROM freight_workspaces WHERE tenant_id=$1 FOR UPDATE",
                        )
                        .bind(tenant)
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(database_error)?
                    }
                    _ => return Err("Internal storage error".into()),
                };
                // The transaction travels with the queued decoder. If the
                // caller disappears, its row/format locks and pool connection
                // stay owned until the real work ends, then roll back.
                let snapshot = canonical::snapshot_from_header(&mut tx, tenant, raw).await?;
                tokio::task::spawn_blocking(move || {
                    let (workspace, before) = snapshot.decode_mutation()?;
                    Ok::<_, String>((tx, workspace, before))
                })
                .await
                .map_err(internal)??
            } else {
                canonical::insert_empty(&mut tx, tenant).await?;
                let snapshot = canonical::snapshot(&mut tx, tenant, true)
                    .await?
                    .ok_or("Internal storage error")?;
                tokio::task::spawn_blocking(move || {
                    let (workspace, before) = snapshot.decode_mutation()?;
                    Ok::<_, String>((tx, workspace, before))
                })
                .await
                .map_err(internal)??
            };
            // Receipt hits have returned above. Every new one-person approval
            // rechecks durable provenance under this same tenant/format lock;
            // a stale process-local demo flag cannot exempt replacement data.
            if financial_action == Some("invoice.approve")
                && tenant == "demo"
                && self.synthetic_demo.load(Ordering::Acquire)
            {
                let verified: bool = sqlx::query_scalar(
                    "SELECT synthetic_demo FROM freight_workspaces WHERE tenant_id=$1",
                )
                .bind(tenant)
                .fetch_one(&mut *tx)
                .await
                .map_err(database_error)?;
                if !verified {
                    return Err(DEMO_PROVENANCE_REQUIRED.into());
                }
            }
            let (mut tx, work) = prepare_mutation(
                tx,
                tenant,
                workspace,
                financial_action,
                blob,
                before,
                change,
            )
            .await?;
            let Some(prepared) = work.prepared else {
                tx.rollback().await.map_err(database_error)?;
                return Ok(work.completed);
            };
            for document in &prepared.documents {
                let bytes: Option<Vec<u8>> = sqlx::query_scalar("SELECT content FROM freight_document_content WHERE tenant_id=$1 AND document_id=$2 FOR SHARE")
                    .bind(tenant).bind(&document.id).fetch_optional(&mut *tx).await.map_err(database_error)?;
                verify_document_bytes(
                    &document.id,
                    document.size_bytes,
                    &document.sha256,
                    bytes.as_deref(),
                )?;
            }
            if let Some((id, content)) = prepared.blob {
                sqlx::query("INSERT INTO freight_document_content(tenant_id,document_id,content) VALUES($1,$2,$3)")
                    .bind(tenant).bind(id).bind(content).execute(&mut *tx).await.map_err(database_error)?;
            }
            canonical::persist(&mut tx, prepared.arguments).await?;
            tx.commit().await.map_err(database_error)?;
            Ok(Some(prepared.response))
        } else {
            let memory = self.memory.clone().write_owned().await;
            if self.workspace_admission.is_some() {
                if let Some((key, signature)) = original {
                    if let Some(receipt) = memory.get(tenant).and_then(|w| w.receipts.get(key)) {
                        return replayed_receipt(receipt, signature);
                    }
                }
            }
            let work_lease = self.workspace_work(tenant).await?;
            let owner = tenant.to_owned();
            let action = financial_action.map(str::to_owned);
            let original = original.map(|(key, signature)| (key.to_owned(), signature.to_owned()));
            // The owned guard travels with CPU work. Dropping its awaiter never
            // releases write admission while an abandoned candidate still runs.
            let work = tokio::task::spawn_blocking(move || {
                if let Some(receipt) = original.as_ref().and_then(|(key, signature)| {
                    memory
                        .get(&owner)
                        .and_then(|workspace| workspace.receipts.get(key))
                        .map(|receipt| (receipt, signature.as_str()))
                }) {
                    let completed = replayed_receipt(receipt.0, receipt.1)?;
                    return Ok::<_, String>(MemoryMutation {
                        _work: work_lease.clone(),
                        memory,
                        candidate: None,
                        completed,
                    });
                }
                let mut workspace = memory.get(&owner).cloned().unwrap_or_default();
                let response = match change(&mut workspace)? {
                    UpdateOutcome::Commit(response) => response,
                    UpdateOutcome::Unchanged(response) => {
                        return Ok(MemoryMutation {
                            _work: work_lease.clone(),
                            memory,
                            candidate: None,
                            completed: Some(response),
                        })
                    }
                    UpdateOutcome::Declined => {
                        return Ok(MemoryMutation {
                            _work: work_lease.clone(),
                            memory,
                            candidate: None,
                            completed: None,
                        })
                    }
                };
                if response["replayed"] != true {
                    workspace.reconcile_operations(chrono::Utc::now());
                }
                let documents = document_integrity(&workspace, &response, action.as_deref())?;
                Ok(MemoryMutation {
                    _work: work_lease.clone(),
                    memory,
                    candidate: Some((workspace, response, documents)),
                    completed: None,
                })
            })
            .await
            .map_err(internal)??;
            let MemoryMutation {
                _work,
                mut memory,
                candidate,
                completed,
            } = work;
            let Some((workspace, response, documents)) = candidate else {
                return Ok(completed);
            };
            for document in documents {
                let blobs = self.blobs.read().await;
                verify_document_bytes(
                    &document.id,
                    document.size_bytes,
                    &document.sha256,
                    blobs
                        .get(&(tenant.into(), document.id.clone()))
                        .map(Vec::as_slice),
                )?;
            }
            if let Some((id, content)) =
                blob.filter(|(id, _)| workspace.documents.iter().any(|v| v.id == *id))
            {
                self.blobs
                    .write()
                    .await
                    .insert((tenant.into(), id), content);
            }
            memory.insert(tenant.into(), workspace);
            Ok(Some(response))
        }
    }
}
struct DocumentIntegrity {
    id: String,
    size_bytes: usize,
    sha256: String,
}
struct MemoryMutation {
    memory: tokio::sync::OwnedRwLockWriteGuard<BTreeMap<String, Workspace>>,
    candidate: Option<(Workspace, Value, Vec<DocumentIntegrity>)>,
    completed: Option<Value>,
    _work: WorkLease,
}
struct PreparedWork {
    prepared: Option<PreparedMutation>,
    completed: Option<Value>,
    _work: WorkLease,
}
struct PreparedMutation {
    arguments: canonical::Prepared,
    documents: Vec<DocumentIntegrity>,
    blob: Option<(String, Vec<u8>)>,
    response: Value,
}
/// Keep the source/format locks and pool connection with real CPU work even
/// after the caller drops its awaiter. An abandoned result rolls back; this
/// worker never inserts bytes, updates source or commits independently.
async fn prepare_mutation<F>(
    tx: WriteTransaction,
    tenant: &str,
    mut workspace: Workspace,
    action: Option<&str>,
    blob: Option<(String, Vec<u8>)>,
    before: canonical::Baseline,
    change: F,
) -> Result<(WriteTransaction, PreparedWork), String>
where
    F: FnOnce(&mut Workspace) -> Result<UpdateOutcome, String> + Send + 'static,
{
    let tenant = tenant.to_owned();
    let action = action.map(str::to_owned);
    let work_lease = tx.work();
    tokio::task::spawn_blocking(move || {
        let response = match change(&mut workspace)? {
            UpdateOutcome::Commit(response) => response,
            UpdateOutcome::Unchanged(response) => {
                return Ok((
                    tx,
                    PreparedWork {
                        _work: work_lease.clone(),
                        prepared: None,
                        completed: Some(response),
                    },
                ))
            }
            UpdateOutcome::Declined => {
                return Ok((
                    tx,
                    PreparedWork {
                        _work: work_lease.clone(),
                        prepared: None,
                        completed: None,
                    },
                ))
            }
        };
        if response["replayed"] != true {
            workspace.reconcile_operations(chrono::Utc::now());
        }
        let documents = document_integrity(&workspace, &response, action.as_deref())?;
        let blob = blob.filter(|(id, _)| workspace.documents.iter().any(|v| v.id == *id));

        let arguments = canonical::prepare(&tenant, before, &workspace)?;
        Ok((
            tx,
            PreparedWork {
                _work: work_lease.clone(),
                prepared: Some(PreparedMutation {
                    arguments,
                    documents,
                    blob,
                    response,
                }),
                completed: None,
            },
        ))
    })
    .await
    .map_err(internal)?
}
fn document_integrity(
    workspace: &Workspace,
    response: &Value,
    action: Option<&str>,
) -> Result<Vec<DocumentIntegrity>, String> {
    Ok(financial_documents(workspace, response, action)?
        .into_iter()
        .map(|document| DocumentIntegrity {
            id: document.id.clone(),
            size_bytes: document.size_bytes,
            sha256: document.sha256.clone(),
        })
        .collect())
}
fn replayed_receipt(receipt: &Receipt, signature: &str) -> Result<Option<Value>, String> {
    let response = receipt.checked_response(signature)?;
    Ok(Some(json!({"data":response,"replayed":true})))
}
async fn require_format(connection: &mut sqlx::PgConnection) -> Result<(), String> {
    let version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM freight_storage_format WHERE singleton FOR SHARE")
            .fetch_optional(connection)
            .await
            .map_err(database_error)?;
    if version != Some(STORAGE_FORMAT) {
        return Err(INCOMPATIBLE_STORAGE.into());
    }
    Ok(())
}
fn financial_documents<'a>(
    workspace: &'a Workspace,
    response: &Value,
    action: Option<&str>,
) -> Result<Vec<&'a crate::documents::Document>, String> {
    if action.is_none() || response["replayed"] == true {
        return Ok(vec![]);
    }
    if matches!(action, Some("portal.publish" | "portal.respond")) {
        if action == Some("portal.respond") && response["data"]["decision"] == "decline" {
            return Ok(vec![]);
        }
        let rate = response["data"]["rate_confirmation_id"].clone();
        return workspace.rate_documents("rate.issue", &json!({"data":{"id":rate}}));
    }
    if matches!(action, Some("rate.issue" | "rate.handoff")) {
        return workspace.rate_documents(action.unwrap_or_default(), response);
    }
    if action == Some("share.create") {
        let id = response["data"]["document_id"]
            .as_str()
            .ok_or("Internal storage error")?;
        return Ok(vec![workspace
            .documents
            .iter()
            .find(|d| d.id == id)
            .ok_or("Internal storage error")?]);
    }
    let mut ids = std::collections::BTreeSet::new();
    for document in
        workspace.customer_confirmation_documents(action.unwrap_or_default(), response)?
    {
        ids.insert(document.id.as_str());
    }
    if action == Some("credit.create") {
        return workspace.customer_confirmation_documents("credit.create", response);
    }
    if action == Some("cash.record") {
        for allocation in response["data"]["allocations"]
            .as_array()
            .ok_or("Internal storage error")?
        {
            if response["data"]["direction"] == "customer_refund" {
                if let Some(agreement) = allocation["target_id"]
                    .as_str()
                    .and_then(|id| workspace.bills.iter().find(|v| v.id == id))
                    .and_then(|bill| bill.supplement.as_ref())
                {
                    if let Some(id) = &agreement.document_id {
                        let doc = workspace
                            .documents
                            .iter()
                            .find(|v| &v.id == id)
                            .ok_or("Internal storage error")?;
                        if agreement.document_sha256.as_ref() != Some(&doc.sha256) {
                            return Err("Internal storage error".into());
                        }
                        ids.insert(id.as_str());
                    }
                }
            }
            if let Some(fee) = allocation["target_id"]
                .as_str()
                .and_then(|id| workspace.cancellation_target(id))
            {
                if let Some(id) = &fee.document_id {
                    let doc = workspace
                        .documents
                        .iter()
                        .find(|v| &v.id == id)
                        .ok_or("Internal storage error")?;
                    if fee.document_sha256.as_ref() != Some(&doc.sha256) {
                        return Err("Internal storage error".into());
                    }
                    ids.insert(id.as_str());
                }
            }
        }
        return ids
            .into_iter()
            .map(|id| {
                workspace
                    .documents
                    .iter()
                    .find(|v| v.id == id)
                    .ok_or_else(|| "Internal storage error".into())
            })
            .collect();
    }
    let mut load_ids = std::collections::BTreeSet::new();
    if action == Some("cancellation.review") {
        if response["data"]["status"] != "posted" {
            return Ok(vec![]);
        }
        if let Some(id) = response["data"]["document_id"].as_str() {
            ids.insert(id);
        }
    }
    if action == Some("carrier_credit.apply") {
        let application = workspace
            .carrier_credit_applications
            .iter()
            .find(|v| Some(v.id.as_str()) == response["data"]["id"].as_str())
            .ok_or("Internal storage error")?;
        load_ids.insert(application.source_load_id.as_str());
        load_ids.extend(application.allocations.iter().map(|v| v.load_id.as_str()));
        if let Some(id) = &application.document_id {
            let doc = workspace
                .documents
                .iter()
                .find(|v| &v.id == id)
                .ok_or("Internal storage error")?;
            if application.document_sha256.as_ref() != Some(&doc.sha256) {
                return Err("Internal storage error".into());
            }
            ids.insert(id.as_str());
        }
    }
    if action == Some("payable.review") {
        if response["data"]["status"] != "posted" {
            return Ok(vec![]);
        }
        if let Some(id) = response["data"]["document_id"].as_str() {
            ids.insert(id);
        }
    }
    if action == Some("charge.review") {
        if response["data"]["status"] != "approved" {
            return Ok(vec![]);
        }
        if let Some(id) = response["data"]["document_id"].as_str() {
            ids.insert(id);
        }
    } else if load_ids.is_empty() {
        load_ids.insert(
            response["data"]["load_id"]
                .as_str()
                .ok_or("Internal storage error")?,
        );
    }
    for load_id in load_ids {
        for fee in workspace
            .cancellation_fees
            .iter()
            .filter(|v| v.load_id == load_id && v.posted_at.is_some())
        {
            if let Some(id) = &fee.document_id {
                let doc = workspace
                    .documents
                    .iter()
                    .find(|v| &v.id == id)
                    .ok_or("Internal storage error")?;
                if fee.document_sha256.as_ref() != Some(&doc.sha256) {
                    return Err("Internal storage error".into());
                }
                ids.insert(id.as_str());
            }
        }
        let load = workspace
            .loads
            .iter()
            .find(|v| v.id == load_id)
            .ok_or("Internal storage error")?;
        if let Some(id) = &load.pod_document_id {
            ids.insert(id.as_str());
        }
        if action == Some("bill.issue") {
            if let Some(bill) = workspace
                .bills
                .iter()
                .find(|v| Some(v.id.as_str()) == response["data"]["id"].as_str())
            {
                if let Some(agreement) = &bill.supplement {
                    if let Some(id) = &agreement.document_id {
                        let doc = workspace
                            .documents
                            .iter()
                            .find(|v| &v.id == id)
                            .ok_or("Internal storage error")?;
                        if agreement.document_sha256.as_ref() != Some(&doc.sha256) {
                            return Err("Internal storage error".into());
                        }
                        ids.insert(id.as_str());
                    }
                }
            }
        }
        let side = if action == Some("bill.issue") {
            "sell"
        } else {
            "buy"
        };
        for charge in workspace
            .charges
            .iter()
            .filter(|v| v.load_id == load_id && v.side == side && v.status == "approved")
        {
            if let Some(id) = &charge.document_id {
                let document = workspace
                    .documents
                    .iter()
                    .find(|v| &v.id == id)
                    .ok_or("Internal storage error")?;
                if charge.document_sha256.as_ref() != Some(&document.sha256) {
                    return Err("Internal storage error".into());
                }
                ids.insert(id.as_str());
            }
        }
        if matches!(action, Some("settlement.create" | "carrier_credit.apply")) {
            for adjustment in workspace
                .carrier_adjustments
                .iter()
                .filter(|v| v.load_id == load_id && v.status == "posted")
            {
                if let Some(id) = &adjustment.document_id {
                    let doc = workspace
                        .documents
                        .iter()
                        .find(|v| &v.id == id)
                        .ok_or("Internal storage error")?;
                    if adjustment.document_sha256.as_ref() != Some(&doc.sha256) {
                        return Err("Internal storage error".into());
                    }
                    ids.insert(id.as_str());
                }
            }
        }
    }
    ids.into_iter()
        .map(|id| {
            workspace
                .documents
                .iter()
                .find(|v| v.id == id)
                .ok_or_else(|| "Internal storage error".into())
        })
        .collect()
}
fn verify_document(
    document: &crate::documents::Document,
    bytes: Option<&[u8]>,
) -> Result<(), String> {
    verify_document_bytes(&document.id, document.size_bytes, &document.sha256, bytes)
}
fn verify_document_bytes(
    id: &str,
    size_bytes: usize,
    sha256: &str,
    bytes: Option<&[u8]>,
) -> Result<(), String> {
    if !bytes.is_some_and(|v| v.len() == size_bytes && crate::documents::sha256(v) == sha256) {
        tracing::error!(
            document_id = id,
            "Supporting document integrity check failed before financial approval"
        );
        return Err("Internal storage error".into());
    }
    Ok(())
}
pub(crate) fn fingerprint(operation: &str, entity: &str, body: &Value) -> String {
    // A refreshed upstream snapshot must not change the identity of the
    // caller's import request after an otherwise successful response was lost.
    let logical = if operation == "sync.import" {
        json!({"order_id":body.get("order_id").unwrap_or(&body["order"]["id"]),"load":body["load"]})
    } else {
        body.clone()
    };
    format!(
        "{:x}",
        Sha256::digest(json!([operation, entity, logical]).to_string().as_bytes())
    )
}
fn apply(
    workspace: &mut Workspace,
    key: &str,
    fingerprint: &str,
    operation: &str,
    entity: &str,
    body: Value,
    principal: (&str, bool),
) -> Result<Value, String> {
    if let Some(receipt) = workspace.receipts.get(key) {
        let response = receipt.checked_response(fingerprint)?;
        return Ok(json!({"data":response,"replayed":true}));
    }
    let (actor, invoice_review_required) = principal;
    let response = match operation {
        "invoice.entry_review" => workspace.apply_invoice_review(operation, entity, body, actor)?,
        "invoice.approve" if invoice_review_required => {
            workspace.apply_invoice_review(operation, entity, body, actor)?
        }
        operation if operation.starts_with("operations.") => {
            workspace.apply_operations(operation, entity, body, actor)?
        }
        "movement.time.correct" => workspace.apply_movement(entity, body, actor)?,
        operation if operation.starts_with("itinerary.") => {
            workspace.apply_itinerary(operation, entity, body, actor)?
        }
        operation if operation.starts_with("contract.") => {
            workspace.apply_contract(operation, entity, body, actor)?
        }
        operation if operation.starts_with("share.") => {
            workspace.apply_share(operation, entity, body, actor)?
        }
        "commerce.reconcile" => crate::commerce::reconcile(workspace, entity, body, actor)?,
        "source.resolve" => workspace.resolve_source(entity, body, actor)?,
        operation if operation.starts_with("portal.") => {
            workspace.apply_portal(operation, entity, body, actor)?
        }
        operation if operation.starts_with("rate.") => {
            workspace.apply_rate_confirmation(operation, entity, body, actor)?
        }
        operation if operation.starts_with("quote.") => {
            workspace.apply_quote(operation, entity, body, actor)?
        }
        operation if operation.starts_with("cancellation.") => {
            workspace.apply_cancellation(operation, entity, body, actor)?
        }
        "document.review" => workspace.review_document(entity, body, actor)?,
        "pod.review" => workspace.review_external_pod(entity, body, actor)?,
        operation if operation.starts_with("carrier_credit.") => {
            workspace.apply_carrier_credit(operation, entity, body, actor)?
        }
        operation if operation.starts_with("payable.") => {
            workspace.apply_payable(operation, entity, body, actor)?
        }
        operation if operation.starts_with("charge.") => {
            workspace.apply_charge_operation(operation, entity, body, actor)?
        }
        operation if operation.starts_with("stop.") => {
            workspace.apply_stop_operation(operation, entity, body, actor)?
        }
        operation if operation.starts_with("tender.") => {
            workspace.apply_tender_operation(operation, entity, body, actor)?
        }
        operation
            if operation.starts_with("bill.")
                || operation.starts_with("cash.")
                || operation.starts_with("credit.") =>
        {
            workspace.apply_finance(operation, entity, body, actor)?
        }
        _ => workspace.apply(operation, entity, body)?,
    };
    workspace.receipts.insert(
        key.into(),
        Receipt {
            fingerprint: fingerprint.into(),
            response: response.clone(),
            stopped: None,
        },
    );
    Ok(json!({"data":response,"replayed":false}))
}
fn internal(error: impl std::fmt::Display) -> String {
    tracing::error!(%error,"Storage operation failed");
    "Internal storage error".into()
}

#[cfg(test)]
#[path = "admission_diagnostics_tests.rs"]
mod admission_diagnostics_tests;

#[cfg(test)]
#[path = "database_budget_tests.rs"]
mod database_budget_tests;

#[cfg(test)]
#[path = "schema_startup_tests.rs"]
mod schema_startup_tests;

#[cfg(test)]
#[path = "read_cache_store_tests.rs"]
mod read_cache_store_tests;

#[cfg(test)]
mod conditional_tests {
    use super::*;
    async fn overload_http_scenario(mut store: Store) {
        use crate::{
            api::{self, App},
            integrations::Integrations,
            read_admission::{ReadLimits, TenantReadTurns},
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        store.read_turns = Arc::new(TenantReadTurns::new(ReadLimits {
            pending: 2,
            tenant_pending: 1,
            timeout: Duration::from_secs(1),
        }));
        let router = api::router(App {
            pdf_renderer: Default::default(),
            identity: Arc::new(None),
            store: store.clone(),
            demo: true,
            integrations: Arc::new(Integrations::default()),
            keys: Arc::new(BTreeMap::from([
                ("owned-demo".into(), "demo".into()),
                ("owned-probe".into(), "probe".into()),
                ("owned-third".into(), "third".into()),
            ])),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("owned listener");
        let origin = format!("http://{}", listener.local_addr().expect("address"));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.expect("owned HTTP");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .expect("client");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let running = store.clone();
        let active = tokio::spawn(async move {
            running
                .read_project("demo", move |w| {
                    let _ = entered_tx.send(
                        w.loads
                            .iter()
                            .find(|l| l.id == "load_1")
                            .expect("load")
                            .version,
                    );
                    release_rx
                        .recv_timeout(Duration::from_secs(10))
                        .expect("release owned job");
                    Ok(json!({"original":true}))
                })
                .await
        });
        assert_eq!(
            entered_rx.await.expect("snapshot before held projection"),
            1
        );
        let mut table_lock = if let Some(pool) = &store.pool {
            let mut tx = pool.begin().await.expect("observer lock");
            sqlx::query("LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE")
                .execute(&mut *tx)
                .await
                .expect("workspace unavailable to SQL reads");
            Some(tx)
        } else {
            None
        };
        let request = |token: &str| {
            client
                .get(format!("{origin}/v1/workspace"))
                .bearer_auth(token)
                .send()
        };
        let first = tokio::spawn(request("owned-demo"));
        async fn pending(store: &Store, expected: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while store.read_turns.pending_count() != expected {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("real HTTP waiter registered");
        }
        pending(&store, 1).await;
        let second = tokio::spawn(request("owned-probe"));
        pending(&store, 2).await;
        let attempted = Arc::new(AtomicBool::new(false));
        let projection = attempted.clone();
        assert_eq!(
            store
                .read_project("third", move |_| {
                    projection.store(true, Ordering::SeqCst);
                    Ok(Value::Null)
                })
                .await
                .expect_err("global budget before projection"),
            READ_BUSY
        );
        for token in ["owned-demo", "owned-third"] {
            let response = request(token).await.expect("overload is an HTTP response");
            assert_eq!(response.status(), 503);
            assert_eq!(response.headers()["retry-after"], "1");
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(
                response.json::<Value>().await.expect("busy JSON")["error"]["code"],
                "read_capacity_exceeded"
            );
        }
        assert!(!attempted.load(Ordering::SeqCst));
        let health = client
            .get(format!("{origin}/health"))
            .send()
            .await
            .expect("health");
        assert_eq!(health.status(), 200);
        for waiter in [first, second] {
            let response = tokio::time::timeout(Duration::from_secs(3), waiter)
                .await
                .expect("finite wait before SQL")
                .expect("request task")
                .expect("HTTP");
            assert_eq!(response.status(), 503);
            assert_eq!(response.headers()["retry-after"], "1");
        }
        assert_eq!(store.read_turns.pending_count(), 0);
        assert_eq!(store.read_slots.available_permits(), 0);
        active.abort();
        let _ = active.await;
        assert_eq!(
            store.read_slots.available_permits(),
            0,
            "caller abort cannot release active work"
        );
        if let Some(tx) = table_lock.take() {
            tx.rollback().await.expect("release table lock");
        }
        let changed = client.post(format!("{origin}/v1/loads/load_1/commands"))
            .bearer_auth("owned-demo").header("idempotency-key", "owned-during-overload")
            .json(&json!({"action":"location","expected_version":1,"location":"Actual observation while reads are saturated"}))
            .send().await.expect("write independently of read admission");
        assert_eq!(
            changed.status(),
            200,
            "{}",
            changed.text().await.expect("write body")
        );
        let commitment = client
            .post(format!("{origin}/v1/loads/load_5/commands"))
            .bearer_auth("owned-demo")
            .header("idempotency-key", "owned-commitment-held")
            .json(&json!({"action":"tender","expected_version":1,"carrier_id":"ca2"}))
            .send()
            .await
            .expect("commitment read still bounded");
        assert_eq!(commitment.status(), 503);
        assert_eq!(
            commitment.json::<Value>().await.expect("busy commitment")["error"]["code"],
            "read_capacity_exceeded"
        );
        release_tx.send(()).expect("finish actual job");
        tokio::time::timeout(Duration::from_secs(3), async {
            while store.read_slots.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("job completion releases slot");
        let recovered = request("owned-demo").await.expect("retry");
        assert_eq!(recovered.status(), 200);
        let workspace: Value = recovered.json().await.expect("workspace");
        let load = workspace["loads"]
            .as_array()
            .expect("loads")
            .iter()
            .find(|l| l["id"] == "load_1")
            .expect("current load");
        assert_eq!(load["version"], 2);
        assert_eq!(
            load["location"],
            "Actual observation while reads are saturated"
        );
        assert_eq!(store.read("probe").await.expect("probe").loads.len(), 0);
        assert_eq!(store.read_turns.pending_count(), 0);
        assert_eq!(
            store.tenant_ids().await.expect("tenant inventory"),
            vec!["demo".to_string()],
            "rejected and expired reads create no tenants"
        );
        server.abort();
        let _ = server.await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn bounded_read_admission_returns_retryable_http_without_starting_rejected_work_or_blocking_writes(
    ) {
        overload_http_scenario(Store::new(None).await.expect("memory")).await;
    }
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires disposable TEST_DATABASE_URL"]
    async fn postgres_overload_expires_before_locked_workspace_sql_and_recovers_without_losing_capacity(
    ) {
        let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("read_overload_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("URL");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let store = Store::new(Some(url.as_str()))
            .await
            .expect("database store");
        overload_http_scenario(store.clone()).await;
        let reconnected = Store::new(Some(url.as_str())).await.expect("reconnected");
        let w = reconnected
            .read("demo")
            .await
            .expect("retained current state");
        assert_eq!(
            w.loads
                .iter()
                .find(|l| l.id == "load_1")
                .expect("load")
                .version,
            2
        );
        assert_eq!(w.receipts.len(), 1, "only actual write consumes a receipt");
        reconnected.pool.as_ref().expect("pool").close().await;
        store.pool.as_ref().expect("pool").close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .expect("cleanup");
    }
    #[test]
    fn database_pool_configuration_is_bounded_and_rejects_ambiguous_values() {
        assert_eq!(database_max_connections(None).expect("default"), 2);
        for n in [1, 4, 64] {
            assert_eq!(
                database_max_connections(Some(&n.to_string())).expect("valid"),
                n
            );
        }
        for raw in [
            "",
            "0",
            "65",
            "-1",
            "+4",
            " 4",
            "4 ",
            "ten",
            "1.5",
            "4294967296",
        ] {
            assert!(
                database_max_connections(Some(raw)).is_err(),
                "accepted {raw}"
            );
        }
    }
    #[test]
    fn read_budget_is_bounded_and_rejects_ambiguous_configuration() {
        assert_eq!(read_concurrency(None).expect("default"), 2);
        for n in [1, 2, 64] {
            assert_eq!(read_concurrency(Some(&n.to_string())).expect("valid"), n);
        }
        for raw in [
            "",
            "0",
            "65",
            "-1",
            "+4",
            " 4",
            "4 ",
            "unlimited",
            "1.5",
            "4294967296",
        ] {
            assert!(read_concurrency(Some(raw)).is_err());
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_read_jobs_retain_the_budget_and_leave_async_health_responsive() {
        let mut store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_store = store.clone();
        let first = tokio::spawn(async move {
            first_store
                .read_project("demo", move |_| {
                    let _ = entered_tx.send(());
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("test release");
                    Ok(json!({"first":true}))
                })
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), entered_rx)
            .await
            .expect("async runtime remains scheduled")
            .expect("job entered");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(500), store.healthy())
                .await
                .expect("health remains responsive")
        );
        first.abort();
        let _ = first.await;
        assert_eq!(
            store.read_slots.available_permits(),
            0,
            "cancelling an awaiter cannot release a running job's permit"
        );
        let waiting_store = store.clone();
        let mut waiting = tokio::spawn(async move {
            waiting_store
                .read_project("demo", |w| Ok(json!({"loads":w.loads.len()})))
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiting)
                .await
                .is_err(),
            "second snapshot waits for the real job"
        );
        release_tx.send(()).expect("release");
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("queue resumes")
            .expect("task")
            .expect("read");
        assert_eq!(
            serde_json::from_slice::<Value>(&response).expect("JSON")["loads"],
            8
        );
        assert_eq!(store.read_slots.available_permits(), 1);
    }
    #[tokio::test]
    async fn queued_reads_of_one_tenant_cannot_fill_admission_ahead_of_a_probe() {
        use std::{
            future::Future,
            pin::Pin,
            task::{Context, Waker},
        };
        fn pending<F: Future>(future: Pin<&mut F>) {
            assert!(future
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending());
        }
        let mut store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let running_store = store.clone();
        let running = tokio::spawn(async move {
            running_store
                .read_project("demo", move |_| {
                    let _ = entered_tx.send(());
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release");
                    Ok(json!({"running":true}))
                })
                .await
        });
        entered_rx.await.expect("first real projection entered");
        let mut head = Box::pin(store.read_project("demo", |w| Ok(json!({"loads":w.loads.len()}))));
        let mut tail = Box::pin(store.read_project("demo", |_| Ok(json!({"tail":true}))));
        let mut probe =
            Box::pin(store.read_project("probe", |w| Ok(json!({"loads":w.loads.len()}))));
        pending(head.as_mut());
        pending(tail.as_mut());
        pending(probe.as_mut());
        release_tx.send(()).expect("release running projection");
        running
            .await
            .expect("running task")
            .expect("running response");
        let head_bytes = head.await.expect("head response");
        assert_eq!(
            serde_json::from_slice::<Value>(&head_bytes).expect("head JSON")["loads"],
            8
        );
        // The tail arrived before the probe but must join global admission
        // behind it. With request FIFO, tail reserves the only slot here and
        // the unpolled tail would prevent the probe from completing.
        pending(tail.as_mut());
        let probe_bytes = tokio::time::timeout(std::time::Duration::from_secs(2), probe)
            .await
            .expect("other tenant proceeds before busy tail")
            .expect("probe response");
        assert_eq!(
            serde_json::from_slice::<Value>(&probe_bytes).expect("probe JSON")["loads"],
            0
        );
        tail.await.expect("busy tail still completes");
        assert_eq!(store.read_slots.available_permits(), 1);
    }
    #[tokio::test]
    async fn projected_reads_keep_one_snapshot_while_later_mutations_recheck_the_current_version() {
        let store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let snapshot_store = store.clone();
        let snapshot = tokio::spawn(async move {
            snapshot_store
                .read_project("demo", move |w| {
                    let _ = entered_tx.send(());
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("release");
                    Ok(json!({"version":w.loads[0].version,"location":w.loads[0].location}))
                })
                .await
        });
        entered_rx.await.expect("snapshot captured");
        store.mutate("demo","new-observation","load.command","load_1",json!({"action":"location","expected_version":1,"location":"Actual later observation"})).await.expect("concurrent source update");
        release_tx.send(()).expect("release");
        let bytes = snapshot.await.expect("task").expect("snapshot");
        let old: Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(old["version"], 1);
        assert_ne!(old["location"], "Actual later observation");
        assert!(store
            .mutate(
                "demo",
                "stale-review",
                "load.command",
                "load_1",
                json!({"action":"location","expected_version":1,"location":"Stale overwrite"})
            )
            .await
            .is_err());
        assert_eq!(
            store.read("demo").await.expect("current").loads[0].location,
            "Actual later observation"
        );
    }
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires disposable TEST_DATABASE_URL"]
    async fn postgres_projected_reads_release_the_database_before_projection_and_recover_after_decode_errors(
    ) {
        use std::sync::atomic::{AtomicBool, Ordering};
        async fn idle(store: &Store) {
            let pool = store.pool.as_ref().expect("database pool");
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
            while pool.num_idle() < pool.size() as usize {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "read connection not returned"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("projected_read_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        let mut store = Store::new(Some(url.as_str())).await.expect("store");
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        let observer = PgPool::connect(url.as_str()).await.expect("observer");
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let reading = store.clone();
        let request = tokio::spawn(async move {
            reading
                .read_project("demo", move |w| {
                    let _ = entered_tx.send(w.loads[0].version);
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .expect("release projection");
                    Ok(json!({"version":w.loads[0].version}))
                })
                .await
        });
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
                .await
                .expect("current-thread runtime drives read/commit IO")
                .expect("projection"),
            1
        );
        idle(&store).await;
        let mut format = observer.begin().await.expect("format observer");
        let version: i32 = sqlx::query_scalar(
            "SELECT version FROM freight_storage_format WHERE singleton FOR UPDATE NOWAIT",
        )
        .fetch_one(&mut *format)
        .await
        .expect("format lock released before projection");
        assert_eq!(version, STORAGE_FORMAT);
        format.rollback().await.expect("release observer");
        assert!(store.healthy().await);
        request.abort();
        let _ = request.await;
        assert_eq!(
            store.read_slots.available_permits(),
            0,
            "aborted caller cannot release running projection's slot"
        );
        store.mutate("demo","later-source","load.command","load_1",
            json!({"action":"location","expected_version":1,"location":"Observation after snapshot commit"}))
            .await.expect("writer proceeds while projection retains old snapshot");
        release_tx.send(()).expect("release");
        assert_eq!(
            store.read("demo").await.expect("queue resumes").loads[0].version,
            2
        );

        let ran = Arc::new(AtomicBool::new(false));
        let mut upgrade = observer.begin().await.expect("atomic future release");
        sqlx::query("ALTER TABLE freight_workspaces DISABLE TRIGGER freight_storage_format_guard")
            .execute(&mut *upgrade)
            .await
            .expect("owned future-writer simulation");
        sqlx::query("UPDATE freight_storage_format SET version=$1 WHERE singleton")
            .bind(STORAGE_FORMAT + 1)
            .execute(&mut *upgrade)
            .await
            .expect("future storage fence");
        sqlx::query("UPDATE freight_workspaces SET data=jsonb_set(jsonb_set(data,'{_freight_format}',to_jsonb($1::integer)),'{frame,roots,_freight_format,scalar}',to_jsonb($1::integer)) WHERE tenant_id='demo'")
            .bind(STORAGE_FORMAT+1)
            .execute(&mut *upgrade).await.expect("future snapshot marker");
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *upgrade)
            .await
            .expect("flush owned deferred events before guard DDL");
        sqlx::query("ALTER TABLE freight_workspaces ENABLE TRIGGER freight_storage_format_guard")
            .execute(&mut *upgrade)
            .await
            .expect("restore deployed guard");
        upgrade.commit().await.expect("future release commit");
        let projected = ran.clone();
        let error = store
            .read_project("demo", move |_| {
                projected.store(true, Ordering::SeqCst);
                Ok(json!({}))
            })
            .await
            .expect_err("future snapshot cannot be projected");
        assert_eq!(error, INCOMPATIBLE_STORAGE);
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(store.read_slots.available_permits(), 1);
        idle(&store).await;
        let mut restore = observer.begin().await.expect("atomic restoration");
        sqlx::query("UPDATE freight_storage_format SET version=$1 WHERE singleton")
            .bind(STORAGE_FORMAT)
            .execute(&mut *restore)
            .await
            .expect("restore current storage fence");
        sqlx::query("UPDATE freight_workspaces SET data=jsonb_set(jsonb_set(data,'{_freight_format}',to_jsonb($1::integer)),'{frame,roots,_freight_format,scalar}',to_jsonb($1::integer)) WHERE tenant_id='demo'")
            .bind(STORAGE_FORMAT)
            .execute(&mut *restore).await.expect("restore current metadata");
        restore
            .commit()
            .await
            .expect("current format with unsupported business data");
        let mut source = crate::storage_fixture::source(&observer, "demo")
            .await
            .expect("original business source");
        source["future_financial_obligation"] = json!({"amount_cents":10001});
        crate::storage_fixture::put(&observer, "demo", &source)
            .await
            .expect("unknown canonical business root");
        let projected = ran.clone();
        assert!(store
            .read_project("demo", move |_| {
                projected.store(true, Ordering::SeqCst);
                Ok(json!({}))
            })
            .await
            .is_err());
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(store.read_slots.available_permits(), 1);
        idle(&store).await;
        source
            .as_object_mut()
            .expect("source")
            .remove("future_financial_obligation");
        crate::storage_fixture::put(&observer, "demo", &source)
            .await
            .expect("restore original business roots");
        let bytes = store
            .read_project("demo", |w| {
                Ok(json!({"version":w.loads[0].version,"location":w.loads[0].location}))
            })
            .await
            .expect("slot, connection and snapshot remain usable");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).expect("JSON"),
            json!({"version":2,"location":"Observation after snapshot commit"})
        );
        store.pool.as_ref().expect("pool").close().await;
        observer.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .expect("cleanup");
        admin.close().await;
    }
    #[tokio::test]
    #[ignore = "requires disposable TEST_DATABASE_URL"]
    async fn postgres_cancelled_admitted_reads_keep_their_slot_until_the_query_and_decode_finish() {
        let base = std::env::var("TEST_DATABASE_URL").expect("disposable database");
        let admin = PgPool::connect(&base).await.expect("admin");
        let schema = format!("read_budget_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .expect("schema");
        let app = format!("read-budget-{}", uuid::Uuid::new_v4().simple());
        let mut url = reqwest::Url::parse(&base).expect("url");
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"))
            .append_pair("application_name", &app);
        let mut store = Store::new(Some(url.as_str())).await.expect("store");
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        let pool = PgPool::connect(url.as_str()).await.expect("observer");
        let mut blocker = pool.begin().await.expect("blocker");
        sqlx::query("LOCK TABLE freight_workspaces IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocker)
            .await
            .expect("hold reads");
        let reading = store.clone();
        let first = tokio::spawn(async move { reading.read("demo").await });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let waiting:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock' AND query LIKE 'SELECT data::text%'").bind(&app).fetch_one(&pool).await.expect("active read");
            if waiting > 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "query did not reach the table lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        first.abort();
        let _ = first.await;
        assert_eq!(
            store.read_slots.available_permits(),
            0,
            "aborting the caller must not abandon an admitted SQL query's budget"
        );
        let reading = store.clone();
        let mut second = tokio::spawn(async move { reading.read("demo").await });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut second)
                .await
                .is_err()
        );
        blocker.commit().await.expect("release");
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), second)
            .await
            .expect("queue resumes")
            .expect("task")
            .expect("workspace");
        assert_eq!(result.loads.len(), 8);
        assert_eq!(store.read_slots.available_permits(), 1);
        drop(store);
        pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
            .execute(&admin)
            .await
            .expect("clean schema");
        admin.close().await;
    }
    #[tokio::test]
    async fn declined_mutation_discards_staged_changes_and_does_not_create_a_tenant() {
        let store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        let before = json!(store.read("demo").await.expect("before"));
        for tenant in ["demo", "unknown"] {
            assert!(store
                .update_if(tenant, |w| {
                    w.loads.clear();
                    w.events.clear();
                    Ok(None)
                })
                .await
                .expect("declined")
                .is_none());
        }
        assert_eq!(json!(store.read("demo").await.expect("after")), before);
        assert_eq!(store.tenant_ids().await.expect("tenants"), vec!["demo"]);
    }
    #[tokio::test]
    async fn concurrent_memory_expiry_records_one_exception_and_one_event() {
        let store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        store
            .update("demo", |w| {
                let l = &mut w.loads[2];
                l.status = domain::Status::Tendered;
                l.exception = None;
                l.tender_expires_at = Some(chrono::Utc::now() - chrono::Duration::minutes(1));
                Ok(Value::Null)
            })
            .await
            .expect("due");
        let (a, b) = tokio::join!(store.expire_tenders("demo"), store.expire_tenders("demo"));
        assert_eq!(a.expect("first") + b.expect("second"), 1);
        let after = store.read("demo").await.expect("after");
        assert_eq!(after.loads[2].version, 2);
        assert_eq!(after.loads[2].status, domain::Status::Tendered);
        assert_eq!(
            after
                .events
                .iter()
                .filter(|e| e.kind == "tender.overdue")
                .count(),
            1
        );
        assert_eq!(store.expire_tenders("unknown").await.expect("absent"), 0);
        assert!(!store
            .tenant_ids()
            .await
            .expect("tenants")
            .contains(&"unknown".into()));
    }
}

#[cfg(test)]
mod history_job_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abandoned_history_encoding_retains_its_admission_until_the_worker_finishes() {
        let mut store = Store::new(None).await.expect("store");
        store.seed_demo().await.expect("seed");
        store.read_slots = Arc::new(Semaphore::new(1));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let owned = store.clone();
        let caller = tokio::spawn(async move {
            owned
                .history_project(
                    "demo",
                    serde_json::from_value(json!({"kind":"events","limit":2})).expect("query"),
                    move |value| {
                        started.send(()).expect("observer");
                        wait.recv().expect("controlled release");
                        serde_json::to_vec(&value).map_err(|_| "Internal storage error".to_string())
                    },
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), ready)
            .await
            .expect("worker started")
            .expect("signal");
        caller.abort();
        assert!(caller.await.expect_err("abandoned caller").is_cancelled());
        assert_eq!(store.read_slots.available_permits(), 0);
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(40),
            store.history_page(
                "demo",
                serde_json::from_value(json!({"kind":"events"})).expect("query")
            )
        )
        .await
        .is_err());
        assert_eq!(store.read_slots.available_permits(), 0);
        release.send(()).expect("release actual worker");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while store.read_slots.available_permits() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker releases slot");
        store
            .history_page(
                "demo",
                serde_json::from_value(json!({"kind":"events"})).expect("query"),
            )
            .await
            .expect("next reader progresses");
    }
}

#[cfg(test)]
#[path = "shared_save_tests.rs"]
mod shared_save_tests;

#[cfg(test)]
#[path = "invoice_review_tests.rs"]
mod invoice_review_tests;

#[cfg(test)]
#[path = "mutation_execution_tests.rs"]
mod mutation_execution_tests;

#[cfg(test)]
#[path = "write_contention_tests.rs"]
mod write_contention_tests;

#[cfg(test)]
#[path = "workspace_admission_tests.rs"]
mod workspace_admission_tests;

#[cfg(test)]
#[path = "catalog_isolation_tests.rs"]
mod catalog_isolation_tests;

#[path = "console_store.rs"]
mod console_store;
