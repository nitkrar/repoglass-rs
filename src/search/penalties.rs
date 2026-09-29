//! Path penalties and greedy saturation for the reranking layer.

use super::boosting::Candidate;
use crate::config::Settings;
use crate::text::path_name;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

struct Patterns {
    test_file: Regex,
    test_dir: Regex,
    compat_dir: Regex,
    examples_dir: Regex,
    type_defs: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        test_file: Regex::new(concat!(
            r"(?:^|/)(?:",
            r"test_[^/]*\.py|[^/]*_test\.py|[^/]*_test\.go|[^/]*Tests?\.java",
            r"|[^/]*Test\.php|[^/]*_spec\.rb|[^/]*_test\.rb",
            r"|[^/]*\.test\.[jt]sx?|[^/]*\.spec\.[jt]sx?",
            r"|[^/]*Tests?\.kt|[^/]*Spec\.kt|[^/]*Tests?\.swift|[^/]*Spec\.swift",
            r"|[^/]*Tests?\.cs|test_[^/]*\.cpp|[^/]*_test\.cpp|test_[^/]*\.c",
            r"|[^/]*_test\.c|[^/]*Spec\.scala|[^/]*Suite\.scala|[^/]*Test\.scala",
            r"|[^/]*_test\.dart|test_[^/]*\.dart|[^/]*_spec\.lua|[^/]*_test\.lua",
            r"|test_[^/]*\.lua|test_helpers?[^/]*\.\w+",
            r")$")).unwrap(),
        test_dir: Regex::new(r"(?:^|/)(?:tests?|__tests__|spec|testing)(?:/|$)").unwrap(),
        compat_dir: Regex::new(r"(?:^|/)(?:compat|_compat|legacy)(?:/|$)").unwrap(),
        examples_dir: Regex::new(r"(?:^|/)(?:_?examples?|docs?_src)(?:/|$)").unwrap(),
        type_defs: Regex::new(r"\.d\.ts$").unwrap(),
    })
}

const STRONG: f64 = 0.3;
const MODERATE: f64 = 0.5;
const MILD: f64 = 0.7;

/// A multiplier in (0, 1] for where a chunk lives. Categories compound.
pub fn path_penalty(path: &str) -> f64 {
    let p = patterns();
    let norm = path.replace('\\', "/");
    let mut out = 1.0;
    if p.test_file.is_match(&norm) || p.test_dir.is_match(&norm) {
        out *= STRONG;
    }
    if ["__init__.py", "package-info.java"].contains(&path_name(path)) {
        out *= MODERATE;
    }
    if p.compat_dir.is_match(&norm) {
        out *= STRONG;
    }
    if p.examples_dir.is_match(&norm) {
        out *= STRONG;
    }
    if p.type_defs.is_match(&norm) {
        out *= MILD;
    }
    out
}

/// Penalise, then take the top `limit` with saturation decay, applied
/// greedily: whether a chunk is the second from its file, or repeats a
/// body already chosen, depends on what was selected before it.
pub fn select_top(cands: Vec<Candidate>, settings: &Settings, limit: usize, penalise_paths: bool) -> Vec<Candidate> {
    let mut scored: Vec<(f64, usize)> = cands.iter().enumerate()
        .map(|(i, c)| (if penalise_paths { c.score * path_penalty(&c.path) } else { c.score }, i)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(cands[a.1].id.cmp(&cands[b.1].id)));
    let decay = settings.saturation_decay.get();
    let mut per_file: HashMap<&str, i32> = HashMap::new();
    let mut per_text: HashMap<&str, i32> = HashMap::new();
    let mut chosen: Vec<(f64, usize)> = Vec::new();
    let mut floor = f64::INFINITY;
    for &(base, i) in &scored {
        if chosen.len() >= limit && base <= floor {
            break;
        }
        let c = &cands[i];
        let n = *per_file.get(c.path.as_str()).unwrap_or(&0);
        // A candidate with no text is one retrieval could not load, not a repeat.
        let track = !c.text.is_empty();
        let repeats = if track { *per_text.get(c.text.as_str()).unwrap_or(&0) } else { 0 };
        let eff = if n != 0 || repeats != 0 { base * decay.powi(n + repeats) } else { base };
        chosen.push((eff, i));
        per_file.insert(c.path.as_str(), n + 1);
        if track {
            per_text.insert(c.text.as_str(), repeats + 1);
        }
        if chosen.len() >= limit {
            floor = chosen.iter().map(|t| t.0).fold(f64::INFINITY, f64::min);
        }
    }
    chosen.sort_by(|a, b| b.0.total_cmp(&a.0).then(cands[a.1].id.cmp(&cands[b.1].id)));
    chosen.truncate(limit);
    let mut slots: Vec<Option<Candidate>> = cands.into_iter().map(Some).collect();
    chosen.into_iter().map(|(score, i)| {
        let mut c = slots[i].take().unwrap();
        c.score = score;
        c
    }).collect()
}
