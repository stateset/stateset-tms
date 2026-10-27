//! Movement lookups and validity belong to one immutable workspace read only.
//! Preserve original order, all duplicate identities and global source chains.
use super::{current_chain, source_view, visible, CarrierSubmission, MovementCorrection};
use crate::domain::{Load, Result, Status, Workspace};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::{
    cell::{OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet},
};

type AssignmentRows<'a, T> = BTreeMap<&'a str, BTreeMap<&'a str, Vec<&'a T>>>;
type Validity = BTreeMap<String, BTreeMap<String, Result<()>>>;

pub(crate) struct MovementReadIndex<'a> {
    workspace: &'a Workspace,
    services: crate::itinerary::ServiceReadIndex<'a>,
    sources: AssignmentRows<'a, CarrierSubmission>,
    source_ids: BTreeMap<&'a str, Vec<&'a CarrierSubmission>>,
    corrections: AssignmentRows<'a, MovementCorrection>,
    chains: BTreeMap<&'a str, Vec<&'a MovementCorrection>>,
    locked: OnceCell<BTreeSet<&'a str>>,
    validity: RefCell<Validity>,
    #[cfg(test)]
    checks: std::cell::Cell<usize>,
}
impl<'a> MovementReadIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        let mut sources: AssignmentRows<'_, CarrierSubmission> = BTreeMap::new();
        let mut source_ids: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut corrections: AssignmentRows<'_, MovementCorrection> = BTreeMap::new();
        let mut chains: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for source in &workspace.carrier_submissions {
            sources
                .entry(source.load_id.as_str())
                .or_default()
                .entry(source.tender_id.as_str())
                .or_default()
                .push(source);
            source_ids
                .entry(source.id.as_str())
                .or_default()
                .push(source);
        }
        for correction in &workspace.movement_corrections {
            corrections
                .entry(correction.load_id.as_str())
                .or_default()
                .entry(correction.tender_id.as_str())
                .or_default()
                .push(correction);
            // Never scope this map to an assignment: foreign/misbound records
            // claiming the same source must still invalidate its whole chain.
            chains
                .entry(correction.source_id.as_str())
                .or_default()
                .push(correction);
        }
        Self {
            workspace,
            services: crate::itinerary::ServiceReadIndex::new(workspace),
            sources,
            source_ids,
            corrections,
            chains,
            locked: OnceCell::new(),
            validity: RefCell::default(),
            #[cfg(test)]
            checks: std::cell::Cell::new(0),
        }
    }
    pub(crate) fn services(&self) -> &crate::itinerary::ServiceReadIndex<'a> {
        &self.services
    }
    pub(crate) fn workspace(&self) -> &'a Workspace {
        self.workspace
    }
    pub(crate) fn sources(&self, load: &Load) -> &[&'a CarrierSubmission] {
        load.tender_id
            .as_deref()
            .and_then(|tender| {
                self.sources
                    .get(load.id.as_str())
                    .and_then(|rows| rows.get(tender))
            })
            .map_or(&[], Vec::as_slice)
    }
    fn corrections(&self, load: &Load) -> &[&'a MovementCorrection] {
        load.tender_id
            .as_deref()
            .and_then(|tender| {
                self.corrections
                    .get(load.id.as_str())
                    .and_then(|rows| rows.get(tender))
            })
            .map_or(&[], Vec::as_slice)
    }
    fn chain(&self, source: &str) -> &[&'a MovementCorrection] {
        self.chains.get(source).map_or(&[], Vec::as_slice)
    }
    pub(crate) fn current(
        &self,
        source: &CarrierSubmission,
    ) -> Result<Option<&'a MovementCorrection>> {
        current_chain(source, self.chain(&source.id).iter().copied())
    }
    pub(crate) fn verify(&self, load: &Load) -> Result<()> {
        let Some(tender) = load.tender_id.as_deref() else {
            return Ok(());
        };
        if let Some(result) = self
            .validity
            .borrow()
            .get(load.id.as_str())
            .and_then(|t| t.get(tender))
        {
            return result.clone();
        }
        // Lazy validation preserves the preceding per-load evaluation/error
        // order; do not eagerly inspect an unused historical assignment.
        let result = self.verify_uncached(load);
        self.validity
            .borrow_mut()
            .entry(load.id.clone())
            .or_default()
            .insert(tender.to_owned(), result.clone());
        result
    }
    fn verify_uncached(&self, load: &Load) -> Result<()> {
        #[cfg(test)]
        self.checks.set(self.checks.get() + 1);
        for source in
            self.sources(load).iter().copied().filter(|s| {
                s.outcome == "applied" && ["pickup", "delivery"].contains(&s.kind.as_str())
            })
        {
            self.current(source)?;
        }
        for correction in self.corrections(load) {
            let source = self
                .source_ids
                .get(correction.source_id.as_str())
                .into_iter()
                .flatten()
                .copied()
                .find(|s| {
                    s.load_id == load.id
                        && s.tender_id == correction.tender_id
                        && s.carrier_id == correction.carrier_id
                        && s.outcome == "applied"
                        && ["pickup", "delivery"].contains(&s.kind.as_str())
                })
                .ok_or("Internal storage error")?;
            self.current(source)?;
        }
        Ok(())
    }
    pub(crate) fn source_views(&self, load: &Load) -> Value {
        let locked = self
            .locked
            .get_or_init(|| {
                self.workspace
                    .invoices
                    .iter()
                    .filter(|i| i.status == "approved")
                    .map(|i| i.load_id.as_str())
                    .chain(
                        self.workspace
                            .settlements
                            .iter()
                            .map(|s| s.load_id.as_str()),
                    )
                    .chain(
                        self.workspace
                            .bills
                            .iter()
                            .filter(|b| b.status != "void")
                            .map(|b| b.load_id.as_str()),
                    )
                    .collect()
            })
            .contains(load.id.as_str());
        let valid = self.verify(load).is_ok();
        let physical = matches!(load.status, Status::InTransit | Status::Delivered);
        json!(self
            .sources(load)
            .iter()
            .copied()
            .filter(|s| visible(s, load))
            .map(|s| {
                source_view(
                    s,
                    self.current(s),
                    self.chain(&s.id),
                    physical,
                    locked,
                    valid,
                )
            })
            .collect::<Vec<_>>())
    }
    pub(crate) fn consistent(&self, load: &Load, gates: &[&crate::stops::StopEvent]) -> bool {
        crate::execution::consistent_with_movement(self, load, gates)
    }
    pub(crate) fn timeline(
        &self,
        load: &Load,
        private: bool,
        gates: &[&crate::stops::StopEvent],
    ) -> Value {
        crate::execution::timeline_with_movement(self, load, private, gates)
    }
    pub(crate) fn delivery_ready(&self, load: &Load, gates: &[&crate::stops::StopEvent]) -> bool {
        self.workspace.delivery_evidence_ready_with_route(
            load,
            || self.services.route_ready(load),
            || self.consistent(load, gates),
        )
    }
    pub(crate) fn operations_summary(&self, now: DateTime<Utc>) -> Value {
        crate::operations::summary_with_movement(self, now)
    }
}

#[cfg(test)]
#[path = "movement_read_tests.rs"]
mod tests;
