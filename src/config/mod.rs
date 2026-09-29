//! Configuration: paths, settings, and their resolution.

pub mod load;
pub mod paths;
pub mod render;
pub mod schema;

pub use load::{load, ConfigError};
pub use paths::Paths;
pub use render::as_toml;
pub use schema::{categories_rev, chunking_rev, Settings, MAX_CHUNK_CHARS, MIN_CHUNK_CHARS};
