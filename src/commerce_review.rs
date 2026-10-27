//! Record reviewed external advancement without rebasing or resending old intent.
use super::*;
use chrono::Utc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    pub expected_load_version: u64,
    pub expected_total_attempts: u64,
    pub expected_shipment_version: i32,
    pub contact: String,
    pub reference: String,
    pub note: String,
}
fn held<'a>(w: &'a Workspace, id: &str) -> Result<(&'a Outbox, &'a Load), String> {
    let job = w
        .outbox
        .iter()
        .find(|j| j.id == id)
        .ok_or("Outbox item not found")?;
    if job.destination != "stateset-icommerce"
        || job.status != "held"
        || job.lease.is_some()
        || job.leased_until.is_some()
    {
        return Err(
            "Only a held native shipment handoff without an active lease can be reviewed".into(),
        );
    }
    let load = w
        .loads
        .iter()
        .find(|l| l.id == job.load_id)
        .ok_or("Load not found")?;
    Ok((job, load))
}
fn proof(
    w: &Workspace,
    id: &str,
    native: &Shipment,
    configured: &Binding,
) -> Result<Value, String> {
    let (job, load) = held(w, id)?;
    let (saved, original, plan) = frozen_intent(job)?;
    if &saved != configured {
        return Err("Native connection changed; original handoff cannot be retargeted".into());
    }
    guard_link(w, &load.id, native.id)?;
    let retained = load
        .source_order
        .as_ref()
        .ok_or("Retained native order missing")?;
    let retained_plan = TrackingPlan::new(&original, retained, &plan.target_status)?
        .ok_or("Original native lifecycle step cannot be verified")?;
    if serde_json::to_value(retained_plan).map_err(|_| "Invalid retained plan")?
        != serde_json::to_value(&plan).map_err(|_| "Invalid original plan")?
        || load.sync_order_id.as_deref() != retained["id"].as_str()
    {
        return Err(
            "Original handoff no longer matches retained order identity and manifest".into(),
        );
    }
    if native.id != original.id
        || native.order_id != original.order_id
        || !same_manifest(native, &original)
    {
        return Err(
            "Current native shipment must retain the original order identity and complete manifest"
                .into(),
        );
    }
    if native.version <= plan.expected_version {
        return Err("Native version has not advanced; the original PATCH may still apply. Keep this handoff held or explicitly retry its original version".into());
    }
    if native.status.is_empty()
        || native.status.len() > 100
        || native.status.chars().any(char::is_control)
    {
        return Err("Native shipment status cannot be reviewed".into());
    }
    Ok(
        json!({"job_id":job.id,"load_id":load.id,"reference":load.reference,"expected_load_version":load.version,"expected_total_attempts":job.total_attempts,"original_plan":plan,"shipment":native,"late_original_fenced":true,"explanation":"The observed native version is newer than the original expected version. Retire the local handoff after reviewing external activity; this does not establish who applied the original PATCH or confirm freight delivery."}),
    )
}
pub async fn review_preview(w: &Workspace, id: &str, c: &Connection) -> Result<Value, String> {
    let (job, _) = held(w, id)?;
    let (_, original, _) = frozen_intent(job)?;
    let configured = binding(c)?;
    // Verify the fixed destination before any outbound read.
    if job.payload["binding"]
        != serde_json::to_value(&configured).map_err(|_| "Invalid native binding")?
    {
        return Err("Native connection changed; original handoff cannot be retargeted".into());
    }
    let current = shipment(c, &original.id.to_string()).await?;
    proof(w, id, &current, &configured)
}
pub fn reconcile(w: &mut Workspace, id: &str, body: Value, actor: &str) -> Result<Value, String> {
    let request: ReconcileRequest = serde_json::from_value(body["request"].clone())
        .map_err(|_| "Invalid external handoff review")?;
    for (value, label) in [
        (&request.contact, "Review contact"),
        (&request.reference, "Review reference"),
        (&request.note, "Review note"),
    ] {
        crate::domain::required(value, label)?;
        if value.chars().any(char::is_control) {
            return Err("Review fields cannot contain control characters".into());
        }
    }
    let native: Shipment = serde_json::from_value(body["verified_shipment"].clone())
        .map_err(|_| "Verified native shipment missing")?;
    let configured: Binding = serde_json::from_value(body["binding"].clone())
        .map_err(|_| "Verified native binding missing")?;
    let preview = proof(w, id, &native, &configured)?;
    let (job, load) = held(w, id)?;
    if request.expected_load_version != load.version
        || request.expected_total_attempts != job.total_attempts
        || request.expected_shipment_version != native.version
    {
        return Err(
            "Shipment, load or original handoff attempts changed; preview the review again".into(),
        );
    }
    let load_id = load.id.clone();
    let record = json!({"kind":"external_native_advancement","reviewed_at":Utc::now(),"reviewed_by":actor,"contact":request.contact.trim(),"reference":request.reference.trim(),"note":request.note.trim(),"observation":preview,"original_patch_outcome":"unknown","original_error":job.last_error,"original_response":job.response});
    let item = w
        .outbox
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or("Outbox item not found")?;
    item.status = "external_reviewed".into();
    item.next_attempt_at = None;
    item.last_error = None;
    item.response = Some(record.to_string());
    // Preserve payload, identity, attempt counters and transmission history.
    let result = json!({"job_id":id,"load_id":load_id,"status":item.status,"review":record});
    w.record(&load_id,"commerce.reconcile","Reviewed native version advancement; original handoff retired locally without another PATCH or a claim of native receipt");
    if let Some(event) = w.events.last_mut() {
        event.actor = actor.into();
        event.details = Some(result.clone());
    }
    Ok(result)
}
