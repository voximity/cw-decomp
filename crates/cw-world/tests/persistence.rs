//! The play-time persistence paths (`cw_world::persistence`): the generation thread's unloading
//! pass (`unloadZone`, `unloadRegion` with `saveEntities`, `removeRegion`), the tick's block
//! writes into `zone+0x68`, and the saves of `World::~World`, replayed over the save scenario of
//! `tools/oracle/save_scenario.py` (`tests/save.rs`): the blobs the unloading pass writes must
//! be the ones the original's `saveZone`/`saveEntities` wrote, and a zone generated again from
//! them must come back as it was.

mod common;

use common::{rows, serialize_zone, world_with_models};
use cw_formats::SaveDb;
use cw_world::World;
use cw_world::inventory::Item;
use cw_world::save::{BlobWriter, MissionState, ModifiedBlock, MonsterState, ZoneBlob, ground_items_equal, mission_key, monster_key, zone_key};
use cw_world::zone::{GroundItem, Zone};
use sha2::{Digest, Sha256};

const SEED: i32 = 26879;
const DAY: i32 = 10;
const ZONE_B: (i32, i32) = (32768, 32768);
const ZONE_C: (i32, i32) = (32769, 32768);
const CELL_C: (i32, i32) = (4096, 4096);
const CELL_OTHER: (i32, i32) = (4096, 4097);
const PROP_SUPPORT: (i32, i32, i32) = (8388608, 8388799, -64);

fn golden(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/save/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn fixed(block: i32, half: bool) -> i64 {
    (i64::from(block) << 16) + if half { 32768 } else { 0 }
}

fn item(item_type: u8, sub_type: u8, level: u16, material: u8) -> Item {
    Item { item_type, sub_type, level, material, ..Item::NEW }
}

/// The crafted `zone32768_32768` blob of the scenario (the same bytes as `tests/save.rs`
/// builds; `crafted_zone_B.blob` pins them).
fn crafted_zone_b() -> Vec<u8> {
    let bx = ZONE_B.0 * 256;
    let mut z = Zone::new(ZONE_B.0, ZONE_B.1);
    z.dirty = true;
    z.items = vec![
        GroundItem { item: item(1, 1, 5, 0), x: fixed(bx + 100, true), y: fixed(bx + 100, true), z: fixed(120, false), rotation: 1.5, f134: 0.1, b138: 2, f13c: 0, f140: 0, f144: -1 },
        GroundItem { item: item(0xb, 0x13, 3, 9), x: fixed(bx + 101, true), y: fixed(bx + 100, true), z: fixed(121, false), rotation: 0.0, f134: 0.1, b138: 0, f13c: 5, f140: 6, f144: 9 },
        GroundItem { item: item(0xc, 0, 1, 10), x: fixed(bx + 102, true), y: fixed(bx + 100, true), z: fixed(122, false), rotation: 2.5, f134: 0.1, b138: 2, f13c: 0, f140: 0, f144: 5 },
    ];
    z.modified = vec![
        ModifiedBlock { x: PROP_SUPPORT.0, y: PROP_SUPPORT.1, z: PROP_SUPPORT.2, block: [0, 0, 0, 0], time: -1 },
        ModifiedBlock { x: bx + 92, y: bx + 92, z: 120, block: [200, 100, 50, 0x01], time: 8 },
        ModifiedBlock { x: bx + 42, y: bx + 42, z: 90, block: [1, 2, 3, 0x06], time: 2 },
    ];
    let mut w = BlobWriter::new();
    w.bytes(&z.save_blob());
    let mut out = w.into_bytes();
    out.truncate(out.len() - 8);
    out.extend_from_slice(&8u32.to_le_bytes());
    for i in 0..8u32 {
        out.extend_from_slice(&(3 * i + 1).to_le_bytes());
        out.extend_from_slice(&(7 * i + 2).to_le_bytes());
    }
    out.extend_from_slice(&1u32.to_le_bytes());
    out.push(7);
    out
}

fn crafted_mission() -> MissionState {
    MissionState { f30: 1, f34: 1, f38: 2, f3c: 3, b40: 4, b41: 0, f44: 5, f48: 6, f4c: 7 }
}

fn crafted_monster() -> MonsterState {
    MonsterState { kind: 0x68, level: 12, b5c: 1, zone: MonsterState::pack_zone(ZONE_C.0, ZONE_C.1) }
}

/// Item padding bytes are heap garbage in the original's blobs (see `tests/save.rs`).
fn mask_item_padding(blob: &[u8]) -> Vec<u8> {
    let mut out = blob.to_vec();
    let n = u32::from_le_bytes(blob[4..8].try_into().unwrap()) as usize;
    let record = Item::SIZE + 24 + 4 + 4 + 1 + 12;
    for i in 0..n {
        for off in [2, 3, 0xf, 0x12, 0x13] {
            out[8 + i * record + off] = 0;
        }
    }
    out
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn scenario_world() -> World {
    let db = SaveDb::in_memory().unwrap();
    db.put(&zone_key(ZONE_B.0, ZONE_B.1), &crafted_zone_b()).unwrap();
    db.put(&mission_key(CELL_C.0, CELL_C.1), &crafted_mission().to_blob()).unwrap();
    db.put(&monster_key(CELL_C.0, CELL_C.1), &crafted_monster().to_blob()).unwrap();
    let mut world = world_with_models(SEED);
    world.day = DAY;
    world.attach_save(db);
    world
}

/// The height of the topmost solid block of a column.
fn top_solid(zone: &Zone, bx: i32, by: i32) -> i32 {
    (-100..400).rev().find(|&z| zone.block(bx, by, z)[3] & 0x1f != 0).expect("a solid block")
}

#[test]
fn unloading_saves_what_the_original_saved_and_generation_restores_it() {
    let expected: std::collections::HashMap<String, String> = rows(include_str!("golden/save/dumps.txt")).into_iter().map(|r| (r[0].to_string(), r[3].to_string())).collect();
    let mut world = scenario_world();

    // The scenario's zones B and C, generated into the world (the generation thread's path).
    world.generate_zone(ZONE_B.0, ZONE_B.1);
    world.generate_zone(ZONE_C.0, ZONE_C.1);
    let zone_b = world.zone(ZONE_B.0, ZONE_B.1).expect("zone B").clone();
    assert_eq!(sha(&serialize_zone(&zone_b)), expected["zone_B"], "zone B dump");
    assert!(zone_b.dirty && zone_b.items.len() == 2 && zone_b.modified.len() == 2);
    // The scenario marks zone C dirty before saving it.
    world.zone_mut(ZONE_C.0, ZONE_C.1).unwrap().dirty = true;

    // A player standing in zone B keeps both zones, their regions and their points.
    let report = world.unload_idle(&[ZONE_B]);
    assert!(report.is_empty(), "{report:?}");
    assert!(world.saved_blob(&mission_key(CELL_C.0, CELL_C.1)).unwrap() == crafted_mission().to_blob());

    // A player four zones away: both zones go (saved), the regions stay (same region).
    let report = world.unload_idle(&[(ZONE_B.0 + 4, ZONE_B.1)]);
    assert_eq!(report.zones, vec![ZONE_B, ZONE_C], "table order: zx outer");
    assert!(report.regions.is_empty() && report.points.is_empty());
    let saved_b = world.saved_blob(&zone_key(ZONE_B.0, ZONE_B.1)).unwrap();
    assert_eq!(mask_item_padding(&saved_b), mask_item_padding(&golden("saved_zone_B.blob")), "zone B save blob");
    let saved_c = world.saved_blob(&zone_key(ZONE_C.0, ZONE_C.1)).unwrap();
    assert_eq!(mask_item_padding(&saved_c), mask_item_padding(&golden("saved_zone_C.blob")), "zone C save blob");

    // A player three regions away: region 512,512 is unloaded with saveEntities (its cells were
    // loaded from the database, +0x15a18), its climate point stays (within four).
    let report = world.unload_idle(&[(ZONE_B.0 + 3 * 64, ZONE_B.1)]);
    assert!(report.regions.contains(&(512, 512)), "{report:?}");
    assert!(!report.points.contains(&(512, 512)));
    assert!(world.region(512, 512).is_none() && world.climate_point(512, 512).is_some());
    for (name, key) in [("saved_mission_C.blob", mission_key(CELL_C.0, CELL_C.1)), ("saved_monster_C.blob", monster_key(CELL_C.0, CELL_C.1)), ("saved_mission_other.blob", mission_key(CELL_OTHER.0, CELL_OTHER.1))] {
        assert_eq!(world.saved_blob(&key).unwrap(), golden(name), "{name}");
    }

    // No players: everything left goes, climate points of the regions still loaded included.
    let report = world.unload_idle(&[]);
    assert!(world.loaded_regions().is_empty());
    assert!(report.points.iter().all(|&(rx, ry)| world.climate_point(rx, ry).is_none()));

    // Zone B generated again from its saved blob comes back as the scenario's zone B, and its
    // region re-reads the saved cells.
    world.generate_zone(ZONE_B.0, ZONE_B.1);
    let again = world.zone(ZONE_B.0, ZONE_B.1).unwrap().clone();
    assert_eq!(sha(&serialize_zone(&again)), expected["zone_B"], "zone B regenerated from its save");
    assert!(ground_items_equal(&again.items, &zone_b.items));
    assert_eq!(again.modified, zone_b.modified);
    assert_eq!(again.statics[0].b30, 7);
    let region = world.region(512, 512).expect("region re-created");
    assert!(region.missions_active);
    assert_eq!(world.cell(CELL_C.0, CELL_C.1).unwrap().mission, crafted_mission());

    // Play: a destroyed block (the tick's block action) and an opened static, then the saves of
    // ~World; the zone generated once more has the block cleared and the static byte back.
    // A land column (at or below z 0 `setBlock` turns air into water).
    let (bx, by, bz) = (0..256)
        .flat_map(|i| (0..256).map(move |j| (ZONE_B.0 * 256 + i, ZONE_B.1 * 256 + j)))
        .map(|(bx, by)| (bx, by, top_solid(&again, bx, by)))
        .find(|&(_, _, bz)| bz >= 1)
        .expect("a land column");
    let write = ModifiedBlock { x: bx, y: by, z: bz, block: [0, 0, 0, 0], time: DAY };
    // A write outside every loaded zone is dropped.
    world.push_block_writes([write, ModifiedBlock { x: 100, y: 100, z: 1, block: [0; 4], time: DAY }]);
    {
        // (The block itself is cleared by `World::clearBlock` in the tick; only the record
        // matters for the save.)
        let z = world.zone_mut(ZONE_B.0, ZONE_B.1).unwrap();
        let open = u8::from(z.statics[0].b30 == 0);
        let _ = z.statics[0].toggle(open);
    }
    let b30 = world.zone(ZONE_B.0, ZONE_B.1).unwrap().statics[0].b30;
    assert_eq!(world.zone(ZONE_B.0, ZONE_B.1).unwrap().modified.last(), Some(&write));
    world.save_all();
    let blob = ZoneBlob::parse(&world.saved_blob(&zone_key(ZONE_B.0, ZONE_B.1)).unwrap(), None, None);
    assert_eq!(blob.blocks.last(), Some(&write));
    assert_eq!(blob.static_bytes, Some(vec![Some(b30)]));
    assert!(world.unload_zone(ZONE_B.0, ZONE_B.1).is_some());
    world.generate_zone(ZONE_B.0, ZONE_B.1);
    let third = world.zone(ZONE_B.0, ZONE_B.1).unwrap();
    assert_eq!(third.block(bx, by, bz)[3] & 0x1f, 0, "destroyed block replayed");
    assert_eq!(third.modified.len(), 3);
    assert_eq!(third.statics[0].b30, b30);
}

/// The unloading decisions without generation (no game directory needed): empty zones in
/// created regions, a dirty one and a clean one.
#[test]
fn unload_pass_decisions() {
    let mut world = World::new(SEED);
    world.attach_save(SaveDb::in_memory().unwrap());
    for (rx, ry) in [(512, 512), (514, 512), (517, 512)] {
        world.create_region(rx, ry);
    }
    let mut dirty = Zone::new(512 * 64 + 5, 512 * 64 + 5);
    dirty.dirty = true;
    world.insert_zone(Box::new(dirty));
    world.insert_zone(Box::new(Zone::new(512 * 64 + 6, 512 * 64 + 5)));
    let mut edited = Zone::new(512 * 64 + 5, 512 * 64 + 6);
    edited.modified.push(ModifiedBlock { x: (512 * 64 + 5) * 256, y: (512 * 64 + 6) * 256, z: 3, block: [0; 4], time: -1 });
    world.insert_zone(Box::new(edited));
    let points_before: Vec<(i32, i32)> = world.loaded_regions();
    assert_eq!(points_before, vec![(512, 512), (514, 512), (517, 512)]);

    // Zone 512*64+5, +7 is at squared distance 1 from the first two and 4 from the third... a
    // player there keeps all three and regions 512 and 514; 517 (five away) loses region and point.
    let player = (512 * 64 + 5, 512 * 64 + 7);
    let report = world.unload_idle(&[player]);
    assert!(report.zones.is_empty(), "{report:?}");
    assert_eq!(report.regions, vec![(517, 512)]);
    assert_eq!(report.points, vec![(517, 512)]);
    assert!(world.climate_point(514, 512).is_some());

    // Block writes land in the zone of `(x / 256, y / 256)`.
    world.push_block_writes([ModifiedBlock { x: (512 * 64 + 6) * 256 + 1, y: (512 * 64 + 5) * 256 + 2, z: 7, block: [0; 4], time: 4 }]);
    assert_eq!(world.zone(512 * 64 + 6, 512 * 64 + 5).unwrap().modified.len(), 1);

    // Far away: every zone is saved only when dirty or edited, then region 512 and 514 go
    // (their missions were never activated, so saveEntities writes nothing).
    // (A player at zone 0,0 would not do: the squared distance wraps in the int arithmetic,
    // as in the original, and comes out negative.)
    let report = world.unload_idle(&[(512 * 64 + 1000, 512 * 64)]);
    assert_eq!(report.zones, vec![(512 * 64 + 5, 512 * 64 + 5), (512 * 64 + 5, 512 * 64 + 6), (512 * 64 + 6, 512 * 64 + 5)]);
    assert_eq!(report.regions, vec![(512, 512), (514, 512)]);
    assert_eq!(report.points, vec![(512, 512), (514, 512)]);
    let keys = world.save.as_ref().unwrap().lock().unwrap().keys().unwrap();
    assert_eq!(keys, vec![zone_key(512 * 64 + 5, 512 * 64 + 5), zone_key(512 * 64 + 5, 512 * 64 + 6), zone_key(512 * 64 + 6, 512 * 64 + 5)]);
    // Climate points of neighbouring regions that were never loaded stay.
    assert!(world.climate_point(513, 512).is_some());
}
