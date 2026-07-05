//! Procedure model — matches the `procedures` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A reusable workflow generated from successful experiences.
///
/// Procedures are not hand-written; they are extracted from repeated
/// successful task patterns and stored for future replay.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Procedure {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub steps: serde_json::Value,
    pub trigger_pattern: Option<String>,
    pub source_experience_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub success_rate: f64,
    pub times_used: i32,
    pub confidence: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_procedure_with_valid_data() {
        let now = Utc::now();
        let proc = Procedure {
            id: Uuid::new_v4(),
            name: "deploy-mt5-ea".into(),
            description: Some("Deploy an MT5 EA to production".into()),
            steps: serde_json::json!([
                {"action": "compile", "tool": "MetaEditor"},
                {"action": "copy", "target": "Experts/"}
            ]),
            trigger_pattern: Some("deploy.*ea".into()),
            source_experience_id: Some(Uuid::new_v4()),
            tags: vec!["mt5".into(), "deploy".into()],
            success_rate: 0.95,
            times_used: 12,
            confidence: 0.9,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(proc.name, "deploy-mt5-ea");
        assert_eq!(proc.success_rate, 0.95);
        assert_eq!(proc.times_used, 12);
        assert_eq!(proc.tags.len(), 2);
    }

    #[test]
    fn procedure_serde_roundtrip() {
        let now = Utc::now();
        let proc = Procedure {
            id: Uuid::new_v4(),
            name: "test-proc".into(),
            description: None,
            steps: serde_json::json!([{"step": 1}]),
            trigger_pattern: None,
            source_experience_id: None,
            tags: vec![],
            success_rate: 0.0,
            times_used: 0,
            confidence: 0.0,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&proc).unwrap();
        let decoded: Procedure = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "test-proc");
        assert_eq!(decoded.success_rate, 0.0);
    }
}
