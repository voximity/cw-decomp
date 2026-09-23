//! `cube::World::generateTree`, `Server.exe 0x00513760`, and the voxel primitives it draws with:
//! `fillSphere` (`0x004d4820`), `fillDisc` (`0x004d44c0`), `fillLeafCrown` (`0x0050bd60`) and the
//! moss factor (`0x00523b90`). See `analysis/notes/functions/00513760_generateTree.md` and
//! `analysis/notes/porting-brief.md`.
//!
//! Float widths follow the disassembly, not Ghidra's pseudo-C: the sphere/disc/crown centres and
//! distances, every angle and every branch position are doubles in the original.

// The comparisons mirror the original's branch conditions, including their NaN behaviour.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp)]

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use cw_math::value_noise_2d as noise;

use crate::fixed::to_block;
use crate::color::color_d;
use crate::model::Model;
use crate::surface::Block;
use crate::world::World;
use crate::zone::{Zone, set_block};

/// `1 / 65536`, the original's `1.52587890625e-05` (16.16 fixed to blocks).
const FIX: f64 = 1.52587890625e-05;
/// The frond angle step of kind 1, `0x00573a20` (`3π / 10` rounded as the original stores it).
const FROND_STEP: f64 = f64::from_bits(0x3fee_28c7_4606_98c7);

use crate::zone::Prop;

/// Appends a prop to the zone's prop list (`FUN_004c6770` on `zone+4`).
fn push_prop(zone: &mut Zone, prop: Prop) {
    zone.props.push(prop);
}

#[inline]
fn rgb_block(c: [f32; 3], kind: u8) -> Block {
    [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, kind]
}

/// `FUN_004e2840`: clamps each channel to `0..=255`.
#[inline]
fn clamp255(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| {
        if v < 0.0 {
            0.0
        } else if v > 255.0 {
            255.0
        } else {
            v
        }
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

/// `cube::World::getBlockFixed(x64, y64, z64, zone)`, `Server.exe 0x00406050`.
fn block_fixed(zone: &Zone, x: i64, y: i64, z: i64) -> Block {
    zone.block(to_block(x), to_block(y), to_block(z))
}

impl World {
    /// `climateA` of the column when it belongs to the zone (getColumn with a zone hint),
    /// otherwise computed (`0x004f8570`).
    fn zone_climate_a(&self, zone: &Zone, x: i32, y: i32) -> f32 {
        if zone.contains(x, y) { zone.column(x, y).climate_a } else { self.climate_a(x, y) }
    }

    /// `climateB` likewise (`0x004f8b40`).
    fn zone_climate_b(&self, zone: &Zone, x: i32, y: i32) -> f32 {
        if zone.contains(x, y) { zone.column(x, y).climate_b } else { self.climate_b(x, y) }
    }

    /// `world->models[index]` (`world+0x20`, a vector of `cube::Model*`): `None` when the table
    /// is not loaded or the slot is empty (the original then skips the placement).
    fn tree_model(&self, index: usize) -> Option<Model> {
        self.models.get(index).filter(|m| !m.voxels.is_empty()).cloned()
    }

    /// Moss factor 0..1 at a block, `Server.exe 0x00523b90`: climate A times a noise term.
    fn moss_factor(&self, zone: &Zone, x: i32, y: i32, z: i32) -> f32 {
        let clim_a = self.zone_climate_a(zone, x, y);
        let n1 = noise(f64::from(y as f32 * 0.2 + 534.0), f64::from(z as f32 * 0.2 + 13.0)) * 0.1;
        let n2 = noise(f64::from(x) * 0.05 + 4343.0, f64::from(z) * 0.1 + 84734.0);
        let v = (n1 + n2) * 0.7 + 0.2;
        let m = v * clim_a;
        if !(m > 1.0) && !(0.0 <= m) {
            return 0.0;
        }
        if !(m <= 1.0) {
            return 1.0;
        }
        m
    }

    /// Applies the moss tint of `0x004d4820`/`0x004d44c0` to a block colour.
    fn mossy(&self, zone: &Zone, x: i32, y: i32, z: i32, block: Block) -> Block {
        let (r, g, b) = (f32::from(block[0]), f32::from(block[1]), f32::from(block[2]));
        let (dr, dg, db) = (50.0 - r, 120.0 - g, 60.0 - b);
        let m = self.moss_factor(zone, x, y, z);
        rgb_block([dr * m + r, dg * m + g, db * m + b], block[3])
    }

    /// `fillSphere(x, y, z, radius, block, zone, centreHalf, mossy)`, `Server.exe 0x004d4820`:
    /// writes `block` at every position within `radius` of the centre that has no 0x40 flag.
    #[allow(clippy::too_many_arguments)]
    fn fill_sphere(
        &self,
        zone: &mut Zone,
        x: i32,
        y: i32,
        z: i32,
        radius: f32,
        block: Block,
        centre_half: bool,
        mossy: bool,
    ) {
        let (mut cx, mut cy, mut cz) = (f64::from(x), f64::from(y), f64::from(z));
        if centre_half {
            cx += 0.5;
            cy += 0.5;
            cz += 0.5;
        }
        let (xf, yf, zf) = (x as f32, y as f32, z as f32);
        let x_max = xf + radius + 1.0;
        let y_max = yf + radius + 1.0;
        let z_max = zf + radius + 1.0;
        let y0 = ((yf - radius) - 1.0) as i32;
        let z0 = ((zf - radius) - 1.0) as i32;
        let r2 = f64::from(radius * radius);
        let mut px = ((xf - radius) - 1.0) as i32;
        while px as f32 <= x_max {
            let mut py = y0;
            while py as f32 <= y_max {
                let dx = f64::from(px) - cx;
                let dy = f64::from(py) - cy;
                let dxy = dy * dy + dx * dx;
                let mut pz = z0;
                while pz as f32 <= z_max {
                    let dz = f64::from(pz) - cz;
                    if dz * dz + dxy <= r2 && zone.block(px, py, pz)[3] & 0x40 == 0 {
                        let b = if mossy { self.mossy(zone, px, py, pz, block) } else { block };
                        set_block(self, zone, px, py, pz, b);
                    }
                    pz += 1;
                }
                py += 1;
            }
            px += 1;
        }
    }

    /// `fillDisc(x, y, z, radius, block, zone, centreHalf, mossy)`, `Server.exe 0x004d44c0`: the
    /// horizontal-circle version of [`Self::fill_sphere`] at height `z`.
    #[allow(clippy::too_many_arguments)]
    fn fill_disc(
        &self,
        zone: &mut Zone,
        x: i32,
        y: i32,
        z: i32,
        radius: f32,
        block: Block,
        centre_half: bool,
        mossy: bool,
    ) {
        let (mut cx, mut cy) = (f64::from(x), f64::from(y));
        if centre_half {
            cx += 0.5;
            cy += 0.5;
        }
        let (xf, yf) = (x as f32, y as f32);
        let x_max = xf + radius + 1.0;
        let y_max = yf + radius + 1.0;
        let y0 = ((yf - radius) - 1.0) as i32;
        let r2 = f64::from(radius * radius);
        let mut px = ((xf - radius) - 1.0) as i32;
        while px as f32 <= x_max {
            let dx = f64::from(px) - cx;
            let dx2 = dx * dx;
            let mut py = y0;
            while py as f32 <= y_max {
                let dy = f64::from(py) - cy;
                if dy * dy + dx2 <= r2 && zone.block(px, py, z)[3] & 0x40 == 0 {
                    let b = if mossy { self.mossy(zone, px, py, z, block) } else { block };
                    set_block(self, zone, px, py, z, b);
                }
                py += 1;
            }
            px += 1;
        }
    }

    /// `fillLeafCrown(x, y, z, rXY, rZ, outer, inner, kind, zone)`, `Server.exe 0x0050bd60`: a
    /// noise-deformed ellipsoid of type-8 leaves (0x28), `outer` at the bottom blending to
    /// `inner` at the top. Kind 4 hangs vines under some leaves.
    #[allow(clippy::too_many_arguments)]
    fn leaf_crown(
        &mut self,
        zone: &mut Zone,
        x: i32,
        y: i32,
        z: i32,
        rxy: f32,
        rz: f32,
        outer: [f32; 3],
        inner: [f32; 3],
        kind: i32,
    ) {
        let cx = f64::from(x) + 0.5;
        let cy = f64::from(y) + 0.5;
        let cz = f64::from(z) + 0.5;
        let na = f64::from(self.rng.rand()); // rand @ 0x0050bd9b
        let nb = f64::from(self.rng.rand()); // rand @ 0x0050bda4
        let rxyd = f64::from(rxy);
        let rzd = f64::from(rz);
        let x_max = rxyd + cx + 1.0;
        let y_max = rxyd + cy + 1.0;
        let y0 = ((cy - rxyd) - 1.0) as i32;
        let z_min = (cz - rzd) - 1.0;
        let z_top = (rzd + cz + 1.0) as i32;
        let shade = |zz: i32, off: f32| -> [f32; 3] {
            let d = (f64::from(zz) - cz) / rzd * 1.7999999523162842 + f64::from(off);
            let mut t = ((d + 1.0) * 0.5) as f32;
            if 0.0 <= t {
                if t > 1.0 {
                    t = 1.0;
                }
            } else {
                t = 0.0;
            }
            let u = 1.0 - t;
            clamp255([u * outer[0] + inner[0] * t, u * outer[1] + inner[1] * t, u * outer[2] + inner[2] * t])
        };
        let mut px = ((cx - rxyd) - 1.0) as i32;
        while f64::from(px) <= x_max {
            let xd = f64::from(px);
            let mut py = y0;
            while f64::from(py) <= y_max {
                let yd = f64::from(py);
                let n1 = noise(xd * 0.04 + na, yd * 0.04 + nb);
                let half = f64::from(n1) * 0.5;
                let n2 = noise(xd * 0.1 + na, yd * 0.1 + nb);
                let off = (half + f64::from(n2 * 0.3)) as f32;
                if f64::from(z_top) >= z_min {
                    let dxn = (xd - cx) / rxyd;
                    let dyn_ = (yd - cy) / rxyd;
                    let r2 = dyn_ * dyn_ + dxn * dxn;
                    let mut pz = z_top;
                    while f64::from(pz) >= z_min {
                        let d = (f64::from(pz) - cz) / rzd * 1.7999999523162842 + f64::from(off);
                        if d * d + r2 <= 1.0 && zone.block(px, py, pz)[3] & 0x40 == 0 {
                            set_block(self, zone, px, py, pz, rgb_block(shade(pz, off), 0x28));
                            if kind == 4 && self.rng.rand() % 20 == 0 {
                                // rand @ 0x0050c0ba
                                let mut len = self.rng.rand() % 10; // rand @ 0x0050c0d0
                                if self.rng.rand() % 4 == 0 {
                                    // rand @ 0x0050c0db
                                    len *= 2;
                                }
                                let mut vz = pz;
                                for _ in 0..len.max(0) {
                                    vz -= 1;
                                    if zone.block(px, py, vz)[3] & 0x40 == 0 {
                                        set_block(self, zone, px, py, vz, rgb_block(shade(vz, off), 0x28));
                                    }
                                }
                            }
                        }
                        pz -= 1;
                    }
                }
                py += 1;
            }
            px += 1;
        }
    }

    /// `generateTree(x, y, z, radius, height, kind, zone)`, `Server.exe 0x00513760`.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_tree(&mut self, zone: &mut Zone, x: i32, y: i32, z: i32, radius: i32, height: i32, kind: i32) {
        let (bx, by) = (x, y);
        let mut r = radius;
        let mut h = height;
        let rf0 = r as f32;
        let mut r_base = rf0 * 0.25;
        let mut r_top = r_base;
        // Block centres in 16.16 (`ftol(-32768.0)` subtracted).
        let cx64 = (i64::from(bx) << 16) + 32768;
        let cy64 = (i64::from(by) << 16) + 32768;

        // Trunk colours.
        let mut clim_b = self.zone_climate_b(zone, bx, by);
        let base = clim_b * 50.0 + 120.0;
        let base2 = clim_b * 100.0 + 50.0;
        let a_g = (self.rng.rand() % 80) as f32 + base; // rand @ 0x005138a4
        let a_b = (self.rng.rand() % 50) as f32 + base2; // rand @ 0x005138e5
        let b_g = (self.rng.rand() % 80) as f32 + base; // rand @ 0x0051392d
        let b_b = (self.rng.rand() % 50) as f32 + base2; // rand @ 0x0051394e
        let mut col_a = [240.0f32, a_g, a_b];
        let mut col_b = [240.0f32, b_g, b_b];

        // Kind adjustments.
        let clamp_radii = |r_base: &mut f32, r_top: &mut f32| {
            if 0.3 > *r_base {
                *r_base = 0.3;
            }
            if 0.3 > *r_top {
                *r_top = 0.3;
            }
        };
        match kind {
            5 => {
                h = (h as f32 * 0.7) as i32;
                r = ((rf0 * 0.2) as i32).min(3);
                let r8 = (r << 3) as f32;
                if r8 > h as f32 {
                    h = r8 as i32;
                }
                clamp_radii(&mut r_base, &mut r_top);
            }
            1 => {
                r_base *= 0.5;
                h *= 2;
                r_top = 0.1;
                clamp_radii(&mut r_base, &mut r_top);
            }
            6 => {
                r_top = r_base * 0.5;
                clamp_radii(&mut r_base, &mut r_top);
            }
            3 => {
                col_a = [240.0, 180.0, 120.0];
                col_b = [220.0, 100.0, 50.0];
                r_base = 2.0;
                r_top = 2.0;
            }
            2 => {
                r_base = 1.5;
                r_top = 1.5;
                col_a = [255.0; 3];
                col_b = [255.0; 3];
            }
            _ => clamp_radii(&mut r_base, &mut r_top),
        }

        // Leaf colours: `l2` outer/primary (`local_2c[6..8]`), `l1` inner (`local_2c[3..5]`).
        let mut leaf_glow = false;
        let t_leaf = (clim_b - 0.5) * 10.0;
        if 0.0 > clim_b {
            clim_b = 0.0;
        } else if clim_b > 1.0 {
            clim_b = 1.0;
        }
        let mut l2: [f32; 3];
        let mut l1: [f32; 3];
        let roll = self.rng.rand(); // rand @ 0x00513bb1
        if t_leaf > roll as f32 / 32767.0 || kind == 4 || kind == 3 {
            let lr = (self.rng.rand() % 20) as f32; // rand @ 0x00514048
            let lg = (self.rng.rand() % 155 + 50) as f32; // rand @ 0x00514061
            let lb = (self.rng.rand() % 100) as f32; // rand @ 0x0051407d
            l2 = clamp255([lr, lg, lb]);
            let k = (self.rng.rand() % 100 + 50) as f32; // rand @ 0x0051413e
            l1 = clamp255([k + l2[0], k + l2[1], l2[2] + 0.0]);
        } else {
            let clim_a = self.zone_climate_a(zone, bx, by);
            if clim_a > 0.5 {
                let lr = (self.rng.rand() % 55 + 200) as f32; // rand @ 0x00513c35
                let lg = (self.rng.rand() % 100) as f32; // rand @ 0x00513c54
                let lb = (self.rng.rand() % 50) as f32; // rand @ 0x00513c6d
                l2 = [lr, lg, lb];
                let k = (self.rng.rand() % 100 + 50) as f32; // rand @ 0x00513ca5
                l1 = clamp255([lr + k, lg + k, lb + 0.0]);
                leaf_glow = true;
            } else if self.rng.rand() % 2 == 0 {
                // rand @ 0x00513d5a
                let a = self.rng.rand(); // rand @ 0x00513efd
                let b = self.rng.rand(); // rand @ 0x00513f0d
                l2 = clamp255([(a % 55 + 200) as f32, 0.0, (b % 200) as f32]);
                let k = (self.rng.rand() % 100 + 100) as f32; // rand @ 0x00513f87
                l1 = clamp255([k + l2[0], k + l2[1], k + l2[2]]);
                leaf_glow = true;
            } else {
                let lr = (self.rng.rand() % 20) as f32; // rand @ 0x00513d78
                let lg = (self.rng.rand() % 100 + 120) as f32; // rand @ 0x00513d91
                let lb = (self.rng.rand() % 100) as f32; // rand @ 0x00513d9e
                l2 = clamp255([lr, lg, lb]);
                let k = (self.rng.rand() % 150 + 100) as f32; // rand @ 0x00513e1e
                l1 = clamp255([k + l2[0], k + l2[1], l2[2] + 0.0]);
            }
        }
        if kind == 2 {
            l2 = [150.0, 255.0, 0.0];
            l1 = [244.0, 255.0, 0.0];
            leaf_glow = false;
        }
        // Snow blend.
        if 0.2 > clim_b {
            let mut w = 1.0 - (clim_b - 0.1) / 0.1;
            if w > 1.0 {
                w = 1.0;
            }
            let f = 1.0 - w;
            let s = color_d(bx, by);
            l2 = [w * s[0] + f * l2[0], w * s[1] + f * l2[1], w * s[2] + f * l2[2]];
            let s = color_d(bx, by);
            l1 = [s[0] * w + f * l1[0], s[1] * w + f * l1[1], s[2] * w + f * l1[2]];
            let w120 = w * 120.0;
            col_a = [w120 + f * col_a[0], w120 + f * col_a[1], w * 80.0 + f * col_a[2]];
        }

        // Kind 3 lean of the trunk top.
        let (mut lean_x, mut lean_y) = (0.0f32, 0.0f32);
        if kind == 3 {
            lean_x = (0.5 - self.rng.rand() as f32 / 32767.0) * h as f32; // rand @ 0x00514571
            lean_y = (0.5 - self.rng.rand() as f32 / 32767.0) * h as f32; // rand @ 0x005145a7
            let rf = r as f32;
            let zx0 = zone.x * 256;
            let sx = bx as f32 + lean_x;
            if zx0 as f32 > sx - rf || !(rf + sx < (zx0 + 256) as f32) {
                lean_x = 0.0;
            }
            let zy0 = zone.y * 256;
            let sy = by as f32 + lean_y;
            if zy0 as f32 > sy - rf || !(rf + sy < (zy0 + 256) as f32) {
                lean_y = 0.0;
            }
        }

        // Trunk voxels.
        {
            let bxf = bx as f32;
            let byf = by as f32;
            let x_max = bxf + r_base + 1.0;
            let y_max = byf + r_base + 1.0;
            let y0 = ((byf - r_base) - 1.0) as i32;
            let hp2 = h as f32 + 2.0;
            let wood = if kind == 6 { 0x07 } else { 0x27 };
            let mut px = ((bxf - r_base) - 1.0) as i32;
            while px as f32 <= x_max {
                let pxf = px as f32;
                let mut py = y0;
                while py as f32 <= y_max {
                    let pyf = py as f32;
                    let mut zz = z + h;
                    let mut cnt = zz - z + 2;
                    while zz >= z - 2 {
                        let mut t = cnt as f32 / hp2;
                        if 0.0 > t {
                            t = 0.0;
                        }
                        let t2 = t * t;
                        let rad = (1.0 - t2) * r_base + t2 * r_top;
                        let (mut sx, mut sy) = (px, py);
                        if kind == 3 {
                            sx = (lean_x * t2 + pxf) as i32;
                            sy = (lean_y * t2 + pyf) as i32;
                        }
                        let (nx, ny);
                        if rad > 0.8 {
                            let xpart = ((i64::from(px) << 16) - cx64) as f64 * FIX;
                            let ypart = ((i64::from(py) << 16) - cy64) as f64 * FIX;
                            if kind == 6 {
                                let s = 5.0 / rad;
                                let zq = f64::from(zz) * 0.025;
                                let n1 = noise(f64::from(py) * 0.025, zq);
                                nx = f64::from(n1 * s) + xpart / f64::from(rad);
                                let n2 = noise(f64::from(px) * 0.025, zq + 8473.0);
                                ny = f64::from(n2 * s) + ypart / f64::from(rad);
                            } else {
                                nx = xpart / f64::from(rad);
                                ny = ypart / f64::from(rad);
                            }
                        } else {
                            let cyb = (cy64 / 65536) as i32;
                            let cxb = (cx64 / 65536) as i32;
                            nx = f64::from(px.wrapping_sub(cxb)) / f64::from(rad);
                            ny = f64::from(py.wrapping_sub(cyb)) / f64::from(rad);
                        }
                        if ny * ny + nx * nx <= 1.0 && zone.block(sx, sy, zz)[3] & 0x40 == 0 {
                            let u = 1.0 - t;
                            let mut c =
                                [col_a[0] * u + col_b[0] * t, col_a[1] * u + col_b[1] * t, col_a[2] * u + col_b[2] * t];
                            if kind == 2 && self.rng.rand() % 8 == 0 {
                                // rand @ 0x00514cda: birch spots
                                c = [50.0; 3];
                            }
                            let m = self.moss_factor(zone, sx, sy, zz);
                            let d = [50.0 - c[0], 120.0 - c[1], 60.0 - c[2]];
                            let c = [m * d[0] + c[0], m * d[1] + c[1], m * d[2] + c[2]];
                            set_block(self, zone, sx, sy, zz, rgb_block(c, wood));
                        }
                        zz -= 1;
                        cnt -= 1;
                    }
                    py += 1;
                }
                px += 1;
            }
        }

        let cxd = cx64 as f64 * FIX;
        let cyd = cy64 as f64 * FIX;
        let r75 = r_base * 0.75;

        // Kind-specific trunk parts.
        if kind == 6 {
            // Roots.
            let n = self.rng.rand() % 4 + 5; // rand @ 0x00515016
            let a0 = (f64::from(self.rng.rand()) * TAU / 32767.0) as f32; // rand @ 0x00515030
            let step = r75;
            for i in 0..n {
                let ang = (f64::from(i) * PI * 2.0 / f64::from(n) + f64::from(a0)) as f32;
                let s = cw_math::sin(f64::from(ang)) as f32;
                let c = cw_math::cos(f64::from(ang)) as f32;
                let mut px = cxd + f64::from((c * r_base) * 0.5);
                let mut py = cyd + f64::from((s * r_base) * 0.5);
                let mut pz = f64::from((0.0 * r_base) * 0.5) + f64::from(z);
                let step_x = (step * c) * 0.1;
                let step_y = (s * step) * 0.1;
                let fall = step * 0.05;
                let mut k = 0;
                loop {
                    let rad = (10 - k / 20) as f32;
                    self.fill_sphere(zone, px as i32, py as i32, pz as i32, rad, rgb_block(col_a, 0x27), true, true);
                    px += f64::from(step_x);
                    py += f64::from(step_y);
                    let rr = self.rng.rand(); // rand @ 0x005153a9
                    k += 7;
                    pz -= f64::from((rr as f32 * fall) / 32767.0);
                    if k > 0x8c {
                        break;
                    }
                }
            }
        } else if kind == 3 {
            // Tree-top models.
            let tx = (((lean_x * 65536.0) as i64 + cx64) / 65536) as i32;
            let ty = (((lean_y * 65536.0) as i64 + cy64) / 65536) as i32;
            let top = z + (h - 5);
            if let Some(m) = self.tree_model(2300) {
                let (sx, sy) = (m.size[0], m.size[1]);
                let zr = self.rng.rand() % 3; // rand @ 0x0051581e
                self.place_model(zone, &m, [tx - sx / 2, ty + 1, top + zr], 0, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x005158db
                self.place_model(zone, &m, [tx + 1, ty - sx / 2, top + zr], 1, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x0051599e
                self.place_model(zone, &m, [tx - sx / 2, ty - sy, top + zr], 2, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x00515a60
                self.place_model(zone, &m, [tx - sy, ty - sx / 2, top + zr], 3, 0x28, 0, false, [0; 4]);
            }
            if let Some(m) = self.tree_model(2301) {
                let sy = m.size[1];
                let zr = self.rng.rand() % 3; // rand @ 0x00515b58
                self.place_model(zone, &m, [tx + 1, ty + 1, top + zr], 0, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x00515c1b
                self.place_model(zone, &m, [tx + 1, ty - sy, top + zr], 1, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x00515cce
                self.place_model(zone, &m, [tx - sy, ty - sy, top + zr], 2, 0x28, 0, false, [0; 4]);
                let zr = self.rng.rand() % 3; // rand @ 0x00515d91
                self.place_model(zone, &m, [tx - sy, ty + 1, top + zr], 3, 0x28, 0, false, [0; 4]);
            }
        } else if kind != 1 {
            // Root flares (kinds 0, 2, 4, 5 and any other kind).
            let mut acc = -1.0f32;
            let mut zz = z;
            for _ in 0..2 {
                let s = ((4 - z + zz) as f32 * r_base) / 5.0;
                acc += s;
                let blk = rgb_block(col_a, 0x27);
                self.fill_sphere(zone, (bx as f32 - acc) as i32, by, zz, s, blk, true, true);
                self.fill_sphere(zone, ((bx + 1) as f32 + acc) as i32, by, zz, s, blk, true, true);
                self.fill_sphere(zone, bx, (by as f32 - acc) as i32, zz, s, blk, true, true);
                self.fill_sphere(zone, bx, ((by + 1) as f32 + acc) as i32, zz, s, blk, true, true);
                zz -= 1;
            }
        }

        // Tree marker prop.
        if self.rng.rand() % 10 == 0 && clim_b > 0.2 {
            // rand @ 0x00515e0e
            push_prop(zone, Prop { kind: 0x3d, x: cx64, y: cy64, z: i64::from(h + z) << 16, f2c: [1.0; 3], flags: 2, ..Prop::NEW });
        }

        let glow_rgb = |l2: [f32; 3]| [l2[0] / 255.0, l2[1] / 255.0, l2[2] / 255.0];

        match kind {
            5 => {
                // Tiered conifer: branches of wood with leaf balls.
                let n = ((h / 4 + 6) as f32 + r as f32 * 0.5) as i32;
                let _ = self.rng.rand(); // rand @ 0x00515f50
                let _ = self.rng.rand(); // rand @ 0x00515f52
                let a0 = (f64::from(self.rng.rand() % 8) * FRAC_PI_4) as f32; // rand @ 0x00515f54
                let mut acc = 0i32;
                for i in 0..n {
                    // rand @ 0x00515fac (odd tiers only)
                    if i % 2 == 0 || self.rng.rand() % 3 == 0 {
                        self.conifer_tier(
                            zone, i, n, acc, h, z, r, a0, r_base, cxd, cyd, col_a, col_b, l2, l1, leaf_glow,
                        );
                    }
                    acc += h;
                }
            }
            0 | 2 | 4 => {
                // Broadleaf crown plus branches with sub-crowns.
                let nb = self.rng.rand() % 5 + 4; // rand @ 0x00517c85
                let a0 = (f64::from(self.rng.rand()) * TAU / 32767.0) as f32; // rand @ 0x00517c98
                let vf = (self.rng.rand() as f32 * 0.8) / 32767.0 + 0.6; // rand @ 0x00517cbe
                let rr = self.rng.rand(); // rand @ 0x00517ce7
                let big = ((f64::from((rr as f32 * 1.25) / 32767.0) + 2.0) * f64::from(r_base)) as f32;
                self.leaf_crown(zone, (cx64 / 65536) as i32, (cy64 / 65536) as i32, h + z, big, big * vf, l2, l1, kind);
                let zx0 = zone.x * 256;
                let zy0 = zone.y * 256;
                for b in 0..nb {
                    let f = (self.rng.rand() as f32 * 0.5) / 32767.0 + 0.5; // rand @ 0x00517e80
                    let pz0 = f64::from(f * h as f32 + z as f32);
                    let ang = (f64::from(b) * PI * 2.0 / f64::from(nb) + f64::from(a0)) as f32;
                    let s = cw_math::sin(f64::from(ang)) as f32;
                    let c = cw_math::cos(f64::from(ang)) as f32;
                    let mut px = f64::from((c * r_base) * 0.5) + cxd;
                    let mut pz = f64::from((0.0 * r_base) * 0.5) + pz0;
                    let mut py = f64::from((s * r_base) * 0.5) + cyd;
                    let u = 1.0 - f;
                    let col = [col_a[0] * u + col_b[0] * f, col_a[1] * u + col_b[1] * f, col_a[2] * u + col_b[2] * f];
                    let dxs = (c * r75) * 0.5;
                    let dys = (s * r75) * 0.5;
                    let dzs = r75 * 0.5;
                    for _ in 0..6 {
                        if kind != 2 && kind != 4 && r_base > 1.5 {
                            self.fill_sphere(
                                zone,
                                px as i32,
                                py as i32,
                                pz as i32,
                                2.0,
                                rgb_block(col, 0x27),
                                true,
                                true,
                            );
                        }
                        px += f64::from(dxs);
                        py += f64::from(dys);
                        let rr = self.rng.rand(); // rand @ 0x00518245
                        pz += f64::from((rr as f32 * dzs) / 32767.0);
                    }
                    let rr = self.rng.rand(); // rand @ 0x00518290
                    let br = ((f64::from((rr as f32 * 1.25) / 32767.0) + 2.0) * f64::from(r_base)) as f32;
                    let brd = f64::from(br);
                    if f64::from(zx0) > (px - brd) - 1.0 {
                        px = f64::from((zx0 as f32 + br) + 1.0);
                    }
                    if f64::from(zy0) > (py - brd) - 1.0 {
                        py = f64::from((zy0 as f32 + br) + 1.0);
                    }
                    if brd + px + 1.0 >= f64::from(zx0 + 256) {
                        px = f64::from(((zx0 + 256) as f32 - br) - 2.0);
                    }
                    if brd + py + 1.0 >= f64::from(zy0 + 256) {
                        py = f64::from(((zy0 + 256) as f32 - br) - 2.0);
                    }
                    if b > 4 {
                        pz += f64::from((vf * br) * 0.5);
                    }
                    let off = (vf * br) * 0.25;
                    self.leaf_crown(
                        zone,
                        px as i32,
                        py as i32,
                        (f64::from(off) + pz) as i32,
                        br,
                        vf * br,
                        l2,
                        l1,
                        kind,
                    );
                    if leaf_glow {
                        let prop = Prop {
                            kind: 0x3c,
                            x: ((px + 0.0) * 65536.0) as i64,
                            y: ((py + 0.0) * 65536.0) as i64,
                            z: ((pz + f64::from(off)) * 65536.0) as i64,
                            f2c: glow_rgb(l2),
                            flags: 2,
                            ..Prop::NEW
                        };
                        push_prop(zone, prop);
                    }
                }
            }
            6 => {
                // Giant tree: long branches with leaf blobs.
                let nb = self.rng.rand() % 5 + 8; // rand @ 0x00516caf
                let a0 = (f64::from(self.rng.rand()) * TAU / 32767.0) as f32; // rand @ 0x00516cc2
                let _ = self.rng.rand(); // rand @ 0x00516ce8
                let _ = self.rng.rand(); // rand @ 0x00516cea
                for b in 0..nb {
                    let f = (self.rng.rand() as f32 * 0.5) / 32767.0 + 0.5; // rand @ 0x00516d86
                    let pz0 = f64::from(h as f32 * f + z as f32);
                    let ang = (f64::from(b) * PI * 2.0 / f64::from(nb) + f64::from(a0)) as f32;
                    let s = cw_math::sin(f64::from(ang)) as f32;
                    let c = cw_math::cos(f64::from(ang)) as f32;
                    let ang2 = (f64::from(ang) + FRAC_PI_2) as f32;
                    let s2 = cw_math::sin(f64::from(ang2)) as f32;
                    let c2 = cw_math::cos(f64::from(ang2)) as f32;
                    let mut px = f64::from((c * r_base) * 0.5) + cxd;
                    let mut py = f64::from((s * r_base) * 0.5) + cyd;
                    let mut pz = f64::from((0.0 * r_base) * 0.5) + pz0;
                    let u = 1.0 - f;
                    let col = [col_a[0] * u + col_b[0] * f, col_a[1] * u + col_b[1] * f, col_a[2] * u + col_b[2] * f];
                    let mut m = 0i32;
                    let mut k = 0i32;
                    loop {
                        let rad = 6.5 - k as f32 / 40.0;
                        self.fill_sphere(zone, px as i32, py as i32, pz as i32, rad, rgb_block(col, 0x27), true, true);
                        if m > 10 {
                            let q = m - 10;
                            let rad2 = (rad * -2.0) as i32;
                            for _ in 0..5 {
                                if self.rng.rand() % 120 < q {
                                    // rand @ 0x00517143
                                    let hz = (self.rng.rand() % 4 + 4) as f32; // rand @ 0x00517180
                                    let hr = (self.rng.rand() % 4 + 4) as f32; // rand @ 0x0051719e
                                    let zr = self.rng.rand() % (2 - rad2); // rand @ 0x005171bc
                                    let zc = (f64::from(zr) + (pz - 3.0)) as i32;
                                    let yr = self.rng.rand() % 20; // rand @ 0x005171fd
                                    let yc = (((f64::from(yr) + py) - 10.0) + f64::from(s2 * 4.0)) as i32;
                                    let xr = self.rng.rand() % 20; // rand @ 0x0051723b
                                    let xc = (((f64::from(xr) + px) - 10.0) + f64::from(c2 * 4.0)) as i32;
                                    self.leaf_crown(zone, xc, yc, zc, hr, hz, l2, l1, 6);
                                }
                                if self.rng.rand() % 120 < q {
                                    // rand @ 0x00517285
                                    let hz = (self.rng.rand() % 4 + 4) as f32; // rand @ 0x005172c2
                                    let hr = (self.rng.rand() % 4 + 4) as f32; // rand @ 0x005172e0
                                    let zr = self.rng.rand() % ((1 - rad2) * 2); // rand @ 0x005172fe
                                    let zc = (f64::from(zr) + (pz - 3.0)) as i32;
                                    let yr = self.rng.rand() % 20; // rand @ 0x00517341
                                    let yc = (((f64::from(yr) + py) - 10.0) - f64::from(s2 * 4.0)) as i32;
                                    let xr = self.rng.rand() % 20; // rand @ 0x0051737f
                                    let xc = (((f64::from(xr) + px) - 10.0) - f64::from(c2 * 4.0)) as i32;
                                    self.leaf_crown(zone, xc, yc, zc, hr, hz, l2, l1, 6);
                                }
                            }
                            if leaf_glow {
                                let prop = Prop {
                                    kind: 0x3c,
                                    x: (px * 65536.0) as i64,
                                    y: (py * 65536.0) as i64,
                                    z: (pz * 65536.0) as i64,
                                    f2c: glow_rgb(l2),
                                    flags: 2,
                                    ..Prop::NEW
                                };
                                push_prop(zone, prop);
                            }
                        }
                        px += f64::from((c * r75) * 0.075);
                        py += f64::from((s * r75) * 0.075);
                        pz += f64::from(((r75 * 0.1) * m as f32) / 40.0);
                        m += 1;
                        k += 5;
                        if k > 200 {
                            break;
                        }
                    }
                }
            }
            1 => {
                // Palm fronds.
                let mut ang = (f64::from(self.rng.rand()) * TAU / 32767.0) as f32; // rand @ 0x00517608
                let c1b = (self.rng.rand() % 100) as u8; // rand @ 0x0051762e
                let c1g = (self.rng.rand() % 50 + 100) as u8; // rand @ 0x0051763e
                let c1r = (self.rng.rand() % 100) as u8; // rand @ 0x00517651
                let c1 = [f32::from(c1r), f32::from(c1g), f32::from(c1b)];
                let c2b = (self.rng.rand() % 100) as u8; // rand @ 0x00517695
                let c2g = (self.rng.rand() % 50 + 100) as u8; // rand @ 0x005176a5
                let c2r = (self.rng.rand() % 100) as u8; // rand @ 0x005176b2
                let c2 = [f32::from(c2r), f32::from(c2g), f32::from(c2b)];
                let rf = r as f32;
                let lean = (-r) as f32 * 0.3;
                let nseg = h / 3;
                let top = z + (h + 1);
                let (bxd, byd) = (f64::from(bx), f64::from(by));
                for i in 0..30 {
                    ang = (f64::from(ang) + FROND_STEP) as f32;
                    let t = (i as f32 * 0.7) / 30.0 + 0.3;
                    let cw = cw_math::cos(f64::from(ang)) as f32 * rf;
                    let sn = cw_math::sin(f64::from(ang)) as f32 * rf;
                    let w = (1.0 - t) * 2.0;
                    let base_z = f64::from(t * h as f32 + z as f32);
                    let ex = bxd + f64::from(w * cw);
                    let ey = byd + f64::from(w * sn);
                    let ez = base_z + f64::from(w * lean);
                    if nseg > 0 {
                        let nf = nseg as f32;
                        let mut j = nseg;
                        while j >= 1 {
                            let q = j as f32 / nf;
                            let u = 1.0 - q;
                            let (qd, ud) = (f64::from(q), f64::from(u));
                            let px = qd * ex + ud * bxd;
                            let py = qd * ey + ud * byd;
                            let pz = qd * ez + ud * base_z;
                            if 0.3 > clim_b {
                                let sc = color_d(bx, by);
                                self.fill_disc(
                                    zone,
                                    px as i32,
                                    py as i32,
                                    (pz + 1.0) as i32,
                                    2.0,
                                    rgb_block(sc, 0x28),
                                    true,
                                    false,
                                );
                            }
                            let col = [u * c1[0] + c2[0] * q, u * c1[1] + c2[1] * q, u * c1[2] + c2[2] * q];
                            self.fill_disc(
                                zone,
                                px as i32,
                                py as i32,
                                pz as i32,
                                2.0,
                                rgb_block(col, 0x28),
                                true,
                                false,
                            );
                            j -= 1;
                        }
                    }
                    set_block(self, zone, bx, by, top, rgb_block(c1, 0x28));
                    set_block(self, zone, bx, by, h + z, rgb_block(c1, 0x28));
                }
            }
            _ => {}
        }
    }

    /// One tier of the kind-5 conifer (the body of the tier loop at `0x00515fcc`): a wood
    /// branch drawn as a curve of block pairs, with leaf balls at its tip and at random points.
    #[allow(clippy::too_many_arguments)]
    fn conifer_tier(
        &mut self,
        zone: &mut Zone,
        i: i32,
        n: i32,
        acc: i32,
        h: i32,
        z: i32,
        r: i32,
        a0: f32,
        r_base: f32,
        cxd: f64,
        cyd: f64,
        col_a: [f32; 3],
        col_b: [f32; 3],
        l2: [f32; 3],
        l1: [f32; 3],
        leaf_glow: bool,
    ) {
        let f = (i as f32 * 0.6) / (n - 1) as f32 + 0.6;
        let pz0 = f64::from(h as f32 * f + z as f32);
        let ang = (f64::from(i) * PI * 0.25 + f64::from(a0)) as f32;
        let s = cw_math::sin(f64::from(ang)) as f32;
        let c = cw_math::cos(f64::from(ang)) as f32;
        let bxd = f64::from(c * r_base) + cxd;
        let byd = f64::from(s * r_base) + cyd;
        let bzd = f64::from(0.0 * r_base) + pz0;
        let u = 1.0 - f;
        let col = clamp255([col_a[0] * u + f * col_b[0], col_a[1] * u + f * col_b[1], col_a[2] * u + f * col_b[2]]);
        let hh = h / 2;
        let rr = self.rng.rand(); // rand @ 0x00516320
        let l = hh + rr % hh + (acc / 4) / n;
        let rf = r as f32;
        let len = (rf * 8.0) * u + rf;
        let along_y = len * s;
        let along_x = len * c;
        let two_l = l * 2;
        if two_l < 0 {
            return;
        }
        let tlf = two_l as f32;
        let wood = rgb_block(col, 0x27);
        for j in 0..=two_l {
            let jf = j as f32;
            let q = 1.0 - jf / tlf;
            let e = 1.0 - q * q;
            let pz64 = ((f64::from(jf * 0.5) + bzd) * 65536.0) as i64;
            let py64 = ((f64::from(e * along_y) + byd) * 65536.0) as i64;
            let px64 = ((f64::from(e * along_x) + bxd) * 65536.0) as i64;
            if block_fixed(zone, px64, py64, pz64)[3] & 0x40 != 0 {
                continue;
            }
            let top64 = pz64 + 0x10000;
            if block_fixed(zone, px64, py64, top64)[3] & 0x40 != 0 {
                continue;
            }
            let (bx, by) = ((px64 / 65536) as i32, (py64 / 65536) as i32);
            set_block(self, zone, bx, by, (top64 / 65536) as i32, wood);
            set_block(self, zone, bx, by, (pz64 / 65536) as i32, wood);
            // rand @ 0x00516670 (only when j % 4 == 0 and j is not the tip)
            if !(j == two_l || (j % 4 == 0 && self.rng.rand() % 3 == 0)) {
                continue;
            }
            // Leaf ball.
            let r1 = self.rng.rand(); // rand @ 0x00516682
            let rad = ((r1 as f32 * (e * 0.75)) / 32767.0 + 0.25) * (l as f32 * 0.5) + 2.0;
            let r2 = self.rng.rand(); // rand @ 0x005166d6
            let hz = ((r2 as f32 * 0.5) / 32767.0 + 0.5) * rad;
            let mut yy = ((rad * s * 0.25 * 65536.0) as i64).wrapping_add(py64);
            let mut xx = ((rad * c * 0.25 * 65536.0) as i64).wrapping_add(px64);
            let r64 = (rad * 65536.0) as i64;
            let zx0 = zone.x * 256;
            let zy0 = zone.y * 256;
            if xx - r64 - 0x10000 < i64::from(zx0) << 16 {
                xx = (((zx0 as f32 + rad) + 1.0) * 65536.0) as i64;
            }
            if yy - r64 - 0x10000 < i64::from(zy0) << 16 {
                yy = (((zy0 as f32 + rad) + 1.0) * 65536.0) as i64;
            }
            if xx + r64 + 0x10000 >= i64::from(zx0 + 256) << 16 {
                xx = ((((zx0 + 256) as f32 - rad) - 2.0) * 65536.0) as i64;
            }
            if yy + r64 + 0x10000 >= i64::from(zy0 + 256) << 16 {
                yy = ((((zy0 + 256) as f32 - rad) - 2.0) * 65536.0) as i64;
            }
            self.leaf_crown(zone, (xx / 65536) as i32, (yy / 65536) as i32, (top64 / 65536) as i32, rad, hz, l2, l1, 5);
            if leaf_glow {
                let rgb = [l2[0] / 255.0, l2[1] / 255.0, l2[2] / 255.0];
                push_prop(zone, Prop { kind: 0x3c, x: xx, y: yy, z: top64, f2c: rgb, flags: 2, ..Prop::NEW });
            }
        }
    }
}
