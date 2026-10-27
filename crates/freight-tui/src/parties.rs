//! Exact catalog identities stay separate from cleaned display text and free-text search.
use crate::model::{clean, App, View};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Party {
    Customer,
    Carrier,
}
impl Party {
    pub const ALL: [Self; 2] = [Self::Customer, Self::Carrier];
    pub fn field(self) -> &'static str {
        match self {
            Self::Customer => "customer_id",
            Self::Carrier => "carrier_id",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Carrier => "carrier",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadParties {
    customer: Option<String>,
    carrier: Option<String>,
}
impl LoadParties {
    pub fn from_load(value: &Value) -> Self {
        let id = |party: Party| {
            let raw = value[party.field()].as_str()?;
            // Never filter by a sanitized/truncated approximation of a server identity.
            (!raw.trim().is_empty() && raw.len() <= 200 && clean(raw) == raw)
                .then(|| raw.to_owned())
        };
        Self {
            customer: id(Party::Customer),
            carrier: id(Party::Carrier),
        }
    }
    pub fn get(&self, party: Party) -> Option<&str> {
        match party {
            Party::Customer => self.customer.as_deref(),
            Party::Carrier => self.carrier.as_deref(),
        }
    }
    fn set(&mut self, party: Party, id: Option<String>) {
        match party {
            Party::Customer => self.customer = id,
            Party::Carrier => self.carrier = id,
        }
    }
    pub fn any(&self) -> bool {
        self.customer.is_some() || self.carrier.is_some()
    }
    pub fn summary(&self) -> &'static str {
        match (self.customer.is_some(), self.carrier.is_some()) {
            (true, true) => "[customer + carrier]",
            (true, false) => "[customer]",
            (false, true) => "[carrier]",
            (false, false) => "",
        }
    }
}
impl App {
    pub(crate) fn verified_load_parties(&self) -> bool {
        self.view == View::Loads
            && !self.denied
            && crate::actions::Owner::from_identity(&self.identity).is_some()
    }
    pub(crate) fn can_filter_selected_party(&self, party: Party) -> bool {
        self.verified_load_parties()
            && !self.needs_fetch
            && self.loading.is_none()
            && self
                .current()
                .is_some_and(|r| r.load_parties.get(party).is_some())
    }
    pub(crate) fn filter_selected_party(&mut self, party: Party) {
        if !self.can_filter_selected_party(party) {
            return;
        }
        let id = self
            .current()
            .and_then(|r| r.load_parties.get(party))
            .map(str::to_owned);
        self.set_load_party(party, id);
    }
    pub(crate) fn clear_party(&mut self, party: Party) {
        if self.verified_load_parties() {
            self.set_load_party(party, None);
        }
    }
    fn set_load_party(&mut self, party: Party, id: Option<String>) {
        if self.load_parties.get(party) == id.as_deref() {
            return;
        }
        self.load_parties.set(party, id);
        self.reset_page();
        self.rows.clear();
        self.visible.clear();
        self.updated = None;
        self.invalidate();
    }
}
