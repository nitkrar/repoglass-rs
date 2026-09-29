//! All path derivation.

use super::schema::{DATA_DIR_NAME, HOME_ENV, IGNORE_FILE_NAME};
use crate::text::blake2b_hex;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// The directory being indexed.
    pub root: PathBuf,
    /// User-level; default `~/.repoglass`.
    pub home: PathBuf,
    pub data_dir: Option<String>,
}

/// `Path.resolve()`: symlinks resolved, and no error for a missing path.
pub fn resolve(path: &Path) -> PathBuf {
    let expanded = expand_user(path);
    std::fs::canonicalize(&expanded).unwrap_or_else(|_| {
        std::path::absolute(&expanded).unwrap_or(expanded)
    })
}

/// `Path.expanduser()`.
pub fn expand_user(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" || text.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(text.trim_start_matches('~').trim_start_matches('/'));
        }
    }
    path.to_path_buf()
}

impl Paths {
    /// Both roots resolved, honouring `REPOGLASS_HOME`.
    pub fn for_root(root: &Path) -> Paths {
        let home = match std::env::var(HOME_ENV) {
            Ok(env) if !env.is_empty() => expand_user(Path::new(&env)),
            _ => dirs::home_dir().unwrap_or_default().join(DATA_DIR_NAME),
        };
        Paths { root: resolve(root), home, data_dir: None }
    }

    /// Where this repository's index lives: central by default, keyed by
    /// the resolved root. `data_dir` overrides; a relative one resolves
    /// against the root.
    pub fn data(&self) -> PathBuf {
        match &self.data_dir {
            None => self.home.join("index").join(index_key(&self.root)),
            Some(dir) => {
                let p = expand_user(Path::new(dir));
                if p.is_absolute() { p } else { self.root.join(p) }
            }
        }
    }

    pub fn db(&self) -> PathBuf {
        self.data().join("index.db")
    }

    pub fn repo_ignore(&self) -> PathBuf {
        self.root.join(IGNORE_FILE_NAME)
    }

    pub fn repo_config(&self) -> PathBuf {
        self.root.join("repoglass.toml")
    }

    pub fn user_config(&self) -> PathBuf {
        self.home.join("repoglass.toml")
    }
}

/// A stable directory name for one repository: its basename, for people,
/// and a hash of the resolved path, to tell same-named projects apart.
pub fn index_key(root: &Path) -> String {
    let resolved = resolve(root);
    let text = resolved.to_string_lossy();
    let digest = blake2b_hex(text.as_bytes(), 4);
    let name = resolved.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let safe: String = name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    format!("{}-{digest}", if safe.is_empty() { "repo" } else { &safe })
}
