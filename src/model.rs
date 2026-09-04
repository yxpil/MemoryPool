use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single memory entry in the pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Importance in [0.0, 1.0]; defaults to 0.5 when missing in stored data.
    #[serde(default = "default_importance")]
    pub importance: f32,
    /// Where the memory came from, e.g. "cli" or "bit".
    #[serde(default = "default_source")]
    pub source: String,
    /// RFC 3339 timestamps (UTC).
    pub created_at: String,
    pub updated_at: String,
}

fn default_importance() -> f32 {
    0.5
}

fn default_source() -> String {
    "cli".to_string()
}

/// Build a new memory with generated id and RFC 3339 timestamps.
pub fn new_memory(
    text: String,
    tags: Vec<String>,
    importance: f32,
    source: Option<String>,
) -> Memory {
    let now = now_rfc3339();
    Memory {
        id: uuid::Uuid::new_v4().to_string(),
        text,
        tags,
        importance: importance.clamp(0.0, 1.0),
        source: source.unwrap_or_else(|| "cli".to_string()),
        created_at: now.clone(),
        updated_at: now,
    }
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Parse an RFC 3339 timestamp; returns None when malformed.
pub fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_memory_defaults() {
        let m = new_memory("hello".into(), vec![], 0.5, None);
        assert_eq!(m.source, "cli");
        assert!((m.importance - 0.5).abs() < f32::EPSILON);
        assert!(!m.id.is_empty());
        assert_eq!(m.created_at, m.updated_at);
        assert!(parse_rfc3339(&m.created_at).is_some());
    }

    #[test]
    fn importance_is_clamped() {
        let m = new_memory("x".into(), vec![], 9.0, None);
        assert!((m.importance - 1.0).abs() < f32::EPSILON);
        let m = new_memory("x".into(), vec![], -3.0, None);
        assert!((m.importance - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn serde_defaults_on_missing_fields() {
        let json = r#"{"id":"a","text":"t","created_at":"2026-01-01T00:00:00+00:00","updated_at":"2026-01-01T00:00:00+00:00"}"#;
        let m: Memory = serde_json::from_str(json).unwrap();
        assert_eq!(m.source, "cli");
        assert!(m.tags.is_empty());
        assert!((m.importance - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_rfc3339_rejects_garbage() {
        assert!(parse_rfc3339("not-a-date").is_none());
        assert!(parse_rfc3339("2026-01-01T00:00:00Z").is_some());
    }
}
