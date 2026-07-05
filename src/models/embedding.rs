//! Embedding wrapper type for VECTOR(384) columns.
//!
//! PostgreSQL pgvector stores vectors as `'[x,y,z]'` text. This wrapper
//! implements sqlx::Type and sqlx::Decode to read that format, and serde
//! Serialize/Deserialize to produce/consume `Vec<f32>`.

use serde::{Deserialize, Serialize};

/// A 384-dimensional embedding vector.
///
/// Stored as `VECTOR(384)` in PostgreSQL (pgvector extension).
/// Serialized as `Vec<f32>` in JSON. sqlx reads the `'[x,y,z]'` text format.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding(pub Vec<f32>);

impl Embedding {
    /// Create a new embedding from a vec of f32 values.
    #[must_use]
    pub fn new(values: Vec<f32>) -> Self {
        Self(values)
    }

    /// Access the inner vec.
    #[must_use]
    pub fn as_vec(&self) -> &Vec<f32> {
        &self.0
    }

    /// Consume and return the inner vec.
    #[must_use]
    pub fn into_inner(self) -> Vec<f32> {
        self.0
    }
}

// --- serde: serialize as Vec<f32> ---

impl Serialize for Embedding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Embedding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<f32>::deserialize(deserializer).map(Embedding)
    }
}

// --- sqlx: decode from pgvector text format '[x,y,z]' ---

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Embedding {
    fn decode(
        value: <sqlx::Postgres as sqlx::Database>::ValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let s: &str = <&str as sqlx::Decode<'r, sqlx::Postgres>>::decode(value)?;
        parse_pgvector(s).map(Embedding)
    }
}

impl sqlx::Type<sqlx::Postgres> for Embedding {
    fn type_info() -> <sqlx::Postgres as sqlx::Database>::TypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

/// Parse a pgvector text representation like `'[0.1,0.2,0.3]'` into `Vec<f32>`.
fn parse_pgvector(s: &str) -> Result<Vec<f32>, Box<dyn std::error::Error + Send + Sync>> {
    let s = s.trim();
    // Strip surrounding brackets and optional quotes
    let inner = s
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .unwrap_or(s);
    let inner = inner
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .ok_or_else(|| format!("invalid pgvector format: {s}"))?;

    if inner.is_empty() {
        return Ok(Vec::new());
    }

    inner
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .map_err(|e| format!("invalid float in pgvector '{part}': {e}").into())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pgvector_valid() {
        let result = parse_pgvector("'[0.1,0.2,0.3]'").unwrap();
        assert_eq!(result, vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn parse_pgvector_empty() {
        let result = parse_pgvector("'[]'").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_pgvector_no_quotes() {
        let result = parse_pgvector("[1.0,2.0]").unwrap();
        assert_eq!(result, vec![1.0, 2.0]);
    }

    #[test]
    fn embedding_serde_roundtrip() {
        let emb = Embedding::new(vec![0.1, 0.2, 0.3]);
        let json = serde_json::to_string(&emb).unwrap();
        let decoded: Embedding = serde_json::from_str(&json).unwrap();
        assert_eq!(emb, decoded);
    }

    #[test]
    fn embedding_construct_and_access() {
        let emb = Embedding::new(vec![1.0, 2.0, 3.0]);
        assert_eq!(emb.as_vec(), &vec![1.0, 2.0, 3.0]);
        assert_eq!(emb.into_inner(), vec![1.0, 2.0, 3.0]);
    }
}
