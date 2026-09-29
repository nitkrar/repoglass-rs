//! Identifier splitting for the reranking layer.

use fancy_regex::Regex as FRegex;
use std::collections::HashSet;
use std::sync::OnceLock;

fn camel_re() -> &'static FRegex {
    static RE: OnceLock<FRegex> = OnceLock::new();
    RE.get_or_init(|| FRegex::new(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+").unwrap())
}

const STOPWORDS: &[&str] = &["a", "an", "and", "are", "as", "at", "be", "by", "do", "does", "for", "from",
    "has", "have", "how", "if", "in", "is", "it", "not", "of", "on", "or", "the", "to", "was", "what",
    "when", "where", "which", "who", "why", "with"];

/// `HandlerStack` -> `[handlerstack, handler, stack]`: the compound kept
/// alongside its parts.
pub fn split_identifier(token: &str) -> Vec<String> {
    let lower = token.to_lowercase();
    let parts: Vec<String> = if token.contains('_') {
        lower.split('_').filter(|p| !p.is_empty()).map(String::from).collect()
    } else {
        camel_re().find_iter(token).map(|m| m.unwrap().as_str().to_lowercase()).collect()
    };
    let mut out = vec![lower];
    if parts.len() >= 2 {
        out.extend(parts);
    }
    out
}

pub fn stem_keywords(query: &str) -> HashSet<String> {
    super::ident_re().find_iter(query).map(|m| m.as_str())
        .filter(|w| w.chars().count() > 2)
        .map(str::to_lowercase)
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}
