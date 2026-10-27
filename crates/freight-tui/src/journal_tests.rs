use super::*;
use crate::{
    client,
    model::App,
    tests::{response, server},
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    process::Command,
};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = env::temp_dir().join(format!("freight-journal-test-{}", uuid::Uuid::new_v4()));
        Self(path)
    }
    fn settings(&self) -> Settings {
        Settings {
            directory: self.0.clone(),
            key: [0x43; 32],
        }
    }
    fn sealed(&self) -> PathBuf {
        fs::read_dir(&self.0)
            .expect("directory")
            .map(|e| e.expect("entry").path())
            .find(|p| p.extension().is_some_and(|v| v == "sealed"))
            .expect("sealed file")
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn owner() -> Owner {
    Owner {
        tenant: "journal-tenant-canary".into(),
        subject: "journal-subject-canary".into(),
    }
}
fn identity() -> serde_json::Value {
    json!({"tenant_id":owner().tenant,"access":{"subject":owner().subject,"capabilities":["workspace.read","operations.write","saves.recover"]}})
}
fn origin() -> reqwest::Url {
    reqwest::Url::parse("https://freight.example/").expect("origin")
}
fn pending() -> Pending {
    Pending::new(
        Target {
            id: "load-canary".into(),
            reference: "PRIVATE-REFERENCE-CANARY".into(),
            version: 7,
            status: "in_transit".into(),
            exception: None,
            work: None,
        },
        owner(),
        Kind::Location,
        "PRIVATE-LOCATION-CANARY",
    )
    .expect("pending")
}
fn submit(p: &Pending, checking: bool) -> WriteJob {
    WriteJob::Submit {
        pending: p.clone(),
        checking,
    }
}
fn manager(d: &Directory, url: &reqwest::Url) -> Manager {
    let mut m = Manager::new(Ok(d.settings()), url);
    let status = m.read(&identity());
    assert!(
        matches!(status, Status::Ready),
        "journal status: {}",
        match &status {
            Status::Blocked(error) => error.as_str(),
            Status::Pending { .. } => "pending",
            Status::Ready => "ready",
        }
    );
    m
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn encrypted_restart_restores_exact_bytes_key_and_owner_without_network() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("durable preparation");
    let bytes = fs::read(d.sealed()).expect("ciphertext");
    let text = String::from_utf8_lossy(&bytes);
    for canary in [
        &p.key,
        &p.target.reference,
        &p.owner.tenant,
        &p.owner.subject,
        "PRIVATE-LOCATION-CANARY",
    ] {
        assert!(!text.contains(canary));
    }
    assert_eq!(
        fs::metadata(d.sealed())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&d.0).expect("metadata").permissions().mode() & 0o777,
        0o700
    );
    drop(m);
    let mut restored = Manager::new(Ok(d.settings()), &origin());
    let Status::Pending {
        pending: recovered,
        stop_note,
    } = restored.read(&identity())
    else {
        panic!("restore")
    };
    assert_eq!(recovered.body, p.body);
    assert_eq!(recovered.key, p.key);
    assert_eq!(recovered.owner, p.owner);
    assert!(stop_note.is_none());
    let mut app = App {
        identity: identity(),
        ..App::default()
    };
    restored.status().apply(&mut app);
    assert!(app.write_ready.is_none());
    assert!(!app.write_busy);
    assert!(app.write_uncertain);
    assert!(app.recovery_durable);
}

#[test]
fn wrong_key_tampering_and_origin_or_owner_substitution_preserve_the_file() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("prepare");
    drop(m);
    let path = d.sealed();
    let bytes = fs::read(&path).expect("file");
    let mut settings = d.settings();
    settings.key = [0x99; 32];
    let mut bad = Manager::new(Ok(settings), &origin());
    assert!(matches!(bad.read(&identity()), Status::Blocked(_)));
    drop(bad);
    assert_eq!(fs::read(&path).expect("preserved"), bytes);
    // Copy valid ciphertext into a different origin's and owner's scoped filename.
    let mut other =
        Journal::open(&d.settings(), "https://different.example/", owner()).expect("other origin");
    fs::write(d.0.join(&other.filename), &bytes).expect("copy");
    fs::set_permissions(d.0.join(&other.filename), fs::Permissions::from_mode(0o600))
        .expect("private copy");
    assert!(other.load().is_err());
    other.record = None;
    drop(other);
    let mut other_owner = owner();
    other_owner.subject = "someone-else".into();
    let other = Journal::open(&d.settings(), origin().as_str(), other_owner).expect("other owner");
    fs::write(d.0.join(&other.filename), &bytes).expect("copy");
    fs::set_permissions(d.0.join(&other.filename), fs::Permissions::from_mode(0o600))
        .expect("private copy");
    assert!(other.load().is_err());
    drop(other);
    let mut envelope: Envelope = serde_json::from_slice(&bytes).expect("envelope");
    envelope.ciphertext.replace_range(
        0..2,
        if &envelope.ciphertext[..2] == "00" {
            "ff"
        } else {
            "00"
        },
    );
    fs::write(&path, serde_json::to_vec(&envelope).expect("serialize")).expect("tamper");
    let tampered = fs::read(&path).expect("tampered");
    let mut m = Manager::new(Ok(d.settings()), &origin());
    assert!(matches!(m.read(&identity()), Status::Blocked(_)));
    assert_eq!(fs::read(&path).expect("preserved"), tampered);
}

#[test]
fn pending_and_stop_decisions_cannot_be_replaced_and_stop_survives_restart() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("prepare");
    let before = fs::read(d.sealed()).expect("before");
    assert!(m.prepare(&submit(&pending(), false)).is_err());
    assert_eq!(fs::read(d.sealed()).expect("unchanged"), before);
    assert!(matches!(m.read(&identity()), Status::Pending { .. }));
    m.prepare(&WriteJob::Stop {
        pending: p.clone(),
        note: "Operator decision".into(),
    })
    .expect("stop persisted");
    drop(m);
    let mut m = Manager::new(Ok(d.settings()), &origin());
    let Status::Pending { stop_note, .. } = m.read(&identity()) else {
        panic!("stop restore")
    };
    assert_eq!(stop_note.as_deref(), Some("Operator decision"));
    assert!(m.prepare(&submit(&p, true)).is_err());
    m.read(&identity());
    assert!(m
        .prepare(&WriteJob::Stop {
            pending: p.clone(),
            note: "Changed decision".into()
        })
        .is_err());
    m.read(&identity());
    m.prepare(&WriteJob::Stop {
        pending: p,
        note: "Operator decision".into(),
    })
    .expect("same resolution");
}

#[test]
fn process_lock_probe() {
    let Some(path) = env::var_os("FREIGHT_JOURNAL_TEST_PROBE") else {
        return;
    };
    let d = Directory(PathBuf::from(path));
    assert!(Journal::open(&d.settings(), origin().as_str(), owner()).is_err());
    // The parent owns cleanup.
    std::mem::forget(d);
}
#[test]
fn another_process_cannot_claim_the_same_operator_until_lock_is_released() {
    let d = Directory::new();
    let m = manager(&d, &origin());
    let output = Command::new(env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "journal::tests::process_lock_probe",
            "--nocapture",
        ])
        .env("FREIGHT_JOURNAL_TEST_PROBE", &d.0)
        .output()
        .expect("child process");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    drop(m);
    assert!(Journal::open(&d.settings(), origin().as_str(), owner()).is_ok());
}

#[test]
fn unsafe_directories_files_and_symlinks_fail_closed() {
    let d = Directory::new();
    fs::create_dir(&d.0).expect("directory");
    fs::set_permissions(&d.0, fs::Permissions::from_mode(0o755)).expect("mode");
    assert!(Journal::open(&d.settings(), origin().as_str(), owner()).is_err());
    fs::set_permissions(&d.0, fs::Permissions::from_mode(0o700)).expect("mode");
    let mut m = manager(&d, &origin());
    let p = pending();
    m.prepare(&submit(&p, false)).expect("prepare");
    drop(m);
    let path = d.sealed();
    let bytes = fs::read(&path).expect("file");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode");
    assert!(Journal::open(&d.settings(), origin().as_str(), owner()).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("mode");
    let target = d.0.join("original");
    fs::rename(&path, &target).expect("rename");
    symlink(&target, &path).expect("symlink");
    assert!(Journal::open(&d.settings(), origin().as_str(), owner()).is_err());
    assert_eq!(fs::read(&target).expect("preserved"), bytes);
    let link = Directory::new();
    symlink(&d.0, &link.0).expect("directory symlink");
    assert!(Journal::open(&link.settings(), origin().as_str(), owner()).is_err());
    fs::remove_file(&link.0).expect("remove symlink");
}
#[test]
fn malformed_oversized_and_semantically_invalid_records_are_refused() {
    let d = Directory::new();
    let mut j = Journal::open(&d.settings(), origin().as_str(), owner()).expect("journal");
    let path = d.0.join(&j.filename);
    let mut file: File = rustix::fs::openat(
        &j.directory,
        &j.filename,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY,
        MODE,
    )
    .expect("create")
    .into();
    file.write_all(b"not an envelope").expect("write");
    assert!(j.load().is_err());
    file.set_len(LIMIT + 1).expect("oversize");
    assert!(j.load().is_err());
    drop(file);
    let mut record = Record {
        pending: pending(),
        stop_note: None,
        phase: Phase::Uncertain,
    };
    record.pending.body =
        br#"{"action":"location","expected_version":7,"location":"x","injected":true}"#.to_vec();
    assert!(j.store(&record).is_err());
    record.pending = pending();
    record.pending.target.id = "../escape".into();
    assert!(j.store(&record).is_err());
    record.pending = pending();
    record.stop_note = Some("\u{1b}unsafe".into());
    assert!(j.store(&record).is_err());
    j.record = None;
    assert_eq!(
        fs::metadata(path).expect("original retained").len(),
        LIMIT + 1
    );
}

#[test]
fn unavailable_storage_prevents_even_the_access_request_and_retains_original() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    fs::remove_dir_all(&d.0).expect("remove backing directory");
    let (outcome, status) =
        runtime().block_on(m.execute(&reqwest::Client::new(), &origin(), &submit(&p, false)));
    assert!(matches!(outcome, Outcome::Uncertain(_)));
    assert!(matches!(status, Status::Blocked(_)));
    assert_eq!(
        m.active
            .as_ref()
            .expect("journal")
            .record
            .as_ref()
            .expect("retained")
            .pending
            .key,
        p.key
    );
}
#[test]
fn confirmed_ack_is_restored_as_read_only_cleanup_after_failed_refresh() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("prepare");
    m.finish(false, &Outcome::Completed);
    drop(m);
    let mut m = Manager::new(Ok(d.settings()), &origin());
    let mut app = App::default();
    app.finish(
        0,
        Err(client::Failure {
            message: "refresh failed".into(),
            denied: false,
        }),
    );
    // Merely restarting and failing a read does not open or remove the receipt.
    assert!(d.sealed().exists());
    assert!(m.active.is_none());
    assert!(app.write_ready.is_none());
    let status = m.read(&identity());
    assert!(
        matches!(status, Status::Ready),
        "restored final ack status: {}",
        match &status {
            Status::Blocked(error) => error.as_str(),
            Status::Pending { .. } => "pending",
            Status::Ready => "ready",
        }
    );
    assert!(fs::read_dir(&d.0).expect("directory").all(|e| e
        .expect("entry")
        .path()
        .extension()
        .is_none_or(|v| v != "sealed")));
    assert!(m.prepare(&submit(&pending(), false)).is_ok());
}
#[test]
fn closing_a_journal_releases_its_lock_even_when_an_inherited_descriptor_remains_open() {
    let d = Directory::new();
    let p = pending();
    let mut original = manager(&d, &origin());
    original
        .prepare(&submit(&p, false))
        .expect("original saved");
    // A duplicate shares the open-file description just as a descriptor
    // inherited across fork does before CLOEXEC closes it in the child.
    let inherited = original
        .active
        .as_ref()
        .expect("journal")
        .lock
        .try_clone()
        .expect("duplicate descriptor");
    drop(original);
    let mut restored = Manager::new(Ok(d.settings()), &origin());
    let status = restored.read(&identity());
    let Status::Pending {
        pending: retained, ..
    } = status
    else {
        panic!(
            "original lock was not released: {}",
            match status {
                Status::Blocked(error) => error,
                _ => "unexpected ready".into(),
            }
        );
    };
    assert_eq!(retained.key, p.key);
    assert_eq!(retained.body, p.body);
    let mut competing = Manager::new(Ok(d.settings()), &origin());
    assert!(
        matches!(competing.read(&identity()), Status::Blocked(_)),
        "the new owner still holds its own lock"
    );
    drop(inherited);
    assert!(
        matches!(competing.read(&identity()), Status::Blocked(_)),
        "closing the old descriptor cannot unlock the new owner"
    );
    drop(restored);
    assert!(matches!(
        competing.read(&identity()),
        Status::Pending { .. }
    ));
}
#[test]
fn denied_access_preserves_encrypted_original_and_owner_changes_clear_presentation() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("prepare");
    let mut app = App {
        identity: identity(),
        ..App::default()
    };
    m.status().apply(&mut app);
    let bytes = fs::read(d.sealed()).expect("file");
    m.finish(false, &Outcome::Denied);
    app.finish_write(false, Outcome::Denied);
    assert!(app.pending.is_none());
    assert!(m.active.is_none());
    assert_eq!(fs::read(d.sealed()).expect("retained"), bytes);
    let mut m = Manager::new(Ok(d.settings()), &origin());
    m.read(&identity()).apply(&mut app);
    assert!(
        app.pending.is_none(),
        "unverified presentation cannot restore private context"
    );
    app.identity = identity();
    m.status().apply(&mut app);
    assert_eq!(app.pending.as_ref().expect("restored").key, p.key);
    let mut snapshot = crate::demo::snapshot(&app.request());
    snapshot.identity = json!({"tenant_id":"different","access":{"subject":"other"}});
    app.apply(snapshot);
    Status::Blocked("key unavailable".into()).apply(&mut app);
    assert!(app.pending.is_none());
    assert!(app.stopping_note.is_none());
    assert!(app.action_dialog.is_none());
}
#[test]
fn restored_stop_sends_only_exact_resolution_after_explicit_execution() {
    let d = Directory::new();
    let p = pending();
    let (url, handle) = server(vec![
        response(identity()),
        response(json!({"data":{"outcome":"stopped","request_key":p.key},"replayed":true})),
    ]);
    let mut m = manager(&d, &url);
    m.prepare(&submit(&p, false)).expect("prepare");
    let job = WriteJob::Stop {
        pending: p.clone(),
        note: "Retained stop note".into(),
    };
    m.prepare(&job).expect("stop intent");
    drop(m);
    let mut m = Manager::new(Ok(d.settings()), &url);
    let Status::Pending { pending, stop_note } = m.read(&identity()) else {
        panic!("restore")
    };
    let job = WriteJob::Stop {
        pending: *pending,
        note: stop_note.expect("retained note"),
    };
    let (outcome, _) = runtime().block_on(m.execute(&reqwest::Client::new(), &url, &job));
    let detail = match &outcome {
        Outcome::Rejected(message) | Outcome::Uncertain(message) => message.as_str(),
        Outcome::Denied => "denied",
        Outcome::Completed => "completed",
        Outcome::Stopped => "stopped",
    };
    assert!(
        matches!(outcome, Outcome::Stopped),
        "restored stop outcome: {detail}"
    );
    let requests = handle.join().expect("server");
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/access "));
    assert!(requests[1].starts_with("POST /v1/save-recovery/resolve "));
    let body: serde_json::Value =
        serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).expect("body")).expect("json");
    assert_eq!(body["request_key"], p.key);
    assert_eq!(body["note"], "Retained stop note");
    assert_eq!(
        body["body"],
        serde_json::from_slice::<serde_json::Value>(&p.body).expect("original")
    );
}

#[test]
fn failed_final_record_replacement_blocks_writes_until_read_only_storage_repair() {
    let d = Directory::new();
    let p = pending();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("prepare");
    let path = d.sealed();
    let backup = d.0.join("original-backup");
    let original = fs::read(&path).expect("encrypted original");
    fs::rename(&path, &backup).expect("move original");
    fs::create_dir(&path).expect("obstruct atomic replacement");
    m.finish(false, &Outcome::Completed);
    assert!(matches!(m.status(), Status::Blocked(_)));
    assert!(m.prepare(&submit(&pending(), false)).is_err());
    assert!(matches!(m.read(&identity()), Status::Blocked(_)));
    assert_eq!(fs::read(&backup).expect("original retained"), original);
    assert_eq!(
        m.active
            .as_ref()
            .expect("journal")
            .record
            .as_ref()
            .expect("record")
            .phase as u8,
        Phase::Completed as u8
    );
    fs::remove_dir(&path).expect("remove obstruction");
    fs::rename(&backup, &path).expect("restore original");
    assert!(matches!(m.read(&identity()), Status::Ready));
    assert!(!path.exists());
    assert!(m.prepare(&submit(&pending(), false)).is_ok());
}
#[test]
fn blocked_storage_disables_both_new_actions_and_original_checks() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App {
        needs_fetch: false,
        ..App::default()
    };
    app.apply(crate::demo::snapshot(&app.request()));
    Status::Blocked("Recovery key is required".into()).apply(&mut app);
    app.open_action();
    assert!(app.action_dialog.is_none());
    app.pending = Some(pending());
    app.write_uncertain = true;
    for c in ['c', 'x'] {
        app.action_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        assert!(app.write_ready.is_none());
        assert!(app.action_dialog.is_none());
    }
    app.pending = None;
    Status::Ready.apply(&mut app);
    assert!(app.write_notice.is_none());
    app.open_action();
    assert!(app.action_dialog.is_some());
}

#[test]
fn work_original_and_stop_decision_restore_with_the_exact_time_body_key_and_source() {
    use crate::work_actions::WorkKind;
    let target=Target::from_issue(&json!({"id":"work-canary","load_reference":"PRIVATE-WORK-CANARY","source_sha256":"c".repeat(64),"source_active":true,"source_changed":false,"evidence_valid":true,"signal":{"priority":1},"owner":null,"case":null})).expect("work identity");
    // Time bounds apply at review/application; an exact original remains recoverable after its time passes.
    let p = Pending::new_work(
        target,
        owner(),
        WorkKind::Claim,
        "PRIVATE-WORK-NOTE",
        "2000-01-01T01:00:00Z",
    )
    .expect("canonical original");
    let d = Directory::new();
    let mut m = manager(&d, &origin());
    m.prepare(&submit(&p, false)).expect("sealed work");
    m.prepare(&WriteJob::Stop {
        pending: p.clone(),
        note: "Retained original work stop".into(),
    })
    .expect("sealed stop");
    let bytes = fs::read(d.sealed()).expect("ciphertext");
    for text in [
        p.key.as_str(),
        p.target.id.as_str(),
        "PRIVATE-WORK-NOTE",
        "2000-01-01T01:00:00Z",
        "Retained original work stop",
    ] {
        assert!(!String::from_utf8_lossy(&bytes).contains(text));
    }
    drop(m);
    let mut restored = Manager::new(Ok(d.settings()), &origin());
    let Status::Pending {
        pending: got,
        stop_note,
    } = restored.read(&identity())
    else {
        panic!("work restoration")
    };
    assert_eq!(
        serde_json::to_value(&got).expect("restored"),
        serde_json::to_value(&p).expect("original")
    );
    assert_eq!(stop_note.as_deref(), Some("Retained original work stop"));
    let mut app = App {
        identity: identity(),
        ..App::default()
    };
    restored.status().apply(&mut app);
    assert!(app.write_ready.is_none() && app.write_uncertain);
}

#[test]
fn work_journal_rejects_source_time_action_or_extra_body_changes_and_cross_kind_targets() {
    use crate::work_actions::WorkKind;
    let target=Target::from_issue(&json!({"id":"work-canary","load_reference":"Work","source_sha256":"c".repeat(64),"source_active":true,"source_changed":false,"evidence_valid":true,"signal":{"priority":1},"owner":null,"case":null})).expect("work identity");
    let original = Pending::new_work(
        target,
        owner(),
        WorkKind::Claim,
        "Actual findings",
        "2026-10-21T01:00:00Z",
    )
    .expect("canonical work");
    for (field, value) in [
        ("source_sha256", json!("d".repeat(64))),
        ("follow_up_at", json!("not-a-time")),
        ("action", json!("force_release")),
        ("subject", json!("forged-admin")),
        ("expected_version", json!(99)),
    ] {
        let mut p = original.clone();
        let mut body: serde_json::Value = serde_json::from_slice(&p.body).expect("original body");
        body[field] = value;
        p.body = serde_json::to_vec(&body).expect("forged body");
        let record = Record {
            pending: p,
            stop_note: None,
            phase: Phase::Uncertain,
        };
        assert!(validate(&record, &owner()).is_err(), "{field}");
    }
    let mut p = original;
    p.kind = Kind::Resolve;
    let record = Record {
        pending: p,
        stop_note: None,
        phase: Phase::Uncertain,
    };
    assert!(validate(&record, &owner()).is_err());
    let old = pending();
    let bytes = serde_json::to_value(&old).expect("old envelope shape");
    assert!(bytes["target"].get("work").is_none());
    let old: Pending =
        serde_json::from_value(bytes).expect("existing shipment original remains compatible");
    assert!(old.target.work.is_none());
}
