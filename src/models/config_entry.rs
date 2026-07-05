//! ConfigEntry model — matches the `config` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A runtime configuration key-value entry.
///
/// The `config` table uses `key` as its primary key (not UUID).
/// This is a simple KV store for platform settings.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConfigEntry {
    pub key: String,
    pub value: String,
    pub description: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_config_entry_with_valid_data() {
        let now = Utc::now();
        let entry = ConfigEntry {
            key: "embedding.model".into(),
            value: "all-MiniLM-L6-v2".into(),
            description: Some("Default embedding model".into()),
            updated_at: now,
        };
        assert_eq!(entry.key, "embedding.model");
        assert_eq!(entry.value, "all-MiniLM-L6-v2");
        assert!(entry.description.is_some());
    }

    #[test]
    fn config_entry_serde_roundtrip() {
        let now = Utc::now();
        let entry = ConfigEntry {
            key: "max_retries".into(),
            value: "3".into(),
            description: None,
            updated_at: now,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let decoded: ConfigEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.key, "max_retries");
        assert_eq!(decoded.value, "3");
    }
}
