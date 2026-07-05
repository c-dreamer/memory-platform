//! Events model — event types and payloads for the event system.
//!
//! These structs are NOT backed by a single database table. Events are
//! dispatched to the event bus and may be persisted to different tables
//! depending on their type.

use serde::{Deserialize, Serialize};

/// The type of event being dispatched.
///
/// Each variant maps to a specific target table via `EventCreate::target_table`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    MemoryStore,
    TaskComplete,
    TradeOpened,
    TradeClosed,
    TradeSignal,
    BacktestComplete,
    Insight,
    Observation,
}

/// An event creation payload sent to the event bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCreate {
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub payload: serde_json::Value,
}

impl EventCreate {
    /// Return the target database table for this event type.
    ///
    /// Each event type maps to the table where its data should be persisted.
    #[must_use]
    pub fn target_table(&self) -> &str {
        match self.event_type {
            EventType::MemoryStore => "memories",
            EventType::TaskComplete => "experiences",
            EventType::TradeOpened | EventType::TradeClosed | EventType::TradeSignal => {
                "trading_results"
            }
            EventType::BacktestComplete => "trading_results",
            EventType::Insight => "memories",
            EventType::Observation => "memories",
        }
    }
}

/// Response returned after an event is processed and persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventResponse {
    pub event_id: String,
    pub memory_id: String,
    pub status: String,
    pub table: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_serde_roundtrip() {
        let variants = [
            EventType::MemoryStore,
            EventType::TaskComplete,
            EventType::TradeOpened,
            EventType::TradeClosed,
            EventType::TradeSignal,
            EventType::BacktestComplete,
            EventType::Insight,
            EventType::Observation,
        ];
        for variant in &variants {
            let json = serde_json::to_string(variant).unwrap();
            let decoded: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, *variant);
        }
    }

    #[test]
    fn event_type_snake_case_serialization() {
        let json = serde_json::to_string(&EventType::MemoryStore).unwrap();
        assert_eq!(json, "\"memory_store\"");
        let json = serde_json::to_string(&EventType::TaskComplete).unwrap();
        assert_eq!(json, "\"task_complete\"");
        let json = serde_json::to_string(&EventType::BacktestComplete).unwrap();
        assert_eq!(json, "\"backtest_complete\"");
    }

    #[test]
    fn event_create_target_table_mapping() {
        let event = EventCreate {
            agent_id: Some("agent-1".into()),
            session_id: None,
            event_type: EventType::MemoryStore,
            payload: serde_json::json!({"content": "test"}),
        };
        assert_eq!(event.target_table(), "memories");

        let event = EventCreate {
            agent_id: None,
            session_id: None,
            event_type: EventType::TradeOpened,
            payload: serde_json::json!({}),
        };
        assert_eq!(event.target_table(), "trading_results");

        let event = EventCreate {
            agent_id: None,
            session_id: None,
            event_type: EventType::TaskComplete,
            payload: serde_json::json!({}),
        };
        assert_eq!(event.target_table(), "experiences");

        let event = EventCreate {
            agent_id: None,
            session_id: None,
            event_type: EventType::Insight,
            payload: serde_json::json!({}),
        };
        assert_eq!(event.target_table(), "memories");
    }

    #[test]
    fn event_create_serde_roundtrip() {
        let event = EventCreate {
            agent_id: Some("agent-1".into()),
            session_id: Some("session-1".into()),
            event_type: EventType::Observation,
            payload: serde_json::json!({"note": "something happened"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: EventCreate = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.agent_id, Some("agent-1".into()));
        assert_eq!(decoded.event_type, EventType::Observation);
    }

    #[test]
    fn event_response_serde_roundtrip() {
        let response = EventResponse {
            event_id: "evt-001".into(),
            memory_id: "mem-001".into(),
            status: "stored".into(),
            table: "memories".into(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let decoded: EventResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.event_id, "evt-001");
        assert_eq!(decoded.table, "memories");
    }
}
