//! Ranked lists and the fusers that merge them.

use std::collections::HashMap;

pub const RRF_K: f64 = 60.0;

/// One item from one source. `score` is higher-is-better; `raw` is the
/// source's own value, kept for display.
#[derive(Clone, Copy, Debug)]
pub struct Scored {
    pub id: i64,
    pub score: f64,
    pub raw: f64,
}

#[derive(Clone, Debug)]
pub struct RankedList {
    pub source: String,
    /// Best first; equal scores by ascending id.
    pub items: Vec<Scored>,
    pub warnings: Vec<String>,
}

impl RankedList {
    pub fn new(source: &str, items: Vec<Scored>) -> RankedList {
        RankedList { source: source.into(), items, warnings: vec![] }
    }
}

/// Best first, ties by ascending id.
fn ordered(scores: HashMap<i64, f64>) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Reciprocal rank fusion: each list adds `weight / (k + rank)`. A source
/// missing from `weights` weighs 1.0.
pub fn rrf(lists: &[&RankedList], weights: Option<&HashMap<&str, f64>>) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for rl in lists {
        let w = weights.and_then(|m| m.get(rl.source.as_str()).copied()).unwrap_or(1.0);
        if w == 0.0 {
            continue;
        }
        for (rank, s) in rl.items.iter().enumerate() {
            *scores.entry(s.id).or_insert(0.0) += w / (RRF_K + (rank + 1) as f64);
        }
    }
    ordered(scores)
}

/// The first non-empty list as it stands.
pub fn first(lists: &[&RankedList]) -> Vec<(i64, f64)> {
    lists.iter().find(|rl| !rl.items.is_empty())
        .map(|rl| ordered(rl.items.iter().map(|s| (s.id, s.score)).collect()))
        .unwrap_or_default()
}

/// semsift's min-max normalisation: scaled by the largest magnitude
/// first, then mapped onto [0, 1]; all-equal values become 1.0.
pub fn min_max(values: &[f64]) -> Vec<f64> {
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high == low {
        return vec![1.0; values.len()];
    }
    let scale = values.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let scaled: Vec<f64> = values.iter().map(|v| v / scale).collect();
    let low = scaled.iter().copied().fold(f64::INFINITY, f64::min);
    let high = scaled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    scaled.iter().map(|v| (v - low) / (high - low)).collect()
}

/// `blend([one list], normalise="min-max")`: one list's scores normalised.
pub fn blend_one(items: &[(i64, f64)]) -> Vec<(i64, f64)> {
    if items.is_empty() {
        return vec![];
    }
    let normed = min_max(&items.iter().map(|p| p.1).collect::<Vec<_>>());
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for ((id, _), n) in items.iter().zip(normed) {
        *scores.entry(*id).or_insert(0.0) += 1.0 * n;
    }
    ordered(scores)
}
