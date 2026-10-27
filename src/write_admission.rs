//! Serialize a tenant's new writes before they can occupy database connections.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const WRITE_BUSY: &str = "Workspace changes are busy. Retry the original request shortly.";

#[derive(Clone, Copy)]
pub(crate) struct WriteLimits {
    pub(crate) total: usize,
    pub(crate) tenant: usize,
}
impl Default for WriteLimits {
    fn default() -> Self {
        Self {
            total: 64,
            tenant: 16,
        }
    }
}
impl WriteLimits {
    fn parse(total: Option<&str>, tenant: Option<&str>) -> Result<Self, String> {
        fn integer(
            raw: Option<&str>,
            default: usize,
            max: usize,
            name: &str,
        ) -> Result<usize, String> {
            let Some(raw) = raw else { return Ok(default) };
            raw.parse::<usize>()
                .ok()
                .filter(|n| {
                    !raw.is_empty()
                        && raw.bytes().all(|b| b.is_ascii_digit())
                        && (1..=max).contains(n)
                })
                .ok_or_else(|| format!("{name} must be an integer from 1 to {max}"))
        }
        let total = integer(total, 64, 4096, "FREIGHT_WRITE_MAX_IN_FLIGHT")?;
        let tenant = integer(
            tenant,
            16.min(total),
            total,
            "FREIGHT_WRITE_TENANT_MAX_IN_FLIGHT",
        )?;
        Ok(Self { total, tenant })
    }
    pub(crate) fn from_env() -> Result<Self, String> {
        fn value(name: &str) -> Result<Option<String>, String> {
            match std::env::var(name) {
                Ok(raw) => Ok(Some(raw)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(_) => Err(format!("Invalid {name}")),
            }
        }
        Self::parse(
            value("FREIGHT_WRITE_MAX_IN_FLIGHT")?.as_deref(),
            value("FREIGHT_WRITE_TENANT_MAX_IN_FLIGHT")?.as_deref(),
        )
    }
}

#[derive(Default)]
struct Registry {
    requests: usize,
    tenants: BTreeMap<String, Tenant>,
}
struct Tenant {
    requests: usize,
    turn: Arc<Semaphore>,
}
pub(crate) struct WriteTurns {
    registry: Arc<Mutex<Registry>>,
    limits: WriteLimits,
    timeout: Duration,
}
struct Reservation {
    registry: Arc<Mutex<Registry>>,
    tenant: String,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        registry.requests -= 1;
        let tenant = registry
            .tenants
            .get_mut(&self.tenant)
            .expect("registered write request");
        tenant.requests -= 1;
        if tenant.requests == 0 {
            registry.tenants.remove(&self.tenant);
        }
    }
}
pub(crate) struct WritePermit {
    _permit: OwnedSemaphorePermit,
    _reservation: Reservation,
}
pub(crate) struct WriteWait {
    turn: Arc<Semaphore>,
    reservation: Reservation,
}
pub(crate) enum Admission {
    Active(WritePermit),
    Waiting(WriteWait),
    Full,
}
impl Admission {
    pub(crate) async fn wait(self, deadline: tokio::time::Instant) -> Result<WritePermit, String> {
        match self {
            Self::Active(permit) => Ok(permit),
            Self::Full => {
                tracing::warn!(reason = "queue_full", "freight_write_admission_rejected");
                Err(WRITE_BUSY.into())
            }
            Self::Waiting(wait) => {
                let permit = tokio::time::timeout_at(deadline, wait.turn.acquire_owned())
                    .await
                    .map_err(|_| {
                        tracing::warn!(
                            reason = "queue_timeout",
                            "freight_write_admission_rejected"
                        );
                        WRITE_BUSY.to_string()
                    })?
                    .map_err(|_| "Write admission is unavailable".to_string())?;
                Ok(WritePermit {
                    _permit: permit,
                    _reservation: wait.reservation,
                })
            }
        }
    }
}
impl WriteTurns {
    #[cfg(test)]
    pub(crate) fn with_registry_locked<T>(&self, observe: impl FnOnce() -> T) -> T {
        let _guard = self.registry.lock().expect("owned test registry");
        observe()
    }
    pub(crate) fn diagnostics(&self) -> crate::admission_diagnostics::WriteSnapshot {
        crate::admission_diagnostics::WriteSnapshot {
            requests: self
                .registry
                .try_lock()
                .ok()
                .map(|registry| registry.requests),
            limit: self.limits.total,
            tenant_limit: self.limits.tenant,
        }
    }
    pub(crate) fn new(limits: WriteLimits, timeout: Duration) -> Self {
        Self {
            registry: Arc::default(),
            limits,
            timeout,
        }
    }
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::now() + self.timeout
    }
    pub(crate) fn request(&self, tenant: &str) -> Result<Admission, String> {
        let mut registry = self
            .registry
            .lock()
            .map_err(|_| "Write admission registry is unavailable".to_string())?;
        if registry.requests >= self.limits.total
            || registry
                .tenants
                .get(tenant)
                .is_some_and(|q| q.requests >= self.limits.tenant)
        {
            return Ok(Admission::Full);
        }
        registry.requests += 1;
        let entry = registry
            .tenants
            .entry(tenant.into())
            .or_insert_with(|| Tenant {
                requests: 0,
                turn: Arc::new(Semaphore::new(1)),
            });
        entry.requests += 1;
        let turn = entry.turn.clone();
        let reservation = Reservation {
            registry: self.registry.clone(),
            tenant: tenant.into(),
        };
        match turn.clone().try_acquire_owned() {
            Ok(permit) => Ok(Admission::Active(WritePermit {
                _permit: permit,
                _reservation: reservation,
            })),
            Err(_) => Ok(Admission::Waiting(WriteWait { turn, reservation })),
        }
    }
    #[cfg(test)]
    pub(crate) fn requests(&self) -> usize {
        self.registry.lock().expect("test registry").requests
    }
}

/// Rollback owns admission too, including when a caller or blocking worker drops
/// an unfinished transaction. Never release its turn before actual rollback.
pub(crate) struct WriteTransaction {
    transaction: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    permit: Option<WritePermit>,
    runtime: tokio::runtime::Handle,
    work: crate::workspace_admission::WorkLease,
    additional_work: Vec<Arc<OwnedSemaphorePermit>>,
}
impl WriteTransaction {
    pub(crate) fn new(
        transaction: sqlx::Transaction<'static, sqlx::Postgres>,
        permit: WritePermit,
    ) -> Self {
        Self {
            transaction: Some(transaction),
            permit: Some(permit),
            runtime: tokio::runtime::Handle::current(),
            work: None,
            additional_work: Vec::new(),
        }
    }
    pub(crate) fn hold_work(&mut self, work: crate::workspace_admission::WorkLease) {
        self.work = work;
    }
    pub(crate) fn hold_additional_work(&mut self, lease: Arc<OwnedSemaphorePermit>) {
        self.additional_work.push(lease);
    }
    pub(crate) fn work(&self) -> crate::workspace_admission::WorkLease {
        self.work.clone()
    }
    pub(crate) async fn commit(mut self) -> Result<(), sqlx::Error> {
        let transaction = self.transaction.take().expect("active write transaction");
        let permit = self.permit.take();
        let work = self.work.take();
        let additional_work = std::mem::take(&mut self.additional_work);
        self.runtime
            .spawn(async move {
                let _permit = permit;
                let _work = work;
                let _additional_work = additional_work;
                transaction.commit().await
            })
            .await
            .map_err(|_| sqlx::Error::Protocol("Write commit task was interrupted".into()))?
    }
    pub(crate) async fn rollback(mut self) -> Result<(), sqlx::Error> {
        let transaction = self.transaction.take().expect("active write transaction");
        let permit = self.permit.take();
        let work = self.work.take();
        let additional_work = std::mem::take(&mut self.additional_work);
        self.runtime
            .spawn(async move {
                let _permit = permit;
                let _work = work;
                let _additional_work = additional_work;
                transaction.rollback().await
            })
            .await
            .map_err(|_| sqlx::Error::Protocol("Write rollback task was interrupted".into()))?
    }
}
impl std::ops::Deref for WriteTransaction {
    type Target = sqlx::PgConnection;
    fn deref(&self) -> &Self::Target {
        self.transaction.as_ref().expect("active write transaction")
    }
}
impl std::ops::DerefMut for WriteTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.transaction.as_mut().expect("active write transaction")
    }
}
impl Drop for WriteTransaction {
    fn drop(&mut self) {
        if let Some(transaction) = self.transaction.take() {
            let permit = self.permit.take();
            let work = self.work.take();
            let additional_work = std::mem::take(&mut self.additional_work);
            self.runtime.spawn(async move {
                let _permit = permit;
                let _work = work;
                let _additional_work = additional_work;
                let _ = transaction.rollback().await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Poll, Waker},
    };

    fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }
    fn turns(total: usize, tenant: usize, milliseconds: u64) -> WriteTurns {
        WriteTurns::new(
            WriteLimits { total, tenant },
            Duration::from_millis(milliseconds),
        )
    }
    #[test]
    fn write_limits_reject_ambiguous_and_impossible_caps() {
        let defaults = WriteLimits::parse(None, None).expect("defaults");
        assert_eq!((defaults.total, defaults.tenant), (64, 16));
        let small = WriteLimits::parse(Some("2"), None).expect("bounded default tenant");
        assert_eq!((small.total, small.tenant), (2, 2));
        for raw in [
            "",
            "0",
            "-1",
            "+1",
            " 2",
            "2 ",
            "1.5",
            "4097",
            "18446744073709551616",
        ] {
            assert!(WriteLimits::parse(Some(raw), None).is_err(), "{raw}");
        }
        assert!(WriteLimits::parse(Some("2"), Some("3")).is_err());
        assert!(WriteLimits::parse(None, Some("0")).is_err());
        assert!(WriteLimits::parse(Some("4096"), Some("4096")).is_ok());
    }
    #[tokio::test]
    async fn a_held_tenant_preserves_other_tenant_progress_and_finite_registry() {
        let admission = turns(3, 2, 1000);
        let hot = admission
            .request("hot")
            .expect("hot")
            .wait(admission.deadline())
            .await
            .expect("active hot");
        let waiting = admission.request("hot").expect("queued hot");
        assert!(matches!(
            admission.request("hot").expect("limit"),
            Admission::Full
        ));
        let cold = admission
            .request("cold")
            .expect("cold")
            .wait(admission.deadline())
            .await
            .expect("cold independent");
        assert!(matches!(
            admission.request("third").expect("global limit"),
            Admission::Full
        ));
        assert_eq!(admission.requests(), 3);
        drop(waiting);
        assert_eq!(admission.requests(), 2);
        drop(hot);
        drop(cold);
        assert_eq!(admission.requests(), 0);
        assert!(admission
            .registry
            .lock()
            .expect("registry")
            .tenants
            .is_empty());
    }
    #[tokio::test]
    async fn queued_turns_are_fifo_and_cancellation_cannot_release_the_active_turn() {
        let admission = turns(4, 4, 1000);
        let hot = admission
            .request("hot")
            .expect("hot")
            .wait(admission.deadline())
            .await
            .expect("active");
        let mut first = Box::pin(
            admission
                .request("hot")
                .expect("first")
                .wait(admission.deadline()),
        );
        let mut second = Box::pin(
            admission
                .request("hot")
                .expect("second")
                .wait(admission.deadline()),
        );
        assert!(poll(first.as_mut()).is_pending());
        assert!(poll(second.as_mut()).is_pending());
        drop(first);
        assert_eq!(admission.requests(), 2);
        assert!(
            poll(second.as_mut()).is_pending(),
            "cancelling a waiter cannot release actual active work"
        );
        let mut third = Box::pin(
            admission
                .request("hot")
                .expect("third")
                .wait(admission.deadline()),
        );
        assert!(poll(third.as_mut()).is_pending());
        drop(hot);
        assert!(
            poll(third.as_mut()).is_pending(),
            "later request cannot overtake queued head"
        );
        let second = second.await.expect("first remaining request");
        assert!(poll(third.as_mut()).is_pending());
        drop(second);
        drop(third.await.expect("last request"));
        assert_eq!(admission.requests(), 0);
    }
    #[tokio::test]
    async fn a_queue_timeout_reclaims_its_entry_without_releasing_active_work() {
        let admission = turns(2, 2, 15);
        let hot = admission
            .request("hot")
            .expect("hot")
            .wait(admission.deadline())
            .await
            .expect("active");
        assert!(
            matches!(admission.request("hot").expect("queued").wait(admission.deadline()).await, Err(error) if error == WRITE_BUSY)
        );
        assert_eq!(admission.requests(), 1);
        drop(hot);
        assert_eq!(admission.requests(), 0);
    }
}
