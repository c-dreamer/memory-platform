//! Experience model — matches the `experiences` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A completed task experience for replay and procedure generation.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Experience {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub goal: String,
    pub reasoning_summary: Option<String>,
    pub actions: serde_json::Value,
    pub files_changed: serde_json::Value,
    pub result: Option<String>,
    pub lessons_learned: Option<String>,
    pub confidence: Option<f64>,
    pub duration_seconds: Option<i32>,
    pub tags: Vec<String>,
    pub related_project: Option<String>,
    pub embedding: Option<Embedding>,
    #[sqlx(skip)]
    pub fts: Option<String>,
    pub is_procedurized: bool,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_experience_with_valid_data() {
        let now = Utc::now();
        let exp = Experience {
            id: Uuid::new_v4(),
            agent_id: Some(Uuid::new_v4()),
            session_id: Some(Uuid::new_v4()),
            goal: "Implement auth".into(),
            reasoning_summary: Some("Used JWT with middleware".into()),
            actions: serde_json::json!(["write auth.rs", "add middleware"]),
            files_changed: serde_json::json!(["src/auth.rs"]),
            result: Some("success".into()),
            lessons_learned: Some("Always validate tokens early".into()),
            confidence: Some(0.9),
            duration_seconds: Some(3600),
            tags: vec!["auth".into(), "security".into()],
            related_project: Some("memory-platform".into()),
            embedding: None,
            fts: None,
            is_procedurized: false,
            created_at: now,
        };
        assert_eq!(exp.goal, "Implement auth");
        assert_eq!(exp.result, Some("success".into()));
        assert!(!exp.is_procedurized);
    }

    #[test]
    fn experience_serde_roundtrip() {
        let now = Utc::now();
        let exp = Experience {
            id: Uuid::new_v4(),
            agent_id: None,
            session_id: None,
            goal: "Test".into(),
            reasoning_summary: None,
            actions: serde_json::json!([]),
            files_changed: serde_json::json!([]),
            result: None,
            lessons_learned: None,
            confidence: None,
            duration_seconds: None,
            tags: vec![],
            related_project: None,
            embedding: None,
            fts: None,
            is_procedurized: true,
            created_at: now,
        };
        let json = serde_json::to_string(&exp).unwrap();
        let decoded: Experience = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.goal, "Test");
        assert!(decoded.is_procedurized);
    }
}
