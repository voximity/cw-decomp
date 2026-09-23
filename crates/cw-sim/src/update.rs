//! The per-creature update of `World::tick` (`Server.exe 0x005322d0`) after the interactions:
//! the HP clamp (0x00537856), the idle and behaviour hook, timers, the mode bookkeeping, buff
//! effects, the stun countdown, regeneration, threat decay and, in [`crate::physics`], the
//! movement. Addresses are Server.exe; `entity+X` is `creature+0x10+X` in the original.
//!
//! Not ported here, and noted where it applies: the behaviour update (`creature+0x13e4`, the
//! AI), path following (0x004db200), the mode machine (0x00537bdb jump table: skills,
//! attack wind-ups and their sounds and hits), the type-4 buff's damage over time (through
//! `creatureAttack` 0x004cfd50). The consumable modes 0x50/0x51 (0x0053e6bd; every creature but
//! the players on a server) are in [`crate::consumables`].
//!
//! The client guards (`world+0xb4`, [`World::is_client`]; `world+0xb8`,
//! [`World::local_player`]) are ported: in the client's world the AI hook, the type-0x90 decay,
//! the path, the out-of-combat heal are the server's, and the combo timer runs for the local
//! player only.

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_world::World;

use crate::combat::{Buff, CreatureState};
use crate::stats::max_hp;
use crate::util::{f32_at, i32_at, is_blocking, is_mage, u16_at, w16, w32, wf32};

/// `EntityData::setFlag` 0x00405570: a bit of the u16 at `entity+0x114`.
pub fn set_flag(e: &mut EntityData, mask: u16, on: bool) {
    let f = u16_at(&e.0, 0x114);
    w16(&mut e.0, 0x114, if on { f | mask } else { f & !mask });
}

/// The update of one creature from the HP clamp to the start of the movement, in the
/// original's order. `dt` is the tick's milliseconds. Returns false when the creature is
/// dead (HP <= 0), which skips the rest of its update (0x0053789b).
#[allow(clippy::too_many_lines)]
pub fn update_creature(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, projectiles: &mut Vec<crate::projectile::Projectile>, dirty: &mut BTreeSet<(i32, i32)>, id: i64, dt: i32, out: &mut ServerUpdate) -> bool {
    let dt_f = dt as f32;
    let (hostile, stun) = {
        let e = entities.get_mut(&id).expect("creature");
        // 0x00537856: HP clamp to maxHp.
        let max = max_hp(e);
        if f32_at(&e.0, 0x15c) > max {
            wf32(&mut e.0, 0x15c, max);
        }
        if !(f32_at(&e.0, 0x15c) > 0.0) {
            return false;
        }
        (e.0[0x50], i32_at(&e.0, 0x11c))
    };
    // 0x005378a1: non-players accumulate idle time and run their behaviour list every 32 ms
    // while not stunned; a stunned creature drops flag 0x80. 0x005378b1: not in the client's
    // world (`world+0xb4`): the server runs the AI.
    if hostile != 0 && !world.is_client {
        let st = states.entry(id).or_default();
        st.idle_ms += dt;
        if stun <= 0 {
            if st.idle_ms >= 0x20 {
                let elapsed = st.idle_ms;
                crate::behavior::run_behaviors(world, entities, states, id, elapsed, out);
                states.entry(id).or_default().idle_ms = 0;
            }
        } else {
            set_flag(entities.get_mut(&id).expect("creature"), 0x80, false);
        }
    }
    let e = entities.get_mut(&id).expect("creature");
    let st = states.entry(id).or_default();
    // 0x0053793b: a type-0x90 creature (a placed consumable) wastes away; 0x00537944 and
    // 0x00537987: in the client's world (`world+0xb4`) the decay and the path below are
    // skipped (to 0x00537ac7).
    if i32_at(&e.0, 0x54) == 0x90 && !world.is_client {
        let hp = f32_at(&e.0, 0x15c) - dt_f * 0.025f32;
        wf32(&mut e.0, 0x15c, if !(0.0 <= hp) { 0.0 } else { hp });
    }
    // 0x00537994: flag 0x80 clears; with a path (creature+0x1460) the creature follows it
    // (0x004db200, `path::path_tick`).
    if !world.is_client {
        let mut path = std::mem::take(&mut st.path);
        crate::path::path_tick(world, e, st, &mut path, dt);
        st.path = path;
    }
    // 0x00537acd: the slow, make-blue and speed-up timers count down; a slowed creature's
    // mode time is held past the skill's end; show-patch (f32) climbs back to zero.
    let slowed = i32_at(&e.0, 0x120) - dt;
    w32(&mut e.0, 0x120, slowed as u32);
    if slowed > 0 {
        // mode time = skillTotalTime(creature) + 1: not ported (the mode machine is not).
    }
    if i32_at(&e.0, 0x120) < 0 {
        w32(&mut e.0, 0x120, 0);
    }
    let v = (i32_at(&e.0, 0x124) - dt).max(0);
    w32(&mut e.0, 0x124, v as u32);
    let v = (i32_at(&e.0, 0x128) - dt).max(0);
    w32(&mut e.0, 0x128, v as u32);
    let sp = f32_at(&e.0, 0x12c);
    if 0.0 > sp {
        let v = dt_f / 180.0f32 + sp;
        wf32(&mut e.0, 0x12c, if 0.0 < v { 0.0 } else { v });
    }
    // 0x00537bc1: the skill mode machine (byte table 0x00548a8c, targets 0x00548a44).
    let mut ms = std::mem::take(&mut st.modes);
    crate::modes::run_mode_machine(world, entities, states, &mut ms, projectiles, dirty, id, dt, out);
    let e = entities.get_mut(&id).expect("creature");
    let st = states.entry(id).or_default();
    st.modes = ms;
    // 0x00537d01: the combo timer (entity+0x64), on a server for every creature and in the
    // client's world only for the local player (`world+0xb4 == 0 || c == world+0xb8`, else
    // to 0x00537e08).
    if !world.is_client || world.local_player == Some(id) {
        let last_hit = i32_at(&e.0, 0x64).wrapping_add(dt);
        w32(&mut e.0, 0x64, last_hit as u32);
        if last_hit > 4000 {
            // mode time = trunc((attackSpeed / attackSpeed) * mode time): unchanged for the
            // values a mode time takes.
            w32(&mut e.0, 0x60, 0);
            // 0x00537da0: cooldowns of modes flagged by 0x0040f6d0 restart at skillCooldown:
            // that predicate returns 0 (decompiled), so none does.
        }
    }
    // 0x00537e08: every cooldown not flagged by 0x0040f6d0 (none is) loses dt: `sub; jns`,
    // a negative result becomes 0.
    for c in st.cooldowns.values_mut() {
        let v = c.wrapping_sub(dt);
        *c = if v < 0 { 0 } else { v };
    }
    // 0x00537ea8: buffs lose dt; a live type-1 buff clears the stun; a type-4 buff deals its
    // damage each time its remaining duration crosses a 200 ms boundary (through
    // creatureAttack 0x004cfd50, `modes::active_buff_effect`); expired buffs go. The effects
    // run in list order after the durations, as the loop interleaves them in the original;
    // nothing an effect does reaches this creature's buff list.
    let mut live: Vec<Buff> = Vec::new();
    st.buffs.retain_mut(|b| {
        let d = i32_at(b, 8).wrapping_sub(dt);
        w32(b, 8, d as u32);
        if d > 0 {
            if matches!(b[0], 1 | 4) {
                live.push(*b);
            }
            true
        } else {
            false
        }
    });
    for b in &live {
        crate::modes::active_buff_effect(world, entities, states, id, b, dt, out, dirty);
    }
    let e = entities.get_mut(&id).expect("creature");
    let st = states.entry(id).or_default();
    // 0x0053e282: the stun counts down without a floor; while it lasts the mode resets.
    let stun = i32_at(&e.0, 0x11c) - dt;
    w32(&mut e.0, 0x11c, stun as u32);
    if stun > 0 {
        w32(&mut e.0, 0x5c, 0);
        e.0[0x58] = 0;
    }
    if e.0[0x50] != 6 {
        let mt = i32_at(&e.0, 0x5c);
        let mode = e.0[0x58];
        if !(mt < 0 && mode != 0) {
            if mt == 0 {
                // 0x0053e2c7: the mode's cooldown starts: `cooldowns[mode] =
                // skillCooldown(c, mode, -1)` (0x00409780; 8000..60000 ms scaled by the skill
                // level, 0 for modes without one). `canStartMode` 0x004096b0 reads it.
                let cd = crate::skills::skill_cooldown(e, i32::from(mode), -1);
                st.cooldowns.insert(mode, cd);
            }
            w32(&mut e.0, 0x5c, mt.wrapping_add(dt) as u32);
            // Mode 0x36's slam (vz = -60 between wind-up and end): not ported with the modes.
        }
    }
    // 0x0053e357: an idle creature flushes its dodge bookkeeping (creature+0x11b4 against
    // +0x11ac): `onMultiHit` and a type-4 Hit record for each dodger, then both sets clear.
    if e.0[0x58] == 0 {
        crate::modes::idle_flush(entities, states, id, out);
    }
    // The guard and haste the skill timings of the MP test below read.
    let (guard, haste) = crate::util::guard_haste(states, id);
    let e = entities.get_mut(&id).expect("creature");
    let st = states.entry(id).or_default();
    // 0x0053e4d3: MP. A caster regenerates 1e-4 per ms (0x00573720 = 0x38d1b717) outside its casting modes; anyone
    // else loses 5e-5 per ms outside the charging modes. Clamped to [0, 1].
    let mode = e.0[0x58];
    let mut mp = f32_at(&e.0, 0x160);
    if is_mage(e) {
        // Mode 0x1c, and the casts (0x5f, 0x25, 0x2e, 0x2d, 0x1f, 0x21, 0x2b, 0x22) before
        // their total time (0x0053e509: `modeTime < skillTotalTime` 0x004084b0, jl), block
        // the regeneration.
        let casting = matches!(mode, 0x5f | 0x25 | 0x2e | 0x2d | 0x1f | 0x21 | 0x2b | 0x22) && i32_at(&e.0, 0x5c) < crate::skills::skill_total_time(e, guard, haste);
        if mode != 0x1c && !casting {
            mp = dt_f * 1e-4f32 + mp;
        }
    } else if !matches!(mode, 0x18 | 0x19 | 0x1b | 0x3b | 0x3f | 0x40 | 8) {
        mp -= dt_f * 5e-5f32;
    }
    if 0.0 > mp {
        mp = 0.0;
    }
    if mp > 1.0 {
        mp = 1.0;
    }
    wf32(&mut e.0, 0x160, mp);
    // 0x0053e5a0: block power drains while blocking (1200 ms for a shield of sub type 0xd
    // in slot 6, else 600 ms per unit) and refills over 2000 ms otherwise; capped at 1.
    let mut bp = f32_at(&e.0, 0x164);
    if is_blocking(e) {
        if bp > 0.0 {
            let s6 = 0x2f0 + 6 * 0x118;
            let per = if e.0[s6] == 3 && e.0[s6 + 1] == 0xd { 1200.0f32 } else { 600.0f32 };
            bp -= dt_f / per;
            if 0.0 > bp {
                bp = 0.0;
            }
        }
    } else {
        bp = dt_f / 2000.0f32 + bp;
    }
    if bp > 1.0 {
        bp = 1.0;
    }
    wf32(&mut e.0, 0x164, bp);
    // 0x0053e64f: a mode clears flag 0x10; any acceleration clears flag 0x400 and stands a
    // sitting or lying creature up.
    if e.0[0x58] != 0 {
        set_flag(e, 0x10, false);
    }
    let a = [f32_at(&e.0, 0x30), f32_at(&e.0, 0x34), f32_at(&e.0, 0x38)];
    let a_len = a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
    if a_len > 0.0 {
        set_flag(e, 0x400, false);
    }
    if matches!(e.0[0x58], 0x53 | 0x54) && a_len > 0.0 {
        e.0[0x58] = 0;
    }
    // 0x0053e6bd: the consumable modes 0x50/0x51, non-players on a server and the local
    // player in the client's world (consumables.rs).
    crate::consumables::consumable_modes(world, e, id, dt, out);
    // 0x0053e9a3: threat decays 2.5e-4 per ms; an entry on a living creature stays at least
    // 0.01; entries on dead or missing creatures (id != 0) and entries at or below zero go.
    let dec = dt_f * 2.5e-4f32;
    let present: BTreeMap<i64, Option<bool>> = st.threat.keys().chain(st.hits_landed.keys()).map(|k| (*k, entities.get(k).map(|c| 0.0 < f32_at(&c.0, 0x15c)))).collect();
    let e = entities.get_mut(&id).expect("creature");
    st.threat.retain(|k, v| {
        *v -= dec;
        // An entry whose creature is missing is kept as it is.
        let Some(living) = present[k] else { return true };
        if living && 0.01f32 > *v {
            *v = 0.01f32;
        }
        let gone = 0.0 >= *v || (*k != 0 && !living);
        !gone
    });
    // 0x0053eb52: the hit map eases to zero (0.001 per ms) and forgets the dead.
    st.hits_landed.retain(|k, v| {
        for _ in 0..dt {
            *v = (0.0f32 - *v) * 0.001f32 + *v;
        }
        present[k] == Some(true)
    });
    // 0x0053ecc2: a monster with no threat on it and HP left heals to full; not in the
    // client's world (0x0053ecc8, `world+0xb4`).
    if !world.is_client && e.0[0x50] == 1 && f32_at(&e.0, 0x15c) > 0.0 && st.threat.is_empty() {
        let max = max_hp(e);
        wf32(&mut e.0, 0x15c, max);
    }
    true
}

