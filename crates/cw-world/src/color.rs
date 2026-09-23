//! Terrain colour palettes that depend only on position and the seeds.
//!
//! Each returns `[r, g, b]` as `f32` in `0.0..=255.0`, clamped exactly as the original does.
//! The `f32` arithmetic order matches the decompilation statement by statement; do not
//! reassociate an expression, because that changes the rounding.

use cw_math::value_noise_2d as noise;

use crate::Seeds;

#[inline]
#[allow(clippy::manual_clamp)] // compare-and-branch order of the original
fn clamp255(v: f32) -> f32 {
    if v < 0.0 {
        0.0
    } else if v > 255.0 {
        255.0
    } else {
        v
    }
}

/// `Server.exe 0x00522320`: the base ground palette (grass to rock). No world state.
pub fn color_a(x: i32, y: i32) -> [f32; 3] {
    let xf = f64::from(x);
    let yf = f64::from(y);
    let n1 = noise(xf * 0.04, yf * 0.04);
    let s = (n1 + 1.0) * 0.5;
    let n1b = noise(xf * 0.005 + 45645.0, yf * 0.005 + 456456.0);
    let mut r0 = -(n1b * 120.0);
    if r0 > 0.0 {
        r0 = 0.0;
    }
    let n2 = noise(xf * 0.02 + 89648.0, yf * 0.02 + 1649.0);
    let g0 = n2 * 80.0 + 1.0;
    let n2b = noise(xf * 0.005 + 342.0, yf * 0.005 + 23423.0);
    let t = 1.0 - s;
    let m = g0 * s;
    let r = m + t * 200.0 + r0;
    let g = s * 120.0 + t * 230.0 + n2b * 30.0;
    let b = m + t * g0 + n1b * 120.0;
    [clamp255(r), clamp255(g), clamp255(b)]
}

/// `Server.exe 0x0052d5d0`: a warm palette (sand-like), seeded by +0x800274..+0x800280.
pub fn color_b(seeds: &Seeds, x: i32, y: i32) -> [f32; 3] {
    let xf = f64::from(x);
    let yf = f64::from(y);
    let n1 = noise(seeds.f(0x800274) + xf * 0.03, seeds.f(0x800278) + yf * 0.03);
    let n2 = noise(seeds.f(0x80027c) + xf * 0.003, seeds.f(0x800280) + yf * 0.003);
    let k = (n1 + 1.0) * 0.5 * 80.0;
    let s = (n2 + 1.0) * 0.5;
    let n3 = noise(xf * 0.01 + 493.0, yf * 0.01 + 789.0);
    let t = 1.0 - s;
    let r = t * 255.0 + s * 255.0 + k;
    let g = t * (n3 * 40.0 + 200.0) + s * 150.0 + k;
    let b = t * 100.0 + s * 50.0 + k;
    [clamp255(r), clamp255(g), clamp255(b)]
}

/// `Server.exe 0x004f82d0`: a second warm palette, same seeds as [`color_b`] at a different scale.
pub fn color_c(seeds: &Seeds, x: i32, y: i32) -> [f32; 3] {
    let xf = f64::from(x);
    let yf = f64::from(y);
    let n1 = noise(seeds.f(0x800274) + xf * 0.03, seeds.f(0x800278) + yf * 0.03);
    let n2 = noise(seeds.f(0x80027c) + xf * 0.01, seeds.f(0x800280) + yf * 0.01);
    let k = (n1 + 1.0) * 0.5 * 80.0;
    let s = (n2 + 1.0) * 0.5;
    let t240 = (1.0 - s) * 240.0;
    let r = t240 + s * 240.0 + k;
    let g = t240 + s * 180.0 + k;
    let b = (1.0 - s) * 100.0 + s * 50.0 + k;
    [clamp255(r), clamp255(g), clamp255(b)]
}

/// `Server.exe 0x0052d870`: a pale blue-white palette (snow/ice). No world state.
pub fn color_d(x: i32, y: i32) -> [f32; 3] {
    let xf = f64::from(x);
    let yf = f64::from(y);
    let n1 = noise(xf * 0.04, yf * 0.04);
    let s = (n1 + 1.0) * 0.5;
    let t = 1.0 - s;
    let r = s * 190.0 + t * 100.0;
    let g = s * 220.0 + t * 180.0;
    let b = s * 255.0 + t * 255.0;
    [clamp255(r), clamp255(g), clamp255(b)]
}
