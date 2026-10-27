//! Attributes JSON/typed decode/projection costs on the qualified service input.
use stateset_freight::{
    auth::{Principal, Role},
    documents::sha256,
    domain,
    operations::QueueQuery,
};
#[path = "../src/read_json.rs"]
mod read_json;
use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};

fn sample(mut operation: impl FnMut() -> Value) -> Value {
    let expected = operation();
    let mut times = vec![];
    for _ in 0..7 {
        let begin = Instant::now();
        let result = black_box(operation());
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(result, expected);
    }
    times.sort_by(f64::total_cmp);
    json!({"samples_ms":times,"median_ms":times[3],"result_sha256":sha256(expected.to_string().as_bytes())})
}
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("qualified service fixture path");
    let bytes = std::fs::read(path).expect("fixture");
    let fixture: Value = serde_json::from_slice(&bytes).expect("fixture JSON");
    let mut value = fixture["workspace"].clone();
    value["_freight_format"] = json!(17);
    let raw = value.to_string();
    let typed: domain::Workspace = {
        let mut v = value.clone();
        v.as_object_mut()
            .expect("workspace")
            .remove("_freight_format");
        serde_json::from_value(v).expect("legacy decode")
    };
    let checksum = sha256(
        serde_json::to_vec(&typed)
            .expect("canonical workspace")
            .as_slice(),
    );
    let principal = Principal {
        tenant_id: "large".into(),
        subject: "user:capacity".into(),
        role: Role::Operations,
        counterparty_id: None,
    };
    let now = fixture["generated_at"]
        .as_str()
        .expect("clock")
        .parse()
        .expect("UTC");
    let mut results = json!({});
    results["json_value_parse"] = sample(|| {
        let v: Value = serde_json::from_str(black_box(&raw)).expect("wire parse");
        json!({"root_fields":v.as_object().expect("object").len()})
    });
    // Clone the already received Value before timing; this phase isolates the
    // production from_value decoder, not the cost of obtaining another input.
    let mut times = vec![];
    for _ in 0..7 {
        let mut input = value.clone();
        input
            .as_object_mut()
            .expect("workspace")
            .remove("_freight_format");
        let begin = Instant::now();
        let w: domain::Workspace = serde_json::from_value(black_box(input)).expect("typed decode");
        times.push(begin.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(
            sha256(&serde_json::to_vec(&w).expect("workspace")),
            checksum
        );
    }
    times.sort_by(f64::total_cmp);
    results["value_to_workspace"] = json!({"samples_ms":times,"median_ms":times[3]});
    results["legacy_total_decode"] = sample(|| {
        let mut v: Value = serde_json::from_str(black_box(&raw)).expect("wire JSON");
        v.as_object_mut()
            .expect("workspace")
            .remove("_freight_format");
        let w: domain::Workspace = serde_json::from_value(v).expect("workspace");
        json!({"loads":w.loads.len(),"events":w.events.len(),"cases":w.operations_cases.len()})
    });
    results["streamed_total_decode"] = sample(|| {
        let w = read_json::decode(black_box(&raw), 17).unwrap_or_else(|e| match e {
            read_json::DecodeError::Invalid(error) => panic!("{error}"),
            read_json::DecodeError::Incompatible => panic!("format"),
        });
        json!({"loads":w.loads.len(),"events":w.events.len(),"cases":w.operations_cases.len()})
    });
    let streamed = read_json::decode(&raw, 17).expect("streamed");
    assert_eq!(
        serde_json::to_value(streamed).expect("streamed value"),
        serde_json::to_value(&typed).expect("original value")
    );
    results["operations_projection_and_encode"] = sample(|| {
        let output = typed
            .operations_page(QueueQuery::default(), &principal, now)
            .expect("operations");
        let bytes = serde_json::to_vec(&output).expect("response");
        json!({"bytes":bytes.len(),"sha256":sha256(&bytes)})
    });
    println!(
        "{}",
        json!({"schema":1,"optimized":!cfg!(debug_assertions),"fixture_sha256":sha256(&bytes),"workspace_sha256":checksum,"serialized_input_bytes":raw.len(),"counts":fixture["counts"],"scope":"CPU phases on compact JSON from the qualified service fixture; excludes SQL, network and contention","results":results})
    );
}
