//! On-disk formats of Cube World Alpha.
//!
//! Every reader here reproduces what the original executables do byte for byte. Doc
//! comments cite the original function as `Server.exe 0x<address>` so the two sides stay
//! linked as names change in the Ghidra project.

pub mod blob;
pub mod cub;
pub mod db;
pub mod pla;
pub mod plx;
pub mod save;

pub use blob::{decode, encode};
pub use cub::CubModel;
pub use db::AssetDb;
pub use save::SaveDb;

/// Errors from any format reader in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("blob {0:?} not found")]
    NotFound(String),
    #[error("{format}: {reason}")]
    Malformed {
        format: &'static str,
        reason: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
