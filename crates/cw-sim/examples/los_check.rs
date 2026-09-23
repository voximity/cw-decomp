//! Evaluates `path::line_of_sight` (0x004d4d80) between pairs of points in a generated world.
//! Usage: `los_check <seed> <zx0,zy0> <zx1,zy1> <x,y,z> <x,y,z> [<x,y,z> <x,y,z> ...]` (blocks);
//! the zones in the square are generated first. Set `CW_GAME_DIR` for the models.
use cw_world::World;

fn p3(s: &str) -> [i64; 3] {
    let v: Vec<f64> = s.split(',').map(|x| x.parse().unwrap()).collect();
    [(v[0] * 65536.0) as i64, (v[1] * 65536.0) as i64, (v[2] * 65536.0) as i64]
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = a[0].parse().unwrap();
    let z0: Vec<i32> = a[1].split(',').map(|x| x.parse().unwrap()).collect();
    let z1: Vec<i32> = a[2].split(',').map(|x| x.parse().unwrap()).collect();
    let mut world = World::new(seed);
    if let Some(dir) = std::env::var_os("CW_GAME_DIR") {
        let mut table = Vec::new();
        cw_world::model::load_models_from_game_dir(std::path::Path::new(&dir), &mut table).expect("model table");
        world.models = std::sync::Arc::new(table);
    }
    for zx in z0[0]..=z1[0] {
        for zy in z0[1]..=z1[1] {
            world.generate_zone(zx, zy);
        }
    }
    for pair in a[3..].chunks(2) {
        let (f, t) = (p3(&pair[0]), p3(&pair[1]));
        let los = cw_sim::path::line_of_sight(&world, f, t, true, 200.0);
        let blocks = cw_sim::path::line_of_sight(&world, f, t, false, 200.0);
        println!("{} -> {}: los {los} (without statics {blocks})", pair[0], pair[1]);
    }
}
