//! Run only in the single-threaded worker/service entry, before opening resources.
use std::io;

pub(super) fn close_inherited() -> io::Result<()> {
    let mut descriptors = Vec::new();
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let entry = entry?;
        let fd: i32 = entry
            .file_name()
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid descriptor name"))?
            .parse()
            .map_err(io::Error::other)?;
        if fd > 2 {
            descriptors.push(fd);
        }
        if descriptors.len() > 4096 {
            return Err(io::Error::other("Too many inherited descriptors"));
        }
    }
    // The directory iterator has already closed its own FD. No thread, runtime,
    // or owned File remains here; also close inherited FDs above the new rlimit.
    for fd in descriptors {
        match nix::unistd::close(fd) {
            Ok(()) | Err(nix::errno::Errno::EBADF) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
