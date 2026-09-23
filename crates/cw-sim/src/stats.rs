//! Creature statistics: `Creature::maxHp` (`Server.exe 0x0040fda0`), `itemPower`
//! (`0x00410f00`) and the armour HP bonus (`0x00413ce0`), over the entity block.

use cw_net::EntityData;
use cw_net::entity::ITEM_SIZE;

use crate::util::{f32_at};

/// `pow` as MSVCR110's `_libm_sse2_pow_precise` computes it (Server.exe imports it at thunk
/// `0x0054ab14`; every `pow` of the game goes there): [`cw_math::pow`], bit-exact unless the
/// `native-libm` feature of cw-math is on.
pub fn pow(x: f64, y: f64) -> f64 {
    cw_math::pow(x, y)
}

/// `Server.exe 0x00410f90`: [`item_power`] over `2^3` (the third `pow`), single precision around
/// each call.
pub fn item_stat(level: f32, rarity: i32) -> f32 {
    let t = (1.0f32 - 1.0f32 / ((level - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    let a = pow(2.0, f64::from(t)) as f32;
    let b = pow(2.0, f64::from(rarity as f32 * 0.25f32)) as f32;
    let v = a * b;
    let c = pow(2.0, 3.0) as f32;
    v / c
}

/// `Server.exe 0x00410f00`: `2^((1 - 1/((level - 1) * 0.05 + 1)) * 3) * 2^(rarity * 0.25)`, single
/// precision around the two `pow` calls.
pub fn item_power(level: f32, rarity: i32) -> f32 {
    let t = (1.0f32 - 1.0f32 / ((level - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    let a = pow(2.0, f64::from(t)) as f32;
    let b = pow(2.0, f64::from(rarity as f32 * 0.25f32)) as f32;
    a * b
}

/// `Server.exe 0x00413ce0`: the HP bonus of an armour item (types 3..7), from its modifier,
/// material, level, spirit count and rarity.
pub fn item_hp_bonus(item: &[u8]) -> f32 {
    let ty = item[0];
    if !matches!(ty, 3..=7) {
        return 0.0;
    }
    let kind = if ty == 4 { 1.0f32 } else { 0.5f32 };
    let modifier = u32::from_le_bytes(item[4..8].try_into().unwrap());
    let m = (modifier.wrapping_shl(3)) % 0x15;
    let mut q = (1.0f32 - (f64::from(m) as f32) / 20.0f32) + 1.0f32;
    match item[0xd] {
        1 => q += 1.0,
        0x1a => q += 0.5,
        0x1b => q += 0.75,
        _ => {}
    }
    let spirits = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap()) as f32;
    let level = u16::from_le_bytes(item[0x10..0x12].try_into().unwrap()) as f32;
    item_power(spirits * 0.1f32 + level, i32::from(item[0xc])) * 5.0f32 * kind * q
}

/// `Creature::maxHp`, `Server.exe 0x0040fda0`, over the entity block: level and power base
/// through `pow`, the max-HP multiplier, a flat `2^1` factor for players, the class and
/// specialisation multipliers, then the bonuses of the armour in slots 6, 7, 2, 3, 4, 5 when
/// each holds the item type it expects.
pub fn max_hp(e: &EntityData) -> f32 {
    let b = &e.0;
    let level = i32::from_le_bytes(b[0x180..0x184].try_into().unwrap()) as f32;
    let t = (1.0f32 - 1.0f32 / ((level - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    let p1 = pow(2.0, f64::from(t)) as f32;
    let p2 = pow(2.0, f64::from(f32::from(b[0x198]) * 0.25f32)) as f32;
    let mult = f32_at(b, 0x168);
    let mut hp = p2 * p1 * mult;
    if b[0x50] == 0 {
        hp = pow(2.0, 1.0) as f32 * p1 * mult;
    }
    match b[0x130] {
        1 => hp *= 1.3f32,
        2 => hp *= 1.1f32,
        4 => hp *= 1.2f32,
        _ => {}
    }
    if b[0x130] == 1 && b[0x131] == 1 {
        hp *= 1.25f32;
    }
    for (slot, ty) in [(6usize, 3u8), (7, 3), (2, 4), (3, 6), (4, 5), (5, 7)] {
        let o = 0x2f0 + slot * ITEM_SIZE;
        if b[o] == ty {
            hp += item_hp_bonus(&b[o..o + ITEM_SIZE]);
        }
    }
    hp
}
