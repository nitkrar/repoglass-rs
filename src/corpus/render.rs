//! Render chunk text for embedding and lexical search.

use crate::config::Settings;
use crate::pyfmt::char_prefix;
use crate::text::{humanise, parent_parts, path_stem};
use fancy_regex::Regex as FRegex;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

fn ident_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]{2,}").unwrap())
}

fn camel_re() -> &'static FRegex {
    static RE: OnceLock<FRegex> = OnceLock::new();
    RE.get_or_init(|| FRegex::new(r"[A-Z]+(?=[A-Z][a-z])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+").unwrap())
}

const IDENT_CAP: usize = 25;

/// The string that gets embedded: the raw span, unless `distill_docs` is
/// on and `lang` is a doc language, when it is a
/// `path :: kind :: comments :: identifiers` summary.
pub fn embed_text(path: &str, lang: &str, node_kind: &str, leading_comments: &[String],
                  body: &str, settings: &Settings) -> String {
    if !(settings.distill_docs && settings.doc_languages.iter().any(|l| l == lang)) {
        return body.to_string();
    }
    let mut idents: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for m in ident_re().find_iter(body) {
        let word = humanise(m.as_str());
        if !word.is_empty() && seen.insert(word.clone()) {
            idents.push(word);
        }
        if idents.len() >= IDENT_CAP {
            break;
        }
    }
    let parts = [humanise(path), node_kind.to_string(), leading_comments.join(" "), idents.join(" ")];
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" :: ")
}

/// The FTS text to store, or None when the store can derive it as the
/// file's path words followed by the chunk's text.
pub fn lexical_override(path: &str, body: &str, settings: &Settings) -> Option<String> {
    let rendered = lexical(path, body, settings);
    if rendered == format!("{}\n{body}", humanise(path)) { None } else { Some(rendered) }
}

/// The text FTS5 indexes: the humanised path, then the raw span, reshaped
/// by `lexical_mode`, `lexical_enrich` and `split_identifiers`.
pub fn lexical(path: &str, body: &str, settings: &Settings) -> String {
    let mut body = body.to_string();
    if settings.lexical_mode == "capped" {
        body = char_prefix(&body, settings.lexical_cap_chars.max(0) as usize).to_string();
    }
    let body = if settings.lexical_enrich {
        enriched(path, &body)
    } else {
        format!("{}\n{body}", humanise(path))
    };
    match settings.split_identifiers.as_str() {
        "inline" => split_inline(&body),
        "append" => format!("{body}\n{}", split_identifiers(&body)),
        _ => body,
    }
}

fn parts_of(raw: &str) -> Vec<String> {
    if raw.contains('_') {
        raw.to_lowercase().split('_').filter(|p| !p.is_empty()).map(String::from).collect()
    } else {
        camel_re().find_iter(raw).map(|m| m.unwrap().as_str().to_lowercase()).collect()
    }
}

/// Each compound identifier rewritten as `compound part part` in place,
/// keeping every occurrence's term frequency.
fn split_inline(text: &str) -> String {
    ident_re().replace_all(text, |caps: &regex::Captures| {
        let raw = &caps[0];
        let parts = parts_of(raw);
        if parts.len() < 2 {
            return raw.to_string();
        }
        let long: Vec<&str> = parts.iter().filter(|p| p.chars().count() > 2).map(String::as_str).collect();
        format!("{raw} {}", long.join(" "))
    }).into_owned()
}

/// The sub-words of every compound identifier, de-duplicated, for appending.
fn split_identifiers(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for m in ident_re().find_iter(text) {
        let parts = parts_of(m.as_str());
        if parts.len() < 2 {
            continue;
        }
        for part in parts {
            if part.chars().count() > 2 && seen.insert(part.clone()) {
                out.push(part);
            }
        }
    }
    out.join(" ")
}

/// Span, then the bare stem twice, then the last three directories.
fn enriched(path: &str, body: &str) -> String {
    let dirs = parent_parts(path);
    let last: Vec<&str> = dirs[dirs.len().saturating_sub(3)..].to_vec();
    let stem = path_stem(path);
    format!("{body} {stem} {stem} {}", last.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_text_is_derivable() {
        assert_eq!(lexical_override("a/b.py", "x = 1", &Settings::default()), None);
    }

    #[test]
    fn identifier_splitting_matches_python() {
        let s = Settings { split_identifiers: "inline".into(), ..Settings::default() };
        assert_eq!(lexical("x.py", "HandlerStack go_fast", &s), "x py\nHandlerStack handler stack go_fast fast");
        let s = Settings { split_identifiers: "append".into(), ..Settings::default() };
        assert_eq!(lexical("x.py", "HandlerStack HandlerStack", &s),
                   "x py\nHandlerStack HandlerStack\nhandler stack");
    }

    #[test]
    fn enrich_puts_stem_and_dirs_after_the_body() {
        let s = Settings { lexical_enrich: true, ..Settings::default() };
        assert_eq!(lexical("a/b/c/d/suppression.py", "body", &s), "body suppression suppression b c d");
    }
}
