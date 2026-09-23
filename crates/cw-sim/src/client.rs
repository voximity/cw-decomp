//! The client-only helpers of `World::tick` (`Server.exe 0x005322d0`, byte-identical to
//! `Cube.exe 0x0060c510`): the blocks guarded by `world+0xb4` (non-zero in the client's world,
//! [`World::is_client`]) or by `world+0xb8` (the local player's creature, NULL on a server,
//! [`World::local_player`]). The guards themselves sit in the modules that port the ranges
//! around them (update.rs, physics.rs, modes.rs, consumables.rs, statics.rs, interact.rs,
//! projectile.rs, day.rs); this module holds what those blocks call that has no server use:
//!
//! - [`apply_incoming`]: the received hits and passives at the head of the tick
//!   (0x00532ac5..0x00532c6a) with the client's clears of its own out lists;
//! - [`nearest_static`]: `World::nearestStatic` 0x004d9410 (the bed interaction);
//! - [`home_teleport`]: the mode-0x69 case of the mode machine (0x00537be2..0x00537cef) with
//!   `World::nearestRegion` 0x004feec0;
//! - [`local_speed`]: the local player's mount and debug speed (0x0053f099..0x0053f228);
//! - [`glide`]: the local player's gliding crash, dive and wind (0x00540a07..0x00540bff) with
//!   `World::wind` 0x004d9720;
//! - [`fall_damage`]: the landing sound and the local player's fall damage
//!   (0x00541f2d..0x005421d8);
//! - [`wall_bounce`], [`footstep`], [`roll_sound`]: 0x00544662..0x0054471c,
//!   0x00544b4f..0x00544cc6, 0x005453fd..0x00545463.
//!
//! Addresses are Server.exe; `creature+X` is `entity+(X-0x10)` below `+0x1178`.

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments, clippy::float_cmp)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Hit, Passive, Sound};
use cw_world::World;

use crate::combat::{CreatureState, apply_hit, apply_passives};
use crate::util::{K, block_fix, block_type, blocks, cell_statics, f32_at, fix, i32_at, pos_at, set_pos, vec3f_at, w32, w64, wf32};

/// A Sound record (0x18 bytes, constructor 0x004c8530: pitch and volume 1.0) at a fixed
/// position (`vec3f::fromFixed` 0x00402550).
pub(crate) fn sound(pos: [i64; 3], kind: u32, pitch: f32, volume: f32) -> Sound {
    let mut b = [0u8; 0x18];
    for i in 0..3 {
        b[i * 4..i * 4 + 4].copy_from_slice(&blocks(pos[i]).to_le_bytes());
    }
    b[0xc..0x10].copy_from_slice(&kind.to_le_bytes());
    b[0x10..0x14].copy_from_slice(&pitch.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&volume.to_le_bytes());
    Sound(b)
}

/// `Server.exe 0x0040ffe0` (decompiled): the stun a crash or a fall gives: 1500 ms on a player,
/// else `3000 - (power * 1500 >> 2)` (power: the byte at `entity+0x198`).
fn stun_duration(e: &EntityData) -> i32 {
    if e.0[0x50] == 0 {
        return 0x5dc;
    }
    3000 - ((i32::from(e.0[0x198]) * 0x5dc) >> 2)
}

/// `0x0052ebb0`: `__ftol2((x87) i64 * f)`. The product of an i64 below 2^40 and a 24-bit
/// mantissa is exact in the x87's 64-bit mantissa, so it is computed exactly here (i128) and
/// truncated toward zero.
fn mul_i64_f32_trunc(a: i64, f: f32) -> i64 {
    if f == 0.0 || !f.is_finite() {
        return if f.is_finite() { 0 } else { i64::MIN };
    }
    let bits = f.to_bits();
    let sign = if bits >> 31 != 0 { -1i128 } else { 1 };
    let exp = ((bits >> 23) & 0xff) as i32;
    let (m, e) = if exp == 0 { (i128::from(bits & 0x7f_ffff), -149) } else { (i128::from((bits & 0x7f_ffff) | 0x80_0000), exp - 150) };
    let p = i128::from(a) * m * sign;
    let r = if e >= 0 { p.checked_shl(e as u32).unwrap_or(0) } else if e > -127 { p / (1i128 << (-e)) } else { 0 };
    i64::try_from(r).unwrap_or(i64::MIN)
}

// ---------------------------------------------------------------------------------------------
// The head of the tick.

/// 0x00532ac5..0x00532c6a: the records received this tick (the tick's third argument; `None`
/// for a NULL pointer, which skips it all). When hits came in, the client's world first empties
/// its own out hit list (0x00532afd, `world+0xb4`, `sub_426f60`); each hit goes through
/// `World::applyHit` 0x004cea80. When passives came in, each adds its buff to its target
/// (0x00411740), then the client's world empties its own out passive list (`out+0x58`,
/// 0x00532c53). cw-server does the same with `world+0xb4 == 0` in its own loop.
pub fn apply_incoming(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, incoming: Option<(&[Hit], &[Passive])>, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>) {
    let Some((hits, passives)) = incoming else { return };
    if !hits.is_empty() {
        if world.is_client {
            out.hits.clear();
        }
        for h in hits {
            apply_hit(world, entities, states, h, out, dirty, false);
        }
    }
    if !passives.is_empty() {
        apply_passives(entities, states, passives);
        if world.is_client {
            out.passives.clear();
        }
    }
}

/// `World::grantXp(kill)` (`Cube.exe 0x005a0bf0`, `Server.exe 0x004d61c0`) for a kill record
/// (0x18 bytes: killer id, victim id, victim type, XP at `+0x14`), when both creatures exist:
///
/// - the victim is the local player (`world+0xb8`): its pet (`creature+0x11c8`), when it
///   exists, drops to 0 HP and its threat map (`creature+0x13a4`) is cleared (`std::map::clear`
///   0x0067e480, 0x005a0ca3), and the player's last target
///   (`creature+0x11d0`) is cleared;
/// - the killer is the local player and the victim a monster (hostile 1): the XP goes to the
///   killer's `entity+0x184` and `Creature::levelUp` 0x00447b00 runs;
/// - in a server world (`world+0xb4` clear): the killer's living pet gains the XP
///   ([`crate::combat::grant_xp`]).
///
/// The kill loop of `applyHit` calls it for every kill it records; the client's `update`
/// calls it for every received kill (0x0048fb25).
pub fn world_grant_xp(world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, kill: &[u8]) {
    let killer = crate::util::i64_at(kill, 0);
    let victim = crate::util::i64_at(kill, 8);
    let xp = i32_at(kill, 0x14);
    if !entities.contains_key(&killer) || !entities.contains_key(&victim) {
        return;
    }
    if world.local_player == Some(victim) {
        let pet = states.get(&victim).map_or(0, |s| s.pet);
        if pet != 0
            && let Some(p) = entities.get_mut(&pet)
        {
            wf32(&mut p.0, 0x15c, 0.0);
        }
        // 0x005a0ca3: `0x0067e480(pet+0x13a4)`, the pet's threat map cleared.
        if pet != 0
            && entities.contains_key(&pet)
            && let Some(ps) = states.get_mut(&pet)
        {
            ps.threat.clear();
        }
        if let Some(s) = states.get_mut(&victim) {
            s.last_target = 0;
        }
    }
    if world.local_player == Some(killer) && entities[&victim].0[0x50] == 1 {
        let k = entities.get_mut(&killer).expect("killer");
        let v = i32_at(&k.0, 0x184).wrapping_add(xp);
        w32(&mut k.0, 0x184, v as u32);
        crate::combat::level_up(k);
    }
    if !world.is_client {
        crate::combat::grant_xp(entities, states, killer, xp);
    }
}

// ---------------------------------------------------------------------------------------------
// World lookups.

/// `World::nearestStatic(&out, &pos)` `Server.exe 0x004d9410` (decompiled): over the 8x8-block
/// cells `cx0 - 1 ..= cx0 + 1` (outer) and `cy0 - 1 ..= cy0 + 1` (inner), `cx0 = (pos.x /
/// 65536) / 8` (both truncating), the static nearest to `pos` by `(float)(dx²/65536 + dy²/65536
/// + dz²/65536) * 2^-16` (first wins ties). Returns its (zone x, zone y, index) when that
/// distance is in `0..=16`, else `(0, 0, -1)`. The cells come from `util::cell_statics` (the
/// zone's static index at `zone+0xac` is not modelled; see that function).
pub fn nearest_static(world: &World, pos: [i64; 3]) -> (i32, i32, i32) {
    let cx0 = ((pos[0] / 65536) as i32) / 8;
    let cy0 = ((pos[1] / 65536) as i32) / 8;
    let mut best = -1.0f32;
    let mut found: Option<(i32, i32, i32)> = None;
    for cx in cx0 - 1..=cx0 + 1 {
        for cy in cy0 - 1..=cy0 + 1 {
            let (zx, zy) = ((cx * 8) / 256, (cy * 8) / 256);
            for (idx, s) in cell_statics(world, cx, cy) {
                let dx = s.x.wrapping_sub(pos[0]);
                let dy = s.y.wrapping_sub(pos[1]);
                let dz = s.z.wrapping_sub(pos[2]);
                let x = dx.wrapping_mul(dx) / 65536;
                let z = dz.wrapping_mul(dz) / 65536;
                let y = dy.wrapping_mul(dy) / 65536;
                let d = (x.wrapping_add(y).wrapping_add(z)) as f32 * K;
                if found.is_none() || d < best {
                    best = d;
                    found = Some((zx, zy, idx as i32));
                }
            }
        }
    }
    match found {
        Some(r) if 0.0 <= best && best <= 16.0 => r,
        _ => (0, 0, -1),
    }
}

/// The mode-0x69 case of the mode machine (`Server.exe 0x00537be2..0x00537cef`, jump-table
/// index 15), the local player only (`world+0xb8`; any other creature goes to the default
/// 0x0053bc80). On the tick the wind-up is crossed (`mt <= windup && mt + dt > windup`): the
/// region of the climate point nearest to the player (`World::nearestRegion` 0x004feec0, the
/// same warped 3x3 search as `nearestClimateRegion` 0x004febd0 returning the region pointer,
/// NULL when not generated) is searched: every one of its 64 cells of kind 1 (the home cell,
/// `cell+0x18`) puts the player at the cell's x and y with z 0 (the last one wins). Returns
/// whether the case ran (the caller then goes to the join).
pub fn home_teleport(world: &World, e: &mut EntityData, windup: i32, dt: i32) -> bool {
    let mt = i32_at(&e.0, 0x5c);
    // 0x00537c03: `jg 0x537cfb`; 0x00537c20: `mt + dt <= windup` -> 0x537d01.
    if mt > windup || mt.wrapping_add(dt) <= windup {
        return false;
    }
    let p = pos_at(&e.0);
    // 0x00537c3a / 0x00537c4c: `fixedToInt` 0x00405640 (`>> 16`).
    let (bx, by) = ((p[0] >> 16) as i32, (p[1] >> 16) as i32);
    let (rx, ry) = world.nearest_climate_region(bx, by);
    let Some(region) = world.region(rx, ry) else { return true };
    let mut pos = p;
    for cell in &region.cells {
        if cell.kind == 1 {
            pos[0] = cell.x;
            pos[1] = cell.y;
            pos[2] = 0;
            set_pos(&mut e.0, pos);
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The local player's movement.

/// `Server.exe 0x004116f0` (disassembled, single precision): the riding speed factor of skill
/// level `k` (`creature+0x113c`, skill 1): 0 below 1, else `(1 - 1 / (k * 0.1 + 1)) + 1`.
fn mount_factor(k: i32) -> f32 {
    if k < 1 {
        return 0.0;
    }
    let one = 1.0f32;
    let d = one / ((k as f32) * 0.1f32 + one);
    (one - d) + one
}

/// `Server.exe 0x0040f7f0` (decompiled): the entity types a player can ride.
fn mountable(ty: i32) -> bool {
    matches!(ty, 0x13 | 0x14 | 0x16 | 0x17 | 0x19..=0x28 | 0x3f..=0x43 | 0x4a | 0x4b | 0x62..=0x64 | 0x66..=0x69 | 0x97)
}

/// 0x0053f099..0x0053f228, the local player only (`c == world+0xb8`), after the swimming
/// factor: swimming as a mount (mode 0x6b) ends on dry ground (`phys & 1` without `phys & 2`);
/// riding (mode 0x6a) needs the pet (`creature+0x11c8`) alive (`HP >= 0`, NaN fails) and of a
/// mountable type, and the riding skill: the cap is multiplied by `mount_factor(skill 1)`
/// (a factor of exactly 0 ends the ride instead); while accelerating the riding stamina
/// (`creature+0x1198`) drains `dt_s * 0.002` (0x005586a0), and at or below 0 it is 0 and the
/// ride ends; climbing (flag 1) ends it too. Then with bit 0 of `world+0x84` the cap is 20
/// (0x0055874c). `pet` is the pet's (HP, entity type) when it exists; `phys` and `flags` are
/// the creature's physics flags and entity flags at this point.
pub fn local_speed(world: &World, e: &mut EntityData, st: &mut CreatureState, id: i64, pet: Option<(f32, i32)>, phys: u32, flags: u16, dt_s: f32, max_speed: &mut f32) {
    if world.local_player == Some(id) {
        // 0x0053f0a5
        if e.0[0x58] == 0x6b && phys & 1 != 0 && phys & 2 == 0 {
            e.0[0x58] = 0;
        }
        // 0x0053f0c6
        if e.0[0x58] == 0x6a {
            let mut keep = false;
            if let Some((hp, ty)) = pet {
                // 0x0053f0f6: comiss hp, 0; jb -> the ride ends (NaN too).
                if hp >= 0.0 && mountable(ty) {
                    let f = mount_factor(crate::skills::skill(e, 1));
                    // 0x0053f141: `ucomiss f, 0; lahf; test ah, 0x44; jpo`: an ordered 0 ends it.
                    if f == 0.0 {
                        e.0[0x58] = 0;
                    } else {
                        *max_speed = f * *max_speed;
                    }
                    // 0x0053f156: the pet is looked up again; it exists.
                    keep = true;
                }
            }
            if !keep {
                e.0[0x58] = 0;
            }
            // 0x0053f19e: the local player's acceleration.
            let a = vec3f_at(&e.0, 0x30);
            if a[0] * a[0] + a[1] * a[1] + a[2] * a[2] > 0.0f32 {
                let d = dt_s * 0.002f32;
                st.riding.ride_stamina = st.riding.ride_stamina - d;
            }
            // 0x0053f1e6: comiss 0, stamina; jb skips (0 < stamina, or NaN).
            if !(0.0f32 < st.riding.ride_stamina) && !st.riding.ride_stamina.is_nan() {
                st.riding.ride_stamina = 0.0;
                e.0[0x58] = 0;
            }
            if flags & 1 != 0 {
                e.0[0x58] = 0;
            }
        }
    }
    // 0x0053f20f
    if world.local_player == Some(id) && world.flags84 & 1 != 0 {
        *max_speed = 20.0;
    }
}

/// `World::wind(&out, &pos, &vel)` `Server.exe 0x004d9720` (disassembled; `vel` unused): three
/// value-noise samples (0x004d5d30) of the position, each coordinate `__ftol2`'d from `(double)
/// pos * k` and scaled back by 2^-16: x `noise(px * 0.05, py * 0.05) * 2`, y
/// `noise(0xd7f0000 - px * -0.05, 0x20f60000 - py * -0.05) * 2`, z `noise(0x108a0000 - px *
/// -0.02, 0x14e10000 - py * -0.02) + 0.5`.
pub fn wind(pos: [i64; 3]) -> [f32; 3] {
    let py = pos[1] as f64;
    let px = pos[0] as f64;
    let k = 1.52587890625e-05f64;
    let ft = |d: f64| d as i64;
    let x = cw_math::value_noise_2d(ft(px * 0.05) as f64 * k, ft(py * 0.05) as f64 * k) * 2.0f32;
    let a = 0x20f6_0000i64.wrapping_sub(ft(py * -0.05)) as f64 * k;
    let b = 0x0d7f_0000i64.wrapping_sub(ft(px * -0.05)) as f64 * k;
    let y = cw_math::value_noise_2d(b, a) * 2.0f32;
    let a = 0x14e1_0000i64.wrapping_sub(ft(py * -0.02)) as f64 * k;
    let b = 0x108a_0000i64.wrapping_sub(ft(px * -0.02)) as f64 * k;
    let z = cw_math::value_noise_2d(b, a) + 0.5f32;
    [x, y, z]
}

/// 0x00540a07..0x00540bff, the local player only (`c == world+0xb8`) while gliding (flag
/// 0x10): in water the glide ends; in the air a fast crash into a wall (`phys & 4`, `|vel|² >
/// 64`, `-4 > wall normal · vel`, the normal of the previous tick at `creature+0x11a0`) stuns
/// (0x0040ffe0), ends the glide and plays sound 0x17; a dive (flag 0x40) while falling cancels
/// this tick's gravity (`vel.z += dt_s * 30`); then the wind (0x004d9720) pushes the velocity
/// by `wind * dt_s`. On the ground a roll tilt (`|rot.y|`) above 40 degrees stuns the same
/// way. `phys` is the creature's physics flags of the previous tick; `flags` and `vel` are the
/// movement's working copies of `entity+0x114` and `entity+0x24`.
pub fn glide(world: &World, e: &mut EntityData, st: &CreatureState, id: i64, phys: u32, flags: &mut u16, vel: &mut [f32; 3], dt_s: f32, out: &mut ServerUpdate) {
    if world.local_player != Some(id) || *flags & 0x10 == 0 {
        return;
    }
    let crash = |e: &mut EntityData, flags: &mut u16, out: &mut ServerUpdate| {
        let s = stun_duration(e);
        w32(&mut e.0, 0x11c, s as u32);
        *flags &= !0x10;
        out.sounds.push(sound(pos_at(&e.0), 0x17, 1.0, 1.0));
    };
    if phys & 2 != 0 {
        *flags &= !0x10;
        return;
    }
    if phys & 1 == 0 {
        // 0x00540a40
        if phys & 4 != 0 {
            let v = *vel;
            let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            if l2 > 64.0f32 {
                let n = st.wall_normal;
                let d = n[0] * v[0] + n[1] * v[1] + n[2] * v[2];
                if -4.0f32 > d {
                    crash(e, flags, out);
                }
            }
        }
        // 0x00540af6
        if *flags & 0x40 != 0 && 0.0f32 > vel[2] {
            vel[2] = dt_s * 30.0f32 + vel[2];
        }
        // 0x00540b29: `vel += wind * dt_s` (0x004e1520, 0x00401650).
        let w = wind(pos_at(&e.0));
        let d = [w[0] * dt_s, w[1] * dt_s, w[2] * dt_s];
        *vel = [vel[0] + d[0], vel[1] + d[1], vel[2] + d[2]];
    } else {
        // 0x00540b6b: fabsf(rot.y) (0x00401ca0) > 40.
        if f32_at(&e.0, 0x1c).abs() > 40.0f32 {
            crash(e, flags, out);
        }
    }
}

/// 0x00541f2d..0x005421d8, the vertical collision response of a sub-step, before the move is
/// undone. With a local player in the world (`world+0xb8 != NULL`) any creature falling faster
/// than 2 blocks/s lands with sound 0x20 (pitch `rand() * 0.2 / 32767 + 1`, volume 0.5). Out of
/// water (`phys & 2`, the previous tick's) and faster than 5 blocks/s, a fall of more than 20
/// blocks below the last ground height (`creature+0x13bc`, 0x004f7a30 `F(z) - pos.z` against
/// 0x00402d40 `F(20)`) hurts the local player: `dmg = blocks(ftol(ftol(ftol(d / 20) - 65536) *
/// maxHp) * 0.5))` (0x004ce310, 0x00405660, 0x0052ebb0 twice, 0x00401420) off its HP, a stun
/// of at least the stun duration, flag 1 (climbing) off, sound 0x17, HP floored at 0 and a Hit
/// record from attacker 0 (not applied). `vel_z` is the velocity's z before the response
/// zeroes it; `flags` the movement's copy of `entity+0x114`.
pub fn fall_damage(world: &mut World, e: &mut EntityData, st: &CreatureState, id: i64, phys: u32, vel_z: f32, flags: &mut u16, out: &mut ServerUpdate) {
    if world.local_player.is_some() && -2.0f32 > vel_z {
        let pitch = world.rng.rand() as f32 * 0.2f32 / 32767.0f32 + 1.0f32;
        out.sounds.push(sound(pos_at(&e.0), 0x20, pitch, 0.5));
    }
    if phys & 2 != 0 || !(-5.0f32 > vel_z) {
        return;
    }
    let pos = pos_at(&e.0);
    let d = fix(st.ground_z).wrapping_sub(pos[2]);
    if !(d > fix(20.0)) || world.local_player != Some(id) {
        return;
    }
    // 0x004ce310: `ftol((x87) d / 20.0)`; d > 0 here and the quotient's fraction is at least
    // 0.05, so the truncating integer division is exact.
    let q = d / 20;
    let r = q.wrapping_sub(1 << 16);
    let t = mul_i64_f32_trunc(r, crate::stats::max_hp(e));
    let u = mul_i64_f32_trunc(t, 0.5);
    let dmg = blocks(u);
    let hp = f32_at(&e.0, 0x15c) - dmg;
    wf32(&mut e.0, 0x15c, hp);
    // 0x005420eb: the stun is at least the stun duration (`cmp; jg` keeps a longer one).
    let mut s = i32_at(&e.0, 0x11c);
    if !(s > stun_duration(e)) {
        s = stun_duration(e);
    }
    w32(&mut e.0, 0x11c, s as u32);
    // 0x0054211c: setFlag(1, 0) on the entity (and the movement's copy).
    *flags &= !1;
    let f = crate::util::u16_at(&e.0, 0x114) & !1;
    e.0[0x114..0x116].copy_from_slice(&f.to_le_bytes());
    out.sounds.push(sound(pos, 0x17, 1.0, 1.0));
    // 0x00542167: comiss 0, hp; jbe keeps.
    if 0.0f32 > f32_at(&e.0, 0x15c) {
        wf32(&mut e.0, 0x15c, 0.0);
    }
    // 0x0054217d: Hit {attacker 0, target id, damage, crit 0, pos} on `out+0` (0x00428400).
    let mut h = [0u8; 0x48];
    w64(&mut h, 0, 0);
    w64(&mut h, 8, id);
    wf32(&mut h, 0x10, dmg);
    h[0x14] = 0;
    for (i, p) in pos.iter().enumerate() {
        w64(&mut h, 0x20 + i * 8, *p);
    }
    out.hits.push(Hit(h));
}

/// 0x00544662..0x0054471c, the local player only (`c == world+0xb8`): climbing (flag 1, not
/// gliding) off a wall it was touching on the previous tick (`phys & 4` then, not now) while
/// facing away from that wall's normal (`fwd · normal <= 0`, NaN skips) and accelerating
/// sideways (`|accel.xy|² > 0`) kicks it off: `vel = old normal * -5`. `fwd` is the facing
/// vector of the wall probes (0x0054356c), `old_wall` the normal saved at 0x00542a9a,
/// `was_wall` the previous `phys & 4`.
pub fn wall_bounce(world: &World, id: i64, fwd: [f32; 3], old_wall: [f32; 3], flags: u16, was_wall: bool, phys: u32, accel: [f32; 3], vel: &mut [f32; 3]) {
    if world.local_player != Some(id) {
        return;
    }
    let d = fwd[0] * old_wall[0] + fwd[1] * old_wall[1] + fwd[2] * old_wall[2];
    // 0x0054468c: comiss 0, dot; jb skips (0 < dot, or NaN).
    if 0.0f32 < d || d.is_nan() {
        return;
    }
    if flags & 1 == 0 || flags & 0x10 != 0 || !was_wall || phys & 4 != 0 {
        return;
    }
    if !(accel[0] * accel[0] + accel[1] * accel[1] > 0.0f32) {
        return;
    }
    *vel = [old_wall[0] * -5.0f32, old_wall[1] * -5.0f32, old_wall[2] * -5.0f32];
}

/// 0x00544b4f..0x00544cc6, in the on-ground walk-cycle branch, with a local player in the world
/// (`world+0xb8 != NULL`, any creature): not rolling, not in water, faster than 5 blocks/s
/// horizontally (`hlen`) and the walk cycle (`creature+0x118c`) passing a multiple of pi
/// (`(int)(old / pi) < (int)(new / pi)`, doubles truncated) plays a footstep at the creature:
/// on a type-3 block under the feet (0x00406050 at `pos.z - F(scale.z * 0.5) - F(0.5)`) kind
/// `0x21 + rand() % 3` with pitch 1, else kind 0x20 with pitch `rand() * 0.2 / 32767 + 1`.
pub fn footstep(world: &mut World, e: &EntityData, phys: u32, hlen: f32, old_walk: f32, walk: f32, out: &mut ServerUpdate) {
    if world.local_player.is_none() {
        return;
    }
    if i32_at(&e.0, 0x118) != 0 || phys & 2 != 0 || !(hlen > 5.0f32) {
        return;
    }
    let pi = std::f64::consts::PI;
    let a = (f64::from(old_walk) / pi) as i32;
    let b = (f64::from(walk) / pi) as i32;
    if a >= b {
        return;
    }
    let pos = pos_at(&e.0);
    let sz = f32_at(&e.0, 0x78);
    let z = pos[2].wrapping_sub(fix(sz * 0.5f32)).wrapping_sub(fix(0.5f32));
    let (kind, pitch) = if block_type(block_fix(world, [pos[0], pos[1], z])) == 3 {
        (0x21 + (world.rng.rand() % 3) as u32, 1.0f32)
    } else {
        (0x20, world.rng.rand() as f32 * 0.2f32 / 32767.0f32 + 1.0f32)
    };
    out.sounds.push(sound(pos, kind, pitch, 1.0));
}

/// 0x005453fd..0x00545463, before the roll timer runs down, with a local player in the world
/// (`world+0xb8 != NULL`, any creature): a roll crossing 500 ms left (`roll > 500 && roll -
/// dt <= 500`) plays sound 0x1a at the creature.
pub fn roll_sound(world: &World, e: &EntityData, dt: i32, out: &mut ServerUpdate) {
    if world.local_player.is_none() {
        return;
    }
    let roll = i32_at(&e.0, 0x118);
    if roll > 0x1f4 && roll.wrapping_sub(dt) <= 0x1f4 {
        out.sounds.push(sound(pos_at(&e.0), 0x1a, 1.0, 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_ftol_product() {
        assert_eq!(mul_i64_f32_trunc(3 << 16, 0.5), 3 << 15);
        assert_eq!(mul_i64_f32_trunc(-3, 0.5), -1);
        assert_eq!(mul_i64_f32_trunc(1 << 20, 100.0), 100 << 20);
        assert_eq!(mul_i64_f32_trunc(7, 0.1), (7.0f64 * f64::from(0.1f32)) as i64);
    }

    #[test]
    fn mount_factor_curve() {
        assert_eq!(mount_factor(0), 0.0);
        assert!((mount_factor(10) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn server_world_runs_nothing_local() {
        let world = World::new(1);
        let mut e = EntityData::constructed();
        e.0[0x58] = 0x6a;
        let mut st = CreatureState::default();
        let mut cap = 6.0f32;
        local_speed(&world, &mut e, &mut st, 1, None, 0, 0, 0.02, &mut cap);
        assert_eq!(e.0[0x58], 0x6a);
        assert_eq!(cap, 6.0);
    }

    #[test]
    fn local_rider_without_pet_dismounts() {
        let mut world = World::new(1);
        world.is_client = true;
        world.local_player = Some(1);
        let mut e = EntityData::constructed();
        e.0[0x58] = 0x6a;
        let mut st = CreatureState::default();
        let mut cap = 6.0f32;
        local_speed(&world, &mut e, &mut st, 1, None, 0, 0, 0.02, &mut cap);
        assert_eq!(e.0[0x58], 0);
    }
}
