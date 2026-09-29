//! Embeds the tag queries and the DDL from `data/` into the binary.

use std::path::Path;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let data = Path::new(&manifest).join("data");
    let queries = data.join("queries");
    let sql = data.join("sql/index.sql");
    println!("cargo:rerun-if-changed={}", queries.display());
    println!("cargo:rerun-if-changed={}", sql.display());

    let mut names: Vec<String> = std::fs::read_dir(&queries)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".scm"))
        .collect();
    names.sort();
    let mut out = String::from("pub static QUERY_FILES: &[(&str, &str)] = &[\n");
    for name in &names {
        let path = queries.join(name).canonicalize().unwrap();
        out.push_str(&format!("    ({name:?}, include_str!({:?})),\n", path.display().to_string()));
    }
    out.push_str("];\n");
    out.push_str(&format!("pub static SCHEMA_SQL: &str = include_str!({:?});\n",
                          sql.canonicalize().unwrap().display().to_string()));
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("embedded.rs");
    std::fs::write(dest, out).unwrap();
}
