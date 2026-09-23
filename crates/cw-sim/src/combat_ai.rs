//! The fighting half of a creature's AI: `CombatBehavior::update` (`Server.exe 0x00402f40`,
//! vtable 0x005587e0) whole — the mode reset, the potion at low HP, the target (most threat, the
//! pet owner's last victim, or the nearest enemy in sight), the chase (direct steering, the
//! straight-walk probe 0x0052ef00 and the path finder 0x004dd2e0), the attack choice by class,
//! weapon and distance, the taming handshake and the give-up after 20 s of chasing — and the
//! creature's hit landing on its target, `World::creatureAttack` (`0x004cfd50`: the target's
//! dodge and deflect, guard block, shield buffs, armour, weapon spirits, the hit sounds, the stun
//! roll, then the Hit record through `applyHit`).
//!
//! Every `rand()` is the tick thread's MSVC stream in [`World::rng`], drawn in the original's
//! order. The creature object keeps a few fields outside the entity block that `CreatureState`
//! (combat.rs) does not have yet; they live in [`CreatureAi`] here. The path finder's state is
//! the path module's `PathState`.

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern, clippy::excessive_precision, clippy::too_many_arguments, clippy::too_many_lines, clippy::cognitive_complexity, clippy::collapsible_if, clippy::collapsible_else_if)]

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Hit, Particle, Sound};
use cw_world::World;
use cw_world::generate::level_curve;

use crate::combat::{Buff, CreatureState, add_buff, apply_hit, friendly_type, is_hostile};
use crate::skills::{max_block, skill_duration, skill_total_time, skill_windup};
use crate::stats::{item_power, max_hp, pow};

use crate::path;
use crate::util::{K, WEAPON, OFFHAND, alert_allies, diff_blocks, elite, f32_at, fix, guard_haste, has_buff, i32_at, i64_at, is_blocking, is_mage, is_ranged, level_pow, mana_cost, passive_record, pos_at, pos_blocks, scaled, set_vec3f, strike_items, to_block, u16_at, u32_at, vec3f_at, w16, w32, w64, wf32, wi32};

// ---------------------------------------------------------------------------------------------
// Byte helpers (duplicated from physics.rs).

fn ent(entities: &mut BTreeMap<i64, EntityData>, id: i64) -> &mut EntityData {
    entities.get_mut(&id).expect("creature")
}

fn mode_time(e: &EntityData) -> i32 {
    i32_at(&e.0, 0x5c)
}

fn set_mode(e: &mut EntityData, m: u8) {
    e.0[0x58] = m;
}

fn set_mode_time(e: &mut EntityData, t: i32) {
    wi32(&mut e.0, 0x5c, t);
}

// ---------------------------------------------------------------------------------------------
// Records.

fn sound_at(pos: [f32; 3], kind: u32, pitch: f32, volume: f32) -> Sound {
    let mut b = [0u8; 0x18];
    set_vec3f(&mut b, 0, pos);
    w32(&mut b, 0xc, kind);
    wf32(&mut b, 0x10, pitch);
    wf32(&mut b, 0x14, volume);
    Sound(b)
}

/// A Hit record (0x48 bytes) as `0x00422a90` leaves it (zero from +0x14) with the fields set:
/// attacker +0, target +8, damage +0x10, critical +0x14, stun +0x18, position +0x20, direction
/// +0x38, +0x44 flag, +0x45 hit type.
fn hit_record(attacker: i64, target: i64, damage: f32, pos: [i64; 3], dir: [f32; 3], kind: u8) -> Hit {
    let mut h = [0u8; 0x48];
    w64(&mut h, 0, attacker);
    w64(&mut h, 8, target);
    wf32(&mut h, 0x10, damage);
    for (i, p) in pos.iter().enumerate() {
        w64(&mut h, 0x20 + i * 8, *p);
    }
    set_vec3f(&mut h, 0x38, dir);
    h[0x45] = kind;
    Hit(h)
}

/// A particle record (0x48 bytes): position +0, velocity +0x18, colour +0x24, scale +0x34,
/// count +0x38, kind +0x3c, spread +0x40; +0x44 is left uninitialised by the original (0 here).
fn particle(pos: [i64; 3], vel: [f32; 3], color: [f32; 4], scale: f32, count: i32, kind: i32, spread: f32) -> Particle {
    let mut p = [0u8; 0x48];
    for (i, v) in pos.iter().enumerate() {
        w64(&mut p, i * 8, *v);
    }
    set_vec3f(&mut p, 0x18, vel);
    for (i, c) in color.iter().enumerate() {
        wf32(&mut p, 0x24 + i * 4, *c);
    }
    wf32(&mut p, 0x34, scale);
    wi32(&mut p, 0x38, count);
    wi32(&mut p, 0x3c, kind);
    wf32(&mut p, 0x40, spread);
    Particle(p)
}

/// `Server.exe 0x004ce9f0`: `addBuff` on the creature (0x00411740) and a passive record from the
/// creature to itself.
fn add_buff_record(states: &mut BTreeMap<i64, CreatureState>, id: i64, buff: &Buff, out: &mut ServerUpdate) {
    add_buff(states.entry(id).or_default(), buff);
    out.passives.push(passive_record(id, id, buff));
}

// ---------------------------------------------------------------------------------------------
// Creature predicates and stats.

/// `Server.exe 0x0040f2b0`: the right hand holds a weapon of the sub types 0xf, 0x10, 0x11, 5,
/// 10, 11, 0x12, 8, 6 or 7 (two-handers, staves, bows).
pub fn heavy_weapon(e: &EntityData) -> bool {
    e.0[WEAPON] == 3 && matches!(e.0[WEAPON + 1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)
}

/// `Server.exe 0x0040ffe0`: the stun a landed stun roll gives: 1500 ms on a player, else
/// `3000 - power * 1500 / 4`.
fn stun_duration(e: &EntityData) -> i32 {
    if e.0[0x50] == 0 {
        return 1500;
    }
    3000 - ((i32::from(e.0[0x198]) * 1500) >> 2)
}

/// `Server.exe 0x0040f2f0`: the recovery part of a mode's animation (the third table of
/// `skillTotalTime` 0x004084b0); `mode < 0` means the current mode.
pub fn recovery_time(e: &EntityData, guard: f32, haste: bool, mode: i32) -> i32 {
    let m = if mode < 0 { i32::from(e.0[0x58]) } else { mode };
    let s = |b: f32| scaled(e, guard, haste, b);
    match m {
        0 | 0x32 | 0x60 => 100,
        3 | 4 | 5 | 0x3e => s(300.0),
        7 | 0xe | 0x12 => s(200.0),
        0xa => 600,
        0xb | 0x3c | 0x3d | 0x68 => s(100.0),
        0xf => s(400.0),
        0x16 | 0x1a | 0x1e..=0x22 | 0x25..=0x2e | 0x5e | 0x5f => s(100.0),
        0x17 => s(10.0),
        0x30 => 0,
        0x36 => 400,
        0x39 | 0x3a => s(300.0),
        0x41 | 0x42 => s(200.0),
        0x43 => s(100.0),
        0x44 | 0x45 | 0x5d => s(800.0),
        0x47 | 0x48 => 200,
        _ => s(500.0),
    }
}

/// `Server.exe 0x004096b0`: whether the creature can start `mode` now: not stunned (or the mode
/// is 0x65), mana for it, and no cooldown running (`creature+0x139c`); mode 0x1c also needs some
/// mana.
pub fn can_use(e: &EntityData, st: &CreatureState, mode: i32) -> bool {
    if i32_at(&e.0, 0x11c) < 1 || mode == 0x65 {
        let cost = mana_cost(e, st, mode, -1);
        let mp = f32_at(&e.0, 0x160);
        if cost < mp || cost == mp {
            let cd = u8::try_from(mode).ok().and_then(|m| st.cooldowns.get(&m).copied());
            if cd.is_none_or(|v| v == 0) {
                if mode != 0x1c {
                    return true;
                }
                return 0.0 < mp;
            }
        }
    }
    false
}

/// `Server.exe 0x00410010`: the basic attack of the creature's weapon, alternating the two
/// swings of a combo while the previous one ended less than 200 ms ago.
pub fn basic_mode(e: &EntityData, guard: f32, haste: bool) -> u8 {
    let class = e.0[0x130];
    let ty = i32_at(&e.0, 0x54);
    let wt = e.0[WEAPON];
    let ws = e.0[WEAPON + 1];
    let spec = e.0[0x131];
    let mode = e.0[0x58];
    let mt = mode_time(e);
    let combo = || mt < skill_total_time(e, guard, haste) + 200;
    if class == 3 || ty == 0x75 || ty == 0x56 || (wt == 3 && matches!(ws, 10..=12)) {
        if ws == 0xb {
            return if spec == 1 { 0x2c } else { 0x26 };
        }
        if ws == 0xa {
            return u8::from(spec == 1) * 2 + 0x1e;
        }
        if spec != 1 {
            if mode == 0x28 && combo() {
                return 0x27;
            }
            return 0x28;
        }
        if mode == 0x2a && combo() {
            return 0x29;
        }
        return 0x2a;
    }
    if ty == 0x68 || (wt == 3 && matches!(ws, 6 | 7)) {
        return 0x16;
    }
    let st = e.0[OFFHAND];
    if st == 3 && matches!(e.0[OFFHAND + 1], 6 | 7) {
        return 0x16;
    }
    if ws == 3 {
        if mode == 0x13 && combo() {
            return 0x12;
        }
        return 0x13;
    }
    if ws != 4 && wt != 0 {
        if ws == 5 {
            if mode == 0xe && combo() {
                return 0xd;
            }
            return 0xe;
        }
        if wt == 3 && ws == 8 {
            return 0x1a;
        }
        if heavy_weapon(e) {
            if mode == 0x39 && combo() {
                return 0x43;
            }
            if mode == 0x43 && combo() {
                return 0x3a;
            }
            return 0x39;
        }
        if st == 3 && e.0[OFFHAND + 1] == 0xd {
            if mode == 0xa && combo() {
                return 9;
            }
            return 0xa;
        }
        if e.0[0x6e] & 0x10 != 0 {
            return 0x4b;
        }
        if mode == 1 && combo() {
            return 2;
        }
        return 1;
    }
    if mode == 6 && combo() {
        return 7;
    }
    6
}

/// `Server.exe 0x00410290`: the special attack of the creature's weapon.
pub fn special_mode(e: &EntityData) -> i32 {
    let wt = e.0[WEAPON];
    let ws = e.0[WEAPON + 1];
    let spec = e.0[0x131];
    let odd = i32_at(&e.0, 0x60) % 2 != 0;
    if wt == 3 && ws == 6 {
        return 0x17;
    }
    let ty = i32_at(&e.0, 0x54);
    let mage = e.0[0x130] == 3 || ty == 0x75 || ty == 0x56 || (wt == 3 && matches!(ws, 10..=12));
    if !mage {
        if wt == 3 {
            if ws == 5 {
                return 5;
            }
            if matches!(ws, 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7) {
                return 0x42 - i32::from(odd);
            }
        }
        return i32::from(odd) + 3;
    }
    if ws == 0xb {
        return i32::from(spec != 1) + 0x2d;
    }
    if ws == 0xa {
        return i32::from(spec == 1) * 2 + 0x1f;
    }
    if spec == 1 { 0x2b } else { 0x25 }
}

/// `Server.exe 0x00410400`: start the basic attack once the current mode's animation (less its
/// recovery while the combo counter runs) is over. False while rolling or in mode 0x30.
pub fn choose_basic(e: &mut EntityData, guard: f32, haste: bool) -> bool {
    if i32_at(&e.0, 0x118) != 0 || e.0[0x58] == 0x30 {
        return false;
    }
    let mut t = skill_total_time(e, guard, haste);
    let m = e.0[0x58];
    if i32_at(&e.0, 0x60) != 0 && m != 0x1e && m != 0x20 {
        t -= recovery_time(e, guard, haste, i32::from(m));
    }
    if t <= mode_time(e) {
        let nm = basic_mode(e, guard, haste);
        set_mode(e, nm);
        set_mode_time(e, 0);
    }
    true
}

/// `Server.exe 0x00410690`: start the weapon's special attack when the animation allows, the
/// creature is not rolling (a rogue specialist's roll attacks excepted) and has the mana; with
/// a type-9 buff the spells 0x25, 0x2e, 0x5f, 0x1f and 0x21 skip their wind-up.
pub fn choose_special(e: &mut EntityData, st: &CreatureState, guard: f32, haste: bool) -> bool {
    let mut t = skill_total_time(e, guard, haste);
    if i32_at(&e.0, 0x60) != 0 {
        t -= recovery_time(e, guard, haste, -1);
    }
    let m = special_mode(e);
    let roll_ok = e.0[0x130] == 4 && e.0[0x131] == 1 && matches!(m, 0x11 | 5 | 0x14);
    let mp = f32_at(&e.0, 0x160);
    if mode_time(e) < t || (i32_at(&e.0, 0x118) != 0 && !roll_ok) {
        return false;
    }
    let cost = mana_cost(e, st, m, -1);
    if mp <= cost && cost != mp {
        return false;
    }
    let m = special_mode(e);
    set_mode(e, m as u8);
    set_mode_time(e, 0);
    if matches!(m as u8, 0x25 | 0x2e | 0x5f | 0x1f | 0x21) && has_buff(st, 9) {
        let w = skill_windup(e, guard, haste, m);
        set_mode_time(e, w);
    }
    if !roll_ok {
        wi32(&mut e.0, 0x118, 0);
    }
    true
}

/// `Server.exe 0x00411800`: the point a creature with appearance flag 4 breathes from: the
/// identity matrix rotated about z by the yaw, applied to `(scale.y * 0.5, 0, scale.z * 0.35)`
/// with the original's full matrix products, added to the position; without the flag the
/// position.
fn mouth_pos(e: &EntityData) -> [i64; 3] {
    let pos = pos_at(&e.0);
    if e.0[0x6e] & 4 == 0 {
        return pos;
    }
    // 0x00411825: identity (0x00401cd0), m[col*4+row].
    let m: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let a = f32_at(&e.0, 0x20) * 0.0174532924f32;
    let c = cw_math::cos(f64::from(a)) as f32;
    let s = cw_math::sin(f64::from(a)) as f32;
    let a0 = m[4] * s + m[0] * c;
    let b0 = m[4] * c - m[0] * s;
    let b1 = m[5] * c - m[1] * s;
    let a1 = m[5] * s + m[1] * c;
    let a2 = m[6] * s + m[2] * c;
    let b2 = m[6] * c - m[2] * s;
    let a3 = m[7] * s + m[3] * c;
    let h = f32_at(&e.0, 0x74) * 0.5f32;
    let z = f32_at(&e.0, 0x78) * 0.35f32;
    let b3 = m[7] * c - m[3] * s;
    let w = ((h * b3 + a3 * 0.0f32) + m[11] * z) + m[15];
    let x = ((h * b0 + a0 * 0.0f32) + m[8] * z) + m[12];
    let y = ((h * b1 + a1 * 0.0f32) + m[9] * z) + m[13];
    let inv = 1.0f32 / w;
    let zz = ((h * b2 + a2 * 0.0f32) + m[10] * z) + m[14];
    let v = [inv * x, inv * y, inv * zz];
    [pos[0].wrapping_add(fix(v[0])), pos[1].wrapping_add(fix(v[1])), pos[2].wrapping_add(fix(v[2]))]
}

// ---------------------------------------------------------------------------------------------
// Item stats for the hit.

/// The level `creatureAttack`'s item helpers pass to `itemPower`: `spirits * 0.1 + level`.
fn item_level(item: &[u8]) -> f32 {
    i32_at(item, 0x114) as f32 * 0.1f32 + f32::from(i16::from_le_bytes([item[0x10], item[0x11]]))
}

/// `Server.exe 0x00414550`: the guard strength of a weapon item (type 3): `itemPower` times 2
/// (sub types 3, 4, 0xd), 4 (5 and the one-handers) or 8 (two-handers, staves, bows).
fn item_block(item: &[u8]) -> f32 {
    if item[0] != 3 {
        return 0.0;
    }
    let p = || item_power(item_level(item), i32::from(item[0xc]));
    match item[1] {
        3 | 4 | 0xd => p() * 2.0f32,
        5 => p() * 4.0f32,
        0xf | 0x10 | 0x11 | 0xa | 0xb | 0x12 | 8 | 6 | 7 => p() * 8.0f32,
        _ => p() * 4.0f32,
    }
}

/// `Server.exe 0x004139b0`: the armour of an armour item (types 4, 5, 6, 7): `itemPower` times
/// 1 (chest) or 0.5, scaled by material (0x12 × 0.8; 0x13, 0x1a, 0x1b × 0.85; 0x17, 0x19 × 0.75).
pub fn item_armor(item: &[u8]) -> f32 {
    if !matches!(item[0], 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let base = if item[0] == 4 { 1.0f32 } else { 0.5f32 };
    let k = match item[0xd] {
        0x12 => base * 0.8f32,
        0x13 | 0x1a | 0x1b => base * 0.85f32,
        0x17 | 0x19 => base * 0.75f32,
        _ => base,
    };
    item_power(item_level(item), i32::from(item[0xc])) * k
}

/// `Server.exe 0x00414260`: the resistance of an armour item: as 0x004139b0 with materials 1 and
/// 0x13 × 0.85, 0x1a and 0x1b × 0.75.
pub fn item_resist(item: &[u8]) -> f32 {
    if !matches!(item[0], 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let base = if item[0] == 4 { 1.0f32 } else { 0.5f32 };
    let k = match item[0xd] {
        1 | 0x13 => base * 0.85f32,
        0x1a | 0x1b => base * 0.75f32,
        _ => base,
    };
    item_power(item_level(item), i32::from(item[0xc])) * k
}

fn slot(e: &EntityData, s: usize) -> &[u8] {
    let o = 0x2f0 + s * 0x118;
    &e.0[o..o + 0x118]
}

/// The shared shape of `armor` 0x00408300 and `resistance` 0x00411540: the multiplier at `mul`
/// times the level and power curves (players: `2^1` in place of the power curve), plus the
/// curve product with appearance flag 0x20, plus the chest, gloves, boots and shoulders items.
fn defence(e: &EntityData, mul: usize, item: fn(&[u8]) -> f32) -> f32 {
    let p1 = level_pow(e);
    let p2 = pow(2.0, f64::from(f32::from(e.0[0x198]) * 0.25f32)) as f32;
    let f = p2 * p1;
    let mut v = f32_at(&e.0, mul) * f;
    if e.0[0x50] == 0 {
        v = (pow(2.0, 1.0) as f32 * p1) * f32_at(&e.0, mul);
    }
    if e.0[0x6e] & 0x20 != 0 {
        v = v + f;
    }
    for (s, ty) in [(2usize, 4u8), (3, 6), (4, 5), (5, 7)] {
        if slot(e, s)[0] == ty {
            v = item(slot(e, s)) + v;
        }
    }
    v
}

/// `Server.exe 0x00408300`: armour (entity+0x174), against physical hits.
pub fn armor(e: &EntityData) -> f32 {
    defence(e, 0x174, item_armor)
}

/// `Server.exe 0x00411540`: resistance (entity+0x178), against magic hits.
pub fn resistance(e: &EntityData) -> f32 {
    defence(e, 0x178, item_resist)
}

/// `Server.exe 0x004094a0`: the guard strength: `entity+0x170` times the level curve (times
/// `2^0`), plus the off-hand weapon's guard (a shield, off-hand sub type 0xd, counts the right
/// hand's weapon four times instead), plus the right hand's; the sum times `entity+0x170` again.
pub fn block_strength(e: &EntityData) -> f32 {
    let p1 = level_pow(e);
    let p2 = pow(2.0, f64::from(0.25f32 * 0.0f32)) as f32;
    let m = f32_at(&e.0, 0x170);
    let mut v = p1 * p2 * m;
    let off = slot(e, 6);
    let w = slot(e, 7);
    if off[0] == 3 {
        if off[1] == 0xd {
            v = item_block(w) * 4.0f32 + v;
            return m * v;
        }
        v = item_block(off) + v;
    }
    if w[0] == 3 {
        v = item_block(w) + v;
    }
    m * v
}

/// `Server.exe 0x00413df0`: a spirit's particle colour: kinds 0x80..0x83 are fire, poison?,
/// ice and wind tints scaled by `1 + f * 0.5` with alpha `(base.a + f) * (1 + f * 0.5)`; other
/// kinds (only >= 0x80 reach here) grey.
fn spirit_color(kind: u8, base: [f32; 4], f: f32) -> [f32; 4] {
    let g = || f * 0.5f32 + 1.0f32;
    match kind {
        1 => [base[0] * 0.7f32, base[1] * 0.7f32, base[2] * 0.7f32, base[3]],
        2 => [base[0] * 0.4f32, base[1] * 0.3f32, base[2] * 0.2f32, base[3]],
        5 => [base[0] * 0.1f32, base[1] * 0.1f32, base[2] * 0.1f32, base[3]],
        7 => [base[0] * 0.9f32, base[1] * 0.9f32, base[2] * 0.9f32, base[3]],
        0xb => [base[0], base[1] * 0.7f32, base[2] * 0.2f32, base[3]],
        0xc => [base[0] * 0.8f32, base[1] * 0.8f32, base[2] * 0.85f32, base[3]],
        0x80 => {
            let v = g();
            [v, v * 0.5f32, v * 0.1f32, (base[3] + f) * v]
        }
        0x81 => {
            let v = g();
            [v * 0.3f32, v, v * 0.5f32, (base[3] + f) * v]
        }
        0x82 => {
            let v = g();
            [v * 0.3f32, v * 0.5f32, v, (base[3] + f) * v]
        }
        0x83 => {
            let v = g();
            [v * 0.8f32, v * 0.8f32, v, (base[3] + f) * v]
        }
        _ => [base[0] * 0.5f32, base[1] * 0.5f32, base[2] * 0.5f32, base[3]],
    }
}

/// `Server.exe 0x004d2190`: one particle burst per spirit (kind >= 0x80) of a weapon item at
/// `pos`, `factor * 3 + 1` particles (ten more and twice the speed with `strong`).
fn spirit_particles(pos: [i64; 3], vel: [f32; 3], factor: f32, strong: bool, item: &[u8], out: &mut ServerUpdate) {
    let mut j = 0;
    while j < i32_at(item, 0x114) {
        let kind = item[0x17 + j as usize * 8];
        if kind >= 0x80 {
            let mut count = (factor * 3.0f32 + 1.0f32) as i32;
            let color = spirit_color(kind, [1.0; 4], factor * 0.5f32);
            let mut v = vel;
            if strong {
                count += 10;
                v = [v[0] * 2.0f32, v[1] * 2.0f32, v[2] * 2.0f32];
            }
            out.particles.push(particle(pos, v, color, 0.1f32, count, 2, 3.0f32));
        }
        j += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// State.

/// The creature fields outside the entity block this module reads or writes and
/// `CreatureState` does not hold yet.
#[derive(Debug, Clone, Default)]
pub struct CreatureAi {
    /// `creature+0x1184`: the hit flash (0.5 unattributed, 1.0 shown, at least 0.3 for mode
    /// 0x1c); `physics::move_creature` eases it back to 0 (0x00544e8b).
    pub hit_flash: f32,
    /// `creature+0x1320`: where a charged spell (modes 0x1e..0x21) was aimed at its wind-up.
    pub spell_target: [i64; 3],
    /// `creature+0x13e0`: zeroed when the target is in reach (meaning unknown).
    pub f13e0: i32,
    /// `creature+0x13e8`: the special attack modes the spawn gives the creature (a vector of
    /// i32); one is drawn whenever `CombatState::timer` has run out. Filled by the creature
    /// setup, not by this module.
    pub special_modes: Vec<i32>,
}

/// `CombatBehavior` (0x14 bytes; vtable 0x005587e0): +4 the special-attack timer (20000 ms
/// after each special), +8 the search range in blocks, +0xc the milliseconds spent chasing,
/// +0x10 the milliseconds before the next enemy search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatState {
    pub timer: i32,
    pub range: f32,
    pub chase_ms: i32,
    pub search_ms: i32,
}

impl CombatState {
    /// As the default behaviour list builds it (`World::tick` 0x00535fe3): 20000, `range`, 0, 0.
    pub fn new(range: f32) -> CombatState {
        CombatState { timer: 20000, range, chase_ms: 0, search_ms: 0 }
    }
}

/// `Creature::clearPath` 0x00405330: the path lists and the passed statics go.
fn clear_path(path: &mut path::PathState, st: &mut CreatureState) {
    path::clear_path(path, st);
}

/// The creature with the most threat, `Server.exe 0x0040fc30`: the last key (map order) whose
/// value is at least the best so far, starting from 0 and skipping key 0; 0 for none.
fn max_threat(st: &CreatureState) -> i64 {
    let mut best = 0.0f32;
    let mut id = 0i64;
    for (k, v) in &st.threat {
        if *k != 0 && best <= *v {
            id = *k;
            best = *v;
        }
    }
    id
}

/// Whether `o` is feeding `me` (mode 0x52 holding in slot 12 a pet item (type 0x14) of `me`'s
/// creature type).
fn feeding(o: &EntityData, me: &EntityData) -> bool {
    o.0[0x58] == 0x52 && o.0[0x1010] == 0x14 && i32::from(o.0[0x1011]) == i32_at(&me.0, 0x54)
}

// ---------------------------------------------------------------------------------------------
// CombatBehavior::update.

/// `CombatBehavior::update`, `Server.exe 0x00402f40`. Returns true when the behaviour handled
/// the creature this update (the list stops there), false when it has nobody to fight.
///
/// In order: aiming (flag 4) clears; a mode other than 0x4f/0x52..0x54 older than 10 s ends;
/// rolling, or drinking (mode 0x50) for less than 3 s, holds (true). Below a quarter of max HP
/// after its animation the creature drinks the first potion (item 1/1) of its inventory and
/// staggers randomly (true). The target is the creature with the most threat; a pet takes its
/// owner's most-hit victim instead (none while the owner is in mode 0x6a: false). Without a
/// target a neutral (hostile 3) below half HP drinks a conjured potion (true). The special
/// timer counts down; with no target yet, and the search timer run out (a pet only without an
/// owner), the nearest enemy within `range` (scaled by the enemy's guard against the level
/// difference), in sight and not ignored by a calm (buff 8) creature becomes the target with
/// threat 1 and an alert. Flag 0x40 (engaged) follows the mode's progress; with no target the
/// chase timer resets (false).
///
/// With a target: the reach is two own widths plus the target's (50 for a ranged attacker in
/// sight, 20 for types 0x2f/0x6f..0x71, per mode, per special), 1 without line of sight. Out of
/// reach the creature steers at the target (80 blocks/s², 40 when close), keeps steering while
/// the straight-walk probe passes, otherwise builds a path; after 20 s it forgets every threat
/// (false). In reach it stops and chooses its move: a drawn special, per-type move lists
/// (0x74, 0x76, 0x2f/0x6f..0x71, 0x77), spells for casters, lunges and jumps for 0x65 and
/// flying creatures, and for fighters a random dodge roll, the basic attack at low mana, a
/// guard or charge attack by weapon, or the special. The aim (entity+0x150) points at the
/// target. A creature fed by its target is tamed (buff 8 for 20 s) after five feeding modes.
pub fn combat_update(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, ai: &mut CreatureAi, path: &mut path::PathState, id: i64, elapsed: i32, cs: &mut CombatState, out: &mut ServerUpdate) -> bool {
    // 0x00402f91: aiming clears; a stale mode ends.
    {
        let e = ent(entities, id);
        let f = u16_at(&e.0, 0x114) & 0xfffb;
        w16(&mut e.0, 0x114, f);
        let m = e.0[0x58];
        if m != 0x53 && m != 0x52 && m != 0x54 && m != 0x4f && mode_time(e) > 10000 {
            e.0[0x58] = 0;
        }
    }
    let me = entities[&id].clone();
    // 0x00402fcc: rolling, or drinking for less than 3 s.
    if i32_at(&me.0, 0x118) != 0 || (me.0[0x58] == 0x50 && mode_time(&me) < 3000) {
        return true;
    }
    let (guard, haste) = guard_haste(states, id);
    // 0x00402fec: below a quarter of max HP, drink the first potion.
    if max_hp(&me) * 0.25f32 > f32_at(&me.0, 0x15c) && mode_time(&me) > skill_total_time(&me, guard, haste) {
        let st = states.entry(id).or_default();
        let mut found = None;
        'pages: for (pi, page) in st.inventory.pages.iter().enumerate() {
            for (si, s) in page.iter().enumerate() {
                if s.count != 0 && s.item.item_type == 1 && s.item.sub_type == 1 {
                    found = Some((pi, si));
                    break 'pages;
                }
            }
        }
        if let Some((pi, si)) = found {
            let item = st.inventory.pages[pi][si].item.to_bytes();
            {
                let e = ent(entities, id);
                set_mode(e, 0x50);
                set_mode_time(e, 0);
                // 0x00402a70: the item copied field by field (padding bytes not copied).
                let o = 0x1d8;
                for k in [0usize, 1, 4, 5, 6, 7, 8, 9, 0xa, 0xb, 0xc, 0xd, 0xe, 0x10, 0x11] {
                    e.0[o + k] = item[k];
                }
                e.0[o + 0x14..o + 0x118].copy_from_slice(&item[0x14..0x118]);
            }
            let s = &mut st.inventory.pages[pi][si];
            s.count -= 1;
            if s.count <= 0 {
                s.count = 0;
                s.item.item_type = 0;
                s.item.sub_type = 0;
            }
            let x = (world.rng.rand() as f32 * 4.0f32) / 32767.0f32 - 2.0f32;
            let y = world.rng.rand() as f32 * 4.0f32;
            let e = ent(entities, id);
            wf32(&mut e.0, 0x30, x);
            wf32(&mut e.0, 0x38, 0.0);
            wf32(&mut e.0, 0x34, y / 32767.0f32 - 2.0f32);
            return true;
        }
    }
    // 0x00403062: the target with the most threat.
    let top = max_threat(states.entry(id).or_default());
    let mut target: Option<i64> = entities.contains_key(&top).then_some(top);
    let hostile = me.0[0x50];
    let parent = i64_at(&me.0, 0x188);
    if hostile == 5 && let Some(owner) = entities.get(&parent) {
        // 0x00403156: a pet fights what its owner hits most.
        if owner.0[0x58] == 0x6a {
            return false;
        }
        target = None;
        let mut best = 0.0f32;
        if let Some(os) = states.get(&parent) {
            for (k, v) in &os.hits_landed {
                if *v > best && entities.contains_key(k) {
                    target = Some(*k);
                    best = *v;
                }
            }
        }
    }
    if target.is_none() {
        // 0x0040329c: a hurt neutral drinks a potion of its level.
        if max_hp(&me) * 0.5f32 > f32_at(&me.0, 0x15c) && hostile == 3 {
            let e = ent(entities, id);
            set_mode(e, 0x50);
            set_mode_time(e, 0);
            w16(&mut e.0, 0x1d8, 0x401);
            let lv = u16_at(&e.0, 0x180);
            w16(&mut e.0, 0x1e8, lv);
            set_vec3f(&mut e.0, 0x30, [0.0; 3]);
            return true;
        }
    }
    // 0x0040325a: the special timer; calm (buff 8).
    cs.timer = (cs.timer - elapsed).max(0);
    let calm = states.get(&id).is_some_and(|s| has_buff(s, 8));
    let engaged;
    if target.is_none() {
        let mut found: Option<i64> = None;
        if (hostile != 5 || parent == 0) && cs.search_ms <= 0 {
            // 0x00403343: the nearest enemy in range and in sight.
            let my_pos = pos_at(&me.0);
            let mut best = cs.range * cs.range;
            // Performance note for a future optimizer: every creature of the world is tested,
            // with a line-of-sight sweep for each candidate closer than the best so far.
            let ids: Vec<i64> = entities.keys().copied().collect();
            for k in ids {
                if k == id {
                    continue;
                }
                let o = &entities[&k];
                if 0.0 >= f32_at(&o.0, 0x15c) || o.0[0x50] == 5 {
                    continue;
                }
                let d = diff_blocks(pos_at(&o.0), my_pos);
                let mut dsq = (d[1] * d[1] + d[0] * d[0]) + d[2] * d[2];
                let og = states.get(&k).map_or(0.0, |s| s.block);
                if og > 0.0 {
                    let lv = i32_at(&o.0, 0x180).wrapping_sub(i32_at(&me.0, 0x180)) as f32;
                    let p = pow(1.5, f64::from(lv)) as f32;
                    let mut f = 1.0f32 - p * og;
                    if 0.1f32 > f {
                        f = 0.1f32;
                    }
                    dsq = dsq / (f * f);
                }
                if calm || !(best > dsq) {
                    continue;
                }
                if !is_hostile(&me, o) && !(hostile == 5 && parent == 0 && feeding(o, &me)) {
                    continue;
                }
                if !path::line_of_sight(world, my_pos, pos_at(&o.0), true, 200.0) {
                    continue;
                }
                if friendly_type(&me) && !feeding(o, &me) {
                    continue;
                }
                found = Some(k);
                best = dsq;
            }
            if let Some(k) = found {
                // 0x00403633: threat 1 and the alert.
                states.entry(id).or_default().threat.insert(k, 1.0);
                alert_allies(entities, states, k, id, out);
            }
        }
        target = found;
        engaged = found.is_some() && engaged_now(&entities[&id], guard, haste);
    } else {
        engaged = engaged_now(&entities[&id], guard, haste);
    }
    {
        let e = ent(entities, id);
        let f = u16_at(&e.0, 0x114);
        w16(&mut e.0, 0x114, if engaged { f | 0x40 } else { f & 0xffbf });
    }
    cs.search_ms = (cs.search_ms - elapsed).max(0);
    let Some(tid) = target else {
        cs.chase_ms = 0;
        return false;
    };
    chase_or_attack(world, entities, states, ai, path, id, tid, elapsed, cs, calm, out)
}

/// 0x00403657: engaged unless a mode is still inside its wind-up, half its recovery and its
/// duration.
fn engaged_now(e: &EntityData, guard: f32, haste: bool) -> bool {
    if e.0[0x58] == 0 {
        return true;
    }
    let r = recovery_time(e, guard, haste, -1) / 2;
    let t = skill_duration(e, guard, haste, -1).wrapping_add(r.wrapping_add(skill_windup(e, guard, haste, -1)));
    mode_time(e) > t
}

/// `0x004036ce..0x004052de`: the reach, then the chase or the attack choice.
fn chase_or_attack(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, ai: &mut CreatureAi, path: &mut path::PathState, id: i64, tid: i64, elapsed: i32, cs: &mut CombatState, calm: bool, out: &mut ServerUpdate) -> bool {
    let me = entities[&id].clone();
    let te = entities[&tid].clone();
    let (guard, haste) = guard_haste(states, id);
    let my_pos = pos_at(&me.0);
    let t_pos = pos_at(&te.0);
    let d = diff_blocks(t_pos, my_pos);
    let (dx, dy, dz) = (d[0], d[1], d[2]);
    let my_sx = f32_at(&me.0, 0x70);
    let t_sx = f32_at(&te.0, 0x70);
    let mut reach = my_sx * 2.0f32 + t_sx;
    // 0x004037ee: line of sight, then the reach by attack.
    let los = path::line_of_sight(world, my_pos, t_pos, true, 200.0);
    if is_ranged(&me) && los {
        let m = me.0[0x58];
        if matches!(m, 0x57 | 0x56 | 0x59) {
            if !(mode_time(&me) < skill_windup(&me, guard, haste, -1)) {
                reach = 50.0;
            }
        } else {
            reach = 50.0;
        }
    }
    if matches!(i32_at(&me.0, 0x54), 0x2f | 0x6f | 0x71 | 0x70) {
        reach = 20.0;
    }
    let m = me.0[0x58];
    if matches!(m, 0x5f | 0x1c | 0x6c) {
        reach = 50.0;
    }
    if m == 0x48 {
        reach = my_sx + t_sx;
    }
    // 0x0040389e: a special attack when its timer has run out.
    let mut special = 0i32;
    if cs.timer == 0 && !ai.special_modes.is_empty() {
        let n = ai.special_modes.len() as u32;
        let r = world.rng.rand() as u32 % n;
        special = ai.special_modes[r as usize];
        if matches!(special, 0x5d | 0x5b | 0x44 | 0x45) {
            reach = 20.0;
        }
        if matches!(special, 0x56 | 0x59) {
            reach = my_sx * 2.0f32 + t_sx;
        }
    }
    if !los {
        reach = 1.0;
    }
    let reach2 = reach * reach;
    let h = dy * dy + dx * dx;
    let dsq = dz * dz + h;
    if dsq > reach2 || !los {
        return chase(world, entities, states, path, id, tid, elapsed, cs, los, [dx, dy, dz], h, reach, reach2, out);
    }
    // 0x004039ac: in reach: stop.
    cs.chase_ms = 0;
    clear_path(path, states.entry(id).or_default());
    ai.f13e0 = 0;
    set_vec3f(&mut ent(entities, id).0, 0x30, [0.0; 3]);
    let hostile = me.0[0x50];
    let parent = i64_at(&me.0, 0x188);
    if !calm && feeding(&te, &me) && (hostile != 5 || parent == 0) && reach2 >= dsq {
        // 0x00403a34: being fed: five feeding modes (0x6e), then tamed.
        states.entry(id).or_default().last_target = tid;
        let e = ent(entities, id);
        if !(mode_time(e) > skill_total_time(e, guard, haste)) {
            return true;
        }
        if e.0[0x58] != 0x6e {
            wi32(&mut e.0, 0x60, 0);
        }
        if i32_at(&e.0, 0x60) < 5 {
            set_mode(e, 0x6e);
            set_mode_time(e, 0);
            return true;
        }
        // 0x00403a79: a held type-7 buff is re-sent with value and duration zeroed, then buff 8
        // (tamed) for 20 s from the feeder. Bytes the original leaves uninitialised are 0.
        let mut buf: Buff = [0; 0x18];
        let mut found = false;
        for b in &states.entry(id).or_default().buffs {
            if b[0] == 7 {
                buf = *b;
                found = true;
            }
        }
        if found {
            w32(&mut buf, 8, 0);
            w32(&mut buf, 4, 0);
            add_buff_record(states, id, &buf, out);
        }
        w64(&mut buf, 0x10, tid);
        buf[0] = 8;
        w32(&mut buf, 8, 20000);
        w32(&mut buf, 4, 0);
        add_buff_record(states, id, &buf, out);
        return true;
    }
    attack_choice(world, entities, states, ai, id, tid, elapsed, cs, special, [dx, dy, dz]);
    true
}

/// `0x00404f44..0x004052de`: out of reach (or out of sight).
fn chase(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut path::PathState, id: i64, tid: i64, elapsed: i32, cs: &mut CombatState, los: bool, d: [f32; 3], h: f32, reach: f32, reach2: f32, out: &mut ServerUpdate) -> bool {
    let _ = out;
    cs.chase_ms = cs.chase_ms.wrapping_add(elapsed);
    if cs.chase_ms > 20000 {
        // 0x00404f53: give up: every threat goes.
        let st = states.entry(id).or_default();
        st.threat.clear();
        clear_path(path, st);
        cs.search_ms = 3000;
        cs.chase_ms = 0;
        return false;
    }
    let te = entities[&tid].clone();
    let (nx, ny, nz);
    {
        let e = ent(entities, id);
        if matches!(e.0[0x58], 0x5f | 0x1c) {
            e.0[0x58] = 0;
        }
        // 0x00404fd9: level steering unless swimming.
        let mut dz = d[2];
        if u32_at(&e.0, 0x4c) & 2 == 0 {
            dz = 0.0;
        }
        let f = dz * dz + h;
        let (mut x, mut y, mut z) = (d[0], d[1], dz);
        if f > 0.0 {
            let s = 1.0f32 / (f64::from(f).sqrt() as f32);
            x = s * d[0];
            y = s * d[1];
            z = s * dz;
        }
        (nx, ny, nz) = (x, y, z);
        let mut k = 80.0f32;
        if reach2 * 4.0f32 > (ny * ny + nx * nx) + nz * nz {
            k = 40.0;
        }
        set_vec3f(&mut e.0, 0x30, [nx * k, ny * k, nz * k]);
    }
    let me = entities[&id].clone();
    let my_pos = pos_at(&me.0);
    let t_pos = pos_at(&te.0);
    if los {
        // 0x004050b1: the straight walk works: no path.
        let st = states.entry(id).or_default();
        if walk_line(world, &me, st, my_pos, t_pos, reach) && u32_at(&me.0, 0x4c) & 0x20 == 0 {
            clear_path(path, st);
            return true;
        }
    }
    // 0x004050cc: the threat decays while chasing.
    {
        let t = states.entry(id).or_default().threat.entry(tid).or_insert(0.0);
        *t = *t * 0.9f32;
    }
    if path.waypoints.is_empty() && u32_at(&te.0, 0x4c) & 7 != 0 {
        // 0x00405102: a path from the creature's feet to the target's.
        let mut goal = t_pos;
        goal[2] = goal[2].wrapping_sub((((f32_at(&te.0, 0x78) * 0.5f32) - 0.1f32) * 65536.0f32) as i64);
        path.goal = goal;
        let mut start = my_pos;
        start[2] = start[2].wrapping_sub((((f32_at(&me.0, 0x78) * 0.5f32) - 0.1f32) * 65536.0f32) as i64);
        path.start = start;
        path.radius = f32_at(&me.0, 0x70) + f32_at(&te.0, 0x70);
        path::start_search(world, &me, states.entry(id).or_default(), path);
    }
    // `creature+0x1410` (the node map's size) is the "has path" test.
    if !path.nodes.is_empty() && path.start != path.goal {
        // 0x0040523f: up to ten more search steps while the path's end is farther than one
        // width from the goal block and holds at most 50 waypoints.
        let mut i = 0;
        loop {
            if path.waypoints.len() > 50 {
                break;
            }
            if let Some(last) = path.waypoints.back() {
                let g = [to_block(path.goal[0]), to_block(path.goal[1]), to_block(path.goal[2])];
                let ex = last[0].wrapping_sub(g[0]);
                let ey = last[1].wrapping_sub(g[1]);
                let ez = last[2].wrapping_sub(g[2]);
                let dd = ez.wrapping_mul(ez).wrapping_add(ey.wrapping_mul(ey)).wrapping_add(ex.wrapping_mul(ex)) as f32;
                let sx = f32_at(&me.0, 0x70);
                if sx * sx >= dd {
                    break;
                }
            }
            path::search_step(world, &me, states.entry(id).or_default(), path);
            path::build_waypoints(path);
            i += 1;
            if i >= 10 {
                break;
            }
        }
    }
    true
}

/// `0x00403b59..0x00404f3f`: the move chosen in reach, and the aim.
fn attack_choice(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, ai: &mut CreatureAi, id: i64, tid: i64, elapsed: i32, cs: &mut CombatState, special: i32, d: [f32; 3]) {
    let (guard, haste) = guard_haste(states, id);
    let st = states.entry(id).or_default().clone();
    let te = entities[&tid].clone();
    let t_pos = pos_at(&te.0);
    let e = ent(entities, id);
    let my_pos = pos_at(&e.0);
    let (mut ax, mut ay, mut az) = (d[0], d[1], d[2]);
    let f = u16_at(&e.0, 0x114) | 4;
    w16(&mut e.0, 0x114, f);
    let dsq = (ay * ay + ax * ax) + az * az;
    let total = |e: &EntityData| skill_total_time(e, guard, haste);
    let ty = i32_at(&e.0, 0x54);
    let pick = |world: &mut World, list: &[u8]| list[(world.rng.rand() as u32 % list.len() as u32) as usize];
    'choice: {
        if special != 0 {
            cs.timer = 20000;
            set_mode(e, special as u8);
            set_mode_time(e, 0);
            break 'choice;
        }
        match ty {
            0x74 => {
                // 0x00403cab
                if mode_time(e) > total(e) {
                    let m = pick(world, &[0x45, 0x4a, 0x49]);
                    set_mode(e, m);
                    set_mode_time(e, 0);
                }
                break 'choice;
            }
            0x76 => {
                // 0x00403d8d
                if mode_time(e) > total(e) {
                    let last = if u16_at(&e.0, 0x7c) == 0x86a { 0x5f } else { 0x56 };
                    let m = pick(world, &[7, 6, 0x14, last]);
                    set_mode(e, m);
                    set_mode_time(e, 0);
                }
                if e.0[0x58] == 0x5f {
                    let a = diff_blocks(t_pos, mouth_pos(e));
                    (ax, ay, az) = (a[0], a[1], a[2]);
                }
                break 'choice;
            }
            0x2f | 0x6f | 0x71 | 0x70 => {
                // 0x00404e17
                if mode_time(e) > total(e) {
                    let m = pick(world, &[0x6c, 0x36, 0x4c]);
                    set_mode(e, m);
                    set_mode_time(e, 0);
                    if m == 0x36 {
                        // 0x00404ec0: a leap: twice the offset (0x00402db0 mulFix by 2.0), up 15.
                        let v: Vec<f32> = (0..3).map(|i| (t_pos[i].wrapping_sub(my_pos[i]).wrapping_mul(0x20000) / 0x10000) as f32 * K).collect();
                        set_vec3f(&mut e.0, 0x24, [v[0], v[1], v[2]]);
                        wf32(&mut e.0, 0x2c, 15.0);
                    }
                }
                break 'choice;
            }
            0x77 => {
                // 0x00403f07: the boss's moves by its appearance parts.
                if mode_time(e) > total(e) {
                    let mut list: Vec<u8> = Vec::new();
                    let a7c = u16_at(&e.0, 0x7c);
                    list.push(if matches!(a7c, 0x863..=0x865) { 0x45 } else { 0x44 });
                    let a86 = u16_at(&e.0, 0x86);
                    if matches!(a86, 0x867 | 0x868) {
                        list.push(0x46);
                    }
                    let a80 = u16_at(&e.0, 0x80);
                    list.push(if matches!(a80, 0x85f | 0x860) { 0x4b } else { 0x4a });
                    let a84 = u16_at(&e.0, 0x84);
                    if matches!(a84, 0x85c | 0x85d) {
                        list.push(0x4d);
                        list.push(0x4e);
                    }
                    if matches!(a84, 0x857..=0x85b) {
                        list.push(0x4c);
                    }
                    let m = e.0[0x58];
                    if m != 0x47 && m != 0x48 && a84 == 0x855 && world.rng.rand() % 2 != 0 {
                        list.push(0x47);
                    }
                    let m = e.0[0x58];
                    if m != 0x47 && m != 0x48 && a84 == 0x856 && world.rng.rand() % 2 != 0 {
                        list.push(0x48);
                    }
                    if e.0[0x58] != 0x49 {
                        list.push(0x49);
                    }
                    if !list.is_empty() {
                        let m = pick(world, &list);
                        set_mode(e, m);
                        set_mode_time(e, 0);
                    }
                }
                break 'choice;
            }
            _ => {}
        }
        if is_mage(e) {
            // 0x00404199: casters within 60 blocks.
            if !(3600.0f32 > dsq) {
                break 'choice;
            }
            if f32_at(&e.0, 0x160) > 0.5f32 && world.rng.rand() % 2 != 0 {
                choose_special(e, &st, guard, haste);
            } else {
                choose_basic(e, guard, haste);
            }
            if matches!(e.0[0x58], 0x1e..=0x21) {
                // 0x004041f6: a charged spell keeps the point it was aimed at.
                let w = skill_windup(e, guard, haste, -1);
                if mode_time(e).wrapping_sub(elapsed) > w {
                    let a = diff_blocks(ai.spell_target, my_pos);
                    (ax, ay, az) = (a[0], a[1], a[2]);
                } else {
                    // The target's render position (`creature+0x1350`), which on the server
                    // is the position the physics left last tick: the entity position here.
                    ai.spell_target = t_pos;
                }
            }
            if e.0[0x58] == 0x1c && dsq > 25.0f32 {
                // 0x00404279: the beam circles the target.
                let f = mode_time(e) as f32 * 0.005f32;
                ax = (cw_math::cos(f64::from(f)) as f32) * 1.5f32 + ax;
                ay = (cw_math::sin(f64::from(f)) as f32) * 1.5f32 + ay;
            }
            break 'choice;
        }
        let app = e.0[0x6e];
        if ty == 0x65 {
            // 0x004042ec
            if 64.0f32 > dsq {
                lunge_or_jump(world, e, t_pos, my_pos, 524288.0f32, 30.0f32);
            } else if 3600.0f32 > dsq && mode_time(e) > total(e) {
                set_mode_time(e, 0);
                set_mode(e, 0x25);
                // The aim written here (the offset raised by 0.4 target heights) is overwritten
                // below (0x00404f21).
                let lift = [fix(0.0), fix(0.0), fix(f32_at(&te.0, 0x78) * 0.4f32)];
                let p = [t_pos[0].wrapping_sub(my_pos[0]).wrapping_add(lift[0]), t_pos[1].wrapping_sub(my_pos[1]).wrapping_add(lift[1]), t_pos[2].wrapping_sub(my_pos[2]).wrapping_add(lift[2])];
                set_vec3f(&mut e.0, 0x150, pos_blocks(p));
            }
            break 'choice;
        }
        if app & 2 != 0 && app & 0x10 != 0 {
            // 0x0040453d: flyers.
            let f = u16_at(&e.0, 0x114) & 0xfffb;
            w16(&mut e.0, 0x114, f);
            if 64.0f32 > dsq {
                lunge_or_jump(world, e, t_pos, my_pos, 131072.0f32, 5.0f32);
            }
            break 'choice;
        }
        let ws = e.0[WEAPON + 1];
        if ty != 0x68 && ws != 6 && ws != 7 && ws != 8 {
            // Melee.
            if app & 0x10 != 0 {
                // 0x004046ac
                if mode_time(e) > total(e) {
                    if ty == 0x19 && can_use(e, &st, 0x48) {
                        set_mode(e, 0x48);
                    } else {
                        set_mode(e, 0x4b);
                    }
                    set_mode_time(e, 0);
                }
                break 'choice;
            }
            if app & 8 != 0 {
                choose_basic(e, guard, haste);
                break 'choice;
            }
            let r3 = (my_sx(e) + f32_at(&te.0, 0x70)) * 3.0f32;
            if !(r3 * r3 > dsq) {
                break 'choice;
            }
            let m = e.0[0x58];
            if m != 0x3b && m != 0x3f && m != 8 && i32_at(&e.0, 0x118) == 0 {
                // 0x00404752: one time in ten a dodge roll.
                if world.rng.rand() % 10 == 0 && !elite(e) && 2.0f32 > my_sx(e) && !(mode_time(e) < total(e)) && u32_at(&e.0, 0x4c) & 3 != 0 {
                    dodge(world, e, t_pos, my_pos);
                    break 'choice;
                }
            }
            // 0x00404966
            let mp = f32_at(&e.0, 0x160);
            let m = e.0[0x58];
            if 0.8f64 > f64::from(mp) && m != 0x3b && m != 0x3f && m != 8 {
                choose_basic(e, guard, haste);
                break 'choice;
            }
            if i32_at(&e.0, 0x118) != 0 || !(mode_time(e) > total(e)) {
                break 'choice;
            }
            if ws == 5 {
                set_mode(e, 5);
                set_mode_time(e, 0);
                break 'choice;
            }
            let wt = e.0[WEAPON];
            let charged = f32_at(&e.0, 0x134);
            let leap = |e: &mut EntityData| {
                let v = f32_at(&e.0, 0x2c) + 20.0f32;
                wf32(&mut e.0, 0x2c, v);
                set_mode(e, 0x36);
                set_mode_time(e, 0);
            };
            if wt != 0 && e.0[OFFHAND] == 0 {
                // 0x004049e3: a lone weapon: charge (0x3b) then release.
                if e.0[0x58] == 0x3b && !(charged < mp) {
                    if e.0[0x131] == 1 {
                        set_mode(e, 0x3d);
                        set_mode_time(e, 0);
                    } else {
                        set_mode(e, 0x3c);
                        let w = skill_windup(e, guard, haste, -1);
                        set_mode_time(e, w);
                    }
                } else if can_use(e, &st, 0x36) {
                    leap(e);
                } else {
                    if e.0[0x58] != 0x3b {
                        set_mode_time(e, 0);
                    }
                    set_mode(e, 0x3b);
                }
                break 'choice;
            }
            // 0x00404a6d: by the off hand.
            match e.0[OFFHAND + 1] {
                0xd => {
                    if e.0[0x58] == 8 {
                        if !(charged < mp) {
                            set_mode(e, 0x68);
                            set_mode_time(e, 0);
                            break 'choice;
                        }
                    } else {
                        set_mode_time(e, 0);
                    }
                    set_mode(e, 8);
                }
                3 => {
                    set_mode(e, 0x11);
                    set_mode_time(e, 0);
                }
                4 => {
                    set_mode(e, 0x14);
                    set_mode_time(e, 0);
                }
                _ => {
                    if wt != 0 {
                        if e.0[0x58] == 0x3f && !(charged < mp) {
                            set_mode(e, 0xb);
                            let w = skill_windup(e, guard, haste, -1);
                            set_mode_time(e, w);
                        } else if can_use(e, &st, 0x36) {
                            leap(e);
                        } else {
                            if e.0[0x58] != 0x3f {
                                set_mode_time(e, 0);
                            }
                            set_mode(e, 0x3f);
                        }
                    } else if can_use(e, &st, 0x36) {
                        leap(e);
                    } else {
                        choose_special(e, &st, guard, haste);
                    }
                }
            }
            break 'choice;
        }
        // 0x00404b4f: bows, crossbows, boomerangs and type 0x68 within 60 blocks.
        if !(3600.0f32 > dsq) {
            break 'choice;
        }
        let m = e.0[0x58];
        if m != 0x18 && m != 0x19 && m != 0x1b && i32_at(&e.0, 0x118) == 0 {
            if world.rng.rand() % 10 == 0 && !elite(e) && 2.0f32 > my_sx(e) && !(mode_time(e) < total(e)) && u32_at(&e.0, 0x4c) & 3 != 0 {
                dodge(world, e, t_pos, my_pos);
                break 'choice;
            }
        }
        // 0x00404cdf
        let mp = f32_at(&e.0, 0x160);
        let m = e.0[0x58];
        if 0.8f64 > f64::from(mp) && m != 0x18 && m != 0x19 && m != 0x1b {
            choose_basic(e, guard, haste);
            break 'choice;
        }
        if i32_at(&e.0, 0x118) != 0 || !(mode_time(e) > total(e)) {
            break 'choice;
        }
        let charged = f32_at(&e.0, 0x134);
        let wt = e.0[WEAPON];
        // Charge-then-shoot pairs: (charge, release).
        let pair = match (wt, ws) {
            (0, _) => {
                if ty == 0x68 {
                    Some((0x19u8, 0x37u8))
                } else {
                    None
                }
            }
            (_, 7) => Some((0x18, 0x16)),
            (_, 8) => Some((0x1b, 0x1a)),
            (_, 6) => Some((0x19, 0x37)),
            _ => {
                if ty == 0x68 {
                    Some((0x19, 0x37))
                } else {
                    None
                }
            }
        };
        if let Some((charge, release)) = pair {
            if e.0[0x58] == charge && !(charged < mp) {
                set_mode(e, release);
                set_mode_time(e, 0);
            } else {
                if e.0[0x58] != charge {
                    set_mode_time(e, 0);
                }
                set_mode(e, charge);
            }
        }
    }
    // 0x00404f21: the aim.
    set_vec3f(&mut e.0, 0x150, [ax, ay, az]);
}

fn my_sx(e: &EntityData) -> f32 {
    f32_at(&e.0, 0x70)
}

/// `0x00404329`/`0x00404582`: after two seconds in the current mode, one time in two (or when
/// more than `drop` fixed units above the target) a lunge (mode 0x33) at 10 blocks/s towards
/// the target, else a jump of `jump` blocks/s. The creature's aim was set first (and is set
/// again at 0x00404f21).
fn lunge_or_jump(world: &mut World, e: &mut EntityData, t_pos: [i64; 3], my_pos: [i64; 3], drop: f32, jump: f32) {
    set_vec3f(&mut e.0, 0x150, diff_blocks(t_pos, my_pos));
    let f = u16_at(&e.0, 0x114) | 4;
    w16(&mut e.0, 0x114, f);
    if mode_time(e) <= 2000 {
        return;
    }
    if world.rng.rand() % 2 != 0 {
        let above = my_pos[2].wrapping_sub(t_pos[2]);
        if above <= drop as i64 {
            wf32(&mut e.0, 0x2c, jump);
            return;
        }
    }
    set_mode_time(e, 0);
    set_mode(e, 0x33);
    let v = diff_blocks(t_pos, my_pos);
    set_vec3f(&mut e.0, 0x24, v);
    if (v[0] * v[0] + v[1] * v[1]) + v[2] * v[2] > 0.0 {
        let s = 1.0f32 / (f64::from((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt() as f32);
        let n = [v[0] * s, v[1] * s, v[2] * s];
        set_vec3f(&mut e.0, 0x24, [n[0] * 10.0f32, n[1] * 10.0f32, n[2] * 10.0f32]);
    }
}

/// `0x004047a8`/`0x00404be0`: a dodge roll (600 ms) forwards, backwards or to either side of
/// the horizontal direction to the target, at 20 blocks/s and 5 up; nothing when the target
/// is straight above or below.
fn dodge(world: &mut World, e: &mut EntityData, t_pos: [i64; 3], my_pos: [i64; 3]) {
    let v = diff_blocks(t_pos, my_pos);
    let f = (v[1] * v[1] + v[0] * v[0]) + 0.0f32 * 0.0f32;
    if !(f > 0.0) {
        return;
    }
    let s = 1.0f32 / (f64::from(f).sqrt() as f32);
    let nx = v[0] * s;
    let ny = v[1] * s;
    let nz = s * 0.0f32;
    let r = world.rng.rand() % 4;
    match r {
        0 => set_vec3f(&mut e.0, 0x24, [nx * 20.0f32, ny * 20.0f32, nz * 20.0f32]),
        1 => set_vec3f(&mut e.0, 0x24, [nx * -20.0f32, ny * -20.0f32, nz * -20.0f32]),
        2 => {
            let a = nz * 0.0f32;
            set_vec3f(&mut e.0, 0x24, [(ny - a) * 20.0f32, (a - nx) * 20.0f32, (nx * 0.0f32 - ny * 0.0f32) * 20.0f32]);
        }
        3 => {
            let a = nz * 0.0f32;
            set_vec3f(&mut e.0, 0x24, [(ny - a) * -20.0f32, (a - nx) * -20.0f32, (nx * 0.0f32 - ny * 0.0f32) * -20.0f32]);
        }
        _ => {}
    }
    let z = f32_at(&e.0, 0x2c) + 5.0f32;
    wf32(&mut e.0, 0x2c, z);
    wi32(&mut e.0, 0x118, 600);
}

// ---------------------------------------------------------------------------------------------
// World::creatureAttack.

/// `World::creatureAttack`, `Server.exe 0x004cfd50`: `attacker`'s hit of `damage` landing on
/// `target` (the original's `this` is the world; `_p7` and `_p13` are unused there).
/// `critical` is written to the Hit (+0x14) and plays the target type's pain sound; `strong`
/// (+0x44) doubles the spirit particles and enables the spirit effects; `stun_factor` scales the
/// stun roll; `dir` is the knockback; `magic` selects resistance over armour and the spell
/// sounds; `hit_kind` 0x11 adds a type-4 buff (damage × 0.1, 3 s) and 0x1c mutes the weapon
/// sound; `show` enables the guard block and the hit sounds.
///
/// Returns false when nothing lands (dead target, a dodging attacker mode 0x5b/0x4a against an
/// airborne target, a monster's deflection (flag 0x80 or physics flag 0x20: a type-3 Hit), a
/// guard block (a type-1 Hit)); true otherwise, including a hit a shield buff absorbs whole.
/// Otherwise: the shield buffs (type 6) absorb their share (a type-5 Hit), armour or resistance
/// reduce the rest by the attacker's penetration (`1 - min(1, entity+0x60 / maxBlock)`), the
/// weapon spirits add damage, life (a negative Hit on the attacker, not applied here), and the
/// make-blue / speed-up timers, the sounds play, type-1 buffs multiply the damage, the stun
/// roll `min(1, 2^((levelCurve(attacker level) - levelCurve(target level)) * 10) * factor) *
/// 0.9` (factor 1 for some modes, × 0.15 against elites, × 0.1 from non-elite non-players) may
/// stun (0 with a type-1 buff), and the Hit is recorded and applied with
/// [`crate::combat::apply_hit`].
pub fn creature_attack(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, target: i64, attacker: Option<i64>, damage: f32, critical: bool, strong: bool, stun_factor: f32, _p7: u32, dir: [f32; 3], out: &mut ServerUpdate, dirty: &mut std::collections::BTreeSet<(i32, i32)>, verbose: bool, magic: bool, hit_kind: i32, _p13: u32, show: bool) -> bool {
    let Some(t0) = entities.get(&target) else { return false };
    // 0x004cfd9f: a dead target takes nothing (the local-player check is client-only).
    if 0.0 >= f32_at(&t0.0, 0x15c) {
        return false;
    }
    let t_pos = pos_at(&t0.0);
    let att = attacker.and_then(|a| entities.get(&a).cloned());
    let aid = attacker.unwrap_or(0);
    let mut dmg;
    let mut factor = stun_factor;
    match &att {
        None => {
            // 0x004d0e5d: an unattributed hit.
            let _ = world.rng.rand();
            states.entry(target).or_default().ai.hit_flash = 0.5;
            dmg = damage;
        }
        Some(a) => {
            let am = a.0[0x58];
            let t = entities[&target].clone();
            // 0x004cfddc: dodging attacks miss airborne targets.
            if (am == 0x5b || am == 0x4a) && u32_at(&t.0, 0x4c) & 1 == 0 {
                return false;
            }
            if am == 0x1c {
                // 0x004cfdf9: the beam's sparks.
                let scale = (world.rng.rand() as f32 * 0.1f32) / 32767.0f32 + 0.1f32;
                out.particles.push(particle(t_pos, [0.0, 0.0, 10.0], [0.6, 0.6, 1.0, 1.0], scale, 8, 1, 3.0));
            }
            if t.0[0x50] == 1 && (u16_at(&t.0, 0x114) & 0x80 != 0 || u32_at(&t.0, 0x4c) & 0x20 != 0) {
                // 0x004cfefe: a monster deflects: sound 0x18 and a type-3 Hit.
                let pitch = (world.rng.rand() as f32 * 0.3f32) / 32767.0f32 + 1.0f32;
                out.sounds.push(sound_at(pos_blocks(t_pos), 0x18, pitch, 1.0));
                let hit = hit_record(aid, target, damage, t_pos, dir, 3);
                out.hits.push(hit.clone());
                apply_hit(world, entities, states, &hit, out, dirty, verbose);
                return false;
            }
            if show && is_blocking(&t) && f32_at(&t.0, 0x164) > 0.0 {
                // 0x004d0069: a guard block: mana and charge from the guard, a type-1 Hit.
                let x = damage / block_strength(&t);
                let hit;
                {
                    let te = ent(entities, target);
                    let bp = f32_at(&te.0, 0x164);
                    let mp = bp + f32_at(&te.0, 0x160);
                    let ch = bp + f32_at(&te.0, 0x134);
                    wf32(&mut te.0, 0x160, mp);
                    wf32(&mut te.0, 0x134, ch);
                    if mp > 1.0f32 {
                        wf32(&mut te.0, 0x160, 1.0);
                    }
                    let mp2 = f32_at(&te.0, 0x160);
                    if ch > mp2 {
                        wf32(&mut te.0, 0x134, mp2);
                    }
                    hit = hit_record(aid, target, damage, t_pos, [dir[0] * 0.5f32, dir[1] * 0.5f32, dir[2] * 0.5f32], 1);
                    let nb = f32_at(&te.0, 0x164) - x;
                    wf32(&mut te.0, 0x164, nb);
                    if -1.0f32 > nb {
                        wf32(&mut te.0, 0x164, -1.0);
                    }
                }
                out.hits.push(hit.clone());
                let pitch = (world.rng.rand() as f32 * 0.3f32) / 32767.0f32 + 1.0f32;
                out.sounds.push(sound_at(pos_blocks(t_pos), 0x18, pitch, 1.0));
                apply_hit(world, entities, states, &hit, out, dirty, verbose);
                return false;
            }
            // 0x004d0255: shield buffs absorb up to their value.
            let mut shield = 0.0f32;
            if let Some(s) = states.get(&target) {
                for b in &s.buffs {
                    if b[0] == 6 {
                        shield = shield + f32_at(b, 4);
                    }
                }
            }
            let mut r = i32_at(&a.0, 0x60) as f32 / max_block(a) as f32;
            if r > 1.0f32 {
                r = 1.0;
            }
            let pen = 1.0f32 - r;
            let absorb = pen * shield;
            if absorb > 0.0 {
                let hit = hit_record(aid, target, damage, t_pos, [dir[0] * 0.5f32, dir[1] * 0.5f32, dir[2] * 0.5f32], 5);
                out.hits.push(hit.clone());
                let pitch = (world.rng.rand() as f32 * 0.3f32) / 32767.0f32 + 1.0f32;
                out.sounds.push(sound_at(pos_blocks(t_pos), 0x5b, pitch, 1.0));
                apply_hit(world, entities, states, &hit, out, dirty, verbose);
                dmg = damage - absorb;
                if !(0.0 < dmg) {
                    return true;
                }
            } else {
                dmg = damage;
            }
            // 0x004d052b: armour or resistance, by the penetration.
            let t = entities[&target].clone();
            let def = if magic { resistance(&t) } else { armor(&t) };
            dmg = dmg - pen * def;
            if 0.0 > dmg {
                dmg = 0.0;
            }
            let ratio = i32_at(&a.0, 0x60) as f32 / max_block(a) as f32;
            let items = strike_items(a);
            if ratio > 0.25f32 {
                // 0x004d05aa: spirit particles along the knockback.
                let (mut x, mut y) = (dir[0], dir[1]);
                let l = (dir[1] * dir[1] + dir[0] * dir[0]) + dir[2] * dir[2];
                if l > 0.0 {
                    let s = 1.0f32 / (f64::from(l).sqrt() as f32);
                    x = dir[0] * s;
                    y = s * dir[1];
                }
                let vel = [x * 4.0f32, y * 4.0f32, 5.0f32];
                for o in &items {
                    spirit_particles(t_pos, vel, ratio, strong, &a.0[*o..*o + 0x118], out);
                }
            }
            if strong && ratio > 0.0 && !items.is_empty() {
                // 0x004d0732: spirit effects: damage, life, make-blue and speed-up.
                let k = 0.1f32 / items.len() as f32;
                let mut heal = 0.0f32;
                for o in &items {
                    let a_now = entities[&aid].clone();
                    let it = &a_now.0[*o..*o + 0x118];
                    for j in 0..i32_at(it, 0x114).max(0) as usize {
                        let kind = it[0x17 + j * 8];
                        let lv = i32_at(it, 0x18 + j * 8) as f32;
                        match kind {
                            0x80 => dmg = level_curve(lv) * k * ratio + dmg,
                            0x81 => {
                                let v = level_curve(lv) * k * ratio;
                                dmg = v * 0.5f32 + dmg;
                                heal = v * 2.0f32 + heal;
                            }
                            0x82 => {
                                dmg = level_curve(lv) * (k * 0.25f32) * ratio + dmg;
                                let te = ent(entities, target);
                                let v = (ratio * 500.0f32 + i32_at(&te.0, 0x124) as f32) as i32;
                                wi32(&mut te.0, 0x124, v);
                            }
                            0x83 => {
                                dmg = level_curve(lv) * (k * 0.25f32) * ratio + dmg;
                                let ae = ent(entities, aid);
                                let v = (ratio * 500.0f32 + i32_at(&ae.0, 0x128) as f32) as i32;
                                wi32(&mut ae.0, 0x128, v);
                            }
                            _ => {}
                        }
                    }
                }
                {
                    let te = ent(entities, target);
                    if i32_at(&te.0, 0x124) > 5000 {
                        wi32(&mut te.0, 0x124, 5000);
                    }
                }
                let ae = ent(entities, aid);
                if i32_at(&ae.0, 0x128) > 5000 {
                    wi32(&mut ae.0, 0x128, 5000);
                }
                if heal > 0.0 {
                    // 0x004d09b6: the life stolen: a negative Hit on the attacker (recorded
                    // only) and the HP added directly.
                    let hit = hit_record(aid, aid, -heal, pos_at(&ae.0), [0.0; 3], 0);
                    out.hits.push(hit);
                    let hp = f32_at(&ae.0, 0x15c) + heal;
                    wf32(&mut ae.0, 0x15c, hp);
                }
            }
            // 0x004d0ae1: the hit flash and the hit sounds.
            let am = entities[&aid].0[0x58];
            let flash = &mut states.entry(target).or_default().ai.hit_flash;
            let sounds = if am == 0x1c {
                if 0.3f32 > *flash {
                    *flash = 0.3;
                }
                show
            } else if show {
                *flash = 1.0;
                true
            } else {
                false
            };
            if sounds {
                let a_now = entities[&aid].clone();
                if magic {
                    if a_now.0[0x131] == 1 {
                        out.sounds.push(sound_at(pos_blocks(t_pos), 0x2a, 1.0, 1.0));
                    } else {
                        out.sounds.push(sound_at(pos_blocks(t_pos), 0x27, 1.5, 1.0));
                    }
                } else if hit_kind != 0x1c {
                    // 0x004d0c84: the weapon's sound by the first struck item's sub type.
                    let items = strike_items(&a_now);
                    let mut sub: i32 = items.first().map_or(-1, |o| i32::from(a_now.0[*o + 1]));
                    if a_now.0[0x58] == 0x44 {
                        sub = 0x11;
                    }
                    let mut pitch = (world.rng.rand() as f32 * 0.2f32) / 32767.0f32 + 0.9f32;
                    let kind = match sub {
                        0 | 1 => {
                            pitch = pitch + 0.1f32;
                            if critical { 2 } else { 1 }
                        }
                        0xf | 0x10 => {
                            if critical {
                                2
                            } else {
                                1
                            }
                        }
                        3 | 4 => {
                            if critical {
                                8
                            } else {
                                7
                            }
                        }
                        5 => {
                            if critical {
                                4
                            } else {
                                3
                            }
                        }
                        6..=8 => {
                            if critical {
                                10
                            } else {
                                9
                            }
                        }
                        _ => {
                            if critical {
                                6
                            } else {
                                5
                            }
                        }
                    };
                    let kind = match a_now.0[0x58] {
                        0x36 => 0xb,
                        0xa => 5,
                        _ => kind,
                    };
                    out.sounds.push(sound_at(pos_blocks(t_pos), kind, pitch, 1.0));
                }
            }
        }
    }
    // 0x004d0e7e: the knockback never slows the target.
    {
        let te = ent(entities, target);
        let v = vec3f_at(&te.0, 0x24);
        let mut m = (v[0] * v[0] + v[1] * v[1]) + v[2] * v[2];
        let dd = (dir[0] * dir[0] + dir[1] * dir[1]) + dir[2] * dir[2];
        if dd > m {
            m = dd;
        }
        if (v[0] * v[0] + v[1] * v[1]) + v[2] * v[2] > m {
            let s = 1.0f32 / (f64::from((v[1] * v[1] + v[0] * v[0]) + v[2] * v[2]).sqrt() as f32);
            let n = [s * v[0], s * v[1], s * v[2]];
            let l = f64::from(m).sqrt() as f32;
            set_vec3f(&mut te.0, 0x24, [l * n[0], l * n[1], l * n[2]]);
        }
    }
    // 0x004d0fc8: the Hit record; type-1 buffs multiply the damage.
    if let Some(s) = states.get(&target) {
        for b in &s.buffs {
            if b[0] == 1 {
                dmg = f32_at(b, 4) * dmg;
            }
        }
    }
    let mut hit = hit_record(aid, target, dmg, t_pos, dir, 0);
    hit.0[0x14] = u8::from(critical);
    hit.0[0x44] = u8::from(strong);
    if hit_kind == 0x11 {
        // 0x004d10e5: buff 4 (damage × 0.1, 3 s) appended to the target's list, and relayed.
        let mut b: Buff = [0; 0x18];
        b[0] = 4;
        wf32(&mut b, 4, dmg * 0.1f32);
        w32(&mut b, 8, 3000);
        w64(&mut b, 0x10, aid);
        states.entry(target).or_default().buffs.push(b);
        out.passives.push(passive_record(aid, target, &b));
    }
    let t = entities[&target].clone();
    if critical && t.0[0x50] != 6 {
        // 0x004d11ff: the target type's pain sound.
        let pitch = (world.rng.rand() as f32 * 0.1f32) / 32767.0f32 + 1.0f32;
        let kind = match i32_at(&t.0, 0x54) {
            0 => Some(0x3c),
            1 => Some(0x3d),
            2 | 0x2b => Some(0x3e),
            3 | 0x2d => Some(0x3f),
            4 => Some(0x40),
            5 => Some(0x41),
            7 => Some(0x42),
            8 => Some(0x43),
            9 => Some(0x44),
            10 => Some(0x45),
            0xb => Some(0x46),
            0xc => Some(0x47),
            0xd => Some(0x4a),
            0xe => Some(0x4b),
            0xf => Some(0x48),
            0x10 => Some(0x49),
            0x25..=0x28 => Some(0x4f),
            0x2e | 0x6c | 0x6d | 0x72 | 0x73 => Some(0x4d),
            0x30 | 0x33 | 0x57 => Some(0x4e),
            0x60 => Some(0x50),
            0x77 => Some(0x4c),
            _ => None,
        };
        if let Some(k) = kind {
            out.sounds.push(sound_at(pos_blocks(t_pos), k, pitch, 1.0));
        }
    }
    if let Some(a) = &att {
        let a_now = entities.get(&aid).cloned().unwrap_or_else(|| a.clone());
        if i32_at(&t.0, 0x11c) < -3000 && t.0[0x50] != 6 {
            // 0x004d1432: the stun roll.
            let am = a_now.0[0x58];
            if matches!(am, 0x3a | 0xc | 0x44 | 0x5d | 0x45) {
                factor = 1.0;
            }
            if elite(&t) {
                factor = factor * 0.15f32;
            }
            if !elite(&a_now) && a_now.0[0x50] != 0 {
                factor = factor * 0.1f32;
            }
            if am == 0x5b || am == 0x4a {
                factor = 1.0;
            }
            if t.0[0x58] == 0x54 {
                factor = 1.0;
            }
            let x = (level_curve(i32_at(&a_now.0, 0x180) as f32) - level_curve(i32_at(&t.0, 0x180) as f32)) * 10.0f32;
            let mut p = pow(2.0, f64::from(x)) as f32 * factor;
            if p > 1.0f32 {
                p = 1.0;
            }
            let mut chance = p * 0.9f32;
            if let Some(s) = states.get(&target) {
                for b in &s.buffs {
                    if b[0] == 1 {
                        chance = 0.0;
                    }
                }
            }
            if chance > world.rng.rand() as f32 / 32767.0f32 {
                wi32(&mut hit.0, 0x18, stun_duration(&t));
                wf32(&mut hit.0, 0x40, 10.0);
                out.sounds.push(sound_at(pos_blocks(t_pos), 0x17, 1.0, 1.0));
            }
        }
        if a_now.0[0x58] == 0x1c {
            hit.0[0x46] = 1;
        }
    }
    out.hits.push(hit.clone());
    apply_hit(world, entities, states, &hit, out, dirty, verbose);
    true
}

// ---------------------------------------------------------------------------------------------
// The straight-walk probe (0x0052ef00).

/// `Server.exe 0x0052ef00` (`World` method; args `creature`, `from*`, `to*` (3×i64 16.16),
/// `float radius`; returns `al`): can the creature walk in a straight line from `from` to
/// `to`? Called by `CombatBehavior` 0x00402f40 at 0x004050b1 as
/// `(creature, &creature.pos, &target.pos, reach)`.
///
/// It simulates the walk: the leg is capped at 50 blocks in xy (the step count is
/// `trunc(sqrt(len² + 0.5625) + 1)` of the capped leg), and each step re-aims at `to` with a unit
/// xy direction and moves the box one axis at a time: x and y by the direction, z by -0.71
/// (gravity probe). Each axis move is kept unless:
/// - the box (entity scale 0x70) touches a solid block (`path::box_collides` 0x004d4f90, no statics): on z the
///   move is undone (standing); on x/y the creature steps up if its appearance flags (0x6e)
///   lack 0x100 and the box raised by 1.01 blocks touches no solid block (then its feet are
///   put 2.02 blocks above the floor of the block holding them: see below), otherwise the
///   move is undone;
/// - (not blocked, or the z axis) it runs into a static of the 3x3 cells around it (kinds other
///   than 7/6/9; kinds 1/8/2/3/5 only with byte +0x30 set; not in `ignored_statics`) while
///   moving towards the static's centre on that axis: the move is undone (no step-up).
///
/// Returns true at once (before the walk, and at the start of every step) when the position
/// is within `radius` of `to` horizontally (`r² > d²` before the walk, `r² >= d²` in it) and
/// `|dz| < scale.z + radius`. Otherwise, after all steps, returns whether the walk ended more
/// than 25 blocks (3-D, `d² > 625`) from `from`: false for a walk that got stuck close to its
/// start (or had no steps).
pub fn walk_line(world: &World, e: &EntityData, st: &CreatureState, from: [i64; 3], to: [i64; 3], radius: f32) -> bool {
    // Two independent ports of 0x0052ef00 were made (this module's and `path.rs`'s) and agree;
    // the path module's is kept.
    crate::path::walk_direct(world, e, st, from, to, radius)
}
