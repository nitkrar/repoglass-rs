//! Records exchanged across module boundaries.

/// A file selected for indexing. `content_type` is resolved at walk time
/// and stored, so a filtered query tests a column.
#[derive(Clone, Debug)]
pub struct SourceFile {
    /// Relative to the index root, `/` separated.
    pub path: String,
    pub mtime_ns: i64,
    pub size: i64,
    pub lang: String,
    pub content_type: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Symbol {
    pub name: String,
    /// `def` or `ref`.
    pub tag: &'static str,
    pub path: String,
    /// 1-indexed.
    pub start_line: i64,
    pub end_line: i64,
    pub lang: String,
    /// A definition's header; None on a reference.
    pub signature: Option<String>,
    pub content_type: String,
    /// For a reference, the definition whose span contains it; None at
    /// module scope and on every definition.
    pub enclosing: Option<String>,
}

/// One embeddable unit: a definition span or a window, and its text.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub name: String,
    pub start_line: i64,
    pub end_line: i64,
    pub text: String,
    pub content_hash: String,
    /// The keyword text, stored only when the store cannot derive it
    /// from the path words and `text`.
    pub lexical_override: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub name: String,
    /// Divided by the top hit: 1.0 at rank 1, comparable within one call.
    pub score: f64,
    pub code: String,
    /// The definition's header, or the first non-blank line of a window.
    pub signature: Option<String>,
    /// Each matching tier's raw score: bm25 is negative and lower is
    /// better, cosine is in [-1, 1], exact is 1.0.
    pub tiers: Vec<(String, f64)>,
}

#[derive(Clone, Debug)]
pub struct RefreshReport {
    pub added: usize,
    pub changed: usize,
    pub deleted: usize,
    pub elapsed_s: f64,
}
