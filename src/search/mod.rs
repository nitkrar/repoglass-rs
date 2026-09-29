//! Retrieval tiers, their fusion, and the reranking layer.

pub mod boosting;
pub mod fuse;
pub mod lexical;
pub mod penalties;
pub mod rank;
pub mod tokens;

use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn ident_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap())
}

/// A bare symbol, a namespace-qualified name, or anything carrying an
/// uppercase letter or underscore. A plain lowercase word is prose.
pub fn is_symbol_query(query: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(concat!(
        r"^(?:",
        r"[A-Za-z_][A-Za-z0-9_]*(?:(?:::|\\|->|\.)[A-Za-z_][A-Za-z0-9_]*)+",
        r"|_[A-Za-z0-9_]*",
        r"|[A-Za-z][A-Za-z0-9]*[A-Z_][A-Za-z0-9_]*",
        r"|[A-Z][A-Za-z0-9]*",
        r")$")).unwrap());
    re.is_match(crate::pyfmt::py_strip(query))
}

/// Whether a query reads as a question rather than an identifier.
pub fn looks_like_prose(query: &str, min_words: i64) -> bool {
    ident_re().find_iter(query).count() as i64 >= min_words
}
