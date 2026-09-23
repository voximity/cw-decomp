//! The held charge attacks of the local player through the shared tick: the charge tick sound
//! (0x37, `charge2.wav`) and the charged MP (`entity+0x134`).

use std::collections::{BTreeMap, BTreeSet};

use cw_net::{EntityData, ServerUpdate};
use cw_sim::combat::CreatureState;
use cw_world::World;

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn wf32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// A class-2 (ranger) player holding a bow (slot 7, type 3 sub type 6), full MP, in `mode`.
fn ranger(mode: u8) -> (World, BTreeMap<i64, EntityData>, BTreeMap<i64, CreatureState>) {
    let mut world = World::new(1);
    world.local_player = Some(1);
    let mut e = EntityData::constructed();
    e.0[0x50] = 0;
    e.0[0x130] = 2;
    e.0[0x131] = 0;
    let w = 0x2f0 + 7 * 0x118;
    e.0[w] = 3;
    e.0[w + 1] = 6;
    wf32(&mut e.0, 0x160, 1.0);
    e.0[0x58] = mode;
    let mut entities = BTreeMap::new();
    entities.insert(1i64, e);
    let mut states = BTreeMap::new();
    states.insert(1i64, CreatureState { stamina: 1.0, ..CreatureState::default() });
    (world, entities, states)
}

/// One 20 ms slice of `World::tick` for the creature: `update_creature` then `move_creature`.
fn slice(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, out: &mut ServerUpdate) {
    let mut projectiles = Vec::new();
    let mut dirty = BTreeSet::new();
    if cw_sim::update::update_creature(world, entities, states, &mut projectiles, &mut dirty, 1, 20, out) {
        cw_sim::physics::move_creature(world, entities, states, 1, 20, out);
    }
}

/// Mode 0x19 (the bow's right button, `Cube.exe 0x0048ba93`): the charge tick of jump-table
/// case 3 (`Cube.exe 0x00615bb3`) plays 0x37 once per 100 ms of mode time, pitched
/// `charged * 0.5 + 1` (0x00615c24..0x00615c40), never more often.
#[test]
fn bow_charge_ticks_every_100_ms() {
    let (mut world, mut entities, mut states) = ranger(0x19);
    let mut pitches = Vec::new();
    for _ in 0..50 {
        let mut out = ServerUpdate::default();
        slice(&mut world, &mut entities, &mut states, &mut out);
        for s in &out.sounds {
            let id = u32::from_le_bytes(s.0[0xc..0x10].try_into().unwrap());
            if id == 0x37 {
                pitches.push(f32_at(&s.0, 0x10));
            }
        }
    }
    assert_eq!(entities[&1].0[0x58], 0x19);
    assert_eq!(pitches.len(), 10, "{pitches:?}");
    assert!(pitches.windows(2).all(|w| w[0] <= w[1]), "{pitches:?}");
    assert!(pitches.iter().all(|p| (1.0..=1.5).contains(p)), "{pitches:?}");
}

/// Mode 0x24 charges at `attackSpeed * dt * 5e-4` (`Cube.exe 0x0061a74d`, constant
/// 0x0071e1dc = 0x3a03126f; `Server.exe 0x0054050d`, 0x00573c80), slower than the
/// `7.5e-4` of the other charge modes (0x0071e1e0).
#[test]
fn mode_24_charges_at_5e_4_per_ms() {
    let (mut world, mut entities, mut states) = ranger(0x24);
    let aspd = cw_sim::skills::attack_speed(&entities[&1], 0.0, false);
    for _ in 0..10 {
        let mut out = ServerUpdate::default();
        slice(&mut world, &mut entities, &mut states, &mut out);
    }
    let charged = f32_at(&entities[&1].0, 0x134);
    let want = aspd * (20.0f32 * 5e-4f32) * 10.0;
    assert!((charged - want).abs() < 1e-4, "charged {charged}, want {want}");
}
