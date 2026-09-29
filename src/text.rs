//! Identifier and path normalisation, and the hashes that name stored rows.

use fancy_regex::Regex as FRegex;
use std::sync::OnceLock;

fn word_re() -> &'static FRegex {
    static RE: OnceLock<FRegex> = OnceLock::new();
    RE.get_or_init(|| FRegex::new(r"[A-Z]+(?![a-z])|[A-Z][a-z]+|[a-z]+|[0-9]+").unwrap())
}

/// Split camelCase, snake_case and paths into space-separated words.
pub fn humanise(text: &str) -> String {
    let t = text.replace('/', " ").replace('.', " ");
    let words: Vec<&str> = word_re().find_iter(&t).map(|m| m.unwrap().as_str()).collect();
    words.join(" ")
}

/// `PurePosixPath(p).name`.
pub fn path_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// `PurePosixPath(p).stem`.
pub fn path_stem(p: &str) -> &str {
    let name = path_name(p);
    match name.rfind('.') {
        Some(i) if 0 < i && i < name.len() - 1 => &name[..i],
        _ => name,
    }
}

/// `PurePosixPath(p).parent.name`.
pub fn parent_name(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => path_name(&p[..i]),
        None => "",
    }
}

/// `PurePosixPath(p).parent.parts`, without `.` and `/`.
pub fn parent_parts(p: &str) -> Vec<&str> {
    match p.rfind('/') {
        Some(i) => p[..i].split('/').filter(|s| !s.is_empty() && *s != ".").collect(),
        None => Vec::new(),
    }
}

/// `hashlib.blake2b(data, digest_size=size).hexdigest()` for the sizes used.
pub fn blake2b_hex(data: &[u8], size: usize) -> String {
    use blake2::digest::consts::{U16, U4, U8};
    use blake2::{Blake2b, Digest};
    match size {
        4 => hex::encode(Blake2b::<U4>::digest(data)),
        8 => hex::encode(Blake2b::<U8>::digest(data)),
        16 => hex::encode(Blake2b::<U16>::digest(data)),
        _ => unreachable!("digest size {size}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanise_splits_like_python() {
        assert_eq!(humanise("src/repoglass/HTTPServer_v2.py"), "src repoglass HTTP Server v 2 py");
    }

    #[test]
    fn stem_keeps_dotfiles_whole() {
        assert_eq!(path_stem("a/.bashrc"), ".bashrc");
        assert_eq!(path_stem("a/b.tar.gz"), "b.tar");
        assert_eq!(parent_name("a/b/c.py"), "b");
        assert_eq!(parent_name("c.py"), "");
    }
}
