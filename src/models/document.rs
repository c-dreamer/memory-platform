//! Document model — matches the `documents` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A document ingested from the Obsidian vault or other sources.
///
/// Documents are read-only mirrors of external content with full-text
/// search and vector embedding for hybrid retrieval.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Document {
    pub id: Uuid,
    pub path: String,
    pub vault_section: Option<String>,
    pub title: Option<String>,
    pub content: String,
    pub checksum: Option<String>,
    pub frontmatter: serde_json::Value,
    pub embedding: Option<Embedding>,
    #[sqlx(skip)]
    pub fts: Option<String>,
    pub token_count: Option<i32>,
    pub file_size_bytes: Option<i32>,
    pub file_modified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_document_with_valid_data() {
        let now = Utc::now();
        let doc = Document {
            id: Uuid::new_v4(),
            path: "/vault/notes/rust.md".into(),
            vault_section: Some("notes".into()),
            title: Some("Rust Notes".into()),
            content: "# Rust\nMemory safety without GC.".into(),
            checksum: Some("abc123".into()),
            frontmatter: serde_json::json!({"tags": ["rust"]}),
            embedding: None,
            fts: None,
            token_count: Some(150),
            file_size_bytes: Some(1024),
            file_modified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        assert_eq!(doc.path, "/vault/notes/rust.md");
        assert_eq!(doc.title, Some("Rust Notes".into()));
        assert!(doc.token_count.is_some());
    }

    #[test]
    fn document_serde_roundtrip() {
        let now = Utc::now();
        let doc = Document {
            id: Uuid::new_v4(),
            path: "/vault/test.md".into(),
            vault_section: None,
            title: None,
            content: "test".into(),
            checksum: None,
            frontmatter: serde_json::json!({}),
            embedding: None,
            fts: None,
            token_count: None,
            file_size_bytes: None,
            file_modified_at: None,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&doc).unwrap();
        let decoded: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.path, "/vault/test.md");
    }
}
