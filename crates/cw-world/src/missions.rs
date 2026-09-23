//! Missions and monsters of a region at play time: `World::activateRegionMissions`
//! (`Server.exe 0x0050d8d0`), `startMission` (`0x0050c550`), `rollMonsterType`
//! (`0x0052b230`), `randomBossType` (`0x0052ae10`) and the mission discovery step of
//! `World::tick` (`0x005329fd..0x00532aa5`).
//!
//! All of this runs on the world tick, so every `rand()` here draws from the tick thread's
//! stream: the caller swaps that stream into [`World::rng`] around the call. The creatures the
//! original deletes on the way (the boss of a cell whose mission restarts, the creatures of
//! spawns `spawnCellNpc` drops) live outside this crate; their ids are queued in
//! [`World::removed_creatures`] and [`World::boss_removals`] for the server to apply.

use cw_math::MsvcRand;
use cw_math::sort::msvc_sort;

use crate::climate::div_trunc;
use crate::region::Cell;
use crate::save::MonsterState;
use crate::world::{PlayerInfo, World};

/// One record of the ServerUpdate mission section (0x38 bytes): the cell and its
/// `+0x2c..+0x54` as `activateRegionMissions` and `startMission` copy them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionRecord {
    pub cell_x: i32,
    pub cell_y: i32,
    /// `cell+0x2c..+0x40`: the word before the mission state (never written, 0), the seed,
    /// the mission type, the creature type and the level.
    pub words: [i32; 5],
    /// `cell+0x40`, `cell+0x41`: the level roll and the "found" flag.
    pub b40: u8,
    pub b41: u8,
    /// `cell+0x44..+0x54`: progress, count, the target zone (x, y).
    pub tail: [i32; 4],
}

impl Cell {
    /// `cube::Cell::falloff(&x, &y)`, `Server.exe 0x0052dee0`: `(1 - normDistance)^2`, 0 at and
    /// beyond the radius.
    pub fn falloff(&self, x: i64, y: i64) -> f32 {
        let d = 1.0f32 - self.norm_distance(x, y);
        if d <= 0.0 { 0.0 } else { d * d }
    }

    /// The mission record of this cell, whose coordinates are `cx`, `cy`.
    pub fn mission_record(&self, cx: i32, cy: i32) -> MissionRecord {
        let m = &self.mission;
        MissionRecord { cell_x: cx, cell_y: cy, words: [0, m.f30, m.f34, m.f38, m.f3c], b40: m.b40, b41: m.b41, tail: [m.f44, m.f48, m.f4c as i32, (m.f4c >> 32) as i32] }
    }
}

/// `Server.exe 0x0052ae10`: the nine boss types one is drawn from.
pub const BOSS_TYPES: [i32; 9] = [0x30, 0x33, 0x60, 0x50, 0x4c, 0x28, 0x2b, 0x2d, 0x34];

/// `Server.exe 0x0052ae10`: one of [`BOSS_TYPES`], uniformly.
pub fn random_boss_type(rng: &mut MsvcRand) -> i32 {
    BOSS_TYPES[(rng.rand() as u32 % BOSS_TYPES.len() as u32) as usize]
}

/// The lowest and highest player level with the original's "0 means none yet" accumulators
/// (`0x0050d95b..0x0050d9a6`): a level-0 player keeps both at 0.
pub fn player_level_range(players: &[PlayerInfo]) -> (i32, i32) {
    let (mut lo, mut hi) = (0, 0);
    for p in players {
        if lo == 0 || p.level < lo {
            lo = p.level;
        }
        if hi == 0 || hi < p.level {
            hi = p.level;
        }
    }
    (lo, hi)
}

/// `FUN_00428290`: whether an item is equipment (gets a level and a rarity).
pub fn is_equipment(item_type: u8, sub_type: u8) -> bool {
    !(matches!(item_type, 0 | 0xc | 0xd | 0x14 | 0x15 | 0x17 | 0x18 | 0x19) || (item_type == 0xb && sub_type != 0xe))
}

fn region_key(rx: i32, ry: i32) -> u32 {
    (rx as u32) * 1024 + ry as u32
}

impl World {
    /// `Server.exe 0x00524500`: clears every region's `+0x15a18` flag, so the next player to
    /// arrive rolls its missions again. The day rollover calls it.
    pub fn clear_region_activation(&mut self) {
        for r in self.regions.values_mut() {
            r.missions_active = false;
        }
    }

    /// The cell of a loaded region, mutable (`getCell` 0x004286f0).
    pub fn cell_mut(&mut self, cx: i32, cy: i32) -> Option<&mut Cell> {
        if !(0..0x2000).contains(&cx) || !(0..0x2000).contains(&cy) {
            return None;
        }
        let key = region_key(div_trunc(cx * 8, 64), div_trunc(cy * 8, 64));
        self.regions.get_mut(&key).map(|r| &mut r.cells[((cx % 8) * 8 + cy % 8) as usize])
    }

    /// `rollMonsterType(rx, ry, level)`, `Server.exe 0x0052b230`: a monster type for the climate
    /// point of region `(rx, ry)` and a level; `0x6c` without a climate point (and then no
    /// `rand()`). `startMission` passes cell coordinates here, which are usually out of range.
    pub fn roll_monster_type(&mut self, rx: i32, ry: i32, level: i32) -> i32 {
        let Some(p) = self.climate_point(rx, ry) else { return 0x6c };
        let (b, flag) = (p.b, p.flag);
        let mut list = vec![if b > 0.2f32 { 0x6c } else { 0x72 }, if b > 0.2f32 { 0x77 } else { 0x74 }];
        if level > 10 {
            list.push(0x73);
        }
        if level > 20 {
            list.push(0x75);
        }
        if level > 50 {
            list.push(if 0.2f32 > b {
                0x71
            } else if flag == 1 {
                0x70
            } else {
                0x6f
            });
            list.push(0x6d);
        }
        if level > 70 {
            list.push(0x6e);
        }
        list[(self.rng.rand() as u32 % list.len() as u32) as usize]
    }

    /// `activateRegionMissions(rx, ry, out)`, `Server.exe 0x0050d8d0`: the tick calls it for the
    /// region of the climate point nearest to a player when the region's `+0x15a18` flag is
    /// clear. For every cell whose centre maps to this region it announces and clears a running
    /// mission, clears the saved monster, rolls a monster for an empty cell (level from the
    /// climate point, type from [`World::roll_monster_type`], zone `cell origin + 2 + rand % 4`),
    /// and sorts the other cells into two candidate lists by level. Two missions are then
    /// started from the candidates and up to two on the new monsters' cells, `spawnCellNpc`
    /// runs on every loaded zone of the region, and the flag is set.
    pub fn activate_region_missions(&mut self, rx: i32, ry: i32, out: &mut Vec<MissionRecord>) {
        if !self.has_name || self.region(rx, ry).is_none() {
            return;
        }
        let key = region_key(rx, ry);
        let (_, hi) = player_level_range(&self.players);
        let mut list_a: Vec<(i32, i32)> = Vec::new();
        let mut list_b: Vec<(i32, i32)> = Vec::new();
        let mut list_c: Vec<(i32, i32)> = Vec::new();
        for i in 0..8 {
            let cx = rx * 8 + i;
            for j in 0..8 {
                let cy = ry * 8 + j;
                let idx = ((cx & 7) * 8 + (cy & 7)) as usize;
                // FUN_004feec0: the cell centre (16.16 fixed, `__alldiv` to blocks) must map to
                // this region's climate point.
                let (bx, by) = {
                    let c = &self.regions[&key].cells[idx];
                    ((c.x / 65536) as i32, (c.y / 65536) as i32)
                };
                if self.nearest_climate_region(bx, by) != (rx, ry) {
                    continue;
                }
                {
                    let cell = &mut self.regions.get_mut(&key).expect("region").cells[idx];
                    if cell.mission.f34 != 0 {
                        cell.mission.f34 = 0;
                        cell.mission.f44 = 0;
                        out.push(cell.mission_record(cx, cy));
                    }
                    if cell.monster.kind != 0 {
                        cell.monster.kind = 0;
                        cell.monster.level = 0;
                        cell.monster.b5c = 0;
                    }
                }
                let cell = self.regions[&key].cells[idx];
                if cell.kind == 0 {
                    let (a, b, flag) = {
                        let p = self.climate_point(rx, ry).expect("a created region has a climate point");
                        (p.a, p.b, p.flag)
                    };
                    let (mut lo_r, mut hi_r) = (1, 10);
                    if 0.2f32 > b {
                        (lo_r, hi_r) = (10, 20);
                    }
                    if 0.2f32 > a && b > 0.8f32 {
                        (lo_r, hi_r) = (15, 25);
                    }
                    if a > 0.8f32 && b > 0.8f32 {
                        (lo_r, hi_r) = (10, 20);
                    }
                    if flag == 1 {
                        (lo_r, hi_r) = (20, 30);
                    }
                    let level = self.rng.rand() % (hi_r - lo_r + 1) + lo_r;
                    let kind = self.roll_monster_type(rx, ry, level);
                    let zy = cy * 8 + 2 + self.rng.rand() % 4;
                    let zx = cx * 8 + 2 + self.rng.rand() % 4;
                    let cell = &mut self.regions.get_mut(&key).expect("region").cells[idx];
                    cell.monster.b5c = 0;
                    cell.monster.level = level;
                    cell.monster.kind = kind;
                    cell.monster.zone = MonsterState::pack_zone(zx, zy);
                    list_c.push((cx, cy));
                }
                if cell.kind != 0 && cell.kind != 10 && cell.level <= hi + 2 && (cell.kind != 1 || self.rng.rand() % 50 == 0) {
                    if hi - 2 <= cell.level {
                        list_b.push((cx, cy));
                    } else {
                        list_a.push((cx, cy));
                    }
                }
            }
        }
        // Two missions: a one-in-six chance of a low-level cell, else a cell near the players'
        // level, else a low-level cell.
        for _ in 0..2 {
            let r = self.rng.rand();
            let pick = if r % 6 == 0 && !list_a.is_empty() {
                Some(&mut list_a)
            } else if !list_b.is_empty() {
                Some(&mut list_b)
            } else if !list_a.is_empty() {
                Some(&mut list_a)
            } else {
                None
            };
            if let Some(list) = pick {
                let k = (self.rng.rand() as u32 % list.len() as u32) as usize;
                let (cx, cy) = list.remove(k);
                self.start_mission(cx, cy, out);
            }
        }
        // Up to two more on the cells that just got a monster.
        for _ in 0..2 {
            if list_c.is_empty() {
                break;
            }
            let k = (self.rng.rand() as u32 % list_c.len() as u32) as usize;
            let (cx, cy) = list_c.remove(k);
            self.start_mission(cx, cy, out);
        }
        // spawnCellNpc on every loaded zone of the region, in zone index order.
        for lx in 0..64 {
            for ly in 0..64 {
                let (zx, zy) = (rx * 64 + lx, ry * 64 + ly);
                if let Some(mut zone) = self.remove_zone(zx, zy) {
                    self.spawn_cell_npc(&mut zone);
                    self.insert_zone(zone);
                }
            }
        }
        self.regions.get_mut(&key).expect("region").missions_active = true;
    }

    /// `startMission(cx, cy, out)`, `Server.exe 0x0050c550`: deletes the cell's boss creatures
    /// and the spawns its loaded zones gained after generation, clears the mission state, rolls
    /// a mission type from the cell kind and fills the state: seed, creature type, the players'
    /// highest level, a count, and either no zone, the saved monster's zone, or the zone of the
    /// cell nearest to its centre (weighted by the cell's falloff, MSVC `std::sort` order).
    pub fn start_mission(&mut self, cx: i32, cy: i32, out: &mut Vec<MissionRecord>) {
        if !self.has_name {
            return;
        }
        // Boss creatures (appearance flag 0x2000) whose home zone lies in this cell.
        self.boss_removals.push((cx, cy));
        // The loaded zones of the cell lose the spawns added after generation (zone+0xa0).
        for zx in cx * 8..cx * 8 + 8 {
            for zy in cy * 8..cy * 8 + 8 {
                if let Some(zone) = self.zone_mut(zx, zy) {
                    let n = zone.gen_spawn_count.min(zone.spawns.len());
                    zone.spawns.truncate(n);
                }
            }
        }
        let Some(cell) = self.cell_mut(cx, cy) else { return };
        cell.mission.f34 = 0;
        cell.mission.b41 = 0;
        cell.mission.f44 = 0;
        cell.mission.f48 = 0;
        let cell = *cell;
        if cell.kind == 10 {
            return;
        }
        let types: &[i32] = match cell.kind {
            0xe => &[5],
            1 => &[9, 3, 4],
            0 => &[1],
            _ => &[5],
        };
        let ty = types[(self.rng.rand() as u32 % types.len() as u32) as usize];
        let (_, hi) = player_level_range(&self.players);
        let creature_type: i32;
        let mut count: Option<i32> = None;
        let zone: i64;
        match ty {
            7 => {
                creature_type = random_boss_type(&mut self.rng);
                count = Some(self.rng.rand() % 8 + 25);
                zone = -1;
            }
            10 => {
                creature_type = random_boss_type(&mut self.rng);
                count = Some(self.rng.rand() % 5 + 10);
                zone = -1;
            }
            11 => {
                creature_type = random_boss_type(&mut self.rng);
                count = Some(self.rng.rand() % 5 + 15);
                zone = -1;
            }
            1 => {
                creature_type = cell.monster.kind;
                zone = cell.monster.zone;
            }
            _ => {
                // Every zone of the cell inside its radius, weighted by the falloff at the zone
                // centre; the odd diagonal is doubled for types 12 and 13.
                let mut list: Vec<(i32, i32, f32)> = Vec::new();
                for zx in cx * 8..cx * 8 + 8 {
                    let bx = i64::from(zx * 256 + 0x80) << 16;
                    for zy in cy * 8..cy * 8 + 8 {
                        let by = i64::from(zy * 256 + 0x80) << 16;
                        let w = 1.0f32 - cell.norm_distance(bx, by);
                        if 0.0 < w {
                            let mut w2 = w * w;
                            if 0.0 < w2 {
                                if (ty == 12 || ty == 13) && (zx + zy) % 2 != 0 {
                                    w2 *= 2.0;
                                }
                                list.push((zx, zy, w2));
                            }
                        }
                    }
                }
                if list.is_empty() {
                    return;
                }
                msvc_sort(&mut list, |a, b| a.2 > b.2);
                let (zx, zy) = (list[0].0, list[0].1);
                match ty {
                    3 => creature_type = self.roll_monster_type(cx, cy, hi),
                    2 | 4 => creature_type = self.pick_creature_type(zx * 256 + 0x80, zy * 256 + 0x80, cell.height as i32, false),
                    12 | 13 => {
                        creature_type = self.pick_creature_type(zx * 256 + 0x80, zy * 256 + 0x80, cell.height as i32, true);
                        if ty == 12 {
                            count = Some(self.rng.rand() % 5 + 6);
                        }
                    }
                    _ => {
                        creature_type = random_boss_type(&mut self.rng);
                        count = Some(self.rng.rand() % 10 + 10);
                    }
                }
                zone = MonsterState::pack_zone(zx, zy);
            }
        }
        let seed = self.rng.rand();
        let cell = self.cell_mut(cx, cy).expect("cell");
        cell.mission.f30 = seed;
        cell.mission.b40 = cell.level_extra as u8;
        cell.mission.f3c = hi;
        cell.mission.f34 = ty;
        cell.mission.f38 = creature_type;
        if let Some(c) = count {
            cell.mission.f48 = c;
        }
        cell.mission.f4c = zone;
        out.push(cell.mission_record(cx, cy));
    }

    /// The mission discovery step of `World::tick` (`0x005329fd..0x00532aa5`) for one player
    /// standing in cell `(cx, cy)` at block position `(x, y)` (16.16 fixed): a running mission
    /// not yet found becomes found when the player is inside the cell's radius, and its record
    /// goes out.
    pub fn discover_mission(&mut self, cx: i32, cy: i32, x: i64, y: i64) -> Option<MissionRecord> {
        let cell = self.cell_mut(cx, cy)?;
        if cell.mission.f34 == 0 || cell.mission.b41 != 0 || cell.falloff(x, y) <= 0.0 {
            return None;
        }
        cell.mission.b41 = 1;
        Some(cell.mission_record(cx, cy))
    }
}
