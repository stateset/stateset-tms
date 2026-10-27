//! Shared admission for whole-workspace materialization and preparation.
//!
//! This limits jobs, not bytes or all memory retained by callers. A configured
//! profile queues before SQL; original receipts bypass processing admission.
use crate::read_admission::{ReadLimits, TenantReadTurns};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const WORKSPACE_BUSY: &str =
    "Workspace processing is busy. Retry the original request shortly.";
pub(crate) type WorkLease = Option<Arc<OwnedSemaphorePermit>>;

pub(crate) fn concurrency(raw: Option<&str>) -> Result<Option<usize>, String> {
    let Some(raw) = raw else { return Ok(None) };
    raw.parse::<usize>()
        .ok()
        .filter(|n| {
            !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) && (1..=64).contains(n)
        })
        .map(Some)
        .ok_or_else(|| "FREIGHT_WORKSPACE_CONCURRENCY must be an integer from 1 to 64".into())
}

pub(crate) struct WorkspaceAdmission {
    work: Arc<Semaphore>,
    turns: TenantReadTurns,
}
impl WorkspaceAdmission {
    pub(crate) fn diagnostics(&self) -> crate::admission_diagnostics::ReadLane {
        crate::admission_diagnostics::ReadLane::new(self.turns.diagnostics(), &self.work)
    }
    pub(crate) fn new(jobs: usize, limits: ReadLimits) -> Self {
        Self {
            work: Arc::new(Semaphore::new(jobs)),
            turns: TenantReadTurns::new(limits),
        }
    }
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.turns.pending_count()
    }
    pub(crate) async fn acquire(&self, tenant: &str) -> Result<Arc<OwnedSemaphorePermit>, String> {
        // Queue before any database connection or tenant lock. New heavy
        // transactions use a short lock wait so a busy external tenant cannot
        // monopolize processing admission. No automatic write retry is added.
        self.turns
            .acquire(tenant, self.work.clone())
            .await
            .map(Arc::new)
            .map_err(|_| WORKSPACE_BUSY.into())
    }
}

/// Own the format fence, work lease through real
/// COMMIT/ROLLBACK, even if the caller or a blocking worker abandons its awaiter.
pub(crate) struct WorkspaceTransaction {
    transaction: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    work: WorkLease,
    runtime: tokio::runtime::Handle,
}
impl WorkspaceTransaction {
    pub(crate) fn new(
        transaction: sqlx::Transaction<'static, sqlx::Postgres>,
        work: WorkLease,
    ) -> Self {
        Self {
            transaction: Some(transaction),
            work,
            runtime: tokio::runtime::Handle::current(),
        }
    }
    pub(crate) async fn commit(mut self) -> Result<(), sqlx::Error> {
        let transaction = self
            .transaction
            .take()
            .expect("active workspace transaction");
        let work = self.work.take();
        self.runtime
            .spawn(async move {
                let _work = work;
                transaction.commit().await
            })
            .await
            .map_err(|_| sqlx::Error::Protocol("Workspace commit task was interrupted".into()))?
    }
}
impl std::ops::Deref for WorkspaceTransaction {
    type Target = sqlx::PgConnection;
    fn deref(&self) -> &Self::Target {
        self.transaction
            .as_ref()
            .expect("active workspace transaction")
    }
}
impl std::ops::DerefMut for WorkspaceTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.transaction
            .as_mut()
            .expect("active workspace transaction")
    }
}
impl Drop for WorkspaceTransaction {
    fn drop(&mut self) {
        if let Some(transaction) = self.transaction.take() {
            let work = self.work.take();
            self.runtime.spawn(async move {
                let _work = work;
                let _ = transaction.rollback().await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    fn admission() -> WorkspaceAdmission {
        WorkspaceAdmission::new(
            1,
            ReadLimits {
                timeout: Duration::from_millis(10),
                ..ReadLimits::default()
            },
        )
    }
    #[tokio::test]
    async fn shared_work_outlives_one_clone_and_recovers_after_last_owner() {
        let admission = admission();
        let job = admission.acquire("read").await.expect("job");
        let worker = job.clone();
        drop(job);
        assert_eq!(
            admission.acquire("write").await.err().as_deref(),
            Some(WORKSPACE_BUSY)
        );
        drop(worker);
        assert!(admission.acquire("write").await.is_ok());
    }
    #[tokio::test]
    async fn cancelled_queue_entry_releases_reservation_for_another_tenant() {
        let admission = Arc::new(admission());
        let first = admission.acquire("one").await.expect("one");
        let other = admission.clone();
        let waiter = tokio::spawn(async move { other.acquire("cancelled").await });
        tokio::task::yield_now().await;
        waiter.abort();
        assert!(waiter.await.expect_err("cancelled").is_cancelled());
        drop(first);
        assert!(admission.acquire("later").await.is_ok());
        assert_eq!(admission.work.available_permits(), 1);
    }
    #[test]
    fn explicit_job_limit_refuses_ambiguous_or_unbounded_configuration() {
        assert_eq!(concurrency(None).expect("legacy"), None);
        assert_eq!(concurrency(Some("1")).expect("one"), Some(1));
        assert_eq!(concurrency(Some("64")).expect("max"), Some(64));
        for value in [
            "",
            "0",
            "65",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.0",
            "18446744073709551616",
        ] {
            assert!(concurrency(Some(value)).is_err(), "{value}");
        }
    }
}
