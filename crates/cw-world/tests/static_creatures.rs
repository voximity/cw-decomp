//! Hand-derived checks of the pure helpers of `FUN_00509e40` (`static_creatures.rs`).

use cw_world::static_creatures::{RACES, follower_count, leader_f28, spawn_id};

#[test]
fn leader_f28_is_three_for_even_draws() {
    assert_eq!(leader_f28(0), 3);
    assert_eq!(leader_f28(1), 1);
    assert_eq!(leader_f28(32766), 3);
    assert_eq!(leader_f28(32767), 1);
}

#[test]
fn follower_count_is_four_times_the_squared_fraction() {
    assert_eq!(follower_count(0), 0);
    assert_eq!(follower_count(32767), 4);
    // r / 32767 = 0.5 -> 0.25 * 4 = 1.0 needs r = 16383.5; 16383 is just below.
    assert_eq!(follower_count(16383), 0);
    assert_eq!(follower_count(16384), 1);
    // sqrt(0.5) * 32767 = 23169.8; sqrt(0.75) * 32767 = 28377.1
    assert_eq!(follower_count(23169), 1);
    assert_eq!(follower_count(23170), 2);
    assert_eq!(follower_count(28377), 2);
    assert_eq!(follower_count(28378), 3);
    assert_eq!(follower_count(32766), 3);
}

#[test]
fn spawn_id_packs_zone_and_index() {
    assert_eq!(spawn_id(0, 0, 5), 5);
    assert_eq!(spawn_id(1, 0, 0), 0x100);
    assert_eq!(spawn_id(0, 1, 0), 0x100_0000);
    assert_eq!(spawn_id(32800, 32801, 7), ((32801i64 << 16) + 32800) * 256 + 7);
}

#[test]
fn race_list() {
    assert_eq!(RACES.len(), 13);
    assert_eq!(RACES[0], 0);
    assert_eq!(RACES[12], 0x2b);
}
