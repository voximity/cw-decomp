//! The player creature the handshake creates (`acceptLoop 0x004258b9..0x00425a91`, note 2.2
//! step 7), as the entity data block the Join packet carries.

use cw_math::MsvcRand;
use cw_net::EntityData;
use cw_net::entity::ITEM_SIZE;
use cw_sim::stats::max_hp;
use cw_world::World;
use cw_world::appearance::Appearance;
use cw_world::zone::Spawn;

/// Entity type of a player creature.
pub const PLAYER_TYPE: u32 = 0x2c;

/// Fills the block for a new player, in the original's order of writes. The appearance roll
/// draws from `rng`, the accept thread's `rand` stream (never seeded in the original, so it
/// starts at 1 in every process).
pub fn new_player_entity(world: &mut World, rng: &mut MsvcRand) -> EntityData {
    let mut e = EntityData::new_creature();
    e.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&Appearance::NEW.to_bytes());
    // Position: `__ftol2(spawn_f32 * 65536.0f)` per axis (single multiply, x87 truncation).
    let spawn = [world.spawn[0], world.spawn[1], 0.0f32];
    for (i, s) in spawn.iter().enumerate() {
        let v = (*s * 65536.0f32) as i64;
        e.0[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    // Appearance scale (0.8, 0.8, 1.8) * 1.5 in single precision (overwritten by the
    // appearance roll below, as in the original), velocities zero, physics flags zero, level 1.
    for (i, s) in [0.8f32 * 1.5f32, 0.8f32 * 1.5f32, 1.8f32 * 1.5f32].iter().enumerate() {
        e.0[0x70 + 4 * i..0x74 + 4 * i].copy_from_slice(&s.to_le_bytes());
    }
    e.0[0x18..0x30].fill(0);
    e.0[0x4c..0x50].fill(0);
    e.0[0x180..0x184].copy_from_slice(&1i32.to_le_bytes());
    // HP from maxHp while the hostile type is still 3, the class 0 and the slots empty.
    let hp = max_hp(&e);
    e.0[0x15c..0x160].copy_from_slice(&hp.to_le_bytes());
    e.0[0x50] = 0;
    e.0[0x54..0x58].copy_from_slice(&PLAYER_TYPE.to_le_bytes());
    let pos: [u8; 24] = e.0[0..24].try_into().unwrap();
    e.0[0x1b0..0x1c8].copy_from_slice(&pos);
    // Equipment slots 7 then 6: material 1, type 3 sub-type 2 (levels stay 0 after the reset).
    for slot in [7usize, 6] {
        let o = 0x2f0 + slot * ITEM_SIZE;
        e.0[o + 0xd] = 1;
        e.0[o] = 3;
        e.0[o + 1] = 2;
    }
    e.0[0x130] = 1;
    // initAppearance(&type, &appearance, 0) with the accept thread's rand stream.
    let mut spawn_rec = Spawn { entity_type: PLAYER_TYPE as i32, ..Spawn::NEW };
    std::mem::swap(&mut world.rng, rng);
    world.init_appearance(&mut spawn_rec);
    std::mem::swap(&mut world.rng, rng);
    e.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&spawn_rec.appearance.to_bytes());
    e
}
