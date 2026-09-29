//! Render Settings as TOML.

use super::load::SECRET;
use super::schema::{Settings, Value, FIELDS};

/// Which TOML table each setting is presented under.
const GROUPS: &[(&str, &[&str])] = &[
    ("retrieval", &["ranker", "prose_min_words", "adaptive_alpha", "alpha_symbol", "alpha_prose",
                    "saturation_decay", "rerank", "file_coherence", "stem_boost",
                    "definition_boost", "candidate_depth", "content", "content_excluded",
                    "index_excluded"]),
    ("categories", &["doc_languages", "config_languages", "data_languages", "test_markers"]),
    ("embeddings", &["embed_backend", "embed_model", "embed_endpoint", "embed_api_key",
                     "embed_query_prefix", "embed_doc_prefix", "embed_providers",
                     "embed_onnx_file"]),
    ("extraction", &["coverage", "window_chars", "lexical_mode", "lexical_cap_chars",
                     "lexical_enrich", "split_identifiers", "distill_docs", "max_chunk_lines"]),
    ("corpus", &["data_dir", "gitignore", "hard_exclude", "max_file_bytes"]),
    ("refresh", &["refresh_mode", "rescan_after_seconds"]),
];

/// A secret never appears in a generated config: the loader would refuse it.
fn emittable(name: &str) -> bool {
    !SECRET.contains(&name)
}

/// Settings as TOML with their doc comments. Keys are commented out
/// unless `active`, and an unset optional key is always commented.
pub fn as_toml(settings: &Settings, active: bool) -> String {
    let head = if active {
        "# repoglass settings as resolved for this run."
    } else {
        "# repoglass settings. Every key is optional. Uncomment only\n\
         # what you want to change; the rest track the defaults."
    };
    let mut out: Vec<String> = vec![head.into(), String::new()];
    let mut placed: Vec<&str> = Vec::new();
    for (table, names) in GROUPS {
        let rows: Vec<&str> = names.iter().copied()
            .filter(|n| FIELDS.iter().any(|f| f.name == *n) && emittable(n)).collect();
        if rows.is_empty() {
            continue;
        }
        out.push(format!("[{table}]"));
        for name in rows {
            placed.push(name);
            let field = FIELDS.iter().find(|f| f.name == name).unwrap();
            for line in field.docs {
                out.push(format!("# {line}"));
            }
            out.push(line_for(settings, name, active));
            out.push(String::new());
        }
    }
    let mut missing: Vec<&str> = FIELDS.iter().map(|f| f.name)
        .filter(|n| !placed.contains(n) && emittable(n)).collect();
    missing.sort();
    if !missing.is_empty() {
        out.push("# not yet grouped".into());
        for name in missing {
            out.push(line_for(settings, name, active));
        }
    }
    format!("{}\n", out.join("\n").trim_end())
}

fn line_for(settings: &Settings, name: &str, active: bool) -> String {
    let value = settings.get(name);
    let unset = matches!(value, Value::OptStr(None));
    let prefix = if active && !unset { "" } else { "# " };
    format!("{prefix}{name} = {}", toml_value(&value))
}

/// One value as TOML. An unset value renders as `""`: TOML has no null.
/// Only `"` is escaped, as in Python 0.3.2.
fn toml_value(value: &Value) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\\\""));
    match value {
        Value::OptStr(None) => "\"\"".into(),
        Value::Bool(b) => if *b { "true".into() } else { "false".into() },
        Value::Int(i) => i.to_string(),
        Value::Float(n) => Value::Float(*n).repr(),
        Value::List(items) => format!("[{}]", items.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ")),
        Value::Str(s) | Value::OptStr(Some(s)) => quote(s),
    }
}
