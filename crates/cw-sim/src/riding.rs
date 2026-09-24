//! Mounts, pet riding and the render fields at the end of a creature's movement in
//! `World::tick` (`Server.exe 0x005322d0`): the pet standing in lava that refills its owner's
//! riding stamina (0x005430e2), the creature mounted on another (`creature+0x11c0`,
//! 0x00544775), and the pet whose owner rides it (owner mode 0x6a) or the render smoothing
//! (`creature+0x1350`/`+0x1374`, 0x00545728..0x00545b14). Transcribed in
//! `analysis/notes/functions/005322d0_pseudo_542b95.md` sections (f), (q) and (x); the port
//! keeps its operation order.
//!
//! What the server can observe: the pet that follows its riding owner (its position, rotation
//! and zeroed velocity go out in its entity updates) and the render position that the combat
//! AI aims charged spells at. On a server `creature+0x11c0` is only ever cleared (by the
//! constructor 0x00406400, `Creature::reset` 0x004110d0 and the dismount here; no writer sets
//! it, since the field is outside the entity block the clients send), and `+0x1398` (the
//! smoothing switch) likewise, so the mounted path and the smoothing branch are ported for
//! completeness but never run. The riding stamina `+0x1198` is read only by the local-player
//! code at 0x0053f1d2 (client only). The walk-cycle accumulators `+0x1188`/`+0x118c`
//! ([`RidingState::anim`], [`RidingState::walk`]) drive animations and the client's footsteps
//! (physics.rs, 0x005447a6); the mounted path and the riding pet copy them here.

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::assign_op_pattern, clippy::needless_range_loop)]

use std::collections::BTreeMap;

use cw_net::EntityData;

use crate::combat::CreatureState;
use crate::util::{f32_at, fix, i64_at, pos_at, set_pos, set_vec3f, vec3f_at};

/// The creature fields of this module (outside the entity block).
#[derive(Debug, Clone)]
pub struct RidingState {
    /// `creature+0x1350` (vec3i64): the render position (the entity position on a server, or
    /// the riding owner's render position lifted by the scale difference).
    pub render_pos: [i64; 3],
    /// `creature+0x1374` (vec3f): the render rotation.
    pub render_rot: [f32; 3],
    /// `creature+0x1198`: the riding stamina the local player spends (1.0 from the constructor
    /// 0x00406400); a pet in lava refills its owner's.
    pub ride_stamina: f32,
    /// `creature+0x1398` (i32): render smoothing on when positive (0 from the constructor and
    /// `Creature::reset`; no server writer sets it).
    pub smoothing: i32,
    /// `creature+0x1188`: the animation phase the walk cycle feeds (capped at 1 and eased to 0
    /// every tick, 0x00544dfb).
    pub anim: f32,
    /// `creature+0x118c`: the walk cycle (radians of stride; a footstep each multiple of pi on
    /// the client, 0x00544b4f).
    pub walk: f32,
}

impl Default for RidingState {
    fn default() -> Self {
        RidingState { render_pos: [0; 3], render_rot: [0.0; 3], ride_stamina: 1.0, smoothing: 0, anim: 0.0, walk: 0.0 }
    }
}

/// What this module reads of another creature (a mount or an owner), taken before the moving
/// creature's own borrow (`World::findEntity` 0x00405420: `None` when the id is not in the
/// creature map).
#[derive(Debug, Clone, Copy)]
pub struct Peer {
    pub pos: [i64; 3],
    pub rot: [f32; 3],
    pub hp: f32,
    pub mode: u8,
    pub scale_z: f32,
    /// `+0x1180`
    pub step_offset: f32,
    /// `+0x1350`, `+0x1374`
    pub render_pos: [i64; 3],
    pub render_rot: [f32; 3],
    /// `+0x1188`, `+0x118c`: the walk cycle.
    pub anim: f32,
    pub walk: f32,
}

/// `World::findEntity(id)` 0x00405420 and the fields of the found creature this module reads.
pub fn peer(entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, id: i64) -> Option<Peer> {
    let e = entities.get(&id)?;
    let st = states.get(&id);
    Some(Peer {
        pos: pos_at(&e.0),
        rot: vec3f_at(&e.0, 0x18),
        hp: f32_at(&e.0, 0x15c),
        mode: e.0[0x58],
        scale_z: f32_at(&e.0, 0x78),
        step_offset: st.map_or(0.0, |s| s.step_offset),
        render_pos: st.map_or([0; 3], |s| s.riding.render_pos),
        render_rot: st.map_or([0.0; 3], |s| s.riding.render_rot),
        anim: st.map_or(0.0, |s| s.riding.anim),
        walk: st.map_or(0.0, |s| s.riding.walk),
    })
}

/// The mount and the owner of creature `id`, looked up before its movement borrows it: the
/// mount by `creature+0x11c0` (`ModeState::mount`), the owner by the parent id
/// `entity+0x188`.
///
/// Port guard: id 0 (either field unset) finds nothing. The spawn ids of zone (0, 0) start at
/// 0 (`spawn_id(0, 0, 0)`), and with that creature in the map every unmounted creature, the
/// player included, would otherwise ride it and be placed on top of it every tick.
pub fn gather(entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, id: i64) -> (Option<Peer>, Option<Peer>) {
    let mount_id = states.get(&id).map_or(0, |s| s.modes.mount);
    let owner_id = entities.get(&id).map_or(0, |e| i64_at(&e.0, 0x188));
    let find = |k: i64| if k == 0 { None } else { peer(entities, states, k) };
    (find(mount_id), find(owner_id))
}

/// 0x005430e2: a pet (hostile 5) whose box touches lava (`liquid_contact`, block type 3 under
/// the feet) refills its owner's riding stamina by `dt_s * 0.05` up to 1 while it is below 1.
/// Called after the movement's borrows end; the value is read only by client code.
pub fn refill_owner_stamina(entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, dt_s: f32) {
    let Some(e) = entities.get(&id) else { return };
    let owner = i64_at(&e.0, 0x188);
    if !entities.contains_key(&owner) {
        return;
    }
    let o = states.entry(owner).or_default();
    // 0x0054311f: comiss/jbe, NaN skips.
    if 1.0f32 > o.riding.ride_stamina {
        let v = (dt_s * 0.05f32) + o.riding.ride_stamina;
        o.riding.ride_stamina = v;
        if v > 1.0f32 {
            o.riding.ride_stamina = 1.0;
        }
    }
}

/// 0x00544775: a creature mounted on another (`creature+0x11c0`). A dead mount (`!(0 < hp)`,
/// so NaN keeps the rider on) drops the rider (the id is cleared); otherwise every creature but
/// a pet (hostile 5) sits 1.5 blocks above the mount less the mount's step offset, faces the
/// mount's rotation tipped 20 degrees forward, and has its velocity and acceleration cleared
/// (the original then skips the walk-cycle update, physics.rs 0x005447a6). Returns whether the
/// rider was placed. `vel` is the movement's working velocity (`c+0x34`).
pub fn follow_mount(e: &mut EntityData, st: &mut CreatureState, mount: Option<&Peer>, vel: &mut [f32; 3]) -> bool {
    let Some(m) = mount else { return false };
    // 0x00544789: comiss 0, [hp]; jb -> the mounted path.
    if !(0.0f32 < m.hp) {
        st.modes.mount = 0;
        return false;
    }
    // 0x00544897: a pet takes the normal path.
    if e.0[0x50] == 5 {
        return false;
    }
    // 0x005448a7: `vec3i64::fromFloat(vec3f(0, 0, 1.5 - m[+0x1180]))` (0x00402510) added to
    // the mount position (0x00402cb0).
    let dz = 1.5f32 - m.step_offset;
    let off = [fix(0.0), fix(0.0), fix(dz)];
    set_pos(&mut e.0, [m.pos[0].wrapping_add(off[0]), m.pos[1].wrapping_add(off[1]), m.pos[2].wrapping_add(off[2])]);
    *vel = [0.0; 3];
    // 0x00544963: `m.rot + vec3f(20, 0, 0)` (0x004014f0).
    set_vec3f(&mut e.0, 0x18, [m.rot[0] + 20.0f32, m.rot[1] + 0.0f32, m.rot[2] + 0.0f32]);
    set_vec3f(&mut e.0, 0x30, [0.0; 3]);
    // 0x0054499f: c[+0x1188] = m[+0x1188] * 0.5 (0x005586d0), c[+0x118c] = m[+0x118c]; the
    // walk-cycle update is skipped (to 0x00544dc0).
    st.riding.anim = m.anim * 0.5f32;
    st.riding.walk = m.walk;
    true
}

/// 0x00545728..0x00545b14: a pet (hostile 5) whose owner (`entity+0x188`, looked up three
/// times in the original) is in mode 0x6a (riding its pet) is moved under the owner: render
/// position and rotation from the owner's, position lifted by `(scale.z * 0.5 - owner.scale.z
/// * 0.5) + 0.01` above the owner's, the owner's rotation and step offset, velocity and
/// acceleration cleared. Otherwise, with render smoothing on (`+0x1398 > 0`, never on a
/// server) the render position and rotation ease towards the entity's; else they are the
/// entity's. `vel` is the movement's working velocity; `dt` the tick's milliseconds.
pub fn follow_owner(e: &mut EntityData, st: &mut CreatureState, owner: Option<&Peer>, dt: i32, vel: &mut [f32; 3]) {
    if e.0[0x50] == 5
        && let Some(o) = owner
        && o.mode == 0x6a
    {
        let scale_z = f32_at(&e.0, 0x78);
        // 0x00545786: `(c.scale.z * 0.5 - o.scale.z * 0.5) + 0.01`, twice.
        let dz = ((scale_z * 0.5f32) - (o.scale_z * 0.5f32)) + 0.01f32;
        let off = fix(dz);
        st.riding.render_pos = [o.render_pos[0].wrapping_add(fix(0.0)), o.render_pos[1].wrapping_add(fix(0.0)), o.render_pos[2].wrapping_add(off)];
        st.riding.render_rot = o.render_rot;
        let dz2 = ((scale_z * 0.5f32) - (o.scale_z * 0.5f32)) + 0.01f32;
        let off2 = fix(dz2);
        set_pos(&mut e.0, [o.pos[0].wrapping_add(fix(0.0)), o.pos[1].wrapping_add(fix(0.0)), o.pos[2].wrapping_add(off2)]);
        set_vec3f(&mut e.0, 0x18, o.rot);
        st.step_offset = o.step_offset;
        // 0x005458c5: +0x1188 and +0x118c copied.
        st.riding.anim = o.anim;
        st.riding.walk = o.walk;
        *vel = [0.0; 3];
        set_vec3f(&mut e.0, 0x30, [0.0; 3]);
        return;
    }
    if st.riding.smoothing > 0 {
        // 0x0054594d: k = lerpRepeat(0 -> 1, dt, 0.015).
        let mut k = 0.0f32;
        for _ in 0..dt {
            k = (1.0f32 - k) * 0.015f32 + k;
        }
        // 0x00402a10: `(i64)(k * 65536)`.
        let kf = fix(k);
        let pos = pos_at(&e.0);
        // 0x00402c50, 0x00402bd0/0x00402db0 (`comp * kf / 65536`, truncating), 0x00402e30.
        for i in 0..3 {
            let d = pos[i].wrapping_sub(st.riding.render_pos[i]);
            let d = d.wrapping_mul(kf) / 65536;
            st.riding.render_pos[i] = st.riding.render_pos[i].wrapping_add(d);
        }
        // 0x005459c9..0x00545a0f: the z again, `__ftol2((x87) dz * k)` (0x0052ebb0). The
        // extended-precision product is approximated in f64.
        let dz = pos[2].wrapping_sub(st.riding.render_pos[2]);
        st.riding.render_pos[2] = st.riding.render_pos[2].wrapping_add((dz as f64 * f64::from(k)) as i64);
        let rot = vec3f_at(&e.0, 0x18);
        for i in 0..3 {
            st.riding.render_rot[i] = crate::physics::turn_towards(st.riding.render_rot[i], rot[i], k);
        }
        st.step_offset = 0.0;
    } else {
        // 0x00545af6
        st.riding.render_pos = pos_at(&e.0);
        st.riding.render_rot = vec3f_at(&e.0, 0x18);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Id 0 is "no creature" (`creature+0x11c0` and `entity+0x188` are 0 when unset), but the
    /// spawn ids of zone (0, 0) start at 0 (`spawn_id(0, 0, 0)`): with that creature in the map
    /// an unmounted creature must not find it as its mount or owner, or every creature (the
    /// player too) is placed on top of it each tick.
    #[test]
    fn gather_ignores_the_id_zero_creature() {
        let mut entities = BTreeMap::new();
        let mut zero = EntityData::ZERO;
        set_pos(&mut zero.0, [24 << 16, 31 << 16, 244 << 16]);
        wf32_(&mut zero, 0x15c, 100.0);
        entities.insert(0, zero);
        entities.insert(5, EntityData::ZERO);
        let states = BTreeMap::new();
        let (mount, owner) = gather(&entities, &states, 5);
        assert!(mount.is_none());
        assert!(owner.is_none());
    }

    fn wf32_(e: &mut EntityData, o: usize, v: f32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
}
