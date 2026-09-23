//! Terrain base height and the factors feeding it.
//!
//! `cube::World::baseHeight` (`Server.exe 0x004f9b70`) is sampled at every column corner of a
//! zone and at cell centres during region creation. Its inputs are the sub-seeds, the climate
//! points (elevation blend), the distance to the "edges" between neighbouring climate points
//! (rivers and roads), and the cell the position falls in.

use cw_math::value_noise_2d as noise;

use crate::climate::div_trunc;
use crate::world::World;

/// `FUN_00523380(a, b, q)`: squared distance from `q` to the segment `a..b`, in doubles,
/// rounded to `f32` on return.
fn segment_dist_sq(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f32 {
    let dqx = q[0] - a[0];
    let dvx = b[0] - a[0];
    let dvy = b[1] - a[1];
    let dqy = q[1] - a[1];
    let len2 = dvx * dvx + dvy * dvy;
    if len2 < 9.999999682655225e-21 {
        return (dqy * dqy + dqx * dqx) as f32;
    }
    let t = (dqy * dvy + dqx * dvx) / len2;
    if t <= 0.0 {
        return (dqy * dqy + dqx * dqx) as f32;
    }
    if t >= 1.0 {
        let ey = q[1] - b[1];
        let ex = q[0] - b[0];
        return (ey * ey + ex * ex) as f32;
    }
    let ex = dqx - dvx * t;
    let ey = dqy - dvy * t;
    (ey * ey + ex * ex) as f32
}

impl World {
    /// `nearestClimateRegion(bx, by)`, `Server.exe 0x004febd0`: region coordinates of the
    /// climate point nearest to the warped query, or `(-1, -1)`.
    pub fn nearest_climate_region(&self, bx: i32, by: i32) -> (i32, i32) {
        let (qx, qy) = Self::warped_query(bx, by);
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let mut best = (-1, -1);
        let mut found = false;
        let mut best_d = 0i32;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let Some(p) = self.climate_point(rx, ry) else { continue };
                let d = Self::point_dist_sq(p, qx, qy) as i32;
                if !found || d < best_d {
                    best = (rx, ry);
                    best_d = d;
                    found = true;
                }
            }
        }
        best
    }

    /// `climateEdgeDistance(bx, by)`, `Server.exe 0x00522840`: scaled squared distance from the
    /// (differently warped) query to the nearest segment joining the nearest climate point
    /// with its four axis neighbours; 1.0 when there is no such segment.
    pub fn climate_edge_distance(&self, bx: i32, by: i32) -> f32 {
        let (rx, ry) = self.nearest_climate_region(bx, by);
        if !(0..1024).contains(&rx) || !(0..1024).contains(&ry) {
            return 1.0;
        }
        let Some(c) = self.climate_point(rx, ry) else { return 1.0 };
        let s = &self.seeds;
        let y01 = f64::from(by) * 0.01;
        let y0005 = f64::from(by) * 0.0005;
        let x01 = f64::from(bx) * 0.01;
        let x0005 = f64::from(bx) * 0.0005;
        let n = noise(s.f(0x800204) + x01, s.f(0x800208) + y01);
        let w0 = f64::from(n) * 0.1;
        let n = noise(s.f(0x8001fc) + x0005, s.f(0x800200) + y0005);
        let qx = (w0 + f64::from(n)) * 500.0 + f64::from(bx);
        let n = noise(s.f(0x800214) + x01, s.f(0x800218) + y01);
        let w1 = f64::from(n) * 0.1;
        let n = noise(s.f(0x80020c) + x0005, s.f(0x800210) + y0005);
        let qy = (w1 + f64::from(n)) * 500.0 + f64::from(by);
        let q = [qx, qy];
        let a = [f64::from(c.x), f64::from(c.y)];
        let mut best = -1.0f32;
        for (nx, ny) in [(rx - 1, ry), (rx + 1, ry), (rx, ry - 1), (rx, ry + 1)] {
            let Some(p) = self.climate_point(nx, ny) else { continue };
            let d = segment_dist_sq(a, [f64::from(p.x), f64::from(p.y)], q);
            if best < 0.0 || d < best {
                best = d;
            }
        }
        if best >= 0.0 { best * 0.0002 } else { 1.0 }
    }

    /// `cube::Cell::falloff` through `FUN_004d19f0(bx, by)`: `(1 - d)^2` inside a type-1 (home)
    /// cell, 0 elsewhere.
    pub fn cell_falloff(&self, bx: i32, by: i32) -> f32 {
        match self.cell_at_block(bx, by) {
            Some(cell) if cell.kind == 1 => {
                let f = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                if f <= 0.0 { 0.0 } else { f * f }
            }
            _ => 0.0,
        }
    }

    /// `heightFactorA(bx, by, zone)`, `Server.exe 0x0052cd50`.
    pub fn height_factor_a(&self, bx: i32, by: i32) -> f32 {
        let s = &self.seeds;
        let y001 = f64::from(by) * 0.001;
        let x001 = f64::from(bx) * 0.001;
        let n1 = noise(s.f(0x800170) + f64::from(bx) * 0.01, s.f(0x800174) + f64::from(by) * 0.01);
        let n2 = noise(s.f(0x800168) + x001, s.f(0x80016c) + y001);
        let v = n1 * 0.1 + n2;
        let n3 = noise(x001, y001);
        let v = v.abs() * ((n3 + 1.0) * 0.1 + 0.8);
        let e = self.climate_edge_distance(bx, by);
        let k = 1.0 - e * 0.75;
        let mut out = v;
        if k > 0.0 {
            out = k * k * 0.05 + v;
        }
        if let Some(cell) = self.cell_at_block(bx, by) {
            let t = cell.kind;
            if t == 1 || t == 2 || t == 4 || t == 0xd {
                let f = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                let f = if f > 0.0 { f * f } else { 0.0 };
                out += f;
            }
            if t == 6 || t == 7 {
                let f = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                let f = if f > 0.0 { f * f } else { 0.0 };
                out += f * 0.5;
            }
        }
        self.column_climate_c(bx, by) + out
    }

    /// `heightFactorB(bx, by)`, `Server.exe 0x0052d990`.
    pub fn height_factor_b(&self, bx: i32, by: i32) -> f32 {
        let f = self.cell_falloff(bx, by);
        let w = crate::warp::warp_position(&self.seeds, bx, by);
        let mut out = self.climate_edge_distance(bx, by);
        if f > 0.0 {
            let mut k = f * 3.0;
            if k > 1.0 {
                k = 1.0;
            }
            let k = 1.0 - k * k;
            let k2 = 1.0 - k * k;
            let c = (cw_math::cos(w[0] * 360.0) * f64::from(k2) + 1.0) as f32;
            if c < out {
                out = c;
            }
            let c = (cw_math::cos(w[1] * 360.0) * f64::from(k2) + 1.0) as f32;
            if c < out {
                out = c;
            }
        }
        if f > 0.65 {
            let mut k = (0.7 - f) / 0.05;
            if k <= 0.0 {
                k = 0.0;
            }
            out *= k;
        }
        if let Some(cell) = self.cell_at_block(bx, by)
            && (cell.kind == 2 || cell.kind == 4)
        {
            let g = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
            let g = if g > 0.0 { g * g } else { 0.0 };
            out += g * 2.0;
        }
        out
    }

    /// `cube::World::baseHeight(bx, by, zone)`, `Server.exe 0x004f9b70`.
    #[allow(clippy::collapsible_if)] // nested null / kind checks as in the original
    ///
    /// Performance note (release, 2026-09, `examples/gen_micro.rs`): about 1.3 µs per call,
    /// and generateZone evaluates it on the 257x257 corner grid, so roughly 90 ms of a zone's
    /// ~250 ms terrain pass. The cost is nine noise samples, the two 3x3 climate-point scans,
    /// and `height_factor_a` with its `climate_edge_distance` warp. It is a pure function of
    /// the block position: rows could be evaluated on a thread pool, and the 3x3 scan shared
    /// with the climate functions, without changing a bit. Left as the original computes it:
    /// parity first.
    pub fn base_height(&self, bx: i32, by: i32) -> f32 {
        let s = &self.seeds;
        let xd = f64::from(bx);
        let yd = f64::from(by);
        let x0001 = xd * 0.0001;
        let y0001 = yd * 0.0001;
        let a = (noise(s.f(0x80018c) + x0001, s.f(0x800190) + y0001) + 1.0) * 0.5;
        let b = (noise(s.f(0x800194) + x0001, s.f(0x800198) + y0001) + 1.0) * 0.5;
        let x001 = xd * 0.001;
        let y001 = yd * 0.001;
        let c = (noise(s.f(0x80019c) + x001, s.f(0x8001a0) + y001) + 1.0) * 0.5;
        let d = (noise(s.f(0x8001a4) + x001, s.f(0x8001a8) + y001) + 1.0) * 0.5;
        let x002 = xd * 0.002;
        let y002 = yd * 0.002;
        let e = (noise(s.f(0x8001ac) + x002, s.f(0x8001b0) + y002) + 1.0) * 0.5;
        let a = a * a;
        let e = e * e;
        let b = b * b;
        let c = c * c;
        let d = d * d;

        let mut f = self.height_factor_a(bx, by) * 4.0;
        if f > 1.0 {
            f = 1.0;
        }
        let sm = f * 3.0 * f - f * 2.0 * f * f;
        let sm = sm * sm;
        let mut e4 = sm * c;
        let mut l174 = sm * d;
        let mut sm = sm * e;

        let rx1 = div_trunc(bx + 0x4000, 0x4000);
        let ry1 = div_trunc(by + 0x4000, 0x4000);
        let rx0 = div_trunc(bx - 0x4000, 0x4000);
        let ry0 = div_trunc(by - 0x4000, 0x4000);
        // Here the warp stays in 16.16 fixed point instead of being truncated to a block.
        let wx = noise(yd * 0.0005, 3423.0) * 3.0 * 256.0;
        let wy = noise(xd * 0.0005, 23421.0) * 3.0 * 256.0;
        let qx64 = (wx * 65536.0) as i64 + (i64::from(bx) << 16);
        let qy64 = (wy * 65536.0) as i64 + (i64::from(by) << 16);
        let dist = |px: i32, py: i32| -> f32 {
            let dx = ((i64::from(px) << 16) - qx64) as f32 * 1.5258789e-05;
            let dy = ((i64::from(py) << 16) - qy64) as f32 * 1.5258789e-05;
            dy * dy + dx * dx
        };
        let mut best: Option<f32> = None;
        for rx in rx0..=rx1 {
            for ry in ry0..=ry1 {
                let Some(p) = self.climate_point(rx, ry) else { continue };
                let dd = dist(p.x, p.y);
                if best.is_none_or(|bd| dd < bd) {
                    best = Some(dd);
                }
            }
        }
        let mut elev_avg = 0.0f32;
        let mut pos_frac = 1.0f32;
        let mut floor_h = 0.0f32;
        if let Some(best_d) = best {
            let mut sum_w = 0.0f32;
            let mut sum_e = 0.0f32;
            let mut sum_pos = 0.0f32;
            for rx in rx0..=rx1 {
                for ry in ry0..=ry1 {
                    let Some(p) = self.climate_point(rx, ry) else { continue };
                    let mut w = (dist(p.x, p.y) - best_d) * 5e-08;
                    if w > 1.0 {
                        w = 1.0;
                    }
                    let w = (1.0 - w) * (1.0 - w);
                    sum_w += w;
                    sum_e += p.elevation as f32 * w;
                    if p.elevation > 0 {
                        sum_pos += w;
                    }
                }
            }
            elev_avg = sum_e / sum_w;
            pos_frac = sum_pos / sum_w;
            floor_h = elev_avg;
        }

        let x0002 = xd * 0.0002;
        let y0002 = yd * 0.0002;
        let h = (noise(s.f(0x8001bc) + x0002, s.f(0x8001c0) + y0002) + 1.0) * 100.0 * b;
        let h = (h + (noise(s.f(0x8001b4) + x0002, s.f(0x8001b8) + y0002) + 1.0) * 100.0 * a) * pos_frac + elev_avg;

        let fo = self.cell_falloff(bx, by);
        if fo > 0.5 {
            let mut k = (fo - 0.5) * 2.0;
            if k > 1.0 {
                k = 1.0;
            }
            sm *= 1.0 - (k * 3.0 * k - k * 2.0 * k * k);
        }
        let hb = self.height_factor_b(bx, by);
        let mut k = hb;
        if k < 0.02 {
            k = 0.02;
        }
        let mut k = k * 2.0;
        if k > 1.0 {
            k = 1.0;
        }
        let k = 1.0 - k;
        let m = 1.0 - k * k * k * k;
        let m1 = m * 0.1 + 0.9;
        sm *= m * 0.5 + 0.5;
        e4 *= m1;
        l174 *= m1;

        let cell = self.cell_at_block(bx, by);
        if let Some(cell) = cell {
            if cell.kind == 1 {
                let g = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                let g = if g > 0.0 { g * g } else { 0.0 };
                let g = 1.0 - g * 0.5;
                e4 *= g;
                l174 *= g;
                sm *= g;
            }
        }

        let mut hh = (noise(s.f(0x8001cc) + x002, s.f(0x8001d0) + y002) + 1.0) * 50.0 * l174;
        hh += (noise(s.f(0x8001c4) + x002, s.f(0x8001c8) + y002) + 1.0) * 50.0 * e4;
        hh = hh + (noise(s.f(0x8001d4) + xd * 0.01, s.f(0x8001d8) + yd * 0.01) + 1.0) * 20.0 * sm + h;
        let mut result = hh;

        if let Some(cell) = cell {
            if cell.kind != 0 {
                let cxb = (cell.x / 65536) as i32;
                let cyb = (cell.y / 65536) as i32;
                if let Some(np) = self.nearest_climate_point(cxb, cyb) {
                    if np.elevation < 0 && cell.kind != 0xb {
                        let x0025 = xd * 0.0025;
                        let y0025 = yd * 0.0025;
                        let n = noise(x0025 + 8432984.0, y0025 + 90493.0);
                        let t = (n * 100.0 * 65536.0) as i64;
                        let dxf = (t - cell.x + (i64::from(bx) << 16)) as f32 * 1.5258789e-05;
                        let n = noise(x0025 + 3423.0, y0025 + 112.0);
                        let t = ((n * 100.0 + by as f32) * 65536.0) as i64;
                        let dyf = (t - cell.y) as f32;
                        let mut f = (dyf * 1.5258789e-05 * dyf * 1.5258789e-05 + dxf * dxf)
                            / ((cell.radius + 256.0) * (cell.radius + 256.0));
                        if f > 1.0 {
                            f = 1.0;
                        }
                        if 1.0 - f > 0.0 {
                            let mut g = (1.0 - f) * 1.1;
                            if g > 1.0 {
                                g = 1.0;
                            }
                            hh -= (g * 3.0 * g - g * 2.0 * g * g) * np.elevation as f32;
                            result = hh;
                        }
                    }
                }
                let t = cell.kind;
                if t == 4 {
                    let nd = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                    if nd > 0.25 {
                        if nd < 1.0 {
                            let q = ((f64::from(nd).sqrt() as f32) - 0.5) * 2.0;
                            let q = 1.0 - q * q;
                            hh = ((cell.height - 25.0) - hh) * q * q + hh;
                            result = hh;
                        }
                    } else {
                        let q = 1.0 - nd * 4.0;
                        let q = q * q;
                        hh = (1.0 - q) * (cell.height - 25.0) + (cell.height - 50.0) * q;
                        result = hh;
                    }
                }
                if t == 6 || t == 7 {
                    let nd = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                    if nd > 0.25 {
                        if nd < 1.0 {
                            let q = ((f64::from(nd).sqrt() as f32) - 0.5) * 2.0;
                            let q = 1.0 - q * q;
                            hh += q * q * 10.0;
                            result = hh;
                        }
                    } else {
                        let q = 1.0 - nd * 4.0;
                        let q = q * q;
                        hh = (1.0 - q) * (hh + 10.0) + (hh - 30.0) * q;
                        result = hh;
                    }
                    if hh < floor_h {
                        hh = floor_h;
                        result = hh;
                    }
                }
                if t == 0xd {
                    let nd = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                    if nd > 0.010000001 {
                        if nd < 1.0 {
                            let q = ((f64::from(nd).sqrt() as f32) - 0.1) / 0.9;
                            let q = 1.0 - q * q;
                            result = q * q * q * q * 150.0 + hh;
                        }
                    } else {
                        result = hh + 150.0;
                    }
                }
            }
        }
        result
    }
}
