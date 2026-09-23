//! The creature path finder and path follower of `Server.exe`: the A* over blocks that the
//! behaviours start (`0x004dd2e0` start, `0x004dde90` one expansion, `0x004dafe0` waypoint
//! list), the per-tick follower (`0x004db200`, run by `World::tick` at 0x00537994), and the
//! world probes the AI uses beside it (`lineOfSight` 0x004d4d80 with its block walk `sweep`
//! 0x004d6730, the box test 0x004d4f90).
//!
//! The state lives on `cube::Creature` at +0x1408..+0x1464 ([`PathState`]):
//! - `+0x140c` a `std::map<std::vector<int>, Node>` keyed by the block `{x, y, z}` (ordered by
//!   `std::lexicographical_compare` of signed ints, i.e. the order of `[i32; 3]`), `+0x1410`
//!   its size (the "has path" test of the callers is `size != 0`), `+0x1408` an iterator to
//!   the node with the smallest heuristic (`end()` when none);
//! - `+0x1414` a `std::set<std::vector<int>>`, the open list, `+0x1418` its size; the closed
//!   nodes are the map entries missing from it;
//! - `+0x141c` the block the search is rooted at (`{-1,-1,-1}` from the constructor),
//!   `+0x1428` the start position, `+0x1440` the goal, `+0x1458` the goal radius in blocks,
//!   `+0x145c` milliseconds on the path, `+0x1460`/`+0x1464` a `std::list` of waypoint blocks
//!   (front: the start end, back: the best node).
//!
//! A node's value is `{g, h, f = g + h, parent}` (`int`s, 0x18 bytes). The heuristic
//! (0x004dd090) is `2 * (10 * |dz| + (|dx| > |dy| ? 10|dx| + 4|dy| : 4|dx| + 10|dy|))`.
//!
//! Statics: `CreatureState::ignored_statics` is the `std::set<Static*>` at `creature+0x1468`
//! (looked up by 0x004db1b0 from the tick's static collision at 0x00541613). The path code
//! clears it with the path (0x00405330), and the search start (0x004dd2e0) fills it with the
//! statics whose box overlaps the creature's box at the start block and at the goal block, so
//! the creature can walk out of (and into) a static it stands in; the expansion ignores those
//! statics as obstacles. `creature+0x1478` (zone x, zone y, static index), outside this block,
//! is the static a `RandomInteractionBehavior` walks to; the expansion never treats that one as
//! a wall either ([`PathState::target_static`]).
//!
//! Blocks come from `World::block` (the original's `getBlock` 0x00405fd0, inlined in every
//! function here with the same constants). Fixed point as in `physics.rs`.

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments, clippy::too_many_lines, clippy::cognitive_complexity, clippy::excessive_precision)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use cw_net::EntityData;
use cw_world::World;
use cw_world::zone::Static;

use crate::combat::CreatureState;
use crate::util::{K, add3, blocks, f32_at, fix, fix3, i32_at, pos_at, set_vec3f, solid, sub3, to_block, to_block3, u16_at, u32_at, vec3f_at, w16, wf32};

/// A block coordinate, the node map's key.
pub type Key = [i32; 3];

/// A node of the search map (`creature+0x140c` values, 0x18 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Node {
    /// `+0`: cost from the root.
    pub g: i32,
    /// `+4`: heuristic to the goal (0x004dd090).
    pub h: i32,
    /// `+8`: `g + h`, the open list's order.
    pub f: i32,
    /// `+0xc`: the block this node was reached from (the root is its own parent).
    pub parent: Key,
}

/// The path state of a creature (`cube::Creature+0x1408..+0x1464`, plus `+0x1478`).
#[derive(Debug, Clone)]
pub struct PathState {
    /// `+0x1408`: the key of the node with the smallest heuristic (`None` = `end()`).
    pub best: Option<Key>,
    /// `+0x140c` (size at `+0x1410`): every node reached, by block.
    pub nodes: BTreeMap<Key, Node>,
    /// `+0x1414` (size at `+0x1418`): the open list.
    pub open: BTreeSet<Key>,
    /// `+0x141c`: the block the search is rooted at (`[-1; 3]` from the constructor).
    pub current: Key,
    /// `+0x1428`: the start position (16.16 fixed).
    pub start: [i64; 3],
    /// `+0x1440`: the goal position (16.16 fixed).
    pub goal: [i64; 3],
    /// `+0x1458`: the goal radius in blocks.
    pub radius: f32,
    /// `+0x145c`: milliseconds on the path. The same field as `CreatureState::path_ms` (the
    /// movement in `physics.rs` adds the tick's `dt` to that one); the integrator keeps one.
    pub path_ms: i32,
    /// `+0x1460` (size at `+0x1464`): the waypoint blocks, front at the start.
    pub waypoints: VecDeque<Key>,
    /// `+0x1478`: the static a `RandomInteractionBehavior` walks to (zone x, zone y, index),
    /// `(-1, -1, 0)` when none. Not a wall for the expansion.
    pub target_static: (i32, i32, i32),
}

impl Default for PathState {
    fn default() -> Self {
        PathState {
            best: None,
            nodes: BTreeMap::new(),
            open: BTreeSet::new(),
            current: [-1; 3],
            start: [0; 3],
            goal: [0; 3],
            radius: 0.0,
            path_ms: 0,
            waypoints: VecDeque::new(),
            target_static: (-1, -1, 0),
        }
    }
}

impl PathState {
    /// `creature+0x1410 != 0`: a search exists.
    pub fn has_path(&self) -> bool {
        !self.nodes.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers.

/// `Entity::setFlag` 0x00405570 on the u16 flags at `entity+0x114`.
fn set_flag(e: &mut EntityData, bit: u16, on: bool) {
    let f = u16_at(&e.0, 0x114);
    w16(&mut e.0, 0x114, if on { f | bit } else { f & !bit });
}

/// `v / 65536` truncated toward zero (`__alldiv` without the floor adjustment).
fn trunc_block(v: i64) -> i32 {
    (v / 65536) as i32
}

/// A block key to 16.16 fixed (`0x004d99d0`): `v << 16` per axis.
fn key_fixed(k: Key) -> [i64; 3] {
    [i64::from(k[0]) << 16, i64::from(k[1]) << 16, i64::from(k[2]) << 16]
}

/// A static's footprint after its rotation (odd quadrants swap x and y), halved and fixed:
/// `(half x, half y)`.
fn static_half(s: &Static) -> (i64, i64) {
    let (w, h) = if s.rotation % 2 != 0 { (s.scale[1], s.scale[0]) } else { (s.scale[0], s.scale[1]) };
    (fix(w * 0.5f32), fix(h * 0.5f32))
}

// duplicated from physics.rs (returning references and the index key)
/// The statics of the 8x8-block cell `(cx, cy)` in the order the zone holds them
/// (`getStaticCell` 0x0041c9e0 over the zone's spatial index at `zone+0xac`; the index is not
/// modelled, so a cell is the zone's statics whose position falls in it). Each comes with its
/// key `(zone x, zone y, index)`, the identity `CreatureState::ignored_statics` uses.
fn cell_statics(world: &World, cx: i32, cy: i32) -> Vec<((i32, i32, usize), &Static)> {
    let (bx, by) = (cx.wrapping_mul(8), cy.wrapping_mul(8));
    if !(0..0x1000000).contains(&bx) || !(0..0x1000000).contains(&by) {
        return Vec::new();
    }
    let (zx, zy) = (bx / 256, by / 256);
    let Some(zone) = world.zone(zx, zy) else { return Vec::new() };
    zone.statics.iter().enumerate().filter(|(_, s)| (s.x >> 16) as i32 / 8 == cx && (s.y >> 16) as i32 / 8 == cy).map(|(i, s)| ((zx, zy, i), s)).collect()
}

/// The heuristic 0x004dd090 of a block difference.
fn heuristic(d: Key) -> i32 {
    let ax = d[0].wrapping_abs();
    let ay = d[1].wrapping_abs();
    let az = d[2].wrapping_abs();
    let xy = if ay < ax { ax.wrapping_mul(10).wrapping_add(ay.wrapping_mul(4)) } else { ax.wrapping_mul(4).wrapping_add(ay.wrapping_mul(10)) };
    xy.wrapping_add(az.wrapping_mul(10)).wrapping_mul(2)
}

fn key_sub(a: Key, b: Key) -> Key {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2])]
}

/// `cvtdq2ps` then `cvttss2si` (0x004dd3bb): an int through a float and back; out of range
/// gives the integer indefinite `0x80000000`.
fn int_float_int(v: i32) -> i32 {
    let f = v as f32;
    if f >= 2147483648.0f32 || f < -2147483648.0f32 { i32::MIN } else { f as i32 }
}

/// `Creature::addPathNode` 0x004dd1a0: `map[key] = value`, the key goes on the open list, and
/// the best node becomes this one when the map had none or its `h` is smaller than the best's
/// (read after the store).
fn add_node(path: &mut PathState, key: Key, v: Node) {
    path.nodes.insert(key, v);
    path.open.insert(key);
    let replace = match path.best {
        None => true,
        Some(b) => match path.nodes.get(&b) {
            Some(bn) => v.h < bn.h,
            None => true,
        },
    };
    if replace {
        path.best = Some(key);
    }
}

// ---------------------------------------------------------------------------------------------
// Public API.

/// `Creature::clearPath` 0x00405330: the node map, the open list and the waypoint list are
/// emptied, the best node becomes `end()` and the passed statics (`creature+0x1468`) are
/// forgotten. The root, start, goal, radius and timer stay.
pub fn clear_path(path: &mut PathState, st: &mut CreatureState) {
    path.nodes.clear();
    path.open.clear();
    path.waypoints.clear();
    path.best = None;
    st.ignored_statics.clear();
}

/// The start position the behaviours give the search: the creature's position lowered to its
/// feet plus `base` blocks (`base` is 0.1 in `WalkPathBehavior` 0x004c6212, 0.5 in
/// `RandomInteractionBehavior` 0x0041c4a5 and in the follower's re-plan 0x004db24a):
/// `pos + fix([0, 0, base - scale.z * 0.5])`.
pub fn path_start(e: &EntityData, base: f32) -> [i64; 3] {
    let sz = f32_at(&e.0, 0x78);
    add3(pos_at(&e.0), fix3([0.0, 0.0, base - sz * 0.5f32]))
}

/// `Creature::startPath` `Server.exe 0x004dd2e0`: roots a new search at `path.start`
/// (`path.goal` must be set; the radius is not read). The path is cleared, the root block
/// becomes `path.current`, the timer restarts, the root node `{g 1, h, f h, parent itself}`
/// goes on the open list (`h` passes through a float, as the original's `cvtdq2ps` /
/// `cvttss2si`), and `CreatureState::ignored_statics` is refilled with the statics whose box
/// overlaps the creature's box standing in the start block and in the goal block.
pub fn start_search(world: &World, e: &EntityData, st: &mut CreatureState, path: &mut PathState) {
    clear_path(path, st);
    let sb = to_block3(path.start);
    path.current = sb;
    path.path_ms = 0;
    let gb = to_block3(path.goal);
    // 0x004dd35d: |d| per axis, `dx <= dy ? 4dx + 10dy : 10dx + 4dy`, + 10dz, * 2.
    let h = int_float_int(heuristic(key_sub(sb, gb)));
    add_node(path, sb, Node { g: 1, h, f: h, parent: sb });
    // 0x004dd3f8: the passed statics restart.
    st.ignored_statics.clear();
    ignore_overlapping(world, e, st, sb);
    ignore_overlapping(world, e, st, gb);
}

/// 0x004dd3fd / 0x004dd8b0: the creature's box standing in block `b` (centre
/// `b * 65536 + fix(0.5, 0.5, scale.z / 2)`, half extents `fix(scale / 2)`), tested against
/// the statics of the 3x3 cells around it; each overlapping one is ignored from now on.
fn ignore_overlapping(world: &World, e: &EntityData, st: &mut CreatureState, b: Key) {
    let scale = vec3f_at(&e.0, 0x70);
    let off = fix3([0.5, 0.5, scale[2] * 0.5f32]);
    let c = add3(key_fixed(b), off);
    let cx0 = trunc_block(c[0]) / 8;
    let cy0 = trunc_block(c[1]) / 8;
    for cx in cx0 - 1..=cx0 + 1 {
        for cy in cy0 - 1..=cy0 + 1 {
            if !(0..0x200000).contains(&cx) || !(0..0x200000).contains(&cy) {
                continue;
            }
            for (key, s) in cell_statics(world, cx, cy) {
                let kind = s.kind;
                if matches!(kind, 7 | 6 | 9) {
                    continue;
                }
                if matches!(kind, 1 | 8 | 2 | 3 | 5) && s.b30 == 0 {
                    continue;
                }
                let (hw, hh) = static_half(s);
                let hx = fix(scale[0] * 0.5f32);
                if !(c[0].wrapping_add(hx) >= s.x.wrapping_sub(hw)) || !(c[0].wrapping_sub(hx) < s.x.wrapping_add(hw)) {
                    continue;
                }
                let hy = fix(scale[1] * 0.5f32);
                if !(c[1].wrapping_add(hy) >= s.y.wrapping_sub(hh)) || !(c[1].wrapping_sub(hy) < s.y.wrapping_add(hh)) {
                    continue;
                }
                let hz = fix(scale[2] * 0.5f32);
                if !(c[2].wrapping_add(hz) >= s.z) || !(c[2].wrapping_sub(hz) < s.z.wrapping_add(fix(s.scale[2]))) {
                    continue;
                }
                st.ignored_statics.insert(key);
            }
        }
    }
}

/// The blocked neighbourhood of an expanded node (0x004de273..0x004df7c5). Shell blocks of the
/// creature's box grown by one block: X-/X+ etc. are that shell's faces.
#[derive(Default, Clone, Copy)]
struct Around {
    /// Corners at the bottom layer: (X-,Y-), (X+,Y-), (X-,Y+), (X+,Y+).
    c_mm: bool,
    c_pm: bool,
    c_mp: bool,
    c_pp: bool,
    /// Vertical edges between the bottom and top layers.
    e_mm: bool,
    e_pm: bool,
    e_mp: bool,
    e_pp: bool,
    /// The bottom face's interior: ground under the box.
    floor: bool,
    /// The top face's interior: a ceiling.
    ceil: bool,
    /// Bottom edges on the X-, X+, Y-, Y+ sides.
    bot_xm: bool,
    bot_xp: bool,
    bot_ym: bool,
    bot_yp: bool,
    /// Top edges on the X-, X+, Y- sides, and the fourth one (the original tests the bottom
    /// layer there, so it duplicates `bot_yp`).
    top_xm: bool,
    top_xp: bool,
    top_ym: bool,
    top_yp_bottom: bool,
    /// Side faces' interiors: a wall on X-, X+, Y-, Y+.
    side_xm: bool,
    side_xp: bool,
    side_ym: bool,
    side_yp: bool,
    /// A static under the box (the box lowered by one block overlaps it).
    on_static: bool,
    /// A static wall on X-, X+, Y-, Y+ (the box shifted by one block overlaps it).
    st_xm: bool,
    st_xp: bool,
    st_ym: bool,
    st_yp: bool,
    /// Bit 1 of the block type 0.001 block above the box centre (water, lava).
    liquid: bool,
}

/// `Creature::stepPath` `Server.exe 0x004dde90`: one A* expansion. Returns false when there is
/// no search, when it grew too big (more than 500 open or 20000 nodes: the path is cleared),
/// when the open list is empty or when the popped node is the goal block; true after an
/// expansion.
///
/// The open node with the smallest `f` is popped (the first in key order on a tie). Its
/// neighbourhood is probed with the creature's box standing in it; the moves are the four
/// horizontal ones, the four diagonals (only when both adjacent straight moves were taken in
/// this expansion), one down (when nothing is underfoot) and one up (next to a wall or in a
/// liquid). Costs: straight 10, diagonal 14, down 0, up 0, each plus 10 (straight, diagonal)
/// or 40 (up) when the node has no floor; a new -y node costs 20 extra instead of 10 (the
/// original's asymmetry). A move to a node already open replaces it when its `g` is smaller.
pub fn search_step(world: &World, e: &EntityData, st: &mut CreatureState, path: &mut PathState) -> bool {
    if path.nodes.is_empty() {
        return false;
    }
    if path.open.len() > 500 || path.nodes.len() > 20000 {
        clear_path(path, st);
        return false;
    }
    let gb = to_block3(path.goal);
    if path.open.is_empty() {
        return false;
    }
    // 0x004ddf01: the open node with the smallest f.
    let mut sel_key = *path.open.iter().next().unwrap();
    let mut sel: Option<Node> = None;
    for k in path.open.iter() {
        if let Some(v) = path.nodes.get(k)
            && (sel.is_none() || v.f < sel.unwrap().f)
        {
            sel = Some(*v);
            sel_key = *k;
        }
    }
    let k = sel_key;
    path.open.remove(&k);
    if k == gb {
        return false;
    }
    // Every open key has a node; the original would dereference null otherwise.
    let Some(cur) = sel else { return false };
    let scale = vec3f_at(&e.0, 0x70);

    // 0x004ddfd7: the box standing in the node. Centre z uses the double product.
    let hzd = (f64::from(scale[2]) * 0.5 * 65536.0) as i64;
    let c = add3(key_fixed(k), [32768, 32768, hzd]);
    let eps = fix3([0.0, 0.0, 0.01]);
    let half = fix3([scale[0] * 0.5f32, scale[1] * 0.5f32, scale[2] * 0.5f32]);
    let bmin = to_block3(add3(eps, sub3(c, half)));
    let bmax = to_block3(add3(c, half));
    let (xm, ym, zm) = (bmin[0] - 1, bmin[1] - 1, bmin[2] - 1);
    let (xp, yp, zp) = (bmax[0] + 1, bmax[1] + 1, bmax[2] + 1);
    let mut a = Around::default();
    // 0x004de330: the block 0.001 above the centre (`ftol(-65.536)` = -65 subtracted).
    {
        let b = world.block(to_block(c[0]), to_block(c[1]), to_block(c[2].wrapping_add(65)));
        a.liquid = (b[3] >> 1) & 1 != 0;
    }
    // 0x004de371: the shell of the grown box.
    for x in xm..=xp {
        for y in ym..=yp {
            for z in zm..=zp {
                if x > xm && x < xp && y > ym && y < yp && z > zm && z < zp {
                    continue;
                }
                if !solid(world.block(x, y, z)) {
                    continue;
                }
                let zmid = z > zm && z < zp;
                let xin = x > xm && x < xp;
                let yin = y > ym && y < yp;
                if x == xm && y == ym && z == zm {
                    a.c_mm = true;
                }
                if x == xp && y == ym && z == zm {
                    a.c_pm = true;
                }
                if x == xm && y == yp && z == zm {
                    a.c_mp = true;
                }
                if x == xp && y == yp && z == zm {
                    a.c_pp = true;
                }
                if x == xm && y == ym && zmid {
                    a.e_mm = true;
                }
                if x == xp && y == ym && zmid {
                    a.e_pm = true;
                }
                if x == xm && y == yp && zmid {
                    a.e_mp = true;
                }
                if x == xp && y == yp && zmid {
                    a.e_pp = true;
                }
                if xin && yin && z == zm {
                    a.floor = true;
                }
                if xin && yin && z == zp {
                    a.ceil = true;
                }
                if x == xm && yin && z == zm {
                    a.bot_xm = true;
                }
                if x == xp && yin && z == zm {
                    a.bot_xp = true;
                }
                if y == ym && xin && z == zm {
                    a.bot_ym = true;
                }
                if y == yp && xin && z == zm {
                    a.bot_yp = true;
                }
                if x == xm && yin && z == zp {
                    a.top_xm = true;
                }
                if x == xp && yin && z == zp {
                    a.top_xp = true;
                }
                if y == ym && xin && z == zp {
                    a.top_ym = true;
                }
                if y == yp && xin && z == zm {
                    a.top_yp_bottom = true;
                }
                if x == xm && yin && zmid {
                    a.side_xm = true;
                }
                if x == xp && yin && zmid {
                    a.side_xp = true;
                }
                if y == ym && xin && zmid {
                    a.side_ym = true;
                }
                if y == yp && xin && zmid {
                    a.side_yp = true;
                }
            }
        }
    }
    // 0x004de968: the statics of the cells under the grown box.
    let hx_c = fix(scale[0] * 0.5f32);
    let hy_c = fix(scale[1] * 0.5f32);
    let hz_c = fix(scale[2] * 0.5f32);
    const B: i64 = 0x10000;
    for cx in xm / 8..=xp / 8 {
        for cy in ym / 8..=yp / 8 {
            if !(0..0x200000).contains(&cx) || !(0..0x200000).contains(&cy) {
                continue;
            }
            for (key, s) in cell_statics(world, cx, cy) {
                if st.ignored_statics.contains(&key) {
                    continue;
                }
                let (hw, hh) = static_half(s);
                let sx_lo = s.x.wrapping_sub(hw);
                let sx_hi = s.x.wrapping_add(hw);
                let sy_lo = s.y.wrapping_sub(hh);
                let sy_hi = s.y.wrapping_add(hh);
                let sz_hi = s.z.wrapping_add(fix(s.scale[2]));
                let bx_hi = c[0].wrapping_add(hx_c);
                let bx_lo = c[0].wrapping_sub(hx_c);
                let by_hi = c[1].wrapping_add(hy_c);
                let by_lo = c[1].wrapping_sub(hy_c);
                let bz_hi = c[2].wrapping_add(hz_c);
                let bz_lo = c[2].wrapping_sub(hz_c);
                // 0x004dea94: any kind, standing on it (the box one block lower).
                if bx_hi >= sx_lo && bx_lo < sx_hi && by_hi >= sy_lo && by_lo < sy_hi && bz_hi.wrapping_sub(B) >= s.z && bz_lo.wrapping_sub(B) < sz_hi {
                    a.on_static = true;
                }
                // 0x004ded91: walls only count when the static is 3 blocks tall or the top
                // layer is blocked somewhere, and never for kinds 1, 2, 8, 7, 6 or the target.
                if !(s.scale[2] >= 3.0 || s.scale[2].is_nan()) && !a.ceil && !a.top_xm && !a.top_xp && !a.top_ym && !a.top_yp_bottom {
                    continue;
                }
                if matches!(s.kind, 1 | 2 | 8 | 7 | 6) {
                    continue;
                }
                if (key.0, key.1, key.2 as i32) == path.target_static {
                    continue;
                }
                let z_ok = bz_hi >= s.z && bz_lo < sz_hi;
                let y_ok = by_hi >= sy_lo && by_lo < sy_hi;
                let x_ok = bx_hi >= sx_lo && bx_lo < sx_hi;
                if bx_hi.wrapping_sub(B) >= sx_lo && bx_lo.wrapping_sub(B) < sx_hi && y_ok && z_ok {
                    a.st_xm = true;
                }
                if bx_hi.wrapping_add(B) >= sx_lo && bx_lo.wrapping_add(B) < sx_hi && y_ok && z_ok {
                    a.st_xp = true;
                }
                if x_ok && by_hi.wrapping_sub(B) >= sy_lo && by_lo.wrapping_sub(B) < sy_hi && z_ok {
                    a.st_ym = true;
                }
                if x_ok && by_hi.wrapping_add(B) >= sy_lo && by_lo.wrapping_add(B) < sy_hi && z_ok {
                    a.st_yp = true;
                }
            }
        }
    }

    // 0x004df7c5: the moves.
    let g = cur.g;
    let flat = |extra: i32| -> i32 { if a.floor { g.wrapping_add(10) } else { g.wrapping_add(10).wrapping_add(extra) } };
    let diag = if a.floor { g.wrapping_add(14) } else { g.wrapping_add(14).wrapping_add(10) };
    let (x, y, z) = (k[0], k[1], k[2]);

    // -x (0x004df7df): note the original leaves `on_static` out of this one.
    let mut did_xm = false;
    if !a.side_xm && !a.st_xm && (a.floor || a.bot_xm || a.side_ym || a.side_yp || a.e_mm || a.e_mp || a.liquid) {
        did_xm = try_move(path, [x.wrapping_sub(1), y, z], flat(10), flat(10), k, gb);
    }
    // +x (0x004df9c4)
    let mut did_xp = false;
    if !a.side_xp && !a.st_xp && (a.floor || a.on_static || a.bot_xp || a.side_ym || a.side_yp || a.e_pm || a.e_pp || a.liquid) {
        did_xp = try_move(path, [x.wrapping_add(1), y, z], flat(10), flat(10), k, gb);
    }
    // -y (0x004dfba1): a new node pays 20 without a floor, an open one 10.
    let mut did_ym = false;
    if !a.side_ym && !a.st_ym && (a.floor || a.on_static || a.bot_ym || a.side_xm || a.side_xp || a.e_mm || a.e_pm || a.liquid) {
        did_ym = try_move(path, [x, y.wrapping_sub(1), z], flat(20), flat(10), k, gb);
    }
    // +y (0x004dfd4f)
    let mut did_yp = false;
    if !a.side_yp && !a.st_yp && (a.floor || a.on_static || a.bot_yp || a.side_xm || a.side_xp || a.e_mp || a.e_pp || a.liquid) {
        did_yp = try_move(path, [x, y.wrapping_add(1), z], flat(10), flat(10), k, gb);
    }
    let support = a.floor || a.on_static || a.liquid;
    // -x-y (0x004dfee2)
    if !a.e_mm && did_xm && did_ym && (support || a.c_mm) {
        try_move(path, [x.wrapping_sub(1), y.wrapping_sub(1), z], diag, diag, k, gb);
    }
    // +x-y (0x004e003a)
    if !a.e_pm && did_xp && did_ym && (support || a.c_pm) {
        try_move(path, [x.wrapping_add(1), y.wrapping_sub(1), z], diag, diag, k, gb);
    }
    // -x+y (0x004e01a1)
    if !a.e_mp && did_xm && did_yp && (support || a.c_mp) {
        try_move(path, [x.wrapping_sub(1), y.wrapping_add(1), z], diag, diag, k, gb);
    }
    // +x+y (0x004e030d)
    if !a.e_pp && did_xp && did_yp && (support || a.c_pp) {
        try_move(path, [x.wrapping_add(1), y.wrapping_add(1), z], diag, diag, k, gb);
    }
    // down (0x004e046a): free when nothing is underfoot.
    if !a.floor && !a.on_static {
        try_move(path, [x, y, z.wrapping_sub(1)], g, g, k, gb);
    }
    // up (0x004e0486): next to a wall or in a liquid, under no ceiling.
    if !a.ceil && (a.side_xm || a.side_xp || a.side_ym || a.side_yp || a.liquid) {
        let up = if a.floor { g } else { g.wrapping_add(40) };
        try_move(path, [x, y, z.wrapping_add(1)], up, up, k, gb);
    }
    true
}

/// One move of the expansion: a new block gets `{g_new, h, g_new + h, parent}`; a block still on
/// the open list whose `g` exceeds `g_open` gets `{g_open, its h, g_open + h, parent}`; a closed
/// block is left alone. Returns whether a node was written (the diagonals need both of theirs).
fn try_move(path: &mut PathState, to: Key, g_new: i32, g_open: i32, parent: Key, gb: Key) -> bool {
    if let Some(ex) = path.nodes.get(&to).copied() {
        // 0x004dcff0: is it on the open list?
        if path.open.contains(&to) && g_open < ex.g {
            add_node(path, to, Node { g: g_open, h: ex.h, f: g_open.wrapping_add(ex.h), parent });
            return true;
        }
        false
    } else {
        let h = heuristic(key_sub(to, gb));
        add_node(path, to, Node { g: g_new, h, f: g_new.wrapping_add(h), parent });
        true
    }
}

/// `Creature::buildPath` `Server.exe 0x004dafe0`: the waypoint list from the best node (the
/// goal block when there is none) back along the parents to the start block, front first. The
/// walk stops at the start block, at a self-parent, at a missing node, or after more steps than
/// the map has nodes.
pub fn build_waypoints(path: &mut PathState) {
    let sb = to_block3(path.start);
    let gb = to_block3(path.goal);
    path.waypoints.clear();
    let mut cur = path.best.unwrap_or(gb);
    path.waypoints.push_back(cur);
    let mut i: i32 = 0;
    loop {
        if cur == sb {
            return;
        }
        if (path.nodes.len() as i32) < i {
            return;
        }
        i += 1;
        let Some(v) = path.nodes.get(&cur) else { return };
        if v.parent == cur {
            return;
        }
        path.waypoints.push_front(v.parent);
        cur = v.parent;
    }
}

/// The search as `WalkPathBehavior` starts it (`Server.exe 0x004c620b..0x004c62f9`): the path
/// is cleared, `start` and `goal` are stored, and when the goal's zone is loaded the search
/// is rooted ([`start_search`]), the radius stored, one expansion run, the waypoints built and
/// the acceleration zeroed. Returns false (nothing else done) when the goal's zone is not
/// loaded. `start` is usually [`path_start`]`(e, 0.1)`.
pub fn find_path(world: &World, e: &mut EntityData, st: &mut CreatureState, path: &mut PathState, start: [i64; 3], goal: [i64; 3], radius: f32) -> bool {
    clear_path(path, st);
    path.start = start;
    path.goal = goal;
    let zx = trunc_block(goal[0]) / 256;
    let zy = trunc_block(goal[1]) / 256;
    if world.zone(zx, zy).is_none() {
        return false;
    }
    start_search(world, e, st, path);
    path.radius = radius;
    search_step(world, e, st, path);
    build_waypoints(path);
    set_vec3f(&mut e.0, 0x30, [0.0; 3]);
    true
}

/// The search's per-update budget as `WalkPathBehavior` and `RandomInteractionBehavior` run it
/// (`Server.exe 0x004c6307..0x004c63a9`, 0x0041c638): with a search, up to ten times, unless the
/// waypoint list holds more than 50 blocks or its last block is within the radius of the goal,
/// one expansion and a rebuild of the waypoints.
pub fn advance_path(world: &World, e: &EntityData, st: &mut CreatureState, path: &mut PathState) {
    if path.nodes.is_empty() {
        return;
    }
    for _ in 0..10 {
        let n = path.waypoints.len();
        if n > 50 {
            break;
        }
        if n != 0 {
            let back = *path.waypoints.back().unwrap();
            let gb = to_block3(path.goal);
            let d = key_sub(back, gb);
            let d2 = d[2].wrapping_mul(d[2]).wrapping_add(d[1].wrapping_mul(d[1])).wrapping_add(d[0].wrapping_mul(d[0])) as f32;
            if path.radius * path.radius > d2 {
                break;
            }
        }
        search_step(world, e, st, path);
        build_waypoints(path);
    }
}

/// `hasGround` `Server.exe 0x004d5740`: whether a solid block lies in the layer under a box
/// centred at `pos` with extents `size` (the layer of `pos - size/2 - 0.2` in z, spanning the
/// box's x and y blocks).
fn has_ground(world: &World, pos: [i64; 3], size: [f32; 3]) -> bool {
    let eps = fix3([0.0, 0.0, 0.2]);
    let half = fix3([size[0] * 0.5f32, size[1] * 0.5f32, size[2] * 0.5f32]);
    let mn = to_block3(sub3(sub3(pos, half), eps));
    let mx = to_block3(add3(pos, half));
    for x in mn[0]..=mx[0] {
        for y in mn[1]..=mx[1] {
            if solid(world.block(x, y, mn[2])) {
                return true;
            }
        }
    }
    false
}

/// A waypoint's standing position: `(b.x + 0.5, b.y + 0.5, b.z + scale.z / 2)` in fixed.
fn stand_pos(b: Key, hz: i64) -> [i64; 3] {
    add3(key_fixed(b), [32768, 32768, hz])
}

/// `n` times `v += (target - v) * t` (the original unrolls it by four; same operations).
fn lerp_repeat(v: f32, target: f32, n: i32, t: f32) -> f32 {
    let mut v = v;
    for _ in 0..n {
        v = (target - v) * t + v;
    }
    v
}

/// `Creature::followPath` `Server.exe 0x004db200`, run by the tick with the creature and `dt`
/// in milliseconds (`World` in `ecx`). Returns true when it steered the creature (or it stands
/// on its target), which tells the tick to rebuild the waypoints; false when the goal is
/// reached (the path is cleared), when there are no waypoints, and when the path does not reach
/// the goal and the creature is not climbing.
///
/// The acceleration is zeroed first. Within the radius of the goal (feet block vs goal block)
/// the path clears. After 3000 ms on a path it re-plans from the feet (the search restarts,
/// which empties the waypoints). The nearest waypoint (last one on a tie) is found; standing
/// exactly in it resets the timer and, when it is a new block, re-roots the search there:
/// up to 70 open nodes with the smallest `h` stay open, the map keeps only the chains from
/// those back to the new root (the others leave the open list), and the best node is
/// recomputed. The target is the next waypoint (or two ahead on a step when there is ground),
/// the direction is normalised when longer than a block, clamped vertically (1 up; -2 down in
/// water; 3 up against a static, which also opens doors — kinds 1 and 2 — the creature
/// touches), and the velocity and acceleration ease towards it.
pub fn follow_path(world: &mut World, e: &mut EntityData, st: &mut CreatureState, path: &mut PathState, dt: i32) -> bool {
    set_vec3f(&mut e.0, 0x30, [0.0; 3]);
    let scale = vec3f_at(&e.0, 0x70);
    let pos = pos_at(&e.0);
    // 0x004db24a: the feet block against the goal block.
    let foot = add3(pos, fix3([0.0, 0.0, 0.5f32 - scale[2] * 0.5f32]));
    let fb = to_block3(foot);
    let gb = to_block3(path.goal);
    let d = key_sub(gb, fb);
    let d2 = d[0].wrapping_mul(d[0]).wrapping_add(d[2].wrapping_mul(d[2])).wrapping_add(d[1].wrapping_mul(d[1])) as f32;
    // `comiss r², d2; jc`: an unordered compare keeps the path.
    if path.radius * path.radius >= d2 {
        clear_path(path, st);
        return false;
    }
    if path.waypoints.is_empty() {
        return false;
    }
    if path.path_ms > 3000 {
        // 0x004db3ec: re-plan from the feet.
        path.start = foot;
        start_search(world, e, st, path);
    }
    if path.waypoints.is_empty() {
        return false;
    }
    // 0x004db4b5: the nearest waypoint to the feet (z lowered by half the height).
    let hz = fix(scale[2] * 0.5f32);
    let feet = to_block3(sub3(pos, [0, 0, hz]));
    let mut best_d2: i32 = -1;
    let mut nearest: usize = 0;
    for (i, w) in path.waypoints.iter().enumerate() {
        let dd = key_sub(feet, *w);
        let dd2 = dd[2].wrapping_mul(dd[2]).wrapping_add(dd[1].wrapping_mul(dd[1])).wrapping_add(dd[0].wrapping_mul(dd[0]));
        if best_d2 < 0 || dd2 <= best_d2 {
            best_d2 = dd2;
            nearest = i;
        }
    }
    if best_d2 == 0 {
        path.path_ms = 0;
    }
    let phys = u32_at(&e.0, 0x4c);
    // 0x004db5b3: a short path whose end is not within the radius of the goal is partial.
    let mut partial = false;
    if phys & 0x40 == 0 && path.waypoints.len() < 30 {
        let back = *path.waypoints.back().unwrap();
        let dd = key_sub(back, gb);
        let dd2 = dd[2].wrapping_mul(dd[2]).wrapping_add(dd[1].wrapping_mul(dd[1])).wrapping_add(dd[0].wrapping_mul(dd[0])) as f32;
        if !(dd2 <= path.radius * path.radius) {
            partial = true;
        }
    }
    let nk = path.waypoints[nearest];
    if !partial && best_d2 == 0 && nk != path.current {
        reroot(path, nk);
    }

    // 0x004dbe60: the target waypoint.
    let last = path.waypoints.len() - 1;
    let ground_here = has_ground(world, stand_pos(nk, hz), scale);
    let mut target = nearest;
    let mut at_end = false;
    let next_ok;
    let mut z_cmp = nk[2];
    if partial || nearest == last {
        next_ok = true;
        at_end = true;
    } else {
        target = nearest + 1;
        let nb = path.waypoints[target];
        z_cmp = nb[2];
        if has_ground(world, stand_pos(nb, hz), scale) {
            next_ok = true;
        } else {
            next_ok = false;
            at_end = true;
        }
    }
    let mut look_ok = true;
    if !partial && (ground_here || next_ok) && z_cmp != nk[2] && target != last {
        // 0x004dc140: a step: aim two waypoints ahead.
        target += 1;
        look_ok = false;
        if has_ground(world, stand_pos(path.waypoints[target], hz), scale) {
            look_ok = true;
            at_end = false;
        }
    }
    let tp = stand_pos(path.waypoints[target], hz);
    let dv = sub3(tp, pos);
    let mut dx = dv[0] as f32 * K;
    let mut dy = dv[1] as f32 * K;
    let mut dz = dv[2] as f32 * K;
    // 0x004dc415: flag 1 (climbing) when on the path, touching a wall and without ground.
    let climbing = best_d2 < 4 && phys & 4 != 0 && !ground_here;
    set_flag(e, 1, climbing);
    // 0x004dc444: whether to hold the vertical speed.
    let hold_z = dz <= 1.0f32 && phys & 0x40 == 0 && ground_here && look_ok;
    let len2 = dy * dy + dx * dx + dz * dz;
    if len2 <= 0.001f32 {
        return true;
    }
    if len2 > 1.0f32 {
        let s = 1.0f32 / (f64::from(len2).sqrt() as f32);
        dx = s * dx;
        dz = s * dz;
        dy = s * dy;
    }
    if dz > 0.0f32 {
        dz = 1.0;
    } else if 0.0f32 > dz && phys & 2 != 0 {
        dz = -2.0;
    }
    if phys & 0x40 != 0 {
        // 0x004dc530: against a static: climb, and open the doors in reach.
        dz = 3.0;
        open_doors(world, e);
    }
    if partial && u16_at(&e.0, 0x114) & 1 == 0 {
        return false;
    }
    if best_d2 >= 4 {
        // 0x004dcf6d: off the path: accelerate hard towards it.
        set_vec3f(&mut e.0, 0x30, [dx * 40.0f32, dy * 40.0f32, dz * 40.0f32]);
    } else {
        let mut lateral = true;
        if !partial {
            if !(dz < 0.0f32) && !hold_z {
                let v = lerp_repeat(f32_at(&e.0, 0x2c), dz * 4.0f32, dt, 0.05f32);
                wf32(&mut e.0, 0x2c, v);
            }
        } else if !(dz < 0.0f32) {
            set_vec3f(&mut e.0, 0x24, [dx * 8.0f32, dy * 8.0f32, dz * 8.0f32]);
            lateral = false;
        }
        if lateral {
            // 0x004dccf1
            let k = if u16_at(&e.0, 0x114) & 0x40 != 0 { 12.0f32 } else { 6.0f32 };
            if !at_end {
                let v = lerp_repeat(f32_at(&e.0, 0x24), k * dx, dt, 0.001f32);
                wf32(&mut e.0, 0x24, v);
                let v = lerp_repeat(f32_at(&e.0, 0x28), k * dy, dt, 0.001f32);
                wf32(&mut e.0, 0x28, v);
            } else {
                let v = lerp_repeat(f32_at(&e.0, 0x24), dx * 8.0f32, dt, 0.01f32);
                wf32(&mut e.0, 0x24, v);
                let v = lerp_repeat(f32_at(&e.0, 0x28), dy * 8.0f32, dt, 0.01f32);
                wf32(&mut e.0, 0x28, v);
            }
        }
        set_vec3f(&mut e.0, 0x30, [dx * 20.0f32, dy * 20.0f32, dz * 20.0f32]);
    }
    set_vec3f(&mut e.0, 0x3c, [0.0; 3]);
    true
}

/// 0x004db6ef..0x004dbe4b: the search re-rooted at waypoint `nk` the creature stands in.
fn reroot(path: &mut PathState, nk: Key) {
    path.current = nk;
    path.start = key_fixed(nk);
    // Up to 70 open nodes with the smallest h (first in key order on a tie) stay open.
    // Performance note for a future optimiser: quadratic in the open list, as the original.
    let mut kept = BTreeSet::new();
    let count = path.open.len().min(70);
    for _ in 0..count {
        let mut pick: Option<Key> = None;
        let mut best_h: i32 = -1;
        for k in path.open.iter() {
            if let Some(v) = path.nodes.get(k)
                && (best_h < 0 || v.h < best_h)
            {
                best_h = v.h;
                pick = Some(*k);
            }
        }
        if best_h >= 0
            && let Some(p) = pick
        {
            kept.insert(p);
            path.open.remove(&p);
        }
    }
    path.open = kept;
    // The chains from the kept nodes back to the root survive.
    let anchor = path.nodes.get(&nk).copied();
    let mut new_nodes: BTreeMap<Key, Node> = BTreeMap::new();
    let mut dropped: Vec<Key> = Vec::new();
    for k in path.open.iter() {
        let mut vk = *k;
        let mut v = path.nodes.get(k).copied();
        let mut chain: BTreeMap<Key, Node> = BTreeMap::new();
        let mut tmp = *k;
        // Pointer equality of the original: the same map entry, or both missing.
        let reached = match v {
            None => anchor.is_none(),
            Some(_) => loop {
                let cur = v.unwrap();
                if anchor.is_some() && vk == nk {
                    break true;
                }
                let parent = cur.parent;
                let p = path.nodes.get(&parent).copied();
                if parent == vk || p.is_none() {
                    break false;
                }
                chain.insert(tmp, cur);
                tmp = cur.parent;
                vk = parent;
                v = p;
            },
        };
        if reached {
            // `map::insert(first, last)`: existing keys are kept.
            for (ck, cv) in chain {
                new_nodes.entry(ck).or_insert(cv);
            }
        } else {
            dropped.push(*k);
        }
    }
    // The root keeps its value with itself as parent (the original copies from a null node when
    // the root is missing; that case keeps the map without a root here).
    if let Some(a) = anchor {
        new_nodes.insert(nk, Node { parent: nk, ..a });
    }
    for k in dropped {
        path.open.remove(&k);
    }
    path.nodes = new_nodes;
    // The best node: the first with the smallest h.
    path.best = None;
    let mut best_h = 0;
    for (k, v) in path.nodes.iter() {
        if path.best.is_none() || v.h < best_h {
            path.best = Some(*k);
            best_h = v.h;
        }
    }
}

/// 0x004dc5c5..0x004dcbd5: the doors (static kinds 1 and 2 still closed, `b30 != 0`) whose box
/// comes within a block of the creature's (in x and y; overlapping in z) open: `b30` and the
/// `+0x34` word are zeroed.
fn open_doors(world: &mut World, e: &EntityData) {
    let scale = vec3f_at(&e.0, 0x70);
    let pos = pos_at(&e.0);
    let half = fix3([scale[0] * 0.5f32, scale[1] * 0.5f32, scale[2] * 0.5f32]);
    let bmin = to_block3(sub3(pos, half));
    let bmax = to_block3(add3(pos, half));
    // `(1, 1, 0)` through `cvttss2si`, then `/ 8` truncated.
    let cx0 = (bmin[0] - 1) / 8;
    let cy0 = (bmin[1] - 1) / 8;
    let cx1 = (bmax[0] + 1) / 8;
    let cy1 = (bmax[1] + 1) / 8;
    let mut open: Vec<(i32, i32, usize)> = Vec::new();
    for cx in cx0..=cx1 {
        for cy in cy0..=cy1 {
            for (key, s) in cell_statics(world, cx, cy) {
                if !matches!(s.kind, 1 | 2) || s.b30 == 0 {
                    continue;
                }
                let (hw, hh) = static_half(s);
                let hx = fix(scale[0] * 0.5f32);
                let hy = fix(scale[1] * 0.5f32);
                let hz = fix(scale[2] * 0.5f32);
                const B: i64 = 0x10000;
                if !(s.x.wrapping_sub(hw) <= pos[0].wrapping_add(hx).wrapping_add(B)) || !(pos[0].wrapping_sub(hx).wrapping_sub(B) < s.x.wrapping_add(hw)) {
                    continue;
                }
                if !(s.y.wrapping_sub(hh) <= pos[1].wrapping_add(hy).wrapping_add(B)) || !(pos[1].wrapping_sub(hy).wrapping_sub(B) < s.y.wrapping_add(hh)) {
                    continue;
                }
                if !(s.z <= pos[2].wrapping_add(hz)) || !(pos[2].wrapping_sub(hz) < s.z.wrapping_add(fix(s.scale[2]))) {
                    continue;
                }
                open.push(key);
            }
        }
    }
    for (zx, zy, i) in open {
        if let Some(zone) = world.zone_mut(zx, zy)
            && let Some(s) = zone.statics.get_mut(i)
        {
            s.b30 = 0;
            s.f34 = 0;
        }
    }
}

/// The path part of the creature update in `World::tick` (`Server.exe 0x00537994..0x00537ac7`):
/// flag 0x80 clears; with waypoints and not stunned, flag 4 (aiming) clears and the path is
/// followed ([`follow_path`]); when that steered, the waypoints are rebuilt
/// ([`build_waypoints`]) and flag 0x80 stays clear. Otherwise, with waypoints left, the creature
/// aims at the last one (flag 4, the ray hit at `entity+0x150` = its block centre minus the
/// position, in blocks); flag 0x80 is set either way.
pub fn path_tick(world: &mut World, e: &mut EntityData, st: &mut CreatureState, path: &mut PathState, dt: i32) {
    set_flag(e, 0x80, false);
    if path.waypoints.is_empty() {
        return;
    }
    if i32_at(&e.0, 0x11c) > 0 {
        return;
    }
    set_flag(e, 4, false);
    if follow_path(world, e, st, path, dt) {
        build_waypoints(path);
        return;
    }
    if let Some(back) = path.waypoints.back().copied() {
        set_flag(e, 4, true);
        let t = add3(key_fixed(back), [32768, 32768, 32768]);
        let d = sub3(t, pos_at(&e.0));
        set_vec3f(&mut e.0, 0x150, [blocks(d[0]), blocks(d[1]), blocks(d[2])]);
    }
    set_flag(e, 0x80, true);
}

/// `boxCollides` `Server.exe 0x004d4f90(world, pos, size, checkStatics)`: whether a solid block
/// lies on the shell of the blocks a box centred at `pos` covers (the block range truncates
/// toward zero, without the floor adjustment of `to_block`), or, when asked, whether a static
/// (kinds 7, 6, 9 never; 1, 8, 2, 3, 5 only while `b30 != 0`) of the 3x3 cells around `pos`
/// overlaps it. The static's upper bound in z tests `pos.z - static.scale.z / 2` (the
/// original's operand) against its top. Ignored statics are not consulted.
pub fn box_collides(world: &World, pos: [i64; 3], size: [f32; 3], check_statics: bool) -> bool {
    let half = fix3([size[0] * 0.5f32, size[1] * 0.5f32, size[2] * 0.5f32]);
    let mn = [trunc_block(pos[0].wrapping_sub(half[0])), trunc_block(pos[1].wrapping_sub(half[1])), trunc_block(pos[2].wrapping_sub(half[2]))];
    let mx = [trunc_block(pos[0].wrapping_add(half[0])), trunc_block(pos[1].wrapping_add(half[1])), trunc_block(pos[2].wrapping_add(half[2]))];
    for x in mn[0]..=mx[0] {
        for y in mn[1]..=mx[1] {
            for z in mn[2]..=mx[2] {
                if x > mn[0] && x < mx[0] && y > mn[1] && y < mx[1] && z > mn[2] && z < mx[2] {
                    continue;
                }
                if solid(world.block(x, y, z)) {
                    return true;
                }
            }
        }
    }
    if check_statics {
        let cx0 = trunc_block(pos[0]) / 8;
        let cy0 = trunc_block(pos[1]) / 8;
        for cx in cx0 - 1..=cx0 + 1 {
            for cy in cy0 - 1..=cy0 + 1 {
                if !(0..0x200000).contains(&cx) || !(0..0x200000).contains(&cy) {
                    continue;
                }
                for (_, s) in cell_statics(world, cx, cy) {
                    if matches!(s.kind, 7 | 6 | 9) {
                        continue;
                    }
                    if matches!(s.kind, 1 | 8 | 2 | 3 | 5) && s.b30 == 0 {
                        continue;
                    }
                    let (hw, hh) = static_half(s);
                    let hx = fix(size[0] * 0.5f32);
                    if !(pos[0].wrapping_add(hx) >= s.x.wrapping_sub(hw)) || !(pos[0].wrapping_sub(hx) < s.x.wrapping_add(hw)) {
                        continue;
                    }
                    let hy = fix(size[1] * 0.5f32);
                    if !(pos[1].wrapping_add(hy) >= s.y.wrapping_sub(hh)) || !(pos[1].wrapping_sub(hy) < s.y.wrapping_add(hh)) {
                        continue;
                    }
                    let hz = fix(size[2] * 0.5f32);
                    if !(pos[2].wrapping_add(hz) >= s.z) {
                        continue;
                    }
                    let shz = fix(s.scale[2] * 0.5f32);
                    if pos[2].wrapping_sub(shz) < s.z.wrapping_add(fix(s.scale[2])) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// `lineOfSight` `Server.exe 0x004d4d80(world, from, to, checkStatics, maxDist)`: true when the
/// points are closer than 0.01 block; false when farther than `max_dist`; otherwise whether the
/// walk through air ([`sweep`] with `solid_wanted` false) from `from` towards `to` gets at
/// least to 0.01 block short of `to`.
pub fn line_of_sight(world: &World, from: [i64; 3], to: [i64; 3], check_statics: bool, max_dist: f32) -> bool {
    let d = sub3(to, from);
    let dx = d[0] as f32 * K;
    let dy = d[1] as f32 * K;
    let dz = d[2] as f32 * K;
    let d2 = dy * dy + dx * dx + dz * dz;
    if 1e-4f32 > d2 {
        return true;
    }
    if d2 > max_dist * max_dist {
        return false;
    }
    let len = f64::from(d2).sqrt() as f32;
    let dir = [dx / len, dy / len, dz / len];
    let t = sweep(world, from, dir, len, false, check_statics);
    t >= len - 0.01f32
}

/// `sweep` `Server.exe 0x004d6730(world, from, dir, maxDist, solidWanted, checkStatics)`: walks
/// the blocks along the unit vector `dir` from `from` while the block's solidity equals
/// `solid_wanted` (at most 501 steps), stepping to the nearest block face each time (the face
/// distances go through `ftol` and back as the original's), and returns the distance reached,
/// capped at `max_dist`. With `check_statics`, the distance is then cut at the first x or y face
/// of a static of kind 4, 3, 5 or 8 with `b30 != 0` (every static of the zones between the
/// start block and the end block, in zone order) that the ray crosses within the static's box.
pub fn sweep(world: &World, from: [i64; 3], dir: [f32; 3], max_dist: f32, solid_wanted: bool, check_statics: bool) -> f32 {
    let mut b = to_block3(from);
    let mut t = 0.0f32;
    let mut steps = 0;
    if 0.0f32 < max_dist {
        loop {
            let s = solid(world.block(b[0], b[1], b[2]));
            if s != solid_wanted || steps > 500 {
                break;
            }
            steps += 1;
            let p = add3(from, fix3([dir[0] * t, dir[1] * t, dir[2] * t]));
            let mut best = 10.0f32;
            let mut axis = 0usize;
            for i in 0..3 {
                let di = dir[i];
                if 1e-6f32 > di * di {
                    continue;
                }
                let face = if di > 0.0f32 { i64::from(b[i] + 1) << 16 } else { i64::from(b[i]) << 16 };
                let q = (face.wrapping_sub(p[i]) as f32) / di;
                let v = ((q as i64) as f32) * K;
                if best > v {
                    best = v;
                    axis = i;
                }
            }
            if dir[axis] > 0.0f32 {
                b[axis] += 1;
            } else {
                b[axis] -= 1;
            }
            t += best;
            if !(max_dist > t) {
                break;
            }
        }
    }
    if t > max_dist {
        t = max_dist;
    }
    if !check_statics {
        return t;
    }
    // 0x004d6bfd: the zones from the start block's to the end block's.
    let z0 = [trunc_block(from[0]) / 256, trunc_block(from[1]) / 256];
    let z1 = [b[0] / 256, b[1] / 256];
    let (zx0, zx1) = if z0[0] > z1[0] { (z1[0], z0[0]) } else { (z0[0], z1[0]) };
    let (zy0, zy1) = if z0[1] > z1[1] { (z1[1], z0[1]) } else { (z0[1], z1[1]) };
    for zx in zx0..=zx1 {
        for zy in zy0..=zy1 {
            let Some(zone) = world.zone(zx, zy) else { continue };
            for s in zone.statics.iter() {
                if !matches!(s.kind, 4 | 3 | 5 | 8) || s.b30 == 0 {
                    continue;
                }
                let (w, h) = if s.rotation % 2 != 0 { (s.scale[1], s.scale[0]) } else { (s.scale[0], s.scale[1]) };
                let sz_hi = |s: &Static| s.z.wrapping_add(fix(s.scale[2]));
                let at = |tt: f32| add3(from, fix3([dir[0] * tt, dir[1] * tt, dir[2] * tt]));
                if dir[0] != 0.0f32 || dir[0].is_nan() {
                    let hw = fix(w * 0.5f32);
                    let hh = fix(h * 0.5f32);
                    for face in [s.x.wrapping_sub(hw), s.x.wrapping_add(hw)] {
                        let t1 = (face.wrapping_sub(from[0]) as f32) * K / dir[0];
                        if t1 >= 0.0f32 && t > t1 {
                            let p = at(t1);
                            if p[1] >= s.y.wrapping_sub(hh) && p[2] >= s.z && p[1] < s.y.wrapping_add(hh) && p[2] < sz_hi(s) {
                                t = t1;
                            }
                        }
                    }
                }
                if dir[1] != 0.0f32 || dir[1].is_nan() {
                    let hw = fix(w * 0.5f32);
                    let hh = fix(h * 0.5f32);
                    for face in [s.y.wrapping_sub(hh), s.y.wrapping_add(hh)] {
                        let t1 = (face.wrapping_sub(from[1]) as f32) * K / dir[1];
                        if t1 >= 0.0f32 && t > t1 {
                            let p = at(t1);
                            if p[0] >= s.x.wrapping_sub(hw) && p[2] >= s.z && p[0] < s.x.wrapping_add(hw) && p[2] < sz_hi(s) {
                                t = t1;
                            }
                        }
                    }
                }
            }
        }
    }
    t
}

/// `canWalkDirect` `Server.exe 0x0052ef00(world, creature, from, to, reach)`: whether the
/// creature `e` can walk a straight line from `from` to `to`, simulated step by step.
///
/// - 0x0052ef1a: already within `reach` horizontally (`reach² > dx² + dy²`) and within
///   `height + reach` vertically: true.
/// - 0x0052f087: the horizontal vector to `to` is clamped to 50 blocks; the walk runs
///   `(int)(sqrt(len² + 0.5625) + 1)` steps from `from`.
/// - 0x0052f220, each step: within reach of `to` (`reach² >= e²`, same vertical test): true.
///   Otherwise the unit horizontal direction to the target and a fixed -0.71 fall make the
///   step; axis by axis (x, y, z) the position moves by `fix(step[a])` and the creature's box
///   is tested ([`box_collides`] without statics). A block hit on x or y tries a step up (unless
///   the appearance flags at `entity+0x6e` carry 0x100): if the box raised by 1.01 block is free
///   of solid blocks, the feet go to their block plus `2 * ftol(1.01 * 65536)` (the original
///   subtracts `ftol(-1.01 * 65536.0)` and then adds `ftol(1.01f * 65536)`), else the move is
///   undone. Without a block hit, the statics of the 3x3 cells around the position (kinds 7, 6,
///   9 never, the creature's ignored statics never, kinds 1, 8, 2, 3, 5 only while
///   `b30 != 0`) are tested when the move on that axis goes towards the static's centre; a hit
///   undoes the move. A block hit on z (the fall) is undone.
/// - 0x00530342: when the steps run out, true when the walk got more than 25 blocks from
///   `from` (`|P - from|² > 625`), false otherwise.
///
/// `reach` is the fifth argument (`my scale.x + leader scale.x` at the Companion call site).
pub fn walk_direct(world: &World, e: &EntityData, st: &CreatureState, from: [i64; 3], to: [i64; 3], reach: f32) -> bool {
    let scale = vec3f_at(&e.0, 0x70);
    let height = scale[2];
    let reach2 = reach * reach;
    // 0x0052ef1a
    {
        let dx = from[0].wrapping_sub(to[0]) as f32 * K;
        let dy = from[1].wrapping_sub(to[1]) as f32 * K;
        let d2 = dy * dy + dx * dx;
        if reach2 > d2 {
            let dz = (from[2].wrapping_sub(to[2]) as f32 * K).abs();
            if height + reach > dz {
                return true;
            }
        }
    }
    // 0x0052f087: the step count.
    let mut dirx = to[0].wrapping_sub(from[0]) as f32 * K;
    let mut diry = to[1].wrapping_sub(from[1]) as f32 * K;
    let z0 = 0.0f32 * 0.0f32;
    let len2 = diry * diry + dirx * dirx + z0;
    if len2 > 2500.0f32 {
        let s = 1.0f32 / (f64::from(len2).sqrt() as f32);
        dirx = s * dirx * 50.0f32;
        diry = s * diry * 50.0f32;
    }
    let steps_f = (f64::from(diry * diry + dirx * dirx + 0.5625f32).sqrt() as f32) + 1.0f32;
    // `cvttss2si`: out of range or NaN is 0x80000000.
    let steps = if steps_f.is_nan() || steps_f >= 2147483648.0f32 || steps_f < -2147483648.0f32 { i32::MIN } else { steps_f as i32 };
    let mut p = from;
    let no_step_up = u16_at(&e.0, 0x6e) & 0x100 != 0;
    // Performance note for a future optimiser: up to ~51 steps × 3 box scans per call.
    for _ in 0..steps.max(0) {
        // 0x0052f220: arrived?
        let ex = p[0].wrapping_sub(to[0]) as f32 * K;
        let ey = p[1].wrapping_sub(to[1]) as f32 * K;
        let e2 = ey * ey + ex * ex;
        if reach2 >= e2 {
            let dz = (p[2].wrapping_sub(to[2]) as f32 * K).abs();
            if height + reach > dz {
                return true;
            }
        }
        // 0x0052f395: the step: unit horizontal direction, 0.71 down.
        let mut sx = to[0].wrapping_sub(p[0]) as f32 * K;
        let mut sy = to[1].wrapping_sub(p[1]) as f32 * K;
        let l2 = sy * sy + sx * sx + z0;
        if l2 > 0.0f32 {
            let s = 1.0f32 / (f64::from(l2).sqrt() as f32);
            sx = s * sx;
            sy = s * sy;
        }
        let step = [sx, sy, f32::from_bits(0xbf35c28f)];
        for a in 0..3 {
            // 0x0052f4d0
            let off = fix(step[a]);
            p[a] = p[a].wrapping_add(off);
            let block_hit = box_collides(world, p, scale, false);
            let (hit, can_step) = if block_hit {
                (true, a != 2)
            } else {
                // 0x0052f54b: the statics of the 3x3 cells around the position.
                (walk_static_hit(world, st, p, scale, a, step[a]), false)
            };
            if !hit {
                continue;
            }
            if a == 2 || !can_step || no_step_up {
                // 0x00530301: undo.
                p[a] = p[a].wrapping_sub(off);
                continue;
            }
            // 0x0052fbd2: the box raised by 1.01 block must be free of solid blocks.
            let half = fix3([scale[0] * 0.5f32, scale[1] * 0.5f32, scale[2] * 0.5f32]);
            let raised = add3(p, fix3([0.0, 0.0, 1.01]));
            let mn = to_block3(sub3(raised, half));
            let mx = to_block3(add3(raised, half));
            let mut blocked = false;
            'scan: for x in mn[0]..=mx[0] {
                for y in mn[1]..=mx[1] {
                    for z in mn[2]..=mx[2] {
                        if solid(world.block(x, y, z)) {
                            blocked = true;
                        }
                    }
                    if blocked {
                        break 'scan;
                    }
                }
            }
            if blocked {
                p[a] = p[a].wrapping_sub(off);
                continue;
            }
            // 0x00530241: the feet to their block, then up by `ftol(1.01 * 65536)` twice.
            let hz = fix(scale[2] * 0.5f32);
            let feet_block = to_block(p[2].wrapping_sub(hz));
            let neg = (-1.01f64 * 65536.0) as i64;
            p[2] = (i64::from(feet_block) << 16).wrapping_sub(neg).wrapping_add(hz).wrapping_add(fix(1.01));
        }
    }
    // 0x00530342: moved more than 25 blocks.
    let dx = p[0].wrapping_sub(from[0]) as f32 * K;
    let dy = p[1].wrapping_sub(from[1]) as f32 * K;
    let dz = p[2].wrapping_sub(from[2]) as f32 * K;
    dy * dy + dx * dx + dz * dz > 625.0f32
}

/// 0x0052f54b..0x0052fb8a: whether a static of the 3x3 cells around `p` blocks the creature's
/// box at `p` after a move on `axis` towards it (`ftol((float)(p[a] - static[a]) * step) < 0`).
fn walk_static_hit(world: &World, st: &CreatureState, p: [i64; 3], scale: [f32; 3], axis: usize, step: f32) -> bool {
    let cx0 = trunc_block(p[0]) / 8;
    let cy0 = trunc_block(p[1]) / 8;
    for cx in cx0 - 1..=cx0 + 1 {
        for cy in cy0 - 1..=cy0 + 1 {
            if !(0..0x200000).contains(&cx) || !(0..0x200000).contains(&cy) {
                continue;
            }
            for (key, s) in cell_statics(world, cx, cy) {
                if matches!(s.kind, 7 | 6 | 9) {
                    continue;
                }
                if st.ignored_statics.contains(&key) {
                    continue;
                }
                if matches!(s.kind, 1 | 8 | 2 | 3 | 5) && s.b30 == 0 {
                    continue;
                }
                let sp = [s.x, s.y, s.z];
                let toward = ((p[axis].wrapping_sub(sp[axis]) as f32) * step) as i64;
                if !(toward < 0) {
                    continue;
                }
                let (hw, hh) = static_half(s);
                let hx = fix(scale[0] * 0.5f32);
                if !(p[0].wrapping_add(hx) >= s.x.wrapping_sub(hw)) || !(p[0].wrapping_sub(hx) < s.x.wrapping_add(hw)) {
                    continue;
                }
                let hy = fix(scale[1] * 0.5f32);
                if !(p[1].wrapping_add(hy) >= s.y.wrapping_sub(hh)) || !(p[1].wrapping_sub(hy) < s.y.wrapping_add(hh)) {
                    continue;
                }
                let hz = fix(scale[2] * 0.5f32);
                if !(p[2].wrapping_add(hz) >= s.z) {
                    continue;
                }
                if p[2].wrapping_sub(hz) < s.z.wrapping_add(fix(s.scale[2])) {
                    return true;
                }
            }
        }
    }
    false
}
