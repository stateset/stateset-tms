//! Read-only process diagnostics: bounded labels, no SQL and no waiting on
//! admission registries. Values sampled from separate primitives are not atomic.
use std::fmt::Write;

pub(crate) struct QueueSnapshot {
    pub(crate) pending: Option<usize>,
    pub(crate) limit: usize,
    pub(crate) tenant_limit: usize,
}
pub(crate) struct ReadLane {
    pub(crate) queue: QueueSnapshot,
    pub(crate) available: usize,
    pub(crate) closed: bool,
}
impl ReadLane {
    pub(crate) fn new(queue: QueueSnapshot, slots: &tokio::sync::Semaphore) -> Self {
        Self {
            queue,
            available: slots.available_permits(),
            closed: slots.is_closed(),
        }
    }
}
pub(crate) struct WriteSnapshot {
    pub(crate) requests: Option<usize>,
    pub(crate) limit: usize,
    pub(crate) tenant_limit: usize,
}
pub(crate) struct PoolSnapshot {
    pub(crate) connections: u32,
    pub(crate) idle: usize,
    pub(crate) limit: u32,
    pub(crate) closed: bool,
}
pub(crate) struct AdmissionSnapshot {
    pub(crate) read: ReadLane,
    pub(crate) catalog: ReadLane,
    pub(crate) workspace: Option<ReadLane>,
    pub(crate) write: WriteSnapshot,
    pub(crate) pool: Option<PoolSnapshot>,
}
impl AdmissionSnapshot {
    pub(crate) fn append(&self, text: &mut String) {
        type ReadMetric = (&'static str, &'static str, fn(&ReadLane) -> Option<usize>);
        let metrics: [ReadMetric; 6] = [
            ("freight_admission_pending", "Queued read/work requests; omitted when registry observation is unavailable.", |lane| lane.queue.pending),
            ("freight_admission_pending_limit", "Configured global waiting limit for the read/work lane.", |lane| Some(lane.queue.limit)),
            ("freight_admission_tenant_pending_limit", "Configured per-tenant waiting limit; no tenant identities are exported.", |lane| Some(lane.queue.tenant_limit)),
            ("freight_admission_available_permits", "Currently unleased permits; does not measure retained bytes or running CPU.", |lane| Some(lane.available)),
            ("freight_admission_closed", "Whether the local lane semaphore is closed.", |lane| Some(usize::from(lane.closed))),
            ("freight_admission_snapshot_available", "Whether this scrape obtained the admission registry without waiting; missing observations are not zero requests.", |lane| Some(usize::from(lane.queue.pending.is_some()))),
        ];
        // Keep every family's HELP/TYPE and samples together for Prometheus
        // parsers. Optional lanes/pools have no invented zero-valued samples.
        for (name, help, value) in metrics {
            let _ = writeln!(text, "# HELP {name} {help}\n# TYPE {name} gauge");
            for (label, lane) in [
                ("snapshot", Some(&self.read)),
                ("catalog", Some(&self.catalog)),
                ("workspace", self.workspace.as_ref()),
            ] {
                if let Some(value) = lane.and_then(value) {
                    let _ = writeln!(text, "{name}{{lane=\"{label}\"}} {value}");
                }
            }
            if name == "freight_admission_snapshot_available" {
                let _ = writeln!(
                    text,
                    "{name}{{lane=\"write\"}} {}",
                    u8::from(self.write.requests.is_some())
                );
            }
        }
        gauge(
            text,
            "freight_workspace_admission_configured",
            "Whether the optional whole-workspace processing limit is configured.",
            Some(u8::from(self.workspace.is_some())),
        );
        gauge(text, "freight_write_admission_requests", "Write reservations including active ownership and waiting requests; omitted when registry observation is unavailable.", self.write.requests);
        gauge(
            text,
            "freight_write_admission_limit",
            "Configured global limit including active and waiting write reservations.",
            Some(self.write.limit),
        );
        gauge(
            text,
            "freight_write_admission_tenant_limit",
            "Configured per-tenant limit including active and waiting write reservations.",
            Some(self.write.tenant_limit),
        );
        gauge(
            text,
            "freight_database_pool_configured",
            "Whether this process uses PostgreSQL storage.",
            Some(u8::from(self.pool.is_some())),
        );
        gauge(
            text,
            "freight_database_pool_connections",
            "SQLx pool connection count; not a database health check.",
            self.pool.as_ref().map(|pool| pool.connections),
        );
        gauge(
            text,
            "freight_database_pool_idle",
            "SQLx idle connection count, sampled independently of total connections.",
            self.pool.as_ref().map(|pool| pool.idle),
        );
        gauge(
            text,
            "freight_database_pool_limit",
            "Configured SQLx maximum connection count.",
            self.pool.as_ref().map(|pool| pool.limit),
        );
        gauge(
            text,
            "freight_database_pool_closed",
            "Whether the local pool was closed; does not detect remote database availability.",
            self.pool.as_ref().map(|pool| u8::from(pool.closed)),
        );
    }
}
fn gauge<T: std::fmt::Display>(
    text: &mut String,
    name: &'static str,
    help: &'static str,
    value: Option<T>,
) {
    let _ = writeln!(text, "# HELP {name} {help}\n# TYPE {name} gauge");
    if let Some(value) = value {
        let _ = writeln!(text, "{name} {value}");
    }
}
