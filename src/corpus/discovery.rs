//! Directory walk, exclusion, classification.
//!
//! No dependency on git: untracked files are indexed and the root need
//! not be a repository. `.gitignore` (when `gitignore` is on) and
//! `.repoglassignore` are read per directory; within one directory
//! `.repoglassignore` settles disagreements, and a nearer `!` pattern
//! re-admits what an outer one excluded.

use super::classify::{classify, TestMarkers};
use super::languages;
use crate::config::schema::{IGNORE_FILE_NAME, NEVER_INDEX_LANGS};
use crate::config::{Paths, Settings};
use crate::models::SourceFile;
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// A file whose median line runs this long was generated, not written.
pub const MAX_MEDIAN_LINE: usize = 1_000;
/// Files smaller than this are never probed.
const PROBE_FLOOR: u64 = 16_384;
/// A generated file is generated from its first byte, so a prefix answers.
const PROBE_BYTES: usize = 65_536;

/// Every indexable file under the root, in path order.
pub fn walk(paths: &Paths, settings: &Settings) -> Vec<SourceFile> {
    let root = &paths.root;
    let pruned: HashSet<String> = settings.hard_exclude.iter().cloned().collect();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .standard_filters(false)
        .git_ignore(settings.gitignore)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .add_custom_ignore_filename(IGNORE_FILE_NAME)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |e| {
            if e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            name != ".git" && !pruned.contains(name.as_ref())
        });
    let tests = TestMarkers::new(&settings.test_markers);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in builder.build().flatten() {
        if entry.depth() == 0 || entry.file_type().is_none_or(|t| t.is_dir()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !is_indexable(&rel) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(entry.path()) else { continue };
        if !seen.insert(real) {
            continue;
        }
        if meta.len() > settings.max_file_bytes.max(0) as u64 {
            continue;
        }
        // Last, because it is the only filter that opens the file.
        if is_generated(entry.path(), meta.len()) {
            continue;
        }
        let Some(lang) = languages::detect(&rel) else { continue };
        let content_type = classify(&lang, &rel, settings, &tests);
        if settings.index_excluded.iter().any(|c| c == content_type) {
            continue;
        }
        out.push(SourceFile {
            path: rel,
            mtime_ns: meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
            size: meta.len() as i64,
            lang,
            content_type: content_type.to_string(),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Whether the extension maps to a language that may be indexed at all.
pub fn is_indexable(path: &str) -> bool {
    languages::detect(path).is_some_and(|l| !NEVER_INDEX_LANGS.contains(&l.as_str()))
}

/// Whether the file's shape says no one typed it: fewer than three
/// newlines in the first 64 KB, or a median line over `MAX_MEDIAN_LINE`.
fn is_generated(path: &Path, size: u64) -> bool {
    if size < PROBE_FLOOR {
        return false;
    }
    let mut probe = vec![0u8; PROBE_BYTES];
    let Ok(mut fh) = std::fs::File::open(path) else { return false };
    let mut n = 0;
    while n < probe.len() {
        match fh.read(&mut probe[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return false,
        }
    }
    probe.truncate(n);
    // The final line is dropped: a truncated read almost always cuts one.
    let mut lines: Vec<usize> = probe.split(|&b| b == b'\n').map(<[u8]>::len).collect();
    lines.pop();
    if lines.len() < 3 {
        return true;
    }
    lines.sort_unstable();
    lines[lines.len() / 2] > MAX_MEDIAN_LINE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let p = dir.path().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    fn walked(dir: &tempfile::TempDir, settings: &Settings) -> Vec<String> {
        walk(&Paths::for_root(dir.path()), settings).into_iter().map(|f| f.path).collect()
    }

    #[test]
    fn ignore_files_and_hard_excludes_apply() {
        let dir = tree(&[
            ("a.py", "x = 1\n"), ("build/b.py", "x\n"), ("keep/c.py", "x\n"),
            ("node_modules/d.js", "x\n"), ("e.csv", "a,b\n"), ("f.pem", "k\n"),
            (".gitignore", "build/\nkeep/\n"), (".repoglassignore", "!keep/\n"),
        ]);
        let got = walked(&dir, &Settings::default());
        assert_eq!(got, vec![".gitignore", "a.py", "keep/c.py"]);
        let off = Settings { gitignore: false, ..Settings::default() };
        assert!(walked(&dir, &off).contains(&"build/b.py".to_string()));
    }

    #[test]
    fn generated_files_are_skipped() {
        let long = format!("{}\n", "x".repeat(20_000));
        let dir = tree(&[("bundle.js", &long), ("ok.js", &"let a = 1;\n".repeat(3_000))]);
        assert_eq!(walked(&dir, &Settings::default()), vec!["ok.js"]);
    }
}
