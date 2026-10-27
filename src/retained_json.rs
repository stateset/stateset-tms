//! Preserve original JSON wherever the typed business view did not change.
//! Typed models may default fields, normalize timestamps or round opaque JSON
//! numbers. None of those effects authorizes rewriting unchanged evidence.
use serde::ser::{Error, SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use serde_json::value::RawValue;
use std::collections::BTreeMap;

type Object<'a> = BTreeMap<String, &'a RawValue>;
fn object(raw: &RawValue) -> Result<Object<'_>, serde_json::Error> {
    serde_json::from_str(raw.get())
}
fn shape(raw: &RawValue) -> Option<u8> {
    raw.get().as_bytes().first().copied()
}
fn identity(raw: &RawValue) -> Option<&str> {
    if shape(raw) != Some(b'{') {
        return None;
    }
    object(raw)
        .ok()?
        .get("id")
        .copied()
        .filter(|id| shape(id) == Some(b'"'))
        .map(RawValue::get)
}
struct RecordMatches<'a> {
    exact: BTreeMap<&'a str, Vec<usize>>,
    unique_ids: BTreeMap<&'a str, Option<usize>>,
}
impl<'a> RecordMatches<'a> {
    fn new(values: &[&'a RawValue]) -> Self {
        let mut exact = BTreeMap::<_, Vec<_>>::new();
        let mut unique_ids = BTreeMap::new();
        for (index, value) in values.iter().enumerate() {
            exact.entry(value.get()).or_default().push(index);
            if let Some(id) = identity(value) {
                unique_ids
                    .entry(id)
                    .and_modify(|old| *old = None)
                    .or_insert(Some(index));
            }
        }
        Self { exact, unique_ids }
    }
}
struct Overlay<'a> {
    original: &'a RawValue,
    before: &'a RawValue,
    after: &'a RawValue,
    depth: usize,
}
impl Serialize for Overlay<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.depth > 128 {
            return Err(S::Error::custom("Retained JSON nesting budget exceeded"));
        }
        if self.before.get() == self.after.get() {
            return self.original.serialize(serializer);
        }
        match (shape(self.original), shape(self.before), shape(self.after)) {
            (Some(b'{'), Some(b'{'), Some(b'{')) => {
                let original = object(self.original).map_err(S::Error::custom)?;
                let before = object(self.before).map_err(S::Error::custom)?;
                let after = object(self.after).map_err(S::Error::custom)?;
                // A different record at the same ordinal is not a mutation of
                // the preceding record's unknown evidence.
                if before
                    .get("id")
                    .zip(after.get("id"))
                    .is_some_and(|(a, b)| a.get() != b.get())
                {
                    return self.after.serialize(serializer);
                }
                let mut map = serializer.serialize_map(None)?;
                for (key, value) in &after {
                    // A serde default in an unchanged typed field does not
                    // authorize adding that field to the original source.
                    if !original.contains_key(key)
                        && before.get(key).is_some_and(|old| old.get() == value.get())
                    {
                        continue;
                    }
                    if let (Some(original), Some(before)) = (original.get(key), before.get(key)) {
                        map.serialize_entry(
                            key,
                            &Overlay {
                                original,
                                before,
                                after: value,
                                depth: self.depth + 1,
                            },
                        )?;
                    } else {
                        map.serialize_entry(key, value)?;
                    }
                }
                // A field absent from the typed view could not be selected
                // for deletion by that view. Retain its original value.
                for (key, value) in &original {
                    if !before.contains_key(key) && !after.contains_key(key) {
                        map.serialize_entry(key, value)?;
                    }
                }
                map.end()
            }
            (Some(b'['), Some(b'['), Some(b'[')) => {
                let original: Vec<&RawValue> =
                    serde_json::from_str(self.original.get()).map_err(S::Error::custom)?;
                let before: Vec<&RawValue> =
                    serde_json::from_str(self.before.get()).map_err(S::Error::custom)?;
                let after: Vec<&RawValue> =
                    serde_json::from_str(self.after.get()).map_err(S::Error::custom)?;
                let mut matches = None;
                let mut used = vec![false; before.len()];
                let mut remaining = before.len();
                let mut sequence = serializer.serialize_seq(Some(after.len()))?;
                for (n, value) in after.iter().enumerate() {
                    // Preserve moved unchanged records without deduplicating
                    // repeated IDs or equal typed representations.
                    let same_position =
                        before.get(n).is_some_and(|old| old.get() == value.get()) && !used[n];
                    // Appending history keeps the complete original prefix.
                    // Build identity indexes only if an unmatched old member
                    // can actually be selected after a move or edit.
                    let lookup: Option<&RecordMatches<'_>> = if !same_position && remaining > 0 {
                        Some(matches.get_or_insert_with(|| RecordMatches::new(&before)))
                    } else {
                        matches.as_ref()
                    };
                    let exact = if same_position {
                        Some(n)
                    } else {
                        lookup
                            .and_then(|lookup| lookup.exact.get(value.get()))
                            .and_then(|positions| {
                                positions.iter().find(|index| !used[**index]).copied()
                            })
                    };
                    if let Some(index) = exact.filter(|index| original.get(*index).is_some()) {
                        used[index] = true;
                        remaining -= 1;
                        sequence.serialize_element(original[index])?;
                    } else {
                        let index = lookup
                            .and_then(|lookup| {
                                identity(value)
                                    .and_then(|id| lookup.unique_ids.get(id).copied().flatten())
                            })
                            .unwrap_or(n);
                        if !used.get(index).copied().unwrap_or(false) {
                            if let (Some(original), Some(before)) =
                                (original.get(index), before.get(index))
                            {
                                used[index] = true;
                                remaining -= 1;
                                sequence.serialize_element(&Overlay {
                                    original,
                                    before,
                                    after: value,
                                    depth: self.depth + 1,
                                })?;
                                continue;
                            }
                        }
                        sequence.serialize_element(value)?;
                    }
                }
                sequence.end()
            }
            _ => self.after.serialize(serializer),
        }
    }
}
pub(crate) fn merge(
    original: &RawValue,
    before: &RawValue,
    after: &RawValue,
) -> Result<Box<RawValue>, serde_json::Error> {
    // Original receipts are immutable. Normalization of a typed response is
    // ignored; an actual typed change/removal is refused before SQL writes.
    let previous = object(before)?;
    let current = object(after)?;
    if let Some(receipts) = previous.get("receipts") {
        let previous = object(receipts)?;
        let current = current
            .get("receipts")
            .ok_or_else(|| serde_json::Error::custom("Original receipts removed"))?;
        let current = object(current)?;
        for (key, value) in previous {
            if current.get(&key).is_none_or(|new| new.get() != value.get()) {
                return Err(serde_json::Error::custom(
                    "Original receipt changed or removed",
                ));
            }
        }
    }
    serde_json::value::to_raw_value(&Overlay {
        original,
        before,
        after,
        depth: 0,
    })
}

struct Literal<'a> {
    raw: &'a RawValue,
    depth: usize,
}
impl Serialize for Literal<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.depth > 128 {
            return Err(S::Error::custom("Retained JSON nesting budget exceeded"));
        }
        match shape(self.raw) {
            Some(b'{') => {
                let values = object(self.raw).map_err(S::Error::custom)?;
                let mut map = serializer.serialize_map(Some(values.len()))?;
                for (key, value) in values {
                    map.serialize_entry(
                        &key,
                        &Literal {
                            raw: value,
                            depth: self.depth + 1,
                        },
                    )?;
                }
                map.end()
            }
            Some(b'[') => {
                let values: Vec<&RawValue> =
                    serde_json::from_str(self.raw.get()).map_err(S::Error::custom)?;
                let mut sequence = serializer.serialize_seq(Some(values.len()))?;
                for value in values {
                    sequence.serialize_element(&Literal {
                        raw: value,
                        depth: self.depth + 1,
                    })?;
                }
                sequence.end()
            }
            _ => self.raw.serialize(serializer),
        }
    }
}
pub(crate) fn serialize_literal<S: Serializer>(
    raw: &RawValue,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    // RawValue's whole-object conversion into Value recognizes private marker
    // keys. Serialize actual maps/sequences so literal user keys stay data.
    Literal { raw, depth: 0 }.serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raw(s: &str) -> Box<RawValue> {
        RawValue::from_string(s.into()).expect("JSON")
    }
    #[test]
    fn unchanged_defaults_remain_absent_at_root_and_inside_edited_records() {
        let original = raw(r#"{"loads":[{"id":"a","location":"old"}],"receipts":{}}"#);
        let before = raw(
            r#"{"loads":[{"id":"a","location":"old","completed":false}],"appointments":[],"receipts":{}}"#,
        );
        let after = raw(
            r#"{"loads":[{"id":"a","location":"new","completed":false}],"appointments":[],"receipts":{}}"#,
        );
        let merged = merge(&original, &before, &after).expect("retained absent defaults");
        let roots = object(&merged).expect("roots");
        assert!(!roots.contains_key("appointments"));
        let loads: Vec<&RawValue> = serde_json::from_str(roots["loads"].get()).expect("loads");
        let load = object(loads[0]).expect("load");
        assert!(!load.contains_key("completed"));
        assert_eq!(load["location"].get(), r#""new""#);
        let changed = raw(
            r#"{"loads":[{"id":"a","location":"new","completed":true}],"appointments":[{"id":"actual"}],"receipts":{}}"#,
        );
        let merged = merge(&original, &before, &changed).expect("actual changed defaults");
        let roots = object(&merged).expect("roots");
        assert_eq!(roots["appointments"].get(), r#"[{"id":"actual"}]"#);
        let loads: Vec<&RawValue> = serde_json::from_str(roots["loads"].get()).expect("loads");
        assert_eq!(object(loads[0]).expect("load")["completed"].get(), "true");
    }
    #[test]
    fn unrelated_changes_retain_exact_numbers_opaque_keys_and_unknown_fields() {
        let original = raw(
            r#"{"loads":[{"id":"a","location":"old","future":{"n":18446744073709551616001,"d":1.00000000000000000001,"$serde_json::private::RawValue":"keep"},"unknown_proof":false}],"receipts":{}}"#,
        );
        let before = raw(
            r#"{"loads":[{"id":"a","location":"old","future":{"n":1.8446744073709552e22,"d":1.0,"$serde_json::private::RawValue":"keep"}}],"receipts":{}}"#,
        );
        let after = raw(
            r#"{"loads":[{"id":"a","location":"new","future":{"n":1.8446744073709552e22,"d":1.0,"$serde_json::private::RawValue":"keep"}}],"receipts":{}}"#,
        );
        let output = merge(&original, &before, &after).expect("retained");
        let roots = object(&output).expect("roots");
        let loads: Vec<&RawValue> = serde_json::from_str(roots["loads"].get()).expect("loads");
        let load = object(loads[0]).expect("load");
        assert_eq!(load["location"].get(), r#""new""#);
        assert_eq!(
            load["future"].get(),
            object(
                serde_json::from_str::<Vec<&RawValue>>(
                    object(&original).expect("roots")["loads"].get()
                )
                .expect("loads")[0]
            )
            .expect("load")["future"]
                .get()
        );
        assert_eq!(load["unknown_proof"].get(), "false");
    }
    #[test]
    fn moved_duplicate_id_records_and_identical_typed_records_are_not_deduplicated() {
        let original = raw(
            r#"{"items":[{"id":"same","n":1,"exact":1.00000000000000000001},{"id":"same","n":2,"exact":2.00000000000000000001},{"id":"same","n":2,"exact":2.00000000000000000002}],"receipts":{}}"#,
        );
        let before = raw(
            r#"{"items":[{"id":"same","n":1,"exact":1.0},{"id":"same","n":2,"exact":2.0},{"id":"same","n":2,"exact":2.0}],"receipts":{}}"#,
        );
        let after = raw(
            r#"{"items":[{"id":"same","n":2,"exact":2.0},{"id":"same","n":1,"exact":1.0},{"id":"same","n":2,"exact":2.0}],"receipts":{}}"#,
        );
        let output = merge(&original, &before, &after).expect("retained");
        let result = object(&output).expect("root");
        let records: Vec<&RawValue> = serde_json::from_str(result["items"].get()).expect("items");
        assert_eq!(records.len(), 3);
        assert!(records[0].get().contains("2.00000000000000000001"));
        assert!(records[1].get().contains("1.00000000000000000001"));
        assert!(records[2].get().contains("2.00000000000000000002"));
    }
    #[test]
    fn appended_and_truncated_history_retains_each_exact_original_in_the_unchanged_prefix() {
        let original = raw(
            r#"{"items":[{"id":"same","n":1.00000000000000000001,"proof":"first"},{"id":"same","n":1.00000000000000000002,"proof":"second"},{"flag":false,"proof":"third"}],"receipts":{}}"#,
        );
        let before = raw(
            r#"{"items":[{"id":"same","n":1.0},{"id":"same","n":1.0},{"flag":false}],"receipts":{}}"#,
        );
        let originals: Vec<&RawValue> =
            serde_json::from_str(object(&original).expect("roots")["items"].get())
                .expect("originals");
        for after in [
            r#"{"items":[{"id":"same","n":1.0},{"id":"same","n":1.0},{"flag":false},{"id":"new","n":2.0}],"receipts":{}}"#,
            r#"{"items":[{"id":"same","n":1.0},{"id":"same","n":1.0}],"receipts":{}}"#,
        ] {
            let after = raw(after);
            let output = merge(&original, &before, &after).expect("retained prefix");
            let items: Vec<&RawValue> =
                serde_json::from_str(object(&output).expect("roots")["items"].get())
                    .expect("items");
            for (item, expected) in items.iter().zip(&originals) {
                assert_eq!(item.get(), expected.get());
            }
            if items.len() == 4 {
                assert_eq!(items[3].get(), r#"{"id":"new","n":2.0}"#);
            }
        }
    }
    #[test]
    fn explicit_known_field_removal_and_new_record_do_not_inherit_unrelated_evidence() {
        let original =
            raw(r#"{"items":[{"id":"a","known":"remove","unknown":"retain"}],"receipts":{}}"#);
        let before = raw(r#"{"items":[{"id":"a","known":"remove"}],"receipts":{}}"#);
        let after = raw(r#"{"items":[{"id":"a"}],"receipts":{}}"#);
        let out = merge(&original, &before, &after).expect("changed");
        let root = object(&out).expect("root");
        let items: Vec<&RawValue> = serde_json::from_str(root["items"].get()).expect("items");
        let item = object(items[0]).expect("item");
        assert!(!item.contains_key("known"));
        assert_eq!(item["unknown"].get(), r#""retain""#);
        let after = raw(r#"{"items":[{"id":"b"}],"receipts":{}}"#);
        let out = merge(&original, &before, &after).expect("new identity");
        assert!(!out.get().contains("unknown"));
    }
    #[test]
    fn old_receipts_are_immutable_and_new_literal_keys_preserve_original_raw_values() {
        let original = raw(r#"{"receipts":{"literal/\"雪":{"response":1.00000000000000000001}}}"#);
        let before = raw(r#"{"receipts":{"literal/\"雪":{"response":1.0}}}"#);
        let after =
            raw(r#"{"receipts":{"literal/\"雪":{"response":1.0},"new":{"response":true}}}"#);
        let out = merge(&original, &before, &after).expect("append");
        assert!(out.get().contains("1.00000000000000000001"));
        for after in [
            r#"{"receipts":{}}"#,
            r#"{"receipts":{"literal/\"雪":{"response":false}}}"#,
        ] {
            assert!(merge(&original, &before, &raw(after)).is_err());
        }
    }
    #[test]
    fn edited_and_reordered_unique_records_keep_their_own_unknown_evidence() {
        let original = raw(
            r#"{"items":[{"id":"a","name":"A","unknown":101},{"id":"b","name":"B","unknown":202}],"receipts":{}}"#,
        );
        let before =
            raw(r#"{"items":[{"id":"a","name":"A"},{"id":"b","name":"B"}],"receipts":{}}"#);
        let after =
            raw(r#"{"items":[{"id":"b","name":"changed"},{"id":"a","name":"A"}],"receipts":{}}"#);
        let out = merge(&original, &before, &after).expect("preserved");
        let root = object(&out).expect("root");
        let items: Vec<&RawValue> = serde_json::from_str(root["items"].get()).expect("items");
        assert_eq!(object(items[0]).expect("B")["unknown"].get(), "202");
        assert_eq!(object(items[1]).expect("A")["unknown"].get(), "101");
    }
    #[test]
    fn literal_private_marker_objects_render_without_interpretation_and_store_exact_numbers() {
        let raw = raw(
            r#"{"$serde_json::private::RawValue":"not JSON","nested":{"$serde_json::private::Number":"literal"},"precise":1.00000000000000000001}"#,
        );
        let literal = Literal {
            raw: &raw,
            depth: 0,
        };
        let value = serde_json::to_value(&literal).expect("literal Value rendering");
        assert_eq!(value["$serde_json::private::RawValue"], "not JSON");
        assert_eq!(value["nested"]["$serde_json::private::Number"], "literal");
        let stored = serde_json::value::to_raw_value(&literal).expect("literal raw storage");
        assert_eq!(
            stored.get(),
            raw.get(),
            "raw numbers must not pass through f64"
        );
    }
}
