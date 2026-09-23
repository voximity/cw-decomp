//! `cube::World::generateDungeon`, `Server.exe 0x00500300`, with the `cube::Dungeon` cell grid
//! (`Server.exe 0x004f7370` and friends), the dungeon block writers (`fillDungeonBox`
//! `0x00513400`, `clearBox` `0x004d2500`, `fillFlat` `0x004ff340`) and the item, furniture and
//! wall-light generators it calls. See `analysis/notes/functions/00500300_generateDungeon.md`
//! and `analysis/notes/porting-brief.md`.
//!
//! Every `rand()` call site carries a `// rand @0x...` comment with the address of the call
//! instruction in the original.
//!
//! Entities the original stores in zone containers that [`Zone`] does not have yet (the marker
//! vector at `zone+0x48`, spawn `+8` and AI, static and ground-item item records) are
//! collected in a [`DungeonExtras`] passed to [`World::generate_dungeon_ext`].

use std::cell::Cell;

use cw_math::MsvcRand;
use cw_math::value_noise_2d as noise;

use crate::fixed::{from_block, to_block};
use crate::appearance::Item;
use crate::inventory::Slot;
use crate::surface::Block;
use crate::world::World;
use crate::zone::{AIR_BLOCK, GroundItem, Prop, Spawn, SpawnAi, Static, Zone, set_block};

// ---------------------------------------------------------------------------------------------
// Fixed-point helpers.

/// `FUN_004dab30(double)`: `_ftol2(d * 65536.0)`.
#[inline]
fn fx_d(v: f64) -> i64 {
    (v * 65536.0) as i64
}

/// `FUN_00402a10(float)`: `_ftol2(f * 65536.0f)` (the multiply is a float `MULSS`).
#[inline]
fn fx_f(v: f32) -> i64 {
    (v * 65536.0f32) as i64
}

/// `FUN_004061f0`: block type is neither air (0) nor water (2).
#[inline]
fn is_solid(b: Block) -> bool {
    let t = b[3] & 0x1f;
    t != 0 && t != 2
}

/// `FUN_00522820`: a dungeon cell a wall prop may face (empty, the out-of-bounds dummy, filler).
#[inline]
fn passable(t: u8) -> bool {
    t == 0 || t == 2 || t == 1
}

// ---------------------------------------------------------------------------------------------
// cube::Dungeon

thread_local! {
    /// The flag byte of the out-of-bounds dummy cell (`DAT_005842c9`). The original zeroes it once
    /// per process and then only ORs into it, so its state leaks from one dungeon to the next.
    static DUMMY_FLAGS: Cell<u8> = const { Cell::new(0) };
}

/// Sets the process-wide out-of-bounds dummy flags (`DAT_005842c9`); for tests and for replaying a
/// generation sequence from a known state.
pub fn set_dungeon_dummy_flags(v: u8) {
    DUMMY_FLAGS.with(|c| c.set(v));
}

/// The current value of the out-of-bounds dummy flags (`DAT_005842c9`).
pub fn dungeon_dummy_flags() -> u8 {
    DUMMY_FLAGS.with(|c| c.get())
}

/// `cube::Dungeon` (0x1c bytes): a grid of `u16` cells, low byte the type (0 empty, 1 the
/// out-of-bounds dummy, 2 filler, 3 room or corridor, 4 entrance), high byte flags (1 ramp,
/// 2 carpet, 4 boss). Every access goes through the rotation/mirror transform.
#[derive(Debug, Clone)]
pub struct Dungeon {
    /// `+4`
    pub rotation: i32,
    /// `+8`
    pub mirror: bool,
    /// `+0xc`, `+0x10`, `+0x14`
    pub sx: i32,
    pub sy: i32,
    pub sz: i32,
    /// `+0x18`: `[type, flags]`, index `(sy * z + y) * sx + x` in storage coordinates.
    pub cells: Vec<[u8; 2]>,
    dummy_flags: u8,
}

impl Drop for Dungeon {
    fn drop(&mut self) {
        let f = self.dummy_flags;
        DUMMY_FLAGS.with(|c| c.set(f));
    }
}

impl Dungeon {
    /// `cube::Dungeon::Dungeon(sx, sy, sz)`, `Server.exe 0x004f7370`.
    pub fn new(sx: i32, sy: i32, sz: i32) -> Self {
        Self {
            rotation: 0,
            mirror: false,
            sx,
            sy,
            sz,
            cells: vec![[0, 0]; (sx * sy * sz) as usize],
            dummy_flags: DUMMY_FLAGS.with(|c| c.get()),
        }
    }

    /// `FUN_0052dde0`: logical to storage coordinates.
    pub fn to_storage(&self, mut x: i32, mut y: i32) -> (i32, i32) {
        match self.rotation % 4 {
            1 => {
                let t = x;
                x = self.sx - y - 1;
                y = t;
            }
            2 => {
                x = self.sx - x - 1;
                y = self.sy - y - 1;
            }
            3 => {
                let t = x;
                x = y;
                y = self.sy - t - 1;
            }
            _ => {}
        }
        if self.mirror {
            y = self.sy - y - 1;
        }
        (x, y)
    }

    /// `FUN_0052de60`: storage to logical coordinates.
    pub fn to_logical(&self, mut x: i32, mut y: i32) -> (i32, i32) {
        if self.mirror {
            y = self.sy - y - 1;
        }
        match self.rotation % 4 {
            1 => {
                let t = x;
                x = y;
                y = self.sy - t - 1;
            }
            2 => {
                x = self.sx - x - 1;
                y = self.sy - y - 1;
            }
            3 => {
                std::mem::swap(&mut x, &mut y);
                x = self.sx - x - 1;
            }
            _ => {}
        }
        (x, y)
    }

    /// `FUN_0052d820`: logical X extent.
    pub fn dim_x(&self) -> i32 {
        if self.rotation % 2 != 0 { self.sy } else { self.sx }
    }

    /// `FUN_0052d840`: logical Y extent.
    pub fn dim_y(&self) -> i32 {
        if self.rotation % 2 != 0 { self.sx } else { self.sy }
    }

    /// `FUN_0052d860`
    pub fn dim_z(&self) -> i32 {
        self.sz
    }

    fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let (x, y) = self.to_storage(x, y);
        if x < 0 || y < 0 || z < 0 || x >= self.sx || y >= self.sy || z >= self.sz {
            return None;
        }
        Some(((self.sy * z + y) * self.sx + x) as usize)
    }

    /// `Dungeon::cell(x, y, z)`, `Server.exe 0x004f84a0`, read: `[type, flags]`. Outside the grid
    /// the shared dummy reads as type 1 with the sticky dummy flags.
    pub fn cell(&self, x: i32, y: i32, z: i32) -> [u8; 2] {
        match self.index(x, y, z) {
            Some(i) => self.cells[i],
            None => [1, self.dummy_flags],
        }
    }

    /// Cell type write; a write to the dummy is lost (its type is reset to 1 on every access).
    pub fn set_type(&mut self, x: i32, y: i32, z: i32, t: u8) {
        if let Some(i) = self.index(x, y, z) {
            self.cells[i][0] = t;
        }
    }

    /// Flag OR; an out-of-bounds OR lands in the sticky dummy flags.
    pub fn or_flags(&mut self, x: i32, y: i32, z: i32, f: u8) {
        match self.index(x, y, z) {
            Some(i) => self.cells[i][1] |= f,
            None => self.dummy_flags |= f,
        }
    }

    /// `Dungeon::carveRoom(pos, size, pillars)`, `Server.exe 0x005236d0`.
    pub fn carve_room(&mut self, rng: &mut MsvcRand, p: [i32; 3], s: [i32; 3], pillars: bool) {
        if !(0 < s[0] && 0 < s[1] && 0 < s[2]) {
            return;
        }
        for x in p[0]..p[0] + s[0] {
            for y in p[1]..p[1] + s[1] {
                for z in p[2]..p[2] + s[2] {
                    self.set_type(x, y, z, 3);
                }
            }
        }
        // rand @0x0052382e
        if rng.rand() % 2 != 0 {
            let w = rng.rand() % s[0] + 1; // rand @0x00523842
            let h = rng.rand() % s[1] + 1; // rand @0x00523850
            let mut cx = p[0];
            let mut cy = p[1];
            if s[0] != w && s[0] - w >= 0 {
                cx += rng.rand() % (s[0] - w + 1); // rand @0x00523876
            }
            if s[1] != h && s[1] - h >= 0 {
                cy += rng.rand() % (s[1] - h + 1); // rand @0x0052389c
            }
            for x in cx..cx + w {
                for y in cy..cy + h {
                    self.or_flags(x, y, p[2], 2);
                }
            }
        }
        if pillars && 2 < s[0] && 2 < s[1] {
            let w = rng.rand() % (s[0] - 2) + 1; // rand @0x005239e4
            let h = rng.rand() % (s[1] - 2) + 1; // rand @0x005239f4
            let mut px = p[0] + 1;
            let mut py = p[1] + 1;
            if 1 < s[0] - w {
                px += rng.rand() % (s[0] - w - 1); // rand @0x00523a1f
            }
            if 1 < s[1] - h {
                py += rng.rand() % (s[1] - h - 1); // rand @0x00523a43
            }
            for x in px..px + w {
                for y in py..py + h {
                    for z in p[2]..p[2] + s[2] {
                        self.set_type(x, y, z, 2);
                    }
                }
            }
        }
    }

    /// `Dungeon::line(p0, p1)`, `Server.exe 0x005234b0`: an integer DDA from `p0` to `p1` setting
    /// type 3; each z step also opens the cell above/below and marks the lower one as a ramp.
    pub fn line(&mut self, p0: [i32; 3], p1: [i32; 3]) {
        let dx = p1[0].wrapping_sub(p0[0]);
        let dy = p1[1].wrapping_sub(p0[1]);
        let dz = p1[2].wrapping_sub(p0[2]);
        let (ax, ay, az) = (dx.abs(), dy.abs(), dz.abs());
        let mxy = if ay < ax { ax } else { ay };
        let mut n = az;
        if az < mxy {
            n = if ay < ax { ax } else { ay };
        }
        if n == 0 {
            return;
        }
        let mut prev_z = p0[2];
        let mut i = 0;
        while i <= n {
            let x = p0[0] + dx.wrapping_mul(i) / n;
            let y = p0[1] + dy.wrapping_mul(i) / n;
            let z = p0[2] + dz.wrapping_mul(i) / n;
            self.set_type(x, y, z, 3);
            if z < prev_z {
                self.set_type(x, y, z + 1, 3);
                self.or_flags(x, y, z, 1);
            }
            if prev_z < z {
                self.set_type(x, y, z - 1, 3);
                self.or_flags(x, y, z - 1, 1);
            }
            prev_z = z;
            i += 1;
        }
    }

    /// `Dungeon::carveCorridor(posA, sizeA, posB, sizeB)`, `Server.exe 0x004f9010`. The
    /// cross-axis comparisons in the second branch (`B.x + sB.x - 1 <= m` against a y value)
    /// are the original's.
    pub fn carve_corridor(&mut self, rng: &mut MsvcRand, a: [i32; 3], sa: [i32; 3], b: [i32; 3], sb: [i32; 3]) {
        let ax_end = a[0] + sa[0];
        if b[0] < ax_end && a[0] < sb[0] + b[0] {
            let mut m = sb[0] / 2 + b[0];
            if m < a[0] {
                m = a[0];
            }
            if ax_end <= m {
                m = ax_end - 1;
            }
            let (y_from, y_to) = if b[1] < a[1] { (a[1], b[1] + sb[1]) } else { (sa[1] - 1 + a[1], b[1] - 1) };
            self.line([m, y_from, a[2]], [m, y_to, b[2]]);
            // rand @0x004f90a7 / 0x004f9183
            if rng.rand() % 2 != 0 && a[0] < m && b[0] < m {
                self.line([m - 1, y_from, a[2]], [m - 1, y_to, b[2]]);
            }
            // rand @0x004f90fa / 0x004f91da
            if rng.rand() % 2 == 0 || sa[0] - 1 + a[0] <= m || sb[0] - 1 + b[0] <= m {
                return;
            }
            self.line([m + 1, y_from, a[2]], [m + 1, y_to, b[2]]);
        } else {
            let ay_end = sa[1] + a[1];
            if ay_end <= b[1] || b[1] + sb[1] <= a[1] {
                return;
            }
            let mut m = sb[1] / 2 + b[1];
            if m < a[1] {
                m = a[1];
            }
            if ay_end <= m {
                m = ay_end - 1;
            }
            if b[0] < a[0] {
                let x_to = b[0] + sb[0];
                self.line([a[0], m, a[2]], [x_to, m, b[2]]);
                // rand @0x004f92b1
                if rng.rand() % 2 != 0 && a[1] < m && b[1] < m {
                    self.line([a[0], m - 1, a[2]], [x_to, m - 1, b[2]]);
                }
                // rand @0x004f9303
                if rng.rand() % 2 == 0 || a[1] - 1 + sa[1] <= m || b[0] + sb[0] - 1 <= m {
                    return;
                }
                self.line([a[0], m + 1, a[2]], [x_to, m + 1, b[2]]);
            } else {
                let x_from = a[0] + sa[0] - 1;
                let x_to = b[0] - 1;
                self.line([x_from, m, a[2]], [x_to, m, b[2]]);
                // rand @0x004f9388
                if rng.rand() % 2 != 0 && a[1] < m && b[1] < m {
                    self.line([x_from, m - 1, a[2]], [x_to, m - 1, b[2]]);
                }
                // rand @0x004f93de
                if rng.rand() % 2 == 0 || sa[0] - 1 + a[0] <= m || sb[0] - 1 + b[0] <= m {
                    return;
                }
                self.line([x_from, m + 1, a[2]], [x_to, m + 1, b[2]]);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Items, props, markers, AI.

/// A marker in the `zone+0x48` vector (0x140 bytes, constructor `Server.exe 0x004f7490`).
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// `+0`: 5 dungeon entrance, 6 boss.
    pub kind: i32,
    /// `+0x11c`: index of the boss in `zone.spawns` (-1 from the constructor).
    pub spawn_index: i32,
    /// `+0x120`: creature type (-1 from the constructor).
    pub creature_type: i32,
    /// `+0x128`, `+0x130`, `+0x138`
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

/// One node of a spawn's AI tree (`spawn+0x109c`).
#[derive(Debug, Clone, PartialEq)]
pub enum Behavior {
    /// `CombatBehavior(float)`, `Server.exe 0x004029e0` (0x14 bytes).
    Combat(f32),
    /// `WalkPathBehavior(float)`, `Server.exe 0x004c5d50`, with its waypoints (`FUN_004e1420`).
    WalkPath(f32, Vec<[i64; 3]>),
    /// `CompanionBehavior`, `Server.exe 0x004055d0`: follows the spawn with this id.
    Companion(i64),
    /// `SequentialBehavior`, `Server.exe 0x0041cfc0`, with its children in order.
    Sequence(Vec<Behavior>),
}

/// Spawn fields the dungeon writes that [`Spawn`] does not carry yet.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnExtra {
    /// `+8`: 200.0 from the constructor, 150.0 for every dungeon spawn.
    pub f8: f32,
    /// `+0x109c`: the AI root.
    pub ai: Option<Behavior>,
}

impl SpawnExtra {
    pub const NEW: SpawnExtra = SpawnExtra { f8: 200.0, ai: None };
}

/// `spawn+0x48` (the 64-bit id) lives in `Spawn::f38[4..6]` (`+0x38..+0x50`).
fn set_spawn_uid(s: &mut Spawn, uid: i64) {
    s.f38[4] = uid as u32;
    s.f38[5] = (uid >> 32) as u32;
}

/// Everything generateDungeon writes that [`Zone`] cannot hold yet.
#[derive(Debug, Clone, Default)]
pub struct DungeonExtras {
    /// `zone+0x48` markers, in push order.
    pub markers: Vec<Marker>,
    /// Extra fields of `zone.spawns[i]`.
    pub spawns: Vec<(usize, SpawnExtra)>,
    /// Item lists (`+0x48`) of `zone.statics[i]` (small chests).
    pub static_items: Vec<(usize, Vec<Item>)>,
    /// Full item of `zone.items[i]`.
    pub ground_items: Vec<(usize, Item)>,
    /// `zone.items[i]` byte `+0x138`.
    pub ground_item_b138: Vec<(usize, u8)>,
}

/// `FUN_004f3850(zx, zy, n)`: `((zy << 16) + zx) << 8 + n` as a 64-bit id.
fn spawn_uid(zx: i32, zy: i32, n: i32) -> i64 {
    ((i64::from(zy) << 16) + i64::from(zx)) * 256 + i64::from(n)
}

/// The rarity roll inlined in the item generators and `FUN_0052bf40`.
fn rarity_roll(rng: &mut MsvcRand, n: i32) -> i32 {
    let mut r = rng.rand() % n;
    if rng.rand() % 100 == 0 {
        r += 1;
    }
    if rng.rand() % 1000 == 0 {
        r += 1;
    }
    if rng.rand() % 10000 == 0 {
        r += 1;
    }
    r
}

/// `FUN_0052bf40(tier, force)`: random rarity; `force` returns `tier + 1` after the rolls.
pub fn random_rarity(rng: &mut MsvcRand, tier: i32, force: bool) -> i32 {
    // rand @0x0052bf50, 0x0052bf57, 0x0052bf66, 0x0052bf75
    let mut r = rarity_roll(rng, tier + 1);
    if force {
        r = tier + 1;
    }
    if 4 < r {
        r = 4;
    }
    r
}

/// `FUN_0052c160(level)`: a consumable/misc item for trap-spawn inventories.
pub fn misc_item(rng: &mut MsvcRand, level: i32) -> Item {
    let mut it = Item::NEW;
    // rand @0x0052c1b1
    if rng.rand() % 2 != 0 {
        pow_item(rng, &mut it, level, 10);
        return it;
    }
    it.level = level as u16;
    // rand @0x0052c239
    if rng.rand() % 6 == 0 {
        // rand @0x0052c293
        match rng.rand() % 5 {
            0 => {
                it.level = level as u16;
                it.item_type = 1;
                it.sub_type = 1;
            }
            1 => {
                it.item_type = 1;
                it.sub_type = 4;
                it.level = level as u16;
                // Falls through into case 2 in the original.
                pow_item(rng, &mut it, level, 0xc);
            }
            2 => pow_item(rng, &mut it, level, 0xc),
            3 => {
                it.level = level as u16;
                it.item_type = 1;
                it.sub_type = 7;
            }
            4 => {
                it.item_type = 1;
                it.sub_type = 5;
                it.level = level as u16;
            }
            _ => {}
        }
    } else {
        // rand @0x0052c247
        let r = rng.rand() % 2;
        if r == 1 {
            it.item_type = 0x12;
            let m = rng.rand(); // rand @0x0052c27b
            it.material = 0;
            it.modifier = m % 3;
        }
    }
    it
}

/// The `pow(2, ...)` coin roll shared by `FUN_0052c160` and generateDungeon's ground items: sets
/// the type byte to 0xc (the subtype byte is kept), the material, and the level field to
/// `rand() % ((int)(float)pow(2, e) * 2 + 2)`.
fn pow_item(rng: &mut MsvcRand, it: &mut Item, level: i32, material: u8) {
    let e = (rng.rand() as f32 * 2.0f32 / 32767.0f32 + 1.0f32) * (level as f32 * 0.25f32); // rand @0x0052c1c1 / 0x0052c2c6 / 0x00507177
    // `_libm_sse2_pow_precise` (direct at 0x0052c201 / 0x0052c305, through the float wrapper
    // 0x004055a0 at 0x005071bf).
    let p = cw_math::pow(2.0, f64::from(e)) as f32;
    let r = rng.rand(); // rand @0x0052c20f / 0x0052c313 / 0x005071dc
    it.item_type = 0xc;
    it.material = material;
    // `cvttss2si` yields INT_MIN for a value outside the i32 range (high-level dungeons overflow
    // the pow), and the `* 2 + 2` then wraps: the original ends up with `rand() % 2`.
    let pi = if p.is_nan() || !(-2147483648.0..2147483648.0).contains(&p) { i32::MIN } else { p as i32 };
    it.level = (r % pi.wrapping_mul(2).wrapping_add(2)) as u16;
}

/// `FUN_00528bf0(level, rarity, -1)`: random armour piece.
fn armor_item(rng: &mut MsvcRand, level: i32, rarity: u8) -> Item {
    let mut list = Vec::new();
    let mut it = Item::NEW;
    it.level = level as u16;
    it.rarity = rarity;
    let mut push = |rng: &mut MsvcRand, it: &mut Item, kind: u8| {
        it.item_type = kind;
        it.modifier = rng.rand() % 100; // rand @0x00528cb3.. (one per entry, in order)
        list.push(*it);
    };
    it.material = 1;
    for k in [7, 4, 5, 6] {
        push(rng, &mut it, k);
    }
    for m in [0x19, 0x1a, 0x1b] {
        it.material = m;
        for k in [7, 7, 4, 5, 6] {
            push(rng, &mut it, k);
        }
    }
    it.material = 11 + (rng.rand() % 2 != 0) as u8; // rand @0x00528fe6
    push(rng, &mut it, 8);
    it.material = 11 + (rng.rand() % 2 != 0) as u8; // rand @0x00529023
    push(rng, &mut it, 9);
    let r = rng.rand() as u32; // rand @0x00529060
    list[(r % list.len() as u32) as usize]
}

/// `FUN_0052c4e0(level, rarity, -1)`: random weapon.
fn weapon_item(rng: &mut MsvcRand, level: i32, rarity: u8) -> Item {
    let mut list = Vec::new();
    let mut it = Item::NEW;
    it.level = level as u16;
    it.rarity = rarity;
    it.material = 1;
    it.item_type = 3;
    let mut push = |rng: &mut MsvcRand, it: &mut Item| {
        it.modifier = rng.rand() % 100; // rand @0x0052c5a6.. (one per entry, in order)
        list.push(*it);
    };
    it.sub_type = (rng.rand() % 3) as u8; // rand @0x0052c596
    push(rng, &mut it);
    it.material = 1;
    it.sub_type = (rng.rand() % 3) as u8 + 0xf; // rand @0x0052c5cf
    push(rng, &mut it);
    it.sub_type = 0xd;
    push(rng, &mut it);
    for s in [3, 5, 4] {
        it.sub_type = s;
        push(rng, &mut it);
    }
    it.material = 2;
    it.sub_type = 6;
    push(rng, &mut it);
    rng.rand(); // rand @0x0052c6e2 (discarded)
    it.sub_type = 8;
    push(rng, &mut it);
    it.sub_type = 10;
    push(rng, &mut it);
    it.sub_type = 0xb;
    push(rng, &mut it);
    it.sub_type = 0xc;
    let r = rng.rand() % 2; // rand @0x0052c76f
    it.material = 0xc - (r != 0) as u8;
    push(rng, &mut it);
    let r = rng.rand() as u32; // rand @0x0052c7ac
    list[(r % list.len() as u32) as usize]
}

/// `FUN_0052a760(level, rarity)`: an equipment formula (type 2, `recipe` = the item type).
pub fn equipment_item(rng: &mut MsvcRand, level: i32, rarity: u8) -> Item {
    // rand @0x0052a7ca
    let mut it = if rng.rand() % 2 == 0 { armor_item(rng, level, rarity) } else { weapon_item(rng, level, rarity) };
    it.f8 = u32::from(it.item_type);
    it.item_type = 2;
    it
}

/// `FUN_0052b470(level, maxRarity)`: random loot item, one of 25 materials/crafting items or an
/// equipment formula.
pub fn loot_item(rng: &mut MsvcRand, level: i32, max_rarity: i32) -> Item {
    let n = max_rarity + 1;
    let mut list = Vec::with_capacity(26);
    let mut it = Item::NEW;
    it.level = level as u16;
    let roll = |rng: &mut MsvcRand, it: &mut Item, list: &mut Vec<Item>| {
        it.rarity = rarity_roll(rng, n).min(4) as u8;
        list.push(*it);
    };
    for _ in 0..4 {
        rng.rand(); // rand @0x0052b529, 0x0052b52b, 0x0052b52d, 0x0052b52f (discarded)
    }
    it.material = 1;
    it.item_type = 3;
    it.sub_type = (rng.rand() % 3) as u8; // rand @0x0052b53f
    roll(rng, &mut it, &mut list);
    for _ in 0..5 {
        rng.rand(); // rand @0x0052b5a6, 0x0052b5a8, 0x0052b5aa, 0x0052b5ac, 0x0052b5ae (discarded)
    }
    it.sub_type = 0xd;
    roll(rng, &mut it, &mut list);
    it.material = 2;
    for s in [6, 7, 10, 11] {
        it.sub_type = s;
        roll(rng, &mut it, &mut list);
    }
    it.item_type = 7;
    it.sub_type = 0;
    it.material = 1;
    roll(rng, &mut it, &mut list);
    for k in [4, 5, 6] {
        it.item_type = k;
        roll(rng, &mut it, &mut list);
    }
    for m in [0x19, 0x1a, 0x1b] {
        it.material = m;
        for k in [7, 7, 4, 5, 6] {
            it.item_type = k;
            roll(rng, &mut it, &mut list);
        }
    }
    list.push(equipment_item(rng, level, max_rarity as u8));
    let r = rng.rand() as u32; // rand @0x0052becf
    list[(r % list.len() as u32) as usize]
}

/// `FUN_004fdd80(inventory, level)`: the ten misc items (`FUN_0052c160`) it fills page 0 with.
pub fn trap_inventory(rng: &mut MsvcRand, level: i32) -> Vec<Item> {
    (0..10).map(|_| misc_item(rng, level)).collect()
}

/// `makeWallLight(style, pos, angle)`, `Server.exe 0x0052c370`.
pub fn wall_light(rng: &mut MsvcRand, style: i32, pos: [i64; 3], angle: f32) -> Prop {
    let mut p = Prop { x: pos[0], y: pos[1], z: pos[2], rotation: angle, scale: 0.0625, ..Prop::NEW };
    let warm = |p: &mut Prop| {
        p.kind = 0x36;
        p.flags = 1;
        p.f2c = [f32::from_bits(0x3f4ccccd), f32::from_bits(0x3f333333), f32::from_bits(0x3e4ccccd)];
    };
    match style {
        0 => {
            // rand @0x0052c418
            if rng.rand() % 2 == 0 {
                p.kind = 0x32;
                p.z += 0x20000;
            } else {
                warm(&mut p);
            }
        }
        1 | 2 => match rng.rand() % 3 {
            // rand @0x0052c473
            0 => p.kind = 0x32,
            1 => p.kind = 0x31,
            _ => {
                p.kind = 0x34;
                p.flags = 1;
                p.f2c = [0.0, 0.5, f32::from_bits(0x3dcccccd)];
            }
        },
        3 => p.kind = 0x30,
        4 => {
            // rand @0x0052c3dd
            if rng.rand() % 2 != 0 {
                p.kind = (rng.rand() % 4 + 0x2c) as u32; // rand @0x0052c3f1
            } else {
                warm(&mut p);
            }
        }
        5 => warm(&mut p),
        _ => {}
    }
    p
}

/// `makeFurniture(pos, dir, zoneStyle)`, `Server.exe 0x0052a830`: a table, shelf or stool
/// against a wall, or (1 in 50) a small chest.
pub fn make_furniture(rng: &mut MsvcRand, pos: [i64; 3], dir: i32, zstyle: i32) -> Static {
    let mut s = Static { x: pos[0], y: pos[1], z: pos[2], rotation: dir, ..Static::NEW };
    // rand @0x0052a90c
    if rng.rand() % 50 != 0 {
        // rand @0x0052a9e0
        match rng.rand() % 3 {
            0 => {
                let r = rng.rand(); // rand @0x0052ad02 / 0x0052ad1a / 0x0052ad32 / 0x0052ad45
                s.kind = (r % 3
                    + match zstyle {
                        3 => 0x20,
                        4 => 0x26,
                        5 => 0x29,
                        _ => 0x23,
                    }) as u32;
                s.scale = [2.0, 1.0, f32::from_bits(0x3fc8f5c3)];
            }
            1 => {
                s.kind = if zstyle == 4 { 0xd } else { (i32::from(zstyle == 5) * 2 + 0xc) as u32 };
                s.scale = [3.0, 3.0, 1.0];
                if (0..4).contains(&dir) {
                    let off = ((rng.rand() as f32 / 32767.0f32 + 1.0f32) * 65536.0f32) as i64; // rand @0x0052abd5 / 0x0052ac0d / 0x0052ac5a / 0x0052acad
                    offset_by_dir(&mut s, dir, off);
                }
            }
            _ => {
                s.kind = if zstyle == 4 { 0xf } else { (i32::from(zstyle == 5) + 0x10) as u32 };
                s.scale = [1.0, 1.0, 0.5];
                if (0..4).contains(&dir) {
                    let off = (rng.rand() as f32 / 32767.0f32 * 65536.0f32) as i64; // rand @0x0052aa5b / 0x0052aaa6 / 0x0052aaeb / 0x0052ab36
                    offset_by_dir(&mut s, dir, off);
                }
            }
        }
    } else {
        s.kind = 10;
        s.scale = [1.5, 1.0, 1.0];
        s.rotation = (dir + 2) % 4;
        match dir {
            0 => s.y += -32768,
            1 => s.x += 32768,
            2 => s.y += 32768,
            3 => s.x += -32768,
            _ => {}
        }
    }
    s
}

fn offset_by_dir(s: &mut Static, dir: i32, off: i64) {
    match dir {
        0 => s.y -= off,
        1 => s.x += off,
        2 => s.y += off,
        _ => s.x -= off,
    }
}

// ---------------------------------------------------------------------------------------------
// Block writers.

const CLEAR_BLOCK: Block = [0, 0, 0, 0xc0];

impl World {
    /// The column of `(x, y)` as `getColumn(x, y, zone)` finds it (only the zone's own columns).
    #[inline]
    fn dungeon_column_top(zone: &Zone, x: i32, y: i32) -> i32 {
        zone.top(x, y)
    }

    /// `mossFactor(x, y, z, zone)`, `Server.exe 0x00523b90`: 0..1 from climate A and noise.
    fn dungeon_moss_factor(&self, zone: &Zone, x: i32, y: i32, z: i32) -> f32 {
        let c = if zone.contains(x, y) { zone.column(x, y).climate_a } else { self.climate_a(x, y) };
        let n1 = noise(f64::from(y as f32 * 0.2f32 + 534.0f32), f64::from(z as f32 * 0.2f32 + 13.0f32)) * 0.1f32;
        let n2 = noise(f64::from(x) * 0.05 + 4343.0, f64::from(z) * 0.1 + 84734.0);
        let f = (n1 + n2) * 0.7f32 + 0.2f32;
        let v = f * c;
        v.clamp(0.0, 1.0)
    }

    /// `fillDungeonBox(x, y, z, size, rgb, noiseThr, zone, flags)`, `Server.exe 0x00513400`:
    /// jittered, moss-tinted type-6 blocks, with a `1 - thr` share of type-13 blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn dungeon_fill(&mut self, zone: &mut Zone, x: i32, y: i32, z: i32, size: [i32; 3], rgb: [u8; 3], thr: f32, flags: u8) {
        let mut tab = [0i32; 23];
        for (i, t) in tab.iter_mut().enumerate() {
            if i & 7 == 0 {
                *t = self.rng.rand() % 20; // rand @0x00513438
            }
        }
        for dx in 0..size[0] {
            for dy in 0..size[1] {
                let mut dz = size[2] - 1;
                while dz >= 0 {
                    let a = 7 * dz + dx;
                    let b = 7 * dz + dy;
                    let j = tab[((a / 2 + (b / 2) * 7) % 23) as usize];
                    let r = (i32::from(rgb[0]) + j) as f32;
                    let g0 = (i32::from(rgb[1]) + j) as f32;
                    let bl = (i32::from(rgb[2]) + j) as f32;
                    let m = self.dungeon_moss_factor(zone, x + dx, y + dy, z + dz);
                    let g = m * (120.0f32 - g0) + g0;
                    let rv = self.rng.rand(); // rand @0x0051359d
                    let kind = if thr < rv as f32 / 32767.0f32 { flags | 0x46 } else { flags | 0x4d };
                    let mut c = [r, g, bl];
                    for v in c.iter_mut() {
                        *v = v.clamp(0.0, 255.0);
                    }
                    let block = [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, kind];
                    set_block(self, zone, x + dx, y + dy, z + dz, block);
                    dz -= 1;
                }
            }
        }
    }

    /// `clearBox(x, y, z, size, zone, decor)`, `Server.exe 0x004d2500`: sets `{0,0,0,0xc0}`; with
    /// `decor`, the faces also get noise-shaped niches one block outside the box.
    #[allow(clippy::too_many_arguments)]
    pub fn dungeon_clear(&self, zone: &mut Zone, x: i32, y: i32, z: i32, size: [i32; 3], decor: bool) {
        for dx in 0..size[0] {
            let px = x + dx;
            for dy in 0..size[1] {
                let py = y + dy;
                let mut dz = size[2] - 1;
                while dz >= 0 {
                    let pz = z + dz;
                    set_block(self, zone, px, py, pz, CLEAR_BLOCK);
                    if decor {
                        let zf = f64::from(pz) * 0.2;
                        if dx == 0 && 0.5 < noise(f64::from(py) * 0.1 + f64::from(px.wrapping_mul(13)), zf) {
                            set_block(self, zone, px - 1, py, pz, CLEAR_BLOCK);
                        }
                        if dy == 0 && 0.5 < noise(f64::from(px) * 0.1 + f64::from(py.wrapping_mul(13)), zf) {
                            set_block(self, zone, px, py - 1, pz, CLEAR_BLOCK);
                        }
                        if dx == size[0] - 1 && 0.5 < noise(f64::from(py) * 0.1 + f64::from(px.wrapping_mul(13)), zf) {
                            set_block(self, zone, px + 1, py, pz, CLEAR_BLOCK);
                        }
                        if dy == size[1] - 1 && 0.5 < noise(f64::from(px) * 0.1 + f64::from(py.wrapping_mul(13)), zf) {
                            set_block(self, zone, px, py + 1, pz, CLEAR_BLOCK);
                        }
                    }
                    dz -= 1;
                }
            }
        }
    }

    /// `fillFlat(x, y, z, size, rgb, zone)`, `Server.exe 0x004ff340`: plain type-6 blocks.
    pub fn dungeon_flat(&self, zone: &mut Zone, x: i32, y: i32, z: i32, size: [i32; 3], rgb: [u8; 3]) {
        for dx in 0..size[0] {
            for dy in 0..size[1] {
                let mut dz = size[2] - 1;
                while dz >= 0 {
                    set_block(self, zone, x + dx, y + dy, z + dz, [rgb[0], rgb[1], rgb[2], 0x46]);
                    dz -= 1;
                }
            }
        }
    }

    /// `generateDungeon(zone, region, bx, by, style, special)`, `Server.exe 0x00500300`. The
    /// region is the one holding the zone (`zone.record` is its record for this zone); the
    /// original does not read it. Entities that [`Zone`] cannot hold are dropped; use
    /// [`World::generate_dungeon_ext`] to keep them.
    pub fn generate_dungeon(&mut self, zone: &mut Zone, bx: i32, by: i32, style: i32, special: bool) {
        let mut extras = DungeonExtras::default();
        self.generate_dungeon_ext(zone, &mut extras, bx, by, style, special);
    }

    /// [`World::generate_dungeon`] collecting the props, markers and extra spawn/static/item
    /// fields in `extras`.
    pub fn generate_dungeon_ext(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, bx: i32, by: i32, style: i32, special: bool) {
        let level = zone.record.level; // zone+0x80
        let tier = i32::from(zone.record.byte0c); // zone+0x84
        let fstyle = i32::from(zone.record.sub); // zone+0x79

        // ---- Phase 0: monster species.
        let mut singles: Vec<i32> = Vec::new();
        let mut groups: Vec<(Vec<i32>, Vec<i32>)> = Vec::new();
        match style {
            1 | 2 => {
                singles.extend([15, 16]);
                groups.push((vec![15, 16], vec![96]));
                // rand @0x005005f0
                let r = self.rng.rand() % 3;
                let leader = match r {
                    1 => 94,
                    2 => 17,
                    _ => 97,
                };
                groups.push((vec![leader], vec![]));
            }
            3 => {
                singles.extend([2, 3]);
                groups.push((vec![2, 3], vec![19]));
            }
            5 => {
                singles.extend([78, 77]);
                groups.push((vec![17, 81], vec![62, 30]));
            }
            _ => {
                singles.extend([11, 12]);
                groups.push((vec![46], vec![19]));
            }
        }

        // ---- Phase 1: 27 room records.
        #[derive(Clone, Copy, Default)]
        struct Room {
            exists: bool,
            boss: bool,
            no_monsters: bool,
            fixed: bool,
            o: [i32; 3],
            s: [i32; 3],
        }
        let mut rooms = [Room::default(); 27];
        for r in rooms.iter_mut() {
            let r1 = self.rng.rand(); // rand @0x005007f8
            let r2 = self.rng.rand(); // rand @0x005007ff
            let r3 = self.rng.rand(); // rand @0x00500807
            let r4 = self.rng.rand(); // rand @0x00500834
            let r5 = self.rng.rand(); // rand @0x00500841
            let r6 = self.rng.rand(); // rand @0x00500852
            r.s[2] = r1 % 3;
            r.s[1] = r2 % 3 - 1;
            r.s[0] = r3 % 3 - 1;
            r.o[2] = r4 % 3 - 1;
            r.o[1] = r5 % 2;
            r.o[0] = r6 % 2;
        }

        // ---- Phase 2: room layout and link list.
        let mut links: Vec<([i32; 3], [i32; 3])> = Vec::new();
        let rng = &mut self.rng;
        let rb = |rng: &mut MsvcRand| rng.rand() % 2 == 0;
        if special {
            for i in [13, 16, 4, 22, 1, 19] {
                rooms[i].exists = true;
            }
            links.extend([
                ([1, 1, 1], [1, 2, 1]),
                ([1, 1, 1], [0, 1, 1]),
                ([1, 1, 1], [2, 1, 1]),
                ([0, 1, 1], [0, 0, 1]),
                ([2, 1, 1], [2, 0, 1]),
            ]);
        } else if style == 4 || style == 5 {
            rng.rand(); // rand @0x0050195e (discarded)
            rooms[14].exists = true;
            rooms[14].no_monsters = true;
            rooms[14].fixed = true;
            for i in [16, 25, 22, 19, 7] {
                rooms[i].exists = true;
            }
            rooms[4].exists = rb(rng); // rand @0x00501990
            rooms[0].exists = true;
            rooms[3].exists = true;
            rooms[6].exists = rb(rng); // rand @0x005019b3
            rooms[12].exists = true;
            rooms[12].boss = true;
            links.extend([
                ([1, 1, 2], [1, 2, 1]),
                ([1, 2, 1], [0, 2, 1]),
                ([1, 2, 1], [2, 2, 1]),
                ([0, 2, 1], [0, 1, 1]),
                ([2, 2, 1], [2, 1, 1]),
                ([2, 1, 1], [2, 0, 1]),
                ([2, 0, 1], [0, 0, 0]),
                ([0, 0, 0], [0, 1, 0]),
                ([0, 1, 0], [0, 2, 0]),
                ([0, 1, 0], [1, 1, 0]),
            ]);
        } else if style != 2 {
            let r = rng.rand() % 3; // rand @0x00500e94
            rooms[10].exists = true;
            if r == 1 {
                rooms[19].exists = rb(rng); // rand @0x005015f0
                rooms[13].exists = true;
                rooms[4].exists = true;
                rooms[1].exists = rb(rng); // rand @0x00501613
                rooms[7].exists = true;
                rooms[16].exists = true;
                rooms[25].exists = true;
                rooms[25].boss = true;
                rooms[22].exists = rb(rng); // rand @0x0050163c
                links.extend([
                    ([1, 0, 1], [1, 1, 1]),
                    ([1, 0, 1], [2, 0, 1]),
                    ([1, 1, 1], [0, 1, 1]),
                    ([0, 1, 1], [0, 0, 1]),
                    ([0, 1, 1], [0, 2, 1]),
                    ([0, 2, 1], [1, 2, 1]),
                    ([1, 2, 1], [2, 2, 1]),
                    ([2, 2, 1], [2, 1, 1]),
                ]);
            } else if r == 2 {
                rooms[19].exists = rb(rng); // rand @0x00501221
                rooms[1].exists = true;
                rooms[4].exists = true;
                rooms[7].exists = rb(rng); // rand @0x00501244
                rooms[13].exists = true;
                rooms[22].exists = true;
                rooms[25].exists = rb(rng); // rand @0x00501267
                rooms[16].exists = true;
                rooms[16].boss = true;
                links.extend([
                    ([1, 0, 1], [0, 0, 1]),
                    ([1, 0, 1], [2, 0, 1]),
                    ([2, 0, 1], [2, 1, 1]),
                    ([0, 0, 1], [0, 1, 1]),
                    ([0, 1, 1], [1, 1, 1]),
                    ([0, 1, 1], [0, 2, 1]),
                    ([1, 1, 1], [1, 2, 1]),
                    ([1, 1, 1], [2, 1, 1]),
                    ([2, 1, 1], [2, 2, 1]),
                ]);
            } else {
                rooms[19].exists = rb(rng); // rand @0x00500eb3
                rooms[1].exists = true;
                rooms[4].exists = true;
                rooms[7].exists = rb(rng); // rand @0x00500ed6
                rooms[13].exists = true;
                rooms[16].exists = rb(rng); // rand @0x00500ef2
                rooms[22].exists = true;
                rooms[25].exists = true;
                rooms[25].boss = true;
                links.extend([
                    ([1, 0, 1], [0, 0, 1]),
                    ([1, 0, 1], [2, 0, 1]),
                    ([0, 0, 1], [0, 1, 1]),
                    ([0, 1, 1], [1, 1, 1]),
                    ([0, 1, 1], [0, 2, 1]),
                    ([1, 1, 1], [1, 2, 1]),
                    ([1, 1, 1], [2, 1, 1]),
                    ([2, 1, 1], [2, 2, 1]),
                ]);
            }
        } else {
            rng.rand(); // rand @0x00500b19 (discarded)
            for i in [11, 16, 25, 22, 19, 7] {
                rooms[i].exists = true;
            }
            rooms[4].exists = rb(rng); // rand @0x00500b42
            rooms[0].exists = true;
            rooms[3].exists = true;
            rooms[6].exists = rb(rng); // rand @0x00500b65
            rooms[12].exists = true;
            rooms[12].boss = true;
            links.extend([
                ([1, 0, 2], [1, 2, 1]),
                ([1, 2, 1], [0, 2, 1]),
                ([1, 2, 1], [2, 2, 1]),
                ([0, 2, 1], [0, 1, 1]),
                ([2, 2, 1], [2, 1, 1]),
                ([2, 1, 1], [2, 0, 1]),
                ([2, 0, 1], [0, 0, 0]),
                ([0, 0, 0], [0, 1, 0]),
                ([0, 1, 0], [0, 2, 0]),
                ([0, 1, 0], [1, 1, 0]),
            ]);
        }

        // ---- Phase 3: carve rooms and corridors.
        let mut d = Dungeon::new(22, 22, 22);
        let ridx = |g: [i32; 3]| ((g[0] * 3 + g[1]) * 3 + g[2]) as usize;
        for gx in 0..3 {
            for gy in 0..3 {
                for gz in 0..3 {
                    let r = &mut rooms[(gx * 9 + gy * 3 + gz) as usize];
                    if !r.exists {
                        continue;
                    }
                    if r.fixed {
                        r.o = [0, 0, 0];
                        r.s = [0, 0, 1];
                    }
                    let pos = [gx * 7 + r.o[0] + 2, gy * 7 + r.o[1] + 2, gz * 7 + r.o[2] + 3];
                    let size = [r.s[0] + 3, r.s[1] + 3, r.s[2] + 1];
                    // rand @0x00501f29 (only when neither fixed nor boss)
                    let pillars = !r.fixed && !r.boss && self.rng.rand() % 3 == 0;
                    d.carve_room(&mut self.rng, pos, size, pillars);
                    if r.boss {
                        d.or_flags(size[0] / 2 + pos[0], size[1] / 2 + pos[1], size[2] / 2 + pos[2], 4);
                    }
                }
            }
        }
        let room_box = |r: &Room, g: [i32; 3]| -> ([i32; 3], [i32; 3]) {
            ([r.o[0] + 2 + g[0] * 7, r.o[1] + 2 + g[1] * 7, r.o[2] + 3 + g[2] * 7], [r.s[0] + 3, r.s[1] + 3, r.s[2] + 1])
        };
        for &(a, b) in &links {
            let (ra, rbm) = (rooms[ridx(a)], rooms[ridx(b)]);
            if ra.exists && rbm.exists {
                let (pa, sa) = room_box(&ra, a);
                let (pb, sb) = room_box(&rbm, b);
                d.carve_corridor(&mut self.rng, pa, sa, pb, sb);
            }
        }
        {
            let a = links[0].0;
            let r = rooms[ridx(a)];
            d.set_type(a[0] * 7 + 2 + r.o[0], a[1] * 7 + 1 + r.o[1], r.o[2] + a[2] * 7 + 3, 4);
        }

        // ---- Phase 4: orientation and palette.
        let rot0 = self.rng.rand() % 4; // rand @0x005022d1
        d.rotation = rot0;
        d.mirror = self.rng.rand() % 2 == 0; // rand @0x005022eb
        let roof2 = (self.rng.rand() % 50 + 20) as u8; // rand @0x00502310
        let roof1 = (self.rng.rand() % 50 + 20) as u8; // rand @0x00502324
        let roof0 = (self.rng.rand() % 50 + 20) as u8; // rand @0x0050232c
        let roof = [roof0, roof1, roof2];
        let mut base = self.rng.rand() % 50; // rand @0x00502351
        if style == 3 {
            base = self.rng.rand() % 50 + 100; // rand @0x00502368
        }
        let bc = base as u8;
        let col = |rng: &mut MsvcRand| ((rng.rand() % 50) as u8).wrapping_add(50).wrapping_add(bc);
        let w2 = col(&mut self.rng); // rand @0x0050237b
        let w1 = col(&mut self.rng); // rand @0x00502391
        let w0 = col(&mut self.rng); // rand @0x005023a4
        let f2 = col(&mut self.rng); // rand @0x005023ce
        let f1 = col(&mut self.rng); // rand @0x005023e8
        let f0 = col(&mut self.rng); // rand @0x005023f6
        let mut wall = [w0, w1, w2];
        let mut floor = [f0, f1, f2];
        let mut light = [f32::from_bits(0x3f19999a), 0.5f32, f32::from_bits(0x3dcccccd)];
        let mut thr = f32::from_bits(0x3ba3d70a); // 0.005
        let mut item_sub: u8 = 0;
        match style {
            1 | 2 => {
                item_sub = 1;
                light = [0.0, 0.5, f32::from_bits(0x3dcccccd)];
                let a = (self.rng.rand() % 50 + 50) as u8; // rand @0x00502493
                let b = (self.rng.rand() % 50 + 50) as u8; // rand @0x005024a7
                let c = (self.rng.rand() % 20 + 50) as u8; // rand @0x005024af
                wall = [c, b, a];
                let a = (self.rng.rand() % 50 + 50) as u8; // rand @0x005024e2
                let b = (self.rng.rand() % 50 + 50) as u8; // rand @0x005024f6
                let c = (self.rng.rand() % 20 + 50) as u8; // rand @0x005024fe
                floor = [c, b, a];
                thr = f32::from_bits(0x3ac49ba6); // 0.0015
            }
            4 => {
                let a = (self.rng.rand() % 50 + 20) as u8; // rand @0x00502546
                let b = (self.rng.rand() % 50 + 80) as u8; // rand @0x0050255a
                let c = (self.rng.rand() % 50 + 100) as u8; // rand @0x00502562
                wall = [c, b, a];
                let a = (self.rng.rand() % 50 + 20) as u8; // rand @0x00502595
                let b = (self.rng.rand() % 50 + 80) as u8; // rand @0x005025a9
                let c = (self.rng.rand() % 50 + 100) as u8; // rand @0x005025b1
                floor = [c, b, a];
                thr = f32::from_bits(0x3b23d70a); // 0.0025
            }
            5 => {
                wall = [0xff, 0xd2, 0x82];
                floor = [0xc8, 0xb4, 0x50];
            }
            _ => {}
        }
        // `world->models[2544 or 2545]` when the table is that long, else null (no placement).
        let models = std::sync::Arc::clone(&self.models);
        let model = models.get(if style == 4 || style == 5 { 2545 } else { 2544 });

        // ---- Phase 5: pillars and filler above rooms (styles 0..3).
        if matches!(style, 0..=3) {
            let odd = rot0 % 2 != 0;
            let (nx, ny) = if odd { (d.sy, d.sx) } else { (d.sx, d.sy) };
            for x in 0..nx {
                for y in 0..ny {
                    let mut h = 1;
                    if style != 2 {
                        // rand @0x00502709
                        if self.rng.rand() % 6 == 0 {
                            h = self.rng.rand() % 3 + 2; // rand @0x00502717
                        }
                    }
                    let mut z = d.sz - 1;
                    while z >= 0 {
                        if d.cell(x, y, z)[0] != 4 && d.cell(x, y, z)[0] != 0 {
                            let top = (h + z).min(d.sz - 1);
                            for zz in z + 1..=top {
                                d.set_type(x, y, zz, 2);
                            }
                            break;
                        }
                        z -= 1;
                    }
                }
            }
        }

        // ---- Phase 6: rotation with the lowest entrance.
        let mut best = -1;
        let mut best_base = 0;
        let mut best_e = [0i32; 3];
        let mut best_water = false;
        let mut rot = rot0;
        for _ in 0..4 {
            rot = (rot + 1) % 4;
            d.rotation = rot;
            let mut e = [0i32; 3];
            let mut z = d.sz - 1;
            while z >= 0 {
                for x in 0..d.dim_x() {
                    for y in 0..d.dim_y() {
                        if d.cell(x, y, z)[0] == 4 {
                            e = [x, y, z];
                        }
                    }
                }
                z -= 1;
            }
            let cx = bx + e[0] * 10 + 5;
            let cy = by + e[1] * 10 + 5;
            if !zone.contains(cx, cy) {
                return;
            }
            let mut h = zone.top(cx, cy);
            while zone.block(cx, cy, h)[3] & 0x1f == 0 {
                h -= 1;
            }
            let mut h1 = h + 1;
            let mut water = false;
            for dx in 0..10 {
                for dy in 0..10 {
                    for dz in 0..10 {
                        let t = zone.block(bx + e[0] * 10 + dx, by + e[1] * 10 + dy, h1 + dz - 5)[3] & 0x1f;
                        if t == 2 || t == 3 {
                            water = true;
                        }
                    }
                }
            }
            if water {
                h1 += 100;
            }
            let cand = h1 - e[2] * 10;
            if best < 0 || cand < best_base {
                best = rot;
                best_e = e;
                best_water = water;
                best_base = cand;
            }
        }
        let mut base_z = best_base;
        if style == 4 || style == 5 {
            base_z += 50;
        }
        if best_water {
            base_z -= 100;
        }
        if base_z + best_e[2] * 10 < 0 {
            base_z = -(best_e[2] * 10);
        }
        if best < 0 {
            best = 0;
        }
        d.rotation = best;
        let ez = best_e[2];

        let c2 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c09
        let c1 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c1d
        let c0 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c25
        let carpet = [c0, c1, c2];
        let b2 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c4a
        let b1 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c5e
        let b0 = (self.rng.rand() % 180 + 50) as u8; // rand @0x00502c66
        let border = [b0, b1, b2];
        if style == 0 || style == 3 {
            self.rng.rand(); // rand @0x00502ca3 (discarded)
        }

        // ---- Phase 7: shell, roofs and foundations per cell column.
        let (dim_x, dim_y, dim_z) = (d.dim_x(), d.dim_y(), d.dim_z());
        for x in 0..dim_x {
            for y in 0..dim_y {
                let x0 = bx + x * 10;
                let y0 = by + y * 10;
                let mut found = false;
                let mut top_z = 0;
                let mut min_top = Self::dungeon_column_top(zone, x0, y0);
                for dx in 0..10 {
                    for dy in 0..10 {
                        min_top = min_top.min(Self::dungeon_column_top(zone, x0 + dx, y0 + dy));
                    }
                }
                let mut entr = false;
                let mut z = dim_z - 1;
                while z >= 0 {
                    let c = d.cell(x, y, z)[0];
                    if c != 0 {
                        if !found {
                            top_z = base_z + (z + 1) * 10;
                            if c == 4 {
                                top_z -= 10;
                                entr = true;
                            } else {
                                if min_top < z * 10 + 5 + base_z {
                                    self.dungeon_shell_top(zone, &d, x, y, z, x0, y0, base_z, style, wall, roof);
                                }
                                self.dungeon_fill(zone, x0 - 2, y0 - 2, base_z + z * 10 - 2, [14, 14, 14], wall, thr, 0);
                            }
                        } else if c == 4 {
                            entr = true;
                        } else {
                            self.dungeon_fill(zone, x0 - 2, y0 - 2, base_z + z * 10 - 2, [14, 14, 14], wall, thr, 0);
                        }
                        found = true;
                    }
                    z -= 1;
                }
                if !found && !(style == 4 || style == 5) {
                    continue;
                }
                if !zone.contains(x0, y0) {
                    continue;
                }
                let mut min_base = zone.column(x0, y0).height;
                for dx in -2..12 {
                    for dy in -2..12 {
                        if zone.contains(x0 + dx, y0 + dy) {
                            let h = zone.column(x0 + dx, y0 + dy).height;
                            if h < min_base {
                                min_base = h;
                            }
                        }
                    }
                }
                if matches!(style, 0..=3) {
                    let fl = if entr { 0x80 } else { 0 };
                    self.dungeon_fill(zone, x0 - 2, y0 - 2, min_base, [14, 14, top_z - min_base], wall, thr, fl);
                } else if style == 4 {
                    self.dungeon_ziggurat(zone, &d, x, y, x0, y0, min_base, base_z, ez, wall, thr);
                }
            }
        }
        if style == 5 {
            let wy = d.dim_y() * 10;
            let half_x = (d.dim_x() * 10) / 2;
            let cxc = bx + half_x - 5;
            let cyc = by + wy / 2 - 5;
            let x_end = d.dim_x() * 10 + bx;
            for px in bx..x_end {
                for py in by..by + wy {
                    if !zone.contains(px, py) {
                        continue;
                    }
                    let h = zone.column(px, py).height;
                    let dd = (py - cyc).abs().max((px - cxc).abs());
                    let sz = base_z + (((ez * 10 - dd) - 51 + half_x) - h);
                    self.dungeon_fill(zone, px, py, h, [1, 1, sz], wall, thr, 0);
                }
            }
            // A surface scan at the centre follows in the original; it has no effect.
        }

        // ---- Phase 8: interiors.
        for x in 0..dim_x {
            for y in 0..dim_y {
                for z in 0..dim_z {
                    let cell = d.cell(x, y, z);
                    let t = cell[0];
                    let fl = cell[1];
                    if t != 3 && t != 4 {
                        continue;
                    }
                    let x0 = bx + x * 10;
                    let y0 = by + y * 10;
                    let zc = base_z + z * 10;
                    if t == 4 {
                        extras.markers.push(Marker {
                            kind: 5,
                            spawn_index: -1,
                            creature_type: -1,
                            x: from_block(x0 + 5),
                            y: from_block(y0 + 5),
                            z: from_block(zc),
                        });
                    }
                    // 1. Carve.
                    if matches!(style, 0..=5) {
                        let decor = style == 4 || style == 2 || style == 1;
                        self.dungeon_clear(zone, x0, y0, zc, [10, 10, 10], decor);
                    }
                    // 2. Doors.
                    if (style == 0 || style == 3 || style == 1) && t != 4 && fl & 1 == 0 {
                        self.dungeon_doors(zone, &d, x, y, z, x0, y0, zc);
                    }
                    // 3. Ramps.
                    if matches!(style, 0..=5) && fl & 1 != 0 {
                        self.dungeon_ramps(zone, &d, x, y, z, x0, y0, zc, floor, thr);
                    }
                    if !(style == 4 && t == 4) {
                        if d.cell(x, y, z - 1)[0] != 3 {
                            // 4. Floor and carpet.
                            if matches!(style, 0..=5) {
                                self.dungeon_fill(zone, x0, y0, zc, [10, 10, 1], floor, thr, 0);
                                if (style == 0 || style == 3) && fl & 2 != 0 {
                                    self.dungeon_carpet(zone, &d, x, y, z, x0, y0, zc, carpet, border);
                                }
                            }
                            // 5. Chest or decoration.
                            if t != 4 && fl & 1 == 0 {
                                // rand @0x005058ed (only when not special)
                                if !special && self.rng.rand() % 40 == 0 {
                                    let mut s = Static { kind: 7, x: from_block(x0 + 5), y: from_block(y0 + 5), z: from_block(zc + 1), ..Static::NEW };
                                    s.scale = [10.0, 10.0, 10.0];
                                    s.rotation = self.rng.rand() % 4; // rand @0x005059b9
                                    s.f34 = (self.rng.rand() % 4000) as u32; // rand @0x005059d7
                                    zone.statics.push(s);
                                } else {
                                    let ctx = DecoCtx { style, fstyle, level, tier, item_sub, x0, y0, zc, base_z };
                                    self.dungeon_decorate(zone, extras, &d, x, y, z, &ctx);
                                }
                            }
                        }
                        // 6. Ceiling models and lanterns (LAB_00507401).
                        if t != 4 && d.cell(x, y, z + 1)[0] != 3 {
                            let ztop = base_z + (z + 1) * 10;
                            if let Some(m) = model
                                && is_solid(zone.block(x0, y0, ztop))
                            {
                                // -x (rot 0), +x (rot 2), -y (rot 3), +y (rot 1).
                                let sides = [(-1, 0, 0), (1, 0, 2), (0, -1, 3), (0, 1, 1)];
                                for (ddx, ddy, dir) in sides {
                                    if d.cell(x + ddx, y + ddy, z)[0] != 3 {
                                        self.place_model(zone, m, [x0, y0, zc], dir, 0x46, 0, false, [0; 4]);
                                    }
                                }
                            }
                            // rand @0x00507760 (style 3 only)
                            if style == 3 && self.rng.rand() % 10 == 0 {
                                let p = Prop {
                                    kind: 0x38,
                                    x: from_block(x0 + 5) + fx_d(0.5),
                                    y: from_block(y0 + 5) + fx_d(0.5),
                                    z: fx_d(f64::from(ztop) - 2.7) + from_block(0),
                                    scale: f32::from_bits(0x3dcccccd),
                                    rotation: 0.0,
                                    f2c: light,
                                    flags: 1,
                                    ..Prop::NEW
                                };
                                zone.props.push(p);
                            }
                        }
                    }
                    // 7. Boss.
                    if fl & 4 != 0 {
                        self.dungeon_boss(zone, extras, &singles, x0, y0, zc, level, tier);
                    }
                }
            }
        }

        // ---- Phase 9: style-specific shapes.
        if style == 4 || style == 5 {
            let dx_n = d.dim_x();
            let cxc = dx_n / 2 - 1;
            let cyc = d.dim_y() / 2 - 1;
            let low = base_z + ez * 10 + 8;
            let mut top = if style == 4 {
                base_z + ((ez + dx_n / 2) * 5 - 33) * 2
            } else {
                base_z + ((ez + dx_n / 2) * 5 - 36) * 2
            };
            let bxc = bx + (cxc * 5 - 1) * 2;
            let byc = by + (cyc * 5 - 1) * 2;
            if style == 4 {
                self.dungeon_fill(zone, bxc, byc, top, [14, 14, 16], wall, thr, 0);
                self.dungeon_clear(zone, bx + cxc * 10, byc, top, [10, 14, 10], false);
                self.dungeon_clear(zone, bxc, by + cyc * 10, top, [14, 10, 10], false);
                self.dungeon_clear(zone, bx + cxc * 10, by + cyc * 10, low, [10, 10, top - low], false);
            } else {
                self.dungeon_fill(zone, bxc, byc, top, [14, 14, 14], wall, thr, 0);
                top -= 12;
                self.dungeon_clear(zone, bx + (cxc * 5 + 1) * 2, by + (cyc * 5 - 16) * 2, top, [6, 74, 6], false);
                self.dungeon_clear(zone, bx + (cxc * 5 - 16) * 2, by + (cyc * 5 + 1) * 2, top, [74, 6, 6], false);
                self.dungeon_clear(zone, bx + cxc * 10, by + cyc * 10, low, [10, 10, top + (6 - low)], false);
            }
            let off = 30 - low;
            let h = off + top;
            self.dungeon_fill(zone, bx + (cxc * 5 + 2) * 2, by + (cyc * 5 + 2) * 2, low - 30, [2, 2, h], wall, thr, 0);
            if 1 < h {
                let mut zz = low - 29;
                loop {
                    let (px, py, size) = match (off + zz) & 7 {
                        0 => (bx + cxc * 10, by + cyc * 10, [5, 5, 1]),
                        1 => (bx + cxc * 10 + 5, by + cyc * 10, [1, 5, 1]),
                        2 => (bx + cxc * 10 + 6, by + cyc * 10, [5, 5, 1]),
                        3 => (bx + cxc * 10 + 6, by + cyc * 10 + 5, [5, 1, 1]),
                        4 => (bx + cxc * 10 + 6, by + cyc * 10 + 6, [5, 5, 1]),
                        5 => (bx + cxc * 10 + 5, by + cyc * 10 + 6, [1, 5, 1]),
                        6 => (bx + cxc * 10, by + cyc * 10 + 6, [5, 5, 1]),
                        _ => (bx + cxc * 10, by + cyc * 10 + 5, [5, 1, 1]),
                    };
                    self.dungeon_fill(zone, px, py, zz, size, wall, thr, 0);
                    zz += 1;
                    if off + zz >= h {
                        break;
                    }
                }
            }
        } else if (style == 1 || style == 2) && 0 < d.dim_x() {
            for i in 0..d.dim_x() {
                for j in 0..d.dim_y() {
                    let x0 = bx + i * 10;
                    let y0 = by + j * 10;
                    for px in x0..x0 + 10 {
                        let pxd = f64::from(px);
                        for py in y0..y0 + 10 {
                            let pyd = f64::from(py);
                            let mut a = noise(pxd * 0.1, pyd * 0.1) * 2.0f32;
                            a += noise(pxd * 0.05, pyd * 0.05) * 8.0f32;
                            if a > 0.0 {
                                a = 0.0;
                            }
                            let top = zone.top(px, py);
                            let low = (top as f32 + a) as i32;
                            let mut h = top;
                            while low <= h {
                                let b = zone.block(px, py, h);
                                if b[3] & 0x40 != 0 && zone.block(px, py, h)[3] & 0x80 == 0 {
                                    set_block(self, zone, px, py, h, AIR_BLOCK);
                                }
                                h -= 1;
                            }
                        }
                    }
                }
            }
        }

        // ---- Phase 10: monsters per room.
        let mc = MonsterCtx { bx, by, base_z, level, tier, special };
        for gx in 0..3 {
            for gy in 0..3 {
                for gz in 0..3 {
                    let r = rooms[((gx * 3 + gy) * 3 + gz) as usize];
                    if !r.exists || r.no_monsters {
                        continue;
                    }
                    let w = r.s[0] + 3;
                    let hh = r.s[1] + 3;
                    let c0 = [r.o[0] + gx * 7 + 2, r.o[1] + gy * 7 + 2, r.o[2] + gz * 7 + 3];
                    let mut corners = [
                        c0,
                        [(w - 1) + c0[0], c0[1], c0[2]],
                        [(w - 1) + c0[0], hh - 1 + c0[1], c0[2]],
                        [c0[0], hh - 1 + c0[1], c0[2]],
                    ];
                    for c in corners.iter_mut() {
                        let (lx, ly) = d.to_logical(c[0], c[1]);
                        c[0] = lx;
                        c[1] = ly;
                    }
                    self.dungeon_room_monsters(zone, extras, &d, &mc, &corners, w, hh, r.boss, &singles, &groups);
                }
            }
        }
    }

    /// Phase 7 decoration of a cell that sticks out of the ground: style 2 cap, battlements
    /// (styles 0, 1, 3) and the style-3 pyramid roof.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_shell_top(&mut self, zone: &mut Zone, d: &Dungeon, x: i32, y: i32, z: i32, x0: i32, y0: i32, base_z: i32, style: i32, wall: [u8; 3], roof: [u8; 3]) {
        let open = |t: u8| t == 0 || t == 4 || t == 1;
        if style == 2 {
            self.dungeon_fill(zone, x0, y0, base_z + z * 10 + 11, [10, 10, 3], wall, 0.0, 0);
        } else if style == 0 || style == 3 || style == 1 {
            let top = base_z + (z + 1) * 10;
            let zz = top + 3;
            let crenel = |w: &mut World, zone: &mut Zone, cx: i32, cy: i32, fx_: i32, fy_: i32| {
                let t = zone.block(cx, cy, zz)[3] & 0x1f;
                if t == 0 || t == 2 {
                    set_block(w, zone, cx, cy, zz, [0, 0, 0, 0x40]);
                }
                w.dungeon_fill(zone, fx_, fy_, zz, [1, 1, 1], wall, 0.0, 0);
            };
            if open(d.cell(x - 1, y, z)[0]) && open(d.cell(x - 1, y, z + 1)[0]) {
                self.dungeon_fill(zone, x0 - 3, y0 - 3, top, [2, 16, 3], wall, 0.0, 0);
                let mut yy = y0 - 2;
                for _ in 0..8 {
                    crenel(self, zone, x0 - 3, yy, x0 - 3, yy - 1);
                    yy += 2;
                }
            }
            if open(d.cell(x + 1, y, z)[0]) && open(d.cell(x + 1, y, z + 1)[0]) {
                self.dungeon_fill(zone, x0 + 11, y0 - 3, top, [2, 16, 3], wall, 0.0, 0);
                let mut yy = y0 - 1;
                for _ in 0..8 {
                    crenel(self, zone, x0 + 12, yy, x0 + 12, yy - 1);
                    yy += 2;
                }
            }
            if open(d.cell(x, y - 1, z)[0]) && open(d.cell(x, y - 1, z + 1)[0]) {
                self.dungeon_fill(zone, x0 - 3, y0 - 3, top, [16, 2, 3], wall, 0.0, 0);
                let mut xx = x0 - 2;
                for _ in 0..8 {
                    crenel(self, zone, xx, y0 - 3, xx - 1, y0 - 3);
                    xx += 2;
                }
            }
            if open(d.cell(x, y + 1, z)[0]) && open(d.cell(x, y + 1, z + 1)[0]) {
                self.dungeon_fill(zone, x0 - 3, y0 + 11, top, [16, 2, 3], wall, 0.0, 0);
                let mut xx = x0 - 1;
                for _ in 0..8 {
                    crenel(self, zone, xx, y0 + 12, xx - 1, y0 + 12);
                    xx += 2;
                }
            }
            if style == 3
                && open(d.cell(x - 1, y, z)[0])
                && open(d.cell(x + 1, y, z)[0])
                && open(d.cell(x, y - 1, z)[0])
                && open(d.cell(x, y + 1, z)[0])
            {
                let mut xs = x0 - 2;
                let mut ys = y0 - 2;
                self.dungeon_fill(zone, xs, ys, top, [14, 14, 4], roof, 0.0, 0);
                let mut s = 14;
                let mut zs = base_z + z * 10 + 14;
                loop {
                    self.dungeon_fill(zone, xs, ys, zs, [s, s, 2], roof, 0.0, 0);
                    ys += 1;
                    s -= 2;
                    zs += 2;
                    xs += 1;
                    if s <= 0 {
                        break;
                    }
                }
            }
            // A surface scan at (x0 + 5, y0 + 5) follows in the original; it has no effect.
        }
    }

    /// Phase 7, style 4: the stepped pyramid foundation, its grass cap and the four stairways.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_ziggurat(&mut self, zone: &mut Zone, d: &Dungeon, x: i32, y: i32, x0: i32, y0: i32, min_base: i32, base_z: i32, ez: i32, wall: [u8; 3], thr: f32) {
        let cy = d.dim_y() / 2 - 1;
        let cx = d.dim_x() / 2 - 1;
        let ady = (y - cy).abs();
        let adx = (x - cx).abs();
        let dd = ady.max(adx);
        let ring = d.dim_x() / 2 - dd;
        let hh = if x == cx && y == cy { ring - 2 } else { ring - 1 };
        let mut t = base_z + (ez + hh - 5) * 10;
        if adx < 2 && ady < 2 {
            t += 2;
        }
        self.dungeon_fill(zone, x0 - 2, y0 - 2, min_base, [14, 14, t - 4 - min_base], wall, thr, 0);
        if t != min_base && t - min_base >= 0 {
            self.dungeon_fill(zone, x0 - 1, y0 - 1, t - 4, [12, 12, 3], wall, 0.0, 0);
            self.dungeon_fill(zone, x0, y0, t - 1, [10, 10, 3], wall, 0.0, 0);
            for dx in -2..12 {
                let px = x0 + dx;
                for dy in -2..12 {
                    let py = y0 + dy;
                    let mut zz = t + 2;
                    if dx == -2 || dx == 11 || dy == -2 || dy == 11 {
                        zz = t + 2 - 6;
                    } else if dx == -1 || dx == 10 || dy == -1 || dy == 10 {
                        zz = t + 2 - 3;
                    }
                    let n = noise(f64::from(px) * 0.02, f64::from(py) * 0.02);
                    if 0.0 < n {
                        let b = zone.block(px, py, zz);
                        if b[3] & 0x1f == 0 || b[3] & 0x1f == 2 {
                            let (ca, cb) = if zone.contains(px, py) {
                                let c = zone.column(px, py);
                                (c.climate_a, c.climate_b)
                            } else {
                                (self.climate_a(px, py), self.climate_b(px, py))
                            };
                            let blk = self.surface_block(px, py, zz, ca, cb);
                            set_block(self, zone, px, py, zz, blk);
                        }
                    }
                }
            }
        }
        if x == cx && ady >= 2 {
            let (start, step) = if y < cy { (y0 - 1, 1) } else { (y0 + 10, -1) };
            if y != cy {
                for (i, k) in (-1..10).enumerate() {
                    self.dungeon_fill(zone, x0 - 2, start + step * i as i32, t, [14, 1, k + 3], wall, thr, 0);
                }
                for (i, k) in (-1..10).enumerate() {
                    self.dungeon_fill(zone, x0 - 2, start + step * i as i32, t, [2, 1, k + 5], wall, thr, 0);
                }
                for (i, k) in (-1..10).enumerate() {
                    self.dungeon_fill(zone, x0 + 10, start + step * i as i32, t, [2, 1, k + 5], wall, thr, 0);
                }
            }
        }
        if y == cy && adx >= 2 && x != cx {
            let (start, step) = if x < cx { (x0 - 1, 1) } else { (x0 + 10, -1) };
            for (i, k) in (-1..10).enumerate() {
                self.dungeon_fill(zone, start + step * i as i32, y0 - 2, t, [1, 14, k + 3], wall, thr, 0);
            }
            for (i, k) in (-1..10).enumerate() {
                self.dungeon_fill(zone, start + step * i as i32, y0 - 2, t, [1, 2, k + 5], wall, thr, 0);
            }
            for (i, k) in (-1..10).enumerate() {
                self.dungeon_fill(zone, start + step * i as i32, y0 + 10, t, [1, 2, k + 5], wall, thr, 0);
            }
        }
    }

    /// Phase 8 step 2: doors on the four sides (-y, +y, -x, +x); each side always draws a rand.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_doors(&mut self, zone: &mut Zone, d: &Dungeon, x: i32, y: i32, z: i32, x0: i32, y0: i32, zc: i32) {
        let zd = zc + 2;
        // (neighbour, probe, clear origin, door position, rotation)
        let sides: [DoorSide; 4] = [
            ((x, y - 1), (x0 + 5, y0 - 3), (x0 + 4, y0 - 2), (x0 + 5, y0 - 1), 0), // rand @0x005049b5
            ((x, y + 1), (x0 + 5, y0 + 13), (x0 + 4, y0 + 10), (x0 + 5, y0 + 11), 2), // rand @0x00504b6c
            ((x - 1, y), (x0 - 3, y0 + 5), (x0 - 2, y0 + 4), (x0 - 1, y0 + 5), 3), // rand @0x00504d10
            ((x + 1, y), (x0 + 13, y0 + 5), (x0 + 10, y0 + 4), (x0 + 11, y0 + 5), 1), // rand @0x00504ebc
        ];
        for (nb, probe, clr, pos, rotation) in sides {
            if self.rng.rand() % 2 == 0 {
                continue;
            }
            if d.cell(nb.0, nb.1, z)[0] != 0 {
                continue;
            }
            if is_solid(zone.block(probe.0, probe.1, zd)) {
                continue;
            }
            self.dungeon_clear(zone, clr.0, clr.1, zd, [2, 2, 4], false);
            let mut s = Static { kind: 4, x: from_block(pos.0), y: from_block(pos.1), z: from_block(zd), rotation, ..Static::NEW };
            s.scale = [2.0, f32::from_bits(0x3e4ccccd), 4.0];
            zone.statics.push(s);
        }
    }

    /// Phase 8 step 3: ramps for cells flagged as stairs.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_ramps(&mut self, zone: &mut Zone, d: &Dungeon, x: i32, y: i32, z: i32, x0: i32, y0: i32, zc: i32, floor: [u8; 3], thr: f32) {
        let t = |dx: i32, dy: i32, dz: i32| d.cell(x + dx, y + dy, z + dz)[0];
        if t(1, 0, 0) != 3 && t(-1, 0, 0) == 3 && t(1, 0, 1) == 3 {
            for k in 0..11 {
                self.dungeon_fill(zone, x0 - 1 + k, y0, zc - 1 + k, [11 - k, 10, 1], floor, thr, 0);
            }
        }
        if t(-1, 0, 0) != 3 && t(1, 0, 0) == 3 && t(-1, 0, 1) == 3 {
            for k in 0..11 {
                self.dungeon_fill(zone, x0, y0, zc - 1 + k, [11 - k, 10, 1], floor, thr, 0);
            }
        }
        if t(0, 1, 0) != 3 && t(0, -1, 0) == 3 && t(0, 1, 1) == 3 {
            for k in 0..11 {
                self.dungeon_fill(zone, x0, y0 - 1 + k, zc - 1 + k, [10, 11 - k, 1], floor, thr, 0);
            }
        }
        if t(0, -1, 0) != 3 && t(0, 1, 0) == 3 && t(0, -1, 1) == 3 {
            for k in 0..11 {
                self.dungeon_fill(zone, x0, y0, zc - 1 + k, [10, 11 - k, 1], floor, thr, 0);
            }
        }
    }

    /// Phase 8 step 4: carpet with borders where the neighbouring cell has no carpet.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_carpet(&mut self, zone: &mut Zone, d: &Dungeon, x: i32, y: i32, z: i32, x0: i32, y0: i32, zc: i32, carpet: [u8; 3], border: [u8; 3]) {
        let (mut ax, mut bx_, mut ay, mut by_) = (x0, x0 + 10, y0, y0 + 10);
        let l = d.cell(x - 1, y, z)[1] & 2 == 0;
        if l {
            ax += 2;
        }
        let r = d.cell(x + 1, y, z)[1] & 2 == 0;
        if r {
            bx_ -= 2;
        }
        let t = d.cell(x, y - 1, z)[1] & 2 == 0;
        if t {
            ay += 2;
        }
        let b = d.cell(x, y + 1, z)[1] & 2 == 0;
        if b {
            by_ -= 2;
        }
        let h = by_ - ay;
        let w = bx_ - ax;
        self.dungeon_flat(zone, ax, ay, zc, [w, h, 1], carpet);
        if l {
            self.dungeon_flat(zone, ax, ay, zc, [1, h, 1], border);
        }
        if r {
            self.dungeon_flat(zone, bx_ - 1, ay, zc, [1, h, 1], border);
        }
        if t {
            self.dungeon_flat(zone, ax, ay, zc, [w, 1, 1], border);
        }
        if b {
            self.dungeon_flat(zone, ax, by_ - 1, zc, [w, 1, 1], border);
        }
    }

    /// Phase 8 step 5 (no chest): hanging props, wall props, torches, trap spawns, furniture and
    /// the loot of the furniture placed here.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_decorate(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, d: &Dungeon, x: i32, y: i32, z: i32, c: &DecoCtx) {
        let (x0, y0, zc) = (c.x0, c.y0, c.zc);
        let start = zone.statics.len();
        let ztop = c.base_z + (z + 1) * 10;
        if passable(d.cell(x, y, z + 1)[0]) {
            // rand @0x00505a54
            let r = self.rng.rand();
            if r % 3 == 0 && (c.style == 4 || c.style == 2) {
                let mut p = Prop::NEW;
                p.rotation = self.rng.rand() as f32 / 32767.0f32 * 360.0f32; // rand @0x00505a85
                let s = self.rng.rand() as f32 * 0.04f32 / 32767.0f32 + 0.08f32; // rand @0x00505aa6
                p.scale = s;
                p.z = fx_f(ztop as f32 - s * 20.0f32);
                p.y = from_block(by_rand(&mut self.rng, y0)); // rand @0x00505b04
                p.x = from_block(by_rand(&mut self.rng, x0)); // rand @0x00505b2c
                p.kind = 0x37;
                zone.props.push(p);
            }
            if c.style == 2 || c.style == 1 {
                // -x, +x, +y, -y: (neighbour dx, dy, angle bits)
                let sides = [(-1, 0, 0u32), (1, 0, 0x43340000), (0, 1, 0x43870000), (0, -1, 0x42b40000)];
                for (i, (ndx, ndy, ang)) in sides.into_iter().enumerate() {
                    // rand @0x00505b98 / 0x00505cf5 / 0x00505e58 / 0x00505fbb
                    if self.rng.rand() % 3 != 0 {
                        continue;
                    }
                    if !passable(d.cell(x + ndx, y + ndy, z)[0]) {
                        continue;
                    }
                    let mut p = Prop::NEW;
                    p.rotation = f32::from_bits(ang);
                    let s = self.rng.rand() as f32 * 0.05f32 / 32767.0f32 + 0.08f32; // rand @0x00505bed / 0x00505d50 / 0x00505eb3 / 0x00506010
                    p.scale = s;
                    p.z = fx_f(ztop as f32 - s * 20.0f32);
                    match i {
                        0 => {
                            p.y = from_block(by_rand(&mut self.rng, y0)); // rand @0x00505c53
                            p.x = fx_f(x0 as f32 + s * 10.0f32);
                        }
                        1 => {
                            p.y = from_block(by_rand(&mut self.rng, y0)); // rand @0x00505db6
                            p.x = fx_f((x0 + 10) as f32 - s * 10.0f32);
                        }
                        2 => {
                            p.y = fx_f((y0 + 10) as f32 - s * 10.0f32);
                            p.x = from_block(by_rand(&mut self.rng, x0)); // rand @0x00505f56
                        }
                        _ => {
                            p.y = fx_f(y0 as f32 + s * 10.0f32);
                            p.x = from_block(by_rand(&mut self.rng, x0)); // rand @0x005060b3
                        }
                    }
                    p.kind = (0x39 + self.rng.rand() % 2) as u32; // rand @0x00505ccf / 0x00505e32 / 0x00505f95 / 0x005060f2
                    zone.props.push(p);
                }
            }
        }
        // Wall lights, trap spawns and furniture: -x, +x, -y, +y (LAB_00506118).
        for side in 0..4 {
            // rand @0x00506118 / 0x00506477 / 0x005067e2 / 0x00506b47
            if self.rng.rand() % 3 != 0 {
                continue;
            }
            let (ndx, ndy) = [(-1, 0), (1, 0), (0, -1), (0, 1)][side];
            if !passable(d.cell(x + ndx, y + ndy, z)[0]) {
                continue;
            }
            let jitter = |rng: &mut MsvcRand, v: i32| v + rng.rand() % 4 + 3;
            if side == 3 {
                // +y: torch only, or spawn/furniture only.
                // rand @0x00506b8d
                if self.rng.rand() % 2 != 0 {
                    let pos = [from_block(jitter(&mut self.rng, x0)), fx_d(f64::from(y0 + 10) - 0.5), from_block(zc + 2)]; // rand @0x00506bf5
                    self.dungeon_torch(zone, pos, (0, 1), 180.0, c.style);
                } else {
                    self.dungeon_spawn_or_furniture(zone, extras, side, c);
                }
                continue;
            }
            // rand @0x00506158 / 0x005064bd / 0x00506822
            if self.rng.rand() % 2 != 0 {
                let (pos, probe, angle) = match side {
                    0 => {
                        let py = from_block(jitter(&mut self.rng, y0)); // rand @0x0050618c
                        ([fx_d(f64::from(x0) + 0.5), py, from_block(zc + 2)], (-1, 0), 270.0)
                    }
                    1 => {
                        let py = from_block(jitter(&mut self.rng, y0)); // rand @0x005064f1
                        ([fx_d(f64::from(x0 + 10) - 0.5), py, from_block(zc + 2)], (1, 0), 90.0)
                    }
                    _ => {
                        let px = from_block(jitter(&mut self.rng, x0)); // rand @0x0050688a
                        ([px, fx_d(f64::from(y0) + 0.5), from_block(zc + 2)], (0, -1), 0.0)
                    }
                };
                self.dungeon_torch(zone, pos, probe, angle, c.style);
            }
            self.dungeon_spawn_or_furniture(zone, extras, side, c);
        }
        // Loot on the statics placed above.
        let mut i = start;
        while i < zone.statics.len() {
            let kind = zone.statics[i].kind;
            if kind == 10 {
                let n = self.rng.rand() % 4 + 1; // rand @0x00506ee3
                let mut items = Vec::with_capacity(5);
                for _ in 0..n {
                    items.push(loot_item(&mut self.rng, c.level as i16 as i32, c.tier + 1));
                }
                extras.static_items.push((i, items));
                zone.statics[i].b30 = 2;
            } else if matches!(kind, 0xd | 0xc | 0x23 | 0x24 | 0x25) {
                self.dungeon_table_items(zone, extras, i, c);
            }
            i += 1;
        }
    }

    /// A wall torch at `pos` if the block one step towards `probe` is solid.
    fn dungeon_torch(&mut self, zone: &mut Zone, pos: [i64; 3], probe: (i64, i64), angle: f32, style: i32) {
        let px = pos[0] + probe.0 * 65536;
        let py = pos[1] + probe.1 * 65536;
        let b = zone.block(to_block(px), to_block(py), to_block(pos[2]));
        if is_solid(b) {
            let p = wall_light(&mut self.rng, style, pos, angle);
            zone.props.push(p);
        }
    }

    /// `rand() % 8 == 0`: a trap spawn (type 0x8f) against the wall; otherwise furniture.
    fn dungeon_spawn_or_furniture(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, side: usize, c: &DecoCtx) {
        let (x0, y0, zc) = (c.x0, c.y0, c.zc);
        let jitter = |rng: &mut MsvcRand, v: i32| v + rng.rand() % 4 + 3;
        // rand @0x00506294 / 0x005065f9 / 0x0050695e / 0x00506cca
        if self.rng.rand() % 8 == 0 {
            let mut s = Spawn { f28: 6, entity_type: 0x8f, level: c.level, f_f58: 10.0, ..Spawn::NEW };
            s.z = from_block(zc + 1);
            match side {
                0 => {
                    s.y = from_block(jitter(&mut self.rng, y0)); // rand @0x005063d8
                    s.x = from_block(x0 + 1);
                }
                1 => {
                    s.y = from_block(jitter(&mut self.rng, y0)); // rand @0x0050673d
                    s.x = from_block(x0 + 9);
                }
                2 => {
                    s.y = from_block(y0 + 1);
                    s.x = from_block(jitter(&mut self.rng, x0)); // rand @0x00506ac3
                }
                _ => {
                    s.y = from_block(y0 + 9);
                    s.x = from_block(jitter(&mut self.rng, x0)); // rand @0x00506e2f
                }
            }
            let mut e = SpawnExtra::NEW;
            e.f8 = 150.0;
            // `FUN_004fdd80` replaces the pages with one page of ten raw slots (count 1, the
            // misc item as generated: type-0 blanks and coins included), bypassing `addItem`.
            s.inventory.pages = vec![trap_inventory(&mut self.rng, c.level).into_iter().map(|item| Slot { count: 1, item }).collect()];
            extras.spawns.push((zone.spawns.len(), e));
            zone.spawns.push(s);
        } else {
            let (pos, dir) = match side {
                0 => {
                    let py = from_block(jitter(&mut self.rng, y0)); // rand @0x005062d0
                    ([fx_d(f64::from(x0) + 0.5), py, from_block(zc + 1)], 1)
                }
                1 => {
                    let py = from_block(jitter(&mut self.rng, y0)); // rand @0x00506635
                    ([fx_d(f64::from(x0 + 10) - 0.5), py, from_block(zc + 1)], 3)
                }
                2 => {
                    let px = from_block(jitter(&mut self.rng, x0)); // rand @0x005069ce
                    ([px, fx_d(f64::from(y0) + 0.5), from_block(zc + 1)], 2)
                }
                _ => {
                    let px = from_block(jitter(&mut self.rng, x0)); // rand @0x00506d3a
                    ([px, fx_d(f64::from(y0 + 10) - 0.5), from_block(zc + 1)], 0)
                }
            };
            let s = make_furniture(&mut self.rng, pos, dir, c.fstyle);
            zone.statics.push(s);
        }
    }

    /// Ground items on a table or shelf (`zone.statics[i]`), one chance in ten per unit of area.
    fn dungeon_table_items(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, i: usize, c: &DecoCtx) {
        let mut ix = 0i32;
        let mut fxc = 0.0f32;
        if zone.statics[i].scale[0] > 0.0 {
            loop {
                let mut iy = 0i32;
                let mut fyc = 0.0f32;
                if zone.statics[i].scale[1] > 0.0 {
                    loop {
                        // rand @0x00507070
                        if self.rng.rand() % 10 == 0 {
                            let mut it = Item::NEW;
                            // rand @0x0050709e
                            if self.rng.rand() % 6 != 0 {
                                // rand @0x005070b0
                                match self.rng.rand() % 4 {
                                    0 => {
                                        it.item_type = 0x0b;
                                        it.sub_type = 0x1a;
                                    }
                                    1 => {
                                        it.item_type = 0x12;
                                        it.sub_type = c.item_sub;
                                        it.modifier = self.rng.rand() % 3; // rand @0x005070ef
                                    }
                                    2 => {
                                        it.item_type = 0x0b;
                                        it.sub_type = 9;
                                        it.material = (self.rng.rand() % 3 + 25) as u8; // rand @0x0050710d
                                    }
                                    _ => {
                                        it.item_type = 1;
                                        it.sub_type = 7;
                                    }
                                }
                            } else {
                                // rand @0x00507133
                                match self.rng.rand() % 6 {
                                    0 => {
                                        it.item_type = 1;
                                        it.sub_type = 1;
                                    }
                                    1 => {
                                        it.item_type = 1;
                                        it.sub_type = 4;
                                    }
                                    2 => {
                                        it.item_type = 1;
                                        it.sub_type = 5;
                                    }
                                    3 => pow_item(&mut self.rng, &mut it, c.level, 10), // rand @0x00507177, 0x005071dc
                                    4 => it = loot_item(&mut self.rng, c.level as i16 as i32, c.tier),
                                    _ => it = equipment_item(&mut self.rng, c.level as i16 as i32, c.tier as u8),
                                }
                            }
                            let st = &zone.statics[i];
                            let sz = st.scale[2];
                            let oy = (fyc - st.scale[1] * 0.5f32) + 0.5f32;
                            let ox = (fxc - st.scale[0] * 0.5f32) + 0.5f32;
                            let (px, py, pz) = (st.x + fx_f(ox), st.y + fx_f(oy), st.z + fx_f(sz));
                            let rot = self.rng.rand() as f32 * 360.0f32 / 32767.0f32; // rand @0x00507325
                            let g = GroundItem {
                                item: it,
                                x: px,
                                y: py,
                                z: pz,
                                rotation: rot,
                                f134: f32::from_bits(0x3d75c28f),
                                b138: 1,
                                ..GroundItem::NEW
                            };
                            let idx = zone.items.len();
                            extras.ground_items.push((idx, it));
                            extras.ground_item_b138.push((idx, 1));
                            zone.items.push(g);
                        }
                        iy += 1;
                        fyc = iy as f32;
                        if zone.statics[i].scale[1] <= fyc {
                            break;
                        }
                    }
                }
                ix += 1;
                fxc = ix as f32;
                if zone.statics[i].scale[0] <= fxc {
                    break;
                }
            }
        }
    }

    /// Phase 8 step 7: the boss of a room flagged 4, its loot, marker and potions.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_boss(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, singles: &[i32], x0: i32, y0: i32, zc: i32, level: i32, tier: i32) {
        let mut s = Spawn::NEW;
        s.z = from_block(zc + 1);
        s.y = fx_f(y0 as f32 + 4.5f32);
        s.x = fx_f(x0 as f32 + 4.5f32);
        s.f28 = 1;
        s.appearance.flags |= 0x1000;
        s.level = level;
        s.b58 = tier as u8;
        if s.level < 1 {
            s.level = 1;
        }
        let r = self.rng.rand() as u32; // rand @0x005079c0
        s.entity_type = singles[(r % singles.len() as u32) as usize];
        let mut e = SpawnExtra::NEW;
        let rarity = random_rarity(&mut self.rng, i32::from(s.b58), true);
        let loot = loot_item(&mut self.rng, s.level as i16 as i32, rarity);
        s.inventory.add_item(loot, -1);
        s.appearance.flags |= 0x200;
        e.ai = Some(Behavior::Combat(20.0));
        s.ai = Some(SpawnAi::Combat); // 0x00507a42
        extras.markers.push(Marker {
            kind: 6,
            spawn_index: zone.spawns.len() as i32,
            creature_type: s.entity_type,
            x: s.x,
            y: s.y,
            z: s.z,
        });
        let n = self.rng.rand() % 4; // rand @0x00507aa5
        let potion = Item { item_type: 1, sub_type: 1, level: s.level as u16, ..Item::NEW };
        for _ in 0..n {
            s.inventory.add_item(potion, -1);
        }
        e.f8 = 150.0;
        s.b10e8 = 1;
        extras.spawns.push((zone.spawns.len(), e));
        zone.spawns.push(s);
    }

    /// Phase 10 for one room: a patrolling group (leader and followers) and rings of single
    /// monsters around the corners and the centre.
    #[allow(clippy::too_many_arguments)]
    fn dungeon_room_monsters(
        &mut self,
        zone: &mut Zone,
        extras: &mut DungeonExtras,
        d: &Dungeon,
        m: &MonsterCtx,
        corners: &[[i32; 3]; 4],
        w: i32,
        h: i32,
        boss_room: bool,
        singles: &[i32],
        groups: &[(Vec<i32>, Vec<i32>)],
    ) {
        let cell_pos = |c: [i32; 3]| [from_block(c[0] * 10 + 5 + m.bx), from_block(c[1] * 10 + 5 + m.by), from_block(m.base_z + c[2] * 10 + 1)];
        if 6 < h * w && !groups.is_empty() {
            let r = self.rng.rand() as u32; // rand @0x00508ac9
            let g = &groups[(r % groups.len() as u32) as usize];
            if !g.0.is_empty() {
                let k = self.rng.rand() % 4; // rand @0x00508af0
                let mut s = Spawn::NEW;
                let p = cell_pos(corners[(k % 4) as usize]);
                s.x = p[0];
                s.y = p[1];
                s.z = p[2];
                s.level = m.level;
                if !m.special {
                    s.f28 = 1;
                    s.appearance.flags |= 0x1000;
                    let r = self.rng.rand() as u32; // rand @0x00508c3d
                    s.entity_type = g.0[(r % g.0.len() as u32) as usize];
                    s.b58 = m.tier as u8;
                } else {
                    s.f28 = 3;
                    s.entity_type = self.rng.rand() % 2; // rand @0x00508c0d
                }
                let mut path = Vec::new();
                // rand @0x00508d34
                match self.rng.rand() % 3 {
                    0 => {
                        for i in 0..4 {
                            path.push(cell_pos(corners[((k + i) % 4) as usize]));
                        }
                    }
                    1 => {
                        for i in 0..4 {
                            path.push(cell_pos(corners[(3 - (k + i) % 4) as usize]));
                        }
                    }
                    _ => {
                        path.push(cell_pos(corners[(k % 4) as usize]));
                        path.push(cell_pos(corners[((k + 1) % 4) as usize]));
                        path.push(cell_pos(corners[((k + 2) % 4) as usize]));
                        path.push(cell_pos(corners[((k + 1) % 4) as usize]));
                    }
                }
                let mut e = SpawnExtra::NEW;
                s.ai = Some(SpawnAi::Patrol { path: path.clone() }); // 0x00508c7d
                e.ai = Some(Behavior::Sequence(vec![Behavior::Combat(20.0), Behavior::WalkPath(2.0, path)]));
                e.f8 = 150.0;
                let n = self.rng.rand() % 2; // rand @0x00509137
                let potion = Item { item_type: 1, sub_type: 1, level: s.level as u16, ..Item::NEW };
                for _ in 0..n {
                    s.inventory.add_item(potion, -1);
                }
                let leader_uid = spawn_uid(zone.x, zone.y, zone.spawns.len() as i32);
                set_spawn_uid(&mut s, leader_uid);
                let leader = s.clone();
                extras.spawns.push((zone.spawns.len(), e));
                zone.spawns.push(s);
                if !m.special && !g.1.is_empty() {
                    let n = self.rng.rand() % 3 + 1; // rand @0x005091fb
                    for _ in 0..n {
                        let mut f = Spawn::NEW;
                        f.x = leader.x;
                        f.y = leader.y;
                        f.z = leader.z;
                        f.level = leader.level;
                        f.f28 = 1;
                        f.appearance.flags |= 0x1000;
                        let r = self.rng.rand() as u32; // rand @0x00509282
                        f.entity_type = g.1[(r % g.1.len() as u32) as usize];
                        f.b58 = leader.b58;
                        let mut fe = SpawnExtra::NEW;
                        fe.ai = Some(Behavior::Sequence(vec![
                            Behavior::Combat(20.0),
                            Behavior::Companion(leader_uid),
                            Behavior::WalkPath(2.0, vec![[leader.x, leader.y, leader.z]]),
                        ]));
                        // 0x005092c2
                        f.ai = Some(SpawnAi::DungeonFollower { leader: leader_uid, path: vec![[leader.x, leader.y, leader.z]] });
                        f.f_f60 *= 0.5f32;
                        fe.f8 = 150.0;
                        extras.spawns.push((zone.spawns.len(), fe));
                        zone.spawns.push(f);
                    }
                }
            }
        }
        if !singles.is_empty() && !boss_room {
            let mut a = self.rng.rand() % 2 + 1; // rand @0x00509457
            let k = self.rng.rand() % 4; // rand @0x0050946f
            if 7 < h * w {
                a += 1;
            }
            for kk in k..k + a {
                let n = self.rng.rand() % 2 + 2; // rand @0x005094a8
                let c = corners[(kk % 4) as usize];
                self.dungeon_ring(zone, extras, m, c, n, singles, false);
            }
            if 2 < w && 2 < h {
                // rand @0x005098b1
                if self.rng.rand() % 2 != 0 {
                    let ctr = [(corners[0][0] + corners[2][0]) / 2, (corners[0][1] + corners[2][1]) / 2, (corners[0][2] + corners[2][2]) / 2];
                    if d.cell(ctr[0], ctr[1], ctr[2])[0] == 3 {
                        let n = self.rng.rand() % 2 + 2; // rand @0x0050993f
                        self.dungeon_ring(zone, extras, m, ctr, n, singles, true);
                    }
                }
            }
        }
    }

    /// A ring of `n` single monsters of radius 2 around cell `c`. `centre` selects the centre-ring
    /// variant (the 0.75 scale is applied before the AI there; no effect on the result).
    #[allow(clippy::too_many_arguments)]
    fn dungeon_ring(&mut self, zone: &mut Zone, extras: &mut DungeonExtras, m: &MonsterCtx, c: [i32; 3], n: i32, singles: &[i32], centre: bool) {
        let _ = centre;
        let nd = f64::from(n);
        let mut j = 0i32;
        for _ in 0..n {
            let ang = ((f64::from(j) * std::f64::consts::PI) / nd) as f32;
            let mut s = Spawn::NEW;
            s.z = from_block(m.base_z + c[2] * 10 + 1);
            let sn = cw_math::sin(f64::from(ang)) as f32;
            s.y = fx_f(sn * 2.0f32 + (c[1] * 10 + 5 + m.by) as f32);
            let cs = cw_math::cos(f64::from(ang)) as f32;
            s.x = fx_f(cs * 2.0f32 + (c[0] * 10 + 5 + m.bx) as f32);
            s.rotation = (f64::from(ang) / std::f64::consts::PI * 180.0 + 90.0) as f32;
            s.level = m.level;
            if !m.special {
                s.f28 = 1;
                s.appearance.flags |= 0x1000;
                let r = self.rng.rand() as u32; // rand @0x005096da / 0x00509b35
                s.entity_type = singles[(r % singles.len() as u32) as usize];
                s.b58 = m.tier as u8;
            } else {
                s.f28 = 3;
                s.entity_type = self.rng.rand() % 2; // rand @0x005096aa / 0x00509b05
            }
            let mut e = SpawnExtra::NEW;
            e.ai = Some(Behavior::Sequence(vec![Behavior::Combat(20.0), Behavior::WalkPath(2.0, vec![[s.x, s.y, s.z]])]));
            s.ai = Some(SpawnAi::Patrol { path: vec![[s.x, s.y, s.z]] }); // 0x00509714 / 0x00509b87
            e.f8 = 150.0;
            s.f_f60 *= 0.75f32;
            let np = self.rng.rand() % 2; // rand @0x005097fb / 0x00509c56
            let potion = Item { item_type: 1, sub_type: 1, level: s.level as u16, ..Item::NEW };
            for _ in 0..np {
                s.inventory.add_item(potion, -1);
            }
            extras.spawns.push((zone.spawns.len(), e));
            zone.spawns.push(s);
            j += 2;
        }
    }
}

/// `y0 + 2 + rand() % 5` (the hanging/wall prop placement inside a cell).
fn by_rand(rng: &mut MsvcRand, v: i32) -> i32 {
    v + 2 + rng.rand() % 5
}

/// A door side: neighbour cell, probe block, clear origin, door position, rotation.
type DoorSide = ((i32, i32), (i32, i32), (i32, i32), (i32, i32), i32);

/// Per-cell values the decoration pass reads.
struct DecoCtx {
    style: i32,
    fstyle: i32,
    level: i32,
    tier: i32,
    item_sub: u8,
    x0: i32,
    y0: i32,
    zc: i32,
    base_z: i32,
}

/// Per-dungeon values the monster pass reads.
struct MonsterCtx {
    bx: i32,
    by: i32,
    base_z: i32,
    level: i32,
    tier: i32,
    special: bool,
}
