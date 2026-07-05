//! Summary model — matches the `summaries` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A compressed context snapshot.
///
/// Summaries capture the essence of a session, project, or day's work
/// in a compact form suitable for context injection.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Summary {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub source_type: Option<String>,
    pub content: String,
    pub embedding: Option<Embedding>,
    pub token_count: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_summary_with_valid_data() {
        let now = Utc::now();
        let summary = Summary {
            id: Uuid::new_v4(),
            session_id: Some(Uuid::new_v4()),
            source_type: Some("session".into()),
            content: "Completed data model implementation".into(),
            embedding: None,
            token_count: Some(42),
            created_at: now,
        };
        assert_eq!(summary.source_type, Some("session".into()));
        assert_eq!(summary.token_count, Some(42));
        assert!(summary.embedding.is_none());
    }

    #[test]
    fn summary_with_embedding() {
        let now = Utc::now();
        let summary = Summary {
            id: Uuid::new_v4(),
            session_id: None,
            source_type: None,
            content: "Embedded summary".into(),
            embedding: Some(Embedding::new(vec![0.2; 384])),
            token_count: None,
            created_at: now,
        };
        assert!(summary.embedding.is_some());
        assert_eq!(summary.embedding.unwrap().as_vec().len(), 384);
    }

    #[test]
    fn summary_serde_roundtrip() {
        let now = Utc::now();
        let summary = Summary {
            id: Uuid::new_v4(),
            session_id: None,
            source_type: Some("day".into()),
            content: "Daily summary".into(),
            embedding: None,
            token_count: Some(100),
            created_at: now,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let decoded: Summary = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.content, "Daily summary");
        assert_eq!(decoded.source_type, Some("day".into()));
    }
}
