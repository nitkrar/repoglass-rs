//! Tier weighting and the configured ranker. Fusion itself is semsift's.

use crate::config::Settings;
use crate::semsift::fuse::{blend_one, first, rrf, RankedList, Scored};
use std::collections::HashMap;

/// (id, raw) pairs as a best-first list. bm25 is lower-is-better, so the
/// lexical tier negates it; `preserve_order` ranks by position, for
/// sources whose raw values do not distinguish items.
pub fn ranked(tier: &str, pairs: &[(i64, f64)], lower_is_better: bool, preserve_order: bool) -> RankedList {
    let items = if preserve_order {
        let n = pairs.len();
        pairs.iter().enumerate().map(|(rank, &(id, raw))| Scored { id, score: (n - rank) as f64, raw }).collect()
    } else {
        let sign = if lower_is_better { -1.0 } else { 1.0 };
        let mut items: Vec<Scored> = pairs.iter().map(|&(id, raw)| Scored { id, score: sign * raw, raw }).collect();
        items.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
        items
    };
    RankedList::new(tier, items)
}

/// An identifier-shaped query is a job for BM25 and exact matching; a
/// sentence for the embedder. `exact` is never down-weighted.
pub fn tier_weights(settings: &Settings, is_prose: bool) -> HashMap<&'static str, f64> {
    let alpha = if is_prose { settings.alpha_prose.get() } else { settings.alpha_symbol.get() };
    HashMap::from([("exact", 1.0), ("vector", alpha), ("lexical", 1.0 - alpha)])
}

/// Apply the configured ranker: RRF, or the first non-empty tier. Scores
/// end on [0, 1], best first.
pub fn merge(lists: &[RankedList], settings: &Settings, is_prose: bool) -> Vec<(i64, f64)> {
    let populated: Vec<&RankedList> = lists.iter().filter(|rl| !rl.items.is_empty()).collect();
    if populated.is_empty() {
        return vec![];
    }
    if settings.ranker == "none" {
        let fused = first(&populated);
        let distinct: std::collections::HashSet<u64> = populated[0].items.iter().map(|s| s.raw.to_bits()).collect();
        if distinct.len() == 1 {
            return fused.into_iter().map(|(i, _)| (i, 1.0)).collect();
        }
        return blend_one(&fused);
    }
    let weights = settings.adaptive_alpha.then(|| tier_weights(settings, is_prose));
    blend_one(&rrf(&populated, weights.as_ref()))
}
