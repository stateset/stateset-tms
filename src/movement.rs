//! Reviewed corrections to actual native movement times, never lifecycle rewinds.
use crate::{
    carrier_portal::CarrierSubmission,
    domain::{self, Load, Result, Status, Workspace},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[path = "movement_read.rs"]
mod read;
pub(crate) use read::MovementReadIndex;

pub fn source_sha256(source: &CarrierSubmission) -> String {
    crate::documents::sha256(json!(source).to_string().as_bytes())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MovementCorrection {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub carrier_id: String,
    pub source_id: String,
    pub source_sha256: String,
    pub supersedes_id: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub evidence_sha256: String,
}
impl MovementCorrection {
    fn digest(&self) -> String {
        let mut basis = json!(self);
        basis
            .as_object_mut()
            .expect("correction object")
            .remove("evidence_sha256");
        crate::documents::sha256(basis.to_string().as_bytes())
    }
    fn verify(&self) -> Result<()> {
        if self.digest() == self.evidence_sha256 {
            Ok(())
        } else {
            Err("Internal storage error".into())
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectInput {
    expected_load_version: u64,
    source_sha256: String,
    expected_previous_id: Option<String>,
    observed_at: DateTime<Utc>,
    contact: String,
    reference: String,
    note: String,
}
fn current_chain<'a>(
    source: &CarrierSubmission,
    corrections: impl Iterator<Item = &'a MovementCorrection>,
) -> Result<Option<&'a MovementCorrection>> {
    let mut last: Option<&MovementCorrection> = None;
    let mut ids = std::collections::BTreeSet::new();
    for correction in corrections {
        correction.verify()?;
        if correction.load_id != source.load_id
            || correction.tender_id != source.tender_id
            || correction.carrier_id != source.carrier_id
            || correction.source_sha256 != source_sha256(source)
            || correction.supersedes_id.as_deref() != last.map(|c| c.id.as_str())
            || !ids.insert(correction.id.as_str())
        {
            return Err("Internal storage error".into());
        }
        last = Some(correction);
    }
    Ok(last)
}
fn visible(source: &CarrierSubmission, load: &Load) -> bool {
    source.load_id == load.id
        && Some(&source.tender_id) == load.tender_id.as_ref()
        && Some(&source.carrier_id) == load.carrier_id.as_ref()
        && source.outcome == "applied"
        && ["pickup", "delivery"].contains(&source.kind.as_str())
}
fn source_view(
    source: &CarrierSubmission,
    latest: Result<Option<&MovementCorrection>>,
    corrections: &[&MovementCorrection],
    physical: bool,
    locked: bool,
    valid: bool,
) -> Value {
    json!({"source":source,"source_sha256":source_sha256(source),"effective_observed_at":latest.as_ref().ok().and_then(|c|c.map(|c|c.observed_at)).or(source.observed_at),"evidence_valid":valid && latest.is_ok(),"can_correct":physical && !locked && valid && latest.is_ok(),"locked":locked,"corrections":corrections})
}
impl Workspace {
    pub fn execution_evidence_locked(&self, load: &str) -> bool {
        self.invoices
            .iter()
            .any(|i| i.load_id == load && i.status == "approved")
            || self.settlements.iter().any(|s| s.load_id == load)
            || self
                .bills
                .iter()
                .any(|b| b.load_id == load && b.status != "void")
    }
    pub fn current_movement_correction<'a>(
        &'a self,
        source: &CarrierSubmission,
    ) -> Result<Option<&'a MovementCorrection>> {
        current_chain(
            source,
            self.movement_corrections
                .iter()
                .filter(|c| c.source_id == source.id),
        )
    }
    pub fn verify_movement_corrections(&self, load: &Load) -> Result<()> {
        for source in self.carrier_submissions.iter().filter(|s| {
            s.load_id == load.id
                && Some(&s.tender_id) == load.tender_id.as_ref()
                && s.outcome == "applied"
                && ["pickup", "delivery"].contains(&s.kind.as_str())
        }) {
            self.current_movement_correction(source)?;
        }
        for correction in self
            .movement_corrections
            .iter()
            .filter(|c| c.load_id == load.id && Some(&c.tender_id) == load.tender_id.as_ref())
        {
            let source = self
                .carrier_submissions
                .iter()
                .find(|s| {
                    s.id == correction.source_id
                        && s.load_id == load.id
                        && s.tender_id == correction.tender_id
                        && s.carrier_id == correction.carrier_id
                        && s.outcome == "applied"
                        && ["pickup", "delivery"].contains(&s.kind.as_str())
                })
                .ok_or("Internal storage error")?;
            self.current_movement_correction(source)?;
        }
        Ok(())
    }
    pub fn effective_movement_time(
        &self,
        source: &CarrierSubmission,
    ) -> Result<Option<DateTime<Utc>>> {
        Ok(self
            .current_movement_correction(source)?
            .map(|c| c.observed_at)
            .or(source.observed_at))
    }
    pub fn movement_sources(&self, load: &Load) -> Value {
        let locked = self.execution_evidence_locked(&load.id);
        let valid = self.verify_movement_corrections(load).is_ok();
        let physical = matches!(load.status, Status::InTransit | Status::Delivered);
        json!(self
            .carrier_submissions
            .iter()
            .filter(|s| visible(s, load))
            .map(|s| {
                source_view(
                    s,
                    self.current_movement_correction(s),
                    &self
                        .movement_corrections
                        .iter()
                        .filter(|c| c.source_id == s.id)
                        .collect::<Vec<_>>(),
                    physical,
                    locked,
                    valid,
                )
            })
            .collect::<Vec<_>>())
    }
    pub fn movement_tracking(&self, load: &Load) -> Value {
        json!(self.carrier_submissions.iter().filter(|s|s.load_id==load.id && Some(&s.tender_id)==load.tender_id.as_ref() && s.outcome=="applied" && ["pickup","delivery"].contains(&s.kind.as_str())).filter_map(|s| {
            self.effective_movement_time(s).ok().flatten().map(|time|json!({"kind":s.kind,"observed_at":time,"corrected":self.movement_corrections.iter().any(|c|c.source_id==s.id)}))
        }).collect::<Vec<_>>())
    }
    pub fn apply_movement(&mut self, entity: &str, body: Value, actor: &str) -> Result<Value> {
        let input: CorrectInput =
            serde_json::from_value(body).map_err(|_| "Invalid movement time correction fields")?;
        let source = self
            .carrier_submissions
            .iter()
            .find(|s| s.id == entity)
            .ok_or("Carrier submission not found")?
            .clone();
        if source.outcome != "applied"
            || !["pickup", "delivery"].contains(&source.kind.as_str())
            || source.observed_at.is_none()
        {
            return Err("Only applied native pickup or delivery times can be corrected".into());
        }
        let index = self
            .loads
            .iter()
            .position(|l| l.id == source.load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        if load.version != input.expected_load_version {
            return Err("Load changed; refresh before correcting movement time".into());
        }
        if load.tender_id.as_deref() != Some(&source.tender_id)
            || load.carrier_id.as_deref() != Some(&source.carrier_id)
            || !matches!(load.status, Status::InTransit | Status::Delivered)
        {
            return Err(
                "Only current physical carrier assignment evidence can be corrected".into(),
            );
        }
        if self.execution_evidence_locked(&load.id) {
            return Err("Movement evidence is locked by financial records".into());
        }
        self.verify_movement_corrections(&load)?;
        if source_sha256(&source) != input.source_sha256 {
            return Err(
                "Original movement evidence changed; refresh and review its exact source".into(),
            );
        }
        let prior = self.current_movement_correction(&source)?;
        if prior.map(|c| c.id.as_str()) != input.expected_previous_id.as_deref() {
            return Err("Movement correction changed; review its current predecessor".into());
        }
        if input.observed_at > Utc::now() {
            return Err("Actual movement time cannot be in the future".into());
        }
        if Some(input.observed_at) == self.effective_movement_time(&source)? {
            return Err("Correction must change the actual movement time".into());
        }
        for (value, name) in [
            (&input.contact, "Actual carrier contact"),
            (&input.reference, "Movement correction reference"),
            (&input.note, "Movement correction findings"),
        ] {
            domain::required(value, name)?;
        }
        let mut fact =
            crate::execution::ExecutionFact::carrier(&source).ok_or("Native movement required")?;
        fact.observed_at = input.observed_at;
        self.validate_execution_fact(&load, Some(&source.id), fact)?;
        let mut correction = MovementCorrection {
            id: domain::id("movement-correction"),
            load_id: load.id.clone(),
            tender_id: source.tender_id,
            carrier_id: source.carrier_id,
            source_id: source.id,
            source_sha256: input.source_sha256,
            supersedes_id: input.expected_previous_id,
            observed_at: input.observed_at,
            contact: input.contact.trim().into(),
            reference: input.reference.trim().into(),
            note: input.note.trim().into(),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
            evidence_sha256: String::new(),
        };
        correction.evidence_sha256 = correction.digest();
        correction.verify()?;
        self.movement_corrections.push(correction.clone());
        load.version = load
            .version
            .checked_add(1)
            .ok_or("Load version exhausted")?;
        load.updated_at = Utc::now();
        self.loads[index] = load.clone();
        self.enqueue_sync(&load);
        self.record(&load.id,"movement.time.correct","Actual native movement time corrected; original carrier report and physical shipment state retained");
        if let Some(event) = self.events.last_mut() {
            event.details = Some(
                json!({"correction_id":correction.id,"source_id":correction.source_id,"supersedes_id":correction.supersedes_id}),
            );
        }
        Ok(json!(correction))
    }
}
