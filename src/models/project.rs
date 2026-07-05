//! Project model — matches the `projects` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A codebase or project registered in the platform.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub root_path: Option<String>,
    pub repo_url: Option<String>,
    pub language: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_project_with_valid_data() {
        let now = Utc::now();
        let project = Project {
            id: Uuid::new_v4(),
            name: "memory-platform".into(),
            description: Some("Rust rewrite of memory manager".into()),
            root_path: Some("/home/user/memory-platform-rust".into()),
            repo_url: Some("https://github.com/org/memory-platform".into()),
            language: Some("rust".into()),
            metadata: serde_json::json!({"stars": 42}),
            created_at: now,
            updated_at: now,
        };
        assert_eq!(project.name, "memory-platform");
        assert_eq!(project.language, Some("rust".into()));
    }

    #[test]
    fn project_serde_roundtrip() {
        let now = Utc::now();
        let project = Project {
            id: Uuid::new_v4(),
            name: "test-project".into(),
            description: None,
            root_path: None,
            repo_url: None,
            language: None,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&project).unwrap();
        let decoded: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "test-project");
    }
}
