//! Shared helpers for integration tests that read the real game files.
//!
//! The game files are copyrighted and never committed. Tests that need them read the
//! `CW_GAME_DIR` environment variable and skip, with a message, when it is unset.

#![allow(dead_code)]

use std::path::PathBuf;

/// Directory holding Cube.exe, Server.exe and data1-4.db, or `None` to skip.
pub fn game_dir() -> Option<PathBuf> {
    match std::env::var_os("CW_GAME_DIR") {
        Some(p) => Some(PathBuf::from(p)),
        None => {
            eprintln!("CW_GAME_DIR not set; skipping test that needs the game files");
            None
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// One line of `tests/golden/blobs.txt`.
pub struct GoldenBlob {
    pub db: String,
    pub key: String,
    pub len: usize,
    pub sha256: String,
}

pub fn golden_blobs() -> Vec<GoldenBlob> {
    let text = include_str!("../golden/blobs.txt");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split(' ');
            GoldenBlob {
                db: it.next().unwrap().to_string(),
                key: it.next().unwrap().to_string(),
                len: it.next().unwrap().parse().unwrap(),
                sha256: it.next().unwrap().to_string(),
            }
        })
        .collect()
}
