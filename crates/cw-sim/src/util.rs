//! Small helpers shared by the simulation modules: byte access into the entity block, 16.16
//! fixed-point and vector arithmetic, block lookups and a few creature predicates. Each was
//! once copied into every module that needed it; the copies were identical and live here now.
//! Doc comments cite the original as `Server.exe 0x<address>` where one exists.

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::assign_op_pattern, clippy::excessive_precision)]

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Damage, Passive};
use cw_world::World;
use cw_world::zone::Static;

use crate::combat::{Buff, CreatureState, is_hostile};
use crate::skills::{attack_speed, skill_level_factor};
use crate::stats::pow;

/// 2^-16, the fixed-point block scale.
pub(crate) const K: f32 = 1.5258789e-05;

/// Weapon (slot 7) and off-hand (slot 6) item offsets in the entity block.
pub(crate) const WEAPON: usize = 0x2f0 + 7 * 0x118;
pub(crate) const OFFHAND: usize = 0x2f0 + 6 * 0x118;

// ---------------------------------------------------------------------------------------------
// Byte helpers.

pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub(crate) fn i64_at(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub(crate) fn w16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn w32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn wi32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn wf32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn w64(b: &mut [u8], o: usize, v: i64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn vec3f_at(b: &[u8], o: usize) -> [f32; 3] {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}

pub(crate) fn set_vec3f(b: &mut [u8], o: usize, v: [f32; 3]) {
    for (i, x) in v.iter().enumerate() {
        wf32(b, o + i * 4, *x);
    }
}

pub(crate) fn pos_at(b: &[u8]) -> [i64; 3] {
    [i64_at(b, 0), i64_at(b, 8), i64_at(b, 16)]
}

pub(crate) fn pos_of(e: &EntityData) -> [i64; 3] {
    [i64_at(&e.0, 0), i64_at(&e.0, 8), i64_at(&e.0, 16)]
}

pub(crate) fn set_pos(b: &mut [u8], p: [i64; 3]) {
    for (i, x) in p.iter().enumerate() {
        w64(b, i * 8, *x);
    }
}

pub(crate) fn set_accel(e: &mut EntityData, a: [f32; 3]) {
    for (i, v) in a.iter().enumerate() {
        wf32(&mut e.0, 0x30 + i * 4, *v);
    }
}

// ---------------------------------------------------------------------------------------------
// Fixed point and vectors.

/// `(i64)(f * 65536)`, truncated (`__ftol2`).
pub(crate) fn fix(f: f32) -> i64 {
    (f * 65536.0f32) as i64
}

/// `vec3i64::fromFloat` 0x00402510: `fix` per axis.
pub(crate) fn fix3(v: [f32; 3]) -> [i64; 3] {
    [fix(v[0]), fix(v[1]), fix(v[2])]
}

/// `(float)v * 2^-16`.
pub(crate) fn blocks(v: i64) -> f32 {
    v as f32 * K
}

/// A fixed vector in blocks, `(float)v * 2^-16` per axis (`vec3f::fromFixed` 0x00402550; the
/// i64 goes through `fild`, exact, and one rounding to single).
pub(crate) fn blocks3(v: [i64; 3]) -> [f32; 3] {
    [blocks(v[0]), blocks(v[1]), blocks(v[2])]
}

/// `vec3f::fromFixed` 0x00402550 on a position.
pub(crate) fn pos_blocks(p: [i64; 3]) -> [f32; 3] {
    [p[0] as f32 * K, p[1] as f32 * K, p[2] as f32 * K]
}

/// `0x00402c50` then `0x00402550`: `(a - b)` per axis, each as `(float)i64 * 2^-16`.
pub(crate) fn diff_blocks(a: [i64; 3], b: [i64; 3]) -> [f32; 3] {
    [a[0].wrapping_sub(b[0]) as f32 * K, a[1].wrapping_sub(b[1]) as f32 * K, a[2].wrapping_sub(b[2]) as f32 * K]
}

pub(crate) fn add3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_add(b[0]), a[1].wrapping_add(b[1]), a[2].wrapping_add(b[2])]
}

pub(crate) fn sub3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2])]
}

/// `vec3i64ToBlock` 0x00405450: per axis `v / 65536` truncated, minus one for a negative value.
pub(crate) fn to_block(v: i64) -> i32 {
    let q = (v / 65536) as i32;
    if v < 0 { q - 1 } else { q }
}

/// `vec3i64ToBlock` 0x00405450 on a vector: `to_block` per axis.
pub(crate) fn to_block3(v: [i64; 3]) -> [i32; 3] {
    [to_block(v[0]), to_block(v[1]), to_block(v[2])]
}

/// `floorDivFix` 0x00405510: `v / 65536` truncated, minus one when the high dword is negative
/// or the high dword is zero and the low dword has its top bit set.
pub(crate) fn floor_div_fix(v: i64) -> i32 {
    let q = (v / 65536) as i32;
    let hi = (v >> 32) as i32;
    let lo = v as u32;
    if hi < 0 || (hi == 0 && (lo as i32) < 0) { q - 1 } else { q }
}

/// `lengthSq3` 0x004021b0: `(x*x + y*y) + z*z` in single precision (`mulss`/`addss`; the x87
/// return only widens the f32 result).
pub(crate) fn len_sq3(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// `length3` 0x00401d80: the squares summed in single precision, a double square root.
pub(crate) fn length3(v: [f32; 3]) -> f32 {
    f64::from((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt() as f32
}

/// `normalize3` 0x00401fb0 (also `normalized` 0x00412670 into a new vector): `1 / sqrt(len²)`
/// in single precision.
pub(crate) fn normalize3(v: &mut [f32; 3]) {
    let s = 1.0f32 / (f64::from(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() as f32);
    v[0] *= s;
    v[1] *= s;
    v[2] *= s;
}

/// `lerpRepeat3` 0x0052e7c0.
pub(crate) fn lerp_repeat3(v: &mut [f32; 3], target: [f32; 3], n: i32, t: f32) {
    for _ in 0..n {
        let d = [target[0] - v[0], target[1] - v[1], target[2] - v[2]];
        v[0] = v[0] + d[0] * t;
        v[1] = d[1] * t + v[1];
        v[2] = d[2] * t + v[2];
    }
}

/// The rotation part of `identity().rotateZ(deg)` applied to `v` (0x00402030 then
/// 0x004dde00): a rotation about z by `deg` degrees.
pub(crate) fn rotate_z(deg: f32, v: [f32; 3]) -> [f32; 3] {
    // 0x00402030: `_libm_sse2_cos_precise` / `_libm_sse2_sin_precise` of the double of the
    // float angle; `cw_math` reproduces MSVCR110's results.
    let r = f64::from(deg * 0.0174532924f32);
    let (c, s) = (cw_math::cos(r) as f32, cw_math::sin(r) as f32);
    // Matrix after rotateZ (column-major as the original stores it):
    // m0 = c, m1 = s, m4 = -s, m5 = c; the product 0x004dde00 is
    // out.x = m4*y + x*m0 + m8*z, out.y = m1*x + m5*y + m9*z, out.z = m2*x + m6*y + m10*z.
    [(-s) * v[1] + v[0] * c, s * v[0] + c * v[1], v[2]]
}

// ---------------------------------------------------------------------------------------------
// Blocks and statics.

pub(crate) fn block_type(b: [u8; 4]) -> u8 {
    b[3] & 0x1f
}

/// `blockIsSolid` 0x004061f0: neither air (0) nor water (2).
pub(crate) fn solid(b: [u8; 4]) -> bool {
    let t = block_type(b);
    t != 0 && t != 2
}

/// `getBlock` on a fixed position, `Server.exe 0x00406050` (`getBlockFixed`): per axis a value
/// whose high dword is negative, or zero with the low dword's top bit set, first loses 0x10000;
/// then the truncated `v / 65536` goes to `getBlock` 0x00405fd0 (`World::block`).
pub(crate) fn block_fix(world: &World, p: [i64; 3]) -> [u8; 4] {
    let c = p.map(|v| {
        let hi = (v >> 32) as i32;
        let lo = v as u32 as i32;
        let v = if hi < 0 || (hi == 0 && lo < 0) { v.wrapping_sub(0x10000) } else { v };
        (v / 65536) as i32
    });
    world.block(c[0], c[1], c[2])
}

/// The statics of the 8x8-block cell `(cx, cy)` in the order the zone holds them
/// (`getStaticCell` 0x0041c9e0 over the zone's spatial index at `zone+0xac`).
pub(crate) fn cell_statics(world: &World, cx: i32, cy: i32) -> Vec<(usize, Static)> {
    let (bx, by) = (cx * 8, cy * 8);
    if !(0..0x1000000).contains(&bx) || !(0..0x1000000).contains(&by) {
        return Vec::new();
    }
    let Some(zone) = world.zone(bx / 256, by / 256) else { return Vec::new() };
    zone.statics.iter().enumerate().filter(|(_, s)| (s.x >> 16) as i32 / 8 == cx && (s.y >> 16) as i32 / 8 == cy).map(|(i, s)| (i, s.clone())).collect()
}

// ---------------------------------------------------------------------------------------------
// Creatures.

/// `isEliteType` `Server.exe 0x0040f650`: the elite monster types whose kills are worth twenty
/// times the XP.
pub(crate) fn elite(e: &EntityData) -> bool {
    matches!(i32_at(&e.0, 0x54), 0x6c | 0x6d | 0x72 | 0x74 | 0x73 | 0x76 | 0x6b | 0x75 | 0x65 | 0x77)
}

/// `isEnemy` 0x004d18c0.
pub fn is_enemy(a: &EntityData, b: &EntityData) -> bool {
    let (ha, hb) = (a.0[0x50], b.0[0x50]);
    if ha == 6 || hb == 6 {
        return true;
    }
    let friendly = crate::combat::friendly_type;
    if ha == 1 {
        if hb != 1 || friendly(a) != friendly(b) {
            return true;
        }
    } else if hb == 1 {
        return true;
    }
    u16_at(&a.0, 0x114) & 0x20 != 0 || u16_at(&b.0, 0x114) & 0x20 != 0
}

/// `Server.exe 0x0040f610`: a blocking mode with no slow: modes 0x47, 0x48, 8, 0x62, or 0x3b/
/// 0x3f for a class-1 specialisation-1 creature.
pub fn is_blocking(e: &EntityData) -> bool {
    if i32_at(&e.0, 0x120) > 0 {
        return false;
    }
    let m = e.0[0x58];
    matches!(m, 0x47 | 0x48 | 8 | 0x62) || (matches!(m, 0x3b | 0x3f) && e.0[0x130] == 1 && e.0[0x131] == 1)
}

/// `Server.exe 0x0040f5a0`: the creature fights at range (mage, types 0x75/0x56/0x68, or a bow,
/// crossbow, staff or wand in the right hand).
pub(crate) fn is_ranged(e: &EntityData) -> bool {
    let w = &e.0[WEAPON..WEAPON + 0x118];
    let sub = w[1];
    if e.0[0x130] == 3 || matches!(i32_at(&e.0, 0x54), 0x75 | 0x56 | 0x68) || (w[0] == 3 && matches!(sub, 10..=12)) {
        return true;
    }
    w[0] == 3 && matches!(sub, 6 | 7 | 8 | 10 | 11)
}

/// `Server.exe 0x0040f690`: a spell caster: class 3, types 0x75/0x56, or a staff, wand or
/// bracelet (weapon sub types 10..12).
pub fn is_mage(e: &EntityData) -> bool {
    if e.0[0x130] == 3 || matches!(i32_at(&e.0, 0x54), 0x75 | 0x56) {
        return true;
    }
    e.0[WEAPON] == 3 && matches!(e.0[WEAPON + 1], 10..=12)
}

pub fn has_buff(st: &CreatureState, ty: u8) -> bool {
    st.buffs.iter().any(|b| b[0] == ty)
}

/// The guard and haste the skill timings take (`attackSpeed` 0x00412150 reads both: the guard
/// at `creature+0x1190` and the haste buff, type 0xc).
pub fn guard_haste(states: &BTreeMap<i64, CreatureState>, id: i64) -> (f32, bool) {
    states.get(&id).map_or((0.0, false), |s| (s.block, has_buff(s, 0xc)))
}

/// `(base / (attackSpeed * entity+0x16c))` truncated, the shape of every scaled timing.
pub(crate) fn scaled(e: &EntityData, guard: f32, haste: bool, base: f32) -> i32 {
    // `cvttss2si`: a zero speed multiplier at 0x16c gives inf, which the original converts
    // to 0x80000000; Rust's saturating cast would give i32::MAX. Neither is meaningful, and
    // the callers' sums wrap as the original's i32 adds do.
    let v = base / (attack_speed(e, guard, haste) * f32_at(&e.0, 0x16c));
    if v.is_nan() || v >= 2147483648.0f32 || v < -2147483648.0f32 { i32::MIN } else { v as i32 }
}

/// `2^((1 - 1/((level - 1) * 0.05 + 1)) * 3)` of `entity+0x180`, as 0x00408300 computes it.
pub(crate) fn level_pow(e: &EntityData) -> f32 {
    let lv = i32_at(&e.0, 0x180) as f32;
    let c = (1.0f32 - 1.0f32 / ((lv - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    pow(2.0, f64::from(c)) as f32
}

/// `Server.exe 0x0040fb20` (`skillMpCost`): the mana a mode costs: 0.1 for modes 3 and 4; 0.3
/// for the spells 0x1f, 0x21, 0x25, 0x2b, 0x2d, 0x2e and 0x5f unless a type-9 buff makes them
/// free; 0x22 costs `(1 - skillLevelFactor * 0.75) * 0.125`; everything else is free.
pub fn mana_cost(e: &EntityData, st: &CreatureState, mode: i32, level: i32) -> f32 {
    match mode {
        3 | 4 => 0.1f32,
        0x1f | 0x21 | 0x25 | 0x2b | 0x2d | 0x2e | 0x5f => {
            if has_buff(st, 9) {
                0.0
            } else {
                0.3f32
            }
        }
        0x22 => (1.0f32 - skill_level_factor(e, 0x22, level) * 0.75f32) * 0.125f32,
        _ => 0.0,
    }
}

/// `chanceRoll` `Server.exe 0x0040f220(c, chance)`: `chance * 0.5` with a weapon (slot 7, type
/// 3) of sub type 0, 1, 2 or 0xc, `* 0.3` with sub type 8 or 10; one `rand()`;
/// `rand / 32767 < chance`.
pub(crate) fn chance_roll(world: &mut World, e: &EntityData, chance: f32) -> bool {
    let mut c = chance;
    if e.0[WEAPON] == 3 {
        match e.0[WEAPON + 1] {
            0 | 1 | 2 | 0xc => c = c * 0.5f32,
            8 | 10 => c = c * 0.3f32,
            _ => {}
        }
    }
    let r = world.rng.rand();
    (r as f32) / 32767.0f32 < c
}

/// `Server.exe 0x00409270`: the weapons (entity offsets of slot 7 / slot 6 items) the current
/// mode strikes with: a bow or crossbow in either hand alone; else by mode the off hand
/// (2, 4, 7, 8, 10, 0x12, 0x27, 0x29), both hands (0xb, 0xc, 0x10, 0x11, 0x14, 0x36, 0x56,
/// 0x5b, 0x60), the right hand (0x28, 0x2a) or by default the right hand plus an off-hand sub
/// type 0xc.
pub(crate) fn strike_items(e: &EntityData) -> Vec<usize> {
    let wt = e.0[WEAPON];
    let st = e.0[OFFHAND];
    if wt == 3 && matches!(e.0[WEAPON + 1], 6 | 7) {
        return vec![WEAPON];
    }
    if st == 3 && matches!(e.0[OFFHAND + 1], 6 | 7) {
        return vec![OFFHAND];
    }
    let mut v = Vec::new();
    match e.0[0x58] {
        2 | 4 | 7 | 8 | 10 | 0x12 | 0x27 | 0x29 => {
            if st == 3 {
                v.push(OFFHAND);
            }
        }
        0xb | 0xc | 0x10 | 0x11 | 0x14 | 0x36 | 0x56 | 0x5b | 0x60 => {
            if st == 3 {
                v.push(OFFHAND);
            }
            if wt == 3 {
                v.push(WEAPON);
            }
        }
        0x28 | 0x2a => {
            if wt == 3 {
                v.push(WEAPON);
            }
        }
        _ => {
            if wt == 3 {
                v.push(WEAPON);
            }
            if st == 3 && e.0[OFFHAND + 1] == 0xc {
                v.push(OFFHAND);
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Output records.

/// A damage record (0x18 bytes): target, attacker, threat.
pub(crate) fn damage_record(target: i64, attacker: i64, threat: f32) -> Damage {
    let mut b = [0u8; 0x18];
    w64(&mut b, 0, target);
    w64(&mut b, 8, attacker);
    wf32(&mut b, 0x10, threat);
    Damage(b)
}

/// A passive (buff) record (0x28 bytes, `out+0x58`): source, target, the buff.
pub(crate) fn passive_record(source: i64, target: i64, buff: &Buff) -> Passive {
    let mut b = [0u8; 0x28];
    w64(&mut b, 0, source);
    w64(&mut b, 8, target);
    b[0x10..0x28].copy_from_slice(buff);
    Passive(b)
}

/// `Server.exe 0x004d5f40`: a damage record for the target's threat towards the attacker, and
/// every other creature hostile to the attacker within eight blocks of the target that holds
/// no threat yet gets 0.5 and its own record.
pub(crate) fn alert_allies(entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, attacker: i64, target: i64, out: &mut ServerUpdate) {
    let threat = *states.entry(target).or_default().threat.entry(attacker).or_insert(0.0);
    out.damage.push(damage_record(target, attacker, threat));
    let Some(att) = entities.get(&attacker) else { return };
    let tpos = entities.get(&target).map(pos_of).unwrap_or([0; 3]);
    for (id, c) in entities {
        if *id == attacker || *id == target || !is_hostile(c, att) {
            continue;
        }
        let p = pos_of(c);
        let d: Vec<f32> = (0..3).map(|i| p[i].wrapping_sub(tpos[i]) as f32 * K).collect();
        if d[1] * d[1] + d[0] * d[0] + d[2] * d[2] < 64.0f32 {
            let cs = states.entry(*id).or_default();
            let t = cs.threat.entry(attacker).or_insert(0.0);
            if *t == 0.0 {
                *t = 0.5;
                out.damage.push(damage_record(*id, attacker, 0.5));
            }
        }
    }
}
