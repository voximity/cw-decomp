//! Times the stages of `generate_zone` for a few zones, to see where generation spends its
//! time.
//!
//!     CW_GAME_DIR=game cargo run --release -p cw-world --example gen_bench -- [seed] [zx zy ...]

use std::time::Instant;

use cw_world::World;
use cw_world::generate::Stage;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = args.first().and_then(|s| s.parse().ok()).unwrap_or(26879);
    let mut zones: Vec<(i32, i32)> = args[1.min(args.len())..].chunks(2).filter_map(|c| Some((c.first()?.parse().ok()?, c.get(1)?.parse().ok()?))).collect();
    if zones.is_empty() {
        zones = vec![(32800, 32800), (32801, 32800), (32800, 32801), (32799, 32799), (32805, 32805)];
    }
    let dir = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from).expect("CW_GAME_DIR");
    let mut world = World::new(seed);
    let mut models = Vec::new();
    let t = Instant::now();
    cw_world::model::load_models_from_game_dir(&dir, &mut models).expect("models");
    println!("models: {} ms", t.elapsed().as_millis());
    world.models = std::sync::Arc::new(models);
    for (zx, zy) in zones {
        let start = Instant::now();
        let mut last = start;
        let mut rows: Vec<(Stage, u128)> = Vec::new();
        world.generate_zone_stages(zx, zy, &mut |stage, _zone| {
            let now = Instant::now();
            rows.push((stage, now.duration_since(last).as_millis()));
            last = now;
        });
        let total = start.elapsed().as_millis();
        let parts: Vec<String> = rows.iter().map(|(s, ms)| format!("{s:?}={ms}")).collect();
        println!("zone {zx},{zy}: {total} ms  [{}]", parts.join(" "));
    }
}
