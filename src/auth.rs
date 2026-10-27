use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Operations,
    Finance,
    Integration,
    Admin,
    Carrier,
    Customer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Read,
    PortalRead,
    CarrierRespond,
    CarrierReport,
    CarrierRelease,
    CustomerQuoteRespond,
    OperationsWrite,
    OperationsConfigure,
    FinanceWrite,
    DocumentsWrite,
    ChargesWrite,
    IntegrationsReceive,
    IntegrationsRetry,
    SaveRecover,
}
impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "workspace.read",
            Self::PortalRead => "portal.read",
            Self::CarrierRespond => "portal.carrier.respond",
            Self::CarrierReport => "portal.carrier.report",
            Self::CarrierRelease => "portal.carrier.release",
            Self::CustomerQuoteRespond => "portal.customer.quote.respond",
            Self::OperationsWrite => "operations.write",
            Self::OperationsConfigure => "operations.configure",
            Self::FinanceWrite => "finance.write",
            Self::DocumentsWrite => "documents.write",
            Self::ChargesWrite => "charges.write",
            Self::IntegrationsReceive => "integrations.receive",
            Self::IntegrationsRetry => "integrations.retry",
            Self::SaveRecover => "saves.recover",
        }
    }
}
impl Role {
    pub fn allows(self, capability: Capability) -> bool {
        use Capability::*;
        if capability == PortalRead {
            return matches!(self, Self::Carrier | Self::Customer);
        }
        if matches!(capability, CarrierRespond | CarrierReport | CarrierRelease) {
            return self == Self::Carrier;
        }
        if capability == CustomerQuoteRespond {
            return self == Self::Customer;
        }
        if matches!(self, Self::Carrier | Self::Customer) {
            return false;
        }
        if capability == SaveRecover {
            return matches!(
                self,
                Self::Operations | Self::Finance | Self::Integration | Self::Admin
            );
        }
        capability == Read
            || self == Self::Admin
            || matches!(
                (self, capability),
                (
                    Self::Operations,
                    OperationsWrite | IntegrationsRetry | DocumentsWrite | ChargesWrite
                ) | (Self::Finance, FinanceWrite | DocumentsWrite | ChargesWrite)
                    | (Self::Integration, IntegrationsReceive | IntegrationsRetry)
            )
    }
    pub fn capabilities(self) -> Vec<&'static str> {
        [
            Capability::Read,
            Capability::PortalRead,
            Capability::CarrierRespond,
            Capability::CarrierReport,
            Capability::CarrierRelease,
            Capability::CustomerQuoteRespond,
            Capability::OperationsWrite,
            Capability::OperationsConfigure,
            Capability::FinanceWrite,
            Capability::DocumentsWrite,
            Capability::ChargesWrite,
            Capability::IntegrationsReceive,
            Capability::IntegrationsRetry,
            Capability::SaveRecover,
        ]
        .into_iter()
        .filter(|v| self.allows(*v))
        .map(Capability::name)
        .collect()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub tenant_id: String,
    pub subject: String,
    pub role: Role,
    #[serde(default)]
    pub counterparty_id: Option<String>,
}

impl Principal {
    pub fn valid_scope(&self) -> bool {
        if matches!(self.role, Role::Carrier | Role::Customer) {
            self.counterparty_id.as_ref().is_some_and(|v| {
                !v.is_empty()
                    && v.len() <= 100
                    && v.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
            })
        } else {
            self.counterparty_id.is_none()
        }
    }
}

/// String tenant mappings retain the original administrator behavior.
/// New deployments should use explicit subjects and roles.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Credential {
    Legacy(String),
    Scoped(Principal),
}
impl From<&str> for Credential {
    fn from(tenant: &str) -> Self {
        Self::Legacy(tenant.into())
    }
}
impl Credential {
    pub fn principal(&self, credential_actor: &str) -> Principal {
        match self {
            Self::Legacy(tenant) => Principal {
                tenant_id: tenant.clone(),
                subject: credential_actor.into(),
                role: Role::Admin,
                counterparty_id: None,
            },
            Self::Scoped(principal) => principal.clone(),
        }
    }
}

pub fn credentials(json: &str) -> Result<BTreeMap<String, Credential>, String> {
    // Never include the source configuration (which contains secrets) in errors.
    let keys: BTreeMap<String, Credential> =
        serde_json::from_str(json).map_err(|_| "Invalid freight credential configuration")?;
    for (key, value) in &keys {
        let principal = value.principal("legacy");
        if !principal.valid_scope()
            || !(32..=512).contains(&key.len())
            || !key.bytes().all(|v| v.is_ascii_graphic())
            || principal.tenant_id.is_empty()
            || principal.tenant_id.len() > 100
            || !principal
                .tenant_id
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"-_.".contains(&v))
            || principal.subject.trim().is_empty()
            || principal.subject.len() > 100
            || principal.subject.chars().any(char::is_control)
        {
            return Err("Credentials require a 32–512 character token, a valid tenant ID, and a 1–100 character subject".into());
        }
    }
    Ok(keys)
}

/// Match the registered route template, never a client supplied operation name.
/// New endpoints remain denied until added explicitly here.
pub fn route_capability(method: &str, route: &str) -> Option<Capability> {
    use Capability::*;
    if method == "POST" && route == "/v1/save-recovery/resolve" {
        return Some(SaveRecover);
    }
    match (method, route) {
        ("GET", "/v1/console" | "/v1/operations" | "/v1/operations/issues/{id}") => Some(Read),
        ("POST", "/v1/operations/policy") => Some(OperationsConfigure),
        ("POST", "/v1/operations/reconcile" | "/v1/operations/issues/{id}/actions") => {
            Some(OperationsWrite)
        }
        (
            "POST",
            "/v1/loads/{id}/stop-services"
            | "/v1/stop-services/{id}/correct"
            | "/v1/carrier-submissions/{id}/time-corrections",
        ) => Some(OperationsWrite),
        ("POST", "/v1/portal/offers/{id}/stop-services") => Some(CarrierReport),
        (
            "GET",
            "/v1/portal"
            | "/v1/portal/rate-confirmations/{id}/statement"
            | "/v1/portal/submissions/{id}/content"
            | "/v1/portal/customer-quotes/{id}/statement"
            | "/v1/portal/files/{id}/content"
            | "/v1/portal/releases/{id}/statement"
            | "/v1/portal/bills/{id}/statement",
        ) => Some(PortalRead),
        ("POST", "/v1/portal/offers/{id}/response") => Some(CarrierRespond),
        ("POST", "/v1/portal/releases/{id}/response") => Some(CarrierRelease),
        (
            "POST",
            "/v1/release-requests/{id}/portal-publications" | "/v1/release-responses/{id}/apply",
        ) => Some(OperationsWrite),
        (
            "POST",
            "/v1/portal/customer-quotes/{id}/response"
            | "/v1/portal/customer-quotes/{id}/source/refresh",
        ) => Some(CustomerQuoteRespond),
        ("POST", "/v1/quotes/{id}/portal-publications") => Some(OperationsWrite),
        ("POST", "/v1/portal/offers/{id}/tracking" | "/v1/portal/offers/{id}/pod") => {
            Some(CarrierReport)
        }
        ("POST", "/v1/loads/{id}/portal-offers") => Some(OperationsWrite),
        (
            "GET",
            "/v1/workspace"
            | "/v1/loads"
            | "/v1/access"
            | "/v1/history"
            | "/v1/invoices/{id}/approval-evidence",
        ) => Some(Read),
        ("GET", "/v1/carrier-contracts/{id}/statement" | "/v1/quotes/{id}/contract-rates") => {
            Some(Read)
        }
        (
            "POST",
            "/v1/carrier-contracts"
            | "/v1/carrier-contracts/{id}/revise"
            | "/v1/carrier-contracts/{id}/withdraw"
            | "/v1/quotes/{id}/contract-offers"
            | "/v1/quotes/{id}/contract-reprocure",
        ) => Some(OperationsWrite),
        ("GET", "/v1/release-publications/{id}/statement") => Some(Read),
        ("GET", "/v1/rate-confirmations/{id}/statement") => Some(Read),
        (
            "POST",
            "/v1/loads/{id}/rate-confirmations"
            | "/v1/rate-confirmations/{id}/handoffs"
            | "/v1/rate-confirmations/{id}/handoffs/retract"
            | "/v1/rate-confirmations/{id}/handoffs/resolve",
        ) => Some(OperationsWrite),
        ("POST", "/v1/loads/{id}/source/refresh" | "/v1/source-reviews/{id}/resolve") => {
            Some(OperationsWrite)
        }
        ("GET", "/v1/quotes/{id}/statement") => Some(Read),
        (
            "POST",
            "/v1/quotes"
            | "/v1/quotes/{id}/amendments"
            | "/v1/quotes/{id}/revise"
            | "/v1/quotes/{id}/offers"
            | "/v1/quotes/{id}/offers/withdraw"
            | "/v1/quotes/{id}/issue"
            | "/v1/quotes/{id}/decision"
            | "/v1/quotes/{id}/book"
            | "/v1/quotes/{id}/reconfirm"
            | "/v1/quotes/{id}/reprocure",
        ) => Some(OperationsWrite),
        ("POST", "/v1/loads/{id}/charges" | "/v1/loads/{id}/cancellation-fees") => {
            Some(ChargesWrite)
        }
        ("POST", "/v1/cancellation-fees/{id}/review" | "/v1/cancellation-fees/{id}/reverse") => {
            Some(FinanceWrite)
        }
        ("POST", "/v1/charges/{id}/review" | "/v1/charges/{id}/void") => Some(FinanceWrite),
        (
            "POST",
            "/v1/loads/{id}/carrier-adjustments"
            | "/v1/carrier-adjustments/{id}/review"
            | "/v1/carrier-adjustments/{id}/reverse"
            | "/v1/carrier-documents/{id}/dismiss"
            | "/v1/carrier-documents/{id}/apply-revision",
        ) => Some(FinanceWrite),
        (
            "POST",
            "/v1/loads/{id}/release-requests"
            | "/v1/loads/{id}/appointments"
            | "/v1/appointments/{id}/cancel"
            | "/v1/loads/{id}/stop-events"
            | "/v1/stop-events/{id}/correct"
            | "/v1/release-requests/{id}/confirm"
            | "/v1/release-requests/{id}/retain"
            | "/v1/tender-incidents/{id}/resolve",
        ) => Some(OperationsWrite),
        ("GET", "/v1/bills/{id}/statement" | "/v1/finance/journal.csv") => Some(Read),
        (
            "POST",
            "/v1/bills"
            | "/v1/finance/profile"
            | "/v1/bills/{id}/plan"
            | "/v1/bills/{id}/supplements"
            | "/v1/bills/{id}/issue"
            | "/v1/bills/{id}/void"
            | "/v1/bills/{id}/credits"
            | "/v1/credits/{id}/reverse"
            | "/v1/cash"
            | "/v1/carrier-credit-applications"
            | "/v1/carrier-credit-applications/{id}/reverse"
            | "/v1/cash/{id}/reverse",
        ) => Some(FinanceWrite),
        (
            "GET",
            "/v1/documents/{id}/content"
            | "/v1/documents/{id}/preview"
            | "/v1/loads/{id}/documents",
        ) => Some(Read),
        ("POST", "/v1/loads/{id}/documents") => Some(DocumentsWrite),
        ("POST", "/v1/documents/{id}/shares" | "/v1/document-shares/{id}/revoke") => {
            Some(DocumentsWrite)
        }
        ("POST", "/v1/documents/{id}/review" | "/v1/loads/{id}/pod/review") => Some(FinanceWrite),
        (
            "GET",
            "/v1/sync/orders"
            | "/v1/commerce/orders"
            | "/v1/commerce/orders/{id}"
            | "/v1/outbox/{id}/commerce-review"
            | "/v1/loads/{id}/commerce-shipments"
            | "/v1/loads/{id}/commerce-tracking",
        ) => Some(OperationsWrite),
        (
            "POST",
            "/v1/invoices"
            | "/v1/invoices/{id}/approve"
            | "/v1/invoices/{id}/entry-review"
            | "/v1/invoices/{id}/resolve"
            | "/v1/loads/{id}/settlement",
        ) => Some(FinanceWrite),
        ("POST", "/v1/events") => Some(IntegrationsReceive),
        ("POST", "/v1/inbox/{id}/retry" | "/v1/outbox/{id}/retry") => Some(IntegrationsRetry),
        (
            "POST",
            "/v1/loads"
            | "/v1/loads/{id}/commands"
            | "/v1/loads/{id}/plan"
            | "/v1/carriers"
            | "/v1/carriers/{id}"
            | "/v1/customers"
            | "/v1/customers/{id}"
            | "/v1/drivers"
            | "/v1/drivers/{id}"
            | "/v1/equipment"
            | "/v1/equipment/{id}"
            | "/v1/sync/import"
            | "/v1/commerce/import"
            | "/v1/outbox/{id}/commerce-review"
            | "/v1/loads/{id}/commerce-tracking",
        ) => Some(OperationsWrite),
        _ => None,
    }
}
