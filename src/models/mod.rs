//! Data models — structs matching all 15 database tables + events schema.
//!
//! Each module contains a struct with serde Serialize/Deserialize + sqlx::FromRow
//! for the corresponding database table, plus unit tests.

pub mod agent;
pub mod code_change;
pub mod config_entry;
pub mod contradiction;
pub mod document;
pub mod embedding;
pub mod events;
pub mod experience;
pub mod memory;
pub mod procedure;
pub mod project;
pub mod relationship;
pub mod session;
pub mod summary;
pub mod trading_result;

pub use agent::Agent;
pub use code_change::CodeChange;
pub use config_entry::ConfigEntry;
pub use contradiction::{Contradiction, ContradictionCandidate};
pub use document::Document;
pub use embedding::Embedding;
pub use events::{EventCreate, EventResponse, EventType};
pub use experience::Experience;
pub use memory::Memory;
pub use procedure::Procedure;
pub use project::Project;
pub use relationship::Relationship;
pub use session::Session;
pub use summary::Summary;
pub use trading_result::TradingResult;
