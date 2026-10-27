//! Default-deny policy inherited by the fixed PDF executable across exec.
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule,
};
use std::{collections::BTreeMap, io};

pub(super) fn install() -> io::Result<()> {
    // A default-deny list also rejects alternate syscall numbers (including x32)
    // rather than relying on an ever-growing list of forbidden socket operations.
    let mut calls = vec![
        libc::SYS_read,
        libc::SYS_write,
        libc::SYS_readv,
        libc::SYS_writev,
        libc::SYS_pread64,
        libc::SYS_preadv,
        libc::SYS_close,
        libc::SYS_close_range,
        libc::SYS_dup,
        libc::SYS_dup3,
        libc::SYS_fcntl,
        libc::SYS_ioctl,
        libc::SYS_newfstatat,
        libc::SYS_fstat,
        libc::SYS_statx,
        libc::SYS_lseek,
        libc::SYS_readlinkat,
        libc::SYS_getdents64,
        libc::SYS_faccessat,
        libc::SYS_faccessat2,
        libc::SYS_getcwd,
        libc::SYS_mmap,
        libc::SYS_mprotect,
        libc::SYS_munmap,
        libc::SYS_mremap,
        libc::SYS_madvise,
        libc::SYS_brk,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_clock_gettime,
        libc::SYS_clock_getres,
        libc::SYS_clock_nanosleep,
        libc::SYS_nanosleep,
        libc::SYS_gettimeofday,
        libc::SYS_futex,
        libc::SYS_set_tid_address,
        libc::SYS_set_robust_list,
        libc::SYS_rseq,
        libc::SYS_getpid,
        libc::SYS_getppid,
        libc::SYS_gettid,
        libc::SYS_getuid,
        libc::SYS_geteuid,
        libc::SYS_getgid,
        libc::SYS_getegid,
        libc::SYS_getgroups,
        libc::SYS_sched_getaffinity,
        libc::SYS_sched_yield,
        libc::SYS_uname,
        libc::SYS_sysinfo,
        libc::SYS_getrandom,
        libc::SYS_prlimit64,
        libc::SYS_getrlimit,
        libc::SYS_getrusage,
        libc::SYS_execve,
        libc::SYS_exit,
        libc::SYS_exit_group,
        libc::SYS_statfs,
        libc::SYS_fstatfs,
        libc::SYS_ppoll,
        libc::SYS_pselect6,
        libc::SYS_pipe2,
    ];
    #[cfg(target_arch = "x86_64")]
    calls.extend([
        libc::SYS_arch_prctl,
        libc::SYS_stat,
        libc::SYS_lstat,
        libc::SYS_access,
        libc::SYS_readlink,
        libc::SYS_dup2,
        libc::SYS_poll,
        libc::SYS_select,
        libc::SYS_pipe,
    ]);
    let mut rules: BTreeMap<_, _> = calls.into_iter().map(|call| (call, vec![])).collect();
    // RLIMIT_FSIZE=0 does not prevent O_TRUNC from destroying existing contents.
    // Permit only known read-only flags, including directory/font/library reads.
    // openat2 passes flags by pointer, so it stays default-denied rather than
    // pretending that BPF can safely inspect the pointed-to open_how structure.
    let open_rule = |argument| -> io::Result<Vec<SeccompRule>> {
        let flags = libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | libc::O_NOCTTY
            | libc::O_DIRECTORY
            | libc::O_NOFOLLOW
            | libc::O_LARGEFILE;
        // Linux uses the low unsigned-int word of open/openat's flags argument.
        let condition = SeccompCondition::new(
            argument,
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::MaskedEq(u64::from(!(flags as u32))),
            0,
        )
        .map_err(io::Error::other)?;
        Ok(vec![
            SeccompRule::new(vec![condition]).map_err(io::Error::other)?
        ])
    };
    rules.insert(libc::SYS_openat, open_rule(2)?);
    #[cfg(target_arch = "x86_64")]
    rules.insert(libc::SYS_open, open_rule(1)?);
    let architecture = std::env::consts::ARCH
        .try_into()
        .map_err(io::Error::other)?;
    let program: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        architecture,
    )
    .map_err(io::Error::other)?
    .try_into()
    .map_err(io::Error::other)?;
    // apply_filter sets no-new-privileges and installs on this one worker thread.
    // This binary starts no runtime or additional threads before exec.
    seccompiler::apply_filter(&program).map_err(io::Error::other)
}

pub(super) fn baseline() -> io::Result<usize> {
    // Demonstrate that this process can create sockets before imposing its policy.
    drop(std::net::TcpListener::bind("0.0.0.0:0")?);
    drop(std::net::UdpSocket::bind("0.0.0.0:0")?);
    drop(std::os::unix::net::UnixStream::pair()?);
    for path in ["/etc/passwd", "/proc/self/environ", "/proc/self/cmdline"] {
        drop(std::fs::File::open(path)?);
    }
    filters()
}

fn filters() -> io::Result<usize> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let count = status
        .lines()
        .find_map(|line| line.strip_prefix("Seccomp_filters:"))
        .ok_or_else(|| io::Error::other("Missing kernel filter count"))?
        .trim()
        .parse()
        .map_err(io::Error::other)?;
    Ok(count)
}

fn denied<T>(result: io::Result<T>) -> io::Result<()> {
    match result {
        Err(error) if error.raw_os_error() == Some(libc::EPERM) => Ok(()),
        _ => Err(io::Error::other(
            "Policy did not deny the operation with EPERM",
        )),
    }
}

pub(super) fn probe(before: usize, filesystem: bool) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    denied(std::net::TcpListener::bind("0.0.0.0:0"))?;
    denied(std::net::TcpListener::bind("[::]:0"))?;
    denied(std::net::UdpSocket::bind("0.0.0.0:0"))?;
    denied(std::net::UdpSocket::bind("[::]:0"))?;
    denied(std::os::unix::net::UnixStream::pair())?;
    denied(std::os::unix::net::UnixDatagram::unbound())?;
    denied(std::process::Command::new("/usr/bin/true").spawn())?;
    for (write, append, truncate) in [
        (true, false, false),
        (true, false, true),
        (false, true, false),
    ] {
        denied(
            std::fs::OpenOptions::new()
                .write(write)
                .append(append)
                .truncate(truncate)
                .open("/dev/null"),
        )?;
    }
    // custom_flags reaches the kernel even for O_RDONLY|O_TRUNC, unlike
    // OpenOptions::truncate(true), which Rust rejects before making a syscall.
    // /dev/null makes these checks harmless if the filter is accidentally broken.
    for flag in [
        libc::O_TRUNC,
        libc::O_CREAT,
        libc::O_APPEND,
        libc::O_PATH,
        libc::O_TMPFILE,
        i32::MIN,
    ] {
        denied(
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flag)
                .open("/dev/null"),
        )?;
    }
    let status = std::fs::read_to_string("/proc/self/status")?;
    if !status.contains("NoNewPrivs:\t1\n")
        || !status.contains("Seccomp:\t2\n")
        || !status.contains("Threads:\t1\n")
        || filters()? != before + 1
    {
        return Err(io::Error::other("Kernel policy state differs"));
    }
    if filesystem {
        for path in ["/etc/passwd", "/proc/self/environ", "/proc/self/cmdline"] {
            match std::fs::read(path) {
                Err(error) if error.raw_os_error() == Some(libc::EACCES) => {}
                _ => return Err(io::Error::other("Filesystem read boundary failed")),
            }
        }
    }
    // This standard symlink resolves inside the trusted /usr/lib hierarchy.
    if std::fs::read("/etc/os-release")?.is_empty() {
        return Err(io::Error::other("Runtime filesystem control missing"));
    }
    println!("{{\"result\":\"pass\",\"socket_denials\":6,\"spawn_denied\":true,\"no_new_privileges\":true,\"threads\":1,\"filter_count_before\":{before},\"filter_count_after\":{},\"readonly_file_opens\":true,\"write_open_denials\":9,\"filesystem_isolation\":{filesystem}}}", before + 1);
    Ok(())
}
