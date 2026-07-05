//! Agent model — matches the `agents` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An agent registered in the memory platform.
///
/// Agents are entities that can write to memory: opencode sessions,
/// MT5 trading bots, Binance connectors, human operators, etc.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Agent {
    pub id: Uuid,
    pub name: String,
    pub agent_type: String,
    pub capabilities: Vec<String>,
    pub metadata: serde_json::Value,
    pub is_active: bool,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_agent_with_valid_data() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "test-agent".into(),
            agent_type: "opencode".into(),
            capabilities: vec!["code".into(), "search".into()],
            metadata: serde_json::json!({"version": "1.0"}),
            is_active: true,
            last_seen_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.agent_type, "opencode");
        assert!(agent.is_active);
        assert_eq!(agent.capabilities.len(), 2);
    }

    #[test]
    fn agent_serde_roundtrip() {
        let now = Utc::now();
        let agent = Agent {
            id: Uuid::new_v4(),
            name: "serde-agent".into(),
            agent_type: "human".into(),
            capabilities: vec!["review".into()],
            metadata: serde_json::json!({}),
            is_active: false,
            last_seen_at: None,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&agent).unwrap();
        let decoded: Agent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "serde-agent");
        assert!(!decoded.is_active);
    }
}
