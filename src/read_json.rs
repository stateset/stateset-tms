//! Stream a stored workspace without materializing its whole JSON value tree.
use crate::domain::Workspace;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use std::fmt;

#[derive(Debug)]
pub(crate) enum DecodeError {
    Incompatible,
    Invalid(serde_json::Error),
}
#[derive(Deserialize)]
struct Format<'a> {
    #[serde(default = "missing_marker", borrow, rename = "_freight_format")]
    marker: &'a RawValue,
}
fn missing_marker() -> &'static RawValue {
    RawValue::NULL
}
struct Stored(Workspace);
struct WithoutFormat<M>(M);
impl<'de, M: MapAccess<'de>> MapAccess<'de> for WithoutFormat<M> {
    type Error = M::Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        loop {
            match self.0.next_key::<String>()? {
                Some(key) if key == "_freight_format" => {
                    self.0.next_value::<IgnoredAny>()?;
                }
                Some(key) => {
                    return seed
                        .deserialize(de::value::StringDeserializer::<M::Error>::new(key))
                        .map(Some)
                }
                None => return Ok(None),
            }
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.next_value_seed(seed)
    }
}
impl<'de> Deserialize<'de> for Stored {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct WorkspaceVisitor;
        impl<'de> Visitor<'de> for WorkspaceVisitor {
            type Value = Stored;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a versioned workspace object")
            }
            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Stored, M::Error> {
                Workspace::deserialize(de::value::MapAccessDeserializer::new(WithoutFormat(map)))
                    .map(Stored)
            }
        }
        deserializer.deserialize_map(WorkspaceVisitor)
    }
}
pub(crate) fn check_format(raw: &str, format: i32) -> Result<(), DecodeError> {
    // Check compatibility before interpreting any business fields, matching the
    // existing Value decoder's precedence for a future/missing/wrong marker.
    if !raw.trim_start().starts_with('{') {
        serde_json::from_str::<IgnoredAny>(raw).map_err(DecodeError::Invalid)?;
        return Err(DecodeError::Incompatible);
    }
    let header: Format<'_> = serde_json::from_str(raw).map_err(DecodeError::Invalid)?;
    if header.marker.get() != format.to_string() {
        return Err(DecodeError::Incompatible);
    }
    Ok(())
}
pub(crate) fn decode(raw: &str, format: i32) -> Result<Workspace, DecodeError> {
    check_format(raw, format)?;
    serde_json::from_str::<Stored>(raw)
        .map(|stored| stored.0)
        .map_err(DecodeError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn legacy(mut value: Value) -> Result<Value, &'static str> {
        if value["_freight_format"] != json!(17) {
            return Err("format");
        }
        if let Some(map) = value.as_object_mut() {
            map.remove("_freight_format");
        }
        serde_json::from_value::<Workspace>(value)
            .map(|w| json!(w))
            .map_err(|_| "storage")
    }
    fn streamed(raw: &str) -> Result<Value, &'static str> {
        decode(raw, 17).map(|w| json!(w)).map_err(|e| match e {
            DecodeError::Incompatible => "format",
            DecodeError::Invalid(_) => "storage",
        })
    }
    #[test]
    fn streaming_preserves_complete_records_quarantine_and_existing_error_boundaries() {
        let mut w = crate::domain::demo();
        w.reconcile_operations(chrono::Utc::now());
        let mut base = json!(w);
        base["_freight_format"] = json!(17);
        let mut variants = vec![base.clone()];
        for cases in [
            Value::Null,
            json!({"future":"retain exactly"}),
            json!([{"unsupported_case":{"original":[1,"two",null]}}]),
        ] {
            let mut v = base.clone();
            v["operations_cases"] = cases;
            variants.push(v);
        }
        let mut future_case = base.clone();
        future_case["operations_cases"][0]["future_evidence"] = json!({"private":"preserve"});
        variants.push(future_case);
        for policy in [
            json!({"version":2,"tracking_stale_minutes":90,"future_policy":["retain"]}),
            json!(["malformed-policy"]),
        ] {
            let mut v = base.clone();
            v["operations_policy"] = policy;
            variants.push(v);
        }
        let mut unknown = base.clone();
        unknown["future_financial_obligation"] = json!({"amount_cents":10001});
        variants.push(unknown);
        let mut invalid = base.clone();
        invalid["loads"] = json!([{"missing":"business fields"}]);
        variants.push(invalid.clone());
        for marker in [
            json!(18),
            json!(17.0),
            json!("17"),
            Value::Null,
            json!({"$serde_json::private::RawValue":"17"}),
        ] {
            let mut v = invalid.clone();
            v["_freight_format"] = marker;
            variants.push(v);
        }
        invalid
            .as_object_mut()
            .expect("object")
            .remove("_freight_format");
        variants.push(invalid);
        variants.extend([json!([]), json!(null), json!(true), json!(17)]);
        for value in variants {
            assert_eq!(streamed(&value.to_string()), legacy(value));
        }
        let mut missing = base;
        missing
            .as_object_mut()
            .expect("object")
            .remove("_freight_format");
        let business = missing.to_string();
        let raw = format!(
            "{{{},\"_freight_format\":17}}",
            &business[1..business.len() - 1]
        );
        assert_eq!(
            streamed(&raw),
            Ok(missing),
            "metadata may appear after every business field"
        );
    }
    #[test]
    fn malformed_json_and_duplicate_markers_cannot_be_interpreted_as_a_snapshot() {
        for raw in [
            "{",
            "[",
            "{\"_freight_format\":17,}",
            "{\"_freight_format\":17,\"_freight_format\":17}",
            "{\"_freight_format\":null,\"_freight_format\":17}",
        ] {
            assert!(matches!(decode(raw, 17), Err(DecodeError::Invalid(_))));
        }
    }
}
