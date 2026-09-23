//! Bit-exact checks of position-only helpers against the original Server.exe.
//! All golden files were captured with the world seed 26879.

mod common;

use common::*;
use cw_world::Seeds;
use cw_world::color::{color_a, color_b, color_c, color_d};
use cw_world::warp::warp_position;

const SEED: i32 = 26879;

#[test]
fn sub_seed_table_matches_the_original_for_seed_26879() {
    // `server_oracle.py seeds` read from world+0x800164 of the running original.
    let want: Vec<i32> = "26879 31446 10031 25463 4253 13936 7697 16134 1423 22278 12641 11232 26032 15693 25723 27979 32382 5102 21683 14999 19968 19318 22323 26133 10193 18785 23188 1734 24192 3926 11767 29469 25408 11271 15953 22789 3552 13145 15878 5926 8445 453 27322 15204 24182 20221 25955 19113 16416 21459 20643 13797 10427 1879 10582 8275 8485 14144 3431 23293 30158 32654 25831 14722 18540 25965 8544 9239 27120 6491 1894 2764 4280 9357 25318 28792 28081 44"
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    let seeds = Seeds::from_seed(SEED);
    // The last slot (+0x800298) is not written by load; it held 44 in the running process.
    assert_eq!(&seeds.slots()[..77], &want[..77]);
}

fn rgb_bits(c: [f32; 3]) -> Vec<u32> {
    c.iter().map(|v| v.to_bits()).collect()
}

fn check_color(name: &str, text: &str, f: impl Fn(i32, i32) -> [f32; 3]) {
    let results = rows(text)
        .into_iter()
        .map(|r| {
            let x: i32 = r[0].parse().unwrap();
            let y: i32 = r[1].parse().unwrap();
            let want = vec![f32_hex(r[2]).to_bits(), f32_hex(r[3]).to_bits(), f32_hex(r[4]).to_bits()];
            ((x, y), rgb_bits(f(x, y)), want)
        })
        .collect();
    check_bits(name, results);
}

fn f64_pair_bits(v: [f64; 2]) -> Vec<u32> {
    v.iter()
        .flat_map(|d| {
            let b = d.to_bits();
            [b as u32, (b >> 32) as u32]
        })
        .collect()
}

#[test]
fn warp_position_matches() {
    let seeds = Seeds::from_seed(SEED);
    let results = rows(include_str!("golden/warpPosition.txt"))
        .into_iter()
        .map(|r| {
            let x: i32 = r[0].parse().unwrap();
            let y: i32 = r[1].parse().unwrap();
            let got = warp_position(&seeds, x, y);
            let want = [f64_hex(r[2]), f64_hex(r[3])];
            ((x, y), f64_pair_bits(got), f64_pair_bits(want))
        })
        .collect();
    check_bits("warpPosition", results);
}

#[test]
fn color_a_matches() {
    check_color("colorA", include_str!("golden/colorA_00522320.txt"), color_a);
}

#[test]
fn color_b_matches() {
    let seeds = Seeds::from_seed(SEED);
    check_color("colorB", include_str!("golden/colorB_0052d5d0.txt"), |x, y| color_b(&seeds, x, y));
}

#[test]
fn color_c_matches() {
    let seeds = Seeds::from_seed(SEED);
    check_color("colorC", include_str!("golden/colorC_004f82d0.txt"), |x, y| color_c(&seeds, x, y));
}

#[test]
fn color_d_matches() {
    check_color("colorD", include_str!("golden/colorD_0052d870.txt"), color_d);
}
