//! `cube::Zone`: 256x256 block columns plus the entities generated with them.

use crate::appearance::{Appearance, Item};
use crate::inventory::Inventory;
use crate::climate::div_trunc;
use crate::region::ZoneRecord;
use crate::surface::Block;
use crate::world::World;

/// Default blocks the original keeps in static storage (read from the running process).
pub const WATER_BLOCK: Block = [255, 255, 255, 0x82];
pub const AIR_BLOCK: Block = [255, 255, 255, 0];
pub const BELOW_BLOCK: Block = [200, 200, 200, 1];

/// One block column, 32 bytes in the original (`zone+0xa8`, index `x + y * 256`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Column {
    /// `+4`
    pub climate_a: f32,
    /// `+8`
    pub climate_b: f32,
    /// `+0xc`
    pub climate_c: f32,
    /// `+0x10`: height of `blocks[0]`.
    pub height: i32,
    /// `+0x14`
    pub f14: i32,
    /// `+0x18`/`+0x1c`: the block array, bottom first. (Grown block by block during the
    /// terrain pass as the original's vector is; pre-sizing it from the column height would
    /// save the regrowth without changing the contents.)
    pub blocks: Vec<Block>,
}

impl Column {
    /// `cube::Column::resize(n, shift)`, `Server.exe 0x00413420`: the array becomes `n` blocks;
    /// `shift` old blocks' worth of zeroed blocks are inserted at the bottom when growing, and
    /// the base height moves down by `shift`.
    pub fn resize(&mut self, n: i32, shift: i32) {
        if n == self.blocks.len() as i32 {
            return;
        }
        if n <= 0 {
            self.blocks.clear();
            self.height -= shift;
            return;
        }
        let mut new = vec![[0u8; 4]; n as usize];
        let old = self.blocks.len() as i32;
        if n < old {
            new.copy_from_slice(&self.blocks[..n as usize]);
        } else {
            new[shift as usize..shift as usize + old as usize].copy_from_slice(&self.blocks);
        }
        self.blocks = new;
        self.height -= shift;
    }

    /// `FUN_0041fe60(index, block)`: writes a block at `height + index`, growing the array as
    /// needed. A block carrying the 0x80 flag is only replaced by a block whose type is not 0,
    /// and keeps the flag.
    pub fn set_raw(&mut self, index: i32, block: Block) {
        let i;
        if index < 0 {
            self.resize(self.blocks.len() as i32 - index, -index);
            i = 0usize;
        } else {
            if self.blocks.len() as i32 <= index {
                self.resize(index + 1, 0);
            }
            i = index as usize;
            if self.blocks[i][3] & 0x80 != 0 {
                if block[3] & 0x1f == 0 {
                    return;
                }
                self.blocks[i] = block;
                self.blocks[i][3] |= 0x80;
                return;
            }
        }
        self.blocks[i] = block;
    }

    /// `cube::Column::blockAt(index)`, `Server.exe 0x00405f20`, for indexes inside the array.
    pub fn block_at(&self, index: i32) -> Block {
        if index < 0 {
            [0, 0, 0, 1]
        } else if index >= self.blocks.len() as i32 {
            [255, 255, 255, 0]
        } else {
            self.blocks[index as usize]
        }
    }
}

/// A ground item record (0x148 bytes in the original, `zone+0x30` vector);
/// `cube::GroundItem::GroundItem` is `Server.exe 0x0041d8d0`.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundItem {
    /// `+0`: the 0x118-byte item.
    pub item: Item,
    /// `+0x118`, `+0x120`, `+0x128`: 16.16 fixed blocks.
    pub x: i64,
    pub y: i64,
    pub z: i64,
    /// `+0x130`
    pub rotation: f32,
    /// `+0x134`: 0x3d924925 from the constructor, 0.1 when placed by the generator.
    pub f134: f32,
    /// `+0x138`: 0 from the constructor; bit 0 set keeps the item from being dropped to the
    /// ground at the end of generateZone.
    pub b138: u8,
    /// `+0x13c`: a countdown the world tick decrements (0 from the constructor).
    pub f13c: i32,
    /// `+0x140`: a second countdown (0 from the constructor).
    pub f140: i32,
    /// `+0x144`: the day the item was dropped, -1 (permanent) from the constructor; the save
    /// loader drops items older than three days.
    pub f144: i32,
}

impl GroundItem {
    /// `cube::GroundItem::GroundItem`, `Server.exe 0x0041d8d0` (`+0x130` is not written by the
    /// constructor; 0 here).
    pub const NEW: GroundItem = GroundItem { item: Item::NEW, x: 0, y: 0, z: 0, rotation: 0.0, f134: f32::from_bits(0x3d92_4925), b138: 0, f13c: 0, f140: 0, f144: -1 };
}

/// The oracle dump projection of an item (format 5, 275 bytes): the fields the constructor
/// initialises (`+2`, `+3`, `+0xf`, `+0x12`, `+0x13` are uninitialised in the original).
pub fn item_dump(it: &Item) -> Vec<u8> {
    let mut out = Vec::with_capacity(275);
    out.push(it.item_type);
    out.push(it.sub_type);
    out.extend_from_slice(&it.modifier.to_le_bytes());
    out.extend_from_slice(&it.f8.to_le_bytes());
    out.push(it.rarity);
    out.push(it.material);
    out.push(it.flags);
    out.extend_from_slice(&it.level.to_le_bytes());
    out.extend_from_slice(&it.spirits);
    out.extend_from_slice(&it.num_spirits.to_le_bytes());
    out
}

/// The oracle dump of an inventory (format 5): gold, the second currency, then the pages.
pub fn inventory_dump(inv: &Inventory) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&inv.gold.to_le_bytes());
    out.extend_from_slice(&inv.f12c.to_le_bytes());
    out.extend_from_slice(&(inv.pages.len() as u32).to_le_bytes());
    for page in &inv.pages {
        out.extend_from_slice(&(page.len() as u32).to_le_bytes());
        for slot in page {
            out.extend_from_slice(&slot.count.to_le_bytes());
            out.extend_from_slice(&item_dump(&slot.item));
        }
    }
    out
}

impl GroundItem {
    /// The oracle dump record (format 6: format 5 plus `+0x13c`, `+0x140`, `+0x144`).
    pub fn dump_bytes(&self) -> Vec<u8> {
        let mut out = item_dump(&self.item);
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        out.extend_from_slice(&self.rotation.to_le_bytes());
        out.extend_from_slice(&self.f134.to_le_bytes());
        out.push(self.b138);
        out.extend_from_slice(&self.f13c.to_le_bytes());
        out.extend_from_slice(&self.f140.to_le_bytes());
        out.extend_from_slice(&self.f144.to_le_bytes());
        out
    }
}

/// A marker record (0x140 bytes, `zone+0x48` vector; constructor `Server.exe 0x004f7490`):
/// dungeon entrances (5) and bosses (6), loot piles (9) and creature groups (10).
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// `+0`
    pub kind: i32,
    /// `+4`, `+5`: item type and sub-type of a loot marker.
    pub b4: u8,
    pub b5: u8,
    /// `+0x11`: item material of a loot marker.
    pub b11: u8,
    /// `+0x11c`: spawn index (-1 from the constructor).
    pub spawn_index: i32,
    /// `+0x120`: creature type (-1 from the constructor).
    pub creature_type: i32,
    /// `+0x128`, `+0x130`, `+0x138`: 16.16 fixed blocks.
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl Marker {
    pub const NEW: Marker = Marker { kind: 0, b4: 0, b5: 0, b11: 0, spawn_index: -1, creature_type: -1, x: 0, y: 0, z: 0 };

    /// The oracle dump record (format 5, 303 bytes): `+0` u32, `+4` 2 bytes, `+0x10` u16 (0) and
    /// `+0x11` u8, `+0x14` u16 (1), `+0x18` the 256-byte name block (zero), `+0x118` u32 (0),
    /// `+0x11c`, `+0x120`, `+0x128` position.
    pub fn dump_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(303);
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.push(self.b4);
        out.push(self.b5);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.push(self.b11);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&[0u8; 256]);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.spawn_index.to_le_bytes());
        out.extend_from_slice(&self.creature_type.to_le_bytes());
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        out
    }
}

/// The AI tree a zone generator hangs on a spawn (`spawn+0x109c`, a `Behavior*`; the tick clones
/// it through vtable slot 1 into `creature+0x13e4` at 0x00535fc8, and builds its default list
/// when it is null). One variant per tree shape the generators build; every node is listed in
/// the order it is pushed onto the `SequentialBehavior` (ctor `Server.exe 0x0041cfc0`, children
/// through `FUN_004d6620` / `FUN_0052dfb0`).
///
/// Node constructors: `CombatBehavior(float)` 0x004029e0 (always `20.0` in the generators),
/// `LookAtPlayerBehavior` 0x00414720, `WalkPathBehavior(float radius)` 0x004c5d50 (waypoints
/// pushed with `FUN_004e1420`, `vector<vec3<i64>>::push_back`, 16.16 fixed), its copy
/// constructor 0x004c5d10, `CompanionBehavior` 0x004055d0 (leader id written at `+8` after) and
/// 0x004055f0 (leader id as arguments), `SpawnLocationBehavior` 0x00428920,
/// `RandomInteractionBehavior` 0x0041ba60, `RandomWalkBehavior` 0x0041cb90.
///
/// A leader id is the 64-bit value the `CompanionBehavior` holds at `+8`: the leader spawn's
/// `+0x48` ([`Spawn::id`]) as it was when the follower's tree was built. The tick creates a
/// creature with that same `+0x48` as its id (0x00535d95) and `CompanionBehavior::update`
/// (0x004057b9) looks the leader up by it in the world creature map.
#[derive(Debug, Clone, PartialEq)]
pub enum SpawnAi {
    /// `Sequence[LookAtPlayer]`: the airship pilot (`generateSettlement`, sequence at
    /// 0x004ee214).
    Pilot,
    /// `Sequence[Combat(20.0), LookAtPlayer, WalkPath(2.0, path)]` with `path` = the spawn
    /// position: the town house NPCs of classes 1..5 (`generateSettlement`, sequences at
    /// 0x004f0406, 0x004f05e2, 0x004f07be, 0x004f099a, 0x004f0b76) and the `placeModel` NPCs of
    /// kinds 6..15 (sequence at 0x00527fb2).
    Sentry { path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), LookAtPlayer, SpawnLocation, RandomInteraction, RandomWalk]`: a
    /// town villager (`generateSettlement`, sequence at 0x004f14dd). `schedule` is the spawn's
    /// `+0x10a0` vector the `SpawnLocationBehavior` walks (the tick copies it to
    /// `creature+0x148c`).
    Villager { schedule: Vec<crate::settlement::ScheduleEntry> },
    /// A bare `CombatBehavior(20.0)` as the root, no sequence: the dungeon boss
    /// (`generateDungeon`, 0x00507a42).
    Combat,
    /// `Sequence[Combat(20.0), WalkPath(2.0, path)]`:
    /// - dungeon patrol leader (`generateDungeon`, 0x00508c7d): four room corners;
    /// - dungeon ring monsters (0x00509714, 0x00509b87): the spawn position;
    /// - type-5 cell boss (`generateSettlement`, 0x004f1e19) and ring guards (0x004f2532): the
    ///   spawn position;
    /// - type-5 cell group guards (0x004f2879): the spawn position, then a random floor spot of
    ///   a random house when that house has one.
    Patrol { path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), Companion(leader), WalkPath(2.0, path)]` with `path` = the
    /// leader's position: a dungeon group follower (`generateDungeon`, 0x005092c2). The leader's
    /// `+0x48` was set to its final id (`spawnId(zx, zy, index)`) before the followers.
    DungeonFollower { leader: i64, path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), WalkPath(2.0, path), RandomInteraction, RandomWalk]` with `path`
    /// = the spawn position: a camp member (`populateZoneCreatures`, 0x00513079).
    Camp { path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), WalkPath(2.0, path), RandomWalk]`: a roaming group leader
    /// (`populateZoneCreatures`, 0x005128e3); `path` = its position, then up to three
    /// neighbouring points raised to the first air or water block.
    GroupLeader { path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), Companion(leader), RandomWalk]`: a roaming group follower
    /// (`populateZoneCreatures`, 0x00512c9d). The original copies the leader's `+0x48` before
    /// anything has written it (the constructor zeroes it and the ids are assigned later by
    /// generateZone), so `leader` is always 0.
    GroupFollower { leader: i64 },
    /// `Sequence[Combat(20.0), RandomWalk]`: the saved cell monster (`spawnCellNpc`,
    /// 0x0050d6b4).
    CellNpc,
    /// `Sequence[Combat(20.0), LookAtPlayer, WalkPath(10.0, path), RandomWalk]`: the wandering
    /// group leader (`FUN_00509e40`, 0x0050af25); `path` = twenty random points of the zone.
    Wanderer { path: Vec<[i64; 3]> },
    /// `Sequence[Combat(20.0), LookAtPlayer, Companion(leader), WalkPath(10.0, path),
    /// RandomWalk]`: a wandering group follower (`FUN_00509e40`, 0x0050b48a). The WalkPath is a
    /// copy (0x004c5d10) of the leader's; `leader` is the leader's final id, written to its
    /// `+0x48` just before.
    WandererFollower { leader: i64, path: Vec<[i64; 3]> },
}

impl SpawnAi {
    /// The `CombatBehavior` range: 20.0 wherever the generators build one.
    pub const COMBAT_RANGE: f32 = 20.0;

    /// The `WalkPathBehavior` radius (`+0x18`) of the tree, if it has one.
    pub fn walk_radius(&self) -> Option<f32> {
        match self {
            SpawnAi::Wanderer { .. } | SpawnAi::WandererFollower { .. } => Some(10.0),
            SpawnAi::Sentry { .. } | SpawnAi::Patrol { .. } | SpawnAi::DungeonFollower { .. } | SpawnAi::Camp { .. } | SpawnAi::GroupLeader { .. } => Some(2.0),
            SpawnAi::Pilot | SpawnAi::Villager { .. } | SpawnAi::Combat | SpawnAi::GroupFollower { .. } | SpawnAi::CellNpc => None,
        }
    }

    /// The `WalkPathBehavior` waypoints of the tree, if it has one.
    pub fn walk_path(&self) -> Option<&[[i64; 3]]> {
        match self {
            SpawnAi::Sentry { path }
            | SpawnAi::Patrol { path }
            | SpawnAi::DungeonFollower { path, .. }
            | SpawnAi::Camp { path }
            | SpawnAi::GroupLeader { path }
            | SpawnAi::Wanderer { path }
            | SpawnAi::WandererFollower { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The `CompanionBehavior` leader id of the tree, if it has one.
    pub fn leader(&self) -> Option<i64> {
        match self {
            SpawnAi::DungeonFollower { leader, .. } | SpawnAi::GroupFollower { leader } | SpawnAi::WandererFollower { leader, .. } => Some(*leader),
            _ => None,
        }
    }
}

/// A creature spawn record (`cube::Spawn`, 0x10f0 bytes, pointers in the `zone+0x18` vector).
/// Field names are the byte offsets in the original; the constructor (`Server.exe 0x004e0f40`)
/// leaves the position uninitialised and the generator always writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    /// `+0x10`, `+0x18`, `+0x20`: 16.16 fixed blocks.
    pub x: i64,
    pub y: i64,
    pub z: i64,
    /// `+0x28`: 1 from the constructor.
    pub f28: i32,
    /// `+0x2c`: entity type, 4 from the constructor.
    pub entity_type: i32,
    /// `+0x30`
    pub f30: u16,
    /// `+0x34`: level, 1 from the constructor.
    pub level: i32,
    /// `+0x38`..`+0x50`
    pub f38: [u32; 6],
    /// `+0x50`
    pub b50: u8,
    /// `+0x54`: rotation in degrees.
    pub rotation: f32,
    /// `+0x58`
    pub b58: u8,
    /// `+0x08`: the activation radius in blocks, 200.0 from the constructor (512.0 for an
    /// airship pilot); the tick makes the creature when a player is within it.
    pub radius: f32,
    /// `+0x5c`: the AI mode (3 or 4 for settlement villagers), 0 from the constructor.
    pub f5c: i32,
    /// `+0x60`, `+0x64`: the target zone of a villager, -1 from the constructor.
    pub f60: i32,
    pub f64: i32,
    /// `+0x74`: the 0xac-byte appearance block (its `flags` field is `+0x7a`).
    pub appearance: Appearance,
    /// `+0x120`: thirteen 0x118-byte equipment slots.
    pub equipment: [Item; 13],
    /// `+0xf58`: 100.0 from the constructor.
    pub f_f58: f32,
    /// `+0xf5c`, `+0xf60`, `+0xf64`, `+0xf68`: 1.0 from the constructor.
    pub f_f5c: f32,
    pub f_f60: f32,
    pub f_f64: f32,
    pub f_f68: f32,
    /// `+0xf6c`: the inventory.
    pub inventory: Inventory,
    /// `+0x10ac`: skill ids pushed by `FUN_004fb480`.
    pub v10ac: Vec<i32>,
    /// `+0x10b8`
    pub f10b8: i32,
    /// `+0x10bc`
    pub v10bc: Vec<i32>,
    /// `+0x10e8`: boss marker set by populateZoneCreatures.
    pub b10e8: u8,
    /// `+0x109c`: the AI tree the generator attached (None: the tick builds its default list).
    /// Not part of the oracle dump.
    pub ai: Option<SpawnAi>,
}

impl Spawn {
    /// The oracle dump record (format 5): the 74 bytes of format 3, then the thirteen equipment
    /// items, the combat floats `+0xf5c..+0xf6c`, the inventory, the two skill vectors, `+0x10b8`
    /// and `+0x10e8`.
    pub fn dump_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4096);
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        out.extend_from_slice(&self.f28.to_le_bytes());
        out.extend_from_slice(&self.entity_type.to_le_bytes());
        out.extend_from_slice(&self.f30.to_le_bytes());
        out.extend_from_slice(&self.level.to_le_bytes());
        for v in self.f38 {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.b50);
        out.extend_from_slice(&self.rotation.to_le_bytes());
        out.push(self.b58);
        out.extend_from_slice(&self.appearance.flags.to_le_bytes());
        out.extend_from_slice(&self.f_f58.to_le_bytes());
        for it in &self.equipment {
            out.extend_from_slice(&item_dump(it));
        }
        for v in [self.f_f5c, self.f_f60, self.f_f64, self.f_f68] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&inventory_dump(&self.inventory));
        out.extend_from_slice(&(self.v10ac.len() as u32).to_le_bytes());
        for v in &self.v10ac {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.f10b8.to_le_bytes());
        out.extend_from_slice(&(self.v10bc.len() as u32).to_le_bytes());
        for v in &self.v10bc {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.b10e8);
        out
    }

    /// The 64-bit spawn id at `+0x48` (`f38[4]`, `f38[5]`), the key of the world creature map.
    pub fn id(&self) -> i64 {
        i64::from(self.f38[4]) | (i64::from(self.f38[5]) << 32)
    }

    /// `cube::Spawn::Spawn` defaults for the fields kept here, with a zero position.
    pub const NEW: Spawn = Spawn { x: 0, y: 0, z: 0, f28: 1, entity_type: 4, f30: 0, level: 1, f38: [0; 6], b50: 0, rotation: 0.0, b58: 0, radius: 200.0, f5c: 0, f60: -1, f64: -1, appearance: Appearance::NEW, equipment: [Item::NEW; 13], f_f58: 100.0, f_f5c: 1.0, f_f60: 1.0, f_f64: 1.0, f_f68: 1.0, inventory: Inventory::NEW, v10ac: Vec::new(), f10b8: 0, v10bc: Vec::new(), b10e8: 0, ai: None };
}

/// A static entity record (0x188 bytes, `zone+0xc` vector); constructor `Server.exe 0x004c84b0`.
/// The name-like block at `+0x58..+0x170` is not tracked yet.
#[derive(Debug, Clone, PartialEq)]
pub struct Static {
    /// `+0`
    pub kind: u32,
    /// `+8`, `+0x10`, `+0x18`: 16.16 fixed blocks.
    pub x: i64,
    pub y: i64,
    pub z: i64,
    /// `+0x20`: rotation quadrant.
    pub rotation: i32,
    /// `+0x24`
    pub scale: [f32; 3],
    /// `+0x30`: 1 from the constructor.
    pub b30: u8,
    /// `+0x34`, `+0x38`
    pub f34: u32,
    pub f38: u32,
    /// `+0x40`, `+0x44`
    pub f40: u32,
    pub f44: u32,
    /// `+0x54`
    pub f54: u32,
    /// `+0x170`..`+0x188`: `[0, 0, -1, -1, -1, 0]` from the constructor.
    pub tail: [u32; 6],
    /// `+0x48`: the inventory (chest contents).
    pub inventory: Inventory,
}

impl Static {
    /// The oracle dump record (format 5): the 89 bytes of format 3, then the inventory.
    pub fn dump_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        out.extend_from_slice(&self.rotation.to_le_bytes());
        for v in self.scale {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.b30);
        out.extend_from_slice(&self.f34.to_le_bytes());
        out.extend_from_slice(&self.f38.to_le_bytes());
        out.extend_from_slice(&self.f40.to_le_bytes());
        out.extend_from_slice(&self.f44.to_le_bytes());
        out.extend_from_slice(&self.f54.to_le_bytes());
        for v in self.tail {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&inventory_dump(&self.inventory));
        out
    }

    pub const NEW: Static = Static { kind: 0, x: 0, y: 0, z: 0, rotation: 0, scale: [0.0; 3], b30: 1, f34: 0, f38: 0, f40: 0, f44: 0, f54: 0, tail: [0, 0, u32::MAX, u32::MAX, u32::MAX, 0], inventory: Inventory::NEW };
}

/// A prop record: an element of the `std::list` at `zone+4` (constructor `Server.exe 0x004c83b0`).
/// Props are the small decorations (flowers, grass tufts, mushrooms, lilies, ...) the per-column
/// decoration pass and `placeModel` scatter over the terrain.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    /// `+0`
    pub kind: u32,
    /// `+8`, `+0x10`, `+0x18`: 16.16 fixed blocks.
    pub x: i64,
    pub y: i64,
    pub z: i64,
    /// `+0x20`
    pub scale: f32,
    /// `+0x24`: rotation in degrees.
    pub rotation: f32,
    /// `+0x28`: filled at the end of generateZone from the block under the prop.
    pub f28: f32,
    /// `+0x2c`: `[1.0, 1.0, 1.0]` from the constructor.
    pub f2c: [f32; 3],
    /// `+0x38`: 2 from the constructor; the decoration pass ORs in 4 for some kinds.
    pub flags: u32,
}

impl Prop {
    /// The oracle dump record (format 4), 52 bytes: `f28` is left out (it is uninitialised until
    /// the end of generateZone and then a pure function of the blocks), and the scale and
    /// rotation of the tree props 0x3c/0x3d are dumped as 0 because generateTree never writes them.
    pub fn dump_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(52);
        out.extend_from_slice(&self.kind.to_le_bytes());
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.z.to_le_bytes());
        let masked = self.kind == 0x3c || self.kind == 0x3d;
        out.extend_from_slice(&if masked { 0.0f32 } else { self.scale }.to_le_bytes());
        out.extend_from_slice(&if masked { 0.0f32 } else { self.rotation }.to_le_bytes());
        for v in self.f2c {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&self.flags.to_le_bytes());
        out
    }

    pub const NEW: Prop = Prop { kind: 0, x: 0, y: 0, z: 0, scale: 0.0, rotation: 0.0, f28: 0.0, f2c: [1.0; 3], flags: 2 };
}

/// `cube::Zone`, 0xc8 bytes in the original.
#[derive(Debug, Clone)]
pub struct Zone {
    /// `+0x60`
    pub x: i32,
    /// `+0x64`
    pub y: i32,
    /// `+0x78`: copied from the region's zone record after the terrain pass.
    pub record: ZoneRecord,
    /// `+0xa8`
    pub columns: Vec<Column>,
    /// `+0x18`
    pub spawns: Vec<Spawn>,
    /// `+0x30`
    pub items: Vec<GroundItem>,
    /// `+0xc`
    pub statics: Vec<Static>,
    /// `+4`
    pub props: Vec<Prop>,
    /// `+0x48`
    pub markers: Vec<Marker>,
    /// `+0x88`: the settlement houses (`cube::House*`), filled by generateSettlement.
    pub houses: Vec<crate::settlement::House>,
    /// `+0x75`: set when the saved ground items differ from the generated ones (and by play).
    pub dirty: bool,
    /// `+0x68`: the player-modified blocks restored from the save blob (and added by play).
    pub modified: Vec<crate::save::ModifiedBlock>,
    /// `+0xa0`: the spawn count generateZone stored just before `spawnCellNpc`; the spawns
    /// from this index on (the boss or wandering group) are replaced at play time.
    pub gen_spawn_count: usize,
    /// `+0x76`: set by the tick's tail when a static record of the zone went out.
    pub statics_dirty: bool,
    /// The `std::list` of positions collected by the terrain pass (case 3 decorations).
    pub marks: Vec<(i32, i32, i32)>,
    /// Counters and extremes the terrain pass leaves for the later passes.
    pub stats: crate::generate::TerrainStats,
}

impl Zone {
    pub fn new(x: i32, y: i32) -> Self {
        Self {
            x,
            y,
            record: ZoneRecord::DEFAULT,
            columns: vec![Column::default(); 65536],
            spawns: Vec::new(),
            items: Vec::new(),
            statics: Vec::new(),
            props: Vec::new(),
            markers: Vec::new(),
            houses: Vec::new(),
            dirty: false,
            modified: Vec::new(),
            gen_spawn_count: 0,
            statics_dirty: false,
            marks: Vec::new(),
            stats: Default::default(),
        }
    }

    /// `cube::World::getBlock(bx, by, h, zone)`, `Server.exe 0x00405fd0`, for a zone under
    /// construction: `getColumn` with a zone hint only finds columns of that zone, so blocks
    /// outside it read as the "below" block.
    pub fn block(&self, bx: i32, by: i32, h: i32) -> Block {
        if !self.contains(bx, by) {
            return BELOW_BLOCK;
        }
        let col = self.column(bx, by);
        if h < col.height {
            return BELOW_BLOCK;
        }
        if col.height + col.blocks.len() as i32 <= h {
            return if h > 0 { AIR_BLOCK } else { WATER_BLOCK };
        }
        let b = col.block_at(h - col.height);
        if b[3] & 0x1f == 0 && h < 1 && b[3] & 0x40 == 0 {
            return WATER_BLOCK;
        }
        b
    }

    /// `column.height + column.blocks.len()`: the first height above the column's blocks.
    #[inline]
    pub fn top(&self, bx: i32, by: i32) -> i32 {
        let col = self.column(bx, by);
        col.height + col.blocks.len() as i32
    }

    #[inline]
    pub fn column(&self, bx: i32, by: i32) -> &Column {
        &self.columns[((bx % 256) + (by % 256) * 256) as usize]
    }

    #[inline]
    pub fn column_mut(&mut self, bx: i32, by: i32) -> &mut Column {
        &mut self.columns[((bx % 256) + (by % 256) * 256) as usize]
    }

    /// True when block `(bx, by)` lies inside this zone.
    #[inline]
    pub fn contains(&self, bx: i32, by: i32) -> bool {
        let x0 = self.x * 256;
        let y0 = self.y * 256;
        bx >= x0 && by >= y0 && bx < x0 + 256 && by < y0 + 256
    }

    /// The house whose room holds block `(bx, by, bz)`: the search of `Server.exe 0x004d4c20`
    /// over this zone's houses (`zone+0x88`), in order. A house matches when one of its room
    /// cells (type 1) at grid `(x, y, z)` (x over `dim_x`, y over `dim_y`, z over the height)
    /// spans the block: origin `pos + (13x, 13y, 7z)`, bounds inclusive up to `+13, +13, +6`.
    pub fn house_at(&self, bx: i32, by: i32, bz: i32) -> Option<&crate::settlement::House> {
        self.houses.iter().find(|h| {
            for x in 0..h.dim_x() {
                for y in 0..h.dim_y() {
                    for z in 0..h.dim_z() {
                        if h.cell(x, y, z).t != 1 {
                            continue;
                        }
                        let ox = h.pos[0] + x * 13;
                        let oy = h.pos[1] + y * 13;
                        let oz = h.pos[2] + z * 7;
                        if ox <= bx && oy <= by && oz <= bz && bx <= ox + 13 && by <= oy + 13 && bz <= oz + 6 {
                            return true;
                        }
                    }
                }
            }
            false
        })
    }
}

impl World {
    /// `Server.exe 0x004d4c20(bx, by, bz)`: the house of the zone holding the block (zone
    /// coordinates by truncating division, as the original) whose room spans the block.
    pub fn house_at(&self, bx: i32, by: i32, bz: i32) -> Option<&crate::settlement::House> {
        self.zone(div_trunc(bx, 256), div_trunc(by, 256))?.house_at(bx, by, bz)
    }

    /// `cube::World::getZone(zx, zy)`, `Server.exe 0x00406290`.
    /// Every loaded zone's coordinates, in no particular order.
    pub fn loaded_zones(&self) -> Vec<(i32, i32)> {
        self.zones.keys().map(|k| ((k >> 16) as i32, (k & 0xffff) as i32)).collect()
    }

    /// Drops a loaded zone (`World::unloadZone` without the save; call [`World::save_zone`]
    /// first). Returns the zone when it was loaded.
    pub fn remove_zone(&mut self, zx: i32, zy: i32) -> Option<Box<Zone>> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        self.zones.remove(&((zx as u32) << 16 | zy as u32))
    }

    pub fn zone_mut(&mut self, zx: i32, zy: i32) -> Option<&mut Zone> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        self.zones.get_mut(&((zx as u32) << 16 | zy as u32)).map(|z| &mut **z)
    }

    /// Puts back a zone taken with [`World::remove_zone`].
    pub fn insert_zone(&mut self, zone: Box<Zone>) {
        self.zones.insert((zone.x as u32) << 16 | zone.y as u32, zone);
    }

    pub fn zone(&self, zx: i32, zy: i32) -> Option<&Zone> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        self.zones.get(&((zx as u32) << 16 | zy as u32)).map(|z| &**z)
    }

    /// `cube::World::getColumn(bx, by, null)`, `Server.exe 0x00406100`: the column of the zone
    /// holding the block, if that zone exists.
    pub fn column(&self, bx: i32, by: i32) -> Option<&Column> {
        if !(0..0x1000000).contains(&bx) || !(0..0x1000000).contains(&by) {
            return None;
        }
        let zone = self.zone(div_trunc(bx, 256), div_trunc(by, 256))?;
        Some(zone.column(bx, by))
    }

    /// `cube::World::getBlock(bx, by, h, zone)`, `Server.exe 0x00405fd0`, with the zone looked up.
    pub fn block(&self, bx: i32, by: i32, h: i32) -> Block {
        let Some(col) = self.column(bx, by) else { return BELOW_BLOCK };
        if h < col.height {
            return BELOW_BLOCK;
        }
        if col.height + col.blocks.len() as i32 <= h {
            return if h > 0 { AIR_BLOCK } else { WATER_BLOCK };
        }
        let b = col.block_at(h - col.height);
        if b[3] & 0x1f == 0 && h < 1 && b[3] & 0x40 == 0 {
            return WATER_BLOCK;
        }
        b
    }
}

/// `cube::World::setBlock(bx, by, h, block, zone)`, `Server.exe 0x0041ff00`, operating on a
/// zone that is being generated (the original passes the zone pointer for this).
pub fn set_block(world: &World, zone: &mut Zone, bx: i32, by: i32, h: i32, mut block: Block) {
    if !zone.contains(bx, by) {
        return;
    }
    let col = zone.column(bx, by);
    let base = col.height;
    if !(block[3] != 0 || h < base + col.blocks.len() as i32) {
        return;
    }
    if h < 1 && block[3] & 0x5f == 0 {
        block = WATER_BLOCK;
    }
    zone.column_mut(bx, by).set_raw(h - base, block);
    let mut hh = h + 1;
    while hh < base {
        let tc = world.terrain_color(bx, by, hh);
        let b = [tc[0] as i32 as u8, tc[1] as i32 as u8, tc[2] as i32 as u8, 6];
        let col = zone.column_mut(bx, by);
        let idx = hh - col.height;
        col.set_raw(idx, b);
        hh += 1;
    }
}

impl Static {
    /// `Server.exe 0x004d8c90`: when the open/closed byte differs from `open`, mirrors the
    /// millisecond timer (`1000 - f34`, floored at 0), sets the byte and returns the sound kind
    /// to play at the static's position: 0x33 for kind 5, 0x35 and 0x34 for kinds 6 and 7 only
    /// when opening, 0x36 for every other kind. `None` when nothing changes or a kind-6/7 static
    /// closes.
    pub fn toggle(&mut self, open: u8) -> Option<u32> {
        if self.b30 == open {
            return None;
        }
        self.f34 = 1000i32.wrapping_sub(self.f34 as i32).max(0) as u32;
        self.b30 = open;
        match self.kind {
            5 => Some(0x33),
            6 => (open != 0).then_some(0x35),
            7 => (open != 0).then_some(0x34),
            _ => Some(0x36),
        }
    }
}

impl World {
    /// `dropItem(item, pos, rotation, scale)`, `Server.exe 0x004d2810`: a ground item at `pos`
    /// (16.16 fixed) lowered onto the first block below that is neither air nor water, pushed
    /// onto its zone's item list. Draws `rand` twice, the `+0x13c` countdown (`500 + rand % 300`)
    /// before the zone lookup and the rotation (`rand * 360 / 32767`, which replaces the
    /// `rotation` argument) after it. Returns the zone the item landed in.
    pub fn drop_item(&mut self, item: Item, pos: [i64; 3], rotation: f32, scale: f32) -> Option<(i32, i32)> {
        let mut g = GroundItem::NEW;
        g.x = pos[0];
        g.y = pos[1];
        g.z = pos[2];
        g.rotation = rotation;
        g.f134 *= scale;
        g.f13c = self.rng.rand() % 300 + 500;
        g.f144 = self.day;
        let (zx, zy) = (div_trunc((pos[0] / 65536) as i32, 256), div_trunc((pos[1] / 65536) as i32, 256));
        self.zone(zx, zy)?;
        g.item = item;
        g.rotation = self.rng.rand() as f32 * 360.0f32 / 32767.0f32;
        // The block search starts at the position's block (one lower for a negative z) and
        // walks down through air and water; getBlock's out-of-zone answers apply.
        let mut zb = (pos[2] / 65536) as i32;
        if pos[2] < 0 {
            zb -= 1;
        }
        let (bx, by) = ((pos[0] / 65536) as i32, (pos[1] / 65536) as i32);
        loop {
            let b = self.block(bx, by, zb);
            if b[3] & 0x1f != 0 && b[3] & 0x1f != 2 {
                break;
            }
            zb -= 1;
        }
        g.z = i64::from(zb + 1) << 16;
        self.zone_mut(zx, zy)?.items.push(g);
        Some((zx, zy))
    }
}
