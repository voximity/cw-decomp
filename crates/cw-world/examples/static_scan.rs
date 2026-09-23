//! Lists the statics of the given kinds in a square of generated zones. Usage:
//! `cargo run --release -p cw-world --example static_scan -- <seed> <zx0,zy0> <zx1,zy1> <kind,...>`
//! Set `CW_GAME_DIR` for the models.
use cw_world::World;

fn pair(s: &str) -> (i32, i32) {
    let mut it = s.split(',');
    (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = args[0].parse().unwrap();
    let (x0, y0) = pair(&args[1]);
    let (x1, y1) = pair(&args[2]);
    let kinds: Vec<u32> = args[3].split(',').map(|k| k.parse().unwrap()).collect();
    let mut world = World::new(seed);
    if let Some(dir) = std::env::var_os("CW_GAME_DIR") {
        let mut table = Vec::new();
        cw_world::model::load_models_from_game_dir(std::path::Path::new(&dir), &mut table).expect("model table");
        world.models = std::sync::Arc::new(table);
    }
    for zx in x0..=x1 {
        for zy in y0..=y1 {
            world.generate_zone(zx, zy);
            let zone = world.zone(zx, zy).expect("zone");
            for (i, s) in zone.statics.iter().enumerate() {
                if kinds.contains(&(s.kind as u32)) {
                    println!("zone {zx},{zy} #{i} kind {} at {:.1}, {:.1}, {:.1}", s.kind, s.x as f64 / 65536.0, s.y as f64 / 65536.0, s.z as f64 / 65536.0);
                }
            }
        }
    }
}
