use crate::client::{Failure, Request, Snapshot};
use crate::operations::Scope;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use serde_json::Value;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Loads,
    Operations,
    Billing,
    Payables,
    Integrations,
}
impl View {
    pub const ALL: [Self; 5] = [
        Self::Loads,
        Self::Operations,
        Self::Billing,
        Self::Payables,
        Self::Integrations,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Loads => "Loads",
            Self::Operations => "Operations",
            Self::Billing => "Billing",
            Self::Payables => "Payables",
            Self::Integrations => "Integrations",
        }
    }
    pub fn paged(self) -> bool {
        matches!(self, Self::Loads | Self::Operations)
    }
    pub fn financial(self) -> bool {
        matches!(self, Self::Billing | Self::Payables)
    }
    pub fn headers(self) -> Vec<&'static str> {
        match self {
            Self::Loads => vec!["Reference", "Status", "Origin", "Destination", "Carrier"],
            Self::Operations => vec!["Shipment", "Priority", "Issue", "Owner", "Source deadline"],
            Self::Billing => vec!["Invoice", "Status", "Customer", "Amount", "Balance"],
            Self::Payables => vec!["Shipment", "Status", "Carrier", "Total", "Balance"],
            Self::Integrations => {
                vec!["Shipment", "Status", "Direction", "Destination", "Attempts"]
            }
        }
    }
}
pub fn clean(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control() && !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')).take(4096).collect()
}
pub fn text(value: &Value) -> String {
    match value {
        Value::String(v) => clean(v),
        Value::Number(v) => v.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => "—".into(),
    }
}
pub fn money(value: &Value) -> String {
    value
        .as_i64()
        .map(stateset_freight_commerce::format_usd)
        .unwrap_or_else(|| "—".into())
}
/// Summarize one finance-summary figure across its currency groups. Returns
/// `None` for USD-only tenants (the caller keeps the legacy USD figure) and
/// otherwise each non-zero group, USD first, without any conversion.
pub fn currency_totals(finance: &Value, field: &str) -> Option<String> {
    let mut groups: Vec<(&str, &Value)> = finance["currencies"]
        .as_array()?
        .iter()
        .filter(|g| g[field].as_i64().is_some_and(|v| v != 0))
        .filter_map(|g| Some((g["currency"].as_str()?, &g[field])))
        .collect();
    if groups.iter().all(|(code, _)| *code == "USD") {
        return None;
    }
    groups.sort_by_key(|(code, _)| (*code != "USD", *code));
    Some(
        groups
            .iter()
            .map(|(code, value)| money_in(value, &Value::from(*code)))
            .collect::<Vec<_>>()
            .join(" · "),
    )
}
/// Format minor units in a record's currency. USD (or an absent currency on
/// legacy records) stays bare; other currencies are prefixed with their code
/// and use their own scale, so amounts are never read as USD.
pub fn money_in(value: &Value, currency: &Value) -> String {
    let code = currency.as_str().unwrap_or("USD");
    match (
        value.as_i64(),
        stateset_freight_commerce::parse_currency(code),
    ) {
        (None, _) => "—".into(),
        (Some(minor), Ok(c)) if c.as_str() == "USD" => stateset_freight_commerce::format_usd(minor),
        (Some(minor), Ok(c)) => format!(
            "{} {}",
            c.as_str(),
            stateset_freight_commerce::format_money(minor, c)
        ),
        (Some(_), Err(_)) => "—".into(),
    }
}

fn city(value: &Value) -> String {
    format!("{}, {}", text(&value["city"]), text(&value["state"]))
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueDirection {
    #[default]
    All,
    Outbound,
    Inbound,
}
impl QueueDirection {
    pub fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::All => "All directions",
            Self::Outbound => "Outbound",
            Self::Inbound => "Inbound",
        }
    }
    fn next(self) -> Self {
        match self {
            Self::All => Self::Outbound,
            Self::Outbound => Self::Inbound,
            Self::Inbound => Self::All,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BalanceFilter {
    #[default]
    All,
    Open,
    Overdue,
    Credits,
    Zero,
    Unknown,
}
impl BalanceFilter {
    pub fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Overdue => "overdue",
            Self::Credits => "credits",
            Self::Zero => "zero",
            Self::Unknown => "unknown",
        }
    }

    pub fn title(self, view: View) -> &'static str {
        match self {
            Self::All => "All balances",
            Self::Open => "Open balances",
            Self::Overdue => "Overdue invoices",
            Self::Credits if view == View::Billing => "Refunds due",
            Self::Credits => "Carrier credits",
            Self::Zero => "Zero balances",
            Self::Unknown => "Unknown balances",
        }
    }
    fn next(self, view: View) -> Self {
        match self {
            Self::All => Self::Open,
            Self::Open if view == View::Billing => Self::Overdue,
            Self::Open | Self::Overdue => Self::Credits,
            Self::Credits => Self::Zero,
            Self::Zero => Self::Unknown,
            Self::Unknown => Self::All,
        }
    }
    fn matches(self, row: &Record) -> bool {
        // Use raw signed integer cents, never formatted money or document status.
        match self {
            Self::All => true,
            Self::Open => row.balance_cents.is_some_and(|v| v > 0),
            Self::Overdue => {
                row.balance_cents.is_some_and(|v| v > 0) && row.overdue_days.is_some_and(|v| v > 0)
            }
            Self::Credits => row.balance_cents.is_some_and(|v| v < 0),
            Self::Zero => row.balance_cents == Some(0),
            Self::Unknown => row.balance_cents.is_none(),
        }
    }
}

pub struct Record {
    pub target: Option<crate::actions::Target>,
    pub related_load: Option<String>,
    pub load_parties: crate::parties::LoadParties,
    pub direction: Option<QueueDirection>,
    balance_cents: Option<i64>,
    overdue_days: Option<i64>,
    pub id: String,
    pub cells: Vec<String>,
    pub status: String,
    pub details: Vec<(String, String)>,
}
impl Record {
    fn new(value: &Value, cells: Vec<String>, fields: &[(&str, &str)]) -> Self {
        Self {
            target: None,
            related_load: None,
            load_parties: crate::parties::LoadParties::default(),
            direction: None,
            balance_cents: None,
            overdue_days: None,
            id: text(&value["id"]),
            status: text(&value["status"]),
            cells,
            details: fields
                .iter()
                .map(|(label, field)| ((*label).into(), text(&value[*field])))
                .collect(),
        }
    }
    fn detail(&mut self, label: &str, value: String) {
        self.details.push((label.into(), value));
    }
    pub fn selection_key(&self) -> String {
        match self.direction {
            Some(direction) => format!("{}:{}", direction.title(), self.id),
            None => self.id.clone(),
        }
    }
    pub fn needs_attention(&self) -> bool {
        if self.direction.is_some() {
            return matches!(self.status.as_str(), "retry" | "held" | "failed");
        }
        matches!(
            self.status.as_str(),
            "attention" | "retry" | "held" | "failed"
        ) || self
            .details
            .iter()
            .any(|(k, v)| k == "Exception" && v != "—")
    }
    fn matches(&self, query: &str) -> bool {
        self.cells
            .iter()
            .chain(std::iter::once(&self.id))
            .any(|v| v.to_lowercase().contains(query))
            || self
                .details
                .iter()
                .any(|(_, v)| v.to_lowercase().contains(query))
    }
}
fn related_load(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    (!id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)))
    .then(|| id.to_owned())
}
pub fn records(view: View, data: &Value) -> Vec<Record> {
    let array = |key: &str| data[key].as_array().map(Vec::as_slice).unwrap_or_default();
    // Catalog rows carry a non-USD currency; workspace settlements follow their load.
    let settlement_currency = |v: &Value| -> Value {
        if v["currency"].is_string() {
            return v["currency"].clone();
        }
        array("loads")
            .iter()
            .find(|l| l["id"] == v["load_id"])
            .map_or(Value::Null, |l| l["currency"].clone())
    };
    match view {
        View::Loads => array("loads")
            .iter()
            .map(|v| {
                let mut row = Record::new(
                    v,
                    vec![
                        text(&v["reference"]),
                        text(&v["status"]),
                        city(&v["origin"]),
                        city(&v["destination"]),
                        text(&v["carrier_id"]),
                    ],
                    &[
                        ("Reference", "reference"),
                        ("Status", "status"),
                        ("Customer", "customer_id"),
                        ("Carrier", "carrier_id"),
                        ("Location", "location"),
                        ("Mode", "mode"),
                        ("Equipment", "equipment"),
                        ("Exception", "exception"),
                        ("Version", "version"),
                        ("Updated", "updated_at"),
                    ],
                );
                row.target = crate::actions::Target::from_load(v);
                row.load_parties = crate::parties::LoadParties::from_load(v);
                row.detail(
                    "Pickup",
                    format!("{} · {}", city(&v["origin"]), text(&v["origin"]["date"])),
                );
                row.detail(
                    "Delivery",
                    format!(
                        "{} · {}",
                        city(&v["destination"]),
                        text(&v["destination"]["date"])
                    ),
                );
                row
            })
            .collect(),
        View::Operations => array("issues")
            .iter()
            .map(|v| {
                let signal = &v["signal"];
                let mut row = Record::new(
                    v,
                    vec![
                        text(&v["load_reference"]),
                        format!("P{}", text(&signal["priority"])),
                        text(&signal["title"]),
                        text(&v["owner"]),
                        text(&signal["deadline"]),
                    ],
                    &[
                        ("Shipment", "load_reference"),
                        ("Owner", "owner"),
                        ("Source active", "source_active"),
                        ("Source changed", "source_changed"),
                        ("Deferred", "deferred"),
                        ("Follow-up overdue", "follow_up_overdue"),
                        ("Evidence valid", "evidence_valid"),
                    ],
                );
                row.related_load = related_load(&signal["load_id"]);
                row.target = crate::actions::Target::from_issue(v);
                for (label, key) in [
                    ("Issue", "title"),
                    ("Load ID", "load_id"),
                    ("Priority", "priority"),
                    ("Summary", "summary"),
                    ("Next action", "next_action"),
                    ("Source deadline", "deadline"),
                ] {
                    row.detail(label, text(&signal[key]));
                }
                for (label, key) in [
                    ("Work status", "status"), ("Work version", "version"),
                    ("Follow-up at", "follow_up_at"), ("Deferred until", "deferred_until"),
                ] { row.detail(label, text(&v["case"][key])); }
                row.status = if v["source_active"].as_bool() == Some(false) {
                    "inactive"
                } else if v["deferred"].as_bool() == Some(true) {
                    "deferred"
                } else if signal["priority"].as_u64().is_some_and(|v| v <= 1) {
                    "attention"
                } else {
                    "open"
                }
                .into();
                row
            })
            .collect(),
        View::Billing => array("bills")
            .iter()
            .map(|v| {
                let mut row = Record::new(
                    v,
                    vec![
                        text(&v["number"]),
                        text(&v["status"]),
                        text(&v["customer_id"]),
                        money_in(&v["snapshot"]["amount_cents"], &v["snapshot"]["currency"]),
                        money_in(&v["totals"]["balance_cents"], &v["snapshot"]["currency"]),
                    ],
                    &[
                        ("Invoice", "number"),
                        ("Status", "status"),
                        ("Customer", "customer_id"),
                        ("Shipment", "load_id"),
                        ("Issued", "issued_at"),
                        ("Due date", "due_date"),
                        ("Version", "version"),
                    ],
                );
                row.related_load = related_load(&v["load_id"]);
                row.balance_cents = v["totals"]["balance_cents"].as_i64();
                row.overdue_days = v["totals"]["overdue_days"].as_i64();
                row.detail("Currency", text(&v["snapshot"]["currency"]));
                for (label, key) in [
                    ("Balance", "balance_cents"),
                    ("Received", "received_cents"),
                    ("Credited", "credited_cents"),
                    ("Refund due", "refund_due_cents"),
                ] {
                    row.detail(label, money_in(&v["totals"][key], &v["snapshot"]["currency"]));
                }
                row.detail("Overdue days", text(&v["totals"]["overdue_days"]));
                row
            })
            .collect(),
        View::Payables => array("settlements")
            .iter()
            .map(|v| {
                let mut row = Record::new(
                    v,
                    vec![
                        text(&v["load_id"]),
                        text(&v["status"]),
                        text(&v["carrier_id"]),
                        money_in(&v["total_amount_cents"], &settlement_currency(v)),
                        money_in(&v["balance_cents"], &settlement_currency(v)),
                    ],
                    &[
                        ("Shipment", "load_id"),
                        ("Carrier", "carrier_id"),
                        ("Status", "status"),
                        ("Created", "created_at"),
                        ("Version", "version"),
                    ],
                );
                row.related_load = related_load(&v["load_id"]);
                row.balance_cents = v["balance_cents"].as_i64();
                let currency = settlement_currency(v);
                row.detail("Currency", text(&currency));
                row.detail("Total", money_in(&v["total_amount_cents"], &currency));
                row.detail("Balance", money_in(&v["balance_cents"], &currency));
                row.detail("Carrier credit", money_in(&v["credit_balance_cents"], &currency));
                row
            })
            .collect(),
        View::Integrations => ["outbox", "inbox"]
            .into_iter()
            .flat_map(|key| {
                array(key).iter().map(move |v| {
                    let direction = if key == "outbox" { QueueDirection::Outbound } else { QueueDirection::Inbound };
                    let mut row = Record::new(
                        v,
                        vec![
                            text(
                                &v[if key == "outbox" {
                                    "load_id"
                                } else {
                                    "document_id"
                                }],
                            ),
                            text(&v["status"]),
                            direction.title().into(),
                            if key == "outbox" {
                                text(&v["destination"])
                            } else {
                                "EDI callback".into()
                            },
                            text(&v["total_attempts"]),
                        ],
                        &[
                            ("Shipment", "load_id"),
                            ("EDI document", "document_id"),
                            ("Status", "status"),
                            ("Destination", "destination"),
                            ("Total attempts", "total_attempts"),
                            ("Attempts this cycle", "attempts"),
                            ("Transmission", "transmission"),
                            ("Next attempt", "next_attempt_at"),
                            ("Lease until", "leased_until"),
                            ("Last error", "last_error"),
                        ],
                    );
                    row.related_load = related_load(&v["load_id"]);
                    row.direction = Some(direction);
                    row.detail("Direction", direction.title().into());
                    if row.needs_attention() {
                        row.detail("Recovery", "Review the original record in the web workspace's Integrations view. Queue recovery is read-only in this terminal.".into());
                    }
                    row
                })
            })
            .collect(),
    }
}

#[derive(Clone)]
pub(crate) struct Navigation {
    query: String,
    status_index: usize,
    cursors: Vec<Option<String>>,
    selected_id: Option<String>,
    table: TableState,
    focused_load: Option<String>,
    related_from: Option<View>,
    queue_attention: bool,
    queue_direction: QueueDirection,
    balance_filter: BalanceFilter,
    operations_scope: Scope,
    load_parties: crate::parties::LoadParties,
    page_positions: Vec<crate::pagination::PagePosition>,
}
impl Default for Navigation {
    fn default() -> Self {
        Self {
            query: String::new(),
            status_index: 0,
            cursors: vec![None],
            selected_id: None,
            table: TableState::default(),
            focused_load: None,
            related_from: None,
            queue_attention: false,
            queue_direction: QueueDirection::All,
            balance_filter: BalanceFilter::All,
            operations_scope: Scope::Actionable,
            load_parties: crate::parties::LoadParties::default(),
            page_positions: vec![crate::pagination::PagePosition::default()],
        }
    }
}

pub struct App {
    pub evidence: Option<crate::evidence::Panel>,
    /// A preview requested while a read was in flight; opened when it lands.
    pub evidence_pending: bool,
    pub evidence_rect: Option<ratatui::layout::Rect>,
    pub kitty_graphics: bool,
    pub kitty_image_id: Option<u32>,
    pub color_preview: bool,
    pub image_cell: (u32, u32),
    pub palette: Option<crate::palette::Palette>,
    pub(crate) page_positions: Vec<crate::pagination::PagePosition>,
    pub(crate) navigation: [Navigation; 5],
    pub(crate) selection_anchor: Option<String>,
    pub focused_load: Option<String>,
    pub related_from: Option<View>,
    pub queue_attention: bool,
    pub queue_direction: QueueDirection,
    pub balance_filter: BalanceFilter,
    pub operations_scope: Scope,
    pub load_parties: crate::parties::LoadParties,
    pub stale_after: Duration,
    pub refresh_interval: Option<Duration>,
    pub connection: bool,
    pub recovery_error: Option<String>,
    pub recovery_durable: bool,
    pub action_dialog: Option<crate::actions::Dialog>,
    pub pending: Option<crate::actions::Pending>,
    pub write_ready: Option<crate::actions::WriteJob>,
    pub write_busy: bool,
    pub write_uncertain: bool,
    pub write_lost: bool,
    pub write_notice: Option<String>,
    pub quit_confirm: bool,
    pub action_viewport_ready: bool,
    pub stopping_note: Option<String>,
    pub view: View,
    pub rows: Vec<Record>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub table: TableState,
    pub query: String,
    pub draft: String,
    pub draft_cursor: usize,
    pub searching: bool,
    pub status_index: usize,
    pub next_cursor: Option<String>,
    pub cursors: Vec<Option<String>>,
    pub generation: u64,
    pub loading: Option<u64>,
    pub needs_fetch: bool,
    pub error: Option<String>,
    pub denied: bool,
    pub identity: Value,
    pub finance: Value,
    pub catalog_total: Option<u64>,
    pub mode: String,
    pub updated: Option<Instant>,
    pub details: bool,
    pub detail_scroll: u16,
    pub detail_max: u16,
    pub detail_page: u16,
    pub help: bool,
    pub quit: bool,
}
impl Default for App {
    fn default() -> Self {
        Self {
            evidence: None,
            evidence_pending: false,
            evidence_rect: None,
            kitty_graphics: false,
            kitty_image_id: None,
            color_preview: true,
            image_cell: (8, 16),
            palette: None,
            navigation: std::array::from_fn(|_| Navigation::default()),
            selection_anchor: None,
            focused_load: None,
            related_from: None,
            queue_attention: false,
            queue_direction: QueueDirection::All,
            balance_filter: BalanceFilter::All,
            operations_scope: Scope::Actionable,
            load_parties: crate::parties::LoadParties::default(),
            page_positions: vec![crate::pagination::PagePosition::default()],
            stale_after: Duration::from_secs(60),
            refresh_interval: None,
            connection: false,
            recovery_error: None,
            recovery_durable: false,
            action_dialog: None,
            pending: None,
            write_ready: None,
            write_busy: false,
            write_uncertain: false,
            write_lost: false,
            write_notice: None,
            quit_confirm: false,
            action_viewport_ready: false,
            stopping_note: None,
            view: View::Loads,
            rows: vec![],
            visible: vec![],
            selected: 0,
            table: TableState::default(),
            query: String::new(),
            draft: String::new(),
            draft_cursor: 0,
            searching: false,
            status_index: 0,
            next_cursor: None,
            cursors: vec![None],
            generation: 0,
            loading: None,
            needs_fetch: true,
            error: None,
            denied: false,
            identity: Value::Null,
            finance: Value::Null,
            catalog_total: None,
            mode: String::new(),
            updated: None,
            details: false,
            detail_scroll: 0,
            detail_max: 0,
            detail_page: 1,
            help: false,
            quit: false,
        }
    }
}
pub const STATUSES: [&str; 8] = [
    "",
    "planned",
    "tendered",
    "accepted",
    "dispatched",
    "in_transit",
    "delivered",
    "cancelled",
];
impl App {
    pub fn stale(&self) -> bool {
        self.updated
            .is_some_and(|at| at.elapsed() >= self.stale_after)
            || self.error.is_some() && self.updated.is_some()
    }
    pub fn refresh_pause(&self) -> Option<&'static str> {
        if self.denied || self.error.is_some() {
            Some("connection error; r retries")
        } else if self.write_busy
            || self.write_ready.is_some()
            || self.pending.is_some()
            || self.write_lost
        {
            Some("save recovery")
        } else if self.action_dialog.is_some() || self.searching {
            Some("editing")
        } else if self.evidence.is_some()
            || self.details
            || self.help
            || self.connection
            || self.quit_confirm
            || self.palette.is_some()
        {
            Some("inspecting")
        } else {
            None
        }
    }
    pub fn tick(&mut self, now: Instant) {
        if self.evidence_pending && self.loading.is_none() && !self.needs_fetch {
            self.evidence_pending = false;
            self.open_evidence();
        }
        let Some(interval) = self.refresh_interval else {
            return;
        };
        if self.loading.is_some() || self.needs_fetch || self.refresh_pause().is_some() {
            return;
        }
        if self
            .updated
            .is_some_and(|at| now.saturating_duration_since(at) >= interval)
        {
            // Refresh the current page without replacing the operator's filters or selection.
            // A failed read pauses scheduling until an explicit retry succeeds.
            self.invalidate();
        }
    }
    pub fn catalog(&self) -> bool {
        !self.view.paged() && self.identity["features"]["console_catalog"] == true
    }
    pub fn paged(&self) -> bool {
        self.view.paged() || self.catalog()
    }
    pub fn request(&self) -> Request {
        Request {
            console_catalog: self.identity["features"]["console_catalog"] == true,
            owner: crate::actions::Owner::from_identity(&self.identity),
            balance: self.balance_filter,
            attention: self.queue_attention,
            direction: self.queue_direction,
            generation: self.generation,
            view: self.view,
            search: self.query.clone(),
            status: STATUSES[self.status_index].into(),
            operations_scope: self.operations_scope,
            load_parties: self.load_parties.clone(),
            cursor: self.cursors.last().cloned().flatten(),
        }
    }
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.needs_fetch = true;
    }
    pub(crate) fn reset_page(&mut self) {
        self.page_positions = vec![crate::pagination::PagePosition::default()];
        self.cursors = vec![None];
        self.next_cursor = None;
        self.selected = 0;
        self.table = TableState::default();
        self.selection_anchor = None;
        self.detail_scroll = 0;
    }
    pub fn apply(&mut self, snapshot: Snapshot) {
        let owner = crate::actions::Owner::from_identity(&snapshot.identity);
        if self
            .pending
            .as_ref()
            .is_some_and(|p| Some(&p.owner) != owner.as_ref())
            || self
                .action_dialog
                .as_ref()
                .is_some_and(|d| Some(&d.owner) != owner.as_ref())
        {
            self.write_lost |= self.pending.is_some();
            self.pending = None;
            self.action_dialog = None;
            self.write_ready = None;
            self.stopping_note = None;
            self.write_uncertain = false;
            self.write_notice = None;
        }
        if !self.identity.is_null()
            && (self.identity["tenant_id"] != snapshot.identity["tenant_id"]
                || self.identity["access"]["subject"] != snapshot.identity["access"]["subject"])
        {
            // Never reuse another operator's search, cursor or record anchor. The response
            // was requested with the previous context, so fetch again from a clean page.
            self.clear_access();
            self.identity = snapshot.identity;
            self.denied = false;
            self.error = None;
            self.invalidate();
            return;
        }
        if !self.identity.is_null()
            && (self.identity["features"]["console_catalog"] == true)
                != (snapshot.identity["features"]["console_catalog"] == true)
        {
            // A deployment or rollback changes cursor semantics. Keep filters and
            // private write recovery, but never restore an incompatible page.
            for view in [View::Billing, View::Payables, View::Integrations] {
                let nav = &mut self.navigation[view.index()];
                nav.cursors = vec![None];
                nav.selected_id = None;
                nav.table = TableState::default();
                nav.page_positions = vec![crate::pagination::PagePosition::default()];
            }
            if !self.view.paged() {
                self.reset_page();
                self.rows.clear();
                self.visible.clear();
                self.updated = None;
                self.finance = Value::Null;
                self.catalog_total = None;
                self.identity = snapshot.identity;
                self.invalidate();
                return;
            }
        }
        let old_id = self
            .current()
            .map(Record::selection_key)
            .or_else(|| self.selection_anchor.take());
        self.rows = if self.view == View::Loads && self.focused_load.is_some() {
            let id = self.focused_load.as_deref();
            let matches = snapshot.data["loads"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter(|row| row["id"].as_str() == id)
                        .take(2)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if matches.len() == 1 {
                records(self.view, &serde_json::json!({"loads": [matches[0]]}))
            } else {
                vec![]
            }
        } else {
            records(self.view, &snapshot.data)
        };
        self.identity = snapshot.identity;
        self.finance = snapshot.data["finance"].clone();
        self.catalog_total = snapshot.data["total"].as_u64();
        self.mode = text(&snapshot.data["mode"]);
        self.next_cursor = snapshot.data["next_cursor"].as_str().map(String::from);
        self.updated = Some(Instant::now());
        self.error = None;
        self.denied = false;
        self.filter();
        if let Some(id) = old_id {
            self.selected = self
                .visible
                .iter()
                .position(|i| self.rows[*i].selection_key() == id)
                .unwrap_or(0);
        }
    }
    pub fn finish(&mut self, generation: u64, result: Result<Snapshot, Failure>) {
        self.loading = None;
        if let Err(error) = &result {
            if error.denied {
                self.clear_access();
                self.needs_fetch = false;
                self.error = Some(error.message.clone());
            }
        }
        if generation == self.generation {
            match result {
                Ok(snapshot) => self.apply(snapshot),
                Err(error) => self.error = Some(error.message),
            }
        }
    }
    pub fn clear_access(&mut self) {
        self.evidence = None;
        self.evidence_rect = None;
        self.palette = None;
        self.write_lost |= self.pending.is_some();
        self.pending = None;
        self.action_dialog = None;
        self.write_ready = None;
        self.stopping_note = None;
        self.write_uncertain = false;
        self.write_notice = None;
        self.navigation = std::array::from_fn(|_| Navigation::default());
        self.query.clear();
        self.draft.clear();
        self.draft_cursor = 0;
        self.searching = false;
        self.status_index = 0;
        self.focused_load = None;
        self.related_from = None;
        self.queue_attention = false;
        self.queue_direction = QueueDirection::All;
        self.balance_filter = BalanceFilter::All;
        self.operations_scope = Scope::Actionable;
        self.load_parties = crate::parties::LoadParties::default();
        self.reset_page();
        self.rows.clear();
        self.visible.clear();
        self.identity = Value::Null;
        self.finance = Value::Null;
        self.catalog_total = None;
        self.updated = None;
        self.next_cursor = None;
        self.details = false;
        self.denied = true;
    }
    pub fn filter(&mut self) {
        let query = self.query.to_lowercase();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                (self.paged() || r.matches(&query))
                    && self
                        .focused_load
                        .as_ref()
                        .is_none_or(|id| self.view != View::Loads || &r.id == id)
                    && (self.catalog()
                        || self.view != View::Integrations
                        || ((!self.queue_attention || r.needs_attention())
                            && (self.queue_direction == QueueDirection::All
                                || r.direction == Some(self.queue_direction))))
                    && (self.catalog() || !self.view.financial() || self.balance_filter.matches(r))
            })
            .map(|(i, _)| i)
            .collect();
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
    }
    pub fn current(&self) -> Option<&Record> {
        self.visible
            .get(self.selected)
            .and_then(|i| self.rows.get(*i))
    }
    fn switch(&mut self, index: usize) {
        let view = View::ALL[index % View::ALL.len()];
        if view == self.view {
            return;
        }
        if crate::actions::Owner::from_identity(&self.identity).is_some() && !self.denied {
            self.remember_page_position();
            let mut table = self.table;
            table.select(Some(self.selected));
            self.navigation[self.view.index()] = Navigation {
                query: self.query.clone(),
                status_index: self.status_index,
                cursors: self.cursors.clone(),
                selected_id: self
                    .current()
                    .map(Record::selection_key)
                    .or_else(|| self.selection_anchor.clone()),
                table,
                focused_load: self.focused_load.clone(),
                related_from: self.related_from,
                queue_attention: self.queue_attention,
                queue_direction: self.queue_direction,
                balance_filter: self.balance_filter,
                operations_scope: self.operations_scope,
                load_parties: self.load_parties.clone(),
                page_positions: self.page_positions.clone(),
            };
        }
        self.view = view;
        let navigation = self.navigation[view.index()].clone();
        self.query = navigation.query;
        self.status_index = navigation.status_index;
        self.cursors = navigation.cursors;
        self.selection_anchor = navigation.selected_id;
        self.table = navigation.table;
        self.focused_load = navigation.focused_load;
        self.related_from = navigation.related_from;
        self.queue_attention = navigation.queue_attention;
        self.queue_direction = navigation.queue_direction;
        self.balance_filter = navigation.balance_filter;
        self.operations_scope = navigation.operations_scope;
        self.load_parties = navigation.load_parties;
        self.page_positions = navigation.page_positions;
        self.selected = self.table.selected().unwrap_or(0);
        self.next_cursor = None;
        self.detail_scroll = 0;
        self.rows.clear();
        self.visible.clear();
        self.finance = Value::Null;
        self.catalog_total = None;
        self.updated = None;
        self.details = false;
        self.error = None;
        self.invalidate();
    }
    fn open_related(&mut self) {
        if self.view == View::Loads || self.denied {
            return;
        }
        let Some(id) = self.current().and_then(|row| row.related_load.clone()) else {
            self.write_notice = Some("This record has no supported shipment link.".into());
            return;
        };
        if crate::actions::Owner::from_identity(&self.identity).is_none() {
            self.write_notice =
                Some("Refresh a verified staff identity before opening a shipment.".into());
            return;
        }
        let from = self.view;
        self.switch(View::Loads.index());
        // The catalog search is bounded and may contain substring matches. Only
        // the exact raw linked identity is eligible for selection or actions.
        self.query = id.clone();
        self.status_index = 0;
        self.load_parties = crate::parties::LoadParties::default();
        self.reset_page();
        self.focused_load = Some(id);
        self.related_from = Some(from);
        if self.pending.is_none() && !self.write_lost && !self.write_busy {
            self.write_notice = None;
        }
    }
    fn return_related(&mut self) {
        if let Some(from) = self.related_from {
            self.switch(from.index());
        }
    }
    pub fn paste(&mut self, value: &str) {
        if let Some(palette) = self.palette.as_mut() {
            palette.insert(value);
            return;
        }
        if self.action_dialog.is_some() {
            self.action_paste(value);
            return;
        }
        if self.searching {
            self.insert_search(value);
        }
    }
    fn search_boundary(&self, forward: bool) -> usize {
        let mut boundaries = self.draft.grapheme_indices(true).map(|(i, _)| i);
        if forward {
            boundaries
                .find(|i| *i > self.draft_cursor)
                .unwrap_or(self.draft.len())
        } else {
            boundaries
                .take_while(|i| *i < self.draft_cursor)
                .last()
                .unwrap_or(0)
        }
    }
    fn normalize_search_cursor(&mut self) {
        self.draft_cursor = self
            .draft
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= self.draft_cursor)
            .unwrap_or(self.draft.len());
    }
    fn insert_search(&mut self, value: &str) {
        let value = clean(value);
        let mut bytes = 0;
        for grapheme in value.graphemes(true) {
            if self.draft.len() + bytes + grapheme.len() > 200 {
                break;
            }
            bytes += grapheme.len();
        }
        self.draft.insert_str(self.draft_cursor, &value[..bytes]);
        self.draft_cursor += bytes;
        self.normalize_search_cursor();
    }
    fn clear_filters(&mut self) {
        if !self.has_filters() {
            return;
        }
        self.query.clear();
        self.draft.clear();
        self.draft_cursor = 0;
        self.status_index = 0;
        self.focused_load = None;
        self.related_from = None;
        self.queue_attention = false;
        self.queue_direction = QueueDirection::All;
        self.balance_filter = BalanceFilter::All;
        self.operations_scope = Scope::Actionable;
        self.load_parties = crate::parties::LoadParties::default();
        self.reset_page();
        if self.paged() {
            self.rows.clear();
            self.visible.clear();
            self.updated = None;
            self.invalidate();
        } else {
            self.filter();
        }
    }
    pub fn has_filters(&self) -> bool {
        !self.query.is_empty()
            || self.status_index != 0
            || self.focused_load.is_some()
            || (self.view == View::Loads && self.load_parties.any())
            || (self.view == View::Integrations
                && (self.queue_attention || self.queue_direction != QueueDirection::All))
            || (self.view.financial() && self.balance_filter != BalanceFilter::All)
            || (self.view == View::Operations && self.operations_scope != Scope::Actionable)
    }
    pub(crate) fn refilter_local(&mut self) {
        if self.catalog() {
            self.reset_page();
            self.rows.clear();
            self.visible.clear();
            self.updated = None;
            self.invalidate();
            return;
        }
        let selected = self.current().map(Record::selection_key);
        self.filter();
        self.selected = selected
            .and_then(|key| {
                self.visible
                    .iter()
                    .position(|i| self.rows[*i].selection_key() == key)
            })
            .unwrap_or(0);
        self.table = TableState::default();
        self.detail_scroll = 0;
    }
    pub(crate) fn refresh_read(&mut self, first_page: bool) {
        if first_page {
            if self.cursors.len() > 1 {
                self.rows.clear();
                self.visible.clear();
                self.updated = None;
            }
            self.reset_page();
        } else if self.needs_fetch || self.loading == Some(self.generation) {
            // A read of this context is already queued or running. Coalesce
            // repeated requests without invalidating its eventual response.
            return;
        }
        self.invalidate();
    }
    fn toggle_auto_reads(&mut self) {
        self.refresh_interval = if self.refresh_interval.is_some() {
            None
        } else {
            Some(Duration::from_secs(30))
        };
    }
    pub(crate) fn set_operations_scope(&mut self, scope: Scope) {
        if self.view != View::Operations || self.operations_scope == scope {
            return;
        }
        self.operations_scope = scope;
        self.reset_page();
        self.rows.clear();
        self.visible.clear();
        self.updated = None;
        self.invalidate();
    }
    pub(crate) fn set_status(&mut self, index: usize) {
        if self.view != View::Loads || index >= STATUSES.len() || self.status_index == index {
            return;
        }
        self.status_index = index;
        self.reset_page();
        self.rows.clear();
        self.visible.clear();
        self.updated = None;
        self.invalidate();
    }
    pub(crate) fn set_balance_filter(&mut self, filter: BalanceFilter) {
        if !self.view.financial()
            || (filter == BalanceFilter::Overdue && self.view != View::Billing)
            || self.balance_filter == filter
        {
            return;
        }
        self.balance_filter = filter;
        self.refilter_local();
    }
    fn scroll_key(&mut self, code: KeyCode) {
        self.detail_scroll = match code {
            KeyCode::Down | KeyCode::Char('j') => self.detail_scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.detail_scroll.saturating_sub(1),
            KeyCode::PageDown => self.detail_scroll.saturating_add(self.detail_page),
            KeyCode::PageUp => self.detail_scroll.saturating_sub(self.detail_page),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => self.detail_max,
            _ => self.detail_scroll,
        }
        .min(self.detail_max);
    }
    pub fn key(&mut self, key: KeyEvent) {
        // Any later key supersedes a preview still waiting for its read.
        if key.code != KeyCode::Char('v') {
            self.evidence_pending = false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.palette = None;
            self.evidence = None;
            self.request_quit();
            return;
        }
        if self.evidence_key(key) {
            return;
        }
        if self.palette.is_some() {
            self.palette_key(key);
            return;
        }
        if self.action_key(key) {
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => self.searching = false,
                KeyCode::Enter => {
                    self.searching = false;
                    if self.query == self.draft.trim() {
                        return;
                    }
                    self.query = self.draft.trim().into();
                    self.focused_load = None;
                    self.related_from = None;
                    self.reset_page();
                    if self.paged() {
                        self.rows.clear();
                        self.visible.clear();
                        self.updated = None;
                        self.invalidate();
                    } else {
                        self.filter();
                    }
                }
                KeyCode::Backspace => {
                    let before = self.search_boundary(false);
                    self.draft.drain(before..self.draft_cursor);
                    self.draft_cursor = before;
                    self.normalize_search_cursor();
                }
                KeyCode::Delete => {
                    let after = self.search_boundary(true);
                    self.draft.drain(self.draft_cursor..after);
                    self.normalize_search_cursor();
                }
                KeyCode::Left => self.draft_cursor = self.search_boundary(false),
                KeyCode::Right => self.draft_cursor = self.search_boundary(true),
                KeyCode::Home => self.draft_cursor = 0,
                KeyCode::End => self.draft_cursor = self.draft.len(),
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.draft.clear();
                    self.draft_cursor = 0;
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !c.is_control() =>
                {
                    self.insert_search(&c.to_string())
                }
                _ => {}
            }
            return;
        }
        if self.connection {
            match key.code {
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if key.kind == crossterm::event::KeyEventKind::Press
                        && !key.modifiers.contains(KeyModifiers::ALT)
                    {
                        self.refresh_read(false);
                    }
                }
                KeyCode::Char('r') if key.modifiers.is_empty() => self.refresh_read(true),
                KeyCode::Char('R')
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.toggle_auto_reads()
                }
                KeyCode::Esc | KeyCode::Char('i' | 'q') | KeyCode::Enter => self.connection = false,
                code => self.scroll_key(code),
            }
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?' | 'q') | KeyCode::Enter => self.help = false,
                code => self.scroll_key(code),
            }
            return;
        }
        if self.details {
            match key.code {
                KeyCode::Char('v')
                    if key.modifiers.is_empty()
                        && key.kind == crossterm::event::KeyEventKind::Press =>
                {
                    self.open_evidence()
                }
                KeyCode::Char('o') => self.open_related(),
                KeyCode::Char('b') => self.return_related(),
                KeyCode::Esc | KeyCode::Enter => {
                    self.details = false;
                    self.detail_scroll = 0;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') => {
                    let selected = if matches!(key.code, KeyCode::Left | KeyCode::Char('h')) {
                        self.selected.saturating_sub(1)
                    } else {
                        self.selected
                            .saturating_add(1)
                            .min(self.visible.len().saturating_sub(1))
                    };
                    if selected != self.selected {
                        self.selected = selected;
                        self.detail_scroll = 0;
                    }
                }
                KeyCode::Char('q') => self.request_quit(),
                code => self.scroll_key(code),
            }
            return;
        }
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.kind == crossterm::event::KeyEventKind::Press
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                self.refresh_read(false);
            }
            return;
        }
        if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.kind == crossterm::event::KeyEventKind::Press {
                self.palette = Some(crate::palette::Palette::default());
            }
            return;
        }
        match key.code {
            KeyCode::Char('v')
                if key.modifiers.is_empty()
                    && key.kind == crossterm::event::KeyEventKind::Press =>
            {
                self.open_evidence()
            }
            KeyCode::Char('f') if self.view == View::Operations => {
                self.set_operations_scope(self.operations_scope.next());
            }
            KeyCode::Char('f') if self.view == View::Integrations => {
                self.queue_attention = !self.queue_attention;
                self.refilter_local();
            }
            KeyCode::Char('f') if self.view.financial() => {
                self.set_balance_filter(self.balance_filter.next(self.view));
            }
            KeyCode::Char('d') if self.view == View::Integrations => {
                self.queue_direction = self.queue_direction.next();
                self.refilter_local();
            }
            KeyCode::Char('o') => self.open_related(),
            KeyCode::Char('b') => self.return_related(),
            KeyCode::Char('q') | KeyCode::Esc => self.request_quit(),
            KeyCode::Char('?') => {
                self.help = true;
                self.detail_scroll = 0;
            }
            KeyCode::Char('i') => {
                self.connection = true;
                self.detail_scroll = 0;
            }
            KeyCode::Char('R') => {
                self.toggle_auto_reads();
            }
            KeyCode::Char(c @ '1'..='5') => self.switch((c as u8 - b'1') as usize),
            KeyCode::Tab | KeyCode::Right => self.switch(self.view.index() + 1),
            KeyCode::BackTab | KeyCode::Left => self.switch(self.view.index() + 4),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.visible.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.selected = self.visible.len().saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.selected = (self.selected + 10).min(self.visible.len().saturating_sub(1))
            }
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::Char('/') => {
                self.searching = true;
                self.draft = self.query.clone();
                self.draft_cursor = self.draft.len();
            }
            KeyCode::Backspace => self.clear_filters(),
            KeyCode::Char('s') if self.view == View::Loads => {
                self.set_status((self.status_index + 1) % STATUSES.len());
            }
            KeyCode::Char('r') => self.refresh_read(true),
            KeyCode::Char('n') => self.turn_page(true),
            KeyCode::Char('p') => self.turn_page(false),
            KeyCode::Enter if self.current().is_some() => {
                self.details = true;
                self.detail_scroll = 0;
            }
            _ => {}
        }
    }
}
