use super::*;
use crate::{canonical_storage as canonical, domain::Workspace};
use serde_json::json;
use stateset_freight_storage_codec as codec;

fn copy(snapshot: &Snapshot) -> Snapshot {
    Snapshot {
        header: snapshot.header.clone(),
        rows: snapshot.rows.clone(),
    }
}
fn stored(workspace: &Workspace) -> Snapshot {
    let mut frame = canonical::encode(workspace).expect("complete frame");
    let rows = std::mem::take(&mut frame.blocks)
        .into_iter()
        .map(|block| {
            let raw = serde_json::to_string(&block).expect("original block");
            (block.root, block.key, raw)
        })
        .collect();
    Snapshot {
        header: canonical::head(frame).expect("header").get().into(),
        rows,
    }
}
fn large() -> Workspace {
    let mut workspace = crate::domain::demo();
    let mut load = workspace.loads[0].clone();
    load.source_order = Some(json!({"literal/雪": "owned opaque metadata ".repeat(7000),
        "large":u64::MAX,"negative":i64::MIN,"nested":[true,null,"quoted \"original\""]}));
    workspace.loads = (0..64)
        .map(|n| {
            let mut load = load.clone();
            load.id = format!("owned-cache-{n}");
            load
        })
        .collect();
    workspace
}

#[test]
fn every_snapshot_byte_tenant_row_boundary_order_and_count_bind_the_key() {
    let original = Snapshot {
        header: "owned header".into(),
        rows: vec![
            ("a".into(), "bc".into(), "雪".into()),
            ("d".into(), "e".into(), "f".into()),
        ],
    };
    let key = fingerprint(&original, "tenant-a");
    assert_ne!(key, fingerprint(&original, "tenant-b"));
    for field in [
        "header",
        "root",
        "key",
        "payload",
        "row_boundary",
        "order",
        "count",
    ] {
        let mut changed = copy(&original);
        match field {
            "header" => changed.header.push('!'),
            "root" => changed.rows[0].0.push('!'),
            "key" => changed.rows[0].1.push('!'),
            "payload" => changed.rows[0].2.push('!'),
            "row_boundary" => {
                changed.rows[0].0 = "ab".into();
                changed.rows[0].1 = "c".into();
            }
            "order" => changed.rows.swap(0, 1),
            "count" => changed
                .rows
                .push((String::new(), String::new(), String::new())),
            _ => unreachable!(),
        }
        assert_ne!(key, fingerprint(&changed, "tenant-a"), "{field}");
    }
    assert_eq!(key, fingerprint(&copy(&original), "tenant-a"));
}

#[test]
fn small_sparse_and_oversized_snapshots_never_use_the_cache() {
    let mut snapshot = Snapshot {
        header: String::new(),
        rows: vec![(String::new(), String::new(), String::new()); 64],
    };
    snapshot.rows[0].2 = "x".repeat(MIN_BYTES - 1);
    assert!(key(&snapshot, "tenant").is_none());
    snapshot.rows[0].2.push('x');
    assert!(key(&snapshot, "tenant").is_some());
    snapshot.rows.pop();
    assert!(key(&snapshot, "tenant").is_none());
    snapshot
        .rows
        .push((String::new(), String::new(), String::new()));
    snapshot.rows[0].2 = "x".repeat(MAX_BYTES + 1);
    assert!(key(&snapshot, "tenant").is_none());
}

#[test]
fn cache_damage_and_contention_fall_back_with_bounded_compression_and_inflation() {
    let cache = ReadCache::default();
    let original = "owned original ".repeat(MIN_BYTES / 15 + 1);
    cache.insert([1; 32], &original);
    assert_eq!(cache.get([1; 32]).as_deref(), Some(original.as_str()));
    assert!(cache.get([2; 32]).is_none());
    for damage in ["compressed", "short", "long", "digest", "out_of_bounds"] {
        {
            let mut slot = cache.entry.lock().expect("owned cache");
            let entry = slot.as_mut().expect("current entry");
            match damage {
                "compressed" => entry.compressed[0] ^= 1,
                "short" => entry.canonical_bytes -= 1,
                "long" => entry.canonical_bytes += 1,
                "digest" => entry.canonical_digest[0] ^= 1,
                "out_of_bounds" => entry.canonical_bytes = MAX_BYTES + 1,
                _ => unreachable!(),
            }
        }
        assert!(cache.get([1; 32]).is_none(), "{damage}");
        assert!(cache.entry.lock().expect("cache").is_none());
        cache.insert([1; 32], &original);
    }
    {
        let _held = cache.entry.lock().expect("hold cache");
        assert!(
            cache.get([1; 32]).is_none(),
            "no cache queue behind inflation"
        );
    }
    cache.filling.store(true, Ordering::Relaxed);
    cache.insert([2; 32], &original);
    assert!(
        cache.filling.load(Ordering::Relaxed),
        "never release another fill"
    );
    assert!(cache.get([2; 32]).is_none());
    cache.filling.store(false, Ordering::Release);
    cache.insert([2; 32], &original);
    assert!(cache.get([1; 32]).is_none(), "exactly one retained entry");
    assert_eq!(cache.get([2; 32]).as_deref(), Some(original.as_str()));
    let mut output = BoundedBuffer::default();
    output
        .write_all(&vec![0; MAX_COMPRESSED - 1])
        .expect("bounded bytes");
    assert!(output.write_all(&[1, 2]).is_err());
    assert_eq!(output.0.len(), MAX_COMPRESSED - 1);
    assert!(output.0.capacity() <= MAX_COMPRESSED);

    // Incompressible eligible input must not publish an oversized entry or
    // leak the filling reservation. The preceding good entry survives.
    let mut state = 0x9e37_79b9_u32;
    let noisy: String = (0..MIN_BYTES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            char::from(32 + (state % 95) as u8)
        })
        .collect();
    cache.insert([3; 32], &noisy);
    assert!(!cache.filling.load(Ordering::Relaxed));
    assert!(cache.get([3; 32]).is_none());
    assert!(cache.get([2; 32]).is_some());
}

#[test]
fn exact_full_frame_reuse_preserves_every_original_and_rejects_changed_or_corrupt_bytes() {
    let workspace = large();
    let snapshot = stored(&workspace);
    let original = json!(workspace);
    let cache = ReadCache::default();
    let first_key = key(&snapshot, "owned-a").expect("eligible real frame");
    assert_eq!(
        json!(copy(&snapshot)
            .decode_cached("owned-a", &cache)
            .expect("first validation")),
        original
    );
    assert!(
        cache.get(first_key).is_some(),
        "first successful decode publishes"
    );
    let previous_hits = cache.hits();
    assert_eq!(
        json!(copy(&snapshot)
            .decode_cached("owned-a", &cache)
            .expect("actual cached decode")),
        original
    );
    assert_eq!(
        cache.hits(),
        previous_hits + 1,
        "second read uses the cached source"
    );
    assert!(cache
        .get(key(&snapshot, "owned-b").expect("other tenant"))
        .is_none());
    for fault in [
        "missing",
        "duplicate",
        "envelope",
        "payload",
        "future",
        "manifest",
    ] {
        let mut changed = copy(&snapshot);
        match fault {
            "missing" => {
                changed.rows.remove(0);
            }
            "duplicate" => changed.rows.push(changed.rows[0].clone()),
            "envelope" => changed.rows[0].0.push_str("-wrong"),
            "payload" => changed.rows[0].2 = "null".into(),
            "future" => {
                changed.header =
                    changed
                        .header
                        .replacen("\"_freight_format\":21", "\"_freight_format\":999", 1)
            }
            "manifest" => {
                let mut header: serde_json::Value =
                    serde_json::from_str(&changed.header).expect("header");
                header["frame"]["root_names"] = json!([]);
                changed.header = header.to_string();
            }
            _ => unreachable!(),
        }
        assert_ne!(key(&changed, "owned-a"), Some(first_key), "{fault}");
        let uncached = copy(&changed)
            .decode()
            .expect_err("ordinary complete validation refuses");
        assert_eq!(
            changed
                .decode_cached("owned-a", &cache)
                .expect_err("no stale original escapes"),
            uncached,
            "{fault}"
        );
        assert!(
            cache.get(first_key).is_some(),
            "refusal cannot poison original cache"
        );
    }
    let mut changed = workspace;
    changed.loads[0].location = "Changed block with unchanged header".into();
    let changed = stored(&changed);
    assert_eq!(
        snapshot.header, changed.header,
        "counts and canonical header stay the same"
    );
    assert_ne!(key(&snapshot, "owned-a"), key(&changed, "owned-a"));
    let expected = json!(copy(&changed).decode().expect("current original"));
    assert_eq!(
        json!(changed
            .decode_cached("owned-a", &cache)
            .expect("fresh changed source")),
        expected
    );
    assert!(
        cache.get(first_key).is_none(),
        "new source replaces the single entry"
    );
}

#[test]
fn typed_model_failure_never_publishes_even_when_complete_codec_validation_passes() {
    let mut source = crate::write_json::encode(&large(), canonical::FORMAT).expect("original");
    let mut value: serde_json::Value = serde_json::from_str(source.get()).expect("source");
    value["future_business_root"] = json!([]);
    source = serde_json::value::to_raw_value(&value).expect("unknown root");
    let mut frame = codec::pack(
        source.get(),
        codec::Config {
            span: 64,
            target_bytes: 65_536,
        },
    )
    .expect("valid codec frame");
    let rows = std::mem::take(&mut frame.blocks)
        .into_iter()
        .map(|block| {
            let raw = serde_json::to_string(&block).expect("block");
            (block.root, block.key, raw)
        })
        .collect();
    let snapshot = Snapshot {
        header: canonical::head(frame).expect("header").get().into(),
        rows,
    };
    assert!(key(&snapshot, "owned").is_some());
    let cache = ReadCache::default();
    let ordinary = copy(&snapshot)
        .decode()
        .expect_err("unsupported typed root");
    assert_eq!(
        snapshot
            .decode_cached("owned", &cache)
            .expect_err("unsupported typed root"),
        ordinary
    );
    assert!(cache.entry.lock().expect("cache").is_none());
}
