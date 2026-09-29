//! repoglass: a local index of one directory tree, searched by exact
//! symbol, keyword and embedding.

pub mod cli;
pub mod config;
pub mod corpus;
pub mod embeddings;
pub mod index;
pub mod models;
pub mod pyfmt;
pub mod search;
pub mod semsift;
pub mod store;
pub mod text;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

pub use embedded::{QUERY_FILES, SCHEMA_SQL};
