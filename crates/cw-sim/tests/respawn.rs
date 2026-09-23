//! The death-to-respawn sequence of a zone spawn: the spawn pass (`World::tick`, Server.exe
//! 0x00535e7a..0x00535eba) gives the creature its home `(zone x, zone y, spawn index)` at
//! `entity+0x1a0..0x1ac`, the death in `applyHit` (0x004cf0a3 onward) starts the kill
//! countdown (`spawn+0x38` = 1200000 ms, `spawn+0x3c` = day) of that spawn, and the dead
//! creatures pass (0x00534911) deletes the corpse 1000 ms after its death.

use std::collections::{BTreeMap, BTreeSet};

use cw_net::packet::Hit;
use cw_net::{EntityData, ServerUpdate};
use cw_sim::combat::{CreatureState, apply_hit};
use cw_sim::creature::entity_from_spawn;
use cw_world::World;
use cw_world::generate::spawn_id;
use cw_world::zone::{Spawn, Zone};

const ZX: i32 = 100;
const ZY: i32 = 100;

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// A world with one zone of four hostile spawns (index `k` at block `(zone * 256 + 8k, 128)`).
fn world_with_spawns() -> World {
    let mut world = World::new(1);
    let mut z = Zone::new(ZX, ZY);
    for k in 0..4 {
        let mut s = Spawn::NEW;
        s.x = (i64::from(ZX) * 256 + 8 * k) << 16;
        s.y = (i64::from(ZY) * 256 + 128) << 16;
        s.z = 10 << 16;
        let id = spawn_id(ZX, ZY, k as i32);
        s.f38[4] = id as u32;
        s.f38[5] = (id >> 32) as u32;
        z.spawns.push(s);
    }
    world.insert_zone(Box::new(z));
    world
}

fn lethal_hit(attacker: i64, target: i64) -> Hit {
    let mut h = [0u8; 0x48];
    h[0..8].copy_from_slice(&attacker.to_le_bytes());
    h[8..16].copy_from_slice(&target.to_le_bytes());
    h[0x10..0x14].copy_from_slice(&1.0e6f32.to_le_bytes());
    Hit(h)
}

/// 0x00535e7a..0x00535eba: `(zone x, zone y, loop index)` built by 0x00402990 and copied by
/// 0x00401060 into `creature+0x1b0` (entity+0x1a0): the third word is the spawn's index.
#[test]
fn a_spawned_creature_knows_its_spawn_index() {
    let world = world_with_spawns();
    let z = world.zone(ZX, ZY).unwrap();
    for k in 0..4 {
        let e = entity_from_spawn(&z.spawns[k], (ZX, ZY), k as i32);
        assert_eq!([i32_at(&e.0, 0x1a0), i32_at(&e.0, 0x1a4), i32_at(&e.0, 0x1a8)], [ZX, ZY, k as i32]);
    }
}

/// The killed creature's own spawn starts the twenty-minute countdown on this day; the
/// other spawns of the zone keep theirs.
#[test]
fn a_kill_starts_the_countdown_of_its_own_spawn() {
    let mut world = world_with_spawns();
    world.day = 7;
    let mut entities: BTreeMap<i64, EntityData> = BTreeMap::new();
    let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
    let victim = spawn_id(ZX, ZY, 3);
    let e = entity_from_spawn(&world.zone(ZX, ZY).unwrap().spawns[3], (ZX, ZY), 3);
    entities.insert(victim, e);
    states.insert(victim, CreatureState::default());
    let mut out = ServerUpdate::default();
    let mut dirty = BTreeSet::new();
    apply_hit(&mut world, &mut entities, &mut states, &lethal_hit(-1, victim), &mut out, &mut dirty, false);
    assert!(f32::from_le_bytes(entities[&victim].0[0x15c..0x160].try_into().unwrap()) <= 0.0);
    let z = world.zone(ZX, ZY).unwrap();
    assert_eq!(z.spawns[3].f38[0], 1_200_000);
    assert_eq!(z.spawns[3].f38[1], 7);
    for k in 0..3 {
        assert_eq!(z.spawns[k].f38[0], 0, "spawn {k}");
    }
}

/// 0x00534911: the corpse stays in the creature map for 1000 ms of mode time, then goes.
#[test]
fn the_corpse_is_removed_after_one_second() {
    let mut world = world_with_spawns();
    let mut entities: BTreeMap<i64, EntityData> = BTreeMap::new();
    let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
    // A player next to the spawns keeps the despawn (0x00533dc3) away.
    let mut p = EntityData::constructed();
    p.0[0x50] = 0;
    let s0 = world.zone(ZX, ZY).unwrap().spawns[0].clone();
    for (i, v) in [s0.x, s0.y, s0.z].iter().enumerate() {
        p.0[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    entities.insert(1, p);
    let victim = spawn_id(ZX, ZY, 2);
    entities.insert(victim, entity_from_spawn(&world.zone(ZX, ZY).unwrap().spawns[2], (ZX, ZY), 2));
    states.insert(victim, CreatureState::default());
    let mut out = ServerUpdate::default();
    let mut dirty = BTreeSet::new();
    apply_hit(&mut world, &mut entities, &mut states, &lethal_hit(1, victim), &mut out, &mut dirty, false);
    let active: BTreeSet<(i32, i32)> = [(ZX, ZY)].into_iter().collect();
    for tick in 1..=10 {
        cw_sim::statics::creature_pass(&mut world, &mut entities, &mut states, &active, 100, &mut out, &mut dirty, false);
        // `modeTime += dt` then `modeTime >= 1000`: gone on the tenth 100 ms tick.
        assert_eq!(entities.contains_key(&victim), tick < 10, "tick {tick}");
    }
    // The countdown lost the ten ticks.
    assert_eq!(world.zone(ZX, ZY).unwrap().spawns[2].f38[0], 1_200_000 - 1000);
}
