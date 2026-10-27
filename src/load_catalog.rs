//! Bounded shipment discovery, independent of whole-workspace history reads.
use crate::domain::{Load, Status, Workspace};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub(crate) const PAGE_QUERY: &str = include_str!("load_catalog_page.sql");

// Keep one canonical statement and at most eight prepared shapes. A supplied
// equality must be visible to PostgreSQL's generic planner: a nullable OR can
// otherwise turn a selective indexed page into a tenant-wide filtered scan.
// Only these fixed predicates are substituted; all seven values remain bound.
static PAGE_QUERIES: LazyLock<[String; 8]> = LazyLock::new(|| {
    std::array::from_fn(|mask| {
        let mut statement = PAGE_QUERY.to_owned();
        for (bit, optional, required) in [
            (1, "($3::text IS NULL OR s.status=$3)", "s.status=$3"),
            (
                2,
                "($4::text IS NULL OR s.customer_id=$4)",
                "s.customer_id=$4",
            ),
            (
                4,
                "($5::text IS NULL OR s.carrier_id=$5)",
                "s.carrier_id=$5",
            ),
        ] {
            if mask & bit != 0 {
                statement = statement.replace(optional, required);
            }
        }
        statement
    })
});

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogQuery {
    pub limit: Option<usize>,
    pub cursor: Option<String>,
    pub status: Option<Status>,
    pub customer_id: Option<String>,
    pub carrier_id: Option<String>,
    pub q: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    scope: String,
    after: String,
}
pub(crate) struct Selection {
    pub limit: usize,
    pub after: String,
    pub status: Option<String>,
    pub customer_id: Option<String>,
    pub carrier_id: Option<String>,
    pub q: String,
    scope: String,
}
impl CatalogQuery {
    pub(crate) fn selection(self, tenant: &str) -> Result<Selection, String> {
        let limit = self.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err("Load page limit must be between 1 and 100".into());
        }
        for value in [&self.customer_id, &self.carrier_id].into_iter().flatten() {
            if value.is_empty() || value.len() > 200 {
                return Err("Catalog identity filter must contain 1–200 bytes".into());
            }
        }
        let q = self.q.unwrap_or_default().trim().to_ascii_lowercase();
        if q.len() > 200 {
            return Err("Load search must contain at most 200 bytes".into());
        }
        let status = self
            .status
            .map(|s| json!(s).as_str().unwrap_or_default().to_string());
        // A cursor is scoped to the tenant and filters, never authorization.
        let scope = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!([
                    tenant,
                    status,
                    self.customer_id,
                    self.carrier_id,
                    q
                ]))
                .map_err(|_| "Internal storage error")?
            )
        );
        let after = match self.cursor {
            None => String::new(),
            Some(raw) => {
                if raw.len() > 4096 {
                    return Err("Invalid load page cursor".into());
                }
                let cursor: Cursor = hex::decode(raw)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .ok_or("Invalid load page cursor")?;
                if cursor.version != 1
                    || cursor.scope != scope
                    || cursor.after.is_empty()
                    || cursor.after.len() > 200
                {
                    return Err("Load page cursor does not match these filters".into());
                }
                cursor.after
            }
        };
        Ok(Selection {
            limit,
            after,
            status,
            customer_id: self.customer_id,
            carrier_id: self.carrier_id,
            q,
            scope,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogStop {
    city: String,
    state: String,
    date: NaiveDate,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogLoad {
    pub id: String,
    reference: String,
    customer_id: String,
    carrier_id: Option<String>,
    status: Status,
    mode: String,
    equipment: String,
    location: String,
    exception: Option<String>,
    version: u64,
    updated_at: DateTime<Utc>,
    origin: CatalogStop,
    destination: CatalogStop,
}
impl From<&Load> for CatalogLoad {
    fn from(load: &Load) -> Self {
        let stop = |s: &crate::domain::Stop| CatalogStop {
            city: s.city.clone(),
            state: s.state.clone(),
            date: s.date,
        };
        Self {
            id: load.id.clone(),
            reference: load.reference.clone(),
            customer_id: load.customer_id.clone(),
            carrier_id: load.carrier_id.clone(),
            status: load.status.clone(),
            mode: load.mode.clone(),
            equipment: load.equipment.clone(),
            location: load.location.clone(),
            exception: load.exception.clone(),
            version: load.version,
            updated_at: load.updated_at,
            origin: stop(&load.origin),
            destination: stop(&load.destination),
        }
    }
}
impl Selection {
    pub(crate) fn page_query(&self) -> &'static str {
        let mask = usize::from(self.status.is_some())
            | (usize::from(self.customer_id.is_some()) << 1)
            | (usize::from(self.carrier_id.is_some()) << 2);
        &PAGE_QUERIES[mask]
    }

    pub(crate) fn page(&self, mut rows: Vec<CatalogLoad>) -> Result<Value, String> {
        let more = rows.len() > self.limit;
        rows.truncate(self.limit);
        let cursor = if more {
            rows.last()
                .map(|row| {
                    serde_json::to_vec(&Cursor {
                        version: 1,
                        scope: self.scope.clone(),
                        after: row.id.clone(),
                    })
                    .map(hex::encode)
                    .map_err(|_| "Internal storage error")
                })
                .transpose()?
        } else {
            None
        };
        Ok(json!({"loads":rows,"next_cursor":cursor,"limit":self.limit}))
    }
    pub(crate) fn memory_page(&self, workspace: &Workspace) -> Result<Value, String> {
        let mut ids = std::collections::BTreeSet::new();
        if workspace
            .loads
            .iter()
            .any(|load| load.id.is_empty() || load.id.len() > 200 || !ids.insert(&load.id))
        {
            return Err("Internal storage error".into());
        }
        let mut rows: Vec<_> = workspace
            .loads
            .iter()
            .filter(|load| {
                load.id > self.after
                    && self
                        .status
                        .as_ref()
                        .is_none_or(|s| json!(load.status).as_str() == Some(s.as_str()))
                    && self
                        .customer_id
                        .as_ref()
                        .is_none_or(|id| id == &load.customer_id)
                    && self
                        .carrier_id
                        .as_ref()
                        .is_none_or(|id| load.carrier_id.as_ref() == Some(id))
                    && [
                        Some(load.id.as_str()),
                        Some(load.reference.as_str()),
                        load.order_number.as_deref(),
                        Some(load.location.as_str()),
                        Some(load.origin.city.as_str()),
                        Some(load.origin.state.as_str()),
                        Some(load.destination.city.as_str()),
                        Some(load.destination.state.as_str()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_ascii_lowercase()
                    .contains(&self.q)
            })
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        self.page(
            rows.into_iter()
                .take(self.limit + 1)
                .map(CatalogLoad::from)
                .collect(),
        )
    }
}

#[cfg(test)]
#[path = "catalog_plan_tests.rs"]
mod plan_tests;
