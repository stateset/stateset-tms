//! Canonical commerce observations and explicit transport authorization.
//! An upstream order change never erases carrier custody or financial history.
use crate::domain::{self, Load, Result, Status, Workspace};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const SOURCE_HOLD: &str = "Sync source review required before new transportation commitments";
pub const SOURCE_UNAVAILABLE: &str =
    "Sync source check is unavailable or expired; refresh before committing transport";
/// Physical facts use the current tenant transaction directly. New transport
/// commitments still need a separate canonical-source observation before it.
pub(crate) fn requires_source_snapshot(operation: &str, body: &Value) -> bool {
    (operation == "load.command" && matches!(body["action"].as_str(), Some("tender" | "dispatch")))
        || matches!(
            operation,
            "commerce.tracking"
                | "quote.book"
                | "quote.reprocure"
                | "quote.contract-reprocure"
                | "quote.decision"
                | "rate.issue"
                | "portal.publish"
                | "portal.respond"
                | "portal.quote.publish"
                | "portal.quote.respond"
        )
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceCheck {
    pub load_id: String,
    pub order_id: String,
    pub version: u64,
    pub snapshot: Value,
    pub digest: String,
    pub checked_at: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub checked_by: String,
    pub last_error: Option<String>,
    pub error_at: Option<DateTime<Utc>>,
    pub active_review_id: Option<String>,
    pub authorized_review_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceReview {
    pub id: String,
    pub load_id: String,
    pub order_id: String,
    pub baseline: Value,
    pub snapshot: Value,
    pub digest: String,
    pub changed_fields: Vec<String>,
    pub plan_snapshot: Value,
    pub observed_at: DateTime<Utc>,
    pub observed_by: String,
    pub status: String,
    pub version: u64,
    pub superseded_by_id: Option<String>,
    pub decision: Option<SourceDecision>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceDecision {
    #[serde(default)]
    pub quote_id: Option<String>,
    pub decision: String,
    pub contact: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub plan_snapshot: Value,
}
#[derive(Clone, Debug)]
pub struct SourceGrant {
    pub load_id: String,
    pub version: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    expected_load_version: u64,
    expected_version: u64,
    expected_digest: String,
    decision: String,
    contact: String,
    reference: String,
    note: String,
}
fn hash(value: &Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
pub fn status(snapshot: &Value) -> String {
    snapshot["status"]
        .as_str()
        .unwrap_or("unknown")
        .trim()
        .to_ascii_lowercase()
}
pub fn needs_explicit_authorization(snapshot: &Value) -> bool {
    !["pending", "processing", "shipped", "fulfilled", "delivered"]
        .contains(&status(snapshot).as_str())
}
pub fn tracking_status_safe(snapshot: &Value) -> bool {
    ["pending", "processing", "shipped"].contains(&status(snapshot).as_str())
}
fn projection(snapshot: &Value, own_tracking: bool) -> Value {
    let mut result = snapshot.clone();
    if let Some(object) = result.as_object_mut() {
        object.retain(|key, _| {
            !matches!(
                key.as_str(),
                "updatedAt"
                    | "tracking"
                    | "latestMovement"
                    | "netsuiteSyncedAt"
                    | "netsuiteLastError"
                    | "netsuiteLastAttemptAt"
                    | "dclSubmittedAt"
                    | "dclLastError"
                    | "dclLastAttemptAt"
                    | "luminousSyncedAt"
                    | "luminousLastError"
                    | "luminousLastAttemptAt"
                    | "capacitySubmittedAt"
                    | "capacityLastError"
                    | "capacityLastAttemptAt"
            )
        });
        let raw = status(snapshot);
        object.insert(
            "status".into(),
            json!(
                if raw == "pending" || raw == "processing" || (raw == "shipped" && own_tracking) {
                    "active"
                } else if raw == "canceled" {
                    "cancelled"
                } else {
                    raw.as_str()
                }
            ),
        );
        if let Some(Value::Array(tags)) = object.get_mut("tags") {
            tags.sort_by_key(Value::to_string);
            tags.dedup();
        }
    }
    result
}
fn bump(v: u64) -> Result<u64> {
    v.checked_add(1)
        .ok_or_else(|| "Source review version exhausted".into())
}
impl Workspace {
    pub fn commitment_source(&self, operation: &str, entity: &str, body: &Value) -> Option<&Load> {
        let load = match operation {
            "portal.quote.publish" => self
                .commitment_quote(operation, entity)
                .and_then(|q| q.current().ok())
                .and_then(|r| r.source_load.as_ref())
                .map(|l| l.id.as_str()),
            "portal.quote.respond" if body["request"]["decision"] == "accept" => self
                .commitment_quote(operation, entity)
                .and_then(|q| q.current().ok())
                .and_then(|r| r.source_load.as_ref())
                .map(|l| l.id.as_str()),
            "rate.issue" | "commerce.tracking" => Some(entity),
            "portal.publish" => Some(entity),
            "portal.respond" if body["request"]["decision"] == "accept" => self
                .portal_offers
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.as_str()),
            "load.command" if requires_source_snapshot(operation, body) => Some(entity),
            "quote.book" => self
                .quotes
                .iter()
                .find(|v| v.id == entity)
                .and_then(|q| q.revisions.last())
                .and_then(|r| r.source_load.as_ref())
                .map(|v| v.id.as_str()),
            "quote.decision" if body["decision"] == "accept" => self
                .quotes
                .iter()
                .find(|v| v.id == entity)
                .and_then(|q| q.revisions.last())
                .filter(|r| r.amendment_of.is_some())
                .and_then(|r| r.source_load.as_ref())
                .map(|v| v.id.as_str()),
            "quote.reprocure" | "quote.contract-reprocure" => self
                .quotes
                .iter()
                .find(|v| v.id == entity)
                .and_then(|q| q.booked_load_id.as_deref()),
            _ => None,
        }?;
        self.loads
            .iter()
            .find(|v| v.id == load && v.sync_order_id.is_some())
    }
    pub fn source_check(&self, load: &str) -> Option<&SourceCheck> {
        self.source_checks.iter().find(|v| v.load_id == load)
    }
    fn own_tracking_attempted(&self, load: &str) -> bool {
        self.outbox.iter().any(|v| {
            v.load_id == load
                && v.destination == "stateset-sync-server"
                && v.transmission == "attempted"
        })
    }
    pub fn guard_source(&self, load: &Load) -> Result<()> {
        if load.sync_order_id.is_none() {
            return Ok(());
        }
        let check = self.source_check(&load.id).ok_or(SOURCE_UNAVAILABLE)?;
        if check.last_error.is_some() || check.valid_until.is_none_or(|v| v <= Utc::now()) {
            return Err(SOURCE_UNAVAILABLE.into());
        }
        if check.active_review_id.is_some() {
            return Err(SOURCE_HOLD.into());
        }
        if let Some(id) = &check.authorized_review_id {
            let review = self
                .source_reviews
                .iter()
                .find(|v| &v.id == id)
                .ok_or(SOURCE_HOLD)?;
            let proof = review.decision.as_ref().ok_or(SOURCE_HOLD)?;
            if proof.plan_snapshot != json!(crate::quotes::QuotePlan::from_load(load)) {
                return Err(SOURCE_HOLD.into());
            }
        } else if needs_explicit_authorization(&check.snapshot)
            || ["fulfilled", "shipped", "delivered"].contains(&status(&check.snapshot).as_str())
                && !self.own_tracking_attempted(&load.id)
        {
            return Err(SOURCE_HOLD.into());
        }
        Ok(())
    }
    pub fn check_source_grant(&self, grant: &SourceGrant) -> Result<()> {
        let check = self
            .source_check(&grant.load_id)
            .ok_or(SOURCE_UNAVAILABLE)?;
        if check.version != grant.version {
            return Err(
                "Sync source check changed; retry the action against the latest observation".into(),
            );
        }
        let load = self
            .loads
            .iter()
            .find(|v| v.id == grant.load_id)
            .ok_or("Load not found")?;
        self.guard_source(load)
    }
    pub fn source_summary(&self, load: &Load) -> Value {
        if load.sync_order_id.is_none() {
            return Value::Null;
        }
        let check = self.source_check(&load.id);
        let readiness = self.guard_source(load);
        json!({"check":check,"held":check.is_some_and(|v|v.active_review_id.is_some()) || readiness.as_ref().err().is_some_and(|v|v==SOURCE_HOLD),"unavailable":check.is_none_or(|v|v.last_error.is_some()||v.valid_until.is_none_or(|at|at<=Utc::now())),"commitment_ready":readiness.is_ok(),"writeback_safe":check.is_some_and(|v|tracking_status_safe(&v.snapshot))})
    }
    pub fn observe_source(
        &mut self,
        load_id: &str,
        snapshot: Value,
        expected_check: u64,
        requested_at: DateTime<Utc>,
        actor: &str,
    ) -> Result<SourceGrant> {
        let load_index = self
            .loads
            .iter()
            .position(|v| v.id == load_id)
            .ok_or("Load not found")?;
        let load = self.loads[load_index].clone();
        let order_id = load
            .sync_order_id
            .clone()
            .ok_or("Shipment is not imported from Sync")?;
        if snapshot["id"].as_str() != Some(&order_id)
            || !snapshot.is_object()
            || snapshot["status"]
                .as_str()
                .is_none_or(|v| v.trim().is_empty() || v.len() > 100)
        {
            return Err("Invalid canonical Sync order snapshot".into());
        }
        let previous = self.source_check(load_id).cloned();
        if previous.as_ref().map_or(0, |v| v.version) != expected_check {
            return Err("Sync source check changed; retry refresh".into());
        }
        if requested_at > Utc::now() + Duration::seconds(5)
            || requested_at < Utc::now() - Duration::minutes(5)
        {
            return Err("Invalid Sync observation request time".into());
        }
        if snapshot.get("updatedAt").is_some_and(|v| {
            v.as_str()
                .is_none_or(|s| DateTime::parse_from_rfc3339(s).is_err())
        }) || previous
            .as_ref()
            .is_some_and(|v| v.snapshot.get("updatedAt").is_some())
            && snapshot.get("updatedAt").is_none()
        {
            return Err("Sync source revision timestamp is missing or invalid".into());
        }
        if let Some(old) = previous.as_ref().map(|v| &v.snapshot) {
            if let (Some(old), Some(new)) = (
                old["updatedAt"]
                    .as_str()
                    .and_then(|v| DateTime::parse_from_rfc3339(v).ok()),
                snapshot["updatedAt"]
                    .as_str()
                    .and_then(|v| DateTime::parse_from_rfc3339(v).ok()),
            ) {
                if new < old {
                    return Err(
                        "Sync returned an older source revision; refresh before continuing".into(),
                    );
                }
            }
        }
        let authorized = previous
            .as_ref()
            .and_then(|v| v.authorized_review_id.as_ref())
            .and_then(|id| self.source_reviews.iter().find(|v| &v.id == id))
            .cloned();
        let baseline = authorized
            .as_ref()
            .map(|v| v.snapshot.clone())
            .or_else(|| load.source_order.clone())
            .ok_or("Original order snapshot not found")?;
        let own_tracking = self.own_tracking_attempted(load_id);
        let current = projection(&snapshot, own_tracking);
        let base = projection(&baseline, own_tracking);
        let digest = hash(&current);
        let plan = json!(crate::quotes::QuotePlan::from_load(&load));
        let plan_changed = authorized
            .as_ref()
            .and_then(|v| v.decision.as_ref())
            .is_some_and(|v| v.plan_snapshot != plan);
        let exceptional = needs_explicit_authorization(&snapshot)
            || (["shipped", "fulfilled", "delivered"].contains(&status(&snapshot).as_str())
                && !own_tracking);
        let mut check = previous.unwrap_or(SourceCheck {
            load_id: load_id.into(),
            order_id: order_id.clone(),
            version: 0,
            snapshot: baseline.clone(),
            digest: String::new(),
            checked_at: None,
            valid_until: None,
            checked_by: actor.into(),
            last_error: None,
            error_at: None,
            active_review_id: None,
            authorized_review_id: None,
        });
        let open = check
            .active_review_id
            .as_ref()
            .and_then(|id| self.source_reviews.iter().find(|v| &v.id == id))
            .cloned();
        let needs_review = current != base
            || plan_changed
            || (exceptional && authorized.is_none())
            || open.is_some();
        if needs_review
            && open
                .as_ref()
                .is_none_or(|v| v.digest != digest || v.plan_snapshot != plan)
        {
            let id = domain::id("source-review");
            let mut fields = std::collections::BTreeSet::new();
            if let (Some(a), Some(b)) = (base.as_object(), current.as_object()) {
                for key in a.keys().chain(b.keys()) {
                    if a.get(key) != b.get(key) {
                        fields.insert(key.clone());
                    }
                }
            }
            if plan_changed {
                fields.insert("freight_plan".into());
            }
            if exceptional && fields.is_empty() {
                fields.insert("status".into());
            }
            if fields.is_empty() {
                fields.insert("upstream_reverted".into());
            }
            if let Some(old) = open {
                let old = self
                    .source_reviews
                    .iter_mut()
                    .find(|v| v.id == old.id)
                    .ok_or("Source review not found")?;
                old.status = "superseded".into();
                old.superseded_by_id = Some(id.clone());
                old.version = bump(old.version)?;
            }
            self.source_reviews.push(SourceReview {
                id: id.clone(),
                load_id: load_id.into(),
                order_id: order_id.clone(),
                baseline,
                snapshot: snapshot.clone(),
                digest: digest.clone(),
                changed_fields: fields.into_iter().collect(),
                plan_snapshot: plan,
                observed_at: Utc::now(),
                observed_by: actor.into(),
                status: "pending".into(),
                version: 1,
                superseded_by_id: None,
                decision: None,
            });
            check.active_review_id = Some(id);
            self.loads[load_index].version = bump(self.loads[load_index].version)?;
            self.loads[load_index].updated_at = Utc::now();
            self.record(load_id,"source.changed","Canonical commerce order changed; new transport commitments require recorded review");
            if let Some(e) = self.events.last_mut() {
                e.actor = actor.into();
            }
        }
        check.version = bump(check.version)?;
        check.snapshot = snapshot;
        check.digest = digest;
        check.checked_at = Some(Utc::now());
        check.valid_until = Some(requested_at + Duration::seconds(30));
        check.checked_by = actor.into();
        check.last_error = None;
        check.error_at = None;
        let grant = SourceGrant {
            load_id: load_id.into(),
            version: check.version,
        };
        if let Some(index) = self.source_checks.iter().position(|v| v.load_id == load_id) {
            self.source_checks[index] = check;
        } else {
            self.source_checks.push(check);
        }
        Ok(grant)
    }
    pub fn fail_source(&mut self, load_id: &str, expected: u64, message: &str) -> Result<()> {
        let load = self
            .loads
            .iter()
            .find(|v| v.id == load_id)
            .ok_or("Load not found")?
            .clone();
        let order_id = load
            .sync_order_id
            .clone()
            .ok_or("Shipment is not imported from Sync")?;
        if self.source_check(load_id).map_or(0, |v| v.version) != expected {
            return Err("Sync source check changed; retry refresh".into());
        }
        if self.source_check(load_id).is_none() {
            self.source_checks.push(SourceCheck {
                load_id: load_id.into(),
                order_id,
                version: 0,
                snapshot: load.source_order.unwrap_or(Value::Null),
                digest: String::new(),
                checked_at: None,
                valid_until: None,
                checked_by: "system".into(),
                last_error: None,
                error_at: None,
                active_review_id: None,
                authorized_review_id: None,
            });
        }
        let check = self
            .source_checks
            .iter_mut()
            .find(|v| v.load_id == load_id)
            .ok_or("Source check not found")?;
        let first = check.last_error.is_none();
        check.version = bump(check.version)?;
        check.last_error = Some(message.into());
        check.error_at = Some(Utc::now());
        if first {
            self.record(load_id,"source.unavailable","Canonical Sync order verification is unavailable; new transport commitments are held");
        }
        Ok(())
    }
    pub fn resolve_source(&mut self, id: &str, body: Value, actor: &str) -> Result<Value> {
        let input: Resolve =
            serde_json::from_value(body).map_err(|_| "Invalid source review decision")?;
        let index = self
            .source_reviews
            .iter()
            .position(|v| v.id == id)
            .ok_or("Source review not found")?;
        let review = self.source_reviews[index].clone();
        let check = self
            .source_check(&review.load_id)
            .ok_or("Source check not found")?
            .clone();
        let load_index = self
            .loads
            .iter()
            .position(|v| v.id == review.load_id)
            .ok_or("Load not found")?;
        let load = self.loads[load_index].clone();
        if review.status != "pending"
            || review.version != input.expected_version
            || load.version != input.expected_load_version
            || check.active_review_id.as_deref() != Some(id)
            || check.digest != input.expected_digest
            || check.digest != review.digest
            || review.plan_snapshot != json!(crate::quotes::QuotePlan::from_load(&load))
        {
            return Err("Source review or shipment changed; refresh before deciding".into());
        }
        if check.last_error.is_some()
            || check
                .checked_at
                .is_none_or(|v| v < Utc::now() - Duration::minutes(5))
        {
            return Err(SOURCE_UNAVAILABLE.into());
        }
        for (v, label) in [
            (&input.contact, "Actual customer contact"),
            (&input.reference, "Customer confirmation reference"),
            (&input.note, "Source review findings"),
        ] {
            domain::required(v, label)?;
        }
        match input.decision.as_str() {
            "confirm_plan" => {
                if needs_explicit_authorization(&check.snapshot) {
                    return Err("Cancelled, returned, or unknown orders require explicit authorization to transport".into());
                }
            }
            "authorize_transport" => {
                if !needs_explicit_authorization(&check.snapshot) {
                    return Err("Use plan confirmation for an active source order".into());
                }
            }
            "cancel_transport" => {
                if load.status != Status::Cancelled {
                    return Err("Cancel the shipment through safe carrier release/withdrawal before closing this review".into());
                }
            }
            _ => {
                return Err(
                    "Source decision must confirm_plan, authorize_transport, or cancel_transport"
                        .into(),
                )
            }
        }
        let proof = SourceDecision {
            quote_id: None,
            decision: input.decision,
            contact: input.contact.trim().into(),
            reference: input.reference.trim().into(),
            note: input.note.trim().into(),
            recorded_at: Utc::now(),
            recorded_by: actor.into(),
            plan_snapshot: json!(crate::quotes::QuotePlan::from_load(&load)),
        };
        self.source_reviews[index].decision = Some(proof);
        self.source_reviews[index].status = "resolved".into();
        self.source_reviews[index].version = bump(review.version)?;
        let state = self
            .source_checks
            .iter_mut()
            .find(|v| v.load_id == review.load_id)
            .ok_or("Source check not found")?;
        state.active_review_id = None;
        state.authorized_review_id = Some(id.into());
        state.version = bump(state.version)?;
        self.loads[load_index].version = bump(load.version)?;
        self.loads[load_index].updated_at = Utc::now();
        self.record(
            &load.id,
            "source.resolved",
            "Customer source findings recorded; carrier custody and financial history retained",
        );
        Ok(json!(self.source_reviews[index]))
    }
}
