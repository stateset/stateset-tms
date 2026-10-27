//! Operator-confirmed appointments and gate evidence. Neither implies custody,
//! delivery, a signed POD, or an automatically incurred accessorial charge.
use crate::domain::{self, Load, Result, Status, Stop, Workspace};
use chrono::{DateTime, Duration, LocalResult, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct StopDetails {
    pub address_lines: Vec<String>,
    pub postal_code: String,
    pub country: String,
    pub contact: String,
    pub phone: String,
    pub instructions: String,
}
impl Default for StopDetails {
    fn default() -> Self {
        Self {
            address_lines: vec![],
            postal_code: String::new(),
            country: "US".into(),
            contact: String::new(),
            phone: String::new(),
            instructions: String::new(),
        }
    }
}
impl StopDetails {
    pub fn demo() -> Self {
        Self {
            address_lines: vec!["100 Synthetic Demo Way".into()],
            postal_code: "00000".into(),
            ..Self::default()
        }
    }
}
pub fn validate_stop(stop: &Stop, tender: bool) -> Result<()> {
    wire_text(&stop.facility, "Facility", 60, true)?;
    wire_text(&stop.city, "City", 30, true)?;
    if stop.state.len() != 2 || !stop.state.bytes().all(|v| v.is_ascii_uppercase()) {
        return Err("State must be a two-letter uppercase code".into());
    }
    let details = &stop.details;
    if details.country.len() != 2 || !details.country.bytes().all(|v| v.is_ascii_uppercase()) {
        return Err("Country must be a two-letter uppercase code".into());
    }
    if details.address_lines.len() > 2 || (tender && details.address_lines.is_empty()) {
        return Err(
            "Provide one or two street address lines for both stops before tendering".into(),
        );
    }
    for line in &details.address_lines {
        wire_text(line, "Street address", 55, true)?;
    }
    wire_text(&details.postal_code, "Postal code", 15, tender)?;
    for (value, field, max) in [
        (&details.contact, "Stop contact", 200),
        (&details.phone, "Stop phone", 100),
        (&details.instructions, "Stop instructions", 1000),
    ] {
        if value.len() > max || value.chars().any(|v| v.is_control() && v != '\n') {
            return Err(format!(
                "{field} exceeds {max} characters or contains invalid controls"
            ));
        }
    }
    Ok(())
}
fn wire_text(value: &str, field: &str, max: usize, required: bool) -> Result<()> {
    if (required && value.trim().is_empty())
        || value.len() > max
        || !value.is_ascii()
        || value
            .bytes()
            .any(|v| v.is_ascii_control() || b"*~:^>".contains(&v))
    {
        return Err(format!(
            "{field} must contain {}–{max} ASCII characters without X12 delimiters",
            if required { 1 } else { 0 }
        ));
    }
    Ok(())
}
pub fn party(stop: &Stop, role: &str) -> Value {
    json!({"role":role,"name":stop.facility,"address_lines":stop.details.address_lines,
        "city":stop.city,"state":stop.state,"postal_code":stop.details.postal_code,"country":stop.details.country})
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Appointment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_snapshot: Option<Stop>,
    pub id: String,
    pub load_id: String,
    pub stop: String,
    pub status: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub timezone: String,
    pub start_local: String,
    pub end_local: String,
    pub disambiguation: String,
    pub contact: String,
    pub confirmation_reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub replaces_id: Option<String>,
    pub cancellation: Option<AppointmentCancellation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppointmentCancellation {
    pub reason: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StopEvent {
    pub id: String,
    pub load_id: String,
    pub tender_id: String,
    pub appointment_id: Option<String>,
    pub stop: String,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub local_time: String,
    pub timezone: String,
    pub disambiguation: String,
    pub reference: String,
    pub note: String,
    pub recorded_at: DateTime<Utc>,
    pub recorded_by: String,
    pub supersedes_id: Option<String>,
}
// Borrowed lookup for one immutable source projection. All supersession IDs
// participate, as in current_stop_events; grouping never changes record order.
// Never retain this index across workspace snapshots or clock evaluations.
pub(crate) struct CurrentStopEvents<'a> {
    rows: std::collections::BTreeMap<
        &'a str,
        std::collections::BTreeMap<&'a str, Vec<&'a StopEvent>>,
    >,
}
impl<'a> CurrentStopEvents<'a> {
    pub(crate) fn new(events: &'a [StopEvent]) -> Self {
        let superseded: std::collections::BTreeSet<&str> = events
            .iter()
            .filter_map(|v| v.supersedes_id.as_deref())
            .collect();
        let mut rows = std::collections::BTreeMap::<
            &str,
            std::collections::BTreeMap<&str, Vec<&StopEvent>>,
        >::new();
        for event in events {
            if !superseded.contains(event.id.as_str()) {
                rows.entry(event.load_id.as_str())
                    .or_default()
                    .entry(event.tender_id.as_str())
                    .or_default()
                    .push(event);
            }
        }
        Self { rows }
    }
    pub(crate) fn for_load(&self, load: &Load) -> &[&'a StopEvent] {
        load.tender_id
            .as_deref()
            .and_then(|tender| {
                self.rows
                    .get(load.id.as_str())
                    .and_then(|rows| rows.get(tender))
            })
            .map_or(&[], Vec::as_slice)
    }
}
// One immutable projection only. Keep the first active row, exactly as the
// single-stop selector does, even when retained input has duplicate active rows.
pub(crate) struct ActiveAppointments<'a> {
    rows: std::collections::BTreeMap<&'a str, std::collections::BTreeMap<&'a str, &'a Appointment>>,
}
impl<'a> ActiveAppointments<'a> {
    pub(crate) fn new(appointments: &'a [Appointment]) -> Self {
        let mut rows = std::collections::BTreeMap::<
            &str,
            std::collections::BTreeMap<&str, &Appointment>,
        >::new();
        for appointment in appointments.iter().filter(|a| a.status == "active") {
            rows.entry(appointment.load_id.as_str())
                .or_default()
                .entry(appointment.stop.as_str())
                .or_insert(appointment);
        }
        Self { rows }
    }
    pub(crate) fn for_stop(&self, load_id: &str, stop: &str) -> Option<&'a Appointment> {
        self.rows
            .get(load_id)
            .and_then(|stops| stops.get(stop))
            .copied()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppointmentInput {
    expected_load_version: u64,
    stop: String,
    start_local: String,
    end_local: String,
    timezone: String,
    #[serde(default = "reject_ambiguity")]
    disambiguation: String,
    contact: String,
    confirmation_reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelInput {
    expected_load_version: u64,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GateInput {
    expected_load_version: u64,
    stop: String,
    kind: String,
    local_time: String,
    timezone: String,
    #[serde(default = "reject_ambiguity")]
    disambiguation: String,
    reference: String,
    note: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectionInput {
    expected_load_version: u64,
    local_time: String,
    timezone: String,
    #[serde(default = "reject_ambiguity")]
    disambiguation: String,
    reference: String,
    note: String,
}
fn reject_ambiguity() -> String {
    "reject".into()
}

/// A local time without an offset is resolved only against the supplied IANA
/// zone. Gaps always fail; repeated times require an explicit earlier/later choice.
pub fn local_instant(local: &str, zone: &str, disambiguation: &str) -> Result<DateTime<Utc>> {
    if !["reject", "earlier", "later"].contains(&disambiguation) {
        return Err("Time disambiguation must be reject, earlier, or later".into());
    }
    let tz: Tz = zone
        .parse()
        .map_err(|_| "Use a valid IANA timezone, such as America/Los_Angeles")?;
    let naive = NaiveDateTime::parse_from_str(local, "%Y-%m-%dT%H:%M")
        .map_err(|_| "Local time must use YYYY-MM-DDTHH:MM")?;
    if naive.format("%Y-%m-%dT%H:%M").to_string() != local {
        return Err("Local time must use YYYY-MM-DDTHH:MM".into());
    }
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(v) => Ok(v.with_timezone(&Utc)),
        LocalResult::Ambiguous(a, b) => match disambiguation {
            "earlier" => Ok(a.min(b).with_timezone(&Utc)),
            "later" => Ok(a.max(b).with_timezone(&Utc)),
            _ => Err("This local time occurs twice during a timezone change; choose earlier or later explicitly".into()),
        },
        LocalResult::None => Err("This local time does not exist during a timezone change; choose a valid time".into()),
    }
}
fn stop_key(load: &Load, value: &str) -> Result<()> {
    stop_of(load, value).map(|_| ())
}
fn stop_of<'a>(load: &'a Load, key: &str) -> Result<&'a Stop> {
    crate::itinerary::route_stops(load)
        .into_iter()
        .find(|(id, _, _)| *id == key)
        .map(|(_, _, s)| s)
        .ok_or("Route stop not found".into())
}

fn bump(load: &mut Load, expected: u64) -> Result<()> {
    if load.version != expected {
        return Err("Load changed; refresh before continuing".into());
    }
    load.version = load
        .version
        .checked_add(1)
        .ok_or("Load version exhausted")?;
    load.updated_at = Utc::now();
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| "Invalid stop operation fields".into())
}
impl Workspace {
    /// Qualification must cover an overnight appointment's final local day.
    pub fn coverage_date(&self, load: &Load) -> chrono::NaiveDate {
        self.appointments
            .iter()
            .filter(|v| v.load_id == load.id && v.status == "active")
            .filter_map(|v| {
                v.timezone
                    .parse::<Tz>()
                    .ok()
                    .map(|zone| v.ends_at.with_timezone(&zone).date_naive())
            })
            .fold(load.destination.date, std::cmp::max)
    }
    pub fn active_appointment(&self, load_id: &str, stop: &str) -> Option<&Appointment> {
        self.appointments
            .iter()
            .find(|v| v.load_id == load_id && v.stop == stop && v.status == "active")
    }
    pub fn current_stop_events<'a>(&'a self, load: &Load) -> Vec<&'a StopEvent> {
        let superseded: std::collections::BTreeSet<&str> = self
            .stop_events
            .iter()
            .filter_map(|v| v.supersedes_id.as_deref())
            .collect();
        self.stop_events
            .iter()
            .filter(|v| {
                v.load_id == load.id
                    && Some(&v.tender_id) == load.tender_id.as_ref()
                    && !superseded.contains(v.id.as_str())
            })
            .collect()
    }
    pub fn check_stop_revision(&self, old: &Load, new: &Load) -> Result<()> {
        let next = crate::itinerary::route_stops(new);
        for (position, (key, _, stop)) in crate::itinerary::route_stops(old).iter().enumerate() {
            if self.active_appointment(&old.id, key).is_some()
                && !next
                    .iter()
                    .enumerate()
                    .any(|(p, (id, _, s))| id == key && p == position && json!(s) == json!(stop))
            {
                return Err(
                    "Cancel the booked appointment before changing its stop details, date or order"
                        .into(),
                );
            }
        }
        Ok(())
    }
    pub fn stop_execution(&self, load: &Load, now: DateTime<Utc>) -> Value {
        let events = self.current_stop_events(load);
        self.stop_execution_with_sources(load, now, &events, |key| {
            self.active_appointment(&load.id, key)
        })
    }
    pub(crate) fn stop_execution_with_sources<'a>(
        &'a self,
        load: &Load,
        now: DateTime<Utc>,
        events: &[&StopEvent],
        appointment: impl Fn(&str) -> Option<&'a Appointment>,
    ) -> Value {
        self.stop_execution_with_completion(load, now, events, appointment, |key| {
            self.stop_complete(load, key)
        })
    }
    pub(crate) fn stop_execution_with_completion<'a>(
        &'a self,
        load: &Load,
        now: DateTime<Utc>,
        events: &[&StopEvent],
        appointment: impl Fn(&str) -> Option<&'a Appointment>,
        complete: impl Fn(&str) -> bool,
    ) -> Value {
        let mut result = json!({"has_overdue":false});
        for (key, _, _) in crate::itinerary::route_stops(load) {
            let appt = appointment(key);
            let arrival = events
                .iter()
                .find(|v| v.stop == key && v.kind == "arrival")
                .copied();
            let departure = events
                .iter()
                .find(|v| v.stop == key && v.kind == "departure")
                .copied();
            let physically_complete = complete(key);
            let overdue = appt.is_some_and(|a| now > a.ends_at)
                && arrival.is_none()
                && !physically_complete
                && load.status != Status::Cancelled;
            if overdue {
                result["has_overdue"] = json!(true);
            }
            let timing = match (appt, arrival) {
                (Some(a), Some(v)) if v.occurred_at < a.starts_at => "early",
                (Some(a), Some(v)) if v.occurred_at > a.ends_at => "late",
                (Some(_), Some(_)) => "on_time",
                (None, Some(_)) => "unscheduled",
                _ => "unrecorded",
            };
            let dwell = arrival.and_then(|a| match departure {
                Some(v) => Some((v.occurred_at - a.occurred_at).num_minutes().max(0)),
                None if !physically_complete && load.status != Status::Cancelled => {
                    Some((now - a.occurred_at).num_minutes().max(0))
                }
                _ => None,
            });
            result[key] = json!({"appointment":appt,"arrival":arrival,"departure":departure,"arrival_status":timing,"overdue":overdue,
                "late_minutes":match (appt,arrival) {(Some(a),Some(v))=>Some((v.occurred_at-a.ends_at).num_minutes().max(0)),_=>None},
                "dwell_minutes":dwell,"dwell_open":arrival.is_some()&&departure.is_none()&&!physically_complete&&load.status!=Status::Cancelled,
                "dwell_complete":arrival.is_some()&&departure.is_some()});
        }
        result
    }
    pub fn stop_tracking(&self, load: &Load) -> Value {
        let events = self.current_stop_events(load);
        let mut result = json!({});
        for (key, _, _) in crate::itinerary::route_stops(load) {
            result[key] = json!({"appointment":self.active_appointment(&load.id,key).map(|v|json!({"starts_at":v.starts_at,"ends_at":v.ends_at,"timezone":v.timezone})),
                "arrival_at":events.iter().find(|v|v.stop==key&&v.kind=="arrival").map(|v|v.occurred_at),
                "departure_at":events.iter().find(|v|v.stop==key&&v.kind=="departure").map(|v|v.occurred_at)});
        }
        result
    }
    pub fn apply_stop_operation(
        &mut self,
        operation: &str,
        entity: &str,
        body: Value,
        actor: &str,
    ) -> Result<Value> {
        let load_id = match operation {
            "stop.appointment.cancel" => self
                .appointments
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.clone())
                .ok_or("Appointment not found")?,
            "stop.event.correct" => self
                .stop_events
                .iter()
                .find(|v| v.id == entity)
                .map(|v| v.load_id.clone())
                .ok_or("Stop event not found")?,
            "stop.appointment.book" | "stop.event.record" => entity.into(),
            _ => return Err("Unsupported stop operation".into()),
        };
        let index = self
            .loads
            .iter()
            .position(|v| v.id == load_id)
            .ok_or("Load not found")?;
        let mut load = self.loads[index].clone();
        let result;
        match operation {
            "stop.appointment.book" => {
                let input: AppointmentInput = decode(body)?;
                stop_key(&load, &input.stop)?;
                bump(&mut load, input.expected_load_version)?;
                if matches!(load.status, Status::Cancelled | Status::Delivered)
                    || self.stop_complete(&load, &input.stop)
                {
                    return Err(
                        "Cannot book an appointment for a completed or cancelled stop".into(),
                    );
                }
                if self
                    .current_stop_events(&load)
                    .iter()
                    .any(|v| v.stop == input.stop)
                {
                    return Err("Arrival already recorded; preserve the appointment against which arrival was measured".into());
                }
                for (v, f) in [
                    (&input.contact, "Appointment contact"),
                    (
                        &input.confirmation_reference,
                        "Appointment confirmation reference",
                    ),
                    (&input.note, "Appointment note"),
                ] {
                    domain::required(v, f)?;
                }
                let starts =
                    local_instant(&input.start_local, &input.timezone, &input.disambiguation)?;
                let ends = local_instant(&input.end_local, &input.timezone, &input.disambiguation)?;
                if ends < starts || ends - starts > Duration::hours(48) {
                    return Err("Appointment end must follow its start within 48 hours".into());
                }
                let tz: Tz = input.timezone.parse().map_err(|_| "Invalid timezone")?;
                if starts.with_timezone(&tz).date_naive() != stop_of(&load, &input.stop)?.date {
                    return Err("Appointment start must match the stop's planned local date; revise the plan first".into());
                }
                let route = crate::itinerary::route_stops(&load);
                let at = route
                    .iter()
                    .position(|(id, _, _)| *id == input.stop)
                    .ok_or("Route stop not found")?;
                for (index, (id, _, _)) in route.iter().enumerate() {
                    if *id == input.stop {
                        continue;
                    }
                    if let Some(other) = self.active_appointment(&load.id, id) {
                        if (index < at && other.starts_at > starts)
                            || (index > at && other.starts_at < starts)
                        {
                            return Err(
                                "Appointment starts must follow the full route stop order".into()
                            );
                        }
                    }
                }
                let mut covered = load
                    .destination
                    .date
                    .max(ends.with_timezone(&tz).date_naive())
                    .max(Utc::now().date_naive());
                for other in self.appointments.iter().filter(|v| {
                    v.load_id == load.id && v.status == "active" && v.stop != input.stop
                }) {
                    let zone: Tz = other
                        .timezone
                        .parse()
                        .map_err(|_| "Stored appointment timezone is invalid")?;
                    covered = covered.max(other.ends_at.with_timezone(&zone).date_naive());
                }
                if let Some(carrier_id) = &load.carrier_id {
                    let carrier = self
                        .carriers
                        .iter()
                        .find(|v| &v.id == carrier_id)
                        .ok_or("Carrier not found")?;
                    if carrier.insurance_expires < covered {
                        return Err(
                            "Carrier insurance must cover the appointment's final local day".into(),
                        );
                    }
                }
                if let Some(driver_id) = &load.driver_id {
                    let driver = self
                        .drivers
                        .iter()
                        .find(|v| &v.id == driver_id)
                        .ok_or("Driver not found")?;
                    if driver.license_expires < covered {
                        return Err(
                            "Driver license must cover the appointment's final local day".into(),
                        );
                    }
                }
                let old = self
                    .active_appointment(&load.id, &input.stop)
                    .map(|v| v.id.clone());
                if let Some(id) = &old {
                    self.appointments
                        .iter_mut()
                        .find(|v| v.id == *id)
                        .ok_or("Appointment not found")?
                        .status = "superseded".into();
                }
                let appointment = Appointment {
                    stop_snapshot: Some(stop_of(&load, &input.stop)?.clone()),
                    id: domain::id("appointment"),
                    load_id: load.id.clone(),
                    stop: input.stop,
                    status: "active".into(),
                    starts_at: starts,
                    ends_at: ends,
                    timezone: input.timezone,
                    start_local: input.start_local,
                    end_local: input.end_local,
                    disambiguation: input.disambiguation,
                    contact: input.contact,
                    confirmation_reference: input.confirmation_reference,
                    note: input.note,
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                    replaces_id: old,
                    cancellation: None,
                };
                result = json!(appointment);
                self.appointments.push(appointment);
            }
            "stop.appointment.cancel" => {
                let input: CancelInput = decode(body)?;
                domain::required(&input.reason, "Appointment cancellation reason")?;
                bump(&mut load, input.expected_load_version)?;
                let a = self
                    .appointments
                    .iter()
                    .position(|v| v.id == entity)
                    .ok_or("Appointment not found")?;
                if self.stop_complete(&load, &self.appointments[a].stop) {
                    return Err("Completed stop appointment evidence is retained".into());
                }
                if self.appointments[a].status != "active" {
                    return Err("Appointment is no longer active".into());
                }
                if self
                    .current_stop_events(&load)
                    .iter()
                    .any(|v| v.appointment_id.as_deref() == Some(entity))
                {
                    return Err("Arrival already recorded; appointment evidence is retained".into());
                }
                self.appointments[a].status = "cancelled".into();
                self.appointments[a].cancellation = Some(AppointmentCancellation {
                    reason: input.reason,
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                });
                result = json!(self.appointments[a]);
            }
            "stop.event.record" | "stop.event.correct" => {
                if load.carrier_id.is_some() {
                    self.adopt_tender(&mut load)?;
                }
                let tender = load.tender_id.clone().ok_or(
                    "An accepted carrier assignment is required before recording gate evidence",
                )?;
                let (input, supersedes, appointment_id) = if operation == "stop.event.correct" {
                    let correction: CorrectionInput = decode(body)?;
                    let original = self
                        .current_stop_events(&load)
                        .into_iter()
                        .find(|v| v.id == entity)
                        .cloned()
                        .ok_or("Stop event is historical or already corrected")?;
                    (
                        GateInput {
                            expected_load_version: correction.expected_load_version,
                            stop: original.stop,
                            kind: original.kind,
                            local_time: correction.local_time,
                            timezone: correction.timezone,
                            disambiguation: correction.disambiguation,
                            reference: correction.reference,
                            note: correction.note,
                        },
                        Some(original.id),
                        original.appointment_id,
                    )
                } else {
                    let input: GateInput = decode(body)?;
                    let appointment = self
                        .active_appointment(&load.id, &input.stop)
                        .map(|v| v.id.clone());
                    (input, None, appointment)
                };
                stop_key(&load, &input.stop)?;
                if !["arrival", "departure"].contains(&input.kind.as_str()) {
                    return Err("Gate event must be arrival or departure".into());
                }
                let allowed = match (input.stop.as_str(), input.kind.as_str()) {
                    ("pickup", "arrival") => matches!(
                        load.status,
                        Status::Accepted
                            | Status::Dispatched
                            | Status::InTransit
                            | Status::Delivered
                    ),
                    ("pickup", "departure") | ("delivery", "arrival") => {
                        matches!(load.status, Status::InTransit | Status::Delivered)
                    }
                    ("delivery", _) => load.status == Status::Delivered,
                    (_, "arrival") => matches!(load.status, Status::InTransit | Status::Delivered),
                    _ => self.stop_complete(&load, &input.stop),
                };
                if !allowed {
                    return Err("Reconcile carrier acceptance, pickup, or delivery before recording this gate event".into());
                }
                domain::required(&input.reference, "Gate evidence reference")?;
                domain::required(&input.note, "Gate evidence note")?;
                let occurred =
                    local_instant(&input.local_time, &input.timezone, &input.disambiguation)?;
                if occurred > Utc::now() + Duration::minutes(5) {
                    return Err("Actual gate time cannot be in the future".into());
                }
                bump(&mut load, input.expected_load_version)?;
                let existing = self.current_stop_events(&load);
                if supersedes.is_none()
                    && existing
                        .iter()
                        .any(|v| v.stop == input.stop && v.kind == input.kind)
                {
                    return Err("Gate event already recorded; use an explicit correction".into());
                }
                let event = StopEvent {
                    id: domain::id("gate"),
                    load_id: load.id.clone(),
                    tender_id: tender,
                    appointment_id,
                    stop: input.stop,
                    kind: input.kind,
                    occurred_at: occurred,
                    local_time: input.local_time,
                    timezone: input.timezone,
                    disambiguation: input.disambiguation,
                    reference: input.reference,
                    note: input.note,
                    recorded_at: Utc::now(),
                    recorded_by: actor.into(),
                    supersedes_id: supersedes,
                };
                let mut sequence: Vec<&StopEvent> = existing
                    .into_iter()
                    .filter(|v| event.supersedes_id.as_deref() != Some(&v.id))
                    .collect();
                sequence.push(&event);
                let route = crate::itinerary::route_stops(&load);
                sequence.sort_by_key(|v| {
                    route
                        .iter()
                        .position(|(id, _, _)| *id == v.stop)
                        .unwrap_or(usize::MAX / 2)
                        * 2
                        + usize::from(v.kind == "departure")
                });
                for service in self
                    .current_services(&load)
                    .iter()
                    .filter(|s| s.stop_id == event.stop)
                {
                    if (event.kind == "arrival" && event.occurred_at > service.observed_at)
                        || (event.kind == "departure" && event.occurred_at < service.observed_at)
                    {
                        return Err(
                            "Gate evidence conflicts with confirmed stop service time".into()
                        );
                    }
                }
                if event.kind == "departure"
                    && !sequence
                        .iter()
                        .any(|v| v.stop == event.stop && v.kind == "arrival")
                {
                    return Err("Record arrival before departure".into());
                }
                if sequence
                    .windows(2)
                    .any(|pair| pair[0].occurred_at > pair[1].occurred_at)
                {
                    return Err(
                        "Gate times must follow the full route arrival and departure order".into(),
                    );
                }
                self.validate_execution_fact(
                    &load,
                    event.supersedes_id.as_deref(),
                    crate::execution::ExecutionFact::gate(&event),
                )?;
                result = json!(event);
                self.stop_events.push(event);
            }
            _ => return Err("Unsupported stop operation".into()),
        }
        self.loads[index] = load.clone();
        self.enqueue_sync(&load);
        self.record(
            &load.id,
            operation,
            "Stop appointment or gate evidence recorded; original history retained",
        );
        if let Some(event) = self.events.last_mut() {
            event.details = Some(result.clone());
        }
        Ok(result)
    }
}
