//! A separate finite lane for indexed catalog pages, never whole snapshots.
use crate::read_admission::{ReadLimits, TenantReadTurns};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct CatalogAdmission {
    slots: Arc<Semaphore>,
    turns: TenantReadTurns,
}
impl CatalogAdmission {
    pub(crate) fn diagnostics(&self) -> crate::admission_diagnostics::ReadLane {
        crate::admission_diagnostics::ReadLane::new(self.turns.diagnostics(), &self.slots)
    }
    pub(crate) fn new(limits: ReadLimits) -> Self {
        Self {
            slots: Arc::new(Semaphore::new(1)),
            turns: TenantReadTurns::new(limits),
        }
    }
    pub(crate) async fn acquire(&self, tenant: &str) -> Result<Arc<OwnedSemaphorePermit>, String> {
        self.turns
            .acquire(tenant, self.slots.clone())
            .await
            .map(Arc::new)
    }
    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.turns.pending_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn catalog_queue_is_finite_and_cancelled_waiter_never_releases_active_work() {
        let admission = Arc::new(CatalogAdmission::new(ReadLimits {
            pending: 1,
            tenant_pending: 1,
            timeout: Duration::from_secs(2),
        }));
        let held = admission.acquire("active").await.expect("active");
        let owner = held.clone();
        drop(held);
        let other = admission.clone();
        let waiter = tokio::spawn(async move { other.acquire("waiting").await });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while admission.pending() == 0 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        assert_eq!(
            admission.acquire("refused").await.err().as_deref(),
            Some(crate::read_admission::READ_BUSY)
        );
        waiter.abort();
        assert!(waiter.await.expect_err("cancelled").is_cancelled());
        assert_eq!(admission.pending(), 0);
        assert_eq!(
            admission.slots.available_permits(),
            0,
            "actual owner still retains capacity"
        );
        drop(owner);
        assert!(admission.acquire("later").await.is_ok());
        assert_eq!(admission.slots.available_permits(), 1);
    }
}
