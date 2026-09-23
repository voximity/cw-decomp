//! `cube::Region`: 64x64 zones, an 8x8 grid of cells, and per-zone records.
//!
//! Creation is `cube::World::createRegion` (`Server.exe 0x0050e080`). It reseeds the generator
//! with `seed188 + ry * 1024 + rx`, so a region is reproducible on its own, except that cell
//! heights come from `baseHeight`, which reads the cells of neighbouring regions that already
//! exist. Regions must therefore be created in the same order as the original.

use cw_math::value_noise_2d as noise;

use crate::climate::{ClimatePoint, div_trunc};
use crate::world::World;

/// One of the 64 cells of a region (0x68 bytes in the original), covering 8x8 zones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// `[0..1]`: centre x in 16.16 fixed blocks.
    pub x: i64,
    /// `[2..3]`: centre y in 16.16 fixed blocks.
    pub y: i64,
    /// `[4]`: radius in blocks.
    pub radius: f32,
    /// `[5]`: terrain height at the centre.
    pub height: f32,
    /// `[6]`: cell type: 0 none, 1 home, 2, 3, 4, 5, 6, 7, 10 mission, 11, 12, 14 town, 15 water variant.
    pub kind: i32,
    /// `[7]`
    pub variant: i32,
    /// `[8]`: id or seed.
    pub id: i32,
    /// `[9]`: level, 1 from the constructor.
    pub level: i32,
    /// `[10]`: level-table roll.
    pub level_extra: i32,
    /// `+0x2c..+0x54`: mission state, from the save database.
    pub mission: crate::save::MissionState,
    /// `+0x54..+0x68`: saved monster, from the save database.
    pub monster: crate::save::MonsterState,
}

impl Cell {
    /// `cube::Cell::Cell`, `Server.exe 0x004f7660`.
    pub const EMPTY: Cell = Cell { x: 0, y: 0, radius: 0.0, height: 0.0, kind: 0, variant: 0, id: 0, level: 1, level_extra: 0, mission: crate::save::MissionState::EMPTY, monster: crate::save::MonsterState::EMPTY };

    /// `cube::Cell::normDistance(&x, &y)`, `Server.exe 0x0052c820`: squared distance from the
    /// centre divided by the squared radius, so 0 at the centre and 1 at the radius.
    ///
    /// Types 0xb, 0xc and 0xe use the plain distance. Every other type first displaces the
    /// query by two noise samples scaled to 100 blocks (type 0xd) or 256 blocks (the rest).
    /// The noise coordinates are built from the query position times -0.0025 truncated to an
    /// integer and subtracted from fixed-point constants, exactly as compiled.
    pub fn norm_distance(&self, x: i64, y: i64) -> f32 {
        if self.radius < 0.001 {
            return 0.0;
        }
        const K: f32 = 1.5258789e-05; // 2^-16
        match self.kind {
            0xb | 0xc | 0xe => {
                let dx = (x - self.x) as f32;
                let dy = (y - self.y) as f32;
                (dy * K * dy * K + dx * K * dx * K) / (self.radius * self.radius)
            }
            kind => {
                let scale: f32 = if kind == 0xd { 100.0 } else { 256.0 };
                let ty = (y as f64 * -0.0025) as i64;
                let tx = (x as f64 * -0.0025) as i64;
                let n1 = noise(
                    (0x80_ad58_0000i64 - tx) as f64 * 1.52587890625e-05,
                    (0x1_617d_0000i64 - ty) as f64 * 1.52587890625e-05,
                );
                let t = (n1 * scale * 65536.0) as i64;
                let dx = (t - self.x + x) as f32 * K;
                let n2 = noise(
                    (0xd5f_0000i64 - tx) as f64 * 1.52587890625e-05,
                    (0x70_0000i64 - ty) as f64 * 1.52587890625e-05,
                );
                let t = (n2 * scale * 65536.0) as i64;
                let dy = (t - self.y + y) as f32 * K;
                (dy * dy + dx * dx) / (self.radius * self.radius)
            }
        }
    }
}

/// The 16-byte per-zone record at `region+0x18`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneRecord {
    /// `+0`: 0 none, 1 town quarter, 3 town, 4 mission.
    pub kind: u8,
    /// `+1`: subtype or quarter number.
    pub sub: u8,
    /// `+4`: zone seed or mission id.
    pub seed: i32,
    /// `+8`: level, 1 from the constructor.
    pub level: i32,
    /// `+0xc`: level-table roll of the town cell.
    pub byte0c: u8,
}

impl ZoneRecord {
    pub const DEFAULT: ZoneRecord = ZoneRecord { kind: 0, sub: 0, seed: 0, level: 1, byte0c: 0 };
}

/// `cube::Region`, 0x15a28 bytes in the original.
#[derive(Debug, Clone)]
pub struct Region {
    /// `+0xc`: `regionLevel(rx, ry)`.
    pub level: i32,
    /// `+0x10`
    pub f10: i32,
    /// `+0x14`
    pub variant: i32,
    /// `+0x18`: index `(zx & 63) * 64 + (zy & 63)`.
    pub zones: Vec<ZoneRecord>,
    /// `+0x14018`: index `cx * 8 + cy`.
    pub cells: [Cell; 64],
    /// `+0x15a18`: the region's missions are live. Set by `createRegion` when a cell's mission
    /// blob was found in the save database and by `activateRegionMissions` when a player
    /// arrives; cleared for every region at the day rollover; gates `saveEntities`.
    pub missions_active: bool,
}

impl Region {
    /// `cube::Region::Region`, `Server.exe 0x004f7570`.
    fn new() -> Self {
        Self { level: 1, f10: 0, variant: 0, zones: vec![ZoneRecord::DEFAULT; 4096], cells: [Cell::EMPTY; 64], missions_active: false }
    }
}

/// `regionLevel(rx, ry)`, `Server.exe 0x004d7870`.
pub fn region_level(rx: i32, ry: i32) -> i32 {
    if rx == 0x200 && ry == 0x200 {
        return 1;
    }
    let dy = (0x200 - ry) as f32;
    let dx = (0x200 - rx) as f32;
    let d = f64::from(dy * dy + dx * dx).sqrt() as f32;
    2 - (d * -0.75) as i32
}

/// `cube::ClimatePoint::levelRange`, `Server.exe 0x00522290`.
fn level_range(p: &ClimatePoint) -> (i32, i32) {
    let mut r = (1, 10);
    if p.b < 0.2 {
        r = (10, 20);
    }
    if p.a < 0.2 && p.b > 0.8 {
        r = (15, 25);
    }
    if p.a > 0.8 && p.b > 0.8 {
        r = (10, 20);
    }
    if p.flag == 1 {
        r = (20, 30);
    }
    r
}

/// Truncating snap to the 256 grid plus 128, in blocks (`((v + (v<0 ? 255 : 0)) & ~0xff) + 0x80`).
#[inline]
fn snap256(v: i32) -> i32 {
    (v - v % 256) + 0x80
}

/// `((v / 65536) / 256) % 64` with truncating divisions, the zone index of a fixed position.
#[inline]
fn zone_of(fixed: i64) -> i32 {
    let blocks = (fixed / 65536) as i32;
    div_trunc(blocks, 256) % 64
}

/// `(x * 4.0) / 32767.0 + dist - 2.0` style rolls: `(r as f32 * scale) / 32767.0`.
#[inline]
fn roll(r: i32, scale: f32) -> f32 {
    (r as f32 * scale) / 32767.0
}

/// Random offset inside a cell for a feature of the given radius, in 16.16 fixed blocks
/// (`((float)r * (2048.0 - (radius + 256.0) * 2.0)) / 32767.0 * 65536.0`, truncated).
#[inline]
fn jitter(r: i32, radius_plus_256: f32) -> i64 {
    (((r as f32 * (2048.0 - radius_plus_256 * 2.0)) / 32767.0) * 65536.0) as i64
}

impl World {
    /// `cube::World::getRegion(rx, ry)`, `Server.exe 0x00406210`.
    pub fn region(&self, rx: i32, ry: i32) -> Option<&Region> {
        if !(0..1024).contains(&rx) || !(0..1024).contains(&ry) {
            return None;
        }
        self.regions.get(&((rx as u32) * 1024 + ry as u32)).map(|b| &**b)
    }

    /// `cube::World::getCell(cx, cy)`, `Server.exe 0x004286f0`, with `cx = bx >> 11`.
    pub fn cell(&self, cx: i32, cy: i32) -> Option<&Cell> {
        if !(0..0x2000).contains(&cx) || !(0..0x2000).contains(&cy) {
            return None;
        }
        let region = self.region(div_trunc(cx * 8, 64), div_trunc(cy * 8, 64))?;
        Some(&region.cells[((cx % 8) * 8 + cy % 8) as usize])
    }

    /// `getCell` for a block position.
    pub fn cell_at_block(&self, bx: i32, by: i32) -> Option<&Cell> {
        self.cell(div_trunc(bx, 2048), div_trunc(by, 2048))
    }

    /// `cube::World::createRegion(rx, ry)`, `Server.exe 0x0050e080`. No-op if the region exists.
    pub fn create_region(&mut self, rx: i32, ry: i32) {
        if !(0..1024).contains(&rx) || !(0..1024).contains(&ry) {
            return;
        }
        let key = (rx as u32) * 1024 + ry as u32;
        if self.regions.contains_key(&key) {
            return;
        }
        // Phase A: climate points of the 5x5 neighbourhood, then the region's own RNG stream.
        for dx in -2..=2 {
            for dy in -2..=2 {
                self.get_or_create_climate_point(rx + dx, ry + dy);
            }
        }
        let seed188 = self.seeds.at(0x800188);
        self.rng.seed(seed188.wrapping_add(ry.wrapping_mul(0x400)).wrapping_add(rx) as u32);
        let mut region = Region::new();

        // Phase B: region-level fields.
        region.level = region_level(rx, ry);
        region.f10 = 0;
        if region.level > 4 {
            region.f10 = self.rng.rand() % 5;
        }
        region.variant = self.rng.rand() % 4;
        let mut id_counter = self.rng.rand() % 10000;
        let climate = *self.climate_point(rx, ry).expect("climate point exists after phase A");
        if climate.b > 0.81 {
            region.variant = self.rng.rand() % 2 + 4;
        }
        let (lvl_a, _lvl_b) = level_range(&climate);
        let home_cx = div_trunc(climate.x, 2048) % 8;
        let home_cy = div_trunc(climate.y, 2048) % 8;
        let region_x = rx * 0x4000;
        let region_y = ry * 0x4000;

        // Phase C: cells, cx outer, cy inner. Candidates are {key, cx, cy}.
        let mut candidates: Vec<(f32, i32, i32)> = Vec::new();
        for cx in 0..8 {
            for cy in 0..8 {
                let cell_min_x = region_x + cx * 0x800;
                let cell_min_y = region_y + cy * 0x800;
                let cell_max_x = cell_min_x + 0x800;
                let cell_max_y = cell_min_y + 0x800;
                let cell = &mut region.cells[(cx * 8 + cy) as usize];
                if !self.has_name || cx != home_cx || cy != home_cy {
                    let nearest = self.nearest_climate_region(cell_max_x - 0x400, cell_max_y - 0x400);
                    if nearest != (rx, ry) {
                        continue;
                    }
                    let r = self.rng.rand() & 0xff;
                    cell.radius = (r + 0x200) as f32;
                    let ybase = (cell.radius * 65536.0) as i64 + ((i64::from(region_y) + i64::from(cy * 0x800)) << 16) + 0x1000000;
                    let rp = cell.radius + 256.0;
                    let r2 = self.rng.rand();
                    let y64 = jitter(r2, rp) + ybase;
                    let xbase = (rp * 65536.0) as i64 + ((i64::from(region_x) + i64::from(cx * 0x800)) << 16);
                    let r3 = self.rng.rand();
                    let x64 = jitter(r3, cell.radius + 256.0) + xbase;
                    cell.x = x64;
                    cell.y = y64;
                    let bx = (x64 / 65536) as i32;
                    let by = (y64 / 65536) as i32;
                    let height = self.base_height(bx, by);
                    let cell = &mut region.cells[(cx * 8 + cy) as usize];
                    cell.height = height;
                    cell.level = lvl_a;
                    if self.nearest_climate_region(bx, by) == (rx, ry) {
                        let dyf = (home_cy - cy) as f32;
                        let dxf = (home_cx - cx) as f32;
                        let dist = f64::from(dyf * dyf + dxf * dxf).sqrt() as f32;
                        let r4 = self.rng.rand();
                        let keyv = (roll(r4, 4.0) + dist) - 2.0;
                        candidates.push((keyv, cx, cy));
                    }
                } else {
                    cell.kind = 1;
                    cell.variant = region.variant;
                    cell.id = climate.seed;
                    cell.radius = (self.rng.rand() % 200 + 0x200) as f32;
                    let mut x64 = i64::from(climate.x) << 16;
                    let mut y64 = i64::from(climate.y) << 16;
                    let t = (cell.radius * 65536.0) as i64;
                    if x64 - t - 0x1000000 < i64::from(cell_min_x) << 16 {
                        x64 = ((cell_min_x as f32 + cell.radius + 256.0) * 65536.0) as i64;
                    }
                    let t = (cell.radius * 65536.0) as i64;
                    if y64 - t - 0x1000000 < i64::from(cell_min_y) << 16 {
                        y64 = ((cell_min_y as f32 + cell.radius + 256.0) * 65536.0) as i64;
                    }
                    let t = (cell.radius * 65536.0) as i64;
                    if x64 + t + 0x1000000 > i64::from(cell_max_x) << 16 {
                        x64 = (((cell_max_x as f32 - cell.radius) - 256.0) * 65536.0) as i64;
                    }
                    let t = (cell.radius * 65536.0) as i64;
                    if y64 + t + 0x1000000 > i64::from(cell_max_y) << 16 {
                        y64 = (((cell_max_y as f32 - cell.radius) - 256.0) * 65536.0) as i64;
                    }
                    cell.x = x64;
                    cell.y = y64;
                    let height = self.base_height((x64 / 65536) as i32, (y64 / 65536) as i32);
                    let cell = &mut region.cells[(cx * 8 + cy) as usize];
                    cell.height = if height < 0.0 { 0.0 } else { height };
                }
            }
        }

        // Phase D: candidates sorted ascending by key (std::sort; keys are distinct in practice).
        candidates.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // Phase E: cell types for every other candidate.
        for i in 0..64i32 {
            if candidates.is_empty() {
                break;
            }
            if i & 1 != 0 {
                continue;
            }
            let (_, cx, cy) = candidates.remove(0);
            let idx = (cy + cx * 8) as usize;
            if region.cells[idx].height < 0.0 {
                region.cells[idx].height = 0.0;
            }
            // FUN_00411090: (1 / (1 - i/64) - 1) * 20 + 1 in single precision, truncated.
            let f = i as f32 * 0.015625;
            region.cells[idx].level = ((1.0f32 / (1.0 - f) - 1.0) * 20.0 + 1.0) as i32;
            if (i >> 1) & 1 != 0 {
                // Town cell.
                let cell = &mut region.cells[idx];
                cell.kind = 14;
                cell.radius = 150.0;
                let ys = snap256((cell.y / 65536) as i32);
                let xs = snap256((cell.x / 65536) as i32);
                cell.x = i64::from(xs) << 16;
                cell.y = i64::from(ys) << 16;
                let zx = (((cell.x as f64 * 0.00390625) as i64) / 65536) as i32 % 64;
                let zy = (((cell.y as f64 * 0.00390625) as i64) / 65536) as i32 % 64;
                let level = cell.level;
                let zi = (zx * 64 + zy) as usize;
                region.zones[zi].kind = 3;
                region.zones[zi].sub = (self.rng.rand() % 4) as u8;
                let bx = (rx * 64 + zx) * 256 + 128;
                let by = (ry * 64 + zy) * 256 + 128;
                if self.climate_b(bx, by) > 0.8 {
                    let ca = self.climate_a(bx, by);
                    region.zones[zi].sub = (ca <= 0.8) as u8 + 4;
                }
                region.zones[zi].seed = self.rng.rand() % 10000000 + 1;
                region.zones[zi].level = level;
                region.cells[idx].id = id_counter;
                id_counter += 1 + self.rng.rand() % 50;
                region.cells[idx].variant = i32::from(region.zones[zi].sub);
                region.zones[zi].byte0c = self.level_table(level) as u8;
                continue;
            }
            // Other site.
            let k = self.rng.rand() % 8;
            let elev_neg = climate.elevation < 0;
            match k {
                0 => region.cells[idx].kind = 2,
                1 => {
                    region.cells[idx].kind = 3;
                    region.cells[idx].variant = self.rng.rand() % 3;
                }
                2 => region.cells[idx].kind = if elev_neg { 15 } else { 4 },
                3 => {
                    let cell = &mut region.cells[idx];
                    cell.kind = 5;
                    cell.variant = if climate.b <= 0.8 {
                        0
                    } else if climate.a <= 0.8 {
                        if climate.a < 0.2 { 2 } else { 0 }
                    } else {
                        3
                    };
                    let r = self.rng.rand() & 0xff;
                    cell.radius = (r + 0x100) as f32;
                    let ybase = (cell.radius * 65536.0) as i64 + ((i64::from(region_y) + (i64::from(cy) << 11)) << 16) + 0x1000000;
                    let rp = cell.radius + 256.0;
                    let r2 = self.rng.rand();
                    let y64 = jitter(r2, rp) + ybase;
                    let xbase = (rp * 65536.0) as i64 + ((i64::from(region_x) + (i64::from(cx) << 11)) << 16);
                    let r3 = self.rng.rand();
                    let x64 = jitter(r3, cell.radius + 256.0) + xbase;
                    cell.x = x64;
                    cell.y = y64;
                }
                4 => region.cells[idx].kind = if elev_neg { 15 } else { 6 },
                5 => region.cells[idx].kind = if elev_neg { 15 } else { 7 },
                _ => {
                    let cell = &mut region.cells[idx];
                    cell.kind = if k == 6 { 0xb } else { 0xc };
                    cell.radius = 128.0;
                    let ys = snap256((cell.y / 65536) as i32);
                    let xs = snap256((cell.x / 65536) as i32);
                    cell.x = i64::from(xs) << 16;
                    cell.y = i64::from(ys) << 16;
                }
            }
            let level = region.cells[idx].level;
            region.cells[idx].level_extra = self.level_table(level);
            region.cells[idx].id = id_counter;
            id_counter += 1 + self.rng.rand() % 50;
        }

        // Phase F: up to five mission cells from the remaining candidates.
        let mut count = 0;
        while !candidates.is_empty() && count < 5 {
            let j = (self.rng.rand() as u32 % candidates.len() as u32) as usize;
            let (_, cx, cy) = candidates.remove(j);
            let idx = (cy + cx * 8) as usize;
            region.cells[idx].kind = 10;
            region.cells[idx].id = self.rng.rand() % 10000000 + 1;
            let zx = zone_of(region.cells[idx].x);
            let zy = zone_of(region.cells[idx].y);
            let zi = (zx * 64 + zy) as usize;
            region.zones[zi].kind = 4;
            region.zones[zi].seed = region.cells[idx].id;
            count += 1;
        }

        // Phase G: every cell's level into the zone record at its centre.
        for i in 0..64 {
            let c = region.cells[i];
            let zi = (zone_of(c.x) * 64 + zone_of(c.y)) as usize;
            region.zones[zi].level = c.level;
        }

        // Phase H of the original samples baseHeight over the region and discards the result.

        // Phase I: the four zones closest to the home cell's centre become town quarters.
        for cx in 0..8 {
            for cy in 0..8 {
                let cell = region.cells[(cx * 8 + cy) as usize];
                if cell.kind != 1 {
                    continue;
                }
                let mut list: Vec<(f32, i32, i32)> = Vec::with_capacity(64);
                for zi in 0..8 {
                    for zj in 0..8 {
                        let bx = (rx * 8 + cx) * 0x800 + 0x80 + zi * 0x100;
                        let by = (ry * 8 + cy) * 0x800 + 0x80 + zj * 0x100;
                        let d = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                        let w = 1.0 - d;
                        let w = if w > 0.0 { w * w } else { 0.0 };
                        list.push((w, zi + cx * 8, zj + cy * 8));
                    }
                }
                list.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                for (k, e) in list.iter().take(4).enumerate() {
                    let zi = (e.1 * 64 + e.2) as usize;
                    region.zones[zi].kind = 1;
                    region.zones[zi].sub = k as u8 + 1;
                }
            }
        }

        // Phase J: the mission and monster blobs of every cell from the save database
        // (0x0050fe89..0x0051040a), cell by cell in `cx * 8 + cy` order, mission then monster.
        if self.has_name && let Some(db) = &self.save {
            let db = db.lock().expect("save database");
            for cx in 0..8 {
                for cy in 0..8 {
                    let cell = &mut region.cells[(cx * 8 + cy) as usize];
                    if let Ok(Some(blob)) = db.get(&crate::save::mission_key(rx * 8 + cx, ry * 8 + cy)) {
                        cell.mission.read_blob(&blob);
                        region.missions_active = true;
                    }
                    if let Ok(Some(blob)) = db.get(&crate::save::monster_key(rx * 8 + cx, ry * 8 + cy)) {
                        cell.monster.read_blob(&blob);
                    }
                }
            }
        }

        self.regions.insert(key, Box::new(region));
    }

    /// The level-table roll shared by town and site cells: 0 below level 5, then `rand % 2`,
    /// `rand % 3`, `rand % 3 + 1`, and `rand % 4 + 1` above level 18.
    fn level_table(&mut self, level: i32) -> i32 {
        if level < 5 {
            0
        } else if level < 10 {
            self.rng.rand() % 2
        } else if level < 15 {
            self.rng.rand() % 3
        } else if level > 18 {
            self.rng.rand() % 4 + 1
        } else {
            self.rng.rand() % 3 + 1
        }
    }
}
