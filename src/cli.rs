//! Command line entry point.
//!
//! JSON on stdout by default, because the caller is usually a program.
//! `--text` is for reading. Progress, warnings and errors go to stderr.
//!
//! Exit codes: 0 it worked, including no results; 1 something failed;
//! 2 the request was not answerable (an unknown category, a category or
//! language this index does not hold, or a config that will not load).
//!
//! Every retrieval setting comes from the config. There are no per-knob
//! flags, so what ran always matches what the settings say.

use crate::config::{as_toml, load, paths::resolve, ConfigError, Paths, Settings};
use crate::index::{BadValue, Index, NotIndexed};
use crate::models::{Hit, Symbol};
use crate::pyfmt::{close_match, float_repr, round, str_repr, Json};
use crate::store::Scope;
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "repoglass", disable_version_flag = true,
          about = "Local code and prose index: symbols, lexical and semantic retrieval.",
          after_help = "every subcommand also takes:\n  \
  -r, --repo PATH  the directory to index and search (default: .). Result\n                   \
paths are relative to it, and its index is keyed by the\n                   \
resolved path, so two checkouts of one project do not\n                   \
share an index. Use it to query another repository\n                   \
without changing directory.\n  \
  --git URL        index a git repository instead: one commit, fetched at\n                   \
depth 1 into ~/.repoglass/git/ the first time and kept,\n                   \
so later runs and rebuilds reuse it. -r then names a\n                   \
directory inside the checkout.\n  \
  --rev REV        with --git, the commit, tag or branch (default: the\n                   \
remote's HEAD). A tag or branch is resolved once; delete\n                   \
its checkout to fetch it again.\n  \
  --config PATH    a TOML file overriding the resolved settings\n  \
  --text           human-readable output instead of json (`config`\n                   \
always emits TOML)\n\nrun `repoglass <command> --help` for a command's own options.")]
struct Cli {
    /// show program's version number and exit
    #[arg(short = 'V', long = "version")]
    version: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct Common {
    /// directory to index and search (default: .); its index is keyed by the resolved path
    #[arg(short = 'r', long = "repo", default_value = ".", hide_default_value = true)]
    repo: PathBuf,
    /// index this git repository: one commit, fetched once into ~/.repoglass/git/ and kept
    #[arg(long, value_name = "URL")]
    git: Option<String>,
    /// with --git: the commit, tag or branch to fetch (default: the remote's HEAD)
    #[arg(long, value_name = "REV", requires = "git")]
    rev: Option<String>,
    /// TOML overriding the resolved settings
    #[arg(long)]
    config: Option<PathBuf>,
    /// human-readable output instead of json
    #[arg(long)]
    text: bool,
}

#[derive(Subcommand)]
enum Command {
    /// search the index
    #[command(after_help = "output (JSON unless --text):\n  \
  query      the query as searched\n  \
  results[]  path, start_line, end_line, name, score, tiers,\n             \
and code unless --no-code\n    \
    score    the fused rank divided by the top hit. Always\n             \
1.0 at rank 1, so it orders results but does\n             \
NOT measure how good any of them are.\n    \
    tiers    raw score from each tier that matched, keyed by\n             \
metric. A result matched by two tiers is better\n             \
corroborated than one matched by a single tier.\n             \
bm25   SQLite FTS5 bm25(); negative, and more\n                    \
negative is better.\n             \
cosine cosine similarity in [-1, 1]; higher is\n                    \
better.\n             \
exact  1.0 when the query literally matched a\n                    \
symbol name.\n\n\
no results is success (exit 0). Exit 2 means the request\n\
could not be answered: an unknown category, or one this\n\
index does not hold.")]
    Search {
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
        /// results (default: 10)
        #[arg(short = 'k', default_value_t = 10, hide_default_value = true)]
        k: usize,
        /// code, tests, docs, config, data, or all
        #[arg(long, num_args = 1..)]
        content: Vec<String>,
        /// only results in these languages, e.g. python go
        #[arg(short = 'l', long, num_args = 1..)]
        lang: Vec<String>,
        /// only paths matching these globs, e.g. 'src/*'. * crosses / , so src/* is recursive
        #[arg(long, num_args = 1.., value_name = "GLOB")]
        include: Vec<String>,
        /// skip paths matching these globs; wins over --include
        #[arg(long, num_args = 1.., value_name = "GLOB")]
        exclude: Vec<String>,
        /// how much of each result to return: the whole span, one line, or neither. that line is
        /// the definition's header, or the span's first non-blank line where it holds no definition
        #[arg(long, value_parser = ["full", "signature", "none"], default_value = "full")]
        code: String,
        /// same as --code none
        #[arg(long)]
        no_code: bool,
        #[command(flatten)]
        common: Common,
    },
    /// where a name is defined (all of them)
    Defs {
        name: String,
        /// only definitions in these languages, e.g. python go
        #[arg(short = 'l', long, num_args = 1..)]
        lang: Vec<String>,
        #[command(flatten)]
        common: Common,
    },
    /// where a name is used (all of them)
    Refs {
        name: String,
        /// only references in these languages, e.g. python go
        #[arg(short = 'l', long, num_args = 1..)]
        lang: Vec<String>,
        #[command(flatten)]
        common: Common,
    },
    /// list or count symbols, complete and unranked
    #[command(after_help = "the complement of `search`, which ranks and truncates.\n\
use this to ask how many, which ones, or whether any.\n\n\
examples:\n  rpg symbols 'test_*' --tag def\n  rpg symbols --count-by lang\n  \
rpg symbols --tag ref --count-by name --limit 10\n  rpg symbols --count-by file --content tests\n\n\
reads the symbol table, so a definition too small to be\n\
chunked is listed here though no search can return it.\n\
count 0 means the index holds none -- which an empty\n\
ranked list does not say.")]
    Symbols {
        /// glob on the symbol name, e.g. 'handle_*'. omit for all of them
        pattern: Option<String>,
        /// definitions or references; omit for both
        #[arg(long, value_parser = ["def", "ref"])]
        tag: Option<String>,
        /// group and count instead of listing
        #[arg(long = "count-by", value_parser = ["name", "lang", "file", "tag", "content"])]
        count_by: Option<String>,
        /// cap the rows, or the groups under --count-by
        #[arg(long)]
        limit: Option<i64>,
        /// code, tests, docs, config, data, or all
        #[arg(long, num_args = 1..)]
        content: Vec<String>,
        /// only symbols in these languages, e.g. python go
        #[arg(short = 'l', long, num_args = 1..)]
        lang: Vec<String>,
        /// only paths matching these globs
        #[arg(long, num_args = 1.., value_name = "GLOB")]
        include: Vec<String>,
        /// skip paths matching these globs; wins over --include
        #[arg(long, num_args = 1.., value_name = "GLOB")]
        exclude: Vec<String>,
        #[command(flatten)]
        common: Common,
    },
    /// build or update the index
    Index {
        /// re-extract every file, not just changed ones
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        common: Common,
    },
    /// what this index contains
    Status {
        #[command(flatten)]
        common: Common,
    },
    /// write repoglass.toml for this repo
    Init {
        /// overwrite an existing file
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        common: Common,
    },
    /// print the resolved settings
    Config {
        #[command(flatten)]
        common: Common,
    },
    /// remove index directories
    Clear {
        /// every index under the repoglass home, not just this one
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        common: Common,
    },
}

/// A command's outcome: an exit code, or an error to report.
enum Fail {
    Code(i32),
    Error(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for Fail {
    fn from(e: E) -> Fail {
        Fail::Error(e.into())
    }
}

type Outcome = Result<i32, Fail>;

fn fail(message: &str, code: i32) -> Fail {
    eprintln!("repoglass: {message}");
    Fail::Code(code)
}

struct Out {
    text: bool,
}

impl Out {
    fn emit(&self, payload: &Json) -> std::io::Result<()> {
        let mut stdout = std::io::stdout().lock();
        if self.text {
            write!(stdout, "{}", render_text(payload))
        } else {
            writeln!(stdout, "{}", payload.dumps())
        }
    }
}

pub fn main() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    if matches!(argv.get(1).map(String::as_str), Some("-V" | "--version")) {
        println!("{VERSION}");
        return 0;
    }
    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    match run(cli.command) {
        Ok(code) | Err(Fail::Code(code)) => code,
        Err(Fail::Error(e)) => {
            if e.chain().any(|c| c.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)) {
                return 0;
            }
            if let Some(msg) = e.downcast_ref::<BadValue>().map(|b| b.0.clone())
                .or_else(|| e.downcast_ref::<NotIndexed>().map(|n| n.0.clone())) {
                eprintln!("repoglass: {msg}");
                return 2;
            }
            eprintln!("repoglass: {e:#}");
            1
        }
    }
}

fn run(command: Command) -> Outcome {
    match command {
        Command::Search { query, k, content, lang, include, exclude, code, no_code, common } => {
            let code = if no_code { "none".to_string() } else { code };
            cmd_search(&common, &query.join(" "), k, Scope { content, lang, include, exclude }, &code)
        }
        Command::Defs { name, lang, common } => cmd_named(&common, &name, &lang, "def"),
        Command::Refs { name, lang, common } => cmd_named(&common, &name, &lang, "ref"),
        Command::Symbols { pattern, tag, count_by, limit, content, lang, include, exclude, common } =>
            cmd_symbols(&common, pattern, tag, count_by, limit, Scope { content, lang, include, exclude }),
        Command::Index { force, common } => cmd_index(&common, force),
        Command::Status { common } => cmd_status(&common),
        Command::Init { force, common } => cmd_init(&common, force),
        Command::Config { common } => cmd_config(&common),
        Command::Clear { all, dry_run, common } => cmd_clear(&common, all, dry_run),
    }
}

/// The directory this invocation is about: `-r`, or with `--git` a
/// directory inside the checkout, fetched first when `fetch` is set.
fn root(common: &Common, fetch: bool) -> Result<PathBuf, Fail> {
    let Some(url) = &common.git else {
        return Ok(resolve(&common.repo));
    };
    if common.repo.is_absolute()
        || common.repo.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(fail("with --git, -r names a directory inside the checkout", 2));
    }
    let home = crate::config::paths::home();
    let rev = common.rev.as_deref();
    let dir = if fetch {
        crate::git::checkout(&home, url, rev).map_err(|e| fail(&format!("{e:#}"), 1))?
    } else {
        crate::git::checkout_dir(&home, url, rev)
    };
    Ok(resolve(&dir.join(&common.repo)))
}

/// Open the index, announcing a first build on stderr: a first search on
/// a large repository can take minutes, and silence reads as a hang.
fn open(common: &Common) -> Result<Index, Fail> {
    let root = root(common, true)?;
    if !root.is_dir() {
        return Err(fail(&format!("not a directory: {}", root.display()), 1));
    }
    let settings = match &common.config {
        Some(path) => Some(load(None, Some(path)).map_err(|e: ConfigError| {
            fail(&format!("{e}\n  regenerate with: repoglass init --force"), 2)
        })?),
        None => None,
    };
    let paths = Paths::for_root(&root);
    if !paths.db().exists() {
        eprintln!("repoglass: building the index for {} (first run; this is not repeated)", root.display());
    }
    Index::open(paths, settings).map_err(|e| match e.downcast_ref::<ConfigError>() {
        Some(c) => fail(&c.0, 1),
        None => Fail::Error(e),
    })
}

/// Refuse a language this index does not hold, suggesting the nearest.
/// Run after the query, so a first refresh has populated the file table.
fn check_lang(index: &Index, lang: &[String]) -> Result<(), Fail> {
    if lang.is_empty() {
        return Ok(());
    }
    let mut have: Vec<String> = index.store.top_languages(1000)?.into_iter().map(|(n, _)| n).collect();
    if have.is_empty() {
        return Ok(());
    }
    have.sort();
    let Some(missing) = lang.iter().find(|l| !have.contains(l)) else { return Ok(()) };
    let hint = match close_match(missing, &have, 0.6) {
        Some(near) => format!("did you mean {}?", str_repr(&near)),
        None => format!("this index has: {}", have.join(", ")),
    };
    Err(fail(&format!("--lang {} matches no indexed language, {hint}", str_repr(missing)), 2))
}

fn metric(tier: &str) -> &str {
    match tier {
        "lexical" => "bm25",
        "vector" => "cosine",
        other => other,
    }
}

fn hit_json(h: &Hit, code: &str) -> Json {
    let mut out = vec![
        ("path", Json::str(&h.path)),
        ("start_line", Json::Int(h.start_line)),
        ("end_line", Json::Int(h.end_line)),
        ("name", Json::str(&h.name)),
        ("score", Json::Float(round(h.score, 4))),
        ("tiers", Json::Obj(h.tiers.iter().map(|(t, v)| (metric(t).to_string(), Json::Float(round(*v, 4)))).collect())),
    ];
    if code == "full" {
        out.push(("code", Json::str(&h.code)));
    } else if code == "signature" {
        if let Some(sig) = &h.signature {
            out.push(("signature", Json::str(sig)));
        }
    }
    Json::obj(out)
}

fn cmd_search(common: &Common, query: &str, k: usize, scope: Scope, code: &str) -> Outcome {
    let mut index = open(common)?;
    let hits = match index.search(query, k, &scope) {
        Ok(h) => h,
        Err(e) => {
            if let Some(msg) = e.downcast_ref::<NotIndexed>().map(|n| n.0.clone())
                .or_else(|| e.downcast_ref::<BadValue>().map(|b| b.0.clone())) {
                return Err(fail(&msg, 2));
            }
            return Err(e.into());
        }
    };
    check_lang(&index, &scope.lang)?;
    let results: Vec<Json> = hits.iter().map(|h| hit_json(h, code)).collect();
    Out { text: common.text }.emit(&Json::obj(vec![
        ("query", Json::str(query)), ("count", Json::Int(results.len() as i64)), ("results", Json::List(results)),
    ]))?;
    Ok(0)
}

fn symbol_json(s: &Symbol) -> Json {
    let mut row = vec![
        ("name", Json::str(&s.name)), ("tag", Json::str(s.tag)), ("path", Json::str(&s.path)),
        ("start_line", Json::Int(s.start_line)), ("end_line", Json::Int(s.end_line)),
        ("lang", Json::str(&s.lang)), ("content_type", Json::str(&s.content_type)),
    ];
    if let Some(e) = &s.enclosing {
        row.push(("enclosing", Json::str(e)));
    }
    if let Some(sig) = &s.signature {
        row.push(("signature", Json::str(sig)));
    }
    Json::obj(row)
}

/// How many results fell in each category, most first; ties keep the
/// order each category was first seen, as `Counter.most_common` does.
fn by_content(symbols: &[Symbol]) -> Json {
    let mut counts: Vec<(String, i64)> = Vec::new();
    for s in symbols {
        match counts.iter_mut().find(|(k, _)| *k == s.content_type) {
            Some(slot) => slot.1 += 1,
            None => counts.push((s.content_type.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    Json::Obj(counts.into_iter().map(|(k, n)| (k, Json::Int(n))).collect())
}

fn cmd_named(common: &Common, name: &str, lang: &[String], tag: &str) -> Outcome {
    let mut index = open(common)?;
    let syms = if tag == "def" { index.definitions(name, lang)? } else { index.references(name, lang)? };
    // After the query, and whatever it found: `-l python pyton` matches
    // python and would otherwise drop the typo silently.
    check_lang(&index, lang)?;
    Out { text: common.text }.emit(&Json::obj(vec![
        ("name", Json::str(name)), ("count", Json::Int(syms.len() as i64)),
        ("by_content_type", by_content(&syms)), ("symbols", Json::List(syms.iter().map(symbol_json).collect())),
    ]))?;
    Ok(0)
}

fn cmd_symbols(common: &Common, pattern: Option<String>, tag: Option<String>, count_by: Option<String>,
               limit: Option<i64>, scope: Scope) -> Outcome {
    let mut index = open(common)?;
    let out = Out { text: common.text };
    if let Some(by) = count_by {
        let (counts, groups, total) = index.symbol_counts(&by, pattern.as_deref(), tag.as_deref(), &scope, limit)?;
        if counts.is_empty() {
            check_lang(&index, &scope.lang)?;
        }
        out.emit(&Json::obj(vec![
            ("count_by", Json::str(&by)), ("groups", Json::Int(groups)), ("total", Json::Int(total)),
            ("shown", Json::Int(counts.len() as i64)),
            ("counts", Json::Obj(counts.into_iter().map(|(g, n)| (g, Json::Int(n))).collect())),
        ]))?;
        return Ok(0);
    }
    let syms = index.symbols(pattern.as_deref(), tag.as_deref(), &scope, limit)?;
    if syms.is_empty() {
        check_lang(&index, &scope.lang)?;
    }
    out.emit(&Json::obj(vec![
        ("pattern", pattern.map_or(Json::Null, Json::Str)), ("count", Json::Int(syms.len() as i64)),
        ("by_content_type", by_content(&syms)), ("symbols", Json::List(syms.iter().map(symbol_json).collect())),
    ]))?;
    Ok(0)
}

fn cmd_index(common: &Common, force: bool) -> Outcome {
    let mut index = open(common)?;
    let r = index.refresh(force)?;
    Out { text: common.text }.emit(&Json::obj(vec![("status", Json::obj(vec![
        ("added", Json::Int(r.added as i64)), ("changed", Json::Int(r.changed as i64)),
        ("deleted", Json::Int(r.deleted as i64)), ("seconds", Json::Float(round(r.elapsed_s, 2))),
    ]))]))?;
    Ok(0)
}

/// Total size of the regular files under a directory, symlinked
/// directories not followed.
fn tree_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(meta) = std::fs::metadata(e.path()) {
                if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

fn cmd_status(common: &Common) -> Outcome {
    let index = open(common)?;
    let (files, chunks, symbols) = index.store.status_counts()?;
    let langs = index.store.top_languages(8)?;
    let db = index.paths.db();
    let dir = db.parent().unwrap();
    let size = tree_size(dir);
    Out { text: common.text }.emit(&Json::obj(vec![("status", Json::obj(vec![
        ("root", Json::str(index.paths.root.display().to_string())),
        ("index", Json::str(dir.display().to_string())),
        ("files", Json::Int(files)), ("chunks", Json::Int(chunks)), ("symbols", Json::Int(symbols)),
        ("size_mb", Json::Float(round(size as f64 / 1e6, 1))),
        ("languages", Json::str(langs.iter().map(|(n, c)| format!("{n} {c}")).collect::<Vec<_>>().join(", "))),
    ]))]))?;
    Ok(0)
}

/// Remove this directory's index, or with `--all` every index. Each
/// removal is reported after the fact.
fn cmd_clear(common: &Common, all: bool, dry_run: bool) -> Outcome {
    let mut paths = Paths::for_root(&root(common, false)?);
    if let Some(config) = &common.config {
        paths.data_dir = load(None, Some(config)).map_err(|e| fail(&e.0, 1))?.data_dir;
    }
    let targets: Vec<PathBuf> = if all {
        let root = paths.home.join("index");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root).map(|rd| rd.flatten().map(|e| e.path())
            .filter(|p| p.is_dir()).collect()).unwrap_or_default();
        dirs.sort();
        dirs
    } else {
        vec![paths.data()]
    };
    let out = Out { text: common.text };
    if targets.is_empty() {
        out.emit(&Json::obj(vec![("cleared", Json::List(vec![Json::str("nothing to clear")]))]))?;
        return Ok(0);
    }
    let mut removed: Vec<String> = Vec::new();
    let mut failed = false;
    for entry in targets {
        if !entry.is_dir() {
            continue;
        }
        let name = entry.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mb = tree_size(&entry) as f64 / 1e6;
        if dry_run {
            removed.push(format!("would remove {name}  {mb:.1} MB"));
            continue;
        }
        // A symlink is refused rather than unlinked, as shutil.rmtree does.
        let is_link = std::fs::symlink_metadata(&entry).is_ok_and(|m| m.file_type().is_symlink());
        if !is_link {
            let _ = std::fs::remove_dir_all(&entry);
        }
        if entry.exists() {
            failed = true;
            removed.push(format!("could NOT remove {}", entry.display()));
        } else {
            removed.push(format!("removed {name}  {mb:.1} MB"));
        }
    }
    if removed.is_empty() {
        removed.push("nothing matched".into());
    }
    out.emit(&Json::obj(vec![("cleared", Json::List(removed.into_iter().map(Json::Str).collect()))]))?;
    Ok(if failed { 1 } else { 0 })
}

/// The settings this invocation would use: the named config, or the
/// repository's. The user config is not read here, as in Python 0.3.2.
fn resolved(common: &Common) -> Result<Settings, Fail> {
    let path = match &common.config {
        Some(p) => p.clone(),
        None => root(common, true)?.join("repoglass.toml"),
    };
    load(None, Some(&path)).map_err(|e| fail(&e.0, 1))
}

const INIT_HEADER: &str = "# Written by `repoglass init`. Values are the ones resolved at\n\
# the time of writing, so this file PINS them: a later change to\n\
# a repoglass default will not reach this repository while the\n\
# key is present. Delete any key you would rather have track the\n\
# default, and regenerate with `repoglass init --force`.\n\n";

fn cmd_init(common: &Common, force: bool) -> Outcome {
    let target = root(common, true)?.join("repoglass.toml");
    if target.exists() && !force {
        return Err(fail(&format!("{} exists; pass --force to overwrite", target.display()), 1));
    }
    let settings = resolved(common)?;
    std::fs::write(&target, format!("{INIT_HEADER}{}", as_toml(&settings, true)))?;
    Out { text: common.text }.emit(&Json::obj(vec![("status", Json::obj(vec![
        ("wrote", Json::str(target.display().to_string())),
    ]))]))?;
    Ok(0)
}

fn cmd_config(common: &Common) -> Outcome {
    let settings = resolved(common)?;
    write!(std::io::stdout().lock(), "{}", as_toml(&settings, true))?;
    Ok(0)
}

/// `str()` of a payload value, as the text renderer prints it.
fn plain(v: &Json) -> String {
    match v {
        Json::Null => "None".into(),
        Json::Bool(b) => if *b { "True".into() } else { "False".into() },
        Json::Int(i) => i.to_string(),
        Json::Float(f) => float_repr(*f),
        Json::Str(s) => s.clone(),
        other => other.dumps(),
    }
}

fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Str(s)) => !s.is_empty(),
        Some(Json::Int(i)) => *i != 0,
        Some(Json::List(l)) => !l.is_empty(),
        Some(Json::Obj(o)) => !o.is_empty(),
        Some(_) => true,
    }
}

/// The same payload as the JSON, rendered for eyes.
fn render_text(payload: &Json) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    let get = |k: &str| payload.get(k);
    if let Some(Json::List(results)) = get("results") {
        for hit in results {
            let f = |k: &str| hit.get(k).map(plain).unwrap_or_default();
            let name = if truthy(hit.get("name")) { format!("  {}", f("name")) } else { String::new() };
            let tiers = match hit.get("tiers") {
                Some(Json::Obj(t)) => t.iter().map(|(k, v)| format!("{k}={}", plain(v))).collect::<Vec<_>>().join("  "),
                _ => String::new(),
            };
            line(format!("{}:{}-{}{name}", f("path"), f("start_line"), f("end_line")));
            line(format!("    score={}{}", f("score"), if tiers.is_empty() { String::new() } else { format!("  [{tiers}]") }));
            if truthy(hit.get("signature")) {
                line(format!("    {}", f("signature")));
            }
            if truthy(hit.get("code")) {
                line(f("code").trim_end_matches(crate::pyfmt::py_isspace).to_string());
            }
            line(String::new());
        }
    }
    if let Some(Json::List(symbols)) = get("symbols") {
        for sym in symbols {
            let f = |k: &str| sym.get(k).map(plain).unwrap_or_default();
            let wher = if truthy(sym.get("enclosing")) { format!("  in {}", f("enclosing")) } else { String::new() };
            line(format!("{}:{}  {}  ({}, {}){wher}", f("path"), f("start_line"), f("name"), f("tag"), f("content_type")));
            if truthy(sym.get("signature")) {
                line(format!("    {}", f("signature")));
            }
        }
    }
    if let Some(Json::Obj(counts)) = get("counts") {
        let width = counts.iter().map(|(g, _)| g.chars().count()).max().unwrap_or(0);
        for (group, n) in counts {
            line(format!("{group:<width$}  {}", plain(n)));
        }
        let groups = get("groups").map(plain).unwrap_or_default();
        let shown_n = match get("shown") { Some(Json::Int(i)) => *i, _ => i64::MAX };
        let groups_n = match get("groups") { Some(Json::Int(i)) => *i, _ => 0 };
        let shown = if shown_n < groups_n { format!("showing {shown_n} of ") } else { String::new() };
        line(format!("# {shown}{groups} group(s), {} symbol(s) by {}",
                     get("total").map(plain).unwrap_or_default(), get("count_by").map(plain).unwrap_or_default()));
    }
    if let Some(count) = get("count") {
        let detail = match get("by_content_type") {
            Some(Json::Obj(split)) if split.len() > 1 =>
                format!(": {}", split.iter().map(|(k, v)| format!("{k} {}", plain(v))).collect::<Vec<_>>().join(", ")),
            _ => String::new(),
        };
        line(format!("# {} result(s){detail}", plain(count)));
    }
    if let Some(Json::Obj(status)) = get("status") {
        for (key, value) in status {
            line(format!("{key:<18}{}", plain(value)));
        }
    }
    if let Some(Json::List(cleared)) = get("cleared") {
        for c in cleared {
            line(plain(c));
        }
    }
    out
}
