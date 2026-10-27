//! Existing API queue scopes. Ownership and deferral do not change shipment truth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Actionable,
    Active,
    Mine,
    Unowned,
    Deferred,
    Inactive,
    All,
}
impl Scope {
    pub const ALL: [Self; 7] = [
        Self::Actionable,
        Self::Active,
        Self::Mine,
        Self::Unowned,
        Self::Deferred,
        Self::Inactive,
        Self::All,
    ];
    pub fn code(self) -> &'static str {
        match self {
            Self::Actionable => "actionable",
            Self::Active => "active",
            Self::Mine => "mine",
            Self::Unowned => "unowned",
            Self::Deferred => "deferred",
            Self::Inactive => "resolved",
            Self::All => "all",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Actionable => "Actionable",
            Self::Active => "All active",
            Self::Mine => "Assigned to me",
            Self::Unowned => "Unowned",
            Self::Deferred => "Deferred",
            Self::Inactive => "Source inactive",
            Self::All => "All issues",
        }
    }
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.code() == code)
    }
    pub fn next(self) -> Self {
        match self {
            Self::Actionable => Self::Active,
            Self::Active => Self::Mine,
            Self::Mine => Self::Unowned,
            Self::Unowned => Self::Deferred,
            Self::Deferred => Self::Inactive,
            Self::Inactive => Self::All,
            Self::All => Self::Actionable,
        }
    }
}
