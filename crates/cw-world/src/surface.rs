//! Surface colouring and shaping helpers sampled per column by the terrain pass.

use cw_math::value_noise_2d as noise;

use crate::color::{color_a, color_b, color_c, color_d};
use crate::world::World;

/// A voxel: red, green, blue and `type | flags`. Type is the low 5 bits; 0x20, 0x40 and 0x80
/// are flags (0x80 marks a block a later pass wrote and protects it from plain overwrites).
pub type Block = [u8; 4];

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

impl World {
    /// `climateB` as the terrain functions read it: from the column when the position has a
    /// generated column, otherwise computed.
    fn column_climate_b(&self, bx: i32, by: i32) -> f32 {
        match self.column(bx, by) {
            Some(c) => c.climate_b,
            None => self.climate_b(bx, by),
        }
    }

    pub(crate) fn column_climate_c(&self, bx: i32, by: i32) -> f32 {
        match self.column(bx, by) {
            Some(c) => c.climate_c,
            None => self.climate_c(bx, by),
        }
    }

    /// `cube::World::terrainColor(out, bx, by, h, zone)`, `Server.exe 0x0052d030`.
    pub fn terrain_color(&self, bx: i32, by: i32, h: i32) -> [f32; 3] {
        let s = &self.seeds;
        let n1 = noise(f64::from(by) * 0.1 + 98984.0, f64::from(h) * 0.4);
        let n2 = noise(f64::from(bx) * 0.1, f64::from(h) * 0.4);
        let clim_b = self.column_climate_b(bx, by);
        let (mut r, mut g, mut b) = (120.0f32, 120.0f32, 130.0f32);
        if clim_b >= 0.5 {
            if clim_b > 0.75 {
                let t = (clim_b - 0.75) * 4.0;
                let n3 = noise(f64::from(t) * 0.001 + 6544.0, f64::from(by) * 0.001 + 123.0);
                let g0 = (1.0 - t) * 120.0;
                b = (1.0 - t) * 130.0 + t * 100.0;
                r = g0 + t * 200.0;
                g = g0 + (n3 * 20.0 + 130.0) * t;
            }
        } else {
            let t = clim_b * 2.0;
            let u = 1.0 - t;
            r = u * 10.0 + t * 160.0;
            g = u * 20.0 + t * 160.0;
            b = u * 50.0 + t * 170.0;
        }
        let n4 = noise(s.f(0x80022c) + f64::from(bx) * 0.005, s.f(0x800230) + f64::from(by) * 0.005);
        let k = ((n1 + n2) * 0.5 + 1.0) * 0.5 * 160.0;
        let rr = k + r + 0.0;
        let gg = k + g + 0.0;
        let bb = k + b + (n4 + 1.0) * 10.0;
        let mut out = [rr, gg, bb];
        if rr >= 0.0 {
            if rr > 255.0 {
                out[0] = 255.0;
            }
        } else {
            out[0] = 0.0;
        }
        if gg < 0.0 {
            out[1] = 0.0;
        }
        if out[1] > 255.0 {
            out[1] = 255.0;
        }
        if bb < 0.0 {
            out[2] = 0.0;
        }
        if out[2] > 255.0 {
            out[2] = 255.0;
        }
        let clim_c = self.column_climate_c(bx, by);
        if clim_c > 0.0 {
            let n5 = noise(f64::from(bx) * 0.05, f64::from(by) * 0.05);
            let n6 = noise(f64::from(bx) * 0.02, f64::from(by) * 0.02);
            let mut m = n5 * 0.1 + n6;
            if m > 1.0 {
                m = 1.0;
            }
            let g0 = out[1];
            let b0 = out[2];
            let p = 1.0 - m.abs();
            let q = 1.0 - clim_c;
            let p = p * p * p * p * p;
            let r0 = out[0];
            let n7 = noise(f64::from(bx) * 0.1, f64::from(by) * 0.1);
            out[0] = r0 * q + (p * 200.0 + 10.0) * clim_c;
            out[1] = g0 * q + ((n7 + 1.0) * p * 50.0 + 10.0) * clim_c;
            out[2] = b0 * q + clim_c * 10.0;
        }
        out
    }

    /// `cube::World::rockColor(out, bx, by, h, zone)`, `Server.exe 0x004fae90` (name tentative).
    pub fn rock_color(&self, bx: i32, by: i32, h: i32) -> [f32; 3] {
        let s = &self.seeds;
        let n1 = noise(f64::from(by) * 0.1 + 98984.0, f64::from(h) * 0.4 + 8437.0);
        let n2 = noise(f64::from(bx) * 0.1, f64::from(h) * 0.4);
        let xd = f64::from(bx) * 0.01;
        let yd = f64::from(by) * 0.01;
        let k = ((n1 + n2) * 0.5 + 1.0) * 0.5;
        let n3 = noise(s.f(0x80025c) + xd, s.f(0x800260) + yd);
        let n4 = noise(s.f(0x800264) + xd, s.f(0x800268) + yd);
        let n5 = noise(s.f(0x80026c) + xd, s.f(0x800270) + yd);
        let k60 = k * 60.0;
        let n6 = noise(s.f(0x80024c) + xd, s.f(0x800250) + yd);
        let n7 = noise(s.f(0x800254) + xd, s.f(0x800258) + yd);
        // The constants are added to the noise terms first (Server.exe 0x004fb0c2, 0x004fb11b).
        let bb = k * 0.0 + 60.0 + n5 * 20.0;
        let rr = k60 + (n6 * 20.0 + 180.0) + n3 * 20.0;
        let gg = k60 + (n7 * 20.0 + 100.0) + n4 * 20.0;
        let mut out = [rr, gg, bb];
        if rr >= 0.0 {
            if rr > 255.0 {
                out[0] = 255.0;
            }
        } else {
            out[0] = 0.0;
        }
        if gg < 0.0 {
            out[1] = 0.0;
        }
        if out[1] > 255.0 {
            out[1] = 255.0;
        }
        if bb < 0.0 {
            out[2] = 0.0;
        }
        if out[2] > 255.0 {
            out[2] = 255.0;
        }
        let mut clim_b = self.column_climate_b(bx, by);
        if clim_b > 0.75 {
            let t = (clim_b - 0.75) * 4.0;
            if t < 0.0 {
                clim_b = 0.0;
            }
            let (r0, g0, b0) = (out[0], out[1], out[2]);
            let f = 1.0 - t;
            let tc = self.terrain_color(bx, by, h);
            out[0] = tc[0] * t * 0.9 + r0 * f;
            out[1] = tc[1] * t * 0.9 + g0 * f;
            out[2] = tc[2] * t * 0.9 + b0 * f;
        }
        if clim_b < 0.2 {
            let g0 = out[1];
            let b0 = out[2];
            let t = 1.0 - clim_b * 4.0;
            let r0 = out[0];
            let f = 1.0 - t;
            let tc = self.terrain_color(bx, by, h);
            let cd = color_d(bx, by);
            let t = t * 0.5;
            out[0] = (cd[0] + tc[0]) * t + r0 * f;
            out[1] = (tc[1] + cd[1]) * t + g0 * f;
            out[2] = (tc[2] + cd[2]) * t + b0 * f;
        }
        let clim_c = self.column_climate_c(bx, by);
        if clim_c > 0.0 {
            let r0 = out[0];
            let g0 = out[1];
            let f = 1.0 - clim_c;
            let b0 = out[2];
            let tc = self.terrain_color(bx, by, h);
            out[0] = r0 * f + tc[0] * clim_c;
            out[2] = tc[2] * clim_c + f * b0;
            out[1] = g0 * f + tc[1] * clim_c;
        }
        out
    }

    /// `cube::World::surfaceBlock(out, bx, by, h, climA, climB, zone)`, `Server.exe 0x004f9450`:
    /// the block placed at the surface of a column.
    pub fn surface_block(&self, bx: i32, by: i32, h: i32, clim_a: f32, clim_b: f32) -> Block {
        let mut kind: u8 = 4;
        let mut c = color_a(bx, by);
        let hf = self.height_factor_a(bx, by);
        let w = clamp01((hf * 10.0 - 0.3) * 1.5);
        if clim_b > 0.8 {
            let mut t = (clim_b - 0.8) / 0.1;
            if t > 1.0 {
                t = 1.0;
            }
            let n = clamp01(noise(f64::from(bx) * 0.03, f64::from(by) * 0.03));
            let n = 1.0 - n * n;
            let m = (1.0 - n * n) * 0.8 * t;
            let f = 1.0 - m;
            c = [c[0] * f, c[1] * f, c[2] * f];
            let tc = self.terrain_color(bx, by, h);
            c = [tc[0] * m + c[0], tc[1] * m + c[1], tc[2] * m + c[2]];
        }
        if clim_a < 0.2 && clim_b > 0.75 {
            kind = 9;
            let mut t = clamp01((1.0 - clim_a / 0.2) * (clim_b - 0.75) * 4.0 * 10.0);
            if let Some(cell) = self.cell_at_block(bx, by)
                && cell.kind == 4
            {
                let u = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16) - 0.5;
                let u = if u >= 0.0 { if u > 1.0 { 1.0 } else { u } } else { 0.0 };
                let u = 1.0 - u * u;
                t *= 1.0 - u * u;
            }
            t *= w;
            if t <= 1.0 {
                if t < 0.5 {
                    kind = 4;
                }
            } else {
                t = 1.0;
            }
            let f = 1.0 - t;
            c = [c[0] * f, c[1] * f, c[2] * f];
            let cb = color_b(&self.seeds, bx, by);
            c = [cb[0] * t + c[0], cb[1] * t + c[1], cb[2] * t + c[2]];
        }
        let n = noise(f64::from(bx) * 0.01 + 854.0, f64::from(by) * 0.01 + 985.0);
        let v = clamp01((n + (15 - h) as f32 / 10.0) - 0.5);
        let v = 1.0 - v * v;
        let mut sn = 1.0 - v * v;
        if clim_b < 0.2 {
            sn *= clim_b / 0.2;
        }
        if sn > 0.0 {
            let f = 1.0 - sn;
            c = [f * c[0], f * c[1], f * c[2]];
            let cc = color_c(&self.seeds, bx, by);
            c = [cc[0] * sn + c[0], cc[1] * sn + c[1], cc[2] * sn + c[2]];
            kind = 9;
        }
        if clim_b < 0.3 {
            kind = 10;
            let mut s2 = 1.0 - (clim_b - 0.29) / 0.01;
            if s2 > 1.0 {
                s2 = 1.0;
            }
            let f = 1.0 - s2;
            c = [f * c[0], f * c[1], f * c[2]];
            let cd = color_d(bx, by);
            c = [cd[0] * s2 + c[0], cd[1] * s2 + c[1], cd[2] * s2 + c[2]];
        }
        let clim_c = self.column_climate_c(bx, by);
        if clim_c > 0.0 {
            let f = 1.0 - clim_c;
            c = [f * c[0], f * c[1], f * c[2]];
            let tc = self.terrain_color(bx, by, h);
            c = [tc[0] * clim_c + c[0], tc[1] * clim_c + c[1], tc[2] * clim_c + c[2]];
            if clim_c > 0.5 {
                kind = 0xc;
            }
        }
        [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, kind | 0x20]
    }

    /// `mountainFactor(bx, by, zone)`, `Server.exe 0x00523d80`: 0..1 mask of mountainous terrain.
    #[allow(clippy::collapsible_if)] // nested null / kind checks as in the original
    pub fn mountain_factor(&self, bx: i32, by: i32) -> f32 {
        let s = &self.seeds;
        let xd = f64::from(bx) * 0.01;
        let yd = f64::from(by) * 0.01;
        let n1 = noise(xd, yd);
        let a = 1.0 - n1 * n1;
        let n2 = noise(s.f(0x8001dc) + xd + 843.0, s.f(0x8001e0) + yd + 984.0);
        let b = n2 * 0.1;
        let n3 = noise(f64::from(bx) * 0.0025 + s.f(0x8001dc), f64::from(by) * 0.0025 + s.f(0x8001e0));
        let mut m = 1.0 - (b + n3).abs() * ((1.0 - a * a) * 1.3 + 2.0);
        let n4 = noise(f64::from(bx) * 0.005 + 94.0, f64::from(by) * 0.005 + 874.0);
        m *= n4 * 0.4 + 0.6;
        let cell = self.cell_at_block(bx, by);
        if let Some(cell) = cell {
            if cell.kind == 6 || cell.kind == 7 {
                let nd = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                let mut p = 0.5;
                if m < 1.0 {
                    if m + 0.5 > 1.0 {
                        p = 1.0 - m;
                    }
                } else {
                    p = 0.0;
                }
                if nd > 0.36 {
                    if nd < 1.0 {
                        let q = ((f64::from(nd).sqrt() as f32) - 0.6) / 0.39999998;
                        let q = 1.0 - q * q;
                        m += q * q * p;
                    }
                } else {
                    let mut q = (1.0 - nd / 0.36) * 1.5;
                    if q > 1.0 {
                        q = 1.0;
                    }
                    m = (p + m) * (1.0 - q * q) + q * q * 0.0;
                }
            }
        }
        if m >= 0.0 {
            let ha = self.height_factor_a(bx, by);
            let mut q = ha * 2.0;
            if q > 1.0 {
                q = 1.0;
            }
            let s1 = smooth(q) * m;
            let hb = self.height_factor_b(bx, by);
            let mut q = hb;
            if q > 1.0 {
                q = 1.0;
            }
            let s2 = smooth(q) * s1;
            let cf = self.cell_falloff(bx, by);
            let mut q = cf * 2.0;
            if cf * 2.0 > 1.0 {
                q = 1.0;
            }
            let q = 1.0 - smooth(q);
            m = q * q * q * s2;
        } else {
            m = 0.0;
        }
        let mut v = m;
        if let Some(cell) = cell {
            if cell.kind == 2 {
                let d = 1.0 - cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                let r = if d > 0.0 { d * d } else { 0.0 };
                return smooth(m) + r;
            }
            if cell.kind == 10 && cell.radius > 0.0 {
                let dx = (cell.x - (i64::from(bx) << 16)) as f32 * 1.5258789e-05;
                let dy = (cell.y - (i64::from(by) << 16)) as f32;
                let r = (dy * 1.5258789e-05 * dy * 1.5258789e-05 + dx * dx) / (cell.radius * cell.radius);
                if r < 1.0 {
                    let r = 1.0 - r;
                    m *= 1.0 - r * r;
                }
            }
            v = m;
            if cell.kind == 4 || cell.kind == 5 {
                let nd = cell.norm_distance(i64::from(bx) << 16, i64::from(by) << 16);
                if nd > 0.25 {
                    v = m;
                    if nd < 1.0 {
                        let q = ((f64::from(nd).sqrt() as f32) - 0.5) * 2.0;
                        let q = 1.0 - q * q;
                        v = (1.0 - q * q) * m;
                    }
                } else {
                    v = 0.0;
                }
            }
        }
        smooth(v)
    }
}
