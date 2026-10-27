//! Versioned human work; source evidence and shipment truth remain distinct.
use crate::actions::{Kind, Owner, Pending, Target};
use crate::model::clean;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkKind {
    Claim,
    Release,
    Note,
    FollowUp,
    Defer,
    Resume,
}
impl WorkKind {
    pub const ALL: [Self; 6] = [
        Self::Claim,
        Self::Release,
        Self::Note,
        Self::FollowUp,
        Self::Defer,
        Self::Resume,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Claim => "Claim work for me",
            Self::Release => "Release my ownership",
            Self::Note => "Record operational findings",
            Self::FollowUp => "Change my follow-up time",
            Self::Defer => "Defer my follow-up",
            Self::Resume => "Resume my deferred work",
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Release => "release",
            Self::Note | Self::FollowUp => "note",
            Self::Defer => "snooze",
            Self::Resume => "resume",
        }
    }
    pub fn timed(self) -> bool {
        matches!(self, Self::Claim | Self::FollowUp | Self::Defer)
    }
    pub fn allowed(self, target: &Target, actor: &Owner) -> bool {
        let Some(work) = &target.work else {
            return false;
        };
        let owns = work.owner.as_deref() == Some(actor.subject.as_str());
        match self {
            Self::Note => true,
            Self::Claim => work.source_active && work.owner.is_none(),
            Self::Release | Self::FollowUp => work.source_active && owns,
            Self::Defer => {
                work.source_active && owns && work.priority > 0 && work.follow_up_at.is_some()
            }
            Self::Resume => work.source_active && owns && work.deferred_until.is_some(),
        }
    }
    pub fn time(self, raw: &str, target: &Target, now: DateTime<Utc>) -> Result<String, String> {
        let raw = raw.trim();
        let time = if let Some((unit, amount)) = raw.chars().last().and_then(|unit| {
            if !matches!(unit, 'm' | 'h' | 'd') {
                return None;
            }
            Some((unit, raw[..raw.len() - 1].parse::<i64>().ok()?))
        }) {
            let minutes = amount
                .checked_mul(match unit {
                    'h' => 60,
                    'd' => 1440,
                    _ => 1,
                })
                .filter(|v| *v > 0 && *v <= 10080)
                .ok_or("Use a future time within seven days.")?;
            now.checked_add_signed(Duration::minutes(minutes))
                .ok_or("Time is out of range.")?
        } else {
            DateTime::parse_from_rfc3339(raw)
                .map_err(|_| "Use 30m, 2h, 1d, or an RFC3339 time with Z/offset.")?
                .with_timezone(&Utc)
        };
        let cap = if self == Self::Defer {
            Duration::hours(24)
        } else {
            Duration::days(7)
        };
        if time <= now || time > now + cap {
            return Err(if self == Self::Defer {
                "Deferral must be in the future and within 24 hours."
            } else {
                "Follow-up must be in the future and within seven days."
            }
            .into());
        }
        if self == Self::Defer
            && target
                .work
                .as_ref()
                .and_then(|w| w.follow_up_at.as_deref())
                .and_then(parse_time)
                .is_none_or(|due| time > due)
        {
            return Err(
                "Defer no later than the current follow-up; change the follow-up first if needed."
                    .into(),
            );
        }
        Ok(time.to_rfc3339_opts(SecondsFormat::Secs, true))
    }
}
fn parse_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
fn safe(raw: &str, max: usize) -> bool {
    !raw.trim().is_empty() && raw.len() <= max && clean(raw) == raw
}
fn nullable_string(value: &Value) -> Option<Option<String>> {
    if value.is_null() {
        Some(None)
    } else {
        value
            .as_str()
            .filter(|s| safe(s, 200))
            .map(|s| Some(s.into()))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkTarget {
    pub source_sha256: String,
    pub source_active: bool,
    pub priority: u64,
    pub owner: Option<String>,
    pub follow_up_at: Option<String>,
    pub deferred_until: Option<String>,
}
impl WorkTarget {
    pub fn valid(&self) -> bool {
        self.source_sha256.len() == 64
            && self
                .source_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && self.priority <= 255
            && self.owner.as_ref().is_none_or(|s| safe(s, 200))
            && [&self.follow_up_at, &self.deferred_until]
                .into_iter()
                .all(|s| {
                    s.as_deref()
                        .is_none_or(|s| s.len() <= 64 && parse_time(s).is_some())
                })
    }
}
impl Target {
    pub fn from_issue(value: &Value) -> Option<Self> {
        if value["evidence_valid"] != true || value["source_changed"] != false {
            return None;
        }
        let case = value.get("case")?;
        let id = value["id"].as_str()?;
        let owner = nullable_string(value.get("owner")?)?;
        let version = if case.is_null() {
            0
        } else {
            case["version"].as_u64().filter(|v| *v > 0)?
        };
        let source_sha256 = value["source_sha256"].as_str()?.to_owned();
        let status = if case.is_null() {
            "open"
        } else {
            case["status"].as_str()?
        };
        if !case.is_null()
            && (case["id"] != id
                || case["source_sha256"] != source_sha256
                || nullable_string(case.get("owner")?)? != owner)
        {
            return None;
        }
        if case.is_null() && (owner.is_some() || value["source_active"] != true) {
            return None;
        }
        let target = Self {
            id: id.into(),
            reference: clean(value["load_reference"].as_str().unwrap_or(id)),
            version,
            status: status.into(),
            exception: None,
            work: Some(WorkTarget {
                source_sha256,
                source_active: value["source_active"].as_bool()?,
                priority: value["signal"]["priority"].as_u64()?,
                owner,
                follow_up_at: nullable_string(&case["follow_up_at"])?,
                deferred_until: nullable_string(&case["deferred_until"])?,
            }),
        };
        target.valid_work().then_some(target)
    }
    pub fn valid_work(&self) -> bool {
        self.work.as_ref().is_some_and(|work| {
            work.valid()
                && (work.source_active || self.status == "resolved")
                && (!work.source_active || self.status != "resolved")
                && (self.status != "open" || work.owner.is_none())
                && (self.status != "working" || work.owner.is_some())
                && (self.version != 0
                    || (self.status == "open"
                        && work.owner.is_none()
                        && work.follow_up_at.is_none()
                        && work.deferred_until.is_none()))
        }) && !self.id.is_empty()
            && self.id.len() <= 200
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            && safe(&self.reference, 4096)
            && self.version.checked_add(2).is_some()
            && matches!(self.status.as_str(), "open" | "working" | "resolved")
            && self.exception.is_none()
    }
}
impl Pending {
    pub fn new_work(
        target: Target,
        owner: Owner,
        kind: WorkKind,
        note: &str,
        time: &str,
    ) -> Result<Self, String> {
        if !target.valid_work() || !kind.allowed(&target, &owner) {
            return Err(
                "This work action is unavailable for the reviewed owner/source state.".into(),
            );
        }
        let note = note.trim();
        if !safe(note, 200) {
            return Err("Enter 1–200 UTF-8 bytes of actual findings or coordination.".into());
        }
        let time = if kind.timed() {
            Some(
                parse_time(time)
                    .ok_or("Review a valid absolute follow-up/defer time.")?
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            )
        } else {
            None
        };
        let work = target.work.as_ref().ok_or("Work identity is missing.")?;
        let body = json!({"expected_version":target.version,"expected_source_active":work.source_active,"source_sha256":work.source_sha256,"action":kind.code(),"note":note,
            "follow_up_at":if matches!(kind,WorkKind::Claim|WorkKind::FollowUp){time.clone()}else{None},"defer_until":if kind==WorkKind::Defer{time}else{None}});
        Ok(Self {
            target,
            owner,
            kind: Kind::Work(kind),
            body: serde_json::to_vec(&body).map_err(|_| "Cannot prepare work command")?,
            key: format!("tui-{}", uuid::Uuid::new_v4()),
        })
    }
    pub fn validates_work_result(&self, value: &Value, kind: WorkKind) -> bool {
        let Some(work) = &self.target.work else {
            return false;
        };
        let Ok(body) = serde_json::from_slice::<Value>(&self.body) else {
            return false;
        };
        let version = if self.target.version == 0 {
            Some(2)
        } else {
            self.target.version.checked_add(1)
        };
        let history = value["history"].as_array();
        let previous = history.and_then(|h| h.len().checked_sub(2).and_then(|i| h.get(i)));
        // The API can record an expired deferral before this exact action. Accept
        // only that identified system event, never an unexplained version jump.
        let resumed = self.target.version > 0
            && work.source_active
            && value["version"].as_u64() == self.target.version.checked_add(2)
            && previous.is_some_and(|event| {
                event["action"] == "resumed"
                    && event["actor"] == "system:operations"
                    && event["at"]
                        .as_str()
                        .and_then(parse_time)
                        .zip(work.deferred_until.as_deref().and_then(parse_time))
                        .is_some_and(|(at, due)| at >= due)
            });
        if self.target.version == 0
            && previous
                .is_none_or(|e| e["action"] != "detected" || e["actor"] != "system:operations")
        {
            return false;
        }
        if value["id"] != self.target.id
            || (value["version"].as_u64() != version && !resumed)
            || value["source_sha256"] != work.source_sha256
        {
            return false;
        }
        let history = value["history"].as_array().and_then(|a| a.last());
        if history.is_none_or(|h| {
            h["action"] != kind.code()
                || h["actor"] != self.owner.subject
                || h["note"] != body["note"]
        }) {
            return false;
        }
        let owner = if kind == WorkKind::Claim {
            Some(self.owner.subject.as_str())
        } else if kind == WorkKind::Release {
            None
        } else {
            work.owner.as_deref()
        };
        if value["owner"].as_str() != owner {
            return false;
        }
        match kind {
            WorkKind::Claim | WorkKind::FollowUp => {
                value["follow_up_at"].as_str().and_then(parse_time)
                    == body["follow_up_at"].as_str().and_then(parse_time)
                    && value["status"] == "working"
            }
            WorkKind::Release => {
                value["status"] == "open"
                    && value["follow_up_at"].is_null()
                    && value["deferred_until"].is_null()
            }
            WorkKind::Defer => {
                value["deferred_until"].as_str().and_then(parse_time)
                    == body["defer_until"].as_str().and_then(parse_time)
            }
            WorkKind::Resume => value["deferred_until"].is_null(),
            WorkKind::Note => true,
        }
    }
}
