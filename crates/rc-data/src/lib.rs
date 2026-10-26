//! Data specific to a game version, prepared automatically from the player's installation and
//! cached outside the repository. Nothing loaded here is in the repository.

mod bin;
mod compile;
pub mod env_flags;
pub mod ids;
pub mod jar;
pub mod model;
pub mod pack_filter;
mod prepare;
pub mod rules;
pub mod symlinks;

pub use bin::{FORMAT, decode, decode_with, encode};
pub use compile::compile;
pub use model::*;
#[doc(hidden)]
pub use prepare::data_jsons_with;
pub use prepare::{BuildInfo, build_info};
pub use prepare::{Install, cache_root, commands_path, load_cached, load_or_prepare, locate, prepare};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path} : {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid format: {0}")]
    Format(String),
    #[error("stale cache: {0}")]
    Stale(String),
    #[error("not found: {0}")]
    Missing(String),
    #[error("download: {0}")]
    Download(String),
    #[error("Java : {0}")]
    Java(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io { path: path.to_owned(), source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
