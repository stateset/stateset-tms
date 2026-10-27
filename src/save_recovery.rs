//! Resolve an uncertain shared save under the same tenant lock as application.
//! A completed receipt wins; otherwise a durable receipt prevents later copies.
use crate::{
    auth::{route_capability, Principal},
    domain::{self, Receipt},
    store::{fingerprint, Store},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const REQUEST_STOPPED: &str = "Original save was stopped before application";
pub const RECOVERY_FORBIDDEN: &str = "Current role cannot resolve this original save";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoppedRequest {
    pub path: String,
    pub note: String,
    pub stopped_by: String,
    pub stopped_at: DateTime<Utc>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveSave {
    pub request_key: String,
    pub path: String,
    pub body: Value,
    pub note: String,
}
struct Target {
    operation: &'static str,
    entity: String,
    route: &'static str,
    empty_body: bool,
}
fn target(path: &str) -> Result<Target, String> {
    let parts = path.split('/').collect::<Vec<_>>();
    // Literal canonical paths only. No queries, encodings, dot segments or
    // client-supplied operation/capability names enter the receipt namespace.
    if parts.iter().any(|part| {
        part.is_empty()
            || *part == "."
            || *part == ".."
            || part.len() > 200
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    }) {
        return Err("Unsupported original save path".into());
    }
    let (operation, entity, route, empty_body) = match parts.as_slice() {
        ["operations", "issues", id, "actions"] => (
            "operations.case.action",
            *id,
            "/v1/operations/issues/{id}/actions",
            false,
        ),
        ["loads"] => ("load.create", "", "/v1/loads", false),
        ["loads", id, "plan"] => ("load.revise", *id, "/v1/loads/{id}/plan", false),
        ["loads", id, "commands"] => ("load.command", *id, "/v1/loads/{id}/commands", false),
        ["loads", id, "settlement"] => {
            ("settlement.create", *id, "/v1/loads/{id}/settlement", true)
        }
        ["loads", id, "pod", "review"] => ("pod.review", *id, "/v1/loads/{id}/pod/review", false),
        ["commerce", "import"] => ("sync.import", "", "/v1/commerce/import", false),
        ["loads", id, "commerce-tracking"] => (
            "commerce.tracking",
            *id,
            "/v1/loads/{id}/commerce-tracking",
            false,
        ),
        ["sync", "import"] => ("sync.import", "", "/v1/sync/import", false),
        ["invoices"] => ("invoice.create", "", "/v1/invoices", false),
        ["invoices", id, "resolve"] => ("invoice.resolve", *id, "/v1/invoices/{id}/resolve", false),
        ["invoices", id, "approve"] => ("invoice.approve", *id, "/v1/invoices/{id}/approve", false),
        ["invoices", id, "entry-review"] => (
            "invoice.entry_review",
            *id,
            "/v1/invoices/{id}/entry-review",
            false,
        ),
        ["customers"] => ("customer.create", "", "/v1/customers", false),
        ["customers", id] => ("customer.update", *id, "/v1/customers/{id}", false),
        ["carriers"] => ("carrier.create", "", "/v1/carriers", false),
        ["carriers", id] => ("carrier.update", *id, "/v1/carriers/{id}", false),
        ["drivers"] => ("driver.create", "", "/v1/drivers", false),
        ["drivers", id] => ("driver.update", *id, "/v1/drivers/{id}", false),
        ["equipment"] => ("equipment.create", "", "/v1/equipment", false),
        ["equipment", id] => ("equipment.update", *id, "/v1/equipment/{id}", false),
        ["inbox", id, "retry"] => ("inbox.retry", *id, "/v1/inbox/{id}/retry", true),
        ["outbox", id, "commerce-review"] => (
            "commerce.reconcile",
            *id,
            "/v1/outbox/{id}/commerce-review",
            false,
        ),
        ["outbox", id, "retry"] => ("outbox.retry", *id, "/v1/outbox/{id}/retry", true),
        _ => return Err("Unsupported original save path".into()),
    };
    Ok(Target {
        operation,
        entity: entity.into(),
        route,
        empty_body,
    })
}
impl Receipt {
    pub(crate) fn checked_response(&self, signature: &str) -> Result<&Value, String> {
        if self.fingerprint != signature {
            return Err("Idempotency key already used for another request".into());
        }
        if self.stopped.is_some() {
            return Err(REQUEST_STOPPED.into());
        }
        Ok(&self.response)
    }
}
impl Store {
    pub async fn resolve_save(
        &self,
        principal: &Principal,
        input: ResolveSave,
    ) -> Result<Value, String> {
        let target = target(&input.path)?;
        if !route_capability("POST", target.route).is_some_and(|cap| principal.role.allows(cap)) {
            return Err(RECOVERY_FORBIDDEN.into());
        }
        if input.request_key.is_empty()
            || input.request_key.len() > 200
            || !input.request_key.bytes().all(|b| (32..=126).contains(&b))
        {
            return Err(
                "Original request key must contain 1–200 printable ASCII characters".into(),
            );
        }
        domain::required(&input.note, "Resolution note")?;
        if target.empty_body && input.body != json!({}) {
            return Err("Original save path accepts an empty object".into());
        }
        if target.operation == "invoice.approve" && input.body != json!({}) {
            crate::invoice_review::validate_approval_body(&input.body)?;
        }
        let canonical_body = if target.operation == "operations.case.action" {
            let request: crate::operations::CaseInput = serde_json::from_value(input.body.clone())
                .map_err(|_| "Invalid original operations action fields")?;
            if request.action == "force_release" && principal.role != crate::auth::Role::Admin {
                return Err(RECOVERY_FORBIDDEN.into());
            }
            crate::operations::trusted_action(principal, request)
        } else {
            input.body.clone()
        };
        let signature = fingerprint(target.operation, &target.entity, &canonical_body);
        let owner = principal.clone();
        self.update_receipted(&principal.tenant_id,None,None,move |workspace| {
            let principal = &owner;
            if let Some(receipt)=workspace.receipts.get(&input.request_key) {
                if receipt.fingerprint!=signature {return Err("Idempotency key already used for another request".into());}
                return Ok(json!({"data":if let Some(stopped)=&receipt.stopped {
                    json!({"outcome":"stopped","request_key":input.request_key,"cancellation":stopped})
                } else {json!({"outcome":"completed","request_key":input.request_key,"result":receipt.response})},"replayed":true}));
            }
            let stopped=StoppedRequest{path:input.path,note:input.note,stopped_by:principal.subject.clone(),stopped_at:Utc::now()};
            workspace.receipts.insert(input.request_key.clone(),Receipt{fingerprint:signature,response:Value::Null,stopped:Some(stopped.clone())});
            workspace.record(&input.request_key,"request.stopped",format!("Original save {} stopped before application: {}",stopped.path,stopped.note));
            if let Some(event)=workspace.events.last_mut(){event.actor=principal.subject.clone();}
            Ok(json!({"data":{"outcome":"stopped","request_key":input.request_key,"cancellation":stopped},"replayed":false}))
        }).await
    }
}
