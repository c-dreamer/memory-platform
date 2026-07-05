//! Contradiction model — matches the `contradictions` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A detected conflict between two memories.
///
/// Contradictions are flagged when two memories have high semantic similarity
/// but contradictory content. They can be resolved manually or automatically.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Contradiction {
    pub id: Uuid,
    pub memory_id_a: Uuid,
    pub memory_id_b: Uuid,
    pub content_a: String,
    pub content_b: String,
    pub similarity: f64,
    pub contradiction_type: String,
    pub detected_by: String,
    pub resolved: bool,
    pub resolution_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A candidate contradiction for API responses — not DB-backed.
///
/// Returned by the contradiction detection service before a contradiction
/// is confirmed and stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionCandidate {
    pub memory_id_a: Uuid,
    pub memory_id_b: Uuid,
    pub content_a: String,
    pub content_b: String,
    pub similarity: f64,
    pub contradiction_type: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_contradiction_with_valid_data() {
        let now = Utc::now();
        let contradiction = Contradiction {
            id: Uuid::new_v4(),
            memory_id_a: Uuid::new_v4(),
            memory_id_b: Uuid::new_v4(),
            content_a: "Rust is memory-safe".into(),
            content_b: "Rust has memory leaks".into(),
            similarity: 0.92,
            contradiction_type: "semantic".into(),
            detected_by: "auto".into(),
            resolved: false,
            resolution_note: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(contradiction.contradiction_type, "semantic");
        assert_eq!(contradiction.similarity, 0.92);
        assert!(!contradiction.resolved);
    }

    #[test]
    fn contradiction_serde_roundtrip() {
        let now = Utc::now();
        let contradiction = Contradiction {
            id: Uuid::new_v4(),
            memory_id_a: Uuid::new_v4(),
            memory_id_b: Uuid::new_v4(),
            content_a: "A".into(),
            content_b: "B".into(),
            similarity: 0.5,
            contradiction_type: "factual".into(),
            detected_by: "manual".into(),
            resolved: true,
            resolution_note: Some("A is correct".into()),
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&contradiction).unwrap();
        let decoded: Contradiction = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.contradiction_type, "factual");
        assert!(decoded.resolved);
        assert_eq!(decoded.resolution_note, Some("A is correct".into()));
    }

    #[test]
    fn contradiction_candidate_serde_roundtrip() {
        let candidate = ContradictionCandidate {
            memory_id_a: Uuid::new_v4(),
            memory_id_b: Uuid::new_v4(),
            content_a: "X is true".into(),
            content_b: "X is false".into(),
            similarity: 0.88,
            contradiction_type: "logical".into(),
        };
        let json = serde_json::to_string(&candidate).unwrap();
        let decoded: ContradictionCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.similarity, 0.88);
        assert_eq!(decoded.contradiction_type, "logical");
    }
}
