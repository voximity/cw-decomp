//! Tests for the `.plx` (and `.pla`) Plasma graphics readers.

mod common;

use cw_formats::plx::{
    self, Attribute, AttributeItem, Chunk, Item, NodeField, PlxDocument, Str, WStr, WidgetField,
    WidgetKind,
};
use cw_formats::{AssetDb, pla};

#[test]
fn the_key_is_the_hash_of_plasmaxgraphics() {
    assert_eq!(plx::plasma_hash(b"PlasmaXGraphics"), 0xfb3a_260b_5ec7_cd5c);
    assert_eq!(plx::default_key(), 0xfb3a_260b_5ec7_cd5c);
    assert_eq!(plx::plasma_hash(b"PlasmaGraphics"), 0x62f1_5bac_c28b_c932);
    assert_eq!(plx::plasma_hash(b""), 0x0003_ffff_ffff_ffe5);
}

#[test]
fn encrypt_and_decrypt_are_inverse_for_every_length() {
    let key = plx::default_key();
    for n in 0..40usize {
        let plain: Vec<u8> = (0..n as u8).map(|i| i.wrapping_mul(37).wrapping_add(5)).collect();
        let sealed = plx::encrypt(&plain, key);
        assert_eq!(plx::decrypt(&sealed, key), plain, "length {n}");
    }
    // Key 0 is the identity, which is how a plaintext seal is accepted.
    assert_eq!(plx::decrypt(b"PlasmaGraphics", 0), b"PlasmaGraphics");
}

#[test]
fn the_shipped_seal_opens_with_the_default_key() {
    // The 14 sealed bytes of every shipped file.
    let sealed = hex::decode("7887816dbd3d2fc76e99ac3928d1").unwrap();
    assert_eq!(
        plx::find_seal_key(&sealed, &plx::default_candidate_keys()),
        Some(plx::default_key())
    );
    assert_eq!(plx::find_seal_key(&sealed, &[0, 1, 2]), None);
}

fn sample_document(key: u64) -> PlxDocument {
    let sealed = plx::encrypt(plx::MAGIC.as_bytes(), key);
    let wname = |s: &str| WStr(s.encode_utf16().collect());
    PlxDocument {
        key,
        items: vec![
            Item::Version(1),
            Item::Seal(Str(sealed)),
            Item::PageWidth(1000.0),
            Item::Unit(0),
            Item::PageColor([1.0, 0.5, 0.25, 0.0]),
            Item::Widget(
                WidgetKind::Button,
                vec![
                    WidgetField::WName(wname("ok")),
                    WidgetField::BindPos([10.0, 20.0]),
                    WidgetField::ButtonType(2),
                ],
            ),
            Item::Node(vec![
                NodeField::WName(wname("root")),
                NodeField::Shape(-1),
                NodeField::Widget(0),
                NodeField::Child(1),
                NodeField::Child(2),
            ]),
            Item::Other(Chunk::leaf("somethingNew", vec![1, 2, 3])),
        ],
    }
}

#[test]
fn a_synthetic_document_round_trips() {
    let doc = sample_document(plx::default_key());
    let bytes = plx::encode(&doc);
    let back = plx::parse(&bytes).unwrap();
    assert_eq!(back, doc);
    assert_eq!(plx::encode(&back), bytes);
    // Names after the seal are not stored in plain text.
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("PlasmaGraphics") && text.contains("Seal"));
    assert!(!text.contains("Node.wname"));
}

#[test]
fn a_plaintext_seal_selects_key_zero() {
    let doc = sample_document(0);
    let bytes = plx::encode(&doc);
    assert!(String::from_utf8_lossy(&bytes).contains("Node.wname"));
    assert_eq!(plx::parse(&bytes).unwrap(), doc);
}

#[test]
fn an_unknown_seal_key_is_rejected() {
    let doc = sample_document(0x1234_5678_9abc_def0);
    let bytes = plx::encode(&doc);
    assert!(plx::parse(&bytes).is_err());
    assert!(plx::parse_with_keys(&bytes, &[0x1234_5678_9abc_def0]).is_ok());
}

#[test]
fn version_two_is_rejected() {
    let mut doc = sample_document(plx::default_key());
    doc.items[0] = Item::Version(2);
    assert!(plx::parse(&plx::encode(&doc)).is_err());
}

#[test]
fn truncated_input_is_an_error() {
    let bytes = plx::encode(&sample_document(plx::default_key()));
    for cut in [3, 10, bytes.len() / 2, bytes.len() - 1] {
        assert!(plx::parse(&bytes[..cut]).is_err(), "cut at {cut}");
    }
}

#[test]
fn attributes_keep_frames_and_sequences_in_order() {
    let attr: Attribute<i32> = Attribute {
        items: vec![AttributeItem::Frame(1), AttributeItem::Frame(0)],
    };
    assert_eq!(attr.frames().copied().collect::<Vec<_>>(), vec![1, 0]);
}

/// One line of `tests/golden/plx.txt`.
fn golden_files() -> Vec<(String, usize, String)> {
    include_str!("golden/plx.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split(' ');
            (
                it.next().unwrap().to_string(),
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn every_shipped_plx_parses_typed_and_round_trips_byte_for_byte() {
    let Some(dir) = common::game_dir() else { return };
    for (name, len, sha) in golden_files() {
        let bytes = std::fs::read(dir.join(&name)).unwrap();
        assert_eq!(bytes.len(), len, "{name}");
        assert_eq!(common::sha256_hex(&bytes), sha, "{name}");
        let doc = plx::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(doc.key, plx::default_key(), "{name}");
        // Every chunk decodes as the type its reader expects: nothing falls back to Other.
        let debug = format!("{:?}", doc.items);
        assert!(!debug.contains("Other("), "{name} has undecoded chunks");
        assert!(plx::encode(&doc) == bytes, "{name} does not round trip");
        assert!(doc.nodes().count() > 0, "{name}");
    }
}

#[test]
fn cursor_plx_dump_matches_the_golden() {
    let Some(dir) = common::game_dir() else { return };
    let bytes = std::fs::read(dir.join("cursor.plx")).unwrap();
    let doc = plx::parse(&bytes).unwrap();
    let dump = doc.dump();
    let golden = include_str!("golden/cursor_plx_dump.txt").replace("\r\n", "\n");
    assert_eq!(dump, golden);
}

#[test]
fn asset_databases_hold_no_plx_or_pla_blobs() {
    // The GUI documents are loose files in the game directory; the databases hold .cub,
    // .wav, .png and .xml only. Any .plx/.pla blob found later must parse and round trip.
    let Some(dir) = common::game_dir() else { return };
    let mut found = 0;
    for db in ["data1.db", "data2.db", "data3.db", "data4.db"] {
        let db = AssetDb::open(dir.join(db)).unwrap();
        for key in db.keys().unwrap() {
            let bytes = db.get(&key).unwrap();
            if key.ends_with(".plx") {
                found += 1;
                assert_eq!(plx::encode(&plx::parse(&bytes).unwrap()), bytes, "{key}");
            } else if key.ends_with(".pla") {
                found += 1;
                assert_eq!(pla::encode(&pla::parse(&bytes).unwrap()), bytes, "{key}");
            }
        }
    }
    assert_eq!(found, 0);
}

#[test]
fn a_synthetic_pla_round_trips() {
    use pla::{PlaBody, PlaChunk, PlaDocument};
    let leaf = |tag, b: &[u8]| PlaChunk { tag, body: PlaBody::Leaf(b.to_vec()) };
    let mut name = 5i32.to_le_bytes().to_vec();
    name.extend_from_slice(b"hello");
    let doc = PlaDocument {
        chunks: vec![
            leaf(20, &640f32.to_le_bytes()),
            leaf(21, &480f32.to_le_bytes()),
            leaf(23, &2i32.to_le_bytes()),
            PlaChunk {
                tag: 9,
                body: PlaBody::Group(vec![leaf(1, &7i32.to_le_bytes()), leaf(3, &name)]),
            },
            leaf(3, &[1, 2, 3, 4, 5]),
        ],
    };
    let bytes = pla::encode(&doc);
    let back = pla::parse(&bytes).unwrap();
    assert_eq!(back, doc);
    let page = back.page();
    assert_eq!(page.width, Some(640.0));
    assert_eq!(page.height, Some(480.0));
    assert_eq!(page.dpi, None);
    assert_eq!(page.unit, Some(2));
    assert_eq!(back.names(), vec![(7, b"hello".to_vec())]);
    assert!(pla::parse(&bytes[..bytes.len() - 1]).is_err());
}
