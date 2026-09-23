//! Tests for the asset blob de-obfuscation (Server.exe 0x00412f80).

mod common;

use cw_formats::blob::{KEY, decode, encode};

#[test]
fn key_table_is_the_one_from_the_binary() {
    assert_eq!(KEY.len(), 44);
    assert_eq!(KEY[0], 4242);
    assert_eq!(KEY[3], 84800);
    assert_eq!(KEY[42], 816583);
    assert_eq!(KEY[43], 13);
}

#[test]
fn decode_empty_is_empty() {
    let mut v: Vec<u8> = Vec::new();
    decode(&mut v);
    assert!(v.is_empty());
}

#[test]
fn decode_single_byte_only_inverts() {
    let mut v = vec![0x00];
    decode(&mut v);
    assert_eq!(v, [0xff]);
    let mut v = vec![0x5a];
    decode(&mut v);
    assert_eq!(v, [0xa5]);
}

#[test]
fn decode_matches_reference_vectors() {
    // Expected values computed with the Python reference implementation that was checked
    // against PNG, RIFF, XML and .cub blobs from the real databases.
    let mut v: Vec<u8> = (0u8..12).collect();
    decode(&mut v);
    assert_eq!(
        v,
        [244, 255, 253, 250, 245, 247, 249, 251, 246, 248, 254, 252]
    );

    let mut v = b"CubeWorld".to_vec();
    decode(&mut v);
    assert_eq!(v, [138, 155, 168, 188, 154, 157, 147, 141, 144]);
}

#[test]
fn encode_is_the_exact_inverse_of_decode() {
    // Deterministic pseudo-random contents over many lengths, including lengths that
    // exercise every key index (n > 44) and the modulo wrap.
    let mut state = 0x1234_5678u32;
    for n in 0..300usize {
        let original: Vec<u8> = (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (state >> 16) as u8
            })
            .collect();
        let mut v = original.clone();
        decode(&mut v);
        encode(&mut v);
        assert_eq!(v, original, "round trip failed for n={n}");

        let mut v = original.clone();
        encode(&mut v);
        decode(&mut v);
        assert_eq!(v, original, "reverse round trip failed for n={n}");
    }
}

#[test]
fn decode_is_a_permutation_plus_inversion() {
    // Whatever the shuffle does, the multiset of inverted bytes must be preserved.
    let original: Vec<u8> = (0..=255u8).chain(0..=255u8).collect();
    let mut v = original.clone();
    decode(&mut v);
    let mut a: Vec<u8> = original.iter().map(|b| !b).collect();
    let mut b = v.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b);
}

#[test]
fn real_png_blob_decodes_to_a_valid_png() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let db = cw_formats::AssetDb::open(dir.join("data3.db")).unwrap();
    let bytes = db.get("aim.png").unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    assert_eq!((info.width, info.height), (14, 14));
}

#[test]
fn real_wav_and_xml_blobs_have_their_magic() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let wav = cw_formats::AssetDb::open(dir.join("data2.db"))
        .unwrap()
        .get("absorb.wav")
        .unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let xml = cw_formats::AssetDb::open(dir.join("data4.db"))
        .unwrap()
        .get("dict_en.xml")
        .unwrap();
    assert!(
        xml.starts_with(b"\xef\xbb\xbf<?xml"),
        "got {:?}",
        &xml[..16]
    );
}

#[test]
fn every_blob_in_every_database_matches_the_golden_hashes() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let golden = common::golden_blobs();
    assert_eq!(golden.len(), 3056);
    let mut opened: std::collections::HashMap<String, cw_formats::AssetDb> = Default::default();
    for g in &golden {
        let db = opened
            .entry(g.db.clone())
            .or_insert_with(|| cw_formats::AssetDb::open(dir.join(&g.db)).unwrap());
        let bytes = db.get(&g.key).unwrap();
        assert_eq!(bytes.len(), g.len, "{}/{} length", g.db, g.key);
        assert_eq!(
            common::sha256_hex(&bytes),
            g.sha256,
            "{}/{} hash",
            g.db,
            g.key
        );
    }
}
