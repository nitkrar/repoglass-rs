//! repoglass settings to an encoder. The same model must embed chunks
//! and queries; the stored vector space enforces it.

use crate::config::Settings;
use crate::semsift::encoders::Encoder;
use crate::semsift::space::{resolve_prefix, Side, VectorSpace};

/// Query instructions naming code search, for models trained to take a task.
const CODE_QUERY_PREFIXES: &[(&str, &str)] = &[
    ("qwen3-embedding", "Instruct: Given a question, retrieve code that answers it\nQuery: "),
    ("coderankembed", "Represent this query for searching relevant code: "),
];

pub fn doc_prefix(settings: &Settings) -> String {
    resolve_prefix(settings.embed_doc_prefix.as_deref(), &settings.embed_model, Side::Doc, &[])
}

/// The space `build(settings)` writes into, without loading a model.
pub fn space(settings: &Settings, dims: i64) -> VectorSpace {
    let variant = match settings.embed_backend.as_str() {
        "onnx" => settings.embed_onnx_file.as_str(),
        "http" => settings.embed_endpoint.as_str(),
        _ => "",
    };
    VectorSpace::of(&settings.embed_model, &settings.embed_backend, variant, Some(&doc_prefix(settings)), dims)
}

/// The configured encoder, or None when `embed_backend = "none"`.
pub fn build(settings: &Settings) -> anyhow::Result<Option<Encoder>> {
    let query = settings.embed_query_prefix.as_deref();
    let doc = settings.embed_doc_prefix.as_deref();
    match settings.embed_backend.as_str() {
        "none" => Ok(None),
        "static" => Ok(Some(Encoder::static_model(&settings.embed_model, query, doc, CODE_QUERY_PREFIXES)?)),
        "http" => {
            anyhow::ensure!(!settings.embed_endpoint.is_empty(), "embed_backend='http' requires embed_endpoint");
            Ok(Some(Encoder::http(&settings.embed_endpoint, &settings.embed_model, &settings.embed_api_key,
                                  query, doc, CODE_QUERY_PREFIXES)?))
        }
        "onnx" => anyhow::bail!("embed_backend='onnx' is not available in this build of repoglass; \
                                 use 'static', 'http' or 'none'"),
        other => anyhow::bail!("unknown embed_backend {other:?}; expected 'static', 'onnx', 'http' or 'none'"),
    }
}
