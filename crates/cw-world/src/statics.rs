//! Static entity placement helpers of `generateZone`: settling a static onto the ground
//! (`Server.exe 0x005287b0`) and the camp site test (`Server.exe 0x004e0740`).

use crate::fixed::to_block;
use crate::world::World;
use crate::zone::{Static, Zone};

/// `_ftol2` of a float: truncation toward zero.
#[inline]
fn ftol(v: f32) -> i64 {
    v as i64
}

impl World {
    /// `settleStatic(static, zone, requireFloor)`, `Server.exe 0x005287b0`: lowers the static
    /// until its footprint at its height meets a solid block, then raises it until the footprint
    /// is clear, at most fifty steps each way. Returns false when it ends at or below height
    /// zero, when `require_floor` is set and any block under the footprint is air or water, or
    /// when the block at its position is water.
    pub fn settle_static(&self, zone: &Zone, s: &mut Static, require_floor: bool) -> bool {
        let (mut sx, mut sy) = (s.scale[0], s.scale[1]);
        if s.rotation % 2 != 0 {
            std::mem::swap(&mut sx, &mut sy);
        }
        let rx = ftol(sx * 0.5 * 65536.0);
        let x_lo = ((s.x - rx) / 65536) as i32;
        let x_hi = ((s.x + rx) / 65536) as i32;
        let ry = ftol(sy * 0.5 * 65536.0);
        let y_lo = ((s.y - ry) / 65536) as i32;
        let y_hi = ((s.y + ry) / 65536) as i32;
        let solid = |b: [u8; 4]| b[3] & 0x1f != 0 && b[3] & 0x1f != 2;
        let any_solid = |zone: &Zone, h: i32| (x_lo..=x_hi).any(|x| (y_lo..=y_hi).any(|y| solid(zone.block(x, y, h))));
        for _ in 0..50 {
            if any_solid(zone, (s.z / 65536) as i32) {
                break;
            }
            s.z -= 65536;
        }
        for _ in 0..50 {
            if !any_solid(zone, (s.z / 65536) as i32) {
                break;
            }
            s.z += 65536;
        }
        if s.z <= 0 {
            return false;
        }
        if require_floor {
            let h = ((s.z - 65536) / 65536) as i32;
            for x in x_lo..=x_hi {
                for y in y_lo..=y_hi {
                    if !solid(zone.block(x, y, h)) {
                        return false;
                    }
                }
            }
        }
        let b = zone.block(to_block(s.x), to_block(s.y), to_block(s.z));
        b[3] & 0x1f != 2
    }

    /// `tryPlaceCamp(zone, pos)`, `Server.exe 0x004e0740`: on steep-enough ground, looks for a
    /// 3x3 block window where a camp fire (static 0x41) settles, then scatters four props
    /// around it. Returns whether the camp was placed.
    #[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN semantics of the original comparison
    pub fn try_place_camp(&mut self, zone: &mut Zone, pos: (i64, i64, i64)) -> bool {
        let ha = self.height_factor_a((pos.0 / 65536) as i32, (pos.1 / 65536) as i32);
        if !(1.0 - ha * 50.0 < 0.0) {
            return false;
        }
        let mut fire = Static { kind: 0x41, scale: [2.4, 2.4, 0.5], ..Static::NEW };
        fire.rotation = self.rng.rand() % 4;
        for i in 0..3i64 {
            for j in 0..3i64 {
                fire.x = pos.0 + (i << 16);
                fire.y = pos.1 + (j << 16);
                fire.z = pos.2;
                if !self.settle_static(zone, &mut fire, true) {
                    continue;
                }
                zone.statics.push(fire.clone());
                let off = ftol(229376.0);
                for i2 in (0..14i64).step_by(7) {
                    for j2 in (0..14i64).step_by(7) {
                        let mut prop = Static::NEW;
                        match self.rng.rand() % 4 {
                            1 => {
                                prop.kind = 0x10;
                                prop.scale = [1.0, 1.0, 0.5];
                            }
                            2 => {
                                prop.kind = 0xc;
                                prop.scale = [3.0, 3.0, 1.0];
                            }
                            3 => {
                                prop.kind = 0x45;
                                prop.scale = [2.0, 2.0, 0.1];
                            }
                            _ => {
                                prop.kind = 0x42;
                                prop.scale = [4.0, 4.0, 3.0];
                            }
                        }
                        prop.rotation = self.rng.rand() % 4;
                        prop.x = pos.0 + (i2 << 16) - off;
                        prop.y = pos.1 + (j2 << 16) - off;
                        prop.z = pos.2;
                        if self.settle_static(zone, &mut prop, true) {
                            zone.statics.push(prop);
                        }
                    }
                }
                return true;
            }
        }
        false
    }
}
