//! Services module — business logic layer.
//!
//! Will contain context assembly, embedding, experience replay,
//! procedure detection, vault ingestion, memory decay, and contradiction detection.

pub mod context;
pub mod contradiction;
pub mod decay;
pub mod embedding;
pub mod experience;
pub mod ingestion;
pub mod procedure;
