//! `cube::World::generateZone` (`Server.exe 0x00518630`).
//!
//! The function is one long sequence of passes over the zone. [`World::generate_zone_stages`]
//! reports a [`Stage`] at the same points where the oracle (`tools/oracle/server_oracle.py
//! zone --stage ...`) snapshots the original, so each pass is verified on its own. Passes past
//! the last implemented stage are not ported yet and the zone is finished as it stands.

use std::fmt;

use cw_math::value_noise_2d as noise;

use crate::fixed::to_block;
use crate::climate::div_trunc;
use crate::surface::Block;
use crate::warp::warp_position;
use crate::world::World;
use crate::appearance::Item;
use crate::spawn_tables::{group_size_range, level_range};
use crate::zone::{AIR_BLOCK, BELOW_BLOCK, GroundItem, Marker, Spawn, Static, WATER_BLOCK, Zone, set_block};

#[inline]
#[allow(clippy::manual_clamp)] // compare-and-branch order of the original
fn clamp01(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * 3.0 * t - t * 2.0 * t * t
}

#[inline]
fn rgb_bytes(c: [f32; 3], kind: u8) -> Block {
    [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, kind]
}

/// Statistics the terrain pass leaves for the later passes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TerrainStats {
    pub min_height: i32,
    pub max_height: i32,
    /// Columns whose surface block has type 4.
    pub type4_columns: i32,
    /// Columns with a mountain factor above 0.25.
    pub mountain_columns: i32,
    /// `warpPosition` of the zone centre, truncated.
    pub warp: [i32; 2],
}

impl World {
    /// `spawnLevel(x64, y64)`, `Server.exe 0x004d2340`: the level of the cell containing a
    /// position, or 1 outside every typed cell.
    pub fn spawn_level(&self, x64: i64, y64: i64) -> i32 {
        let cy = ((y64 as f64 * 0.00048828125) as i64 / 65536) as i32;
        let cx = div_trunc((x64 / 65536) as i32, 2048);
        if let Some(cell) = self.cell(cx, cy)
            && cell.kind != 0
            && cell.norm_distance(x64, y64) <= 1.0
        {
            return cell.level;
        }
        1
    }

    /// `cube::World::generateZone(zx, zy)`.
    pub fn generate_zone(&mut self, zx: i32, zy: i32) {
        self.generate_zone_stages(zx, zy, &mut |_, _| {});
    }

    /// `generateZone` reporting the zone at every [`Stage`] boundary. The callback runs with
    /// the zone as the original has it when the oracle's hook for that stage fires.
    // Mirrors the original loops: explicit height counters, index loops and compare-and-store clamps.
    #[allow(clippy::explicit_counter_loop, clippy::needless_range_loop, clippy::manual_clamp)]
    pub fn generate_zone_stages(&mut self, zx: i32, zy: i32, on_stage: &mut dyn FnMut(Stage, &Zone)) {
        self.generate_zone_hooked(zx, zy, on_stage, &mut |_| {});
    }

    /// [`World::generate_zone_stages`] with `before_cell_npc` run on the world right before
    /// `spawnCellNpc` 0x0050d260 (the end of generation). The original generates with no world
    /// lock held, so `spawnCellNpc` reads the cells, players and creatures as the tick has left
    /// them by then (a region activated during the zone's generation included); a server that
    /// generates in a copy of the world refreshes that play state here.
    #[allow(clippy::explicit_counter_loop, clippy::needless_range_loop, clippy::manual_clamp)]
    pub fn generate_zone_hooked(&mut self, zx: i32, zy: i32, on_stage: &mut dyn FnMut(Stage, &Zone), before_cell_npc: &mut dyn FnMut(&mut World)) {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return;
        }
        let rx = div_trunc(zx, 64);
        let ry = div_trunc(zy, 64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                self.create_region(rx + dx, ry + dy);
            }
        }
        if self.zone(zx, zy).is_some() {
            return;
        }
        let mut zone = Zone::new(zx, zy);
        let seed188 = self.seeds.at(0x800188);
        self.rng.seed(seed188.wrapping_add(zy.wrapping_mul(0x10000)).wrapping_add(zx) as u32);
        let x0 = zx * 256;
        let y0 = zy * 256;
        let warp = warp_position(&self.seeds, x0 + 128, y0 + 128);
        let mut stats = TerrainStats { warp: [warp[0] as i32, warp[1] as i32], ..Default::default() };
        let cell = *self.cell(div_trunc(zx, 8), div_trunc(zy, 8)).expect("regions exist");
        let s178 = self.seeds.f(0x800178);
        let s17c = self.seeds.f(0x80017c);

        // Climate pass. (Performance note, release 2026-09: the climate pass is ~28 ms, the
        // base-height grid below ~90 ms, and the surface pass after it ~130 ms of a ~250 ms
        // terrain stage, `examples/gen_bench.rs`. The first two are pure functions of the
        // position; the surface pass draws `rand` a few times per zone, so its order matters.
        // The original, timed through the oracle, takes 1.3..1.7 s per zone against our
        // 0.4..0.8 s, so nothing here is optimised beyond the original's algorithm.)
        for x in x0..x0 + 256 {
            for y in y0..y0 + 256 {
                let a = self.climate_a(x, y);
                let b = self.climate_b(x, y);
                let c = self.climate_c(x, y);
                let col = zone.column_mut(x, y);
                col.climate_a = a;
                col.climate_b = b;
                col.climate_c = c;
            }
        }
        // Base heights on the 257x257 corner grid.
        let mut heights = vec![0f32; 257 * 257];
        for x in x0..=x0 + 256 {
            for y in y0..=y0 + 256 {
                heights[((y - y0) * 257 + (x - x0)) as usize] = self.base_height(x, y);
            }
        }

        for x in x0..x0 + 256 {
            let xd = f64::from(x);
            let c_4234 = xd * 0.08 + 4234.0;
            let c_432a = xd * 0.08 + 432.0;
            let x01 = xd * 0.01;
            let c_423432 = xd * 0.05 + 423432.0;
            let c_34432 = x01 + 34432.0;
            let c_435 = x01 + 435.0;
            let c_432b = xd * 0.04 + 432.0;
            for y in y0..y0 + 256 {
                let hi = ((y - y0) * 257 + (x - x0)) as usize;
                let hbase = heights[hi];
                let steep = (hbase - heights[hi + 1]).abs() > 0.3 || (hbase - heights[hi + 257]).abs() > 0.3;
                let mf = self.mountain_factor(x, y);
                if mf > 0.25 {
                    stats.mountain_columns += 1;
                }
                let yd = f64::from(y);
                let y08 = yd * 0.08;
                let n = noise(c_432b, yd * 0.04 + 432.0);
                let n2 = noise(c_432a, y08 + 432.0);
                // Both shaping factors form `1 - |n + n' * 0.05|` in double (Server.exe 0x518d27).
                let t = (1.0 - (f64::from(n) + f64::from(n2) * 0.05).abs()) as f32;
                let t = 1.0 - t * t * t;
                let a = t * t + 0.0;
                let n3 = noise(c_4234, y08 + 234.0);
                let n3d = f64::from(n3) * 0.05;
                let n4 = noise(c_423432, yd * 0.05 + 54352.0);
                let t = (1.0 - (n3d + f64::from(n4)).abs()) as f32;
                let t = 1.0 - t * t * t;
                let bfac = t * t + a;
                let mut mf2 = mf;
                if cell.kind == 6 || cell.kind == 13 {
                    let nd = cell.norm_distance(i64::from(x) << 16, i64::from(y) << 16);
                    let t = 1.0 - nd;
                    let t = if t > 0.0 { t * t } else { 0.0 };
                    let mut t = t * 2.0;
                    if t > 1.0 {
                        t = 1.0;
                    }
                    let t = 1.0 - t * t;
                    let cfac = 1.0 - t * t;
                    let n5 = noise(x01 + 985.0, yd * 0.01 + 98584.0);
                    let t = clamp01(n5 * 1.3 + 1.0);
                    mf2 = smooth(t) * cfac * 0.4 + mf;
                }
                let m = mf2 * bfac;
                let mask = smooth(clamp01(m * 1.25));
                let y01 = yd * 0.01;
                let n6 = noise(c_34432, y01 + 8992.0);
                // `(n6 + 1.5) * 60` is evaluated in double and rounded to float before the
                // multiplication by `mf` (`cvtps2pd`/`addsd`/`mulsd`/`cvtpd2ps`, Server.exe 0x519089).
                let height_f = ((f64::from(n6) + 1.5) * 60.0) as f32 * mf + m * 8.0 + hbase;
                let height = height_f as i32;
                let (ca, cb) = {
                    let col = zone.column(x, y);
                    (col.climate_a, col.climate_b)
                };
                let mut blk = self.surface_block(x, y, height, ca, cb);
                if mask > 0.5 {
                    blk[3] = 6;
                }
                let inv = 1.0 - mask;
                let base = [f32::from(blk[0]) * inv, f32::from(blk[1]) * inv, f32::from(blk[2]) * inv];
                let tc = self.terrain_color(x, y, height);
                let rgb = [tc[0] * mask + base[0], tc[1] * mask + base[1], tc[2] * mask + base[2]];
                let btype = blk[3];
                blk = rgb_bytes(rgb, btype);
                zone.column_mut(x, y).height = height;
                if height_f < stats.min_height as f32 {
                    stats.min_height = height;
                }
                let mut mf3 = mf2 * 8.0;
                if (stats.max_height as f32) < height_f {
                    stats.max_height = height;
                }
                if mf3 > 1.0 {
                    mf3 = 1.0;
                }
                let block0 = if !steep {
                    let rc = self.rock_color(x, y, height);
                    let f = 1.0 - mf3;
                    let rc = [rc[0] * f, rc[1] * f, rc[2] * f];
                    let tc2 = self.terrain_color(x, y, height);
                    let s = [f32::from(blk[0]) + tc2[0], f32::from(blk[1]) + tc2[1], f32::from(blk[2]) + tc2[2]];
                    let k = mf3 * 0.5;
                    rgb_bytes([s[0] * k + rc[0], s[1] * k + rc[1], s[2] * k + rc[2]], 6)
                } else {
                    blk
                };
                {
                    let col = zone.column_mut(x, y);
                    if col.blocks.is_empty() {
                        col.resize(1, 0);
                    }
                    col.set_raw(0, block0);
                    if col.blocks.len() < 2 {
                        col.resize(2, 0);
                    }
                    col.set_raw(1, blk);
                    if btype & 0x1f == 4 {
                        stats.type4_columns += 1;
                    }
                    col.f14 = col.height - 8;
                }

                // Valleys, rivers and lakes.
                let n7 = noise(c_435, y01 + 847.0);
                let valley_h = (n7 + 1.0) * 20.0 + hbase;
                let mut skip_cell4 = false;
                if valley_h < height_f {
                    let n8 = noise(xd * 0.005 + s178, yd * 0.005 + s17c);
                    let n9 = noise(s178 + x01, s17c + y01);
                    let t = 1.0 - (n8 + n9).abs() * 4.0;
                    if t < 0.0 {
                        skip_cell4 = true;
                    } else {
                        let t2 = 1.0 - t * t;
                        let v = 1.0 - t2 * t2;
                        let t3 = 1.0 - t * t;
                        let w = 1.0 - t3 * t3 * t3 * t3;
                        let n10 = noise(xd * 0.03 + 7635.0, yd * 0.03 + 123847.0);
                        let d = clamp01((n10 * 10.0 + ((height_f - valley_h) - 20.0)) / 10.0);
                        let e0 = 1.0 - d * d;
                        let dd = 1.0 - d;
                        let e = 1.0 - e0 * e0;
                        let dd7 = dd * dd * dd * dd * dd * dd * dd;
                        let water_h = ((mf * 20.0 + 4.0) * v + valley_h) * (1.0 - dd7) + (height_f + 2.0) * dd7;
                        let f = bfac * mf * 6.0 + valley_h;
                        let floor_h = (1.0 - w) * height_f + w * f + 2.0;
                        if f < height_f - 2.0 {
                            let inv_e = 1.0 - e;
                            let mut hcur = height;
                            for k in 0..2 {
                                let base_block = {
                                    let col = zone.column(x, y);
                                    if hcur < col.height {
                                        BELOW_BLOCK
                                    } else if hcur < col.height + col.blocks.len() as i32 {
                                        let b = col.block_at(hcur - col.height);
                                        if b[3] & 0x1f == 0 && hcur < 1 && b[3] & 0x40 == 0 { WATER_BLOCK } else { b }
                                    } else if hcur > 0 {
                                        AIR_BLOCK
                                    } else {
                                        WATER_BLOCK
                                    }
                                };
                                let tc3 = self.terrain_color(x, y, (k as f32 + height_f) as i32);
                                let bc = [f32::from(base_block[0]), f32::from(base_block[1]), f32::from(base_block[2])];
                                let mixed = [bc[0] * e + inv_e * tc3[0], bc[1] * e + inv_e * tc3[1], bc[2] * e + inv_e * tc3[2]];
                                let b = rgb_bytes(mixed, base_block[3]);
                                set_block(self, &mut zone, x, y, hcur, b);
                                hcur += 1;
                            }
                        }
                        if water_h <= floor_h {
                            skip_cell4 = true;
                        } else {
                            let hstart = floor_h as i32;
                            let water_flag: u8 = if water_h < height_f { 0x40 } else { 0 };
                            let mut hh = hstart;
                            while (hh as f32) < water_h {
                                let b = if hh < 1 { [0, 0, 0, 2] } else { [0, 0, 0, water_flag] };
                                set_block(self, &mut zone, x, y, hh, b);
                                hh += 1;
                            }
                            let hb = (floor_h - 1.0) as i32;
                            let mut sb = self.surface_block(x, y, hb, ca, cb);
                            if mask > 0.5 || e > 0.5 {
                                sb[3] = 6;
                            }
                            let inv2 = 1.0 - e;
                            let base = [f32::from(sb[0]) * inv2, f32::from(sb[1]) * inv2, f32::from(sb[2]) * inv2];
                            let tc4 = self.terrain_color(x, y, hb);
                            let rc2 = self.rock_color(x, y, hb);
                            let s = [rc2[0] + tc4[0], tc4[1] + rc2[1], tc4[2] + rc2[2]];
                            let k = e * 0.4;
                            sb = rgb_bytes([s[0] * k + base[0], s[1] * k + base[1], s[2] * k + base[2]], sb[3]);
                            let base2 = [f32::from(sb[0]) * inv, f32::from(sb[1]) * inv, f32::from(sb[2]) * inv];
                            let tc5 = self.terrain_color(x, y, hb);
                            sb = rgb_bytes([tc5[0] * mask + base2[0], tc5[1] * mask + base2[1], tc5[2] * mask + base2[2]], sb[3]);
                            set_block(self, &mut zone, x, y, hb, sb);
                            if water_h - floor_h > 2.0 && (x + y * 3) % 7 == 0 && self.rng.rand() % 30 == 0 {
                                let x64 = (i64::from(x) << 16) + 32768;
                                let y64 = (i64::from(y) << 16) + 32768;
                                let z64 = i64::from(hstart) << 16;
                                match self.rng.rand() % 4 {
                                    0 => {
                                        if water_h <= height_f && self.rng.rand() % 4 == 0 {
                                            // `cvtdq2pd`, `mulsd 2pi`, `divsd 32767`, `cvtpd2ps` (0x51a37e): double arithmetic with
                                            // the true double 2pi (0x573848), although Ghidra prints the float constant.
                                            let rotation = ((f64::from(self.rng.rand()) * std::f64::consts::TAU) / 32767.0) as f32;
                                            zone.items.push(GroundItem { item: Item { item_type: 0xb, sub_type: 0x13, material: 9, ..Item::NEW }, x: x64, y: y64, z: z64, rotation, f134: 0.1, b138: 0, ..GroundItem::NEW });
                                        }
                                    }
                                    1 => {
                                        if water_h <= height_f {
                                            let entity_type = match self.rng.rand() % 10 {
                                                0 => 131,
                                                1 => 133,
                                                3 => match self.rng.rand() % 100 {
                                                    0 => 138,
                                                    1..=3 => 137,
                                                    4..=8 => 136,
                                                    _ => 135,
                                                },
                                                _ => 132,
                                            };
                                            zone.spawns.push(Spawn { x: x64, y: y64, z: z64, f28: 6, entity_type, level: 1, f_f58: 25.0, ..Spawn::NEW });
                                        }
                                    }
                                    2 => {
                                        if water_h <= height_f {
                                            let level = self.spawn_level(x64, y64);
                                            zone.spawns.push(Spawn { x: x64, y: y64, z: z64, f28: 1, entity_type: 59, level, ..Spawn::NEW });
                                        }
                                    }
                                    _ => zone.marks.push((x, y, (floor_h + 2.0) as i32)),
                                }
                            }
                        }
                    }
                }
                if !skip_cell4 && cell.kind == 4 {
                    let nd = cell.norm_distance(i64::from(x) << 16, i64::from(y) << 16);
                    if nd < 0.25 {
                        let mut hh = (cell.height - 25.0) as i32;
                        let col_h = zone.column(x, y).height;
                        while col_h <= hh {
                            let b = zone.block(x, y, hh);
                            let nb = if b[3] & 0x1f == 0 || b[3] & 0x1f == 2 {
                                WATER_BLOCK
                            } else {
                                rgb_bytes(self.terrain_color(x, y, hh), 3)
                            };
                            set_block(self, &mut zone, x, y, hh, nb);
                            hh -= 1;
                            if hh < col_h {
                                break;
                            }
                        }
                    }
                }
            }
        }
        zone.stats = stats;
        drop(heights);
        zone.record = self.region(rx, ry).expect("region exists").zones[((zx & 63) * 64 + (zy & 63)) as usize];
        on_stage(Stage::Terrain, &zone);

        // Positions of placed features (the `std::list` at generateZone's local_137c), in 16.16
        // fixed blocks; later placements keep their distance from them.
        let mut features: Vec<(i64, i64, i64)> = Vec::new();
        let (center_a, center_b) = {
            let c = zone.column(x0 + 128, y0 + 128);
            (c.climate_a, c.climate_b)
        };
        let spawn = self.spawn;
        let far_from_spawn = |px: i32, py: i32, limit: f32| {
            let dx = px as f32 - spawn[0];
            let dy = py as f32 - spawn[1];
            dy * dy + dx * dx >= limit
        };
        let cell_center = ((cell.x / 65536) as i32, (cell.y / 65536) as i32);
        let center_in_zone = div_trunc(cell_center.0, 256) == zx && div_trunc(cell_center.1, 256) == zy;

        // Boulder field (cell kind 6): a 3x3 grid across the zone (the loop counters are
        // `float*` locals in the decompilation, so their `+ 0x40` step is 256).
        if cell.kind == 6 {
            let mut i = 0;
            while i < 0x300 {
                let mut j = 0;
                while j < 0x300 {
                    if self.rng.rand() % 4 != 0 {
                        let px = x0 + i / 3 + 42;
                        let py = y0 + j / 3 + 42;
                        let f = 1.0 - cell.norm_distance(i64::from(px) << 16, i64::from(py) << 16);
                        if f > 0.0 && f * f >= 0.5 && !near_feature(&features, px, py, 6400.0) {
                            let sx = self.rng.rand() % 10 + 20;
                            let sy = self.rng.rand() % 10 + 20;
                            let sz = self.rng.rand() % 16 + 20;
                            if far_from_spawn(px, py, 3600.0) {
                                let z = zone.top(px, py);
                                self.place_boulder(&mut zone, px, py, z, sx, sy, sz);
                            }
                        }
                    }
                    j += 0x100;
                }
                i += 0x100;
            }
        }
        // (Cell kinds 13 and 4 only locate their centre column here; no side effects.)
        let mut placed_landmark = false;
        if cell.kind == 11 && center_in_zone {
            // One huge boulder at the zone centre.
            let (px, py) = (x0 + 128, y0 + 128);
            let r = self.rng.rand();
            let z = zone.top(px, py);
            self.place_boulder(&mut zone, px, py, z, 100, 100, r % 100 + 100);
            placed_landmark = true;
        } else if cell.kind == 12 && center_in_zone {
            // One huge tree at the zone centre.
            let (px, py) = (x0 + 128, y0 + 128);
            let _ = self.rng.rand();
            let z = zone.top(px, py);
            self.generate_tree(&mut zone, px, py, z, 0x50, 0x50, 6);
            placed_landmark = true;
        }
        if !placed_landmark && !matches!(zone.record.kind, 4 | 1 | 3) {
            // Scattered boulders and rocks.
            let mut n = self.rng.rand() % 4;
            if (center_b > 0.6 || center_b < 0.3) && (center_a > 0.7 || (center_a < 0.4 && (center_a > 0.2 || center_b < 0.8))) {
                n += 2;
            }
            for _ in 0..n {
                let sx = self.rng.rand() % 40 + 10;
                let sy = self.rng.rand() % 40 + 10;
                let sz = self.rng.rand() % 25 + 10;
                let px = self.rng.rand() % ((64 - sx) * 4) + (sx + zx * 128) * 2;
                let py = self.rng.rand() % ((64 - sy) * 4) + (sy + zy * 128) * 2;
                if !far_from_spawn(px, py, 3600.0) || near_feature(&features, px, py, 6400.0) {
                    continue;
                }
                if self.cell_falloff(px, py) <= 0.6 && 1.0 - self.height_factor_a(px, py) * 50.0 < 0.0 {
                    let mf = self.mountain_factor(px, py);
                    let z = zone.top(px, py);
                    let r = self.rng.rand();
                    if r % 2 == 0 || mf >= 0.25 {
                        self.place_boulder(&mut zone, px, py, z, sx, sy, sz);
                    } else {
                        self.place_rock(&mut zone, px, py, z, sx, sy, sz / 2);
                    }
                }
            }
        }
        on_stage(Stage::Features, &zone);

        // Ground layer: a type-0xb block under the surface of low, flat ground, air above.
        let region_variant = self.region(rx, ry).expect("region exists").variant;
        for x in x0..x0 + 256 {
            let xd = f64::from(x);
            let xq = (x / 2) * 0xea;
            for y in y0..y0 + 256 {
                let k = 1.0 - self.height_factor_b(x, y) * 50.0;
                let cf = self.cell_falloff(x, y);
                if k < 0.0 {
                    continue;
                }
                let bh = self.base_height(x, y);
                let bh0 = if bh < 0.0 { 0.0 } else { bh };
                let inv = 1.0 - k;
                let h1 = (bh0 + 1.0) as i32;
                let e = 1.0 - inv * inv * inv;
                let yd = f64::from(y);
                let n = noise(xd * 0.02 + 55432.0, yd * 0.02 + 974.0);
                let mut top = ((n + 1.0) * 4.0 + e * 5.0 + bh0) as i32;
                let b = zone.block(x, y, h1);
                if b[3] & 0x1f == 0 || b[3] & 0x1f == 2 || b[3] & 0x40 != 0 {
                    continue;
                }
                let n2 = noise(xd * 0.05 + 843.0, yd * 0.05 + 984.0);
                let mut c = [40.0 * n2 + 140.0, 40.0 * n2 + 140.0, 40.0 * n2 + 140.0];
                let yq = (y / 2) * 0xea;
                if noise(f64::from(xq + 0x12e2), f64::from(yq + 0xc11a)) > 0.5 {
                    let n4 = noise(f64::from(xq), f64::from(yq + 0x31));
                    c = [20.0 * n4 + c[0], 20.0 * n4 + c[1], 20.0 * n4 + c[2]];
                }
                if matches!(region_variant, 1 | 4 | 5) {
                    c = self.rock_color(x, y, h1);
                }
                let cc = [cf * c[0], cf * c[1], cf * c[2]];
                let rc = self.rock_color(x, y, h1);
                let inv_cf = 1.0 - cf;
                let mut out = [inv_cf * rc[0] + cc[0], inv_cf * rc[1] + cc[1], inv_cf * rc[2] + cc[2]];
                for v in &mut out {
                    if *v < 0.0 {
                        *v = 0.0;
                    }
                    if *v > 255.0 {
                        *v = 255.0;
                    }
                }
                set_block(self, &mut zone, x, y, h1 - 1, rgb_bytes(out, 0xb));
                if cf > 0.92 {
                    top = zone.top(x, y);
                }
                for h in h1..top {
                    if zone.block(x, y, h)[3] & 0x40 == 0 {
                        set_block(self, &mut zone, x, y, h, AIR_BLOCK);
                    }
                }
            }
        }
        on_stage(Stage::Layers, &zone);

        // Terraces: stepped shores on low ground, with water filling the steps.
        let mut terraces: Vec<(i32, i32, i32)> = Vec::new();
        for x in x0..x0 + 256 {
            let xd = f64::from(x);
            for y in y0..y0 + 256 {
                let k = 1.0 - self.height_factor_a(x, y) * 50.0;
                if k < 0.0 || self.cell_falloff(x, y) > 0.95 {
                    continue;
                }
                let mut bh = self.base_height(x, y);
                if bh < 0.0 {
                    bh = 0.0;
                }
                let q = (bh as i32 / 5) * 5;
                let fq = q as f32;
                let frac = (bh - fq) / 5.0;
                let t = if frac >= 0.5 {
                    let t = 1.0 - (frac - 0.5) * 4.0;
                    if t < 0.0 { (t + 1.0) * (t + 1.0) - 1.0 } else { t }
                } else {
                    frac * 2.0
                };
                let hq = if t < 0.0 { (fq - t * 5.0) as i32 } else { q };
                let hstart = ((fq - t * 5.0) + 2.0) as i32;
                for h in hstart..=q {
                    if zone.block(x, y, h)[3] & 0x40 == 0 {
                        let inv = 1.0 - t;
                        let c = [inv * 0.0 + t * 0.0, inv * 0.0 + t * 0.0, inv * 255.0 + t * 0.0];
                        set_block(self, &mut zone, x, y, h, rgb_bytes(c, 2));
                    }
                }
                if q <= hq {
                    let b = zone.block(x, y, hq);
                    if b[3] & 0x1f != 2 && b[3] & 0x40 == 0 {
                        set_block(self, &mut zone, x, y, hq, rgb_bytes(self.terrain_color(x, y, hq), 3));
                        if self.rng.rand() % 200 == 0 {
                            terraces.push((x, y, hq));
                        }
                    }
                }
                let inv = 1.0 - k;
                let k2 = 1.0 - inv * inv * inv;
                let n = noise(xd * 0.02 + 55432.0, f64::from(y) * 0.02 + 974.0);
                let top = ((n + 1.0) * 2.0 + k2 * 5.0 + bh) as i32;
                for h in hq + 1..top {
                    if zone.block(x, y, h)[3] & 0x40 == 0 {
                        set_block(self, &mut zone, x, y, h, AIR_BLOCK);
                    }
                }
            }
        }
        on_stage(Stage::Terraces, &zone);
        if zone.record.kind == 3 || zone.record.kind == 5 {
            let (style, special) = if zone.record.kind == 3 { (i32::from(zone.record.sub), false) } else { (3, true) };
            let mut extras = crate::dungeon::DungeonExtras::default();
            self.generate_dungeon_ext(&mut zone, &mut extras, x0 + 32, y0 + 32, style, special);
            for m in extras.markers {
                zone.markers.push(Marker { kind: m.kind, spawn_index: m.spawn_index, creature_type: m.creature_type, x: m.x, y: m.y, z: m.z, ..Marker::NEW });
            }
            for (i, items) in extras.static_items {
                for it in items {
                    zone.statics[i].inventory.add_item(it, -1);
                }
            }
        }
        on_stage(Stage::Dungeons, &zone);

        // Rock blobs (type 6) on a sample of terrace blocks.
        for i in 0..terraces.len() {
            let (x, y, h) = terraces[i];
            let a = self.rng.rand() % 3 + 2;
            let b = self.rng.rand() % 3 + 2;
            let c = self.rng.rand() % 3 + 2;
            self.blob(&mut zone, x, y, h, a, b, c, 6, true, true);
        }
        on_stage(Stage::Decor1, &zone);

        // Blobs (type 6, flag 0x20) on the positions the terrain pass marked.
        for i in 0..zone.marks.len() {
            let (x, y, h) = zone.marks[i];
            let a = self.rng.rand() % 4 + 4;
            let b = self.rng.rand() % 4 + 4;
            let c = self.rng.rand() % 6 + 4;
            self.blob(&mut zone, x, y, h, a, b, c, 0x26, true, false);
        }
        // (The spawn-point model is only stamped for a world without a name, never on a server.)

        // A camp in every other zone: up to ten random sites.
        if (zy + zx) % 2 != 0 {
            for _ in 0..10 {
                let py = y0 + self.rng.rand() % 160 + 48;
                let px = x0 + self.rng.rand() % 160 + 48;
                let mut pos = ((i64::from(px) << 16) + 32768, (i64::from(py) << 16) + 32768, 0i64);
                if cell.kind == 1 || cell.kind == 5 {
                    let f = 1.0 - cell.norm_distance(pos.0, pos.1);
                    if f > 0.0 && f * f > 0.0 {
                        continue;
                    }
                }
                let (bx, by) = ((pos.0 / 65536) as i32, (pos.1 / 65536) as i32);
                if zone.contains(bx, by) {
                    pos.2 = i64::from(zone.top(bx, by)) << 16;
                }
                if self.try_place_camp(&mut zone, pos) {
                    features.push(pos);
                    break;
                }
            }
        }

        // Small blobs on ground away from the spawn and the placed features.
        if !matches!(zone.record.kind, 4 | 1 | 3) {
            let mut n = self.rng.rand() % 10;
            if (center_b > 0.6 || center_b < 0.3) && (center_a > 0.7 || (center_a < 0.4 && (center_a > 0.2 || center_b < 0.8))) {
                n += 10;
            }
            for _ in 0..n {
                let a = self.rng.rand() % 8 + 3;
                let b = self.rng.rand() % 8 + 3;
                let c = self.rng.rand() % 8 + 3;
                let px = self.rng.rand() % ((64 - a) * 4) + (zx * 128 + a) * 2;
                let py = self.rng.rand() % ((64 - b) * 4) + (zy * 128 + b) * 2;
                if !far_from_spawn(px, py, 400.0) || near_feature(&features, px, py, 1600.0) {
                    continue;
                }
                if self.cell_falloff(px, py) > 0.25 {
                    continue;
                }
                let zt = {
                    let col = zone.column(px, py);
                    let mut zt = col.height + col.blocks.len() as i32;
                    let mut i = col.blocks.len() as i32 - 1;
                    while i > -1 {
                        let t = col.block_at(i)[3] & 0x1f;
                        if t != 0 && t != 2 {
                            break;
                        }
                        zt -= 1;
                        i -= 1;
                    }
                    zt
                };
                let under = zone.block(px, py, zt - 1);
                let t = under[3] & 0x1f;
                if t != 0xb && t != 8 && t != 7 && under[3] & 0x40 == 0 {
                    self.blob(&mut zone, px, py, zt, a, b, c, 0x26, false, true);
                }
            }
        }
        if cell.kind == 1 || cell.kind == 5 {
            let mut extras = crate::settlement::SettlementExtras::default();
            self.generate_settlement_ext(&mut zone, &cell, &mut extras);
            for ex in &extras.spawns {
                for add in &ex.inventory {
                    zone.spawns[ex.spawn_index].inventory.add_item(add.item, add.bag);
                }
            }
        }
        if zone.record.kind == 4 {
            // A ring of rock pillars around the zone centre and an altar static (kind 0x2d).
            let (px, py) = (x0 + 128, y0 + 128);
            let top = zone.top(px, py);
            features.push((i64::from(px) << 16, i64::from(py) << 16, i64::from(top) << 16));
            let n = self.rng.rand() % 3 + 6; // 0x51d4e7
            for k in 0..n {
                let ang = ((f64::from(2 * k) * std::f64::consts::PI) / f64::from(n)) as f32;
                let sinv = cw_math::sin(f64::from(ang)) as f32;
                let cosv = cw_math::cos(f64::from(ang)) as f32;
                let cx = px + (cosv * 25.0) as i32;
                let cy = py + (sinv * 25.0) as i32;
                let h = if zone.contains(cx, cy) { zone.top(cx, cy) } else { top };
                let base = h + 4;
                let a = self.rng.rand() % 4 + 3; // 0x51d5e2
                let b = self.rng.rand() % 4 + 3; // 0x51d603
                for x in cx - a * 2..=cx + a * 2 {
                    let xd05 = f64::from(x) * 0.05;
                    for y in cy - b * 2..=cy + b * 2 {
                        let yd05 = f64::from(y) * 0.05;
                        let k1 = noise(yd05, yd05) * 0.3;
                        let bf = b as f32;
                        let mut z = base + 20;
                        while base - 20 <= z {
                            let t1 = (z - base) as f32 / 10.0;
                            let ty = (y - cy) as f32 / bf + k1;
                            let tx = noise(xd05, f64::from(z) * 0.05) * 0.3 + (x - cx) as f32 / a as f32;
                            if tx * tx + ty * ty + t1 * t1 <= 1.0 && zone.block(x, y, z)[3] & 0x40 == 0 {
                                set_block(self, &mut zone, x, y, z, rgb_bytes(self.terrain_color(x, y, z), 6));
                            }
                            z -= 1;
                        }
                    }
                }
            }
            let mut altar = Static { kind: 0x2d, scale: [4.0, 4.0, 5.0], ..Static::NEW };
            altar.x = (i64::from(px) << 16) + 229376;
            altar.y = (i64::from(py) << 16) + 229376;
            altar.z = i64::from(top) << 16;
            let solid_at = |zone: &Zone, s: &Static| {
                let b = zone.block(to_block(s.x), to_block(s.y), to_block(s.z));
                b[3] & 0x1f != 0 && b[3] & 0x1f != 2
            };
            while !solid_at(&zone, &altar) {
                altar.z -= 65536;
            }
            while solid_at(&zone, &altar) {
                altar.z += 65536;
            }
            altar.rotation = self.rng.rand() % 4; // 0x51d9f5
            zone.statics.push(altar);
        }
        on_stage(Stage::Trees, &zone);

        // Tree grid: 14x14 candidates 18 blocks apart.
        let s294 = self.seeds.f(0x800294);
        let s298 = self.seeds.f(0x800298);
        for i in 0..14 {
            let gx = x0 + i * 18;
            for j in 0..14 {
                let gy = y0 + 8 + j * 18;
                let mut f = 0.0f32;
                if cell.kind == 3 {
                    let t = 1.0 - cell.norm_distance(i64::from(gx + 8) << 16, i64::from(gy) << 16);
                    f = if t > 0.0 { t * t } else { 0.0 };
                }
                let a = zone.column(gx + 8, gy).climate_a;
                let r = self.rng.rand(); // 0x51dd6a
                let mut size = ((r % 5) as f32 + a * 2.0 + 6.0 + f * 4.0) as i32;
                let r = self.rng.rand(); // 0x51dd9c
                let mut py = (gy - 8) + size;
                let mut height = (((r as f32 * 8.0) / 32767.0 + f * 6.0 + 8.0) * (a * 0.5 + 1.0)) as i32;
                let mut px = gx + size;
                if !far_from_spawn(px, py, 400.0) {
                    continue;
                }
                let lim = x0 - size / 2 + 256;
                if lim <= px {
                    px = lim;
                }
                let lim = y0 - size / 2 + 256;
                if lim <= py {
                    py = lim;
                }
                if near_feature(&features, px, py, 1600.0) {
                    continue;
                }
                let pxd = f64::from(px) * 0.001;
                let pyd = f64::from(py) * 0.001;
                let n = noise(s294 + pxd, s298 + pyd);
                let mut kind = if self.rng.rand() % 2 != 0 { 5 } else { 0 }; // 0x51df1e
                if n <= 0.3 {
                    if self.rng.rand() % 10 == 0 {
                        kind = 1;
                    }
                } else if self.rng.rand() % 10 != 0 {
                    kind = 1;
                }
                if self.rng.rand() % 10 == 0 {
                    kind = 2;
                }
                let b = zone.column(px, py).climate_b;
                if b > 0.8 && a > 0.7 {
                    kind = 4 + i32::from(self.rng.rand() % 2 != 0);
                    if self.rng.rand() % 4 == 0 {
                        kind = 3;
                    }
                }
                let mut shrink = false;
                if b >= 0.3 {
                    let mut grow = false;
                    if b < 0.7 && noise(pxd + 8473.0, pyd + 9438.0) > 0.8 && self.rng.rand() % 5 != 0 {
                        kind = 2;
                        grow = true;
                    }
                    if !grow && kind == 1 {
                        shrink = true;
                    } else if grow || kind == 2 {
                        height += self.rng.rand() % (height / 2);
                    }
                } else {
                    let mut grow = false;
                    if b > 0.2 && noise(pxd + 8473.0, pyd + 9438.0) > 0.6 && self.rng.rand() % 5 != 0 {
                        kind = 2;
                        grow = true;
                    }
                    if grow {
                        height += self.rng.rand() % (height / 2);
                    } else {
                        kind = 1;
                        shrink = true;
                    }
                }
                if shrink {
                    if (height as f32) * 0.5 < size as f32 {
                        size = ((height as f32) * 0.5) as i32;
                    }
                    if size < 1 {
                        size = 1;
                    }
                }
                if x0 + 256 < px + size {
                    px = x0 + (256 - size);
                }
                if y0 + 256 < size + py {
                    py = y0 + (256 - size);
                }
                let density = self.tree_density(px, py) + f;
                let r = self.rng.rand(); // 0x51e3f8
                if r as f32 / 32767.0 > density {
                    continue;
                }
                let mut h = zone.top(px, py) - 1;
                loop {
                    let t = zone.block(px, py, h)[3] & 0x1f;
                    if t != 0 && t != 2 {
                        break;
                    }
                    h -= 1;
                }
                if h > -1 {
                    let under = zone.block(px, py, h)[3] & 0x1f;
                    h += 1;
                    let col = zone.column(px, py);
                    let mut k = kind;
                    if col.climate_b > 0.8 && col.climate_a < 0.2 {
                        k = 3;
                    }
                    if under == 4 || under == 10 || (under == 9 && h < 3) {
                        self.generate_tree(&mut zone, px, py, h, size, height, k);
                    }
                }
            }
        }
        // The spawn-point static in every other zone.
        if (zy + zx) % 2 == 0 {
            let px = x0 + self.rng.rand() % 224 + 16;
            let py = y0 + self.rng.rand() % 224 + 16;
            let mut h = zone.top(px, py);
            loop {
                let t = zone.block(px, py, h)[3] & 0x1f;
                if t != 0 && t != 2 {
                    break;
                }
                h -= 1;
            }
            let rot = self.rng.rand() % 4;
            zone.statics.push(Static {
                kind: 0,
                x: (i64::from(px) << 16) + 32768,
                y: (i64::from(py) << 16) + 32768,
                z: i64::from(h + 1) << 16,
                rotation: rot,
                scale: [2.0, 2.0, 8.0],
                ..Static::NEW
            });
        }
        // Creature candidate grid, then the cell's creature population.
        if !matches!(cell.kind, 0 | 10 | 14 | 1 | 5) {
            let mut points: Vec<(i64, i64, i64)> = Vec::new();
            let mut row = 0;
            let mut xoff = 0;
            while xoff < 0xfc {
                let mut counter = row;
                let mut yoff = 0;
                while yoff < 0xfc {
                    if counter % 5 == 0 {
                        let px = x0 + xoff + 4;
                        let py = y0 + yoff + 4;
                        let t = 1.0 - cell.norm_distance(i64::from(px) << 16, i64::from(py) << 16);
                        let f = if t > 0.0 { t * t } else { 0.0 };
                        let r = self.rng.rand(); // 0x51e8f4
                        if r as f32 / 32767.0 <= f * 0.75 {
                            let mut h = zone.column(px, py).f14;
                            loop {
                                let t = zone.block(px, py, h)[3] & 0x1f;
                                if t == 0 || t == 2 {
                                    break;
                                }
                                h += 1;
                            }
                            points.push(((i64::from(px) << 16) + 32768, (i64::from(py) << 16) + 32768, i64::from(h) << 16));
                        }
                    }
                    yoff += 18;
                    counter += 3;
                }
                xoff += 18;
                row += 1;
            }
            self.populate_zone_creatures(&mut zone, &cell, &points);
        }
        // The boss of a kind-9 cell at the cell centre.
        if cell.kind == 9 && center_in_zone {
            let (cx, cy) = cell_center;
            let z = i64::from(zone.column(cx, cy).height) << 16;
            zone.spawns.push(Spawn { x: cell.x, y: cell.y, z, f28: 1, entity_type: 0x6b, level: cell.level, b58: cell.level_extra as u8, ..Spawn::NEW });
        }
        // (The zone-record vector sorted here is never used.)
        on_stage(Stage::Creatures, &zone);

        // Creature spawns on a 3x3 grid.
        for i in 0..3 {
            for j in 0..3 {
                if self.rng.rand() % 4 == 0 {
                    continue;
                }
                let px = x0 + i * 0x55 + 0x18 + self.rng.rand() % 10;
                let py = y0 + j * 0x55 + 0x18 + self.rng.rand() % 10;
                if cell.kind != 0 && cell.kind != 10 {
                    let t = 1.0 - cell.norm_distance(i64::from(px) << 16, i64::from(py) << 16);
                    if t > 0.0 && t * t > 0.3 {
                        continue;
                    }
                }
                let b = zone.column(px, py).climate_b;
                if b < 0.2 && self.rng.rand() % 4 == 0 {
                    continue;
                }
                let a = zone.column(px, py).climate_a;
                if a < 0.2 && self.rng.rand() % 4 == 0 {
                    continue;
                }
                if near_feature(&features, px, py, 400.0) {
                    continue;
                }
                let mut h = zone.column(px, py).f14;
                loop {
                    let t = zone.block(px, py, h)[3] & 0x1f;
                    if t == 0 || t == 2 {
                        break;
                    }
                    h += 1;
                }
                let under = zone.block(px, py, h - 1)[3] & 0x1f;
                if self.cell_falloff(px, py) > 0.0 {
                    continue;
                }
                if self.height_factor_b(px, py) < 1.0 {
                    continue;
                }
                let mut spawn = Spawn {
                    x: (i64::from(px) << 16) + 32768,
                    y: (i64::from(py) << 16) + 32768,
                    z: i64::from(h) << 16,
                    ..Spawn::NEW
                };
                spawn.rotation = (self.rng.rand() as f32 * 360.0) / 32767.0;
                spawn.level = 1;
                spawn.entity_type = self.pick_creature_type(px, py, h, false);
                let mut critter = false;
                if under == 0xc {
                    spawn.entity_type = if self.rng.rand() % 2 != 0 { 0x7e } else { 0x82 };
                    critter = true;
                } else if h > -1 {
                    if matches!(under, 4 | 5 | 9) {
                        if self.rng.rand() % 3 != 0 && a > 0.8 && b < 0.1 {
                            spawn.entity_type = if self.rng.rand() % 2 == 0 { 0x7c } else { 0x80 };
                            critter = true;
                        } else if under == 4 && self.rng.rand() % 3 != 0 && zone.column(px, py).climate_a > 0.1 {
                            spawn.entity_type = match self.rng.rand() % 4 {
                                1 => 0x7b,
                                2 => 0x7f,
                                3 => 0x7d,
                                _ => 0x78,
                            };
                            spawn.f_f58 = 25.0;
                            critter = true;
                        }
                    } else if under == 10 && self.rng.rand() % 4 == 0 {
                        spawn.entity_type = match self.rng.rand() % 4 {
                            1 => 0x7b,
                            2 => 0x7d,
                            3 => 0x7a,
                            _ => 0x79,
                        };
                        spawn.f_f58 = 25.0;
                        critter = true;
                    }
                }
                if critter {
                    spawn.f28 = 6;
                    zone.spawns.push(spawn);
                    continue;
                }
                if spawn.f28 == 1 && spawn.appearance.flags & 0x1000 == 0 && self.rng.rand() % 100 == 0 {
                    spawn.appearance.flags |= 0x200;
                }
                let home = self.cell_at_block((spawn.x / 65536) as i32, (spawn.y / 65536) as i32).copied();
                if spawn.f28 != 6 {
                    let (lo, hi) = level_range(spawn.entity_type);
                    spawn.level = self.rng.rand() % (hi - lo + 1) + lo;
                    if let Some(hc) = home
                        && lo <= hc.level
                        && hc.level <= hi
                    {
                        let t = 1.0 - hc.norm_distance(spawn.x, spawn.y);
                        if t > 0.0 && t * t > 0.0 {
                            spawn.b58 = hc.level_extra as u8;
                        }
                    }
                }
                if spawn.f28 == 1 {
                    spawn.f38[2] = 0x1499700;
                    spawn.f38[3] = 0x5265c00;
                }
                let leader = spawn.clone();
                zone.spawns.push(spawn);
                let (glo, ghi) = group_size_range(leader.entity_type);
                let n = glo - 1 + self.rng.rand() % (ghi - glo + 1);
                for k in 0..n {
                    let mut member = Spawn { f28: 1, ..Spawn::NEW };
                    let ang = (k as f32 * 6.2831855) / n as f32;
                    let sx = (cw_math::sin(f64::from(ang)) as f32 * 8.0 * 65536.0) as i64;
                    let cx = (cw_math::cos(f64::from(ang)) as f32 * 8.0 * 65536.0) as i64;
                    member.x = leader.x + cx;
                    member.y = leader.y + sx;
                    member.z = leader.z;
                    member.entity_type = self.group_member_type(leader.entity_type);
                    member.appearance.flags &= !0x200;
                    if member.f28 != 6 {
                        let (lo, hi) = level_range(member.entity_type);
                        member.level = self.rng.rand() % (hi - lo + 1) + lo;
                        member.b58 = leader.b58;
                    }
                    member.rotation = (self.rng.rand() as f32 * 360.0) / 32767.0;
                    zone.spawns.push(member);
                }
            }
        }
        on_stage(Stage::Decor2, &zone);

        // Per-column decorations (props, ground items, a few statics).
        self.decorate_columns(&mut zone);
        // (The `zone+0x24` vector gets ids here too; nothing fills it during generation.)
        // Creature initialisation: id, appearance, class and equipment, then the item rolls.
        // The oracle's `appearance` stage fires at the first initAppearance call, i.e. after the
        // first spawn's id is assigned.
        let mut i = 0;
        while i < zone.spawns.len() {
            if zone.spawns[i].f38[4] == 0 && zone.spawns[i].f38[5] == 0 {
                let id = spawn_id(zx, zy, i as i32);
                zone.spawns[i].f38[4] = id as u32;
                zone.spawns[i].f38[5] = (id >> 32) as u32;
            }
            if i == 0 {
                on_stage(Stage::Appearance, &zone);
            }
            let mut s = std::mem::replace(&mut zone.spawns[i], Spawn::NEW);
            self.init_appearance(&mut s);
            self.init_creature(&mut s);
            let boss = s.appearance.flags & 0x200 != 0;
            if boss {
                let r = self.rng.rand(); // 0x521143
                let mut item = Item { item_type: 0xb, sub_type: 0xe, rarity: 2, level: s.level as u16, ..Item::NEW };
                item.material = (r % 4 - 0x80) as u8;
                s.inventory.add_item(item, -1);
            }
            for k in [7, 6, 5, 2, 4, 3, 1] {
                s.equipment[k].rarity = self.rarity_roll(i32::from(s.b58), boss) as u8;
            }
            if !boss {
                for k in [7, 6, 5, 2, 4, 3, 1, 8, 9] {
                    let mut item = s.equipment[k];
                    self.adjust_item_level(&mut item, 0.05, false);
                    s.equipment[k] = item;
                }
            }
            if s.f28 == 1 && !boss {
                let _ = self.rng.rand(); // 0x5214d5
            }
            zone.spawns[i] = s;
            i += 1;
        }
        // Loot markers (kind 9): ground items counted by (type, sub-type, material) in a
        // `std::map`; on the way every item not flagged at +0x138 is dropped until the block half
        // a block below it is solid.
        let mut item_counts: std::collections::BTreeMap<(i32, i32, i32), i32> = std::collections::BTreeMap::new();
        for i in 0..zone.items.len() {
            let key = (i32::from(zone.items[i].item.item_type), i32::from(zone.items[i].item.sub_type), i32::from(zone.items[i].item.material));
            *item_counts.entry(key).or_insert(0) += 1;
            if zone.items[i].b138 & 1 == 0 {
                let (ix, iy) = (zone.items[i].x, zone.items[i].y);
                let below = |zone: &Zone, z: i64| {
                    // `int64SubFloat` 0x004014b0 with 0.5f * 65536f: half a block down.
                    let b = zone.block(to_block(ix), to_block(iy), to_block(z - 32768));
                    b[3] & 0x1f != 0 && b[3] & 0x1f != 2
                };
                while !below(&zone, zone.items[i].z) {
                    zone.items[i].z -= 65536;
                }
            }
        }
        let (cx64, cy64) = (i64::from(x0 + 128) << 16, i64::from(y0 + 128) << 16);
        for (&(a, b, c), &n) in &item_counts {
            if (a == 1 || a == 0xb) && n > 7 {
                zone.markers.push(Marker { kind: 9, b4: a as u8, b5: b as u8, b11: c as u8, x: cx64, y: cy64, z: 0, ..Marker::NEW });
            }
        }
        // Creature group markers (kind 10): spawns with the day-length timer at +0x44 counted by
        // (+0x28, entity type).
        let mut creature_counts: std::collections::BTreeMap<(i32, i32), i32> = std::collections::BTreeMap::new();
        for s in &zone.spawns {
            if s.f38[3] == 0x5265c00 {
                *creature_counts.entry((s.f28, s.entity_type)).or_insert(0) += 1;
            }
        }
        for (&(a, b), &n) in &creature_counts {
            if (a == 1 || a == 6 || a == 5) && n > 4 {
                zone.markers.push(Marker { kind: 10, creature_type: b, x: cx64, y: cy64, z: 0, ..Marker::NEW });
            }
        }
        // spawnCellNpc 0x0050d260: the cell's saved boss, or with no saved NPC state a
        // wandering group per zone (except the world-spawn zone) of a named world.
        zone.gen_spawn_count = zone.spawns.len(); // zone+0xa0 (0x00521b3e)
        before_cell_npc(self);
        self.spawn_cell_npc(&mut zone);
        // The zone's save blob (0x00521bed): ground items, modified blocks, spawn and static
        // state of a previously saved zone, for a named world.
        if self.has_name && let Some(blob) = self.saved_blob(&crate::save::zone_key(zx, zy)) {
            self.apply_zone_blob(&mut zone, &blob);
        }
        self.post_process_blocks(&mut zone, x0, y0, x0 + 256, y0 + 256, 0);
        for i in 0..zone.props.len() {
            let (px, py, pz) = (zone.props[i].x, zone.props[i].y, zone.props[i].z);
            // The coordinates are truncating `/65536` here (not getBlockFixed's adjustment).
            let b = zone.block((px / 65536) as i32, (py / 65536) as i32, (pz / 65536) as i32);
            let t = b[3] & 0x1f;
            let v: u8 = if t == 0xd {
                0xff
            } else if t != 0 && t != 2 {
                0
            } else if b[0] < 5 {
                5
            } else {
                b[0]
            };
            zone.props[i].f28 = f32::from(v);
        }
        on_stage(Stage::Full, &zone);

        self.zones.insert((zx as u32) << 16 | zy as u32, Box::new(zone));
    }

    /// `rarityRoll(base, boss)`, `Server.exe 0x0052bf40`: `rand() % (base + 1)` with three
    /// long-shot bumps, `base + 1` for bosses, capped at 4. Always draws four times.
    pub fn rarity_roll(&mut self, base: i32, boss: bool) -> i32 {
        let mut v = self.rng.rand() % (base + 1);
        if self.rng.rand() % 100 == 0 {
            v += 1;
        }
        if self.rng.rand() % 1000 == 0 {
            v += 1;
        }
        if self.rng.rand() % 10000 == 0 {
            v += 1;
        }
        if boss {
            v = base + 1;
        }
        if v > 4 {
            v = 4;
        }
        v
    }

    /// `Item::adjustLevel(delta, up)`, `Server.exe 0x00414470`: re-rolls an item's level within
    /// the band `delta` below its current level on the level curve.
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN semantics of the original comparison
    pub fn adjust_item_level(&mut self, item: &mut Item, delta: f32, up: bool) {
        let lvl = item.level as i16;
        if lvl < 2 {
            return;
        }
        let t = level_curve(f32::from(lvl)) - delta;
        if !(0.0 < t) {
            item.level = 1;
            return;
        }
        let mut lo = level_curve_inv(t) as i32;
        let mut hi = i32::from(lvl);
        if lo < 1 {
            lo = 1;
        }
        if up {
            let mut f = level_curve(hi as f32) + delta;
            if f > 0.9999 {
                f = 0.9999;
            }
            hi = level_curve_inv(f) as i32;
        }
        let r = self.rng.rand();
        let n = (hi - lo) + 1;
        item.level = (r % n + lo) as u16;
    }

    /// `treeDensity(x, y, zone)`, `Server.exe 0x004d9010`.
    pub fn tree_density(&self, x: i32, y: i32) -> f32 {
        let n = noise(f64::from(x) * 0.008 + self.seeds.f(0x8001f4), f64::from(y) * 0.008 + self.seeds.f(0x8001f8));
        let v = n * 1.2;
        let v = if v >= 0.0 {
            if v <= 1.0 { v } else { 1.0 }
        } else {
            0.0
        };
        let a = self.climate_a(x, y);
        let d = a * 0.2 * a + 0.02 + v;
        if d > 1.0 { 1.0 } else { d }
    }

    /// The noise-warped ellipsoid of terrain-coloured blocks that generateZone stamps on
    /// terrace blocks, marks and open ground: half-axes `(a, b)` in x and y, `2c` in z.
    /// `skip_80` leaves protected blocks alone and `skip_40` blocks with flag 0x40.
    #[allow(clippy::too_many_arguments)]
    fn blob(&self, zone: &mut Zone, x: i32, y: i32, h: i32, a: i32, b: i32, c: i32, kind: u8, skip_80: bool, skip_40: bool) {
        let cf = c as f32;
        for px in x - a..=x + a {
            let dxn = (px - x) as f32 / a as f32;
            let pxd = f64::from(px);
            for py in y - b..=y + b {
                let zmin = h - c * 2;
                let zmax = h + c * 2;
                if zmin > zmax {
                    continue;
                }
                let n = noise(pxd * 0.05, f64::from(py) * 0.05);
                let dyn_ = (py - y) as f32 / b as f32;
                let r2 = dyn_ * dyn_ + dxn * dxn;
                let nn = n * 0.8;
                let mut pz = zmax;
                while zmin <= pz {
                    let f = (pz - h) as f32 / cf + nn;
                    if f * f + r2 <= 1.0 {
                        let blk = zone.block(px, py, pz);
                        if !(skip_80 && blk[3] & 0x80 != 0) && !(skip_40 && blk[3] & 0x40 != 0) {
                            set_block(self, zone, px, py, pz, rgb_bytes(self.terrain_color(px, py, pz), kind));
                        }
                    }
                    pz -= 1;
                }
            }
        }
    }

}

/// `levelCurve(level)`, `Server.exe 0x00407d60`: `1 - 1 / ((level - 1) * 0.05 + 1)`.
#[inline]
pub fn level_curve(level: f32) -> f32 {
    1.0 - 1.0 / ((level - 1.0) * 0.05 + 1.0)
}

/// `levelCurveInv(f)`, `Server.exe 0x00411090`: `(1 / (1 - f) - 1) * 20 + 1`.
#[inline]
pub fn level_curve_inv(f: f32) -> f32 {
    (1.0 / (1.0 - f) - 1.0) * 20.0 + 1.0
}

/// `spawnId(zx, zy, index)`, `Server.exe 0x004f3850`: `((zy << 16) + zx) << 8 + index` with the
/// 32-bit sum zero-extended before the shift.
#[inline]
pub fn spawn_id(zx: i32, zy: i32, index: i32) -> i64 {
    (i64::from(zy.wrapping_mul(0x10000).wrapping_add(zx) as u32) << 8).wrapping_add(i64::from(index))
}

/// True when `(x, y)` in blocks lies within `sqrt(limit)` blocks of a placed feature. The
/// original computes the y term as `((dy * k) * dy) * k` and the x term as `(dx * k)^2`.
pub(crate) fn near_feature(features: &[(i64, i64, i64)], x: i32, y: i32, limit: f32) -> bool {
    const K: f32 = 1.5258789e-05;
    let fx = i64::from(x) << 16;
    let fy = i64::from(y) << 16;
    features.iter().any(|&(px, py, _)| {
        let dx = (fx - px) as f32;
        let dy = (fy - py) as f32;
        let a = dx * K;
        a * a + dy * K * dy * K < limit
    })
}

/// The points of `generateZone` at which the oracle snapshots the original, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// After the terrain pass (climate, heights, surface, valleys and lakes).
    Terrain,
    /// After the boulder and rock placements that depend on the cell kind.
    Features,
    /// After the ground-layer pass.
    Layers,
    /// After the terrace pass, before the dungeon calls (the oracle's `dungeon_in` bracket).
    Terraces,
    /// After the terrace pass and the dungeon calls.
    Dungeons,
    /// After the terrace blobs.
    Decor1,
    /// After the mark blobs, the camp, the small blobs, the settlement and the kind-4 pillars.
    Trees,
    /// After the tree grid, the spawn-point static, populateZoneCreatures and the kind-9 boss.
    Creatures,
    /// After the creature spawn grid.
    Decor2,
    /// After the per-column decorations and the spawn ids, before the creature initialisation.
    Appearance,
    /// The finished zone.
    Full,
}

impl Stage {
    /// The oracle's `--stage` name.
    pub fn name(self) -> &'static str {
        match self {
            Stage::Terrain => "terrain",
            Stage::Features => "features",
            Stage::Layers => "layers",
            Stage::Terraces => "dungeon_in",
            Stage::Dungeons => "dungeons",
            Stage::Decor1 => "decor1",
            Stage::Trees => "trees",
            Stage::Creatures => "creatures",
            Stage::Decor2 => "decor2",
            Stage::Appearance => "appearance",
            Stage::Full => "full",
        }
    }

    pub fn from_name(name: &str) -> Option<Stage> {
        [Stage::Terrain, Stage::Features, Stage::Layers, Stage::Terraces, Stage::Dungeons, Stage::Decor1, Stage::Trees, Stage::Creatures, Stage::Decor2, Stage::Appearance, Stage::Full].into_iter().find(|s| s.name() == name)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
