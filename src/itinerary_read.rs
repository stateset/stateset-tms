//! Borrowed service history for one immutable workspace read. Global
//! supersession and original record order remain authoritative. No index or
//! validation result survives a write or authorizes a mutation.
use super::{route_stops, StopService};
use crate::domain::{Load, Status, Workspace};
use serde_json::{json, Value};
use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet},
};

type Assignments<'a> = BTreeMap<(&'a str, &'a str), Vec<&'a StopService>>;

pub(crate) struct ServiceReadIndex<'a> {
    history: Assignments<'a>,
    current: Assignments<'a>,
    // Only exact Load objects borrowed from this workspace get memoized. An
    // alternate load with the same business ID must validate its own route.
    validity: BTreeMap<*const Load, OnceCell<bool>>,
    #[cfg(test)]
    checks: std::cell::Cell<usize>,
}

impl<'a> ServiceReadIndex<'a> {
    pub(crate) fn new(workspace: &'a Workspace) -> Self {
        let replaced: BTreeSet<_> = workspace
            .stop_services
            .iter()
            .filter_map(|s| s.supersedes_id.as_deref())
            .collect();
        let mut history: Assignments<'_> = BTreeMap::new();
        let mut current: Assignments<'_> = BTreeMap::new();
        for service in &workspace.stop_services {
            let key = (service.load_id.as_str(), service.tender_id.as_str());
            history.entry(key).or_default().push(service);
            if !replaced.contains(service.id.as_str()) {
                current.entry(key).or_default().push(service);
            }
        }
        Self {
            history,
            current,
            validity: workspace
                .loads
                .iter()
                .map(|l| (l as *const Load, OnceCell::new()))
                .collect(),
            #[cfg(test)]
            checks: std::cell::Cell::new(0),
        }
    }

    fn rows<'b>(rows: &'b Assignments<'a>, load: &'b Load) -> &'b [&'a StopService] {
        load.tender_id
            .as_deref()
            .and_then(|t| rows.get(&(load.id.as_str(), t)))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn current<'b>(&'b self, load: &'b Load) -> &'b [&'a StopService] {
        Self::rows(&self.current, load)
    }

    fn valid(&self, load: &Load) -> bool {
        let verify = || {
            #[cfg(test)]
            self.checks.set(self.checks.get() + 1);
            super::verify_services(load, Self::rows(&self.history, load)).is_ok()
        };
        self.validity
            .get(&(load as *const Load))
            .map_or_else(verify, |cell| *cell.get_or_init(verify))
    }

    pub(crate) fn route_ready(&self, load: &Load) -> bool {
        self.valid(load)
            && load.intermediate_stops.iter().all(|stop| {
                self.current(load)
                    .iter()
                    .filter(|e| e.stop_id == stop.id && json!(e.stop) == json!(stop))
                    .count()
                    == 1
            })
    }

    pub(crate) fn stop_complete(&self, load: &Load, key: &str) -> bool {
        match key {
            "pickup" => matches!(load.status, Status::InTransit | Status::Delivered),
            "delivery" => load.status == Status::Delivered,
            _ => self.valid(load) && self.current(load).iter().any(|s| s.stop_id == key),
        }
    }

    pub(crate) fn view(&self, load: &Load, private: bool) -> Value {
        let services = self.current(load);
        json!({"requires_native_handoff":!load.intermediate_stops.is_empty(),
            "all_intermediate_stops_confirmed":self.route_ready(load),
            "stops":route_stops(load).iter().enumerate().map(|(index,(id,kind,stop))|{
                let service=services.iter().find(|s|s.stop_id==*id);
                json!({"id":id,"sequence":index+1,"kind":kind,"stop":stop,
                    "completed":self.stop_complete(load,id),
                    "service":service.map(|s|if private {json!(s)} else {json!({"observed_at":s.observed_at})})})
            }).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
#[path = "itinerary_read_tests.rs"]
pub(crate) mod tests;
