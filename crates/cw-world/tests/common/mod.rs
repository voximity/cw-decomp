//! Readers for the golden sample files written by `tools/oracle/server_oracle.py fn`.

#![allow(dead_code)]

pub fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

pub fn f32_hex(s: &str) -> f32 {
    f32::from_le_bytes(hex_bytes(s).try_into().unwrap())
}

pub fn f64_hex(s: &str) -> f64 {
    f64::from_le_bytes(hex_bytes(s).try_into().unwrap())
}

/// Non-comment lines split on spaces.
pub fn rows(text: &str) -> Vec<Vec<&str>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split(' ').collect())
        .collect()
}

/// Asserts that every `got` equals its `want` bit for bit, reporting all mismatches at once.
pub fn check_bits<T: std::fmt::Debug>(name: &str, results: Vec<(T, Vec<u32>, Vec<u32>)>) {
    let total = results.len();
    let bad: Vec<_> = results.into_iter().filter(|(_, got, want)| got != want).collect();
    assert!(total >= 1000, "{name}: golden file too small ({total})");
    assert!(
        bad.is_empty(),
        "{name}: {} of {total} samples differ; first: {:?}",
        bad.len(),
        &bad[..bad.len().min(3)]
    );
}

use cw_world::zone::Zone;

/// The oracle's `serializeZone` format, version 6: the columns, then every entity record
/// length-prefixed (spawns, ground items, statics, props, markers), then the dirty byte and the
/// modified-block records.
pub fn serialize_zone(zone: &Zone) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 << 20);
    out.extend_from_slice(&0x454e4f5au32.to_le_bytes());
    out.extend_from_slice(&zone.x.to_le_bytes());
    out.extend_from_slice(&zone.y.to_le_bytes());
    out.extend_from_slice(&6u32.to_le_bytes());
    for col in &zone.columns {
        out.extend_from_slice(&col.climate_a.to_le_bytes());
        out.extend_from_slice(&col.climate_b.to_le_bytes());
        out.extend_from_slice(&col.climate_c.to_le_bytes());
        out.extend_from_slice(&col.height.to_le_bytes());
        out.extend_from_slice(&col.f14.to_le_bytes());
        out.extend_from_slice(&(col.blocks.len() as u32).to_le_bytes());
        for b in &col.blocks {
            out.extend_from_slice(b);
        }
    }
    fn records(out: &mut Vec<u8>, recs: Vec<Vec<u8>>) {
        out.extend_from_slice(&(recs.len() as u32).to_le_bytes());
        for r in recs {
            out.extend_from_slice(&(r.len() as u32).to_le_bytes());
            out.extend_from_slice(&r);
        }
    }
    records(&mut out, zone.spawns.iter().map(|s| s.dump_bytes()).collect());
    records(&mut out, zone.items.iter().map(|s| s.dump_bytes()).collect());
    records(&mut out, zone.statics.iter().map(|s| s.dump_bytes()).collect());
    records(&mut out, zone.props.iter().map(|s| s.dump_bytes()).collect());
    records(&mut out, zone.markers.iter().map(|s| s.dump_bytes()).collect());
    out.push(u8::from(zone.dirty));
    records(&mut out, zone.modified.iter().map(|m| m.to_bytes().to_vec()).collect());
    out
}

/// A world with the model table loaded from `$CW_GAME_DIR/data1.db`; panics without the game
/// files, since the generator needs the models for trees, towns and dungeons.
pub fn world_with_models(seed: i32) -> cw_world::World {
    let dir = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from).expect("CW_GAME_DIR must point at the game directory");
    let mut world = cw_world::World::new(seed);
    let mut table = Vec::new();
    cw_world::model::load_models_from_game_dir(&dir, &mut table).expect("model table");
    world.models = std::sync::Arc::new(table);
    world
}
