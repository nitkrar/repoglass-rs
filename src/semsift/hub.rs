//! Hugging Face Hub snapshots, read from and written to the cache
//! `huggingface_hub` uses, so Python and Rust share one download.

use std::io::Read;
use std::path::{Path, PathBuf};

fn hub_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("HF_HUB_CACHE") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HF_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        dirs::home_dir().unwrap_or_default().join(".cache/huggingface")
    });
    home.join("hub")
}

fn endpoint() -> String {
    std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".into()).trim_end_matches('/').into()
}

/// A local directory holding `files` for `model`: the path itself when it
/// exists, else the cached snapshot, else a fresh download into the cache.
pub fn resolve(model: &str, files: &[&str]) -> anyhow::Result<PathBuf> {
    if Path::new(model).exists() {
        return Ok(PathBuf::from(model));
    }
    let repo = hub_cache().join(format!("models--{}", model.replace('/', "--")));
    if let Ok(rev) = std::fs::read_to_string(repo.join("refs/main")) {
        let snap = repo.join("snapshots").join(rev.trim());
        if files.iter().all(|f| snap.join(f).exists()) {
            return Ok(snap);
        }
    }
    download(model, &repo, files)
}

fn download(model: &str, repo: &Path, files: &[&str]) -> anyhow::Result<PathBuf> {
    eprintln!("repoglass: downloading {model} (first run; this is not repeated)");
    let base = endpoint();
    let info: serde_json::Value = ureq::get(&format!("{base}/api/models/{model}/revision/main"))
        .call()?.body_mut().read_json()?;
    let sha = info["sha"].as_str().ok_or_else(|| anyhow::anyhow!("{model}: no revision on the hub"))?.to_string();
    let snap = repo.join("snapshots").join(&sha);
    std::fs::create_dir_all(&snap)?;
    for file in files {
        let dest = snap.join(file);
        if dest.exists() {
            continue;
        }
        let mut data = Vec::new();
        ureq::get(&format!("{base}/{model}/resolve/{sha}/{file}")).call()?
            .body_mut().with_config().limit(4 << 30).reader().read_to_end(&mut data)?;
        let tmp = snap.join(format!(".{file}.{}", std::process::id()));
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, &dest)?;
    }
    std::fs::create_dir_all(repo.join("refs"))?;
    std::fs::write(repo.join("refs/main"), &sha)?;
    Ok(snap)
}
