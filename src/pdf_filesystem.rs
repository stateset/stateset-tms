//! Mandatory native-worker filesystem rules. No best-effort fallback.
use landlock::{
    Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, ABI,
};
use std::{collections::BTreeSet, io, path::Path};

pub(super) fn install(probe: bool) -> io::Result<()> {
    // ABI1 protects reads/exec/creation/removal. The syscall policy additionally
    // blocks truncation, link/rename and operations introduced by later ABIs.
    let mut rules = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(ABI::V1))
        .map_err(io::Error::other)?
        .create()
        .map_err(io::Error::other)?;
    let mut libraries = BTreeSet::new();
    for path in ["/usr/lib", "/usr/lib64", "/lib", "/lib64"] {
        let path = Path::new(path);
        if !path.try_exists()? {
            continue;
        }
        let resolved = path.canonicalize()?;
        if !["/usr/lib", "/usr/lib64", "/lib", "/lib64"]
            .iter()
            .any(|root| resolved.starts_with(root))
        {
            return Err(io::Error::other("Unexpected native library hierarchy"));
        }
        libraries.insert(resolved);
    }
    if libraries.is_empty() {
        return Err(io::Error::other("Native libraries unavailable"));
    }
    for path in libraries {
        rules = rules
            .add_rule(PathBeneath::new(
                PathFd::new(path).map_err(io::Error::other)?,
                AccessFs::ReadFile | AccessFs::ReadDir | AccessFs::Execute,
            ))
            .map_err(io::Error::other)?;
    }
    for name in [
        "/etc/fonts",
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/usr/share/fontconfig",
        "/usr/share/poppler",
        "/var/cache/fontconfig",
    ] {
        let path = Path::new(name);
        if !path.try_exists()? {
            continue;
        }
        // Standard Debian/Ubuntu runtime trees only; never follow a font/config
        // hierarchy redirected into an application's data or home directory.
        if !path.canonicalize()?.starts_with(path) {
            return Err(io::Error::other("Redirected font/config hierarchy"));
        }
        rules = rules
            .add_rule(PathBeneath::new(
                PathFd::new(path).map_err(io::Error::other)?,
                AccessFs::ReadFile | AccessFs::ReadDir,
            ))
            .map_err(io::Error::other)?;
    }
    for name in ["/usr/bin/pdfinfo", "/usr/bin/pdftoppm"] {
        rules = rules
            .add_rule(PathBeneath::new(
                PathFd::new(name).map_err(io::Error::other)?,
                AccessFs::ReadFile | AccessFs::Execute,
            ))
            .map_err(io::Error::other)?;
    }
    for name in ["/etc/ld.so.cache", "/dev/null", "/dev/urandom"] {
        if Path::new(name).try_exists()? {
            rules = rules
                .add_rule(PathBeneath::new(
                    PathFd::new(name).map_err(io::Error::other)?,
                    AccessFs::ReadFile,
                ))
                .map_err(io::Error::other)?;
        }
    }
    if probe {
        rules = rules
            .add_rule(PathBeneath::new(
                PathFd::new("/proc/self/status").map_err(io::Error::other)?,
                AccessFs::ReadFile,
            ))
            .map_err(io::Error::other)?;
    }
    let status = rules.restrict_self().map_err(io::Error::other)?;
    if status.ruleset != RulesetStatus::FullyEnforced || !status.no_new_privs {
        return Err(io::Error::other("Native filesystem policy not enforced"));
    }
    Ok(())
}
