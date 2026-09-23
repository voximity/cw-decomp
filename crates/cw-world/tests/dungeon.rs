//! Hand-derived checks of the pure `cube::Dungeon` helpers (`Server.exe 0x004f7370` and friends).

use cw_math::MsvcRand;
use cw_world::dungeon::{Dungeon, dungeon_dummy_flags, random_rarity, set_dungeon_dummy_flags};

#[test]
fn transforms_are_inverse() {
    let mut d = Dungeon::new(22, 22, 22);
    for rot in 0..4 {
        for mirror in [false, true] {
            d.rotation = rot;
            d.mirror = mirror;
            for x in 0..22 {
                for y in 0..22 {
                    let (sx, sy) = d.to_storage(x, y);
                    assert_eq!(d.to_logical(sx, sy), (x, y), "rot {rot} mirror {mirror}");
                }
            }
        }
    }
}

#[test]
fn rotation_one_maps_as_the_original() {
    // FUN_0052dde0 with rotation 1: (x, y) -> (sx - y - 1, x).
    let mut d = Dungeon::new(22, 20, 5);
    d.rotation = 1;
    assert_eq!(d.to_storage(3, 7), (22 - 7 - 1, 3));
    assert_eq!(d.dim_x(), 20);
    assert_eq!(d.dim_y(), 22);
    d.mirror = true;
    // The mirror applies after the rotation on the y axis: y -> sy - y - 1.
    assert_eq!(d.to_storage(3, 7), (14, 20 - 3 - 1));
}

#[test]
fn out_of_bounds_dummy_is_sticky() {
    set_dungeon_dummy_flags(0);
    {
        let mut d = Dungeon::new(4, 4, 4);
        assert_eq!(d.cell(-1, 0, 0), [1, 0]);
        d.set_type(-1, 0, 0, 3);
        assert_eq!(d.cell(-1, 0, 0), [1, 0], "the dummy type reads 1 again");
        d.or_flags(0, 0, 9, 4);
        assert_eq!(d.cell(5, 5, 5), [1, 4]);
    }
    assert_eq!(dungeon_dummy_flags(), 4, "the flags survive the dungeon");
    let d = Dungeon::new(4, 4, 4);
    assert_eq!(d.cell(9, 0, 0), [1, 4]);
    drop(d);
    set_dungeon_dummy_flags(0);
}

#[test]
fn line_steps_and_ramps() {
    let mut d = Dungeon::new(8, 8, 8);
    // From (0,0,0) to (3,0,2): n = 3, z = 0, 0, 1, 2 (2*i/3 truncated).
    d.line([0, 0, 0], [3, 0, 2]);
    for x in 0..4 {
        let z = 2 * x / 3;
        assert_eq!(d.cell(x, 0, z)[0], 3);
    }
    // Rising steps open the cell below and mark it as a ramp.
    assert_eq!(d.cell(2, 0, 0), [3, 1]);
    assert_eq!(d.cell(3, 0, 1), [3, 1]);
    assert_eq!(d.cell(1, 0, 0), [3, 0]);
    assert_eq!(d.cell(0, 0, 0), [3, 0]);
}

#[test]
fn line_of_length_zero_draws_nothing() {
    let mut d = Dungeon::new(4, 4, 4);
    d.line([1, 1, 1], [1, 1, 1]);
    assert!(d.cells.iter().all(|c| *c == [0, 0]));
}

#[test]
fn forced_rarity_is_tier_plus_one_capped() {
    let mut rng = MsvcRand::default();
    assert_eq!(random_rarity(&mut rng, 2, true), 3);
    assert_eq!(random_rarity(&mut rng, 7, true), 4);
}
