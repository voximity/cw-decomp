//! `cube::Model`, the model table and `placeModel` (`Server.exe 0x00524540`).
//!
//! The placement tests use hand-built models on a fresh zone, where every expected value follows
//! from the decompilation by hand. `server_exe_model_table` re-derives the checks of
//! `model_names.rs` from the bytes of `Server.exe` and is skipped when `CW_GAME_DIR` is unset.
//! `real_model_table` needs the `cw-formats` dependency (see `model.rs`).


use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cw_math::MsvcRand;
use cw_world::World;
use cw_world::model::{EMPTY, LoggedBlock, MODEL_COUNT, MODEL_LOADS, Model, load_models};
use cw_world::zone::{Static, Zone};

const ZX: i32 = 100;
const ZY: i32 = 120;
const X0: i32 = ZX * 256;
const Y0: i32 = ZY * 256;

fn cub_bytes(size: [u32; 3], voxels: &[[u8; 3]]) -> Vec<u8> {
    let mut b = Vec::new();
    for s in size {
        b.extend_from_slice(&s.to_le_bytes());
    }
    for v in voxels {
        b.extend_from_slice(v);
    }
    b
}

/// A model from voxels listed x fastest, then y, then z.
fn model(size: [i32; 3], voxels: &[[u8; 3]]) -> Model {
    Model::new(size, voxels.to_vec(), true)
}

fn setup() -> (World, Zone) {
    (World::new(1234), Zone::new(ZX, ZY))
}

fn place(world: &mut World, zone: &mut Zone, m: &Model, pos: [i32; 3], rot: i32, kind: i32) -> Vec<LoggedBlock> {
    let mut log = Vec::new();
    world.place_model_logged(zone, m, pos, rot, 6, kind, false, [0; 4], Some(&mut log));
    log
}

#[test]
fn model_table_facts() {
    assert_eq!(MODEL_LOADS.len(), 2557);
    let mut per_slot: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    for &(_, index, key, _) in MODEL_LOADS.iter() {
        assert!(usize::from(index) < MODEL_COUNT);
        assert!(key.ends_with(".cub"));
        per_slot.entry(index).or_default().push(key);
    }
    let missing: Vec<u16> = (0..MODEL_COUNT as u16).filter(|i| !per_slot.contains_key(i)).collect();
    assert_eq!(
        missing,
        [
            1559, 1560, 1561, 1562, 1563, 1589, 1590, 1591, 1592, 1593, 2060, 2061, 2071, 2353, 2354, 2355, 2356, 2419,
            2420
        ]
    );
    let twice: Vec<u16> = per_slot.iter().filter(|(_, k)| k.len() > 1).map(|(&i, _)| i).collect();
    assert_eq!(twice, [1224, 1225, 1226, 1227, 1228, 1230, 1300]);
    assert_eq!(per_slot[&1300], ["orc-head-m01.cub", "orc-head.cub"]);
    assert_eq!(per_slot[&0x84b], ["character-platform.cub"]);
    assert_eq!(per_slot[&2300], ["palm-leaf.cub"]);
    assert_eq!(per_slot[&2301], ["palm-leaf-diagonal.cub"]);
    assert_eq!(per_slot[&2544], ["castle-arc.cub"]);
    assert_eq!(per_slot[&2545], ["temple-arc.cub"]);
    // Call addresses are strictly increasing: the table is in call order.
    assert!(MODEL_LOADS.windows(2).all(|w| w[0].0 < w[1].0));
}

#[test]
fn load_models_follows_call_order() {
    let absent: BTreeSet<&str> = ["palm-leaf.cub", "orc-head.cub"].into();
    let colour = |key: &str| [key.len() as u8, key.as_bytes()[0], 1];
    let mut table = Vec::new();
    load_models(&mut table, |key| (!absent.contains(key)).then(|| cub_bytes([1, 1, 1], &[colour(key)])));
    assert_eq!(table.len(), MODEL_COUNT);
    // Never loaded.
    assert_eq!(table[1559], Model::default());
    // The only key is absent: the slot stays empty but records `raw`.
    assert_eq!(table[2300].size, [0; 3]);
    assert!(table[2300].raw);
    // The second load of 1300 has no blob, so the first one stays.
    assert_eq!(table[1300].voxel(0, 0, 0), colour("orc-head-m01.cub"));
    assert_eq!(table[0x84b].voxel(0, 0, 0), colour("character-platform.cub"));
}

#[test]
fn cub_bytes_layout() {
    let mut voxels = Vec::new();
    for z in 0..4u8 {
        for y in 0..3u8 {
            for x in 0..2u8 {
                voxels.push([x + 1, y + 1, z + 1]);
            }
        }
    }
    let m = Model::from_cub_bytes(&cub_bytes([2, 3, 4], &voxels), false);
    assert_eq!(m.size, [2, 3, 4]);
    assert!(!m.raw);
    assert_eq!(m.voxel(1, 2, 3), [2, 3, 4]);
    assert_eq!(m.voxel(0, 1, 2), [1, 2, 3]);
    assert_eq!(m.voxel(2, 0, 0), EMPTY);
    assert_eq!(m.voxel(0, -1, 0), EMPTY);
    // A body that is too short leaves the zero-filled buffer.
    let short = Model::from_cub_bytes(&cub_bytes([2, 1, 1], &[[9, 9, 9]]), true);
    assert_eq!(short.voxels, [EMPTY, EMPTY]);
}

#[test]
fn plain_column_with_hollow_interior() {
    let (mut world, mut zone) = setup();
    let rng = world.rng;
    let a = [10, 20, 30];
    let m = model([1, 1, 4], &[a, EMPTY, EMPTY, a]);
    let log = place(&mut world, &mut zone, &m, [X0 + 5, Y0 + 7, 10], 0, 0);
    let (x, y) = (X0 + 5, Y0 + 7);
    // Top to bottom: voxel, hollow, hollow, voxel.
    assert_eq!(
        log,
        [
            LoggedBlock { x, y, z: 13, block: [10, 20, 30, 0x46] },
            LoggedBlock { x, y, z: 12, block: [255, 255, 255, 0xc0] },
            LoggedBlock { x, y, z: 11, block: [255, 255, 255, 0xc0] },
            LoggedBlock { x, y, z: 10, block: [10, 20, 30, 0x46] },
        ]
    );
    assert_eq!(zone.block(x, y, 13), [10, 20, 30, 0x46]);
    assert_eq!(zone.block(x, y, 12), [255, 255, 255, 0xc0]);
    assert_eq!(world.rng, rng, "kind 0 draws no rand()");
}

#[test]
fn rotation_mapping() {
    // Three voxels along x: (1,0,0), (2,0,0), (3,0,0).
    let m = model([3, 1, 1], &[[1, 0, 0], [2, 0, 0], [3, 0, 0]]);
    let (px, py) = (X0 + 50, Y0 + 50);
    let expect = |rot: i32| -> Vec<(i32, i32)> {
        (0..3)
            .map(|mx| match rot {
                0 => (px + mx, py),
                1 => (px, py + (3 - mx) - 1),
                2 => (px + (3 - mx) - 1, py),
                _ => (px, py + mx),
            })
            .collect()
    };
    for rot in 0..4 {
        let (mut world, mut zone) = setup();
        let log = place(&mut world, &mut zone, &m, [px, py, 5], rot, 0);
        let got: Vec<(i32, i32)> = log.iter().map(|l| (l.x, l.y)).collect();
        assert_eq!(got, expect(rot), "rot {rot}");
        for (i, l) in log.iter().enumerate() {
            assert_eq!(l.block, [i as u8 + 1, 0, 0, 0x46]);
        }
    }
}

#[test]
fn margins_crop_in_rotated_order() {
    // 4x4x1 model, margins [left, top, right, bottom] = [1, 0, 0, 0] with rot 1 map to
    // y0 = m[0]: the first model row is skipped.
    let m = model([4, 4, 1], &[[5, 5, 5]; 16]);
    let (mut world, mut zone) = setup();
    let mut log = Vec::new();
    world.place_model_logged(&mut zone, &m, [X0 + 10, Y0 + 10, 3], 1, 6, 0, false, [1, 0, 0, 0], Some(&mut log));
    assert_eq!(log.len(), 12);
    // rot 1: wx = px + my, so my = 0 (wx = X0 + 10) is cropped.
    assert!(log.iter().all(|l| l.x != X0 + 10));
}

#[test]
fn outside_zone_is_skipped() {
    let (mut world, mut zone) = setup();
    let m = model([2, 2, 1], &[[1, 1, 1]; 4]);
    let log = place(&mut world, &mut zone, &m, [X0 - 2, Y0, 3], 0, 0);
    assert!(log.is_empty());
    // Overlapping by one column: all four voxels are logged, only two land in the zone.
    let log = place(&mut world, &mut zone, &m, [X0 - 1, Y0, 3], 0, 0);
    assert_eq!(log.len(), 4);
    assert_eq!(zone.block(X0, Y0, 3), [1, 1, 1, 0x46]);
}

#[test]
fn protected_structure_block_is_kept() {
    let (mut world, mut zone) = setup();
    let (x, y) = (X0 + 3, Y0 + 3);
    cw_world::zone::set_block(&world, &mut zone, x, y, 4, [9, 9, 9, 0x41]);
    let m = model([1, 1, 1], &[[7, 7, 7]]);
    let log = place(&mut world, &mut zone, &m, [x, y, 4], 0, 0);
    assert!(log.is_empty());
    assert_eq!(zone.block(x, y, 4), [9, 9, 9, 0x41]);
    // Structure air (type 0 with 0x40) is overwritten.
    cw_world::zone::set_block(&world, &mut zone, x, y, 4, [0, 0, 0, 0x40]);
    place(&mut world, &mut zone, &m, [x, y, 4], 0, 0);
    assert_eq!(zone.block(x, y, 4), [7, 7, 7, 0x46]);
}

#[test]
fn foundation_fills_down_to_ground() {
    let (mut world, mut zone) = setup();
    let (x, y) = (X0 + 8, Y0 + 9);
    cw_world::zone::set_block(&world, &mut zone, x, y, 0, [100, 100, 100, 1]);
    cw_world::zone::set_block(&world, &mut zone, x, y, 1, [100, 100, 100, 1]);
    let m = model([1, 1, 1], &[[4, 5, 6]]);
    world.place_model(&mut zone, &m, [x, y, 5], 0, 6, 0, true, [0; 4]);
    assert_eq!(zone.block(x, y, 5), [4, 5, 6, 0x46]);
    for h in 2..5 {
        assert_eq!(zone.block(x, y, h), [4, 5, 6, 6], "h {h}");
    }
    assert_eq!(zone.block(x, y, 1), [100, 100, 100, 1]);
}

#[test]
fn kind_8_blue_is_water() {
    let (mut world, mut zone) = setup();
    let m = model([1, 1, 1], &[[0, 0, 255]]);
    let log = place(&mut world, &mut zone, &m, [X0 + 1, Y0 + 1, 3], 0, 8);
    assert_eq!(log[0].block, [255, 255, 255, 2]);
    // Kind 0 writes it as an ordinary voxel.
    let log = place(&mut world, &mut zone, &m, [X0 + 2, Y0 + 1, 3], 0, 0);
    assert_eq!(log[0].block, [0, 0, 255, 0x46]);
}

#[test]
fn red_box_becomes_one_door() {
    let (mut world, mut zone) = setup();
    let rng = world.rng;
    let red = [255, 0, 0];
    // A 2x1x3 red box: one static from its minimum corner, air everywhere.
    let m = model([2, 1, 3], &[red; 6]);
    let (px, py, pz) = (X0 + 20, Y0 + 30, 7);
    let log = place(&mut world, &mut zone, &m, [px, py, pz], 0, 7);
    assert_eq!(log.len(), 6);
    assert!(log.iter().all(|l| l.block == [0, 0, 0, 0x40]));
    let fx = |v: i32| i64::from(v) << 16;
    assert_eq!(
        zone.statics,
        [Static {
            kind: 2,
            x: fx(px) + 2 * 32768,
            y: fx(py) + 32768,
            z: fx(pz),
            rotation: 0,
            scale: [2.0, 1.0, 3.0],
            ..Static::NEW
        }]
    );
    // Rotated by 1 the box lies along y: the long side is reported first with rotation 1.
    let (mut world, mut zone) = setup();
    place(&mut world, &mut zone, &m, [px, py, pz], 1, 1);
    let s = &zone.statics[0];
    assert_eq!((s.kind, s.rotation, s.scale), (1, 1, [2.0, 1.0, 3.0]));
    assert_eq!((s.x, s.y), (fx(px) + 32768, fx(py) + 2 * 32768));
    assert_eq!(world.rng, rng, "doors draw no rand()");
}

#[test]
fn marker_statics_and_rand_order() {
    let (mut world, mut zone) = setup();
    let seed_rng = MsvcRand::new(99);
    world.rng = seed_rng;
    // Column (x = 0): (127,127,0) above (255,127,0); interior test fails for both (2 voxels).
    let m = model([1, 1, 2], &[[255, 127, 0], [127, 127, 0]]);
    let (px, py, pz) = (X0 + 40, Y0 + 41, 12);
    let log = place(&mut world, &mut zone, &m, [px, py, pz], 0, 1);
    assert!(log.is_empty(), "neither voxel is strictly inside the column");
    let mut r = seed_rng;
    let o1 = r.rand() % 4;
    let o2 = r.rand() % 4;
    r.rand();
    assert_eq!(world.rng, r);
    let fx = |v: i32| i64::from(v) << 16;
    let (x, y) = (fx(px), fx(py));
    let kinds: Vec<(u32, i64, i64, i64, i32, [f32; 3])> =
        zone.statics.iter().map(|s| (s.kind, s.x - x, s.y - y, s.z, s.rotation, s.scale)).collect();
    assert_eq!(
        kinds,
        [
            // z = 1 first (top to bottom): (127,127,0)
            (0xc, 32768, 32768, fx(pz + 1), o1, [3.0, 3.0, 1.0]),
            (0x10, 176947, 32768, fx(pz + 1), o2, [1.0, 1.0, 0.5]),
            (0x10, -111411, 32768, fx(pz + 1), o2, [1.0, 1.0, 0.5]),
            // z = 0: (255,127,0)
            (0x13, 32768, 32768, fx(pz), 0, [2.0, 3.0, 1.0]),
            (0x14, 147456, 32768, fx(pz), 0, [1.0, 1.0, 1.0]),
        ]
    );
}

#[test]
fn blue_prop_and_interior_air() {
    let (mut world, mut zone) = setup();
    let solid = [9, 9, 9];
    let m = model([1, 1, 3], &[solid, [0, 0, 255], solid]);
    let (px, py, pz) = (X0 + 60, Y0 + 61, 20);
    let log = place(&mut world, &mut zone, &m, [px, py, pz], 1, 3);
    assert_eq!(log[1], LoggedBlock { x: px, y: py, z: pz + 1, block: [0, 0, 0, 0x40] });
    assert_eq!(zone.props.len(), 1);
    let p = &zone.props[0];
    assert_eq!(p.kind, 0x22);
    assert_eq!(
        (p.x, p.y, p.z),
        ((i64::from(px) << 16) + 32768, (i64::from(py) << 16) + 32768, (i64::from(pz + 1) << 16) - 32768)
    );
    assert_eq!(p.rotation, -180.0);
    assert_eq!(p.scale.to_bits(), 0x3dcccccd);
    assert_eq!((p.f2c, p.flags), ([1.0; 3], 0));
}

/// The PE sections of `Server.exe`: `(virtual address, virtual size, raw offset, raw size)`.
struct Pe {
    bytes: Vec<u8>,
    sections: Vec<(u32, u32, u32, u32)>,
}

impl Pe {
    fn open(path: PathBuf) -> Self {
        let bytes = std::fs::read(path).expect("read Server.exe");
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let pe = u32_at(0x3c) as usize;
        let (count, opt) = (u16_at(pe + 6), u16_at(pe + 20));
        let base = u32_at(pe + 52);
        let sections = (0..count)
            .map(|i| {
                let o = pe + 24 + opt + 40 * i;
                (base + u32_at(o + 12), u32_at(o + 8), u32_at(o + 20), u32_at(o + 16))
            })
            .collect();
        Self { bytes, sections }
    }

    fn offset(&self, va: u32) -> Option<usize> {
        let &(start, _, raw, _) =
            self.sections.iter().find(|&&(start, vs, _, rs)| va >= start && va - start < vs.max(rs))?;
        Some((raw + va - start) as usize)
    }

    fn slice(&self, from: u32, to: u32) -> &[u8] {
        &self.bytes[self.offset(from).unwrap()..self.offset(to).unwrap()]
    }

    fn cstr(&self, va: u32) -> Option<&str> {
        let o = self.offset(va)?;
        let end = self.bytes[o..].iter().position(|&b| b == 0)?;
        std::str::from_utf8(&self.bytes[o..o + end]).ok()
    }
}

#[test]
fn server_exe_model_table() {
    let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
        eprintln!("CW_GAME_DIR not set; skipping test that needs the game files");
        return;
    };
    let pe = Pe::open(dir.join("Server.exe"));
    let (lo, hi) = (0x0043_1400u32, 0x0045_f080u32);
    let code = pe.slice(lo, hi);
    let rel_target =
        |at: usize| (lo + at as u32 + 5).wrapping_add(u32::from_le_bytes(code[at + 1..at + 5].try_into().unwrap()));
    // Every `call Model::load` in Database::load, byte-scanned. The rel32 of a false positive
    // would have to hit the target exactly; the count check below would catch one.
    let calls: Vec<usize> = (0..code.len() - 5).filter(|&i| code[i] == 0xe8 && rel_target(i) == 0x0042_f9a0).collect();
    assert_eq!(calls.len(), MODEL_LOADS.len());
    let mut prev = 0usize;
    for (&at, &(addr, index, key, raw)) in calls.iter().zip(MODEL_LOADS.iter()) {
        assert_eq!(lo + at as u32, addr);
        let window = &code[prev..at];
        prev = at;
        // The key: a `push imm32` of that string (the argument of `operator+`).
        let pushes_key = window
            .windows(5)
            .any(|w| w[0] == 0x68 && pe.cstr(u32::from_le_bytes(w[1..5].try_into().unwrap())) == Some(key));
        assert!(pushes_key, "{addr:#x}: key {key}");
        // The slot: `mov ecx,[ecx+4*i]` (disp32, disp8 or none) or `push i; mov ecx,ebx`.
        let d = u32::from(index) * 4;
        let by_disp = match d {
            0 => window.windows(2).any(|w| w == [0x8b, 0x09]),
            1..=0x7f => window.windows(3).any(|w| w == [0x8b, 0x49, d as u8]),
            _ => window.windows(6).any(|w| w[..2] == [0x8b, 0x89] && w[2..] == d.to_le_bytes()),
        };
        let by_push = window
            .windows(7)
            .any(|w| w[0] == 0x68 && w[1..5] == u32::from(index).to_le_bytes() && w[5..7] == [0x8b, 0xcb])
            || window.windows(4).any(|w| {
                w[0] == 0x6a && u32::from(w[1]) == u32::from(index) && index < 0x80 && w[2..4] == [0x8b, 0xcb]
            });
        assert!(by_disp || by_push, "{addr:#x}: slot {index}");
        // The raw flag: the `push 0/1` right before `push edi; push eax|esi` and the call.
        let flag_push = [0x6a, u8::from(raw)];
        assert!(window.windows(2).any(|w| w == flag_push), "{addr:#x}: raw {raw}");
    }
}

#[test]
fn real_model_table() {
    let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
        eprintln!("CW_GAME_DIR not set; skipping test that needs the game files");
        return;
    };
    let mut table = Vec::new();
    cw_world::model::load_models_from_game_dir(&dir, &mut table).unwrap();
    let db = cw_formats::AssetDb::open(dir.join("data1.db")).unwrap();
    let mut found = 0;
    for &(_, _, key, raw) in MODEL_LOADS.iter() {
        if let Ok(bytes) = db.get(key) {
            let cub = cw_formats::CubModel::parse(&bytes).unwrap();
            assert_eq!(Model::from_cub(&cub, raw), Model::from_cub_bytes(&bytes, raw), "{key}");
            found += 1;
        }
    }
    assert_eq!(found, MODEL_LOADS.len() - 21);
    assert_eq!(table.iter().filter(|m| m.voxels.is_empty()).count(), 19 + 21);
    assert_eq!(table[0x84b].size, [10, 10, 4]);
    assert_eq!(table[2300].size, [4, 8, 9]);
    assert_eq!(table[2544].size, [10, 1, 10]);
}
