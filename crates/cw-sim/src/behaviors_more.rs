//! The remaining `cube::Behavior` subclasses of Server.exe and the AI trees the zone generators
//! hang on a spawn (`spawn+0x109c`): `SpawnLocationBehavior` (vtable 0x0055b250, update
//! 0x00428940), `WalkPathBehavior` (0x005716a0, update 0x004c5e00), `RandomInteractionBehavior`
//! (0x0055aff0, update 0x0041bc20) and `CompanionBehavior` (0x00558850, update 0x00405780).
//!
//! Every behaviour vtable has two slots: `+0` update (`__thiscall (creature, world, elapsed,
//! unused)`, returns a bool) and `+4` clone. There are nine behaviour classes in the RTTI
//! (`Behavior`, `CombatBehavior`, `CompanionBehavior`, `LookAtPlayerBehavior`,
//! `RandomInteractionBehavior`, `RandomWalkBehavior`, `SequentialBehavior`,
//! `SpawnLocationBehavior`, `WalkPathBehavior`); there are no Pilot, Guard or Villager classes:
//! those "AI kinds" are trees of the classes above that the generators build (see
//! [`SpawnAi`] and [`spawn_ai_behaviors`]).
//!
//! The creature fields these behaviours use outside the entity block: the path state
//! (`creature+0x1408..0x1464`, [`path::PathState`], owned by the path module), the day schedule
//! (`+0x1484` applied entry, `+0x1488` current entry, `+0x148c` the vector copied from
//! `spawn+0x10a0`; kept in [`SpawnLocationBehavior`], its only reader), the interaction target
//! (`+0x1478` zone x, `+0x147c` zone y, `+0x1480` static index; kept in
//! [`RandomInteractionBehavior`]) and the queued Interact records (`+0x130c`, a list the tick's
//! interaction switch consumes; also kept in [`RandomInteractionBehavior::queued`] until the
//! integrator moves it onto the creature).
//!
//! Every `rand()` draws from the tick thread's stream, [`World::rng`].

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments, clippy::excessive_precision)]

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::Interact;
use cw_world::World;
use cw_world::settlement::ScheduleEntry;
use cw_world::zone::{SpawnAi, Static};

use crate::behavior::Behavior;
use crate::combat::CreatureState;

// Temporary: the path module is written concurrently; see `path_stub` at the bottom.
use crate::path;
use crate::util::{add3, block_fix, blocks3, cell_statics, f32_at, fix3, pos_at, set_accel, solid, sub3, to_block3, u16_at, w16, w32, w64, wf32};

// ---------------------------------------------------------------------------------------------
// Small helpers (duplicated from physics.rs / behavior.rs, which keep them private).

fn scale_at(b: &[u8]) -> [f32; 3] {
    [f32_at(b, 0x70), f32_at(b, 0x74), f32_at(b, 0x78)]
}

/// The zone test before a path search (`getZone` 0x00406290 on `(v / 65536) / 256` per axis,
/// both truncating).
fn zone_loaded_at(world: &World, p: [i64; 3]) -> bool {
    let zx = ((p[0] / 65536) as i32) / 256;
    let zy = ((p[1] / 65536) as i32) / 256;
    world.zone(zx, zy).is_some()
}

/// `Server.exe 0x004056c0`: the fixed dot product `sum(a_i * b_i / 65536)` (64-bit wrapping
/// multiply, truncating divide).
fn fix_dot(a: [i64; 3], b: [i64; 3]) -> i64 {
    let mut r = b[0].wrapping_mul(a[0]) / 65536;
    r = r.wrapping_add(a[1].wrapping_mul(b[1]) / 65536);
    r.wrapping_add(a[2].wrapping_mul(b[2]) / 65536)
}

/// `Server.exe 0x004d4f90` with its last argument false (the only way these behaviours call
/// it): whether a box of `size` blocks centred on `pos` touches a solid block. The block range
/// per axis is `(pos -/+ fix(size * 0.5)) / 65536`, truncated (not floored); only the blocks on
/// the shell of that range are tested, x outermost. The inlined block lookup is `getBlock`.
/// (With the flag set it also tests the statics of the 3x3 static cells around; not needed
/// here.)
fn box_collides(world: &World, pos: [i64; 3], size: [f32; 3]) -> bool {
    let h = fix3([size[0] * 0.5f32, size[1] * 0.5f32, size[2] * 0.5f32]);
    let lo: [i32; 3] = [0, 1, 2].map(|i| (pos[i].wrapping_sub(h[i]) / 65536) as i32);
    let hi: [i32; 3] = [0, 1, 2].map(|i| (pos[i].wrapping_add(h[i]) / 65536) as i32);
    // Performance note: the original walks the whole cube and skips the interior per block.
    let mut x = lo[0];
    while x <= hi[0] {
        let mut y = lo[1];
        while y <= hi[1] {
            let mut z = lo[2];
            while z <= hi[2] {
                let shell = x <= lo[0] || x >= hi[0] || y <= lo[1] || y >= hi[1] || z <= lo[2] || z >= hi[2];
                if shell && solid(world.block(x, y, z)) {
                    return true;
                }
                z += 1;
            }
            y += 1;
        }
        x += 1;
    }
    false
}

/// The static a `(zone x, zone y, index)` key names, when its zone is loaded and the index is
/// in range (the inlined `getZone` and `index < statics.size()` checks).
fn static_at(world: &World, key: [i32; 3]) -> Option<&Static> {
    let zone = world.zone(key[0], key[1])?;
    if key[2] < 0 {
        return None;
    }
    zone.statics.get(key[2] as usize)
}

/// A static no creature is using (`static+0x40` i64 user id zero).
fn static_free(s: &Static) -> bool {
    s.f40 | s.f44 == 0
}

/// `Server.exe 0x00405330` on a creature: the path state cleared (the path module's part) and
/// the ignored-statics set at `creature+0x1468` emptied (`CreatureState::clear_path`).
fn clear_all(path: &mut path::PathState, states: &mut BTreeMap<i64, CreatureState>, id: i64) {
    path::clear_path(path, states.entry(id).or_default());
}

/// The search continuation every path-using behaviour inlines (0x00428d4b, 0x004c6307,
/// 0x0041c638, 0x00405e0f): while the search is running (`creature+0x1410`), up to ten times,
/// stop when the waypoint list holds more than 50 entries or its back is within the goal
/// radius of the goal's block (squared block distance in wrapping i32, converted to float,
/// compared with `radius²`), else run one search step (0x004dde90) and rebuild the waypoints
/// (0x004dafe0).
fn continue_search(world: &World, e: &EntityData, st: &mut CreatureState, path: &mut path::PathState) {
    if !path.has_path() {
        return;
    }
    for _ in 0..10 {
        let n = path.waypoints.len() as i32;
        if n > 0x32 {
            break;
        }
        if n != 0 {
            let b = *path.waypoints.back().expect("non-empty");
            let g = to_block3(path.goal);
            let dz = b[2].wrapping_sub(g[2]);
            let dy = b[1].wrapping_sub(g[1]);
            let dx = b[0].wrapping_sub(g[0]);
            let s = dz.wrapping_mul(dz).wrapping_add(dy.wrapping_mul(dy)).wrapping_add(dx.wrapping_mul(dx));
            let r = path.radius;
            if r * r > s as f32 {
                break;
            }
        }
        path::search_step(world, e, st, path);
        path::build_waypoints(path);
    }
}

// ---------------------------------------------------------------------------------------------
// SpawnLocationBehavior

/// `SpawnLocationBehavior` (4 bytes, no fields of its own; constructor 0x00428920, clone
/// 0x00428ec0). The creature state it drives is kept here: the day schedule copied from
/// `spawn+0x10a0` (`creature+0x148c`, 0x20-byte entries: position and start time in ms of the
/// day), the entry the creature has reached (`+0x1484`) and the entry it is heading for
/// (`+0x1488`).
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnLocationBehavior {
    /// `creature+0x148c`
    pub schedule: Vec<ScheduleEntry>,
    /// `creature+0x1484`: the entry whose position became the spawn point.
    pub applied: i32,
    /// `creature+0x1488`: the entry being walked to.
    pub current: i32,
}

impl SpawnLocationBehavior {
    /// The creature's schedule as `World::tick` sets it up when it makes the creature
    /// (0x00535f00..0x00535fc2): the vector copied, the applied entry the last one whose time
    /// is at or before the time of day (`World+0x80015c`), and the current entry the same.
    /// The applied entry starts from the Creature constructor's value, assumed 0.
    pub fn new(schedule: Vec<ScheduleEntry>, time_of_day: i32) -> Self {
        let mut applied = 0;
        for (i, s) in schedule.iter().enumerate() {
            if time_of_day >= s.time {
                applied = i as i32;
            }
        }
        SpawnLocationBehavior { schedule, applied, current: applied }
    }
}

/// `SpawnLocationBehavior::update` 0x00428940: a villager's day schedule. With no schedule the
/// next behaviour runs. When the time of day passes the start of a different entry, the path
/// is cleared, the goal becomes that entry's position raised out of solid blocks (at most 10
/// blocks, else false) and lowered onto the ground (at most 10 blocks, else false), and a path
/// search starts from the creature's feet plus half a block with a goal radius of 4 blocks;
/// true. While the entry is not reached yet: when no player (hostile type 0) is within 256
/// blocks the creature does nothing (true); otherwise the search continues, and once the
/// waypoint list is empty (or holds fewer than three entries ending at the goal block) the
/// entry becomes the creature's spawn point (`entity+0x1b0`, the centre of `RandomWalk`) and
/// the acceleration stops; true. Otherwise false.
pub fn spawn_location_update(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut path::PathState, id: i64, elapsed: i32, this: &mut SpawnLocationBehavior, out: &mut ServerUpdate) -> bool {
    let _ = (elapsed, out);
    // 0x00428955: an empty schedule.
    if this.schedule.is_empty() {
        return false;
    }
    // 0x00428994: the last entry whose time has come (`cmovge` on `time_of_day >= time`).
    let tod = world.time_of_day;
    let mut best = 0i32;
    for (i, s) in this.schedule.iter().enumerate() {
        if tod >= s.time {
            best = i as i32;
        }
    }
    if best != this.current {
        // 0x004289c6: head for the new entry.
        clear_all(path, states, id);
        this.current = best;
        let e = entities.get(&id).expect("creature");
        let h = f32_at(&e.0, 0x78);
        let off = fix3([0.0, 0.0, 0.5f32 - h * 0.5f32]);
        path.start = add3(pos_at(&e.0), off);
        path.goal = this.schedule[best as usize].pos;
        // 0x00428ad0: up out of solid blocks.
        let mut n = 0;
        while solid(block_fix(world, path.goal)) {
            path.goal[2] = path.goal[2].wrapping_add(0x10000);
            n += 1;
            if n > 10 {
                return false;
            }
        }
        // 0x00428b40: down onto the first solid block.
        let mut n = 0;
        loop {
            let below = [path.goal[0], path.goal[1], path.goal[2].wrapping_sub(0x10000)];
            if solid(block_fix(world, below)) {
                // 0x00428ba1
                let st = states.entry(id).or_default();
                path::start_search(world, e, st, path);
                path.radius = 4.0;
                path::search_step(world, e, st, path);
                path::build_waypoints(path);
                set_accel(entities.get_mut(&id).expect("creature"), [0.0; 3]);
                return true;
            }
            path.goal[2] = path.goal[2].wrapping_sub(0x10000);
            n += 1;
            if n > 10 {
                return false;
            }
        }
    }
    if this.applied == this.current {
        return false;
    }
    // 0x00428bf9: the nearest player (hostile type 0) in the creature map; none within 256
    // blocks (65536 squared) freezes the walk.
    let me = pos_at(&entities[&id].0);
    let mut min = 40000.0f32;
    for o in entities.values() {
        if o.0[0x50] != 0 {
            continue;
        }
        let d = blocks3(sub3(pos_at(&o.0), me));
        let dsq = d[1] * d[1] + d[0] * d[0] + d[2] * d[2];
        if min > dsq {
            min = dsq;
        }
    }
    if !(65536.0f32 > min) {
        return true;
    }
    // 0x00428d4b
    {
        let e = entities.get(&id).expect("creature");
        let st = states.entry(id).or_default();
        continue_search(world, e, st, path);
    }
    // 0x00428def: arrived when the waypoint list is empty, or short and ending at the goal.
    let n = path.waypoints.len();
    if n != 0 {
        let back = *path.waypoints.back().expect("non-empty");
        if back != to_block3(path.goal) || n >= 3 {
            return true;
        }
    }
    // 0x00428e39
    clear_all(path, states, id);
    let e = entities.get_mut(&id).expect("creature");
    set_accel(e, [0.0; 3]);
    this.applied = this.current;
    let p = this.schedule[this.current as usize].pos;
    for (i, v) in p.iter().enumerate() {
        w64(&mut e.0, 0x1b0 + i * 8, *v);
    }
    true
}

// ---------------------------------------------------------------------------------------------
// WalkPathBehavior

/// `WalkPathBehavior` (0x1c bytes; constructor 0x004c5d50 `(float radius)`, copy constructor
/// 0x004c5d10, clone 0x004c63e0). The generators push the waypoints after construction
/// (`FUN_004e1420`, `vector<vec3<i64>>::push_back`): the creature's own position, the group
/// leader's position plus up to three neighbouring points (`populateZoneCreatures`), a dungeon
/// room's corners, or a house floor spot for group guards (see [`SpawnAi`]).
#[derive(Debug, Clone, PartialEq)]
pub struct WalkPathBehavior {
    /// `+4..+0xc`: the waypoints, 16.16 fixed.
    pub path: Vec<[i64; 3]>,
    /// `+0x10`: the waypoint being walked to.
    pub index: i32,
    /// `+0x14`: the milliseconds to wait at a reached waypoint (the constructor leaves it
    /// unset; the clone the creature gets zeroes it).
    pub timer: i32,
    /// `+0x18`: the arrival radius in blocks (2.0 from every generator).
    pub radius: f32,
}

impl WalkPathBehavior {
    /// The object a creature gets: the spawn's object cloned (0x004c63e0 through the copy
    /// constructor 0x004c5d10), index 0, timer 0.
    pub fn new(radius: f32, path: Vec<[i64; 3]>) -> Self {
        WalkPathBehavior { path, index: 0, timer: 0, radius }
    }
}

/// `WalkPathBehavior::update` 0x004c5e00: a patrol. With no waypoints the next behaviour runs.
/// While the wait timer runs the creature idles (true). Then the acceleration stops and the
/// current waypoint is settled: lowered until the creature's box stands in solid blocks (at
/// most 20 blocks) and raised until it is free again (at most 20 blocks); failing either, the
/// next waypoint is chosen and false. Within `radius` of it the creature waits
/// `rand() % 4000 + 2000` ms, the next waypoint is chosen and false. Otherwise a path search
/// starts when there are no waypoints (false when the goal's zone is not loaded), the search
/// continues, flag 0x40 clears and true.
pub fn walk_path_update(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut path::PathState, id: i64, elapsed: i32, this: &mut WalkPathBehavior, out: &mut ServerUpdate) -> bool {
    let _ = out;
    // 0x004c5e26
    if this.path.is_empty() {
        return false;
    }
    // 0x004c5e3f: the wait timer (`cmovs` on the difference).
    let t = this.timer.wrapping_sub(elapsed);
    this.timer = if t < 0 { 0 } else { t };
    if this.timer != 0 {
        return true;
    }
    // 0x004c5e6b: the index, unsigned modulo the count.
    if this.index < 0 {
        this.index = 0;
    }
    let n = this.path.len() as u32;
    this.index = ((this.index as u32) % n) as i32;
    let advance = |this: &mut WalkPathBehavior| {
        // 0x004c61c5
        this.index = this.index.wrapping_add(1);
        this.index = ((this.index as u32) % n) as i32;
    };
    let (pos, scale) = {
        let e = entities.get_mut(&id).expect("creature");
        set_accel(e, [0.0; 3]);
        (pos_at(&e.0), scale_at(&e.0))
    };
    let mut wp = this.path[this.index as usize];
    // 0x004c5f00: down until the box touches solid blocks.
    let mut c = 0;
    loop {
        let off = fix3([0.0, 0.0, scale[2] * 0.5f32]);
        if box_collides(world, add3(wp, off), scale) {
            break;
        }
        wp[2] = wp[2].wrapping_sub(0x10000);
        c += 1;
        if c > 0x14 {
            advance(this);
            return false;
        }
    }
    // 0x004c6010: up until it is free.
    let mut c = 0;
    loop {
        let off = fix3([0.0, 0.0, scale[2] * 0.5f32]);
        if !box_collides(world, add3(wp, off), scale) {
            break;
        }
        wp[2] = wp[2].wrapping_add(0x10000);
        c += 1;
        if c > 0x14 {
            advance(this);
            return false;
        }
    }
    // 0x004c6100: the distance from the creature's feet to the waypoint.
    let off = fix3([0.0, 0.0, scale[2] * 0.5f32]);
    let d = blocks3(sub3(sub3(pos, off), wp));
    let dsq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let r = this.radius;
    if r * r > dsq {
        // 0x004c6192: reached; wait, then the next waypoint.
        clear_all(path, states, id);
        set_accel(entities.get_mut(&id).expect("creature"), [0.0; 3]);
        this.timer = world.rng.rand() % 4000 + 2000;
        advance(this);
        return false;
    }
    let e = entities.get(&id).expect("creature");
    if path.waypoints.is_empty() {
        // 0x004c620b: a new search from the feet plus 0.1 block.
        clear_all(path, states, id);
        let off = fix3([0.0, 0.0, 0.1f32 - scale[2] * 0.5f32]);
        path.start = add3(pos, off);
        path.goal = wp;
        if !zone_loaded_at(world, path.goal) {
            return false;
        }
        let st = states.entry(id).or_default();
        path::start_search(world, e, st, path);
        path.radius = this.radius;
        path::search_step(world, e, st, path);
        path::build_waypoints(path);
        // 0x004c62f2
        set_accel(entities.get_mut(&id).expect("creature"), [0.0; 3]);
    }
    // 0x004c6307
    let e = entities.get(&id).expect("creature");
    let st = states.entry(id).or_default();
    continue_search(world, e, st, path);
    // 0x004c63af
    let e = entities.get_mut(&id).expect("creature");
    let f = u16_at(&e.0, 0x114) & 0xffbf;
    w16(&mut e.0, 0x114, f);
    true
}

// ---------------------------------------------------------------------------------------------
// RandomInteractionBehavior

/// A house's interaction statics (`House+0x48` for the day, `+0x54` for the night: vectors of
/// `(zone x, zone y, static index)`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HouseSpots {
    pub day: Vec<[i32; 3]>,
    pub night: Vec<[i32; 3]>,
}

/// `Server.exe 0x004d4c20`: the house of the block's zone (`zone+0x88`, `vector<House*>`) whose
/// room cells contain the block ([`World::house_at`]), with its seat (`+0x48`) and bed
/// (`+0x54`) statics.
fn house_at(world: &World, bx: i32, by: i32, bz: i32) -> Option<HouseSpots> {
    world.house_at(bx, by, bz).map(|h| HouseSpots { day: h.seats.clone(), night: h.beds.clone() })
}

/// `RandomInteractionBehavior` (8 bytes; constructor 0x0041ba60, clone 0x0041cac0) with the
/// creature fields only it uses.
#[derive(Debug, Clone, PartialEq)]
pub struct RandomInteractionBehavior {
    /// `+4`: milliseconds until the creature may pick a new static (0 from the clone).
    pub timer: i32,
    /// `creature+0x1478..0x1480`: the static being walked to, `(zone x, zone y, index)`;
    /// `(-1, -1, 0)` when none.
    pub target: [i32; 3],
    /// Interact records for `creature+0x130c` (the tick's per-creature interaction queue),
    /// type 3 (use a static) on `target`.
    pub queued: Vec<Interact>,
}

impl Default for RandomInteractionBehavior {
    fn default() -> Self {
        RandomInteractionBehavior { timer: 0, target: [-1, -1, 0], queued: Vec::new() }
    }
}

/// The Interact record 0x0041b9e0 builds: item level (`+0x10` u16) 1, the static key at
/// `+0x118` all -1, `+0x124` 0, `+0x12a` u16 1, the rest zero.
fn interact_record(target: [i32; 3], kind: u8) -> Interact {
    let mut it = Interact::default();
    w16(&mut it.0, 0x10, 1);
    w32(&mut it.0, 0x118, target[0] as u32);
    w32(&mut it.0, 0x11c, target[1] as u32);
    w32(&mut it.0, 0x120, target[2] as u32);
    w32(&mut it.0, 0x124, 0);
    it.0[0x128] = kind;
    w16(&mut it.0, 0x12a, 1);
    it
}

/// `RandomInteractionBehavior::update` 0x0041bc20: a villager uses beds, chairs and the like.
///
/// The timer counts down; when it runs out a creature sitting or lying (mode 0x53 / 0x54)
/// stands up (mode 0, path and target cleared, acceleration `(0, 0, 1)`, timer 20000 ms) and
/// the next behaviour runs; any other mode is dropped. While sitting or lying: true.
///
/// Inside a house (`0x004d4c20`, not modelled: see [`house_at`]) the target stays when it is
/// one of the house's statics and still free; otherwise, once the timer is 0, a random static
/// of the house's day list (06:00..23:00) or night list becomes the target when free (timer
/// 20000 ms, goal the static's position, goal radius 2, a path search). Outside a house the
/// target stays when it is a free static of the 2x2 static cells from `(cx - 1, cy - 1)` (a
/// quirk: the cell ranges stop before `c + 1`); otherwise, once the timer is 0, a random
/// static of kind 0x10, 0x12 or 0x44 in those cells within sight (0x004d4d80, 200 blocks)
/// becomes the target when free, with its goal raised out of solid blocks and lowered onto
/// the ground. Without a target the path and target clear and the next behaviour runs.
///
/// With a target the search continues, and within 3 blocks of the static an Interact record of
/// type 3 on it is queued, the path and target clear, acceleration and velocity stop and the
/// timer restarts at 20000 ms. True.
pub fn random_interaction_update(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut path::PathState, id: i64, elapsed: i32, this: &mut RandomInteractionBehavior, out: &mut ServerUpdate) -> bool {
    let _ = out;
    // 0x0041bc62: the timer (`cmovs` on the difference).
    let t = this.timer.wrapping_sub(elapsed);
    this.timer = if t < 0 { 0 } else { t };
    {
        let e = entities.get_mut(&id).expect("creature");
        if this.timer == 0 {
            let m = e.0[0x58];
            if m == 0x53 || m == 0x54 {
                // 0x0041bdaf: stand up.
                path::clear_path(path, states.entry(id).or_default());
                states.entry(id).or_default().clear_path();
                this.target = [-1, -1, 0];
        path.target_static = (-1, -1, 0);
                path.target_static = (-1, -1, 0);
                e.0[0x58] = 0;
                set_accel(e, [0.0, 0.0, 1.0]);
                this.timer = 20000;
                return false;
            }
            e.0[0x58] = 0;
        }
        // 0x0041bc96
        if matches!(e.0[0x58], 0x53 | 0x54) {
            return true;
        }
    }
    let (pos, scale) = {
        let e = &entities[&id];
        (pos_at(&e.0), scale_at(&e.0))
    };
    let fail = |path: &mut path::PathState, states: &mut BTreeMap<i64, CreatureState>, this: &mut RandomInteractionBehavior| {
        // 0x0041c5f8
        clear_all(path, states, id);
        this.target = [-1, -1, 0];
        path.target_static = (-1, -1, 0);
        false
    };
    let start_off = fix3([0.0, 0.0, 0.5f32 - scale[2] * 0.5f32]);
    // 0x0041bca9: the house containing the creature's block (truncating divisions).
    let house = house_at(world, (pos[0] / 65536) as i32, (pos[1] / 65536) as i32, (pos[2] / 65536) as i32);
    if let Some(h) = house {
        // 0x0041bd10: is the target one of the house's statics and still free?
        let mut found = false;
        for s in h.day.iter().chain(h.night.iter()) {
            if *s == this.target && static_at(world, *s).is_some_and(static_free) {
                found = true;
            }
        }
        if !found {
            // 0x0041bed8
            if this.timer != 0 {
                return fail(path, states, this);
            }
            let tod = world.time_of_day;
            let list = if tod >= 0x1499700 && tod < 0x4ef6d80 { &h.day } else { &h.night };
            if list.is_empty() {
                return fail(path, states, this);
            }
            let k = (world.rng.rand() as u32 % list.len() as u32) as usize;
            let key = list[k];
            let Some(s) = static_at(world, key) else { return fail(path, states, this) };
            if !static_free(s) {
                return fail(path, states, this);
            }
            // 0x0041bfb6
            this.timer = 20000;
            path.start = add3(pos, start_off);
            path.goal = [s.x, s.y, s.z];
            this.target = key;
            path.target_static = (key[0], key[1], key[2]);
            path.radius = 2.0;
            let e = entities.get(&id).expect("creature");
            path::start_search(world, e, states.entry(id).or_default(), path);
        }
    } else {
        // 0x0041c069: the static cells (8x8 blocks) around the creature.
        let cx = ((pos[0] / 65536) as i32) / 8;
        let cy = ((pos[1] / 65536) as i32) / 8;
        let mut found = false;
        for x in cx - 1..cx + 1 {
            for y in cy - 1..cy + 1 {
                if !(0..0x200000).contains(&x) || !(0..0x200000).contains(&y) {
                    continue;
                }
                let (zx, zy) = (x / 32, y / 32);
                for (i, s) in cell_statics(world, x, y) {
                    if [zx, zy, i as i32] == this.target && static_free(&s) {
                        found = true;
                    }
                }
            }
        }
        if !found {
            // 0x0041c228
            if this.timer != 0 {
                return fail(path, states, this);
            }
            let mut cands: Vec<[i32; 3]> = Vec::new();
            for x in cx - 1..cx + 1 {
                for y in cy - 1..cy + 1 {
                    if !(0..0x200000).contains(&x) || !(0..0x200000).contains(&y) {
                        continue;
                    }
                    let (zx, zy) = (x / 32, y / 32);
                    for (i, s) in cell_statics(world, x, y) {
                        if matches!(s.kind, 0x12 | 0x10 | 0x44) && path::line_of_sight(world, pos, [s.x, s.y, s.z], true, 200.0) {
                            cands.push([zx, zy, i as i32]);
                        }
                    }
                }
            }
            if !cands.is_empty() {
                // 0x0041c3b1
                let k = (world.rng.rand() as u32 % cands.len() as u32) as usize;
                let key = cands[k];
                if let Some(s) = static_at(world, key)
                    && static_free(s)
                {
                    this.timer = 20000;
                    found = true;
                    path.start = add3(pos, start_off);
                    path.goal = [s.x, s.y, s.z];
                    // 0x0041c4e0: up out of solid blocks, then down onto the ground (unbounded).
                    while solid(block_fix(world, path.goal)) {
                        path.goal[2] = path.goal[2].wrapping_add(0x10000);
                    }
                    loop {
                        let below = [path.goal[0], path.goal[1], path.goal[2].wrapping_sub(0x10000)];
                        if solid(block_fix(world, below)) {
                            break;
                        }
                        path.goal[2] = path.goal[2].wrapping_sub(0x10000);
                    }
                    this.target = key;
                    path.target_static = (key[0], key[1], key[2]);
                    path.radius = 2.0;
                    let e = entities.get(&id).expect("creature");
                    path::start_search(world, e, states.entry(id).or_default(), path);
                }
            }
            if !found {
                return fail(path, states, this);
            }
        }
    }
    // 0x0041c638
    {
        let e = entities.get(&id).expect("creature");
        continue_search(world, e, states.entry(id).or_default(), path);
    }
    // 0x0041c6eb: within 3 blocks of the target static, use it.
    let t = this.target;
    if !(0..0x10000).contains(&t[0]) || !(0..0x10000).contains(&t[1]) {
        return true;
    }
    let Some(s) = static_at(world, t) else { return true };
    let d = blocks3(sub3([s.x, s.y, s.z], pos));
    let dsq = d[1] * d[1] + d[0] * d[0] + d[2] * d[2];
    if 9.0f32 > dsq {
        // 0x0041c8d5
        this.queued.push(interact_record(t, 3));
        clear_all(path, states, id);
        this.target = [-1, -1, 0];
        path.target_static = (-1, -1, 0);
        let e = entities.get_mut(&id).expect("creature");
        set_accel(e, [0.0; 3]);
        for i in 0..3 {
            wf32(&mut e.0, 0x24 + i * 4, 0.0);
        }
        this.timer = 20000;
    }
    true
}

// ---------------------------------------------------------------------------------------------
// CompanionBehavior

/// `CompanionBehavior` (0x10 bytes; constructors 0x004055d0 and 0x004055f0 `(i64 leader)`,
/// clone 0x00405ef0): `+8` the id of the creature to follow. The generators store the group
/// leader's spawn id (`spawn+0x48`); for a pet made in the tick (0x0053b8c0) and a summon
/// (0x00522580) it is the owner's id, which also goes to the pet's parent id `entity+0x188`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompanionBehavior {
    /// `+8`
    pub leader: i64,
}

/// `CompanionBehavior::update` 0x00405780: follow the leader. Without the leader the next
/// behaviour runs. Flag 0x40 is set while the leader is more than 8 blocks away (fixed dot
/// product > 0x400000). A pet (hostile type 5) regenerates a tenth of its max HP per second,
/// capped. The acceleration stops; within 4 blocks the next behaviour runs. When the way is
/// walkable (0x0052ef00 with a reach of both widths) the path clears and the creature
/// accelerates at 5x the offset in blocks, capped at 30; true. Otherwise the goal is the
/// leader's position lowered onto the ground and raised out of it (at most 20 blocks each,
/// else false); within 4 blocks of the feet the path clears and the next behaviour runs; else
/// a path search starts when there are no waypoints (false when the zone is not loaded; goal
/// radius 2), the search continues and true.
pub fn companion_update(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut path::PathState, id: i64, elapsed: i32, this: &mut CompanionBehavior, out: &mut ServerUpdate) -> bool {
    let _ = out;
    // 0x004057b9: the leader in the world's creature map.
    let Some(leader) = entities.get(&this.leader).cloned() else { return false };
    let lp = pos_at(&leader.0);
    let (me, scale) = {
        let e = entities.get_mut(&id).expect("creature");
        let me = pos_at(&e.0);
        // 0x0040585a: flag 0x40 while more than 8 blocks away.
        let d = sub3(lp, me);
        let dd = fix_dot(d, d);
        let f = u16_at(&e.0, 0x114);
        w16(&mut e.0, 0x114, if dd > 0x400000 { f | 0x40 } else { f & 0xffbf });
        // 0x00405895: a pet's regeneration.
        if e.0[0x50] == 5 {
            let m = crate::stats::max_hp(e);
            let x = elapsed as f32 * 0.001f32 * 0.1f32;
            let hp = m * x + f32_at(&e.0, 0x15c);
            wf32(&mut e.0, 0x15c, hp);
            if hp > crate::stats::max_hp(e) {
                let m = crate::stats::max_hp(e);
                wf32(&mut e.0, 0x15c, m);
            }
        }
        set_accel(e, [0.0; 3]);
        (me, scale_at(&e.0))
    };
    // 0x00405903
    let d = blocks3(sub3(lp, me));
    let dsq = d[1] * d[1] + d[0] * d[0] + d[2] * d[2];
    if 16.0f32 > dsq {
        return false;
    }
    // 0x00405a07: straight to the leader when the way is walkable.
    let reach = scale[0] + f32_at(&leader.0, 0x70);
    let direct = {
        let e = entities.get(&id).expect("creature");
        path::walk_direct(world, e, states.entry(id).or_default(), me, lp, reach)
    };
    if direct {
        clear_all(path, states, id);
        let mut a = [d[0] * 5.0f32, d[1] * 5.0f32, 0.0f32 * 5.0f32];
        let l2 = a[1] * a[1] + a[0] * a[0] + a[2] * a[2];
        if l2 > 900.0f32 {
            let s = 1.0f32 / (f64::from(l2).sqrt() as f32);
            a = [a[0] * s * 30.0f32, a[1] * s * 30.0f32, a[2] * s * 30.0f32];
        }
        set_accel(entities.get_mut(&id).expect("creature"), a);
        return true;
    }
    // 0x00405b13: the leader's position settled on the ground.
    let mut g = lp;
    let mut c = 0;
    while !solid(block_fix(world, g)) {
        g[2] = g[2].wrapping_sub(0x10000);
        c += 1;
        if c > 0x14 {
            return false;
        }
    }
    let mut c = 0;
    while solid(block_fix(world, g)) {
        g[2] = g[2].wrapping_add(0x10000);
        c += 1;
        if c > 0x14 {
            return false;
        }
    }
    // 0x00405c15
    let off = fix3([0.0, 0.0, scale[2] * 0.5f32]);
    let d = blocks3(sub3(sub3(me, off), g));
    let dsq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if 16.0f32 > dsq {
        clear_all(path, states, id);
        set_accel(entities.get_mut(&id).expect("creature"), [0.0; 3]);
        return false;
    }
    if path.waypoints.is_empty() {
        // 0x00405cee
        clear_all(path, states, id);
        let off = fix3([0.0, 0.0, 0.1f32 - scale[2] * 0.5f32]);
        path.start = add3(me, off);
        path.goal = g;
        if !zone_loaded_at(world, path.goal) {
            return false;
        }
        {
            let e = entities.get(&id).expect("creature");
            let st = states.entry(id).or_default();
            path::start_search(world, e, st, path);
            path.radius = 2.0;
            path::search_step(world, e, st, path);
            path::build_waypoints(path);
        }
        set_accel(entities.get_mut(&id).expect("creature"), [0.0; 3]);
    }
    // 0x00405e0f
    let e = entities.get(&id).expect("creature");
    continue_search(world, e, states.entry(id).or_default(), path);
    true
}

// ---------------------------------------------------------------------------------------------
// The spawn's AI tree

/// One node of a creature's behaviour tree (`creature+0x13e4`, cloned from `spawn+0x109c`).
#[derive(Debug, Clone)]
pub enum AiNode {
    /// `CombatBehavior`, `LookAtPlayerBehavior`, `RandomWalkBehavior` (behavior.rs).
    Base(Behavior),
    SpawnLocation(SpawnLocationBehavior),
    WalkPath(WalkPathBehavior),
    RandomInteraction(RandomInteractionBehavior),
    Companion(CompanionBehavior),
    /// `SequentialBehavior` (constructor 0x0041cfc0, update 0x0041d080): the children in order
    /// until one returns true; when none does the acceleration is zeroed and false.
    Sequence(Vec<AiNode>),
}

/// Moves the Interact records the tree's `RandomInteractionBehavior`s queued into `out`, in
/// order. In the original they go straight onto the creature's interaction queue
/// (`creature+0x130c`), which the tick's per-creature loop dispatches for every creature at the
/// start of its next iteration (0x005362ac..0x00536306, jump table 0x005489e0).
pub fn take_queued_interactions(node: &mut AiNode, out: &mut Vec<Interact>) {
    match node {
        AiNode::RandomInteraction(b) => out.append(&mut b.queued),
        AiNode::Sequence(list) => {
            for c in list.iter_mut() {
                take_queued_interactions(c, out);
            }
        }
        _ => {}
    }
}

fn combat() -> AiNode {
    AiNode::Base(Behavior::Combat(crate::combat_ai::CombatState::new(SpawnAi::COMBAT_RANGE)))
}

fn look() -> AiNode {
    AiNode::Base(Behavior::LookAtPlayer)
}

fn random_walk() -> AiNode {
    AiNode::Base(Behavior::RandomWalk { timer: 0 })
}

fn walk(radius: f32, path: &[[i64; 3]]) -> AiNode {
    AiNode::WalkPath(WalkPathBehavior::new(radius, path.to_vec()))
}

fn companion(leader: i64) -> AiNode {
    AiNode::Companion(CompanionBehavior { leader })
}

/// The behaviour tree a creature made from a spawn gets (`World::tick` 0x00535fc8..0x00536130):
/// the spawn's own tree (`spawn+0x109c`, [`SpawnAi`], cloned through vtable slot 1) when it has
/// one, else the default list (none for hostile type 6). `e` is the creature's entity block;
/// `time_of_day` sets up a villager's schedule.
pub fn spawn_ai_behaviors(ai: Option<&SpawnAi>, e: &EntityData, time_of_day: i32) -> Option<AiNode> {
    let seq = AiNode::Sequence;
    let Some(ai) = ai else {
        if e.0[0x50] == 6 {
            return None;
        }
        return Some(seq(crate::behavior::default_behaviors(e).into_iter().map(AiNode::Base).collect()));
    };
    let r = ai.walk_radius().unwrap_or(2.0);
    Some(match ai {
        SpawnAi::Pilot => seq(vec![look()]),
        SpawnAi::Sentry { path } => seq(vec![combat(), look(), walk(r, path)]),
        SpawnAi::Villager { schedule } => seq(vec![
            combat(),
            look(),
            AiNode::SpawnLocation(SpawnLocationBehavior::new(schedule.clone(), time_of_day)),
            AiNode::RandomInteraction(RandomInteractionBehavior::default()),
            random_walk(),
        ]),
        SpawnAi::Combat => combat(),
        SpawnAi::Patrol { path } => seq(vec![combat(), walk(r, path)]),
        SpawnAi::DungeonFollower { leader, path } => seq(vec![combat(), companion(*leader), walk(r, path)]),
        SpawnAi::Camp { path } => seq(vec![combat(), walk(r, path), AiNode::RandomInteraction(RandomInteractionBehavior::default()), random_walk()]),
        SpawnAi::GroupLeader { path } => seq(vec![combat(), walk(r, path), random_walk()]),
        SpawnAi::GroupFollower { leader } => seq(vec![combat(), companion(*leader), random_walk()]),
        SpawnAi::CellNpc => seq(vec![combat(), random_walk()]),
        SpawnAi::Wanderer { path } => seq(vec![combat(), look(), walk(r, path), random_walk()]),
        SpawnAi::WandererFollower { leader, path } => seq(vec![combat(), look(), companion(*leader), walk(r, path), random_walk()]),
    })
}

/// Runs a behaviour tree once (the tick's `creature+0x13e4` update). `base` runs the nodes
/// behavior.rs owns (Combat, LookAtPlayer, RandomWalk), which it keeps private.
pub fn run_ai(
    world: &mut World,
    entities: &mut BTreeMap<i64, EntityData>,
    states: &mut BTreeMap<i64, CreatureState>,
    path: &mut path::PathState,
    id: i64,
    elapsed: i32,
    node: &mut AiNode,
    out: &mut ServerUpdate,
    base: &mut dyn FnMut(&mut World, &mut BTreeMap<i64, EntityData>, &mut BTreeMap<i64, CreatureState>, &mut path::PathState, i64, i32, &mut Behavior, &mut ServerUpdate) -> bool,
) -> bool {
    match node {
        AiNode::Base(b) => base(world, entities, states, path, id, elapsed, b, out),
        AiNode::SpawnLocation(b) => spawn_location_update(world, entities, states, path, id, elapsed, b, out),
        AiNode::WalkPath(b) => walk_path_update(world, entities, states, path, id, elapsed, b, out),
        AiNode::RandomInteraction(b) => random_interaction_update(world, entities, states, path, id, elapsed, b, out),
        AiNode::Companion(b) => companion_update(world, entities, states, path, id, elapsed, b, out),
        AiNode::Sequence(list) => {
            // 0x0041d080
            for c in list.iter_mut() {
                if run_ai(world, entities, states, path, id, elapsed, c, out, base) {
                    return true;
                }
            }
            if let Some(e) = entities.get_mut(&id) {
                set_accel(e, [0.0; 3]);
            }
            false
        }
    }
}
