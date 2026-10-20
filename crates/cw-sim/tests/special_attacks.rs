//! A creature made from a zone spawn keeps the spawn's special attacks, summon look and summon
//! types (`World::tick` 0x0053613d..0x00536161), so a djinn casts its 0x57 and 0x5c.

use cw_sim::combat::CreatureState;
use cw_world::World;
use cw_world::zone::Spawn;

#[test]
fn a_djinn_keeps_its_special_attacks() {
    let mut world = World::new(1);
    let mut s = Spawn::NEW;
    s.entity_type = 0x51;
    world.init_creature(&mut s);
    assert_eq!(s.v10ac, vec![0x57, 0x5c]);
    let st = CreatureState::from_spawn(&s);
    assert_eq!(st.ai.special_modes, vec![0x57, 0x5c]);
}

#[test]
fn a_creature_keeps_its_summon_setup() {
    let mut s = Spawn::NEW;
    s.f10b8 = 2;
    s.v10bc = vec![0x11, 0x60];
    let st = CreatureState::from_spawn(&s);
    assert_eq!(st.modes.summon_look, 2);
    assert_eq!(st.modes.summon_types, vec![0x11, 0x60]);
}
