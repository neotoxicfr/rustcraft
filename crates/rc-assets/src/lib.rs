//! Player resources read at run time (jar of the version, packs): textures and baked
//! block models. None of these resources is in the repository; what is missing has a fallback.

pub mod bake;
pub mod cache;
pub mod font;
pub mod geom;
pub mod gui;
pub mod item;
pub mod mip;
pub mod model;
pub mod packs;
pub mod palette;
pub mod source;
pub mod texture;

pub use bake::{AnimationClock, BakedQuad, BlockModels, FluidTextures, Layer, TextureAnimation, build};
pub use source::{Assets, Source};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path} : {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid format: {0}")]
    Format(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io { path: path.to_owned(), source }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
