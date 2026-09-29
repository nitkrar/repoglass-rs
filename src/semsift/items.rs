//! semsift's item store for repoglass's chunks: items, their vectors and
//! an FTS5 keyword index, in tables named `rg_*` inside the index
//! database. Table layout and every stored value match semsift 0.0.5.
//!
//! The store never commits: writes join the caller's transaction.

use super::codec::{dot32, norm32, pack, unit_rows, unpack};
use super::encoders::Encoder;
use super::filters::{compile, Filter};
use super::fuse::{RankedList, Scored};
use super::space::VectorSpace;
use crate::pyfmt::Json;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::HashMap;

/// semsift's table layout version, part of the index schema revision.
pub const LAYOUT: i64 = 2;
const PREFIX: &str = "rg";
const BATCH: usize = 500;
/// Declared fields: a chunk's file category, language and path.
pub const FIELDS: [&str; 3] = ["content_type", "lang", "path"];

/// Fixed texts whose vectors are recorded with the first vectors, so a
/// later re-encode shows whether the encoder still produces the same outputs.
pub const CANARY_TEXTS: [&str; 3] = [
    "The landlord renewed the lease; rent is due on the first.",
    "def add(a, b):\n    return a + b",
    "when is the next dentist appointment",
];
const DRIFT_WARN: f64 = 0.9999;
const DRIFT_STALE: f64 = 0.99;

/// The encoder's outputs moved too far from the stored vectors to rank by.
#[derive(Debug)]
pub struct StaleVectors(pub String);

impl std::fmt::Display for StaleVectors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StaleVectors {}

pub struct Item {
    pub id: i64,
    pub text: String,
    pub content_type: String,
    pub lang: String,
    pub path: String,
    pub keyword_text: Option<String>,
    pub keywords: Option<String>,
}

fn declaration() -> String {
    let fields: Vec<Json> = FIELDS.iter().map(|f| Json::obj(vec![
        ("name", Json::str(*f)), ("kind", Json::str("text")), ("indexed", Json::Bool(true)),
    ])).collect();
    Json::obj(vec![
        ("layout", Json::Int(LAYOUT)),
        ("fields", Json::List(fields)),
        ("tokenizer", Json::str("unicode61")),
    ]).dumps()
}

fn keyword_text(row: &str) -> String {
    format!("coalesce({row}.keyword_override, CASE WHEN {row}.keywords IS NULL THEN {row}.text \
             ELSE {row}.keywords || char(10) || {row}.text END)")
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = a.iter().map(|x| x * x).sum::<f64>().sqrt() * b.iter().map(|y| y * y).sum::<f64>().sqrt();
    if norm != 0.0 { dot / norm } else { 0.0 }
}

/// The store over one connection. Cheap: holds no state of its own.
pub struct Items<'c> {
    pub conn: &'c Connection,
}

impl<'c> Items<'c> {
    /// Open the tables, creating them when absent. A store declared with
    /// other fields is refused.
    pub fn open(conn: &'c Connection) -> anyhow::Result<Items<'c>> {
        let exists = conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                                    [format!("{PREFIX}_meta")], |_| Ok(())).optional()?.is_some();
        let decl = declaration();
        if exists {
            let stored: String = conn.query_row(&format!("SELECT declaration FROM \"{PREFIX}_meta\""), [], |r| r.get(0))?;
            anyhow::ensure!(stored == decl, "store '{PREFIX}' was created with {stored}");
        } else {
            create(conn, &decl)?;
        }
        Ok(Items { conn })
    }

    fn bump(&self, reset_space: bool) -> anyhow::Result<()> {
        let sql = if reset_space {
            format!("UPDATE \"{PREFIX}_meta\" SET generation = generation + 1, space = NULL, \
                     canary = NULL, embedded_at = NULL")
        } else {
            format!("UPDATE \"{PREFIX}_meta\" SET generation = generation + 1")
        };
        self.conn.execute(&sql, [])?;
        Ok(())
    }

    fn has_vectors(&self) -> anyhow::Result<bool> {
        Ok(self.conn.query_row(&format!("SELECT 1 FROM \"{PREFIX}_vectors\" LIMIT 1"), [], |_| Ok(()))
            .optional()?.is_some())
    }

    pub fn space(&self) -> anyhow::Result<Option<VectorSpace>> {
        let text: Option<String> = self.conn.query_row(&format!("SELECT space FROM \"{PREFIX}_meta\""), [], |r| r.get(0))?;
        text.map(|t| VectorSpace::from_json(&t)).transpose()
    }

    /// Write items without vectors, replacing rows with the same ids.
    pub fn upsert(&self, items: &[Item]) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        {
            let mut ins = self.conn.prepare_cached(&format!(
                "INSERT INTO \"{PREFIX}_items\" (id, text, keywords, keyword_override, \"content_type\", \
                 \"lang\", \"path\", extra) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET \
                 text = excluded.text, keywords = excluded.keywords, keyword_override = \
                 excluded.keyword_override, \"content_type\" = excluded.\"content_type\", \"lang\" = \
                 excluded.\"lang\", \"path\" = excluded.\"path\", extra = excluded.extra"))?;
            for it in items {
                ins.execute(params![it.id, it.text, it.keywords, it.keyword_text, it.content_type,
                                    it.lang, it.path, "{}"])?;
            }
            let mut del = self.conn.prepare_cached(&format!("DELETE FROM \"{PREFIX}_vectors\" WHERE id = ?"))?;
            for it in items {
                del.execute([it.id])?;
            }
        }
        let reset = !self.has_vectors()?;
        self.bump(reset)
    }

    pub fn remove(&self, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        for chunk in ids.chunks(BATCH) {
            let marks = vec!["?"; chunk.len()].join(", ");
            for table in ["items", "vectors"] {
                self.conn.execute(&format!("DELETE FROM \"{PREFIX}_{table}\" WHERE id IN ({marks})"),
                                  params_from_iter(chunk))?;
            }
        }
        let reset = !self.has_vectors()?;
        self.bump(reset)
    }

    /// Delete everything and forget the vector space.
    pub fn clear(&self) -> anyhow::Result<()> {
        for table in ["items", "vectors"] {
            self.conn.execute(&format!("DELETE FROM \"{PREFIX}_{table}\""), [])?;
        }
        self.bump(true)
    }

    pub fn select_ids(&self, filter: &Filter) -> anyhow::Result<Vec<i64>> {
        let w = compile(Some(filter));
        let mut st = self.conn.prepare(&format!("SELECT i.id FROM \"{PREFIX}_items\" i WHERE {}", w.sql))?;
        Ok(st.query_map(params_from_iter(&w.params), |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    pub fn texts(&self, ids: &[i64]) -> anyhow::Result<HashMap<i64, String>> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(BATCH) {
            let marks = vec!["?"; chunk.len()].join(", ");
            let mut st = self.conn.prepare(&format!(
                "SELECT i.id, i.text FROM \"{PREFIX}_items\" i WHERE i.id IN ({marks})"))?;
            let rows = st.query_map(params_from_iter(chunk), |r| Ok((r.get(0)?, r.get(1)?)))?;
            for row in rows {
                let (id, text) = row?;
                out.insert(id, text);
            }
        }
        Ok(out)
    }

    /// Ids of stored items that have no vector, in id order.
    pub fn missing_vectors(&self) -> anyhow::Result<Vec<i64>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT i.id FROM \"{PREFIX}_items\" i LEFT JOIN \"{PREFIX}_vectors\" v ON v.id = i.id \
             WHERE v.id IS NULL ORDER BY i.id"))?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    /// The canary vectors, one text per call: a vector can depend on
    /// what shares its batch.
    pub fn canary(encoder: &Encoder) -> anyhow::Result<Vec<Vec<f64>>> {
        CANARY_TEXTS.iter().map(|t| Ok(encoder.encode(&[t])?.remove(0))).collect()
    }

    /// Vectors for items already stored. The first vectors fix the space,
    /// the canary and the time.
    pub fn add_vectors(&self, ids: &[i64], rows: &[Vec<f64>], space: &VectorSpace,
                       canary: Option<&[Vec<f64>]>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        {
            let mut ins = self.conn.prepare_cached(&format!(
                "INSERT INTO \"{PREFIX}_vectors\" (id, vec) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET vec = excluded.vec"))?;
            for (id, v) in ids.iter().zip(rows) {
                ins.execute(params![id, pack(v)])?;
            }
        }
        let canary_json = canary.map(|c| Json::List(c.iter()
            .map(|row| Json::List(row.iter().map(|x| Json::Float(*x)).collect())).collect()).dumps());
        self.conn.execute(&format!(
            "UPDATE \"{PREFIX}_meta\" SET generation = generation + 1, \
             embedded_at = CASE WHEN space IS NULL THEN ? ELSE embedded_at END, \
             canary = CASE WHEN space IS NULL THEN ? ELSE canary END, \
             space = coalesce(space, ?)"),
            params![now(), canary_json, space.to_json()])?;
        Ok(())
    }

    pub fn keywords_stale(&self) -> anyhow::Result<bool> {
        let v: i64 = self.conn.query_row(&format!("SELECT keywords_stale FROM \"{PREFIX}_meta\""), [], |r| r.get(0))?;
        Ok(v != 0)
    }

    /// Stop maintaining the keyword index per write until `sync_keywords`.
    pub fn defer_keywords(&self) -> anyhow::Result<()> {
        self.conn.execute(&format!("UPDATE \"{PREFIX}_meta\" SET keywords_stale = 1"), [])?;
        Ok(())
    }

    pub fn sync_keywords(&self) -> anyhow::Result<()> {
        if self.keywords_stale()? {
            self.conn.execute(&format!("INSERT INTO \"{PREFIX}_fts\" (\"{PREFIX}_fts\") VALUES ('rebuild')"), [])?;
            self.conn.execute(&format!("UPDATE \"{PREFIX}_meta\" SET keywords_stale = 0"), [])?;
        }
        Ok(())
    }

    /// FTS5 BM25, best first. `raw` is bm25(), where lower is better.
    pub fn search_keyword(&self, query: &str, k: usize, filter: Option<&Filter>) -> anyhow::Result<RankedList> {
        anyhow::ensure!(!self.keywords_stale()?,
                        "store '{PREFIX}': keyword writes were deferred; run sync_keywords");
        let w = compile(filter);
        if k == 0 {
            return Ok(RankedList::new("keyword", vec![]));
        }
        // Each word quoted, so FTS5 operators in the query are plain words.
        let words: Vec<String> = word_re().find_iter(query).map(|m| format!("\"{}\"", m.as_str())).collect();
        if words.is_empty() {
            return Ok(RankedList::new("keyword", vec![]));
        }
        let mut args: Vec<rusqlite::types::Value> = vec![words.join(" OR ").into()];
        args.extend(w.params.iter().map(|p| p.clone().into()));
        args.push((k as i64).into());
        let mut st = self.conn.prepare(&format!(
            "SELECT f.rowid, bm25(\"{PREFIX}_fts\") FROM \"{PREFIX}_fts\" f JOIN \"{PREFIX}_items\" i ON i.id = f.rowid \
             WHERE \"{PREFIX}_fts\" MATCH ? AND ({}) ORDER BY bm25(\"{PREFIX}_fts\"), f.rowid LIMIT ?", w.sql))?;
        let items = st.query_map(params_from_iter(args), |r| {
            let (id, raw): (i64, f64) = (r.get(0)?, r.get(1)?);
            Ok(Scored { id, score: -raw, raw })
        })?.collect::<Result<_, _>>()?;
        Ok(RankedList::new("keyword", items))
    }

    /// Cosine similarity to `query` encoded with `encode_query`, best
    /// first, after the canary check. Errors with `StaleVectors` when the
    /// encoder moved too far from the stored vectors.
    pub fn search_vector(&self, encoder: &Encoder, query: &str, k: usize,
                         filter: Option<&Filter>) -> anyhow::Result<RankedList> {
        let stored = self.space()?;
        if let Some(stored) = &stored {
            anyhow::ensure!(*stored == encoder.space(),
                            "store '{PREFIX}' holds vectors in {stored}; the encoder writes {}", encoder.space());
        }
        let w = compile(filter);
        let Some(space) = stored else { return Ok(RankedList::new("vector", vec![])) };
        let canary: Option<String> = self.conn.query_row(&format!("SELECT canary FROM \"{PREFIX}_meta\""), [], |r| r.get(0))?;
        let mut warnings = Vec::new();
        if let Some(canary) = canary {
            let recorded: Vec<Vec<f64>> = serde_json::from_str(&canary)?;
            let fresh = Self::canary(encoder)?;
            let similarity = recorded.iter().zip(&fresh).map(|(a, b)| cosine(a, b)).fold(f64::INFINITY, f64::min);
            if similarity < DRIFT_STALE {
                return Err(StaleVectors(format!(
                    "store '{PREFIX}': the encoder's outputs moved from the stored vectors \
                     (canary similarity {similarity:.4}); re-embed needed")).into());
            }
            if similarity < DRIFT_WARN {
                warnings.push(format!(
                    "store '{PREFIX}': the encoder's outputs drifted from the stored vectors \
                     (canary similarity {similarity:.4}); re-embed recommended"));
            }
        }
        let q64 = encoder.encode_query(&[query])?.remove(0);
        anyhow::ensure!(space.dims == 0 || q64.len() as i64 == space.dims,
                        "query width {}; stored width {}", q64.len(), space.dims);
        if k == 0 {
            return Ok(RankedList { source: "vector".into(), items: vec![], warnings });
        }
        let mut q: Vec<f32> = q64.iter().map(|&x| x as f32).collect();
        let qn = norm32(&q);
        if qn != 0.0 {
            q.iter_mut().for_each(|x| *x /= qn);
        }
        let mut st = self.conn.prepare(&format!(
            "SELECT v.id, v.vec FROM \"{PREFIX}_vectors\" v JOIN \"{PREFIX}_items\" i ON i.id = v.id \
             WHERE {} ORDER BY v.id", w.sql))?;
        let mut ids = Vec::new();
        let mut matrix: Vec<f32> = Vec::new();
        let mut rows = st.query(params_from_iter(&w.params))?;
        while let Some(r) = rows.next()? {
            ids.push(r.get::<_, i64>(0)?);
            matrix.extend(unpack(r.get_ref(1)?.as_blob()?));
        }
        let d = q.len();
        unit_rows(&mut matrix, d);
        let mut scored: Vec<(i64, f64)> = matrix.chunks_exact(d.max(1)).zip(&ids)
            .map(|(row, &id)| (id, dot32(row, &q) as f64)).collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(k);
        Ok(RankedList {
            source: "vector".into(),
            items: scored.into_iter().map(|(id, s)| Scored { id, score: s, raw: s }).collect(),
            warnings,
        })
    }
}

fn word_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\w+").unwrap())
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64()
}

fn create(conn: &Connection, decl: &str) -> anyhow::Result<()> {
    let t = |name: &str| format!("\"{PREFIX}_{name}\"");
    let live = format!("WHEN (SELECT keywords_stale FROM {}) = 0", t("meta"));
    let fts_add = |row: &str| format!("INSERT INTO {} (rowid, keyword_text) VALUES ({row}.id, {});",
                                      t("fts"), keyword_text(row));
    let fts_drop = |row: &str| format!("INSERT INTO {} ({}, rowid, keyword_text) VALUES ('delete', {row}.id, {});",
                                       t("fts"), t("fts"), keyword_text(row));
    let cols: String = FIELDS.iter().map(|f| format!(", \"{f}\" TEXT")).collect();
    let mut statements = vec![
        format!("CREATE TABLE {} (id INTEGER PRIMARY KEY CHECK (id = 1), declaration TEXT NOT NULL, \
                 space TEXT, canary TEXT, embedded_at REAL, generation INTEGER NOT NULL DEFAULT 0, \
                 keywords_stale INTEGER NOT NULL DEFAULT 0)", t("meta")),
        format!("INSERT INTO {} (id, declaration) VALUES (1, ?)", t("meta")),
        format!("CREATE TABLE {} (id INTEGER PRIMARY KEY, text TEXT NOT NULL, keywords TEXT, \
                 keyword_override TEXT{cols}, extra TEXT NOT NULL DEFAULT '{{}}')", t("items")),
        format!("CREATE TABLE {} (id INTEGER PRIMARY KEY, vec BLOB NOT NULL)", t("vectors")),
        format!("CREATE VIEW {} AS SELECT id, {} AS keyword_text FROM {} items",
                t("keyword"), keyword_text("items"), t("items")),
        format!("CREATE VIRTUAL TABLE {} USING fts5(keyword_text, content='{PREFIX}_keyword', \
                 content_rowid='id', tokenize='unicode61')", t("fts")),
        format!("CREATE TRIGGER \"{PREFIX}_items_insert\" AFTER INSERT ON {} {live} BEGIN {} END",
                t("items"), fts_add("new")),
        format!("CREATE TRIGGER \"{PREFIX}_items_delete\" AFTER DELETE ON {} {live} BEGIN {} END",
                t("items"), fts_drop("old")),
        format!("CREATE TRIGGER \"{PREFIX}_items_update\" AFTER UPDATE OF text, keywords, keyword_override \
                 ON {} {live} BEGIN {} {} END", t("items"), fts_drop("old"), fts_add("new")),
    ];
    statements.extend(FIELDS.iter().map(|f| format!(
        "CREATE INDEX \"{PREFIX}_items_{f}\" ON {} (\"{f}\")", t("items"))));
    conn.execute_batch(&format!("SAVEPOINT \"semsift_{PREFIX}_schema\""))?;
    for sql in &statements {
        let result = if sql.contains("VALUES (1, ?)") { conn.execute(sql, [decl]) } else { conn.execute(sql, []) };
        if let Err(e) = result {
            conn.execute_batch(&format!("ROLLBACK TO \"semsift_{PREFIX}_schema\"; RELEASE \"semsift_{PREFIX}_schema\""))?;
            return Err(e.into());
        }
    }
    conn.execute_batch(&format!("RELEASE \"semsift_{PREFIX}_schema\""))?;
    Ok(())
}
