//! Source-derived work, with human ownership kept separate from shipment truth.
use crate::{
    domain::{self, Result, Status, Workspace},
    store::Store,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

#[path = "operations_digest.rs"]
mod canonical_digest;

fn digest(value: &Value) -> String {
    crate::documents::sha256(value.to_string().as_bytes())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationsPolicy {
    pub version: u64,
    pub tracking_stale_minutes: i64,
}
/// Preserve an unreadable work projection verbatim; it must not poison source
/// shipment decoding or disappear when another workflow commits.
#[derive(Clone, Debug, Default)]
pub struct OperationsIndex {
    records: Vec<OperationsCase>,
    quarantined: Option<Box<RawValue>>,
}
impl std::ops::Deref for OperationsIndex {
    type Target = Vec<OperationsCase>;
    fn deref(&self) -> &Self::Target {
        &self.records
    }
}
impl std::ops::DerefMut for OperationsIndex {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.records
    }
}
impl Serialize for OperationsIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Some(raw) = &self.quarantined {
            crate::retained_json::serialize_literal(raw, serializer)
        } else {
            self.records.serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for OperationsIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        Ok(
            match serde_json::from_str::<Vec<OperationsCase>>(raw.get()) {
                Ok(records) => Self {
                    records,
                    quarantined: None,
                },
                Err(_) => Self {
                    records: vec![],
                    quarantined: Some(raw),
                },
            },
        )
    }
}
#[derive(Clone, Debug)]
pub struct RetainedPolicy {
    value: OperationsPolicy,
    quarantined: Option<Box<RawValue>>,
}
impl From<OperationsPolicy> for RetainedPolicy {
    fn from(value: OperationsPolicy) -> Self {
        Self {
            value,
            quarantined: None,
        }
    }
}
impl Serialize for RetainedPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Some(raw) = &self.quarantined {
            crate::retained_json::serialize_literal(raw, serializer)
        } else {
            self.value.serialize(serializer)
        }
    }
}
impl<'de> Deserialize<'de> for RetainedPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        Ok(match serde_json::from_str::<OperationsPolicy>(raw.get()) {
            Ok(value) => Self {
                value,
                quarantined: None,
            },
            Err(_) => Self {
                value: OperationsPolicy::default(),
                quarantined: Some(raw),
            },
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueQuery {
    pub view: Option<String>,
    pub search: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
impl Default for OperationsPolicy {
    fn default() -> Self {
        Self {
            version: 0,
            tracking_stale_minutes: 240,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub id: String,
    pub load_id: Option<String>,
    pub kind: String,
    pub subject_id: String,
    pub title: String,
    pub summary: String,
    pub next_action: String,
    pub priority: u8,
    pub source: Value,
    pub deadline: Option<DateTime<Utc>>,
}
impl Signal {
    fn hash(&self) -> String {
        canonical_digest::source(self)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEvent {
    pub action: String,
    pub actor: String,
    pub note: String,
    pub at: DateTime<Utc>,
    pub cycle: u64,
    pub source: Option<Signal>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationsCase {
    pub id: String,
    pub version: u64,
    pub cycle: u64,
    pub status: String,
    pub source: Signal,
    pub source_sha256: String,
    pub owner: Option<String>,
    pub follow_up_at: Option<DateTime<Utc>>,
    pub deferred_until: Option<DateTime<Utc>>,
    pub first_detected_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub first_claimed_at: Option<DateTime<Utc>>,
    pub history: Vec<CaseEvent>,
    pub evidence_sha256: String,
}
impl OperationsCase {
    fn summary(&self) -> Value {
        json!({"id":self.id,"version":self.version,"cycle":self.cycle,"status":self.status,"owner":self.owner,"follow_up_at":self.follow_up_at,"deferred_until":self.deferred_until,"first_detected_at":self.first_detected_at,"updated_at":self.updated_at,"resolved_at":self.resolved_at,"first_claimed_at":self.first_claimed_at,"source_sha256":self.source_sha256})
    }
    fn seal(&mut self) {
        self.evidence_sha256 = self.evidence_digest();
    }
    fn evidence_digest(&self) -> String {
        canonical_digest::evidence(self)
    }
    pub fn verify(&self) -> Result<()> {
        if self.evidence_digest() != self.evidence_sha256
            || self.id != self.source.id
            || self.source_sha256 != self.source.hash()
        {
            return Err("Internal storage error".into());
        }
        Ok(())
    }
    fn record(
        &mut self,
        action: &str,
        actor: &str,
        note: &str,
        now: DateTime<Utc>,
        source: bool,
    ) -> Result<()> {
        self.version = self
            .version
            .checked_add(1)
            .ok_or("Case version exhausted")?;
        self.updated_at = now;
        self.history.push(CaseEvent {
            action: action.into(),
            actor: actor.into(),
            note: note.into(),
            at: now,
            cycle: self.cycle,
            source: source.then(|| self.source.clone()),
        });
        self.seal();
        Ok(())
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseInput {
    pub expected_version: u64,
    pub expected_source_active: bool,
    pub source_sha256: String,
    pub action: String,
    pub note: String,
    pub follow_up_at: Option<DateTime<Utc>>,
    pub defer_until: Option<DateTime<Utc>>,
}
/// Both application and original-save resolution bind identical authenticated context.
pub(crate) fn trusted_action(principal: &crate::auth::Principal, request: CaseInput) -> Value {
    json!({"request":request,"is_admin":principal.role==crate::auth::Role::Admin,"subject":principal.subject})
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustedAction {
    request: CaseInput,
    is_admin: bool,
    subject: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyInput {
    expected_version: u64,
    tracking_stale_minutes: i64,
    note: String,
}

/// Validity is reused only within one immutable workspace read. Never cache it
/// across requests: newly read corrupt evidence must still fail closed.
struct OperationsProjection<'a> {
    workspace: &'a Workspace,
    now: DateTime<Utc>,
    valid: bool,
    needs_reconciliation: bool,
    issues: Vec<OperationIssue<'a>>,
}
struct OperationIssue<'a> {
    id: String,
    signal: Cow<'a, Signal>,
    source_sha256: String,
    case: Option<&'a OperationsCase>,
    owner: Option<&'a str>,
    source_active: bool,
    source_changed: bool,
    deferred: bool,
    follow_up_overdue: bool,
    evidence_valid: bool,
}
impl OperationIssue<'_> {
    fn view(&self) -> Value {
        json!({"id":self.id,"signal":self.signal,"source_sha256":self.source_sha256,"case":self.case.map(OperationsCase::summary),"owner":self.owner,"source_active":self.source_active,"source_changed":self.source_changed,"deferred":self.deferred,"follow_up_overdue":self.follow_up_overdue,"evidence_valid":self.evidence_valid})
    }
}
impl OperationsProjection<'_> {
    fn summary(&self) -> Value {
        let mut responses = vec![];
        let mut cycles = 0;
        let mut acknowledged = 0;
        let mut unavailable = 0;
        for c in self
            .workspace
            .operations_cases
            .iter()
            .filter(|_| self.valid)
        {
            let mut detected = BTreeMap::new();
            let mut claimed = BTreeSet::new();
            for e in &c.history {
                if ["detected", "reopened"].contains(&e.action.as_str()) {
                    cycles += 1;
                    detected.insert(e.cycle, e.at);
                }
                if e.action == "claim" && claimed.insert(e.cycle) {
                    acknowledged += 1;
                    if let Some(at) = detected.get(&e.cycle) {
                        if e.at >= *at {
                            responses.push((e.at - *at).num_seconds());
                        } else {
                            unavailable += 1;
                        }
                    } else {
                        unavailable += 1;
                    }
                }
            }
        }
        responses.sort_unstable();
        let median = if responses.is_empty() {
            None
        } else {
            let high = responses.len() / 2;
            Some(
                responses[(responses.len() - 1) / 2]
                    + (responses[high] - responses[(responses.len() - 1) / 2]) / 2,
            )
        };
        let active = self.issues.iter().filter(|r| r.source_active);
        let mut summary = json!({"generated_at":self.now,"policy":self.workspace.operations_policy(),"evidence_valid":self.valid,"needs_reconciliation":self.needs_reconciliation,"metrics":{"active":active.clone().count(),"critical":active.clone().filter(|r|r.signal.priority==0).count(),"unowned":active.clone().filter(|r|r.owner.is_none()).count(),"deferred":active.clone().filter(|r|r.deferred).count(),"follow_ups_overdue":active.filter(|r|r.follow_up_overdue).count(),"resolved":self.workspace.operations_cases.iter().filter(|c|c.status=="resolved").count(),"detected_cycles":cycles,"acknowledged_cycles":acknowledged,"response_time_samples":responses.len(),"response_time_unavailable_cycles":unavailable,"median_first_response_seconds":median}});
        if !self.valid {
            for key in [
                "unowned",
                "deferred",
                "follow_ups_overdue",
                "resolved",
                "detected_cycles",
                "acknowledged_cycles",
                "median_first_response_seconds",
                "response_time_samples",
                "response_time_unavailable_cycles",
            ] {
                summary["metrics"][key] = Value::Null;
            }
        }
        summary
    }
}

impl Workspace {
    pub fn operations_policy(&self) -> OperationsPolicy {
        self.operations_policy
            .as_ref()
            .map(|p| p.value.clone())
            .unwrap_or_default()
    }
    pub fn verify_operations_cases(&self) -> Result<()> {
        if self.operations_cases.quarantined.is_some()
            || self
                .operations_policy
                .as_ref()
                .is_some_and(|p| p.quarantined.is_some())
        {
            return Err("Internal storage error".into());
        }
        if !(15..=10080).contains(&self.operations_policy().tracking_stale_minutes) {
            return Err("Internal storage error".into());
        }
        let mut ids = BTreeSet::new();
        for c in self.operations_cases.iter() {
            c.verify()?;
            if !ids.insert(&c.id) {
                return Err("Internal storage error".into());
            }
        }
        Ok(())
    }
    pub fn operations_signals(&self, now: DateTime<Utc>) -> Vec<Signal> {
        // This immutable scope ends before a reconciliation applies its owned
        // signals. Validity never survives into the following mutation/read.
        let movement = crate::movement::MovementReadIndex::new(self);
        self.operations_signals_with_movement(now, Some(&movement))
    }
    fn operations_signals_with_movement(
        &self,
        now: DateTime<Utc>,
        movement: Option<&crate::movement::MovementReadIndex<'_>>,
    ) -> Vec<Signal> {
        let mut exception_events = BTreeMap::new();
        for event in self.events.iter().filter(|e| e.kind == "load.exception") {
            exception_events.insert(event.entity_id.as_str(), event);
        }
        let recovery_subjects: BTreeSet<_> = if self.outbox.is_empty() && self.inbox.is_empty() {
            BTreeSet::new()
        } else {
            self.operations_cases
                .iter()
                .filter(|c| {
                    c.status != "resolved"
                        && ["outbound_failure", "inbound_failure"].contains(&c.source.kind.as_str())
                })
                .map(|c| (c.source.kind.as_str(), c.source.subject_id.as_str()))
                .collect()
        };
        let mut signals = vec![];
        let mut add = |load: Option<&str>,
                       kind: &str,
                       subject: &str,
                       priority: u8,
                       title: &str,
                       summary: &str,
                       next: &str,
                       source: Value,
                       deadline: Option<DateTime<Utc>>| {
            let id = format!("issue_{}", digest(&json!([load, kind, subject])));
            signals.push(Signal {
                id,
                load_id: load.map(str::to_owned),
                kind: kind.into(),
                subject_id: subject.into(),
                priority,
                title: title.into(),
                summary: summary.into(),
                next_action: next.into(),
                source,
                deadline,
            });
        };
        let gates = crate::stops::CurrentStopEvents::new(&self.stop_events);
        let mut appointments: BTreeMap<&str, Vec<&crate::stops::Appointment>> = BTreeMap::new();
        for appointment in self
            .appointments
            .iter()
            .filter(|a| a.status == "active" && a.ends_at <= now)
        {
            appointments
                .entry(appointment.load_id.as_str())
                .or_default()
                .push(appointment);
        }
        let policy = self.operations_policy();
        let policy_valid = self
            .operations_policy
            .as_ref()
            .is_none_or(|p| p.quarantined.is_none())
            && (15..=10080).contains(&policy.tracking_stale_minutes);
        for l in &self.loads {
            let current_gates = gates.for_load(l);
            let tender = l.tender_id.as_deref().unwrap_or("unassigned");
            if let Some(reason) = &l.exception {
                let event = exception_events.get(l.id.as_str());
                add(Some(&l.id),"shipment_exception",tender,1,"Shipment exception",reason,"Investigate with the carrier and record the actual resolution on the shipment.",json!({"reason":reason,"event_id":event.map(|e|&e.id)}),None);
            }
            if movement
                .map_or_else(|| self.verify_movement_corrections(l), |m| m.verify(l))
                .is_err()
            {
                add(Some(&l.id),"execution_integrity",tender,0,"Execution evidence cannot be verified","Correction history needs controlled recovery.","Reconcile retained original records; financial approval and final writeback remain held.",json!({"tender_id":l.tender_id,"evidence_valid":false}),None);
            } else if !movement.map_or_else(
                || self.execution_consistent_with_gates(l, current_gates),
                |m| m.consistent(l, current_gates),
            ) {
                add(
                    Some(&l.id),
                    "execution_conflict",
                    tender,
                    0,
                    "Actual execution times conflict",
                    "Known gate, service, and movement times disagree.",
                    "Review the execution timeline and correct the original mistaken evidence.",
                    movement.map_or_else(
                        || self.execution_timeline_with_gates(l, false, current_gates),
                        |m| m.timeline(l, false, current_gates),
                    ),
                    None,
                );
            }
            if l.status == Status::Delivered
                && !movement.map_or_else(|| self.route_ready(l), |m| m.services().route_ready(l))
            {
                let missing = crate::itinerary::route_stops(l)
                    .iter()
                    .filter(|(id, _, _)| {
                        !movement.map_or_else(
                            || self.stop_complete(l, id),
                            |m| m.services().stop_complete(l, id),
                        )
                    })
                    .map(|(id, _, _)| id.to_string())
                    .collect::<Vec<_>>();
                add(Some(&l.id),"incomplete_route",tender,0,"Delivered route has unconfirmed stops","Final delivery is recorded; intermediate service evidence remains missing.","Obtain the actual intermediate service confirmations. Do not invent another delivery or POD.",json!({"missing_stop_ids":missing,"tender_id":l.tender_id}),None);
            }
            if l.status == Status::Tendered && l.tender_expires_at.is_some_and(|at| at <= now) {
                add(Some(&l.id),"tender_overdue",tender,1,"Carrier response is overdue","The tender response deadline elapsed.","Review transmission and carrier response; coordinate release before replacing a communicated tender.",json!({"tender_id":l.tender_id,"expires_at":l.tender_expires_at}),l.tender_expires_at);
            }
            if l.status == Status::InTransit && policy_valid {
                let deadline = l.last_tracking_at.map(|at| {
                    at + Duration::minutes(policy.tracking_stale_minutes.clamp(15, 10080))
                });
                if deadline.is_none_or(|at| at <= now) {
                    add(Some(&l.id),"tracking_stale",tender,1,"Shipment needs a tracking update","No sufficiently recent carrier observation is recorded.","Request and record an actual carrier observation; claiming this work does not make tracking current.",json!({"last_tracking_at":l.last_tracking_at,"tracking_stale_minutes":policy.tracking_stale_minutes}),deadline);
                }
            }
            if l.status == Status::Delivered {
                let state = if let Some(id) = &l.pod_document_id {
                    self.documents
                        .iter()
                        .find(|d| &d.id == id)
                        .map(|d| d.status().to_string())
                        .unwrap_or("unavailable".into())
                } else {
                    l.pod_review.as_ref().map(|r| r.decision.clone()).unwrap_or(
                        if l.pod_reference.is_some() {
                            "unreviewed"
                        } else {
                            "missing"
                        }
                        .into(),
                    )
                };
                if state != "accepted" {
                    add(Some(&l.id),"pod_needed",tender,2,"Delivery evidence needs follow-up","Consolidated proof of delivery has not been finance-accepted.","Obtain original POD or coordinate finance review; ownership does not approve the evidence.",json!({"document_id":l.pod_document_id,"reference":l.pod_reference,"review_state":state}),None);
                }
            }
            for a in appointments
                .get(l.id.as_str())
                .into_iter()
                .flatten()
                .filter(|_| l.status != Status::Cancelled)
            {
                if !movement.map_or_else(
                    || self.stop_complete(l, &a.stop),
                    |m| m.services().stop_complete(l, &a.stop),
                ) && !current_gates
                    .iter()
                    .any(|g| g.stop == a.stop && g.kind == "arrival")
                {
                    add(Some(&l.id),"arrival_unrecorded",&a.id,1,"Appointment window elapsed without recorded arrival","An active confirmed stop window has ended; actual arrival is unknown.","Contact the facility or carrier and record the actual arrival. This is not a predicted ETA or proof of physical lateness.",json!({"appointment_id":a.id,"stop_id":a.stop,"ends_at":a.ends_at,"timezone":a.timezone}),Some(a.ends_at));
                }
            }
            if self
                .source_check(&l.id)
                .is_some_and(|s| s.active_review_id.is_some())
            {
                let check = self.source_check(&l.id).expect("check");
                add(Some(&l.id),"source_review",tender,0,"Commerce source requires review","A retained source change is awaiting transportation authorization.","Inspect original and current commerce evidence and record the appropriate customer decision.",json!({"review_id":check.active_review_id,"digest":check.digest,"source_error":check.last_error}),None);
            }
        }
        for incident in self.tender_incidents.iter().filter(|i| i.status == "open") {
            add(Some(&incident.load_id),"carrier_conflict",&incident.id,0,"Carrier custody conflict needs investigation","Evidence from a previous assignment requires reconciliation.","Review the tender incident and obtain actual carrier coordination before resolving it.",json!({"incident_id":incident.id,"tender_id":incident.tender_id}),None);
        }
        for release in self.releases.iter().filter(|r| r.status == "pending") {
            add(Some(&release.load_id),"carrier_release",&release.id,1,"Carrier release is pending","The current carrier retains responsibility until an actual release is applied.","Review the exact release request and carrier confirmation; keep the current assignment until coordination completes.",json!({"release_id":release.id,"tender_id":release.tender_id}),None);
        }
        for job in &self.outbox {
            let following = recovery_subjects.contains(&("outbound_failure", job.id.as_str()));
            // Backoff already represents a retained failed attempt. Keep its
            // work open when a retry clears the error, until the source clears.
            if job.status == "held"
                || ["pending_handoff", "sending", "retry"].contains(&job.status.as_str())
                    && (job.last_error.is_some() || following)
            {
                add(Some(&job.load_id),"outbound_failure",&job.id,1,"Outbound handoff needs recovery","An integration handoff failed and is awaiting recovery or a retry result.","Inspect the retained handoff and its destination. Retry only after reviewing the actual error and source holds.",json!({"outbox_id":job.id,"destination":job.destination,"status":job.status,"total_attempts":job.total_attempts,"error":job.last_error}),None);
            }
        }
        for job in &self.inbox {
            let following = recovery_subjects.contains(&("inbound_failure", job.id.as_str()));
            if job.status == "held"
                || ["pending", "processing", "retry"].contains(&job.status.as_str())
                    && (job.last_error.is_some() || following)
            {
                add(None,"inbound_failure",&job.id,1,"Inbound document needs recovery","A retained EDI document requires inspection or is awaiting its retry result.","Review the original inbound document and processing error; a queue action does not apply its business event.",json!({"inbox_id":job.id,"document_id":job.document_id,"status":job.status,"total_attempts":job.total_attempts,"error":job.last_error}),None);
            }
        }
        signals.sort_by(|a, b| (a.priority, &a.id).cmp(&(b.priority, &b.id)));
        signals
    }
    pub fn operations_need_reconciliation(&self, now: DateTime<Utc>) -> bool {
        if self.verify_operations_cases().is_err() {
            return false;
        }
        let cases: BTreeMap<_, _> = self
            .operations_cases
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();
        let signals = self.operations_signals(now);
        if signals.iter().any(|s| {
            cases.get(s.id.as_str()).is_none_or(|c| {
                c.status == "resolved"
                    || c.source_sha256 != s.hash()
                    || c.deferred_until.is_some_and(|at| at <= now)
            })
        }) {
            return true;
        }
        let active: BTreeSet<_> = signals.iter().map(|s| s.id.as_str()).collect();
        self.operations_cases
            .iter()
            .any(|c| c.status != "resolved" && !active.contains(c.id.as_str()))
    }
    pub fn reconcile_operations(&mut self, now: DateTime<Utc>) -> usize {
        // A broken work index must never prevent a real carrier event or bank
        // reconciliation. Its own mutations fail closed until records recover.
        if self.verify_operations_cases().is_err() {
            return 0;
        }
        let signals = self.operations_signals(now);
        let active_ids: BTreeSet<_> = signals.iter().map(|s| s.id.as_str()).collect();
        let mut case_indices: BTreeMap<_, _> = self
            .operations_cases
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        let mut changes = 0;
        for signal in &signals {
            let source_sha256 = signal.hash();
            let Some(index) = case_indices.get(&signal.id).copied() else {
                let mut c = OperationsCase {
                    id: signal.id.clone(),
                    version: 0,
                    cycle: 1,
                    status: "open".into(),
                    source: signal.clone(),
                    source_sha256,
                    owner: None,
                    follow_up_at: None,
                    deferred_until: None,
                    first_detected_at: now,
                    updated_at: now,
                    resolved_at: None,
                    first_claimed_at: None,
                    history: vec![],
                    evidence_sha256: String::new(),
                };
                if c.record(
                    "detected",
                    "system:operations",
                    "Current source condition detected",
                    now,
                    true,
                )
                .is_ok()
                {
                    case_indices.insert(c.id.clone(), self.operations_cases.len());
                    self.operations_cases.push(c);
                    changes += 1;
                }
                continue;
            };
            let c = &self.operations_cases[index];
            let reopened = c.status == "resolved";
            let changed = c.source_sha256 != source_sha256;
            let expired = c.deferred_until.is_some_and(|at| at <= now);
            if !reopened && !changed && !expired {
                continue;
            }
            let mut c = c.clone();
            if reopened {
                let Some(cycle) = c.cycle.checked_add(1) else {
                    continue;
                };
                c.cycle = cycle;
                c.owner = None;
                c.follow_up_at = None;
                c.first_claimed_at = None;
                c.status = "open".into();
                c.resolved_at = None;
            }
            c.source = signal.clone();
            c.source_sha256 = source_sha256;
            c.deferred_until = None;
            let action = if reopened {
                "reopened"
            } else if changed {
                "source_changed"
            } else {
                "resumed"
            };
            if c.record(action,"system:operations","Current source condition rechecked; retained ownership does not suppress changed evidence",now,reopened||changed).is_ok(){self.operations_cases[index]=c;changes+=1;}
        }
        for c in self.operations_cases.iter_mut() {
            if c.status != "resolved" && !active_ids.contains(c.id.as_str()) {
                let mut resolved = c.clone();
                resolved.status = "resolved".into();
                resolved.resolved_at = Some(now);
                resolved.deferred_until = None;
                if resolved.record("source_cleared","system:operations","The original source condition no longer applies; this is not evidence of an external delivery or payment",now,false).is_ok(){*c=resolved;changes+=1;}
            }
        }
        changes
    }
    fn operations_projection(&self, now: DateTime<Utc>) -> OperationsProjection<'_> {
        let movement = crate::movement::MovementReadIndex::new(self);
        self.operations_projection_with_movement(now, Some(&movement))
    }
    fn operations_projection_with_movement(
        &self,
        now: DateTime<Utc>,
        movement: Option<&crate::movement::MovementReadIndex<'_>>,
    ) -> OperationsProjection<'_> {
        let valid = self.verify_operations_cases().is_ok();
        let cases: BTreeMap<_, _> = self
            .operations_cases
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();
        let mut issues = vec![];
        let mut ids = BTreeSet::new();
        for signal in self.operations_signals_with_movement(now, movement) {
            let case = cases.get(signal.id.as_str()).copied().filter(|_| valid);
            let source_sha256 = signal.hash();
            ids.insert(signal.id.clone());
            let owner = case
                .filter(|c| c.status != "resolved")
                .and_then(|c| c.owner.as_deref());
            let source_changed =
                case.is_some_and(|c| c.source_sha256 != source_sha256 || c.status == "resolved");
            let deferred = case.is_some_and(|c| {
                c.deferred_until.is_some_and(|at| at > now)
                    && c.source_sha256 == source_sha256
                    && signal.priority > 0
            });
            let follow_up_overdue = case.is_some_and(|c| {
                c.follow_up_at.is_some_and(|at| at <= now) && c.status != "resolved"
            });
            issues.push(OperationIssue {
                id: signal.id.clone(),
                signal: Cow::Owned(signal),
                source_sha256,
                case,
                owner,
                source_active: true,
                source_changed,
                deferred,
                follow_up_overdue,
                evidence_valid: valid,
            });
        }
        for case in self.operations_cases.iter() {
            if !ids.contains(&case.id) {
                issues.push(OperationIssue {
                    id: case.id.clone(),
                    signal: Cow::Borrowed(&case.source),
                    source_sha256: case.source_sha256.clone(),
                    case: valid.then_some(case),
                    owner: valid.then_some(case.owner.as_deref()).flatten(),
                    source_active: false,
                    source_changed: case.status != "resolved",
                    deferred: false,
                    follow_up_overdue: false,
                    evidence_valid: valid,
                });
            }
        }
        let needs_reconciliation = valid
            && issues.iter().any(|r| {
                r.source_changed
                    || r.source_active
                        && (r.case.is_none()
                            || r.case
                                .is_some_and(|c| c.deferred_until.is_some_and(|at| at <= now)))
            });
        issues.sort_by(|a, b| {
            (
                !a.source_active,
                a.signal.priority,
                !a.follow_up_overdue,
                a.id.as_str(),
            )
                .cmp(&(
                    !b.source_active,
                    b.signal.priority,
                    !b.follow_up_overdue,
                    b.id.as_str(),
                ))
        });
        OperationsProjection {
            workspace: self,
            now,
            valid,
            needs_reconciliation,
            issues,
        }
    }
    pub fn operations_board(&self, now: DateTime<Utc>) -> Value {
        let projection = self.operations_projection(now);
        let mut board = projection.summary();
        board["issues"] = json!(projection
            .issues
            .iter()
            .map(OperationIssue::view)
            .collect::<Vec<_>>());
        board
    }
    pub fn operations_summary(&self, now: DateTime<Utc>) -> Value {
        self.operations_projection(now).summary()
    }
    pub fn operations_page(
        &self,
        query: QueueQuery,
        principal: &crate::auth::Principal,
        now: DateTime<Utc>,
    ) -> Result<Value> {
        let view = query.view.as_deref().unwrap_or("actionable");
        if ![
            "actionable",
            "active",
            "mine",
            "unowned",
            "deferred",
            "resolved",
            "all",
        ]
        .contains(&view)
        {
            return Err("Unknown operations queue view".into());
        }
        let limit = query.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err("Queue page size must be 1–100".into());
        }
        let search = query.search.unwrap_or_default().trim().to_lowercase();
        if search.len() > 200 {
            return Err("Queue search must be at most 200 characters".into());
        }
        let projection = self.operations_projection(now);
        let mut loads = BTreeMap::new();
        for load in &self.loads {
            loads.entry(load.id.as_str()).or_insert(load);
        }
        let reference = |row: &OperationIssue<'_>| {
            row.signal
                .load_id
                .as_deref()
                .and_then(|id| loads.get(id))
                .map(|l| l.reference.as_str())
        };
        let issues = projection
            .issues
            .iter()
            .filter(|r| {
                let matches_view = match view {
                    "actionable" => r.source_active && !r.deferred,
                    "active" => r.source_active,
                    "mine" => r.source_active && r.owner == Some(principal.subject.as_str()),
                    "unowned" => r.source_active && r.owner.is_none(),
                    "deferred" => r.source_active && r.deferred,
                    "resolved" => !r.source_active,
                    _ => true,
                };
                matches_view
                    && (search.is_empty()
                        || format!(
                            "{} {} {}",
                            reference(r).unwrap_or(""),
                            r.signal.title,
                            r.signal.summary
                        )
                        .to_lowercase()
                        .contains(&search))
            })
            .collect::<Vec<_>>();
        let total = issues.len();
        let scope = digest(&json!([
            principal.tenant_id,
            principal.subject,
            view,
            search,
            limit,
            issues.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
        ]));
        let start = if let Some(cursor) = query.cursor {
            let (id, hash) = cursor.split_once('.').ok_or("Invalid queue cursor")?;
            if hash != scope {
                return Err("Queue changed; refresh before loading another page".into());
            }
            issues
                .iter()
                .position(|r| r.id == id)
                .ok_or("Queue changed; refresh before loading another page")?
                + 1
        } else {
            0
        };
        let page = issues
            .into_iter()
            .skip(start)
            .take(limit)
            .collect::<Vec<_>>();
        let next_cursor = if start + page.len() < total {
            page.last().map(|r| format!("{}.{scope}", r.id))
        } else {
            None
        };
        let rows = page
            .into_iter()
            .map(|r| {
                let mut value = r.view();
                value["load_reference"] = json!(reference(r));
                value
            })
            .collect::<Vec<_>>();
        let mut board = projection.summary();
        board["next_cursor"] = json!(next_cursor);
        board["issues"] = json!(rows);
        board["total"] = json!(total);
        board["view"] = json!(view);
        Ok(board)
    }
    pub fn operations_case(&self, id: &str, now: DateTime<Utc>) -> Result<Value> {
        let projection = self.operations_projection(now);
        let issue = projection
            .issues
            .iter()
            .find(|r| r.id == id)
            .ok_or("Issue not found")?;
        let mut row = issue.view();
        row["case"] = json!(self.operations_cases.iter().find(|c| c.id == id));
        row["load_reference"] = json!(self
            .loads
            .iter()
            .find(|l| Some(l.id.as_str()) == issue.signal.load_id.as_deref())
            .map(|l| &l.reference));
        Ok(row)
    }
    pub fn apply_operations(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        if operation == "operations.reconcile" {
            if body != json!({}) {
                return Err("Reconciliation body must be empty".into());
            }
            self.verify_operations_cases()?;
            let count = self.reconcile_operations(Utc::now());
            return Ok(json!({"changed":count}));
        }
        if operation == "operations.policy" {
            if self
                .operations_policy
                .as_ref()
                .is_some_and(|p| p.quarantined.is_some())
            {
                return Err("Internal storage error".into());
            }
            let input: PolicyInput =
                serde_json::from_value(body).map_err(|_| "Invalid operations policy fields")?;
            domain::required(&input.note, "Policy change reason")?;
            let policy = self.operations_policy();
            if policy.version != input.expected_version {
                return Err("Policy changed; refresh before editing".into());
            }
            if !(15..=10080).contains(&input.tracking_stale_minutes) {
                return Err("Tracking interval must be 15–10080 minutes".into());
            }
            if policy.tracking_stale_minutes == input.tracking_stale_minutes {
                return Err("Policy must change the tracking interval".into());
            }
            let next = OperationsPolicy {
                version: policy
                    .version
                    .checked_add(1)
                    .ok_or("Policy version exhausted")?,
                tracking_stale_minutes: input.tracking_stale_minutes,
            };
            self.operations_policy = Some(next.clone().into());
            self.record(
                "operations",
                "operations.policy",
                format!("Tracking follow-up policy changed: {}", input.note.trim()),
            );
            if let Some(event) = self.events.last_mut() {
                event.details = Some(json!({"previous":policy,"current":next}));
            }
            return Ok(json!(next));
        }
        if operation != "operations.case.action" {
            return Err("Unsupported operations command".into());
        }
        let trusted: TrustedAction =
            serde_json::from_value(body).map_err(|_| "Invalid operations action fields")?;
        let input = trusted.request;
        if trusted.subject != actor {
            return Err("Operations identity does not match the authenticated actor".into());
        }
        domain::required(&input.note, "Action findings")?;
        self.verify_operations_cases()?;
        let now = Utc::now();
        let current = self.operations_cases.iter().find(|c| c.id == entity);
        if current.map_or(0, |c| c.version) != input.expected_version {
            return Err("Issue changed; refresh and review before acting".into());
        }
        let signal = self
            .operations_signals(now)
            .into_iter()
            .find(|s| s.id == entity);
        if input.expected_source_active != signal.is_some() {
            return Err(
                "Issue source state changed; refresh and review its current evidence".into(),
            );
        }
        let source = signal
            .as_ref()
            .or(current.map(|c| &c.source))
            .ok_or("Issue not found")?;
        if source.hash() != input.source_sha256 {
            return Err("Issue source changed; refresh and review its current evidence".into());
        }
        if signal.is_none() && input.action != "note" {
            return Err("Issue source is clear; refresh the work queue".into());
        }
        if input
            .follow_up_at
            .is_some_and(|at| at <= now || at > now + Duration::days(7))
        {
            return Err("Follow-up must be in the future and within seven days".into());
        }
        if input.action != "snooze" && input.defer_until.is_some() {
            return Err("Deferred time is only valid for snooze".into());
        }
        if !["claim", "note"].contains(&input.action.as_str()) && input.follow_up_at.is_some() {
            return Err("Follow-up time is only valid for claim or note".into());
        }
        self.reconcile_operations(now);
        let index = self
            .operations_cases
            .iter()
            .position(|c| c.id == entity)
            .ok_or("Issue not found")?;
        let mut case = self.operations_cases[index].clone();
        let owns = case.owner.as_deref() == Some(actor);
        match input.action.as_str() {
            "claim" => {
                if case.owner.is_some() {
                    return Err(
                        "Issue already has an owner; coordinate release before claiming".into(),
                    );
                }
                let due = input
                    .follow_up_at
                    .ok_or("A named follow-up time is required")?;
                case.owner = Some(actor.into());
                case.follow_up_at = Some(due);
                case.status = "working".into();
                case.deferred_until = None;
                case.first_claimed_at.get_or_insert(now);
            }
            "release" | "force_release" => {
                if input.action == "force_release" && !trusted.is_admin {
                    return Err("Administrator required to release another owner".into());
                }
                if !(owns || input.action == "force_release" && trusted.is_admin) {
                    return Err("Only the current owner can release this work".into());
                }
                if case.owner.is_none() {
                    return Err("Issue has no owner to release".into());
                }
                case.owner = None;
                case.follow_up_at = None;
                case.deferred_until = None;
                case.status = "open".into();
            }
            "note" => {
                if input.follow_up_at.is_some() {
                    if !owns || case.status == "resolved" {
                        return Err("Only the current owner can change an active follow-up".into());
                    }
                    case.follow_up_at = input.follow_up_at;
                    if case
                        .deferred_until
                        .is_some_and(|at| input.follow_up_at.is_some_and(|due| at > due))
                    {
                        case.deferred_until = None;
                    }
                }
            }
            "snooze" => {
                if !owns {
                    return Err("Only the current owner can defer this work".into());
                }
                if case.source.priority == 0 {
                    return Err("Critical source holds cannot be deferred".into());
                }
                let until = input.defer_until.ok_or("Deferred time required")?;
                if until <= now
                    || until > now + Duration::hours(24)
                    || case.follow_up_at.is_none_or(|due| until > due)
                {
                    return Err("Defer until a future time within 24 hours and no later than the next follow-up".into());
                }
                case.deferred_until = Some(until);
            }
            "resume" => {
                if !owns {
                    return Err("Only the current owner can resume this work".into());
                }
                if case.deferred_until.is_none() {
                    return Err("Issue is not deferred".into());
                }
                case.deferred_until = None;
            }
            _ => return Err(
                "Unsupported case action; clear the underlying source condition to resolve work"
                    .into(),
            ),
        }
        case.record(&input.action, actor, input.note.trim(), now, false)?;
        self.operations_cases[index] = case.clone();
        self.record(
            entity,
            "operations.case.action",
            format!("Operations work {}: {}", input.action, input.note.trim()),
        );
        if let Some(event) = self.events.last_mut() {
            event.details = Some(
                json!({"case_id":case.id,"action":input.action,"version":case.version,"cycle":case.cycle}),
            );
        }
        Ok(json!(case))
    }
}
impl Store {
    pub async fn reconcile_operations(&self, tenant: &str) -> Result<usize> {
        let now = Utc::now();
        if !self.read(tenant).await?.operations_need_reconciliation(now) {
            return Ok(0);
        }
        let result = self
            .update(tenant, |w| Ok(json!(w.reconcile_operations(Utc::now()))))
            .await?;
        Ok(result.as_u64().unwrap_or(0) as usize)
    }
}
pub async fn worker(store: Store) {
    worker_until(store, crate::lifecycle::Shutdown::default()).await;
}
pub(crate) fn summary_with_movement(
    movement: &crate::movement::MovementReadIndex<'_>,
    now: DateTime<Utc>,
) -> Value {
    movement
        .workspace()
        .operations_projection_with_movement(now, Some(movement))
        .summary()
}
#[cfg(test)]
pub(crate) fn preceding_summary(workspace: &Workspace, now: DateTime<Utc>) -> Value {
    workspace
        .operations_projection_with_movement(now, None)
        .summary()
}
#[cfg(test)]
pub(crate) fn preceding_signals(workspace: &Workspace, now: DateTime<Utc>) -> Vec<Signal> {
    workspace.operations_signals_with_movement(now, None)
}
pub async fn worker_until(store: Store, shutdown: crate::lifecycle::Shutdown) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            _ = interval.tick() => {}
        }
        match store.tenant_ids().await {
            Ok(tenants) => {
                for tenant in tenants {
                    if shutdown.requested() {
                        return;
                    }
                    if store.reconcile_operations(&tenant).await.is_err() {
                        tracing::warn!(
                            "Operations reconciliation failed; source workflows remain available"
                        );
                    }
                }
            }
            Err(_) => {
                tracing::warn!("Operations reconciliation cannot enumerate compatible workspaces")
            }
        }
    }
}
