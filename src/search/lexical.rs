//! FTS5/BM25 keyword search.

use crate::store::{Scope, Store};
use fancy_regex::Regex as FRegex;
use std::sync::OnceLock;

/// Words carrying no retrieval signal in a natural-language query.
const STOP: &[&str] = &["how", "does", "do", "the", "where", "is", "are", "in", "to", "a", "an", "of", "and",
    "with", "it", "we", "for", "from", "on", "this", "that", "what", "when", "which", "work", "works", "done",
    "use", "used", "app"];

/// (chunk id, raw bm25) for any of the query's words.
pub fn keyword(store: &Store, query: &str, limit: usize, scope: &Scope) -> anyhow::Result<Vec<(i64, f64)>> {
    let terms = tokenize_query(query);
    if terms.is_empty() {
        return Ok(vec![]);
    }
    store.fts_search(&terms.join(" "), limit, scope)
}

/// Content words for an FTS5 MATCH expression. Identifiers are split as
/// well as kept, so `decryptVault` also matches `decrypt`.
pub fn tokenize_query(query: &str) -> Vec<String> {
    static PART: OnceLock<FRegex> = OnceLock::new();
    let part = PART.get_or_init(|| FRegex::new(r"[A-Z]+(?![a-z])|[A-Z][a-z]+|[a-z]+").unwrap());
    let mut words: Vec<String> = Vec::new();
    for m in super::ident_re().find_iter(query) {
        let raw = m.as_str();
        let low = raw.to_lowercase();
        if low.chars().count() > 2 && !STOP.contains(&low.as_str()) {
            words.push(low);
        }
        for p in part.find_iter(raw) {
            let p = p.unwrap().as_str().to_lowercase();
            if p.chars().count() > 2 && !STOP.contains(&p.as_str()) && !words.contains(&p) {
                words.push(p);
            }
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_keeps_and_splits_identifiers() {
        assert_eq!(tokenize_query("how does decryptVault work"), vec!["decryptvault", "decrypt", "vault"]);
    }
}
