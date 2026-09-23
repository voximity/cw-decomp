//! Skill timing and factors the creature update reads: `skillWindup` 0x00407db0,
//! `skillDuration` 0x00411d60, `skillTotalTime` 0x004084b0, `attackSpeed` 0x00412150,
//! `skillLevelFactor` 0x00409de0, `skillSlotOfMode` 0x00407cc0, `maxBlock` 0x0040fcf0 and the
//! skill-level curves 0x004095d0, 0x004120f0, 0x0040a7f0, 0x00409740.
//!
//! The skill levels live at `entity+0x1128` (eleven i32). A mode of -1 means the creature's
//! current mode (`entity+0x58`).

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern)]

use cw_net::EntityData;

use crate::util::{f32_at, i32_at, scaled, u16_at};

/// The skill level at index `k` of the eleven at `entity+0x1128`.
pub fn skill(e: &EntityData, k: usize) -> i32 {
    i32_at(&e.0, 0x1128 + k * 4)
}

/// `Server.exe 0x00407cc0`: the skill slot a mode belongs to (6, 7 or 8), or -1.
pub fn skill_slot_of_mode(mode: i32) -> i32 {
    match mode {
        0x15 | 0x22 | 0x30 | 0x36 | 0x58 => 6,
        0x31 | 0x60 | 0x61 | 0x63..=0x66 => 8,
        0x32 | 0x4f | 0x56 | 0x67 => 7,
        _ => -1,
    }
}

/// `Server.exe 0x00409de0`: `1 - 1 / (level * 0.1 + 1)` for the skill of `mode`; `level < 0`
/// reads the entity's skill level; a level of 0 on a non-player means `(level - 1) / 2 +
/// power + 1`. A mode without a skill slot gives 1.
pub fn skill_level_factor(e: &EntityData, mode: i32, level: i32) -> f32 {
    let slot = skill_slot_of_mode(mode);
    if slot < 0 {
        return 1.0;
    }
    let mut lv = level;
    if lv < 0 {
        lv = skill(e, slot as usize);
    }
    if lv == 0 && e.0[0x50] != 0 {
        lv = ((i32_at(&e.0, 0x180) - 1) >> 1) + i32::from(e.0[0x198]) + 1;
    }
    1.0f32 - 1.0f32 / (lv as f32 * 0.1f32 + 1.0f32)
}

/// `Server.exe 0x0040fcf0`: the guard a creature can hold: `speed * 50` (× 20 with appearance
/// flag 0x10); by the weapon in slot 7 (type 3): sub type 6 → 60, 8 → 80, 11, 12, 15, 16, 17
/// → 20, else 50.
pub fn max_block(e: &EntityData) -> i32 {
    let m = f32_at(&e.0, 0x16c);
    let w = 0x2f0 + 7 * 0x118;
    if e.0[w] != 3 {
        if u16_at(&e.0, 0x6e) & 0x10 != 0 {
            return (m * 20.0f32) as i32;
        }
        return (m * 50.0f32) as i32;
    }
    let k = match e.0[w + 1] {
        6 => 60.0f32,
        8 => 80.0f32,
        11 | 12 | 15 | 16 | 17 => 20.0f32,
        _ => 50.0f32,
    };
    (m * k) as i32
}

/// `Server.exe 0x00414350`: the attack speed an item adds: for types 3..9, `itemStat(level,
/// rarity) * k * m` where `k` is 0.1 (0.2 for a weapon of sub type 5, 6, 7, 8, 0xa, 0xb, 0xf,
/// 0x10, 0x11 or 0x12, and for type 4) and `m` is `(modifier % 21) / 20`, plus 1 for material
/// 0xc; 0 below 0.001.
pub fn item_attack_speed(item: &[u8]) -> f32 {
    let ty = item[0];
    if !matches!(ty, 8 | 9 | 3 | 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let mut k = 0.1f32;
    if (ty == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)) || ty == 4 {
        k = 0.2f32;
    }
    // 0x004143d0: modifier % 21 (unsigned), through a double, over 20.
    let m = u32::from_le_bytes(item[4..8].try_into().unwrap()) % 0x15;
    let mut f = (f64::from(m) as f32) / 20.0f32;
    if item[0xd] == 0xc {
        f = f + 1.0f32;
    }
    let level = f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
    let v = crate::stats::item_stat(level, i32::from(item[0xc])) * k * f;
    if 0.001f32 > v { 0.0 } else { v }
}

/// `Server.exe 0x00412300`: the gear part of the attack speed: `2^((1 - 1/((level - 1) * 0.05
/// + 1)) * 3) * 2^(0.25 * 0) / 2^3 * 0.1` of the creature's level, plus [`item_attack_speed`]
/// of each equipment slot holding its type (slots 6 and 7 weapons (3), 2 (4), 3 (6), 4 (5), 5
/// (7), 1 (8), 8 and 9 (9)).
fn gear_attack_speed(e: &EntityData) -> f32 {
    let b = &e.0;
    let level = i32_at(b, 0x180) as f32;
    let t = (1.0f32 - 1.0f32 / ((level - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    let a = crate::stats::pow(2.0, f64::from(t)) as f32;
    let p2 = crate::stats::pow(2.0, f64::from(0.25f32 * 0.0f32)) as f32;
    let v = a * p2;
    let c = crate::stats::pow(2.0, 3.0) as f32;
    let mut v = v / c * 0.1f32;
    for (slot, ty) in [(6usize, 3u8), (7, 3), (2, 4), (3, 6), (4, 5), (5, 7), (1, 8), (8, 9), (9, 9)] {
        let o = 0x2f0 + slot * 0x118;
        if b[o] == ty {
            v = item_attack_speed(&b[o..o + 0x118]) + v;
        }
    }
    v
}

/// `Server.exe 0x00412150`: the attack speed multiplier: 1 for players, `power * 0.0625 +
/// 0.75` for every other creature, plus half the guard fraction for a class-1/spec-0 warrior
/// and the full fraction for a class-3/spec-1 mage, plus the gear part; times `1 +
/// factor(level)` while a type-0xc buff is held (`has_haste`), where `level` is
/// `entity+0x1148`, or for a creature with 0 there `(level - 1) / 2 + power + 1`.
pub fn attack_speed(e: &EntityData, guard: f32, has_haste: bool) -> f32 {
    let _ = guard;
    let mut v = 1.0f32;
    if e.0[0x50] != 0 {
        v = f32::from(e.0[0x198]) * 0.0625f32 + 0.75f32;
    }
    let class = e.0[0x130];
    if class == 1 && e.0[0x131] == 0 {
        let r = (i32_at(&e.0, 0x60) as f32) / (max_block(e) as f32);
        let r = if r <= 1.0 { r } else { 1.0 };
        v = r * 0.5f32 + v;
    }
    if class == 3 && e.0[0x131] == 1 {
        let r = (i32_at(&e.0, 0x60) as f32) / (max_block(e) as f32);
        let r = if r <= 1.0 { r } else { 1.0 };
        v += r;
    }
    let total = gear_attack_speed(e) + v;
    if has_haste {
        let mut lv = i32_at(&e.0, 0x1148);
        if lv == 0 && e.0[0x50] != 0 {
            lv = ((i32_at(&e.0, 0x180) - 1) >> 1) + i32::from(e.0[0x198]) + 1;
        }
        return (1.0f32 - 1.0f32 / (lv as f32 * 0.1f32 + 1.0f32) + 1.0f32) * total;
    }
    total
}

/// `Server.exe 0x00407db0`: milliseconds before a mode's hit lands (`mode < 0`: the current
/// mode).
pub fn skill_windup(e: &EntityData, guard: f32, haste: bool, mode: i32) -> i32 {
    let m = if mode < 0 { i32::from(e.0[0x58]) } else { mode };
    let s = |b: f32| scaled(e, guard, haste, b);
    match m {
        0 | 8 | 0xb | 0x1c | 0x32 | 0x37 | 0x3c | 0x3d | 0x3e | 0x60 | 0x62 | 0x68 => 0,
        1 | 9 => s(300.0),
        2..=5 | 0xa | 0xe | 0x11..=0x15 => s(100.0),
        6 | 7 | 0x17 | 0x18 | 0x19 | 0x1b | 0x24 | 0x3b | 0x3f | 0x40 => s(50.0),
        0xc | 0x10 | 0x43 | 0xd | 0xf | 0x27..=0x2a => s(200.0),
        0x16 => s(400.0),
        0x1a | 0x41 | 0x42 | 0x44..=0x46 | 0x49..=0x4e => s(300.0),
        0x1e | 0x20 | 0x39 | 0x3a | 0x5d => s(800.0),
        0x1f | 0x21 | 0x22 => s(1600.0),
        0x25 | 0x2b | 0x59 => {
            if e.0[0x2f0 + 7 * 0x118 + 1] != 0xc {
                s(1200.0)
            } else {
                s(600.0)
            }
        }
        0x26 | 0x2c | 0x5e => s(500.0),
        0x2d | 0x2e => s(1200.0),
        0x30 | 0x65 => 100,
        0x36 => 400,
        0x47 | 0x48 => 200,
        0x57 => s(5000.0),
        0x5b | 0x5f => s(1000.0),
        0x69 => 5000,
        _ => s(400.0),
    }
}

/// `Server.exe 0x00411d60`: milliseconds a mode lasts after its wind-up.
pub fn skill_duration(e: &EntityData, guard: f32, haste: bool, mode: i32) -> i32 {
    let m = if mode < 0 { i32::from(e.0[0x58]) } else { mode };
    let s = |b: f32| scaled(e, guard, haste, b);
    match m {
        0 | 0x31 => 0,
        1 | 2 | 9 | 0xd | 0xe | 0xf | 0x43 => s(200.0),
        3 | 4 | 0x3e | 0x25 | 0x2b => s(100.0),
        5 | 0xc | 0x10 | 0x11 | 0x41 | 0x42 | 0x14 | 0x15 => s(400.0),
        6 | 7 | 0x12 | 0x13 => s(150.0),
        0xa => 200,
        0xb | 0x26..=0x2a | 0x2c | 0x4b | 0x68 => s(300.0),
        0x16 | 0x17 => s(50.0),
        0x1a | 0x22 => s(1200.0),
        0x1e | 0x20 | 0x49 | 0x5d => 600,
        0x1f | 0x21 => 1200,
        0x2d | 0x2e | 0x37 | 0x5e => s(500.0),
        0x32 | 0x4c | 0x4d | 0x4e | 0x60 => 500,
        0x36 => 100,
        0x44 | 0x45 => 1000,
        0x47 => 3000,
        0x48 | 0x56 => 5000,
        0x5b => 6000,
        0x5f => 2000,
        _ => s(300.0),
    }
}

/// `Server.exe 0x004084b0`: the whole animation of the current mode: wind-up, duration and a
/// third per-mode part.
pub fn skill_total_time(e: &EntityData, guard: f32, haste: bool) -> i32 {
    let m = i32::from(e.0[0x58]);
    let a = skill_windup(e, guard, haste, m);
    let b = skill_duration(e, guard, haste, m);
    let s = |v: f32| scaled(e, guard, haste, v);
    let c = match m {
        0 | 0x32 | 0x60 => 100,
        3 | 4 | 5 | 0x3e | 0x39 | 0x3a => s(300.0),
        7 | 0xe | 0x12 | 0x41 | 0x42 => s(200.0),
        0xa => 600,
        0xb | 0x3c | 0x3d | 0x68 | 0x43 | 0x16 | 0x1a | 0x1e..=0x22 | 0x25..=0x2e | 0x5e | 0x5f => s(100.0),
        0xf => s(400.0),
        0x17 => s(10.0),
        0x30 => 0,
        0x36 => 400,
        0x44 | 0x45 | 0x5d => s(800.0),
        0x47 | 0x48 => 200,
        _ => s(500.0),
    };
    c.wrapping_add(b).wrapping_add(a)
}

/// `Server.exe 0x004120f0`: the swimming speed factor of skill level `k`.
pub fn swim_factor(k: i32) -> f32 {
    if k < 1 {
        return 0.5;
    }
    (1.0f32 - 1.0f32 / (k as f32 * 0.1f32 + 1.0f32)) * 0.5f32 + 0.5f32
}

/// `Server.exe 0x004095d0`: the riding (mode 0x6b) speed factor of skill level `k`.
pub fn ride_factor(k: i32) -> f32 {
    if k < 1 {
        return 0.0;
    }
    (1.0f32 - 1.0f32 / (k as f32 * 0.1f32 + 1.0f32)) * 3.0f32 + 1.0f32
}

/// `Server.exe 0x0040a7f0`: the gliding speed factor of skill level `k`.
pub fn glide_factor(k: i32) -> f32 {
    if k < 1 {
        return 0.0;
    }
    (1.0f32 - 1.0f32 / (k as f32 * 0.1f32 + 1.0f32)) * 3.0f32 + 1.5f32
}

/// `Server.exe 0x00409740`: the climbing factor of skill level `k`.
pub fn climb_factor(k: i32) -> f32 {
    1.0f32 - 1.0f32 / ((k + 1) as f32 * 0.1f32 + 1.0f32)
}

/// `skillCooldown` `Server.exe 0x00409780` (`Cube.exe 0x0043e6a0`, the same function): the
/// milliseconds a mode's cooldown lasts once the mode starts, `level < 0` meaning the
/// creature's skill level (`skill_level_factor`). Single precision, `cvttss2si`; modes without
/// a cooldown give 0. The tick stores it in `creature+0x139c` at 0x0053e2c7 when a mode starts
/// (update.rs) and `canStartMode` (0x004096b0, `combat_ai::can_use`; the client's skill keys)
/// reads it.
pub fn skill_cooldown(e: &EntityData, mode: i32, level: i32) -> i32 {
    let f = || skill_level_factor(e, mode, level);
    match mode {
        0x15 | 0x58 => (8000.0f32 - f() * 8000.0f32) as i32,
        0x30 => (20000.0f32 - f() * 12000.0f32) as i32,
        0x31 | 0x32 => (16000.0f32 - f() * 10000.0f32) as i32,
        0x36 | 0x60 => (20000.0f32 - f() * 14000.0f32) as i32,
        0x48 => 15000,
        0x56 => (60000.0f32 - f() * 40000.0f32) as i32,
        0x61 | 100 | 0x65 | 0x66 => (60000.0f32 - f() * 30000.0f32) as i32,
        99 => (12000.0f32 - f() * 10000.0f32) as i32,
        0x67 => (20000.0f32 - f() * 10000.0f32) as i32,
        _ => 0,
    }
}
