//! Bit-exact checks of the per-column surface helpers (seed 26879, points of the 3x3 around
//! each sample created, no regions).

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

fn rgb_bits(c: [f32; 3]) -> Vec<u32> {
    c.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn mountain_factor_matches() {
    let samples = rows(include_str!("golden/mountainFactor_00523d80.txt"));
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let (x, y) = (r[0].parse().unwrap(), r[1].parse().unwrap());
            ((x, y), vec![world.mountain_factor(x, y).to_bits()], vec![f32_hex(r[3]).to_bits()])
        })
        .collect();
    check_bits("mountainFactor", results);
}

#[test]
fn terrain_color_matches() {
    let samples = rows(include_str!("golden/terrainColor.txt"));
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let (x, y, h) = (r[0].parse().unwrap(), r[1].parse().unwrap(), r[2].parse().unwrap());
            let want = vec![f32_hex(r[4]).to_bits(), f32_hex(r[5]).to_bits(), f32_hex(r[6]).to_bits()];
            ((x, y, h), rgb_bits(world.terrain_color(x, y, h)), want)
        })
        .collect();
    check_bits("terrainColor", results);
}

#[test]
fn rock_color_matches() {
    let samples = rows(include_str!("golden/rockColor_004fae90.txt"));
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let (x, y, h) = (r[0].parse().unwrap(), r[1].parse().unwrap(), r[2].parse().unwrap());
            let want = vec![f32_hex(r[4]).to_bits(), f32_hex(r[5]).to_bits(), f32_hex(r[6]).to_bits()];
            ((x, y, h), rgb_bits(world.rock_color(x, y, h)), want)
        })
        .collect();
    check_bits("rockColor", results);
}

#[test]
fn surface_block_matches() {
    let samples = rows(include_str!("golden/surfaceBlock.txt"));
    let world = world_with_points_for(&samples);
    let results = samples
        .iter()
        .map(|r| {
            let (x, y, h) = (r[0].parse().unwrap(), r[1].parse().unwrap(), r[2].parse().unwrap());
            let (a, b) = (f32_hex(r[3]), f32_hex(r[4]));
            let want = u32::from_str_radix(r[6], 16).unwrap();
            let got = u32::from_le_bytes(world.surface_block(x, y, h, a, b));
            ((x, y, h, a, b), vec![got], vec![want])
        })
        .collect();
    check_bits("surfaceBlock", results);
}
