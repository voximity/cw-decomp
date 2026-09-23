//! A killed zone spawn stays away for the kill countdown: the death (`applyHit`, Server.exe
//! 0x004cfc33..0x004cfc5d) writes `spawn+0x38` = 1200000 and `spawn+0x3c` = day for the
//! spawn index at `creature+0x1b8` (entity+0x1a8), the tick takes `dt` off it (0x00533c7b),
//! the corpse goes after 1000 ms (0x00534911), and the spawn pass (0x005359c8..0x005359e3)
//! skips the spawn while the countdown is non-zero on the same day.

use std::collections::{BTreeMap, BTreeSet};

use cw_net::packet::Hit;
use cw_net::{EntityData, ServerUpdate};
use cw_server::server::spawn_creatures_with;
use cw_sim::combat::{CreatureState, apply_hit};
use cw_world::World;
use cw_world::generate::spawn_id;
use cw_world::world::PlayerInfo;
use cw_world::zone::{Spawn, Zone};

const ZX: i32 = 100;
const ZY: i32 = 100;
const DT: i32 = 100;

fn tick(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) {
    let active: BTreeSet<(i32, i32)> = [(ZX, ZY)].into_iter().collect();
    let mut out = ServerUpdate::default();
    let mut dirty = BTreeSet::new();
    // 0x00533b5c..0x00535702, then the spawns 0x00535702.
    cw_sim::statics::creature_pass(world, entities, states, &active, DT, &mut out, &mut dirty, false);
    spawn_creatures_with(world, entities, states, ZX, ZY, false);
}

#[test]
fn a_killed_spawn_returns_after_twenty_minutes_not_before() {
    let mut world = World::new(1);
    let mut z = Zone::new(ZX, ZY);
    for k in 0..4i64 {
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
    // A player among the spawns: within every spawn's radius, and the despawn stays away.
    let ppos = [(i64::from(ZX) * 256 + 12) << 16, (i64::from(ZY) * 256 + 128) << 16, 10 << 16];
    let mut p = EntityData::constructed();
    p.0[0x50] = 0;
    for (i, v) in ppos.iter().enumerate() {
        p.0[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    let mut entities: BTreeMap<i64, EntityData> = BTreeMap::new();
    let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
    entities.insert(1, p);
    world.players = vec![PlayerInfo { id: 1, level: 1, pos: ppos }];

    spawn_creatures_with(&mut world, &mut entities, &mut states, ZX, ZY, false);
    let victim = spawn_id(ZX, ZY, 3);
    for k in 0..4 {
        assert!(entities.contains_key(&spawn_id(ZX, ZY, k)), "spawn {k}");
    }

    let mut h = [0u8; 0x48];
    h[0..8].copy_from_slice(&1i64.to_le_bytes());
    h[8..16].copy_from_slice(&victim.to_le_bytes());
    h[0x10..0x14].copy_from_slice(&1.0e6f32.to_le_bytes());
    let mut out = ServerUpdate::default();
    let mut dirty = BTreeSet::new();
    apply_hit(&mut world, &mut entities, &mut states, &Hit(h), &mut out, &mut dirty, false);

    // 1200000 ms of 100 ms ticks: the countdown reaches 0 on tick 12000 and the spawn pass of
    // that tick makes the creature again.
    let n = 1_200_000 / DT;
    for t in 1..=n {
        tick(&mut world, &mut entities, &mut states);
        let alive = entities.contains_key(&victim);
        if t < 1000 / DT {
            assert!(alive, "the corpse stays for a second (tick {t})");
        } else if t < n {
            assert!(!alive, "respawned early at tick {t} ({} ms)", t * DT);
        } else {
            assert!(alive, "not respawned after the countdown");
        }
        for k in 0..3 {
            assert!(entities.contains_key(&spawn_id(ZX, ZY, k)), "spawn {k} at tick {t}");
        }
    }
    let e = &entities[&victim];
    assert!(f32::from_le_bytes(e.0[0x15c..0x160].try_into().unwrap()) > 0.0);
}
