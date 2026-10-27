//! Lossless JSON blocks for the canonical-storage investigation.
//!
//! This codec is independent of the application format/migrations. Bounds apply
//! to canonical grouped JSON, not HTTP text, PostgreSQL allocation or an entire
//! tenant. A single original record/scalar can exceed the grouping target.
use serde::de::{MapAccess, Visitor};
use serde::ser::{Error, SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::borrow::{Borrow, Cow};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};

pub const VERSION: u32 = 1;
type Result<T> = std::result::Result<T, String>;

// Large-frame validation may borrow one additional CPU. A single process-wide
// slot prevents concurrent frames from multiplying helper threads. Contention
// and thread-creation failure use the same serial validation, without queuing.
struct HelperSlot(AtomicBool);
impl HelperSlot {
    const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    fn acquire(&self) -> Option<HelperPermit<'_>> {
        self.0
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| HelperPermit(self))
    }
}
struct HelperPermit<'a>(&'a HelperSlot);
impl Drop for HelperPermit<'_> {
    fn drop(&mut self) {
        self.0 .0.store(false, Ordering::Release);
    }
}
static VALIDATION_HELPER: HelperSlot = HelperSlot::new();

enum Validation {
    Auto,
    #[cfg(test)]
    Serial,
    #[cfg(test)]
    Parallel,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub span: u64,
    pub target_bytes: usize,
}
impl Config {
    pub fn validate(self) -> Result<()> {
        if !(1..=1024).contains(&self.span)
            || !self.span.is_power_of_two()
            || !(1024..=1_048_576).contains(&self.target_bytes)
        {
            return Err("Unsupported block configuration".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Array,
    Object,
    String,
    Number,
    Boolean,
    Null,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub kind: Kind,
    pub size: u64,
    pub scalar: Box<RawValue>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub position: u64,
    pub name: String,
    pub payload: Box<RawValue>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Block<T = Entry> {
    pub root: String,
    pub key: String,
    pub items: Vec<T>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Packed {
    pub version: u32,
    pub config: Config,
    pub root_names: Vec<String>,
    #[serde(deserialize_with = "unique_roots")]
    pub roots: BTreeMap<String, Root>,
    pub blocks: Vec<Block>,
}

/// Originals that have passed complete frame validation and normalization.
/// Private construction prevents a caller from marking arbitrary text canonical.
pub struct CanonicalFrame {
    frame: Packed,
}
impl CanonicalFrame {
    pub fn blocks(&self) -> &[Block] {
        &self.frame.blocks
    }
}

#[derive(Default)]
struct OriginalRoot {
    scalar: Option<Box<RawValue>>,
    positions: BTreeMap<u64, Box<RawValue>>,
    names: BTreeMap<String, Box<RawValue>>,
}
type Originals = BTreeMap<String, OriginalRoot>;
fn originals(frame: CanonicalFrame) -> Result<Originals> {
    let mut roots: Originals = frame
        .frame
        .roots
        .into_iter()
        .map(|(name, root)| {
            (
                name,
                OriginalRoot {
                    scalar: Some(root.scalar),
                    ..OriginalRoot::default()
                },
            )
        })
        .collect();
    for block in frame.frame.blocks {
        let root = roots
            .get_mut(&block.root)
            .ok_or("Validated block root missing")?;
        for item in block.items {
            if item.position == 0 {
                root.names.insert(item.name, item.payload);
            } else {
                root.positions.insert(item.position, item.payload);
            }
        }
    }
    Ok(roots)
}
fn reuse_or_normalize(
    previous: Option<Box<RawValue>>,
    current: &RawValue,
    depth: usize,
) -> Result<Box<RawValue>> {
    if let Some(previous) = previous {
        if previous.get() == current.get() {
            return Ok(previous);
        }
    }
    canonical_raw_at(current, depth)
}

struct Map<T = Box<RawValue>, K = String>(BTreeMap<K, T>);
fn unique_roots<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, Root>, D::Error> {
    let Map(values): Map = Map::deserialize(d)?;
    values
        .into_iter()
        .map(|(k, v)| {
            serde_json::from_str(v.get())
                .map(|root| (k, root))
                .map_err(serde::de::Error::custom)
        })
        .collect()
}
impl<'de, T: Deserialize<'de>, K: Deserialize<'de> + Ord> Deserialize<'de> for Map<T, K> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Unique<T, K>(PhantomData<(T, K)>);
        impl<'de, T: Deserialize<'de>, K: Deserialize<'de> + Ord> Visitor<'de> for Unique<T, K> {
            type Value = Map<T, K>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Map<T, K>, A::Error> {
                let mut values = BTreeMap::new();
                while let Some((k, v)) = a.next_entry::<K, T>()? {
                    if values.insert(k, v).is_some() {
                        return Err(serde::de::Error::custom("Duplicate JSON key"));
                    }
                }
                Ok(Map(values))
            }
        }
        d.deserialize_map(Unique::<T, K>(PhantomData))
    }
}
// Object normalization only needs sorted decoded keys while the input is
// alive. Borrow ordinary keys; escaped keys keep an owned decoded spelling.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct ObjectKey<'a>(Cow<'a, str>);
impl<'de> Deserialize<'de> for ObjectKey<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Key;
        impl<'de> Visitor<'de> for Key {
            type Value = ObjectKey<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object key")
            }
            fn visit_borrowed_str<E: serde::de::Error>(
                self,
                key: &'de str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(ObjectKey(Cow::Borrowed(key)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                key: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(ObjectKey(Cow::Owned(key.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                key: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(ObjectKey(Cow::Owned(key)))
            }
        }
        d.deserialize_str(Key)
    }
}
fn err(e: impl fmt::Display) -> String {
    e.to_string()
}
fn raw(s: String) -> Result<Box<RawValue>> {
    RawValue::from_string(s).map_err(err)
}
fn json<T: Serialize + ?Sized>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(err)
}
fn kind(s: &str) -> Result<Kind> {
    match s.trim_start().as_bytes().first() {
        Some(b'{') => Ok(Kind::Object),
        Some(b'[') => Ok(Kind::Array),
        Some(b'"') => Ok(Kind::String),
        Some(b't' | b'f') => Ok(Kind::Boolean),
        Some(b'n') => Ok(Kind::Null),
        Some(b'-' | b'0'..=b'9') => Ok(Kind::Number),
        _ => Err("Invalid JSON kind".into()),
    }
}

/// Expand a JSON decimal without binary floating point, retaining its scale.
/// The expansion guard prevents hostile lexical exponents allocating unbounded
/// memory. Native PostgreSQL JSONB snapshots already contain expanded numbers.
fn number(s: &str) -> Result<Cow<'_, str>> {
    // RawValue already checked JSON number syntax. PostgreSQL's expanded
    // decimals need no allocation unless their sign denotes negative zero.
    // Preserve the expansion budget on this path as well as exponent inputs.
    if !s.contains(['e', 'E']) {
        let unsigned = s.strip_prefix('-').unwrap_or(s);
        if unsigned.len() - usize::from(unsigned.contains('.')) > 1_000_000 {
            return Err("Numeric expansion exceeds codec budget".into());
        }
        return Ok(Cow::Borrowed(
            if s.starts_with('-') && !unsigned.bytes().any(|v| matches!(v, b'1'..=b'9')) {
                unsigned
            } else {
                s
            },
        ));
    }
    let (negative, s) = s.strip_prefix('-').map_or((false, s), |v| (true, v));
    let (mantissa, exponent) = s.split_once(['e', 'E']).unwrap_or((s, "0"));
    let exponent = exponent
        .parse::<i64>()
        .map_err(|_| "Numeric exponent exceeds codec budget")?;
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{integer}{fraction}");
    let point = (integer.len() as i64)
        .checked_add(exponent)
        .ok_or("Numeric expansion overflow")?;
    let fractional = (digits.len() as i64)
        .checked_sub(point)
        .ok_or("Numeric expansion overflow")?
        .max(0);
    let integer_count = point.max(1);
    if fractional
        .checked_add(integer_count)
        .ok_or("Numeric expansion overflow")?
        > 1_000_000
    {
        return Err("Numeric expansion exceeds codec budget".into());
    }
    let (whole, tail) = if point <= 0 {
        (
            "0".to_owned(),
            format!("{}{}", "0".repeat((-point) as usize), digits),
        )
    } else if point as usize >= digits.len() {
        (
            format!("{}{}", digits, "0".repeat(point as usize - digits.len())),
            String::new(),
        )
    } else {
        (
            digits[..point as usize].to_owned(),
            digits[point as usize..].to_owned(),
        )
    };
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let nonzero = digits.bytes().any(|v| v != b'0');
    Ok(Cow::Owned(format!(
        "{}{}{}",
        if negative && nonzero { "-" } else { "" },
        whole,
        if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        }
    )))
}

/// Stable JSON whitespace/key order, with exact decimals and opaque objects.
/// No serde_json::Value conversion: private-looking keys and large numbers are
/// ordinary retained data, not a number-token escape or a rounded f64.
pub fn canonical(s: &str) -> Result<String> {
    let validated: &RawValue = serde_json::from_str(s).map_err(err)?;
    canonical_at(validated, 0)
}
fn canonical_at(value: &RawValue, depth: usize) -> Result<String> {
    json(&Canonical { value, depth })
}
fn canonical_raw_at(value: &RawValue, depth: usize) -> Result<Box<RawValue>> {
    serde_json::value::to_raw_value(&Canonical { value, depth }).map_err(err)
}
// Serialize the complete normalized value through the public JSON serializer.
// Raw output then needs no second parse of the whole newly written record.
// Every member still passes the same recursive normalization and budgets.
struct Canonical<'a> {
    value: &'a RawValue,
    depth: usize,
}
impl Serialize for Canonical<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if self.depth > 128 {
            return Err(S::Error::custom("JSON nesting exceeds codec budget"));
        }
        let source = self.value.get();
        match kind(source).map_err(S::Error::custom)? {
            Kind::Object => {
                let Map(values): Map<&RawValue, ObjectKey<'_>> =
                    serde_json::from_str(source).map_err(S::Error::custom)?;
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(
                        &key.0,
                        &Canonical {
                            value,
                            depth: self.depth + 1,
                        },
                    )?;
                }
                map.end()
            }
            Kind::Array => {
                let values: Vec<&RawValue> =
                    serde_json::from_str(source).map_err(S::Error::custom)?;
                let mut array = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    array.serialize_element(&Canonical {
                        value,
                        depth: self.depth + 1,
                    })?;
                }
                array.end()
            }
            Kind::String if source.as_bytes().contains(&b'\\') => {
                let value: String = serde_json::from_str(source).map_err(S::Error::custom)?;
                value.serialize(serializer)
            }
            Kind::Number => {
                let normalized = number(source).map_err(S::Error::custom)?;
                if normalized == source {
                    self.value.serialize(serializer)
                } else {
                    raw(normalized.into_owned())
                        .map_err(S::Error::custom)?
                        .serialize(serializer)
                }
            }
            Kind::String | Kind::Boolean | Kind::Null => self.value.serialize(serializer),
        }
    }
}

fn string_len(value: &str) -> Result<usize> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("Block size overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_err(err)?;
    Ok(count.0)
}
fn body_len<T: Borrow<Entry>>(root: &str, key: &str, items: &[T]) -> Result<usize> {
    // RawValue serializes its exact bytes. Count that payload directly rather
    // than repeatedly copying entire blocks into temporary sizing buffers.
    let mut len = b"{\"root\":,\"key\":,\"items\":[]}".len();
    for size in [string_len(root)?, string_len(key)?] {
        len = len.checked_add(size).ok_or("Block size overflow")?;
    }
    for (index, item) in items.iter().enumerate() {
        let item = item.borrow();
        for size in [
            usize::from(index != 0),
            b"{\"position\":,\"name\":,\"payload\":}".len(),
            item.position.checked_ilog10().map_or(1, |n| n as usize + 1),
            string_len(&item.name)?,
            item.payload.get().len(),
        ] {
            len = len.checked_add(size).ok_or("Block size overflow")?;
        }
    }
    Ok(len)
}
fn fits<T: Borrow<Entry>>(root: &str, key: &str, items: &[T], cfg: Config) -> Result<bool> {
    Ok(items.len() <= cfg.span as usize && body_len(root, key, items)? <= cfg.target_bytes)
}
fn array_blocks<T: Borrow<Entry> + Serialize>(
    root: &str,
    base: u64,
    width: u64,
    path: String,
    items: Vec<T>,
    cfg: Config,
    out: &mut Vec<Block<T>>,
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let key = format!("a/{:016x}/{path}", base / cfg.span);
    if items.len() == 1 || fits(root, &key, &items, cfg)? {
        out.push(Block {
            root: root.into(),
            key,
            items,
        });
        return Ok(());
    }
    if width <= 1 {
        return Err("Invalid array partition".into());
    }
    let half = width / 2;
    let middle = base + half;
    let (left, right) = items
        .into_iter()
        .partition(|v: &T| v.borrow().position - 1 < middle);
    array_blocks(root, base, half, format!("{path}0"), left, cfg, out)?;
    // Keep the stable base group in the label while splitting its range.
    array_blocks(root, middle, half, format!("{path}1"), right, cfg, out)
}
fn hash(name: &str) -> [u8; 32] {
    Sha256::digest(name.as_bytes()).into()
}
fn object_blocks<T: Borrow<Entry> + Serialize>(
    root: &str,
    address: (u8, usize, String),
    mut items: Vec<T>,
    cfg: Config,
    out: &mut Vec<Block<T>>,
) -> Result<()> {
    let (bucket, depth, path) = address;
    if items.is_empty() {
        return Ok(());
    }
    let key = format!("o/{bucket:02x}/{path}");
    if items.len() == 1 || fits(root, &key, &items, cfg)? {
        out.push(Block {
            root: root.into(),
            key,
            items,
        });
        return Ok(());
    }
    if depth < 256 {
        let (left, right) = items
            .into_iter()
            .partition(|v: &T| hash(&v.borrow().name)[depth / 8] & (1 << (7 - depth % 8)) == 0);
        object_blocks(
            root,
            (bucket, depth + 1, format!("{path}0")),
            left,
            cfg,
            out,
        )?;
        object_blocks(
            root,
            (bucket, depth + 1, format!("{path}1")),
            right,
            cfg,
            out,
        )
    } else {
        // Full-hash collisions retain every literal key. Splitting this rare
        // collision group cannot overwrite another member by digest identity.
        items.sort_by(|a, b| a.borrow().name.cmp(&b.borrow().name));
        let right = items.split_off(items.len() / 2);
        object_blocks(root, (bucket, depth, format!("{path}c0")), items, cfg, out)?;
        object_blocks(root, (bucket, depth, format!("{path}c1")), right, cfg, out)
    }
}

// The encoder owns entries; the decoder borrows its already canonical entries.
// Sharing partition construction keeps address/count/byte/collision validation
// identical without copying payloads or repacking a materialized workspace.
fn partition<T: Borrow<Entry> + Serialize>(
    root: &str,
    shape: Kind,
    values: impl IntoIterator<Item = T>,
    config: Config,
) -> Result<Vec<Block<T>>> {
    let mut blocks = vec![];
    match shape {
        Kind::Array => {
            let mut groups: BTreeMap<u64, Vec<T>> = BTreeMap::new();
            for item in values {
                groups
                    .entry((item.borrow().position - 1) / config.span)
                    .or_default()
                    .push(item);
            }
            for (group, items) in groups {
                array_blocks(
                    root,
                    group * config.span,
                    config.span,
                    String::new(),
                    items,
                    config,
                    &mut blocks,
                )?;
            }
        }
        Kind::Object => {
            let mut groups: BTreeMap<u8, Vec<T>> = BTreeMap::new();
            for item in values {
                groups
                    .entry(hash(&item.borrow().name)[0])
                    .or_default()
                    .push(item);
            }
            for (bucket, items) in groups {
                object_blocks(root, (bucket, 8, String::new()), items, config, &mut blocks)?;
            }
        }
        _ => return Err("Noncollection partition".into()),
    }
    Ok(blocks)
}

pub fn pack(source: &str, config: Config) -> Result<Packed> {
    pack_inner(source, config, None)
}

/// Reuse only byte-identical originals from a fully validated prior frame.
/// Changed values still take the ordinary complete normalization path. Record
/// identities, partitioning and the resulting version-1 frame remain identical.
pub fn pack_reusing(source: &str, config: Config, prior: CanonicalFrame) -> Result<Packed> {
    pack_inner(source, config, Some(prior))
}
fn pack_inner(source: &str, config: Config, prior: Option<CanonicalFrame>) -> Result<Packed> {
    config.validate()?;
    let mut originals = prior.map(originals).transpose()?.unwrap_or_default();
    // Borrow validated source members and normalize each original once. A
    // complete intermediate canonical workspace adds another parse/copy of
    // every original before the same records are split into their blocks.
    let Map(values): Map<&RawValue> = serde_json::from_str(source).map_err(err)?;
    let mut roots = BTreeMap::new();
    let mut blocks = vec![];
    for (root, value) in values {
        let shape = kind(value.get())?;
        let mut size = 0;
        match shape {
            Kind::Array => {
                let values: Vec<&RawValue> = serde_json::from_str(value.get()).map_err(err)?;
                size = values.len() as u64;
                let entries = values
                    .into_iter()
                    .enumerate()
                    .map(|(n, payload)| {
                        Ok(Entry {
                            position: n as u64 + 1,
                            name: String::new(),
                            payload: reuse_or_normalize(
                                originals
                                    .get_mut(&root)
                                    .and_then(|v| v.positions.remove(&(n as u64 + 1))),
                                payload,
                                2,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                blocks.extend(partition(&root, shape, entries, config)?);
            }
            Kind::Object => {
                let Map(values): Map<&RawValue> = serde_json::from_str(value.get()).map_err(err)?;
                size = values.len() as u64;
                let entries = values
                    .into_iter()
                    .map(|(name, payload)| {
                        let previous = originals.get_mut(&root).and_then(|v| v.names.remove(&name));
                        Ok(Entry {
                            position: 0,
                            name,
                            payload: reuse_or_normalize(previous, payload, 2)?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                blocks.extend(partition(&root, shape, entries, config)?);
            }
            _ => {}
        }
        let scalar = if matches!(shape, Kind::Array | Kind::Object) {
            raw("null".into())?
        } else {
            reuse_or_normalize(
                originals.get_mut(&root).and_then(|v| v.scalar.take()),
                value,
                1,
            )?
        };
        roots.insert(
            root,
            Root {
                kind: shape,
                size,
                scalar,
            },
        );
    }
    blocks.sort_by(|a, b| (&a.root, &a.key).cmp(&(&b.root, &b.key)));
    Ok(Packed {
        version: VERSION,
        config,
        root_names: roots.keys().cloned().collect(),
        roots,
        blocks,
    })
}

/// Validate the complete frame before returning any materialized workspace.
fn helper_split(blocks: &[Block], validation: Validation) -> Option<usize> {
    let automatic = match validation {
        Validation::Auto => true,
        #[cfg(test)]
        Validation::Serial => return None,
        #[cfg(test)]
        Validation::Parallel => false,
    };
    if blocks.len() < 2
        || automatic
            && (blocks.len() < 64
                || std::thread::available_parallelism().map_or(true, |n| n.get() < 2))
    {
        return None;
    }
    let bytes = blocks
        .iter()
        .flat_map(|block| &block.items)
        .try_fold(0usize, |n, item| n.checked_add(item.payload.get().len()))?;
    if automatic && bytes < 8 * 1024 * 1024 {
        return None;
    }
    let mut prefix = 0;
    Some(
        blocks
            .iter()
            .position(|block| {
                for item in &block.items {
                    prefix += item.payload.get().len();
                }
                prefix >= bytes / 2
            })
            .map_or(blocks.len() / 2, |n| n + 1)
            .clamp(1, blocks.len() - 1),
    )
}

fn normalize_block_range(
    blocks: &mut [Block],
    roots: &BTreeMap<String, Root>,
    config: Config,
    offset: usize,
    duplicate: Option<usize>,
) -> Result<()> {
    for (index, block) in blocks.iter_mut().enumerate() {
        if duplicate == Some(offset + index) {
            return Err("Duplicate block".into());
        }
        let root = roots.get(&block.root).ok_or("Block without root")?;
        if block.items.is_empty() || block.items.len() > config.span as usize {
            return Err("Invalid block item count".into());
        }
        for item in &mut block.items {
            if (root.kind == Kind::Array && (item.position == 0 || !item.name.is_empty()))
                || (root.kind == Kind::Object && item.position != 0)
                || !matches!(root.kind, Kind::Array | Kind::Object)
            {
                return Err("Invalid record identity".into());
            }
            item.payload = canonical_raw_at(&item.payload, 2)?;
        }
        if block.items.len() > 1
            && body_len(&block.root, &block.key, &block.items)? > config.target_bytes
        {
            return Err("Oversized grouped block".into());
        }
    }
    Ok(())
}

fn normalize_blocks(
    blocks: &mut [Block],
    roots: &BTreeMap<String, Root>,
    config: Config,
    validation: Validation,
    helper: &HelperSlot,
) -> Result<bool> {
    // Retain the first duplicate's global ordinal. Both halves must choose the
    // same first error as the preceding serial walk, regardless of finish order.
    let mut ids = BTreeSet::new();
    let duplicate = blocks
        .iter()
        .position(|block| !ids.insert((block.root.as_str(), block.key.as_str())));
    drop(ids);
    let split = helper_split(blocks, validation);
    let permit = split.and_then(|_| helper.acquire());
    let (Some(split), Some(_permit)) = (split, permit) else {
        return normalize_block_range(blocks, roots, config, 0, duplicate).map(|()| false);
    };
    let (left, right) = blocks.split_at_mut(split);
    // The mutex keeps ownership available for serial fallback if spawn fails;
    // its two paths never contend. No unsafe lifetime erasure or global data.
    let right = std::sync::Mutex::new(right);
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("freight-canonical".into())
            .spawn_scoped(scope, || {
                let mut right = right.lock().unwrap_or_else(|error| error.into_inner());
                normalize_block_range(&mut right, roots, config, split, duplicate)
            });
        let left_result = normalize_block_range(left, roots, config, 0, duplicate);
        let (right_result, used_helper) = match worker {
            Ok(worker) => (
                worker
                    .join()
                    .unwrap_or_else(|_| Err("Canonical validation worker interrupted".into())),
                true,
            ),
            Err(_) => {
                let mut right = right.lock().unwrap_or_else(|error| error.into_inner());
                (
                    normalize_block_range(&mut right, roots, config, split, duplicate),
                    false,
                )
            }
        };
        // Join even on an earlier error; the shared slot is released only after
        // all borrowed work finishes. A failed frame is never returned as valid.
        left_result.and(right_result).map(|()| used_helper)
    })
}

pub fn unpack(mut packed: Packed) -> Result<String> {
    unpack_in_place(&mut packed)
}

pub fn unpack_for_reuse(mut packed: Packed) -> Result<(String, CanonicalFrame)> {
    let source = unpack_in_place(&mut packed)?;
    Ok((source, CanonicalFrame { frame: packed }))
}

/// Validate and canonicalize a retained frame for exact before/after deltas.
/// An error can leave normalization partially applied; no input may be stored
/// or treated as a valid baseline unless this function returns successfully.
pub fn unpack_in_place(packed: &mut Packed) -> Result<String> {
    unpack_using(packed, Validation::Auto, &VALIDATION_HELPER)
}
fn unpack_using(
    packed: &mut Packed,
    validation: Validation,
    helper: &HelperSlot,
) -> Result<String> {
    if packed.version != VERSION {
        return Err("Unsupported block version".into());
    }
    packed.config.validate()?;
    if packed.root_names != packed.roots.keys().cloned().collect::<Vec<_>>() {
        return Err("Missing or invalid root manifest".into());
    }
    normalize_blocks(
        &mut packed.blocks,
        &packed.roots,
        packed.config,
        validation,
        helper,
    )?;
    for root in packed.roots.values_mut() {
        root.scalar = canonical_raw_at(&root.scalar, 1)?;
    }
    let mut entries: BTreeMap<&str, BTreeMap<(u64, &str), &Entry>> = BTreeMap::new();
    let mut stored: BTreeMap<&str, BTreeMap<&str, &[Entry]>> = BTreeMap::new();
    for block in &packed.blocks {
        stored
            .entry(&block.root)
            .or_default()
            .insert(&block.key, &block.items);
        for item in &block.items {
            if entries
                .entry(&block.root)
                .or_default()
                .insert((item.position, &item.name), item)
                .is_some()
            {
                return Err("Duplicate record identity".into());
            }
        }
    }
    let mut source = String::from("{");
    for (n, (name, root)) in packed.roots.iter().enumerate() {
        let values = entries.remove(name.as_str()).unwrap_or_default();
        let mut root_blocks = stored.remove(name.as_str()).unwrap_or_default();
        match root.kind {
            Kind::Array => {
                if root.scalar.get() != "null"
                    || values.len() as u64 != root.size
                    || values
                        .keys()
                        .enumerate()
                        .any(|(n, (p, k))| *p != n as u64 + 1 || !k.is_empty())
                {
                    return Err("Missing or invalid array ordinal".into());
                }
            }
            Kind::Object => {
                if root.scalar.get() != "null"
                    || values.len() as u64 != root.size
                    || values.keys().any(|(p, _)| *p != 0)
                {
                    return Err("Missing or invalid object member".into());
                }
            }
            _ => {
                if root.size != 0 || !values.is_empty() || kind(root.scalar.get())? != root.kind {
                    return Err("Invalid scalar descriptor".into());
                }
            }
        }
        if matches!(root.kind, Kind::Array | Kind::Object) {
            for expected in partition(name, root.kind, values.values().copied(), packed.config)? {
                let actual = root_blocks
                    .remove(expected.key.as_str())
                    .ok_or("Noncanonical block address or partition")?;
                if actual.len() != expected.items.len()
                    || !actual.iter().zip(expected.items).all(|(a, b)| {
                        a.position == b.position
                            && a.name == b.name
                            && a.payload.get() == b.payload.get()
                    })
                {
                    return Err("Noncanonical block address or partition".into());
                }
            }
        }
        if !root_blocks.is_empty() {
            return Err("Noncanonical block address or partition".into());
        }
        if n != 0 {
            source.push(',');
        }
        source.push_str(&json(name)?);
        source.push(':');
        match root.kind {
            Kind::Array | Kind::Object => {
                source.push(if root.kind == Kind::Array { '[' } else { '{' });
                for (n, ((_, key), item)) in values.iter().enumerate() {
                    if n != 0 {
                        source.push(',');
                    }
                    if root.kind == Kind::Object {
                        source.push_str(&json(key)?);
                        source.push(':');
                    }
                    source.push_str(item.payload.get());
                }
                source.push(if root.kind == Kind::Array { ']' } else { '}' });
            }
            _ => source.push_str(root.scalar.get()),
        }
    }
    if !entries.is_empty() {
        return Err("Unowned records".into());
    }
    if !stored.is_empty() {
        return Err("Noncanonical block address or partition".into());
    }
    source.push('}');
    Ok(source)
}

#[cfg(test)]
mod tests {
    #[test]
    fn validated_original_reuse_matches_fresh_frames_after_edits_reorder_and_shape_changes() {
        let initial = r#"{"items":[{"id":"a","raw":{"$serde_json::private::Number":"1e99"},"n":1.2300},{"id":"b","note":"雪\nline"}],"index":{"alpha":{"k":true},"beta":null},"scalar":"雪"}"#;
        let canonical_original = canonical(initial).expect("canonical original");
        let candidates = [
            canonical_original.as_str(),
            initial,
            r#"{"items":[{"id":"a","raw":{"$serde_json::private::Number":"1e99"},"n":1.2300},{"id":"b","note":"new"},{"id":"c","n":-0.00}],"index":{"alpha":{"k":true},"gamma":false},"scalar":"雪"}"#,
            r#"{"items":[{"id":"b","note":"雪\nline"},{"id":"a","raw":{"$serde_json::private::Number":"1e99"},"n":1.2300}],"index":{"beta":null},"scalar":17}"#,
            r#"{"items":{"changed":1.2300},"index":[null],"scalar":null}"#,
            r#"{"items":[],"index":{},"scalar":"雪"}"#,
        ];
        for old_config in [
            Config {
                span: 16,
                target_bytes: 1024,
            },
            Config {
                span: 64,
                target_bytes: 65536,
            },
        ] {
            for config in [
                Config {
                    span: 16,
                    target_bytes: 1024,
                },
                Config {
                    span: 64,
                    target_bytes: 65536,
                },
            ] {
                for source in candidates {
                    let (_, prior) = unpack_for_reuse(pack(initial, old_config).expect("original"))
                        .expect("validated");
                    let expected = pack(source, config).expect("fresh");
                    let reused = pack_reusing(source, config, prior).expect("reused");
                    assert_eq!(
                        serde_json::to_vec(&reused).expect("frame"),
                        serde_json::to_vec(&expected).expect("fresh frame")
                    );
                }
            }
        }
        for bad in [
            r#"{"items":[{"v":1,"v":2}]}"#,
            r#"{"items":[1e999999999999999999999999999]}"#,
        ] {
            let (_, prior) =
                unpack_for_reuse(pack(initial, cfg()).expect("original")).expect("validated");
            assert!(pack_reusing(bad, cfg(), prior).is_err());
        }
        let mut corrupt = pack(initial, cfg()).expect("original");
        corrupt
            .blocks
            .iter_mut()
            .find(|v| v.root == "items")
            .expect("array block")
            .items[0]
            .position = 0;
        assert!(unpack_for_reuse(corrupt).is_err());
    }
    use super::*;
    fn cfg() -> Config {
        Config {
            span: 64,
            target_bytes: 65_536,
        }
    }
    fn frame(s: &str) -> Packed {
        pack(s, cfg()).expect("pack")
    }
    #[test]
    fn exact_values_opaque_keys_and_duplicate_business_ids() {
        let s = r#"{"events":[{"id":"same","n":18446744073709551615},{"id":"same","n":1.000000000000000000123456789}],"receipts":{"literal/\"雪{}":{"$serde_json::private::Number":"retain"}},"operations_cases":null,"zero":0,"false":false,"empty":[],"obj":{}}"#;
        assert_eq!(
            unpack(frame(s)).expect("unpack"),
            canonical(s).expect("canonical")
        );
    }
    #[test]
    fn exponent_and_scale_preserved_without_float() {
        assert_eq!(
            canonical("[1.2300e2,1e-4,-0.00,1e3]").expect("numbers"),
            "[123.00,0.0001,0.00,1000]"
        );
        assert!(canonical("1e999999999999999999999999999").is_err());
        assert_eq!(
            canonical(r#"{"$serde_json::private::Number":"123456"}"#).expect("opaque"),
            r#"{"$serde_json::private::Number":"123456"}"#
        );
    }
    #[test]
    fn expanded_numbers_borrow_exact_bytes_and_keep_the_expansion_budget() {
        for input in [
            "0",
            "123",
            "-123",
            "1.2300",
            "18446744073709551615",
            "0.0001",
        ] {
            assert!(matches!(number(input).expect("expanded"), Cow::Borrowed(v) if v == input));
        }
        for (input, expected) in [("-0", "0"), ("-0.000", "0.000"), ("-0.01", "-0.01")] {
            assert_eq!(number(input).expect("zero sign"), expected);
        }
        let at_budget = format!("0.{}", "0".repeat(999_999));
        assert_eq!(canonical(&at_budget).expect("budget boundary"), at_budget);
        assert!(canonical(&format!("{at_budget}0")).is_err());
        assert_eq!(number("1.2300e2").expect("exponent"), "123.00");
    }
    #[test]
    fn block_size_matches_independent_serialization_with_escaped_names_and_large_originals() {
        #[derive(Serialize)]
        struct Body<'a> {
            root: &'a str,
            key: &'a str,
            items: &'a [Entry],
        }
        let controls: String = (0..=0x7f).map(char::from).collect();
        let names = ["", "literal/\"\\雪🚚", controls.as_str()];
        let positions = [0, 1, 9, 10, 99, 100, u64::MAX];
        let original = format!(
            "{{\"large\":{},\"scale\":1.2300}}",
            json(&"雪".repeat(50_000)).expect("text")
        );
        let entries: Vec<_> = positions
            .into_iter()
            .enumerate()
            .map(|(index, position)| Entry {
                position,
                name: names[index % names.len()].into(),
                payload: raw(if index == 0 {
                    original.clone()
                } else {
                    "[true, null, 1.2300]".into()
                })
                .expect("original"),
            })
            .collect();
        for root in names {
            for key in names {
                for count in 0..=entries.len() {
                    let items = &entries[..count];
                    let expected = serde_json::to_vec(&Body { root, key, items })
                        .expect("independent bytes")
                        .len();
                    assert_eq!(body_len(root, key, items).expect("size"), expected);
                    let borrowed: Vec<_> = items.iter().collect();
                    assert_eq!(
                        body_len(root, key, &borrowed).expect("borrowed size"),
                        expected
                    );
                }
            }
        }
    }
    #[test]
    fn escaped_strings_unicode_forms_and_literal_keys_have_exact_canonical_bytes() {
        let input = r#"{"\u0062":"\ud83d\ude9a","a":"\/\\\b\f\n\r\t\u0000","é":["\u0065\u0301",{"$serde_json::private::RawValue":"literal","e\u0301":"keep","é":1.2300e2}]}"#;
        let expected = "{\"a\":\"/\\\\\\b\\f\\n\\r\\t\\u0000\",\"b\":\"🚚\",\"é\":[\"e\u{301}\",{\"$serde_json::private::RawValue\":\"literal\",\"e\u{301}\":\"keep\",\"é\":123.00}]}";
        assert_eq!(canonical(input).expect("canonical bytes"), expected);
        assert_eq!(unpack(frame(input)).expect("roundtrip"), expected);
    }
    #[test]
    fn escaped_duplicate_keys_and_invalid_strings_refuse_before_output() {
        for payload in [
            r#"{"a":1,"\u0061":2}"#,
            r#"[{"nested":{"雪":1,"\u96ea":2}}]"#,
            r#""\ud800""#,
            r#""\udc00""#,
            r#"{"value":"\ud800x"}"#,
        ] {
            assert!(canonical(payload).is_err(), "{payload}");
            assert!(pack(&format!("{{\"a\":[{payload}]}}"), cfg()).is_err());
            let mut p = frame(r#"{"a":[0]}"#);
            p.blocks[0].items[0].payload = raw(payload.into()).expect("structural JSON");
            assert!(unpack(p).is_err(), "{payload}");
        }
        for invalid in ["true false", "[1] trailing", "{\"a\":1}{}"] {
            assert!(canonical(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn literal_unicode_scalars_roundtrip_exactly_and_raw_controls_refuse() {
        let literal: String = (0x20..=0x10ffff)
            .filter_map(char::from_u32)
            .filter(|&c| c != '"' && c != '\\')
            .collect();
        let encoded = json(&literal).expect("all unescaped scalar values");
        assert!(
            !encoded.contains('\\'),
            "literal corpus must need no escapes"
        );
        assert_eq!(canonical(&encoded).expect("literal bytes"), encoded);
        let source = format!("{{\"a\":[{encoded}]}}");
        assert_eq!(unpack(frame(&source)).expect("literal frame"), source);
        for byte in 0..0x20_u8 {
            let invalid = format!("\"{}\"", char::from(byte));
            assert!(canonical(&invalid).is_err(), "raw control {byte}");
            assert!(pack(&format!("{{\"a\":[{invalid}]}}"), cfg()).is_err());
        }
    }
    #[test]
    fn whitespace_and_object_order_do_not_move_blocks() {
        let a = frame(r#"{"x":[{"b":2,"a":1}],"m":{"雪":true}}"#);
        let b = frame("{ \"m\": {\"雪\": true}, \"x\": [{\"a\": 1, \"b\": 2}] }");
        assert_eq!(json(&a).expect("a"), json(&b).expect("b"));
    }
    #[test]
    fn missing_record_and_duplicate_block_refused() {
        let mut a = frame(r#"{"a":[1,2,3]}"#);
        a.blocks[0].items.pop();
        assert!(unpack(a).is_err());
        let mut a = frame(r#"{"a":[1,2,3]}"#);
        let duplicate = serde_json::from_str(&json(&a.blocks[0]).expect("block")).expect("copy");
        a.blocks.push(duplicate);
        assert!(unpack(a).is_err());
    }
    #[test]
    fn missing_root_wrong_address_and_future_version_refused() {
        let mut a = frame(r#"{"a":[1,2]}"#);
        a.roots.clear();
        assert!(unpack(a).is_err());
        let mut a = frame(r#"{"a":[1,2]}"#);
        a.blocks[0].key = "elsewhere".into();
        assert!(unpack(a).is_err());
        let mut a = frame("{}");
        a.version += 1;
        assert!(unpack(a).is_err());
    }
    #[test]
    fn duplicate_json_keys_refused() {
        assert!(pack(r#"{"x":1,"x":2}"#, cfg()).is_err());
        assert!(pack(r#"{"x":[{"v":1,"v":2}]}"#, cfg()).is_err());
    }
    #[test]
    fn oversized_original_is_retained_as_singleton() {
        let source = format!(
            "{{\"a\":[{},1]}}",
            json(&"x".repeat(131_072)).expect("text")
        );
        let p = frame(&source);
        assert!(p
            .blocks
            .iter()
            .any(|b| body_len(&b.root, &b.key, &b.items).expect("size") > cfg().target_bytes));
        assert_eq!(
            unpack(p).expect("retained"),
            canonical(&source).expect("source")
        );
    }
    #[test]
    fn append_and_local_change_keep_unrelated_array_blocks() {
        let source = format!(
            "{{\"a\":[{}]}}",
            (0..200)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let a = frame(&source);
        let next = source
            .replace("0,1,2,", "999,1,2,")
            .replace("199]", "199,200]");
        let b = frame(&next);
        let old: BTreeMap<_, _> = a
            .blocks
            .iter()
            .map(|v| (v.key.clone(), json(v).expect("old")))
            .collect();
        let new: BTreeMap<_, _> = b
            .blocks
            .iter()
            .map(|v| (v.key.clone(), json(v).expect("new")))
            .collect();
        assert_eq!(
            old.get("a/0000000000000001/")
                .expect("original middle block"),
            new.get("a/0000000000000001/")
                .expect("retained middle block")
        );
        assert_eq!(
            old.get("a/0000000000000002/")
                .expect("original later block"),
            new.get("a/0000000000000002/")
                .expect("retained later block")
        );
        assert_eq!(
            unpack(b).expect("decode"),
            canonical(&next).expect("source")
        );
    }
    #[test]
    fn blocks_obey_count_and_byte_targets_except_single_original() {
        let source = format!(
            "{{\"a\":[{}]}}",
            (0..200)
                .map(|n| format!(
                    "{{\"id\":{n},\"text\":{}}}",
                    json(&"z".repeat(2000)).expect("text")
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let p = frame(&source);
        assert!(p.blocks.len() > 4);
        for b in &p.blocks {
            assert!(b.items.len() <= 64);
            assert!(
                body_len(&b.root, &b.key, &b.items).expect("size") <= cfg().target_bytes
                    || b.items.len() == 1
            );
        }
        assert_eq!(
            unpack(p).expect("decode"),
            canonical(&source).expect("source")
        );
    }
    #[test]
    fn missing_scalar_root_and_duplicate_root_metadata_refused() {
        let mut a = frame(r#"{"scalar":"original","a":[]}"#);
        a.roots.remove("scalar");
        assert!(unpack(a)
            .expect_err("no scalar default")
            .contains("root manifest"));
        let encoded = json(&frame(r#"{"scalar":"original"}"#)).expect("frame");
        let repeated = encoded.replace(
            "\"roots\":{",
            "\"roots\":{\"scalar\":{\"kind\":\"string\",\"size\":0,\"scalar\":\"wrong\"},",
        );
        assert!(serde_json::from_str::<Packed>(&repeated).is_err());
    }
    #[test]
    fn empty_roots_scalars_and_null_shape_roundtrip() {
        for s in ["{}", r#"{"a":[],"b":{},"c":null,"d":true,"e":"雪"}"#] {
            assert_eq!(
                unpack(frame(s)).expect("roundtrip"),
                canonical(s).expect("source")
            );
        }
    }
    #[test]
    fn new_object_key_does_not_repack_other_hash_buckets() {
        let mut values = BTreeMap::new();
        for n in 0..500 {
            values.insert(
                format!("literal/\"雪{n}"),
                raw(n.to_string()).expect("number"),
            );
        }
        let before = format!("{{\"receipts\":{}}}", json(&values).expect("map"));
        values.insert(
            "new literal/\"雪[]".into(),
            raw("true".into()).expect("bool"),
        );
        let after = format!("{{\"receipts\":{}}}", json(&values).expect("map"));
        let config = Config {
            span: 16,
            target_bytes: 1024,
        };
        let a = pack(&before, config).expect("old");
        let b = pack(&after, config).expect("new");
        let prefix = format!("o/{:02x}/", hash("new literal/\"雪[]")[0]);
        let next: BTreeMap<_, _> = b
            .blocks
            .iter()
            .map(|v| (v.key.clone(), json(v).expect("block")))
            .collect();
        let mut compared = 0;
        for block in &a.blocks {
            if !block.key.starts_with(&prefix) {
                assert_eq!(
                    next.get(&block.key).expect("unrelated block retained"),
                    &json(block).expect("old block")
                );
                compared += 1;
            }
        }
        assert!(compared > 100);
        assert_eq!(
            unpack(b).expect("decode"),
            canonical(&after).expect("source")
        );
    }
    #[test]
    fn full_hash_collision_fallback_retains_every_original_key() {
        let items = vec![
            Entry {
                position: 0,
                name: "first".into(),
                payload: raw(json(&"a".repeat(700)).expect("text")).expect("raw"),
            },
            Entry {
                position: 0,
                name: "second".into(),
                payload: raw(json(&"b".repeat(700)).expect("text")).expect("raw"),
            },
        ];
        let mut blocks = vec![];
        object_blocks(
            "receipts",
            (0, 256, String::new()),
            items,
            Config {
                span: 16,
                target_bytes: 1024,
            },
            &mut blocks,
        )
        .expect("collision split");
        assert_eq!(blocks.len(), 2);
        assert_ne!(blocks[0].key, blocks[1].key);
        assert_eq!(
            blocks
                .iter()
                .flat_map(|b| b.items.iter().map(|e| e.name.as_str()))
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }
    #[test]
    fn unsupported_configuration_and_unknown_envelope_fields_refused() {
        assert!(pack(
            "{}",
            Config {
                span: 3,
                target_bytes: 65536
            }
        )
        .is_err());
        assert!(pack(
            "{}",
            Config {
                span: 64,
                target_bytes: 0
            }
        )
        .is_err());
        let encoded = json(&frame("{}")).expect("frame");
        let future = encoded.replacen('{', "{\"future_layout\":true,", 1);
        assert!(serde_json::from_str::<Packed>(&future).is_err());
    }
    #[test]
    fn complete_records_in_unnecessary_subdivisions_are_refused() {
        let mut p = frame(r#"{"a":[1,2,3]}"#);
        let mut block = p.blocks.pop().expect("original block");
        let right = block.items.split_off(2);
        block.key.push('0');
        p.blocks.push(Block {
            root: "a".into(),
            key: "a/0000000000000000/1".into(),
            items: right,
        });
        p.blocks.push(block);
        assert!(unpack(p)
            .expect_err("noncanonical subdivisions")
            .contains("partition"));
    }
    #[test]
    fn item_order_within_array_or_object_blocks_is_checked() {
        let mut p = frame(r#"{"a":[1,2,3]}"#);
        p.blocks[0].items.reverse();
        assert!(unpack(p)
            .expect_err("array item order")
            .contains("partition"));
        let source = format!(
            "{{\"r\":{{{}}}}}",
            (0..500)
                .map(|n| format!("\"key{n}\":{n}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut p = frame(&source);
        p.blocks
            .iter_mut()
            .find(|b| b.items.len() > 1)
            .expect("shared object bucket")
            .items
            .reverse();
        assert!(unpack(p)
            .expect_err("object item order")
            .contains("partition"));
    }
    #[test]
    fn duplicate_record_identity_across_different_blocks_is_refused() {
        let source = format!(
            "{{\"a\":[{}]}}",
            (0..65).map(|n| n.to_string()).collect::<Vec<_>>().join(",")
        );
        let mut p = frame(&source);
        assert_eq!(p.blocks.len(), 2);
        let duplicate =
            serde_json::from_str(&json(&p.blocks[0].items[0]).expect("entry")).expect("copy");
        p.blocks[1].items.push(duplicate);
        assert!(unpack(p)
            .expect_err("duplicate identity")
            .contains("Duplicate record identity"));
    }
    #[test]
    fn noncanonical_payload_text_and_scalar_scale_still_roundtrip() {
        let source = r#"{"literal\n\"雪}":[{"a":123.00,"b":false}],"scalar":123.00}"#;
        let mut p = frame(source);
        p.blocks[0].items[0].payload =
            raw("{ \"b\": false, \"a\": 1.2300e2 }".into()).expect("payload");
        p.roots.get_mut("scalar").expect("scalar").scalar =
            raw("1.2300e2".into()).expect("decimal");
        assert_eq!(
            unpack(p).expect("normalization retained"),
            canonical(source).expect("source")
        );
    }
    #[test]
    fn corrupt_descriptors_and_extreme_ordinals_fail_before_partitioning() {
        let mut p = frame(r#"{"a":[1]}"#);
        p.roots.get_mut("a").expect("root").size += 1;
        assert!(unpack(p).expect_err("wrong size").contains("ordinal"));
        for position in [0, u64::MAX] {
            let mut p = frame(r#"{"a":[1]}"#);
            p.blocks[0].items[0].position = position;
            assert!(unpack(p).is_err());
        }
        let mut p = frame(r#"{"scalar":1}"#);
        p.roots.get_mut("scalar").expect("root").scalar = raw("false".into()).expect("bool");
        assert!(unpack(p)
            .expect_err("wrong scalar type")
            .contains("scalar descriptor"));
    }
    #[test]
    fn nesting_budget_is_consistent_between_workspace_packing_and_frame_decoding() {
        let source = format!("{{\"a\":[{}0{}]}}", "[".repeat(120), "]".repeat(120));
        let p = frame(&source);
        assert_eq!(unpack(p).expect("supported depth"), source);
        let source = format!("{{\"a\":[{}0{}]}}", "[".repeat(256), "]".repeat(256));
        assert!(pack(&source, cfg()).is_err());
        let mut p = frame(r#"{"a":[0]}"#);
        p.blocks[0].items[0].payload =
            raw(format!("{}0{}", "[".repeat(256), "]".repeat(256))).expect("raw structural JSON");
        assert!(unpack(p).is_err());
    }
}

#[cfg(test)]
#[path = "parallel_tests.rs"]
mod parallel_tests;
