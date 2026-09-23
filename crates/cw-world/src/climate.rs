//! Per-region climate points and the three blends sampled at every block column.
//!
//! Every region owns one climate point (`world+0x4000bc` table in the original). A point
//! is created on demand by `cube::World::getOrCreateClimatePoint` (`Server.exe 0x0050b870`)
//! and never changes. The blends `climateA/B/C` (0x004f8570, 0x004f8b40, 0x00522e20) look
//! at the 3x3 regions around a block, warp the query position with noise, pick the nearest
//! point and average the points' values with distance weights.

use cw_math::value_noise_2d as noise;

use crate::world::World;

/// One region's climate point, 0x1c bytes in the original.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClimatePoint {
    /// Block position, snapped to a 2048 grid plus 1024.
    pub x: i32,
    pub y: i32,
    /// `+8`: 1 for the rare "special" climate (10% of the mid-range points).
    pub flag: u8,
    /// `[3]`: first climate value in `0.0..=1.0`.
    pub a: f32,
    /// `[4]`: second climate value in `0.0..=1.0`.
    pub b: f32,
    /// `[5]`: `ry * 1024 + rx + seed188`.
    pub seed: i32,
    /// `[6]`: elevation in blocks, `-100..` (negative means water).
    pub elevation: i32,
}

/// Truncating division, the `(v + (v >> 31 & (d-1))) >> log2(d)` idiom of the original.
#[inline]
pub(crate) fn div_trunc(v: i32, d: i32) -> i32 {
    v / d
}

/// `(float)r * scale / 32767.0 + offset` in single precision, the original's dice-to-float idiom.
#[inline]
fn unit(r: i32, scale: f32, offset: f32) -> f32 {
    (r as f32 * scale) / 32767.0 + offset
}

impl World {
    /// `cube::World::getOrCreateClimatePoint(rx, ry)`, `Server.exe 0x0050b870`.
    ///
    /// Returns `None` outside the 1024x1024 region grid. Reseeds the generator's `rand`
    /// stream, so the caller's stream position is lost, exactly as in the original.
    pub fn get_or_create_climate_point(&mut self, rx: i32, ry: i32) -> Option<ClimatePoint> {
        if !(0..1024).contains(&rx) || !(0..1024).contains(&ry) {
            return None;
        }
        let key = (rx as u32) * 1024 + ry as u32;
        if let Some(p) = self.points.get(&key) {
            return Some(*p);
        }
        let seed188 = self.seeds.at(0x800188);
        self.rng.seed(
            rx.wrapping_add(0x108a)
                .wrapping_add(ry.wrapping_mul(0x400))
                .wrapping_add(seed188.wrapping_mul(3)) as u32,
        );
        let mut p = ClimatePoint {
            x: 0,
            y: 0,
            flag: 0,
            a: 0.0,
            b: 0.0,
            seed: ry.wrapping_mul(0x400).wrapping_add(rx).wrapping_add(seed188),
            elevation: 0,
        };
        let sx = self.spawn[0] as i32;
        let sy = self.spawn[1] as i32;
        let is_spawn = rx == div_trunc(div_trunc(sx, 256), 64) && ry == div_trunc(div_trunc(sy, 256), 64);

        let mut x;
        let mut y;
        let rng = &mut self.rng;
        let r = rng.rand() % 2;
        let snap_from_spawn;
        if r == 0 || is_spawn {
            p.a = unit(rng.rand(), 0.4, 0.3);
            // This one line the compiler evaluated in double (cvtdq2pd/mulsd/divsd/addsd,
            // Server.exe 0x0050ba4f) with the offset 0.4 stored as a float widened to double,
            // before rounding to float.
            p.b = ((rng.rand() as f64 * 0.4) / 32767.0 + f64::from(0.4f32)) as f32;
            if is_spawn {
                x = sx;
                y = sy;
                snap_from_spawn = true;
            } else {
                snap_from_spawn = false;
                if rng.rand() % 10 == 0 {
                    p.flag = 1;
                    p.b = unit(rng.rand(), 0.2, 0.8);
                    p.a = unit(rng.rand(), 0.5, 0.0);
                }
                x = 0;
                y = 0;
            }
        } else {
            snap_from_spawn = false;
            p.a = if rng.rand() % 2 == 0 { unit(rng.rand(), 0.1, 0.0) } else { unit(rng.rand(), 0.1, 0.9) };
            p.b = if rng.rand() % 2 == 0 { unit(rng.rand(), 0.1, 0.0) } else { unit(rng.rand(), 0.1, 0.9) };
            x = 0;
            y = 0;
        }
        if !snap_from_spawn {
            let a = rng.rand();
            let b = rng.rand();
            y = ry * 0x4000 + a % 0x3c00 + 0x200;
            x = rx * 0x4000 + 0x200 + b % 0x3c00;
        }
        // Snap to the 2048 grid (truncating) plus 1024.
        p.x = (x - x % 0x800) + 0x400;
        p.y = (y - y % 0x800) + 0x400;

        let d12 = f64::from(seed188);
        let n1 = noise(f64::from(rx) * 1.4 + d12, f64::from(ry) * 1.4 + d12 + 843.0);
        let n2 = noise(f64::from(rx) * 4.0 + d12, f64::from(ry) * 4.0 + d12 + 843.0);
        let elev = (((n1 + 1.0) * 100.0 - 70.0) + n2 * 30.0) as i32;
        p.elevation = elev;
        if elev < 1 {
            p.flag = 0;
            if is_spawn {
                p.elevation = rng.rand() % 0x32 + 0x14;
                self.points.insert(key, p);
                return Some(p);
            }
            p.elevation = (elev - 100).max(-100);
            if rng.rand() % 2 == 0 {
                p.a = unit(rng.rand(), 0.1, 0.9);
                p.b = unit(rng.rand(), 0.1, 0.9);
            } else {
                p.a = unit(rng.rand(), 0.4, 0.3);
                // Double again, with the widened 0.4 (Server.exe 0x0050bcc2).
                p.b = ((rng.rand() as f64 * 0.4) / 32767.0 + f64::from(0.4f32)) as f32;
            }
        }
        self.points.insert(key, p);
        Some(p)
    }

    /// The climate point of region `(rx, ry)` if it has been created.
    #[inline]
    pub fn climate_point(&self, rx: i32, ry: i32) -> Option<&ClimatePoint> {
        if !(0..1024).contains(&rx) || !(0..1024).contains(&ry) {
            return None;
        }
        self.points.get(&((rx as u32) * 1024 + ry as u32))
    }

    /// The noise-warped query position the climate lookups use (`Server.exe 0x00522d80` plus
    /// the callers' `(int)((float)b + warp)`): x is displaced by a function of y and vice versa.
    #[inline]
    pub(crate) fn warped_query(bx: i32, by: i32) -> (i32, i32) {
        let wx = noise(f64::from(by) * 0.0005, 3423.0) * 3.0 * 256.0;
        let wy = noise(f64::from(bx) * 0.0005, 23421.0) * 3.0 * 256.0;
        ((bx as f32 + wx) as i32, (by as f32 + wy) as i32)
    }

    /// Squared block distance between a point and a query, computed the original's way:
    /// 16.16 fixed subtraction, `int64 -> float`, times 2^-16, `y*y + x*x`.
    #[inline]
    pub(crate) fn point_dist_sq(p: &ClimatePoint, qx: i32, qy: i32) -> f32 {
        let dx = ((i64::from(p.x) << 16) - (i64::from(qx) << 16)) as f32 * 1.5258789e-05;
        let dy = ((i64::from(p.y) << 16) - (i64::from(qy) << 16)) as f32 * 1.5258789e-05;
        dy * dy + dx * dx
    }

    /// `cube::World::nearestClimatePoint(bx, by)`, `Server.exe 0x0042e090`: the point of the
    /// 3x3 regions around the block that is nearest to the warped query (integer distances,
    /// first wins ties, missing points are skipped).
    pub fn nearest_climate_point(&self, bx: i32, by: i32) -> Option<&ClimatePoint> {
        let (qx, qy) = Self::warped_query(bx, by);
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let mut best: Option<&ClimatePoint> = None;
        let mut best_d = 0i32;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let Some(p) = self.climate_point(rx, ry) else { continue };
                let d = Self::point_dist_sq(p, qx, qy) as i32;
                if best.is_none() || d < best_d {
                    best = Some(p);
                    best_d = d;
                }
            }
        }
        best
    }

    /// Shared body of the three blends. Returns `None` when a point of the 3x3 is missing,
    /// otherwise `(sum of weights, sum of weight * value(point))`.
    ///
    /// Performance note (release, 2026-09): the warped 3x3 scan of the climate points runs
    /// here for `climate_a`, `climate_b` and `climate_c` of every column (about 190 ns each),
    /// and again inside `base_height`, `height_factor_a` and `mountain_factor`. The nearest
    /// point and the weights depend only on the block position, so one scan per column could
    /// serve all of them. Kept as the original does it: parity first.
    fn climate_blend(&self, bx: i32, by: i32, value: impl Fn(&ClimatePoint) -> f32) -> Option<(f32, f32)> {
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let (qx, qy) = Self::warped_query(bx, by);
        let mut best: Option<&ClimatePoint> = None;
        let mut best_d = 0i32;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let p = self.climate_point(rx, ry)?;
                let d = Self::point_dist_sq(p, qx, qy) as i32;
                if best.is_none() || d < best_d {
                    best = Some(p);
                    best_d = d;
                }
            }
        }
        best?;
        let mut sum_w = 0.0f32;
        let mut sum_v = 0.0f32;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let p = self.climate_point(rx, ry)?;
                let d = Self::point_dist_sq(p, qx, qy) as i32;
                let mut w = (d - best_d) as f32 * 5e-07;
                if w > 1.0 {
                    w = 1.0;
                }
                sum_w += 1.0 - w;
                sum_v += value(p) * (1.0 - w);
            }
        }
        Some((sum_w, sum_v))
    }

    /// `climateA(bx, by)`, `Server.exe 0x004f8570`: blend of the points' `a` values, plus a
    /// bonus near the centre of a type-3 cell. 0.5 when a neighbouring point is missing.
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // `!(w > 0)` also catches NaN, as in the original
    pub fn climate_a(&self, bx: i32, by: i32) -> f32 {
        let Some((sum_w, sum_v)) = self.climate_blend(bx, by, |p| p.a) else { return 0.5 };
        if !(sum_w > 0.0) {
            return 0.5;
        }
        let v = sum_v / sum_w;
        if v < 0.2
            && let Some(cell) = self.cell_at_block(bx, by)
            && cell.kind == 3
        {
            let k = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
            let k = if k > 0.0 { k * k } else { 0.0 };
            let out = k * 0.3 + v;
            return if out > 1.0 { 1.0 } else { out };
        }
        v
    }

    /// `climateB(bx, by)`, `Server.exe 0x004f8b40`: blend of the points' `b` values.
    pub fn climate_b(&self, bx: i32, by: i32) -> f32 {
        let Some((sum_w, sum_v)) = self.climate_blend(bx, by, |p| p.b) else { return 0.5 };
        if sum_w > 0.0 { sum_v / sum_w } else { 0.5 }
    }

    /// `climateC(bx, by)`, `Server.exe 0x00522e20`: weight fraction of flagged points, or 0
    /// when no point of the 3x3 is flagged or one is missing.
    pub fn climate_c(&self, bx: i32, by: i32) -> f32 {
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let mut any = false;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let Some(p) = self.climate_point(rx, ry) else { return 0.0 };
                if p.flag == 1 {
                    any = true;
                }
            }
        }
        if !any {
            return 0.0;
        }
        let Some((sum_w, sum_v)) = self.climate_blend(bx, by, |p| if p.flag == 1 { 1.0 } else { 0.0 }) else {
            return 0.0;
        };
        if sum_w > 0.0 { sum_v / sum_w } else { 0.0 }
    }

    /// `Cube.exe 0x005f0720` (client only; its one caller is the sky colour code 0x0059d640
    /// at 0x0059da51): the water fraction of the climate blend. A byte-for-byte clone of
    /// `climateC` (Cube 0x005ef040, 1376 bytes) with the predicate `elevation < 0` (point
    /// field `[6]`, +0x18) instead of `flag == 1`: 0 when a point of the 3x3 is missing or
    /// none is water, else the blend's weight fraction of water points. (The original leaves
    /// the pre-scan row at the first hit; the result is the same.) No golden: Server.exe has
    /// no copy and the oracle only calls Server.exe; the shared parts are covered through
    /// `climateC`.
    pub fn climate_water(&self, bx: i32, by: i32) -> f32 {
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let mut any = false;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let Some(p) = self.climate_point(rx, ry) else { return 0.0 };
                if p.elevation < 0 {
                    any = true;
                }
            }
        }
        if !any {
            return 0.0;
        }
        let Some((sum_w, sum_v)) = self.climate_blend(bx, by, |p| if p.elevation < 0 { 1.0 } else { 0.0 }) else {
            return 0.0;
        };
        if sum_w > 0.0 { sum_v / sum_w } else { 0.0 }
    }
}
