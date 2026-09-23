//! `cube::World::generateSettlement`, `Server.exe 0x004e28e0`. See
//! `analysis/notes/functions/004e28e0_generateSettlement.md` and `analysis/notes/porting-brief.md`.
//!
//! The zone is cut into an N x N grid of plots (N = 5 for towns, 4 for cell type 5). Plots are
//! classified, the most central house lots get special roles, and every plot is then built:
//! houses (`cube::House`, a 3x3x4 grid of rooms stamped from voxel models), farms, guard posts,
//! special buildings, wild creatures. Trees with flattened plazas and benches follow, then the
//! NPCs: villagers with day schedules and shopkeepers in towns, guards and bosses in type-5 cells.
//!
//! Objects the crate's `Zone` does not track yet (airships `zone+0x24`, creature fields beyond
//! [`Spawn`]) are collected in [`SettlementExtras`]; the houses (`zone+0x88`) are also copied to
//! [`Zone::houses`] and each spawn's AI tree to [`Spawn::ai`].
//!
//! Negated float comparisons are kept where the original's NaN behaviour depends on them.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::type_complexity)]

use cw_math::MsvcRand;
use cw_math::value_noise_2d as noise;

use crate::fixed::{add_double, add_float, from_block, sub_double, to_block};
use crate::model::Model;
use crate::region::Cell;
use crate::surface::Block;
use crate::world::World;
use crate::inventory::Item;
use crate::zone::{Column, GroundItem, Prop, Spawn, SpawnAi, Static, Zone, set_block};

// ---------------------------------------------------------------------------------------------
// Records the settlement produces that `Zone` / `Spawn` do not model yet.
// ---------------------------------------------------------------------------------------------

/// One `FUN_00427000(inventory, &item, bag)` call on a creature's inventory (`creature+0xf6c`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InventoryAdd {
    pub item: Item,
    pub bag: i32,
}

/// One schedule entry (`FUN_004e20d0`, 0x1c bytes): a 16.16 fixed position and a time in ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleEntry {
    pub pos: [i64; 3],
    pub time: i32,
}

/// Which AI object (`creature+0x109c`) the generator attached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ai {
    /// `FUN_0041cfc0` holding `FUN_00414720` only (the airship pilot).
    Pilot,
    /// `{FUN_004029e0(20.0), FUN_00414720, FUN_004c5d50(2.0) home = creature position}`;
    /// `target` is the house spot the `FUN_004c5d50` object gets for group guards. Only the
    /// town house NPCs (classes 1..5) get this tree; the type-5 cell boss, ring guards and group
    /// guards that also carry `Guard` here have no `FUN_00414720` (see [`SpawnAi::Patrol`], which
    /// [`Spawn::ai`] records exactly).
    Guard { target: Option<[i64; 3]> },
    /// `{FUN_004029e0(20.0), FUN_00414720, FUN_00428920, FUN_0041ba60, FUN_0041cb90}`.
    Villager,
}

/// Creature fields beyond [`Spawn`] for one spawn this function pushed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpawnExtra {
    /// Index in `zone.spawns`.
    pub spawn_index: usize,
    /// `+0x08` (512.0 for the airship pilot).
    pub f08: Option<f32>,
    /// `+0x5c`
    pub ai_mode: Option<i32>,
    /// `+0x60` vec3i target (x, y, 0) in zone coordinates.
    pub target: Option<[i32; 3]>,
    /// `+0x10e0` int64 id.
    pub id: Option<i64>,
    /// `+0x109c`
    pub ai: Option<Ai>,
    /// `+0x10a0` schedule vector.
    pub schedule: Vec<ScheduleEntry>,
    /// Calls on the `+0xf6c` inventory, in order.
    pub inventory: Vec<InventoryAdd>,
}

/// The airship-like object of `zone+0x24` (0xa0 bytes, ctor `FUN_004e2110`); only the fields
/// written here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Airship {
    /// `+0x10`
    pub pos: [i64; 3],
    /// `+0x34`
    pub rotation: f32,
    /// `+0x38`
    pub anchor: [i64; 3],
    /// `+0x50`
    pub heading: f32,
    /// `+0x58`
    pub pos2: [i64; 3],
}

/// One room cell of a house (12 bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HouseCell {
    /// `+0`: 0 empty, 1 room, 2 ground, 3 roof, 5 special.
    pub t: u8,
    /// `+1`: rotation quadrant.
    pub rot: u8,
    /// `+2`: door flag.
    pub door: u8,
    /// `+3`
    pub b3: u8,
    /// `+4`: ground height under the cell.
    pub h: i32,
    /// `+8`: 1 link room, 2 table room, 3 counter room.
    pub b8: u8,
}

/// `cube::House` (0x74 bytes, ctor `Server.exe 0x004e1f80`).
#[derive(Debug, Clone, PartialEq)]
pub struct House {
    /// `+4`: raw direction (may reach 6; accessors use `% 4`).
    pub dir: i32,
    /// `+8`
    pub mirror: bool,
    /// `+0xc`
    pub pos: [i32; 3],
    /// `+0x24`
    pub floor_spots: Vec<[i32; 3]>,
    /// `+0x30`
    pub door_spots: Vec<[i32; 3]>,
    /// `+0x3c`
    pub special_spots: Vec<[i32; 3]>,
    /// `+0x48`: (zx, zy, static index) of statics 0x10 / 0x12.
    pub seats: Vec<[i32; 3]>,
    /// `+0x54`: (zx, zy, static index) of statics 0x13.
    pub beds: Vec<[i32; 3]>,
    /// `+0x60`
    pub class: i32,
    /// `+0x64`, `+0x68`, `+0x6c`
    pub size: [i32; 3],
    /// `+0x70`
    pub cells: Vec<HouseCell>,
}

impl House {
    /// `cube::House::House(x, y, z)`, `Server.exe 0x004e1f80`.
    pub fn new(sx: i32, sy: i32, sz: i32) -> Self {
        House {
            dir: 0,
            mirror: false,
            pos: [0; 3],
            floor_spots: Vec::new(),
            door_spots: Vec::new(),
            special_spots: Vec::new(),
            seats: Vec::new(),
            beds: Vec::new(),
            class: 0,
            size: [sx, sy, sz],
            cells: vec![HouseCell::default(); (sx * sy * sz) as usize],
        }
    }

    /// `FUN_004d8f90` plus the bounds check of `FUN_004d1950`.
    fn index(&self, mut x: i32, mut y: i32, z: i32) -> Option<usize> {
        let [sx, sy, sz] = self.size;
        match self.dir % 4 {
            1 => {
                let t = x;
                x = sx - y - 1;
                y = t;
            }
            2 => {
                x = sx - x - 1;
                y = sy - y - 1;
            }
            3 => {
                let t = x;
                x = y;
                y = sy - t - 1;
            }
            _ => {}
        }
        if self.mirror {
            y = sy - y - 1;
        }
        if x >= 0 && y >= 0 && z >= 0 && x < sx && y < sy && z < sz {
            Some(((sy * z + y) * sx + x) as usize)
        } else {
            None
        }
    }

    /// `FUN_004d1950(x, y, z)`: the cell, or a zero cell outside the grid.
    pub fn cell(&self, x: i32, y: i32, z: i32) -> HouseCell {
        self.index(x, y, z).map(|i| self.cells[i]).unwrap_or_default()
    }

    /// A write through `FUN_004d1950`; writes outside the grid land in a scratch cell.
    fn edit(&mut self, x: i32, y: i32, z: i32, f: impl FnOnce(&mut HouseCell)) {
        if let Some(i) = self.index(x, y, z) {
            f(&mut self.cells[i]);
        }
    }

    fn set_t(&mut self, x: i32, y: i32, z: i32, t: u8) {
        self.edit(x, y, z, |c| c.t = t);
    }

    fn set_b8(&mut self, x: i32, y: i32, z: i32, v: u8) {
        self.edit(x, y, z, |c| c.b8 = v);
    }

    fn set_b3(&mut self, x: i32, y: i32, z: i32, v: u8) {
        self.edit(x, y, z, |c| c.b3 = v);
    }

    /// `FUN_004d8dc0`
    pub fn dim_x(&self) -> i32 {
        if self.dir % 2 != 0 { self.size[1] } else { self.size[0] }
    }

    /// `FUN_004d8de0`
    pub fn dim_y(&self) -> i32 {
        if self.dir % 2 != 0 { self.size[0] } else { self.size[1] }
    }

    /// `FUN_004d8e00`
    pub fn dim_z(&self) -> i32 {
        self.size[2]
    }
}

/// Everything `generateSettlement` produces that `Zone` cannot hold yet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettlementExtras {
    /// `zone+0x88`
    pub houses: Vec<House>,
    /// `zone+0x24`
    pub airships: Vec<Airship>,
    /// Creature fields for the spawns pushed here.
    pub spawns: Vec<SpawnExtra>,
    /// Item records of `zone.items` entries pushed here, same order (`GroundItem` keeps only
    /// kind/sub/material).
    pub items: Vec<(usize, Item)>,
}

/// `FUN_004013f0(world+0x1c, index)`: `world->models[index]`, null past the end of the table.
#[inline]
fn model_at(models: &[Model], index: u32) -> Option<&Model> {
    models.get(index as usize)
}

// ---------------------------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------------------------

/// A plot record (0x1c bytes).
#[derive(Debug, Clone, Copy, Default)]
struct Plot {
    /// `+0`
    min: i32,
    /// `+4`
    max: i32,
    /// `+8` (never read)
    _mid: i32,
    /// `+0xc`
    kind: i32,
    /// `+0x10`
    class: i32,
    /// `+0x14`
    dir: i32,
    /// `+0x18`
    f18: f32,
}

/// `FUN_00402510` component: `_ftol(f * 65536.0f)`.
#[inline]
fn f_to_fix(f: f32) -> i64 {
    (f * 65536.0) as i64
}

/// `FUN_004d99d0`: vec3i to fixed.
#[inline]
fn fix3(p: [i32; 3]) -> [i64; 3] {
    [from_block(p[0]), from_block(p[1]), from_block(p[2])]
}

/// `FUN_004061f0`: neither air nor water.
#[inline]
fn solid(b: Block) -> bool {
    let t = b[3] & 0x1f;
    t != 0 && t != 2
}

#[inline]
fn btype(b: Block) -> u8 {
    b[3] & 0x1f
}

/// `FUN_0041d160` + `FUN_00401370`: float rgb to bytes (`(char)(int)f`) plus a type byte.
#[inline]
fn rgb_block(c: [f32; 3], t: u8) -> Block {
    [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, t]
}

/// `FUN_004e2840`: clamp each channel to `0..=255`.
#[inline]
fn clamp255(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| {
        if !(0.0 <= v) {
            0.0
        } else if !(v <= 255.0) {
            255.0
        } else {
            v
        }
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

/// `World::getColumn(bx, by, zone)` with the zone hint: only this zone's columns are found.
#[inline]
fn col(zone: &Zone, x: i32, y: i32) -> Option<&Column> {
    if zone.contains(x, y) { Some(zone.column(x, y)) } else { None }
}

/// `height + count`, the first z above a column's blocks.
#[inline]
fn col_top(c: &Column) -> i32 {
    c.height + c.blocks.len() as i32
}

/// `(float)rand() * 360.0f / 32767.0f`
#[inline]
fn rand_rot(r: i32) -> f32 {
    (r as f32 * 360.0) / 32767.0
}

/// `cube::Cell::falloff(&x64, &y64)`, `Server.exe 0x0052dee0`.
#[inline]
fn falloff(cell: &Cell, x64: i64, y64: i64) -> f32 {
    let f = 1.0 - cell.norm_distance(x64, y64);
    if f <= 0.0 { 0.0 } else { f * f }
}

/// `FUN_0040f0a0(species, &min, &max)`: level range of a wild species.
pub fn species_level_range(species: i32) -> (i32, i32) {
    match species {
        0x11 | 0x31 | 0x67 | 0x71 | 0x72 | 0x9a => (0x2f, 0x51),
        0x13 | 0x14 | 0x19 | 0x1a | 0x1e | 0x1f | 0x20 | 0x3c | 0x3f | 0x43 | 0x45 | 0x46 => (3, 6),
        0x15 | 0x2e | 0x2f | 0x32 | 0x3a | 0x4b | 0x50 | 0x56 | 0x59 | 0x66 | 0x68 | 0x96 => (0x1f, 0x2f),
        0x1c | 0x3d | 0x5a | 0x9b => (0xe, 0x15),
        0x24 | 0x36 | 0x40 | 0x41 | 0x42 | 0x48 | 0x49 | 0x5b | 99 | 0x98 | 0x99 => (0x15, 0x1f),
        0x25 | 0x26 | 0x27 | 0x28 | 0x35 | 0x3b | 0x58 | 0x69 | 0x6a | 0x97 => (9, 0xe),
        0x3e | 0x52 | 0x55 | 0x61 | 0x6e | 0x70 => (0xb4, 0x4e0d),
        0x51 | 0x53 | 0x54 | 0x5e => (0x51, 0xb4),
        0x62 | 100 => (6, 9),
        _ => (2, 4),
    }
}

/// `FUN_004f3850(zx, zy, n)`: `((zy << 16) + zx) << 8 + n` in 64 bits.
pub fn airship_id(zx: i32, zy: i32, n: i32) -> i64 {
    (((i64::from(zy) << 16) + i64::from(zx)) << 8) + i64::from(n)
}

/// `FUN_004f3630(ent, pos, orient)`: a guard-post prop, types 0x15..0x17.
fn guard_prop(rng: &mut MsvcRand, pos: [i64; 3], orient: i32) -> Static {
    let mut s = Static { x: pos[0], y: pos[1], z: pos[2], rotation: orient, ..Static::NEW };
    s.kind = (rng.rand() % 3 + 0x15) as u32; // 0x004f36bb
    s.scale = [3.5, 2.0, 3.0];
    s
}

/// The shared barrel/crate switch of `FUN_004f3490` (cases 0..3) and `FUN_004f2cd0`.
fn barrel_case(rng: &mut MsvcRand, s: &mut Static, case: i32) -> bool {
    match case {
        0 => {
            s.kind = 0x18;
            s.scale = [2.0, 2.0, 2.0];
        }
        1 => {
            s.kind = 0x19;
            let f = ((rng.rand() as f32 * 0.5) / 32767.0 + 1.0) * 1.5; // 4f3490 case 1 (call edi, 0x004f3569) / 4f2cd0 case 1 (call edi)
            s.scale = [f, f, f];
        }
        2 => {
            s.kind = 0x1a;
            let f = (rng.rand() as f32 * 0.5) / 32767.0 + 1.0; // 4f3490 case 2 (call edi, 0x004f35af) / 4f2cd0 case 2 (call edi)
            let g = f * 1.5;
            s.scale = [g, g, f * 0.75];
        }
        3 => {
            s.kind = 0x1b;
            s.scale = [1.5, 1.5, f32::from_bits(0x3fb33333)];
        }
        _ => return false,
    }
    true
}

/// `FUN_004f3490(ent, pos, orient)`: a yard prop, types 0x18..0x1b.
fn yard_prop(rng: &mut MsvcRand, pos: [i64; 3], orient: i32) -> Static {
    let mut s = Static { x: pos[0], y: pos[1], z: pos[2], rotation: orient, ..Static::NEW };
    let c = rng.rand() % 4; // 0x004f34f5
    barrel_case(rng, &mut s, c);
    s
}

/// `FUN_004f2cd0(ent, pos, orient)`: house clutter, types 0x18/19/1a/1b/12/10/1c.
fn clutter_prop(rng: &mut MsvcRand, pos: [i64; 3], orient: i32) -> Static {
    let mut s = Static { x: pos[0], y: pos[1], z: pos[2], rotation: orient, ..Static::NEW };
    let c = rng.rand() % 7; // 0x004f2d35
    if !barrel_case(rng, &mut s, c) {
        match c {
            4 => {
                s.kind = 0x12;
                s.scale = [3.0, 1.0, f32::from_bits(0x3ecccccd)];
            }
            5 => {
                s.kind = 0x10;
                s.scale = [1.0, 1.0, 0.5];
            }
            _ => {
                s.kind = 0x1c;
                s.scale = [3.0, 3.0, 2.5];
            }
        }
    }
    s
}

/// `FUN_004f2ee0(ent, pos, orient, kind)`: wall furniture of a room; `kind` is the room's b8.
fn wall_furniture(rng: &mut MsvcRand, pos: [i64; 3], orient: i32, kind: i32) -> Static {
    let mut s = Static { x: pos[0], y: pos[1], z: pos[2], rotation: orient, ..Static::NEW };
    // Offsets towards the wall, `_ftol2` of the doubles the compiler folded.
    let shift = |s: &mut Static, a: i64| match orient {
        0 => s.y += -a,
        1 => s.x += a,
        2 => s.y += a,
        3 => s.x += -a,
        _ => {}
    };
    if rng.rand() % 50 == 0 {
        // 0x004f2fbc (later calls in 4f2ee0 go through edi)
        s.kind = 10;
        s.scale = [f32::from_bits(0x3f99999a), f32::from_bits(0x3f4ccccd), f32::from_bits(0x3f4ccccd)];
        shift(&mut s, 32768);
        return s;
    }
    let shelf = |s: &mut Static, r: i32| {
        s.kind = (r % 3 + 0x20) as u32;
        s.scale = [2.0, 1.0, f32::from_bits(0x3fc8f5c3)];
    };
    if kind == 1 {
        if rng.rand() % 2 == 0 {
            // 0x004f3069
            let r = rng.rand(); // 4f2ee0, call edi
            shelf(&mut s, r);
        } else {
            s.kind = 0x14;
            s.scale = [1.0, 1.0, 1.0];
        }
        return s;
    }
    match rng.rand() % 6 {
        // 0x004f30c4
        0 => {
            let r = rng.rand(); // 0x004f30e2
            shelf(&mut s, r);
        }
        1 => {
            s.kind = 0x12;
            s.scale = [3.0, 1.0, f32::from_bits(0x3ecccccd)];
        }
        2 => {
            s.kind = 0x10;
            s.scale = [1.0, 1.0, 0.5];
            if (0..4).contains(&orient) {
                let f = ((rng.rand() as f32) / 32767.0) * 65536.0; // 0x004f3148 / 0x004f3193 / 0x004f31d8 / 0x004f3223 per orientation
                shift(&mut s, f as i64);
            }
        }
        3 => {
            s.kind = 0x1e;
            s.scale = [3.0, 1.0, 1.0];
            shift(&mut s, 13107);
        }
        4 => {
            s.kind = 0x1d;
            s.scale = [2.5, 1.0, 3.0];
            shift(&mut s, 19660);
        }
        _ => {
            s.kind = (rng.rand() % 9 + 0x38) as u32; // 0x004f339c
            s.scale = [1.0, 1.0, 1.0];
        }
    }
    s
}

/// `FUN_0052b1c0(out)`: a random item of kind 0x19.
fn random_item(rng: &mut MsvcRand) -> Item {
    Item { item_type: 0x19, modifier: rng.rand() % 200, level: 1, ..Item::NEW } // 0x0052b20c
}

/// Model size of an optional model (`FUN_00402150/60/70`).
#[inline]
fn msize(m: Option<&Model>) -> [i32; 3] {
    m.as_ref().map(|m| m.size).unwrap_or([0; 3])
}

impl World {
    /// `FUN_0052db90(out, x, y, z)`: stone colour of the plaza rims.
    pub fn settlement_stone_color(&self, x: i32, y: i32, z: i32) -> [f32; 3] {
        let s = &self.seeds;
        let z3 = f64::from(z) * 0.3;
        let n = noise(f64::from(y) * 0.01 + 98984.0, z3 + 8437.0) + noise(f64::from(x) * 0.01, z3);
        let x5 = f64::from(x) * 0.005;
        let y5 = f64::from(y) * 0.005;
        let a = noise(s.f(0x800234) + x5, s.f(0x800238) + y5) * 20.0;
        let b = noise(s.f(0x80023c) + x5, s.f(0x800240) + y5) * 20.0;
        let base = (n * 0.5 + 1.0) * 0.5 * 200.0 + 50.0;
        let c = noise(s.f(0x800244) + x5, s.f(0x800248) + y5) * 20.0;
        clamp255([base + a, base + b, base + c])
    }

    /// `FUN_004fc140(x, y, zone)`: climate B from the column when it exists.
    fn settlement_climate_b(&self, zone: &Zone, x: i32, y: i32) -> f32 {
        match col(zone, x, y) {
            Some(c) => c.climate_b,
            None => self.climate_b(x, y),
        }
    }

    /// `cube::World::placeModel` on an optional model (the original never passes null here).
    #[allow(clippy::too_many_arguments)]
    fn settlement_place(
        &mut self,
        zone: &mut Zone,
        m: Option<&Model>,
        pos: [i32; 3],
        rot: i32,
        block_type: u8,
        kind: i32,
        foundation: bool,
        margins: [i32; 4],
    ) {
        if let Some(m) = m {
            self.place_model(zone, m, pos, rot, block_type, kind, foundation, margins);
        }
    }

    /// `generateSettlement(zone, cell)`, `Server.exe 0x004e28e0`.
    ///
    /// Houses, airships and creature fields the crate cannot store yet are dropped;
    /// [`World::generate_settlement_ext`] returns them.
    pub fn generate_settlement(&mut self, zone: &mut Zone, cell: &Cell) {
        let mut extras = SettlementExtras::default();
        self.generate_settlement_ext(zone, cell, &mut extras);
    }

    /// [`World::generate_settlement`] returning the records `Zone` cannot hold yet.
    pub fn generate_settlement_ext(&mut self, zone: &mut Zone, cell: &Cell, out: &mut SettlementExtras) {
        let models = std::sync::Arc::clone(&self.models);
        let models: &[Model] = &models;
        let n: i32 = if cell.kind == 5 { 4 } else { 5 };
        let species: [i32; 5] = [0x22, 0x1e, 0x13, 0x1a, 0x21];
        let s = 0x100 / n;
        let h = s / 2;
        let zx = zone.x;
        let zy = zone.y;
        let mut plots = vec![Plot::default(); (n * n) as usize];

        // Phase A: sample each plot.
        for i in 0..n {
            for j in 0..n {
                let idx = (j * n + i) as usize;
                let x0 = zx * 256 + (i << 8) / n;
                let y0 = zy * 256 + (j << 8) / n;
                let mut first = true;
                let mut any_central = false;
                let mut blocked = false;
                let mut plaza = false;
                let (mut min, mut max, mut mid) = (0i32, 0i32, -10000i32);
                for u in 0..s {
                    for v in 0..s {
                        let bx = x0 + u;
                        let by = y0 + v;
                        let f = falloff(cell, from_block(bx), from_block(by));
                        if f > 0.1 {
                            any_central = true;
                        }
                        let c = col(zone, bx, by).expect("plot columns lie in the zone");
                        if col_top(c) < 1 {
                            blocked = true;
                        }
                        for k in 0..c.blocks.len() as i32 {
                            let t = btype(c.block_at(k));
                            if t == 2 || t == 3 {
                                blocked = true;
                            }
                            if 7 < u && 7 < v && u < s - 7 && v < s - 7 && f < 0.65 && t == 0xb {
                                plaza = true;
                            }
                        }
                        let top = col_top(c) - 1;
                        if first {
                            first = false;
                            max = top;
                            min = top;
                        } else {
                            if max < top {
                                max = top;
                            }
                            if top < min {
                                min = top;
                            }
                        }
                        if u >= 8 && v >= 8 && u < s - 8 && v < s - 8 && mid != 0 {
                            mid = top;
                        }
                    }
                }
                let p = &mut plots[idx];
                p.min = min;
                p.max = max;
                p._mid = mid;
                p.dir = self.rng.rand() % 4; // 0x004e2d7d
                p.f18 = falloff(cell, from_block(x0 + h), from_block(y0 + h));
                if !blocked && any_central {
                    if plaza {
                        p.kind = 7;
                    } else {
                        let r = self.rng.rand(); // 0x004e2e35
                        p.kind = if p.f18 + 0.25 > r as f32 / 32767.0 { 2 } else { 0 };
                        if p.max - p.min > 0x10 {
                            p.kind = 0;
                        }
                    }
                } else {
                    p.kind = 0;
                }
            }
        }

        // Phase B: turn door directions away from neighbouring houses (no rand).
        for i in 0..n {
            for j in 0..n {
                let idx = (j * n + i) as usize;
                for _ in 0..3 {
                    let (ni, nj) = match plots[idx].dir % 4 {
                        1 => (i - 1, j),
                        2 => (i, j + 1),
                        3 => (i + 1, j),
                        _ => (i, j - 1),
                    };
                    if ni >= 0 && nj >= 0 && ni < n && nj < n && plots[(nj * n + ni) as usize].kind != 2 {
                        break;
                    }
                    plots[idx].dir += 1;
                }
            }
        }

        // Phase C: farms (town only).
        if cell.kind == 1 {
            for p in plots.iter_mut() {
                if p.kind == 0 && p.max - p.min < 0x10 {
                    let r = self.rng.rand(); // 0x004e3033
                    if r % 2 == 0 && 0.0 < p.f18 {
                        p.kind = 6;
                    }
                }
            }
        }

        // Phase D: candidate lists and role assignment.
        let mut cand: Vec<usize> = (0..plots.len()).filter(|&k| plots[k].kind == 2).collect();
        if cand.is_empty() {
            return;
        }
        // std::sort of <= 32 elements: insertion sort, stable, ascending by f18.
        for a in 1..cand.len() {
            let v = cand[a];
            let mut b = a;
            while b > 0 && plots[v].f18 < plots[cand[b - 1]].f18 {
                cand[b] = cand[b - 1];
                b -= 1;
            }
            cand[b] = v;
        }
        let n0 = cand.len();
        let role = zone.record.sub;
        let take = |rng: &mut MsvcRand, cand: &mut Vec<usize>| {
            let r = rng.rand() as u32;
            cand.remove((r % cand.len() as u32) as usize)
        };
        if cell.kind == 1 {
            if role == 1 && n0 > 1 {
                let p = cand.pop().unwrap();
                plots[p].kind = 9;
            }
            if role == 3 {
                for t in [10, 11, 12, 13] {
                    if !cand.is_empty() {
                        let p = take(&mut self.rng, &mut cand); // 0x004e3232 / 0x004e3292 / 0x004e32f2 / 0x004e3352
                        plots[p].kind = t;
                    }
                }
            }
            if role == 2 {
                for t in [14, 16, 15] {
                    if !cand.is_empty() {
                        let p = take(&mut self.rng, &mut cand); // 0x004e33c2 / 0x004e3422 / 0x004e3482
                        plots[p].kind = t;
                    }
                }
            }
            if role == 3 && cand.len() > 1 {
                let p = cand.pop().unwrap();
                plots[p].kind = 3;
            }
            if (role == 0 || role == 3) && cand.len() > 1 {
                let p = cand.pop().unwrap();
                plots[p].class = 1;
            }
            if role == 1 {
                for k in 0..4 {
                    let Some(p) = cand.pop() else { break };
                    plots[p].class = k + 2;
                }
            }
            if role == 0 {
                let p = take(&mut self.rng, &mut cand); // 0x004e35bf
                plots[p].kind = 5;
            }
            for _ in 0..6 {
                if cand.len() > 3 {
                    let p = take(&mut self.rng, &mut cand); // 0x004e3640
                    plots[p].kind = 0;
                }
            }
        } else if cell.kind == 5 {
            let cx = crate::climate::div_trunc((cell.x / 65536) as i32, 256);
            let cy = crate::climate::div_trunc((cell.y / 65536) as i32, 256);
            if zx == cx && zy == cy && n0 > 1 {
                let p = cand.pop().unwrap();
                plots[p].kind = 0x14;
            }
            for _ in 0..2 {
                if cand.len() > 3 {
                    let p = take(&mut self.rng, &mut cand); // 0x004e3760
                    plots[p].kind = 0;
                }
            }
            if cell.variant == 0 {
                for _ in 0..2 {
                    if cand.len() > 3 {
                        let p = take(&mut self.rng, &mut cand); // 0x004e37e8
                        plots[p].kind = 0x12;
                        plots[p].dir = self.rng.rand() % 4; // 0x004e3812
                    }
                }
            } else if cell.variant == 2 {
                if self.rng.rand() % 2 != 0 && cand.len() > 3 {
                    // 0x004e3867
                    let p = take(&mut self.rng, &mut cand); // 0x004e3883
                    plots[p].kind = 0x11;
                    plots[p].dir = self.rng.rand() % 4; // 0x004e38b7
                }
                if self.rng.rand() % 2 != 0 && cand.len() > 3 {
                    // 0x004e3906
                    let p = take(&mut self.rng, &mut cand); // 0x004e3952
                    plots[p].kind = 0x13;
                    plots[p].dir = self.rng.rand() % 4; // 0x004e39ae
                }
            }
        }

        // Phase E: per-plot build.
        let mut guard_posts: Vec<[i32; 3]> = Vec::new();
        let mut tree_spots: Vec<[i32; 3]> = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let idx = (j * n + i) as usize;
                let x0 = zx * 256 + (i << 8) / n;
                let y0 = zy * 256 + (j << 8) / n;
                let cx = x0 + h;
                let cy = y0 + h;
                let p = plots[idx];
                // Type 8 is never assigned here; its branch (FUN_00513400 rings) is dead.
                if p.kind == 9 {
                    self.settlement_guard_post(zone, &mut guard_posts, x0, y0, cx, cy, s, p.max);
                }
                if p.kind == 6 {
                    self.settlement_farm(zone, out, x0, y0, s, h);
                }
                // climateA / climateB of (cx, cy) are computed here with the results discarded.
                if p.kind == 2 {
                    self.settlement_house(zone, cell, models, out, &p, i, j, n);
                } else if 0.2 < p.f18 {
                    for a in [0, s] {
                        for b in [0, s] {
                            if self.rng.rand() % 8 == 0 {
                                // 0x004eda52
                                let x = x0 + a / 2 + s / 4;
                                let y = y0 + b / 2 + s / 4;
                                let Some(c) = col(zone, x, y) else { continue };
                                let mut z = c.f14;
                                while solid(zone.block(x, y, z)) {
                                    z += 1;
                                }
                                let mut sp = Spawn { x: from_block(x), y: from_block(y), z: from_block(z + 1), ..Spawn::NEW };
                                sp.rotation = rand_rot(self.rng.rand()); // 0x004edbd3
                                let r = self.rng.rand() as u32; // 0x004edc02
                                sp.entity_type = species[(r % species.len() as u32) as usize];
                                let (lo, hi) = species_level_range(sp.entity_type);
                                sp.level = self.rng.rand() % (hi - lo + 1) + lo; // 0x004edc3a
                                sp.f28 = 5;
                                sp.b58 = cell.level_extra as u8;
                                zone.spawns.push(sp);
                            }
                        }
                    }
                }
                let zero = [0i32; 4];
                if p.kind == 3 {
                    let m = model_at(models, 0x881);
                    self.settlement_place(zone, m, [cx - 16, cy - 16, p.max - 7], p.dir, 6, 7, true, zero);
                }
                if p.kind == 4 {
                    let m = model_at(models, 0x887);
                    self.settlement_place(zone, m, [cx - 16, cy - 16, p.max - 5], p.dir, 6, 7, true, zero);
                    self.settlement_airship(zone, cell, out, &p, cx, cy);
                }
                if (10..=16).contains(&p.kind) {
                    let (id, kind) = match p.kind - 0xb {
                        0 => (0x900, 10),
                        1 => (0x901, 11),
                        2 => (0x902, 12),
                        3 => (0x903, 13),
                        4 => (0x905, 15),
                        5 => (0x904, 14),
                        _ => (0x8ff, 9),
                    };
                    let m = model_at(models, id);
                    let [w, d, _] = msize(m);
                    let mut z = 0;
                    for a in 0..w {
                        for b in 0..d {
                            if let Some(c) = col(zone, cx - 16 + a, cy - 16 + b)
                                && z < col_top(c)
                            {
                                    z = col_top(c);
                            }
                        }
                    }
                    self.settlement_place(zone, m, [cx - 16, cy - 16, z - 7], p.dir, 6, kind, true, zero);
                }
                if p.kind == 5 {
                    let m = model_at(models, 0x882);
                    let [w, d, _] = msize(m);
                    let mut pos = [cx - w / 2, cy - d / 2, p.max];
                    while !solid(zone.block(pos[0], pos[1], pos[2])) {
                        pos[2] -= 1;
                    }
                    self.settlement_place(zone, m, pos, p.dir, 6, 8, true, zero);
                }
                if p.kind == 0x11 || p.kind == 0x13 {
                    let m = model_at(models, if p.kind == 0x11 { 0xa04 } else { 0xa05 });
                    let pos = self.settlement_place_centered(zone, m, &p, cx, cy);
                    let (x, y) = (pos[0], pos[1]);
                    let c = col(zone, x, y).expect("corner column lies in the zone");
                    let mut z = col_top(c);
                    while !solid(zone.block(x, y, z)) {
                        z -= 1;
                    }
                    z += 1;
                    let item = random_item(&mut self.rng);
                    let gi = GroundItem {
                        item,
                        x: add_double(from_block(x), 0.5),
                        y: add_double(from_block(y), 0.5),
                        z: from_block(z),
                        // +0x130 is not written by the ctor or here (uninitialised in the original).
                        rotation: 0.0,
                        f134: f32::from_bits(0x3d924925),
                        b138: 0,
                        ..GroundItem::NEW
                    };
                    out.items.push((zone.items.len(), item));
                    zone.items.push(gi);
                }
                if p.kind == 0x14 {
                    let r = self.rng.rand() % 2; // 0x004eee49
                    let m = model_at(models, 0x84c + r as u32);
                    self.settlement_place_centered(zone, m, &p, cx, cy);
                }
                if p.kind == 0x12 {
                    let r = self.rng.rand() % 4; // 0x004ef038
                    let m = model_at(models, 0x84c + r as u32);
                    self.settlement_place_centered(zone, m, &p, cx, cy);
                }
            }
        }

        // Phase F: trees and plazas on empty plots.
        for i in 0..n {
            for j in 0..n {
                let idx = (j * n + i) as usize;
                if plots[idx].kind != 0 && plots[idx].kind != 7 {
                    continue;
                }
                let mut any = false;
                for a in [0, 256] {
                    for b in [0, 256] {
                        let x = zx * 256 + (i << 8) / n + (a / n) / 2 + s / 4;
                        let y = zy * 256 + (j << 8) / n + (b / n) / 2 + s / 4;
                        if !(f64::from(falloff(cell, from_block(x), from_block(y))) >= 0.72) {
                            continue;
                        }
                        let mut g = col(zone, x, y).expect("tree column lies in the zone").height;
                        while solid(zone.block(x, y, g)) {
                            g += 1;
                        }
                        if cell.kind == 1 {
                            if btype(zone.block(x, y, g - 1)) != 0xb {
                                continue;
                            }
                            self.settlement_plaza(zone, x, y, &mut g);
                        }
                        any = true;
                        let kind = if self.settlement_climate_b(zone, x, y) > 0.8 { 3 } else { 5 };
                        if g > 1 {
                            let size = self.rng.rand() % 8 + 13; // 0x004ef932
                            let height = self.rng.rand() % 6 + 7; // 0x004ef948
                            self.generate_tree(zone, x, y, g + 1, height, size, kind);
                        }
                        if cell.kind == 1 {
                            self.settlement_benches(zone, x, y, g);
                        }
                    }
                }
                if any {
                    let x = zx * 256 + (i << 8) / n + s / 2;
                    let y = zy * 256 + (j << 8) / n + s / 2;
                    let mut z = col(zone, x, y).expect("plot centre lies in the zone").height;
                    while solid(zone.block(x, y, z)) {
                        z += 1;
                    }
                    tree_spots.push([x, y, z]);
                }
            }
        }

        if cell.kind == 1 {
            self.settlement_town_npcs(zone, cell, out, &guard_posts, &tree_spots);
        } else {
            self.settlement_guard_npcs(zone, cell, out, &plots, n, s);
        }
        // The spawn fields the extras carry that `Spawn` now stores: the pilot's radius, the
        // villagers' AI mode and target zone.
        for ex in &out.spawns {
            if let Some(sp) = zone.spawns.get_mut(ex.spawn_index) {
                if let Some(r) = ex.f08 {
                    sp.radius = r;
                }
                if let Some(m) = ex.ai_mode {
                    sp.f5c = m;
                }
                if let Some(t) = ex.target {
                    sp.f60 = t[0];
                    sp.f64 = t[1];
                }
            }
        }
        // `zone+0x88`: the houses stay on the zone (villager schedules and 0x004d4c20 use them).
        zone.houses = out.houses.clone();
    }

    /// Type 0x11/0x12/0x13/0x14 placement: the model's footprint is swapped for odd
    /// directions, centred on the plot, and lowered onto the ground. Returns the position.
    fn settlement_place_centered(&mut self, zone: &mut Zone, m: Option<&Model>, p: &Plot, cx: i32, cy: i32) -> [i32; 3] {
        let [mut w, mut d, _] = msize(m);
        if p.dir % 2 != 0 {
            std::mem::swap(&mut w, &mut d);
        }
        let mut pos = [cx - w / 2, cy - d / 2, p.max];
        while !solid(zone.block(pos[0], pos[1], pos[2])) {
            pos[2] -= 1;
        }
        self.settlement_place(zone, m, pos, p.dir, 6, 0, true, [0; 4]);
        pos
    }

    /// Plot type 9: a guard post / barracks yard with up to 20 props.
    #[allow(clippy::too_many_arguments)]
    fn settlement_guard_post(&mut self, zone: &mut Zone, posts: &mut Vec<[i32; 3]>, x0: i32, y0: i32, cx: i32, cy: i32, s: i32, z: i32) {
        if let Some(c) = col(zone, cx, cy) {
            posts.push([cx, cy, col_top(c)]);
        }
        // (kind, x or None, y or None, orient): the jittered coordinate is `None`.
        // Order of the 20 attempts; rand order per attempt: gate, jitter, constructor.
        #[derive(Clone, Copy)]
        enum J {
            Near,
            Far,
        }
        let attempts: [(bool, Option<i32>, Option<i32>, J, i32); 20] = [
            (false, Some(cx - 7), None, J::Near, 2),
            (false, Some(cx), None, J::Near, 2),
            (false, Some(cx + 7), None, J::Near, 2),
            (false, Some(cx - 7), None, J::Far, 0),
            (false, Some(cx), None, J::Far, 0),
            (false, Some(cx + 7), None, J::Far, 0),
            (false, None, Some(cy - 7), J::Far, 3),
            (false, None, Some(cy), J::Far, 3),
            (false, None, Some(cy + 7), J::Far, 3),
            (false, None, Some(cy - 7), J::Near, 1),
            (false, None, Some(cy), J::Near, 1),
            (false, None, Some(cy + 7), J::Near, 1),
            (true, Some(cx - 3), None, J::Near, 2),
            (true, Some(cx + 3), None, J::Near, 2),
            (true, Some(cx - 3), None, J::Far, 0),
            (true, Some(cx + 3), None, J::Far, 0),
            (true, None, Some(cy - 3), J::Far, 3),
            (true, None, Some(cy + 3), J::Far, 3),
            (true, None, Some(cy - 3), J::Near, 1),
            (true, None, Some(cy + 3), J::Near, 1),
        ];
        for (yard, fx, fy, jit, orient) in attempts {
            if self.rng.rand() % 5 == 0 {
                // gates 0x004e3ea7, 0x004e4012, 0x004e4172, 0x004e42dd, 0x004e43a9, 0x004e446b, 0x004e4537,
                // 0x004e4605, 0x004e46c7, 0x004e4795, 0x004e485b, 0x004e4915, 0x004e49db, 0x004e4a9f, 0x004e4b63,
                // 0x004e4c2f, 0x004e4cfb, 0x004e4dc9, 0x004e4e97, 0x004e4f5d
                continue;
            }
            let r = self.rng.rand() % 3; // jitters: the second address of each pair (0x004e3ee5, 0x004e4050, ... 0x004e4fad)
            let (x, y) = match (fx, fy) {
                (Some(x), _) => (x, match jit { J::Near => y0 + r + 6, J::Far => y0 + (s - r) - 6 }),
                (None, Some(y)) => (match jit { J::Near => x0 + r + 6, J::Far => x0 + (s - r) - 6 }, y),
                _ => unreachable!(),
            };
            let pos = [from_block(x), from_block(y), from_block(z)];
            let mut ent = if yard { yard_prop(&mut self.rng, pos, orient) } else { guard_prop(&mut self.rng, pos, orient) };
            if self.settle_static(zone, &mut ent, true) {
                zone.statics.push(ent);
            }
        }
    }

    /// Plot type 6: a crop field with a farmer, crops, items and fences.
    fn settlement_farm(&mut self, zone: &mut Zone, out: &mut SettlementExtras, x0: i32, y0: i32, s: i32, h: i32) {
        let row_axis = self.rng.rand() % 2; // 0x004e503a
        let mut item_sub = 0u8;
        let mut obj_type: i32;
        let mut obj_int = 0i32;
        let mut obj_size = f32::from_bits(0x3dcccccd);
        let crop = self.rng.rand() % 6; // 0x004e507d
        for u in 0..s {
            for v in 0..s {
                match crop {
                    0 => {
                        item_sub = 0x10;
                        obj_type = 0x17;
                    }
                    1 => {
                        obj_size = f32::from_bits(0x3e4ccccd);
                        obj_type = 0x19;
                        obj_int = 4;
                    }
                    2 => {
                        obj_size = f32::from_bits(0x3e4ccccd);
                        obj_type = 0x1a;
                    }
                    3 => {
                        let r = self.rng.rand(); // crop switch case 3 (jump table 0x004f2b48 entry 3, call through a register)
                        obj_int = 4;
                        if r % 10 == 0 {
                            obj_size = f32::from_bits(0x3d4ccccd);
                            obj_type = 0x15;
                        } else {
                            obj_size = f32::from_bits(0x3e4ccccd);
                            obj_type = 0x1d;
                        }
                    }
                    4 => {
                        obj_size = f32::from_bits(0x3e4ccccd);
                        obj_type = 0x1e;
                        obj_int = 4;
                    }
                    _ => {
                        obj_size = f32::from_bits(0x3e19999a);
                        item_sub = 0x11;
                        obj_type = 0x18;
                    }
                }
                let x = x0 + u;
                let y = y0 + v;
                let Some(c) = col(zone, x, y) else { continue };
                let count = c.blocks.len() as i32;
                if count == 0 || btype(c.block_at(count - 1)) != 4 {
                    continue;
                }
                let base = c.height;
                let k = if row_axis == 0 { u } else { v };
                let stripe = ((k as u32 >> 1) & 1) as f32;
                let sv = [10.0 * stripe, 10.0 * stripe, 0.0 * stripe];
                let rc = self.rock_color(x, y, base + count - 1);
                let c7 = [rc[0] * 0.7, rc[1] * 0.7, rc[2] * 0.7];
                let blk = rgb_block(clamp255([sv[0] + c7[0], sv[1] + c7[1], sv[2] + c7[2]]), 5);
                zone.column_mut(x, y).set_raw(count - 1, blk);
                let top = base + count;
                if u == h && v == h {
                    let mut sp = Spawn { f28: 6, entity_type: 0x8c, ..Spawn::NEW };
                    sp.level = self.spawn_level(from_block(x), from_block(y));
                    sp.x = add_double(from_block(x), 0.5);
                    sp.y = add_double(from_block(y), 0.5);
                    sp.z = from_block(top);
                    sp.rotation = (self.rng.rand() % 4) as f32 * 90.0; // 0x004e54a2
                    zone.spawns.push(sp);
                } else if self.rng.rand() % 10 == 0 {
                    // 0x004e54e2
                    let pos = [from_block(x) + f_to_fix(0.5), from_block(y) + f_to_fix(0.5), from_block(top) + f_to_fix(0.0)];
                    if self.rng.rand() % 4 == 0 && item_sub != 0 {
                        // 0x004e54f8
                        let rotation = rand_rot(self.rng.rand()); // 0x004e55dd
                        let item = Item { item_type: 0xb, sub_type: item_sub, level: 1, ..Item::NEW };
                        out.items.push((zone.items.len(), item));
                        // +0x138 = 2 is also written (not tracked).
                        zone.items.push(GroundItem {
                            item,
                            x: pos[0],
                            y: pos[1],
                            z: pos[2],
                            rotation,
                            f134: f32::from_bits(0x3dcccccd),
                            b138: 2,
                            ..GroundItem::NEW
                        });
                    } else if obj_type != 0 {
                        let rotation = if obj_type == 0x19 { 0.0 } else { rand_rot(self.rng.rand()) }; // 0x004e5662
                        zone.props.push(Prop { kind: obj_type as u32, x: pos[0], y: pos[1], z: pos[2], scale: obj_size, rotation, flags: obj_int as u32, ..Prop::NEW });
                    }
                }
            }
        }
        // Fences along the two near edges.
        for axis in 0..2 {
            if self.rng.rand() % 2 == 0 {
                // 0x004e5784 / 0x004e5995
                continue;
            }
            for k in 0..h {
                if self.rng.rand() % 10 == 0 {
                    // 0x004e57b3 / 0x004e5a83
                    continue;
                }
                let (x, y) = if axis == 0 { (x0 + 2 * k + 1, y0) } else { (x0, y0 + 2 * k + 1) };
                let Some(c) = col(zone, x, y) else { continue };
                let count = c.blocks.len() as i32;
                if count == 0 || btype(c.block_at(count - 1)) == 0 {
                    continue;
                }
                let top = col_top(c);
                let mut st = Static::NEW;
                st.kind = (self.rng.rand() % 4 + 0x34) as u32; // 0x004e587a / 0x004e5b7f
                st.x = from_block(x) + f_to_fix(0.0);
                st.y = from_block(y) + f_to_fix(0.5);
                st.z = from_block(top) + f_to_fix(0.0);
                st.rotation = axis;
                st.scale = [2.0, 1.0, 1.5];
                zone.statics.push(st);
            }
        }
    }

    /// Plot type 4 extra: an airship moored next to the tower when a neighbouring region's
    /// climate point lies above water.
    fn settlement_airship(&mut self, zone: &mut Zone, cell: &Cell, out: &mut SettlementExtras, p: &Plot, cx: i32, cy: i32) {
        let rx = crate::climate::div_trunc(zone.x, 64);
        let ry = crate::climate::div_trunc(zone.y, 64);
        let mut any = false;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                if let Some(pt) = self.climate_point(rx + dx, ry + dy)
                    && pt.elevation > 0
                {
                        any = true;
                }
            }
        }
        if !any {
            return;
        }
        let heading = (p.dir * 90) as f32;
        let a = (f64::from(p.dir + 2) * std::f64::consts::PI * 0.5) as f32;
        let sa = cw_math::sin(f64::from(a)) as f32;
        let y = (sa * 19.0 + cy as f32) as i32;
        let ca = cw_math::cos(f64::from(a)) as f32;
        let x = (ca * 19.0 + cx as f32) as i32;
        let anchor = fix3([x, y, p.max + 0x42]);
        let r = ((f64::from(-heading) * std::f64::consts::PI) / 180.0) as f32;
        let cr = cw_math::cos(f64::from(r)) as f32 * -100.0;
        let sr = cw_math::sin(f64::from(r)) as f32 * -100.0;
        let off = [f_to_fix(sr), f_to_fix(cr), f_to_fix(100.0)];
        let pos = [anchor[0] + off[0], anchor[1] + off[1], anchor[2] + off[2]];
        let pos2 = [anchor[0] + f_to_fix(50.0), anchor[1] + f_to_fix(50.0), anchor[2] + f_to_fix(20.0)];
        let ship = Airship { pos, rotation: heading, anchor, heading, pos2 };
        let mut sp = Spawn { x: pos[0], y: pos[1], z: pos[2], f28: 3, f30: 0x88, entity_type: 4, ..Spawn::NEW };
        sp.level = cell.level;
        sp.rotation = ship.rotation;
        // 0x004ee214: `Sequence[LookAtPlayer]`.
        sp.ai = Some(SpawnAi::Pilot);
        let id = airship_id(zone.x, zone.y, out.airships.len() as i32);
        out.spawns.push(SpawnExtra {
            spawn_index: zone.spawns.len(),
            f08: Some(512.0),
            id: Some(id),
            ai: Some(Ai::Pilot),
            ..SpawnExtra::default()
        });
        zone.spawns.push(sp);
        out.airships.push(ship);
    }

    /// Tree plaza (town): flattens a disc of radius 8 around `(x + 0.5, y + 0.5)`. `g` is
    /// raised to the highest ground of the disc.
    fn settlement_plaza(&mut self, zone: &mut Zone, x: i32, y: i32, g: &mut i32) {
        let ground = |zone: &Zone, px: i32, py: i32| {
            let mut z = col(zone, px, py).expect("plaza column lies in the zone").height;
            while solid(zone.block(px, py, z)) {
                z += 1;
            }
            z
        };
        for px in x - 8..=x + 8 {
            let dx = (f64::from(x) + 0.5) - f64::from(px);
            for py in y - 8..=y + 8 {
                let dy = (f64::from(y) + 0.5) - f64::from(py);
                if dx * dx + dy * dy <= 64.0 {
                    let g2 = ground(zone, px, py);
                    if *g < g2 {
                        *g = g2;
                    }
                }
            }
        }
        for px in x - 8..=x + 8 {
            let dx = (f64::from(x) + 0.5) - f64::from(px);
            for py in y - 8..=y + 8 {
                let dy = (f64::from(y) + 0.5) - f64::from(py);
                let d2 = dx * dx + dy * dy;
                if d2 > 64.0 {
                    continue;
                }
                let g2 = ground(zone, px, py);
                let ca = self.climate_a(px, py);
                let cb = self.climate_b(px, py);
                for z in g2 - 1..=*g {
                    let blk = if d2 < 49.0 {
                        // Note: the centre and the plaza height, not (px, py, z).
                        self.surface_block(x, y, *g, ca, cb)
                    } else {
                        let r = self.rng.rand(); // 0x004ef7c2
                        let f = (r as f32 * 0.1) / 32767.0 + 0.75;
                        let c = self.settlement_stone_color(px, py, z);
                        rgb_block([c[0] * f, c[1] * f, c[2] * f], 6)
                    };
                    set_block(self, zone, px, py, z, blk);
                }
            }
        }
    }

    /// Four benches around a town tree, each dropped with probability 1/8.
    fn settlement_benches(&mut self, zone: &mut Zone, x: i32, y: i32, g: i32) {
        let xs = add_double(from_block(x), 0.5);
        let ys = add_double(from_block(y), 0.5);
        let spots = [
            (xs, ys - from_block(8), 0),
            (xs, ys + from_block(8) + from_block(1), 2),
            (xs - from_block(8), ys, 3),
            (xs + from_block(8) + from_block(1), ys, 1),
        ];
        for (bx, by, rot) in spots {
            if self.rng.rand() % 8 == 0 {
                // 0x004ef985 and the three after it
                continue;
            }
            let mut st = Static { kind: 0x12, x: bx, y: by, z: from_block(g), rotation: rot, ..Static::NEW };
            st.scale = [3.0, f32::from_bits(0x3ecccccd), f32::from_bits(0x3ecccccd)];
            if self.settle_static(zone, &mut st, true) {
                zone.statics.push(st);
            }
        }
    }

    /// Plot type 2: one house (section 3b of the notes).
    #[allow(clippy::too_many_arguments)]
    fn settlement_house(
        &mut self,
        zone: &mut Zone,
        cell: &Cell,
        models: &[Model],
        out: &mut SettlementExtras,
        p: &Plot,
        i: i32,
        j: i32,
        n: i32,
    ) {
        let town = cell.kind == 1;
        // Prefab slots P0..P13.
        let ids: [u32; 14] = if town {
            let base = match cell.variant {
                1 => 0x8ef,
                2 => 0x8a7,
                3 => 0x89a,
                4 => 0x8b4,
                5 => 0x8c3,
                _ => 0x88d,
            };
            let door = match cell.variant {
                4 => 0x8c1,
                5 => 0x8c2,
                _ => 0x889,
            };
            let mut ids = [door; 14];
            for (k, id) in ids.iter_mut().enumerate().skip(1) {
                *id = base + k as u32 - 1;
            }
            ids
        } else if cell.variant == 2 {
            [0x8e4, 0x8e5, 0x8e6, 0x8e7, 0x8e8, 0x8e8, 0x8e9, 0x8ea, 0x8e9, 0x8e9, 0x8eb, 0x8ec, 0x8ed, 0x8ee]
        } else if cell.variant == 3 {
            [0x8d9, 0x8da, 0x8db, 0x8dc, 0x8dd, 0x8dd, 0x8de, 0x8df, 0x8dd, 0x8dd, 0x8e0, 0x8e1, 0x8e2, 0x8e3]
        } else {
            [0x8d0, 0x8d1, 0x8d2, 0x8d3, 0x8d4, 0x8d4, 0x8d4, 0x8d4, 0x8d4, 0x8d4, 0x8d5, 0x8d6, 0x8d7, 0x8d8]
        };
        let pm: Vec<Option<&Model>> = ids.iter().map(|&id| model_at(models, id)).collect();

        let mut hs = House::new(3, 3, 4);
        let statics_start = zone.statics.len();
        hs.class = p.class;
        self.settlement_house_layout(&mut hs, p.class);
        hs.dir = p.dir;
        hs.mirror = self.rng.rand() % 2 == 0; // 0x004e6fa6

        let zx = zone.x;
        let zy = zone.y;
        let px0 = zx * 256 + (i << 8) / n;
        let py0 = zy * 256 + (j << 8) / n;
        let (dx, dy, dz) = (hs.dim_x(), hs.dim_y(), hs.dim_z());

        // Height of each ground-floor room.
        let mut max_h = 0;
        for x in 0..dx {
            for y in 0..dy {
                let c1 = hs.cell(x, y, 1);
                if c1.t == 1 && hs.cell(x, y, 2).b3 == 0 && c1.b8 != 3 {
                    let mut hh = 0;
                    for a in 0..13 {
                        for b in 0..13 {
                            if let Some(c) = col(zone, px0 + 7 + x * 13 + a, py0 + 7 + y * 13 + b)
                                && hh < col_top(c)
                            {
                                    hh = col_top(c);
                            }
                        }
                    }
                    hs.edit(x, y, 1, |c| c.h = hh);
                    if max_h < hh {
                        max_h = hh;
                    }
                }
            }
        }
        // Door.
        let mut doors: Vec<[i32; 3]> = Vec::new();
        for x in 0..dx {
            for y in 0..dy {
                let c1 = hs.cell(x, y, 1);
                if c1.t == 1 && hs.cell(x, y, 2).b3 == 0 && c1.h == max_h && c1.b8 != 3 {
                    doors.push([x, y, 1]);
                }
            }
        }
        if !doors.is_empty() {
            let r = self.rng.rand() as u32; // 0x004e731b
            let d = doors[(r % doors.len() as u32) as usize];
            hs.edit(d[0], d[1], d[2], |c| c.door = 1);
        }
        // Rotations.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let rot = if hs.cell(x, y, z).t == 5 {
                        p.dir % 2
                    } else {
                        self.rng.rand() % 4 // 0x004e7428
                    };
                    hs.edit(x, y, z, |c| c.rot = rot as u8);
                }
            }
        }
        // Base height from the door's surroundings.
        let mut off = 0;
        let mut first = true;
        let mut z = dz - 1;
        while z >= 0 {
            for x in 0..dx {
                for y in 0..dy {
                    if hs.cell(x, y, z).door == 0 {
                        continue;
                    }
                    for a in 0..15 {
                        for b in 0..15 {
                            if let Some(c) = col(zone, px0 + 6 + x * 13 + a, py0 + 6 + y * 13 + b) {
                                let v = c.height - max_h + c.blocks.len() as i32;
                                if first || off < v {
                                    first = false;
                                    off = v;
                                }
                            }
                        }
                    }
                }
            }
            z -= 1;
        }
        hs.pos = [px0 + 7, py0 + 7, max_h + off - 6];
        let [hx, hy, hz] = hs.pos;

        // Paving bounds.
        let bx0 = hx - 2;
        let by0 = hy - 2;
        let mut bx1 = hx - 2;
        let mut by1 = hy - 2;
        for x in 0..dx {
            for y in 0..dy {
                if hs.cell(x, y, 0).t != 0 {
                    if bx1 < hx + x * 13 + 13 {
                        bx1 = hx + x * 13 + 13;
                    }
                    if by1 < hy + y * 13 + 13 {
                        by1 = hy + y * 13 + 13;
                    }
                }
            }
        }
        let bx1 = bx1 + 2;
        let by1 = by1 + 2;
        if town && cell.variant != 4 && cell.variant != 5 {
            self.settlement_pave(zone, [bx0, by0, bx1, by1], hz);
        }

        let floors = match p.class {
            1 => 2,
            2 => 3,
            3 => 4,
            4 => 5,
            5 => 6,
            _ => 1,
        };
        let zero = [0i32; 4];
        let dpos = |m: Option<&Model>, x: i32, y: i32, zz: i32| {
            let [w, d, _] = msize(m);
            [hx + (x * 13 - w / 2) + 7, hy + 7 + (y * 13 - d / 2), zz]
        };

        // Walls.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let c = hs.cell(x, y, z);
                    if c.t == 1 {
                        // (neighbour, rotation, door position)
                        let sides: [((i32, i32), i32); 4] = [((x, y - 1), 0), ((x, y + 1), 2), ((x - 1, y), 1), ((x + 1, y), 3)];
                        for ((nx, ny), rot) in sides {
                            let nb = hs.cell(nx, ny, z);
                            if nb.t == 0 {
                                let below = hs.cell(nx, ny, z - 1).t;
                                if c.door == 0 && (below == 0 || below == 3) {
                                    let m = if z == 1 {
                                        if self.rng.rand() % 6 == 0 {
                                            // sides y-1 / y+1 / x-1 / x+1: 0x004e7fd3 / 0x004e85ac / 0x004e8b85 / 0x004e9157
                                            pm[9]
                                        } else {
                                            pm[5]
                                        }
                                    } else if z >= 2 && self.rng.rand() % 6 == 0 {
                                        // sides y-1 / y+1 / x-1 / x+1: 0x004e80a7 / 0x004e8680 / 0x004e8c58 / 0x004e922b
                                        pm[8]
                                    } else {
                                        pm[5]
                                    };
                                    let pos = dpos(m, x, y, hz + z * 7);
                                    self.settlement_place(zone, m, pos, rot, 6, 1, false, zero);
                                } else {
                                    let pos = dpos(pm[6], x, y, hz + z * 7);
                                    self.settlement_place(zone, pm[6], pos, rot, 6, floors, false, zero);
                                    if c.door != 0 {
                                        let [w0, d0, _] = msize(pm[0]);
                                        let zz = z * 7 + hz - 4;
                                        let pos = match rot {
                                            0 => [x * 13 + hx + (7 - w0 / 2), y * 13 + ((hy - 6) - d0 / 2), zz],
                                            2 => [x * 13 + hx + (7 - w0 / 2), y * 13 + ((hy + 0x14) - d0 / 2), zz],
                                            1 => [x * 13 + hx + (-6 - w0 / 2), y * 13 + ((hy + 7) - d0 / 2), zz],
                                            _ => [x * 13 + hx + (0x14 - w0 / 2), y * 13 + ((hy + 7) - d0 / 2), zz],
                                        };
                                        self.settlement_place(zone, pm[0], pos, rot, 6, floors, true, zero);
                                    }
                                }
                            } else if nb.t != 1 {
                                let pos = dpos(pm[4], x, y, hz + z * 7);
                                self.settlement_place(zone, pm[4], pos, rot, 6, 1, false, zero);
                            }
                        }
                    } else if c.t == 2 {
                        let pos = dpos(pm[1], x, y, hz + z * 7);
                        self.settlement_place(zone, pm[1], pos, i32::from(c.rot), 6, 1, true, zero);
                    } else if c.t == 5 {
                        let pos = dpos(pm[13], x, y, hz + z * 7);
                        self.settlement_place(zone, pm[13], pos, i32::from(c.rot), 6, 1, false, zero);
                    }
                }
            }
        }

        // Roofs.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let c = hs.cell(x, y, z);
                    if c.t != 3 {
                        continue;
                    }
                    let mut mask = [0i32; 4];
                    if hs.cell(x - 1, y, z).t != 0 {
                        mask[0] = 1;
                    }
                    if hs.cell(x + 1, y, z).t != 0 {
                        mask[2] = 1;
                    }
                    if hs.cell(x, y - 1, z).t != 0 {
                        mask[1] = 1;
                    }
                    if hs.cell(x, y + 1, z).t != 0 {
                        mask[3] = 1;
                    }
                    let (na, nb) = match c.rot & 3 {
                        0 => ((x - 1, y), (x + 1, y)),
                        1 => ((x, y + 1), (x, y - 1)),
                        2 => ((x + 1, y), (x - 1, y)),
                        _ => ((x, y - 1), (x, y + 1)),
                    };
                    let join = |hs: &House, (qx, qy): (i32, i32)| {
                        let q = hs.cell(qx, qy, z);
                        if q.t == 3 {
                            if q.rot & 1 == c.rot & 1 { 12 } else { 11 }
                        } else {
                            10
                        }
                    };
                    let ma = pm[join(&hs, na)];
                    let mb = pm[join(&hs, nb)];
                    let pos = dpos(ma, x, y, hz + z * 7);
                    self.settlement_place(zone, ma, pos, i32::from(c.rot), 0xe, 1, false, mask);
                    let pos = dpos(mb, x, y, hz + z * 7);
                    self.settlement_place(zone, mb, pos, (i32::from(c.rot) + 2) % 4, 0xe, 1, false, mask);
                }
            }
        }

        // Floors and furniture models.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let c = hs.cell(x, y, z);
                    let zz = z * 7;
                    if c.t == 1 {
                        let slot = if c.b3 == 0 {
                            if town && self.rng.rand() % 2 == 0 && hs.cell(x, y, z + 1).b3 == 0 {
                                // 0x004ea24e
                                let r = self.rng.rand() % 3; // 0x004ea287
                                let fm = model_at(models, 0x88a + r as u32);
                                let rot = self.rng.rand() % 4; // right after 0x004ea287 (call through a register)
                                let [w, d, hgt] = msize(fm);
                                let pos = [x * 13 + hx + (7 - w / 2), y * 13 + ((hy + 7) - d / 2), zz + ((hz + 1) - hgt)];
                                self.settlement_place(zone, fm, pos, rot, 6, 1, false, zero);
                            }
                            2
                        } else {
                            3
                        };
                        let [w, d, hgt] = msize(pm[slot]);
                        let pos = [x * 13 + hx + (7 - w / 2), y * 13 + ((hy + 7) - d / 2), zz + hz + (1 - hgt)];
                        self.settlement_place(zone, pm[slot], pos, i32::from(c.rot), 6, 1, false, zero);
                    } else if c.t == 3 && hs.cell(x, y, z - 1).t == 1 {
                        let [w, d, hgt] = msize(pm[2]);
                        let pos = [x * 13 + hx + (7 - w / 2), y * 13 + ((hy + 7) - d / 2), zz + hz + (1 - hgt)];
                        self.settlement_place(zone, pm[2], pos, i32::from(c.rot), 6, 1, false, zero);
                    }
                }
            }
        }

        // Links between rooms.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let c = hs.cell(x, y, z);
                    if c.t != 1 || c.b8 != 1 {
                        continue;
                    }
                    for ((nx, ny), rot) in [((x, y - 1), 0), ((x - 1, y), 1), ((x, y + 1), 2), ((x + 1, y), 3)] {
                        if hs.cell(nx, ny, z).t == 1 {
                            let pos = dpos(pm[7], x, y, hz + z * 7);
                            self.settlement_place(zone, pm[7], pos, rot, 6, 1, false, zero);
                        }
                    }
                }
            }
        }

        // Carve the interiors and collect the spots.
        for x in 0..dx {
            for y in 0..dy {
                for z in 0..dz {
                    let c = hs.cell(x, y, z);
                    if c.t != 1 {
                        continue;
                    }
                    for a in 0..14 {
                        for b in 0..14 {
                            let mut k = 5;
                            while k >= 0 {
                                let (bx, by, bz) = (a + x * 13 + hx, hy + b + y * 13, hz + k + z * 7);
                                let blk = zone.block(bx, by, bz);
                                if blk[3] & 0x40 == 0 {
                                    set_block(self, zone, bx, by, bz, [blk[0], blk[1], blk[2], 0x40]);
                                }
                                k -= 1;
                            }
                        }
                    }
                    if hs.cell(x, y, z - 1).t == 2 {
                        let spot = [x * 13 + hx + 7, hy + y * 13 + 7, z * 7 + hz + 1];
                        if c.door != 0 {
                            hs.door_spots.push(spot);
                        }
                        if c.b8 == 3 {
                            hs.special_spots.push(spot);
                        }
                        hs.floor_spots.push(spot);
                    }
                }
            }
        }

        // Furnishing (lights, wall furniture, room furniture).
        if town {
            for x in 0..dx {
                for y in 0..dy {
                    for z in 0..dz {
                        self.settlement_furnish_room(zone, &hs, x, y, z);
                    }
                }
            }
        }

        for k in statics_start..zone.statics.len() {
            let kind = zone.statics[k].kind;
            if kind == 0x10 || kind == 0x12 {
                hs.seats.push([zx, zy, k as i32]);
            }
            if kind == 0x13 {
                hs.beds.push([zx, zy, k as i32]);
            }
        }

        // Exterior clutter.
        if town {
            for x in 0..dx {
                for y in 0..dy {
                    if hs.cell(x, y, 0).t != 2 {
                        continue;
                    }
                    let fz = from_block(hz + 1);
                    let bx = from_block(hx) + from_block(x * 13);
                    let by = from_block(hy) + from_block(y * 13);
                    let sides: [((i32, i32), [(i64, i64); 2], i32); 4] = [
                        ((x - 1, y), [(sub_double(from_block(hx), 1.5) + from_block(x * 13), by + from_block(4)), (sub_double(from_block(hx), 1.5) + from_block(x * 13), by + from_block(10))], 3),
                        ((x + 1, y), [(add_double(from_block(hx), 15.5) + from_block(x * 13), by + from_block(4)), (add_double(from_block(hx), 15.5) + from_block(x * 13), by + from_block(10))], 1),
                        ((x, y - 1), [(bx + from_block(4), sub_double(from_block(hy), 1.5) + from_block(y * 13)), (bx + from_block(10), sub_double(from_block(hy), 1.5) + from_block(y * 13))], 0),
                        ((x, y + 1), [(bx + from_block(4), add_double(from_block(hy), 15.5) + from_block(y * 13)), (bx + from_block(10), add_double(from_block(hy), 15.5) + from_block(y * 13))], 2),
                    ];
                    for ((nx, ny), spots, rot) in sides {
                        if hs.cell(nx, ny, 0).t != 0 {
                            continue;
                        }
                        for (sx, sy) in spots {
                            if self.rng.rand() % 6 == 0 {
                                // 0x004ed030, 0x004ed15a, 0x004ed276, 0x004ed3be, 0x004ed50d, 0x004ed62d, 0x004ed767,
                                // 0x004ed88f in side order
                                let mut ent = clutter_prop(&mut self.rng, [sx, sy, fz], rot);
                                if self.settle_static(zone, &mut ent, true) {
                                    zone.statics.push(ent);
                                }
                            }
                        }
                    }
                }
            }
        }
        out.houses.push(hs);
    }

    /// Room layout of `House(3, 3, 4)` for a house class, written untransformed.
    fn settlement_house_layout(&mut self, hs: &mut House, class: i32) {
        let col3 = |hs: &mut House, x: i32, y: i32, ts: &[u8]| {
            for (z, &t) in ts.iter().enumerate() {
                hs.set_t(x, y, z as i32, t);
            }
        };
        match class {
            2..=5 => {
                for (x, y) in [(0, 0), (0, 1), (1, 1), (1, 0)] {
                    hs.set_t(x, y, 0, 2);
                }
                for (x, y) in [(0, 0), (0, 1), (1, 1), (1, 0)] {
                    hs.set_t(x, y, 1, 1);
                    if (x, y) == (0, 1) {
                        hs.set_b8(0, 1, 1, 3);
                    }
                }
                for (x, y) in [(0, 0), (0, 1), (1, 1), (1, 0)] {
                    hs.set_t(x, y, 2, 3);
                }
            }
            1 => {
                for x in 0..3 {
                    hs.set_t(x, 0, 0, 2);
                    hs.set_t(x, 0, 1, 1);
                    hs.set_b8(x, 0, 1, 2);
                    hs.set_t(x, 0, 2, 1);
                    if x == 0 {
                        hs.set_b3(x, 0, 2, 1);
                    } else {
                        hs.set_b8(x, 0, 2, 1);
                    }
                    hs.set_t(x, 0, 3, 3);
                }
                hs.set_t(0, 1, 0, 2);
                hs.set_t(0, 1, 1, 1);
                hs.set_b8(0, 1, 1, 2);
                hs.set_t(0, 1, 2, 3);
                if self.rng.rand() % 2 != 0 {
                    // 0x004e675c
                    hs.set_t(2, 1, 0, 2);
                    hs.set_t(2, 1, 1, 1);
                    hs.set_b8(2, 1, 1, 2);
                    hs.set_t(2, 1, 2, 3);
                }
                hs.set_t(1, 1, 0, 2);
                hs.set_t(1, 1, 1, 1);
                hs.set_b8(1, 1, 1, 3);
                hs.set_t(1, 1, 2, 3);
                col3(hs, 1, 2, &[2, 1, 3]);
            }
            _ => match self.rng.rand() % 3 {
                // 0x004e6843
                0 => {
                    col3(hs, 0, 0, &[2, 1, 1]);
                    hs.set_b8(0, 0, 2, 1);
                    hs.set_b3(0, 0, 2, 1);
                    hs.set_t(0, 0, 3, 3);
                    col3(hs, 2, 0, &[2, 1, 1]);
                    hs.set_b3(2, 0, 2, 1);
                    hs.set_t(2, 0, 3, 3);
                    col3(hs, 0, 1, &[2, 1]);
                    if self.rng.rand() % 2 == 0 {
                        // 0x004e6dac
                        hs.set_t(0, 1, 2, 1);
                        hs.set_t(0, 1, 3, 3);
                    } else {
                        hs.set_t(0, 1, 2, 3);
                    }
                    if self.rng.rand() % 2 != 0 {
                        // 0x004e6de8
                        col3(hs, 2, 1, &[2, 1, 3]);
                    }
                    if self.rng.rand() % 2 == 0 {
                        // 0x004e6e38
                        col3(hs, 1, 0, &[2, 1, 3]);
                    } else {
                        hs.set_t(1, 0, 1, 5);
                    }
                }
                1 => {
                    col3(hs, 0, 0, &[2, 1, 3]);
                    col3(hs, 1, 0, &[2, 1, 3]);
                    col3(hs, 0, 1, &[2, 1, 3]);
                    hs.set_b8(0, 1, 1, 1);
                    col3(hs, 1, 1, &[2, 1]);
                    if self.rng.rand() % 2 != 0 {
                        // 0x004e6c09
                        hs.set_t(1, 1, 2, 1);
                        hs.set_b3(1, 1, 2, 1);
                        hs.set_t(1, 1, 3, 3);
                    } else {
                        hs.set_t(1, 1, 2, 3);
                    }
                    if self.rng.rand() % 2 != 0 {
                        // 0x004e6c5a
                        col3(hs, 0, 2, &[2, 1, 3]);
                    }
                }
                _ => {
                    if self.rng.rand() % 2 == 0 {
                        // 0x004e6868
                        col3(hs, 0, 0, &[2, 1, 1, 3]);
                        hs.set_b3(0, 0, 2, 1);
                        col3(hs, 1, 0, &[2, 1, 1, 3]);
                        col3(hs, 2, 0, &[2, 1, 1, 3]);
                        hs.set_b8(2, 0, 1, 1);
                    } else {
                        col3(hs, 0, 0, &[2, 1, 3]);
                        col3(hs, 1, 0, &[2, 1, 3]);
                        hs.set_b8(1, 0, 1, 1);
                        col3(hs, 2, 0, &[2, 1, 3]);
                    }
                    if self.rng.rand() % 2 != 0 {
                        // 0x004e6a47
                        col3(hs, 0, 1, &[2, 1, 3]);
                    }
                    if self.rng.rand() % 2 != 0 {
                        // 0x004e6a97
                        col3(hs, 2, 1, &[2, 1, 3]);
                    }
                    col3(hs, 1, 1, &[2, 1, 3]);
                }
            },
        }
    }

    /// Paving around a town house: raises the plaza ground (type 0xb) under the bounds to the
    /// house floor, with a noise-grey kerb on the border. Skipped entirely unless every column
    /// has plaza ground at or above the house base.
    fn settlement_pave(&mut self, zone: &mut Zone, [x0, y0, x1, y1]: [i32; 4], hz: i32) {
        let ground = |zone: &Zone, x: i32, y: i32| {
            let c = col(zone, x, y)?;
            let mut g = col_top(c) - 1;
            while !solid(zone.block(x, y, g)) {
                g -= 1;
            }
            Some(g)
        };
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(g) = ground(zone, x, y)
                    && (btype(zone.block(x, y, g)) != 0xb || 5 < (5 - g) + hz)
                {
                        return;
                }
            }
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                let Some(g) = ground(zone, x, y) else { continue };
                let blk = zone.block(x, y, g);
                if btype(blk) != 0xb {
                    continue;
                }
                let mut rgb = [f32::from(blk[0]), f32::from(blk[1]), f32::from(blk[2])];
                if x == x0 || x == x1 || y == y0 || y == y1 {
                    let nv = noise(f64::from(x) * 0.05 + 843.0, f64::from(y) * 0.05 + 984.0);
                    let a = [40.0 * nv, 40.0 * nv, 40.0 * nv];
                    let c = [a[0] + 140.0, a[1] + 140.0, a[2] + 140.0];
                    rgb = [c[0] * 0.7, c[1] * 0.7, c[2] * 0.7];
                }
                let mut z = g + 1;
                while z < hz + 6 {
                    set_block(self, zone, x, y, z, rgb_block(rgb, 0xb));
                    z += 1;
                }
            }
        }
    }

    /// Lights and furniture of one town room cell.
    fn settlement_furnish_room(&mut self, zone: &mut Zone, hs: &House, x: i32, y: i32, z: i32) {
        let [hx, hy, hz] = hs.pos;
        let c = hs.cell(x, y, z);
        if c.t == 1 && (x + y) % 2 != 0 {
            let pos = [
                from_block(hx) + from_block(7) + from_block(x * 13),
                from_block(hy) + from_block(7) + from_block(y * 13),
                add_double(from_block(hz) + from_block(z * 7), 5.3),
            ];
            zone.props.push(Prop {
                kind: 0xd,
                x: pos[0],
                y: pos[1],
                z: pos[2],
                scale: f32::from_bits(0x3d99999a),
                rotation: 0.0,
                f2c: [f32::from_bits(0x3f19999a), 0.5, f32::from_bits(0x3ecccccd)],
                flags: 1,
                ..Prop::NEW
            });
        }
        let b8 = c.b8;
        if c.t != 1 || hs.cell(x, y, z + 1).b3 != 0 {
            return;
        }
        let zz = from_block(z * 7 + hz + 1);
        // Places a wall piece if the block half a block under it is solid (no settle).
        let wall = |w: &mut World, zone: &mut Zone, px: i64, py: i64, rot: i32| {
            let ent = wall_furniture(&mut w.rng, [px, py, zz], rot, i32::from(b8));
            if solid(zone.block(to_block(ent.x), to_block(ent.y), to_block(sub_double(ent.z, 0.5)))) {
                zone.statics.push(ent);
            }
        };
        let xo = |d: f64| add_double(from_block(hx), d) + from_block(x * 13);
        let yo = |d: f64| add_double(from_block(hy), d) + from_block(y * 13);
        if hs.cell(x - 1, y, z).t != 1 {
            if self.rng.rand() % 3 != 0 {
                // 0x004eafd0
                wall(self, zone, xo(1.5), yo(4.5), 1);
            }
            if self.rng.rand() % 3 != 0 {
                // 0x004eb160
                wall(self, zone, xo(1.5), yo(9.5), 1);
            }
        }
        if hs.cell(x + 1, y, z).t != 1 {
            if self.rng.rand() % 3 != 0 {
                // 0x004eb313
                wall(self, zone, xo(12.5), yo(4.5), 3);
            }
            if self.rng.rand() % 3 != 0 {
                // 0x004eb4a3
                wall(self, zone, xo(12.5), yo(9.5), 3);
            }
        }
        if hs.cell(x, y - 1, z).t != 1 {
            wall(self, zone, xo(4.5), yo(1.5), 2);
        }
        if hs.cell(x, y + 1, z).t != 1 {
            if self.rng.rand() % 3 != 0 {
                // 0x004eb7ed
                wall(self, zone, xo(4.5), yo(12.5), 0);
            }
            if self.rng.rand() % 3 != 0 {
                // 0x004eb97b
                wall(self, zone, xo(9.5), yo(12.5), 0);
            }
        }
        let xi = |d: i32| from_block(hx) + from_block(d) + from_block(x * 13);
        let yi = |d: i32| from_block(hy) + from_block(d) + from_block(y * 13);
        let check_push = |zone: &mut Zone, ent: &Static| {
            if solid(zone.block(to_block(ent.x), to_block(ent.y), to_block(sub_double(ent.z, 0.5)))) {
                zone.statics.push(ent.clone());
            }
        };
        match b8 {
            1 => {
                let mut ent = Static { kind: 0x13, scale: [2.0, 3.0, 1.0], ..Static::NEW };
                for (ax, ay) in [(4, 4), (10, 4), (4, 10), (10, 10)] {
                    ent.x = xi(ax);
                    ent.y = yi(ay);
                    ent.z = zz;
                    ent.rotation = self.rng.rand() % 4; // 0x004ebb3c, 0x004ebc54, then two more through a register
                    check_push(zone, &ent);
                }
            }
            2 => {
                let mut table = Static { kind: 0xc, scale: [3.0, 3.0, 1.0], ..Static::NEW };
                table.x = xi(7);
                table.y = yi(7);
                table.z = zz;
                table.rotation = self.rng.rand() % 4; // 0x004ec370
                check_push(zone, &table);
                let mut seat = Static { kind: 0x10, scale: [1.0, 1.0, 0.5], ..Static::NEW };
                let spots = [
                    (xo(3.5), yi(7), 1),
                    (xo(10.5), yi(7), 3),
                    (xi(7), yo(3.5), 2),
                    (xi(7), yo(10.5), 0),
                ];
                for (sx, sy, rot) in spots {
                    seat.x = sx;
                    seat.y = sy;
                    seat.z = zz;
                    seat.rotation = rot;
                    check_push(zone, &seat);
                }
            }
            3 => {
                let mut counter = Static { kind: 0x1f, scale: [4.0, 1.0, 1.0], ..Static::NEW };
                let spots = [
                    (xo(4.5), yi(7), 3),
                    (xo(9.5), yi(7), 1),
                    (xi(7), yo(4.5), 0),
                    (xi(7), yo(9.5), 2),
                ];
                for (sx, sy, rot) in spots {
                    counter.x = sx;
                    counter.y = sy;
                    counter.z = zz;
                    counter.rotation = rot;
                    check_push(zone, &counter);
                }
            }
            _ => {
                if self.rng.rand() % 5 == 0 {
                    // 0x004eca67 or 0x004ecbc1 (see report)
                    let mut table = Static { kind: 0xc, scale: [3.0, 3.0, 1.0], ..Static::NEW };
                    table.x = xi(7);
                    table.y = yi(7);
                    table.z = zz;
                    table.rotation = self.rng.rand() % 4; // 0x004ece1b
                    check_push(zone, &table);
                }
            }
        }
    }

    /// Phase G (towns): shopkeepers and villagers with day schedules.
    fn settlement_town_npcs(&mut self, zone: &mut Zone, cell: &Cell, out: &mut SettlementExtras, guard_posts: &[[i32; 3]], tree_spots: &[[i32; 3]]) {
        let class1: Vec<usize> = (0..out.houses.len()).filter(|&k| out.houses[k].class == 1).collect();
        let shops: Vec<usize> = (0..out.houses.len()).filter(|&k| matches!(out.houses[k].class, 2..=5)).collect();
        // Region spots: the region's cells that are not towns/missions (or zone records with content).
        let mut region_spots: Vec<[i32; 2]> = Vec::new();
        let rx = crate::climate::div_trunc(zone.x, 64);
        let ry = crate::climate::div_trunc(zone.y, 64);
        if let Some(region) = self.region(rx, ry) {
            for c in region.cells.iter() {
                if c.kind == 1 || c.kind == 10 {
                    continue;
                }
                let bx = (c.x / 65536) as i32;
                let by = (c.y / 65536) as i32;
                if c.kind == 0 {
                    let zxr = crate::climate::div_trunc(bx, 256);
                    let zyr = crate::climate::div_trunc(by, 256);
                    let ok = self.settlement_zone_record(zxr, zyr).is_some_and(|r| r.kind != 0 && r.seed != 0);
                    if !ok {
                        continue;
                    }
                }
                region_spots.push([bx / 256, by / 256]);
            }
        }
        // (The level range of the region's climate point is computed here and not used.)
        let mut spot_counter: u32 = 0;
        for hi in 0..out.houses.len() {
            if out.houses[hi].floor_spots.is_empty() {
                continue;
            }
            let class = out.houses[hi].class;
            if (1..=5).contains(&class) {
                if out.houses[hi].special_spots.is_empty() {
                    continue;
                }
                let spots = &out.houses[hi].special_spots;
                let r = self.rng.rand() as u32; // 0x004f0390 (class 1), 0x004f0554 (2), 0x004f0730 (4), 0x004f090c (3), 0x004f0ae8 (5)
                let spot = spots[(r % spots.len() as u32) as usize];
                let mut sp = Spawn { f28: 3, ..Spawn::NEW };
                [sp.x, sp.y, sp.z] = fix3(spot);
                sp.entity_type = self.rng.rand() % 2 + 2; // 0x004f03c4 / 0x004f0588 / 0x004f0764 / 0x004f0940 / 0x004f0b1c
                sp.level = cell.level;
                sp.f30 = match class {
                    1 => 0x84,
                    2 => 0x80,
                    3 => 0x81,
                    4 => 0x82,
                    _ => 0x83,
                };
                let mut inv = Vec::new();
                match class {
                    2 => shops::shop_setup_4fd920(&mut self.rng, &mut inv),
                    4 => shops::shop_setup_4fc180(&mut self.rng, &mut inv),
                    3 | 5 => shops::shop_setup_4fde90(&mut self.rng, &mut inv),
                    _ => {}
                }
                // 0x004f0406 / 0x004f05e2 / 0x004f07be / 0x004f099a / 0x004f0b76.
                sp.ai = Some(SpawnAi::Sentry { path: vec![[sp.x, sp.y, sp.z]] });
                out.spawns.push(SpawnExtra {
                    spawn_index: zone.spawns.len(),
                    ai: Some(Ai::Guard { target: None }),
                    inventory: inv,
                    ..SpawnExtra::default()
                });
                zone.spawns.push(sp);
                continue;
            }
            if class != 0 {
                continue;
            }
            let k = self.rng.rand() % 2 + 1; // 0x004f0ca1
            let no_region = region_spots.is_empty();
            let no_posts = guard_posts.is_empty();
            let no_class1 = class1.is_empty();
            let no_shops = shops.is_empty();
            let no_trees = tree_spots.is_empty();
            for _ in 0..k {
                let house = &out.houses[hi];
                let r = self.rng.rand() as u32; // 0x004f0d60
                let spot = house.floor_spots[(r % house.floor_spots.len() as u32) as usize];
                let mut sp = Spawn { f28: 3, f30: 0x88, ..Spawn::NEW };
                [sp.x, sp.y, sp.z] = fix3(spot);
                let home = [sp.x, sp.y, sp.z];
                sp.entity_type = self.rng.rand() % 2 + 2; // 0x004f0d92
                sp.level = cell.level;
                let mut ex = SpawnExtra { spawn_index: zone.spawns.len(), ai: Some(Ai::Villager), ..SpawnExtra::default() };
                if !no_region && self.rng.rand() % 2 == 0 {
                    // 0x004f0dc3
                    let s = region_spots[(spot_counter % region_spots.len() as u32) as usize];
                    spot_counter = spot_counter.wrapping_add(1);
                    ex.target = Some([s[0], s[1], 0]);
                    ex.ai_mode = Some(4);
                }
                if self.rng.rand() % 10 == 0 {
                    // 0x004f0e43
                    ex.ai_mode = Some(3);
                }
                let sched = &mut ex.schedule;
                sched.push(ScheduleEntry { pos: home, time: 0 });
                let mut t = (self.rng.rand() % 0xb4 + 0x1a4) * 60000; // 0x004f0e7b
                let rng = &mut self.rng;
                let pick = |rng: &mut MsvcRand, len: usize| {
                    let r = rng.rand() as u32;
                    (r % len as u32) as usize
                };
                let visit_tree = |rng: &mut MsvcRand, sched: &mut Vec<ScheduleEntry>, t: &mut i32| {
                    let s = tree_spots[pick(rng, tree_spots.len())]; // 0x004f0eb5 / 0x004f10be / 0x004f13a9
                    sched.push(ScheduleEntry { pos: fix3(s), time: *t });
                    *t += (rng.rand() % 0x3c + 0x3c) * 60000; // 0x004f0f1b / 0x004f1124 / 0x004f140f
                };
                if !no_trees {
                    visit_tree(rng, sched, &mut t);
                }
                if !no_posts {
                    let s = guard_posts[pick(rng, guard_posts.len())]; // 0x004f0f54
                    sched.push(ScheduleEntry { pos: fix3(s), time: t });
                    t += (rng.rand() % 0xb4 + 0x3c) * 60000; // 0x004f0fba
                }
                let houses = &out.houses;
                let visit_door = |rng: &mut MsvcRand, sched: &mut Vec<ScheduleEntry>, t: &mut i32, h: usize, m: i32, add: i32| {
                    let doors = &houses[h].door_spots;
                    if !doors.is_empty() {
                        let s = doors[pick(rng, doors.len())];
                        sched.push(ScheduleEntry { pos: fix3(s), time: *t });
                        *t += (rng.rand() % m + add) * 60000;
                    }
                };
                if !no_shops {
                    let hsel = shops[pick(rng, shops.len())]; // 0x004f0ff3
                    visit_door(rng, sched, &mut t, hsel, 0x14, 0); // 0x004f102c / 0x004f1086
                }
                if !no_trees {
                    visit_tree(rng, sched, &mut t);
                }
                if !no_class1 {
                    let hsel = class1[pick(rng, class1.len())]; // 0x004f115a
                    visit_door(rng, sched, &mut t, hsel, 0xb4, 0x3c); // 0x004f1190 / 0x004f11ea
                }
                if !no_shops {
                    let n = shops.len();
                    let hsel = shops[pick(rng, n)]; // 0x004f1229
                    visit_door(rng, sched, &mut t, hsel, 0x1e, 0); // 0x004f1262 / 0x004f12bc
                    let hsel = shops[pick(rng, n)]; // 0x004f12da
                    visit_door(rng, sched, &mut t, hsel, 0x1e, 0); // 0x004f1317 / 0x004f1371
                }
                if !no_trees {
                    visit_tree(rng, sched, &mut t);
                }
                if !houses[hi].door_spots.is_empty() {
                    rng.rand(); // 0x004f143c (value discarded)
                    sched.push(ScheduleEntry { pos: home, time: t });
                    t += (rng.rand() % 0x3c + 0x3c) * 60000; // 0x004f1479
                }
                sched.push(ScheduleEntry { pos: home, time: t + 1_800_000 });
                // 0x004f14dd
                sp.ai = Some(SpawnAi::Villager { schedule: sched.clone() });
                out.spawns.push(ex);
                zone.spawns.push(sp);
            }
        }
    }

    /// `FUN_0042e880(zx, zy)`: the 16-byte zone record of the region holding the zone.
    fn settlement_zone_record(&self, zx: i32, zy: i32) -> Option<crate::region::ZoneRecord> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        let region = self.region(crate::climate::div_trunc(zx, 64), crate::climate::div_trunc(zy, 64))?;
        Some(region.zones[((zx % 64) * 64 + zy % 64) as usize])
    }

    /// Phase H (cell type 5): the boss, patrols, guard rings and group guards.
    fn settlement_guard_npcs(&mut self, zone: &mut Zone, cell: &Cell, out: &mut SettlementExtras, plots: &[Plot], n: i32, s: i32) {
        let (guards, groups): (Vec<i32>, Vec<Vec<i32>>) = match cell.variant {
            0 => (vec![0x29, 0x11, 0x60, 0x60], vec![vec![0x29, 0x11]]),
            1 => (vec![9, 10], vec![vec![9, 10]]),
            2 => (vec![0x53, 0x54], vec![vec![0x53, 0x54], vec![0x51]]),
            3 => (vec![0x4c], vec![vec![0x4c], vec![0x51]]),
            4 => {
                let last = match cell.id % 3 {
                    1 => 0x5e,
                    2 => 0x11,
                    _ => 0x61,
                };
                (vec![0xf, 0x10], vec![vec![0xf, 0x10], vec![last]])
            }
            _ => (vec![0xb, 0xc], vec![vec![0x2e]]),
        };
        let zx = zone.x;
        let zy = zone.y;
        let ground = |zone: &Zone, x: i32, y: i32| {
            let c = col(zone, x, y)?;
            let mut z = c.f14;
            while solid(zone.block(x, y, z)) {
                z += 1;
            }
            Some(z)
        };
        for i in 0..n {
            for j in 0..n {
                let p = plots[(j * n + i) as usize];
                if p.kind == 0x14 {
                    if guards.is_empty() {
                        continue;
                    }
                    let x = s / 2 + (i << 8) / n + zx * 256;
                    let y = s / 2 + (j << 8) / n + zy * 256;
                    let Some(z) = ground(zone, x, y) else { continue };
                    let mut sp = Spawn { x: from_block(x), y: from_block(y), z: from_block(z + 1), ..Spawn::NEW };
                    sp.rotation = rand_rot(self.rng.rand()); // 0x004f1d77
                    sp.level = cell.level;
                    sp.f28 = 1;
                    let r = self.rng.rand() as u32; // 0x004f1db9
                    sp.entity_type = guards[(r % guards.len() as u32) as usize];
                    sp.b58 = cell.level_extra as u8;
                    sp.appearance.flags |= 0x1000;
                    sp.appearance.flags |= 0x200;
                    sp.b10e8 = 1;
                    // 0x004f1e19: `Sequence[Combat, WalkPath([pos])]`, no LookAtPlayer.
                    sp.ai = Some(SpawnAi::Patrol { path: vec![[sp.x, sp.y, sp.z]] });
                    out.spawns.push(SpawnExtra {
                        spawn_index: zone.spawns.len(),
                        ai: Some(Ai::Guard { target: None }),
                        ..SpawnExtra::default()
                    });
                    zone.spawns.push(sp);
                } else if p.kind != 2 && 0.2 < p.f18 {
                    for a in [0, s] {
                        for b in [0, s] {
                            if self.rng.rand() % 5 != 0 {
                                // 0x004f1f45
                                continue;
                            }
                            let x = s / 4 + a / 2 + zx * 256 + (i << 8) / n;
                            let y = s / 4 + b / 2 + zy * 256 + (j << 8) / n;
                            if guards.is_empty() {
                                continue;
                            }
                            let Some(z) = ground(zone, x, y) else { continue };
                            let mut sp = Spawn { x: from_block(x), y: from_block(y), z: from_block(z + 1), ..Spawn::NEW };
                            sp.rotation = rand_rot(self.rng.rand()); // 0x004f20f3
                            sp.level = cell.level;
                            sp.f28 = 1;
                            sp.appearance.flags |= 0x1000;
                            let r = self.rng.rand() as u32; // 0x004f213e
                            sp.entity_type = guards[(r % guards.len() as u32) as usize];
                            sp.b58 = cell.level_extra as u8;
                            zone.spawns.push(sp);
                        }
                    }
                }
            }
        }

        let guard_items = |rng: &mut MsvcRand, level: i32| {
            let c = rng.rand() % 2; // ring guards 0x004f25fa; group guards: register call near 0x004f29d1
            let item = Item { item_type: 0x01, sub_type: 0x01, level: level as i16 as u16, ..Item::NEW };
            (0..c).map(|_| InventoryAdd { item, bag: -1 }).collect::<Vec<_>>()
        };
        for hi in 0..out.houses.len() {
            if out.houses[hi].floor_spots.is_empty() {
                continue;
            }
            let mut spots = out.houses[hi].floor_spots.clone();
            if !guards.is_empty() {
                let k = self.rng.rand() % 3; // 0x004f226f
                for _ in 0..k {
                    if spots.is_empty() {
                        break;
                    }
                    let r = self.rng.rand() as u32 % spots.len() as u32; // 0x004f22b1
                    let pf = fix3(spots.remove(r as usize));
                    let m = self.rng.rand() % 3 + 2; // 0x004f2301
                    let md = f64::from(m);
                    let mut t2 = 0;
                    for _ in 0..m {
                        let a = ((f64::from(t2) * std::f64::consts::PI) / md) as f32;
                        let z = pf[2] + from_block(1);
                        let sa = cw_math::sin(f64::from(a)) as f32;
                        let y = add_float(pf[1], sa * 2.0);
                        let ca = cw_math::cos(f64::from(a)) as f32;
                        let x = add_float(pf[0], ca * 2.0);
                        let mut sp = Spawn { x, y, z, ..Spawn::NEW };
                        sp.rotation = ((f64::from(a) / std::f64::consts::PI) * 180.0 + 90.0) as f32;
                        sp.level = cell.level;
                        sp.f28 = 1;
                        sp.appearance.flags |= 0x1000;
                        let r = self.rng.rand() as u32; // 0x004f24f3
                        sp.entity_type = guards[(r % guards.len() as u32) as usize];
                        sp.b58 = cell.level_extra as u8;
                        let inventory = guard_items(&mut self.rng, sp.level);
                        // 0x004f2532
                        sp.ai = Some(SpawnAi::Patrol { path: vec![[sp.x, sp.y, sp.z]] });
                        out.spawns.push(SpawnExtra {
                            spawn_index: zone.spawns.len(),
                            ai: Some(Ai::Guard { target: None }),
                            inventory,
                            ..SpawnExtra::default()
                        });
                        zone.spawns.push(sp);
                        t2 += 2;
                    }
                }
            }
            if !groups.is_empty() {
                let k = self.rng.rand() % 2; // 0x004f26ab
                for _ in 0..k {
                    if spots.is_empty() {
                        break;
                    }
                    let r = self.rng.rand() as u32 % spots.len() as u32; // register call after 0x004f26ab
                    let pf = fix3(spots.remove(r as usize));
                    let g = self.rng.rand() as u32 % groups.len() as u32; // register call (before 0x004f27f1)
                    let group = &groups[g as usize];
                    if group.is_empty() {
                        continue;
                    }
                    let mut sp = Spawn { x: pf[0], y: pf[1], z: pf[2] + from_block(1), ..Spawn::NEW };
                    sp.rotation = rand_rot(self.rng.rand()); // 0x004f27f1 (approx.)
                    sp.level = cell.level;
                    sp.f28 = 1;
                    sp.appearance.flags |= 0x1000;
                    let r = self.rng.rand() as u32; // 0x004f2838 (approx.)
                    sp.entity_type = group[(r % group.len() as u32) as usize];
                    sp.b58 = cell.level_extra as u8;
                    let mut target = None;
                    if !out.houses.is_empty() {
                        let r = self.rng.rand() as u32; // 0x004f2941 (approx.)
                        let hh = &out.houses[(r % out.houses.len() as u32) as usize];
                        if !hh.floor_spots.is_empty() {
                            let r = self.rng.rand() as u32; // 0x004f2976 (approx.)
                            target = Some(fix3(hh.floor_spots[(r % hh.floor_spots.len() as u32) as usize]));
                        }
                    }
                    let inventory = guard_items(&mut self.rng, sp.level);
                    // 0x004f2879: the spawn position, then the house floor spot.
                    let mut path = vec![[sp.x, sp.y, sp.z]];
                    path.extend(target);
                    sp.ai = Some(SpawnAi::Patrol { path });
                    out.spawns.push(SpawnExtra {
                        spawn_index: zone.spawns.len(),
                        ai: Some(Ai::Guard { target }),
                        inventory,
                        ..SpawnExtra::default()
                    });
                    zone.spawns.push(sp);
                }
            }
        }
    }
}

/// Shopkeeper inventories (`FUN_004fd920`, `FUN_004fc180`, `FUN_004fde90`) and their item generators.
pub mod shops {
    use super::{InventoryAdd, Item};
    use cw_math::MsvcRand;

    /// `Server.exe 0x00411090`: the level curve `(1 / (1 - x) - 1) * 20 + 1`, all in `float` (SSE `divss`/`subss`/
    /// `mulss`/`addss`; the result is stored as a float and reloaded with `FLD float`, so it is an exact `f32`).
    fn level_curve_411090(x: f32) -> f32 {
        let one = 1.0f32;
        (one / (one - x) - one) * 20.0 + one
    }

    /// The level bounds computed at the top of every shop loop iteration:
    /// `lo = (int)curve((float)i / 30f)`, `hi = (int)curve(((float)i + 0.99999f) / 30f)` (`cvttss2si`).
    fn shop_level_bounds(i: i32) -> (i32, i32) {
        // 0x005739d8 = 0x3f7fff58 = 0.99998999f
        let near_one = f32::from_bits(0x3f7f_ff58);
        let fi = i as f32;
        let lo = level_curve_411090(fi / 30.0) as i32;
        let hi = level_curve_411090((fi + near_one) / 30.0) as i32;
        (lo, hi)
    }

    /// `lo + r % (hi - lo + 1)`, clamped to at least 1 (`cmp eax, 1; cmovl`).
    fn shop_level(lo: i32, hi: i32, r: i32) -> i32 {
        (lo + r % (hi - lo + 1)).max(1)
    }

    /// The rarity bonus rolls shared by all shop code: `+1` on `rand() % 100 == 0`, `+1` on `rand() % 1000 == 0`,
    /// `+1` on `rand() % 10000 == 0`, then clamped to 4 (`cmovg`). The three `rand()` calls sit at, e.g.,
    /// 0x004fdba7 / 0x004fdbb6 / 0x004fdbc5 (4fd920) and 0x004fc2aa / 0x004fc2b9 / 0x004fc2c8 (first inlined copy in
    /// 4fc180); every inlined item block of 4fc180/4fde90 has its own copy right after its discarded `rand()`.
    fn roll_rarity(rng: &mut MsvcRand, base: i32) -> u8 {
        let mut rarity = base;
        if rng.rand() % 100 == 0 {
            // e.g. 0x004fc2aa
            rarity += 1;
        }
        if rng.rand() % 1000 == 0 {
            // e.g. 0x004fc2b9
            rarity += 1;
        }
        if rng.rand() % 10000 == 0 {
            // e.g. 0x004fc2c8
            rarity += 1;
        }
        rarity.min(4) as u8
    }

    /// One inlined "roll and push_back" block of 4fc180 / 4fde90: a discarded `rand()`, the rarity roll (starting at
    /// 0), `modifier = rand()` (the full value, not `% 100`), then `vector<Item>::push_back(template)`.
    /// Addresses of the discarded `rand()` of each block are listed at the call sites.
    fn push_rolled(rng: &mut MsvcRand, template: &mut Item, items: &mut Vec<Item>) {
        rng.rand(); // discarded, e.g. 0x004fc2a6
        template.rarity = roll_rarity(rng, 0);
        template.modifier = rng.rand(); // e.g. 0x004fc2e8
        items.push(*template);
    }

    /// The tail of 4fc180 / 4fde90: `n = rand() % 3 + 1` items are drawn without replacement from the vector
    /// (`rand()` divided unsigned by the element count), each one added to the inventory with bag -1 and then erased
    /// (`FUN_004f59f0` moves the tail down by one element).
    fn draw_into_inventory(rng: &mut MsvcRand, items: &mut Vec<Item>, inv: &mut Vec<InventoryAdd>) {
        let n = rng.rand() % 3 + 1; // 0x004fd810 (4fc180) / 0x004feac4 (4fde90)
        let mut drawn = 0;
        while drawn < n && !items.is_empty() {
            let r = rng.rand() as u32; // 0x004fd84c (4fc180) / 0x004feb00 (4fde90)
            let index = (r % items.len() as u32) as usize;
            inv.push(InventoryAdd {
                item: items[index],
                bag: -1,
            });
            items.remove(index);
            drawn += 1;
        }
    }

    /// Material roll written as `neg eax; sbb al, al; add al, 0xc` after `rand() % 2`: odd -> 0x0b, even -> 0x0c.
    fn material_neg_sbb(r: i32) -> u8 {
        if r & 1 != 0 { 0x0b } else { 0x0c }
    }

    /// Material roll written as `setne al; add al, 0xb` after `rand() % 2`: odd -> 0x0c, even -> 0x0b.
    fn material_setne(r: i32) -> u8 {
        if r & 1 != 0 { 0x0c } else { 0x0b }
    }

    /// `Server.exe 0x00528bf0`: random weapon-like item `(out, level, rarity, category)`. Builds a candidate vector
    /// (kind 7/4/5/6 of material 1 for category 1, kinds 7/7/4/5/6 of materials 0x19 / 0x1a / 0x1b for categories
    /// 3 / 2 / 4, a negative category selects all of them), always appends kinds 8 and 9 with a rolled material, and
    /// returns a copy of `candidates[rand() % len]`. Every candidate has `modifier = rand() % 100`, `sub = 0`,
    /// `f8 = 0`, `flags = 0`.
    pub fn random_item_528bf0(rng: &mut MsvcRand, level: i16, rarity: u8, category: i32) -> Item {
        let mut items: Vec<Item> = Vec::with_capacity(21);
        let mut template = Item {
            item_type: 0,
            sub_type: 0,
            modifier: 0,
            f8: 0,
            rarity,
            material: 0,
            flags: 0,
            level: level as u16,
            ..Item::NEW
        };
        let mut push = |rng: &mut MsvcRand, template: &mut Item, kind: u8| {
            template.item_type = kind;
            template.modifier = rng.rand() % 100;
            items.push(*template);
        };
        if category == 1 || category < 0 {
            template.material = 1;
            push(rng, &mut template, 7); // 0x00528cb3
            push(rng, &mut template, 4); // 0x00528cdc
            push(rng, &mut template, 5); // 0x00528d05
            push(rng, &mut template, 6); // 0x00528d2e
        }
        if category == 3 || category < 0 {
            template.material = 0x19;
            push(rng, &mut template, 7); // 0x00528d6b
            push(rng, &mut template, 7); // 0x00528d94
            push(rng, &mut template, 4); // 0x00528dbd
            push(rng, &mut template, 5); // 0x00528de6
            push(rng, &mut template, 6); // 0x00528e0f
        }
        if category == 2 || category < 0 {
            template.material = 0x1a;
            push(rng, &mut template, 7); // 0x00528e4c
            push(rng, &mut template, 7); // 0x00528e75
            push(rng, &mut template, 4); // 0x00528e9e
            push(rng, &mut template, 5); // 0x00528ec7
            push(rng, &mut template, 6); // 0x00528ef0
        }
        if category == 4 || category < 0 {
            template.material = 0x1b;
            push(rng, &mut template, 7); // 0x00528f2d
            push(rng, &mut template, 7); // 0x00528f56
            push(rng, &mut template, 4); // 0x00528f7a
            push(rng, &mut template, 5); // 0x00528f9e
            push(rng, &mut template, 6); // 0x00528fc2
        }
        template.material = material_setne(rng.rand()); // 0x00528fe6
        push(rng, &mut template, 8); // 0x00529006
        template.material = material_setne(rng.rand()); // 0x00529023
        push(rng, &mut template, 9); // 0x00529043
        let r = rng.rand() as u32; // 0x00529060 (unsigned div by the element count)
        items[(r % items.len() as u32) as usize]
    }

    /// `Server.exe 0x0052c4e0`: random kind-3 item `(out, level, rarity, category)`. Candidates (all kind 3, `f8 = 0`,
    /// `flags = 0`, `modifier = rand() % 100`): category 1 -> subs `rand() % 3`, `rand() % 3 + 15`, 13 (material 1);
    /// category 4 -> subs 3, 5, 4 (material 1); category 2 -> subs 6, 8 (material 2, one discarded `rand()` between);
    /// category 3 -> subs 10, 11 (material 2), 12 (rolled material 0x0b / 0x0c). A negative category selects all.
    /// Returns a copy of `candidates[rand() % len]`.
    pub fn random_item_52c4e0(rng: &mut MsvcRand, level: i16, rarity: u8, category: i32) -> Item {
        let mut items: Vec<Item> = Vec::with_capacity(12);
        let mut template = Item {
            item_type: 3,
            sub_type: 0,
            modifier: 0,
            f8: 0,
            rarity,
            material: 1,
            flags: 0,
            level: level as u16,
            ..Item::NEW
        };
        let mut push = |rng: &mut MsvcRand, template: &mut Item| {
            template.modifier = rng.rand() % 100;
            items.push(*template);
        };
        if category == 1 || category < 0 {
            template.sub_type = (rng.rand() % 3) as u8; // 0x0052c596
            push(rng, &mut template); // 0x0052c5a6
            template.material = 1;
            template.sub_type = (rng.rand() % 3 + 0xf) as u8; // 0x0052c5cf
            push(rng, &mut template); // 0x0052c5dd
            template.sub_type = 0xd;
            push(rng, &mut template); // 0x0052c606
        }
        if category == 4 || category < 0 {
            template.sub_type = 3;
            push(rng, &mut template); // 0x0052c639
            template.sub_type = 5;
            push(rng, &mut template); // 0x0052c662
            template.sub_type = 4;
            push(rng, &mut template); // 0x0052c686
        }
        template.material = 2;
        if category == 2 || category < 0 {
            template.sub_type = 6;
            push(rng, &mut template); // 0x0052c6c0
            rng.rand(); // 0x0052c6e2, discarded
            template.sub_type = 8;
            push(rng, &mut template); // 0x0052c6eb
        }
        if category == 3 || category < 0 {
            template.sub_type = 0xa;
            push(rng, &mut template); // 0x0052c722
            template.sub_type = 0xb;
            push(rng, &mut template); // 0x0052c74b
            template.sub_type = 0xc;
            template.material = material_neg_sbb(rng.rand()); // 0x0052c76f
            push(rng, &mut template); // 0x0052c789
        }
        // The original divides by `(end - begin) / 0x118` unsigned; an empty vector (category 0 or > 4) would fault
        // there, like the `%` below panics.
        let r = rng.rand() as u32; // 0x0052c7ac
        items[(r % items.len() as u32) as usize]
    }

    /// `Server.exe 0x004fd920`: shop inventory setup (formulas / recipes). Only the creature's inventory
    /// (`creature + 0xf6c`) is touched; no creature field is read.
    ///
    /// For each tier `i` in `0..30`: a level in `[curve(i / 30), curve((i + 0.99999) / 30)]` (forced to 1 for tier 0).
    /// Tier 0 additionally adds a fixed starter set. Then `rand() % 3` formula items (kind 2, `f8` = the kind of a
    /// random item from 0x528bf0 or 0x52c4e0) and, with probability 1/2, a kind 1 / sub 7 item of the tier level.
    pub(crate) fn shop_setup_4fd920(rng: &mut MsvcRand, inv: &mut Vec<InventoryAdd>) {
        for i in 0..30 {
            let (lo, hi) = shop_level_bounds(i);
            let r = rng.rand(); // 0x004fd9ad
            let mut level = shop_level(lo, hi, r);
            if i == 0 {
                level = 1;
                // The item buffer is set up once here and then only partially rewritten between the adds.
                let mut item = Item {
                    item_type: 0x0b,
                    sub_type: 0x0c,
                    modifier: 0,
                    f8: 0,
                    rarity: 0,
                    material: 0x18,
                    flags: 0,
                    level: 1,
                    ..Item::NEW
                };
                inv.push(InventoryAdd { item, bag: 0 });
                // `mov word [kind], 0x18` also clears sub.
                item.item_type = 0x18;
                item.sub_type = 0;
                item.material = 1;
                item.level = 1;
                inv.push(InventoryAdd { item, bag: 0 });
                let mut tier_rarity = 1;
                let mut j = 10;
                while j < 50 {
                    let r = rng.rand(); // 0x004fda80
                    if r % j == 0 {
                        item.level = 1;
                        item.rarity = tier_rarity as u8;
                        item.item_type = 0x18;
                        item.sub_type = 0;
                        item.material = 1;
                        // The bag argument is `edx`, the (zero) remainder.
                        inv.push(InventoryAdd { item, bag: 0 });
                    }
                    tier_rarity += 1;
                    j += 10;
                }
                // `mov word [rarity], 0x200` -> rarity 0, material 2; `mov word [kind], 0x17` -> kind 0x17, sub 0.
                item.rarity = 0;
                item.material = 2;
                item.item_type = 0x17;
                item.sub_type = 0;
                inv.push(InventoryAdd { item, bag: 0 });
                item.sub_type = 1;
                item.material = 2;
                inv.push(InventoryAdd { item, bag: 0 });
                item.item_type = 0x14;
                item.material = 0;
                item.level = 1;
                // rand() is never negative, so the `ja` default of the jump table is unreachable.
                item.sub_type = match rng.rand() % 6 {
                    // 0x004fdb28
                    0 => 0x22,
                    1 => 0x23,
                    2 => 0x13,
                    3 => 0x1a,
                    4 => 0x1e,
                    _ => 0x57,
                };
                inv.push(InventoryAdd { item, bag: 0 });
            }

            let formulas = rng.rand() % 3; // 0x004fdb82
            for _ in 0..formulas {
                let base = rng.rand() & 1; // 0x004fdb96 (`and 0x80000001` idiom = % 2)
                let rarity = roll_rarity(rng, base); // 0x004fdba7, 0x004fdbb6, 0x004fdbc5
                // `push dword level`; the callee reads it as int16.
                let generated = if rng.rand() & 1 == 0 {
                    // 0x004fdc35
                    random_item_528bf0(rng, level as i16, rarity, -1)
                } else {
                    random_item_52c4e0(rng, level as i16, rarity, -1)
                };
                // Copied with FUN_00402a70, then `f8 = (int)(u8)kind; kind = 2`.
                let item = Item {
                    item_type: 2,
                    f8: u32::from(generated.item_type),
                    ..generated
                };
                inv.push(InventoryAdd { item, bag: 0 });
            }

            if rng.rand() & 1 != 0 {
                // 0x004fdcb7
                let item = Item {
                    item_type: 1,
                    sub_type: 7,
                    level: level as i16 as u16,
                    ..Item::NEW
                };
                inv.push(InventoryAdd { item, bag: 0 });
            }
        }
    }

    /// `Server.exe 0x004fc180`: shop inventory setup (weapons). No creature field is read.
    ///
    /// For each tier `i` in `0..30`: a level as in 4fd920 (1 for tier 0), a vector of 21 inlined random weapons
    /// (same kinds and materials as 0x528bf0 with category -1, but with a discarded `rand()`, a rarity roll and
    /// `modifier = rand()` per item, and `flags = 1` from `mov word [material], 0x101`), then 1..=3 of them are drawn
    /// into the inventory with bag -1.
    pub(crate) fn shop_setup_4fc180(rng: &mut MsvcRand, inv: &mut Vec<InventoryAdd>) {
        for i in 0..30 {
            let (lo, hi) = shop_level_bounds(i);
            let r = rng.rand(); // 0x004fc217
            let mut level = shop_level(lo, hi, r);
            if i == 0 {
                level = 1;
            }
            let mut items: Vec<Item> = Vec::with_capacity(21);
            let mut template = Item {
                item_type: 0,
                sub_type: 0,
                modifier: 0,
                f8: 0,
                rarity: 0,
                material: 1,
                flags: 1,
                level: level as i16 as u16,
                ..Item::NEW
            };
            // Discarded rand() of each block: 0x004fc2a6, 0x004fc328, 0x004fc423, 0x004fc52c.
            for kind in [7, 4, 5, 6] {
                template.item_type = kind;
                push_rolled(rng, &mut template, &mut items);
            }
            // Discarded rand() of each block: material 0x19: 0x004fc63c, 0x004fc745, 0x004fc84e, 0x004fc957,
            // 0x004fca60; 0x1a: 0x004fcb70, 0x004fcc79, 0x004fcd82, 0x004fce8b, 0x004fcf94; 0x1b: 0x004fd0a4,
            // 0x004fd1ad, 0x004fd2b6, 0x004fd3bf, 0x004fd4c8.
            for material in [0x19, 0x1a, 0x1b] {
                template.material = material;
                for kind in [7, 7, 4, 5, 6] {
                    template.item_type = kind;
                    push_rolled(rng, &mut template, &mut items);
                }
            }
            template.item_type = 8;
            template.material = material_neg_sbb(rng.rand()); // 0x004fd5d1
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fd5eb
            template.item_type = 9;
            template.material = material_neg_sbb(rng.rand()); // 0x004fd6f4
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fd70e
            draw_into_inventory(rng, &mut items, inv); // 0x004fd810, 0x004fd84c
        }
    }

    /// `Server.exe 0x004fde90`: shop inventory setup (kind 3 items). No creature field is read.
    ///
    /// Same shape as 4fc180, with the 11 candidates of 0x52c4e0 (category -1: subs `rand() % 3`,
    /// `rand() % 3 + 15`, 13, 3, 5, 4, 6, 8, 10, 11, 12) built inline with the discarded `rand()` / rarity roll /
    /// `modifier = rand()` pattern and `flags = 1`. Five extra discarded `rand()` calls sit between subs 6 and 8.
    pub(crate) fn shop_setup_4fde90(rng: &mut MsvcRand, inv: &mut Vec<InventoryAdd>) {
        for i in 0..30 {
            let (lo, hi) = shop_level_bounds(i);
            let r = rng.rand(); // 0x004fdf27
            let mut level = shop_level(lo, hi, r);
            if i == 0 {
                level = 1;
            }
            let mut items: Vec<Item> = Vec::with_capacity(11);
            let mut template = Item {
                item_type: 3,
                sub_type: 0,
                modifier: 0,
                f8: 0,
                rarity: 0,
                material: 1,
                flags: 1,
                level: level as i16 as u16,
                ..Item::NEW
            };
            template.sub_type = (rng.rand() % 3) as u8; // 0x004fdfaf
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fdfbf
            template.material = 1;
            template.sub_type = (rng.rand() % 3 + 0xf) as u8; // 0x004fe041
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fe054
            // Discarded rand() of each block: 0x004fe14f, 0x004fe258, 0x004fe361, 0x004fe46a.
            for sub in [0xd, 3, 5, 4] {
                template.sub_type = sub;
                push_rolled(rng, &mut template, &mut items);
            }
            template.material = 2;
            template.sub_type = 6;
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fe57a
            for _ in 0..5 {
                rng.rand(); // 0x004fe67c, 0x004fe67e, 0x004fe680, 0x004fe682, 0x004fe684 (discarded)
            }
            // Discarded rand() of each block: 0x004fe68d, 0x004fe796, 0x004fe89f.
            for sub in [8, 0xa, 0xb] {
                template.sub_type = sub;
                push_rolled(rng, &mut template, &mut items);
            }
            template.sub_type = 0xc;
            template.material = material_neg_sbb(rng.rand()); // 0x004fe9a8
            push_rolled(rng, &mut template, &mut items); // discarded rand() at 0x004fe9c2
            draw_into_inventory(rng, &mut items, inv); // 0x004feac4, 0x004feb00
        }
    }

    #[cfg(test)]
    mod shop_tests {
        use super::*;

        #[test]
        fn level_bounds_tier0_and_tier29() {
            assert_eq!(shop_level_bounds(0), (1, 1));
            // curve(29 / 30) = 1 / (1 / 30) ... = 581 up to float rounding.
            let (lo, _) = shop_level_bounds(29);
            assert!((579..=582).contains(&lo));
        }

        #[test]
        fn material_idioms() {
            assert_eq!(material_neg_sbb(1), 0x0b);
            assert_eq!(material_neg_sbb(2), 0x0c);
            assert_eq!(material_setne(1), 0x0c);
            assert_eq!(material_setne(2), 0x0b);
        }
    }
}
