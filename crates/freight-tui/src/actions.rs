//! Reviewed, owner-bound commands. No automatic write retries.
use crate::{
    client,
    model::{clean, App, View},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub reference: String,
    pub version: u64,
    pub status: String,
    pub exception: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::work_actions::WorkTarget>,
}
impl Target {
    pub fn from_load(value: &Value) -> Option<Self> {
        let id = value["id"].as_str()?;
        let version = value["version"].as_u64()?;
        if id.is_empty()
            || id.len() > 200
            || !id
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"_-".contains(&v))
            || version.checked_add(1).is_none()
        {
            return None;
        }
        Some(Self {
            id: id.into(),
            reference: clean(value["reference"].as_str()?),
            version,
            status: value["status"].as_str()?.into(),
            exception: value["exception"].as_str().map(clean),
            work: None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Location,
    Exception,
    Resolve,
    Work(crate::work_actions::WorkKind),
}
impl Kind {
    pub const ALL: [Self; 3] = [Self::Location, Self::Exception, Self::Resolve];
    pub fn title(self) -> &'static str {
        match self {
            Self::Location => "Update reported location",
            Self::Exception => "Report shipment exception",
            Self::Resolve => "Resolve shipment exception",
            Self::Work(kind) => kind.title(),
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::Exception => "exception",
            Self::Resolve => "resolve",
            Self::Work(kind) => kind.code(),
        }
    }
    pub fn choices(target: &Target) -> Vec<Self> {
        if target.work.is_some() {
            crate::work_actions::WorkKind::ALL
                .into_iter()
                .map(Self::Work)
                .collect()
        } else {
            Self::ALL.to_vec()
        }
    }
    pub fn allowed_for(self, target: &Target, owner: &Owner) -> bool {
        match self {
            Self::Work(kind) => kind.allowed(target, owner),
            _ => self.allowed(target),
        }
    }
    pub fn allowed(self, target: &Target) -> bool {
        if target.work.is_some() {
            return matches!(self, Self::Work(_));
        }
        match self {
            Self::Location => target.status == "in_transit",
            Self::Exception => !matches!(target.status.as_str(), "delivered" | "cancelled"),
            Self::Resolve => target.exception.is_some(),
            Self::Work(_) => false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub tenant: String,
    pub subject: String,
}
impl Owner {
    pub fn from_identity(value: &Value) -> Option<Self> {
        let owner = Self {
            tenant: value["tenant_id"].as_str()?.into(),
            subject: value["access"]["subject"].as_str()?.into(),
        };
        if owner.tenant.trim().is_empty()
            || owner.subject.trim().is_empty()
            || owner.tenant.len() > 200
            || owner.subject.len() > 200
        {
            return None;
        }
        Some(owner)
    }
}
pub fn capability(identity: &Value, name: &str) -> bool {
    identity["access"]["capabilities"]
        .as_array()
        .is_some_and(|v| v.iter().any(|v| v == name))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub target: Target,
    pub owner: Owner,
    pub kind: Kind,
    pub body: Vec<u8>,
    pub key: String,
}
impl Pending {
    pub fn new(target: Target, owner: Owner, kind: Kind, input: &str) -> Result<Self, String> {
        if matches!(kind, Kind::Work(_)) || !kind.allowed(&target) {
            return Err("This action does not apply to the reviewed shipment.".into());
        }
        let input = input.trim();
        if kind != Kind::Resolve && (input.is_empty() || input.len() > 200 || clean(input) != input)
        {
            return Err(
                "Enter 1–200 UTF-8 bytes of actual location or exception information.".into(),
            );
        }
        let mut body = json!({"action":kind.code(),"expected_version":target.version});
        if kind != Kind::Resolve {
            body[if kind == Kind::Location {
                "location"
            } else {
                "note"
            }] = json!(input);
        }
        Ok(Self {
            target,
            owner,
            kind,
            body: serde_json::to_vec(&body).map_err(|_| "Cannot prepare command")?,
            key: format!("tui-{}", uuid::Uuid::new_v4()),
        })
    }
    pub fn path(&self) -> String {
        if self.target.work.is_some() {
            format!("operations/issues/{}/actions", self.target.id)
        } else {
            format!("loads/{}/commands", self.target.id)
        }
    }
    pub fn validates_result(&self, value: &Value) -> bool {
        if let Kind::Work(kind) = self.kind {
            return self.validates_work_result(value, kind);
        }
        if value["id"] != self.target.id
            || value["version"].as_u64() != self.target.version.checked_add(1)
        {
            return false;
        }
        let Ok(body) = serde_json::from_slice::<Value>(&self.body) else {
            return false;
        };
        match self.kind {
            Kind::Location => value["location"] == body["location"],
            Kind::Exception => value["exception"] == body["note"],
            Kind::Resolve => value.get("exception").is_some_and(Value::is_null),
            Kind::Work(_) => false,
        }
    }
}
#[derive(Clone)]
pub enum WriteJob {
    Submit { pending: Pending, checking: bool },
    Stop { pending: Pending, note: String },
}
impl WriteJob {
    pub fn pending(&self) -> &Pending {
        match self {
            Self::Submit { pending, .. } | Self::Stop { pending, .. } => pending,
        }
    }
    pub fn recovery(&self) -> bool {
        !matches!(
            self,
            Self::Submit {
                checking: false,
                ..
            }
        )
    }
}
pub enum Outcome {
    Completed,
    Stopped,
    Rejected(String),
    Uncertain(String),
    Denied,
}
#[derive(PartialEq, Eq)]
pub enum Stage {
    Choose,
    Edit,
    Review,
    StopEdit,
    StopReview,
    Time,
}
pub struct Dialog {
    pub target: Target,
    pub owner: Owner,
    pub kind: Kind,
    pub stage: Stage,
    pub input: String,
    pub when: String,
    pub error: Option<String>,
}

impl App {
    pub fn can_open_action(&self) -> bool {
        matches!(self.view, View::Loads | View::Operations)
            && (self.view != View::Operations
                || self.identity["features"]["operations_save_recovery"] == true)
            && capability(&self.identity, "operations.write")
            && Owner::from_identity(&self.identity).is_some()
            && self.current().is_some_and(|r| r.target.is_some())
            && self.recovery_error.is_none()
            && !self.stale()
            && self.pending.is_none()
            && !self.write_busy
            && !self.write_lost
            && self.loading.is_none()
            && !self.needs_fetch
            && self.error.is_none()
            && !self.denied
    }
    pub fn open_action(&mut self) {
        if let Some(error) = &self.recovery_error {
            self.write_notice = Some(error.clone());
            return;
        }
        if self.stale() {
            self.write_notice =
                Some("Snapshot is stale. Press r to refresh before reviewing an action.".into());
            return;
        }
        if self.pending.is_some()
            || self.write_busy
            || self.write_lost
            || self.loading.is_some()
            || self.needs_fetch
            || self.error.is_some()
        {
            self.write_notice = Some(
                "Refresh current data and finish any original save before another action.".into(),
            );
            return;
        }
        if !matches!(self.view, View::Loads | View::Operations)
            || !capability(&self.identity, "operations.write")
        {
            self.write_notice = Some(
                "Actions require an operations or administrator credential on Loads or Operations."
                    .into(),
            );
            return;
        }
        if self.view == View::Operations
            && self.identity["features"]["operations_save_recovery"] != true
        {
            self.write_notice=Some("This API does not advertise recoverable work actions. Deploy a compatible API before saving work.".into());
            return;
        }
        let Some(target) = self.current().and_then(|r| r.target.clone()) else {
            self.write_notice = Some(
                "This record has no supported command identity/version or trusted source evidence."
                    .into(),
            );
            return;
        };
        let Some(owner) = Owner::from_identity(&self.identity) else {
            self.write_notice =
                Some("Refresh a verified staff identity before opening an action.".into());
            return;
        };
        let kind = if target.work.is_some() {
            Kind::Work(crate::work_actions::WorkKind::Claim)
        } else {
            Kind::Location
        };
        self.action_dialog = Some(Dialog {
            target,
            owner,
            kind,
            stage: Stage::Choose,
            input: String::new(),
            when: String::new(),
            error: None,
        });
        self.quit_confirm = false;
    }
    pub fn action_paste(&mut self, value: &str) {
        if let Some(dialog) = &mut self.action_dialog {
            if matches!(dialog.stage, Stage::Edit | Stage::StopEdit | Stage::Time) {
                let input = if dialog.stage == Stage::Time {
                    &mut dialog.when
                } else {
                    &mut dialog.input
                };
                for c in clean(value).chars() {
                    if input.len() + c.len_utf8() > 200 {
                        break;
                    }
                    input.push(c);
                }
            }
        }
    }
    /// Returns true when an action/recovery dialog consumed the key.
    pub fn action_key(&mut self, key: KeyEvent) -> bool {
        if self.quit_confirm {
            match key.code {
                KeyCode::Char('Q') if key.kind == KeyEventKind::Press => self.quit = true,
                KeyCode::Esc => self.quit_confirm = false,
                _ => {}
            }
            return true;
        }
        if self.action_dialog.is_some() {
            if key.kind != KeyEventKind::Press {
                return true;
            }
            let mut dialog = self.action_dialog.take().expect("dialog checked");
            if key.code == KeyCode::Esc {
                return true;
            }
            match dialog.stage {
                Stage::Choose => {
                    if let KeyCode::Char(c @ '1'..='6') = key.code {
                        let choices = Kind::choices(&dialog.target);
                        let Some(&kind) = choices.get((c as u8 - b'1') as usize) else {
                            self.action_dialog = Some(dialog);
                            return true;
                        };
                        if kind.allowed_for(&dialog.target, &dialog.owner) {
                            dialog.kind = kind;
                            dialog.stage = if matches!(kind,Kind::Work(work) if work.timed()) {
                                Stage::Time
                            } else if kind == Kind::Resolve {
                                Stage::Review
                            } else {
                                Stage::Edit
                            };
                            dialog.error = None;
                        } else {
                            dialog.error =
                                Some("Unavailable for this shipment's reviewed state.".into());
                        }
                    }
                }
                Stage::Time => match key.code {
                    KeyCode::Backspace => {
                        dialog.when.pop();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                            && clean(&c.to_string()) == c.to_string()
                            && dialog.when.len() + c.len_utf8() <= 200 =>
                    {
                        dialog.when.push(c)
                    }
                    KeyCode::Enter => {
                        if let Kind::Work(kind) = dialog.kind {
                            match kind.time(&dialog.when, &dialog.target, chrono::Utc::now()) {
                                Ok(time) => {
                                    dialog.when = time;
                                    dialog.stage = Stage::Edit;
                                    dialog.error = None;
                                }
                                Err(message) => dialog.error = Some(message),
                            }
                        }
                    }
                    _ => {}
                },
                Stage::Edit | Stage::StopEdit => match key.code {
                    KeyCode::Backspace => {
                        dialog.input.pop();
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                            && !c.is_control()
                            && clean(&c.to_string()) == c.to_string()
                            && dialog.input.len() + c.len_utf8() <= 200 =>
                    {
                        dialog.input.push(c)
                    }
                    KeyCode::Enter => {
                        if dialog.input.trim().is_empty() {
                            dialog.error = Some(
                                "Enter an actual location, exception, or resolution note.".into(),
                            );
                        } else {
                            dialog.input = dialog.input.trim().into();
                            dialog.stage = if dialog.stage == Stage::StopEdit {
                                Stage::StopReview
                            } else {
                                Stage::Review
                            };
                            dialog.error = None;
                        }
                    }
                    _ => {}
                },
                Stage::Review | Stage::StopReview => {
                    if key.code == KeyCode::Char('y') && key.modifiers.is_empty() {
                        if !self.action_viewport_ready {
                            dialog.error = Some(
                                "Resize to at least 72 columns × 22 rows before confirming.".into(),
                            );
                            self.action_dialog = Some(dialog);
                            return true;
                        }
                        if dialog.stage == Stage::StopReview {
                            if let Some(pending) = self.pending.clone() {
                                self.stopping_note = Some(dialog.input.trim().into());
                                self.write_ready = Some(WriteJob::Stop {
                                    pending,
                                    note: dialog.input.trim().into(),
                                });
                            }
                        } else {
                            let pending = if let Kind::Work(kind) = dialog.kind {
                                if kind.timed() {
                                    if let Err(message) =
                                        kind.time(&dialog.when, &dialog.target, chrono::Utc::now())
                                    {
                                        dialog.error = Some(message);
                                        self.action_dialog = Some(dialog);
                                        return true;
                                    }
                                }
                                Pending::new_work(
                                    dialog.target.clone(),
                                    dialog.owner.clone(),
                                    kind,
                                    &dialog.input,
                                    &dialog.when,
                                )
                            } else {
                                Pending::new(
                                    dialog.target.clone(),
                                    dialog.owner.clone(),
                                    dialog.kind,
                                    &dialog.input,
                                )
                            };
                            match pending {
                                Ok(pending) => {
                                    self.pending = Some(pending.clone());
                                    self.write_uncertain = false;
                                    self.write_ready = Some(WriteJob::Submit {
                                        pending,
                                        checking: false,
                                    });
                                }
                                Err(message) => {
                                    dialog.error = Some(message);
                                    self.action_dialog = Some(dialog);
                                }
                            }
                        }
                        return true;
                    }
                }
            }
            self.action_dialog = Some(dialog);
            return true;
        }
        if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Char('a' | 'c' | 'x')) {
            return true;
        }
        match key.code {
            KeyCode::Char('a')
                if !self.searching && !self.details && !self.help && !self.connection =>
            {
                self.open_action();
                true
            }
            KeyCode::Char('c' | 'x')
                if self.pending.is_some()
                    && self.write_uncertain
                    && !self.write_busy
                    && self.write_ready.is_none()
                    && !self.searching
                    && !self.details
                    && !self.help
                    && !self.connection =>
            {
                if let Some(error) = &self.recovery_error {
                    self.write_notice = Some(error.clone());
                    return true;
                }
                let pending = self.pending.clone().expect("pending checked");
                if let Some(note) = self.stopping_note.clone() {
                    if key.code == KeyCode::Char('c') {
                        self.write_ready = Some(WriteJob::Stop { pending, note });
                    } else {
                        self.action_dialog = Some(Dialog {
                            target: pending.target,
                            owner: pending.owner,
                            kind: pending.kind,
                            stage: Stage::StopReview,
                            input: note,
                            when: String::new(),
                            error: None,
                        });
                    }
                    return true;
                }
                if key.code == KeyCode::Char('c') {
                    self.write_ready = Some(WriteJob::Submit {
                        pending,
                        checking: true,
                    });
                } else {
                    self.action_dialog = Some(Dialog {
                        target: pending.target,
                        owner: pending.owner,
                        kind: pending.kind,
                        stage: Stage::StopEdit,
                        input: String::new(),
                        when: String::new(),
                        error: None,
                    });
                }
                true
            }
            _ => false,
        }
    }
    pub fn request_quit(&mut self) {
        if self.pending.is_some() || self.write_busy || self.write_lost {
            self.quit_confirm = true;
        } else {
            self.quit = true;
        }
    }
    pub fn finish_write(&mut self, recovery: bool, outcome: Outcome) {
        self.write_busy = false;
        match outcome {
            Outcome::Completed | Outcome::Stopped => {
                self.write_notice = Some(
                    if matches!(outcome, Outcome::Completed) {
                        "Save confirmed. r refreshes records only."
                    } else {
                        "Original save stopped. r refreshes records only."
                    }
                    .into(),
                );
                self.pending = None;
                self.stopping_note = None;
                self.write_uncertain = false;
                self.action_dialog = None;
                self.reset_page();
                self.invalidate();
            }
            Outcome::Rejected(message) if !recovery => {
                self.pending = None;
                self.write_uncertain = false;
                self.write_notice = Some(format!("Not saved. {message}"));
                self.error = Some("Refresh and review the record before another action.".into());
            }
            Outcome::Denied => {
                self.clear_access();
                self.needs_fetch = false;
                self.write_notice=Some("Access closed. Any earlier save may have completed; review it after signing in.".into());
                self.error = Some("Access denied. Records cleared.".into());
            }
            Outcome::Rejected(message) | Outcome::Uncertain(message) => {
                self.write_uncertain = true;
                self.write_notice = Some(format!(
                    "Outcome unconfirmed. {message}  {}",
                    if self.stopping_note.is_some() {
                        "c retry resolution · x review stop"
                    } else {
                        "c check original · x stop original"
                    }
                ));
            }
        }
    }
}

pub async fn execute(api: &reqwest::Client, origin: &reqwest::Url, job: &WriteJob) -> Outcome {
    let pending = job.pending();
    let identity = match client::access(api, origin).await {
        Ok(v) => v,
        Err(e) => {
            return if e.denied {
                Outcome::Denied
            } else {
                Outcome::Rejected("Access could not be checked; no command was sent.".into())
            }
        }
    };
    if Owner::from_identity(&identity).as_ref() != Some(&pending.owner)
        || !capability(&identity, "operations.write")
        || matches!(job, WriteJob::Stop { .. }) && !capability(&identity, "saves.recover")
    {
        return Outcome::Denied;
    }
    if pending.target.work.is_some() && identity["features"]["operations_save_recovery"] != true {
        return Outcome::Rejected(
            "The API does not advertise Operations recovery support; no command was sent.".into(),
        );
    }
    let mut url = origin.clone();
    let builder = match job {
        WriteJob::Submit { .. } => {
            url.set_path(&format!("/v1/{}", pending.path()));
            api.post(url)
                .header("idempotency-key", &pending.key)
                .header("content-type", "application/json")
                .body(pending.body.clone())
        }
        WriteJob::Stop { note, .. } => {
            url.set_path("/v1/save-recovery/resolve");
            let Ok(body) = serde_json::from_slice::<Value>(&pending.body) else {
                return Outcome::Uncertain("Original body could not be decoded.".into());
            };
            api.post(url).json(
                &json!({"request_key":pending.key,"path":pending.path(),"body":body,"note":note}),
            )
        }
    };
    let response = match builder.send().await {
        Ok(v) => v,
        Err(_) => {
            return Outcome::Uncertain(
                "Connection interrupted; the original request is retained.".into(),
            )
        }
    };
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Outcome::Denied;
    }
    if status.is_server_error() || status.is_redirection() {
        return Outcome::Uncertain("Server response does not establish the save outcome.".into());
    }
    // Operations acknowledgements include retained case history. Keep the same
    // bounded response budget used for reads instead of truncating a valid audit trail.
    let limit = if pending.target.work.is_some() {
        32 * 1024 * 1024
    } else {
        64 * 1024
    };
    let value = match client::read_json(response, limit).await {
        Ok(v) => v,
        Err(_) => {
            return Outcome::Uncertain(
                "Save response unreadable; the original request is retained.".into(),
            )
        }
    };
    if status.as_u16() == 409 && value["error"]["code"] == "request_stopped" {
        return Outcome::Stopped;
    }
    if !status.is_success() {
        return if status.is_client_error() && value["error"]["message"].is_string() {
            Outcome::Rejected(
                "The API refused this request. Refresh and review current state.".into(),
            )
        } else {
            Outcome::Uncertain("Unrecognized save response.".into())
        };
    }
    match job {
        WriteJob::Submit { .. }
            if pending.validates_result(&value["data"]) && value["replayed"].is_boolean() =>
        {
            Outcome::Completed
        }
        WriteJob::Stop { .. }
            if value["data"]["request_key"] == pending.key && value["replayed"].is_boolean() =>
        {
            match value["data"]["outcome"].as_str() {
                Some("stopped") => Outcome::Stopped,
                Some("completed") if pending.validates_result(&value["data"]["result"]) => {
                    Outcome::Completed
                }
                _ => Outcome::Uncertain("Resolution was not confirmed.".into()),
            }
        }
        _ => Outcome::Uncertain("Save acknowledgement does not match the original command.".into()),
    }
}
