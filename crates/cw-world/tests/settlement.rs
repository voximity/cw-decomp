//! Hand-derived checks of the pure helpers of `generateSettlement` (`Server.exe 0x004e28e0`).

use cw_world::settlement::{House, HouseCell, airship_id, species_level_range};

/// Marks cell (x, y, z) of the untransformed grid with `t = 10 * x + y + 1` style tags.
fn tagged() -> House {
    let mut h = House::new(3, 3, 4);
    for z in 0..4 {
        for y in 0..3 {
            for x in 0..3 {
                let i = ((3 * z + y) * 3 + x) as usize;
                h.cells[i] = HouseCell { t: (10 * x + y + 1) as u8, h: z, ..HouseCell::default() };
            }
        }
    }
    h
}

#[test]
fn house_transform_follows_fun_004d8f90() {
    let mut h = tagged();
    // dir 0: identity.
    assert_eq!(h.cell(2, 0, 1).t, 21);
    // dir 1: (x, y) -> (X - 1 - y, x).
    h.dir = 1;
    assert_eq!(h.cell(0, 0, 0).t, 10 * 2 + 1);
    assert_eq!(h.cell(2, 1, 0).t, 12 + 1);
    // dir 2: (X - 1 - x, Y - 1 - y).
    h.dir = 2;
    assert_eq!(h.cell(0, 0, 3).t, 10 * 2 + 2 + 1);
    assert_eq!(h.cell(0, 0, 3).h, 3);
    // dir 3: (y, Y - 1 - x).
    h.dir = 3;
    assert_eq!(h.cell(0, 0, 0).t, 2 + 1);
    // Raw directions above 3 wrap with % 4 (Phase B can push them to 6).
    h.dir = 5;
    assert_eq!(h.cell(0, 0, 0).t, 10 * 2 + 1);
    // Mirror flips y after the rotation.
    h.dir = 0;
    h.mirror = true;
    assert_eq!(h.cell(1, 0, 0).t, 10 + 2 + 1);
}

#[test]
fn house_cells_outside_the_grid_are_empty() {
    let h = tagged();
    assert_eq!(h.cell(-1, 0, 0), HouseCell::default());
    assert_eq!(h.cell(0, 3, 0), HouseCell::default());
    assert_eq!(h.cell(0, 0, -1), HouseCell::default());
    assert_eq!(h.cell(0, 0, 4), HouseCell::default());
}

#[test]
fn house_logical_dims_swap_for_odd_directions() {
    let mut h = House::new(3, 2, 4);
    assert_eq!((h.dim_x(), h.dim_y(), h.dim_z()), (3, 2, 4));
    h.dir = 3;
    assert_eq!((h.dim_x(), h.dim_y()), (2, 3));
}

#[test]
fn wild_species_level_ranges() {
    // The five species of the settlement's wild-creature list.
    assert_eq!(species_level_range(0x22), (2, 4));
    assert_eq!(species_level_range(0x21), (2, 4));
    assert_eq!(species_level_range(0x1e), (3, 6));
    assert_eq!(species_level_range(0x13), (3, 6));
    assert_eq!(species_level_range(0x1a), (3, 6));
    assert_eq!(species_level_range(0x51), (0x51, 0xb4));
}

#[test]
fn airship_ids_pack_zone_and_count() {
    assert_eq!(airship_id(0x8020, 0x8010, 0), ((0x8010i64 << 16) + 0x8020) << 8);
    assert_eq!(airship_id(1, 2, 3), 0x0200_0103);
}
