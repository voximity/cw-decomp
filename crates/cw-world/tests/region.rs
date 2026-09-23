//! Bit-exact check of `createRegion` against ten regions created by the original in the
//! order listed in the golden file (world seed 26879).

mod common;

use common::*;
use cw_world::World;
use cw_world::region::ZoneRecord;

const SEED: i32 = 26879;

#[test]
fn regions_match_the_original() {
    let text = include_str!("golden/regions.txt");
    let mut world = World::new(SEED);
    let mut current: Option<(i32, i32)> = None;
    let mut golden_zones: Vec<(usize, ZoneRecord)> = Vec::new();
    let mut regions_checked = 0;
    let mut errors: Vec<String> = Vec::new();

    let flush_zones = |world: &World, cur: Option<(i32, i32)>, golden: &mut Vec<(usize, ZoneRecord)>, errors: &mut Vec<String>| {
        if let Some((rx, ry)) = cur {
            let region = world.region(rx, ry).unwrap();
            let mut got: Vec<(usize, ZoneRecord)> =
                region.zones.iter().enumerate().filter(|(_, z)| **z != ZoneRecord::DEFAULT).map(|(i, z)| (i, *z)).collect();
            got.sort_by_key(|e| e.0);
            golden.sort_by_key(|e| e.0);
            if got != *golden {
                let diff: Vec<String> = got
                    .iter()
                    .filter(|g| !golden.contains(g))
                    .map(|g| format!("extra/changed {:?}", g))
                    .chain(golden.iter().filter(|g| !got.contains(g)).map(|g| format!("missing {:?}", g)))
                    .take(6)
                    .collect();
                errors.push(format!("region {rx},{ry}: zone records differ ({} vs {} non-default): {}", got.len(), golden.len(), diff.join("; ")));
            }
            golden.clear();
        }
    };

    for r in rows(text) {
        match r[0] {
            "region" => {
                flush_zones(&world, current, &mut golden_zones, &mut errors);
                let rx: i32 = r[1].parse().unwrap();
                let ry: i32 = r[2].parse().unwrap();
                world.create_region(rx, ry);
                let region = world.region(rx, ry).expect("region created");
                let want = (r[3].parse::<i32>().unwrap(), r[4].parse::<i32>().unwrap(), r[5].parse::<i32>().unwrap());
                if (region.level, region.f10, region.variant) != want {
                    errors.push(format!("region {rx},{ry}: header {:?} vs {:?}", (region.level, region.f10, region.variant), want));
                }
                current = Some((rx, ry));
                regions_checked += 1;
            }
            "cell" => {
                let (rx, ry) = current.unwrap();
                let i: usize = r[1].parse().unwrap();
                let c = world.region(rx, ry).unwrap().cells[i];
                let got = (c.x, c.y, c.radius.to_bits(), c.height.to_bits(), c.kind, c.variant, c.id, c.level, c.level_extra);
                let want = (
                    r[2].parse::<i64>().unwrap(),
                    r[3].parse::<i64>().unwrap(),
                    u32::from_str_radix(r[4], 16).unwrap(),
                    u32::from_str_radix(r[5], 16).unwrap(),
                    r[6].parse::<i32>().unwrap(),
                    r[7].parse::<i32>().unwrap(),
                    r[8].parse::<i32>().unwrap(),
                    r[9].parse::<i32>().unwrap(),
                    r[10].parse::<i32>().unwrap(),
                );
                if got != want {
                    errors.push(format!("region {rx},{ry} cell {i}: got {got:?} want {want:?}"));
                }
            }
            "zone" => {
                golden_zones.push((
                    r[1].parse().unwrap(),
                    ZoneRecord {
                        kind: r[2].parse().unwrap(),
                        sub: r[3].parse().unwrap(),
                        seed: r[4].parse().unwrap(),
                        level: r[5].parse().unwrap(),
                        byte0c: r[6].parse().unwrap(),
                    },
                ));
            }
            _ => panic!("unexpected row {r:?}"),
        }
    }
    flush_zones(&world, current, &mut golden_zones, &mut errors);
    assert!(regions_checked == 10);
    assert!(errors.is_empty(), "{} differences; first:\n{}", errors.len(), errors.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
}
