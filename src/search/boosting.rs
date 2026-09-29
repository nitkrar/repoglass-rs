//! The boosts reranking applies before path penalties and saturation.

use super::is_symbol_query;
use super::tokens::{split_identifier, stem_keywords};
use crate::config::Settings;
use crate::text::{parent_name, path_stem};
use fancy_regex::Regex as FRegex;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// One scored chunk, with the text and path reranking needs.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: i64,
    pub path: String,
    pub text: String,
    pub score: f64,
}

/// Chunks retrieval never returned whose file path matches a queried
/// name: the only way a chunk absent from every tier reaches the results.
pub type NonCandidateLoader<'a> = dyn FnMut(&HashSet<String>) -> anyhow::Result<Vec<Candidate>> + 'a;

const DEFINITION_KEYWORDS: &[&str] = &["class", "module", "defmodule", "def", "interface", "struct", "enum",
    "trait", "type", "func", "function", "object", "abstract class", "data class", "fn", "fun", "package",
    "namespace", "protocol", "record", "typedef"];
const SQL_KEYWORDS: &[&str] = &["CREATE TABLE", "CREATE VIEW", "CREATE PROCEDURE", "CREATE FUNCTION"];

/// Whether `text` declares `name`, by keyword rather than parse: a chunk
/// may be a fragment no grammar accepts. `class foo.Bar` defines `Bar`.
pub struct Definitions {
    cache: HashMap<String, (FRegex, FRegex)>,
}

impl Definitions {
    pub fn new() -> Definitions {
        Definitions { cache: HashMap::new() }
    }

    pub fn defines(&mut self, text: &str, name: &str) -> bool {
        let (general, sql) = self.cache.entry(name.to_string()).or_insert_with(|| {
            let escaped = fancy_regex::escape(name);
            let ns = r"(?:[A-Za-z_][A-Za-z0-9_]*(?:\.|::))*";
            let suffix = format!(r")\s+{ns}{escaped}(?:\s|[<({{:\[;]|$)");
            let body = |ks: &[&str]| ks.iter().map(|k| fancy_regex::escape(k).into_owned()).collect::<Vec<_>>().join("|");
            // Case-sensitive keywords; SQL's are not.
            (FRegex::new(&format!(r"(?m)(?:^|(?<=\s))(?:{}{suffix}", body(DEFINITION_KEYWORDS))).unwrap(),
             FRegex::new(&format!(r"(?mi)(?:^|(?<=\s))(?:{}{suffix}", body(SQL_KEYWORDS))).unwrap())
        });
        general.is_match(text).unwrap_or(false) || sql.is_match(text).unwrap_or(false)
    }
}

fn stem_matches(stem: &str, name: &str) -> bool {
    let flat = stem.replace('_', "");
    [stem, flat.as_str(), stem.trim_end_matches('s'), flat.trim_end_matches('s')].contains(&name)
}

/// Add to each file's best chunk in proportion to that file's total.
pub fn boost_coherence(cands: &mut [Candidate], settings: &Settings) {
    let top = cands.iter().map(|c| c.score).fold(f64::NEG_INFINITY, f64::max);
    if top <= 0.0 {
        return;
    }
    let mut by_file: HashMap<String, f64> = HashMap::new();
    let mut best: Vec<(String, usize)> = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        *by_file.entry(c.path.clone()).or_insert(0.0) += c.score;
        match best.iter_mut().find(|(p, _)| *p == c.path) {
            Some(slot) => if c.score > cands[slot.1].score { slot.1 = i },
            None => best.push((c.path.clone(), i)),
        }
    }
    let scale = by_file.values().copied().fold(f64::NEG_INFINITY, f64::max);
    if scale <= 0.0 {
        return;
    }
    let unit = top * settings.file_coherence.get();
    for (path, i) in best {
        cands[i].score += unit * by_file[&path] / scale;
    }
}

pub fn boost_query(cands: &mut Vec<Candidate>, query: &str, settings: &Settings,
                   loader: Option<&mut NonCandidateLoader>, defs: &mut Definitions) -> anyhow::Result<()> {
    let top = cands.iter().map(|c| c.score).fold(f64::NEG_INFINITY, f64::max);
    if top <= 0.0 {
        return Ok(());
    }
    if is_symbol_query(query) {
        boost_definitions(cands, query, top, settings, loader, defs)
    } else {
        boost_stems(cands, query, top, settings);
        boost_embedded(cands, query, top, settings, loader, defs)
    }
}

/// Half again when the file is named for the symbol it defines.
fn definition_bonus(c: &Candidate, names: &[String], unit: f64, defs: &mut Definitions) -> f64 {
    if !names.iter().any(|n| defs.defines(&c.text, n)) {
        return 0.0;
    }
    let stem = path_stem(&c.path).to_lowercase();
    let matched = names.iter().any(|n| stem_matches(&stem, &n.to_lowercase()));
    unit * if matched { 1.5 } else { 1.0 }
}

fn boost_named(cands: &mut Vec<Candidate>, names: &[String], unit: f64,
               loader: Option<&mut NonCandidateLoader>, defs: &mut Definitions) -> anyhow::Result<()> {
    for c in cands.iter_mut() {
        let bonus = definition_bonus(c, names, unit, defs);
        c.score += bonus;
    }
    let Some(loader) = loader else { return Ok(()) };
    let seen: HashSet<i64> = cands.iter().map(|c| c.id).collect();
    let set: HashSet<String> = names.iter().cloned().collect();
    for mut extra in loader(&set)? {
        if seen.contains(&extra.id) {
            continue;
        }
        let bonus = definition_bonus(&extra, names, unit, defs);
        if bonus != 0.0 {
            extra.score = bonus;
            cands.push(extra);
        }
    }
    Ok(())
}

/// Boost definitions of the queried symbol; `pkg::Widget` also boosts
/// definitions of `Widget`.
fn boost_definitions(cands: &mut Vec<Candidate>, query: &str, top: f64, settings: &Settings,
                     loader: Option<&mut NonCandidateLoader>, defs: &mut Definitions) -> anyhow::Result<()> {
    let q = crate::pyfmt::py_strip(query);
    let mut name = q;
    for sep in ["::", "\\", "->", "."] {
        if name.contains(sep) {
            name = name.rsplit(sep).next().unwrap();
            break;
        }
    }
    let mut names = vec![name.to_string()];
    if q != name {
        names.push(q.to_string());
    }
    boost_named(cands, &names, top * settings.definition_boost.get(), loader, defs)
}

fn embedded_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(
        r"\b(?:[A-Z][a-z][a-zA-Z0-9]*[A-Z][a-zA-Z0-9]*|[a-z][a-zA-Z0-9]*[A-Z][a-zA-Z0-9]+)\b").unwrap())
}

/// Boost definitions of camelCase words inside a prose query, at half strength.
fn boost_embedded(cands: &mut Vec<Candidate>, query: &str, top: f64, settings: &Settings,
                  loader: Option<&mut NonCandidateLoader>, defs: &mut Definitions) -> anyhow::Result<()> {
    let mut names: Vec<String> = Vec::new();
    for m in embedded_re().find_iter(query) {
        if !names.iter().any(|n| n == m.as_str()) {
            names.push(m.as_str().to_string());
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    boost_named(cands, &names, top * settings.definition_boost.get() * 0.5, loader, defs)
}

/// Exact matches, then prefix overlap of at least three characters.
fn count_matches(keywords: &HashSet<String>, parts: &HashSet<String>) -> usize {
    let exact: HashSet<&String> = keywords.intersection(parts).collect();
    if exact.len() == keywords.len() {
        return exact.len();
    }
    let mut n = exact.len();
    for kw in keywords.iter().filter(|k| !exact.contains(k)) {
        for part in parts {
            let (short, long) = if kw.chars().count() <= part.chars().count() { (kw, part) } else { (part, kw) };
            if short.chars().count() >= 3 && long.starts_with(short.as_str()) {
                n += 1;
                break;
            }
        }
    }
    n
}

/// Match query words against the file stem and its parent directory,
/// scaled by the fraction of the query the path accounts for.
fn boost_stems(cands: &mut [Candidate], query: &str, top: f64, settings: &Settings) {
    let keywords = stem_keywords(query);
    if keywords.is_empty() {
        return;
    }
    let boost = top * settings.stem_boost.get();
    let mut cache: HashMap<String, HashSet<String>> = HashMap::new();
    for c in cands.iter_mut() {
        let parts = cache.entry(c.path.clone()).or_insert_with(|| {
            let mut parts: HashSet<String> = split_identifier(path_stem(&c.path)).into_iter().collect();
            let parent = parent_name(&c.path);
            if ![".", "/", "..", ""].contains(&parent) {
                parts.extend(split_identifier(parent));
            }
            parts
        });
        let n = count_matches(&keywords, parts);
        if n > 0 {
            let ratio = n as f64 / keywords.len() as f64;
            if ratio >= 0.10 {
                c.score += boost * ratio;
            }
        }
    }
}
