//! Bit-exact checks of the base-height chain against the original Server.exe (seed 26879),
//! captured in a fresh process: climate points of the 3x3 around each sample exist, no regions.

mod common;

use common::*;
use cw_world::World;

const SEED: i32 = 26879;

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

fn check(name: &str, text: &str, f: impl Fn(&World, i32, i32) -> f32) {
    let samples = rows(text);
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let x: i32 = r[0].parse().unwrap();
            let y: i32 = r[1].parse().unwrap();
            let want = r.last().unwrap();
            ((x, y), vec![f(&world, x, y).to_bits()], vec![f32_hex(want).to_bits()])
        })
        .collect();
    check_bits(name, results);
}

#[test]
fn climate_edge_distance_matches() {
    check("edgeDist", include_str!("golden/edgeDist_00522840.txt"), |w, x, y| w.climate_edge_distance(x, y));
}

#[test]
fn height_factor_a_matches() {
    check("heightA", include_str!("golden/heightA_0052cd50.txt"), |w, x, y| w.height_factor_a(x, y));
}

#[test]
fn height_factor_b_matches() {
    check("heightB", include_str!("golden/heightB_0052d990.txt"), |w, x, y| w.height_factor_b(x, y));
}

#[test]
fn cell_falloff_matches() {
    check("cellFalloff", include_str!("golden/cellFalloff_004d19f0.txt"), |w, x, y| w.cell_falloff(x, y));
}

#[test]
fn base_height_matches() {
    check("baseHeight", include_str!("golden/baseHeight.txt"), |w, x, y| w.base_height(x, y));
}
