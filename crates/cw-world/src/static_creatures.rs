//! `FUN_00509e40(world, zone, flag)`, `Server.exe 0x00509e40`: the wandering NPC group of a
//! zone. `spawnCellNpc` (`0x0050d260`) calls it with `flag = 0` whenever the cell has no saved
//! NPC for this zone.
//!
//! Despite the module name (kept for the caller's layout), the original does not look at the
//! zone's statics: it places one leader of a random playable race at the zone centre, on top
//! of the centre column, with 0..=4 followers stacked next to it, all at level 0 in a fresh
//! world. The leader's behaviour tree ([`SpawnAi::Wanderer`]: `CombatBehavior(20.0)`,
//! `LookAtPlayerBehavior`, `WalkPathBehavior(10.0)` with a 20-point patrol path,
//! `RandomWalkBehavior`) and the followers' ([`SpawnAi::WandererFollower`]: the same with a
//! `CompanionBehavior` on the leader id before a copy of the leader's WalkPath) are recorded in
//! [`Spawn::ai`].

use crate::appearance::Item;
use crate::climate::div_trunc;
use crate::world::World;
use crate::fixed::to_block;
use crate::zone::{Spawn, SpawnAi, Zone};

/// The race list the leader is drawn from (a one-element vector `{0}` followed by twelve
/// pushes, `0x0050a3cb..0x0050adfd`).
pub const RACES: [i32; 13] = [0, 9, 0xb, 4, 7, 0xf, 2, 0xd, 0x33, 0x30, 0x4c, 0x2d, 0x2b];

/// `+0x28` of the leader from its `rand()` (`0x0050ae5e..0x0050ae7d`: `r % 2`, `neg`, `sbb`,
/// `and ~1`, `add 3`): 1 for an odd draw, 3 for an even one.
pub fn leader_f28(r: i32) -> i32 {
    if r % 2 != 0 { 1 } else { 3 }
}

/// The follower count from its `rand()` (`0x0050b319..0x0050b34a`):
/// `(int)((float)pow((double)((float)r / 32767.0f), 2.0) * 4.0f)`. Squaring a float widened to
/// double is exact, so `pow(x, 2.0)` is `x * x` here.
pub fn follower_count(r: i32) -> i32 {
    let f = f64::from(r as f32 / 32767.0f32);
    let p = (f * f) as f32;
    (p * 4.0f32) as i32
}

/// The 64-bit spawn id written at `+0x48`: `(((zy << 16) + zx) << 8) + index` in i64.
pub fn spawn_id(zx: i32, zy: i32, index: usize) -> i64 {
    (((i64::from(zy) << 16) + i64::from(zx)) << 8) + index as i64
}

impl World {
    /// `World::spawnCellNpc(zone)`, `Server.exe 0x0050d260`, at the end of `generateZone` for a
    /// named world: when the zone's cell has a saved monster (`cell+0x54`, from the `monster`
    /// blob) placed in this very zone (`cell+0x60`), one boss creature of that type and level
    /// is created at the zone centre with a weapon and an armour piece; otherwise the
    /// wandering group of [`World::spawn_static_creatures`]. The reconciliation of spawns
    /// already registered in the world creature map (`world+4`) has nothing to do during
    /// generation.
    pub fn spawn_cell_npc(&mut self, zone: &mut Zone) {
        if !self.has_name {
            return;
        }
        let (zx, zy) = (zone.x, zone.y);
        // The spawns added after generation (zone+0xa0: the previous boss or wandering group)
        // go first, with the creatures they had become (0x0050d2a8..0x0050d31f).
        let keep = zone.gen_spawn_count.min(zone.spawns.len());
        for s in zone.spawns.drain(keep..) {
            let id = s.id();
            if self.creature_ids.remove(&id) {
                self.removed_creatures.push(id);
            }
        }
        let (cx, cy) = (zx / 8, zy / 8);
        let saved = if !(0..0x2000).contains(&cx) || !(0..0x2000).contains(&cy) {
            None
        } else {
            let (rx, ry) = (cx / 8, cy / 8);
            self.region(rx, ry)
                .map(|r| r.cells[((cx & 7) * 8 + (cy & 7)) as usize])
                .filter(|cell| cell.monster.kind != 0 && cell.monster.zone_xy() == (zx, zy))
        };
        let Some(cell) = saved else {
            self.spawn_static_creatures(zone, 0);
            return;
        };

        let mut s = Spawn::NEW;
        s.f28 = 1;
        if cell.mission.f34 == 1 {
            s.appearance.flags |= 0x2000;
        }
        let (bx, by) = (zx * 256 + 0x80, zy * 256 + 0x80);
        s.x = i64::from(bx) << 16;
        s.y = i64::from(by) << 16;
        s.z = i64::from(zone.top(bx, by)) << 16;
        s.entity_type = cell.monster.kind;
        s.level = cell.monster.level;
        s.b58 = cell.monster.b5c;

        // A weapon (0x0052c4e0) then an armour piece (0x00528bf0), each with a rarity of
        // `+0x58 + 1` bumped by two long shots and capped at 4.
        let mut rarity = i32::from(s.b58) + 1;
        if self.rng.rand() % 0x14 == 0 {
            rarity += 1;
        }
        if self.rng.rand() % 0x64 == 0 {
            rarity += 1;
        }
        let item = crate::settlement::shops::random_item_52c4e0(&mut self.rng, s.level as i16, rarity.min(4) as u8, -1);
        s.inventory.add_item(item, -1);
        let mut rarity = i32::from(s.b58) + 1;
        if self.rng.rand() % 0x64 == 0 {
            rarity += 1;
        }
        if self.rng.rand() % 0x3e8 == 0 {
            rarity += 1;
        }
        let item = crate::settlement::shops::random_item_528bf0(&mut self.rng, s.level as i16, rarity.min(4) as u8, -1);
        s.inventory.add_item(item, -1);

        // 0x0050d6b4: SequentialBehavior { CombatBehavior(20.0), RandomWalkBehavior }: no rand.
        s.ai = Some(SpawnAi::CellNpc);
        self.init_appearance(&mut s);
        self.init_creature(&mut s);
        let id = spawn_id(zx, zy, zone.spawns.len());
        s.f38[4] = id as u32;
        s.f38[5] = (id >> 32) as u32;
        zone.spawns.push(s);
    }

    /// `FUN_00509e40(world, zone, flag)`, `Server.exe 0x00509e40`: see the module docs.
    pub fn spawn_static_creatures(&mut self, zone: &mut Zone, flag: i32) {
        let _ = flag; // `zone+0xa4 += flag` (0x0050a390); `zone+0xa4` is not modelled.

        // FUN_0042e880(zx, zy): the zone record must exist and have kind 0.
        let (zx, zy) = (zone.x, zone.y);
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return;
        }
        let Some(region) = self.region(div_trunc(zx, 64), div_trunc(zy, 64)) else { return };
        if region.zones[((zx % 64) * 64 + zy % 64) as usize].kind != 0 {
            return;
        }

        // Not the zone holding the world spawn (`cvttss2si` of the spawn floats, then `/ 256`).
        let sx = div_trunc(self.spawn[0] as i32, 256);
        let sy = div_trunc(self.spawn[1] as i32, 256);
        if zx == sx && zy == sy {
            return;
        }

        // A spawn from index zone+0xa0 on that already is a creature: nothing to do (0x00509f37).
        let keep = zone.gen_spawn_count.min(zone.spawns.len());
        if zone.spawns[keep..].iter().any(|s| self.creature_ids.contains(&s.id())) {
            return;
        }

        // The zone centre on top of its column (`getColumn(cx, cy, zone)`, height + count).
        let cx = zx * 256 + 0x80;
        let cy = zy * 256 + 0x80;
        let center = (i64::from(cx) << 16, i64::from(cy) << 16, i64::from(zone.top(cx, cy)) << 16);

        // A player within 100 blocks of the centre: nothing (0x0050a0d8..0x0050a1e6). Each axis
        // is the 64-bit difference rounded once to f32, times 2^-16; the squares add as
        // `dy + dx + dz`.
        const K: f32 = 1.5258789e-05;
        for p in &self.players {
            let dx = p.pos[0].wrapping_sub(center.0) as f32 * K;
            let dy = p.pos[1].wrapping_sub(center.1) as f32 * K;
            let dz = p.pos[2].wrapping_sub(center.2) as f32 * K;
            if dy * dy + dx * dx + dz * dz < 10000.0f32 {
                return;
            }
        }

        // FUN_0042e090: a climate point must exist near the centre.
        if self.nearest_climate_point(cx, cy).is_none() {
            return;
        }

        // Level: min + rand() % (max - min + 1) over the players' levels (+0x190), with the
        // original's "0 means none yet" accumulators.
        let (lo, hi) = crate::missions::player_level_range(&self.players);
        let level = lo + self.rng.rand() % (hi - lo + 1); // 0x0050a366

        // The spawns from zone+0xa0 on are dropped (0x0050a3c6): none at generation, and none
        // at play time either, since spawnCellNpc just did the same.
        zone.spawns.truncate(keep);
        let first = zone.spawns.len();

        let race = RACES[(self.rng.rand() as u32 % RACES.len() as u32) as usize]; // 0x0050ae17
        let mut leader = Spawn::NEW;
        leader.f28 = leader_f28(self.rng.rand()); // 0x0050ae5c
        leader.x = center.0;
        leader.y = center.1;
        leader.z = center.2;
        // FUN_0052bfa0(x, y, z, race): 0x0050aef6 (one rand() for the paired races).
        leader.entity_type = self.group_member_type(race);
        leader.level = level;
        leader.b58 = 0;

        // WalkPathBehavior(10.0) path (0x0050b040..0x0050b0f0): twenty random points of the
        // zone; each z starts at the centre column's `f14` (the original passes the centre
        // column, not the point's) and rises through solid blocks of the point, at most 100
        // times.
        let f14 = i64::from(zone.column(cx, cy).f14) << 16;
        let mut path = Vec::with_capacity(20);
        for _ in 0..20 {
            let y = zy * 256 + 0x10 + self.rng.rand() % 0xe0; // 0x0050b06a
            let x = zx * 256 + 0x10 + self.rng.rand() % 0xe0; // 0x0050b09c
            let mut z = f14;
            for _ in 0..100 {
                let t = zone.block(x, y, to_block(z))[3] & 0x1f;
                if t == 0 || t == 2 {
                    break;
                }
                z += 0x10000;
            }
            path.push([i64::from(x) << 16, i64::from(y) << 16, z]);
        }
        // 0x0050af25: Sequence[Combat, LookAtPlayer, WalkPath, RandomWalk].
        leader.ai = Some(SpawnAi::Wanderer { path: path.clone() });

        // +0x48 id (rewritten identically by the final loop); the followers' CompanionBehavior
        // (0x004055f0) copies it.
        let leader_id = spawn_id(zx, zy, zone.spawns.len());

        // The inventory item: type 1, sub type 1, the leader's level, everything else 0.
        let item = Item {
            item_type: 1,
            sub_type: 1,
            level: leader.level as u16,
            ..Item::NEW
        };
        let n = self.rng.rand() % 2; // 0x0050b286
        for _ in 0..n {
            leader.inventory.add_item(item, -1);
        }
        let lead = zone.spawns.len();
        zone.spawns.push(leader);

        let count = follower_count(self.rng.rand()); // 0x0050b319
        let mut k = 1;
        while k - 1 < count {
            let mut s = Spawn::NEW;
            let l = &mut zone.spawns[lead];
            s.f28 = l.f28;
            s.x = l.x;
            s.y = l.y;
            s.z = l.z;
            // The original then moves the leader, not the follower.
            l.x += i64::from(k % 2) << 16;
            l.y += i64::from(k / 2) << 16;
            let (l_level, l_b58) = (l.level, l.b58);
            s.entity_type = self.group_member_type(race); // 0x0050b450
            s.level = l_level;
            s.b58 = l_b58;
            // 0x0050b48a: Sequence[Combat, LookAtPlayer, Companion(leader +0x48), a copy of the
            // leader's WalkPath (0x004c5d10), RandomWalk].
            s.ai = Some(SpawnAi::WandererFollower { leader: leader_id, path: path.clone() });
            let n = self.rng.rand() % 2; // 0x0050b6fe
            for _ in 0..n {
                s.inventory.add_item(item, -1);
            }
            zone.spawns.push(s);
            k += 1;
        }

        // 0x0050b758: ids, appearance and equipment of every new spawn.
        for j in first..zone.spawns.len() {
            let mut s = std::mem::replace(&mut zone.spawns[j], Spawn::NEW);
            let id = spawn_id(zx, zy, j);
            s.f38[4] = id as u32;
            s.f38[5] = (id >> 32) as u32;
            self.init_appearance(&mut s);
            self.init_creature(&mut s);
            zone.spawns[j] = s;
        }
    }
}
