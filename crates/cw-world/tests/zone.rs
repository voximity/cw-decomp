//! Whole-zone comparison against stage snapshots of the original. The golden file holds
//! SHA-256 hashes of the oracle's dump format (format 6) per seed, zone and stage; the zones of
//! one seed are generated in the listed order in one world, as they were in one Server.exe
//! process, and every stage of a zone comes from that one generation.

mod common;

use std::collections::BTreeMap;

use common::{rows, serialize_zone, world_with_models};
use cw_world::generate::Stage;
use sha2::{Digest, Sha256};

#[test]
fn zone_stages_match_the_original_snapshots() {
    // Expected hashes by stage name, per zone in generation order, per seed.
    type ZoneHashes = BTreeMap<Stage, String>;
    type SeedZones = Vec<((i32, i32), ZoneHashes)>;
    let mut expected: Vec<(i32, SeedZones)> = Vec::new();
    for r in rows(include_str!("golden/zones_stages.txt")) {
        let seed: i32 = r[0].parse().unwrap();
        let key = (r[1].parse().unwrap(), r[2].parse().unwrap());
        let stage = Stage::from_name(r[3]).unwrap_or_else(|| panic!("unknown stage {}", r[3]));
        if expected.last().map(|e| e.0) != Some(seed) {
            expected.push((seed, Vec::new()));
        }
        let zones = &mut expected.last_mut().unwrap().1;
        if zones.last().map(|e| e.0) != Some(key) {
            zones.push((key, BTreeMap::new()));
        }
        zones.last_mut().unwrap().1.insert(stage, r[4].to_string());
    }
    let mut errors = Vec::new();
    let mut checked = 0;
    for (seed, zones) in &expected {
        let mut world = world_with_models(*seed);
        for ((zx, zy), stages) in zones {
            world.generate_zone_stages(*zx, *zy, &mut |stage, zone| {
                let Some(want) = stages.get(&stage) else { return };
                let bytes = serialize_zone(zone);
                let got = hex::encode(Sha256::digest(&bytes));
                checked += 1;
                if got != *want {
                    errors.push(format!("seed {seed} zone {zx},{zy} stage {stage}: {got} vs {want} ({} bytes)", bytes.len()));
                }
            });
        }
    }
    assert!(checked >= 2, "no stages checked");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// `World::fix_camp_boss_boost`: the ruler of a landscape cell (the leader of the camp at the
/// cell centre) gets the boss boost once, and nothing else in the zone changes. Zone
/// 32820,32827 of seed 26879 holds the centre of a Valley cell.
#[test]
fn camp_leader_gets_one_boss_boost() {
    use cw_world::zone::{Spawn, SpawnAi};
    let (seed, zx, zy) = (26879, 32820, 32827);
    let mut alpha = world_with_models(seed);
    let mut fixed = world_with_models(seed);
    fixed.fix_camp_boss_boost = true;
    alpha.generate_zone(zx, zy);
    fixed.generate_zone(zx, zy);
    let a = alpha.zone(zx, zy).unwrap();
    let f = fixed.zone(zx, zy).unwrap();
    let leaders: Vec<usize> = (0..a.spawns.len()).filter(|&i| matches!(a.spawns[i].ai, Some(SpawnAi::Camp { .. })) && a.spawns[i].appearance.flags & 0x200 != 0).collect();
    assert_eq!(leaders.len(), 1);
    let i = leaders[0];
    let multipliers = |s: &Spawn| (s.f_f58, s.f_f5c, s.f_f60, s.f_f64, s.f_f68);
    // The alpha's two boosts: health 100 * 15 * 15 capped at 10000, attack speed 0.75^2,
    // damage 3^2, armour and resistance 1 + 2 + 2.
    assert_eq!(multipliers(&a.spawns[i]), (10000.0, 0.5625, 9.0, 5.0, 5.0));
    assert_eq!(multipliers(&f.spawns[i]), (1500.0, 0.75, 3.0, 3.0, 3.0));
    let mut f = f.clone();
    let s = &mut f.spawns[i];
    (s.f_f58, s.f_f5c, s.f_f60, s.f_f64, s.f_f68) = multipliers(&a.spawns[i]);
    assert!(serialize_zone(&f) == serialize_zone(a), "the fix changed more than the leader's multipliers");
}
