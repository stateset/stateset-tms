//! Reuse a fully validated canonical source only for byte-identical snapshots.
//! Every read still fetches the complete repeatable-read snapshot. This cache
//! stores one compressed entry, no Workspace, derived response or write frame.
//! The resident compressed payload is at most 4 MiB, with one additional bounded
//! fill; canonical input/output is at most 64 MiB. Small reads stay uncached.
use super::Snapshot;
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

const MIN_BYTES: usize = 8 * 1024 * 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_COMPRESSED: usize = 4 * 1024 * 1024;

pub(super) fn key(snapshot: &Snapshot, tenant: &str) -> Option<[u8; 32]> {
    if snapshot.rows.len() < 64 {
        return None;
    }
    let bytes = snapshot
        .rows
        .iter()
        .try_fold(snapshot.header.len(), |n, row| {
            n.checked_add(row.0.len())?
                .checked_add(row.1.len())?
                .checked_add(row.2.len())
        })?;
    if !(MIN_BYTES..=MAX_BYTES).contains(&bytes) {
        return None;
    }
    Some(fingerprint(snapshot, tenant))
}

fn fingerprint(snapshot: &Snapshot, tenant: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    // Length prefixes preserve literal keys, boundaries, row order and count.
    // Process-local state cannot survive a model/codec/format deployment.
    for part in ["freight-validated-read-v1", tenant, &snapshot.header] {
        member(&mut digest, part);
    }
    digest.update((snapshot.rows.len() as u64).to_be_bytes());
    for (root, key, payload) in &snapshot.rows {
        for part in [root, key, payload] {
            member(&mut digest, part);
        }
    }
    digest.finalize().into()
}
fn member(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

struct Entry {
    key: [u8; 32],
    compressed: Vec<u8>,
    canonical_bytes: usize,
    canonical_digest: [u8; 32],
}

#[derive(Default)]
pub(crate) struct ReadCache {
    entry: Mutex<Option<Entry>>,
    filling: AtomicBool,
    #[cfg(test)]
    hits: std::sync::atomic::AtomicUsize,
}

struct Fill<'a>(&'a AtomicBool);
impl Drop for Fill<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl ReadCache {
    pub(super) fn get(&self, key: [u8; 32]) -> Option<String> {
        // Never queue cache users. Holding this try-lock through inflation
        // prevents old compressed entries accumulating in detached Arc clones.
        let mut slot = self.entry.try_lock().ok()?;
        let entry = slot.as_ref().filter(|entry| entry.key == key)?;
        let source = entry.inflate();
        if source.is_none() {
            // Derived-cache damage always returns to complete validation.
            slot.take();
        }
        #[cfg(test)]
        if source.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        }
        source
    }
    #[cfg(test)]
    pub(crate) fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }
    pub(super) fn insert(&self, key: [u8; 32], source: &str) {
        if !(MIN_BYTES..=MAX_BYTES).contains(&source.len())
            || self
                .filling
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        let _fill = Fill(&self.filling);
        let mut encoder = ZlibEncoder::new(BoundedBuffer::default(), Compression::fast());
        if encoder.write_all(source.as_bytes()).is_err() {
            return;
        }
        let Ok(compressed) = encoder.finish() else {
            return;
        };
        let entry = Entry {
            key,
            compressed: compressed.0,
            canonical_bytes: source.len(),
            canonical_digest: Sha256::digest(source.as_bytes()).into(),
        };
        if let Ok(mut slot) = self.entry.try_lock() {
            *slot = Some(entry);
        }
    }
}
impl Entry {
    fn inflate(&self) -> Option<String> {
        if !(MIN_BYTES..=MAX_BYTES).contains(&self.canonical_bytes)
            || self.compressed.len() > MAX_COMPRESSED
        {
            return None;
        }
        let mut source = vec![0; self.canonical_bytes];
        let mut decoder = ZlibDecoder::new(self.compressed.as_slice());
        decoder.read_exact(&mut source).ok()?;
        let mut extra = [0];
        if decoder.read(&mut extra).ok()? != 0
            || <[u8; 32]>::from(Sha256::digest(&source)) != self.canonical_digest
        {
            return None;
        }
        String::from_utf8(source).ok()
    }
}
#[derive(Default)]
struct BoundedBuffer(Vec<u8>);
impl Write for BoundedBuffer {
    fn write(&mut self, value: &[u8]) -> io::Result<usize> {
        if value.len() > MAX_COMPRESSED - self.0.len() {
            return Err(io::Error::other(
                "Compressed canonical source exceeds cache budget",
            ));
        }
        // Avoid geometric Vec growth exceeding the declared compressed bound.
        self.0
            .try_reserve_exact(value.len())
            .map_err(io::Error::other)?;
        self.0.extend_from_slice(value);
        Ok(value.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "canonical_read_cache_tests.rs"]
mod tests;
