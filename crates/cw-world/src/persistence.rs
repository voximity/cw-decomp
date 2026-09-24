//! Unloading and saving at play time: the generation thread's once-a-second pass that unloads
//! zones and regions no player is near (`generationThread` `Server.exe 0x00549550`, the block
//! at 0x005497f0..0x005499a8), `World::unloadZone` (`0x004d79f0`), `World::unloadRegion`
//! (`0x004d7960`), `World::removeRegion` (`0x004d78e0`), the saves of `World::~World`
//! (`0x004cd940`), and the tick's write of the block actions it produced into their zones
//! (`World::tick` 0x0054710b..0x005471d6).
//!
//! The blob formats and the single-object savers (`World::saveZone` 0x004d81b0,
//! `World::saveEntities` 0x004d7c50) are in [`crate::save`].

use crate::save::ModifiedBlock;
use crate::world::World;
use crate::zone::Zone;

/// What one [`World::unload_idle`] pass removed, in the order it removed it, so a second copy
/// of the world (the server's generator copy) can drop the same regions and climate points.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UnloadReport {
    /// Zones unloaded (`unloadZone`), each saved first.
    pub zones: Vec<(i32, i32)>,
    /// Regions unloaded (`unloadRegion`), each after `saveEntities`.
    pub regions: Vec<(i32, i32)>,
    /// Climate points freed (`removeRegion`).
    pub points: Vec<(i32, i32)>,
}

impl UnloadReport {
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty() && self.regions.is_empty() && self.points.is_empty()
    }
}

/// `(v + (v >> 31 & 0x3f)) >> 6`: the region of a zone coordinate, truncating toward zero.
fn region_of(z: i32) -> i32 {
    z / 64
}

impl World {
    /// The keys of the loaded regions as `(rx, ry)`, in the original's table order
    /// (`world+0xbc + (rx * 1024 + ry) * 4`: `rx` outer, `ry` inner).
    ///
    /// The original walks all 1024 x 1024 slots; walking the sorted keys of the loaded ones
    /// visits the same regions in the same order.
    pub fn loaded_regions(&self) -> Vec<(i32, i32)> {
        let mut keys: Vec<u32> = self.regions.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter().map(|k| ((k / 1024) as i32, (k % 1024) as i32)).collect()
    }

    /// The zones of region `(rx, ry)` that are loaded, in the order of the region's zone table
    /// (`region+0x10018 + ((zx & 63) * 64 + (zy & 63)) * 4`: `zx` outer, `zy` inner).
    fn region_zones(&self, rx: i32, ry: i32) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for i in 0..64 {
            for j in 0..64 {
                let (zx, zy) = (rx * 64 + i, ry * 64 + j);
                if self.zone(zx, zy).is_some() {
                    out.push((zx, zy));
                }
            }
        }
        out
    }

    /// `World::unloadZone(zx, zy)`, `Server.exe 0x004d79f0`: when the zone is loaded, takes it out
    /// of its region's table, saves it (`World::saveZone`, only while `world+0xb4` is clear,
    /// which it always is for a server world) and deletes it. Returns the zone taken out.
    pub fn unload_zone(&mut self, zx: i32, zy: i32) -> Option<Box<Zone>> {
        // 0x004d7a00..0x004d7a4a: the region must exist, then its zone slot.
        self.region(region_of(zx), region_of(zy))?;
        let zone = self.remove_zone(zx, zy)?;
        // 0x004d7ab3..0x004d7ac0: saveZone; `putBlob`'s result is not checked.
        let _ = self.save_zone(&zone);
        Some(zone)
    }

    /// `World::unloadRegion(rx, ry)`, `Server.exe 0x004d7960`: when the region is loaded,
    /// `World::saveEntities` (the mission and monster blobs of its 64 cells, when its missions
    /// were activated or loaded) and then the region is deleted. Returns whether it was loaded.
    ///
    /// The region's zones are not touched: the generation pass unloads them first (a region
    /// is only unloaded three or more regions away from every player, and its zones are then
    /// all further than three zones away).
    pub fn unload_region(&mut self, rx: i32, ry: i32) -> bool {
        if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) || self.region(rx, ry).is_none() {
            return false;
        }
        // 0x004d79a1: saveEntities before the slot is cleared.
        let _ = self.save_region_entities(rx, ry);
        self.regions.remove(&((rx as u32) * 1024 + ry as u32));
        true
    }

    /// `World::removeRegion(rx, ry)`, `Server.exe 0x004d78e0`: frees the region's climate point
    /// (`world+0x4000bc + (rx * 1024 + ry) * 4`), not the region. Returns whether it existed.
    pub fn remove_region(&mut self, rx: i32, ry: i32) -> bool {
        if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) {
            return false;
        }
        self.points.remove(&((rx as u32) * 1024 + ry as u32)).is_some()
    }

    /// [`World::unload_region`] without the save: the region taken out, for the caller to save
    /// with [`crate::save::SaveTarget::save_region_entities`] (after releasing the world).
    pub fn take_region(&mut self, rx: i32, ry: i32) -> Option<Box<crate::region::Region>> {
        if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) {
            return None;
        }
        self.regions.remove(&((rx as u32) * 1024 + ry as u32))
    }

    /// Drops a region without saving it: for a copy of the world that mirrors an
    /// [`World::unload_region`] done on the served world (the copy must not write the blobs
    /// again from its possibly older cells).
    pub fn forget_region(&mut self, rx: i32, ry: i32) {
        if (0..0x400).contains(&rx) && (0..0x400).contains(&ry) {
            self.regions.remove(&((rx as u32) * 1024 + ry as u32));
        }
    }

    /// The generation thread's pass every second (`Server.exe 0x005497f0..0x005499a8`), given
    /// the zone of every player (`main` step 5, in list order):
    ///
    /// for every loaded region, `rx` outer and `ry` inner: every loaded zone of it whose squared
    /// zone distance to every player zone is `>= 9` is unloaded (`unloadZone`, which saves it);
    /// then the region is unloaded (`unloadRegion`, with `saveEntities`) unless a player's region
    /// (`zone / 64`, truncating) is within 2 on both axes, and its climate point is freed
    /// (`removeRegion`) unless a player's region is within 4 on both axes. With no players
    /// everything goes. `removeRegion` is only reached for a region that was loaded when the
    /// pass came to it, so a climate point whose region went in an earlier pass stays.
    ///
    /// The distances are the original's int arithmetic (`imul`/`add`, wrapping; the absolute
    /// value as `(v ^ s) - s`).
    pub fn unload_idle(&mut self, players: &[(i32, i32)]) -> UnloadReport {
        let mut report = UnloadReport::default();
        for (rx, ry) in self.loaded_regions() {
            // 0x00549840..0x005498a9: the zones of the region.
            for (zx, zy) in self.region_zones(rx, ry) {
                let near = players.iter().any(|&(px, py)| {
                    let (dx, dy) = (zx.wrapping_sub(px), zy.wrapping_sub(py));
                    dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy)) < 9
                });
                if !near && self.unload_zone(zx, zy).is_some() {
                    report.zones.push((zx, zy));
                }
            }
            let within = |r: i32| {
                players.iter().any(|&(px, py)| rx.wrapping_sub(region_of(px)).wrapping_abs() < r && ry.wrapping_sub(region_of(py)).wrapping_abs() < r)
            };
            // 0x005498b1..0x00549912: unloadRegion beyond two regions (or with no players).
            if !within(3) && self.unload_region(rx, ry) {
                report.regions.push((rx, ry));
            }
            // 0x0054991a..0x00549966: removeRegion beyond four regions (or with no players).
            if !within(5) && self.remove_region(rx, ry) {
                report.points.push((rx, ry));
            }
        }
        report
    }

    /// The saves of `World::~World`, `Server.exe 0x004cd940` (run when the server quits): for
    /// every loaded region, `rx` outer and `ry` inner, `saveZone` on each of its zone slots in
    /// table order and then `saveEntities` on the region. Nothing is unloaded.
    ///
    /// The writes go in one transaction ([`crate::save::SaveTarget::batch`]; the same rows).
    pub fn save_all(&self) {
        self.save_target().batch(|| {
            for (rx, ry) in self.loaded_regions() {
                for (zx, zy) in self.region_zones(rx, ry) {
                    if let Some(zone) = self.zone(zx, zy) {
                        let _ = self.save_zone(zone);
                    }
                }
                let _ = self.save_region_entities(rx, ry);
            }
        })
    }

    /// `World::tick` 0x0054710b..0x005471d6: every block action the tick produced (the output
    /// list at `out+0x18`, in order) is appended to the modified-block vector (`zone+0x68`,
    /// `Zone::pushModifiedBlock` 0x0041f4d0, a plain `push_back`) of the zone
    /// `(x / 256, y / 256)` (truncating divisions), when that zone is loaded. `saveZone` then
    /// writes the zone even when it is not dirty.
    pub fn push_block_writes(&mut self, blocks: impl IntoIterator<Item = ModifiedBlock>) {
        for b in blocks {
            if let Some(zone) = self.zone_mut(b.x / 256, b.y / 256) {
                zone.modified.push(b);
            }
        }
    }
}
