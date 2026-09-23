//! Per-call cost of the building blocks the terrain pass evaluates for every column, on a
//! world with the spawn region created.
//!
//!     CW_GAME_DIR=game cargo run --release -p cw-world --example gen_micro

use std::hint::black_box;
use std::time::Instant;

use cw_world::World;

fn time<T>(name: &str, n: usize, mut f: impl FnMut(i32, i32) -> T) {
    let t = Instant::now();
    let mut acc = 0u64;
    for i in 0..n {
        let x = 32800 * 256 + (i % 256) as i32;
        let y = 32800 * 256 + (i / 256) as i32 % 256;
        black_box(f(x, y));
        acc += 1;
    }
    let ns = t.elapsed().as_nanos() as f64 / acc as f64;
    println!("{name:<28} {ns:8.1} ns/call  ({:.1} ms per 65536)", ns * 65536.0 / 1e6);
}

fn main() {
    let dir = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from).expect("CW_GAME_DIR");
    let mut world = World::new(26879);
    let mut models = Vec::new();
    cw_world::model::load_models_from_game_dir(&dir, &mut models).expect("models");
    world.models = std::sync::Arc::new(models);
    world.generate_zone(32800, 32800);
    let n = 65536;
    time("value_noise_2d", n, |x, y| cw_math::value_noise_2d(f64::from(x) * 0.01 + 1.5, f64::from(y) * 0.01 + 2.5));
    time("cos_msvc", n, |x, _| cw_math::cos_msvc::cos(f64::from(x) * 0.001));
    time("climate_a", n, |x, y| world.climate_a(x, y));
    time("climate_b", n, |x, y| world.climate_b(x, y));
    time("climate_c", n, |x, y| world.climate_c(x, y));
    time("nearest_climate_point", n, |x, y| world.nearest_climate_point(x, y).map(|p| p.a));
    time("nearest_climate_region", n, |x, y| world.nearest_climate_region(x, y));
    time("climate_edge_distance", n, |x, y| world.climate_edge_distance(x, y));
    time("base_height", n, |x, y| world.base_height(x, y));
    time("mountain_factor", n, |x, y| world.mountain_factor(x, y));
    time("cell", n, |x, y| world.cell(x >> 11, y >> 11).map(|c| c.kind));
    time("region", n, |x, y| world.region(x >> 14, y >> 14).map(|r| r.level));
    time("zone.column", n, |x, y| world.column(x, y).map(|c| c.height));
}
