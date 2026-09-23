//! The asset databases data1.db through data4.db.
//!
//! Each is a SQLite 3 file with one table, `blobs(key TEXT PRIMARY KEY, value BLOB)`.
//! data1 holds `.cub` models, data2 `.wav` sounds, data3 `.png` textures, data4 the two
//! XML dictionaries. Values are stored obfuscated; see [`crate::blob`].
//!
//! Reading mirrors `Server.exe 0x00413070` (`cube::Database::getBlob`), which prepares
//! `SELECT value FROM blobs WHERE key = ?`, and `0x00413130` (`getBlobVec`), which copies
//! the blob into a vector; the asset callers then decode it (`decodeBlob`).

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::{Error, Result, blob};

/// A read-only handle on one asset database.
pub struct AssetDb {
    conn: Connection,
}

impl AssetDb {
    /// Open an existing database read-only. Fails if the file does not exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Ok(Self { conn })
    }

    /// Every key, sorted.
    pub fn keys(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT key FROM blobs ORDER BY key")?;
        let keys = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }

    /// Number of blobs.
    pub fn len(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// True when the database holds no blobs.
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// The stored, still obfuscated, bytes of one blob.
    pub fn raw(&self, key: &str) -> Result<Vec<u8>> {
        let value: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT value FROM blobs WHERE key = ?", [key], |r| r.get(0))
            .optional()?;
        value.ok_or_else(|| Error::NotFound(key.to_string()))
    }

    /// The decoded contents of one blob.
    pub fn get(&self, key: &str) -> Result<Vec<u8>> {
        let mut bytes = self.raw(key)?;
        blob::decode(&mut bytes);
        Ok(bytes)
    }
}
