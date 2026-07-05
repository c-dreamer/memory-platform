//! Relationship model — matches the `relationships` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An entity-relationship edge in the knowledge graph.
///
/// Relationships connect any two entities (memories, sessions, agents,
/// projects, etc.) with a typed relation and weight. Mirrored in Graphiti.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Relationship {
    pub id: Uuid,
    pub source_type: String,
    pub source_id: Uuid,
    pub target_type: String,
    pub target_id: Uuid,
    pub relation_type: String,
    pub weight: f64,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_relationship_with_valid_data() {
        let now = Utc::now();
        let rel = Relationship {
            id: Uuid::new_v4(),
            source_type: "memory".into(),
            source_id: Uuid::new_v4(),
            target_type: "session".into(),
            target_id: Uuid::new_v4(),
            relation_type: "derived_from".into(),
            weight: 0.85,
            metadata: serde_json::json!({"confidence": 0.9}),
            created_at: now,
        };
        assert_eq!(rel.source_type, "memory");
        assert_eq!(rel.target_type, "session");
        assert_eq!(rel.relation_type, "derived_from");
        assert_eq!(rel.weight, 0.85);
    }

    #[test]
    fn relationship_serde_roundtrip() {
        let now = Utc::now();
        let rel = Relationship {
            id: Uuid::new_v4(),
            source_type: "agent".into(),
            source_id: Uuid::new_v4(),
            target_type: "project".into(),
            target_id: Uuid::new_v4(),
            relation_type: "owns".into(),
            weight: 1.0,
            metadata: serde_json::json!({}),
            created_at: now,
        };
        let json = serde_json::to_string(&rel).unwrap();
        let decoded: Relationship = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.source_type, "agent");
        assert_eq!(decoded.relation_type, "owns");
    }
}
