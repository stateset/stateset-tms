//! Bound queued snapshot work while keeping one global waiter per tenant.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

pub const READ_BUSY: &str = "Workspace is busy. Please retry shortly.";

#[derive(Clone, Copy)]
pub(crate) struct ReadLimits {
    pub(crate) pending: usize,
    pub(crate) tenant_pending: usize,
    pub(crate) timeout: Duration,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            pending: 64,
            tenant_pending: 16,
            timeout: Duration::from_millis(5000),
        }
    }
}
impl ReadLimits {
    #[cfg(test)]
    fn parse(
        pending: Option<&str>,
        tenant: Option<&str>,
        timeout: Option<&str>,
    ) -> Result<Self, String> {
        Self::parse_named(
            pending,
            tenant,
            timeout,
            [
                "FREIGHT_READ_MAX_PENDING",
                "FREIGHT_READ_TENANT_MAX_PENDING",
                "FREIGHT_READ_QUEUE_TIMEOUT_MS",
            ],
        )
    }
    fn parse_named(
        pending: Option<&str>,
        tenant: Option<&str>,
        timeout: Option<&str>,
        names: [&str; 3],
    ) -> Result<Self, String> {
        fn integer(
            raw: Option<&str>,
            default: usize,
            max: usize,
            name: &str,
        ) -> Result<usize, String> {
            let Some(raw) = raw else {
                return Ok(default);
            };
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{name} must be an integer from 1 to {max}"));
            }
            raw.parse::<usize>()
                .ok()
                .filter(|n| (1..=max).contains(n))
                .ok_or_else(|| format!("{name} must be an integer from 1 to {max}"))
        }
        let pending = integer(pending, 64, 4096, names[0])?;
        let tenant_pending = integer(tenant, 16.min(pending), pending, names[1])?;
        let timeout = integer(timeout, 5000, 60000, names[2])?;
        Ok(Self {
            pending,
            tenant_pending,
            timeout: Duration::from_millis(timeout as u64),
        })
    }
    pub(crate) fn from_env() -> Result<Self, String> {
        Self::from_env_named([
            "FREIGHT_READ_MAX_PENDING",
            "FREIGHT_READ_TENANT_MAX_PENDING",
            "FREIGHT_READ_QUEUE_TIMEOUT_MS",
        ])
    }
    pub(crate) fn catalog_from_env() -> Result<Self, String> {
        Self::from_env_named([
            "FREIGHT_CATALOG_MAX_PENDING",
            "FREIGHT_CATALOG_TENANT_MAX_PENDING",
            "FREIGHT_CATALOG_QUEUE_TIMEOUT_MS",
        ])
    }
    fn from_env_named(names: [&str; 3]) -> Result<Self, String> {
        fn value(name: &str) -> Result<Option<String>, String> {
            match std::env::var(name) {
                Ok(raw) => Ok(Some(raw)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                _ => Err(format!("Invalid {name}")),
            }
        }
        Self::parse_named(
            value(names[0])?.as_deref(),
            value(names[1])?.as_deref(),
            value(names[2])?.as_deref(),
            names,
        )
    }
}

#[derive(Default)]
struct Registry {
    pending: usize,
    tenants: BTreeMap<String, TenantQueue>,
}
struct TenantQueue {
    pending: usize,
    turn: Arc<AsyncMutex<()>>,
}
pub(crate) struct TenantReadTurns {
    registry: Arc<Mutex<Registry>>,
    limits: ReadLimits,
}
impl Default for TenantReadTurns {
    fn default() -> Self {
        Self::new(ReadLimits::default())
    }
}
// Own the reservation before either asynchronous queue. Dropping a cancelled,
// expired or admitted waiter reclaims both limits and its tenant registry entry.
struct Pending {
    registry: Arc<Mutex<Registry>>,
    tenant: String,
}
impl Drop for Pending {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.pending -= 1;
        if let Some(queue) = registry.tenants.get_mut(&self.tenant) {
            queue.pending -= 1;
            if queue.pending == 0 {
                registry.tenants.remove(&self.tenant);
            }
        }
    }
}
impl TenantReadTurns {
    #[cfg(test)]
    pub(crate) fn with_registry_locked<T>(&self, observe: impl FnOnce() -> T) -> T {
        let _guard = self.registry.lock().expect("owned test registry");
        observe()
    }
    pub(crate) fn diagnostics(&self) -> crate::admission_diagnostics::QueueSnapshot {
        crate::admission_diagnostics::QueueSnapshot {
            pending: self
                .registry
                .try_lock()
                .ok()
                .map(|registry| registry.pending),
            limit: self.limits.pending,
            tenant_limit: self.limits.tenant_pending,
        }
    }
    #[cfg(test)]
    pub(crate) fn pending_count(&self) -> usize {
        self.registry.lock().expect("test registry").pending
    }
    pub(crate) fn new(limits: ReadLimits) -> Self {
        Self {
            registry: Arc::default(),
            limits,
        }
    }
    pub(crate) async fn acquire(
        &self,
        tenant: &str,
        slots: Arc<Semaphore>,
    ) -> Result<OwnedSemaphorePermit, String> {
        let deadline = tokio::time::Instant::now() + self.limits.timeout;
        let (queue, reservation) = {
            let mut registry = self
                .registry
                .lock()
                .map_err(|_| "Read admission registry is unavailable".to_string())?;
            // Do not bypass a previously queued tenant when a permit has just
            // been released but that tenant has not been scheduled yet.
            if registry.pending == 0 {
                match slots.clone().try_acquire_owned() {
                    Ok(permit) => return Ok(permit),
                    Err(tokio::sync::TryAcquireError::Closed) => {
                        return Err("Read admission is unavailable".into())
                    }
                    Err(tokio::sync::TryAcquireError::NoPermits) => {}
                }
            }
            if registry.pending >= self.limits.pending
                || registry
                    .tenants
                    .get(tenant)
                    .is_some_and(|q| q.pending >= self.limits.tenant_pending)
            {
                tracing::debug!(
                    reason = "queue_full",
                    pending = registry.pending,
                    "freight_read_admission_rejected"
                );
                return Err(READ_BUSY.into());
            }
            registry.pending += 1;
            let queue = registry
                .tenants
                .entry(tenant.into())
                .or_insert_with(|| TenantQueue {
                    pending: 0,
                    turn: Arc::new(AsyncMutex::new(())),
                });
            queue.pending += 1;
            (
                queue.turn.clone(),
                Pending {
                    registry: self.registry.clone(),
                    tenant: tenant.into(),
                },
            )
        };
        // Both Tokio primitives are FIFO. Only a tenant's head joins the global
        // queue; release its turn upon admission so idle capacity is usable.
        let admitted = tokio::time::timeout_at(deadline, async {
            let turn = queue.lock().await;
            let permit = slots
                .acquire_owned()
                .await
                .map_err(|_| "Read admission is unavailable".to_string());
            drop(turn);
            permit
        })
        .await
        .map_err(|_| {
            tracing::debug!(reason = "queue_timeout", "freight_read_admission_rejected");
            READ_BUSY.to_string()
        });
        drop(reservation);
        admitted?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        task::{Context, Waker},
    };

    fn pending<F: Future>(future: Pin<&mut F>) {
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.poll(&mut context).is_pending());
    }

    fn bounded(global: usize, tenant: usize, millis: u64) -> TenantReadTurns {
        TenantReadTurns::new(ReadLimits {
            pending: global,
            tenant_pending: tenant,
            timeout: Duration::from_millis(millis),
        })
    }
    fn counts(turns: &TenantReadTurns) -> (usize, usize) {
        let registry = turns.registry.lock().expect("registry");
        (registry.pending, registry.tenants.len())
    }
    #[test]
    fn admission_configuration_has_finite_limits_and_rejects_ambiguous_values() {
        let defaults = ReadLimits::parse(None, None, None).expect("default");
        assert_eq!(
            (
                defaults.pending,
                defaults.tenant_pending,
                defaults.timeout.as_millis()
            ),
            (64, 16, 5000)
        );
        let small = ReadLimits::parse(Some("2"), None, Some("1")).expect("small global default");
        assert_eq!((small.pending, small.tenant_pending), (2, 2));
        assert!(ReadLimits::parse(Some("2"), Some("3"), None).is_err());
        assert!(ReadLimits::parse(Some("4096"), Some("4096"), Some("60000")).is_ok());
        for bad in [
            "",
            "0",
            "-1",
            "+2",
            " 2",
            "2 ",
            "1.5",
            "unlimited",
            "99999999999999999999999999",
        ] {
            assert!(
                ReadLimits::parse(Some(bad), None, None).is_err(),
                "global {bad}"
            );
            assert!(
                ReadLimits::parse(None, Some(bad), None).is_err(),
                "tenant {bad}"
            );
            assert!(
                ReadLimits::parse(None, None, Some(bad)).is_err(),
                "deadline {bad}"
            );
        }
        assert!(ReadLimits::parse(Some("4097"), None, None).is_err());
        assert!(ReadLimits::parse(None, Some("65"), None).is_err());
        assert!(ReadLimits::parse(None, None, Some("60001")).is_err());
    }

    #[tokio::test]
    async fn overload_caps_local_and_global_backlogs_without_growing_the_registry() {
        let turns = bounded(3, 2, 5000);
        let slots = Arc::new(Semaphore::new(1));
        let active = turns.acquire("busy", slots.clone()).await.expect("active");
        assert_eq!(
            counts(&turns),
            (0, 0),
            "running jobs do not consume pending capacity"
        );
        let mut head = Box::pin(turns.acquire("busy", slots.clone()));
        let mut tail = Box::pin(turns.acquire("busy", slots.clone()));
        pending(head.as_mut());
        pending(tail.as_mut());
        assert_eq!(
            turns
                .acquire("busy", slots.clone())
                .await
                .expect_err("tenant cap"),
            READ_BUSY
        );
        let mut other = Box::pin(turns.acquire("other", slots.clone()));
        pending(other.as_mut());
        for n in 0..1000 {
            assert_eq!(
                turns
                    .acquire(&format!("rejected-{n}"), slots.clone())
                    .await
                    .expect_err("global cap"),
                READ_BUSY
            );
            assert_eq!(counts(&turns), (3, 2));
        }
        drop(tail);
        assert_eq!(counts(&turns), (2, 2));
        drop(active);
        let head = head.await.expect("head");
        assert_eq!(counts(&turns), (1, 1));
        drop(head);
        drop(other.await.expect("other"));
        assert_eq!(counts(&turns), (0, 0));
        assert_eq!(slots.available_permits(), 1);
        drop(
            turns
                .acquire("new", slots.clone())
                .await
                .expect("capacity reusable"),
        );
    }

    #[tokio::test]
    async fn one_deadline_covers_both_local_and_global_queues_and_reclaims_capacity() {
        let turns = bounded(3, 2, 40);
        let slots = Arc::new(Semaphore::new(1));
        let active = turns.acquire("busy", slots.clone()).await.expect("active");
        let mut head = Box::pin(turns.acquire("busy", slots.clone()));
        let mut tail = Box::pin(turns.acquire("busy", slots.clone()));
        let mut other = Box::pin(turns.acquire("other", slots.clone()));
        pending(head.as_mut());
        pending(tail.as_mut());
        pending(other.as_mut());
        let (a, b, c) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(head, tail, other)
        })
        .await
        .expect("finite queue deadline");
        for result in [a, b, c] {
            assert_eq!(result.expect_err("expired before SQL"), READ_BUSY);
        }
        assert_eq!(counts(&turns), (0, 0));
        assert_eq!(
            slots.available_permits(),
            0,
            "timeouts cannot release active work"
        );
        drop(active);
        drop(
            turns
                .acquire("retry", slots.clone())
                .await
                .expect("retry after recovery"),
        );
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn expiry_and_permit_release_races_do_not_leak_or_duplicate_slots() {
        let turns = bounded(1, 1, 3);
        let slots = Arc::new(Semaphore::new(1));
        for _ in 0..20 {
            let active = turns
                .acquire("active", slots.clone())
                .await
                .expect("active");
            let waiter = turns.acquire("waiting", slots.clone());
            let release = async {
                tokio::time::sleep(Duration::from_millis(3)).await;
                drop(active);
            };
            let (result, ()) = tokio::join!(waiter, release);
            match result {
                Ok(permit) => drop(permit),
                Err(error) => assert_eq!(error, READ_BUSY),
            }
            assert_eq!(counts(&turns), (0, 0));
            assert_eq!(slots.available_permits(), 1);
        }
    }

    #[tokio::test]
    async fn tenant_backlogs_share_admission_without_wasting_idle_slots() {
        let turns = TenantReadTurns::default();
        let slots = Arc::new(Semaphore::new(2));
        let first = turns.acquire("busy", slots.clone()).await.expect("first");
        let second = turns
            .acquire("busy", slots.clone())
            .await
            .expect("idle capacity");
        assert_eq!(slots.available_permits(), 0);
        let mut busy_head = std::pin::pin!(turns.acquire("busy", slots.clone()));
        let mut busy_tail = std::pin::pin!(turns.acquire("busy", slots.clone()));
        pending(busy_head.as_mut());
        pending(busy_tail.as_mut());
        let mut other = std::pin::pin!(turns.acquire("other", slots.clone()));
        pending(other.as_mut());
        drop(first);
        let head = busy_head.await.expect("first tenant's head");
        // This request was queued before the other tenant, but was held out of
        // global admission by its tenant's head. It must now join behind other.
        pending(busy_tail.as_mut());
        pending(other.as_mut());
        drop(second);
        let other = other.await.expect("other tenant gets next capacity");
        pending(busy_tail.as_mut());
        drop(head);
        let tail = busy_tail.await.expect("original tenant resumes");
        drop((other, tail));
        assert_eq!(slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn cancelling_heads_and_local_waiters_keeps_other_tenants_moving() {
        let turns = TenantReadTurns::default();
        let slots = Arc::new(Semaphore::new(1));
        let running = turns
            .acquire("running", slots.clone())
            .await
            .expect("running");
        let mut cancelled_head = Box::pin(turns.acquire("busy", slots.clone()));
        pending(cancelled_head.as_mut());
        let mut cancelled_tail = Box::pin(turns.acquire("busy", slots.clone()));
        pending(cancelled_tail.as_mut());
        let mut surviving_tail = std::pin::pin!(turns.acquire("busy", slots.clone()));
        pending(surviving_tail.as_mut());
        let mut other = std::pin::pin!(turns.acquire("other", slots.clone()));
        pending(other.as_mut());
        drop(cancelled_tail);
        drop(cancelled_head);
        pending(surviving_tail.as_mut());
        drop(running);
        let other = other.await.expect("cancelled head cannot block other");
        pending(surviving_tail.as_mut());
        drop(other);
        drop(surviving_tail.await.expect("surviving local waiter"));
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn completed_tenant_queues_are_reclaimed_before_registering_new_identities() {
        let turns = TenantReadTurns::default();
        let slots = Arc::new(Semaphore::new(1));
        for index in 0..1000 {
            let permit = turns
                .acquire(&format!("tenant-{index}"), slots.clone())
                .await
                .expect("admit");
            drop(permit);
            let registry = turns.registry.lock().expect("registry");
            assert_eq!(registry.pending, 0);
            assert!(registry.tenants.is_empty());
        }
    }
}

#[cfg(test)]
mod catalog_configuration_tests {
    use super::*;
    const NAMES: [&str; 3] = [
        "FREIGHT_CATALOG_MAX_PENDING",
        "FREIGHT_CATALOG_TENANT_MAX_PENDING",
        "FREIGHT_CATALOG_QUEUE_TIMEOUT_MS",
    ];
    #[test]
    fn catalog_queue_defaults_and_validation_are_independent_and_bounded() {
        let defaults = ReadLimits::parse_named(None, None, None, NAMES).expect("defaults");
        assert_eq!(
            (
                defaults.pending,
                defaults.tenant_pending,
                defaults.timeout.as_millis()
            ),
            (64, 16, 5000)
        );
        let one = ReadLimits::parse_named(Some("1"), None, Some("40"), NAMES).expect("one");
        assert_eq!((one.pending, one.tenant_pending), (1, 1));
        for (pending, tenant, timeout, field) in [
            (Some("0"), None, None, NAMES[0]),
            (Some("4097"), None, None, NAMES[0]),
            (Some("1"), Some("2"), None, NAMES[1]),
            (None, None, Some("0"), NAMES[2]),
            (None, None, Some("1.5"), NAMES[2]),
        ] {
            assert!(ReadLimits::parse_named(pending, tenant, timeout, NAMES)
                .err()
                .expect("invalid")
                .contains(field));
        }
        let snapshot = ReadLimits::parse(Some("1"), None, Some("50")).expect("snapshot");
        assert_eq!(snapshot.pending, 1);
        assert_eq!(defaults.pending, 64);
    }
}
