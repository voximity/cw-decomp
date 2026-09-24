//! The world save database, `Save/world_<name>.db`.
//!
//! The same one-table SQLite layout as the asset databases (`blobs(key TEXT PRIMARY KEY,
//! value BLOB)`), but read and written through `cube::Database` without the asset
//! obfuscation: `getBlobVec` (`Server.exe 0x00413130`) copies the row into a vector and the
//! callers that read assets decode it afterwards; the save readers (`World::load`,
//! `createRegion`, `generateZone`) use the bytes as stored, and `putBlob`
//! (`Server.exe 0x00413240`) binds the vector as is.
//!
//! `Database::open` (`0x00413010`) opens or creates the file and runs `CREATE TABLE blobs(...)`
//! ignoring the error when the table exists. `putBlob` does `SELECT 1 FROM blobs WHERE key = ?`
//! and then `UPDATE` or `INSERT`. The contents of the blobs are defined by the writers in
//! `cw-world` (`save.rs` there).

use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::Result;

/// A read-write handle on one world save database.
pub struct SaveDb {
    conn: Connection,
}

impl fmt::Debug for SaveDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveDb").field("path", &self.conn.path()).finish()
    }
}

impl SaveDb {
    /// Open the database, creating the file and the `blobs` table when missing
    /// (`cube::Database::open`). The parent directory must exist, as for the original.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS blobs(key TEXT PRIMARY KEY, value BLOB);")?;
        Ok(Self { conn })
    }

    /// An in-memory database, for tests and for worlds that must not touch the disk.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("CREATE TABLE blobs(key TEXT PRIMARY KEY, value BLOB);")?;
        Ok(Self { conn })
    }

    /// Every key, sorted.
    pub fn keys(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT key FROM blobs ORDER BY key")?;
        let keys = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(keys)
    }

    /// The stored bytes of one blob (`getBlobVec`), or `None` when the key is absent.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let value: Option<Vec<u8>> =
            self.conn.query_row("SELECT value FROM blobs WHERE key = ?", [key], |r| r.get(0)).optional()?;
        Ok(value)
    }

    /// Store one blob (`putBlob`): an `UPDATE` when the key exists, else an `INSERT`.
    pub fn put(&self, key: &str, value: &[u8]) -> Result<()> {
        let exists: Option<i64> =
            self.conn.query_row("SELECT 1 FROM blobs WHERE key = ?", [key], |r| r.get(0)).optional()?;
        if exists.is_some() {
            self.conn.execute("UPDATE blobs SET value=? WHERE key=?", params![value, key])?;
        } else {
            self.conn.execute("INSERT INTO blobs(key, value) VALUES(?, ?)", params![key, value])?;
        }
        Ok(())
    }

    /// Runs `f`'s writes as one transaction (one journal and one sync of the file instead of one
    /// per statement; the stored rows are the same). The transaction is committed whatever `f`
    /// returns, so a failure keeps the writes before it, as separate `putBlob` calls would.
    pub fn batch<T>(&self, f: impl FnOnce(&SaveDb) -> Result<T>) -> Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let r = f(self);
        tx.commit()?;
        r
    }

    /// Delete one blob (Cube.exe 0x00449720, called with the key by the client's
    /// `deleteCharacter` 0x004816f0; its statement was not read, a `DELETE` is assumed).
    pub fn delete(&self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM blobs WHERE key=?", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SaveDb;

    #[test]
    fn put_then_get_round_trips_and_updates_in_place() {
        let db = SaveDb::in_memory().unwrap();
        assert_eq!(db.get("time").unwrap(), None);
        db.put("time", &[1, 0, 0, 0, 2, 0, 0, 0]).unwrap();
        assert_eq!(db.get("time").unwrap().unwrap(), vec![1, 0, 0, 0, 2, 0, 0, 0]);
        db.put("time", &[9]).unwrap();
        assert_eq!(db.get("time").unwrap().unwrap(), vec![9]);
        db.put("zone1_2", &[]).unwrap();
        assert_eq!(db.keys().unwrap(), vec!["time".to_string(), "zone1_2".to_string()]);
    }

    #[test]
    fn open_creates_the_file_and_the_table() {
        let dir = std::env::temp_dir().join(format!("cw-save-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("world_test.db");
        {
            let db = SaveDb::open(&path).unwrap();
            db.put("k", b"v").unwrap();
        }
        let db = SaveDb::open(&path).unwrap();
        assert_eq!(db.get("k").unwrap().unwrap(), b"v".to_vec());
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A batch of puts is one transaction (one journal and sync instead of one per blob):
    /// nothing is visible to another connection until it ends, then everything is.
    #[test]
    fn a_batch_commits_once_at_the_end() {
        let dir = std::env::temp_dir().join(format!("cw-save-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("world_batch.db");
        let db = SaveDb::open(&path).unwrap();
        db.put("a", b"old").unwrap();
        let other = SaveDb::open(&path).unwrap();
        db.batch(|db| {
            db.put("a", b"new")?;
            db.put("b", b"2")?;
            assert_eq!(other.get("a").unwrap(), Some(b"old".to_vec()), "committed inside the batch");
            assert_eq!(other.get("b").unwrap(), None, "committed inside the batch");
            Ok(())
        })
        .unwrap();
        assert_eq!(other.get("a").unwrap(), Some(b"new".to_vec()));
        assert_eq!(other.get("b").unwrap(), Some(b"2".to_vec()));
        drop((db, other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed batch keeps what was written before the failure, as the separate writes did.
    #[test]
    fn a_failed_batch_keeps_the_earlier_writes() {
        let db = SaveDb::in_memory().unwrap();
        let r: crate::Result<()> = db.batch(|db| {
            db.put("a", b"1")?;
            Err(crate::Error::NotFound("x".into()))
        });
        assert!(r.is_err());
        assert_eq!(db.get("a").unwrap(), Some(b"1".to_vec()));
    }
}
