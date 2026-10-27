use crate::{model::View, operations::Scope};
use reqwest::{Client, StatusCode, Url};
use serde_json::Value;

const MAX_RESPONSE: usize = 32 * 1024 * 1024;
#[derive(Clone)]
pub struct Request {
    pub console_catalog: bool,
    pub owner: Option<crate::actions::Owner>,
    pub balance: crate::model::BalanceFilter,
    pub attention: bool,
    pub direction: crate::model::QueueDirection,
    pub generation: u64,
    pub view: View,
    pub search: String,
    pub status: String,
    pub operations_scope: Scope,
    pub load_parties: crate::parties::LoadParties,
    pub cursor: Option<String>,
}
pub struct Snapshot {
    pub identity: Value,
    pub data: Value,
}
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub denied: bool,
}
impl Failure {
    fn message(message: &str) -> Self {
        Self {
            message: message.into(),
            denied: false,
        }
    }
}
pub fn endpoint(origin: &Url, request: &Request) -> Url {
    let mut url = origin.clone();
    url.set_path(match request.view {
        View::Loads => "/v1/loads",
        View::Operations => "/v1/operations",
        _ if request.console_catalog => "/v1/console",
        _ => "/v1/workspace",
    });
    {
        let mut pairs = url.query_pairs_mut();
        match request.view {
            View::Loads | View::Operations => {
                pairs.append_pair("limit", "50");
                if !request.search.is_empty() {
                    pairs.append_pair(
                        if request.view == View::Loads {
                            "q"
                        } else {
                            "search"
                        },
                        &request.search,
                    );
                }
                if let Some(cursor) = &request.cursor {
                    pairs.append_pair("cursor", cursor);
                }
                if request.view == View::Operations {
                    pairs.append_pair("view", request.operations_scope.code());
                }
                if request.view == View::Loads {
                    if !request.status.is_empty() {
                        pairs.append_pair("status", &request.status);
                    }
                    for party in crate::parties::Party::ALL {
                        if let Some(id) = request.load_parties.get(party) {
                            pairs.append_pair(party.field(), id);
                        }
                    }
                }
            }
            _ if request.console_catalog => {
                pairs.append_pair(
                    "view",
                    match request.view {
                        View::Billing => "billing",
                        View::Payables => "payables",
                        _ => "integrations",
                    },
                );
                pairs.append_pair("limit", "50");
                if !request.search.is_empty() {
                    pairs.append_pair("q", &request.search);
                }
                if let Some(cursor) = &request.cursor {
                    pairs.append_pair("cursor", cursor);
                }
                if request.view.financial() {
                    pairs.append_pair("balance", request.balance.code());
                } else {
                    pairs.append_pair(
                        "attention",
                        if request.attention { "true" } else { "false" },
                    );
                    pairs.append_pair("direction", request.direction.code());
                }
            }
            _ => {
                pairs.append_pair("history", "deferred");
            }
        }
    }
    url
}
async fn get(client: &Client, url: Url) -> Result<Value, Failure> {
    let response = client
        .get(url)
        .header("x-freight-approval-details", "deferred")
        .send()
        .await
        .map_err(|error| {
            Failure::message(if error.is_timeout() {
                "API timed out. The previous snapshot may be stale; press r to retry."
            } else {
                "Cannot reach the API. Check the origin and connection; press r to retry."
            })
        })?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(Failure { message: "Access denied. Records cleared. Check your staff credential and organization; press r to retry.".into(), denied: true });
    }
    if !status.is_success() {
        return Err(Failure::message(match status.as_u16() {
            300..=399 => "API redirect refused. Configure the API origin directly.",
            429 | 503 => "API is busy or unavailable. Press r to retry when ready.",
            400 | 422 => "The page or filters were refused. Press r to start from the first page.",
            _ => "API request failed. Press r to retry.",
        }));
    }
    read_json(response, MAX_RESPONSE).await
}
pub async fn read_json(mut response: reqwest::Response, limit: usize) -> Result<Value, Failure> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(Failure::message(
            "Response exceeds the 32 MiB console limit. Use the shipment catalog or web workspace.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Failure::message("API response interrupted. Press r to retry."))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Failure::message("Response exceeds the 32 MiB console limit. Use the shipment catalog or web workspace."));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| Failure::message("API returned invalid JSON. Press r to retry."))
}
pub async fn access(client: &Client, origin: &Url) -> Result<Value, Failure> {
    let mut access = origin.clone();
    access.set_path("/v1/access");
    let identity = get(client, access).await?;
    if identity["tenant_id"].as_str().is_none()
        || !identity["access"]["capabilities"]
            .as_array()
            .is_some_and(|v| v.iter().any(|v| v == "workspace.read"))
    {
        return Err(Failure {
            message: "This console requires staff workspace access. Records cleared.".into(),
            denied: true,
        });
    }
    Ok(identity)
}
pub async fn fetch(client: &Client, origin: &Url, request: &Request) -> Result<Snapshot, Failure> {
    let identity = access(client, origin).await?;
    let mut verified = request.clone();
    verified.console_catalog = identity["features"]["console_catalog"] == true;
    if (!request.view.paged() && verified.console_catalog != request.console_catalog)
        || request.owner.as_ref().is_some_and(|owner| {
            Some(owner) != crate::actions::Owner::from_identity(&identity).as_ref()
        })
    {
        verified.cursor = None;
    }
    let data = get(client, endpoint(origin, &verified)).await?;
    if data
        .get("tenant_id")
        .is_some_and(|tenant| tenant != &identity["tenant_id"])
    {
        return Err(Failure {
            message: "Workspace identity mismatch. Records cleared.".into(),
            denied: true,
        });
    }
    if verified.console_catalog && !request.view.paged() {
        validate_catalog(request.view, &identity, &data)?;
    }
    let root = match request.view {
        View::Loads => "loads",
        View::Operations => "issues",
        View::Billing => "bills",
        View::Payables => "settlements",
        View::Integrations => "outbox",
    };
    if !data[root].is_array() {
        return Err(Failure::message(
            "API response is missing the expected records.",
        ));
    }
    Ok(Snapshot { identity, data })
}

fn validate_catalog(view: View, identity: &Value, data: &Value) -> Result<(), Failure> {
    let expected = match view {
        View::Billing => "billing",
        View::Payables => "payables",
        _ => "integrations",
    };
    let count = ["bills", "settlements", "outbox", "inbox"]
        .iter()
        .try_fold(0usize, |n, key| data[key].as_array().map(|a| n + a.len()));
    let shape = match view {
        View::Billing => ["settlements", "outbox", "inbox"]
            .iter()
            .all(|key| data[key].as_array().is_some_and(Vec::is_empty)),
        View::Payables => ["bills", "outbox", "inbox"]
            .iter()
            .all(|key| data[key].as_array().is_some_and(Vec::is_empty)),
        _ => ["bills", "settlements"]
            .iter()
            .all(|key| data[key].as_array().is_some_and(Vec::is_empty)),
    };
    if data["tenant_id"] != identity["tenant_id"]
        || data["view"] != expected
        || data["limit"] != 50
        || data["totals_scope"] != "workspace"
        || !shape
        || !count.is_some_and(|n| {
            n <= 50
                && data["total"]
                    .as_u64()
                    .is_some_and(|total| total >= n as u64)
        })
        || !(data["next_cursor"].is_null()
            || data["next_cursor"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 2048))
    {
        return Err(Failure::message(
            "API returned an invalid console page. Press r to start from the first page.",
        ));
    }
    Ok(())
}
