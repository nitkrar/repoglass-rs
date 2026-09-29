//! Whole-file window chunking.
//!
//! A language with a tags query is chunked by walking its parse tree
//! (semsift's `LanguagePackChunker`): sibling nodes grouped up to
//! `window_chars`, larger nodes descended into, neighbours merged back
//! towards the target. Any other language groups whole lines. Every byte
//! is covered, and chunks are of comparable size.

use super::languages::Grammar;
use super::render::lexical_override;
use crate::config::{Settings, MAX_CHUNK_CHARS, MIN_CHUNK_CHARS};
use crate::models::Chunk;
use crate::pyfmt::{char_len, decode_ignore, py_strip};
use tree_sitter::{Node, Parser};

/// A node smaller than this is emitted whole rather than descended into.
const MIN_NODE_BYTES: usize = 50;
/// Recursion bound for pathological nesting.
const MAX_DEPTH: usize = 500;
/// semsift's `max_source_bytes`; over it the tree walk is skipped.
const MAX_SOURCE_BYTES: usize = 5_000_000;

pub fn window_chunks(path: &str, source: &str, grammar: Option<&Grammar>, settings: &Settings) -> Vec<Chunk> {
    if py_strip(source).is_empty() {
        return vec![];
    }
    let target = settings.window_chars.max(1) as usize;
    let data = source.as_bytes();
    let newlines: Vec<usize> = memchr::memchr_iter(b'\n', data).collect();
    let line_of = |offset: usize| newlines.partition_point(|&p| p < offset) as i64 + 1;
    if let Some(g) = grammar {
        if let Some(spans) = tree_spans(g, source, target) {
            let mut out = Vec::new();
            for (a, b) in spans {
                let body = &source[a..b];
                if char_len(py_strip(body)) < MIN_CHUNK_CHARS {
                    continue;
                }
                let last = char_start(data, b - 1).max(a);
                out.push(window(path, body.to_string(), line_of(a), line_of(last), settings));
            }
            return out;
        }
        return text_chunks(path, source, target, settings);
    }
    let mut out = Vec::new();
    for (s, e) in bounded_plain(line_spans(data, target)) {
        let body = decode_ignore(&data[s..e]);
        if char_len(py_strip(&body)) < MIN_CHUNK_CHARS {
            continue;
        }
        out.push(window(path, body, line_of(s), line_of((e - 1).max(s)), settings));
    }
    out
}

fn window(path: &str, body: String, first: i64, last: i64, settings: &Settings) -> Chunk {
    Chunk {
        name: String::new(),
        start_line: first,
        end_line: last,
        content_hash: super::extract::content_hash(&body),
        lexical_override: lexical_override(path, &body, settings),
        text: body,
    }
}

/// The tree walk's spans, at character boundaries and at most
/// `MAX_CHUNK_CHARS` bytes each; None where semsift falls back to text.
fn tree_spans(g: &Grammar, source: &str, target: usize) -> Option<Vec<(usize, usize)>> {
    let data = source.as_bytes();
    if data.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let mut parser = Parser::new();
    parser.set_language(&g.language).ok()?;
    let tree = parser.parse(data, None)?;
    let merged = merge_adjacent(split_node(tree.root_node(), target, 0), target);
    Some(bounded_chars(&merged, MAX_CHUNK_CHARS, data).into_iter()
        .map(|(a, b)| (char_start(data, a), char_start(data, b)))
        .filter(|(a, b)| b > a)
        .collect())
}

/// semsift's `TextChunker(markdown=False)` fallback, reached only when a
/// parse fails or a file exceeds `MAX_SOURCE_BYTES`.
fn text_chunks(path: &str, source: &str, max_chars: usize, settings: &Settings) -> Vec<Chunk> {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let byte_at = |ci: usize| chars.get(ci).map_or(source.len(), |c| c.0);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let (mut start, mut pos) = (0usize, 0usize);
    let mut after_blank = false;
    let lines = crate::pyfmt::py_splitlines_keepends(source);
    for line in lines {
        let len = char_len(line);
        let size = pos - start;
        let blank = py_strip(line).is_empty();
        if size > 0 && ((after_blank && !blank && size >= max_chars / 2) || size + len > max_chars) {
            spans.push((start, pos));
            start = pos;
        }
        if len > max_chars {
            let mut cut = pos;
            while cut < pos + len {
                spans.push((cut, (cut + max_chars).min(pos + len)));
                cut += max_chars;
            }
            start = pos + len;
        }
        pos += len;
        after_blank = blank;
    }
    if pos > start {
        spans.push((start, pos));
    }
    let data = source.as_bytes();
    let newlines: Vec<usize> = memchr::memchr_iter(b'\n', data).collect();
    let line_of = |offset: usize| newlines.partition_point(|&p| p < offset) as i64 + 1;
    let mut out = Vec::new();
    for (a, b) in spans {
        let (ba, bb) = (byte_at(a), byte_at(b));
        let body = &source[ba..bb];
        if char_len(py_strip(body)) < MIN_CHUNK_CHARS {
            continue;
        }
        let last = if bb > ba { char_start(data, bb - 1).max(ba) } else { ba };
        out.push(window(path, body.to_string(), line_of(ba), line_of(last), settings));
    }
    out
}

fn split_node(node: Node, target: usize, depth: usize) -> Vec<(usize, usize)> {
    if node.child_count() == 0 || depth > MAX_DEPTH
        || node.end_byte() - node.start_byte() < MIN_NODE_BYTES {
        return vec![(node.start_byte(), node.end_byte())];
    }
    let mut cur = node.walk();
    let children: Vec<Node> = node.children(&mut cur).collect();
    let mut groups = Vec::new();
    let mut i = 0;
    while i < children.len() {
        let (start, mut end) = (children[i].start_byte(), children[i].end_byte());
        let mut size = end - start;
        i += 1;
        if size > target {
            groups.extend(split_node(children[i - 1], target, depth + 1));
            continue;
        }
        while i < children.len() {
            let nxt = children[i];
            if size + (nxt.end_byte() - nxt.start_byte()) > target {
                break;
            }
            end = nxt.end_byte();
            size += nxt.end_byte() - nxt.start_byte();
            i += 1;
        }
        groups.push((start, end));
    }
    groups
}

fn merge_adjacent(spans: Vec<(usize, usize)>, target: usize) -> Vec<(usize, usize)> {
    let Some(&(mut start, mut end)) = spans.first() else { return spans };
    let mut out = Vec::new();
    for &(ns, ne) in &spans[1..] {
        if (end - start) + (ne - ns) > target {
            out.push((start, end));
            start = ns;
            end = ne;
            continue;
        }
        end = ne;
    }
    out.push((start, end));
    out
}

/// The start of the UTF-8 character holding byte `offset`.
fn char_start(data: &[u8], offset: usize) -> usize {
    let mut o = offset.min(data.len());
    while 0 < o && o < data.len() && data[o] & 0xC0 == 0x80 {
        o -= 1;
    }
    o
}

fn bounded_chars(spans: &[(usize, usize)], limit: usize, data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for &(mut start, end) in spans {
        while end - start > limit {
            let cut = char_start(data, start + limit);
            out.push((start, cut));
            start = cut;
        }
        out.push((start, end));
    }
    out
}

/// Cut any span over `MAX_CHUNK_CHARS` bytes into pieces; the caller
/// decodes with invalid bytes dropped, so a mid-character cut loses it.
fn bounded_plain(spans: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (mut start, end) in spans {
        while end - start > MAX_CHUNK_CHARS {
            out.push((start, start + MAX_CHUNK_CHARS));
            start += MAX_CHUNK_CHARS;
        }
        out.push((start, end));
    }
    out
}

/// Whole lines grouped until a group reaches `target` bytes.
fn line_spans(data: &[u8], target: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for i in memchr::memchr_iter(b'\n', data) {
        if i + 1 - start >= target {
            spans.push((start, i + 1));
            start = i + 1;
        }
    }
    if start < data.len() {
        spans.push((start, data.len()));
    }
    spans
}
