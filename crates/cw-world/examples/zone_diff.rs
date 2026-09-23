//! Compares a generated zone with an oracle dump (format version 6) column by column and
//! record by record.
//! Usage: cargo run --release -p cw-world --example zone_diff -- <seed> <stage> <zx,zy>... <dump.bin>
//! The zones are generated in the given order; the last one is compared with the dump at the
//! named stage (see `cw_world::generate::Stage`). Set `CW_GAME_DIR` to load the model table.

use cw_world::World;
use cw_world::generate::Stage;
use cw_world::zone::Zone;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: i32 = args[0].parse().unwrap();
    let stage = Stage::from_name(&args[1]).expect("known stage");
    let dump = std::fs::read(args.last().unwrap()).unwrap();
    let zones: Vec<(i32, i32)> = args[2..args.len() - 1]
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
    for &(zx, zy) in &zones[..zones.len() - 1] {
        world.generate_zone(zx, zy);
    }
    let (zx, zy) = *zones.last().unwrap();
    let mut snapshot: Option<Zone> = None;
    world.generate_zone_stages(zx, zy, &mut |s, z| {
        if s == stage {
            snapshot = Some(z.clone());
        }
    });
    let zone = snapshot.expect("stage reached");
    let cell = world.cell(zx.div_euclid(8), zy.div_euclid(8)).unwrap();
    println!("zone record {:?}; cell kind {} level {} id {}", zone.record, cell.kind, cell.level, cell.id);
    compare(&zone, &dump, zx, zy);
}

fn compare(zone: &Zone, dump: &[u8], zx: i32, zy: i32) {
    let rd_f32 = |o: usize| f32::from_le_bytes(dump[o..o + 4].try_into().unwrap());
    let rd_i32 = |o: usize| i32::from_le_bytes(dump[o..o + 4].try_into().unwrap());
    let rd_u32 = |o: usize| u32::from_le_bytes(dump[o..o + 4].try_into().unwrap());
    assert_eq!(rd_u32(12), 6, "dump format version");
    let mut off = 16;
    let mut shown = 0;
    let mut differing = 0;
    for i in 0..65536usize {
        let col = &zone.columns[i];
        let (a, b, c) = (rd_f32(off), rd_f32(off + 4), rd_f32(off + 8));
        let (h, f14, n) = (rd_i32(off + 12), rd_i32(off + 16), rd_i32(off + 20) as usize);
        off += 24;
        let blocks: Vec<[u8; 4]> = (0..n).map(|j| dump[off + j * 4..off + j * 4 + 4].try_into().unwrap()).collect();
        off += n * 4;
        let same = col.climate_a.to_bits() == a.to_bits()
            && col.climate_b.to_bits() == b.to_bits()
            && col.climate_c.to_bits() == c.to_bits()
            && col.height == h
            && col.f14 == f14
            && col.blocks == blocks;
        if !same {
            differing += 1;
            if shown < 8 {
                shown += 1;
                let x = zx * 256 + (i % 256) as i32;
                let y = zy * 256 + (i / 256) as i32;
                println!("column ({x},{y}) index {i}:");
                println!("  ours:   clim ({:?},{:?},{:?}) h {} f14 {} blocks {:?}", col.climate_a, col.climate_b, col.climate_c, col.height, col.f14, col.blocks);
                println!("  oracle: clim ({a:?},{b:?},{c:?}) h {h} f14 {f14} blocks {blocks:?}");
            }
        }
    }
    println!("{differing} of 65536 columns differ");
    // Entity records: compare the dump bytes of each (length-prefixed) record.
    let records = |name: &str, ours: Vec<Vec<u8>>, off: &mut usize| {
        let n = rd_u32(*off) as usize;
        *off += 4;
        let mut bad = 0;
        for i in 0..n {
            let len = rd_u32(*off) as usize;
            *off += 4;
            let r = &dump[*off..*off + len];
            *off += len;
            let same = ours.get(i).map(|o| o.as_slice() == r).unwrap_or(false);
            if !same {
                bad += 1;
                if bad <= 6 {
                    println!("  {name} #{i} oracle: {}", hex(r));
                    println!("  {name} #{i} ours:   {}", ours.get(i).map(|o| hex(o)).unwrap_or_else(|| "(missing)".into()));
                }
            }
        }
        println!("{name}: ours {} oracle {n}, {bad} differ", ours.len());
    };
    records("spawns", zone.spawns.iter().map(|s| s.dump_bytes()).collect(), &mut off);
    records("items", zone.items.iter().map(|s| s.dump_bytes()).collect(), &mut off);
    records("statics", zone.statics.iter().map(|s| s.dump_bytes()).collect(), &mut off);
    records("props", zone.props.iter().map(|s| s.dump_bytes()).collect(), &mut off);
    records("markers", zone.markers.iter().map(|s| s.dump_bytes()).collect(), &mut off);
    // Format 6 adds the dirty byte (zone+0x75) and the modified-block records (zone+0x68,
    // 20 bytes each); a freshly generated zone has neither set.
    if off + 5 <= dump.len() {
        let dirty = dump[off];
        let n = u32::from_le_bytes(dump[off + 1..off + 5].try_into().unwrap()) as usize;
        off += 5 + n * 20;
        println!("dirty {dirty}, {n} modified blocks (ours {}, {})", u8::from(zone.dirty), zone.modified.len());
    }
    assert_eq!(off, dump.len(), "dump fully consumed");
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect::<Vec<_>>().join("")
}
