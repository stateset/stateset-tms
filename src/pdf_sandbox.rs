//! A single-threaded, fail-closed syscall boundary installed before parsing a PDF.
//!
//! Native workers also require Landlock. Container workers are launched only by
//! the dedicated isolated renderer service, never by the API's local backend.
use std::io;

#[cfg(target_os = "linux")]
#[path = "pdf_descriptors.rs"]
mod descriptors;
#[cfg(target_os = "linux")]
#[path = "pdf_filesystem.rs"]
mod filesystem;
#[cfg(target_os = "linux")]
#[path = "pdf_sandbox_policy.rs"]
mod policy;

#[derive(Debug, PartialEq)]
enum Tool {
    Info,
    Page(u32),
    Probe,
}

fn parse(args: &[String]) -> Result<Tool, &'static str> {
    match args {
        [mode] if mode == "info" => Ok(Tool::Info),
        [mode] if mode == "--probe-policy" => Ok(Tool::Probe),
        [mode, page] if mode == "page" => {
            let value = page.parse::<u32>().map_err(|_| "Invalid page")?;
            if value == 0 || value > 1000 || page != &value.to_string() {
                return Err("Invalid page");
            }
            Ok(Tool::Page(value))
        }
        _ => Err("Usage: stateset-freight --pdf-worker info | page <1..1000> | --probe-policy"),
    }
}

#[cfg(target_os = "linux")]
fn run(tool: Tool, container: bool) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let before = if tool == Tool::Probe {
        Some(policy::baseline()?)
    } else {
        None
    };
    descriptors::close_inherited()?;
    std::env::set_current_dir("/")?;
    if container {
        if nix::unistd::getuid().as_raw() != 10002 || nix::unistd::getppid().as_raw() != 1 {
            return Err(io::Error::other("Isolated renderer parent unavailable"));
        }
    } else {
        filesystem::install(tool == Tool::Probe)?;
    }
    policy::install()?;
    if let Some(before) = before {
        return policy::probe(before, !container);
    }
    let mut command = match tool {
        Tool::Info => {
            let mut command = std::process::Command::new("/usr/bin/pdfinfo");
            command.arg("-");
            command
        }
        Tool::Page(page) => {
            let mut command = std::process::Command::new("/usr/bin/pdftoppm");
            command.args([
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-singlefile",
                "-scale-to",
                "960",
                "-png",
                "-",
            ]);
            command
        }
        Tool::Probe => unreachable!("handled before tool selection"),
    };
    // No caller-supplied executable, filename, environment or extra renderer options.
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("HOME", "/nonexistent");
    Err(command.exec())
}

#[cfg(not(target_os = "linux"))]
fn run(_: Tool, _: bool) -> io::Result<()> {
    Err(io::Error::other("PDF syscall policy requires Linux"))
}

pub(crate) fn entry(args: &[String], container: bool) -> u8 {
    let tool = match parse(args) {
        Ok(tool) => tool,
        Err(error) => {
            eprintln!("{error}");
            return 78;
        }
    };
    match run(tool, container) {
        Ok(()) => 0,
        Err(_) => {
            eprintln!("PDF syscall policy or fixed renderer unavailable");
            78
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn only_fixed_modes_and_canonical_bounded_pages_are_accepted() {
        assert_eq!(parse(&args(&["info"])), Ok(Tool::Info));
        assert_eq!(parse(&args(&["page", "1"])), Ok(Tool::Page(1)));
        assert_eq!(parse(&args(&["page", "1000"])), Ok(Tool::Page(1000)));
        for values in [
            vec![],
            vec!["/bin/sh"],
            vec!["info", "filename"],
            vec!["page", "0"],
            vec!["page", "1001"],
            vec!["page", "01"],
            vec!["page", "+1"],
            vec!["page", "-1"],
            vec!["page", "1", "-opw", "password"],
        ] {
            assert!(parse(&args(&values)).is_err(), "{values:?}");
        }
    }
}
