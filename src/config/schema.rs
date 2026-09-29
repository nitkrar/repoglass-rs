//! Configuration schema and fixed values.

use crate::pyfmt::{float_repr, str_repr};
use crate::text::blake2b_hex;

pub const ENV_PREFIX: &str = "REPOGLASS_";
/// Shorter spans are navigable, not retrievable.
pub const MIN_CHUNK_CHARS: usize = 60;
/// Upper bound on one chunk's text; over it a definition is navigable only.
pub const MAX_CHUNK_CHARS: usize = 20_000;
/// Languages refused outright: material that must not enter an index.
pub const NEVER_INDEX_LANGS: &[&str] = &["pem"];
pub const DATA_DIR_NAME: &str = ".repoglass";
pub const IGNORE_FILE_NAME: &str = ".repoglassignore";
pub const HOME_ENV: &str = "REPOGLASS_HOME";

/// A float setting that remembers whether its source wrote a whole
/// number, because Python keeps `stem_boost = 1` an int and renders it so.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    pub fn get(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    fn repr(self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) => float_repr(f),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub ranker: String,
    pub prose_min_words: i64,
    pub adaptive_alpha: bool,
    pub alpha_symbol: Num,
    pub alpha_prose: Num,
    pub saturation_decay: Num,
    pub rerank: bool,
    pub file_coherence: Num,
    pub stem_boost: Num,
    pub definition_boost: Num,
    pub candidate_depth: i64,
    pub distill_docs: bool,
    pub content: Vec<String>,
    pub content_excluded: Vec<String>,
    pub index_excluded: Vec<String>,
    pub doc_languages: Vec<String>,
    pub config_languages: Vec<String>,
    pub data_languages: Vec<String>,
    pub test_markers: Vec<String>,
    pub embed_backend: String,
    pub embed_model: String,
    pub embed_endpoint: String,
    pub embed_api_key: String,
    pub embed_query_prefix: Option<String>,
    pub embed_doc_prefix: Option<String>,
    pub embed_providers: String,
    pub embed_onnx_file: String,
    pub coverage: String,
    pub window_chars: i64,
    pub lexical_mode: String,
    pub lexical_cap_chars: i64,
    pub lexical_enrich: bool,
    pub split_identifiers: String,
    pub max_chunk_lines: i64,
    pub data_dir: Option<String>,
    pub gitignore: bool,
    pub hard_exclude: Vec<String>,
    pub max_file_bytes: i64,
    pub refresh_mode: String,
    pub rescan_after_seconds: i64,
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            ranker: "rrf".into(),
            prose_min_words: 4,
            adaptive_alpha: true,
            alpha_symbol: Num::Float(0.3),
            alpha_prose: Num::Float(0.5),
            saturation_decay: Num::Float(0.5),
            rerank: true,
            file_coherence: Num::Float(0.2),
            stem_boost: Num::Float(1.0),
            definition_boost: Num::Float(3.0),
            candidate_depth: 30,
            distill_docs: false,
            content: vec![],
            content_excluded: strings(&["data"]),
            index_excluded: strings(&["data"]),
            doc_languages: strings(&["markdown", "markdown_inline", "html", "rst", "asciidoc",
                                     "latex"]),
            config_languages: strings(&["toml", "yaml", "json", "ini", "xml", "dockerfile",
                                        "make", "cmake", "hcl", "terraform", "properties",
                                        "editorconfig", "gitignore", "gitattributes", "gomod",
                                        "gosum", "requirements", "pymanifest"]),
            data_languages: strings(&["csv", "tsv", "psv"]),
            test_markers: strings(&["test", "spec"]),
            embed_backend: "static".into(),
            embed_model: "minishlab/potion-code-16M-v2".into(),
            embed_endpoint: String::new(),
            embed_api_key: String::new(),
            embed_query_prefix: None,
            embed_doc_prefix: None,
            embed_providers: "webgpu".into(),
            embed_onnx_file: "onnx/model.onnx".into(),
            coverage: "hybrid".into(),
            window_chars: 750,
            lexical_mode: "full".into(),
            lexical_cap_chars: 2_000,
            lexical_enrich: false,
            split_identifiers: "off".into(),
            max_chunk_lines: 200,
            data_dir: None,
            gitignore: true,
            hard_exclude: strings(&[".git", ".repoglass", ".venv", "venv", "node_modules",
                                    "__pycache__", ".build", "dist", "target", ".pytest_cache",
                                    ".mypy_cache"]),
            max_file_bytes: 1_000_000,
            refresh_mode: "auto".into(),
            rescan_after_seconds: 5,
        }
    }
}

/// What a setting holds, for loading and rendering.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Str,
    OptStr,
    Choice(&'static [&'static str]),
    Int,
    Float,
    Bool,
    List,
}

pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    /// The `#:` comment above the field in Python's `schema.py`, one
    /// line per entry, rendered into generated config files.
    pub docs: &'static [&'static str],
}

/// Every setting, in declaration order.
pub static FIELDS: &[Field] = &[
    Field { name: "ranker", kind: Kind::Choice(&["rrf", "none"]), docs: &[] },
    Field { name: "prose_min_words", kind: Kind::Int, docs: &[] },
    Field { name: "adaptive_alpha", kind: Kind::Bool, docs: &[
        "Weight the dense tier against the lexical tier by query shape",
        "instead of letting unweighted RRF give both an equal say."] },
    Field { name: "alpha_symbol", kind: Kind::Float, docs: &[
        "Dense share for an identifier-shaped query, and for a sentence.",
        "The lexical tier takes the remainder; `exact` is never weighted."] },
    Field { name: "alpha_prose", kind: Kind::Float, docs: &[] },
    Field { name: "saturation_decay", kind: Kind::Float, docs: &[
        "Each further chunk from an already-represented file is multiplied",
        "by this. 1.0 disables. Stops one large file filling every slot.",
        "Applied inside `rerank`, so it does nothing while that is off."] },
    Field { name: "rerank", kind: Kind::Bool, docs: &[
        "Post-retrieval re-ranking. Off leaves raw fusion order.",
        "`saturation_decay` and `candidate_depth` only take effect with",
        "it on."] },
    Field { name: "file_coherence", kind: Kind::Float, docs: &[
        "Weights inside that layer, as multiples of the top fused score."] },
    Field { name: "stem_boost", kind: Kind::Float, docs: &[] },
    Field { name: "definition_boost", kind: Kind::Float, docs: &[] },
    Field { name: "candidate_depth", kind: Kind::Int, docs: &[
        "Candidates pulled per tier before fusion. Fixed, not a multiple",
        "of k, or the top 3 would depend on how many results the caller",
        "asked for. Also the ceiling on how many distinct files a search",
        "can return. Raising it only helps with `rerank` on."] },
    Field { name: "distill_docs", kind: Kind::Bool, docs: &[
        "Distil prose chunks instead of embedding them verbatim. Code is",
        "always embedded raw. Off by default: suppressing a category is",
        "the content filter's job."] },
    Field { name: "content", kind: Kind::List, docs: &[
        "What a caller asked for, when the caller is a config file rather",
        "than an argument. Empty means \"use the default\", which is every",
        "category except `content_excluded`."] },
    Field { name: "content_excluded", kind: Kind::List, docs: &[
        "Categories a search excludes unless the caller names them.",
        "Stated as exclusions rather than inclusions so a category added",
        "later is visible by default instead of silently dropped. Config",
        "stays in: a question about how something is wired is often",
        "answered by a manifest or a dockerfile."] },
    Field { name: "index_excluded", kind: Kind::List, docs: &[
        "Categories that never enter the index. Opting one back in is a",
        "reindex -- which is what makes excluding it safe to default."] },
    Field { name: "doc_languages", kind: Kind::List, docs: &[
        "Languages that count as documentation. Matched against the",
        "stored `file.lang`, which comes from the extension."] },
    Field { name: "config_languages", kind: Kind::List, docs: &[
        "Languages that count as configuration. None has a tags query, so",
        "these are retrievable as windows and carry no symbols."] },
    Field { name: "data_languages", kind: Kind::List, docs: &[
        "Tabular data. Its own category rather than config, because a",
        "question about configuration should not return spreadsheet rows."] },
    Field { name: "test_markers", kind: Kind::List, docs: &[
        "Plain substrings, matched case-insensitively against the whole",
        "relative path. Words like 'attestation' and 'latest' collide;",
        "narrow the marker ('test_', '/tests/') if that bites."] },
    Field { name: "embed_backend", kind: Kind::Choice(&["static", "onnx", "http", "none"]), docs: &[
        "'static'  model2vec, no extra dependency, effectively instant.",
        "'onnx'    a real transformer via onnxruntime. Much slower to",
        "index, and costs an optional dependency. Pair with",
        "embed_model 'BAAI/bge-small-en-v1.5'.",
        "'http'    an OpenAI-compatible /v1/embeddings server.",
        "'none'    no vector tier; exact and lexical only."] },
    Field { name: "embed_model", kind: Kind::Str, docs: &[
        "Code-specialised static model. Changing it invalidates every",
        "stored vector, so it forces a reindex."] },
    Field { name: "embed_endpoint", kind: Kind::Str, docs: &[
        "Empty means unset. No tri-state here -- it is only ever tested",
        "for truthiness, unlike the prefixes below."] },
    Field { name: "embed_api_key", kind: Kind::Str, docs: &[
        "Secret. Prefer the REPOGLASS_EMBED_API_KEY environment variable",
        "over writing it into a config file that lives in the repo.",
        "Empty means unset."] },
    Field { name: "embed_query_prefix", kind: Kind::OptStr, docs: &[
        "None = use whatever the model family expects (bge and e5 want an",
        "instruction on queries only). '' = deliberately none."] },
    Field { name: "embed_doc_prefix", kind: Kind::OptStr, docs: &[
        "Document-side marker. e5 and nomic want one; most models do not.",
        "None = whatever the model family expects, '' = deliberately none."] },
    Field { name: "embed_providers", kind: Kind::Str, docs: &[
        "onnxruntime execution provider.",
        "",
        "webgpu  the plugin provider, via `repoglass[webgpu]`. Takes the",
        "whole graph on Metal, Vulkan or D3D12.",
        "cpu     always present, and the fallback when the plugin is not",
        "installed.",
        "auto    CoreML or CUDA where the build offers them, then CPU.",
        "Avoid on Apple silicon: CoreML claims only part of the",
        "graph and splits the rest across dozens of partitions,",
        "which is slower than cpu and costs far more memory."] },
    Field { name: "embed_onnx_file", kind: Kind::Str, docs: &[
        "Path of the graph inside the HF repo. Layout is not standardised:",
        "bge and nomic use onnx/model.onnx, e5-small-v2 puts model.onnx at",
        "the root, and quantised variants sit beside them."] },
    Field { name: "coverage", kind: Kind::Choice(&["definition", "hybrid"]), docs: &[
        "How much of each file ends up in a chunk.",
        "",
        "definition  only tags-query definitions, leaving module-level",
        "code in no chunk and so unreachable by search",
        "hybrid      plus windows over the lines no definition owns",
        "",
        "Both keep the definition chunk and its symbol link, so the exact",
        "tier can go from a name straight to its text. Changing this",
        "forces a reindex."] },
    Field { name: "window_chars", kind: Kind::Int, docs: &[
        "Target -- not a cap -- for a window, in bytes. An indivisible node",
        "larger than this is emitted whole."] },
    Field { name: "lexical_mode", kind: Kind::Choice(&["full", "capped"]), docs: &[] },
    Field { name: "lexical_cap_chars", kind: Kind::Int, docs: &[] },
    Field { name: "lexical_enrich", kind: Kind::Bool, docs: &[
        "Build the FTS input as span + stem + stem + dirs instead of",
        "humanised-path + span. Changing it forces a reindex."] },
    Field { name: "split_identifiers", kind: Kind::Choice(&["off", "inline", "append"]), docs: &[
        "Append or inline the sub-words of compound identifiers, so",
        "\"handler\" can match `HandlerStack` -- FTS5's unicode61 tokenizer",
        "does not split camelCase while `tokenize_query` does.",
        "",
        "Off by default: the gap it closes is one the exact-symbol tier",
        "already covers."] },
    Field { name: "max_chunk_lines", kind: Kind::Int, docs: &[] },
    Field { name: "data_dir", kind: Kind::OptStr, docs: &[
        "Where the index is written. Unset means a central directory",
        "under `~/.repoglass/index`, keyed by the repository path, so the",
        "indexed tree is never written to and a read-only checkout can be",
        "indexed. A relative path resolves against the repository, so",
        "\".repoglass\" puts the index beside the code instead."] },
    Field { name: "gitignore", kind: Kind::Bool, docs: &[
        "Honour the repository's .gitignore. A good default heuristic for",
        "\"not my code\", but only a heuristic: git-ignored is not the same",
        "claim as not-worth-searching. A `!` pattern in .repoglassignore",
        "overrides this per file."] },
    Field { name: "hard_exclude", kind: Kind::List, docs: &[] },
    Field { name: "max_file_bytes", kind: Kind::Int, docs: &[
        "Skip files larger than this. `hard_exclude` catches the usual",
        "homes for vendored and generated code by directory name, but a",
        "bundle committed anywhere else -- an editor plugin, a saved web",
        "page -- is indistinguishable from source by extension alone."] },
    Field { name: "refresh_mode", kind: Kind::Choice(&["auto", "manual"]), docs: &[] },
    Field { name: "rescan_after_seconds", kind: Kind::Int, docs: &[
        "Minimum gap between staleness walks triggered by a read. 0 walks",
        "on every call, which on a large repo spends most of the query",
        "re-stat-ing files that have not changed. Edits are still picked",
        "up, just at most this often."] },
];

/// A setting's value, typed as the schema declares it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    OptStr(Option<String>),
    Int(i64),
    Float(Num),
    Bool(bool),
    List(Vec<String>),
}

impl Value {
    /// Python `repr`, as `chunking_rev` hashes it.
    pub fn repr(&self) -> String {
        match self {
            Value::Str(s) | Value::OptStr(Some(s)) => str_repr(s),
            Value::OptStr(None) => "None".into(),
            Value::Int(i) => i.to_string(),
            Value::Float(n) => n.repr(),
            Value::Bool(b) => if *b { "True".into() } else { "False".into() },
            Value::List(items) => {
                let parts: Vec<String> = items.iter().map(|s| str_repr(s)).collect();
                if parts.len() == 1 { format!("({},)", parts[0]) } else { format!("({})", parts.join(", ")) }
            }
        }
    }
}

impl Settings {
    pub fn get(&self, name: &str) -> Value {
        use Value as V;
        match name {
            "ranker" => V::Str(self.ranker.clone()),
            "prose_min_words" => V::Int(self.prose_min_words),
            "adaptive_alpha" => V::Bool(self.adaptive_alpha),
            "alpha_symbol" => V::Float(self.alpha_symbol),
            "alpha_prose" => V::Float(self.alpha_prose),
            "saturation_decay" => V::Float(self.saturation_decay),
            "rerank" => V::Bool(self.rerank),
            "file_coherence" => V::Float(self.file_coherence),
            "stem_boost" => V::Float(self.stem_boost),
            "definition_boost" => V::Float(self.definition_boost),
            "candidate_depth" => V::Int(self.candidate_depth),
            "distill_docs" => V::Bool(self.distill_docs),
            "content" => V::List(self.content.clone()),
            "content_excluded" => V::List(self.content_excluded.clone()),
            "index_excluded" => V::List(self.index_excluded.clone()),
            "doc_languages" => V::List(self.doc_languages.clone()),
            "config_languages" => V::List(self.config_languages.clone()),
            "data_languages" => V::List(self.data_languages.clone()),
            "test_markers" => V::List(self.test_markers.clone()),
            "embed_backend" => V::Str(self.embed_backend.clone()),
            "embed_model" => V::Str(self.embed_model.clone()),
            "embed_endpoint" => V::Str(self.embed_endpoint.clone()),
            "embed_api_key" => V::Str(self.embed_api_key.clone()),
            "embed_query_prefix" => V::OptStr(self.embed_query_prefix.clone()),
            "embed_doc_prefix" => V::OptStr(self.embed_doc_prefix.clone()),
            "embed_providers" => V::Str(self.embed_providers.clone()),
            "embed_onnx_file" => V::Str(self.embed_onnx_file.clone()),
            "coverage" => V::Str(self.coverage.clone()),
            "window_chars" => V::Int(self.window_chars),
            "lexical_mode" => V::Str(self.lexical_mode.clone()),
            "lexical_cap_chars" => V::Int(self.lexical_cap_chars),
            "lexical_enrich" => V::Bool(self.lexical_enrich),
            "split_identifiers" => V::Str(self.split_identifiers.clone()),
            "max_chunk_lines" => V::Int(self.max_chunk_lines),
            "data_dir" => V::OptStr(self.data_dir.clone()),
            "gitignore" => V::Bool(self.gitignore),
            "hard_exclude" => V::List(self.hard_exclude.clone()),
            "max_file_bytes" => V::Int(self.max_file_bytes),
            "refresh_mode" => V::Str(self.refresh_mode.clone()),
            "rescan_after_seconds" => V::Int(self.rescan_after_seconds),
            _ => unreachable!("no setting {name}"),
        }
    }

    /// Assign an already-typed value. The caller checked the kind.
    pub fn set(&mut self, name: &str, value: Value) {
        use Value as V;
        let s = |v: V| match v { V::Str(s) => s, _ => unreachable!() };
        let o = |v: V| match v { V::OptStr(s) => s, _ => unreachable!() };
        let i = |v: V| match v { V::Int(i) => i, _ => unreachable!() };
        let f = |v: V| match v { V::Float(n) => n, _ => unreachable!() };
        let b = |v: V| match v { V::Bool(b) => b, _ => unreachable!() };
        let l = |v: V| match v { V::List(l) => l, _ => unreachable!() };
        match name {
            "ranker" => self.ranker = s(value),
            "prose_min_words" => self.prose_min_words = i(value),
            "adaptive_alpha" => self.adaptive_alpha = b(value),
            "alpha_symbol" => self.alpha_symbol = f(value),
            "alpha_prose" => self.alpha_prose = f(value),
            "saturation_decay" => self.saturation_decay = f(value),
            "rerank" => self.rerank = b(value),
            "file_coherence" => self.file_coherence = f(value),
            "stem_boost" => self.stem_boost = f(value),
            "definition_boost" => self.definition_boost = f(value),
            "candidate_depth" => self.candidate_depth = i(value),
            "distill_docs" => self.distill_docs = b(value),
            "content" => self.content = l(value),
            "content_excluded" => self.content_excluded = l(value),
            "index_excluded" => self.index_excluded = l(value),
            "doc_languages" => self.doc_languages = l(value),
            "config_languages" => self.config_languages = l(value),
            "data_languages" => self.data_languages = l(value),
            "test_markers" => self.test_markers = l(value),
            "embed_backend" => self.embed_backend = s(value),
            "embed_model" => self.embed_model = s(value),
            "embed_endpoint" => self.embed_endpoint = s(value),
            "embed_api_key" => self.embed_api_key = s(value),
            "embed_query_prefix" => self.embed_query_prefix = o(value),
            "embed_doc_prefix" => self.embed_doc_prefix = o(value),
            "embed_providers" => self.embed_providers = s(value),
            "embed_onnx_file" => self.embed_onnx_file = s(value),
            "coverage" => self.coverage = s(value),
            "window_chars" => self.window_chars = i(value),
            "lexical_mode" => self.lexical_mode = s(value),
            "lexical_cap_chars" => self.lexical_cap_chars = i(value),
            "lexical_enrich" => self.lexical_enrich = b(value),
            "split_identifiers" => self.split_identifiers = s(value),
            "max_chunk_lines" => self.max_chunk_lines = i(value),
            "data_dir" => self.data_dir = o(value),
            "gitignore" => self.gitignore = b(value),
            "hard_exclude" => self.hard_exclude = l(value),
            "max_file_bytes" => self.max_file_bytes = i(value),
            "refresh_mode" => self.refresh_mode = s(value),
            "rescan_after_seconds" => self.rescan_after_seconds = i(value),
            _ => unreachable!("no setting {name}"),
        }
    }
}

/// Settings that shape a stored chunk: its span, embedded text, or lexical text.
pub const CHUNKING_FIELDS: &[&str] = &["max_chunk_lines", "window_chars", "distill_docs",
    "lexical_mode", "lexical_cap_chars", "lexical_enrich", "split_identifiers"];

/// Fingerprint of `CHUNKING_FIELDS`, part of the index identity.
pub fn chunking_rev(settings: &Settings) -> String {
    let joined: Vec<String> = CHUNKING_FIELDS.iter().map(|f| settings.get(f).repr()).collect();
    blake2b_hex(joined.join("\u{1f}").as_bytes(), 8)
}

/// Fingerprint of the lists that decide a file's content type.
pub fn categories_rev(settings: &Settings) -> String {
    let mut excluded = settings.index_excluded.clone();
    excluded.sort();
    let parts = [&settings.doc_languages, &settings.config_languages, &settings.data_languages,
                 &settings.test_markers, &excluded];
    let joined: Vec<String> = parts.iter().map(|p| p.join("\u{1f}")).collect();
    blake2b_hex(joined.join("\n").as_bytes(), 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_round_trips() {
        let settings = Settings::default();
        let mut copy = Settings::default();
        for f in FIELDS {
            copy.set(f.name, settings.get(f.name));
        }
        assert_eq!(copy, settings);
    }

    /// Expected hashes are Python 0.3.2's for the same settings.
    #[test]
    fn revisions_match_python() {
        let s = Settings::default();
        assert_eq!(chunking_rev(&s), "56007f11b5918982");
        assert_eq!(categories_rev(&s), "90fd7dbbbd68ddc5");
        let mut t = Settings::default();
        t.window_chars = 900;
        t.split_identifiers = "inline".into();
        t.index_excluded = strings(&["data", "config"]);
        t.test_markers = strings(&["test"]);
        assert_eq!(chunking_rev(&t), "e87255f2c569abd0");
        assert_eq!(categories_rev(&t), "a103e9db82302720");
    }
}
