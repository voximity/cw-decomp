//! The light pass at the end of `generateZone`: `FUN_004d1a70` (`Server.exe 0x004d1a70`).
//!
//! Only "open" blocks (type 0 = air, type 2 = water, flags ignored) are touched, and only their
//! colour bytes: the fourth byte (type and flags) is never written. The colour bytes of open
//! blocks hold a light level:
//!
//! 1. Sky pass: every column is walked from the top block down; open blocks above the first
//!    non-open block get `b[1] = b[2] = 0xff` (sky light), open blocks below it get
//!    `b[1] = b[2] = 0`. `b[0]` is kept.
//! 2. Sixteen propagation rounds, each a Jacobi step over all columns: for every open block
//!    with `b[2] != 0xff`, `b[1] = m * 85 / 100` where `m` is the maximum over the six
//!    neighbours `(x-1), (x+1), (y-1), (y+1), (h-1), (h+1)` (read through `getBlock`) of:
//!    type 13 -> 255; type 0 or 2 -> `max(b[2], 5)`; anything else -> 0. Then every open
//!    block copies `b[2] = b[1]`.
//! 3. Every open block copies `b[0] = b[2]`, so an open block ends as `[L, L, L, type|flags]`.
//!
//! Neighbours are read with the original `getBlock` rules (`Zone::block`): outside the zone or
//! below the column base the "below" block (type 1, light 0); above the column top air or water
//! (light 255); a type-0 block without flag 0x40 at `h <= 0` reads as the water block (light 255).

use crate::world::World;
use crate::zone::Zone;

#[inline]
fn is_open(b: [u8; 4]) -> bool {
    let t = b[3] & 0x1f;
    t == 0 || t == 2
}

/// The light a neighbour contributes (`Server.exe 0x004d1c52` and its five inlined copies).
#[inline]
fn neighbour_light(b: [u8; 4]) -> u32 {
    let t = b[3] & 0x1f;
    if t == 0xd {
        0xff
    } else if t == 0 || t == 2 {
        u32::from(b[2].max(5))
    } else {
        0
    }
}

impl World {
    /// `FUN_004d1a70(x0, y0, x1, y1, margin, zone)`, `Server.exe 0x004d1a70`: computes the light
    /// level of the air and water blocks of `[x0, x1) x [y0, y1)`. `margin` widens the range
    /// of the sky and propagation passes (not the final copy into `b[0]`) on every side;
    /// `generateZone` passes 0. Columns outside `zone` are skipped (`getColumn` with a zone hint
    /// finds none) and read as the "below" block when they are neighbours.
    pub fn post_process_blocks(&self, zone: &mut Zone, x0: i32, y0: i32, x1: i32, y1: i32, margin: i32) {
        let (ax0, ay0, ax1, ay1) = (x0 - margin, y0 - margin, x1 + margin, y1 + margin);

        // Sky pass (0x004d1ac0).
        for x in ax0..ax1 {
            for y in ay0..ay1 {
                if !zone.contains(x, y) {
                    continue;
                }
                let col = zone.column_mut(x, y);
                let mut sky = true;
                for b in col.blocks.iter_mut().rev() {
                    if is_open(*b) {
                        let v = if sky { 0xff } else { 0 };
                        b[1] = v;
                        b[2] = v;
                    } else {
                        sky = false;
                    }
                }
            }
        }

        for _round in 0..16 {
            // Propagation (0x004d1b95): reads only b[2], writes only b[1].
            for x in ax0..ax1 {
                for y in ay0..ay1 {
                    if !zone.contains(x, y) {
                        continue;
                    }
                    let (base, n) = {
                        let col = zone.column(x, y);
                        (col.height, col.blocks.len())
                    };
                    for i in 0..n {
                        let b = zone.column(x, y).blocks[i];
                        if !is_open(b) || b[2] == 0xff {
                            continue;
                        }
                        let h = base + i as i32;
                        let mut m = 0u32;
                        for (nx, ny, nh) in [
                            (x - 1, y, h),
                            (x + 1, y, h),
                            (x, y - 1, h),
                            (x, y + 1, h),
                            (x, y, h - 1),
                            (x, y, h + 1),
                        ] {
                            m = m.max(neighbour_light(zone.block(nx, ny, nh)));
                            if m >= 0xff {
                                break;
                            }
                        }
                        // 0x004d200e: signed `m * 0x55 / 100`, m in 0..=255.
                        zone.column_mut(x, y).blocks[i][1] = (m * 0x55 / 100) as u8;
                    }
                }
            }
            // Commit (0x004d2066): b[2] = b[1].
            for x in ax0..ax1 {
                for y in ay0..ay1 {
                    if !zone.contains(x, y) {
                        continue;
                    }
                    for b in zone.column_mut(x, y).blocks.iter_mut() {
                        if is_open(*b) {
                            b[2] = b[1];
                        }
                    }
                }
            }
        }

        // Final copy over the unwidened range: b[0] = b[2].
        for x in x0..x1 {
            for y in y0..y1 {
                if !zone.contains(x, y) {
                    continue;
                }
                for b in zone.column_mut(x, y).blocks.iter_mut() {
                    if is_open(*b) {
                        b[0] = b[2];
                    }
                }
            }
        }
    }
}
