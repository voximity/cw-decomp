//! End-to-end tests for the `cwtool` binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cwtool() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cwtool"))
}

fn game_dir() -> Option<PathBuf> {
    match std::env::var_os("CW_GAME_DIR") {
        Some(p) => Some(PathBuf::from(p)),
        None => {
            eprintln!("CW_GAME_DIR not set; skipping test that needs the game files");
            None
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cwtool-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A tiny database in the game's schema with two encoded blobs.
fn fixture_db(dir: &Path) -> PathBuf {
    let path = dir.join("fixture.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute("CREATE TABLE blobs(key TEXT PRIMARY KEY, value BLOB)", [])
        .unwrap();
    for (key, contents) in [
        ("b.txt", b"second".as_slice()),
        ("a.txt", b"first".as_slice()),
    ] {
        let mut stored = contents.to_vec();
        cw_formats::encode(&mut stored);
        conn.execute(
            "INSERT INTO blobs(key, value) VALUES(?, ?)",
            rusqlite::params![key, stored],
        )
        .unwrap();
    }
    path
}

#[test]
fn no_arguments_prints_usage_and_fails() {
    let out = cwtool().output().unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Usage"), "{text}");
}

#[test]
fn db_list_prints_sorted_keys_one_per_line() {
    let dir = temp_dir("list");
    let db = fixture_db(&dir);
    let out = cwtool().args(["db", "list"]).arg(&db).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a.txt\nb.txt\n");
}

#[test]
fn db_list_on_a_missing_file_fails_with_a_message() {
    let out = cwtool()
        .args(["db", "list", "/nonexistent/none.db"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("none.db"));
}

#[test]
fn db_extract_writes_every_decoded_blob_into_the_output_directory() {
    let dir = temp_dir("extract");
    let db = fixture_db(&dir);
    let out_dir = dir.join("out");
    let out = cwtool()
        .args(["db", "extract"])
        .arg(&db)
        .arg("--out")
        .arg(&out_dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(out_dir.join("a.txt")).unwrap(), b"first");
    assert_eq!(std::fs::read(out_dir.join("b.txt")).unwrap(), b"second");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("2 blobs"), "{text}");
}

#[test]
fn db_extract_one_key_to_stdout() {
    let dir = temp_dir("cat");
    let db = fixture_db(&dir);
    let out = cwtool()
        .args(["db", "cat"])
        .arg(&db)
        .arg("b.txt")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"second");
}

#[test]
fn db_cat_of_a_missing_key_fails() {
    let dir = temp_dir("cat-missing");
    let db = fixture_db(&dir);
    let out = cwtool()
        .args(["db", "cat"])
        .arg(&db)
        .arg("zzz.txt")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("zzz.txt"));
}

#[test]
fn cub_info_reports_dimensions_and_voxel_counts() {
    let dir = temp_dir("cub");
    let mut bytes = Vec::new();
    for d in [2u32, 1, 2] {
        bytes.extend_from_slice(&d.to_le_bytes());
    }
    // Two filled voxels and two empty (black) ones.
    bytes.extend_from_slice(&[1, 2, 3, 0, 0, 0, 0, 0, 0, 9, 9, 9]);
    let path = dir.join("tiny.cub");
    std::fs::write(&path, &bytes).unwrap();
    let out = cwtool().args(["cub", "info"]).arg(&path).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("size: 2 x 1 x 2"), "{text}");
    assert!(text.contains("voxels: 4"), "{text}");
    assert!(text.contains("filled: 2"), "{text}");
}

#[test]
fn cub_info_on_garbage_fails() {
    let dir = temp_dir("cub-bad");
    let path = dir.join("bad.cub");
    std::fs::write(&path, b"nope").unwrap();
    let out = cwtool().args(["cub", "info"]).arg(&path).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cub"));
}

#[test]
fn real_data3_extracts_valid_pngs() {
    let Some(game) = game_dir() else { return };
    let dir = temp_dir("real-png");
    let out = cwtool()
        .args(["db", "extract"])
        .arg(game.join("data3.db"))
        .arg("--out")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let bytes = std::fs::read(entry.unwrap().path()).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        count += 1;
    }
    assert_eq!(count, 104);
}

#[test]
fn save_show_decodes_a_zone_blob() {
    use cw_world::save::ModifiedBlock;
    use cw_world::zone::{GroundItem, Zone};
    let dir = temp_dir("save");
    let path = dir.join("world_test.db");
    let db = cw_formats::SaveDb::open(&path).unwrap();
    let mut zone = Zone::new(1, 2);
    zone.dirty = true;
    zone.items.push(GroundItem { item: cw_world::inventory::Item { item_type: 0xb, sub_type: 0x13, level: 3, material: 9, ..cw_world::inventory::Item::NEW }, x: 100 << 16, y: 200 << 16, z: 50 << 16, f144: 7, ..GroundItem::NEW });
    zone.modified.push(ModifiedBlock { x: 5, y: 6, z: 7, block: [1, 2, 3, 0x21], time: -1 });
    db.put("zone1_2", &zone.save_blob()).unwrap();
    db.put("time", &[3, 0, 0, 0, 0x40, 0x77, 0x1b, 0]).unwrap();
    drop(db);

    let out = cwtool().args(["save", "show"]).arg(&path).arg("zone1_2").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("1 ground items"), "{text}");
    assert!(text.contains("item type 11 sub 19 level 3 material 9 rarity 0 at (100.00, 200.00, 50.00)"), "{text}");
    assert!(text.contains("day 7"), "{text}");
    assert!(text.contains("(5, 6, 7) rgb (1, 2, 3) type 0x21 day -1"), "{text}");
    assert!(text.contains("0 spawns"), "{text}");

    let out = cwtool().args(["save", "show"]).arg(&path).arg("time").output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "day 3, time of day 1800000 ms (00:30:00)\n");

    let out = cwtool().args(["save", "list"]).arg(&path).output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "time\nzone1_2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plx_dump_prints_decrypted_names() {
    use cw_formats::plx::{self, Item, NodeField, PlxDocument, Str, WStr};
    let key = plx::default_key();
    let doc = PlxDocument {
        key,
        items: vec![
            Item::Version(1),
            Item::Seal(Str(plx::encrypt(plx::MAGIC.as_bytes(), key))),
            Item::Node(vec![NodeField::WName(WStr("root".encode_utf16().collect())), NodeField::Child(1)]),
        ],
    };
    let dir = temp_dir("plx");
    let path = dir.join("tiny.plx");
    std::fs::write(&path, plx::encode(&doc)).unwrap();
    let out = cwtool().args(["plx", "dump"]).arg(&path).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Node.wname [12] \"root\""), "{text}");
    assert!(text.contains("Node.child [4] 1"), "{text}");
    std::fs::write(&path, b"junk").unwrap();
    let out = cwtool().args(["plx", "dump"]).arg(&path).output().unwrap();
    assert!(!out.status.success());
}
