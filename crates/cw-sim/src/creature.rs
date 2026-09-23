//! A creature from a zone spawn, as `World::tick` makes one (`Server.exe
//! 0x00535d7a..0x005361ed`): the entity block the Creature constructor leaves, the spawn's
//! fields copied over it, the HP from `Creature::maxHp`, a boss's doubled scale and the lift by
//! half its height. The creature-only fields (behaviours, schedule, inventory) have no place in
//! the entity block.

use cw_net::EntityData;
use cw_net::entity::ITEM_SIZE;
use cw_world::appearance::Appearance;
use cw_world::zone::Spawn;

use crate::stats::max_hp;
use crate::util::{f32_at};

/// The entity block the creature spawn `s`, index `index` of zone `zone`'s spawn list, becomes.
pub fn entity_from_spawn(s: &Spawn, zone: (i32, i32), index: i32) -> EntityData {
    let mut e = EntityData::constructed();
    {
        let b = &mut e.0;
        let w32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let w64 = |b: &mut [u8], o: usize, v: i64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
        w64(b, 0, s.x);
        w64(b, 8, s.y);
        w64(b, 16, s.z);
        // Yaw (the third rotation float) from the spawn's rotation.
        w32(b, 0x20, s.rotation.to_bits());
        w32(b, 0x180, s.level as u32);
        b[0x50] = s.f28 as u8;
        w32(b, 0x54, s.entity_type as u32);
        b[0x130] = s.f30 as u8;
        b[0x131] = (s.f30 >> 8) as u8;
        b[0x198] = s.b58;
        b[0x68..0x68 + Appearance::SIZE].copy_from_slice(&s.appearance.to_bytes());
        for (k, it) in s.equipment.iter().enumerate() {
            let o = 0x2f0 + k * ITEM_SIZE;
            b[o..o + ITEM_SIZE].copy_from_slice(&it.to_bytes());
        }
        for (i, v) in [s.f_f58, s.f_f5c, s.f_f60, s.f_f64, s.f_f68].iter().enumerate() {
            w32(b, 0x168 + 4 * i, v.to_bits());
        }
        let pos: [u8; 24] = b[0..24].try_into().unwrap();
        b[0x1b0..0x1c8].copy_from_slice(&pos);
        // Flag bit 8 of the u16 at +0x104 follows the spawn's +0x50 byte (0x00405570).
        let mut flags = u16::from_le_bytes([b[0x104], b[0x105]]);
        flags = if s.b50 != 0 { flags | 8 } else { flags & !8 };
        b[0x104..0x106].copy_from_slice(&flags.to_le_bytes());
        // 0x00535e7a..0x00535eba: `(zone x, zone y, spawn index)` (0x00402990, the loop index
        // `[ebp-0x2b64]` pushed first) copied into `creature+0x1b0` (entity+0x1a0) by
        // 0x00401060. The death (`applyHit` 0x004cf0a3 onward) starts the kill countdown of
        // the spawn this names, and the kind-5 static whose `+0x178` matches closes.
        w32(b, 0x1a0, zone.0 as u32);
        w32(b, 0x1a4, zone.1 as u32);
        w32(b, 0x1a8, index as u32);
        b[0x1c8] = s.f5c as u8;
        w32(b, 0x1cc, s.f60 as u32);
        w32(b, 0x1d0, s.f64 as u32);
        // +0x19c takes spawn+0x10c8 and the name spawn+0x10cc: -1 and empty for every generated
        // spawn, which the constructor already left there.
    }
    let hp = max_hp(&e);
    e.0[0x15c..0x160].copy_from_slice(&hp.to_le_bytes());
    if s.appearance.flags & 0x200 != 0 {
        for o in [0x70, 0x74, 0x78] {
            let v = f32_at(&e.0, o) * 2.0f32;
            e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    // Lift by half the scaled height plus 0.1 block: `(scale.z * 0.5 + 0.1) * 65536.0f`
    // truncated to i64 (0x00402a10), added to z.
    let lift = ((f32_at(&e.0, 0x78) * 0.5f32 + 0.1f32) * 65536.0f32) as i64;
    let z = i64::from_le_bytes(e.0[16..24].try_into().unwrap()).wrapping_add(lift);
    e.0[16..24].copy_from_slice(&z.to_le_bytes());
    e
}
