//! PDF parsing stays outside the API process, with no inherited environment or disk output.
use super::{Failure, Rendered, MAX_PAGES, OUTPUT_LIMIT};
use crate::documents::MAX_DOCUMENT_BYTES;
use std::{ffi::OsStr, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const WALL_LIMIT: Duration = Duration::from_secs(5);

fn invalid() -> Failure {
    Failure::Invalid
}
fn unavailable() -> Failure {
    Failure::Unavailable
}

// Fixed executable paths and arguments: a filename, PDF metadata or URL never becomes a command.
// This wrapper sets resource/privilege limits. The fixed PDF helper installs a
// default-deny syscall policy and native Landlock before exec. Container mode
// is called only by the isolated renderer service.
fn command(tool: impl AsRef<OsStr>, args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/prlimit");
    command
        .args([
            "--as=268435456",
            "--cpu=4",
            "--fsize=0",
            "--core=0",
            "--nofile=32",
            "--stack=8388608",
            "--",
            "/usr/bin/setpriv",
            "--no-new-privs",
            "--",
        ])
        .arg(tool)
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn run(
    mut command: Command,
    bytes: &[u8],
    limit: usize,
    wall: Duration,
) -> Result<Vec<u8>, Failure> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| unavailable())?;
    let mut input = child.stdin.take().ok_or_else(unavailable)?;
    let output = child.stdout.take().ok_or_else(unavailable)?;
    let result = tokio::time::timeout(wall, async {
        let write = async {
            input.write_all(bytes).await.map_err(|_| invalid())?;
            input.shutdown().await.map_err(|_| invalid())?;
            drop(input);
            Ok::<_, Failure>(())
        };
        let read = async {
            let mut data = Vec::new();
            output
                .take(limit as u64 + 1)
                .read_to_end(&mut data)
                .await
                .map_err(|_| invalid())?;
            if data.len() > limit {
                return Err(invalid());
            }
            Ok::<_, Failure>(data)
        };
        let transferred = tokio::try_join!(write, read);
        let status = child.wait().await.map_err(|_| invalid())?;
        if status.code() == Some(78) {
            return Err(unavailable());
        }
        if !status.success() {
            return Err(invalid());
        }
        let (_, data) = transferred?;
        Ok(data)
    })
    .await;
    match result {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(error)) if error == Failure::Unavailable => Err(error),
        _ => {
            // Reap before returning: the caller retains the decoder permit until this completes.
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(invalid())
        }
    }
}

fn sandbox_path() -> Result<PathBuf, Failure> {
    let executable = std::env::current_exe().map_err(|_| unavailable())?;
    let directory = executable.parent().ok_or_else(unavailable)?;
    // Cargo library test executables live below debug/deps. Real API children
    // re-execute that exact API binary, without a separate installed helper.
    if directory.file_name() == Some(OsStr::new("deps")) {
        let path = directory
            .parent()
            .ok_or_else(unavailable)?
            .join("stateset-freight");
        return if path.is_file() {
            Ok(path)
        } else {
            Err(unavailable())
        };
    }
    Ok(executable)
}

pub(super) async fn probe_container_policy() -> Result<serde_json::Value, Failure> {
    let output = run(
        command(
            sandbox_path()?,
            &["--pdf-isolated-worker", "--probe-policy"],
        ),
        b"",
        32 * 1024,
        WALL_LIMIT,
    )
    .await?;
    let value: serde_json::Value = serde_json::from_slice(&output).map_err(|_| unavailable())?;
    let before = value["filter_count_before"]
        .as_u64()
        .ok_or_else(unavailable)?;
    let after = before.checked_add(1).ok_or_else(unavailable)?;
    if value.as_object().map(|v| v.len()) != Some(10)
        || value["result"] != "pass"
        || value["socket_denials"] != 6
        || value["spawn_denied"] != true
        || value["no_new_privileges"] != true
        || value["threads"] != 1
        || before == 0
        || value["filter_count_after"].as_u64() != Some(after)
        || value["readonly_file_opens"] != true
        || value["write_open_denials"] != 9
        || value["filesystem_isolation"] != false
    {
        return Err(unavailable());
    }
    Ok(value)
}

fn pages(info: &[u8]) -> Result<u32, Failure> {
    let text = std::str::from_utf8(info).map_err(|_| invalid())?;
    let mut counts = text.lines().filter_map(|line| line.strip_prefix("Pages:"));
    let pages = counts
        .next()
        .ok_or_else(invalid)?
        .trim()
        .parse::<u32>()
        .map_err(|_| invalid())?;
    // Reject duplicate/forged metadata fields instead of guessing which count to trust.
    let mut encrypted = text
        .lines()
        .filter_map(|line| line.strip_prefix("Encrypted:"));
    if counts.next().is_some()
        || encrypted.next().map(str::trim) != Some("no")
        || encrypted.next().is_some()
        || pages == 0
        || pages > MAX_PAGES
    {
        return Err(invalid());
    }
    Ok(pages)
}

pub(super) async fn render(bytes: &[u8], page: u32, container: bool) -> Result<Rendered, Failure> {
    if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES || page == 0 || page > MAX_PAGES {
        return Err(invalid());
    }
    if !cfg!(target_os = "linux") {
        return Err(unavailable());
    }
    if [
        "/usr/bin/prlimit",
        "/usr/bin/setpriv",
        "/usr/bin/pdfinfo",
        "/usr/bin/pdftoppm",
    ]
    .iter()
    .any(|path| !std::path::Path::new(path).is_file())
    {
        return Err(unavailable());
    }
    let sandbox = sandbox_path()?;
    let role = if container {
        "--pdf-isolated-worker"
    } else {
        "--pdf-worker"
    };
    let info = run(
        command(&sandbox, &[role, "info"]),
        bytes,
        32 * 1024,
        WALL_LIMIT,
    )
    .await?;
    let pages = pages(&info)?;
    if page > pages {
        return Err(invalid());
    }
    let page = page.to_string();
    let png = run(
        command(&sandbox, &[role, "page", &page]),
        bytes,
        OUTPUT_LIMIT,
        WALL_LIMIT,
    )
    .await?;
    Ok(Rendered { png, pages })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_requires_one_bounded_count_and_no_encryption() {
        assert_eq!(
            pages(b"Title: POD\nPages: 2\nEncrypted: no\n").ok(),
            Some(2)
        );
        for info in [
            "Pages: 0\nEncrypted: no",
            "Pages: 1001\nEncrypted: no",
            "Pages: 1\nEncrypted: yes (print:yes)",
            "Pages: 2\nPages: 1\nEncrypted: no",
            "Pages: 1\nEncrypted: no\nEncrypted: no",
            "Pages: 1",
            "Pages: -1\nEncrypted: no",
        ] {
            assert!(pages(info.as_bytes()).is_err(), "{info}");
        }
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn worker_bounds_output_timeout_and_exit_and_is_reaped() {
        let mut output = Command::new("/usr/bin/printf");
        output.arg("oversized");
        assert!(run(output, b"", 4, WALL_LIMIT).await.is_err());
        let mut failed = Command::new("/usr/bin/false");
        failed.stdin(Stdio::piped());
        assert!(run(failed, b"", 4, WALL_LIMIT).await.is_err());
        // Exercise the real limits/privilege wrapper as well as the pipe boundary.
        let result = run(command("/usr/bin/printf", &["owned"]), b"", 5, WALL_LIMIT)
            .await
            .unwrap_or_else(|_| panic!("worker"));
        assert_eq!(result, b"owned");
        let start = std::time::Instant::now();
        assert!(run(
            command("/usr/bin/sleep", &["30"]),
            b"",
            4,
            Duration::from_millis(30)
        )
        .await
        .is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn policy_setup_failure_is_unavailable_even_when_the_input_pipe_breaks() {
        let mut failed_policy = Command::new("/bin/sh");
        failed_policy.args(["-c", "exit 78"]);
        let error = run(failed_policy, &vec![0; 1024 * 1024], 1024, WALL_LIMIT)
            .await
            .expect_err("failed policy must not become a preview");
        assert_eq!(error, Failure::Unavailable);
        let mut invalid_pdf = Command::new("/bin/sh");
        invalid_pdf.args(["-c", "exit 1"]);
        let error = run(invalid_pdf, &vec![0; 1024 * 1024], 1024, WALL_LIMIT)
            .await
            .expect_err("failed renderer must not become a preview");
        assert_eq!(error, Failure::Invalid);
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn worker_has_no_inherited_environment_or_privilege_gain_and_has_resource_limits() {
        let env = run(command("/usr/bin/env", &[]), b"", 1024, WALL_LIMIT)
            .await
            .unwrap_or_else(|_| panic!("env"));
        let env = String::from_utf8(env).expect("environment encoding");
        let mut fields: Vec<_> = env.lines().collect();
        fields.sort_unstable();
        assert_eq!(fields, ["HOME=/nonexistent", "LC_ALL=C"]);
        let status = run(
            command("/usr/bin/cat", &["/proc/self/status"]),
            b"",
            8192,
            WALL_LIMIT,
        )
        .await
        .unwrap_or_else(|_| panic!("status"));
        assert!(String::from_utf8(status)
            .unwrap_or_else(|_| panic!("status"))
            .contains("NoNewPrivs:\t1\n"));
        let limits = run(
            command("/usr/bin/cat", &["/proc/self/limits"]),
            b"",
            8192,
            WALL_LIMIT,
        )
        .await
        .unwrap_or_else(|_| panic!("limits"));
        let limits = String::from_utf8(limits).unwrap_or_else(|_| panic!("limits"));
        for (name, expected) in [
            ("Max cpu time", "4"),
            ("Max address space", "268435456"),
            ("Max file size", "0"),
            ("Max core file size", "0"),
            ("Max open files", "32"),
            ("Max stack size", "8388608"),
        ] {
            let line = limits
                .lines()
                .find(|line| line.starts_with(name))
                .expect("limit");
            let values: Vec<_> = line[name.len()..].split_whitespace().collect();
            assert_eq!(&values[..2], [expected, expected], "{name}");
        }
    }
}
