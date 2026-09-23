//! Large-scale domain warp of a block position, `Server.exe 0x004d5a80` (`cube::World::warpPosition`).

use cw_math::value_noise_2d as noise;

use crate::Seeds;

/// Returns the warped position of block `(x, y)` in units of 16384 blocks, as the original's
/// two doubles. Called by generateZone with the zone centre and by 0x0052d990.
pub fn warp_position(seeds: &Seeds, x: i32, y: i32) -> [f64; 2] {
    let y01 = f64::from(y) * 0.01;
    let y0005 = f64::from(y) * 0.0005;
    let x01 = f64::from(x) * 0.01;
    let x0005 = f64::from(x) * 0.0005;

    let a = noise(seeds.f(0x800204) + x01, seeds.f(0x800208) + y01);
    let b = noise(seeds.f(0x8001fc) + x0005, seeds.f(0x800200) + y0005);
    let out0 = (f64::from(a) * 0.1 + f64::from(b)) * 500.0 * 6.103515625e-05 + f64::from(x) * 6.103515625e-05;

    let c = noise(seeds.f(0x800214) + x01, seeds.f(0x800218) + y01);
    let d = noise(seeds.f(0x80020c) + x0005, seeds.f(0x800210) + y0005);
    let out1 = (f64::from(c) * 0.1 + f64::from(d)) * 500.0 * 6.103515625e-05 + f64::from(y) * 6.103515625e-05;
    [out0, out1]
}
