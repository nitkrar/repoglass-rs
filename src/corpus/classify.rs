//! Which category a file belongs to, tried in a fixed order:
//! docs, config, data, tests, code. Language decides the first three,
//! path decides tests, and code is the remainder.

use crate::config::Settings;
use regex::Regex;

pub fn classify(lang: &str, path: &str, settings: &Settings, tests: &TestMarkers) -> &'static str {
    let lowered = lang.to_lowercase();
    let has = |langs: &[String]| langs.iter().any(|l| l.to_lowercase() == lowered);
    if has(&settings.doc_languages) {
        "docs"
    } else if has(&settings.config_languages) {
        "config"
    } else if has(&settings.data_languages) {
        "data"
    } else if tests.is_test(path) {
        "tests"
    } else {
        "code"
    }
}

/// The loose and camelCase test patterns for a set of markers.
///
/// Three forms: a directory (`tests/`, `__tests__/`), a delimited name
/// part (`test_a.py`, `a.spec.ts`), and camelCase (`FooTests.swift`).
/// camelCase is matched case-sensitively, or `latest` would be a test.
pub struct TestMarkers {
    patterns: Option<(Regex, Regex)>,
}

fn title(marker: &str) -> String {
    // `str.title()` for a marker: first letter of each word upper, rest lower.
    let mut out = String::new();
    let mut start = true;
    for c in marker.chars() {
        if c.is_alphabetic() {
            if start { out.extend(c.to_uppercase()) } else { out.extend(c.to_lowercase()) }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

impl TestMarkers {
    pub fn new(markers: &[String]) -> TestMarkers {
        if markers.is_empty() {
            return TestMarkers { patterns: None };
        }
        let alt = markers.iter().map(|m| regex::escape(m)).collect::<Vec<_>>().join("|");
        let loose = Regex::new(&format!(
            r"(?i)(?:^|/)__?(?:{alt})s?__?(?:/|$)|(?:^|/)(?:{alt})(?:s|ing)?(?:/|$)|(?:^|/|[_.\-])(?:{alt})s?[_.\-]|[_.\-](?:{alt})s?\.[A-Za-z0-9]+$"
        )).unwrap();
        let titled = markers.iter().map(|m| regex::escape(&title(m))).collect::<Vec<_>>().join("|");
        let camel = Regex::new(&format!(r"[a-z0-9](?:{titled})s?\.[A-Za-z0-9]+$")).unwrap();
        TestMarkers { patterns: Some((loose, camel)) }
    }

    pub fn is_test(&self, path: &str) -> bool {
        match &self.patterns {
            Some((loose, camel)) if !path.is_empty() => loose.is_match(path) || camel.is_match(path),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forms_and_near_misses() {
        let m = TestMarkers::new(&["test".into(), "spec".into()]);
        for p in ["tests/a.py", "a/__tests__/b.js", "test_a.py", "a_test.go", "a.spec.ts",
                  "FooTests.swift", "CaptureControllerSpec.swift", "testing/x.c"] {
            assert!(m.is_test(p), "{p}");
        }
        for p in ["inspection.py", "attestation.py", "latest.py", "protest.rb"] {
            assert!(!m.is_test(p), "{p}");
        }
    }
}
