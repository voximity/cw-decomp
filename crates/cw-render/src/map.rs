//! The voxel map: `cube::WorldMap::draw` (`Cube.exe 0x005fc1b0`, 23 KB, `this` = the
//! `cube::WorldMap` at `GameController+0x800d44`, `ret 0x28`) and the tile cache it draws
//! from (`WorldMap+0xb0` zone records, `WorldMap+0x4000b0` landscape tiles), which the
//! client's landscape thread (`cw-client` `landscape.rs`, `Cube.exe 0x00469590`) fills.
//!
//! Fidelity: Tier B for every number (matrices, offsets, colours, fades, selection rules and
//! draw order), Tier C for the state machinery. The `SetScissorRect` the original issues at
//! `0x005fcd2e` (rect `(w-320, 320, w-20, 20)`) is never enabled (`SCISSORTESTENABLE` is only
//! ever set to 0, `0x006002d8`) and is dropped.
//!
//! # Units
//!
//! The map's world unit is 8 blocks (one voxel of a zone tile, [`ZONE_TILE_EDGE`] of them per
//! zone). The view ([`map_matrices`]) looks at the map centre from 300 units away; world
//! matrices place models relative to the centre (x/y relative to the centre's zone origin to
//! keep floats small). Border posts, mission icons and creature heads are placed in blocks
//! and drawn with the stored view `WorldMap+0x44`, which is the map view with rows 0..2
//! scaled by 1/8.
//!
//! # Map of `0x005fc1b0`
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x005fc1b0..0x005fc211` | prologue; `WorldMap+0x8000b8 += frame_ms` (the map clock) | [`MapState::clock_ms`], [`map_advance`] |
//! | `0x005fc217..0x005fc6d0` | projection: `PerspectiveLH(45°)` with x mirrored, then `T(ndc_x, ndc_y, -0.1)`; copy of the previous projection `WorldMap+4` with `[3][2] -= 80` | [`map_matrices`], [`biased_projection`] |
//! | `0x005fc6d0..0x005fcd2e` | view: `T(0,0,300)`, zoom scale, `Rz(0)`, `Rx(tilt)`, `Rz(yaw)` | [`map_matrices`] |
//! | `0x005fcd2e..0x005fce24` | `SetScissorRect` (dropped), `ZENABLE=1`, bind VS00+PS01, fog distance 1e6, ten zero point lights | [`map_draws`] |
//! | `0x005fce24..0x005fd694` | zoom < 0.35: landscape tiles (one voxel per zone, per climate region) of the regions around the centre's climate region | [`MapPass::RegionTiles`] |
//! | `0x005fd694..0x005fe2b0` | zoom ≥ 0.35: per zone around the centre: `maptile.cub` placeholder (full map only) | [`MapPass::Placeholders`] |
//! | `0x005fe2b0..0x005fea9c` | … the zone tile (fade-in rise, darkened when unvisited on the full map) | [`MapPass::ZoneTiles`] |
//! | `0x005fea9c..0x005fee56` | … the zone's border posts (previous frame's view and biased projection) | [`MapPass::BorderPosts`] |
//! | `0x005fee56..0x005ffdc0` | full map: mission icons (`mission.cub`, spinning) and the home-town marker (`city.cub`, beyond 512 blocks) of the centre's region | [`MapPass::MissionIcons`] |
//! | `0x005ffdc0..0x006000a0` | `SCISSORTESTENABLE=0`; store the projection at `WorldMap+4` and the view /8 at `WorldMap+0x44`; bind; fog distance 1e9 | [`MapState`], [`map_advance`] |
//! | `0x006000a0..0x006002f0` | collect creatures (players, marked bosses, pets at zoom ≥ 1.6) within `radius·256` blocks, depth key from the stored view | [`collect_heads`] |
//! | `0x006002f0..0x00600380` | `std::sort` by key (`0x005fa9e0`, ascending: far to near) | [`collect_heads`] |
//! | `0x00600380..0x00600638` | arrow point 300 blocks from the player away from the world centre → `WorldMap+0x84`; `setLight` | [`map_arrow`], [`map_draws`] |
//! | `0x00600638..0x00601c78` | per creature: black silhouette at 5/zoom with `ZWRITE=0`, then the head at 4/zoom with `ZWRITE=1` | [`MapPass::CreatureHeads`] |
//!
//! # Hook for `passes.rs`
//!
//! `build_frame` keeps emitting at the two call sites; replace each
//! `b.emit(addr, Geometry::Map(view))` by splicing [`map_draws`]:
//!
//! ```ignore
//! if let Some(scene) = &inputs.map {            // new field `map: Option<map::MapScene<'a>>`
//!     let ctx = map::MapContext { state: b.state, uniforms: b.uniforms.clone(), draw: b.draw.clone() };
//!     let draws = map::map_draws(&view, scene, &ctx, &mut b.out.uniforms);
//!     b.out.passes.last_mut().unwrap().draws.extend(draws);
//!     let (state, uniforms) = map::map_exit(&ctx, &b.out.passes.last().unwrap().draws);
//!     b.state = state; b.uniforms = uniforms; b.uniforms_dirty = true;
//! }
//! ```
//!
//! and after the frame the client calls [`map_advance`] with the same view (the clock, the
//! tile fades and the stored matrices are the original's side effects of the call).

// The loops index matrices the way the original's unrolled code does.
#![allow(clippy::needless_range_loop)]
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::HashMap;

use cw_world::region::ZoneRecord;

use crate::frame::*;
use crate::mesh::{ModelMesh, VoxelGrid, build_model_mesh};
use crate::passes::{pre_rotate_z, pre_scale, pre_translate};

/// `1/65536` (`0x006fcd94`).
const INV_FIXED: f32 = 1.525_878_9e-5;

/// Zone tile edge in voxels: 256 blocks / 8.
pub const ZONE_TILE_EDGE: i32 = 32;

/// `ZoneTile+0x2c` / fade timer start (`0x00603a00`: 0xfa when a tile model appears).
pub const TILE_FADE_MS: i32 = 250;

/// First [`ModelRef`] the tile cache hands out for its own meshes (zone and landscape
/// tiles). World-table models are expected to use their table slot as handle.
pub const TILE_MODEL_BASE: ModelRef = 0x4000_0000;

/// World model slots the map draws (`world+0x20`, see `cw_world::model_names`).
pub const MODEL_MISSION: u16 = 0xa00; // mission.cub
/// `city.cub`: the home-town marker.
pub const MODEL_CITY: u16 = 0xa01;
/// `skull.cub`: hostile marked creatures.
pub const MODEL_SKULL: u16 = 0xa02;
/// `maptile.cub`: placeholder of a zone without a tile.
pub const MODEL_MAPTILE: u16 = 0xa07;

/// Pet/animal race icons (`creature+0x140` → model slot), `0x00600c..`/`0x0060150b..`.
pub const RACE_ICONS: [(u8, u16); 8] = [
    (0x80, 0x994),
    (0x81, 0x995),
    (0x82, 0x996),
    (0x83, 0x997),
    (0x84, 0x993),
    (0x85, 0x998),
    (0x86, 0x999),
    (0x87, 0x99a),
];

// ---------------------------------------------------------------------------------------
// The tile cache
// ---------------------------------------------------------------------------------------

/// The model of a zone tile (`ZoneTile+8`, a `cube::Sprite` built by `0x00603a00`): 32×32
/// voxels, each the average colour of an 8×8×8 block cell, `size[2]` cells high.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneTile {
    /// Voxel counts (`sprite+0x44..+0x4c`): 32, 32, height.
    pub size: [i32; 3],
    /// Voxels, `(z * 32 + y) * 32 + x`.
    pub voxels: Vec<[u8; 3]>,
    /// Mesh (`cube::Sprite::buildMesh 0x004e7870`, tint 0, raw).
    pub mesh: ModelMesh,
    /// Handle the backend knows the mesh by.
    pub model: ModelRef,
}

/// One zone of a region record, `cube::ZoneTile` (ctor `0x005fb7f0`, 0x34 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct ZoneEntry {
    /// `+4`: z of the tile's lowest voxel layer, in 8-block cells.
    pub base_z: i32,
    /// `+8`: the tile model.
    pub tile: Option<ZoneTile>,
    /// `+0x10..+0x1f`: the zone's site record, a copy of the world region's 16-byte zone
    /// record (`Region+0x18 + ((zx & 63) * 64 + (zy & 63)) * 16`, found by `0x004a6ad0`),
    /// taken by `buildZoneTile 0x00603a00` (0x00603a9a..0x00603ac0, under the map's critical
    /// section `WorldMap+0x8000d8`) whenever the world has the region: `+0x10` kind, `+0x11`
    /// sub type, `+0x14` seed, `+0x18` level, `+0x1c` the town cell's level roll. The ctor
    /// sets kind 0 and level 1. `MapOverlayWidget::update 0x004c9680` reads it for the site
    /// labels (kind, level, and the name through `0x004e5a20`).
    pub site: ZoneRecord,
    /// `+0x20`: border posts, block positions (list, `+0x24` count).
    pub posts: Vec<[i32; 3]>,
    /// `+0x28`: the zone changed (set by the zone thread after `generateZone`, `0x0046ac..`).
    pub dirty: bool,
    /// `+0x2c`: fade timer in ms (250 when the model appears, counted down by the map draw).
    pub fade: i32,
    /// `+0x30`: bit 0 visited (`WorldMap::markVisited 0x005fc160`), bit 1 has had a tile.
    pub flags: u8,
}

impl Default for ZoneEntry {
    /// The ctor `0x005fb7f0`: everything zero, the site level (`+0x18`) 1.
    fn default() -> Self {
        ZoneEntry {
            base_z: 0,
            tile: None,
            site: ZoneRecord::DEFAULT,
            posts: Vec::new(),
            dirty: false,
            fade: 0,
            flags: 0,
        }
    }
}

/// A landscape tile, `cube::LandscapeTile` (0x34 bytes, `WorldMap+0x4000b0`): the zones owned
/// by one climate region as a flat model, one voxel per zone (`0x006024d0`).
#[derive(Clone, Debug, PartialEq)]
pub struct RegionTile {
    /// `+4..+0x1c`: copy of the region's climate point: block x, y and elevation (`+0x1c`).
    pub point: [i32; 2],
    /// `+0x1c`: the point's elevation in blocks.
    pub elevation: i32,
    /// `+0x20`, `+0x24`: zone coordinates of voxel (0, 0).
    pub min_zone: [i32; 2],
    /// `+0x28`: drawn by the far map (tested at 0x005fcfa8). Set by `GameController::update`
    /// for the tile of the region the player stands in ([`MapTiles::discover_region`],
    /// 0x0048d5bc), kept across rebuilds and round-tripped by the `"land"` blob.
    pub visible: bool,
    /// `+0x2c`: the model, `None` until a zone was found to own.
    pub tile: Option<ZoneTile>,
    /// `+0x30`: rebuild requested (0x0048d5e1, at most every two seconds for the tile under
    /// the player; cleared when `0x006024d0` rebuilds it).
    pub dirty: bool,
}

/// A landscape tile as the `"land"` blob holds it (`0x006024d0` load branch, read with
/// `0x0044d620`; written by 0x006050b0).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedLand {
    /// `+0x28`.
    pub visible: bool,
    /// `+0x20`, `+0x24`.
    pub min_zone: [i32; 2],
    /// The model's size; no model unless every axis is positive.
    pub size: [i32; 3],
    pub voxels: Vec<[u8; 3]>,
    /// `+4`, `+8`: the climate point.
    pub point: [i32; 2],
    /// `+0x1c`.
    pub elevation: i32,
}

/// What the backend has to upload or release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshEvent {
    /// A mesh appeared; fetch it with [`MapTiles::mesh`].
    Added(ModelRef),
    /// A mesh went away.
    Removed(ModelRef),
}

/// `cube::WorldMap`'s tile state (`+0xb0` region records of 64×64 [`ZoneEntry`]s,
/// `+0x4000b0` [`RegionTile`]s, the `+0x8000bc` discovered counter). The original guards it
/// with the critical sections `+0x8000c0`/`+0x8000d8`; the port keeps it behind one mutex.
#[derive(Debug, Default)]
pub struct MapTiles {
    records: HashMap<(i32, i32), Vec<ZoneEntry>>,
    region_tiles: HashMap<(i32, i32), RegionTile>,
    /// `+0x8000bc`: zones marked visited (saved as the "discovered" blob).
    pub discovered: i32,
    next_ref: ModelRef,
    events: Vec<MeshEvent>,
    /// The static `0x0076b0fc` (zero-initialised): world clock of the last rebuild request
    /// of [`MapTiles::discover_region`]. A process global in the original; the port resets it
    /// with the cache.
    pub region_discover_ms: i32,
    /// `"land"` blobs read from the map database, waiting for `0x006024d0` to find the region
    /// without a tile object (the original reads the blob there, on demand).
    saved_land: HashMap<(i32, i32), SavedLand>,
}

fn zone_index(zx: i32, zy: i32) -> usize {
    ((zx % 64) * 64 + zy % 64) as usize
}

impl MapTiles {
    /// An empty cache.
    pub fn new() -> MapTiles {
        MapTiles { next_ref: TILE_MODEL_BASE, ..Default::default() }
    }

    /// Whether the record of region `(rx, ry)` exists (`WorldMap+0xb0` slot non-null).
    pub fn has_record(&self, rx: i32, ry: i32) -> bool {
        self.records.contains_key(&(rx, ry))
    }

    /// Creates the record of a region (`0x00603230` allocates `0x35a00` bytes with 4096
    /// default `cube::ZoneTile`s). Returns whether it was created. Coordinates outside
    /// `0..1024` are refused as in the original.
    pub fn ensure_record(&mut self, rx: i32, ry: i32) -> bool {
        if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) || self.has_record(rx, ry) {
            return false;
        }
        self.records.insert((rx, ry), vec![ZoneEntry::default(); 4096]);
        true
    }

    /// `WorldMap::zoneTile 0x00602440`: the entry of zone `(zx, zy)`, if its region record
    /// exists.
    pub fn entry(&self, zx: i32, zy: i32) -> Option<&ZoneEntry> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        self.records.get(&(zx / 64, zy / 64)).map(|r| &r[zone_index(zx, zy)])
    }

    /// Mutable [`MapTiles::entry`].
    pub fn entry_mut(&mut self, zx: i32, zy: i32) -> Option<&mut ZoneEntry> {
        if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        self.records.get_mut(&(zx / 64, zy / 64)).map(|r| &mut r[zone_index(zx, zy)])
    }

    /// `WorldMap::markVisited 0x005fc160`: sets bit 0 of the zone's flags and counts it.
    /// Returns whether the zone was newly visited.
    pub fn mark_visited(&mut self, zx: i32, zy: i32) -> bool {
        let Some(e) = self.entry_mut(zx, zy) else { return false };
        if e.flags & 1 != 0 {
            return false;
        }
        e.flags |= 1;
        self.discovered += 1;
        true
    }

    /// The zone thread after `generateZone` (`0x0046ab..`): `entry(zx, zy)+0x28 = 1`.
    pub fn mark_dirty(&mut self, zx: i32, zy: i32) {
        if let Some(e) = self.entry_mut(zx, zy) {
            e.dirty = true;
        }
    }

    fn alloc_ref(&mut self) -> ModelRef {
        let r = self.next_ref;
        self.next_ref = self.next_ref.wrapping_add(1);
        r
    }

    /// Puts a zone tile model in place (replacing and releasing any previous one) and
    /// returns its handle. The caller sets the entry's other fields.
    pub fn set_zone_tile(&mut self, zx: i32, zy: i32, size: [i32; 3], voxels: Vec<[u8; 3]>, mesh: ModelMesh) -> Option<ModelRef> {
        self.entry(zx, zy)?;
        let model = self.alloc_ref();
        let old = self.entry_mut(zx, zy)?.tile.replace(ZoneTile { size, voxels, mesh, model });
        if let Some(o) = old {
            self.events.push(MeshEvent::Removed(o.model));
        }
        self.events.push(MeshEvent::Added(model));
        Some(model)
    }

    /// `0x006022d0`: drops a zone's tile model and its posts.
    pub fn drop_zone_tile(&mut self, zx: i32, zy: i32) {
        let Some(e) = self.entry_mut(zx, zy) else { return };
        if let Some(t) = e.tile.take() {
            e.posts.clear();
            self.events.push(MeshEvent::Removed(t.model));
        }
    }

    /// Visits every zone entry that has a tile: `(zx, zy)`.
    pub fn tiled_zones(&self) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for (&(rx, ry), rec) in &self.records {
            for (i, e) in rec.iter().enumerate() {
                if e.tile.is_some() {
                    out.push((rx * 64 + i as i32 / 64, ry * 64 + i as i32 % 64));
                }
            }
        }
        out
    }

    /// The landscape tile of region `(rx, ry)`.
    pub fn region_tile(&self, rx: i32, ry: i32) -> Option<&RegionTile> {
        self.region_tiles.get(&(rx, ry))
    }

    /// Creates (or returns) the landscape tile object of a region with its climate point
    /// copy (`0x006024d0` creates it before sampling).
    pub fn ensure_region_tile(&mut self, rx: i32, ry: i32, point: [i32; 2], elevation: i32) -> &mut RegionTile {
        let t = self.region_tiles.entry((rx, ry)).or_insert(RegionTile {
            point,
            elevation,
            min_zone: [0, 0],
            visible: false,
            tile: None,
            dirty: false,
        });
        t.point = point;
        t.elevation = elevation;
        t
    }

    /// Replaces a landscape tile's model (`0x006024d0` tail).
    pub fn set_region_model(&mut self, rx: i32, ry: i32, min_zone: [i32; 2], size: [i32; 3], voxels: Vec<[u8; 3]>, mesh: ModelMesh) {
        if !self.region_tiles.contains_key(&(rx, ry)) {
            return;
        }
        let model = self.alloc_ref();
        let t = self.region_tiles.get_mut(&(rx, ry)).expect("checked");
        t.min_zone = min_zone;
        t.dirty = false;
        let old = t.tile.replace(ZoneTile { size, voxels, mesh, model });
        if let Some(o) = old {
            self.events.push(MeshEvent::Removed(o.model));
        }
        self.events.push(MeshEvent::Added(model));
    }

    /// Keeps a `"land"` blob read from the map database for [`MapTiles::take_saved_land`].
    pub fn stash_saved_land(&mut self, rx: i32, ry: i32, land: SavedLand) {
        self.saved_land.insert((rx, ry), land);
    }

    /// The `"land"` blob of a region, once (`0x004498d0` finding the key).
    pub fn take_saved_land(&mut self, rx: i32, ry: i32) -> Option<SavedLand> {
        self.saved_land.remove(&(rx, ry))
    }

    /// `GameController::update` 0x0048d51c..0x0048d5e5, every frame under the map's section
    /// `+0x8000c0` (0x00601cb0/0x00601e90): the tile of the nearest region around the player
    /// (`0x00601cc0`, null unless all 3×3 regions have tile models) becomes visible when its
    /// climate point equals the world's nearest climate point of the player's block
    /// (`World::nearestClimatePoint 0x00477e10`, compared by `0x00468840`: both ints);
    /// then, when `world_ms - [0x0076b0fc] > 2000` (signed), `[0x0076b0fc] = world_ms` and the
    /// tile's `+0x30` asks the landscape thread for a rebuild (fresh 200/220/255 shades).
    /// `world_ms` is `World+0x8000bc` (0x00488b80). Returns the region marked.
    pub fn discover_region(&mut self, climate_point: Option<[i32; 2]>, bx: i32, by: i32, world_ms: i32) -> Option<(i32, i32)> {
        let key = nearest_region_key(self, bx, by)?;
        let point = climate_point?;
        let t = self.region_tiles.get_mut(&key)?;
        if t.point != point {
            return None;
        }
        t.visible = true;
        if world_ms.wrapping_sub(self.region_discover_ms) > 2000 {
            self.region_discover_ms = world_ms;
            t.dirty = true;
        }
        Some(key)
    }

    /// `0x005fbed0` second half: saves (`0x006050b0`, here into the saved-land store the map
    /// database stands for) and drops a landscape tile.
    pub fn drop_region_tile(&mut self, rx: i32, ry: i32) {
        let Some(t) = self.region_tiles.remove(&(rx, ry)) else { return };
        let (size, voxels) = match &t.tile {
            Some(m) => (m.size, m.voxels.clone()),
            None => ([0; 3], Vec::new()),
        };
        self.saved_land.insert((rx, ry), SavedLand { visible: t.visible, min_zone: t.min_zone, size, voxels, point: t.point, elevation: t.elevation });
        if let Some(m) = t.tile {
            self.events.push(MeshEvent::Removed(m.model));
        }
    }

    /// The saved-land store: `"land"` blobs loaded and not yet taken, and tiles evicted this
    /// session (for the map database save).
    pub fn saved_land(&self) -> impl Iterator<Item = (&(i32, i32), &SavedLand)> {
        self.saved_land.iter()
    }

    /// Regions that have a landscape tile object.
    pub fn region_tile_keys(&self) -> Vec<(i32, i32)> {
        self.region_tiles.keys().copied().collect()
    }

    /// The mesh behind a tile handle.
    pub fn mesh(&self, model: ModelRef) -> Option<&ModelMesh> {
        for rec in self.records.values() {
            for e in rec {
                if let Some(t) = &e.tile
                    && t.model == model
                {
                    return Some(&t.mesh);
                }
            }
        }
        self.region_tiles.values().filter_map(|t| t.tile.as_ref()).find(|t| t.model == model).map(|t| &t.mesh)
    }

    /// Meshes added and removed since the last call, in order.
    pub fn take_mesh_events(&mut self) -> Vec<MeshEvent> {
        std::mem::take(&mut self.events)
    }
}

/// `cube::Sprite::buildMesh 0x004e7870` for a tile: the sprites of `0x00603a00`/`0x006024d0`
/// come from `Sprite::Sprite(device, 0)` (`0x004e6a20`), which sets the raw flag `+0x55 = 1`
/// and a zero tint.
pub fn mesh_tile_voxels(size: [i32; 3], voxels: &[[u8; 3]]) -> ModelMesh {
    build_model_mesh(VoxelGrid { size, voxels }, [0, 0, 0], true)
}

/// The border post model `WorldMap+0x8000b0` built by the `WorldMap` constructor
/// (`0x005fae40`): a 4×4×4 sprite whose bottom layer is white inside (x, y in 1..=2) and
/// (10, 10, 10) around; the three upper layers stay empty. Mesh it with
/// [`mesh_tile_voxels`] and hand the handle in [`MapModels::border_post`].
pub fn border_post_voxels() -> ([i32; 3], Vec<[u8; 3]>) {
    let mut v = vec![[0u8; 3]; 64];
    for x in 0..4 {
        for y in 0..4 {
            let inner = (1..=2).contains(&x) && (1..=2).contains(&y);
            v[(y * 4 + x) as usize] = if inner { [255, 255, 255] } else { [10, 10, 10] };
        }
    }
    ([4, 4, 4], v)
}

// ---------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------

/// A model the map can draw: its handle and voxel size (`sprite+0x44..+0x4c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapModel {
    /// Handle.
    pub model: ModelRef,
    /// Size in voxels.
    pub size: [i32; 3],
}

/// The models the map reads outside the tile cache.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapModels {
    /// The world model table `world+0x20` by slot (`Database` lookup `0x004120c0`); `None`
    /// for empty or missing slots.
    pub world: Vec<Option<MapModel>>,
    /// `WorldMap+0x8000b0`, see [`border_post_voxels`].
    pub border_post: Option<MapModel>,
}

impl MapModels {
    /// Slot lookup.
    pub fn get(&self, slot: u16) -> Option<MapModel> {
        self.world.get(slot as usize).copied().flatten()
    }
}

/// The fields of a `cube::Cell` (0x68 bytes, `region+0x14018`) the map reads.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MapCell {
    /// `+0`, `+8`: centre, fixed-point.
    pub x: i64,
    /// See `x`.
    pub y: i64,
    /// `+0x14`: height in blocks.
    pub height: f32,
    /// `+0x18`: cell type (1 = home town).
    pub kind: i32,
    /// `+0x34`: mission state (0 none, 1 = icon on the mission zone, else on the cell).
    pub mission: i32,
    /// `+0x41`: mission progress (2 = done: no icon; 1 = active: the zone is lit).
    pub progress: u8,
    /// `+0x4c`, `+0x50`: the mission's zone.
    pub mission_zone: [i32; 2],
}

/// World queries of the map (the client's `cube::World` at `WorldMap+0xac`).
pub trait MapWorld {
    /// `nearestClimateRegion 0x005cade0`: `(-1, -1)` when none.
    fn nearest_climate_region(&self, bx: i32, by: i32) -> (i32, i32);
    /// `World::getCell 0x00487da0` by cell coordinates (8×8 zones per cell).
    fn cell(&self, cx: i32, cy: i32) -> Option<MapCell>;
    /// `World::regionOfNearestClimatePoint(bx, by)`: the 64 cells of that region in memory
    /// order (`cx * 8 + cy`).
    fn region_cells(&self, bx: i32, by: i32) -> Option<Vec<MapCell>>;
}

fn map_cell(c: &cw_world::region::Cell) -> MapCell {
    MapCell {
        x: c.x,
        y: c.y,
        height: c.height,
        kind: c.kind,
        mission: c.mission.f34,
        progress: c.mission.b41,
        mission_zone: [c.mission.f4c as i32, (c.mission.f4c >> 32) as i32],
    }
}

impl MapWorld for cw_world::World {
    fn nearest_climate_region(&self, bx: i32, by: i32) -> (i32, i32) {
        cw_world::World::nearest_climate_region(self, bx, by)
    }
    fn cell(&self, cx: i32, cy: i32) -> Option<MapCell> {
        cw_world::World::cell(self, cx, cy).map(map_cell)
    }
    fn region_cells(&self, bx: i32, by: i32) -> Option<Vec<MapCell>> {
        let (rx, ry) = cw_world::World::nearest_climate_region(self, bx, by);
        let r = self.region(rx, ry)?;
        Some(r.cells.iter().map(map_cell).collect())
    }
}

/// The fields of a creature (`world+4` map value) the head pass reads. Offsets are from the
/// `cube::Creature` (the entity block starts at `+0x10`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MapCreature {
    /// `+0x10`: position, fixed-point (selection and depth key).
    pub position: [i64; 3],
    /// `+0x1350`: smoothed render position (where the head is drawn).
    pub render_position: [i64; 3],
    /// `+0x137c`: render yaw in degrees.
    pub yaw: f32,
    /// `+0x60`: hostility (0 player, 1 hostile, 3 pet).
    pub hostility: u8,
    /// `+0x7e`: appearance flags (`& 0x2000` = marked, e.g. a mission boss).
    pub flags: u32,
    /// `+0x140`: race (0x80..=0x87 animals with a map icon).
    pub race: u8,
    /// `+0x8c`: head model slot.
    pub head_model: u16,
    /// `+0x8e`: hair model slot.
    pub hair_model: u16,
    /// `+0x7a`: hair colour.
    pub hair_color: [u8; 3],
}

/// The `WorldMap` fields that carry over between calls.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MapState {
    /// `+0x8000b8`: sum of the frame times passed to every call (spins the mission icons).
    pub clock_ms: i32,
    /// `+0x44`: the last call's view with rows 0..2 scaled by 1/8 (blocks → map units).
    /// Uninitialised in the original before the first call; `None` here skips the draws
    /// that use it.
    pub stored_view: Option<D3dMatrix>,
    /// `+4`: the last call's projection.
    pub stored_projection: Option<D3dMatrix>,
    /// `+0x84`: a point 300 blocks from the local player away from the world centre,
    /// relative to the map centre in blocks (read elsewhere; see [`map_arrow`]).
    pub arrow: [f32; 3],
}

/// Everything [`map_draws`] reads besides the view.
pub struct MapScene<'a> {
    /// The tile cache (locked by the caller for the call).
    pub tiles: &'a MapTiles,
    /// Models.
    pub models: &'a MapModels,
    /// World queries.
    pub world: &'a dyn MapWorld,
    /// The world's creatures in map order (`world+4`, keyed by id).
    pub creatures: &'a [MapCreature],
    /// `world+0xb8 → +0x10`: the local player's position.
    pub player_position: [i64; 3],
    /// Persistent fields.
    pub state: &'a MapState,
}

/// The device state and CubeShader constants current when `render` calls the map.
#[derive(Clone, Debug, PartialEq)]
pub struct MapContext {
    /// Fixed state.
    pub state: PipelineState,
    /// Per-pass constants.
    pub uniforms: CubeUniforms,
    /// Per-draw constants.
    pub draw: DrawUniforms,
}

// ---------------------------------------------------------------------------------------
// Matrices
// ---------------------------------------------------------------------------------------

/// The map's projection and view (`0x005fc217..0x005fcd2e`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapMatrices {
    /// `P · T(ndc_x, ndc_y, -0.1)` with `P` the 45° left-handed perspective with the x axis
    /// mirrored, near 0.1 (the literals 1.0001 and -0.10001).
    pub projection: D3dMatrix,
    /// `Rz(yaw) · Rx(tilt) · Rz(0) · S(zoom) · T(0, 0, 300)`.
    pub view: D3dMatrix,
}

fn pre_rotate_x(m: &mut D3dMatrix, degrees: f32) {
    let r = (degrees * 0.017_453_292) as f64;
    let (c, s) = (r.cos() as f32, r.sin() as f32);
    for j in 0..4 {
        let (r1, r2) = (m[1][j], m[2][j]);
        m[1][j] = c * r1 + s * r2;
        m[2][j] = c * r2 - r1 * s;
    }
}

/// `0x005fc217..0x005fcd2e`.
pub fn map_matrices(v: &MapView) -> MapMatrices {
    // 0x005fc217: f = 1 / tan(pi/8) (double tan, then single precision).
    let f = 1.0f32 / (0.392_699_092_626_571_66f64.tan() as f32);
    let (w, h) = (v.viewport[0] as f32, v.viewport[1] as f32);
    let sx = -(f / (w / h));
    // 0x005fc26f: T = identity with row 3 = fx·row0 + fy·row1 - 0.1·row2 + row3.
    let fx = ((v.center_pixel[0] as f32 - w * 0.5) / w) * 2.0;
    let fy = ((v.center_pixel[1] as f32 - h * 0.5) / h) * 2.0;
    let mut t = IDENTITY;
    for j in 0..4 {
        t[3][j] = fx * IDENTITY[0][j] + fy * IDENTITY[1][j] - IDENTITY[2][j] * 0.1 + IDENTITY[3][j];
    }
    // 0x005fc3..: rows of P · T.
    let mut p = t;
    for j in 0..4 {
        p[0][j] = t[0][j] * sx + t[1][j] * 0.0 + t[2][j] * 0.0 + t[3][j] * 0.0;
        p[1][j] = f * t[1][j] + t[0][j] * 0.0 + t[2][j] * 0.0 + t[3][j] * 0.0;
        p[2][j] = t[2][j] * 1.0001 + (t[1][j] * 0.0 + t[0][j] * 0.0) + t[3][j];
        p[3][j] = (t[1][j] * 0.0 + t[0][j] * 0.0) - t[2][j] * 0.10001 + t[3][j] * 0.0;
    }
    // 0x005fc6d0: view.
    let mut m = IDENTITY;
    pre_translate(&mut m, [0.0, 0.0, 300.0]);
    if v.zoom != 1.0 {
        for row in m.iter_mut().take(3) {
            for c in row.iter_mut() {
                *c *= v.zoom;
            }
        }
    }
    pre_rotate_z(&mut m, 0.0);
    pre_rotate_x(&mut m, v.rotation[0]);
    pre_rotate_z(&mut m, v.rotation[2]);
    MapMatrices { projection: p, view: m }
}

/// `0x005ffdc0..`: the view as stored at `WorldMap+0x44` (rows 0..2 × 0.125).
pub fn stored_view(view: &D3dMatrix) -> D3dMatrix {
    let mut m = *view;
    for row in m.iter_mut().take(3) {
        for c in row.iter_mut() {
            *c *= 0.125;
        }
    }
    m
}

/// `0x005fc6a0..`: the copy of the stored projection with `[3][2] -= 80` (a depth bias
/// toward the camera) used by the border posts and the creature heads.
pub fn biased_projection(stored: &D3dMatrix) -> D3dMatrix {
    let mut m = *stored;
    m[3][2] -= 80.0;
    m
}

// ---------------------------------------------------------------------------------------
// Coordinates
// ---------------------------------------------------------------------------------------

fn block_of(c: i64) -> i32 {
    (c / 65536) as i32
}

/// `(b + (b >> 31 & (d-1))) >> k`, then minus one for a negative fixed-point coordinate.
fn floor_div_fixed(c: i64, d: i32) -> i32 {
    let mut v = block_of(c) / d;
    if c < 0 {
        v -= 1;
    }
    v
}

/// x or y of the centre's offset from its zone origin, negated, in map units (8 blocks):
/// `c·256/65536/65536` is the zone, `c·8192/65536` the centre in 1/65536 map units
/// (`vec3i64::mulFloat` is `v·k/65536`).
fn zone_local_offset(c: i64) -> f32 {
    let e = (c.wrapping_mul(256) / 65536) / 65536;
    let f = c.wrapping_mul(8192) / 65536;
    let off = f - (e << 21);
    let neg = off.wrapping_mul(-65536) / 65536;
    neg as f32 * INV_FIXED
}

/// The z translation of a zone-mode model: `(ftol(z·65536) - ftol(cz·0.125)) / 65536`.
fn zone_z(z: f32, cz: i64) -> f32 {
    let a = (z * 65536.0) as i64;
    let b = cz / 8;
    (a - b) as f32 * INV_FIXED
}

/// The zones the zone mode visits (`0x005fd694..0x005fd7c0`), in draw order: `(dx, dy)`
/// relative to the centre zone.
pub fn zone_mode_cells(v: &MapView) -> (i32, i32, Vec<(i32, i32)>) {
    let zx = floor_div_fixed(v.center[0], 256);
    let zy = floor_div_fixed(v.center[1], 256);
    let r = v.radius;
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (-r, r, -r, r);
    if r == 0 {
        if block_of(v.center[0]) % 256 < 0x80 {
            lo_x = -1;
        } else {
            hi_x = 1;
        }
        if block_of(v.center[1]) % 256 < 0x80 {
            lo_y = -1;
        } else {
            hi_y = 1;
        }
    }
    let mut out = Vec::new();
    for dx in lo_x..=hi_x {
        for dy in lo_y..=hi_y {
            out.push((dx, dy));
        }
    }
    (zx, zy, out)
}

/// The noise-warped climate query position (`0x005eefa0` plus the callers'
/// `(int)((float)b + warp)`), as `cw_world::World::warped_query`.
pub fn warped_query(bx: i32, by: i32) -> (i32, i32) {
    let wx = cw_math::value_noise_2d(f64::from(by) * 0.0005, 3423.0) * 3.0 * 256.0;
    let wy = cw_math::value_noise_2d(f64::from(bx) * 0.0005, 23421.0) * 3.0 * 256.0;
    ((bx as f32 + wx) as i32, (by as f32 + wy) as i32)
}

/// `0x005eeee0`: squared block distance between a climate point and a query position.
pub fn point_dist_sq(p: [i32; 2], qx: i32, qy: i32) -> f32 {
    let dx = ((i64::from(p[0]) << 16) - (i64::from(qx) << 16)) as f32 * 1.525_878_9e-5;
    let dy = ((i64::from(p[1]) << 16) - (i64::from(qy) << 16)) as f32 * 1.525_878_9e-5;
    dy * dy + dx * dx
}

/// `0x00601cc0`: the landscape tile of the climate region nearest to block `(bx, by)`,
/// `None` when any of the 3×3 around lacks a tile model.
pub fn nearest_region_tile(tiles: &MapTiles, bx: i32, by: i32) -> Option<&RegionTile> {
    nearest_region_key(tiles, bx, by).and_then(|k| tiles.region_tile(k.0, k.1))
}

/// [`nearest_region_tile`] by region coordinates.
pub fn nearest_region_key(tiles: &MapTiles, bx: i32, by: i32) -> Option<(i32, i32)> {
    let rx0 = (bx - 0x4000) / 0x4000;
    let ry0 = (by - 0x4000) / 0x4000;
    let rx1 = (bx + 0x4000) / 0x4000;
    let ry1 = (by + 0x4000) / 0x4000;
    let (qx, qy) = warped_query(bx, by);
    let mut best: Option<(i32, i32)> = None;
    let mut best_d = 0i32;
    for rx in rx0..=rx1 {
        for ry in ry0..=ry1 {
            if rx < 0 || ry < 0 || rx > 0x3ff || ry > 0x3ff {
                return None;
            }
            let t = tiles.region_tile(rx, ry)?;
            t.tile.as_ref()?;
            let d = point_dist_sq(t.point, qx, qy) as i32;
            if best.is_none() || d < best_d {
                best = Some((rx, ry));
                best_d = d;
            }
        }
    }
    best
}

/// `0x00600380..0x00600550`: the point 300 blocks from the player away from the world
/// centre (`0x80 << 32` fixed), relative to the map centre, in blocks (`WorldMap+0x84`).
pub fn map_arrow(player: [i64; 3], center: [i64; 3]) -> [f32; 3] {
    let world_center = 0x80i64 << 32;
    let mut x = (player[0] - world_center) as f32 * INV_FIXED;
    let mut y = (player[1] - world_center) as f32 * INV_FIXED;
    let l2 = y * y + x * x + 0.0 * 0.0;
    if !(l2 <= 0.0) {
        let inv = 1.0 / ((l2 as f64).sqrt() as f32);
        x = x * inv * 300.0;
        y = y * inv * 300.0;
    }
    let ax = ((x * 65536.0) as i64 - center[0] + player[0]) as f32 * INV_FIXED;
    let ay = ((y * 65536.0) as i64 - center[1] + player[1]) as f32 * INV_FIXED;
    [ax, ay, 0.0]
}

// ---------------------------------------------------------------------------------------
// Draws
// ---------------------------------------------------------------------------------------

/// Which part of `0x005fc1b0` a draw comes from (for tests and debugging; the `origin` of
/// each [`Draw`] names the call site).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapPass {
    /// `0x005fd60a`.
    RegionTiles,
    /// `0x005fe29b`.
    Placeholders,
    /// `0x005fea65`.
    ZoneTiles,
    /// `0x005fedfd`.
    BorderPosts,
    /// `0x005ff7a6` (mission) and `0x005ffdaa` (home town).
    MissionIcons,
    /// `0x00600680..0x00601c78`.
    CreatureHeads,
}

/// Draw origins (the `Model::draw 0x004e6df0` call sites inside `0x005fc1b0`).
pub const ORIGIN_REGION_TILE: u32 = 0x005f_d60a;
/// Placeholder.
pub const ORIGIN_PLACEHOLDER: u32 = 0x005f_e29b;
/// Zone tile.
pub const ORIGIN_ZONE_TILE: u32 = 0x005f_ea65;
/// Border post.
pub const ORIGIN_BORDER_POST: u32 = 0x005f_edfd;
/// Mission icon.
pub const ORIGIN_MISSION: u32 = 0x005f_f7a6;
/// Home-town marker.
pub const ORIGIN_CITY: u32 = 0x005f_fdaa;
/// Creature heads (the loop; the individual calls are inside helpers).
pub const ORIGIN_HEADS: u32 = 0x0060_0680;

struct Emitter<'a> {
    out: Vec<Draw>,
    uniforms: &'a mut Vec<CubeUniforms>,
    cur: CubeUniforms,
    pushed: Option<usize>,
    state: PipelineState,
    draw: DrawUniforms,
}

impl Emitter<'_> {
    fn uni(&mut self, f: impl FnOnce(&mut CubeUniforms)) {
        let before = self.cur.clone();
        f(&mut self.cur);
        if self.cur != before {
            self.pushed = None;
        }
    }

    /// `setMaterialColor`, `setTransforms(world, view, proj)`, `Model::draw`.
    fn model(&mut self, origin: u32, model: ModelRef, world: D3dMatrix, view: &D3dMatrix, proj: &D3dMatrix, material: [f32; 4]) {
        self.uni(|u| {
            u.view = *view;
            u.projection = *proj;
        });
        let idx = match self.pushed {
            Some(i) => i,
            None => {
                self.uniforms.push(self.cur.clone());
                let i = self.uniforms.len() - 1;
                self.pushed = Some(i);
                i
            }
        };
        self.draw.world = world;
        self.draw.material = material;
        self.out.push(Draw { origin, state: self.state, uniforms: idx, draw: self.draw.clone(), geometry: Geometry::Model { model } });
    }
}

/// `setLight` of `0x00600633`: front white, back (0.2, 0.3, 0.4), ambient 0.4, direction
/// `(0, 0.5, 1)/sqrt(1.25)` (normalised by the setter).
pub fn map_head_light(u: &mut CubeUniforms) {
    let k = 1.0f32 / ((0.0f32 * 0.0 + 0.25 + 1.0) as f64).sqrt() as f32;
    let d = [k * 0.0, k * 0.5, k];
    let l = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) as f64).sqrt() as f32;
    u.light_direction = [d[0] / l, d[1] / l, d[2] / l];
    u.light_front = [1.0, 1.0, 1.0, 1.0];
    u.light_back = [0.2, 0.3, 0.4, 1.0];
    u.ambient = [0.4, 0.4, 0.4, 1.0];
}

/// The creatures the head pass draws, sorted (`0x006000a0..0x00600380`): players,
/// hostile creatures with flag 0x2000, and at zoom ≥ 1.6 any creature of race 0x80..=0x87,
/// within `max(radius, 1)·256` blocks of the centre on both axes (low 32 bits of the
/// fixed-point difference, as compiled); key `-z/w` through the stored view, ascending
/// (far to near), with the MSVC `std::sort` (`cw_math::sort::msvc_sort`, not stable).
pub fn collect_heads<'c>(v: &MapView, creatures: &'c [MapCreature], view: &D3dMatrix) -> Vec<&'c MapCreature> {
    let r = if v.radius < 1 { 1 } else { v.radius };
    let limit = (r << 8) as f32;
    let mut picked: Vec<(&MapCreature, f32)> = Vec::new();
    for e in creatures {
        let h = e.hostility;
        let wanted = h == 0
            || (h == 1 && e.flags & 0x2000 != 0)
            || (!(v.zoom < 1.6) && (0x80..=0x87).contains(&e.race));
        if !wanted {
            continue;
        }
        let dx = (v.center[0] as i32).wrapping_sub(e.position[0] as i32) as f32 * INV_FIXED;
        if limit < dx.abs() {
            continue;
        }
        let dy = (v.center[1] as i32).wrapping_sub(e.position[1] as i32) as f32 * INV_FIXED;
        if limit < dy.abs() {
            continue;
        }
        let p = [
            (e.position[0] - v.center[0]) as f32 * INV_FIXED,
            (e.position[1] - v.center[1]) as f32 * INV_FIXED,
            (e.position[2] - v.center[2]) as f32 * INV_FIXED,
        ];
        let col = |j: usize| view[0][j] * p[0] + view[1][j] * p[1] + view[2][j] * p[2] + view[3][j];
        let inv_w = 1.0 / col(3);
        picked.push((e, -(inv_w * col(2))));
    }
    cw_math::sort::msvc_sort(&mut picked, |a, b| a.1 < b.1);
    picked.into_iter().map(|p| p.0).collect()
}

fn centred(m: &D3dMatrix, size: [i32; 3]) -> D3dMatrix {
    let mut r = *m;
    pre_translate(&mut r, [-size[0] as f32 * 0.5, -size[1] as f32 * 0.5, -size[2] as f32 * 0.5]);
    r
}

/// The draws of one `cube::WorldMap::draw` call. Uniform blocks the draws reference are
/// appended to `uniforms` (the frame's [`FrameCommands::uniforms`]). Pure: the side
/// effects of the call (clock, fades, stored matrices, arrow) are applied by
/// [`map_advance`].
pub fn map_draws(v: &MapView, scene: &MapScene, ctx: &MapContext, uniforms: &mut Vec<CubeUniforms>) -> Vec<Draw> {
    let mats = map_matrices(v);
    let (proj, view) = (mats.projection, mats.view);
    let st = scene.state;
    // 0x005fc211: the clock is advanced before anything reads it.
    let clock = st.clock_ms.wrapping_add(v.frame_ms);
    let biased = st.stored_projection.as_ref().map(biased_projection);
    let mut state = ctx.state;
    // 0x005fcd3e..0x005fce1f: ZENABLE=1, VS00+PS01, fog distance 1e6, zero point lights.
    state.depth.test = true;
    state.program = Program::WORLD;
    let mut cur = ctx.uniforms.clone();
    cur.fog_scale = 1.0 / 1_000_000.0;
    let mut draw = ctx.draw.clone();
    draw.light_set = 0;
    let mut em = Emitter { out: Vec::new(), uniforms, cur, pushed: None, state, draw };
    let tiles = scene.tiles;
    let one = [1.0f32; 4];

    if v.zoom < 0.35 {
        // 0x005fce48..0x005fd694: landscape tiles.
        let mut rx0 = floor_div_fixed(v.center[0], 16384);
        let mut ry0 = floor_div_fixed(v.center[1], 16384);
        let (nx, ny) = scene.world.nearest_climate_region(block_of(v.center[0]), block_of(v.center[1]));
        if nx >= 0 {
            rx0 = nx;
            ry0 = ny;
        }
        let r = v.radius;
        for dx in -r..=r {
            for dy in -r..=r {
                let (rx, ry) = (rx0 + dx, ry0 + dy);
                if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) {
                    continue;
                }
                let Some(t) = tiles.region_tile(rx, ry) else { continue };
                let Some(model) = &t.tile else { continue };
                if !t.visible {
                    continue;
                }
                let mut m = IDENTITY;
                pre_scale(&mut m, [32.0, 32.0, 32.0]);
                let ty = (((((t.min_zone[1].wrapping_shl(8)) as i64) << 16) - v.center[1]) as f64 * 0.00390625) as i64;
                let tx = (((((t.min_zone[0].wrapping_shl(8)) as i64) << 16) - v.center[0]) as f64 * 0.00390625) as i64;
                pre_translate(&mut m, [tx as f32 * INV_FIXED, ty as f32 * INV_FIXED, -1.0]);
                if t.elevation < 0 {
                    pre_translate(&mut m, [0.0, 0.0, -0.5]);
                } else {
                    let s = t.elevation as f32 * 0.00390625 + 1.0;
                    pre_scale(&mut m, [1.0, 1.0, s]);
                }
                let k = ((rx + ry * 2) % 4) as f32 * 0.1 + 0.8;
                let clamp = |x: f32| if !(x <= 1.0) { 1.0 } else { x };
                let mat = [clamp(k * 0.2), clamp(k * 0.4), clamp(k * 1.0), 1.0];
                em.model(ORIGIN_REGION_TILE, model.model, m, &view, &proj, mat);
            }
        }
    } else {
        // 0x005fd694..0x005fee56: zones.
        let (zx0, zy0, cells) = zone_mode_cells(v);
        let off = [zone_local_offset(v.center[0]), zone_local_offset(v.center[1])];
        let cb = [block_of(v.center[0]), block_of(v.center[1]), block_of(v.center[2])];
        for (dx, dy) in cells {
            let (zx, zy) = (zx0 + dx, zy0 + dy);
            if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
                continue;
            }
            let e = tiles.entry(zx, zy);
            let flags = e.map_or(0, |e| e.flags);
            let cell = scene.world.cell(zx / 8, zy / 8);
            let placement = |z: f32| {
                let mut m = IDENTITY;
                pre_translate(&mut m, [off[0], off[1], 0.0]);
                pre_translate(&mut m, [(dx << 5) as f32, (dy << 5) as f32, zone_z(z, v.center[2])]);
                m
            };
            // 0x005fd8..0x005fe29b: placeholder.
            let needs_placeholder = e.is_none_or(|e| e.tile.is_none() || e.fade != 0);
            if v.full
                && needs_placeholder
                && let Some(ph) = scene.models.get(MODEL_MAPTILE)
            {
                let mut z = 0.0f32;
                if let Some(rt) = nearest_region_tile(tiles, zx << 8, zy << 8) {
                    z = (rt.elevation - 100) as f32 * 0.125;
                }
                if let Some(e) = e
                    && e.tile.is_some()
                {
                    z += (e.fade as f32 / 250.0 - 1.0) * 20.0;
                }
                let mut mat = [0.2, 0.3, 1.0, 1.0];
                if flags != 0 {
                    mat[1] = if flags & 1 == 0 { 0.35 } else { 0.4 };
                }
                let mut m = placement(z);
                for row in m.iter_mut().take(3) {
                    for c in row.iter_mut() {
                        *c *= 4.0;
                    }
                }
                em.model(ORIGIN_PLACEHOLDER, ph.model, m, &view, &proj, mat);
            }
            // 0x005fe2b0..0x005fea9c: the zone tile.
            if let Some(e) = e
                && let Some(t) = &e.tile
            {
                let z = e.base_z as f32 - e.fade as f32 * 0.1;
                let m = placement(z);
                let lit = cell.is_some_and(|c| c.mission != 0 && c.progress == 1 && c.mission_zone == [zx, zy]);
                let mut mat = one;
                if !lit && v.full && flags & 1 == 0 {
                    mat[0] *= 0.6;
                    mat[1] *= 0.6;
                    mat[2] *= 0.6;
                }
                em.model(ORIGIN_ZONE_TILE, t.model, m, &view, &proj, mat);
            }
            // 0x005fea9c..0x005fee56: border posts, previous frame's view, biased projection.
            if let (Some(post), Some(e), Some(sv), Some(bp)) = (scene.models.border_post, e, st.stored_view.as_ref(), biased.as_ref()) {
                for p in &e.posts {
                    let mut m = IDENTITY;
                    pre_translate(&mut m, [(p[0] - cb[0]) as f32, (p[1] - cb[1]) as f32, (p[2] - cb[2]) as f32]);
                    for row in m.iter_mut().take(3) {
                        for c in row.iter_mut() {
                            *c *= 5.0;
                        }
                    }
                    em.model(ORIGIN_BORDER_POST, post.model, m, sv, bp, one);
                }
            }
        }
    }

    // 0x005fee56..0x005ffdc0: mission icons and the home-town marker (full map).
    if v.full
        && let Some(sv) = st.stored_view.as_ref()
        && let Some(cells) = scene.world.region_cells(block_of(v.center[0]), block_of(v.center[1]))
    {
        let icon_scale = {
            let s = 8.0 / v.zoom;
            if !(2.0 <= s) {
                2.0
            } else if !(s <= 10.0) {
                10.0
            } else {
                s
            }
        };
        for c in &cells {
            let z = ((c.height * 65536.0) as i64 - v.center[2] + 0x64_0000) as f32 * INV_FIXED;
            if c.mission != 0 && c.progress != 2 {
                let (x, y) = if c.mission != 1 {
                    ((c.x - v.center[0]) as f32 * INV_FIXED, (c.y - v.center[1]) as f32 * INV_FIXED)
                } else {
                    let fx = ((c.mission_zone[0].wrapping_shl(8).wrapping_add(0x80)) as i64) << 16;
                    let fy = ((c.mission_zone[1].wrapping_shl(8).wrapping_add(0x80)) as i64) << 16;
                    ((fx - v.center[0]) as f32 * INV_FIXED, (fy - v.center[1]) as f32 * INV_FIXED)
                };
                if let Some(icon) = scene.models.get(MODEL_MISSION) {
                    let mut m = IDENTITY;
                    pre_translate(&mut m, [x, y, z]);
                    pre_rotate_z(&mut m, clock as f32 * 0.02);
                    pre_scale(&mut m, [icon_scale; 3]);
                    pre_translate(&mut m, [-icon.size[0] as f32 * 0.5, -icon.size[1] as f32 * 0.5, 0.0]);
                    em.model(ORIGIN_MISSION, icon.model, m, sv, &proj, one);
                }
            }
            if c.kind == 1 {
                let dx = (scene.player_position[0] as i32).wrapping_sub(c.x as i32) as f32 * INV_FIXED;
                let dy = (scene.player_position[1] as i32).wrapping_sub(c.y as i32) as f32 * INV_FIXED;
                if !(dy * dy + dx * dx <= 262_144.0)
                    && let Some(city) = scene.models.get(MODEL_CITY)
                {
                    let mut m = IDENTITY;
                    pre_translate(&mut m, [(c.x - v.center[0]) as f32 * INV_FIXED, (c.y - v.center[1]) as f32 * INV_FIXED, z]);
                    pre_scale(&mut m, [icon_scale; 3]);
                    pre_translate(&mut m, [-city.size[0] as f32 * 0.5, -city.size[1] as f32 * 0.5, 0.0]);
                    em.model(ORIGIN_CITY, city.model, m, sv, &proj, one);
                }
            }
        }
    }

    // 0x005ffdc0..0x006000a0: stored matrices (this call's), fog distance 1e9.
    let sv_now = stored_view(&view);
    em.uni(|u| u.fog_scale = 1.0 / 1e9);
    // 0x006000a0..0x00600638: pick and sort the creatures, then setLight.
    let heads = collect_heads(v, scene.creatures, &sv_now);
    em.uni(map_head_light);
    let Some(bp) = biased else { return em.out };
    let black = [0.0, 0.0, 0.0, 1.0];
    let (s_black, s_white) = (5.0 / v.zoom, 4.0 / v.zoom);
    for c in heads {
        let p = [
            (c.render_position[0] - v.center[0]) as f32 * INV_FIXED,
            (c.render_position[1] - v.center[1]) as f32 * INV_FIXED,
            (c.render_position[2] - v.center[2]) as f32 * INV_FIXED,
        ];
        let race_model = RACE_ICONS.iter().find(|r| r.0 == c.race).and_then(|r| scene.models.get(r.1));
        // Black silhouette, ZWRITE=0 (0x00600680..0x00600e..).
        let mut m = IDENTITY;
        pre_translate(&mut m, p);
        pre_scale(&mut m, [s_black; 3]);
        em.state.depth.write = false;
        let parts = |m: &D3dMatrix, white: bool, em: &mut Emitter| {
            match c.hostility {
                1 => {
                    if let Some(sk) = scene.models.get(MODEL_SKULL) {
                        let mut r = *m;
                        pre_rotate_z(&mut r, c.yaw);
                        em.model(ORIGIN_HEADS, sk.model, centred(&r, sk.size), &sv_now, &bp, if white { one } else { black });
                    }
                }
                3 => {
                    if let Some(rm) = race_model {
                        let mut r = *m;
                        for row in r.iter_mut().take(3) {
                            for x in row.iter_mut() {
                                *x *= 0.5;
                            }
                        }
                        pre_rotate_z(&mut r, -v.rotation[2]);
                        em.model(ORIGIN_HEADS, rm.model, centred(&r, rm.size), &sv_now, &bp, if white { one } else { black });
                    }
                }
                _ => {
                    if let Some(hd) = scene.models.get(c.head_model) {
                        let mut r = *m;
                        pre_rotate_z(&mut r, c.yaw);
                        em.model(ORIGIN_HEADS, hd.model, centred(&r, hd.size), &sv_now, &bp, if white { one } else { black });
                    }
                    if let Some(hr) = scene.models.get(c.hair_model) {
                        let mut r = *m;
                        pre_rotate_z(&mut r, c.yaw);
                        let col = c.hair_color;
                        let mat = if white {
                            [f32::from(col[0]) / 255.0, f32::from(col[1]) / 255.0, f32::from(col[2]) / 255.0, 1.0]
                        } else {
                            black
                        };
                        em.model(ORIGIN_HEADS, hr.model, centred(&r, hr.size), &sv_now, &bp, mat);
                    }
                }
            }
        };
        parts(&m, false, &mut em);
        // The head, ZWRITE=1 (0x00600e..0x00601c78).
        let mut m = IDENTITY;
        pre_translate(&mut m, p);
        pre_scale(&mut m, [s_white; 3]);
        em.state.depth.write = true;
        parts(&m, true, &mut em);
    }
    em.out
}

/// The state `0x005fc1b0` leaves behind: ZENABLE on, VS00+PS01, fog distance 1e9, the head
/// light, zero point lights, and ZWRITE on when a head was drawn (else as before).
pub fn map_exit(ctx: &MapContext, draws: &[Draw]) -> (PipelineState, CubeUniforms) {
    let mut s = ctx.state;
    s.depth.test = true;
    s.program = Program::WORLD;
    if draws.iter().any(|d| d.origin == ORIGIN_HEADS) {
        s.depth.write = true;
    }
    let mut u = ctx.uniforms.clone();
    u.fog_scale = 1.0 / 1e9;
    map_head_light(&mut u);
    (s, u)
}

/// The side effects of one call: the clock (`+0x8000b8 += frame_ms`), the fade of every
/// visited zone tile (`+0x2c -= frame_ms`, not below 0), the stored projection and view
/// (`+4`, `+0x44`) and the arrow point (`+0x84`). Call after [`map_draws`] with the same
/// view.
pub fn map_advance(v: &MapView, tiles: &mut MapTiles, state: &mut MapState, player_position: [i64; 3]) {
    state.clock_ms = state.clock_ms.wrapping_add(v.frame_ms);
    if !(v.zoom < 0.35) {
        let (zx0, zy0, cells) = zone_mode_cells(v);
        for (dx, dy) in cells {
            if let Some(e) = tiles.entry_mut(zx0 + dx, zy0 + dy)
                && e.tile.is_some()
            {
                e.fade = (e.fade - v.frame_ms).max(0);
            }
        }
    }
    let m = map_matrices(v);
    state.stored_projection = Some(m.projection);
    state.stored_view = Some(stored_view(&m.view));
    state.arrow = map_arrow(player_position, v.center);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoWorld;
    impl MapWorld for NoWorld {
        fn nearest_climate_region(&self, _: i32, _: i32) -> (i32, i32) {
            (-1, -1)
        }
        fn cell(&self, _: i32, _: i32) -> Option<MapCell> {
            None
        }
        fn region_cells(&self, _: i32, _: i32) -> Option<Vec<MapCell>> {
            None
        }
    }

    fn minimap(yaw: f32, zoom: f32) -> MapView {
        // passes::minimap_view for a 1280×720 screen.
        MapView {
            center: [(1000i64 << 24) + (100 << 16), (2000i64 << 24) + (200 << 16), 50 << 16],
            center_pixel: [1280 - 0xf0, 720 - 0xaa],
            viewport: [1280, 720],
            rotation: [-120.0, 0.0, -yaw],
            zoom,
            frame_ms: 16,
            radius: 1,
            full: false,
        }
    }

    fn close(a: f32, b: f64) -> bool {
        (f64::from(a) - b).abs() < 1e-5
    }

    #[test]
    fn minimap_matrices() {
        let m = map_matrices(&minimap(30.0, 0.8));
        let f = 1.0 / (std::f64::consts::PI / 8.0).tan();
        let p = m.projection;
        assert!(close(p[0][0], -f / (1280.0 / 720.0)));
        assert!(close(p[1][1], f));
        // Screen pixel (1040, 550) → NDC offsets 0.625, 0.527 (y not flipped: the minimap
        // sits in the top-right corner).
        assert!(close(p[2][0], 0.625));
        assert!(close(p[2][1], (550.0 - 360.0) / 720.0 * 2.0));
        assert!(close(p[2][2], 1.0001 - 0.1));
        assert_eq!(p[2][3], 1.0);
        assert!(close(p[3][2], -0.10001));
        assert_eq!(p[3][3], 0.0);
        // View: Rz(-30)·Rx(-120)·S(0.8), camera 300 units away.
        let z = 0.8f64;
        let (sa, ca) = (-30f64.to_radians().sin(), 30f64.to_radians().cos());
        let (sb, cb) = ((-120f64).to_radians().sin(), (-120f64).to_radians().cos());
        let v = m.view;
        let want = [
            [z * ca, z * sa * cb, z * sa * sb, 0.0],
            [-z * sa, z * ca * cb, z * ca * sb, 0.0],
            [0.0, -z * sb, z * cb, 0.0],
            [0.0, 0.0, 300.0, 1.0],
        ];
        for i in 0..4 {
            for j in 0..4 {
                assert!(close(v[i][j], want[i][j]), "view[{i}][{j}] = {} want {}", v[i][j], want[i][j]);
            }
        }
        let s = stored_view(&v);
        assert!(close(s[0][0], z * ca * 0.125));
        assert_eq!(s[3], v[3]);
    }

    #[test]
    fn zone_mode_ring_and_tile_draw() {
        let v = minimap(0.0, 0.8);
        let (zx, zy, cells) = zone_mode_cells(&v);
        assert_eq!((zx, zy), (1000, 2000));
        assert_eq!(cells.len(), 9);
        let mut tiles = MapTiles::new();
        assert!(tiles.ensure_record(1000 / 64, 2000 / 64));
        let (size, vox) = ([32, 32, 1], vec![[10, 20, 30]; 1024]);
        let mesh = mesh_tile_voxels(size, &vox);
        let r = tiles.set_zone_tile(1000, 2000, size, vox, mesh).unwrap();
        {
            let e = tiles.entry_mut(1000, 2000).unwrap();
            e.base_z = 6;
            e.fade = TILE_FADE_MS;
            e.flags |= 2;
        }
        let models = MapModels::default();
        let state = MapState::default();
        let scene = MapScene { tiles: &tiles, models: &models, world: &NoWorld, creatures: &[], player_position: v.center, state: &state };
        let ctx = MapContext {
            state: PipelineState {
                program: Program::GUI,
                depth: DepthState { test: false, write: true, func: CompareFunc::LessEqual },
                blend: BlendState::ALPHA,
                cull: Cull::None,
                color_write: 0xf,
                sampler0: SamplerState::DEFAULT,
            },
            uniforms: CubeUniforms {
                view: IDENTITY,
                projection: IDENTITY,
                camera_position: [0.0; 3],
                light_direction: [0.0, 0.0, 1.0],
                light_front: [1.0; 4],
                light_back: [0.0; 4],
                ambient: [0.0; 4],
                darkness: [0.0; 4],
                fog_scale: 0.01,
                fog_gradient_translation: 0.0,
                sky_color1: [0.0; 4],
                sky_color2: [0.0; 4],
                fog_color: [0.0; 4],
            },
            draw: DrawUniforms { world: IDENTITY, material: [1.0; 4], alpha: 1.0, white: 0.0, shininess: 0.0, light_set: 3 },
        };
        let mut uniforms = Vec::new();
        let draws = map_draws(&v, &scene, &ctx, &mut uniforms);
        assert_eq!(draws.len(), 1);
        let d = &draws[0];
        assert_eq!(d.geometry, Geometry::Model { model: r });
        assert!(d.state.depth.test);
        assert_eq!(d.state.program, Program::WORLD);
        assert_eq!(d.draw.light_set, 0);
        assert!(close(uniforms[d.uniforms].fog_scale, 1e-6));
        // Centre 100 blocks into the zone = 12.5 map units; z = 6 - 25 - 50/8.
        assert!(close(d.draw.world[3][0], -12.5));
        assert!(close(d.draw.world[3][1], -25.0));
        assert!(close(d.draw.world[3][2], 6.0 - 25.0 - 6.25));
        let mut st = MapState::default();
        map_advance(&v, &mut tiles, &mut st, v.center);
        assert_eq!(tiles.entry(1000, 2000).unwrap().fade, TILE_FADE_MS - 16);
        assert_eq!(st.clock_ms, 16);
        assert!(st.stored_view.is_some());
    }

    /// Region tiles with models for regions 9..=11 on both axes, climate points at the
    /// region centres.
    fn discover_tiles() -> MapTiles {
        let mut t = MapTiles::new();
        for rx in 9..=11 {
            for ry in 9..=11 {
                t.ensure_region_tile(rx, ry, [rx * 0x4000 + 0x2000, ry * 0x4000 + 0x2000], 0);
                let v = vec![[200u8; 3]];
                let mesh = mesh_tile_voxels([1, 1, 1], &v);
                t.set_region_model(rx, ry, [rx * 64, ry * 64], [1, 1, 1], v, mesh);
            }
        }
        t
    }

    #[test]
    fn discover_region_marks_the_tile_under_the_player() {
        let mut t = discover_tiles();
        let b = 10 * 0x4000 + 0x2000;
        let point = Some([b, b]);
        assert!(!t.region_tile(10, 10).unwrap().visible);
        // 0x0048d5bc: visible; 0x0048d5c9: 3000 - 0 > 2000, so the rebuild flag too.
        assert_eq!(t.discover_region(point, b, b, 3000), Some((10, 10)));
        let r = t.region_tile(10, 10).unwrap();
        assert!(r.visible);
        assert!(r.dirty);
        assert_eq!(t.region_discover_ms, 3000);
        // Neighbours untouched.
        assert!(!t.region_tile(9, 10).unwrap().visible);
        // Within two seconds: visible stays, no new rebuild request, clock kept.
        t.set_region_model(10, 10, [640, 640], [1, 1, 1], vec![[200; 3]], mesh_tile_voxels([1, 1, 1], &[[200; 3]]));
        assert!(!t.region_tile(10, 10).unwrap().dirty);
        assert_eq!(t.discover_region(point, b, b, 5000), Some((10, 10)));
        assert!(!t.region_tile(10, 10).unwrap().dirty);
        assert_eq!(t.region_discover_ms, 3000);
        assert_eq!(t.discover_region(point, b, b, 5001), Some((10, 10)));
        assert!(t.region_tile(10, 10).unwrap().dirty);
        assert_eq!(t.region_discover_ms, 5001);
    }

    #[test]
    fn discover_region_needs_the_matching_point_and_all_neighbours() {
        let b = 10 * 0x4000 + 0x2000;
        // No climate point (0x0048d5a3) or a different one (0x00468840 fails).
        let mut t = discover_tiles();
        assert_eq!(t.discover_region(None, b, b, 3000), None);
        assert_eq!(t.discover_region(Some([b, b + 1]), b, b, 3000), None);
        assert!(!t.region_tile(10, 10).unwrap().visible);
        assert_eq!(t.region_discover_ms, 0);
        // A neighbour without a model: 0x00601cc0 returns null.
        let mut t = discover_tiles();
        t.drop_region_tile(11, 9);
        assert_eq!(t.discover_region(Some([b, b]), b, b, 3000), None);
        assert!(!t.region_tile(10, 10).unwrap().visible);
    }

    #[test]
    fn saved_land_is_taken_once() {
        let mut t = MapTiles::new();
        let s = SavedLand { visible: true, min_zone: [1, 2], size: [1, 1, 1], voxels: vec![[255; 3]], point: [3, 4], elevation: 5 };
        t.stash_saved_land(7, 8, s.clone());
        assert_eq!(t.take_saved_land(7, 8), Some(s));
        assert_eq!(t.take_saved_land(7, 8), None);
    }

    #[test]
    fn border_post_model() {
        let (size, v) = border_post_voxels();
        assert_eq!(size, [4, 4, 4]);
        assert_eq!(v[5], [255, 255, 255]);
        assert_eq!(v[0], [10, 10, 10]);
        assert_eq!(v[16], [0, 0, 0]);
    }
}
