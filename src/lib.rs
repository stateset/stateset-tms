mod admission_diagnostics;
pub mod amendments;
pub mod api;
pub mod auth;
pub mod cancellations;
mod canonical_storage;
pub mod carrier_credits;
pub mod carrier_portal;
pub mod charges;
pub mod commerce;
pub mod console_catalog;
pub mod contracts;
pub mod customer_portal;
pub mod customer_refunds;
mod database_budget;
pub mod documents;
pub mod domain;
pub mod execution;
pub mod finance;
mod finance_read;
#[cfg(test)]
mod finance_read_tests;
pub mod history;
pub mod identity;
pub mod inbox;
mod integration_worker;
pub mod integrations;
mod invoice_review;
pub mod itinerary;
pub mod lifecycle;
pub mod load_catalog;
pub mod movement;
pub mod observability;
pub mod operations;
pub mod payables;
pub mod pdf_renderer;
pub mod portal;
pub mod quotes;
pub mod rate_confirmations;
mod read_admission;
mod read_json;
mod read_profile;
pub mod release_portal;
mod retained_json;
pub mod save_recovery;
pub mod sharing;
pub mod source;
pub mod stops;
pub mod store;
pub mod tenders;
mod workspace_admission;
mod write_admission;

mod write_json;
mod write_profile;

#[cfg(test)]
#[path = "../tests/support/canonical.rs"]
pub(crate) mod storage_fixture;

mod catalog_admission;
