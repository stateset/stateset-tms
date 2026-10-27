//! Opt-in phase observations; no workspace payloads, credentials or actor data.
use std::{sync::Arc, time::Instant};

struct Identity {
    id: uuid::Uuid,
    tenant: String,
    started: Instant,
}

#[derive(Clone, Default)]
pub(crate) struct ReadProfile(Option<Arc<Identity>>);

impl ReadProfile {
    pub(crate) fn new(tenant: &str) -> Self {
        if tracing::enabled!(target: "stateset_freight::read_profile", tracing::Level::DEBUG) {
            Self(Some(Arc::new(Identity {
                id: uuid::Uuid::new_v4(),
                tenant: tenant.into(),
                started: Instant::now(),
            })))
        } else {
            Self::default()
        }
    }

    pub(crate) fn stage(&self, stage: &'static str, started: Instant) {
        if let Some(identity) = &self.0 {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            tracing::debug!(target: "stateset_freight::read_profile", "freight_read_profile={}",
                serde_json::json!({"read_id":identity.id,"tenant_id":identity.tenant,
                    "stage":stage,"elapsed_ms":elapsed_ms,
                    "thread_id":format!("{:?}",std::thread::current().id())}));
        }
    }

    pub(crate) fn complete(&self) {
        if let Some(identity) = &self.0 {
            self.stage("complete", identity.started);
        }
    }
    pub(crate) fn approval_volume(&self, header_bytes: usize, blocks: usize, payload_bytes: usize) {
        if let Some(identity) = &self.0 {
            tracing::debug!(target:"stateset_freight::read_profile","freight_approval_read_volume={}",
                serde_json::json!({"read_id":identity.id,"tenant_id":identity.tenant,
                    "header_bytes":header_bytes,"original_blocks":blocks,"original_payload_bytes":payload_bytes}));
        }
    }
}
