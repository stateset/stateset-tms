//! Bounded original-record history. Cursors capture an append-only array window;
//! stop histories also fence changes to the selected shipment's version.
use crate::domain::Workspace;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{postgres::PgRow, Row};

pub(crate) const PAGE_SQL: &str = include_str!("history_page.sql");
pub const CHANGED: &str = "History changed; refresh before loading more";
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Events,
    Appointments,
    Gates,
}
impl Kind {
    pub(crate) fn field(self) -> &'static str {
        match self {
            Self::Events => "events",
            Self::Appointments => "appointments",
            Self::Gates => "stop_events",
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub kind: Kind,
    pub load_id: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    scope: String,
    upper: i64,
    before: i64,
    anchor: Option<String>,
    total: i64,
    load_version: Option<u64>,
}
pub(crate) struct Selection {
    pub kind: Kind,
    pub load_id: Option<String>,
    pub limit: usize,
    scope: String,
    cursor: Option<Cursor>,
}
impl Query {
    pub(crate) fn selection(self, tenant: &str) -> Result<Selection> {
        let limit = self.limit.unwrap_or(25);
        if !(1..=100).contains(&limit) {
            return Err("History page limit must be between 1 and 100".into());
        }
        if self
            .load_id
            .as_ref()
            .is_some_and(|v| v.is_empty() || v.len() > 200)
            || self.kind != Kind::Events && self.load_id.is_none()
        {
            return Err("A valid shipment identity is required for stop history".into());
        }
        let scope = crate::documents::sha256(
            json!([tenant, self.kind, self.load_id])
                .to_string()
                .as_bytes(),
        );
        let cursor = match self.cursor {
            None => None,
            Some(raw) => {
                if raw.len() > 4096 {
                    return Err("Invalid history cursor".into());
                }
                let c: Cursor = hex::decode(raw)
                    .ok()
                    .and_then(|v| serde_json::from_slice(&v).ok())
                    .ok_or("Invalid history cursor")?;
                if c.version != 1
                    || c.scope != scope
                    || c.upper < 0
                    || c.before <= 0
                    || c.before > c.upper
                    || c.total < 0
                    || c.total > c.upper
                    || c.anchor
                        .as_ref()
                        .is_none_or(|v| v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    return Err("History cursor does not match these filters".into());
                }
                Some(c)
            }
        };
        Ok(Selection {
            kind: self.kind,
            load_id: self.load_id,
            limit,
            scope,
            cursor,
        })
    }
}
#[derive(Serialize)]
pub(crate) struct Item {
    position: i64,
    record: Value,
}
impl Selection {
    pub(crate) fn upper(&self) -> Option<i64> {
        self.cursor.as_ref().map(|c| c.upper)
    }
    pub(crate) fn before(&self) -> Option<i64> {
        self.cursor.as_ref().map(|c| c.before)
    }
    fn page(
        &self,
        upper: i64,
        anchor: Option<String>,
        total: i64,
        load_version: Option<u64>,
        newer: bool,
        mut items: Vec<Item>,
    ) -> Result<Value> {
        if let Some(c) = &self.cursor {
            if c.anchor != anchor || c.total != total || c.load_version != load_version {
                return Err(CHANGED.into());
            }
        }
        let more = items.len() > self.limit;
        items.truncate(self.limit);
        let next = if more {
            let before = items.last().ok_or("Internal storage error")?.position;
            Some(hex::encode(
                serde_json::to_vec(&Cursor {
                    version: 1,
                    scope: self.scope.clone(),
                    upper,
                    before,
                    anchor,
                    total,
                    load_version,
                })
                .map_err(|_| "Internal storage error")?,
            ))
        } else {
            None
        };
        Ok(
            json!({"kind":self.kind,"load_id":self.load_id,"total":total,"captured_through":upper,"has_new_records":newer,"items":items,"next_cursor":next}),
        )
    }
    fn decode(&self, raw: &str) -> Result<Value> {
        match self.kind {
            Kind::Events => {
                serde_json::from_str::<crate::domain::Event>(raw).and_then(serde_json::to_value)
            }
            Kind::Appointments => serde_json::from_str::<crate::stops::Appointment>(raw)
                .and_then(serde_json::to_value),
            Kind::Gates => {
                serde_json::from_str::<crate::stops::StopEvent>(raw).and_then(serde_json::to_value)
            }
        }
        .map_err(|_| "Internal storage error".into())
    }
    pub(crate) fn postgres_page(&self, rows: Vec<PgRow>) -> Result<Value> {
        let head = rows.first().ok_or("Internal storage error")?;
        let get_error = |_| "Internal storage error".to_string();
        let marker: bool = head.try_get("marker_valid").map_err(get_error)?;
        if !marker {
            return Err(crate::store::INCOMPATIBLE_STORAGE.into());
        }
        let valid: bool = head.try_get("valid").map_err(get_error)?;
        if !valid {
            return Err("Internal storage error".into());
        }
        let found: i64 = head.try_get("load_count").map_err(get_error)?;
        if found == 0 {
            return Err("Shipment not found".into());
        }
        if found != 1 {
            return Err("Internal storage error".into());
        }
        let raw_version: Option<String> = head.try_get("load_version").map_err(get_error)?;
        let load_version = if self.kind == Kind::Events {
            None
        } else {
            Some(
                raw_version
                    .ok_or("Internal storage error")?
                    .parse::<u64>()
                    .map_err(|_| "Internal storage error")?,
            )
        };
        let upper: i64 = head.try_get("upper_bound").map_err(get_error)?;
        let length: i64 = head.try_get("source_length").map_err(get_error)?;
        if upper > length {
            return Err(CHANGED.into());
        }
        let anchor: Option<String> = head.try_get("anchor").map_err(get_error)?;
        let total: i64 = head.try_get("total").map_err(get_error)?;
        let newer: bool = head.try_get("newer").map_err(get_error)?;
        let mut items = vec![];
        for row in rows {
            let position: Option<i64> = row.try_get("position").map_err(get_error)?;
            if let Some(position) = position {
                let raw: String = row.try_get("record").map_err(get_error)?;
                items.push(Item {
                    position,
                    record: self.decode(&raw)?,
                });
            }
        }
        self.page(upper, anchor, total, load_version, newer, items)
    }
    pub(crate) fn memory_page(&self, w: &Workspace) -> Result<Value> {
        let load = if let Some(id) = &self.load_id {
            let matches: Vec<_> = w.loads.iter().filter(|l| &l.id == id).collect();
            if matches.is_empty() {
                return Err("Shipment not found".into());
            }
            if matches.len() != 1 {
                return Err("Internal storage error".into());
            }
            Some(matches[0])
        } else {
            None
        };
        let (length, ids, identities): (usize, Vec<&str>, Vec<&str>) = match self.kind {
            Kind::Events => (
                w.events.len(),
                w.events.iter().map(|v| v.id.as_str()).collect(),
                w.events.iter().map(|v| v.entity_id.as_str()).collect(),
            ),
            Kind::Appointments => (
                w.appointments.len(),
                w.appointments.iter().map(|v| v.id.as_str()).collect(),
                w.appointments.iter().map(|v| v.load_id.as_str()).collect(),
            ),
            Kind::Gates => (
                w.stop_events.len(),
                w.stop_events.iter().map(|v| v.id.as_str()).collect(),
                w.stop_events.iter().map(|v| v.load_id.as_str()).collect(),
            ),
        };
        if ids.iter().any(|id| id.is_empty()) {
            return Err("Internal storage error".into());
        }
        let upper = self.upper().unwrap_or(length as i64);
        if upper > length as i64 {
            return Err(CHANGED.into());
        }
        let anchor = if upper == 0 {
            None
        } else {
            Some(crate::documents::sha256(ids[upper as usize - 1].as_bytes()))
        };
        let load_version = if self.kind == Kind::Events {
            None
        } else {
            Some(load.ok_or("Shipment not found")?.version)
        };
        let matches = |i: usize| self.load_id.as_ref().is_none_or(|id| identities[i] == id);
        let total = (0..upper as usize).filter(|i| matches(*i)).count() as i64;
        let newer = (upper as usize..length).any(matches);
        let before = self.before().unwrap_or(upper + 1);
        let mut items = vec![];
        for i in (0..upper as usize)
            .rev()
            .filter(|i| (*i as i64 + 1) < before && matches(*i))
            .take(self.limit + 1)
        {
            let record = match self.kind {
                Kind::Events => json!(w.events[i]),
                Kind::Appointments => json!(w.appointments[i]),
                Kind::Gates => json!(w.stop_events[i]),
            };
            items.push(Item {
                position: i as i64 + 1,
                record,
            });
        }
        self.page(upper, anchor, total, load_version, newer, items)
    }
}
