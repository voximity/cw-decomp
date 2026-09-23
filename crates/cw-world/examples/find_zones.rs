//! Lists zones near the spawn whose cell or zone record has a special kind, to pick oracle
//! golden zones that exercise every generateZone branch.
//! Usage: cargo run --release -p cw-world --example find_zones -- <seed> [region radius]

use cw_world::World;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = args.first().map(|s| s.parse().unwrap()).unwrap_or(26879);
    let radius: i32 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(2);
    let mut world = World::new(seed);
    for rx in 512 - radius..=512 + radius {
        for ry in 512 - radius..=512 + radius {
            world.create_region(rx, ry);
        }
    }
    for rx in 512 - radius..=512 + radius {
        for ry in 512 - radius..=512 + radius {
            let region = world.region(rx, ry).unwrap();
            for cx in 0..8 {
                for cy in 0..8 {
                    let c = &region.cells[(cx * 8 + cy) as usize];
                    if c.kind != 0 {
                        let zx = (c.x / 65536 / 256) as i32;
                        let zy = (c.y / 65536 / 256) as i32;
                        println!("cell kind {:2} level {:2} id {:3} centre zone {},{} (region {},{} cell {},{})", c.kind, c.level, c.id, zx, zy, rx, ry, cx, cy);
                    }
                }
            }
            for i in 0..4096 {
                let r = &region.zones[i];
                if r.kind != 0 {
                    let zx = rx * 64 + (i as i32) / 64;
                    let zy = ry * 64 + (i as i32) % 64;
                    println!("zone record kind {} sub {} seed {} level {} at zone {},{}", r.kind, r.sub, r.seed, r.level, zx, zy);
                }
            }
        }
    }
}
