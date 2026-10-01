//! Checkouts of remote repositories, fetched once and kept.
//!
//! `--git URL --rev REV` names a directory under `<home>/git/`: one
//! commit's files, fetched at depth 1. A checkout that exists is used as
//! it is, so indexes built with different settings share one download. A
//! branch or tag is resolved when first fetched; delete the checkout to
//! fetch it again.

use crate::text::blake2b_hex;
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest a single git command may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(600);

/// Where the checkout of `url` at `rev` lives: the repository's name, for
/// people, and a hash of both, to keep revisions apart.
pub fn checkout_dir(home: &Path, url: &str, rev: Option<&str>) -> PathBuf {
    let name = url.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or("")
        .trim_end_matches(".git");
    let safe: String = name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let key = format!("{url}@{}", rev.unwrap_or("HEAD"));
    home.join("git").join(format!("{}-{}", if safe.is_empty() { "repo" } else { &safe },
                                  blake2b_hex(key.as_bytes(), 4)))
}

/// The checkout, fetched first if it is not there.
pub fn checkout(home: &Path, url: &str, rev: Option<&str>) -> anyhow::Result<PathBuf> {
    let dir = checkout_dir(home, url, rev);
    if dir.is_dir() {
        return Ok(dir);
    }
    let parent = dir.parent().expect("checkout_dir has a parent");
    std::fs::create_dir_all(parent)?;
    // Fetched beside the destination and renamed into place, so a
    // checkout that exists is always complete.
    let tmp = parent.join(format!(".fetch-{}-{}", dir.file_name().unwrap().to_string_lossy(),
                                  std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let fetched = fetch(&tmp, url, rev);
    if let Err(e) = fetched {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if std::fs::rename(&tmp, &dir).is_err() {
        // Another process finished the same checkout first.
        let _ = std::fs::remove_dir_all(&tmp);
        if !dir.is_dir() {
            bail!("could not move the checkout into {}", dir.display());
        }
    }
    Ok(dir)
}

fn fetch(dir: &Path, url: &str, rev: Option<&str>) -> anyhow::Result<()> {
    let wanted = rev.unwrap_or("HEAD");
    eprintln!("repoglass: fetching {url} at {wanted}");
    git(dir, &["init", "-q"])?;
    // `--` keeps a URL starting with '-' from being read as an option.
    git(dir, &["fetch", "-q", "--depth", "1", "--", url, wanted])
        .with_context(|| format!("could not fetch {url} at {wanted}"))?;
    git(dir, &["-c", "advice.detachedHead=false", "checkout", "-q", "FETCH_HEAD"])?;
    if let Some(rev) = rev.filter(|r| r.len() >= 7 && r.chars().all(|c| c.is_ascii_hexdigit())) {
        let head = git(dir, &["rev-parse", "HEAD"])?;
        if !head.starts_with(&rev.to_ascii_lowercase()) {
            bail!("{url}: fetched {head}, not {rev}");
        }
    }
    Ok(())
}

/// Runs git in `dir`, returning its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut child = Command::new("git").args(args).current_dir(dir)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().context("git is not installed or not on PATH")?;
    let start = Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git {} timed out after {}s", args[0], TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("git {} failed: {}", args[0], String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-commit repository to fetch from, and its commit.
    fn origin(dir: &Path) -> String {
        let run = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        run(&["init", "-q"]);
        // A file:// fetch of a bare commit id needs the server to allow it.
        run(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
        std::fs::write(dir.join("lib.py"), "def f():\n    return 1\n").unwrap();
        run(&["add", "lib.py"]);
        run(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "one"]);
        run(&["rev-parse", "HEAD"])
    }

    #[test]
    fn fetches_a_commit_once_and_reuses_it() {
        let src = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let sha = origin(src.path());
        let url = format!("file://{}", src.path().display());
        let dir = checkout(home.path(), &url, Some(&sha)).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("lib.py")).unwrap(), "def f():\n    return 1\n");
        // With the origin gone, only a kept checkout can answer.
        drop(src);
        assert_eq!(checkout(home.path(), &url, Some(&sha)).unwrap(), dir);
    }

    #[test]
    fn a_failed_fetch_leaves_nothing_behind() {
        let src = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        origin(src.path());
        let url = format!("file://{}", src.path().display());
        let missing = "0123456789abcdef0123456789abcdef01234567";
        assert!(checkout(home.path(), &url, Some(missing)).is_err());
        let left: Vec<_> = std::fs::read_dir(home.path().join("git")).unwrap().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn revisions_of_one_repository_get_their_own_checkouts() {
        let home = Path::new("/h");
        let a = checkout_dir(home, "https://github.com/aio-libs/aiohttp.git", Some("abc1234"));
        let b = checkout_dir(home, "https://github.com/aio-libs/aiohttp.git", Some("def5678"));
        assert_ne!(a, b);
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("aiohttp-"));
    }
}
