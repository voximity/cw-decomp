//! Tests for the .cub voxel model format (Server.exe 0x0042f9a0, cube::Model::load).

mod common;

use cw_formats::{AssetDb, CubModel, Error};

fn header(x: u32, y: u32, z: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&x.to_le_bytes());
    v.extend_from_slice(&y.to_le_bytes());
    v.extend_from_slice(&z.to_le_bytes());
    v
}

#[test]
fn parses_a_one_voxel_model() {
    let mut bytes = header(1, 1, 1);
    bytes.extend_from_slice(&[10, 20, 30]);
    let m = CubModel::parse(&bytes).unwrap();
    assert_eq!(m.size, [1, 1, 1]);
    assert_eq!(m.voxels, vec![[10, 20, 30]]);
}

#[test]
fn parses_a_two_by_three_by_four_model_in_file_order() {
    let mut bytes = header(2, 3, 4);
    let n = 2 * 3 * 4;
    for i in 0..n as u8 {
        bytes.extend_from_slice(&[i, i.wrapping_add(100), i.wrapping_add(200)]);
    }
    let m = CubModel::parse(&bytes).unwrap();
    assert_eq!(m.size, [2, 3, 4]);
    assert_eq!(m.voxels.len(), n);
    assert_eq!(m.voxels[0], [0, 100, 200]);
    assert_eq!(m.voxels[n - 1], [23, 123, 223]);
}

#[test]
fn zero_sized_models_parse_to_no_voxels() {
    let m = CubModel::parse(&header(0, 5, 5)).unwrap();
    assert_eq!(m.size, [0, 5, 5]);
    assert!(m.voxels.is_empty());
}

#[test]
fn truncated_header_is_malformed() {
    let r = CubModel::parse(&[1, 0, 0]);
    assert!(
        matches!(r, Err(Error::Malformed { format: "cub", .. })),
        "{r:?}"
    );
}

#[test]
fn payload_shorter_than_declared_is_malformed() {
    let mut bytes = header(2, 2, 2);
    bytes.extend_from_slice(&[0; 3 * 7]);
    let r = CubModel::parse(&bytes);
    assert!(
        matches!(r, Err(Error::Malformed { format: "cub", .. })),
        "{r:?}"
    );
}

#[test]
fn trailing_bytes_after_the_payload_are_rejected() {
    // The original reads exactly x*y*z*3 bytes; anything after that is not a .cub we
    // understand, and silently ignoring it would hide format misunderstandings.
    let mut bytes = header(1, 1, 1);
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    let r = CubModel::parse(&bytes);
    assert!(
        matches!(r, Err(Error::Malformed { format: "cub", .. })),
        "{r:?}"
    );
}

#[test]
fn absurd_dimensions_do_not_overflow_or_allocate() {
    let r = CubModel::parse(&header(u32::MAX, u32::MAX, u32::MAX));
    assert!(
        matches!(r, Err(Error::Malformed { format: "cub", .. })),
        "{r:?}"
    );
}

#[test]
fn to_bytes_round_trips() {
    let mut bytes = header(3, 1, 2);
    for i in 0..6u8 {
        bytes.extend_from_slice(&[i, i * 2, i * 3]);
    }
    let m = CubModel::parse(&bytes).unwrap();
    assert_eq!(m.to_bytes(), bytes);
}

#[test]
fn real_aim_cub_is_16_by_16_by_18() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let db = AssetDb::open(dir.join("data1.db")).unwrap();
    let m = CubModel::parse(&db.get("aim.cub").unwrap()).unwrap();
    assert_eq!(m.size, [16, 16, 18]);
    assert_eq!(m.voxels.len(), 16 * 16 * 18);
}

#[test]
fn every_real_cub_parses_and_round_trips() {
    let Some(dir) = common::game_dir() else {
        return;
    };
    let db = AssetDb::open(dir.join("data1.db")).unwrap();
    let mut total_voxels = 0usize;
    for key in db.keys().unwrap() {
        let bytes = db.get(&key).unwrap();
        let m = CubModel::parse(&bytes).unwrap_or_else(|e| panic!("{key}: {e}"));
        assert_eq!(m.to_bytes(), bytes, "{key} round trip");
        assert!(
            m.size.iter().all(|&d| d > 0 && d <= 256),
            "{key}: size {:?}",
            m.size
        );
        total_voxels += m.voxels.len();
    }
    assert!(
        total_voxels > 1_000_000,
        "suspiciously few voxels: {total_voxels}"
    );
}
