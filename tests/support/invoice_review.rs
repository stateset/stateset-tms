#![allow(dead_code)]
use serde_json::{json, Value};
use stateset_freight::{domain, store::Store};

pub const CHECKER: &str = "fixture:independent-invoice-checker";
pub async fn body(store: &Store, tenant: &str, invoice: &str) -> Value {
    let control = store
        .read(tenant)
        .await
        .expect("current source")
        .invoice_approval_control(invoice, CHECKER)
        .expect("reviewed invoice");
    store.mutate_authorized((tenant,"fixture:invoice-entry-reviewer"),&domain::id("entry-review"),"invoice.entry_review",invoice,
        json!({"expected_revision":control["revision"],"evidence_reference":"Owned actual invoice source document","note":"Checked source invoice, carrier, exact amount, and current agreed charges"}))
        .await.expect("entry review");
    json!({"expected_revision":control["revision"],"note":"Independently verified the source invoice, agreed charges, and delivery evidence"})
}
pub async fn approve(
    store: &Store,
    tenant: &str,
    key: &str,
    invoice: &str,
) -> Result<Value, String> {
    let request = body(store, tenant, invoice).await;
    store
        .mutate_authorized((tenant, CHECKER), key, "invoice.approve", invoice, request)
        .await
}
