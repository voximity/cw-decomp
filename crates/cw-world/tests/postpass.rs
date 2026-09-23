//! Hand-built cases for the light pass `FUN_004d1a70` (`World::post_process_blocks`).

use cw_world::World;
use cw_world::zone::Zone;

const STONE: [u8; 4] = [10, 20, 30, 1];
const AIR: [u8; 4] = [1, 2, 3, 0];
const LAMP: [u8; 4] = [40, 50, 60, 0xd];

/// Zone (1, 1) with solid 5-block columns (height 0) over `[x0, x1] x [y0, y1]`.
fn zone_with_rock(x0: i32, y0: i32, x1: i32, y1: i32) -> Zone {
    let mut zone = Zone::new(1, 1);
    for x in x0..=x1 {
        for y in y0..=y1 {
            let col = zone.column_mut(x, y);
            col.height = 0;
            col.blocks = vec![STONE; 5];
        }
    }
    zone
}

fn run(zone: &mut Zone) {
    World::new(1).post_process_blocks(zone, 256, 256, 512, 512, 0);
}

#[test]
fn sky_and_open_cave() {
    let mut zone = Zone::new(1, 1);
    let col = zone.column_mut(300, 300);
    col.blocks = vec![STONE, AIR, STONE, AIR, [7, 8, 9, 0x82]];
    run(&mut zone);
    let b = &zone.column(300, 300).blocks;
    // Open blocks above the first solid one are sky-lit; flags are untouched.
    assert_eq!(b[4], [255, 255, 255, 0x82]);
    assert_eq!(b[3], [255, 255, 255, 0]);
    // The covered air sees the empty neighbour columns (air above h = 0): 255 * 85 / 100.
    assert_eq!(b[1], [216, 216, 216, 0]);
    // Solid blocks are never written.
    assert_eq!(b[0], STONE);
    assert_eq!(b[2], STONE);
}

#[test]
fn enclosed_cave_propagation() {
    let mut zone = zone_with_rock(299, 299, 304, 301);
    zone.column_mut(300, 300).blocks[2] = LAMP;
    zone.column_mut(301, 300).blocks[2] = AIR;
    zone.column_mut(302, 300).blocks[2] = [1, 2, 3, 0x40];
    // A sealed pair of air blocks with no light source.
    let mut zone2 = zone_with_rock(349, 349, 352, 351);
    zone2.column_mut(350, 350).blocks[2] = AIR;
    zone2.column_mut(351, 350).blocks[2] = AIR;
    run(&mut zone);
    run(&mut zone2);
    assert_eq!(zone.column(300, 300).blocks[2], LAMP);
    assert_eq!(zone.column(301, 300).blocks[2], [216, 216, 216, 0]);
    assert_eq!(zone.column(302, 300).blocks[2], [183, 183, 183, 0x40]);
    // Open neighbours contribute at least 5: 5 * 85 / 100 = 4.
    assert_eq!(zone2.column(350, 350).blocks[2], [4, 4, 4, 0]);
    assert_eq!(zone2.column(351, 350).blocks[2], [4, 4, 4, 0]);
}

#[test]
fn neighbours_outside_zone_are_dark() {
    // x = 256 is the zone's west edge; x = 255 reads as the "below" block.
    let mut zone = zone_with_rock(256, 399, 257, 401);
    zone.column_mut(256, 400).blocks[2] = AIR;
    run(&mut zone);
    assert_eq!(zone.column(256, 400).blocks[2], [0, 0, 0, 0]);
}
