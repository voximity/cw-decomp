//! Prints the spawns of a generated zone: index, id, type, hostile byte, level, class,
//! appearance flags and position. Usage:
//! `cargo run --release -p cw-world --example spawn_list -- <seed> <zx,zy>...` (the zones are
//! generated in the given order; the last one is printed). Set `CW_GAME_DIR` for the models.
use cw_world::World;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = args[0].parse().unwrap();
    let zones: Vec<(i32, i32)> = args[1..]
        .iter()
        .map(|s| {
            let mut it = s.split(',');
            (it.next().unwrap().parse().unwrap(), it.next().unwrap().parse().unwrap())
        })
        .collect();
    let mut world = World::new(seed);
    if let Some(dir) = std::env::var_os("CW_GAME_DIR") {
        let mut table = Vec::new();
        cw_world::model::load_models_from_game_dir(std::path::Path::new(&dir), &mut table).expect("model table");
        world.models = std::sync::Arc::new(table);
    } else {
        eprintln!("CW_GAME_DIR not set: generating without models");
    }
    for &(zx, zy) in &zones {
        world.generate_zone(zx, zy);
    }
    let (zx, zy) = *zones.last().unwrap();
    let zone = world.zone(zx, zy).expect("zone");
    println!("zone {zx},{zy}: {} spawns ({} at generation)", zone.spawns.len(), zone.gen_spawn_count);
    for (i, s) in zone.spawns.iter().enumerate() {
        println!(
            "  [{i}] id {} type {:#x} ({}) hostile {} level {} b58 {} flags {:#06x} at {:.1}, {:.1}, {:.1} class {} rot {} f38 {:?} ai {}",
            s.id(),
            s.entity_type,
            s.entity_type,
            s.f28,
            s.level,
            s.b58,
            s.appearance.flags,
            s.x as f64 / 65536.0,
            s.y as f64 / 65536.0,
            s.z as f64 / 65536.0,
            s.f30,
            s.rotation,
            s.f38,
            match &s.ai {
                None => "default".to_string(),
                Some(a) => format!("{a:?}").chars().take(70).collect::<String>(),
            }
        );
    }
}
