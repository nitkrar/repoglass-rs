//! Filters over the item store's declared text fields, compiled to SQL
//! with every value bound as a parameter.

#[derive(Clone, Debug)]
pub enum Filter {
    In(&'static str, Vec<String>),
    Glob(&'static str, String),
    Not(Box<Filter>),
    Or(Vec<Filter>),
    And(Vec<Filter>),
}

pub struct Compiled {
    pub sql: String,
    pub params: Vec<String>,
}

/// SQL over alias `i`; `None` matches everything.
pub fn compile(filter: Option<&Filter>) -> Compiled {
    let mut params = Vec::new();
    let sql = match filter {
        None => "1".to_string(),
        Some(f) => walk(f, &mut params),
    };
    Compiled { sql, params }
}

fn walk(f: &Filter, params: &mut Vec<String>) -> String {
    match f {
        Filter::In(field, values) => {
            params.extend(values.iter().cloned());
            format!("i.\"{field}\" IN ({})", vec!["?"; values.len()].join(", "))
        }
        Filter::Glob(field, pattern) => {
            params.push(pattern.clone());
            format!("i.\"{field}\" GLOB ?")
        }
        // NOT over NULL is NULL; coalescing counts a missing field as not matching.
        Filter::Not(inner) => format!("NOT coalesce(({}), 0)", walk(inner, params)),
        Filter::Or(parts) => format!("({})", parts.iter().map(|p| walk(p, params)).collect::<Vec<_>>().join(" OR ")),
        Filter::And(parts) => format!("({})", parts.iter().map(|p| walk(p, params)).collect::<Vec<_>>().join(" AND ")),
    }
}
