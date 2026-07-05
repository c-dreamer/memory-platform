//! CodeChange model — matches the `code_changes` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A record of a coding task: problem, solution, files changed, commit info.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CodeChange {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub problem: Option<String>,
    pub solution: Option<String>,
    pub files_changed: serde_json::Value,
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub embedding: Option<Embedding>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_code_change_with_valid_data() {
        let now = Utc::now();
        let change = CodeChange {
            id: Uuid::new_v4(),
            agent_id: Some(Uuid::new_v4()),
            session_id: Some(Uuid::new_v4()),
            project_id: Some(Uuid::new_v4()),
            problem: Some("Fix null pointer".into()),
            solution: Some("Added Option<T>".into()),
            files_changed: serde_json::json!(["src/lib.rs"]),
            commit_hash: Some("abc123def".into()),
            branch: Some("fix/null-ptr".into()),
            embedding: None,
            tags: vec!["bugfix".into()],
            created_at: now,
        };
        assert_eq!(change.problem, Some("Fix null pointer".into()));
        assert_eq!(change.tags.len(), 1);
    }

    #[test]
    fn code_change_serde_roundtrip() {
        let now = Utc::now();
        let change = CodeChange {
            id: Uuid::new_v4(),
            agent_id: None,
            session_id: None,
            project_id: None,
            problem: None,
            solution: None,
            files_changed: serde_json::json!([]),
            commit_hash: None,
            branch: None,
            embedding: None,
            tags: vec![],
            created_at: now,
        };
        let json = serde_json::to_string(&change).unwrap();
        let decoded: CodeChange = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.files_changed, serde_json::json!([]));
    }
}
