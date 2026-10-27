//! Fail closed on deployment settings that would expose API data to the renderer.
use std::{collections::BTreeSet, io};

pub(super) fn validate() -> io::Result<()> {
    for (name, value) in std::env::vars_os() {
        if !value.is_empty()
            && !matches!(
                name.to_str(),
                Some(
                    "PATH"
                        | "HOSTNAME"
                        | "HOME"
                        | "LANG"
                        | "LC_ALL"
                        | "TZ"
                        | "RUST_LOG"
                        | "FREIGHT_BIND_ADDR"
                )
            )
        {
            return Err(io::Error::other(
                "Renderer must not receive service credentials or API settings",
            ));
        }
    }
    if nix::unistd::getpid().as_raw() != 1
        || nix::unistd::getuid().as_raw() != 10002
        || nix::unistd::getgid().as_raw() != 10001
    {
        return Err(io::Error::other(
            "Renderer requires its dedicated unprivileged PID namespace",
        ));
    }
    let status = std::fs::read_to_string("/proc/self/status")?;
    if !status.contains("NoNewPrivs:\t1\n") || !status.contains("CapEff:\t0000000000000000\n") {
        return Err(io::Error::other("Renderer privileges must be dropped"));
    }
    let interfaces: BTreeSet<_> = std::fs::read_dir("/sys/class/net")?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    if interfaces != BTreeSet::from([std::ffi::OsString::from("lo")]) {
        return Err(io::Error::other("Renderer requires network isolation"));
    }
    check_mounts(&std::fs::read_to_string("/proc/self/mountinfo")?)
}

fn check_mounts(mounts: &str) -> io::Result<()> {
    let allowed = [
        "/",
        "/proc",
        "/dev",
        "/dev/pts",
        "/dev/shm",
        "/dev/mqueue",
        "/sys",
        "/sys/fs/cgroup",
        "/etc/resolv.conf",
        "/etc/hostname",
        "/etc/hosts",
        "/tmp",
        "/run/freight-pdf",
        "/proc/bus",
        "/proc/fs",
        "/proc/irq",
        "/proc/sys",
        "/proc/sysrq-trigger",
        "/proc/acpi",
        "/proc/asound",
        "/proc/kcore",
        "/proc/keys",
        "/proc/latency_stats",
        "/proc/timer_list",
        "/proc/timer_stats",
        "/proc/sched_debug",
        "/proc/scsi",
        "/sys/firmware",
        "/sys/devices/virtual/powercap",
    ];
    let mut root_readonly = false;
    let mut socket_tmpfs = false;
    for line in mounts.lines() {
        let (mount, source) = line
            .split_once(" - ")
            .ok_or_else(|| io::Error::other("Invalid renderer mount table"))?;
        let fields: Vec<_> = mount.split_whitespace().collect();
        let controller = fields
            .get(4)
            .and_then(|path| path.strip_prefix("/sys/fs/cgroup/"));
        let standard_controller = matches!(
            controller,
            Some(
                "systemd"
                    | "net_cls,net_prio"
                    | "pids"
                    | "freezer"
                    | "perf_event"
                    | "rdma"
                    | "devices"
                    | "hugetlb"
                    | "misc"
                    | "cpu,cpuacct"
                    | "memory"
                    | "blkio"
                    | "cpuset"
                    | "unified"
                    | "cpu"
                    | "cpuacct"
                    | "net_cls"
                    | "net_prio"
            )
        );
        if fields.len() < 6 || (!allowed.contains(&fields[4]) && !standard_controller) {
            return Err(io::Error::other("Unexpected renderer data mount"));
        }
        if fields[4] == "/" {
            root_readonly = fields[5].split(',').any(|flag| flag == "ro");
        }
        if fields[4] == "/run/freight-pdf" {
            socket_tmpfs = source.starts_with("tmpfs ");
        }
    }
    if !root_readonly || !socket_tmpfs {
        return Err(io::Error::other(
            "Renderer requires a read-only image and private tmpfs socket volume",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mount_policy_refuses_data_mounts_writable_roots_and_persistent_socket_storage() {
        let valid = "1 0 0:1 / / ro - overlay overlay ro\n2 1 0:2 / /run/freight-pdf rw - tmpfs tmpfs rw\n3 1 0:3 / /sys/fs/cgroup/memory ro - cgroup cgroup ro\n";
        assert!(check_mounts(valid).is_ok());
        for invalid in [
            valid.replace("/ ro - overlay", "/ rw - overlay"),
            valid.replace("- tmpfs tmpfs", "- ext4 /dev/vda"),
            valid.to_string() + "4 1 0:4 / /run/secrets/api ro - ext4 /dev/vda ro\n",
            valid.to_string() + "4 1 0:4 / /sys/fs/cgroup/custom-secret ro - ext4 /dev/vda ro\n",
            valid.lines().take(1).collect::<Vec<_>>().join("\n"),
            "malformed".to_owned(),
        ] {
            assert!(check_mounts(&invalid).is_err(), "{invalid}");
        }
    }
}
