//! Hand-derived checks of the creature tables of `populateZoneCreatures` (`Server.exe 0x005104e0`).

use cw_math::MsvcRand;
use cw_world::creatures::{creature_tables, creature_theme};

#[test]
fn theme_lists() {
    // Type 4: only theme 9.
    assert_eq!(creature_theme(4, 12345), 9);
    // Type 1: [0,1,2,3,4,5,1,7,8,6].
    let t1 = [0, 1, 2, 3, 4, 5, 1, 7, 8, 6];
    for id in 0..30 {
        assert_eq!(creature_theme(1, id), t1[id as usize % 10]);
    }
    // Types 2 and 13 drop the 6: nine entries.
    assert_eq!(creature_theme(2, 9), 0);
    assert_eq!(creature_theme(0xd, 8), 8);
    // Type 3 appends 6 and 10.
    assert_eq!(creature_theme(3, 10), 10);
    assert_eq!(creature_theme(3, 9), 6);
    // The id is reduced as unsigned: -1 = 0xffffffff, 0xffffffff % 10 = 5.
    assert_eq!(creature_theme(1, -1), 5);
}

#[test]
fn fixed_tables_use_no_rand() {
    for theme in [0, 5, 6, 7, 8, 9, 10] {
        let mut rng = MsvcRand::new(7);
        creature_tables(theme, &mut rng);
        assert_eq!(rng, MsvcRand::new(7), "theme {theme}");
    }
}

#[test]
fn theme7_keeps_leader() {
    let t = creature_tables(7, &mut MsvcRand::new(1));
    assert!(!t.has_props);
    assert_eq!(t.camp_types, vec![0x34]);
    assert_eq!(t.groups, vec![(vec![0x3e], vec![0x3e]), (vec![0x3c], vec![0x3c]), (vec![0x3c, 0x34], vec![0x34])]);
}

#[test]
fn default_theme() {
    let t = creature_tables(0, &mut MsvcRand::new(1));
    assert!(t.has_props);
    assert_eq!(t.camp_types, vec![0xb, 0xc]);
    assert_eq!(t.groups, vec![(vec![0x2e], vec![0x13, 0x21, 0x1a]), (vec![0xb, 0xc], vec![0xb, 0xc])]);
}

#[test]
fn theme1_rolls() {
    // srand(1): the first two rand() values are 41 and 18467 (MSVC), so r1 = 41 % 4 = 1 -> 0x29
    // and r2 = 18467 % 4 = 3 -> 0x60.
    let mut rng = MsvcRand::new(1);
    let t = creature_tables(1, &mut rng);
    assert_eq!(t.groups[0], (vec![0x29], vec![0x60]));
    assert_eq!(t.groups[1], (vec![0xf, 0x10], vec![0xf, 0x10]));
    assert_eq!(rng.rand(), 6334);
}
