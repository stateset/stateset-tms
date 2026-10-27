//! One route order for independently recorded gates, service, and native movement.
//! EDI processing timestamps are deliberately not treated as actual stop times.
use crate::domain::{Load, Result, Workspace};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Serialize)]
pub(crate) struct ExecutionFact {
    pub id: String,
    pub stop_id: String,
    pub kind: String,
    pub source: String,
    pub observed_at: DateTime<Utc>,
    pub reference: String,
    pub recorded_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_observed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction: Option<Value>,
}
impl ExecutionFact {
    pub fn gate(v: &crate::stops::StopEvent) -> Self {
        Self {
            id: v.id.clone(),
            stop_id: v.stop.clone(),
            kind: v.kind.clone(),
            source: "gate".into(),
            observed_at: v.occurred_at,
            reference: v.reference.clone(),
            recorded_by: v.recorded_by.clone(),
            original_observed_at: None,
            correction: None,
        }
    }
    pub fn service(v: &crate::itinerary::StopService) -> Self {
        Self {
            id: v.id.clone(),
            stop_id: v.stop_id.clone(),
            kind: "service".into(),
            source: "stop_service".into(),
            observed_at: v.observed_at,
            reference: v.reference.clone(),
            recorded_by: v.recorded_by.clone(),
            original_observed_at: None,
            correction: None,
        }
    }
    pub fn carrier(v: &crate::carrier_portal::CarrierSubmission) -> Option<Self> {
        if !["pickup", "delivery"].contains(&v.kind.as_str()) {
            return None;
        }
        Some(Self {
            id: v.id.clone(),
            stop_id: v.kind.clone(),
            kind: v.kind.clone(),
            source: "carrier".into(),
            observed_at: v.observed_at?,
            reference: v.reference.clone(),
            recorded_by: v.recorded_by.clone(),
            original_observed_at: None,
            correction: None,
        })
    }
    fn phase(&self) -> usize {
        match self.kind.as_str() {
            "arrival" => 0,
            "departure" => 2,
            _ => 1,
        }
    }
    fn view(&self, private: bool) -> Value {
        if private {
            json!(self)
        } else {
            json!({"id":self.id,"stop_id":self.stop_id,"kind":self.kind,"source":self.source,"observed_at":self.observed_at,"corrected":self.correction.is_some(),"original_observed_at":self.original_observed_at})
        }
    }
}
fn ordered<'a>(load: &Load, facts: &'a [ExecutionFact]) -> Vec<&'a ExecutionFact> {
    let route = crate::itinerary::route_stops(load);
    let mut facts: Vec<_> = facts.iter().collect();
    facts.sort_by_key(|f| {
        (
            route
                .iter()
                .position(|(id, _, _)| *id == f.stop_id)
                .unwrap_or(usize::MAX),
            f.phase(),
            f.observed_at,
            f.id.as_str(),
        )
    });
    facts
}
fn conflicts<'a>(
    load: &Load,
    facts: &'a [ExecutionFact],
) -> Vec<(&'a ExecutionFact, &'a ExecutionFact)> {
    ordered(load, facts)
        .windows(2)
        .filter(|p| p[0].observed_at > p[1].observed_at)
        .map(|p| (p[0], p[1]))
        .collect()
}
fn facts_from_sources<'a>(
    services: impl Iterator<Item = &'a crate::itinerary::StopService>,
    gates: &[&crate::stops::StopEvent],
    sources: impl Iterator<Item = &'a crate::carrier_portal::CarrierSubmission>,
    correction: impl Fn(
        &crate::carrier_portal::CarrierSubmission,
    ) -> Result<Option<&'a crate::movement::MovementCorrection>>,
) -> Vec<ExecutionFact> {
    let mut facts: Vec<_> = gates.iter().copied().map(ExecutionFact::gate).collect();
    facts.extend(services.map(ExecutionFact::service));
    facts.extend(
        sources
            .filter(|s| s.outcome == "applied")
            .filter_map(|source| {
                let mut fact = ExecutionFact::carrier(source)?;
                if let Ok(Some(correction)) = correction(source) {
                    fact.original_observed_at = Some(fact.observed_at);
                    fact.observed_at = correction.observed_at;
                    fact.correction = Some(json!(correction));
                }
                Some(fact)
            }),
    );
    facts
}
fn timeline_view(
    load: &Load,
    private: bool,
    facts: &[ExecutionFact],
    valid: impl Fn() -> bool,
) -> Value {
    let conflicts = conflicts(load, facts);
    let ordered = ordered(load, facts);
    json!({"consistent":valid() && conflicts.is_empty(),"evidence_valid":valid(),"record_count":facts.len(),"conflicts":conflicts.iter().map(|(before,after)|json!({"before":before.view(private),"after":after.view(private)})).collect::<Vec<_>>(),"stops":crate::itinerary::route_stops(load).iter().enumerate().map(|(i,(id,kind,stop))|json!({"id":id,"sequence":i+1,"kind":kind,"facility":stop.facility,"facts":ordered.iter().filter(|f|f.stop_id==*id).map(|f|f.view(private)).collect::<Vec<_>>()})).collect::<Vec<_>>()})
}
fn indexed_facts(
    movement: &crate::movement::MovementReadIndex<'_>,
    load: &Load,
    gates: &[&crate::stops::StopEvent],
) -> Vec<ExecutionFact> {
    facts_from_sources(
        movement.services().current(load).iter().copied(),
        gates,
        movement.sources(load).iter().copied(),
        |s| movement.current(s),
    )
}
pub(crate) fn consistent_with_movement(
    movement: &crate::movement::MovementReadIndex<'_>,
    load: &Load,
    gates: &[&crate::stops::StopEvent],
) -> bool {
    movement.verify(load).is_ok()
        && conflicts(load, &indexed_facts(movement, load, gates)).is_empty()
}
pub(crate) fn timeline_with_movement(
    movement: &crate::movement::MovementReadIndex<'_>,
    load: &Load,
    private: bool,
    gates: &[&crate::stops::StopEvent],
) -> Value {
    timeline_view(load, private, &indexed_facts(movement, load, gates), || {
        movement.verify(load).is_ok()
    })
}
impl Workspace {
    fn execution_facts(&self, load: &Load) -> Vec<ExecutionFact> {
        self.execution_facts_with_gates(load, &self.current_stop_events(load))
    }
    fn execution_facts_with_gates(
        &self,
        load: &Load,
        gates: &[&crate::stops::StopEvent],
    ) -> Vec<ExecutionFact> {
        facts_from_sources(
            self.current_services(load).into_iter(),
            gates,
            self.carrier_submissions
                .iter()
                .filter(|s| s.load_id == load.id && Some(&s.tender_id) == load.tender_id.as_ref()),
            |s| self.current_movement_correction(s),
        )
    }
    pub fn execution_consistent(&self, load: &Load) -> bool {
        self.verify_movement_corrections(load).is_ok()
            && conflicts(load, &self.execution_facts(load)).is_empty()
    }
    pub(crate) fn execution_consistent_with_gates(
        &self,
        load: &Load,
        gates: &[&crate::stops::StopEvent],
    ) -> bool {
        self.verify_movement_corrections(load).is_ok()
            && conflicts(load, &self.execution_facts_with_gates(load, gates)).is_empty()
    }
    pub(crate) fn validate_execution_fact(
        &self,
        load: &Load,
        replace: Option<&str>,
        fact: ExecutionFact,
    ) -> Result<()> {
        self.verify_movement_corrections(load)?;
        let mut facts = self.execution_facts(load);
        facts.retain(|f| Some(f.id.as_str()) != replace);
        let candidate_id = fact.id.clone();
        facts.push(fact);
        let prior = self.execution_facts(load);
        let prior_conflicts = conflicts(load, &prior);
        let conflicts = conflicts(load, &facts);
        // Repairing one historical record may leave independent conflicts.
        // Permit that repair without allowing a new conflict or hiding one
        // involving the corrected evidence itself.
        let rejected = conflicts.iter().find(|(before, after)| {
            replace.is_none()
                || before.id == candidate_id
                || after.id == candidate_id
                || !prior_conflicts
                    .iter()
                    .any(|(a, b)| a.id == before.id && b.id == after.id)
        });
        if let Some((before, after)) = rejected {
            let route = crate::itinerary::route_stops(load);
            let label = |f: &ExecutionFact| {
                let at = route
                    .iter()
                    .position(|(id, _, _)| *id == f.stop_id)
                    .map_or(0, |i| i + 1);
                format!("stop {at} {}", f.kind)
            };
            return Err(format!("Execution chronology conflict: {} at {} is after {} at {}; correct the original evidence before continuing",label(before),before.observed_at.to_rfc3339(),label(after),after.observed_at.to_rfc3339()));
        }
        Ok(())
    }
    pub fn execution_timeline(&self, load: &Load, private: bool) -> Value {
        self.execution_timeline_with_gates(load, private, &self.current_stop_events(load))
    }
    pub(crate) fn execution_timeline_with_gates(
        &self,
        load: &Load,
        private: bool,
        gates: &[&crate::stops::StopEvent],
    ) -> Value {
        let facts = self.execution_facts_with_gates(load, gates);
        timeline_view(load, private, &facts, || {
            self.verify_movement_corrections(load).is_ok()
        })
    }
}
