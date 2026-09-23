//! The projectiles of `World::tick` (`Server.exe 0x005322d0`): the loop over the `std::list` at
//! `world+0x14` (0x00546830..0x00546c02) and its out-of-line body (0x00547225..0x00548952):
//! the kind-2 return flight, gravity, the age limit, the sub-stepped direct hits against every
//! creature, the block and sweep collision, the impact explosion (area damage) and the kind-1
//! sub-2 projectile turning into a lingering kind-3 area. Ported from the disassembly
//! (`tick_full.asm`) and the transcription `pseudo_54710b.md`; every float operation keeps the
//! original's order and precision, every `rand()` its place in the stream.
//!
//! Client and server: the guards on `world+0xb4` ([`World::is_client`], 0 on a server) and
//! `world+0xb8` ([`World::local_player`], NULL on a server) are ported where they sit: in the
//! client's world only the local player gains MP and rolls the class buffs, a heal is applied
//! only when the local player heals itself, and area damage is not limited to non-player
//! owners.
//!
//! Removal: the original pushes the list iterator of every projectile to remove onto a local
//! `std::list<iterator>` (0x00546b28, `sub_4d6620`) and erases them all after the loop
//! (0x00546b79, `sub_5305b0`). Nothing in the loop reads the other projectiles, so this port marks
//! them and drops them with one `retain` after the loop, in list order.

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern, clippy::excessive_precision, clippy::too_many_arguments, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Hit, Particle, Shoot, Sound};
use cw_world::World;

use crate::combat::{Buff, CreatureState, add_buff, apply_hit, is_hostile};
use crate::path::{line_of_sight, sweep};
use crate::skills::{attack_speed, skill_total_time};
use crate::stats::{item_stat, pow};
use crate::util::{add3, block_fix, blocks3, chance_roll, f32_at, fix, fix3, guard_haste, i32_at, is_enemy, len_sq3, length3, lerp_repeat3, normalize3, passive_record, pos_at, solid, sub3, vec3f_at, w32, w64, wf32};



// ---------------------------------------------------------------------------------------------
// The record.

/// One element of the projectile list at `world+0x14`: the 0x70-byte Shoot record
/// (`cw_net::packet::Shoot`, client packet 9), whose layout it shares byte for byte. Field names
/// follow their use in the tick (cuwo's names in brackets).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Projectile {
    /// `+0x00`: the shooter's entity id (`entity_id`).
    pub owner_id: i64,
    /// `+0x08`: zone x (`chunk_x`); not read by the tick.
    pub zone_x: i32,
    /// `+0x0c`: zone y (`chunk_y`); not read by the tick.
    pub zone_y: i32,
    /// `+0x10` (`something5`); not read by the tick.
    pub u10: u32,
    /// `+0x14`: alignment padding, kept for a byte-exact round trip.
    pub pad14: u32,
    /// `+0x18`: position, 3 x i64 16.16 fixed blocks.
    pub pos: [i64; 3],
    /// `+0x30` (`something13`).
    pub u30: u32,
    /// `+0x34` (`something14`).
    pub u34: u32,
    /// `+0x38` (`something15`).
    pub u38: u32,
    /// `+0x3c`: velocity, blocks per second.
    pub vel: [f32; 3],
    /// `+0x48`: damage (`legacy_damage`).
    pub damage: f32,
    /// `+0x4c`: half-size of the projectile's hit box in blocks (`something20`).
    pub radius: f32,
    /// `+0x50` (`scale`); not read by the tick.
    pub scale: f32,
    /// `+0x54`: knockback, the creatureAttack factor (`mana`).
    pub knockback: f32,
    /// `+0x58` (`particles`); not read by the tick.
    pub u58: u32,
    /// `+0x5c`: explodes on impact instead of hitting directly (`skill`).
    pub flag5c: u8,
    /// `+0x5d..+0x60`: padding.
    pub pad5d: [u8; 3],
    /// `+0x60`: kind: 0, 1 (explodes), 2 (returns to the owner, hits every 100 ms), 3 (a
    /// lingering area that hits once a second), 4 (`projectile`).
    pub kind: i32,
    /// `+0x64`: 1, or 2 for a healing projectile (`something26`).
    pub sub: u8,
    /// `+0x65..+0x68`: padding.
    pub pad65: [u8; 3],
    /// `+0x68`: age in milliseconds (`something27`).
    pub age: i32,
    /// `+0x6c`: raised into the target's `entity+0x124` (max) on a hit (`something28`).
    pub val6c: i32,
}

fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn rd_i64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn rd_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// The projectile a Shoot record describes (the same 0x70 bytes).
pub fn projectile_from_shoot(s: &Shoot) -> Projectile {
    let b = &s.0;
    Projectile {
        owner_id: rd_i64(b, 0),
        zone_x: rd_i32(b, 8),
        zone_y: rd_i32(b, 0xc),
        u10: rd_u32(b, 0x10),
        pad14: rd_u32(b, 0x14),
        pos: [rd_i64(b, 0x18), rd_i64(b, 0x20), rd_i64(b, 0x28)],
        u30: rd_u32(b, 0x30),
        u34: rd_u32(b, 0x34),
        u38: rd_u32(b, 0x38),
        vel: [rd_f32(b, 0x3c), rd_f32(b, 0x40), rd_f32(b, 0x44)],
        damage: rd_f32(b, 0x48),
        radius: rd_f32(b, 0x4c),
        scale: rd_f32(b, 0x50),
        knockback: rd_f32(b, 0x54),
        u58: rd_u32(b, 0x58),
        flag5c: b[0x5c],
        pad5d: [b[0x5d], b[0x5e], b[0x5f]],
        kind: rd_i32(b, 0x60),
        sub: b[0x64],
        pad65: [b[0x65], b[0x66], b[0x67]],
        age: rd_i32(b, 0x68),
        val6c: rd_i32(b, 0x6c),
    }
}

impl Projectile {
    /// The Shoot record of this projectile (inverse of [`projectile_from_shoot`]).
    pub fn to_shoot(&self) -> Shoot {
        let mut b = [0u8; 0x70];
        b[0..8].copy_from_slice(&self.owner_id.to_le_bytes());
        b[8..0xc].copy_from_slice(&self.zone_x.to_le_bytes());
        b[0xc..0x10].copy_from_slice(&self.zone_y.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.u10.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&self.pad14.to_le_bytes());
        for i in 0..3 {
            b[0x18 + i * 8..0x20 + i * 8].copy_from_slice(&self.pos[i].to_le_bytes());
        }
        b[0x30..0x34].copy_from_slice(&self.u30.to_le_bytes());
        b[0x34..0x38].copy_from_slice(&self.u34.to_le_bytes());
        b[0x38..0x3c].copy_from_slice(&self.u38.to_le_bytes());
        for i in 0..3 {
            b[0x3c + i * 4..0x40 + i * 4].copy_from_slice(&self.vel[i].to_le_bytes());
        }
        b[0x48..0x4c].copy_from_slice(&self.damage.to_le_bytes());
        b[0x4c..0x50].copy_from_slice(&self.radius.to_le_bytes());
        b[0x50..0x54].copy_from_slice(&self.scale.to_le_bytes());
        b[0x54..0x58].copy_from_slice(&self.knockback.to_le_bytes());
        b[0x58..0x5c].copy_from_slice(&self.u58.to_le_bytes());
        b[0x5c] = self.flag5c;
        b[0x5d..0x60].copy_from_slice(&self.pad5d);
        b[0x60..0x64].copy_from_slice(&self.kind.to_le_bytes());
        b[0x64] = self.sub;
        b[0x65..0x68].copy_from_slice(&self.pad65);
        b[0x68..0x6c].copy_from_slice(&self.age.to_le_bytes());
        b[0x6c..0x70].copy_from_slice(&self.val6c.to_le_bytes());
        Shoot(b)
    }
}


// ---------------------------------------------------------------------------------------------
// Helpers (duplicated from physics.rs unless noted).

/// `0x00402bd0` / `0x00402db0`: each component `(v * s) / 65536` (`__allmul`, `__alldiv`
/// truncating toward zero).
fn mul_fix3(v: [i64; 3], s: i64) -> [i64; 3] {
    [v[0].wrapping_mul(s) / 65536, v[1].wrapping_mul(s) / 65536, v[2].wrapping_mul(s) / 65536]
}

/// `cvttss2si`: truncation, `0x80000000` for NaN or out of range.
fn cvtt(f: f32) -> i32 {
    if f.is_nan() || f >= 2147483648.0f32 || f < -2147483648.0f32 { i32::MIN } else { f as i32 }
}

fn has_buff(st: Option<&CreatureState>, ty: u8) -> bool {
    st.is_some_and(|s| s.buffs.iter().any(|b| b[0] == ty))
}

/// A Sound as `Sound::Sound` 0x004c8530 leaves it (pitch and volume 1) with the fields set.
fn sound_at(pos: [f32; 3], kind: u32, pitch: f32) -> Sound {
    let mut b = [0u8; 0x18];
    for (i, x) in pos.iter().enumerate() {
        wf32(&mut b, i * 4, *x);
    }
    w32(&mut b, 0xc, kind);
    wf32(&mut b, 0x10, pitch);
    wf32(&mut b, 0x14, 1.0);
    Sound(b)
}

/// `0x00413ac0`: the critical chance an item adds (types 3..9 but 3/4/5/6/7/8/9 only; 0 below
/// 0.001).
fn item_crit(item: &[u8]) -> f32 {
    let ty = item[0];
    if !matches!(ty, 8 | 9 | 3 | 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let mut k = 0.05f32;
    if (ty == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)) || ty == 4 {
        k = 0.1f32;
    }
    // 0x413b40: modifier % 21 (unsigned), through a double (the `[0x55ac20 + sign*8]` fix-up adds
    // 0 for these small values), divided by 20.
    let m = rd_u32(item, 4) % 0x15;
    let f = (f64::from(m as i32) + 0.0) as f32 / 20.0f32;
    let mut q = 1.0f32 - f;
    if item[0xd] == 0xb {
        q = q + 1.0f32;
    }
    let level = f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
    let v = item_stat(level, i32::from(item[0xc])) * k * q;
    if 0.001f32 > v { 0.0 } else { v }
}

/// `0x00409ac0`: the critical chance of a creature: `2^((1 - 1/((level - 1) * 0.05 + 1)) * 3) *
/// 2^(0.25 * 0) / 2^3 * 0.1`, plus [`item_crit`] of each equipment slot holding its type (slots
/// 6 and 7 weapons, 2 chest, 3, 4, 5, 1, 8 and 9).
fn crit_base(e: &EntityData) -> f32 {
    let b = &e.0;
    let level = i32_at(b, 0x180) as f32;
    let t = (1.0f32 - 1.0f32 / ((level - 1.0f32) * 0.05f32 + 1.0f32)) * 3.0f32;
    let a = pow(2.0, f64::from(t)) as f32;
    let p2 = pow(2.0, f64::from(0.25f32 * 0.0f32)) as f32;
    let v = a * p2;
    let c = pow(2.0, 3.0) as f32;
    let mut v = v / c * 0.1f32;
    for (slot, ty) in [(6usize, 3u8), (7, 3), (2, 4), (3, 6), (4, 5), (5, 7), (1, 8), (8, 9), (9, 9)] {
        let o = 0x2f0 + slot * 0x118;
        if b[o] == ty {
            v = item_crit(&b[o..o + 0x118]) + v;
        }
    }
    v
}

/// `rollCritical` `Server.exe 0x0040f520`: chance 1 with a type-0xb buff, else [`crit_base`] +
/// guard × 0.15; one `rand()`.
fn roll_critical(world: &mut World, entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, id: i64) -> bool {
    let st = states.get(&id);
    let chance = if has_buff(st, 0xb) {
        1.0f32
    } else {
        crit_base(&entities[&id]) + st.map_or(0.0, |s| s.block) * 0.15f32
    };
    chance > world.rng.rand() as f32 / 32767.0f32
}

/// The owner's attack animation rescaled after its projectile is gone or stopped
/// (0x00546abf..0x00546b26 and 0x005480bd..0x005480fc): `a = attackSpeed`, the hit counter
/// (`entity+0x60`, which `attackSpeed` reads) cleared, `b = attackSpeed`, mode time × `a / b`.
fn rescale_mode_time(entities: &mut BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, id: i64) {
    let (guard, haste) = guard_haste(states, id);
    let Some(e) = entities.get_mut(&id) else { return };
    let a = attack_speed(e, guard, haste);
    w32(&mut e.0, 0x60, 0);
    let b = attack_speed(e, guard, haste);
    let mt = i32_at(&e.0, 0x5c);
    w32(&mut e.0, 0x5c, cvtt(a / b * mt as f32) as u32);
}

/// The class buff an owner may gain on a hit (0x005479be..0x00547a9a, 0x00548731..0x0054880d):
/// `addBuff` (duration 30000 ms), a buff record from the owner to itself, sound 0x2f.
fn owner_buff(entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, ty: u8, out: &mut ServerUpdate) {
    let mut buff: Buff = [0u8; 0x18];
    buff[0] = ty;
    buff[8..0xc].copy_from_slice(&30000i32.to_le_bytes());
    add_buff(states.entry(id).or_default(), &buff);
    out.passives.push(passive_record(id, id, &buff));
    out.sounds.push(sound_at(blocks3(pos_at(&entities[&id].0)), 0x2f, 1.0));
}

/// The next creature of the map after `cur` (the original walks its `std::map` live).
fn next_id(entities: &BTreeMap<i64, EntityData>, cur: Option<i64>) -> Option<i64> {
    match cur {
        None => entities.keys().next().copied(),
        Some(c) => entities.range((Excluded(c), Unbounded)).next().map(|(k, _)| *k),
    }
}

/// The box test of 0x0054745c..0x005476d2 and 0x005483d5..0x00548612: the projectile's cube of
/// half-size `half` against the target's box of half-size `scale * 0.5`, in fixed point.
fn boxes_overlap(p: [i64; 3], half: f32, tpos: [i64; 3], scale: [f32; 3]) -> bool {
    for i in 0..3 {
        // i64AddFloat(pos, half) >= i64SubFloat(tpos, scale * 0.5)
        if !(p[i].wrapping_add(fix(half)) >= tpos[i].wrapping_sub(fix(scale[i] * 0.5f32))) {
            return false;
        }
    }
    for i in 0..3 {
        // i64SubFloat(pos, half) < i64AddFloat(tpos, scale * 0.5)
        if !(p[i].wrapping_sub(fix(half)) < tpos[i].wrapping_add(fix(scale[i] * 0.5f32))) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The loop.

/// The projectile loop of `World::tick` (`Server.exe 0x00546830..0x00546c02` with its body
/// 0x00547225..0x00548952): every projectile of the list in order: the kind-2 return flight or
/// gravity, the step `vel * dt_s`, the age; a projectile past 5000 ms (other than kind 2) goes,
/// rescaling its owner's attack animation unless it is a kind-3 area; the others move in
/// sub-steps of at most half a block, hit the creatures their box overlaps in line of sight,
/// stop at blocks and explode or go on impact. Removed projectiles are erased after the loop,
/// as the original's removal list does. The creature fields `+0x11b4` (the dodged set) and
/// `+0x11c0` are `CreatureState::modes` (`pending_set`, `mount`); `dirty` is the tick's
/// dirty-zone set.
pub fn update_projectiles(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, projectiles: &mut Vec<Projectile>, dt: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>) {
    let dt_f = dt as f32;
    let dt_s = dt_f * 0.001f32;
    // 0x546830: the removal list; 0x54683b: for (it = world->projectiles.begin(); ...).
    let mut remove = vec![false; projectiles.len()];
    for (idx, p) in projectiles.iter_mut().enumerate() {
        if !update_one(world, entities, states, p, dt, dt_s, out, dirty) {
            // 0x546b28: removeList.push_back(it)
            remove[idx] = true;
        }
    }
    // 0x546b79..0x546c02: erase every listed projectile.
    let mut i = 0;
    projectiles.retain(|_| {
        let keep = !remove[i];
        i += 1;
        keep
    });
}

/// One projectile; false when it is to be removed (0x546b28), true to keep it (0x546b3a).
fn update_one(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, p: &mut Projectile, dt: i32, dt_s: f32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>) -> bool {
    // 0x546880: proj = &*it; kind 1: the velocity is untouched.
    if p.kind == 1 {
    } else if p.kind == 2 {
        // 0x5468a2: the return flight. No owner: removed.
        let Some(owner) = entities.get(&p.owner_id) else { return false };
        let opos = pos_at(&owner.0);
        let age = p.age;
        // 0x5468be: past 1200 ms: removed.
        if age > 1200 {
            return false;
        }
        if age > 1100 {
            // 0x5468d7: pos += (owner.pos - pos) * t * t in fixed point, t = (age - 1100) / 100.
            let t = (age - 1100) as f32 / 100.0f32;
            let tf = fix(t); // 0x546901 / 0x546920 (both 0x402a10 of the same t)
            let d = sub3(opos, p.pos); // 0x54693d
            let m = mul_fix3(mul_fix3(d, tf), tf); // 0x546944, 0x54694b
            p.pos = add3(p.pos, m); // 0x54695a, 0x546962
        } else if age > 800 {
            // 0x546973: vel = fromFixed(owner.pos - pos) * 10.
            let v = blocks3(sub3(opos, p.pos));
            p.vel = [v[0] * 10.0f32, v[1] * 10.0f32, v[2] * 10.0f32];
        } else {
            // 0x5469b6: vel eased (dt times, 0.05) towards fromFixed(owner.pos + fix(owner.aim)
            // - pos) * 5; the aim is the ray hit at entity+0x150.
            let aim = fix3(vec3f_at(&owner.0, 0x150));
            let v = blocks3(sub3(add3(opos, aim), p.pos));
            let target = [v[0] * 5.0f32, v[1] * 5.0f32, v[2] * 5.0f32];
            lerp_repeat3(&mut p.vel, target, dt, 0.05f32);
        }
    } else {
        // 0x546a2f: gravity, vel.z -= dt_s * 30 * 0.25.
        let g = dt_s * 30.0f32 * 0.25f32;
        p.vel[2] = p.vel[2] - g;
    }
    // 0x546a5d: step = vel * dt_s; age += dt.
    let step = [p.vel[0] * dt_s, p.vel[1] * dt_s, p.vel[2] * dt_s];
    p.age = p.age.wrapping_add(dt);
    // 0x546a8d / 0x546a9a
    if !(p.kind == 2 || p.age <= 5000) {
        // 0x546aa0: expired. A kind-3 area just goes; otherwise the owner's animation rescales.
        if p.kind == 3 {
            return false;
        }
        if entities.contains_key(&p.owner_id) {
            rescale_mode_time(entities, states, p.owner_id);
        }
        return false;
    }
    step_body(world, entities, states, p, step, dt, dt_s, out, dirty)
}

/// The out-of-line body 0x00547225..0x00548952.
fn step_body(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, p: &mut Projectile, step: [f32; 3], dt: i32, dt_s: f32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>) -> bool {
    // ---- B1 0x547225: setup.
    // ownerA (0x547236) and owner (0x54732c) are both findEntity(world, &proj->ownerId).
    let owner_a: Option<i64> = entities.contains_key(&p.owner_id).then_some(p.owner_id);
    let len = length3(step); // 0x547257
    let steps = cvtt(len * 2.0f32 + 1.0f32); // 0x547262
    let sf = steps as f32;
    let sub_step = [step[0] / sf, step[1] / sf, step[2] / sf]; // 0x547285 sub_4f7a70
    let mut kb = p.vel; // 0x5472a0
    if len_sq3(kb) > 0.0f32 {
        normalize3(&mut kb); // 0x5472d4
    }
    let s = p.knockback * 5.0f32; // 0x5472d9
    kb = [kb[0] * s, kb[1] * s, kb[2] * s];
    kb[2] = p.knockback * 3.0f32; // 0x5472f7
    let owner: Option<i64> = owner_a; // 0x54732c
    let mut i = 0i32;
    if steps <= 0 {
        return true; // 0x54734a
    }
    let mount_of = |states: &BTreeMap<i64, CreatureState>, id: i64| states.get(&id).map_or(0, |s| s.modes.mount);

    loop {
        // ---- B2 0x547350: one sub-step; the direct hits.
        let mut remove_flag = false;
        let mut hit_any = false;
        let mut cur: Option<i64> = None;
        while let Some(tid) = next_id(entities, cur) {
            cur = Some(tid);
            let t = &entities[&tid];
            // 0x5473bc: dead targets are skipped (a NaN HP is not).
            if 0.0f32 >= f32_at(&t.0, 0x15c) {
                continue;
            }
            // 0x5473cc: a healing area (kind 3, sub 2) hits anyone, owner included.
            if !(p.kind == 3 && p.sub == 2)
                && let Some(oid) = owner
            {
                if tid == oid {
                    continue;
                }
                if !is_enemy(&entities[&oid], t) {
                    continue;
                }
                if tid == mount_of(states, oid) {
                    continue;
                }
            }
            let tpos = pos_at(&t.0);
            let scale = vec3f_at(&t.0, 0x70);
            // 0x54745c..0x5476f9: box overlap and line of sight; else L_547c92.
            if boxes_overlap(p.pos, p.radius, tpos, scale) && line_of_sight(world, p.pos, tpos, true, 200.0f32) {
                // 0x5476ff: a rolling target dodges and is noted on the owner.
                if i32_at(&t.0, 0x118) != 0 {
                    if let Some(oid) = owner {
                        states.entry(oid).or_default().modes.pending_set.insert(tid); // 0x547727 sub_530690
                    }
                    continue;
                }
                // 0x547731: does it hit now?
                let kind = p.kind;
                let hit = if kind == 0 && p.flag5c == 0 {
                    true
                } else if (kind == 1 || kind == 4) && p.flag5c == 0 {
                    true
                } else if kind == 3 && p.age.wrapping_sub(dt) / 1000 != p.age / 1000 {
                    true // 0x54775a: crossed a whole second
                } else if p.age <= 300 {
                    false // 0x547793
                } else if kind != 2 {
                    false // 0x5477a2
                } else {
                    p.age.wrapping_sub(dt) / 100 != p.age / 100 // 0x5477ab: every 100 ms
                };
                if hit {
                    // 0x5477df L_hit
                    if kind != 3 && kind != 2 {
                        remove_flag = true; // 0x5477e4 (cmovne)
                    }
                    // 0x5477fe: MP for the owner (kinds 0 and 2 without flag5c).
                    if p.flag5c == 0 && kind != 3 && kind != 1 && kind != 4
                        && let Some(oid) = owner
                    {
                        let r = world.rng.rand(); // 0x547831
                        let mut g = 1.0f32 - r as f32 * 2.0f32 / 32767.0f32;
                        g = g * 0.05f32; // 0x54785e
                        g = g + 0.1f32; // 0x547866
                        if p.kind == 2 {
                            g = g * 0.2f32; // 0x547878
                        }
                        let (guard, haste) = guard_haste(states, oid);
                        let tt = skill_total_time(&entities[&oid], guard, haste); // 0x547888
                        g = tt as f32 / 300.0f32 * g;
                        if world.rng.rand() % 8 == 0 {
                            g = g * 2.0f32; // 0x5478ae..0x5478ca
                        }
                        // 0x5478d2: `world+0xb4 == 0 || owner == world+0xb8`.
                        if !world.is_client || world.local_player == Some(oid) {
                            let e = entities.get_mut(&oid).expect("owner");
                            let mp = f32_at(&e.0, 0x160) + g;
                            wf32(&mut e.0, 0x160, mp); // 0x5478e9
                            if mp > 1.0f32 {
                                wf32(&mut e.0, 0x160, 1.0); // 0x5478f5
                            }
                        }
                    }
                    // 0x547910
                    let crit = match owner {
                        Some(oid) => roll_critical(world, entities, states, oid),
                        None => false,
                    };
                    let mut dmg = p.damage; // 0x547929
                    if crit {
                        dmg = dmg * 2.0f32; // 0x54793a
                    }
                    // 0x54794a: `ownerA == world+0xb8 || (world+0xb4 == 0 && ownerA.hostile
                    // != 0)`: on a server the non-player owners, on the client the local player.
                    if let Some(a) = owner_a
                        && (world.local_player == Some(a) || (!world.is_client && entities[&a].0[0x50] != 0))
                        && chance_roll(world, &entities[&a], 0.15f32) // 0x54797d
                        && p.flag5c == 0
                        && entities[&a].0[0x130] == 2
                        && entities[&a].0[0x131] == 1
                    {
                        owner_buff(entities, states, a, 0xa, out); // 0x5479be..0x547a9a
                    }
                    if 0.0f32 > dmg {
                        dmg = 0.0; // 0x547ab3
                    }
                    if p.sub == 2 {
                        // 0x547ac3: a healing projectile: a negative Hit from a friendly owner.
                        if let Some(a) = owner_a
                            && !is_hostile(&entities[&a], &entities[&tid])
                            && entities[&tid].0[0x50] != 6
                        {
                            let mut h = [0u8; 0x48]; // 0x547aff sub_422a90
                            for (k, v) in tpos.iter().enumerate() {
                                w64(&mut h, 0x20 + k * 8, *v); // 0x547b10
                            }
                            let mut hd = -dmg; // 0x547b15
                            if entities[&a].0[0x50] == 1 {
                                hd = hd * 0.5f32; // 0x547b32
                            }
                            wf32(&mut h, 0x10, hd);
                            h[0x14] = u8::from(crit); // 0x547b54
                            w64(&mut h, 0, p.owner_id); // 0x547b5a
                            w64(&mut h, 8, tid); // 0x547b6b
                            let hit = Hit(h);
                            out.hits.push(hit.clone()); // 0x547b8a
                            // 0x547b95: `world+0xb4 == 0 || (ownerA == world+0xb8 && target ==
                            // world+0xb8)`.
                            if !world.is_client || (world.local_player == Some(a) && world.local_player == Some(tid)) {
                                apply_hit(world, entities, states, &hit, out, dirty, false); // 0x547bd4
                            }
                        }
                    } else {
                        // 0x547c4c creatureAttack
                        let r = crate::combat_ai::creature_attack(world, entities, states, tid, owner, dmg, crit, p.flag5c != 0, p.knockback, 0, kb, out, dirty, false, p.kind == 1, 0, 0, true);
                        if r {
                            hit_any = true; // 0x547c51 (cmovne)
                        }
                    }
                    // 0x547c70: target entity+0x124 = max(itself, proj.val6c) (signed).
                    if let Some(te) = entities.get_mut(&tid)
                        && p.val6c > i32_at(&te.0, 0x124)
                    {
                        w32(&mut te.0, 0x124, p.val6c as u32);
                    }
                }
                // 0x547c81 L_547c81: kinds 0, 1 and 4 stop at the first target they overlap.
                if p.kind == 0 || p.kind == 1 || p.kind == 4 {
                    remove_flag = true;
                    break;
                }
            }
            // 0x547c92 L_547c92
            if remove_flag {
                break;
            }
        }

        // ---- B3 0x547cde: sound, move, collision.
        let mut blocked = false;
        let mut to_impact = false;
        if p.kind != 3 {
            // 0x547cea: kind 2 whirs every 200 ms.
            if p.kind == 2 && p.age.wrapping_sub(dt) / 200 != p.age / 200 {
                let pitch = world.rng.rand() as f32 * 0.25f32 / 32767.0f32 + 1.0f32; // 0x547d41
                out.sounds.push(sound_at(blocks3(p.pos), 0xf, pitch));
            }
            // 0x547d91: pos += fromFloat(subStep). (0x547dad..0x547e32 builds two unused boxes.)
            p.pos = add3(p.pos, fix3(sub_step));
            let blk = block_fix(world, p.pos); // 0x547e3f sub_406050 (getBlockFixed)
            if solid(blk) {
                p.pos = sub3(p.pos, fix3(sub_step)); // 0x547ea9
                blocked = true;
                if p.kind == 0 {
                    // 0x547eda: an arrow sprays the block's colour.
                    let b2 = block_fix(world, p.pos);
                    let col = [f32::from(b2[0]) / 255.0f32, f32::from(b2[1]) / 255.0f32, f32::from(b2[2]) / 255.0f32];
                    let mut pb = [0u8; 0x48];
                    for (k, v) in p.pos.iter().enumerate() {
                        w64(&mut pb, k * 8, *v); // 0x547fd9
                    }
                    for (k, v) in [0.0f32, 0.0, 10.0].iter().enumerate() {
                        wf32(&mut pb, 0x18 + k * 4, *v); // 0x547f93
                    }
                    for (k, v) in [col[0], col[1], col[2], 1.0f32].iter().enumerate() {
                        wf32(&mut pb, 0x24 + k * 4, *v); // 0x547f2f
                    }
                    wf32(&mut pb, 0x34, 0.1); // 0x547fb6
                    w32(&mut pb, 0x38, 4); // 0x547fac
                    w32(&mut pb, 0x3c, 0); // ctor 0x4c8510
                    wf32(&mut pb, 0x40, 3.0); // ctor 0x4c8510
                    out.particles.push(Particle(pb)); // 0x547ff4
                }
            } else {
                // 0x547ffe: a sweep back along -vel over this tick's travel.
                let l = length3(p.vel) * dt_s;
                let mut back = p.vel;
                normalize3(&mut back); // 0x54803f sub_412670
                let back = [back[0] * -1.0f32, back[1] * -1.0f32, back[2] * -1.0f32]; // 0x548046
                let d = sweep(world, p.pos, back, l, false, true); // 0x548053
                if l > d {
                    blocked = true; // 0x548079
                    p.pos = sub3(p.pos, fix3(sub_step)); // 0x548080
                }
            }
            // 0x548099
            if !remove_flag && blocked && p.kind != 3
                && let Some(oid) = owner
                && entities.contains_key(&oid)
            {
                rescale_mode_time(entities, states, oid); // 0x5480bd..0x5480fc
            }
            if p.kind == 2 {
                // 0x548105
                if hit_any
                    && let Some(oid) = owner
                    && let Some(e) = entities.get_mut(&oid)
                {
                    let hc = i32_at(&e.0, 0x60).wrapping_add(1);
                    w32(&mut e.0, 0x60, hc as u32); // 0x54811d
                    // 0x548122 sub_4103a0: walks creature+0x139c, writes nothing.
                    w32(&mut e.0, 0x64, 0); // 0x548127
                }
                if blocked {
                    // 0x548138: the `kind != 2` re-test is dead (sound 0x13 at 0x5481ac never
                    // runs).
                    p.vel = [0.0, 0.0, 0.0]; // 0x548142
                }
            } else if remove_flag || blocked {
                to_impact = true; // 0x548172 -> L_548217
            }
        }
        if !to_impact {
            // 0x548188 L_548188
            i += 1;
            if i >= steps {
                return true; // 0x54819b
            }
            continue; // 0x5481a7
        }

        // ---- B4 0x548217: impact.
        if p.kind == 1 || p.flag5c != 0 {
            if !remove_flag {
                // 0x548232
                let pitch = world.rng.rand() as f32 * 0.4f32 / 32767.0f32 + 1.0f32;
                let kind = if p.kind == 1 {
                    if p.sub == 2 { 0x2a } else { 0x27 }
                } else {
                    0x14
                };
                out.sounds.push(sound_at(blocks3(p.pos), kind, pitch)); // 0x54829d
            }
            let flag5c_copy = p.flag5c; // 0x5482a2
            let mut cur: Option<i64> = None;
            'area: while let Some(tid) = next_id(entities, cur) {
                cur = Some(tid);
                // 0x54832f: on the server a player-owned projectile does no area damage
                // (`ownerA && world+0xb4 == 0 && ownerA.hostile == 0 && ownerA != world+0xb8`).
                if let Some(a) = owner_a
                    && !world.is_client
                    && entities[&a].0[0x50] == 0
                    && world.local_player != Some(a)
                {
                    break 'area;
                }
                let t = &entities[&tid];
                if let Some(oid) = owner {
                    // 0x54835a
                    if tid == oid {
                        continue;
                    }
                    if !is_enemy(&entities[&oid], t) {
                        continue;
                    }
                    if tid == mount_of(states, oid) {
                        continue;
                    }
                    if 0.0f32 >= f32_at(&t.0, 0x15c) {
                        continue; // 0x548397 (only checked with an owner)
                    }
                }
                let tpos = pos_at(&t.0);
                let scale = vec3f_at(&t.0, 0x70);
                // 0x5483d5..0x548612: a fixed half-size of 5 blocks.
                if !boxes_overlap(p.pos, 5.0f32, tpos, scale) {
                    continue;
                }
                if !line_of_sight(world, p.pos, tpos, true, 200.0f32) {
                    continue; // 0x548618
                }
                // 0x54863f: a rolling target dodges only when there is an owner.
                if i32_at(&t.0, 0x118) != 0
                    && let Some(oid) = owner
                {
                    states.entry(oid).or_default().modes.pending_set.insert(tid);
                    continue;
                }
                // 0x548675: note ownerA here.
                let crit2 = match owner_a {
                    Some(a) => roll_critical(world, entities, states, a),
                    None => false,
                };
                let mut dmg2 = p.damage; // 0x5486ab
                if crit2 {
                    dmg2 = dmg2 * 2.0f32; // 0x5486bc
                }
                // 0x5486cc: `ownerA == world+0xb8 || (world+0xb4 == 0 && ownerA.hostile !=
                // 0)`.
                if let Some(a) = owner_a
                    && (world.local_player == Some(a) || (!world.is_client && entities[&a].0[0x50] != 0))
                    && chance_roll(world, &entities[&a], 0.25f32) // 0x5486f9
                    && flag5c_copy == 0
                    && p.kind == 1
                    && p.sub == 1
                {
                    owner_buff(entities, states, a, 9, out); // 0x548731..0x54880d
                }
                // No `dmg2 < 0` clamp here. 0x548878 creatureAttack
                let r = crate::combat_ai::creature_attack(world, entities, states, tid, owner, dmg2, crit2, flag5c_copy != 0, p.knockback, 0, kb, out, dirty, false, p.kind == 1, 0, 0, true);
                if r {
                    hit_any = true; // 0x54887d
                }
            }
        }
        // 0x5488d0 L_5488d0
        if hit_any
            && p.kind != 3
            && let Some(oid) = owner
            && let Some(e) = entities.get_mut(&oid)
        {
            let hc = i32_at(&e.0, 0x60).wrapping_add(1);
            w32(&mut e.0, 0x60, hc as u32); // 0x5488e4
            // 0x5488e9 sub_4103a0: no effect.
            w32(&mut e.0, 0x64, 0); // 0x5488ee
        }
        // 0x5488f5: anything but kind 1 / sub 2 is removed.
        if p.kind != 1 || p.sub != 2 {
            return false;
        }
        // 0x548909: kind 1 / sub 2 becomes a lingering kind-3 area (2 s, or 5 s with flag5c).
        p.kind = 3;
        p.age = if p.flag5c != 0 { 0 } else { 3000 };
        p.damage = p.damage * 0.05f32;
        p.radius = p.knockback * 5.0f32 + 5.0f32;
        p.knockback = 0.0;
        p.flag5c = 0;
        return true; // 0x548952
    }
}
