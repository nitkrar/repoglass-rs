//! Language detection, tag queries, and the grammars that parse them.
//!
//! Grammars are the shared libraries tree-sitter-language-pack 1.20.0
//! publishes, the same builds Python repoglass loads, kept in the pack's
//! own cache so both implementations share one download.

use crate::text::blake2b_hex;
use crate::QUERY_FILES;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tree_sitter::{Language, Query};

const PACK_VERSION: &str = "1.20.0";
const RELEASES: &str = "https://github.com/xberg-io/tree-sitter-language-pack/releases/download";

fn filename_langs() -> &'static HashMap<String, String> {
    static MAP: OnceLock<HashMap<String, String>> = OnceLock::new();
    MAP.get_or_init(|| serde_json::from_str(include_str!("filename_langs.json")).unwrap())
}

/// grep_ast's `filename_to_lang`: the basename, then `os.path.splitext`.
pub fn detect(path: &str) -> Option<String> {
    let base = path.rsplit('/').next().unwrap_or(path);
    if let Some(lang) = filename_langs().get(base) {
        return Some(lang.clone());
    }
    let rest = base.trim_start_matches('.');
    let ext = rest.rfind('.').map(|i| &rest[i..]).unwrap_or("");
    filename_langs().get(ext).cloned()
}

fn query_file(name: &str) -> Option<&'static str> {
    QUERY_FILES.iter().find(|(n, _)| *n == name).map(|(_, text)| *text)
}

/// The tags query plus a supplemental `<lang>-refs.scm` where one exists.
pub fn tag_query(lang: &str) -> Option<String> {
    let tags = query_file(&format!("{lang}-tags.scm"))?;
    let mut parts = vec![tags];
    if let Some(refs) = query_file(&format!("{lang}-refs.scm")) {
        parts.push(refs);
    }
    Some(parts.join("\n"))
}

pub fn has_query(lang: &str) -> bool {
    query_file(&format!("{lang}-tags.scm")).is_some()
}

/// Hash over the query set: a changed query shifts spans, so it forces a reindex.
pub fn extractor_rev() -> String {
    let mut data = Vec::new();
    for (name, text) in QUERY_FILES {
        data.extend_from_slice(name.as_bytes());
        data.extend_from_slice(text.as_bytes());
    }
    blake2b_hex(&data, 16)
}

/// Every language with a tags query: the grammars an index can need.
fn query_languages() -> Vec<String> {
    QUERY_FILES.iter().filter_map(|(n, _)| n.strip_suffix("-tags.scm")).map(String::from).collect()
}

pub struct Grammar {
    pub language: Language,
    pub query: Query,
}

/// Grammars and compiled queries, loaded once per language per process.
pub fn grammar(lang: &str) -> anyhow::Result<Option<Arc<Grammar>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Grammar>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(g) = cache.lock().unwrap().get(lang) {
        return Ok(g.clone());
    }
    let loaded = match tag_query(lang) {
        None => None,
        Some(src) => {
            let language = load_language(lang)?;
            let query = Query::new(&language, &src)
                .map_err(|e| anyhow::anyhow!("{lang} tags query: {e}"))?;
            Some(Arc::new(Grammar { language, query }))
        }
    };
    cache.lock().unwrap().insert(lang.to_string(), loaded.clone());
    Ok(loaded)
}

/// The C symbol a grammar exports, where the pack names it differently.
fn c_symbol(lang: &str) -> &str {
    match lang {
        "csharp" => "c_sharp",
        other => other,
    }
}

fn libs_dir() -> anyhow::Result<PathBuf> {
    let base = match std::env::var("TREE_SITTER_LANGUAGE_PACK_CACHE_DIR") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => dirs::cache_dir().ok_or_else(|| anyhow::anyhow!("no cache directory"))?,
    };
    Ok(base.join("tree-sitter-language-pack").join(format!("v{PACK_VERSION}")).join("libs"))
}

fn lib_name(lang: &str) -> String {
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    format!("libtree_sitter_{}.{ext}", c_symbol(lang))
}

fn load_language(lang: &str) -> anyhow::Result<Language> {
    let path = libs_dir()?.join(lib_name(lang));
    if !path.exists() {
        download_grammars()?;
    }
    // SAFETY: the library is a tree-sitter grammar from the pack; the
    // symbol returns a static language table, and the library stays
    // loaded for the life of the process.
    unsafe {
        let lib = libloading::Library::new(&path)?;
        let sym: libloading::Symbol<unsafe extern "C" fn() -> *const tree_sitter::ffi::TSLanguage> =
            lib.get(format!("tree_sitter_{}", c_symbol(lang)).as_bytes())?;
        let raw = sym();
        std::mem::forget(lib);
        Ok(Language::from_raw(raw))
    }
}

fn platform_key() -> String {
    let os = if cfg!(target_os = "macos") { "macos" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") {
        if cfg!(target_os = "macos") { "arm64" } else { "aarch64" }
    } else {
        std::env::consts::ARCH
    };
    format!("{os}-{arch}")
}

/// Fetch the pack's bundle for this platform, check its digest, and
/// extract the grammars every tags query needs. One download serves all
/// of them, as it does for the pack itself.
fn download_grammars() -> anyhow::Result<()> {
    static ONCE: Mutex<()> = Mutex::new(());
    let _guard = ONCE.lock().unwrap();
    let dir = libs_dir()?;
    let wanted: Vec<String> = query_languages().iter().map(|l| lib_name(l)).collect();
    if wanted.iter().all(|n| dir.join(n).exists()) {
        return Ok(());
    }
    eprintln!("repoglass: downloading tree-sitter grammars (first run; this is not repeated)");
    let manifest: serde_json::Value = ureq::get(&format!("{RELEASES}/v{PACK_VERSION}/parsers.json"))
        .call()?.body_mut().read_json()?;
    let key = platform_key();
    let bundle = manifest["platforms"].get(&key)
        .ok_or_else(|| anyhow::anyhow!("no prebuilt grammars for {key}"))?;
    let url = bundle["url"].as_str().ok_or_else(|| anyhow::anyhow!("manifest has no url"))?;
    let sha = bundle["sha256"].as_str().ok_or_else(|| anyhow::anyhow!("manifest has no sha256"))?;
    let mut data = Vec::new();
    ureq::get(url).call()?.body_mut().with_config().limit(2 << 30).reader().read_to_end(&mut data)?;
    use sha2::Digest;
    let actual = hex::encode(sha2::Sha256::digest(&data));
    anyhow::ensure!(actual == sha, "grammar bundle digest {actual}, expected {sha}");
    std::fs::create_dir_all(&dir)?;
    let mut archive = tar::Archive::new(zstd::Decoder::new(&data[..])?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if wanted.contains(&name) {
            let tmp = dir.join(format!(".{name}.{}", std::process::id()));
            let mut out = std::fs::File::create(&tmp)?;
            std::io::copy(&mut entry, &mut out)?;
            std::fs::rename(&tmp, dir.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_follows_grep_ast() {
        assert_eq!(detect("src/a.py").as_deref(), Some("python"));
        assert_eq!(detect("docker/Dockerfile").as_deref(), Some("dockerfile"));
        assert_eq!(detect("a/.bashrc"), None);
        assert_eq!(detect("notes"), None);
    }

    #[test]
    fn every_query_language_has_its_tags_file() {
        for lang in query_languages() {
            assert!(tag_query(&lang).unwrap().contains("@name.definition."), "{lang}");
        }
    }
}
