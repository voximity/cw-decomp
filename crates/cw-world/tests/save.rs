//! The save-database scenario of `tools/oracle/save_scenario.py`, replayed through the port and
//! compared with what the original produced (`golden/save/`): the crafted blobs both sides
//! build, the format-6 dumps of the three zones generated with the database attached, and the
//! blobs `World::saveZone` and `World::saveEntities` wrote back.

mod common;

use common::{rows, serialize_zone, world_with_models};
use cw_formats::SaveDb;
use cw_world::inventory::Item;
use cw_world::save::{BlobWriter, MissionState, ModifiedBlock, MonsterState, mission_key, monster_key, zone_key};
use cw_world::zone::{GroundItem, Zone};
use sha2::{Digest, Sha256};

const SEED: i32 = 26879;
const DAY: i32 = 10;
const ZONE_B: (i32, i32) = (32768, 32768);
const ZONE_C: (i32, i32) = (32769, 32768);
const ZONE_A: (i32, i32) = (40000, 20000);
const CELL_C: (i32, i32) = (4096, 4096);
const CELL_OTHER: (i32, i32) = (4096, 4097);
const REGION: (i32, i32) = (512, 512);
const PROP_SUPPORT: (i32, i32, i32) = (8388608, 8388799, -64);
const ZONE_B_SPAWNS: usize = 8;
const ZONE_B_STATICS: usize = 1;

fn golden(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/save/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn item(item_type: u8, sub_type: u8, level: u16, material: u8) -> Item {
    Item { item_type, sub_type, level, material, ..Item::NEW }
}

fn fixed(block: i32, half: bool) -> i64 {
    (i64::from(block) << 16) + if half { 32768 } else { 0 }
}

/// The three ground items of the crafted zone blob (the third has expired on day 10).
fn crafted_items() -> Vec<GroundItem> {
    let bx = ZONE_B.0 * 256;
    vec![
        GroundItem { item: item(1, 1, 5, 0), x: fixed(bx + 100, true), y: fixed(bx + 100, true), z: fixed(120, false), rotation: 1.5, f134: 0.1, b138: 2, f13c: 0, f140: 0, f144: -1 },
        GroundItem { item: item(0xb, 0x13, 3, 9), x: fixed(bx + 101, true), y: fixed(bx + 100, true), z: fixed(121, false), rotation: 0.0, f134: 0.1, b138: 0, f13c: 5, f140: 6, f144: 9 },
        GroundItem { item: item(0xc, 0, 1, 10), x: fixed(bx + 102, true), y: fixed(bx + 100, true), z: fixed(122, false), rotation: 2.5, f134: 0.1, b138: 2, f13c: 0, f140: 0, f144: 5 },
    ]
}

/// The three modified blocks (the third has expired; the first clears the support of the
/// zone's first flag-2 prop).
fn crafted_blocks() -> Vec<ModifiedBlock> {
    let bx = ZONE_B.0 * 256;
    vec![
        ModifiedBlock { x: PROP_SUPPORT.0, y: PROP_SUPPORT.1, z: PROP_SUPPORT.2, block: [0, 0, 0, 0], time: -1 },
        ModifiedBlock { x: bx + 92, y: bx + 92, z: 120, block: [200, 100, 50, 0x01], time: 8 },
        ModifiedBlock { x: bx + 42, y: bx + 42, z: 90, block: [1, 2, 3, 0x06], time: 2 },
    ]
}

/// The crafted `zone32768_32768` blob, built with the port's writer in the original's layout.
fn crafted_zone_b() -> Vec<u8> {
    let mut z = Zone::new(ZONE_B.0, ZONE_B.1);
    z.dirty = true;
    z.items = crafted_items();
    z.modified = crafted_blocks();
    let mut w = BlobWriter::new();
    w.bytes(&z.save_blob());
    // save_blob wrote empty spawn and static sections; replace them with the crafted ones.
    let mut out = w.into_bytes();
    out.truncate(out.len() - 8);
    out.extend_from_slice(&(ZONE_B_SPAWNS as u32).to_le_bytes());
    for i in 0..ZONE_B_SPAWNS as u32 {
        out.extend_from_slice(&(3 * i + 1).to_le_bytes());
        out.extend_from_slice(&(7 * i + 2).to_le_bytes());
    }
    out.extend_from_slice(&(ZONE_B_STATICS as u32).to_le_bytes());
    out.extend_from_slice(&[7u8; ZONE_B_STATICS]);
    out
}

fn crafted_mission() -> MissionState {
    MissionState { f30: 1, f34: 1, f38: 2, f3c: 3, b40: 4, b41: 0, f44: 5, f48: 6, f4c: 7 }
}

fn crafted_monster() -> MonsterState {
    MonsterState { kind: 0x68, level: 12, b5c: 1, zone: MonsterState::pack_zone(ZONE_C.0, ZONE_C.1) }
}

/// Zeroes the bytes of every item record in a zone blob that the item constructor never
/// writes (`+2`, `+3`, `+0xf`, `+0x12`, `+0x13`). They are structure padding: the original's
/// member-wise item copies (into the zone's item vector) leave them as whatever the heap held,
/// so even items loaded from a zeroed blob come back with garbage there.
fn mask_item_padding(blob: &[u8]) -> Vec<u8> {
    let mut out = blob.to_vec();
    let n = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;
    let record = Item::SIZE + 24 + 4 + 4 + 1 + 12;
    for i in 0..n {
        let base = 8 + i * record;
        for off in [2, 3, 0xf, 0x12, 0x13] {
            out[base + off] = 0;
        }
    }
    out
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn crafted_blobs_match_the_scenario_bytes() {
    assert_eq!(crafted_zone_b(), golden("crafted_zone_B.blob"));
    assert_eq!(crafted_mission().to_blob(), golden("crafted_mission_C.blob"));
    assert_eq!(crafted_monster().to_blob(), golden("crafted_monster_C.blob"));
}

#[test]
fn save_scenario_matches_the_original() {
    let expected: std::collections::HashMap<String, String> = rows(include_str!("golden/save/dumps.txt")).into_iter().map(|r| (r[0].to_string(), r[3].to_string())).collect();

    let db = SaveDb::in_memory().unwrap();
    db.put(&zone_key(ZONE_B.0, ZONE_B.1), &crafted_zone_b()).unwrap();
    db.put(&mission_key(CELL_C.0, CELL_C.1), &crafted_mission().to_blob()).unwrap();
    db.put(&monster_key(CELL_C.0, CELL_C.1), &crafted_monster().to_blob()).unwrap();
    let mut world = world_with_models(SEED);
    world.day = DAY;
    world.attach_save(db);

    let mut errors = Vec::new();
    let mut generate = |world: &mut cw_world::World, name: &str, (zx, zy): (i32, i32)| -> Zone {
        let mut snapshot = None;
        world.generate_zone_stages(zx, zy, &mut |stage, zone| {
            if stage == cw_world::generate::Stage::Full {
                snapshot = Some(zone.clone());
            }
        });
        let zone = snapshot.expect("full stage");
        let got = sha(&serialize_zone(&zone));
        if got != expected[name] {
            errors.push(format!("{name} {zx},{zy}: dump {got} vs {}", expected[name]));
        }
        zone
    };

    // Zone B: the crafted blob is applied at the end of generation.
    let zone_b = generate(&mut world, "zone_B", ZONE_B);
    assert!(zone_b.dirty, "zone B must be dirty after loading items it did not generate");
    assert_eq!(zone_b.items.len(), 2, "one crafted item expired");
    assert_eq!(zone_b.modified.len(), 2, "one crafted block expired");
    assert!(zone_b.spawns.iter().enumerate().all(|(i, s)| s.f38[0] == 3 * i as u32 + 1 && s.f38[1] == 7 * i as u32 + 2));
    assert_eq!(zone_b.statics[0].b30, 7);
    assert!(world.save_zone(&zone_b).unwrap());
    let saved_b = world.saved_blob(&zone_key(ZONE_B.0, ZONE_B.1)).unwrap();
    assert_eq!(mask_item_padding(&saved_b), mask_item_padding(&golden("saved_zone_B.blob")), "zone B save blob");

    // Zone C: the saved boss of the shared cell, then saveEntities on the region.
    let mut zone_c = generate(&mut world, "zone_C", ZONE_C);
    assert_eq!(zone_c.spawns.last().map(|s| (s.entity_type, s.level, s.b58)), Some((0x68, 12, 1)), "boss spawn");
    zone_c.dirty = true;
    assert!(world.save_zone(&zone_c).unwrap());
    let saved_c = world.saved_blob(&zone_key(ZONE_C.0, ZONE_C.1)).unwrap();
    assert_eq!(mask_item_padding(&saved_c), mask_item_padding(&golden("saved_zone_C.blob")), "zone C save blob");
    assert!(world.save_region_entities(REGION.0, REGION.1).unwrap());
    for (name, key) in [
        ("saved_mission_C.blob", mission_key(CELL_C.0, CELL_C.1)),
        ("saved_monster_C.blob", monster_key(CELL_C.0, CELL_C.1)),
        ("saved_mission_other.blob", mission_key(CELL_OTHER.0, CELL_OTHER.1)),
    ] {
        assert_eq!(world.saved_blob(&key).unwrap(), golden(name), "{name}");
    }
    // A cell without a monster serialises whatever the heap held at +0x60 (the cell constructor
    // never writes it), so only the initialised fields of that blob are compared.
    let other = world.saved_blob(&monster_key(CELL_OTHER.0, CELL_OTHER.1)).unwrap();
    assert_eq!(other[..13], golden("saved_monster_other.blob")[..13], "saved_monster_other.blob");
    assert_eq!(other.len(), 21);

    // Zone A: generated ground items serialised (their constructor padding masked).
    let mut zone_a = generate(&mut world, "zone_A", ZONE_A);
    assert_eq!(zone_a.items.len(), 5);
    zone_a.dirty = true;
    assert!(world.save_zone(&zone_a).unwrap());
    let saved_a = world.saved_blob(&zone_key(ZONE_A.0, ZONE_A.1)).unwrap();
    assert_eq!(mask_item_padding(&saved_a), mask_item_padding(&golden("saved_zone_A.blob")), "zone A save blob");

    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
