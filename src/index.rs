//! The facade: open an index, bring it up to date, and query it.

use crate::config::{categories_rev, chunking_rev, load, ConfigError, Paths, Settings};
use crate::corpus::{discovery, extract, languages};
use crate::embeddings;
use crate::models::{Chunk, Hit, RefreshReport, SourceFile, Symbol};
use crate::pyfmt::{char_prefix, py_splitlines, py_strip};
use crate::search::boosting::Candidate;
use crate::search::{fuse, lexical, looks_like_prose, rank};
use crate::semsift::encoders::Encoder;
use crate::semsift::fuse::RankedList;
use crate::semsift::items::StaleVectors;
use crate::store::{normalise_content, Identity, Scope, Store};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub struct Index {
    pub store: Store,
    pub settings: Settings,
    pub paths: Paths,
    encoder: Option<Option<Encoder>>,
}

/// A category the index does not hold; exit 2.
#[derive(Debug)]
pub struct NotIndexed(pub String);

impl std::fmt::Display for NotIndexed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotIndexed {}

/// A bad argument value, such as an unknown content type; exit 2.
#[derive(Debug)]
pub struct BadValue(pub String);

impl std::fmt::Display for BadValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadValue {}

fn is_busy(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<rusqlite::Error>().is_some_and(|e| matches!(
        e.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))))
}

impl Index {
    /// Open or create the index for a directory. With `settings` None,
    /// they resolve from the user and repository configs.
    pub fn open(paths: Paths, settings: Option<Settings>) -> anyhow::Result<Index> {
        let settings = match settings {
            Some(s) => s,
            None => load(Some(&paths.user_config()), Some(&paths.repo_config()))
                .map_err(|e: ConfigError| anyhow::Error::new(e))?,
        };
        let mut paths = paths;
        if settings.data_dir != paths.data_dir {
            paths.data_dir = settings.data_dir.clone();
        }
        let store = Store::open(&paths.db(), &settings, &languages::extractor_rev())?;
        Ok(Index { store, settings, paths, encoder: None })
    }

    fn embed(&mut self) -> anyhow::Result<Option<&Encoder>> {
        if self.encoder.is_none() {
            self.encoder = Some(embeddings::build(&self.settings)?);
        }
        Ok(self.encoder.as_ref().unwrap().as_ref())
    }

    /// Bring the index up to date. A file counts as changed when its
    /// (mtime_ns, size) differs, by inequality: mtime moves backwards on
    /// checkout and archive extraction.
    pub fn refresh(&mut self, force: bool) -> anyhow::Result<RefreshReport> {
        let started = Instant::now();
        self.reset_if_needed(force)?;
        let walked: HashMap<String, SourceFile> = discovery::walk(&self.paths, &self.settings)
            .into_iter().map(|f| (f.path.clone(), f)).collect();
        let indexed = self.store.known_files()?;
        let added: HashSet<&String> = walked.keys().filter(|p| !indexed.contains_key(*p)).collect();
        let mut deleted: Vec<String> = indexed.keys().filter(|p| !walked.contains_key(*p)).cloned().collect();
        deleted.sort();
        let changed: HashSet<&String> = walked.iter()
            .filter(|(p, f)| indexed.get(*p).is_some_and(|&v| v != (f.mtime_ns, f.size)))
            .map(|(p, _)| p).collect();
        if !deleted.is_empty() {
            self.store.delete_files(&deleted)?;
        }
        let mut touched: Vec<&String> = added.union(&changed).copied().collect();
        touched.sort();
        for (path, result) in touched.iter().zip(self.extract_all(&touched, &walked)?) {
            // No row for an unreadable file, so the next walk tries again.
            let Some((symbols, chunks)) = result else { continue };
            let file = &walked[*path];
            // One transaction per file: the row says the file is indexed,
            // and the chunks are what make that true.
            self.store.transaction(|| {
                self.store.upsert_file(file)?;
                self.store.replace_symbols(&file.path, &symbols)?;
                self.store.upsert_chunks(&file.path, &chunks)
            })?;
        }
        self.store.sync_keywords()?;
        self.embed_pending()?;
        self.store.mark_scanned()?;
        Ok(RefreshReport {
            added: added.len(),
            changed: changed.len(),
            deleted: deleted.len(),
            elapsed_s: started.elapsed().as_secs_f64(),
        })
    }

    /// Read and extract every touched file on all cores, in input order.
    fn extract_all(&self, touched: &[&String], walked: &HashMap<String, SourceFile>)
        -> anyhow::Result<Vec<Option<(Vec<Symbol>, Vec<Chunk>)>>> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(touched.len().max(1));
        let mut out: Vec<Option<(Vec<Symbol>, Vec<Chunk>)>> = (0..touched.len()).map(|_| None).collect();
        let root = &self.paths.root;
        let settings = &self.settings;
        std::thread::scope(|scope| -> anyhow::Result<()> {
            let handles: Vec<_> = (0..threads).map(|w| scope.spawn(move || {
                let mut done = Vec::new();
                for i in (w..touched.len()).step_by(threads) {
                    let file = &walked[touched[i]];
                    let Ok(source) = crate::pyfmt::read_text(&root.join(&file.path)) else { continue };
                    let e = extract::extract(file, &source, settings)?;
                    done.push((i, (e.symbols, e.chunks)));
                }
                anyhow::Ok(done)
            })).collect();
            for h in handles {
                for (i, r) in h.join().expect("extract thread")? {
                    out[i] = Some(r);
                }
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn current_identity(&mut self) -> anyhow::Result<Identity> {
        let stored = self.store.identity()?;
        let configured = embeddings::space(&self.settings, 0);
        // The stored width answers `dims` only while the whole stored
        // vector space is still configured; otherwise the model is loaded.
        let no_embedder = self.settings.embed_backend == "none";
        let settled = stored.as_ref().is_some_and(|s| s.embed_model == configured.model
            && s.embed_backend == configured.backend && s.embed_variant == configured.variant
            && s.embed_doc_prefix == configured.doc_prefix && s.embed_pooling == configured.pooling
            && s.embed_dims > 0);
        let dims = match &stored {
            Some(s) if no_embedder || settled => s.embed_dims,
            _ => match self.embed()? {
                Some(e) => e.dims() as i64,
                None => stored.as_ref().map_or(0, |s| s.embed_dims),
            },
        };
        Ok(Identity {
            schema_rev: stored.as_ref().map(|s| s.schema_rev.clone()).unwrap_or_default(),
            embed_model: configured.model,
            embed_backend: configured.backend,
            embed_dims: dims,
            coverage: self.settings.coverage.clone(),
            extractor_rev: languages::extractor_rev(),
            categories_rev: categories_rev(&self.settings),
            embed_variant: configured.variant,
            embed_doc_prefix: configured.doc_prefix,
            embed_pooling: configured.pooling,
            chunking_rev: chunking_rev(&self.settings),
        })
    }

    /// Empty the index when forced, or when anything that shaped it changed.
    fn reset_if_needed(&mut self, force: bool) -> anyhow::Result<()> {
        let current = self.current_identity()?;
        if force || self.store.needs_reindex(&current)? {
            self.store.reset(&current)?;
        }
        Ok(())
    }

    fn embed_pending(&mut self) -> anyhow::Result<()> {
        // Checked before building the encoder, which loads a model.
        if self.settings.embed_backend == "none" || self.store.items().missing_vectors()?.is_empty() {
            return Ok(());
        }
        let Some(dims) = self.embed()?.map(|e| e.dims()) else { return Ok(()) };
        let encoder = self.encoder.as_ref().unwrap().as_ref().unwrap();
        self.store.embed_missing(encoder)?;
        self.store.set_embed_dims(dims as i64)
    }

    /// A refresh before a read under `refresh_mode = "auto"`, at most every
    /// `rescan_after_seconds`. On a held lock it serves what is there,
    /// recording nothing, since a write would wait on the same lock. An
    /// index never built fails instead of answering empty.
    fn maybe_refresh(&mut self) -> anyhow::Result<()> {
        if self.settings.refresh_mode != "auto" {
            return Ok(());
        }
        let last = self.store.last_scan_at()?.unwrap_or(0.0);
        let never_built = last == 0.0;
        if !never_built && self.settings.rescan_after_seconds > 0
            && crate::semsift::items::now() - last < self.settings.rescan_after_seconds as f64 {
            return Ok(());
        }
        if let Err(e) = self.refresh(false) {
            if never_built || !is_busy(&e) {
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn definitions(&mut self, name: &str, lang: &[String]) -> anyhow::Result<Vec<Symbol>> {
        self.maybe_refresh()?;
        self.store.named(name, "def", lang)
    }

    /// Every reference, unbounded, in path then line order.
    pub fn references(&mut self, name: &str, lang: &[String]) -> anyhow::Result<Vec<Symbol>> {
        self.maybe_refresh()?;
        self.store.named(name, "ref", lang)
    }

    pub fn symbols(&mut self, pattern: Option<&str>, tag: Option<&str>, scope: &Scope,
                   limit: Option<i64>) -> anyhow::Result<Vec<Symbol>> {
        self.maybe_refresh()?;
        self.store.symbol_rows(pattern, tag, scope, limit)
    }

    pub fn symbol_counts(&mut self, by: &str, pattern: Option<&str>, tag: Option<&str>, scope: &Scope,
                         limit: Option<i64>) -> anyhow::Result<(Vec<(String, i64)>, i64, i64)> {
        self.maybe_refresh()?;
        self.store.symbol_counts(by, pattern, tag, scope, limit)
    }

    /// Refuse a category the index does not hold, after checking spelling.
    fn require_indexed(&self, content: &[String]) -> anyhow::Result<()> {
        normalise_content(content, &self.settings).map_err(BadValue)?;
        let mut missing: Vec<&String> = content.iter().filter(|c| *c != "all")
            .filter(|c| self.settings.index_excluded.contains(c)).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            let names = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            return Err(NotIndexed(format!(
                "content={names} is not in this index: index_excluded = {}. To include it, drop {names} \
                 from index_excluded in repoglass.toml and reindex with refresh(force=True).",
                self.settings.index_excluded.join(", "))).into());
        }
        Ok(())
    }

    fn tiers(&mut self, query: &str, scope: &Scope) -> anyhow::Result<Vec<RankedList>> {
        let mut lists = Vec::new();
        let depth = self.settings.candidate_depth.max(0) as usize;
        let exact: Vec<(i64, f64)> = self.store.chunked_definitions(query, scope)?.into_iter().map(|id| (id, 1.0)).collect();
        if !exact.is_empty() {
            lists.push(fuse::ranked("exact", &exact, false, true));
        }
        let kw = lexical::keyword(&self.store, query, depth, scope)?;
        if !kw.is_empty() {
            lists.push(fuse::ranked("lexical", &kw, true, false));
        }
        if self.embed()?.is_some() {
            let encoder = self.encoder.as_ref().unwrap().as_ref().unwrap();
            match self.store.vector_search(encoder, query, depth, scope) {
                Ok(vec) => {
                    for w in &vec.warnings {
                        eprintln!("{w}");
                    }
                    if !vec.items.is_empty() {
                        lists.push(vec);
                    }
                }
                Err(e) if e.downcast_ref::<StaleVectors>().is_some() => {
                    eprintln!("{e}; run `rpg index --force`");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(lists)
    }

    /// Stand-in header for a chunk that holds no definition.
    fn opening_line(text: &str) -> Option<String> {
        py_splitlines(text).into_iter().map(py_strip).find(|l| !l.is_empty())
            .map(|l| char_prefix(l, extract::MAX_SIGNATURE_CHARS).to_string())
    }

    fn to_hits(&self, scored: &[(i64, f64)], limit: usize,
               evidence: &HashMap<i64, Vec<(String, f64)>>) -> anyhow::Result<Vec<Hit>> {
        let top: Vec<(i64, f64)> = scored.iter().take(limit).copied().collect();
        let rows = self.store.hits(&top.iter().map(|p| p.0).collect::<Vec<_>>())?;
        Ok(top.into_iter().filter_map(|(id, score)| {
            let (path, start, end, name, text, signature) = rows.get(&id)?.clone();
            let signature = signature.filter(|s| !s.is_empty()).or_else(|| Self::opening_line(&text));
            Some(Hit { path, start_line: start, end_line: end, name, score, code: text, signature,
                       tiers: evidence.get(&id).cloned().unwrap_or_default() })
        }).collect())
    }

    /// Exact, then keyword, then vector tiers, fused and reranked.
    /// `scope.content` empty takes the `content` setting.
    pub fn search(&mut self, query: &str, k: usize, scope: &Scope) -> anyhow::Result<Vec<Hit>> {
        self.maybe_refresh()?;
        let mut scope = scope.clone();
        if scope.content.is_empty() {
            scope.content = self.settings.content.clone();
        }
        self.require_indexed(&scope.content)?;
        let lists = self.tiers(query, &scope)?;
        if lists.is_empty() {
            return Ok(vec![]);
        }
        let is_prose = looks_like_prose(query, self.settings.prose_min_words);
        let mut evidence: HashMap<i64, Vec<(String, f64)>> = HashMap::new();
        for rl in &lists {
            for s in &rl.items {
                evidence.entry(s.id).or_default().push((rl.source.clone(), s.raw));
            }
        }
        let scored = fuse::merge(&lists, &self.settings, is_prose);
        if !self.settings.rerank {
            return self.to_hits(&scored, k, &evidence);
        }
        let rows = self.store.chunk_rows(&scored.iter().map(|p| p.0).collect::<Vec<_>>())?;
        let cands: Vec<Candidate> = scored.iter().filter_map(|&(id, score)| {
            let (path, text) = rows.get(&id)?.clone();
            Some(Candidate { id, path, text, score })
        }).collect();
        // Path priors are about code layout: applied when the caller asked
        // for code, or for nothing in particular, which returns tests and
        // docs without asking for them.
        let legacy = Settings { content_excluded: vec!["config".into(), "data".into()], ..Settings::default() };
        let penalise = scope.content.is_empty()
            || normalise_content(&scope.content, &legacy).map_err(BadValue)? == ["code"];
        let store = &self.store;
        let mut loader = |names: &HashSet<String>| -> anyhow::Result<Vec<Candidate>> {
            Ok(store.non_candidate_rows(names, &scope)?.into_iter()
                .map(|(id, path, text)| Candidate { id, path, text, score: 0.0 }).collect())
        };
        let ranked = rank::rerank(cands, query, &self.settings, Some(&mut loader), penalise, k)?;
        let top = ranked.iter().map(|c| c.score).fold(0.0f64, f64::max);
        let top = if top == 0.0 { 1.0 } else { top };
        let rescaled: Vec<(i64, f64)> = ranked.iter().map(|c| (c.id, c.score / top)).collect();
        self.to_hits(&rescaled, k, &evidence)
    }
}
