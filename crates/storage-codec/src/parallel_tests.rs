//! Preserve the preceding serial validator as an independent compatibility oracle.
use super::*;
fn preceding(packed: &mut Packed) -> Result<()> {
    let mut ids = BTreeSet::new();
    for block in &mut packed.blocks {
        if !ids.insert((block.root.clone(), block.key.clone())) {
            return Err("Duplicate block".into());
        }
        let root = packed.roots.get(&block.root).ok_or("Block without root")?;
        if block.items.is_empty() || block.items.len() > packed.config.span as usize {
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
            && body_len(&block.root, &block.key, &block.items)? > packed.config.target_bytes
        {
            return Err("Oversized grouped block".into());
        }
    }
    Ok(())
}
fn copy(packed: &Packed) -> Packed {
    serde_json::from_slice(&serde_json::to_vec(packed).expect("frame bytes")).expect("frame")
}
fn fixture() -> Packed {
    let original = serde_json::json!({
        "records":(0..24).map(|n| serde_json::json!({"id":n,"opaque":{"$serde_json::private::RawValue":"retain","literal/雪":"value"}})).collect::<Vec<_>>(),
        "mapping":{"literal/\"雪":{"scale":1.25},"empty":{}},"empty":[],"scalar":"original"});
    pack(
        &original.to_string(),
        Config {
            span: 1,
            target_bytes: 1024,
        },
    )
    .expect("frame")
}
fn check(packed: Packed) {
    let mut old = copy(&packed);
    let expected = preceding(&mut old);
    for policy in [Validation::Serial, Validation::Parallel] {
        let slot = HelperSlot::new();
        let mut candidate = copy(&packed);
        let actual = normalize_blocks(
            &mut candidate.blocks,
            &candidate.roots,
            candidate.config,
            policy,
            &slot,
        )
        .map(|_| ());
        assert_eq!(actual, expected, "first validation error changes");
        if actual.is_ok() {
            assert_eq!(
                json(&candidate).expect("candidate frame"),
                json(&old).expect("preceding frame")
            );
            assert_eq!(
                unpack(candidate),
                unpack(copy(&old)),
                "complete canonical bytes change"
            );
        }
        assert!(
            slot.acquire().is_some(),
            "helper slot leaked after completion/error"
        );
    }
}
#[test]
fn both_execution_modes_preserve_exact_unicode_opaque_numbers_and_reusable_frames() {
    let original = r#"{"records":[{"id":"duplicate","n":18446744073709551615},{"id":"duplicate","n":1.000000000000000000123456789},{"literal/\"雪":{"$serde_json::private::Number":"retain","n":1.2300e2}},[true,null,0]],"scalar":false}"#;
    let packed = pack(
        original,
        Config {
            span: 1,
            target_bytes: 1024,
        },
    )
    .expect("frame");
    let mut serial = copy(&packed);
    let mut parallel = copy(&packed);
    let serial = unpack_using(&mut serial, Validation::Serial, &HelperSlot::new()).expect("serial");
    let parallel =
        unpack_using(&mut parallel, Validation::Parallel, &HelperSlot::new()).expect("parallel");
    assert_eq!(serial, canonical(original).expect("independent canonical"));
    assert_eq!(serial, parallel);
    check(packed);
}
#[test]
fn corruptions_in_both_halves_retain_first_error_and_never_return_a_frame() {
    let base = fixture();
    let n = base.blocks.len();
    assert!(n >= 24);
    for index in [0, 1, n / 2, n - 2, n - 1] {
        for corruption in [
            "root",
            "empty",
            "count",
            "identity",
            "duplicate-key",
            "number",
            "duplicate-block",
            "depth",
        ] {
            let mut p = copy(&base);
            match corruption {
                "root" => p.blocks[index].root = "missing-root".into(),
                "empty" => p.blocks[index].items.clear(),
                "count" => p.blocks[index].items.push(Entry {
                    position: 999,
                    name: String::new(),
                    payload: raw("0".into()).expect("raw"),
                }),
                "identity" => {
                    let kind = p.roots[&p.blocks[index].root].kind;
                    p.blocks[index].items[0].position = if kind == Kind::Array { 0 } else { 1 };
                }
                "duplicate-key" => {
                    p.blocks[index].items[0].payload =
                        raw(r#"{"a":1,"\u0061":2}"#.into()).expect("structural JSON")
                }
                "number" => {
                    p.blocks[index].items[0].payload =
                        raw("1e999999999999999999999".into()).expect("lexical JSON")
                }
                "depth" => {
                    p.blocks[index].items[0].payload =
                        raw(format!("{}0{}", "[".repeat(128), "]".repeat(128))).expect("raw depth")
                }
                "duplicate-block" => {
                    let prior = if index == 0 { n - 1 } else { 0 };
                    p.blocks[index].root = p.blocks[prior].root.clone();
                    p.blocks[index].key = p.blocks[prior].key.clone();
                }
                _ => unreachable!(),
            }
            // When the duplicate is after another bad block, the preceding
            // validator must still report the earlier corruption.
            check(p);
        }
    }
    let mut p = copy(&base);
    p.blocks[0].root = "missing-root".into();
    p.blocks[n - 1].root = p.blocks[n - 2].root.clone();
    p.blocks[n - 1].key = p.blocks[n - 2].key.clone();
    check(p);
    let mut p = copy(&base);
    p.blocks[0].items.clear();
    p.blocks[n - 1].root = "missing-root".into();
    check(p);
}
#[test]
fn competing_frames_fall_back_without_waiting_or_releasing_someone_elses_slot() {
    let slot = HelperSlot::new();
    let held = slot.acquire().expect("reserve helper");
    let original = fixture();
    let expected = unpack(copy(&original)).expect("reference");
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let mut frame = copy(&original);
            let used = normalize_blocks(
                &mut frame.blocks,
                &frame.roots,
                frame.config,
                Validation::Parallel,
                &slot,
            )
            .expect("serial fallback");
            assert!(!used);
            assert_eq!(unpack(frame).expect("complete output"), expected);
        });
        worker
            .join()
            .expect("competing frame returned while slot is held");
    });
    assert!(slot.acquire().is_none(), "competitor released our permit");
    drop(held);
    assert!(slot.acquire().is_some());
}
#[test]
fn parallel_path_really_spawns_and_automatic_selection_keeps_small_frames_serial() {
    let slot = HelperSlot::new();
    let mut p = fixture();
    assert!(helper_split(&p.blocks, Validation::Auto).is_none());
    assert!(normalize_blocks(
        &mut p.blocks,
        &p.roots,
        p.config,
        Validation::Parallel,
        &slot
    )
    .expect("parallel validator"));
    assert!(slot.acquire().is_some());
    // Individual originals may exceed the grouping target. The production
    // large-frame path must still return every byte without repartitioning.
    let payload = "雪".repeat(48_000);
    let original=serde_json::json!({"records":(0..64).map(|n| serde_json::json!({"id":n,"body":payload})).collect::<Vec<_>>()}).to_string();
    let p = pack(
        &original,
        Config {
            span: 64,
            target_bytes: 65536,
        },
    )
    .expect("large frame");
    assert_eq!(p.blocks.len(), 64);
    let eligible = helper_split(&p.blocks, Validation::Auto).is_some();
    assert_eq!(
        eligible,
        std::thread::available_parallelism().is_ok_and(|n| n.get() >= 2)
    );
    let expected = canonical(&original).expect("full reference");
    assert_eq!(unpack(p).expect("automatic complete bytes"), expected);
}
