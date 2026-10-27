//! Shared terminal projections and cursor semantics for memory and indexed SQL reads.
use crate::{auth::Principal, domain::Workspace, finance::FinanceReadIndex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Billing,
    Payables,
    Integrations,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Balance {
    #[default]
    All,
    Open,
    Overdue,
    Credits,
    Zero,
    Unknown,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    All,
    Outbound,
    Inbound,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub view: View,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    pub balance: Option<Balance>,
    pub attention: Option<bool>,
    pub direction: Option<Direction>,
}
impl Query {
    pub(crate) fn validate(&self) -> Result<(usize, String), String> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err("Console page limit must be between 1 and 100".into());
        }
        let q = self.q.as_deref().unwrap_or_default().trim().to_lowercase();
        if q.len() > 200 {
            return Err("Console search must contain at most 200 bytes".into());
        }
        if self.view == View::Integrations && self.balance.is_some()
            || self.view != View::Integrations
                && (self.attention.is_some() || self.direction.is_some())
            || self.view == View::Payables && self.balance == Some(Balance::Overdue)
        {
            return Err("Console filters do not match the requested view".into());
        }
        if self.cursor.as_ref().is_some_and(|raw| raw.len() > 2048) {
            return Err("Invalid console page cursor".into());
        }
        Ok((limit, q))
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    pub(crate) version: u8,
    pub(crate) scope: String,
    pub(crate) after: String,
}

// Search exposed values, never serialized field names, authentication, payloads
// or financial counterparties' private contact/payment instructions.
fn matches(value: &Value, q: &str) -> bool {
    match value {
        Value::String(s) => s.to_lowercase().contains(q),
        Value::Number(n) => n.to_string().contains(q),
        Value::Array(a) => a.iter().any(|v| matches(v, q)),
        Value::Object(m) => m.values().any(|v| matches(v, q)),
        _ => false,
    }
}
fn balance_matches(row: &Value, filter: Balance, view: View) -> bool {
    let balance = if view == View::Billing {
        &row["totals"]["balance_cents"]
    } else {
        &row["balance_cents"]
    }
    .as_i64();
    match filter {
        Balance::All => true,
        Balance::Open => balance.is_some_and(|v| v > 0),
        Balance::Overdue => {
            balance.is_some_and(|v| v > 0)
                && row["totals"]["overdue_days"]
                    .as_i64()
                    .is_some_and(|v| v > 0)
        }
        Balance::Credits => balance.is_some_and(|v| v < 0),
        Balance::Zero => balance == Some(0),
        Balance::Unknown => balance.is_none(),
    }
}
fn formatted_minor(map: &serde_json::Map<String, Value>, minor: i64) -> String {
    map.get("currency")
        .and_then(Value::as_str)
        .and_then(|code| stateset_freight_commerce::parse_currency(code).ok())
        .map_or_else(
            || stateset_freight_commerce::format_usd(minor),
            |currency| stateset_freight_commerce::format_money(minor, currency),
        )
}
fn money_matches(value: &Value, q: &str) -> bool {
    match value {
        Value::Object(m) => m.iter().any(|(k, v)| {
            (k.ends_with("_cents")
                && v.as_i64()
                    .is_some_and(|n| formatted_minor(m, n).to_lowercase().contains(q)))
                || money_matches(v, q)
        }),
        _ => false,
    }
}
pub(crate) type ConsoleRow = (String, &'static str, Value);
pub(crate) fn identity_digest<'a>(keys: impl Iterator<Item = &'a str>) -> String {
    let encoded = keys
        .map(|key| hex::encode(key.as_bytes()))
        .collect::<Vec<_>>()
        .join(":");
    format!("{:x}", Sha256::digest(encoded.as_bytes()))
}
pub(crate) fn scope(
    tenant: &str,
    principal: &Principal,
    query: &Query,
    limit: usize,
    q: &str,
    identities: &str,
) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!([
                tenant,
                principal.subject,
                query.view,
                limit,
                q,
                query.balance.unwrap_or_default(),
                query.attention.unwrap_or(false),
                query.direction.unwrap_or_default(),
                identities
            ]))
            .map_err(|_| "Internal storage error")?
        )
    ))
}
impl View {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Billing => "billing",
            Self::Payables => "payables",
            Self::Integrations => "integrations",
        }
    }
}
fn search_values(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.to_lowercase()),
        Value::Number(n) => out.push(n.to_string()),
        Value::Array(a) => {
            for v in a {
                search_values(v, out)
            }
        }
        Value::Object(m) => {
            for (k, v) in m.clone() {
                search_values(&v, out);
                if k.ends_with("_cents") {
                    if let Some(n) = v.as_i64() {
                        out.push(formatted_minor(m, n));
                    }
                }
            }
        }
        _ => {}
    }
}
#[derive(Serialize)]
pub(crate) struct IndexRow {
    pub(crate) view: &'static str,
    pub(crate) record_key: String,
    pub(crate) collection: &'static str,
    pub(crate) summary: Value,
    pub(crate) search_values: Vec<String>,
    pub(crate) balance: &'static str,
    pub(crate) direction: &'static str,
    pub(crate) attention: bool,
    pub(crate) overdue: bool,
}
pub(crate) struct Projection {
    pub(crate) date: chrono::NaiveDate,
    pub(crate) rows: Vec<IndexRow>,
    pub(crate) finance: Value,
    pub(crate) errors: Value,
}
impl Projection {
    pub(crate) fn build(workspace: &Workspace) -> Self {
        let date = chrono::Utc::now().date_naive();
        let mut result = Self {
            date,
            rows: Vec::new(),
            finance: Value::Null,
            errors: json!({}),
        };
        for view in [View::Billing, View::Payables, View::Integrations] {
            match workspace.console_projection(view) {
                Err(error) => result.errors[view.code()] = json!(error),
                Ok((rows, finance)) => {
                    if view != View::Integrations {
                        result.finance = finance;
                    }
                    let mut projected = Vec::with_capacity(rows.len());
                    for (record_key, collection, summary) in rows {
                        if record_key.is_empty() || record_key.len() > 202 {
                            result.errors[view.code()] = json!("Invalid console record identity");
                            break;
                        }
                        let amount = if view == View::Billing {
                            &summary["totals"]["balance_cents"]
                        } else {
                            &summary["balance_cents"]
                        }
                        .as_i64();
                        let balance = match amount {
                            Some(n) if n > 0 => "open",
                            Some(n) if n < 0 => "credits",
                            Some(_) => "zero",
                            None => "unknown",
                        };
                        let overdue = amount.is_some_and(|n| n > 0)
                            && summary["totals"]["overdue_days"]
                                .as_i64()
                                .is_some_and(|n| n > 0);
                        let direction = if collection == "outbox" {
                            "outbound"
                        } else if collection == "inbox" {
                            "inbound"
                        } else {
                            "all"
                        };
                        let attention = summary["status"]
                            .as_str()
                            .is_some_and(|s| matches!(s, "retry" | "held" | "failed"));
                        let mut values = Vec::new();
                        search_values(&summary, &mut values);
                        projected.push(IndexRow {
                            view: view.code(),
                            record_key,
                            collection,
                            summary,
                            search_values: values,
                            balance,
                            direction,
                            attention,
                            overdue,
                        });
                    }
                    if result.errors.get(view.code()).is_none() {
                        result.rows.extend(projected);
                    }
                }
            }
        }
        result
    }
}
impl Workspace {
    pub fn console_page(
        &self,
        tenant: &str,
        principal: &Principal,
        query: Query,
    ) -> Result<Value, String> {
        let (limit, q) = query.validate()?;
        let (mut rows, finance) = self.console_projection(query.view)?;
        rows.retain(|(_, collection, row)| {
            (query.view != View::Integrations
                || match query.direction.unwrap_or_default() {
                    Direction::All => true,
                    Direction::Outbound => *collection == "outbox",
                    Direction::Inbound => *collection == "inbox",
                })
                && (q.is_empty() || matches(row, &q) || money_matches(row, &q))
                && (query.view == View::Integrations
                    || balance_matches(row, query.balance.unwrap_or_default(), query.view))
                && (!query.attention.unwrap_or(false)
                    || row["status"]
                        .as_str()
                        .is_some_and(|s| matches!(s, "retry" | "held" | "failed")))
        });
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        if rows.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err("Duplicate console record identity".into());
        }
        let identities = identity_digest(rows.iter().map(|r| r.0.as_str()));
        let scope = scope(tenant, principal, &query, limit, &q, &identities)?;
        let start = if let Some(raw) = query.cursor {
            if raw.len() > 2048 {
                return Err("Invalid console page cursor".into());
            }
            let cursor: Cursor = hex::decode(raw)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .ok_or("Invalid console page cursor")?;
            if cursor.version != 2 || cursor.scope != scope {
                return Err("Console page cursor does not match these records or filters".into());
            }
            rows.iter()
                .position(|r| r.0 == cursor.after)
                .ok_or("Console page cursor anchor is missing")?
                + 1
        } else {
            0
        };
        let selected = &rows[start..rows.len().min(start + limit)];
        let next = if start + selected.len() < rows.len() {
            Some(hex::encode(
                serde_json::to_vec(&Cursor {
                    version: 2,
                    scope,
                    after: selected.last().ok_or("Invalid console page")?.0.clone(),
                })
                .map_err(|_| "Internal storage error")?,
            ))
        } else {
            None
        };
        let mut page = json!({"tenant_id":tenant,"view":query.view,"mode":"console","limit":limit,"total":rows.len(),"next_cursor":next,
            "totals_scope":"workspace","finance":finance,"bills":[],"settlements":[],"outbox":[],"inbox":[]});
        for (_, key, row) in selected {
            page[key]
                .as_array_mut()
                .ok_or("Internal storage error")?
                .push(row.clone());
        }
        Ok(page)
    }
    pub(crate) fn console_projection(
        &self,
        view: View,
    ) -> Result<(Vec<ConsoleRow>, Value), String> {
        let mut rows = Vec::new();
        let mut finance = Value::Null;
        if view != View::Integrations {
            let index = FinanceReadIndex::new(self);
            let summary = index.summary()?;
            finance = json!({"currency":"USD","currencies":summary["currencies"],"receivable_cents":summary["receivable_cents"],"overdue_cents":summary["overdue_cents"],"prepared_payable_cents":summary["prepared_payable_cents"],"customer_credit_cents":summary["customer_credit_cents"],"carrier_credit_cents":summary["carrier_credit_cents"]});
            if view == View::Billing {
                for b in &self.bills {
                    let row = json!({"id":b.id,"number":b.number,"load_id":b.load_id,"customer_id":b.customer_id,
                        "snapshot":{"amount_cents":b.snapshot.amount_cents,"currency":b.snapshot.currency,"load_reference":b.snapshot.load_reference},
                        "status":b.status,"version":b.version,"issued_at":b.issued_at,"due_date":b.due_date,"totals":index.bill_totals(b)?});
                    rows.push((b.id.clone(), "bills", row));
                }
            } else {
                for s in &self.settlements {
                    let mut row = json!({"id":s.id,"load_id":s.load_id,"carrier_id":s.carrier_id,
                        "status":s.status,"version":s.version,"created_at":s.created_at,"total_amount_cents":index.settlement_total(s)?,
                        "balance_cents":index.settlement_balance(&s.id)?,"credit_application_cents":index.carrier_credit_effect(&s.id)?,"credit_balance_cents":crate::finance::credit_balance(index.settlement_balance(&s.id)?)?});
                    // Settlements follow their load's currency; USD rows keep
                    // their historical shape without the field.
                    if let Some(load) = self.loads.iter().find(|l| l.id == s.load_id) {
                        if !crate::domain::is_default_currency(&load.currency) {
                            row["currency"] = json!(load.currency);
                        }
                    }
                    rows.push((s.id.clone(), "settlements", row));
                }
            }
        } else {
            {
                for o in &self.outbox {
                    rows.push((format!("0:{}",o.id), "outbox", json!({"id":o.id,"load_id":o.load_id,"destination":o.destination,
                        "status":o.status,"attempts":o.attempts,"total_attempts":o.total_attempts,"transmission":o.transmission,
                        "next_attempt_at":o.next_attempt_at,"leased_until":o.leased_until,"last_error":o.last_error,"direction":"Outbound"})));
                }
            }
            {
                for i in &self.inbox {
                    rows.push((format!("1:{}",i.id), "inbox", json!({"id":i.id,"document_id":i.document_id,"status":i.status,
                        "attempts":i.attempts,"total_attempts":i.total_attempts,"next_attempt_at":i.next_attempt_at,
                        "leased_until":i.leased_until,"last_error":i.last_error,"direction":"Inbound","destination":"EDI callback"})));
                }
            }
        }

        rows.sort_by(|a, b| a.0.cmp(&b.0));
        if rows.windows(2).any(|p| p[0].0 == p[1].0) {
            return Err("Duplicate console record identity".into());
        }
        Ok((rows, finance))
    }
}

#[cfg(test)]
#[path = "console_catalog_tests.rs"]
mod tests;
