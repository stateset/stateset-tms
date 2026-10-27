//! Opt-in write-preparation attribution; never log request or document contents.
use std::time::Instant;

pub(crate) struct WriteProfile(Option<(uuid::Uuid, String)>);
impl WriteProfile {
    pub(crate) fn new(tenant: &str) -> Self {
        Self(
            tracing::enabled!(target: "stateset_freight::write_profile", tracing::Level::DEBUG)
                .then(|| (uuid::Uuid::new_v4(), tenant.into())),
        )
    }
    pub(crate) fn stage(&self, stage: &'static str, started: Instant) {
        if let Some((id, tenant)) = &self.0 {
            tracing::debug!(target: "stateset_freight::write_profile", "freight_write_profile={}",
                serde_json::json!({"write_id":id,"tenant_id":tenant,"stage":stage,
                    "elapsed_ms":started.elapsed().as_secs_f64()*1000.0,
                    "thread_id":format!("{:?}",std::thread::current().id())}));
        }
    }
}
