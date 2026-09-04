//! Coercion helpers shared by the CLI (piped stdin JSON) and the HTTP layer
//! (query strings and `/invoke` params). Values may arrive in slightly
//! different shapes (array vs comma-separated string, number vs string), so
//! all accessors are lenient and fall back to `None`.

use serde_json::Value;

/// Extract a string field from a JSON object.
pub fn get_string(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(s)) = obj.get(*key) {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Extract a usize field; accepts numbers and numeric strings.
pub fn get_usize(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<usize> {
    for key in keys {
        match obj.get(*key) {
            Some(Value::Number(n)) => {
                if let Some(u) = n.as_u64() {
                    return Some(u as usize);
                }
            }
            Some(Value::String(s)) => {
                if let Ok(u) = s.parse::<usize>() {
                    return Some(u);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract an f32 field; accepts numbers and numeric strings.
pub fn get_f32(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f32> {
    for key in keys {
        match obj.get(*key) {
            Some(Value::Number(n)) => {
                if let Some(f) = n.as_f64() {
                    return Some(f as f32);
                }
            }
            Some(Value::String(s)) => {
                if let Ok(f) = s.parse::<f32>() {
                    return Some(f);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract tags: either a JSON array of strings or a single comma-separated
/// string. Entries are trimmed and empties are dropped.
pub fn get_tags(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Vec<String>> {
    for key in keys {
        match obj.get(*key) {
            Some(Value::Array(items)) => {
                let tags: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect();
                return Some(tags);
            }
            Some(Value::String(s)) => return Some(split_csv(s)),
            _ => {}
        }
    }
    None
}

/// Normalize `--tags a,b` style CLI values the same way as JSON input.
pub fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn get_string_picks_first_present_key() {
        let o = obj(json!({"query": "hello", "q": "ignored"}));
        assert_eq!(get_string(&o, &["query", "q"]), Some("hello".into()));
        assert_eq!(get_string(&o, &["missing"]), None);
    }

    #[test]
    fn get_usize_accepts_number_and_string() {
        let o = obj(json!({"a": 5, "b": "7"}));
        assert_eq!(get_usize(&o, &["a"]), Some(5));
        assert_eq!(get_usize(&o, &["b"]), Some(7));
        assert_eq!(get_usize(&o, &["c"]), None);
    }

    #[test]
    fn get_f32_accepts_number_and_string() {
        let o = obj(json!({"a": 0.8, "b": "0.3"}));
        assert!((get_f32(&o, &["a"]).unwrap() - 0.8).abs() < 1e-6);
        assert!((get_f32(&o, &["b"]).unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(get_f32(&o, &["c"]), None);
    }

    #[test]
    fn get_tags_accepts_array_and_csv() {
        let arr = obj(json!({"tags": ["a", " b ", "", "c"]}));
        assert_eq!(
            get_tags(&arr, &["tags"]),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
        let csv = obj(json!({"tags": "x,y , z"}));
        assert_eq!(
            get_tags(&csv, &["tags"]),
            Some(vec!["x".into(), "y".into(), "z".into()])
        );
        assert_eq!(get_tags(&obj(json!({})), &["tags"]), None);
    }

    #[test]
    fn split_csv_trims_and_skips_empty() {
        assert_eq!(split_csv("a,,b ,"), vec!["a".to_string(), "b".to_string()]);
        assert!(split_csv(",,").is_empty());
    }
}
