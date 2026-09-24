//! Other creatures' melee in the client's world (`world+0xb4` set, `world+0xb8` the local
//! player). Case 0 of the mode machine (`Cube.exe 0x00612357`, `Server.exe 0x00538117`) is
//! guarded `b4 != 0 || hostile != 0 || c == b8`: in the client's world every creature runs it,
//! so another player's swing plays its sounds locally (the server skips players' case 0 and
//! sends none). `creatureAttack` (`Cube.exe 0x00596d9f`, `Server.exe 0x004cfdbf`) then leaves
//! the local player to the server: in the client's world it returns at once for that target.

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

fn w64(b: &mut [u8], o: usize, v: i64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// A creature of `hostile` type at block (x, 0, 0), size 1, in `mode` with mode time 0.
fn creature(hostile: u8, x: i64, mode: u8) -> EntityData {
    let mut e = EntityData::constructed();
    e.0[0x50] = hostile;
    w64(&mut e.0, 0, x << 16);
    e.0[0x58] = mode;
    for o in [0x70, 0x74, 0x78] {
        wf32(&mut e.0, o, 1.0);
    }
    e
}

/// The client's world: the local player 1 idle at the origin, creature 2 `other` next to it.
fn client_world(other: EntityData) -> (World, BTreeMap<i64, EntityData>, BTreeMap<i64, CreatureState>) {
    let mut world = World::new(1);
    world.is_client = true;
    world.local_player = Some(1);
    let mut entities = BTreeMap::new();
    entities.insert(1i64, creature(0, 0, 0));
    entities.insert(2i64, other);
    let mut states = BTreeMap::new();
    states.insert(1i64, CreatureState { stamina: 1.0, ..CreatureState::default() });
    states.insert(2i64, CreatureState { stamina: 1.0, ..CreatureState::default() });
    (world, entities, states)
}

/// `ms` of 20 ms slices of `update_creature` for creature `id`; the sound ids it played.
fn run(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, ms: i32) -> (Vec<u32>, ServerUpdate) {
    let mut projectiles = Vec::new();
    let mut dirty = BTreeSet::new();
    let mut all = ServerUpdate::default();
    for _ in 0..ms / 20 {
        let mut out = ServerUpdate::default();
        cw_sim::update::update_creature(world, entities, states, &mut projectiles, &mut dirty, id, 20, &mut out);
        all.sounds.extend(out.sounds);
        all.hits.extend(out.hits);
    }
    let ids = all.sounds.iter().map(|s| u32::from_le_bytes(s.0[0xc..0x10].try_into().unwrap())).collect();
    (ids, all)
}

/// Another player's basic attack (mode 1) crosses its wind-up: the swing sound 0xf
/// (`blade1.wav`, 0x005384b3 / `Cube.exe 0x006126f3`) plays in the local client's world.
#[test]
fn remote_player_swing_plays_in_client_world() {
    let (mut world, mut entities, mut states) = client_world(creature(0, 1000, 1));
    let (ids, _) = run(&mut world, &mut entities, &mut states, 2, 1000);
    assert!(ids.contains(&0xf), "{ids:x?}");
}

/// The same swing on a server (`world+0xb4` clear, no local player) is not simulated: players
/// run case 0 only in their own client.
#[test]
fn player_swing_is_not_simulated_on_server() {
    let (mut world, mut entities, mut states) = client_world(creature(0, 1000, 1));
    world.is_client = false;
    world.local_player = None;
    let (ids, _) = run(&mut world, &mut entities, &mut states, 2, 1000);
    assert!(!ids.contains(&0xf), "{ids:x?}");
}

/// A monster's swing that reaches the local player in the client's world plays the swing but
/// lands nothing: `creatureAttack` returns for the local player (the server's hit arrives in the
/// received lists), so no Hit and no HP change.
#[test]
fn remote_monster_swing_leaves_local_player_to_server() {
    let (mut world, mut entities, mut states) = client_world(creature(1, 0, 1));
    let hp = f32_at(&entities[&1].0, 0x15c);
    assert!(hp > 0.0);
    wf32(&mut entities.get_mut(&2).unwrap().0, 0x15c, 1000.0);
    let (ids, all) = run(&mut world, &mut entities, &mut states, 2, 1000);
    assert!(ids.contains(&0xf), "{ids:x?}");
    assert!(all.hits.iter().all(|h| i64::from_le_bytes(h.0[8..16].try_into().unwrap()) != 1), "{:?}", all.hits.len());
    assert_eq!(f32_at(&entities[&1].0, 0x15c), hp);
}

/// The local player's own swing, as before: the guard's `c == b8` arm.
#[test]
fn local_player_swing_plays_in_client_world() {
    let (mut world, mut entities, mut states) = client_world(creature(0, 1000, 0));
    entities.get_mut(&1).unwrap().0[0x58] = 1;
    let (ids, _) = run(&mut world, &mut entities, &mut states, 1, 1000);
    assert!(ids.contains(&0xf), "{ids:x?}");
}
