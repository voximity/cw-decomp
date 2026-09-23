//! Tests for reading the asset databases (data1-4.db).

mod common;

use cw_formats::{AssetDb, Error};

#[test]
fn opening_a_missing_file_is_an_error() {
    let r = AssetDb::open("/nonexistent/definitely-not-here.db");
    assert!(r.is_err());
}

#[test]
fn opening_a_non_database_file_is_an_error() {
    let dir = std::env::temp_dir().join(format!("cw-formats-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("junk.db");
    std::fs::write(&path, b"this is not sqlite").unwrap();
    let r = AssetDb::open(&path).and_then(|db| db.keys());
    assert!(r.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_fresh_database_with_the_game_schema_round_trips_a_blob() {
    let dir = std::env::temp_dir().join(format!("cw-formats-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rt.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE blobs(key TEXT PRIMARY KEY, value BLOB)", [])
            .unwrap();
        let mut stored = b"hello, cube world".to_vec();
        cw_formats::encode(&mut stored);
        conn.execute(
            "INSERT INTO blobs(key, value) VALUES(?, ?)",
            rusqlite::params!["greeting.txt", stored],
        )
        .unwrap();
    }
    let db = AssetDb::open(&path).unwrap();
    assert_eq!(db.keys().unwrap(), vec!["greeting.txt".to_string()]);
    assert_eq!(db.get("greeting.txt").unwrap(), b"hello, cube world");
    assert!(matches!(db.get("missing.txt"), Err(Error::NotFound(k)) if k == "missing.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn real_databases_have_the_expected_key_counts() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let expected = [
        ("data1.db", 2857),
        ("data2.db", 93),
        ("data3.db", 104),
        ("data4.db", 2),
    ];
    for (name, count) in expected {
        let db = AssetDb::open(dir.join(name)).unwrap();
        let keys = db.keys().unwrap();
        assert_eq!(keys.len(), count, "{name}");
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{name}: keys() must be sorted");
    }
}

#[test]
fn real_keys_are_grouped_by_type_per_database() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let ext = |name: &str, want: &str| {
        let db = AssetDb::open(dir.join(name)).unwrap();
        for k in db.keys().unwrap() {
            assert!(k.ends_with(want), "{name}: unexpected key {k}");
        }
    };
    ext("data1.db", ".cub");
    ext("data2.db", ".wav");
    ext("data3.db", ".png");
    ext("data4.db", ".xml");
}

#[test]
fn raw_bytes_differ_from_decoded_bytes() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let db = AssetDb::open(dir.join("data3.db")).unwrap();
    let raw = db.raw("aim.png").unwrap();
    let decoded = db.get("aim.png").unwrap();
    assert_eq!(raw.len(), decoded.len());
    assert_ne!(raw, decoded);
}
