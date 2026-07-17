//! Memory model — matches the `memories` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A general-purpose knowledge item with vector embedding and decay tracking.
///
/// Memories are the core unit of the platform. They carry content, tags,
/// importance, and a decay score computed from access patterns.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Memory {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub content: String,
    pub content_type: String,
    pub embedding: Option<Embedding>,
    #[sqlx(skip)]
    pub fts: Option<String>,
    pub importance: f64,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub access_count: i32,
    pub decay_score: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_memory_with_valid_data() {
        let now = Utc::now();
        let memory = Memory {
            id: Uuid::new_v4(),
            agent_id: Some(Uuid::new_v4()),
            session_id: Some(Uuid::new_v4()),
            content: "Rust is memory-safe".into(),
            content_type: "insight".into(),
            embedding: None,
            fts: None,
            importance: 0.8,
            tags: vec!["rust".into(), "memory".into()],
            metadata: serde_json::json!({"source": "docs"}),
            last_accessed_at: None,
            access_count: 0,
            decay_score: 1.0,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(memory.content_type, "insight");
        assert_eq!(memory.importance, 0.8);
        assert_eq!(memory.tags.len(), 2);
        assert_eq!(memory.decay_score, 1.0);
    }

    #[test]
    fn memory_serde_roundtrip() {
        let now = Utc::now();
        let memory = Memory {
            id: Uuid::new_v4(),
            agent_id: None,
            session_id: None,
            content: "Test memory".into(),
            content_type: "note".into(),
            embedding: Some(Embedding::new(vec![0.5; 2048])),
            fts: None,
            importance: 0.5,
            tags: vec![],
            metadata: serde_json::json!({}),
            last_accessed_at: Some(now),
            access_count: 3,
            decay_score: 0.9,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&memory).unwrap();
        let decoded: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, "Test memory");
        assert_eq!(decoded.access_count, 3);
        assert!(decoded.embedding.is_some());
    }
}
