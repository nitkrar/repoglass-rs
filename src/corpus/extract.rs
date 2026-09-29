//! Parse a file into symbols, spans and chunks.
//!
//! One path for every language: markdown is a language whose tags query
//! captures sections rather than functions.

use super::languages::{self, Grammar};
use super::render::{embed_text, lexical_override};
use super::windows::window_chunks;
use crate::config::{Settings, MAX_CHUNK_CHARS, MIN_CHUNK_CHARS};
use crate::models::{Chunk, SourceFile, Symbol};
use crate::pyfmt::{char_len, char_prefix, decode_ignore, py_splitlines, py_strip};
use crate::text::blake2b_hex;
use std::collections::{HashMap, HashSet};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, QueryCursor};

/// A signature longer than this is not a signature.
pub const MAX_SIGNATURE_CHARS: usize = 500;

pub struct Extraction {
    pub symbols: Vec<Symbol>,
    pub chunks: Vec<Chunk>,
}

pub fn content_hash(source: &str) -> String {
    blake2b_hex(source.as_bytes(), 16)
}

/// (start_byte, end_byte, start_line, end_line) for a definition. A
/// markdown `section` ends where its first nested section begins, so a
/// heading's chunk does not repeat everything beneath it.
type Span = (usize, usize, i64, i64);

fn span_of(node: Node) -> Span {
    let (mut end, mut end_line) = (node.end_byte(), node.end_position().row as i64 + 1);
    if node.kind() == "section" {
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if child.kind() == "section" {
                end = child.start_byte();
                end_line = child.start_position().row as i64;
                break;
            }
        }
    }
    (node.start_byte(), end, node.start_position().row as i64 + 1, end_line)
}

/// A definition's header: everything before its `body` field, or its
/// first line when it has no body.
fn signature(node: Option<&Node>, raw: &[u8]) -> Option<String> {
    let node = node?;
    let body = node.child_by_field_name("body");
    let end = body.map_or(node.end_byte(), |b| b.start_byte());
    let decoded = decode_ignore(&raw[node.start_byte()..end]);
    let mut text = py_strip(&decoded).to_string();
    if body.is_none() {
        text = match py_splitlines(&text).first() {
            Some(first) => py_strip(first).to_string(),
            None => String::new(),
        };
    }
    let cut = char_prefix(&text, MAX_SIGNATURE_CHARS);
    if cut.is_empty() { None } else { Some(cut.to_string()) }
}

/// `"\n".join(lines[from:to])` with Python's slice clamping.
fn slice_join(lines: &[&str], from: i64, to: i64) -> String {
    let n = lines.len() as i64;
    let a = from.clamp(0, n);
    let b = to.clamp(0, n).max(a);
    lines[a as usize..b as usize].join("\n")
}

fn node_text(raw: &[u8], node: &Node) -> String {
    String::from_utf8_lossy(&raw[node.start_byte()..node.end_byte()]).into_owned()
}

/// A definition the tags query found, before it becomes a chunk.
struct Def {
    name: String,
    node_kind: String,
    start_line: i64,
    /// Capped at `max_chunk_lines`.
    end_line: i64,
}

pub fn extract(file: &SourceFile, source: &str, settings: &Settings) -> anyhow::Result<Extraction> {
    let grammar = languages::grammar(&file.lang)?;
    let raw = source.as_bytes();
    // Capture name -> nodes. Names are sorted and each name's nodes put in
    // position order below, so every build of a file yields the same rows.
    let mut keys: Vec<String> = Vec::new();
    let mut captures: HashMap<String, Vec<Node>> = HashMap::new();
    let tree;
    if let Some(g) = &grammar {
        let mut parser = Parser::new();
        parser.set_language(&g.language)?;
        tree = parser.parse(raw, None).ok_or_else(|| anyhow::anyhow!("parse failed: {}", file.path))?;
        let names = g.query.capture_names();
        let mut cursor = QueryCursor::new();
        let mut it = cursor.matches(&g.query, tree.root_node(), raw);
        while let Some(m) = it.next() {
            for cap in m.captures {
                let name = names[cap.index as usize];
                captures.entry(name.to_string()).or_insert_with(|| {
                    keys.push(name.to_string());
                    Vec::new()
                }).push(cap.node);
            }
        }
    }
    keys.sort();
    for nodes in captures.values_mut() {
        nodes.sort_by(|a, b| (a.start_byte(), a.end_byte(), a.kind()).cmp(&(b.start_byte(), b.end_byte(), b.kind())));
    }

    // Smallest first, so the first containing span is the innermost.
    let mut spans: Vec<Span> = Vec::new();
    let mut def_nodes: HashMap<(usize, usize), Node> = HashMap::new();
    for key in keys.iter().filter(|k| k.starts_with("definition.")) {
        for n in &captures[key] {
            let s = span_of(*n);
            spans.push(s);
            def_nodes.insert((s.0, s.1), *n);
        }
    }
    spans.sort_by_key(|s| s.1 - s.0);
    let innermost = |start: usize, end: usize| spans.iter().find(|s| s.0 <= start && end <= s.1).copied();

    let lines = py_splitlines(source);
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut defs: Vec<Def> = Vec::new();
    let mut by_span: HashMap<(usize, usize), String> = HashMap::new();
    // A node naming a definition is not also a reference to it.
    let mut named_here: HashSet<(usize, usize)> = HashSet::new();
    let ignored: HashSet<(usize, usize)> = captures.get("ignore")
        .map(|v| v.iter().map(|n| (n.start_byte(), n.end_byte())).collect()).unwrap_or_default();

    for key in keys.iter().filter(|k| k.starts_with("name.definition.")) {
        let node_kind = key.rsplit('.').next().unwrap_or("").to_string();
        for n in &captures[key] {
            let r = (n.start_byte(), n.end_byte());
            if named_here.contains(&r) {
                continue;
            }
            let Some(span) = innermost(r.0, r.1) else { continue };
            let (start, end) = (span.2, span.3);
            let name = node_text(raw, n);
            symbols.push(Symbol {
                name: name.clone(), tag: "def", path: file.path.clone(),
                start_line: start, end_line: end, lang: file.lang.clone(),
                signature: signature(def_nodes.get(&(span.0, span.1)), raw),
                content_type: file.content_type.clone(), enclosing: None,
            });
            by_span.insert((span.0, span.1), name.clone());
            named_here.insert(r);
            // The chunk's span is capped; the symbol's is not.
            let chunk_end = end.min(start - 1 + settings.max_chunk_lines);
            let len = char_len(&slice_join(&lines, start - 1, chunk_end));
            if (MIN_CHUNK_CHARS..=MAX_CHUNK_CHARS).contains(&len) {
                defs.push(Def { name, node_kind: node_kind.clone(), start_line: start, end_line: chunk_end });
            }
        }
    }

    for key in keys.iter().filter(|k| k.starts_with("name.reference.")) {
        for n in &captures[key] {
            let r = (n.start_byte(), n.end_byte());
            if named_here.contains(&r) || ignored.contains(&r) {
                continue;
            }
            let owner = innermost(r.0, r.1).and_then(|s| by_span.get(&(s.0, s.1)).cloned());
            symbols.push(Symbol {
                name: node_text(raw, n), tag: "ref", path: file.path.clone(),
                start_line: n.start_position().row as i64 + 1,
                end_line: n.end_position().row as i64 + 1,
                lang: file.lang.clone(), signature: None,
                content_type: file.content_type.clone(), enclosing: owner,
            });
        }
    }

    let chunks = match settings.coverage.as_str() {
        "definition" => definition_chunks(file, &lines, &defs, settings),
        _ => hybrid_chunks(file, source, &lines, &defs, grammar.as_deref(), settings),
    };
    Ok(Extraction { symbols, chunks })
}

fn definition_chunks(file: &SourceFile, lines: &[&str], defs: &[Def], settings: &Settings) -> Vec<Chunk> {
    defs.iter().map(|d| {
        let body = slice_join(lines, d.start_line - 1, d.end_line);
        let comments = leading_comments(lines, d.start_line);
        Chunk {
            name: d.name.clone(),
            start_line: d.start_line,
            end_line: d.end_line,
            text: embed_text(&file.path, &file.lang, &d.node_kind, &comments, &body, settings),
            lexical_override: lexical_override(&file.path, &body, settings),
            content_hash: content_hash(&body),
        }
    }).collect()
}

/// Definition chunks, plus windows over the lines none of them owns. A
/// window at least half covered by definitions is dropped.
fn hybrid_chunks(file: &SourceFile, source: &str, lines: &[&str], defs: &[Def],
                 grammar: Option<&Grammar>, settings: &Settings) -> Vec<Chunk> {
    let mut out = definition_chunks(file, lines, defs, settings);
    let covered: HashSet<i64> = out.iter().flat_map(|c| c.start_line..=c.end_line).collect();
    for w in window_chunks(&file.path, source, grammar, settings) {
        let n = w.end_line + 1 - w.start_line;
        let uncovered = (w.start_line..=w.end_line).filter(|l| !covered.contains(l)).count() as i64;
        if uncovered * 2 >= n {
            out.push(w);
        }
    }
    out
}

const COMMENT_PREFIXES: &[&str] = &["#", "//", "*", "/*", "///", ";", "--"];

/// Comment lines among the six above a definition. Read only by
/// `embed_text`, and only under `distill_docs` for a doc language.
fn leading_comments(lines: &[&str], start_line: i64) -> Vec<String> {
    let from = (start_line - 1 - 6).max(0);
    let mut out = Vec::new();
    for i in from..(start_line - 1) {
        let Some(line) = lines.get(i as usize) else { continue };
        let stripped = py_strip(line);
        if COMMENT_PREFIXES.iter().any(|p| stripped.starts_with(p)) {
            let trimmed = stripped.trim_start_matches(['/', '*', '#', ';', '-', ' ']);
            out.push(py_strip(trimmed).to_string());
        }
    }
    out
}
