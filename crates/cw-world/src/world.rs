//! `cube::World` state that the generator reads and writes.

use std::collections::HashMap;

use cw_math::MsvcRand;

use crate::Seeds;
use crate::climate::ClimatePoint;
use crate::model::Model;
use crate::region::Region;
use crate::zone::Zone;

/// The world spawn position in blocks as `World::load` sets it: the float with bits
/// 0x4b002080 (8396928.0, zone 32800, region 512) on both axes.
pub const DEFAULT_SPAWN: [f32; 2] = [f32::from_bits(0x4b002080), f32::from_bits(0x4b002080)];

/// `world+0x88` (`vector<int>`): the creature types pet food (bait, item type 0x14) exists for,
/// in the order the World ctor pushes them: `Server.exe 0x004cd507..0x004cd836` (`edi` =
/// `world+0x88` from 0x004c8628, 45 calls of `vector<int>::push_back` 0x004f2be0), and the
/// byte-identical `Cube.exe 0x00593a97..0x00593dbd`. Nothing else writes the vector, so it is
/// a constant here. Readers: the loot's `randomConsumable` (`Server.exe 0x0052b3f0`, from
/// `dropLoot` 0x004d2ae0 at 0x004d2f2a) and, in Cube.exe, the villager text (0x004e4cfd,
/// 0x004e5078).
pub const PET_TYPES: [i32; 45] = [
    0x23, 0x57, 0x3c, 0x37, 0x22, 0x17, 0x16, 0x1e, 0x21, 0x62, 0x19, 0x35, 0x43, 0x66, 0x68, 0x69, 0x13, 0x28, 0x25, 0x26,
    0x27, 0x5c, 0x5d, 0x38, 0x5a, 0x5b, 0x32, 0x1b, 0x4b, 0x1a, 0x56, 0x63, 0x6a, 0x3f, 0x42, 0x40, 0x41, 0x4a, 0x24, 0x39,
    0x3a, 0x3b, 0x3d, 0x3e, 0x58,
];

/// The generator's view of `cube::World`.
/// A player creature as the world tick sees it (hostile type 0 in the creature map).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerInfo {
    pub id: i64,
    /// `entity+0x180`.
    pub level: i32,
    /// `entity+0x00..0x18`, 16.16 fixed blocks.
    pub pos: [i64; 3],
}

#[derive(Debug, Clone)]
pub struct World {
    pub seeds: Seeds,
    /// `world+0x8000f0/+0x8000f4`, floats in blocks.
    pub spawn: [f32; 2],
    /// `world+0xa4 != 0`: the world has a name, so regions get a home cell and the save
    /// database is consulted. Always true for a server world (`server_<seed>`).
    pub has_name: bool,
    /// The `rand` state of the thread using this world. [`World::new`] leaves it as
    /// `World::load` leaves the caller's; every later `srand`/`rand` of the original's
    /// generator maps onto this one stream.
    pub rng: MsvcRand,
    /// `world+0x4000bc`: one climate point per region, keyed by `rx * 1024 + ry`.
    pub(crate) points: HashMap<u32, ClimatePoint>,
    /// `world+0xbc`: regions, keyed by `rx * 1024 + ry`.
    pub(crate) regions: HashMap<u32, Box<Region>>,
    /// Generated zones (`region+0x10018` in the original), keyed by `zx << 16 | zy`.
    pub(crate) zones: HashMap<u32, Box<Zone>>,
    /// `world+0x20`: the voxel model table (`model::load_models`), shared so generation can
    /// borrow a model while mutating the world.
    pub models: std::sync::Arc<Vec<Model>>,
    /// `world+0xac`: the save database (`Save/world_<name>.db`), when one is attached.
    pub save: Option<std::sync::Arc<std::sync::Mutex<cw_formats::SaveDb>>>,
    /// `world+0x800160`: the day counter, from the `time` blob (0 for a fresh world).
    pub day: i32,
    /// `world+0x80015c`: the time of day in milliseconds, from the `time` blob (noon,
    /// 43200000, for a fresh world; 32400000 for an unnamed one).
    pub time_of_day: i32,
    /// The players of the creature map (`world+4`, hostile type 0), refreshed by the server
    /// before generation and every tick: their levels and positions steer the wandering
    /// groups, the mission rolls and the creature spawns.
    pub players: Vec<PlayerInfo>,
    /// The ids of the creatures made from zone spawns that exist in the server's creature
    /// map, so `spawnCellNpc` and the wandering-group spawner can see them.
    pub creature_ids: std::collections::BTreeSet<i64>,
    /// Creatures the world deleted (spawns dropped by `spawnCellNpc`); the server removes
    /// them from its map and clears the queue.
    pub removed_creatures: Vec<i64>,
    /// Cells whose boss creatures (appearance flag 0x2000, home zone in the cell)
    /// `startMission` deleted; the server removes them and clears the queue.
    pub boss_removals: Vec<(i32, i32)>,
    /// `world+0xb4`: non-zero in the client's world (Cube.exe), 0 on a server. `World::tick`
    /// (Server.exe 0x005322d0, Cube.exe 0x0060c510, byte-identical) skips the server's
    /// authority (spawns, AI, releases, applied hits) and runs the client's own effects on it.
    pub is_client: bool,
    /// `world+0xb8`: the local player's creature (its id here), NULL on a server. The tick runs
    /// the local player's physics, stamina, riding and sounds only for this creature.
    pub local_player: Option<i64>,
    /// `world+0x84`: bit 0 raises the local player's speed cap to 20 (0x0053f217); no writer
    /// in Server.exe, 0 by default.
    pub flags84: u32,
    /// Not in the original. The alpha never writes a pet's level and XP back into its owner's
    /// pet item in multiplayer (the write-back at 0x00534818 runs only for the local player
    /// inside the server's creature pass, which a client replaces and a server has no local
    /// player for), so pet progress is lost on servers; the community fix
    /// (coremaze/Cube-World-Multiplayer-Pet-Leveling-Fix) copies the pet's XP and level into
    /// the item once a second. With this on, the client tick does that write-back every tick.
    /// Off reproduces the original.
    pub fix_pet_leveling: bool,
}

impl World {
    /// A world as `World::load(seed, name)` leaves it, with no regions or points yet.
    ///
    /// `rng` is the stream of the thread that called `load` (`Server.exe 0x004d83a0`,
    /// `Cube.exe 0x005a52e0`): `srand(seed)` is the first call after the seed is stored
    /// (Server.exe 0x004d83ea, Cube.exe 0x005a532a), then the 77 sub-seed draws; the rest of
    /// `load` (unload/`saveZone`, database open, `time` blob) draws nothing. Server.exe calls
    /// `load` on the main thread (`main` 0x00549e67, the tick's stream); Cube.exe's world
    /// switch calls it on the zone thread (0x0046afab in 0x0046a8a0), whose generation reseeds
    /// per region and zone before drawing.
    pub fn new(seed: i32) -> Self {
        let (seeds, rng) = Seeds::from_seed_with_rng(seed);
        Self {
            seeds,
            spawn: DEFAULT_SPAWN,
            has_name: true,
            rng,
            points: HashMap::new(),
            regions: HashMap::new(),
            zones: HashMap::new(),
            models: std::sync::Arc::default(),
            save: None,
            day: 0,
            time_of_day: 43_200_000,
            players: Vec::new(),
            creature_ids: std::collections::BTreeSet::new(),
            removed_creatures: Vec::new(),
            boss_removals: Vec::new(),
            is_client: false,
            fix_pet_leveling: true,
            local_player: None,
            flags84: 0,
        }
    }
}

impl World {
    /// A copy for a generation thread: everything but the loaded zones and the queued
    /// removals, with a fresh `rand` stream (that thread's own; generation reseeds it per
    /// region and zone anyway).
    pub fn generator_copy(&self) -> World {
        let mut w = self.clone();
        w.zones.clear();
        w.rng = MsvcRand::default();
        w.removed_creatures.clear();
        w.boss_removals.clear();
        w
    }

    /// Before a zone is generated: the play-time state generation reads, taken from the served
    /// world. Cells carry the mission and monster state the tick rolls, and `spawnCellNpc` and
    /// the wandering-group spawner look at the players and the existing creatures.
    ///
    /// Performance note (2026-09): this copies the cells of every loaded region (64 cells of
    /// ~0x68 bytes each) and the whole creature id set before each zone. Generation only reads
    /// the 3x3 regions around the zone, so copying those alone would do, and the id set could
    /// be shared behind a read lock. Neither showed up in profiles; kept simple until it does.
    pub fn pull_play_state(&mut self, from: &World) {
        for (k, r) in &from.regions {
            if let Some(mine) = self.regions.get_mut(k) {
                mine.cells = r.cells;
                mine.missions_active = r.missions_active;
            }
        }
        self.players = from.players.clone();
        self.creature_ids = from.creature_ids.clone();
        self.day = from.day;
        self.time_of_day = from.time_of_day;
    }

    /// After a zone is generated: the regions and climate points this copy created that the
    /// served world lacks, and the creature removals generation queued, moved to it.
    pub fn publish_generation(&mut self, to: &mut World) {
        for (k, p) in &self.points {
            to.points.entry(*k).or_insert(*p);
        }
        for (k, r) in &self.regions {
            if !to.regions.contains_key(k) {
                to.regions.insert(*k, r.clone());
            }
        }
        to.removed_creatures.append(&mut self.removed_creatures);
    }
}
