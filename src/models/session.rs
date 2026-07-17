//! Session model — matches the `sessions` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A task or conversation session.
///
/// Sessions track work units: active, completed, failed, or abandoned.
/// They form a tree via `parent_session_id`.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub parent_session_id: Option<Uuid>,
    pub goal: Option<String>,
    pub status: String,
    pub summary: Option<String>,
    pub embedding: Option<Embedding>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_session_with_valid_data() {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            agent_id: Some(Uuid::new_v4()),
            parent_session_id: None,
            goal: Some("Implement data models".into()),
            status: "active".into(),
            summary: None,
            embedding: None,
            started_at: now,
            ended_at: None,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(session.status, "active");
        assert!(session.goal.is_some());
        assert!(session.agent_id.is_some());
        assert!(session.parent_session_id.is_none());
    }

    #[test]
    fn session_with_embedding() {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            agent_id: None,
            parent_session_id: None,
            goal: None,
            status: "completed".into(),
            summary: Some("Done".into()),
            embedding: Some(Embedding::new(vec![0.1; 2048])),
            started_at: now,
            ended_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        assert!(session.embedding.is_some());
        assert_eq!(session.embedding.unwrap().as_vec().len(), 2048);
    }

    #[test]
    fn session_serde_roundtrip() {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4(),
            agent_id: None,
            parent_session_id: None,
            goal: Some("Test".into()),
            status: "active".into(),
            summary: None,
            embedding: None,
            started_at: now,
            ended_at: None,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&session).unwrap();
        let decoded: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.goal, Some("Test".into()));
        assert_eq!(decoded.status, "active");
    }
}
