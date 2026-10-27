//! Encrypted write-ahead recovery. Restoration never performs a network write.
use crate::actions::{Kind, Outcome, Owner, Pending, Target, WriteJob};
use ring::{
    aead, digest,
    rand::{SecureRandom, SystemRandom},
};
use rustix::fs::{self, AtFlags, Mode, OFlags};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    env,
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

const LIMIT: u64 = 64 * 1024;
const STORAGE_ERROR: &str = "Recovery storage unavailable; no further command was sent. Keep the recovery directory and key, fix storage, then refresh.";
const MODE: Mode = Mode::from_raw_mode(0o600);

pub struct Settings {
    directory: PathBuf,
    key: [u8; 32],
}
impl Settings {
    pub fn environment(override_key: Option<String>) -> Result<Self, String> {
        let raw = override_key.map(Ok).unwrap_or_else(|| env::var("FREIGHT_TUI_RECOVERY_KEY"))
            .map_err(|_| "Live saves require FREIGHT_TUI_RECOVERY_KEY (64 hexadecimal characters). Browsing is available.")?;
        if raw.len() != 64 {
            return Err("FREIGHT_TUI_RECOVERY_KEY must be 64 hexadecimal characters. Browsing is available.".into());
        }
        let key = hex::decode(raw)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or(
            "FREIGHT_TUI_RECOVERY_KEY must be 64 hexadecimal characters. Browsing is available.",
        )?;
        let directory = if let Some(path) = env::var_os("FREIGHT_TUI_RECOVERY_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("stateset-freight")
        } else {
            PathBuf::from(
                env::var_os("HOME")
                    .ok_or("Set FREIGHT_TUI_RECOVERY_DIR to an absolute private directory.")?,
            )
            .join(".local/state/stateset-freight")
        };
        if !directory.is_absolute() {
            return Err("Recovery directory must be absolute.".into());
        }
        Ok(Self { directory, key })
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Uncertain,
    Completed,
    Stopped,
    Rejected,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    pending: Pending,
    stop_note: Option<String>,
    phase: Phase,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u8,
    nonce: String,
    ciphertext: String,
}

/// UI state is delivered only alongside a freshly verified read for this owner.
pub enum Status {
    Ready,
    Pending {
        pending: Box<Pending>,
        stop_note: Option<String>,
    },
    Blocked(String),
}
impl Status {
    pub fn apply(self, app: &mut crate::model::App) {
        match self {
            Self::Blocked(error) => {
                app.recovery_error = Some(error.clone());
                if app.write_notice.is_none() {
                    app.write_notice = Some(error);
                }
            }
            Self::Ready => {
                if app.recovery_error.is_some() && app.write_notice == app.recovery_error {
                    app.write_notice = None;
                }
                app.recovery_error = None;
                app.recovery_durable = true;
                app.write_lost = false;
                if app
                    .pending
                    .as_ref()
                    .is_some_and(|p| Owner::from_identity(&app.identity).as_ref() != Some(&p.owner))
                {
                    app.pending = None;
                    app.write_ready = None;
                    app.stopping_note = None;
                    app.action_dialog = None;
                    app.write_uncertain = false;
                }
            }
            Self::Pending { pending, stop_note } => {
                if Owner::from_identity(&app.identity).as_ref() != Some(&pending.owner) {
                    app.clear_access();
                    app.recovery_error = Some(
                        "Refresh the original verified identity before restoring recovery.".into(),
                    );
                    return;
                }
                let restored = app.recovery_error.is_some()
                    || app.pending.as_ref().is_none_or(|p| p.key != pending.key);
                app.recovery_error = None;
                app.recovery_durable = true;
                app.write_lost = false;
                app.pending = Some(*pending);
                app.stopping_note = stop_note;
                app.write_uncertain = true;
                if restored {
                    app.action_dialog = None;
                    app.write_ready = None;
                    app.write_notice = Some(if app.stopping_note.is_some() {
                        "Recovered original stop decision. c retries resolution; x reviews it. Nothing was sent automatically."
                    } else {
                        "Recovered unresolved save. c checks the exact original; x reviews a stop. Nothing was sent automatically."
                    }.into());
                }
            }
        }
    }
}

pub struct Manager {
    settings: Result<Settings, String>,
    origin: String,
    active: Option<Journal>,
    blocked: Option<String>,
}
impl Manager {
    pub fn new(settings: Result<Settings, String>, origin: &reqwest::Url) -> Self {
        Self {
            settings,
            origin: origin.as_str().into(),
            active: None,
            blocked: None,
        }
    }
    pub fn close(&mut self) {
        self.active = None;
        self.blocked = None;
    }
    pub async fn execute(
        &mut self,
        api: &reqwest::Client,
        origin: &reqwest::Url,
        job: &WriteJob,
    ) -> (Outcome, Status) {
        if let Err(error) = self.prepare(job) {
            return (Outcome::Uncertain(error), self.status());
        }
        let outcome = crate::actions::execute(api, origin, job).await;
        self.finish(job.recovery(), &outcome);
        (outcome, self.status())
    }
    pub fn read(&mut self, identity: &serde_json::Value) -> Status {
        let Some(owner) = Owner::from_identity(identity) else {
            self.close();
            return Status::Blocked(
                "Live saves require a verified tenant and staff subject.".into(),
            );
        };
        if self.active.as_ref().is_some_and(|j| j.owner != owner) {
            self.close();
        }
        // A refresh is also the explicit retry for previously unavailable storage.
        self.blocked = None;
        if self.active.is_none() {
            let result = self
                .settings
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|s| Journal::open(s, &self.origin, owner));
            match result {
                Ok(journal) => self.active = Some(journal),
                Err(error) => {
                    self.blocked = Some(error.clone());
                    return Status::Blocked(error);
                }
            }
        }
        let journal = self.active.as_mut().expect("opened journal");
        if journal
            .record
            .as_ref()
            .is_some_and(|r| r.phase != Phase::Uncertain)
        {
            // Only a successful, same-owner API read permits retirement of a final receipt.
            if journal.clear().is_err() {
                self.blocked = Some(STORAGE_ERROR.into());
                return Status::Blocked(STORAGE_ERROR.into());
            }
        }
        self.status()
    }
    pub fn status(&self) -> Status {
        if let Some(error) = &self.blocked {
            return Status::Blocked(error.clone());
        }
        match &self.active {
            Some(journal) => match &journal.record {
                Some(record) if record.phase == Phase::Uncertain => Status::Pending {
                    pending: Box::new(record.pending.clone()),
                    stop_note: record.stop_note.clone(),
                },
                Some(_) => {
                    Status::Blocked("Save outcome recorded. Refresh before another action.".into())
                }
                None => Status::Ready,
            },
            None => Status::Blocked("Refresh to verify identity and open recovery storage.".into()),
        }
    }
    pub fn prepare(&mut self, job: &WriteJob) -> Result<(), String> {
        if let Some(error) = &self.blocked {
            return Err(error.clone());
        }
        let result = self
            .active
            .as_mut()
            .ok_or_else(|| STORAGE_ERROR.to_string())
            .and_then(|journal| journal.prepare(job));
        if result.is_err() {
            self.blocked = Some(STORAGE_ERROR.into());
        }
        result.map_err(|_| STORAGE_ERROR.into())
    }
    pub fn finish(&mut self, recovery: bool, outcome: &Outcome) {
        if matches!(outcome, Outcome::Denied) {
            self.close();
            return;
        }
        let phase = match outcome {
            Outcome::Completed => Phase::Completed,
            Outcome::Stopped => Phase::Stopped,
            Outcome::Rejected(_) if !recovery => Phase::Rejected,
            _ => return,
        };
        if let Some(journal) = self.active.as_mut() {
            if let Some(mut record) = journal.record.clone() {
                record.phase = phase;
                // Keep the known final phase in memory even if the disk acknowledgement fails.
                // The disk still holds the original key/body; a restart can safely resolve it.
                journal.record = Some(record.clone());
                if journal.store(&record).is_err() {
                    self.blocked = Some(STORAGE_ERROR.into());
                }
            }
        }
    }
}

struct Journal {
    directory: File,
    lock: File,
    filename: String,
    owner: Owner,
    key: aead::LessSafeKey,
    aad: Vec<u8>,
    record: Option<Record>,
}
impl Drop for Journal {
    fn drop(&mut self) {
        // Closing one descriptor does not release an open-file-description
        // lock while a duplicate (for example across fork before exec) exists.
        // This owner is done; release explicitly before its descriptors close.
        let _ = self.lock.unlock();
    }
}
impl Journal {
    fn open(settings: &Settings, origin: &str, owner: Owner) -> Result<Self, String> {
        let directory = private_directory(&settings.directory).map_err(|_| STORAGE_ERROR)?;
        let aad = serde_json::to_vec(&json!([
            "stateset-freight-tui-recovery",
            1,
            origin,
            owner.tenant,
            owner.subject
        ]))
        .map_err(|_| STORAGE_ERROR)?;
        let scope = hex::encode(digest::digest(&digest::SHA256, &aad));
        let filename = format!("{scope}.sealed");
        let lock: File = fs::openat(
            &directory,
            format!("{scope}.lock"),
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            MODE,
        )
        .map_err(|_| STORAGE_ERROR)?
        .into();
        private_file(&lock).map_err(|_| STORAGE_ERROR)?;
        lock.try_lock().map_err(|_| "Another terminal holds this operator's recovery lock. Browsing is available; close that terminal and refresh.")?;
        let key = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &settings.key).map_err(|_| STORAGE_ERROR)?,
        );
        let mut journal = Self {
            directory,
            lock,
            filename,
            owner,
            key,
            aad,
            record: None,
        };
        journal.record = journal.load().map_err(|_| "Recovery file cannot be authenticated or validated. Keep the file and original key; live saves are blocked.")?;
        Ok(journal)
    }
    fn load(&self) -> Result<Option<Record>, String> {
        let fd = match fs::openat(
            &self.directory,
            &self.filename,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(STORAGE_ERROR.into()),
        };
        let mut file = File::from(fd);
        private_file(&file).map_err(|_| STORAGE_ERROR)?;
        if file.metadata().map_err(|_| STORAGE_ERROR)?.len() > LIMIT {
            return Err(STORAGE_ERROR.into());
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| STORAGE_ERROR)?;
        if bytes.len() as u64 > LIMIT {
            return Err(STORAGE_ERROR.into());
        }
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| STORAGE_ERROR)?;
        if envelope.version != 1 {
            return Err(STORAGE_ERROR.into());
        }
        let nonce: [u8; 12] = hex::decode(envelope.nonce)
            .ok()
            .and_then(|n| n.try_into().ok())
            .ok_or(STORAGE_ERROR)?;
        let mut ciphertext = hex::decode(envelope.ciphertext).map_err(|_| STORAGE_ERROR)?;
        let plaintext = self
            .key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&self.aad),
                &mut ciphertext,
            )
            .map_err(|_| STORAGE_ERROR)?;
        let record: Record = serde_json::from_slice(plaintext).map_err(|_| STORAGE_ERROR)?;
        validate(&record, &self.owner)?;
        Ok(Some(record))
    }
    fn store(&self, record: &Record) -> Result<(), String> {
        validate(record, &self.owner)?;
        let mut ciphertext = serde_json::to_vec(record).map_err(|_| STORAGE_ERROR)?;
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| STORAGE_ERROR)?;
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(&self.aad),
                &mut ciphertext,
            )
            .map_err(|_| STORAGE_ERROR)?;
        let bytes = serde_json::to_vec(&Envelope {
            version: 1,
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        })
        .map_err(|_| STORAGE_ERROR)?;
        if bytes.len() as u64 > LIMIT {
            return Err(STORAGE_ERROR.into());
        }
        let temporary = format!("{}.tmp-{}", self.filename, uuid::Uuid::new_v4());
        let result = (|| {
            let mut file: File = fs::openat(
                &self.directory,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                MODE,
            )
            .map_err(|_| STORAGE_ERROR)?
            .into();
            file.write_all(&bytes).map_err(|_| STORAGE_ERROR)?;
            file.sync_all().map_err(|_| STORAGE_ERROR)?;
            fs::renameat(&self.directory, &temporary, &self.directory, &self.filename)
                .map_err(|_| STORAGE_ERROR)?;
            self.directory.sync_all().map_err(|_| STORAGE_ERROR)?;
            Ok(())
        })();
        let _ = fs::unlinkat(&self.directory, &temporary, AtFlags::empty());
        result
    }
    fn prepare(&mut self, job: &WriteJob) -> Result<(), String> {
        let pending = job.pending();
        let stop_note = match job {
            WriteJob::Stop { note, .. } => Some(note.clone()),
            _ => None,
        };
        match &self.record {
            Some(record)
                if record.phase == Phase::Uncertain
                    && record.pending.key == pending.key
                    && record.pending.body == pending.body
                    && record.pending.target == pending.target
                    && record.pending.owner == pending.owner
                    && record.pending.kind == pending.kind
                    && (record.stop_note.is_none() || record.stop_note == stop_note) => {}
            None if !job.recovery() => {}
            _ => return Err(STORAGE_ERROR.into()),
        }
        let record = Record {
            pending: pending.clone(),
            stop_note,
            phase: Phase::Uncertain,
        };
        validate(&record, &self.owner)?;
        // Remember a possible rename even if directory fsync fails. Never replace it with a new key.
        self.record = Some(record.clone());
        self.store(&record)
    }
    fn clear(&mut self) -> Result<(), String> {
        // A known final phase is re-synced before deletion, including after an earlier I/O failure.
        if let Some(record) = &self.record {
            self.store(record)?;
        }
        match fs::unlinkat(&self.directory, &self.filename, AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(STORAGE_ERROR.into()),
        }
        self.directory.sync_all().map_err(|_| STORAGE_ERROR)?;
        self.record = None;
        Ok(())
    }
}
fn validate(record: &Record, owner: &Owner) -> Result<(), String> {
    let p = &record.pending;
    if &p.owner != owner
        || Owner::from_identity(
            &json!({"tenant_id":owner.tenant,"access":{"subject":owner.subject}}),
        )
        .as_ref()
            != Some(owner)
        || p.key
            .strip_prefix("tui-")
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .is_none()
        || p.target.reference.len() > 4096
        || p.target.status.len() > 200
        || p.target.exception.as_ref().is_some_and(|s| s.len() > 16384)
    {
        return Err(STORAGE_ERROR.into());
    }
    let t = &p.target;
    if t.work.is_some() { if !t.valid_work() { return Err(STORAGE_ERROR.into()); } }
    else if Target::from_load(&json!({"id":t.id,"reference":t.reference,"version":t.version,"status":t.status,"exception":t.exception})).as_ref() != Some(t) { return Err(STORAGE_ERROR.into()); }
    let value: serde_json::Value = serde_json::from_slice(&p.body).map_err(|_| STORAGE_ERROR)?;
    let input = match p.kind {
        Kind::Location => value["location"].as_str(),
        Kind::Exception => value["note"].as_str(),
        Kind::Resolve => Some(""),
        Kind::Work(_) => value["note"].as_str(),
    }
    .ok_or(STORAGE_ERROR)?;
    let expected = if let Kind::Work(kind) = p.kind {
        let field = if kind == crate::work_actions::WorkKind::Defer {
            "defer_until"
        } else {
            "follow_up_at"
        };
        Pending::new_work(
            t.clone(),
            owner.clone(),
            kind,
            input,
            value[field].as_str().unwrap_or_default(),
        )?
    } else {
        Pending::new(t.clone(), owner.clone(), p.kind, input)?
    };
    if expected.body != p.body
        || record.stop_note.as_ref().is_some_and(|n| {
            n.trim() != n || n.is_empty() || n.len() > 200 || crate::model::clean(n) != *n
        })
    {
        return Err(STORAGE_ERROR.into());
    }
    Ok(())
}
fn private_file(file: &File) -> std::io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file()
        || m.uid() != rustix::process::getuid().as_raw()
        || m.mode() & 0o777 != 0o600
        || m.nlink() != 1
    {
        return Err(std::io::Error::other("Unsafe recovery file"));
    }
    Ok(())
}
fn private_directory(path: &Path) -> std::io::Result<File> {
    if !path.is_absolute() {
        return Err(std::io::Error::other("Absolute directory required"));
    }
    let mut directory = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                match fs::mkdirat(&directory, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) => directory.sync_all()?,
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => return Err(e.into()),
                }
                directory = fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?
                .into();
                let m = directory.metadata()?;
                // Refuse untrusted writable ancestors; root-owned sticky /tmp is permitted.
                if (m.uid() != 0 && m.uid() != rustix::process::getuid().as_raw())
                    || (m.mode() & 0o022 != 0 && !(m.uid() == 0 && m.mode() & 0o1000 != 0))
                {
                    return Err(std::io::Error::other("Unsafe directory ancestor"));
                }
            }
            _ => return Err(std::io::Error::other("Non-canonical directory")),
        }
    }
    let m = directory.metadata()?;
    if m.uid() != rustix::process::getuid().as_raw() || m.mode() & 0o777 != 0o700 {
        return Err(std::io::Error::other(
            "Recovery directory must be private (0700)",
        ));
    }
    Ok(directory)
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
