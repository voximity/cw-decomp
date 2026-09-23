//! Boulders and rock piles placed by `generateZone`: `placeBoulder` (`Server.exe 0x004ffbf0`)
//! and `placeRock` (`Server.exe 0x004ff3f0`).
//!
//! Both carve a noise-warped ellipsoid of half-axes `(sx, sy)` and height `sz` above `z` into
//! the zone. A first pass only checks for protected blocks (flag 0x80) and abandons the whole
//! feature when one is inside the ellipsoid; the second pass writes the blocks.

use cw_math::value_noise_2d as noise;

use crate::surface::Block;
use crate::world::World;
use crate::zone::{Zone, set_block};

#[inline]
fn rgb_bytes(c: [f32; 3], kind: u8) -> Block {
    [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8, kind]
}

/// Block types a feature may overwrite: air, water, and the three soft ground types.
#[inline]
fn replaceable(b: Block) -> bool {
    let t = b[3] & 0x1f;
    matches!(t, 0 | 2 | 4 | 9 | 5) && b[3] & 0x40 == 0
}

impl World {
    /// `placeBoulder(x, y, z, sx, sy, sz, zone)`, `Server.exe 0x004ffbf0`: type-6 rock with the
    /// terrain colour, capped by a surface block.
    #[allow(clippy::too_many_arguments)]
    ///
    /// Performance note (release, 2026-09, `examples/gen_bench.rs`): the Features stage of a
    /// zone is 0..350 ms and almost all of it is here and in `place_rock`: two noise samples
    /// per block over a box up to 200 blocks a side, twice (the protection pass, then the
    /// write pass). The original does exactly this work; only the block loop of pass 1 could
    /// be skipped when pass 2 will find the same blocks, and that is not worth the risk.
    pub fn place_boulder(&self, zone: &mut Zone, x: i32, y: i32, z: i32, sx: i32, sy: i32, sz: i32) {
        let top_base = (sz + z) as f32;
        let ux = 10.0 / sx as f32;
        let uy = 10.0 / sy as f32;
        // Pass 1: abort on a protected block.
        for px in x - sx * 2..=x + sx * 2 {
            let dx = (px - x) as f32 / sx as f32;
            let pxd = f64::from(px);
            for py in y - sy * 2..=y + sy * 2 {
                if !zone.contains(px, py) {
                    continue;
                }
                let base = zone.column(px, py).height;
                let pyd = f64::from(py);
                let n = noise(pxd * 0.01 + 4394.0, pyd * 0.01 + 8974.0);
                let top = (n * 20.0 + top_base) as i32;
                let dy = (py - y) as f32 / sy as f32;
                let mut h = top;
                while base <= h {
                    let hd = f64::from(h);
                    let u = noise(pyd * 0.05, hd * 0.05) * ux + dx;
                    let v = noise(pxd * 0.05 + 4374.0, hd * 0.05 + 9898.0) * uy + dy;
                    if v * v + u * u <= 1.0 {
                        let b = zone.block(px, py, h);
                        if b[3] & 0x1f != 2 && b[3] & 0x80 != 0 {
                            return;
                        }
                    }
                    h -= 1;
                }
            }
        }
        // Pass 2: write the blocks.
        for px in x - sx * 2..=x + sx * 2 {
            let dx = (px - x) as f32 / sx as f32;
            let pxd = f64::from(px);
            for py in y - sy * 2..=y + sy * 2 {
                let pyd = f64::from(py);
                let n = noise(pxd * 0.01 + 4394.0, pyd * 0.01 + 8974.0);
                let top = (n * 20.0 + top_base) as i32;
                if !zone.contains(px, py) {
                    continue;
                }
                let base = zone.column(px, py).height;
                if base > top {
                    continue;
                }
                let dy = (py - y) as f32 / sy as f32;
                let mut h = top;
                while base <= h {
                    let b = zone.block(px, py, h);
                    if replaceable(b) {
                        let hd = f64::from(h);
                        let u = noise(pyd * 0.05, hd * 0.05) * ux + dx;
                        let v = noise(pxd * 0.05 + 4374.0, hd * 0.05 + 9898.0) * uy + dy;
                        if v * v + u * u <= 1.0 {
                            let nb = if h == top {
                                let col = zone.column(px, py);
                                self.surface_block(px, py, h, col.climate_a, col.climate_b)
                            } else {
                                rgb_bytes(self.terrain_color(px, py, h), 6)
                            };
                            set_block(self, zone, px, py, h, nb);
                        }
                    }
                    h -= 1;
                }
            }
        }
    }

    /// `placeRock(x, y, z, sx, sy, sz, zone)`, `Server.exe 0x004ff3f0`: type-5 rock blending
    /// from the rock colour at the base to the terrain colour eight blocks up.
    #[allow(clippy::too_many_arguments)]
    pub fn place_rock(&self, zone: &mut Zone, x: i32, y: i32, z: i32, sx: i32, sy: i32, sz: i32) {
        let top_base = (sz + z) as f32;
        let szf = sz as f32;
        // Pass 1: abort on a protected block.
        for px in x - sx * 2..=x + sx * 2 {
            let dx = (px - x) as f32 / sx as f32;
            let pxd = f64::from(px);
            for py in y - sy * 2..=y + sy * 2 {
                if !zone.contains(px, py) {
                    continue;
                }
                let base = zone.column(px, py).height;
                let pyd = f64::from(py);
                let n = noise(pxd * 0.01 + 4394.0, pyd * 0.01 + 8974.0);
                let top = (n * szf + top_base) as i32;
                let dy = (py - y) as f32 / sy as f32;
                let mut h = top;
                while base <= h {
                    let hd = f64::from(h);
                    let u = noise(pyd * 0.05, hd * 0.05) * 0.4 + dx;
                    let v = noise(pxd * 0.05 + 4374.0, hd * 0.05 + 9898.0) * 0.4 + dy;
                    if v * v + u * u <= 1.0 && zone.block(px, py, h)[3] & 0x80 != 0 {
                        return;
                    }
                    h -= 1;
                }
            }
        }
        // Pass 2: write the blocks.
        for px in x - sx * 2..=x + sx * 2 {
            let dx = (px - x) as f32 / sx as f32;
            let pxd = f64::from(px);
            for py in y - sy * 2..=y + sy * 2 {
                let pyd = f64::from(py);
                let n = noise(pxd * 0.01 + 4394.0, pyd * 0.01 + 8974.0);
                let top = (n * szf + top_base) as i32;
                if !zone.contains(px, py) {
                    continue;
                }
                let base = zone.column(px, py).height;
                if base > top {
                    continue;
                }
                let dy = (py - y) as f32 / sy as f32;
                let below = -8 - base;
                let mut h = top;
                while base <= h {
                    let b = zone.block(px, py, h);
                    if replaceable(b) {
                        let h02 = f64::from(h) * 0.02;
                        let u = noise(pyd * 0.05, h02) * 0.5 + dx;
                        let v = noise(pxd * 0.05 + 4374.0, h02 + 9898.0) * 0.5 + dy;
                        if v * v + u * u <= 1.0 {
                            let nb = if h == top {
                                let col = zone.column(px, py);
                                self.surface_block(px, py, h, col.climate_a, col.climate_b)
                            } else {
                                let mut t = (below + h) as f32 * 0.05;
                                if t >= 0.0 {
                                    if t > 1.0 {
                                        t = 1.0;
                                    }
                                } else {
                                    t = 0.0;
                                }
                                let tc = self.terrain_color(px, py, h);
                                let rc = self.rock_color(px, py, h);
                                let inv = 1.0 - t;
                                rgb_bytes([rc[0] * inv + tc[0] * t, rc[1] * inv + tc[1] * t, rc[2] * inv + tc[2] * t], 5)
                            };
                            set_block(self, zone, px, py, h, nb);
                        }
                    }
                    h -= 1;
                }
            }
        }
    }
}
