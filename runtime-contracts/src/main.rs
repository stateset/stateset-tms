//! Test-only wire signing through the actual sibling callback implementation.
//! Network policy stays in the gateway; this helper performs no HTTP requests.
use std::io::{self, Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut body = String::new();
    io::stdin()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut body)?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    let signature = stateset_edi::callbacks::signature(
        value["secret"]
            .as_str()
            .ok_or("Missing test signing secret")?,
        value["timestamp"]
            .as_i64()
            .ok_or("Missing test timestamp")?,
        value["delivery_id"]
            .as_str()
            .ok_or("Missing test delivery ID")?,
        value["payload"]
            .as_str()
            .ok_or("Missing original callback payload")?,
    )?;
    println!("{signature}");
    Ok(())
}
