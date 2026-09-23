//! Bit-exact checks of the climate layer against the original Server.exe (world seed 26879).

mod common;

use common::*;
use cw_world::World;

const SEED: i32 = 26879;

#[test]
fn climate_points_match_the_original() {
    // `server_oracle.py points`: rx ry x y flag A B seed elevation (A and B as f32 bit patterns).
    let mut world = World::new(SEED);
    let mut checked = 0;
    let mut bad = Vec::new();
    for r in rows(include_str!("golden/climatePoints.txt")) {
        let rx: i32 = r[0].parse().unwrap();
        let ry: i32 = r[1].parse().unwrap();
        let p = world.get_or_create_climate_point(rx, ry).unwrap();
        let got = (p.x, p.y, p.flag, p.a.to_bits(), p.b.to_bits(), p.seed, p.elevation);
        let want = (
            r[2].parse::<i32>().unwrap(),
            r[3].parse::<i32>().unwrap(),
            r[4].parse::<u8>().unwrap(),
            u32::from_str_radix(r[5], 16).unwrap(),
            u32::from_str_radix(r[6], 16).unwrap(),
            r[7].parse::<i32>().unwrap(),
            r[8].parse::<i32>().unwrap(),
        );
        if got != want {
            bad.push((rx, ry, got, want));
        }
        checked += 1;
    }
    assert!(checked >= 1000);
    assert!(bad.is_empty(), "{} of {checked} points differ; first: {:?}", bad.len(), &bad[..bad.len().min(3)]);
}

/// The oracle created the climate points of the 3x3 regions around every sample before
/// sampling; do the same so the missing-point paths are exercised identically.
fn world_with_points_for(samples: &[Vec<&str>]) -> World {
    let mut world = World::new(SEED);
    let mut regions: Vec<(i32, i32)> = samples
        .iter()
        .flat_map(|r| {
            let rx = r[0].parse::<i32>().unwrap() >> 14;
            let ry = r[1].parse::<i32>().unwrap() >> 14;
            (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (rx + dx, ry + dy)))
        })
        .filter(|&(rx, ry)| (0..1024).contains(&rx) && (0..1024).contains(&ry))
        .collect();
    regions.sort();
    regions.dedup();
    for (rx, ry) in regions {
        world.get_or_create_climate_point(rx, ry);
    }
    world
}

fn check_climate(name: &str, text: &str, f: impl Fn(&World, i32, i32) -> f32) {
    let samples = rows(text);
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let x: i32 = r[0].parse().unwrap();
            let y: i32 = r[1].parse().unwrap();
            ((x, y), vec![f(&world, x, y).to_bits()], vec![f32_hex(r[2]).to_bits()])
        })
        .collect();
    check_bits(name, results);
}

#[test]
fn climate_a_matches() {
    check_climate("climateA", include_str!("golden/climateA.txt"), |w, x, y| w.climate_a(x, y));
}

#[test]
fn climate_b_matches() {
    check_climate("climateB", include_str!("golden/climateB.txt"), |w, x, y| w.climate_b(x, y));
}

#[test]
fn climate_c_matches() {
    check_climate("climateC", include_str!("golden/climateC.txt"), |w, x, y| w.climate_c(x, y));
}
