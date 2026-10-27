//! The existing operations seal bytes, serialized directly from borrowed fields.
//! Field order must match the sorted Value object used by the original seals.
use super::{CaseEvent, OperationsCase, Signal};
use serde::{ser::SerializeMap, Serialize, Serializer};

struct Source<'a>(&'a Signal);
struct Event<'a>(&'a CaseEvent);
struct History<'a>(&'a [CaseEvent]);
struct Evidence<'a>(&'a OperationsCase);

impl Serialize for Source<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = self.0;
        let mut map = serializer.serialize_map(Some(10))?;
        map.serialize_entry("deadline", &s.deadline)?;
        map.serialize_entry("id", &s.id)?;
        map.serialize_entry("kind", &s.kind)?;
        map.serialize_entry("load_id", &s.load_id)?;
        map.serialize_entry("next_action", &s.next_action)?;
        map.serialize_entry("priority", &s.priority)?;
        map.serialize_entry("source", &s.source)?;
        map.serialize_entry("subject_id", &s.subject_id)?;
        map.serialize_entry("summary", &s.summary)?;
        map.serialize_entry("title", &s.title)?;
        map.end()
    }
}
impl Serialize for Event<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let e = self.0;
        let mut map = serializer.serialize_map(Some(6))?;
        map.serialize_entry("action", &e.action)?;
        map.serialize_entry("actor", &e.actor)?;
        map.serialize_entry("at", &e.at)?;
        map.serialize_entry("cycle", &e.cycle)?;
        map.serialize_entry("note", &e.note)?;
        map.serialize_entry("source", &e.source.as_ref().map(Source))?;
        map.end()
    }
}
impl Serialize for History<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(Event))
    }
}
impl Serialize for Evidence<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        let mut map = serializer.serialize_map(Some(14))?;
        map.serialize_entry("cycle", &c.cycle)?;
        map.serialize_entry("deferred_until", &c.deferred_until)?;
        map.serialize_entry("first_claimed_at", &c.first_claimed_at)?;
        map.serialize_entry("first_detected_at", &c.first_detected_at)?;
        map.serialize_entry("follow_up_at", &c.follow_up_at)?;
        map.serialize_entry("history", &History(&c.history))?;
        map.serialize_entry("id", &c.id)?;
        map.serialize_entry("owner", &c.owner)?;
        map.serialize_entry("resolved_at", &c.resolved_at)?;
        map.serialize_entry("source", &Source(&c.source))?;
        map.serialize_entry("source_sha256", &c.source_sha256)?;
        map.serialize_entry("status", &c.status)?;
        map.serialize_entry("updated_at", &c.updated_at)?;
        map.serialize_entry("version", &c.version)?;
        map.end()
    }
}

pub(super) fn source(signal: &Signal) -> String {
    crate::documents::sha256(
        &serde_json::to_vec(&Source(signal)).expect("serializable operations source"),
    )
}
pub(super) fn evidence(case: &OperationsCase) -> String {
    crate::documents::sha256(
        &serde_json::to_vec(&Evidence(case)).expect("serializable operations evidence"),
    )
}

#[cfg(test)]
#[path = "operations_digest_tests.rs"]
mod tests;
