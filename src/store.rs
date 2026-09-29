//! All SQL: schema, connection setup, writes and queries. One database
//! per indexed directory, in the layout Python repoglass 0.3.2 writes.

use crate::config::{categories_rev, Settings};
use crate::models::{Chunk, SourceFile, Symbol};
use crate::semsift::encoders::Encoder;
use crate::semsift::filters::Filter;
use crate::semsift::fuse::RankedList;
use crate::semsift::items::{self, Item, Items};
use crate::text::{blake2b_hex, humanise};
use crate::SCHEMA_SQL;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Fingerprint of the DDL, semsift's layout included. A mismatch at open
/// drops and rebuilds every table.
pub fn schema_rev() -> String {
    blake2b_hex(format!("{SCHEMA_SQL}\n-- semsift layout {}", items::LAYOUT).as_bytes(), 8)
}

/// What shaped the stored rows. Any difference clears them.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub schema_rev: String,
    pub embed_model: String,
    pub embed_backend: String,
    pub embed_dims: i64,
    pub coverage: String,
    pub extractor_rev: String,
    pub categories_rev: String,
    pub embed_variant: String,
    pub embed_doc_prefix: String,
    pub embed_pooling: String,
    pub chunking_rev: String,
}

const CONTENT_TYPES: [&str; 5] = ["code", "tests", "docs", "config", "data"];
/// The one category `all` never covers.
const NEVER_IMPLICIT: [&str; 1] = ["data"];

/// A category, several, or nothing, as the categories to return, sorted.
/// `all` means every category but data; nothing means every category
/// but `content_excluded`.
pub fn normalise_content(content: &[String], settings: &Settings) -> Result<Vec<String>, String> {
    let default: Vec<String> = CONTENT_TYPES.iter().filter(|c| !settings.content_excluded.iter().any(|e| e == *c))
        .map(|c| c.to_string()).collect();
    if content.is_empty() {
        return Ok(default);
    }
    let named: Vec<&String> = content.iter().filter(|c| *c != "all").collect();
    let mut unknown: Vec<&str> = named.iter().filter(|c| !CONTENT_TYPES.contains(&c.as_str()))
        .map(|c| c.as_str()).collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        return Err(format!("unknown content type(s): {}; expected any of {}",
                           unknown.join(", "), CONTENT_TYPES.join(", ")));
    }
    let mut out: Vec<String> = named.iter().map(|s| s.to_string()).collect();
    if named.len() != content.len() {
        out.extend(CONTENT_TYPES.iter().filter(|c| !NEVER_IMPLICIT.contains(c)).map(|c| c.to_string()));
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The filters every symbol and chunk query applies to `f`, the file row.
#[derive(Clone, Debug, Default)]
pub struct Scope {
    pub content: Vec<String>,
    pub lang: Vec<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Scope {
    fn lang_clause(&self) -> String {
        if self.lang.is_empty() {
            return String::new();
        }
        format!(" AND f.lang IN ({})", self.lang.iter().map(|l| quote(l)).collect::<Vec<_>>().join(", "))
    }

    /// SQLite GLOB, so `*` crosses `/` and `src/*` is recursive.
    fn path_clause(&self) -> String {
        if self.include.is_empty() {
            return String::new();
        }
        format!(" AND ({})", self.include.iter().map(|p| format!("f.path GLOB {}", quote(p)))
            .collect::<Vec<_>>().join(" OR "))
    }

    /// Excluding wins over including: both are ANDed.
    fn exclude_clause(&self) -> String {
        if self.exclude.is_empty() {
            return String::new();
        }
        format!(" AND ({})", self.exclude.iter().map(|p| format!("f.path NOT GLOB {}", quote(p)))
            .collect::<Vec<_>>().join(" AND "))
    }
}

pub struct Store {
    pub conn: Connection,
    settings: Settings,
    /// Depth of nested `transaction` calls; writers commit at zero.
    depth: std::cell::Cell<usize>,
}

fn connect(db: &Path) -> anyhow::Result<Connection> {
    let dir = db.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    // Keeps the index directory out of `git status` when `data_dir` puts
    // it inside the repository.
    let marker = dir.join(".gitignore");
    if !marker.exists() {
        std::fs::write(&marker, "*\n")?;
    }
    let conn = Connection::open(db)?;
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA busy_timeout = 5000")?;
    Ok(conn)
}

fn has_table(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(()))
        .optional()?.is_some())
}

/// Drop every object this schema owns, views first.
fn drop_all(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = OFF")?;
    let objects: Vec<(String, String)> = {
        let mut st = conn.prepare(
            "SELECT name, type FROM sqlite_master WHERE type IN ('view','table','index') \
             AND name NOT LIKE 'sqlite_%' ORDER BY CASE type WHEN 'view' THEN 0 WHEN 'index' THEN 1 ELSE 2 END")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
    };
    for (name, kind) in objects {
        let _ = conn.execute_batch(&format!("DROP {} IF EXISTS \"{name}\"", kind.to_uppercase()));
    }
    conn.execute_batch("PRAGMA foreign_keys = ON")?;
    Ok(())
}

impl Store {
    /// Open the database, creating or rebuilding the schema as needed.
    pub fn open(db: &Path, settings: &Settings, extractor_rev: &str) -> anyhow::Result<Store> {
        let conn = connect(db)?;
        if has_table(&conn, "meta")? {
            let stored: Option<String> = conn.query_row("SELECT schema_rev FROM meta WHERE id=1", [], |r| r.get(0))
                .optional().ok().flatten();
            if stored.as_deref() != Some(schema_rev().as_str()) {
                drop_all(&conn)?;
            }
        }
        if !has_table(&conn, "meta")? {
            conn.execute_batch(SCHEMA_SQL)?;
            conn.execute(
                "INSERT INTO meta (id, schema_rev, embed_model, embed_backend, embed_dims, coverage, \
                 extractor_rev, categories_rev, last_scan_at) VALUES (1,?,?,?,?,?,?,?,0)",
                params![schema_rev(), settings.embed_model, settings.embed_backend, 0, settings.coverage,
                        extractor_rev, categories_rev(settings)])?;
        }
        Items::open(&conn)?;
        Ok(Store { conn, settings: settings.clone(), depth: std::cell::Cell::new(0) })
    }

    pub fn items(&self) -> Items<'_> {
        Items { conn: &self.conn }
    }

    /// Group writes so a failure leaves none of them. Nested calls join
    /// the outermost.
    pub fn transaction<T>(&self, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let outer = self.depth.get() == 0 && self.conn.is_autocommit();
        if outer {
            self.conn.execute_batch("BEGIN")?;
        }
        self.depth.set(self.depth.get() + 1);
        let result = f();
        self.depth.set(self.depth.get() - 1);
        if outer {
            match &result {
                Ok(_) => self.conn.execute_batch("COMMIT")?,
                Err(_) => self.conn.execute_batch("ROLLBACK")?,
            }
        }
        result
    }

    pub fn identity(&self) -> anyhow::Result<Option<Identity>> {
        Ok(self.conn.query_row(
            "SELECT schema_rev, embed_model, embed_backend, embed_dims, coverage, extractor_rev, \
             categories_rev, embed_variant, embed_doc_prefix, embed_pooling, chunking_rev FROM meta WHERE id = 1",
            [], |r| Ok(Identity {
                schema_rev: r.get(0)?, embed_model: r.get(1)?, embed_backend: r.get(2)?, embed_dims: r.get(3)?,
                coverage: r.get(4)?, extractor_rev: r.get(5)?, categories_rev: r.get(6)?,
                embed_variant: r.get(7)?, embed_doc_prefix: r.get(8)?, embed_pooling: r.get(9)?,
                chunking_rev: r.get(10)?,
            })).optional()?)
    }

    pub fn needs_reindex(&self, current: &Identity) -> anyhow::Result<bool> {
        Ok(self.identity()?.as_ref() != Some(current))
    }

    pub fn last_scan_at(&self) -> anyhow::Result<Option<f64>> {
        Ok(self.conn.query_row("SELECT last_scan_at FROM meta WHERE id=1", [], |r| r.get(0)).optional()?)
    }

    pub fn mark_scanned(&self) -> anyhow::Result<()> {
        self.transaction(|| Ok(self.conn.execute("UPDATE meta SET last_scan_at=? WHERE id=1", [items::now()])?))?;
        Ok(())
    }

    pub fn upsert_file(&self, f: &SourceFile) -> anyhow::Result<()> {
        self.transaction(|| {
            self.conn.execute(
                "INSERT INTO file (path, mtime_ns, size, lang, path_words, content_type) VALUES (?,?,?,?,?,?) \
                 ON CONFLICT(path) DO UPDATE SET mtime_ns=excluded.mtime_ns, size=excluded.size, \
                 lang=excluded.lang, content_type=excluded.content_type",
                params![f.path, f.mtime_ns, f.size, f.lang, humanise(&f.path), f.content_type])?;
            Ok(())
        })
    }

    pub fn delete_files(&self, paths: &[String]) -> anyhow::Result<()> {
        self.transaction(|| {
            for p in paths {
                self.conn.execute("DELETE FROM file WHERE path = ?", [p])?;
            }
            for chunk in paths.chunks(500) {
                let stale = self.items().select_ids(&Filter::In("path", chunk.to_vec()))?;
                if !stale.is_empty() {
                    self.items().remove(&stale)?;
                }
            }
            Ok(())
        })
    }

    fn file_id(&self, path: &str) -> anyhow::Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM file WHERE path=?", [path], |r| r.get(0)).optional()?)
    }

    /// Replace a file's symbols. Definitions go first, so a reference can
    /// point at the definition whose span holds it.
    pub fn replace_symbols(&self, path: &str, symbols: &[Symbol]) -> anyhow::Result<()> {
        let Some(fid) = self.file_id(path)? else { return Ok(()) };
        self.transaction(|| {
            let c = &self.conn;
            c.execute("DELETE FROM symbol WHERE file_id=?", [fid])?;
            {
                let mut ins = c.prepare_cached(
                    "INSERT INTO symbol (file_id, name, tag, start_line, end_line, signature) VALUES (?,?,?,?,?,?)")?;
                for s in symbols.iter().filter(|s| s.tag == "def") {
                    ins.execute(params![fid, s.name, s.tag, s.start_line, s.end_line, s.signature])?;
                }
            }
            let mut by_name: HashMap<String, Vec<(i64, i64, i64)>> = HashMap::new();
            {
                let mut st = c.prepare_cached("SELECT id, name, start_line, end_line FROM symbol WHERE file_id=? AND tag='def'")?;
                let rows = st.query_map([fid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?,
                                                       r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?;
                for row in rows {
                    let (sid, name, first, last) = row?;
                    by_name.entry(name).or_default().push((first, last, sid));
                }
            }
            // Matched on span as well as name: one file can define a name twice.
            let mut ins = c.prepare_cached(
                "INSERT INTO symbol (file_id, name, tag, start_line, end_line, enclosing_id) VALUES (?,?,?,?,?,?)")?;
            for s in symbols.iter().filter(|s| s.tag != "def") {
                let enclosing = by_name.get(s.enclosing.as_deref().unwrap_or(""))
                    .and_then(|v| v.iter().find(|(a, b, _)| *a <= s.start_line && s.start_line <= *b))
                    .map(|t| t.2);
                ins.execute(params![fid, s.name, s.tag, s.start_line, s.end_line, enclosing])?;
            }
            Ok(())
        })
    }

    /// Replace a file's chunks. Definition chunks link to their symbol by
    /// (name, start_line); the first chunk claiming a symbol wins.
    pub fn upsert_chunks(&self, path: &str, chunks: &[Chunk]) -> anyhow::Result<()> {
        let Some(fid) = self.file_id(path)? else { return Ok(()) };
        self.transaction(|| {
            let c = &self.conn;
            let ids: HashMap<(String, i64), i64> = {
                let mut st = c.prepare_cached("SELECT id, name, start_line FROM symbol WHERE file_id=? AND tag='def'")?;
                st.query_map([fid], |r| Ok(((r.get::<_, String>(1)?, r.get::<_, i64>(2)?), r.get::<_, i64>(0)?)))?
                    .collect::<Result<_, _>>()?
            };
            c.execute("DELETE FROM chunk WHERE file_id=?", [fid])?;
            let mut kept: Vec<&Chunk> = Vec::new();
            let mut claimed: HashSet<i64> = HashSet::new();
            {
                let mut ins = c.prepare_cached(
                    "INSERT INTO chunk (file_id, symbol_id, start_line, end_line, content_hash) VALUES (?,?,?,?,?)")?;
                for ch in chunks {
                    let sid = if ch.name.is_empty() { None } else { ids.get(&(ch.name.clone(), ch.start_line)).copied() };
                    if let Some(s) = sid {
                        if !claimed.insert(s) {
                            continue;
                        }
                    }
                    ins.execute(params![fid, sid, ch.start_line, ch.end_line, ch.content_hash])?;
                    kept.push(ch);
                }
            }
            let stale = self.items().select_ids(&Filter::In("path", vec![path.to_string()]))?;
            if !stale.is_empty() {
                self.items().remove(&stale)?;
            }
            let (words, content_type, lang): (String, String, String) = c.query_row(
                "SELECT path_words, content_type, lang FROM file WHERE id = ?", [fid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let chunk_ids: Vec<i64> = {
                let mut st = c.prepare_cached("SELECT id FROM chunk WHERE file_id = ? ORDER BY id")?;
                st.query_map([fid], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            let items: Vec<Item> = chunk_ids.iter().zip(&kept).map(|(id, ch)| Item {
                id: *id,
                text: ch.text.clone(),
                content_type: content_type.clone(),
                lang: lang.clone(),
                path: path.to_string(),
                keywords: if ch.lexical_override.is_none() { Some(words.clone()) } else { None },
                keyword_text: ch.lexical_override.clone(),
            }).collect();
            // One keyword rebuild per refresh costs less than per-file upkeep.
            self.items().defer_keywords()?;
            self.items().upsert(&items)?;
            Ok(())
        })
    }

    pub fn sync_keywords(&self) -> anyhow::Result<()> {
        self.transaction(|| self.items().sync_keywords())
    }

    /// Drop every indexed row, keeping meta.
    pub fn reset_content(&self) -> anyhow::Result<()> {
        self.transaction(|| {
            self.conn.execute("DELETE FROM file", [])?;
            self.items().clear()
        })
    }

    pub fn set_identity(&self, i: &Identity) -> anyhow::Result<()> {
        self.transaction(|| {
            self.conn.execute(
                "UPDATE meta SET schema_rev=?, embed_model=?, embed_backend=?, embed_dims=?, coverage=?, \
                 extractor_rev=?, categories_rev=?, embed_variant=?, embed_doc_prefix=?, embed_pooling=?, \
                 chunking_rev=? WHERE id=1",
                params![i.schema_rev, i.embed_model, i.embed_backend, i.embed_dims, i.coverage, i.extractor_rev,
                        i.categories_rev, i.embed_variant, i.embed_doc_prefix, i.embed_pooling, i.chunking_rev])?;
            Ok(())
        })
    }

    pub fn known_files(&self) -> anyhow::Result<HashMap<String, (i64, i64)>> {
        let mut st = self.conn.prepare("SELECT path, mtime_ns, size FROM file")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn symbol_rows_sql(&self, sql: &str, args: &[String]) -> anyhow::Result<Vec<Symbol>> {
        let mut st = self.conn.prepare(sql)?;
        let rows = st.query_map(params_from_iter(args), |r| Ok(Symbol {
            name: r.get(0)?, tag: if r.get::<_, String>(1)? == "def" { "def" } else { "ref" },
            path: r.get(2)?, start_line: r.get(3)?, end_line: r.get(4)?, lang: r.get(5)?,
            content_type: r.get(6)?, enclosing: r.get(7)?, signature: r.get(8)?,
        }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    const SYMBOL_SELECT: &str = "SELECT s.name, s.tag, f.path, s.start_line, s.end_line, f.lang, \
        f.content_type, e.name, s.signature FROM symbol s JOIN file f ON f.id = s.file_id \
        LEFT JOIN symbol e ON e.id = s.enclosing_id";

    /// Every symbol with this exact name and tag, in path and line order.
    pub fn named(&self, name: &str, tag: &str, lang: &[String]) -> anyhow::Result<Vec<Symbol>> {
        let mut sql = format!("{} WHERE s.name = ? AND s.tag = ?", Self::SYMBOL_SELECT);
        let mut args = vec![name.to_string(), tag.to_string()];
        if !lang.is_empty() {
            sql += &format!(" AND f.lang IN ({})", vec!["?"; lang.len()].join(","));
            args.extend(lang.iter().cloned());
        }
        sql += " ORDER BY f.path, s.start_line";
        self.symbol_rows_sql(&sql, &args)
    }

    /// Dimensions `symbol_counts` can group on, each a column.
    pub const COUNT_BY: [(&'static str, &'static str); 5] = [
        ("name", "s.name"), ("lang", "f.lang"), ("file", "f.path"), ("tag", "s.tag"), ("content", "f.content_type")];

    fn symbol_filter(&self, pattern: Option<&str>, tag: Option<&str>, scope: &Scope) -> anyhow::Result<(String, Vec<String>)> {
        let mut sql = String::new();
        let mut args = Vec::new();
        if let Some(p) = pattern.filter(|p| !p.is_empty()) {
            sql += " AND s.name GLOB ?";
            args.push(p.to_string());
        }
        if let Some(t) = tag.filter(|t| !t.is_empty()) {
            sql += " AND s.tag = ?";
            args.push(t.to_string());
        }
        sql += &scope.lang_clause();
        sql += &self.mode_clause(&scope.content)?;
        sql += &scope.path_clause();
        sql += &scope.exclude_clause();
        Ok((sql, args))
    }

    /// Every matching symbol in path order, read from the symbol table,
    /// so a definition too small to be chunked is listed.
    pub fn symbol_rows(&self, pattern: Option<&str>, tag: Option<&str>, scope: &Scope,
                       limit: Option<i64>) -> anyhow::Result<Vec<Symbol>> {
        let (wher, args) = self.symbol_filter(pattern, tag, scope)?;
        let mut sql = format!("{} WHERE 1=1{wher} ORDER BY f.path, s.start_line", Self::SYMBOL_SELECT);
        if let Some(l) = limit.filter(|l| *l != 0) {
            sql += &format!(" LIMIT {l}");
        }
        self.symbol_rows_sql(&sql, &args)
    }

    /// Matching symbols grouped by one column, largest group first, with
    /// the group and symbol totals over everything matched.
    pub fn symbol_counts(&self, by: &str, pattern: Option<&str>, tag: Option<&str>, scope: &Scope,
                         limit: Option<i64>) -> anyhow::Result<(Vec<(String, i64)>, i64, i64)> {
        let column = Self::COUNT_BY.iter().find(|(k, _)| *k == by).map(|(_, c)| *c)
            .ok_or_else(|| anyhow::anyhow!("cannot count by {by}"))?;
        let (wher, args) = self.symbol_filter(pattern, tag, scope)?;
        let from = format!(" FROM symbol s JOIN file f ON f.id = s.file_id WHERE 1=1{wher}");
        let (groups, total): (i64, i64) = self.conn.query_row(
            &format!("SELECT COUNT(DISTINCT {column}), COUNT(*){from}"), params_from_iter(&args),
            |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut sql = format!("SELECT {column}, COUNT(*) c{from} GROUP BY {column} ORDER BY c DESC, {column} ASC");
        if let Some(l) = limit.filter(|l| *l != 0) {
            sql += &format!(" LIMIT {l}");
        }
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(&args), |r| {
            let g: rusqlite::types::Value = r.get(0)?;
            let text = match g {
                rusqlite::types::Value::Text(s) => s,
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Real(f) => crate::pyfmt::float_repr(f),
                rusqlite::types::Value::Null => "None".into(),
                rusqlite::types::Value::Blob(_) => String::new(),
            };
            Ok((text, r.get(1)?))
        })?;
        Ok((rows.collect::<Result<_, _>>()?, groups, total))
    }

    /// Categories as SQL on `f`; nothing requested means the default set.
    fn mode_clause(&self, content: &[String]) -> anyhow::Result<String> {
        let wanted = normalise_content(content, &self.settings).map_err(anyhow::Error::msg)?;
        if wanted.is_empty() {
            return Ok(String::new());
        }
        Ok(format!(" AND f.content_type IN ({})", wanted.iter().map(|w| quote(w)).collect::<Vec<_>>().join(", ")))
    }

    /// repoglass's filters as a semsift filter, applied before both searches.
    fn item_filter(&self, scope: &Scope) -> anyhow::Result<Option<Filter>> {
        let mut parts = Vec::new();
        let wanted = normalise_content(&scope.content, &self.settings).map_err(anyhow::Error::msg)?;
        if !wanted.is_empty() {
            parts.push(Filter::In("content_type", wanted));
        }
        if !scope.lang.is_empty() {
            parts.push(Filter::In("lang", scope.lang.clone()));
        }
        if !scope.include.is_empty() {
            parts.push(Filter::Or(scope.include.iter().map(|p| Filter::Glob("path", p.clone())).collect()));
        }
        parts.extend(scope.exclude.iter().map(|p| Filter::Not(Box::new(Filter::Glob("path", p.clone())))));
        Ok(match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(Filter::And(parts)),
        })
    }

    /// (chunk id, bm25) for any of the query's words; lower bm25 is better.
    pub fn fts_search(&self, query: &str, limit: usize, scope: &Scope) -> anyhow::Result<Vec<(i64, f64)>> {
        let ranked = self.items().search_keyword(query, limit, self.item_filter(scope)?.as_ref())?;
        Ok(ranked.items.iter().map(|s| (s.id, s.raw)).collect())
    }

    pub fn vector_search(&self, encoder: &Encoder, query: &str, limit: usize, scope: &Scope) -> anyhow::Result<RankedList> {
        self.items().search_vector(encoder, query, limit, self.item_filter(scope)?.as_ref())
    }

    /// Embed every chunk without a vector; the model runs outside the transaction.
    pub fn embed_missing(&self, encoder: &Encoder) -> anyhow::Result<usize> {
        let ids = self.items().missing_vectors()?;
        if ids.is_empty() {
            return Ok(0);
        }
        let space = encoder.space();
        if let Some(stored) = self.items().space()? {
            anyhow::ensure!(stored == space, "store 'rg' holds vectors in {stored}; the encoder writes {space}");
        }
        let texts = self.items().texts(&ids)?;
        let refs: Vec<&str> = ids.iter().map(|i| texts[i].as_str()).collect();
        let vectors = encoder.encode(&refs)?;
        let canary = if self.items().space()?.is_none() { Some(Items::canary(encoder)?) } else { None };
        self.transaction(|| self.items().add_vectors(&ids, &vectors, &space, canary.as_deref()))?;
        Ok(ids.len())
    }

    pub fn set_embed_dims(&self, dims: i64) -> anyhow::Result<()> {
        self.transaction(|| Ok(self.conn.execute("UPDATE meta SET embed_dims=? WHERE id=1", [dims])?))?;
        Ok(())
    }

    fn marks(n: usize) -> String {
        vec!["?"; n].join(",")
    }

    /// chunk id -> (path, start_line, end_line, name, text, signature).
    pub fn hits(&self, ids: &[i64]) -> anyhow::Result<HashMap<i64, (String, i64, i64, String, String, Option<String>)>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let texts = self.items().texts(ids)?;
        let mut st = self.conn.prepare(&format!(
            "SELECT c.id, f.path, c.start_line, c.end_line, s.name, s.signature FROM chunk c \
             JOIN file f ON f.id = c.file_id LEFT JOIN symbol s ON s.id = c.symbol_id WHERE c.id IN ({})",
            Self::marks(ids.len())))?;
        let rows = st.query_map(params_from_iter(ids), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, path, a, b, name, sig) = row?;
            let text = texts.get(&id).cloned().unwrap_or_default();
            out.insert(id, (path, a, b, name.unwrap_or_default(), text, sig));
        }
        Ok(out)
    }

    /// chunk id -> (path, text).
    pub fn chunk_rows(&self, ids: &[i64]) -> anyhow::Result<HashMap<i64, (String, String)>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let texts = self.items().texts(ids)?;
        let mut st = self.conn.prepare(&format!(
            "SELECT c.id, f.path FROM chunk c JOIN file f ON f.id = c.file_id WHERE c.id IN ({})",
            Self::marks(ids.len())))?;
        let rows = st.query_map(params_from_iter(ids), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, path) = row?;
            out.insert(id, (path, texts.get(&id).cloned().unwrap_or_default()));
        }
        Ok(out)
    }

    /// Chunks whose file path contains one of these names, lowercased,
    /// inside the search's filters; names under three characters are dropped.
    pub fn non_candidate_rows(&self, names: &HashSet<String>, scope: &Scope) -> anyhow::Result<Vec<(i64, String, String)>> {
        let mut stems: Vec<String> = names.iter().filter(|n| n.chars().count() >= 3).map(|n| n.to_lowercase()).collect();
        stems.sort();
        stems.dedup();
        if stems.is_empty() {
            return Ok(vec![]);
        }
        let wher = vec!["lower(f.path) LIKE ?"; stems.len()].join(" OR ");
        let sql = format!("SELECT c.id, f.path FROM chunk c JOIN file f ON f.id = c.file_id WHERE ({wher}){}{}{}{} LIMIT 200",
                          self.mode_clause(&scope.content)?, scope.lang_clause(), scope.path_clause(),
                          scope.exclude_clause());
        let args: Vec<String> = stems.iter().map(|s| format!("%{s}%")).collect();
        let rows: Vec<(i64, String)> = {
            let mut st = self.conn.prepare(&sql)?;
            st.query_map(params_from_iter(&args), |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let texts = self.items().texts(&rows.iter().map(|r| r.0).collect::<Vec<_>>())?;
        Ok(rows.into_iter().map(|(id, path)| {
            let text = texts.get(&id).cloned().unwrap_or_default();
            (id, path, text)
        }).collect())
    }

    /// Chunk ids for definitions of `name` that have a chunk and pass the
    /// filter: the definition's own chunk, or under hybrid coverage the
    /// window holding a definition too small to have one.
    pub fn chunked_definitions(&self, name: &str, scope: &Scope) -> anyhow::Result<Vec<i64>> {
        let sql = format!(
            "SELECT DISTINCT c.id FROM symbol s JOIN file f ON f.id = s.file_id JOIN chunk c ON c.file_id = f.id \
             AND (c.symbol_id = s.id OR (c.symbol_id IS NULL AND s.start_line BETWEEN c.start_line AND c.end_line)) \
             WHERE s.name=? AND s.tag='def'{}{}{}{} ORDER BY f.path, s.start_line",
            self.mode_clause(&scope.content)?, scope.lang_clause(), scope.path_clause(), scope.exclude_clause());
        let mut st = self.conn.prepare(&sql)?;
        Ok(st.query_map([name], |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    pub fn status_counts(&self) -> anyhow::Result<(i64, i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT (SELECT count(*) FROM file), (SELECT count(*) FROM chunk), (SELECT count(*) FROM symbol)",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)
    }

    pub fn top_languages(&self, limit: i64) -> anyhow::Result<Vec<(String, i64)>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT lang, count(*) FROM file GROUP BY lang ORDER BY 2 DESC LIMIT {limit}"))?;
        Ok(st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn content_defaults_and_all() {
        let s = Settings::default();
        assert_eq!(normalise_content(&[], &s).unwrap(), v(&["code", "tests", "docs", "config"]));
        assert_eq!(normalise_content(&v(&["all"]), &s).unwrap(), v(&["code", "config", "docs", "tests"]));
        assert_eq!(normalise_content(&v(&["all", "data"]), &s).unwrap(), v(&["code", "config", "data", "docs", "tests"]));
        assert_eq!(normalise_content(&v(&["docs"]), &s).unwrap(), v(&["docs"]));
        assert!(normalise_content(&v(&["cod"]), &s).unwrap_err().contains("unknown content type(s): cod"));
    }
}
