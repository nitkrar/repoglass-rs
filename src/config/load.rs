//! Settings resolution through files and the environment.

use super::schema::{Field, Kind, Num, Settings, Value, ENV_PREFIX, FIELDS};
use crate::pyfmt::str_repr;
use std::collections::BTreeMap;
use std::path::Path;

/// Settings that must never appear in the repository config, which is
/// meant to be committed.
pub const SECRET: &[&str] = &["embed_api_key"];

/// A config that cannot be loaded; exit 2 when named on the command line.
#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Grouped tables and flat keys mean the same thing.
fn flatten(table: &toml::Table, out: &mut BTreeMap<String, toml::Value>, order: &mut Vec<String>) {
    for (key, value) in table {
        if let toml::Value::Table(inner) = value {
            flatten(inner, out, order);
        } else {
            if !out.contains_key(key) {
                order.push(key.clone());
            }
            out.insert(key.clone(), value.clone());
        }
    }
}

fn read(path: Option<&Path>) -> Result<BTreeMap<String, toml::Value>, ConfigError> {
    let mut out = BTreeMap::new();
    let Some(path) = path.filter(|p| p.is_file()) else { return Ok(out) };
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError(e.to_string()))?;
    let table: toml::Table = text.parse().map_err(|e: toml::de::Error| ConfigError(e.message().to_string()))?;
    flatten(&table, &mut out, &mut Vec::new());
    Ok(out)
}

fn type_error(field: &Field, got: &str) -> ConfigError {
    ConfigError(format!("setting {} expects {}; got {got}", field.name, kind_name(field.kind)))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Str | Kind::OptStr | Kind::Choice(_) => "a string",
        Kind::Int => "an integer",
        Kind::Float => "a number",
        Kind::Bool => "a boolean",
        Kind::List => "a list of strings",
    }
}

fn from_toml(field: &Field, value: &toml::Value) -> Result<Value, ConfigError> {
    let got = value.type_str();
    let v = match (field.kind, value) {
        (Kind::Str | Kind::Choice(_), toml::Value::String(s)) => Value::Str(s.clone()),
        (Kind::OptStr, toml::Value::String(s)) => Value::OptStr(Some(s.clone())),
        (Kind::Int, toml::Value::Integer(i)) => Value::Int(*i),
        (Kind::Float, toml::Value::Integer(i)) => Value::Float(Num::Int(*i)),
        (Kind::Float, toml::Value::Float(f)) => Value::Float(Num::Float(*f)),
        (Kind::Bool, toml::Value::Boolean(b)) => Value::Bool(*b),
        (Kind::List, toml::Value::Array(items)) => Value::List(items.iter().map(|x| match x {
            toml::Value::String(s) => Ok(s.clone()),
            other => Err(type_error(field, other.type_str())),
        }).collect::<Result<_, _>>()?),
        _ => return Err(type_error(field, got)),
    };
    check_choice(field, &v)?;
    Ok(v)
}

/// An environment value parsed as the field's type; lists are
/// comma-separated.
fn from_env(field: &Field, raw: &str) -> Result<Value, ConfigError> {
    let kind = match field.kind { Kind::Bool => "bool", Kind::Int => "int", _ => "float" };
    let bad = || ConfigError(format!("{ENV_PREFIX}{} expects {kind}; got {}",
                                     field.name.to_uppercase(), str_repr(raw)));
    let v = match field.kind {
        Kind::Str | Kind::Choice(_) => Value::Str(raw.to_string()),
        Kind::OptStr => Value::OptStr(Some(raw.to_string())),
        Kind::Int => Value::Int(raw.trim().parse().map_err(|_| bad())?),
        Kind::Float => Value::Float(Num::Float(raw.trim().parse().map_err(|_| bad())?)),
        Kind::Bool => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Value::Bool(true),
            "0" | "false" | "no" | "off" | "" => Value::Bool(false),
            _ => return Err(bad()),
        },
        Kind::List => Value::List(raw.split(',').map(str::trim).filter(|s| !s.is_empty())
                                  .map(String::from).collect()),
    };
    check_choice(field, &v)?;
    Ok(v)
}

fn check_choice(field: &Field, value: &Value) -> Result<(), ConfigError> {
    if let (Kind::Choice(allowed), Value::Str(s)) = (field.kind, value) {
        if !allowed.contains(&s.as_str()) {
            let quoted: Vec<String> = allowed.iter().map(|a| format!("'{a}'")).collect();
            return Err(ConfigError(format!("unknown {} {}; expected {}", field.name, str_repr(s),
                                           quoted.join(", "))));
        }
    }
    Ok(())
}

/// Resolve Settings from every source, later ones winning: the user
/// config, the repository config, then `REPOGLASS_*` environment
/// variables. An unknown key is an error, and a secret in the repository
/// config is refused.
pub fn load(user_config: Option<&Path>, repo_config: Option<&Path>) -> Result<Settings, ConfigError> {
    let mut merged: BTreeMap<String, toml::Value> = read(user_config)?;
    let from_repo = read(repo_config)?;
    let leaked: Vec<&str> = SECRET.iter().copied().filter(|s| from_repo.contains_key(*s)).collect();
    if let Some(first) = leaked.first() {
        return Err(ConfigError(format!(
            "{} must not be set in the repository config ({}): that file is meant to be \
             committed. Use the {ENV_PREFIX}{} environment variable, or the user-level config.",
            leaked.join(", "), repo_config.unwrap().display(), first.to_uppercase())));
    }
    merged.extend(from_repo);

    let unknown: Vec<&String> = merged.keys().filter(|k| !FIELDS.iter().any(|f| f.name == *k)).collect();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|s| s.as_str()).collect();
        return Err(ConfigError(format!("unknown setting(s): {}", names.join(", "))));
    }

    let mut settings = Settings::default();
    for field in FIELDS {
        let env = std::env::var(format!("{ENV_PREFIX}{}", field.name.to_uppercase())).ok();
        let value = match (env, merged.get(field.name)) {
            (Some(raw), _) => from_env(field, &raw)?,
            (None, Some(v)) => from_toml(field, v)?,
            (None, None) => continue,
        };
        settings.set(field.name, value);
    }
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn later_sources_win_and_tables_flatten() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(dir.path(), "user.toml", "ranker = \"none\"\nwindow_chars = 500\n");
        let repo = write(dir.path(), "repo.toml", "[extraction]\nwindow_chars = 900\n");
        let s = load(Some(&user), Some(&repo)).unwrap();
        assert_eq!(s.ranker, "none");
        assert_eq!(s.window_chars, 900);
    }

    #[test]
    fn unknown_keys_and_repo_secrets_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let typo = write(dir.path(), "a.toml", "rerankk = true\n");
        assert!(load(None, Some(&typo)).unwrap_err().0.contains("unknown setting(s): rerankk"));
        let secret = write(dir.path(), "b.toml", "embed_api_key = \"x\"\n");
        assert!(load(None, Some(&secret)).unwrap_err().0.contains("must not be set"));
        assert!(load(Some(&secret), None).is_ok());
    }

    #[test]
    fn whole_numbers_stay_whole_for_float_settings() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(dir.path(), "c.toml", "stem_boost = 2\n");
        assert_eq!(load(None, Some(&p)).unwrap().stem_boost, Num::Int(2));
    }
}
