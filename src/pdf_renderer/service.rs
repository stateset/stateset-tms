//! One-request Unix socket protocol; no filesystem paths, URLs or API credentials.
use super::{local, Failure, Rendered, ServiceConfig, MAX_PAGES, OUTPUT_LIMIT};
use crate::documents::MAX_DOCUMENT_BYTES;
use sha2::{Digest, Sha256};
use std::{
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};

#[path = "service/container.rs"]
mod container;
#[path = "../pdf_descriptors.rs"]
mod descriptors;

const SOCKET: &str = "/run/freight-pdf/renderer.sock";
const HEADER_BYTES: usize = 52;
const IO_LIMIT: Duration = Duration::from_millis(250);
const BODY_LIMIT: Duration = Duration::from_secs(2);
const REQUEST_LIMIT: Duration = Duration::from_secs(14);
const OK: u32 = 0;
const INVALID: u32 = 1;
const UNAVAILABLE: u32 = 2;
const BUSY: u32 = 3;
const READY: u32 = 4;

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
fn header(
    magic: &[u8; 4],
    status: u32,
    page: u32,
    pages: u32,
    size: u32,
    source: &[u8; 32],
) -> [u8; HEADER_BYTES] {
    let mut bytes = [0; HEADER_BYTES];
    bytes[..4].copy_from_slice(magic);
    for (offset, value) in [(4, status), (8, page), (12, pages), (16, size)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    bytes[20..].copy_from_slice(source);
    bytes
}
fn plain(magic: &[u8; 4], status: u32) -> [u8; HEADER_BYTES] {
    header(magic, status, 0, 0, 0, &[0; 32])
}
async fn read_header(stream: &mut UnixStream) -> io::Result<[u8; HEADER_BYTES]> {
    let mut bytes = [0; HEADER_BYTES];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}
async fn connect(config: &ServiceConfig) -> io::Result<UnixStream> {
    let stream = timeout(IO_LIMIT, UnixStream::connect(&config.path)).await??;
    if stream.peer_cred()?.uid() != config.uid {
        return Err(io::Error::other("Unexpected renderer peer"));
    }
    Ok(stream)
}

pub(super) async fn request(
    config: &ServiceConfig,
    bytes: &[u8],
    page: u32,
) -> Result<Rendered, Failure> {
    timeout(REQUEST_LIMIT, async {
        let mut stream = connect(config).await.map_err(|_| Failure::Unavailable)?;
        let mut request = [0; 12];
        request[..4].copy_from_slice(b"FRQ1");
        request[4..8].copy_from_slice(&page.to_be_bytes());
        request[8..].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        let admission = timeout(IO_LIMIT, async {
            stream.write_all(&request).await?;
            read_header(&mut stream).await
        })
        .await
        .map_err(|_| Failure::Unavailable)?
        .map_err(|_| Failure::Unavailable)?;
        // No body is sent before the authenticated service grants admission.
        if admission == plain(b"FRA1", BUSY) {
            return Err(Failure::Busy);
        }
        if admission != plain(b"FRA1", OK) {
            return Err(Failure::Unavailable);
        }
        stream
            .write_all(bytes)
            .await
            .map_err(|_| Failure::Unavailable)?;
        stream.shutdown().await.map_err(|_| Failure::Unavailable)?;
        let response = read_header(&mut stream)
            .await
            .map_err(|_| Failure::Unavailable)?;
        if &response[..4] != b"FRP1" {
            return Err(Failure::Unavailable);
        }
        let status = word(&response, 4);
        if status != OK {
            if response != plain(b"FRP1", status) {
                return Err(Failure::Unavailable);
            }
            return Err(if status == INVALID {
                Failure::Invalid
            } else {
                Failure::Unavailable
            });
        }
        let pages = word(&response, 12);
        let size = word(&response, 16) as usize;
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        if word(&response, 8) != page
            || pages < page
            || pages > MAX_PAGES
            || size == 0
            || size > OUTPUT_LIMIT
            || response[20..] != digest
        {
            return Err(Failure::Unavailable);
        }
        let mut png = vec![0; size];
        stream
            .read_exact(&mut png)
            .await
            .map_err(|_| Failure::Unavailable)?;
        let mut extra = [0];
        if stream
            .read(&mut extra)
            .await
            .map_err(|_| Failure::Unavailable)?
            != 0
        {
            return Err(Failure::Unavailable);
        }
        Ok(Rendered { png, pages })
    })
    .await
    .map_err(|_| Failure::Unavailable)?
}

async fn send(stream: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
    timeout(IO_LIMIT, stream.write_all(bytes)).await??;
    Ok(())
}

async fn handle(
    mut stream: UnixStream,
    page: u32,
    size: usize,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let body = timeout(BODY_LIMIT, async {
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await?;
        let mut extra = [0];
        if stream.read(&mut extra).await? != 0 {
            return Err(io::Error::other("Extra request bytes"));
        }
        Ok::<_, io::Error>(bytes)
    })
    .await;
    let bytes = match body {
        Ok(Ok(bytes)) => bytes,
        _ => {
            let _ = send(&mut stream, &plain(b"FRP1", INVALID)).await;
            return;
        }
    };
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    match local::render(&bytes, page, true).await {
        Ok(rendered) => {
            if send(
                &mut stream,
                &header(
                    b"FRP1",
                    OK,
                    page,
                    rendered.pages,
                    rendered.png.len() as u32,
                    &digest,
                ),
            )
            .await
            .is_ok()
            {
                let _ = timeout(Duration::from_secs(1), stream.write_all(&rendered.png)).await;
            }
        }
        Err(error) => {
            let status = if error == Failure::Invalid {
                INVALID
            } else {
                UNAVAILABLE
            };
            let _ = send(&mut stream, &plain(b"FRP1", status)).await;
        }
    }
    let _ = stream.shutdown().await;
    // The permit survives body handling, renderer termination/reaping and output.
}

struct SocketGuard {
    device: u64,
    inode: u64,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(SOCKET) {
            if metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
            {
                let _ = std::fs::remove_file(SOCKET);
            }
        }
    }
}

async fn bind() -> io::Result<(UnixListener, SocketGuard, std::fs::File)> {
    let directory = Path::new("/run/freight-pdf");
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 10002
        || metadata.gid() != 10001
        || metadata.mode() & 0o027 != 0
        || directory.canonicalize()? != directory
    {
        return Err(io::Error::other("Unsafe renderer socket directory"));
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join("renderer.lock"))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != 10002
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other("Unsafe renderer lock"));
    }
    lock.try_lock().map_err(io::Error::other)?;
    match std::fs::symlink_metadata(SOCKET) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() || metadata.uid() != 10002 || metadata.nlink() != 1
            {
                return Err(io::Error::other(
                    "Preserve unexpected renderer socket entry",
                ));
            }
            match timeout(IO_LIMIT, UnixStream::connect(SOCKET)).await {
                Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    let current = std::fs::symlink_metadata(SOCKET)?;
                    if current.dev() != metadata.dev() || current.ino() != metadata.ino() {
                        return Err(io::Error::other("Renderer socket changed"));
                    }
                    std::fs::remove_file(SOCKET)?;
                }
                _ => {
                    return Err(io::Error::other(
                        "Renderer socket is already active or unavailable",
                    ))
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(SOCKET)?;
    let metadata = std::fs::symlink_metadata(SOCKET)?;
    let guard = SocketGuard {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    std::fs::set_permissions(SOCKET, std::fs::Permissions::from_mode(0o660))?;
    Ok((listener, guard, lock))
}

async fn serve() -> io::Result<()> {
    let policy = local::probe_container_policy()
        .await
        .map_err(|_| io::Error::other("Renderer syscall policy unavailable"))?;
    println!("PDF renderer policy {policy}");
    let (listener, _guard, _lock) = bind().await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let permits = Arc::new(Semaphore::new(1));
    let mut active = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            _ = active.join_next(), if !active.is_empty() => {},
            accepted = listener.accept() => {
                let (mut stream, _) = accepted?;
                if !matches!(stream.peer_cred()?.uid(), 10001 | 10002) { continue; }
                let mut request = [0; 12];
                if !matches!(timeout(IO_LIMIT, stream.read_exact(&mut request)).await, Ok(Ok(_))) { continue; }
                if &request[..4] == b"FRH1" && request[4..] == [0; 8] {
                    let _ = send(&mut stream, &plain(b"FRP1", READY)).await;
                    continue;
                }
                let page = word(&request, 4);
                let size = word(&request, 8) as usize;
                if &request[..4] != b"FRQ1" || page == 0 || page > MAX_PAGES || size == 0 || size > MAX_DOCUMENT_BYTES {
                    let _ = send(&mut stream, &plain(b"FRA1", INVALID)).await;
                    continue;
                }
                let permit = match permits.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => { let _ = send(&mut stream, &plain(b"FRA1", BUSY)).await; continue; }
                };
                if send(&mut stream, &plain(b"FRA1", OK)).await.is_err() { continue; }
                active.spawn(handle(stream, page, size, permit));
            }
        }
    }
    drop(listener);
    while active.join_next().await.is_some() {}
    Ok(())
}

pub(super) fn entry() -> Result<(), Box<dyn std::error::Error>> {
    container::validate()?;
    descriptors::close_inherited()?;
    nix::sys::prctl::set_dumpable(false)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()?;
    let result = runtime.block_on(serve());
    runtime.shutdown_timeout(Duration::ZERO);
    result.map_err(Into::into)
}

pub(super) fn check() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime
        .block_on(async {
            timeout(Duration::from_secs(2), async {
                let mut stream = connect(&ServiceConfig {
                    path: SOCKET.into(),
                    uid: 10002,
                })
                .await?;
                stream.write_all(b"FRH1\0\0\0\0\0\0\0\0").await?;
                if read_header(&mut stream).await? != plain(b"FRP1", READY) {
                    return Err(io::Error::other("Renderer unavailable"));
                }
                Ok::<_, io::Error>(())
            })
            .await?
        })
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Directory(std::path::PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "freight-renderer-wire-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("owned test directory");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("private directory");
            Self(path)
        }
        fn listen(&self) -> (UnixListener, ServiceConfig) {
            let path = self.0.join("s");
            (
                UnixListener::bind(&path).expect("owned listener"),
                ServiceConfig {
                    path,
                    uid: nix::unistd::getuid().as_raw(),
                },
            )
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn authenticated_transport_checks_page_source_size_and_exact_response_end() {
        for mutation in 0..8 {
            let directory = Directory::new();
            let (listener, config) = directory.listen();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("client");
                let mut request = [0; 12];
                socket.read_exact(&mut request).await.expect("header");
                assert_eq!(&request[..4], b"FRQ1");
                assert_eq!(word(&request, 4), 2);
                socket.write_all(&plain(b"FRA1", OK)).await.expect("grant");
                let mut body = Vec::new();
                socket.read_to_end(&mut body).await.expect("request EOF");
                assert_eq!(body, b"owned-pdf");
                let digest: [u8; 32] = Sha256::digest(&body).into();
                let mut response = header(b"FRP1", OK, 2, 2, 4, &digest);
                match mutation {
                    1 => response[0] = b'X',
                    2 => response[11] = 1,
                    3 => response[12..16].copy_from_slice(&1001u32.to_be_bytes()),
                    4 => {
                        response[16..20].copy_from_slice(&((OUTPUT_LIMIT + 1) as u32).to_be_bytes())
                    }
                    5 => response[20] ^= 1,
                    _ => {}
                }
                let _ = socket.write_all(&response).await;
                let _ = socket
                    .write_all(if mutation == 6 {
                        b"png"
                    } else if mutation == 7 {
                        b"png!extra"
                    } else {
                        b"png!"
                    })
                    .await;
            });
            let rendered = request(&config, b"owned-pdf", 2).await;
            if mutation == 0 {
                assert_eq!(rendered.expect("exact valid frame").png, b"png!");
            } else {
                assert_eq!(
                    rendered.expect_err("malformed response"),
                    Failure::Unavailable
                );
            }
            server.await.expect("fake service");
        }
    }

    #[tokio::test]
    async fn wrong_peer_busy_and_slow_admission_never_receive_pdf_bytes() {
        for case in 0..3 {
            let directory = Directory::new();
            let (listener, mut config) = directory.listen();
            if case == 0 {
                config.uid += 1;
            }
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.expect("client");
                if case != 0 {
                    let mut request = [0; 12];
                    socket
                        .read_exact(&mut request)
                        .await
                        .expect("request header only");
                    if case == 1 {
                        socket.write_all(&plain(b"FRA1", BUSY)).await.expect("busy");
                    } else {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                }
                let mut body = Vec::new();
                socket.read_to_end(&mut body).await.expect("client closes");
                assert!(body.is_empty(), "No PDF bytes before trusted admission");
            });
            assert_eq!(
                request(&config, b"owned-pdf", 1)
                    .await
                    .expect_err("refused"),
                if case == 1 {
                    Failure::Busy
                } else {
                    Failure::Unavailable
                }
            );
            timeout(Duration::from_secs(2), server)
                .await
                .expect("bounded fake service")
                .expect("join");
        }
    }
}
