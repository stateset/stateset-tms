//! Bounded tenant cycles; network calls never hold up the expiry sweep.
use crate::{domain::Status, integrations::Integrations, lifecycle::Shutdown, store::Store};
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    sync::Arc,
    time::Duration,
};
use tokio::{
    task::{Id, JoinSet},
    time::Instant,
};

const POLL: Duration = Duration::from_secs(2);
const SOURCE_POLL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy)]
pub struct WorkerConfig {
    concurrency: usize,
}
impl Default for WorkerConfig {
    fn default() -> Self {
        Self { concurrency: 2 }
    }
}
impl WorkerConfig {
    fn parse(raw: Option<&str>) -> Result<Self, String> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(concurrency @ 1..=16) = raw.parse() {
                return Ok(Self { concurrency });
            }
        }
        Err("FREIGHT_INTEGRATION_CONCURRENCY must be an integer from 1 to 16".into())
    }
    pub fn from_env() -> Result<Self, String> {
        let raw = match std::env::var("FREIGHT_INTEGRATION_CONCURRENCY") {
            Ok(v) => Some(v),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err("Invalid FREIGHT_INTEGRATION_CONCURRENCY".into()),
        };
        Self::parse(raw.as_deref())
    }
}

#[derive(Clone)]
struct TenantCycle {
    tenant: String,
    next_poll: Instant,
    next_source: Instant,
    source_cursor: usize,
    dispatch_cursor: crate::integrations::DispatchCursor,
}
impl TenantCycle {
    fn new(tenant: String) -> Self {
        Self {
            tenant,
            next_poll: Instant::now(),
            next_source: Instant::now(),
            source_cursor: 0,
            dispatch_cursor: crate::integrations::DispatchCursor::default(),
        }
    }
}

// Only running tenants become Tokio tasks. Every other configured tenant has
// one small queue entry; completing a cycle returns it to the back of the queue.
async fn schedule<F, Fut>(tenants: Vec<String>, config: WorkerConfig, shutdown: Shutdown, cycle: F)
where
    F: Fn(TenantCycle) -> Fut,
    Fut: Future<Output = TenantCycle> + Send + 'static,
{
    let mut pending: VecDeque<_> = tenants.into_iter().map(TenantCycle::new).collect();
    let mut tasks = JoinSet::new();
    let mut running: BTreeMap<Id, TenantCycle> = BTreeMap::new();
    loop {
        while tasks.len() < config.concurrency && !shutdown.requested() {
            let now = Instant::now();
            let Some(index) = pending.iter().position(|state| state.next_poll <= now) else {
                break;
            };
            let state = pending.remove(index).expect("queued tenant");
            let recovery = state.clone();
            let handle = tasks.spawn(cycle(state));
            running.insert(handle.id(), recovery);
        }
        if shutdown.requested() {
            // Drain only the admitted cycles. Do not start queued tenants or
            // requeue completed cycles, including after a child panic.
            while tasks.join_next().await.is_some() {}
            return;
        }
        let next = (tasks.len() < config.concurrency)
            .then(|| pending.iter().map(|state| state.next_poll).min())
            .flatten();
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {}
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let mut state = match result.expect("running cycle") {
                    Ok((id, state)) => { running.remove(&id); state }
                    Err(error) => {
                        let state = running.remove(&error.id()).expect("owned cycle");
                        tracing::warn!(tenant=%state.tenant, "Integration cycle interrupted; tenant will be polled again");
                        state
                    }
                };
                state.next_poll = Instant::now() + POLL;
                pending.push_back(state);
            }
            _ = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {}
            else => std::future::pending::<()>().await,
        }
    }
}

async fn tenant_cycle(
    mut state: TenantCycle,
    store: Store,
    integrations: Arc<Integrations>,
    shutdown: Shutdown,
) -> TenantCycle {
    if shutdown.requested() {
        return state;
    }
    let tenant = &state.tenant;
    if let Err(error) = integrations.process_inbound_one(&store, tenant).await {
        tracing::warn!(%tenant,%error,"Inbound EDI processing failed");
    }
    if shutdown.requested() {
        return state;
    }
    if Instant::now() >= state.next_source {
        // Release the workspace before remote verification, rather than keeping
        // a whole tenant snapshot alive across four network round trips.
        let active: Vec<_> = match store.read(tenant).await {
            Ok(workspace) => workspace
                .loads
                .iter()
                .filter(|load| {
                    load.sync_order_id.is_some()
                        && !matches!(load.status, Status::Cancelled | Status::Delivered)
                })
                .map(|load| load.id.clone())
                .collect(),
            Err(_) => Vec::new(),
        };
        if !active.is_empty() {
            let count = active.len().min(4);
            for offset in 0..count {
                if shutdown.requested() {
                    return state;
                }
                let load = &active[(state.source_cursor % active.len() + offset) % active.len()];
                if let Err(error) = integrations
                    .refresh_source(&store, tenant, load, "system", None)
                    .await
                {
                    tracing::warn!(%tenant,%load,%error,"Scheduled source verification failed");
                }
            }
            state.source_cursor = (state.source_cursor % active.len() + count) % active.len();
        }
        state.next_source = Instant::now() + SOURCE_POLL;
    }
    if shutdown.requested() {
        return state;
    }
    if let Err(error) = integrations
        .dispatch_turn(&store, tenant, &mut state.dispatch_cursor, &shutdown)
        .await
    {
        tracing::warn!(%tenant,%error,"Integration dispatch failed");
    }
    state
}

async fn expiry(store: Store, shutdown: Shutdown) {
    let mut ticks = tokio::time::interval(SOURCE_POLL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = ticks.tick() => {}
        }
        match store.tenant_ids().await {
            Ok(tenants) => {
                for tenant in tenants {
                    if shutdown.requested() {
                        return;
                    }
                    if let Err(error) = store.expire_tenders(&tenant).await {
                        tracing::warn!(%tenant,%error,"Tender expiry sweep failed");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error,"Tender expiry cannot enumerate compatible workspaces")
            }
        }
    }
}

pub async fn worker(store: Store, integrations: Integrations, config: WorkerConfig) {
    worker_until(store, integrations, config, Shutdown::default()).await;
}

pub async fn worker_until(
    store: Store,
    integrations: Integrations,
    config: WorkerConfig,
    shutdown: Shutdown,
) {
    let tenants = integrations.tenants.keys().cloned().collect();
    let integrations = Arc::new(integrations);
    let tenant_store = store.clone();
    // Dropping the worker drops both futures and the JoinSet, aborting its
    // tenant tasks. In-flight durable leases keep their existing crash rules.
    tokio::join!(
        schedule(tenants, config, shutdown.clone(), {
            let shutdown = shutdown.clone();
            move |state| {
                tenant_cycle(
                    state,
                    tenant_store.clone(),
                    integrations.clone(),
                    shutdown.clone(),
                )
            }
        }),
        expiry(store, shutdown),
    );
}

#[cfg(test)]
#[path = "integration_worker_tests.rs"]
mod tests;
