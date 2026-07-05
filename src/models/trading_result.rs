//! TradingResult model — matches the `trading_results` table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::embedding::Embedding;

/// A backtest or live trade record from MT5 / Binance / BTC agents.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TradingResult {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    pub ea_version: Option<String>,
    pub strategy: Option<String>,
    pub symbol: Option<String>,
    pub timeframe: Option<String>,
    pub trade_type: Option<String>,
    pub direction: Option<String>,
    pub entry_price: Option<f64>,
    pub exit_price: Option<f64>,
    pub profit_factor: Option<f64>,
    pub drawdown: Option<f64>,
    pub win_rate: Option<f64>,
    pub total_trades: Option<i32>,
    pub net_profit: Option<f64>,
    pub duration_days: Option<i32>,
    pub indicators: serde_json::Value,
    pub inputs: serde_json::Value,
    pub notes: Option<String>,
    pub embedding: Option<Embedding>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn construct_trading_result_with_valid_data() {
        let now = Utc::now();
        let result = TradingResult {
            id: Uuid::new_v4(),
            agent_id: Some(Uuid::new_v4()),
            ea_version: Some("2.1.0".into()),
            strategy: Some("MACD crossover".into()),
            symbol: Some("EURUSD".into()),
            timeframe: Some("H1".into()),
            trade_type: Some("backtest".into()),
            direction: Some("long".into()),
            entry_price: Some(1.0850),
            exit_price: Some(1.0900),
            profit_factor: Some(1.5),
            drawdown: Some(0.05),
            win_rate: Some(0.65),
            total_trades: Some(100),
            net_profit: Some(500.0),
            duration_days: Some(30),
            indicators: serde_json::json!({"rsi": 14}),
            inputs: serde_json::json!({"lot": 0.01}),
            notes: Some("Good run".into()),
            embedding: None,
            created_at: now,
        };
        assert_eq!(result.symbol, Some("EURUSD".into()));
        assert_eq!(result.trade_type, Some("backtest".into()));
        assert!(result.profit_factor.is_some());
    }

    #[test]
    fn trading_result_serde_roundtrip() {
        let now = Utc::now();
        let result = TradingResult {
            id: Uuid::new_v4(),
            agent_id: None,
            ea_version: None,
            strategy: None,
            symbol: None,
            timeframe: None,
            trade_type: None,
            direction: None,
            entry_price: None,
            exit_price: None,
            profit_factor: None,
            drawdown: None,
            win_rate: None,
            total_trades: None,
            net_profit: None,
            duration_days: None,
            indicators: serde_json::json!({}),
            inputs: serde_json::json!({}),
            notes: None,
            embedding: None,
            created_at: now,
        };
        let json = serde_json::to_string(&result).unwrap();
        let decoded: TradingResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.indicators, serde_json::json!({}));
    }
}
