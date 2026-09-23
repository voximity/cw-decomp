//! The movement of a living creature in `World::tick` (`Server.exe 0x005322d0`, from 0x0053ed04
//! to 0x00545be1): the acceleration and speed cap, gravity, the push between creatures, the
//! sub-stepped collision with blocks and static objects, the ground, water and wall probes, the
//! friction and the yaw and pitch that follow the motion. Ported from the disassembly
//! (`analysis/notes/functions/005322d0_World_tick_creatures.md`); every float operation keeps
//! the original's order and precision, positions are 16.16 fixed i64 as in the entity block.
//!
//! Client and server: the blocks gated on the local player (`world+0xb8`,
//! [`World::local_player`], NULL on a server) or on `world+0xb4` ([`World::is_client`]) are
//! ported where they sit, their bodies in [`crate::client`]; with the defaults (a server) they
//! do nothing. Not ported: the airships (the world has none yet) and the gear part of the
//! attack speed. The sneak mode 0x4f's exposure is in
//! [`crate::consumables`], the mount and pet riding and the render position at
//! `creature+0x1350` in [`crate::riding`].

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern, clippy::excessive_precision, clippy::too_many_arguments)]

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Particle, Sound};
use cw_world::World;

use crate::combat::CreatureState;
use crate::skills::{attack_speed, climb_factor, glide_factor, ride_factor, skill, skill_duration, skill_level_factor, skill_total_time, skill_windup, swim_factor};
use crate::util::{block_type, blocks, cell_statics, elite, f32_at, fix, floor_div_fix, has_buff, i32_at, is_enemy, len_sq3, length3, lerp_repeat3, normalize3, pos_at, rotate_z, set_pos, set_vec3f, solid, to_block, u16_at, u32_at, vec3f_at, w16, w32, wf32};

fn len_sq2(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1]
}

fn normalize2(v: &mut [f32; 3]) {
    let s = 1.0f32 / (f64::from(v[0] * v[0] + v[1] * v[1]).sqrt() as f32);
    v[0] *= s;
    v[1] *= s;
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    b[1] * a[1] + b[0] * a[0] + b[2] * a[2]
}

fn dot2(a: [f32; 3], b: [f32; 3]) -> f32 {
    b[1] * a[1] + b[0] * a[0]
}

/// `lerpRepeat` 0x0052e710: `n` times `v += (target - v) * t`.
fn lerp_repeat(v: &mut f32, target: f32, n: i32, t: f32) {
    for _ in 0..n {
        *v = (target - *v) * t + *v;
    }
}

/// `turnTowards` 0x005306d0: `a + asin(clamp(sin(b)cos(a) - cos(b)sin(a))) * k` in degrees.
pub(crate) fn turn_towards(a: f32, b: f32, k: f32) -> f32 {
    let ra = f64::from((f64::from(a / 180.0f32) * std::f64::consts::PI) as f32);
    let rb = f64::from((f64::from(b / 180.0f32) * std::f64::consts::PI) as f32);
    // `_libm_sse2_cos_precise` / `_libm_sse2_sin_precise` / `_libm_sse2_asin_precise` (cw_math).
    let (ca, sa) = (cw_math::cos(ra), cw_math::sin(ra));
    let (cb, sb) = (cw_math::cos(rb), cw_math::sin(rb));
    let mut x = (sb * ca - cb * sa) as f32;
    if x > 1.0 {
        x = 1.0;
    } else if !(-1.0f32 <= x) {
        x = -1.0;
    }
    // 0x005307a5: `cvtss2sd; call asin; cvtsd2ss`, then `(double)asin * (k * 180.0 / PI)`.
    let s = cw_math::asin(f64::from(x)) as f32;
    ((f64::from(s) * (f64::from(k) * 180.0 / std::f64::consts::PI)) as f32) + a
}

/// `isGliding` 0x0040f6e0 (named `isSwimmingDown` in earlier notes): flag 0x10 set, falling,
/// airborne, not stunned or sped up.
fn is_gliding(e: &EntityData) -> bool {
    u16_at(&e.0, 0x114) & 0x10 != 0 && !(0.0 <= f32_at(&e.0, 0x2c)) && u32_at(&e.0, 0x4c) & 1 == 0 && i32_at(&e.0, 0x11c) <= 0 && i32_at(&e.0, 0x118) <= 0
}

/// `Server.exe 0x005308b0`: a creature whose box overlaps a solid block is lifted so its
/// bottom sits 0.1 block above the block containing it (one lift per tick; the airship part
/// is not ported). Unlike `getBlock`, this pass looks the region and zone up itself and
/// tests nothing where the zone is not loaded.
fn unstick(world: &World, e: &mut EntityData, st: &mut CreatureState) {
    if !(0.0 < f32_at(&e.0, 0x15c) || f32_at(&e.0, 0x15c) == 0.0) {
        return;
    }
    let scale = vec3f_at(&e.0, 0x70);
    let pos = pos_at(&e.0);
    let h = [fix(scale[0] * 0.5f32), fix(scale[1] * 0.5f32), fix(scale[2] * 0.5f32)];
    let bmin = [to_block(pos[0] - h[0]), to_block(pos[1] - h[1]), to_block(pos[2] - h[2])];
    let bmax = [to_block(pos[0] + h[0]), to_block(pos[1] + h[1]), to_block(pos[2] + h[2])];
    for x in bmin[0]..=bmax[0] {
        for y in bmin[1]..=bmax[1] {
            let mut hit = false;
            let loaded = (0..0x10000).contains(&(x >> 8)) && (0..0x10000).contains(&(y >> 8)) && world.zone(x >> 8, y >> 8).is_some();
            for z in bmin[2]..=bmax[2] {
                if loaded && solid(world.block(x, y, z)) {
                    hit = true;
                }
            }
            if hit {
                let hz = fix(scale[2] * 0.5f32);
                let t = pos[2] - hz;
                let blk = to_block(t);
                let z = ((i64::from(blk) << 16) + hz) + fix(1.1f32);
                let mut p = pos;
                p[2] = z;
                set_pos(&mut e.0, p);
                st.step_offset = 0.0;
                return;
            }
        }
    }
}

/// The movement of one living creature this tick, from `delta = accel * dt_s` (0x0053ed04) to
/// the roll timer and aim smoothing before the loop's `++creature` (0x00545be1).
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
pub fn move_creature(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, dt: i32, out: &mut ServerUpdate) {
    let dt_f = dt as f32;
    let dt_s = dt_f * 0.001f32;
    // Performance note for a future optimizer: the original reads the other creatures in
    // place; this copy of every other entity block per creature keeps the borrow simple and
    // costs O(n²) copies per tick, which is fine for tens of creatures.
    let others: Vec<(i64, EntityData)> = entities.iter().filter(|(k, _)| **k != id).map(|(k, v)| (*k, v.clone())).collect();
    // The mount (`creature+0x11c0`) and the owner (`entity+0x188`) read at 0x00544775 and
    // 0x00545728 (riding.rs), looked up before this creature's borrow.
    let (mount, owner) = crate::riding::gather(entities, states, id);
    let e = entities.get_mut(&id).expect("creature");
    let st = states.entry(id).or_default();
    let hostile = e.0[0x50];
    let stun = i32_at(&e.0, 0x11c);
    let mut flags = u16_at(&e.0, 0x114);
    let phys = u32_at(&e.0, 0x4c);
    let scale = vec3f_at(&e.0, 0x70);
    let accel = vec3f_at(&e.0, 0x30);

    // 0x0053ed04: the velocity change from the acceleration; none while stunned; no upward
    // part without flag 1 while the physics flag 0x10 (inside blocks) is set.
    let mut delta = [accel[0] * dt_s, accel[1] * dt_s, accel[2] * dt_s];
    let mut can_step_up = true;
    if stun > 0 {
        delta = [0.0; 3];
    }
    if flags & 1 == 0 && phys & 0x10 != 0 && delta[2] > 0.0 {
        delta[2] = 0.0;
    }
    // 0x0053ed8c: the body tilt from turning (rotation[1]), eased.
    let mut tilt = 0.0f32;
    let mut dxy = delta;
    if f64::from(len_sq2(dxy)) > 0.01 {
        let mut vxy = vec3f_at(&e.0, 0x24);
        if len_sq2(vxy) > 0.01f32 {
            normalize2(&mut dxy);
            normalize2(&mut vxy);
            let mut cr = vxy[1] * dxy[0];
            cr -= vxy[0] * dxy[1];
            if -1.0f32 > cr {
                cr = -1.0;
            } else if cr > 1.0f32 {
                cr = 1.0;
            }
            let a = cw_math::asinf(cr); // 0x0053eeaa: the float wrapper 0x00402480
            tilt = (f64::from(a) / std::f64::consts::PI * 180.0 * 0.5) as f32;
        }
    }
    if scale[0] > 2.0f32 {
        tilt = (2.0f32 / scale[0]) * tilt;
    }
    {
        let mut ry = f32_at(&e.0, 0x1c);
        if is_gliding(e) {
            tilt *= 40.0f32;
            if tilt > 90.0f32 {
                tilt = 90.0;
            } else if -90.0f32 > tilt {
                tilt = -90.0;
            }
            lerp_repeat(&mut ry, tilt, dt, 0.001f32);
        } else {
            lerp_repeat(&mut ry, tilt, dt, 0.005f32);
        }
        wf32(&mut e.0, 0x1c, ry);
    }
    // 0x0053eff4: the speed cap.
    let mut max_speed = 6.0f32;
    if elite(e) || u16_at(&e.0, 0x6e) & 0x200 != 0 {
        max_speed = 8.0;
    }
    if hostile == 5 {
        max_speed = 12.0;
    }
    if e.0[0x58] == 0x6b {
        if phys & 2 != 0 {
            max_speed = ride_factor(skill(e, 5)) * max_speed;
        }
    } else if phys & 2 != 0 {
        max_speed = swim_factor(skill(e, 4)) * max_speed;
    }
    // 0x0053f099..0x0053f228: the local player's mount and debug speed (client.rs). The pet
    // (`creature+0x11c8`) is looked up by `World::findEntity` 0x00405420.
    {
        let pet_id = st.pet;
        let pet = if pet_id == id { Some((f32_at(&e.0, 0x15c), i32_at(&e.0, 0x54))) } else { others.iter().find(|(k, _)| *k == pet_id).map(|(_, o)| (f32_at(&o.0, 0x15c), i32_at(&o.0, 0x54))) };
        crate::client::local_speed(world, e, st, id, pet, phys, flags, dt_s, &mut max_speed);
    }
    // 0x0053f230: gliding.
    if flags & 0x10 != 0 {
        let g = glide_factor(skill(e, 3));
        if g > 0.0 {
            if phys & 1 == 0 {
                max_speed = g * max_speed;
            }
        } else {
            flags &= 0xffef;
        }
    }
    // 0x0053f281: climbing drains stamina.
    if flags & 1 != 0 && len_sq3(accel) > 0.0 {
        let y = climb_factor(skill(e, 2));
        if y > 0.0 {
            if flags & 0x10 == 0 && phys & 4 != 0 && phys & 1 == 0 {
                let s = st.stamina - ((1.0f32 - y) * 5e-4f32) * dt_f;
                st.stamina = s;
                if 0.0 > s {
                    st.stamina = 0.0;
                }
                if 0.01f32 > st.stamina {
                    flags &= 0xfffe;
                }
            }
        } else {
            flags &= 0xfffe;
        }
    }
    // 0x0053f37a: the guard. Mode 0x4f (sneaking) computes it from the light level and the
    // nearby objects (consumables.rs).
    if e.0[0x58] == 0x4f {
        max_speed = (skill_level_factor(e, 0x4f, -1) * 0.4f32 + 0.5f32) * max_speed;
        // 0x0053f3d1..0x0053fa2a
        crate::consumables::sneak_guard(world, &others, id, flags, e, st, dt_f);
    } else if flags & 0x400 != 0 {
        let s = skill_level_factor(e, 0x63, -1);
        let g = (s * 5e-4f32 + 1e-5f32) * dt_f + st.block;
        st.block = g;
        if g > 1.0 {
            st.block = 1.0;
        }
    } else {
        let g = st.block - dt_f * 5e-4f32;
        st.block = g;
        if 0.0 > g {
            st.block = 0.0;
        }
    }
    if has_buff(st, 3) {
        st.block = 1.0;
    }
    // 0x0053fa77: MP from the guard.
    {
        let mp = (dt_f * 2.5e-4f32) * st.block + f32_at(&e.0, 0x160);
        wf32(&mut e.0, 0x160, if mp > 1.0 { 1.0 } else { mp });
    }
    // 0x0053fab9: stamina.
    let glide = flags & 0x10;
    if glide != 0 && phys & 1 == 0 {
        if flags & 0x40 != 0 {
            st.stamina -= dt_f * 2e-4f32;
        }
    } else if flags & 1 != 0 && glide == 0 && phys & 4 != 0 && phys & 1 == 0 {
        // climbing: unchanged
    } else {
        // 0x0053fb09: `mulss [0x573720]`, 1e-4 (0x38d1b717).
        st.stamina = dt_f * 1e-4f32 + st.stamina;
    }
    if 0.0 > st.stamina {
        st.stamina = 0.0;
    }
    if st.stamina > 1.0 {
        st.stamina = 1.0;
    }
    // 0x0053fb4e: sprinting doubles the cap; a type-0xc buff scales it by the haste skill.
    if flags & 0x40 != 0 && !(flags & 1 != 0 && phys & 4 != 0) && !(flags & 0x10 != 0 && phys & 1 == 0) {
        max_speed = (f64::from(max_speed) * 2.0) as f32;
    }
    let haste = has_buff(st, 0xc);
    if haste {
        max_speed = (skill_level_factor(e, 0x64, -1) + 1.0f32) * max_speed;
    }
    let guard = st.block;
    // 0x0053fbca: dashes.
    let mode = e.0[0x58];
    let mode_time = i32_at(&e.0, 0x5c);
    if mode == 0x30 || (mode == 0x36 && mode_time < skill_windup(e, guard, haste, -1) + skill_duration(e, guard, haste, -1)) || (matches!(mode, 0x32 | 0x60) && mode_time < 500) {
        max_speed = 40.0;
    }
    // 0x0053fc20: the melee lunge towards an enemy within reach.
    let mut blocked = false;
    let lunge = mode == 0x30 || (matches!(mode, 0x3a | 0x41 | 0x42 | 0x43 | 0xc | 0x10 | 3 | 0x3e | 0xb | 4 | 1 | 9 | 2 | 6 | 7 | 0xe | 0xd | 0xf | 0x14 | 0x13 | 0x12 | 0x11 | 5 | 0xa) && mode_time < 200);
    // 0x0053fca5: on a server every lunge; in the client's world only the local player's.
    if (world.local_player == Some(id) || !world.is_client) && lunge {
        let me = e.clone();
        let mypos = pos_at(&me.0);
        for (_, o) in &others {
            if !(f32_at(&o.0, 0x15c) > 0.0) || !is_enemy(&me, o) {
                continue;
            }
            let r = f32_at(&o.0, 0x70) * 0.7f32 + (scale[0] + 1.0f32);
            let r2 = r * r;
            let op = pos_at(&o.0);
            let d = [mypos[0].wrapping_sub(op[0]), mypos[1].wrapping_sub(op[1])];
            let dsq = d[0].wrapping_mul(d[0]) / 65536 + d[1].wrapping_mul(d[1]) / 65536;
            if dsq < fix(r2) {
                // The original subtracts the creature's own z from itself: dz is always 0.
                let adz = blocks(0).abs();
                let mut h = f32_at(&o.0, 0x78) * 0.5f32;
                h += scale[2];
                h += 1.0f32;
                if h > adz {
                    blocked = true;
                }
            }
        }
        if blocked || mode_time > 500 {
            if e.0[0x58] == 0x30 {
                st.charge = 0.0;
                wf32(&mut e.0, 0x160, 1.0);
                e.0[0x58] = 0;
                w32(&mut e.0, 0x5c, 0);
                let w = 0x2f0 + 7 * 0x118;
                if e.0[w] == 3 {
                    let sub = e.0[w + 1];
                    e.0[0x58] = if sub == 5 {
                        5
                    } else if sub == 3 {
                        0x11
                    } else {
                        0x14
                    };
                }
            }
            if e.0[0x58] == 0x2f {
                st.charge = 0.0;
                e.0[0x58] = 0x36;
            }
        }
        if !blocked {
            let v = rotate_z(f32_at(&e.0, 0x20), [0.0, max_speed * 2.0f32, 0.0]);
            delta = [delta[0] + v[0], delta[1] + v[1], delta[2] + v[2]];
        }
    }
    // 0x0053ffaa: a drink ends in a sit.
    if e.0[0x58] == 0x51 {
        let dur = consumable_duration(&e.0[0x1d8..0x1d8 + 0x118]);
        if i32_at(&e.0, 0x5c) > dur {
            e.0[0x58] = 0x53;
        } else {
            max_speed = 0.0;
        }
    }
    // 0x0053ffd1: rolling cancels modes.
    if i32_at(&e.0, 0x118) != 0 {
        let m = e.0[0x58];
        if matches!(m, 0x32 | 0x60) && i32_at(&e.0, 0x5c) <= skill_total_time(e, guard, haste) {
        } else if e.0[0x130] == 4 && e.0[0x131] == 1 {
            if !matches!(m, 5 | 0x14 | 0x11) {
                e.0[0x58] = 0;
            }
        } else {
            e.0[0x58] = 0;
            w32(&mut e.0, 0x134, 0);
        }
    }
    // 0x00540026: mode-driven displacement and speed.
    let mut forced = false;
    let m = e.0[0x58];
    let mt = i32_at(&e.0, 0x5c);
    if matches!(m, 0x44 | 0x5d | 0x45) && !(mt > skill_windup(e, guard, haste, -1)) {
        max_speed = 0.0;
    }
    if matches!(m, 0x44 | 0x5d | 0x45) && mt > skill_windup(e, guard, haste, -1) {
        let dur = skill_duration(e, guard, haste, -1);
        let q = (dur * 3) / 4;
        if mt < skill_windup(e, guard, haste, -1) + q {
            let v = rotate_z(f32_at(&e.0, 0x20), [0.0, 1.0, 0.0]);
            delta = [v[0] * 20.0f32, v[1] * 20.0f32, v[2] * 20.0f32];
            max_speed = 50.0;
            forced = true;
        }
    }
    for (mm, dir, sp) in [(0x4du8, [-1.0f32, 0.0, 0.0], 60.0f32), (0x4e, [1.0, 0.0, 0.0], 60.0), (0x4c, [0.0, 1.0, 0.0], 60.0)] {
        if m == mm && mt > skill_windup(e, guard, haste, -1) && mt < skill_windup(e, guard, haste, -1) + skill_duration(e, guard, haste, -1) {
            let v = rotate_z(f32_at(&e.0, 0x20), dir);
            delta = [v[0] * 20.0f32, v[1] * 20.0f32, v[2] * 20.0f32];
            max_speed = sp;
            forced = true;
        }
    }
    if m == 0x47 && mt < skill_total_time(e, guard, haste) {
        max_speed = 0.0;
        forced = true;
    }
    if m == 0x48 && mt < skill_total_time(e, guard, haste) {
        let rz = dt_s * 720.0f32 + f32_at(&e.0, 0x20);
        wf32(&mut e.0, 0x20, rz);
        forced = true;
        max_speed = 12.0;
    }
    if matches!(m, 0x44 | 0x5d | 0x45) && mt > skill_windup(e, guard, haste, -1) && mt < skill_total_time(e, guard, haste) {
        forced = true;
    }
    if hostile == 4 {
        max_speed = 30.0;
    }
    if matches!(m, 0x39 | 0x4a) && mt < skill_total_time(e, guard, haste) && mt >= skill_windup(e, guard, haste, -1) {
        max_speed = 0.1;
        forced = true;
    }
    // 0x0054050d (Cube.exe 0x0061a74d): charged MP. Mode 0x24 charges at 5e-4 per ms
    // (0x00573c80 = 0x3a03126f; Cube.exe 0x0071e1dc), the other charge modes below at 7.5e-4
    // (0x00573c84; Cube.exe 0x0071e1e0).
    if m == 0x24 {
        let c = attack_speed(e, guard, haste) * (dt_f * 5e-4f32) + f32_at(&e.0, 0x134);
        wf32(&mut e.0, 0x134, c);
    }
    if matches!(m, 0x18 | 0x19 | 0x1b | 8 | 0x3b | 0x3f | 0x40) && mt >= skill_windup(e, guard, haste, -1) {
        let mut rate = attack_speed(e, guard, haste) * (dt_f * 7.5e-4f32);
        if e.0[0x130] == 2 && e.0[0x131] == 0 {
            let mut r = (i32_at(&e.0, 0x60) as f32) / (crate::skills::max_block(e) as f32);
            if r > 4.0f32 {
                r = 4.0;
            }
            rate = (r + 1.0f32) * rate;
        }
        let add = if has_buff(st, 2) { (skill_level_factor(e, 0x66, -1) * 9.0f32 + 1.0f32) * rate } else { rate };
        let c = add + f32_at(&e.0, 0x134);
        wf32(&mut e.0, 0x134, c);
        if has_buff(st, 0xa) {
            let mp = f32_at(&e.0, 0x160);
            wf32(&mut e.0, 0x134, mp);
        }
    }
    if f32_at(&e.0, 0x134) > f32_at(&e.0, 0x160) {
        let mp = f32_at(&e.0, 0x160);
        wf32(&mut e.0, 0x134, mp);
    }
    // 0x005406d8: the slow and speed-up timers.
    if i32_at(&e.0, 0x124) != 0 {
        max_speed *= 0.5f32;
    }
    if i32_at(&e.0, 0x128) != 0 {
        max_speed *= 1.5f32;
    }
    // 0x00540712: slower into a wall the aim points at.
    if flags & 4 != 0 && i32_at(&e.0, 0x118) == 0 && !(matches!(e.0[0x58], 0x32 | 0x60) && i32_at(&e.0, 0x5c) <= 1000) {
        let vv = vec3f_at(&e.0, 0x24);
        let rh = vec3f_at(&e.0, 0x150);
        if 0.0 > dot2(rh, vv) {
            let (mut a, mut b) = (rh, vv);
            normalize2(&mut a);
            normalize2(&mut b);
            let d = dot2(a, b);
            max_speed = (d * 0.5f32 + 1.0f32) * max_speed;
        }
    }
    let _ = forced;
    // 0x005407ff: the velocity takes the change; its horizontal part is capped.
    let mut vel = vec3f_at(&e.0, 0x24);
    vel = [vel[0] + delta[0], vel[1] + delta[1], vel[2] + delta[2]];
    if len_sq2(vel) > max_speed * max_speed {
        let mut n = vel;
        normalize2(&mut n);
        n[0] *= max_speed;
        n[1] *= max_speed;
        vel[0] = n[0];
        vel[1] = n[1];
    }
    // 0x005408be: gravity and the vertical velocity.
    let app_flags = u16_at(&e.0, 0x6e);
    let pos0 = pos_at(&e.0);
    if flags & 1 != 0 && flags & 0x10 == 0 && phys & 4 != 0 {
        lerp_repeat3(&mut vel, [0.0; 3], dt, 0.0025f32);
        if vel[2] > -10.0f32 {
            st.ground_z = blocks(pos0[2]);
        }
    } else if (app_flags & 2 != 0 && stun <= 0) || is_gliding(e) {
        vel[2] -= (dt_s * 0.1f32) * 30.0f32;
        st.ground_z = blocks(pos0[2]);
    } else {
        if !(vel[2] < 0.0) {
            st.ground_z = blocks(pos0[2]);
        }
        vel[2] -= dt_s * 30.0f32;
    }
    set_vec3f(&mut e.0, 0x24, vel);
    // 0x00540a07..0x00540bff: the local player's glide: crash, dive, wind (client.rs).
    crate::client::glide(world, e, st, id, phys, &mut flags, &mut vel, dt_s, out);
    set_vec3f(&mut e.0, 0x24, vel);
    // The stun is read again from here on (the crash may have set it).
    let stun = i32_at(&e.0, 0x11c);
    // 0x00540bff: a stun or a running mode drops the movement flags.
    if stun > 0 {
        flags &= !0x10;
        flags &= !1;
        w32(&mut e.0, 0x134, 0);
    }
    if e.0[0x58] != 0 && i32_at(&e.0, 0x5c) < skill_total_time(e, guard, haste) {
        flags &= !0x10;
        flags &= !1;
    }
    w16(&mut e.0, 0x114, flags);
    // 0x00540c60: the displacement of this tick.
    let extra = vec3f_at(&e.0, 0x3c);
    let mut delta = [(extra[0] + vel[0]) * dt_s, (extra[1] + vel[1]) * dt_s, (extra[2] + vel[2]) * dt_s];
    // 0x00540ca0: creatures push each other apart.
    if hostile != 6 && !matches!(e.0[0x58], 0x53 | 0x54) {
        let pos = pos_at(&e.0);
        for (_, o) in &others {
            if hostile == 0 && o.0[0x50] != 1 && o.0[0x50] != 6 {
                continue;
            }
            if !(0.0 < f32_at(&o.0, 0x15c)) && !f32_at(&o.0, 0x15c).is_nan() {
                continue;
            }
            let mut os = vec3f_at(&o.0, 0x70);
            if o.0[0x50] != 6 {
                os = [os[0] + 1.0f32, os[1] + 1.0f32, os[2] + 0.0f32];
            }
            let op = pos_at(&o.0);
            let mut overlap = true;
            for k in 0..3 {
                let o_min = op[k] - fix(os[k] * 0.5f32);
                let s_max = pos[k] + fix(scale[k] * 0.5f32);
                if !(s_max >= o_min) {
                    overlap = false;
                    break;
                }
                let o_max = op[k] + fix(os[k] * 0.5f32);
                let s_min = pos[k] - fix(scale[k] * 0.5f32);
                if !(s_min < o_max) {
                    overlap = false;
                    break;
                }
            }
            if !overlap {
                continue;
            }
            let mut d = [blocks(pos[0].wrapping_sub(op[0])), blocks(pos[1].wrapping_sub(op[1])), 0.0];
            if !(len_sq3(d) > 0.0) {
                continue;
            }
            normalize3(&mut d);
            if stun > 0 {
                d = [d[0] * 0.1f32, d[1] * 0.1f32, d[2] * 0.1f32];
            }
            let k2 = ((dt_s * 5.0f32) * f32_at(&o.0, 0x70)) / scale[0];
            delta = [delta[0] + d[0] * k2, delta[1] + d[1] * k2, delta[2] + d[2] * k2];
        }
    }
    // 0x005411b5: the sub-steps.
    let len = length3(delta);
    let n_steps = (len + 1.0f32) as i32;
    let recip = 1.0f32 / (n_steps as f32);
    delta = [delta[0] * recip, delta[1] * recip, delta[2] * recip];
    let old_pos = pos_at(&e.0);
    let mut landed_static_z = false;
    let mut hit_static = false;
    // 0x00541216: `(world+0xb4 == 0 || c == world+0xb8) && hostile != 6`.
    if (!world.is_client || world.local_player == Some(id)) && hostile != 6 {
        unstick(world, e, st);
    }
    if n_steps > 0 {
        for _step in 0..n_steps {
            let step_base_z: i64 = 0;
            for axis in 0..3usize {
                let mut pos = pos_at(&e.0);
                pos[axis] = pos[axis].wrapping_add(fix(delta[axis]));
                set_pos(&mut e.0, pos);
                let mut found = false;
                let h = [fix(scale[0] * 0.5f32), fix(scale[1] * 0.5f32), fix(scale[2] * 0.5f32)];
                let bmin = [to_block(pos[0] - h[0]), to_block(pos[1] - h[1]), to_block(pos[2] - h[2])];
                let bmax = [to_block(pos[0] + h[0]), to_block(pos[1] + h[1]), to_block(pos[2] + h[2])];
                'scan: for bx in bmin[0]..=bmax[0] {
                    for by in bmin[1]..=bmax[1] {
                        for bz in bmin[2]..=bmax[2] {
                            let b = world.block(bx, by, bz);
                            if solid(b) || (app_flags & 0x100 != 0 && block_type(world.block(bx, by, bz)) != 2) {
                                found = true;
                            }
                        }
                        if found {
                            break 'scan;
                        }
                    }
                }
                // A block hit on x or y, or on z with a static landing already: the response.
                let mut respond = found && (axis != 2 || landed_static_z);
                let mut cl = can_step_up;
                if !respond {
                    // 0x005414ee: the static objects of the 3x3 cells around the position. With a
                    // block hit on z the test stops after the first cell (the original's order).
                    let cx0 = ((pos[0] >> 16) as i32) / 8;
                    let cy0 = ((pos[1] >> 16) as i32) / 8;
                    'cells: for cx in cx0 - 1..=cx0 + 1 {
                        for cy in cy0 - 1..=cy0 + 1 {
                            for (idx, s) in cell_statics(world, cx, cy) {
                                let kind = s.kind;
                                if matches!(kind, 7 | 6 | 9) {
                                    continue;
                                }
                                let zone_xy = ((s.x >> 16) as i32 / 256, (s.y >> 16) as i32 / 256);
                                if st.ignored_statics.contains(&(zone_xy.0, zone_xy.1, idx)) {
                                    continue;
                                }
                                if matches!(kind, 1 | 8 | 2 | 3 | 5) && s.b30 == 0 {
                                    continue;
                                }
                                let mut size = s.scale;
                                if s.rotation % 2 != 0 {
                                    size.swap(0, 1);
                                }
                                let sp = [s.x, s.y, s.z];
                                let rel = pos[axis].wrapping_sub(sp[axis]);
                                let prod = (rel as f64 * f64::from(delta[axis])) as i64;
                                if prod >= 0 {
                                    continue;
                                }
                                if !(pos[0] + fix(scale[0] * 0.5f32) >= sp[0] - fix(size[0] * 0.5f32)) {
                                    continue;
                                }
                                if !(pos[0] - fix(scale[0] * 0.5f32) < sp[0] + fix(size[0] * 0.5f32)) {
                                    continue;
                                }
                                if !(pos[1] + fix(scale[1] * 0.5f32) >= sp[1] - fix(size[1] * 0.5f32)) {
                                    continue;
                                }
                                if !(pos[1] - fix(scale[1] * 0.5f32) < sp[1] + fix(size[1] * 0.5f32)) {
                                    continue;
                                }
                                if !(pos[2] + fix(scale[2] * 0.5f32) >= sp[2]) {
                                    continue;
                                }
                                if !(pos[2] - fix(scale[2] * 0.5f32) < sp[2] + fix(size[2])) {
                                    continue;
                                }
                                if axis == 2 && 0.0 > vel[2] {
                                    landed_static_z = true;
                                }
                                hit_static = true;
                                can_step_up = false;
                                cl = false;
                                respond = true;
                                found = true;
                                break 'cells;
                            }
                            if found {
                                respond = true;
                                break 'cells;
                            }
                        }
                    }
                    // 0x00541a70: the airships (none here).
                }
                if !found {
                    continue;
                }
                let _ = respond;
                if axis == 2 {
                    // 0x00541f18: the vertical response; first the client's landing sound and the
                    // local player's fall damage (0x00541f2d..0x005421d8, client.rs).
                    crate::client::fall_damage(world, e, st, id, phys, vel[2], &mut flags, out);
                    let mut p = pos_at(&e.0);
                    p[2] = p[2].wrapping_sub(fix(delta[2]));
                    set_pos(&mut e.0, p);
                    vel[2] = 0.0;
                    wf32(&mut e.0, 0x2c, 0.0);
                    wf32(&mut e.0, 0x44, 0.0);
                    st.ground_z = blocks(p[2]);
                    continue;
                }
                // 0x00542250: horizontal: a step up by 1.01 block when nothing blocks it.
                let mut undo = !cl || app_flags & 0x100 != 0;
                if !undo {
                    let up = [0i64, 0, fix(1.01f32)];
                    let p = pos_at(&e.0);
                    let raised = [p[0] + up[0], p[1] + up[1], p[2] + up[2]];
                    let umin = [to_block(raised[0] - h[0]), to_block(raised[1] - h[1]), to_block(raised[2] - h[2])];
                    let umax = [to_block(raised[0] + h[0]), to_block(raised[1] + h[1]), to_block(raised[2] + h[2])];
                    'up: for bx in umin[0]..=umax[0] {
                        for by in umin[1]..=umax[1] {
                            for bz in umin[2]..=umax[2] {
                                if solid(world.block(bx, by, bz)) {
                                    undo = true;
                                    break 'up;
                                }
                            }
                        }
                    }
                }
                if undo {
                    let mut p = pos_at(&e.0);
                    p[axis] = p[axis].wrapping_sub(fix(delta[axis]));
                    set_pos(&mut e.0, p);
                } else {
                    let mut p = pos_at(&e.0);
                    p[2] -= step_base_z;
                    let z0 = p[2];
                    let hz = scale[2] * 0.5f32;
                    let t = z0 - fix(hz);
                    let blk = floor_div_fix(t);
                    p[2] = ((i64::from(blk) << 16) + fix(hz)) - ((1.01f64 * -65536.0) as i64);
                    st.step_offset = blocks(p[2] - (z0 - fix(st.step_offset)));
                    p[2] += step_base_z;
                    set_pos(&mut e.0, p);
                }
            }
        }
    }
    // The stun again (the fall damage may have set it).
    let stun = i32_at(&e.0, 0x11c);
    // 0x00542a67: the physics flags start over.
    let was_in_water = phys & 2 != 0;
    let was_on_ground = phys & 1 != 0;
    // 0x00542a93 / 0x00542a9a: the old wall flag and normal, read by the local player's wall
    // kick (0x00544662).
    let was_wall = phys & 4 != 0;
    let old_wall = st.wall_normal;
    let mut phys: u32 = 0;
    st.path.path_ms += dt;
    let pos = pos_at(&e.0);
    {
        let moved = [blocks(old_pos[0].wrapping_sub(pos[0])), blocks(old_pos[1].wrapping_sub(pos[1])), blocks(old_pos[2].wrapping_sub(pos[2]))];
        if stun <= 0 && len_sq2(vel) > 16.0f32 && len_sq3(accel) > 16.0f32 && len_sq3(delta) * 0.01f32 > len_sq3(moved) {
            phys |= 0x20;
        }
    }
    if hit_static {
        phys |= 0x40;
    }
    // 0x00542b87: the box around the position.
    let h = [fix(scale[0] * 0.5f32), fix(scale[1] * 0.5f32), fix(scale[2] * 0.5f32)];
    let bmin = [to_block(pos[0] - h[0]), to_block(pos[1] - h[1]), to_block(pos[2] - h[2])];
    let bmax = [to_block(pos[0] + h[0]), to_block(pos[1] + h[1]), to_block(pos[2] + h[2])];
    'inside: for x in bmin[0]..=bmax[0] {
        for y in bmin[1]..=bmax[1] {
            for z in bmin[2]..=bmax[2] {
                if block_type(world.block(x, y, z)) != 2 {
                    phys |= 0x10;
                    break 'inside;
                }
            }
        }
    }
    let mut liquid_contact = false;
    if hostile != 6 {
        let mut water_top = bmin[2];
        for x in bmin[0]..=bmax[0] {
            for y in bmin[1]..=bmax[1] {
                for z in bmin[2]..=bmax[2] {
                    if block_type(world.block(x, y, z)) == 2 {
                        if z + 1 > water_top {
                            water_top = z + 1;
                        }
                        liquid_contact = true;
                        phys |= 2;
                    }
                }
            }
        }
        if phys & 2 != 0 {
            let depth = blocks((i64::from(water_top) << 16).wrapping_sub(pos[2]));
            let r = depth / (scale[2] * 0.5f32);
            let mut buoy = 0.0f32;
            if r > 0.0 {
                buoy = if 1.0f32 > r { r + 1.0f32 } else { 1.0 };
                if e.0[0x58] == 0x6b {
                    buoy = r + 1.0f32;
                }
            }
            vel[2] = ((dt_s * 30.0f32) * buoy) + vel[2];
        }
    }
    let friction = if len_sq3(accel) > 0.0 { 0.0025f32 } else { 0.005f32 };
    // 0x00542f9f: the ground under the feet.
    if phys & 2 == 0 {
        let t1 = pos[2] - fix(scale[2] * 0.5f32);
        let t2 = t1 - fix(0.2f32);
        let z_feet = floor_div_fix(t2);
        for x in bmin[0]..=bmax[0] {
            for y in bmin[1]..=bmax[1] {
                if solid(world.block(x, y, z_feet)) {
                    phys |= 1;
                }
                if block_type(world.block(x, y, z_feet)) == 3 {
                    liquid_contact = true;
                }
            }
            if phys & 1 != 0 {
                break;
            }
        }
    }
    // 0x005430e2: a pet standing in lava refills its owner's +0x1198 (riding.rs, applied at
    // the end, when this creature's borrow is released).
    let refill_owner = liquid_contact && hostile == 5;
    // 0x0054314f: a creature that was walking on the ground and now has none under it steps
    // down onto the block up to 1.2 below.
    if was_on_ground && can_step_up && len_sq2(delta) > 0.0 && phys & 3 == 0 && !(0.0 < delta[2]) {
        let t1 = pos[2] - fix(scale[2] * 0.5f32);
        let t2 = t1 - fix(1.2f32);
        let z_step = floor_div_fix(t2);
        'down: for x in bmin[0]..=bmax[0] {
            for y in bmin[1]..=bmax[1] {
                if solid(world.block(x, y, z_step)) {
                    phys |= 1;
                    let old = pos[2] - fix(st.step_offset);
                    let a = i64::from(z_step) << 16;
                    let b = a + fix(1.01f32);
                    let n = b + fix(scale[2] * 0.5f32);
                    let mut p = pos_at(&e.0);
                    p[2] = n;
                    set_pos(&mut e.0, p);
                    st.step_offset = blocks(n - old);
                    break 'down;
                }
            }
            if phys & 1 != 0 {
                break;
            }
        }
    }
    let pos = pos_at(&e.0);
    // 0x00543370: a landing on a static object counts as ground.
    if landed_static_z {
        phys |= 1;
    }
    // 0x00543394: swimming as a mount damps the velocity.
    if e.0[0x58] == 0x6b && phys & 2 != 0 {
        let k = friction * 0.1f32;
        lerp_repeat(&mut vel[0], 0.0, dt, k);
        lerp_repeat(&mut vel[1], 0.0, dt, k);
        lerp_repeat(&mut vel[2], 0.0, dt, friction);
    }
    // 0x00543459: ground friction.
    if phys & 1 != 0 && e.0[0x58] != 0x30 {
        let m = e.0[0x58];
        let mt = i32_at(&e.0, 0x5c);
        let mut skip = false;
        if m == 0x36 && mt <= skill_windup(e, guard, haste, -1) + skill_duration(e, guard, haste, -1) {
            skip = true;
        }
        if !skip && matches!(m, 6 | 7 | 0x14 | 0x13 | 0x12 | 0x11 | 0xa) && mt < skill_windup(e, guard, haste, -1) {
            skip = true;
        }
        if !skip {
            lerp_repeat3(&mut vel, [0.0; 3], dt, friction);
        }
    }
    // 0x0054350d: the extra velocity fades on the ground, in water, while gliding or on a path.
    let mut extra = vec3f_at(&e.0, 0x3c);
    if phys & 3 != 0 || flags & 0x10 != 0 || !st.path_empty() {
        lerp_repeat3(&mut extra, [0.0; 3], dt, friction);
    }
    set_vec3f(&mut e.0, 0x3c, extra);
    // 0x0054356c: the walls around the box, by side and then by corner.
    let fwd = rotate_z(f32_at(&e.0, 0x20), [0.0, 1.0, 0.0]);
    let mut wall = [0.0f32; 3];
    let lo = |p: i64, s: f32| -> i32 {
        if hostile == 0 {
            (((p - fix(s * 0.5f32)) - fix(0.2f32)) >> 16) as i32
        } else {
            let n = (s * 0.5f32 + 0.5f32) as i32;
            ((((p - (i64::from(n) << 16)) - fix(0.5f32)) - fix(0.05f32)) >> 16) as i32
        }
    };
    let hi = |p: i64, s: f32| -> i32 {
        if hostile == 0 {
            (((p + fix(s * 0.5f32)) + fix(0.2f32)) >> 16) as i32
        } else {
            let n = (s * 0.5f32 + 0.5f32) as i32;
            ((((p + (i64::from(n) << 16)) + fix(0.5f32)) + fix(0.05f32)) >> 16) as i32
        }
    };
    let side = |world: &World, fixed_axis: usize, coord: i32, phys: &mut u32, wall: &mut [f32; 3], nx: f32, ny: f32| {
        let (a_min, a_max) = if fixed_axis == 1 { (bmin[0], bmax[0]) } else { (bmin[1], bmax[1]) };
        for a in a_min..=a_max {
            for z in bmin[2]..=bmax[2] {
                let b = if fixed_axis == 1 { world.block(a, coord, z) } else { world.block(coord, a, z) };
                if solid(b) {
                    *phys |= 4;
                    if fixed_axis == 1 {
                        wall[1] = ny;
                    } else {
                        wall[0] = nx;
                    }
                    return;
                }
            }
        }
    };
    side(world, 1, lo(pos[1], scale[1]), &mut phys, &mut wall, 0.0, 1.0);
    side(world, 1, hi(pos[1], scale[1]), &mut phys, &mut wall, 0.0, -1.0);
    side(world, 0, lo(pos[0], scale[0]), &mut phys, &mut wall, 1.0, 0.0);
    side(world, 0, hi(pos[0], scale[0]), &mut phys, &mut wall, -1.0, 0.0);
    if phys & 4 == 0 {
        let corners: [([f32; 3], bool, bool, f32, f32); 4] = [([0.7, 0.7, 0.0], false, false, 1.0, 1.0), ([-0.7, 0.7, 0.0], true, false, -1.0, 1.0), ([0.7, -0.7, 0.0], false, true, 1.0, -1.0), ([-0.7, -0.7, 0.0], true, true, -1.0, -1.0)];
        for (d, x_hi, y_hi, nx, ny) in corners {
            if phys & 4 != 0 {
                break;
            }
            if 0.1f32 < dot3(fwd, d) {
                continue;
            }
            let cx = if x_hi { hi(pos[0], scale[0]) } else { lo(pos[0], scale[0]) };
            let cy = if y_hi { hi(pos[1], scale[1]) } else { lo(pos[1], scale[1]) };
            for z in bmin[2]..=bmax[2] {
                if solid(world.block(cx, cy, z)) {
                    phys |= 4;
                    wall[0] = nx;
                    wall[1] = ny;
                    break;
                }
            }
        }
    }
    st.wall_normal = wall;
    // 0x00544444: water drags and tips the body; a splash on entry.
    let mut rot_x = f32_at(&e.0, 0x18);
    if phys & 2 != 0 && e.0[0x58] != 0x6b {
        lerp_repeat3(&mut vel, [0.0; 3], dt, 0.005f32);
        if hostile != 6 && app_flags & 1 == 0 {
            lerp_repeat(&mut rot_x, -60.0f32, dt, 0.01f32);
        }
        if !was_in_water && -3.0f32 > vel[2] {
            let mut p = [0u8; 0x48];
            for (i, v) in pos.iter().enumerate() {
                p[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
            }
            p[0x18..0x1c].copy_from_slice(&0.0f32.to_le_bytes());
            p[0x1c..0x20].copy_from_slice(&0.0f32.to_le_bytes());
            p[0x20..0x24].copy_from_slice(&10.0f32.to_le_bytes());
            for (i, c) in [0.2f32, 0.7, 1.0, 1.0].iter().enumerate() {
                p[0x24 + i * 4..0x28 + i * 4].copy_from_slice(&c.to_le_bytes());
            }
            p[0x34..0x38].copy_from_slice(&0.4f32.to_le_bytes());
            p[0x38..0x3c].copy_from_slice(&15u32.to_le_bytes());
            p[0x40..0x44].copy_from_slice(&3.0f32.to_le_bytes());
            out.particles.push(Particle(p));
            let pitch = world.rng.rand() as f32 / 32767.0f32 + 0.9f32;
            let mut s = [0u8; 0x18];
            for i in 0..3 {
                s[i * 4..i * 4 + 4].copy_from_slice(&blocks(pos[i]).to_le_bytes());
            }
            s[0xc..0x10].copy_from_slice(&0x1fu32.to_le_bytes());
            s[0x10..0x14].copy_from_slice(&pitch.to_le_bytes());
            s[0x14..0x18].copy_from_slice(&1.0f32.to_le_bytes());
            out.sounds.push(Sound(s));
        }
    } else {
        lerp_repeat(&mut rot_x, 0.0, dt, 0.01f32);
    }
    // 0x00544662..0x0054471c: the local player's kick off a wall (client.rs).
    crate::client::wall_bounce(world, id, fwd, old_wall, flags, was_wall, phys, accel, &mut vel);
    if i32_at(&e.0, 0x54) == 0x65 {
        let t = if e.0[0x58] == 0x33 && i32_at(&e.0, 0x5c) < 1000 { 60.0f32 } else { 0.0 };
        lerp_repeat(&mut rot_x, t, dt, 0.01f32);
    }
    wf32(&mut e.0, 0x18, rot_x);
    // 0x00544775: a creature mounted on another follows it (riding.rs) and skips the walk
    // cycle.
    if !crate::riding::follow_mount(e, st, mount.as_ref(), &mut vel) {
        walk_cycle(world, e, st, phys, flags, vel, scale, guard, haste, dt, out);
    }
    // 0x00544dc8: gliding and the 0x200 flag need their items.
    let s11 = 0x2f0 + 11 * 0x118;
    if !(e.0[s11] == 0x17 && e.0[s11 + 1] == 0) {
        flags &= 0xffef;
    }
    let s10 = 0x2f0 + 10 * 0x118;
    if e.0[s10] != 0x18 {
        flags &= 0xfdff;
    }
    w16(&mut e.0, 0x114, flags);
    // 0x00544dfb: the animation phase is capped at 1 (comiss/jbe) and eases to 0.
    if st.riding.anim > 1.0f32 {
        st.riding.anim = 1.0;
    }
    lerp_repeat(&mut st.riding.anim, 0.0, dt, 0.005f32);
    // 0x00544e3e: the step offset eases out.
    {
        let t = if 0.0 > st.step_offset { 0.01f32 } else { 0.02f32 };
        lerp_repeat(&mut st.step_offset, 0.0, dt, t);
    }
    // 0x00544e8b: the hit flash (`creature+0x1184`) eases to 0, `lerpRepeat(&flash, 0, dt,
    // 0.0075)` (0x3bf5c28f), for every creature (unguarded; a render field on a server).
    lerp_repeat(&mut st.ai.hit_flash, 0.0, dt, 0.0075f32);
    // 0x00544eb1: yaw and look pitch follow the motion.
    let mut look = f32_at(&e.0, 0x48);
    let mut rot_z = f32_at(&e.0, 0x20);
    if flags & 1 != 0 && phys & 4 != 0 && flags & 0x10 == 0 {
        lerp_repeat(&mut look, 45.0f32, dt, 0.005f32);
        let mut n = [-wall[0], -wall[1], -wall[2]];
        normalize3(&mut n);
        if n[1] > 1.0 {
            n[1] = 1.0;
        }
        if -1.0f32 > n[1] {
            n[1] = -1.0;
        }
        let ang = if n[0] > 0.0 { (-f64::from(cw_math::acosf(n[1])) / std::f64::consts::PI * 180.0) as f32 } else { (f64::from(cw_math::acosf(n[1])) / std::f64::consts::PI * 180.0) as f32 }; // acosf 0x00548b00 at 0x00544f7f / 0x00544ff6
        let t = turn_towards(rot_z, ang, 1.0);
        lerp_repeat(&mut rot_z, t, dt, 0.005f32);
    } else {
        look = cw_math::expf(dt_f * -0.01f32) * look; // expf 0x00548b20 at 0x00545036
        if flags & 4 != 0 && stun <= 0 {
            let mut d = st.aim;
            if len_sq2(d) > 0.0 && !forced {
                normalize2(&mut d);
                if d[1] > 1.0 {
                    d[1] = 1.0;
                }
                if -1.0f32 > d[1] {
                    d[1] = -1.0;
                }
                let ang = if d[0] > 0.0 { (-f64::from(cw_math::acosf(d[1])) / std::f64::consts::PI * 180.0) as f32 } else { (f64::from(cw_math::acosf(d[1])) / std::f64::consts::PI * 180.0) as f32 }; // acosf at 0x0054512c / 0x00545160
                let t = turn_towards(rot_z, ang, 1.0);
                lerp_repeat(&mut rot_z, t, dt, 0.1f32);
            }
            let mut v = st.aim;
            if len_sq3(v) > 0.0 {
                normalize3(&mut v);
                let p = (f64::from(cw_math::asinf(v[2])) / std::f64::consts::PI * 180.0) as f32; // asinf at 0x00545232
                let t = turn_towards(look, p, 1.0);
                lerp_repeat(&mut look, t, dt, 0.005f32);
            }
        } else if i32_at(&e.0, 0x118) == 0 && len_sq2(vel) > 0.02f32 && dot3(vel, accel) > 0.0 && !forced {
            let mut d = vel;
            normalize2(&mut d);
            if d[1] > 1.0 {
                d[1] = 1.0;
            }
            if -1.0f32 > d[1] {
                d[1] = -1.0;
            }
            rot_z = if d[0] > 0.0 { (-f64::from(cw_math::acosf(d[1])) / std::f64::consts::PI * 180.0) as f32 } else { (f64::from(cw_math::acosf(d[1])) / std::f64::consts::PI * 180.0) as f32 }; // acosf at 0x00545380 / 0x005453b4
        }
    }
    wf32(&mut e.0, 0x48, look);
    wf32(&mut e.0, 0x20, rot_z);
    // 0x005453fd..0x00545463: the client's roll sound (client.rs).
    crate::client::roll_sound(world, e, dt, out);
    // 0x00545469: the roll timer.
    let roll = i32_at(&e.0, 0x118);
    st.prev_roll = roll;
    if roll != 0 {
        let r = roll - dt;
        w32(&mut e.0, 0x118, if r < 0 { 0 } else { r } as u32);
    }
    // 0x0054549a: seats on airships (`creature+0x1d30`, world+0xc): not ported (no airships).
    // 0x00545728: a pet under its riding owner, the render position and rotation (riding.rs).
    crate::riding::follow_owner(e, st, owner.as_ref(), dt, &mut vel);
    // 0x00545b14: the aim eases towards the ray hit.
    let mut k2 = 0.0f32;
    lerp_repeat(&mut k2, 1.0, dt, 0.025f32);
    let ray = vec3f_at(&e.0, 0x150);
    for (a, r) in st.aim.iter_mut().zip(ray) {
        *a = (r - *a) * k2 + *a;
    }
    if e.0[0x58] == 0 || i32_at(&e.0, 0x5c) == 0 || i32_at(&e.0, 0x5c) > skill_total_time(e, guard, haste) {
        st.aim = ray;
    }
    set_vec3f(&mut e.0, 0x24, vel);
    w32(&mut e.0, 0x4c, phys);
    if refill_owner {
        crate::riding::refill_owner_stamina(entities, states, id, dt_s);
    }
}

/// `consumableDuration` 0x00413aa0: type 1 items last 3000 ms (sub type 1) or 10000 ms.
fn consumable_duration(item: &[u8]) -> i32 {
    if item[0] != 1 {
        0
    } else if item[1] == 1 {
        3000
    } else {
        10000
    }
}

/// The walk cycle (`creature+0x1188` anim phase, `+0x118c` stride) at 0x005447a6..0x00544dc0,
/// for a creature not placed on a mount. Moving (`|vel|² > 0.5`, not mode 0x6b) on the ground,
/// in water, with appearance flag 2 or climbing (flag 1 at a wall): a climber strides `s *
/// ((|vel| * dt_s) * 6)` and its phase grows `dt_s * 0.2`; otherwise with `hlen = |vel.xy|`
/// and `s = sqrtf(0.8 / scale.x)`: on the ground the stride grows `s * ((hlen * dt_s) * 1.5)`
/// (doubled in mode 0x4f), or `s * (((dt_f * 1e-4) * hlen) * 1.5)` while a lunge (0x30), the
/// slam (0x36 before its total time) or a charging swing (6, 7, 0x14, 0x13, 0x12, 0x11, 0xa
/// before the wind-up) holds it, the phase grows `dt_s`, and the client's footstep follows
/// (0x00544b4f, client.rs); off the ground (water, flag 2, climbing) the stride grows `s *
/// (((dt_f * 0.002) * hlen) * 1.5)` and the phase `dt_s`. Not moving, or moving without any
/// of those supports: in the air (no ground, no water, not climbing) the stride is 0 and the
/// phase grows `s * dt_s`. `phys` is the rebuilt physics flags, `flags` the entity flags,
/// `vel` the working velocity.
#[allow(clippy::too_many_arguments)]
fn walk_cycle(world: &mut World, e: &EntityData, st: &mut CreatureState, phys: u32, flags: u16, vel: [f32; 3], scale: [f32; 3], guard: f32, haste: bool, dt: i32, out: &mut ServerUpdate) {
    let dt_f = dt as f32;
    let dt_s = dt_f * 0.001f32;
    // 0x004024e0: `sqrtf` as a double square root of the f32 quotient, stored f32.
    let sq = |x: f32| f64::from(x).sqrt() as f32;
    let on_ground = phys & 1 != 0;
    let supported = on_ground || u16_at(&e.0, 0x6e) & 2 != 0 || (flags & 1 != 0 && phys & 4 != 0) || phys & 2 != 0;
    // 0x005447da: comiss |vel|², 0.5; jbe skips (NaN too).
    if supported && len_sq3(vel) > 0.5f32 && e.0[0x58] != 0x6b {
        if flags & 1 != 0 && phys & 4 != 0 {
            // 0x0054481b: climbing.
            let s = sq(0.8f32 / scale[0]);
            let len = length3(vel);
            let k = (len * dt_s) * 6.0f32;
            st.riding.walk = s * k + st.riding.walk;
            st.riding.anim = dt_s * 0.2f32 + st.riding.anim;
            return;
        }
        // 0x005449c8
        let old_walk = st.riding.walk;
        let hlen = f64::from(vel[0] * vel[0] + vel[1] * vel[1]).sqrt() as f32; // 0x00401d40
        if phys & 1 != 0 {
            let m = e.0[0x58];
            let mt = i32_at(&e.0, 0x5c);
            let locked = m == 0x30 || (m == 0x36 && mt < skill_total_time(e, guard, haste)) || (matches!(m, 6 | 7 | 0x14 | 0x13 | 0x12 | 0x11 | 0xa) && mt < skill_windup(e, guard, haste, -1));
            if !locked {
                // 0x00544a50
                let k = (hlen * dt_s) * 1.5f32;
                let s = sq(0.8f32 / scale[0]);
                let add = if m == 0x4f { (s * k) * 2.0f32 } else { s * k };
                st.riding.walk = add + st.riding.walk;
            } else {
                // 0x00544ac7
                let s = sq(0.8f32 / scale[0]);
                let k = ((dt_f * 1e-4f32) * hlen) * 1.5f32;
                st.riding.walk = s * k + st.riding.walk;
            }
            st.riding.anim = st.riding.anim + dt_s;
            // 0x00544b4f: the client's footstep.
            crate::client::footstep(world, e, phys, hlen, old_walk, st.riding.walk, out);
        } else {
            // 0x00544ccb
            let s = sq(0.8f32 / scale[0]);
            let k = ((dt_f * 0.002f32) * hlen) * 1.5f32;
            st.riding.walk = s * k + st.riding.walk;
            st.riding.anim = st.riding.anim + dt_s;
        }
        return;
    }
    // 0x00544d53
    if !on_ground && phys & 2 == 0 && !(flags & 1 != 0 && phys & 4 != 0) {
        st.riding.walk = 0.0;
        let s = sq(0.8f32 / scale[0]);
        st.riding.anim = s * dt_s + st.riding.anim;
    }
}
