//! What decides whether two stored vectors are comparable, and the
//! model-family defaults for prefixes and pooling.

use crate::pyfmt::Json;

#[derive(Clone, Debug, PartialEq)]
pub struct VectorSpace {
    pub model: String,
    pub backend: String,
    pub variant: String,
    pub dims: i64,
    pub doc_prefix: String,
    pub pooling: String,
}

impl VectorSpace {
    /// The space an encoder built from these arguments writes into,
    /// resolved without loading a model.
    pub fn of(model: &str, backend: &str, variant: &str, doc_prefix: Option<&str>, dims: i64) -> VectorSpace {
        VectorSpace {
            model: model.into(),
            backend: backend.into(),
            variant: if backend == "http" { variant.trim_end_matches('/').into() } else { variant.into() },
            dims,
            doc_prefix: resolve_prefix(doc_prefix, model, Side::Doc, &[]),
            pooling: if backend == "onnx" { default_pooling(model).into() } else { String::new() },
        }
    }

    /// `json.dumps(asdict(space), sort_keys=True)`.
    pub fn to_json(&self) -> String {
        Json::obj(vec![
            ("backend", Json::str(&self.backend)),
            ("dims", Json::Int(self.dims)),
            ("doc_prefix", Json::str(&self.doc_prefix)),
            ("model", Json::str(&self.model)),
            ("pooling", Json::str(&self.pooling)),
            ("variant", Json::str(&self.variant)),
        ]).dumps()
    }

    pub fn from_json(text: &str) -> anyhow::Result<VectorSpace> {
        let v: serde_json::Value = serde_json::from_str(text)?;
        let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        Ok(VectorSpace {
            model: s("model"), backend: s("backend"), variant: s("variant"),
            dims: v["dims"].as_i64().unwrap_or(0), doc_prefix: s("doc_prefix"), pooling: s("pooling"),
        })
    }
}

impl std::fmt::Display for VectorSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VectorSpace(model={:?}, backend={:?}, variant={:?}, dims={}, doc_prefix={:?}, pooling={:?})",
               self.model, self.backend, self.variant, self.dims, self.doc_prefix, self.pooling)
    }
}

#[derive(Clone, Copy)]
pub enum Side {
    Query,
    Doc,
}

const QUERY_PREFIXES: &[(&str, &str)] = &[
    ("bge", "Represent this sentence for searching relevant passages: "),
    ("e5", "query: "),
    ("gte", ""),
    ("nomic-embed", "search_query: "),
];
const DOC_PREFIXES: &[(&str, &str)] = &[("e5", "passage: "), ("nomic-embed", "search_document: ")];
const POOLING: &[(&str, &str)] = &[
    ("bge", "cls"), ("e5", "mean"), ("gte", "mean"), ("qwen3-embedding", "last"),
    ("coderankembed", "mean"), ("all-minilm", "mean"), ("nomic-embed", "mean"),
];

/// The longest known family name contained in the model's last path part.
fn family(model: &str, extra: &[(&str, &str)]) -> String {
    let stem = model.rsplit('/').next().unwrap_or(model).to_lowercase();
    let mut known: Vec<&str> = POOLING.iter().chain(QUERY_PREFIXES).chain(DOC_PREFIXES).chain(extra)
        .map(|(k, _)| *k).collect();
    known.sort();
    known.dedup();
    // Python sorts by length only, stably over a set; the families here
    // never tie on length while both matching.
    known.sort_by_key(|k| std::cmp::Reverse(k.len()));
    known.into_iter().find(|f| stem.contains(f)).map(String::from).unwrap_or_default()
}

fn lookup(table: &[(&str, &str)], key: &str) -> Option<String> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
}

pub fn default_pooling(model: &str) -> String {
    lookup(POOLING, &family(model, &[])).unwrap_or_else(|| "cls".into())
}

/// An explicit prefix wins; `None` takes the model family's default and
/// `Some("")` means none. `extra` adds query-side families.
pub fn resolve_prefix(explicit: Option<&str>, model: &str, side: Side, extra: &[(&str, &str)]) -> String {
    if let Some(p) = explicit {
        return p.to_string();
    }
    match side {
        Side::Query => {
            let fam = family(model, extra);
            lookup(extra, &fam).or_else(|| lookup(QUERY_PREFIXES, &fam)).unwrap_or_default()
        }
        Side::Doc => lookup(DOC_PREFIXES, &family(model, &[])).unwrap_or_default(),
    }
}
