use chrono::{DateTime, Utc};

use crate::model::{parse_rfc3339, Memory};

/// Half-life (in days) of the recency weighting: a memory loses half of its
/// recency factor every `HALF_LIFE_DAYS` days.
pub const HALF_LIFE_DAYS: f64 = 30.0;

/// Weights for the scoring model.
pub const TEXT_MATCH_SCORE: f64 = 2.0;
pub const TAG_MATCH_SCORE: f64 = 3.0;
pub const FULL_PHRASE_BONUS: f64 = 3.0;

/// Exponential time decay: `0.5^(age_days / half_life_days)`.
/// A fresh memory decays to 1.0, a `half_life_days`-old one to 0.5.
pub fn time_decay(created_at: &str, now: DateTime<Utc>, half_life_days: f64) -> f64 {
    let age_days = match parse_rfc3339(created_at) {
        Some(t) => (now - t).num_seconds().max(0) as f64 / 86_400.0,
        None => return 1.0, // unparseable timestamps keep full weight
    };
    0.5f64.powf(age_days / half_life_days.max(f64::MIN_POSITIVE))
}

/// Split a query into lowercase search terms.
pub fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Match score (text + tag hits) for a memory; 0.0 means "no match".
pub fn match_score(mem: &Memory, query_terms: &[String]) -> f64 {
    if query_terms.is_empty() {
        return 1.0; // no query: everything matches at baseline
    }
    let text_lower = mem.text.to_lowercase();
    let mut score = 0.0;
    for term in query_terms {
        if text_lower.contains(term.as_str()) {
            score += TEXT_MATCH_SCORE;
        }
        if mem.tags.iter().any(|tag| tag.to_lowercase() == *term) {
            score += TAG_MATCH_SCORE;
        }
    }
    let full = query_terms.join(" ");
    if query_terms.len() > 1 && text_lower.contains(&full) {
        score += FULL_PHRASE_BONUS;
    }
    score
}

/// Total relevance score = match score (weighted by recency) + importance.
/// Memories with zero match score are not relevant at all (unless there is
/// no query, in which case the baseline of 1.0 applies).
pub fn score_memory(mem: &Memory, query_terms: &[String], now: DateTime<Utc>) -> f64 {
    let base = match_score(mem, query_terms);
    if base <= 0.0 {
        return 0.0;
    }
    base * time_decay(&mem.created_at, now, HALF_LIFE_DAYS) + f64::from(mem.importance)
}

/// Ranked search results, best first. `tag` filters to memories carrying it.
pub fn search(
    memories: &[Memory],
    query: &str,
    tag: Option<&str>,
    limit: usize,
) -> Vec<(Memory, f64)> {
    let now = Utc::now();
    let query_terms = terms(query);
    let mut scored: Vec<(Memory, f64)> = memories
        .iter()
        .filter(|m| tag.is_none_or(|t| m.tags.iter().any(|x| x == t)))
        .map(|m| {
            let s = score_memory(m, &query_terms, now);
            (m.clone(), s)
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| b.0.created_at.cmp(&a.0.created_at))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::new_memory;

    fn fixed_now() -> DateTime<Utc> {
        Utc::now()
    }

    fn mem_with_age(text: &str, tags: &[&str], age_days: f64, importance: f32) -> Memory {
        let mut m = new_memory(
            text.to_string(),
            tags.iter().map(|s| s.to_string()).collect(),
            importance,
            None,
        );
        let past = (Utc::now() - chrono::Duration::milliseconds((age_days * 86_400_000.0) as i64))
            .to_rfc3339();
        m.created_at = past.clone();
        m.updated_at = past;
        m
    }

    #[test]
    fn time_decay_is_exponential() {
        let now = fixed_now();
        assert!((time_decay(&now.to_rfc3339(), now, 30.0) - 1.0).abs() < 1e-9);
        let half = (now - chrono::Duration::days(30)).to_rfc3339();
        assert!((time_decay(&half, now, 30.0) - 0.5).abs() < 1e-6);
        let double = (now - chrono::Duration::days(60)).to_rfc3339();
        assert!((time_decay(&double, now, 30.0) - 0.25).abs() < 1e-6);
        // Malformed timestamp keeps full weight instead of panicking.
        assert!((time_decay("garbage", now, 30.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn time_decay_never_negative() {
        let now = fixed_now();
        let future = (now + chrono::Duration::days(5)).to_rfc3339();
        assert!((time_decay(&future, now, 30.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn text_match_scores_beat_no_match() {
        let now = fixed_now();
        let hit = mem_with_age("deploy the redis cluster", &[], 0.0, 0.5);
        let miss = mem_with_age("buy groceries", &[], 0.0, 0.5);
        let terms_v = terms("redis deploy");
        assert!(match_score(&hit, &terms_v) > 0.0);
        assert!(match_score(&miss, &terms_v) == 0.0);
        assert!(score_memory(&hit, &terms_v, now) > 0.0);
        assert!(score_memory(&miss, &terms_v, now) == 0.0);
    }

    #[test]
    fn tag_exact_match_scores() {
        let tagged = mem_with_age("something unrelated", &["rust"], 0.0, 0.0);
        let t = terms("RUST");
        // Case-insensitive exact tag hit even though the text does not match.
        assert!(match_score(&tagged, &t) >= TAG_MATCH_SCORE);
        // A non-matching tag on the query gives nothing.
        let other = mem_with_age("something unrelated", &["python"], 0.0, 0.0);
        assert!(match_score(&other, &t) == 0.0);
    }

    #[test]
    fn newer_memory_ranks_higher_on_equal_terms() {
        let now = fixed_now();
        let fresh = mem_with_age("release checklist", &[], 0.0, 0.5);
        let old = mem_with_age("release checklist", &[], 90.0, 0.5);
        let t = terms("release");
        assert!(score_memory(&fresh, &t, now) > score_memory(&old, &t, now));
    }

    #[test]
    fn importance_boosts_ranking() {
        let now = fixed_now();
        let critical = mem_with_age("api key rotation", &[], 0.0, 1.0);
        let trivial = mem_with_age("api key rotation", &[], 0.0, 0.1);
        let t = terms("api");
        assert!(score_memory(&critical, &t, now) > score_memory(&trivial, &t, now));
    }

    #[test]
    fn full_phrase_beats_scattered_words() {
        let now = fixed_now();
        let phrase = mem_with_age("error handling in async code", &[], 0.0, 0.0);
        let scattered = mem_with_age("error handling for async and code review", &[], 0.0, 0.0);
        let t = terms("async code");
        assert!(score_memory(&phrase, &t, now) > score_memory(&scattered, &t, now));
    }

    #[test]
    fn empty_query_baselines_everything() {
        let now = fixed_now();
        let m = mem_with_age("anything", &[], 0.0, 0.5);
        assert!((match_score(&m, &[]) - 1.0).abs() < 1e-9);
        assert!(score_memory(&m, &[], now) > 0.0);
    }

    #[test]
    fn search_orders_filters_and_limits() {
        let a = mem_with_age("project kickoff notes", &["work"], 0.0, 0.3);
        let b = mem_with_age("project retro notes", &["work"], 0.0, 0.9);
        let _c = mem_with_age("unrelated", &["home"], 0.0, 1.0);
        let all = vec![a.clone(), b.clone()];
        let ranked = search(&all, "project", Some("work"), 10);
        assert_eq!(ranked.len(), 2);
        // importance 0.9 ranks above 0.3 for identical matches
        assert_eq!(ranked[0].0.id, b.id);
        assert!(ranked[0].1 > ranked[1].1);
        let limited = search(&all, "project", None, 1);
        assert_eq!(limited.len(), 1);
        let tag_only = search(&all, "", Some("home"), 10);
        assert!(tag_only.is_empty()); // tag "home" not in this slice
    }
}
