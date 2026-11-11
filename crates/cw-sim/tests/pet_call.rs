//! T, the pet call (Interact 8, 0x00536459): the pet forgets its threat and the player its hits
//! and last target, so the pet leaves the fight; a pet more than 200 blocks away is put on the
//! player. In the client's world it does nothing.

use std::collections::{BTreeMap, BTreeSet};

use cw_net::packet::Interact;
use cw_net::{EntityData, ServerUpdate};
use cw_sim::combat::CreatureState;
use cw_sim::interact::handle_interacts;
use cw_world::World;

const PLAYER: i64 = 1;
const PET: i64 = -1;
const ENEMY: i64 = 7;

// Hostility bytes (entity+0x50).
const PLAYER_HOSTILITY: u8 = 0;
const MONSTER: u8 = 1;
const PET_HOSTILITY: u8 = 5;

fn wf32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn set_pos(e: &mut EntityData, blocks: [f64; 3]) {
    for (i, v) in blocks.iter().enumerate() {
        e.0[i * 8..i * 8 + 8].copy_from_slice(&((v * 65536.0) as i64).to_le_bytes());
    }
}

fn pos(e: &EntityData) -> [f64; 3] {
    [0, 1, 2].map(|i| i64::from_le_bytes(e.0[i * 8..i * 8 + 8].try_into().unwrap()) as f64 / 65536.0)
}

fn creature(hostile: u8, at: [f64; 3]) -> EntityData {
    let mut e = EntityData::constructed();
    e.0[0x50] = hostile;
    set_pos(&mut e, at);
    wf32(&mut e.0, 0x15c, 100.0);
    e
}

struct Scene {
    world: World,
    entities: BTreeMap<i64, EntityData>,
    states: BTreeMap<i64, CreatureState>,
}

impl Scene {
    /// A player at (1000, 1000, 50) with a pet `pet_dist` blocks east of it, fighting an enemy
    /// beside it that the player hit.
    fn new(pet_dist: f64) -> Scene {
        let mut entities = BTreeMap::new();
        let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
        entities.insert(PLAYER, creature(PLAYER_HOSTILITY, [1000.0, 1000.0, 50.0]));
        let mut pet = creature(PET_HOSTILITY, [1000.0 + pet_dist, 1000.0, 50.0]);
        pet.0[0x188..0x190].copy_from_slice(&PLAYER.to_le_bytes());
        entities.insert(PET, pet);
        entities.insert(ENEMY, creature(MONSTER, [1001.0 + pet_dist, 1000.0, 50.0]));
        let st = states.entry(PLAYER).or_default();
        st.pet = PET;
        st.hits_landed.insert(ENEMY, 3.0);
        st.last_target = ENEMY;
        states.entry(PET).or_default().threat.insert(ENEMY, 2.0);
        Scene { world: World::new(1), entities, states }
    }

    fn press_t(&mut self) {
        let mut it = [0u8; Interact::SIZE];
        it[0x128] = 8;
        let mut out = ServerUpdate::default();
        let mut dirty = BTreeSet::new();
        handle_interacts(&mut self.world, &mut self.entities, &mut self.states, PLAYER, &[Interact(it)], &mut out, &mut dirty, false);
    }

    fn pet_pos(&self) -> [f64; 3] {
        pos(&self.entities[&PET])
    }

    fn fighting(&self) -> bool {
        let st = &self.states[&PLAYER];
        !st.hits_landed.is_empty() || st.last_target != 0 || !self.states[&PET].threat.is_empty()
    }
}

#[test]
fn t_calls_the_pet_off_the_fight() {
    let mut s = Scene::new(50.0);
    s.press_t();
    assert!(!s.fighting());
    // It walks back: no jump at this distance.
    assert_eq!(s.pet_pos(), [1050.0, 1000.0, 50.0]);
}

#[test]
fn t_brings_a_pet_from_past_200_blocks() {
    let mut s = Scene::new(200.5);
    s.press_t();
    assert!(!s.fighting());
    assert_eq!(s.pet_pos(), [1000.0, 1000.0, 50.0]);
}

#[test]
fn a_pet_200_blocks_away_walks() {
    let mut s = Scene::new(200.0);
    s.press_t();
    assert_eq!(s.pet_pos(), [1200.0, 1000.0, 50.0]);
}

#[test]
fn a_dead_pet_is_brought_too() {
    let mut s = Scene::new(300.0);
    wf32(&mut s.entities.get_mut(&PET).unwrap().0, 0x15c, 0.0);
    s.press_t();
    assert_eq!(s.pet_pos(), [1000.0, 1000.0, 50.0]);
}

/// The first spawn of zone (0, 0) has id 0, which is also "no pet".
#[test]
fn without_a_pet_the_creature_with_id_0_stays() {
    let mut s = Scene::new(300.0);
    s.states.get_mut(&PLAYER).unwrap().pet = 0;
    s.entities.insert(0, creature(MONSTER, [1300.0, 1000.0, 50.0]));
    s.press_t();
    assert_eq!(pos(&s.entities[&0]), [1300.0, 1000.0, 50.0]);
    assert!(s.states[&PLAYER].hits_landed.is_empty());
}

#[test]
fn the_clients_world_leaves_it_to_the_server() {
    let mut s = Scene::new(300.0);
    s.world.is_client = true;
    s.press_t();
    assert!(s.fighting());
    assert_eq!(s.pet_pos(), [1300.0, 1000.0, 50.0]);
}
