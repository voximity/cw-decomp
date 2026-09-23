//! The landscape thread (`Cube.exe 0x00469590`, started by the `GameController` constructor
//! through the lambda thunk `0x0046db80`) and the `cube::WorldMap` cache functions it calls:
//! `loadRegion 0x00603230`, `buildLandscapeTile 0x006024d0`, `buildZoneTile 0x00603a00` and
//! `evict 0x005fbed0`. It turns the client's `cw_world::World` into the
//! [`cw_render::map::MapTiles`] that `cube::WorldMap::draw` (`cw_render::map`) draws.
//!
//! Fidelity: Tier B for what a tile contains (which zone data is sampled, at what resolution,
//! the colours, the border posts), when tiles are built and evicted; Tier C for the threading
//! ([`LandscapeWorker`] is a plain thread fed by a channel) and for persistence (see below).
//!
//! # What a tile is
//!
//! - A **zone tile** (`cube::ZoneTile+8`) is a 32×32×n voxel model of one loaded zone: each
//!   voxel is an 8×8×8 block cell. The top voxel of a column of cells is the average colour of
//!   the topmost solid block of its 64 block columns (water colour for columns under water
//!   level, the terrain colour when the column has no solid block above its `f14` height), at
//!   the cell of the columns' average surface height; the cells below it, down to the cell
//!   under the lowest `f14`, are the average colour of their solid blocks. Border posts are
//!   collected every 32 blocks where the nearest climate point changes. Built when the zone
//!   is loaded in the client world and has no tile yet, rebuilt when its record is dirty (the
//!   zone thread marks it after `generateZone`), nearest first, at most ten per pass, within ±10 zones of the map centre
//!   (the player's zone plus the map pan).
//! - A **landscape tile** (`cube::LandscapeTile`, `WorldMap+0x4000b0`) is a 1-voxel-thick
//!   model of the zones a climate region owns (nearest climate point), one voxel per zone,
//!   coloured 200 (no record or never seen), 220 (has had a tile) or 255 (visited). Built for
//!   the regions within ±3 of the centre's region once their 3×3 climate points exist.
//!
//! # Persistence (Tier C)
//!
//! The original saves each zone tile (`"<prefix>" zx "," zy` blobs through `0x004499c0`, zlib
//! `0x005fc0d0`), each landscape tile and each region record (flags, zone records, cells) in
//! the client's database and reloads them when they come back in range, plus a
//! `"discovered"` counter. This port keeps the zone-tile blobs in memory
//! ([`LandscapeBuilder`]'s store), never evicts region records (so the visited flags survive
//! the session). Landscape tiles go through the map's saved-land store: `singleplayer::load_map`
//! fills it from the `"land"` blobs, [`MapTiles::drop_region_tile`] (eviction) puts the tile
//! back, and [`LandscapeBuilder::region_tile_from`] takes it when the region has no tile
//! object, then rebuilds it as the original does after loading. The visible byte `+0x28`
//! (set by `GameController::update` 0x0048d5bc for the region under the player) survives
//! both eviction and a restart that way.
//!
//! # Map of `0x00469590`
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x00469590..0x004696a0` | take the landscape (`+0x8005e8`) and world (`+0x8005d0`) sections; centre = `(int)(pan/256 + player zone)`; skip when the loaded world is not the map's world | [`landscape_centre`] |
//! | `0x004696a0..0x00469835` | `loadRegion` for the regions ±3 around the centre's | [`LandscapeBuilder::load_region`] |
//! | `0x00469835..0x00469993` | `buildLandscapeTile` for the same regions | [`LandscapeBuilder::region_tile`] |
//! | `0x00469993..0x00469b50` | ten times: the nearest zone within [-10, 10) needing a tile → `buildZoneTile` | [`LandscapeBuilder::iterate`], [`LandscapeBuilder::zone_tile`] |
//! | `0x00469b50..0x00469bb0` | every second: `evict` | [`LandscapeBuilder::evict`] |
//! | `0x00469bb0..` | leave the landscape section, `Sleep(10)` | [`LandscapeWorker`] |

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cw_render::map::{MapTiles, mesh_tile_voxels, point_dist_sq, warped_query};
use cw_world::World;
use cw_world::climate::ClimatePoint;
use cw_world::zone::{Column, Zone};

use crate::threads::FrameGate;

/// `ZoneTile` fade start (`0x00603a00`: `+0x2c = 0xfa`).
const FADE_MS: i32 = cw_render::map::TILE_FADE_MS;

/// The water colour of the tile builders (block types 2 and 3).
const WATER_RGB: [f32; 3] = [80.0, 100.0, 255.0];

/// A zone tile as `0x00603a00` computes it (and saves it: the blob holds the version
/// `WorldMap+0x8000f8`, `base_z`, the size, the zlib voxels and the posts).
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltZoneTile {
    /// `ZoneTile+4`: z of voxel layer 0 in 8-block cells.
    pub base_z: i32,
    /// 32, 32, layers.
    pub size: [i32; 3],
    /// `(z * 32 + y) * 32 + x`.
    pub voxels: Vec<[u8; 3]>,
    /// Border posts in blocks, list order.
    pub posts: Vec<[i32; 3]>,
}

/// `(int)(pan * (1/256) + (float)playerZone)` per axis (`0x004695d0..0x0046960b`): the
/// zone the landscape thread centres on. `pan` is `GameController+0x1000e4c` in blocks,
/// `player_zone` `GameController+0x2bc`.
pub fn landscape_centre(player_zone: [i32; 2], pan: [f32; 2]) -> [i32; 2] {
    [
        (pan[0] * 0.00390625 + player_zone[0] as f32) as i32,
        (pan[1] * 0.00390625 + player_zone[1] as f32) as i32,
    ]
}

fn block_type(b: [u8; 4]) -> u8 {
    b[3] & 0x1f
}

/// The colour a solid block contributes (`0x006041..`): water types 2 and 3 are
/// (80, 100, 255), others their RGB bytes.
fn solid_rgb(b: [u8; 4]) -> [f32; 3] {
    if block_type(b).wrapping_sub(2) < 2 {
        WATER_RGB
    } else {
        [f32::from(b[0]), f32::from(b[1]), f32::from(b[2])]
    }
}

fn column(zone: &Zone, x: i32, y: i32) -> &Column {
    &zone.columns[(y * 256 + x) as usize]
}

/// The nearest climate point of a block (`World::nearestClimatePoint 0x00477e10`), by
/// position (the original compares pointers).
fn region_id(world: &World, bx: i32, by: i32) -> Option<(i32, i32)> {
    world.nearest_climate_point(bx, by).map(|p| (p.x, p.y))
}

/// The tile half of `buildZoneTile 0x00603a00` (`0x00603f32..0x00604ba0`) for a loaded zone.
pub fn build_zone_tile(world: &World, zone: &Zone) -> BuiltZoneTile {
    let mut b = ZoneTileBuild::new(zone);
    for cx in 0..32 {
        b.row(world, zone, cx);
    }
    b.finish()
}

/// [`build_zone_tile`] in units: the z range ([`ZoneTileBuild::new`]), then one unit per row of
/// 8-block cells (`cx`, the outer loop of `0x00604190..`), so the landscape thread can take the
/// world's read lock per row.
pub struct ZoneTileBuild {
    base_z: i32,
    layers: i32,
    voxels: Vec<[u8; 3]>,
    posts: Vec<[i32; 3]>,
}

impl ZoneTileBuild {
    /// 0x00603f3a..0x00603fb8: z range over every column: lowest f14 - 1, highest top.
    pub fn new(zone: &Zone) -> ZoneTileBuild {
        let c0 = column(zone, 0, 0);
        let (mut lo, mut hi) = (c0.f14, c0.f14);
        for y in 0..256 {
            for x in 0..256 {
                let c = column(zone, x, y);
                if c.f14 - 1 < lo {
                    lo = c.f14 - 1;
                }
                let top = c.height + c.blocks.len() as i32;
                if hi < top {
                    hi = top;
                }
            }
        }
        let lo = lo.max(0);
        let hi = hi.max(0);
        let layers = (hi - lo) / 8 + 1;
        ZoneTileBuild { base_z: lo / 8, layers, voxels: vec![[0u8; 3]; (32 * 32 * layers) as usize], posts: Vec::new() }
    }

    fn set(&mut self, x: i32, y: i32, z: i32, c: [f32; 3]) {
        if (0..32).contains(&x) && (0..32).contains(&y) && (0..self.layers).contains(&z) {
            self.voxels[((z * 32 + y) * 32 + x) as usize] = [c[0] as i32 as u8, c[1] as i32 as u8, c[2] as i32 as u8];
        }
    }

    /// The cells `(cx, 0..32)`.
    pub fn row(&mut self, world: &World, zone: &Zone, cx: i32) {
        let (zx, zy) = (zone.x, zone.y);
        for cy in 0..32 {
            // 0x00604190..0x006042b6: the surface colour and height of the 8×8 columns.
            let mut rgb = [0.0f32; 3];
            let mut hsum = 0.0f32;
            let mut min_f14 = 10_000_000;
            for j in 0..8 {
                for i in 0..8 {
                    let (x, y) = (cx * 8 + j, cy * 8 + i);
                    let c = column(zone, x, y);
                    if c.f14 < min_f14 {
                        min_f14 = c.f14;
                    }
                    let n = c.blocks.len() as i32;
                    if c.height + n < 1 {
                        // The static water block (type 2) of blockAt.
                        for k in 0..3 {
                            rgb[k] += WATER_RGB[k];
                        }
                        continue;
                    }
                    let (bx, by) = (zx * 256 + x, zy * 256 + y);
                    let mut k = n;
                    let h = loop {
                        k -= 1;
                        if k < 0 {
                            let t = world.terrain_color(bx, by, c.f14);
                            for m in 0..3 {
                                rgb[m] += t[m];
                            }
                            break c.height - 1;
                        }
                        let b = c.block_at(k);
                        if block_type(b) != 0 {
                            let t = solid_rgb(b);
                            for m in 0..3 {
                                rgb[m] += t[m];
                            }
                            break c.height + k;
                        }
                        if c.height + k == c.f14 - 1 {
                            let t = world.terrain_color(bx, by, c.f14);
                            for m in 0..3 {
                                rgb[m] += t[m];
                            }
                            break c.height + k;
                        }
                    };
                    hsum += h as f32;
                }
            }
            let avg = [rgb[0] * 0.015625, rgb[1] * 0.015625, rgb[2] * 0.015625];
            let t = (hsum * -0.015625) as i32;
            let top = -(t / 8) - self.base_z;
            // 0x006042f0..0x006043c0: border posts every 32 blocks.
            if cx % 4 == 0 && cy % 4 == 0 {
                let (x0, y0) = (zx * 256 + cx * 8, zy * 256 + cy * 8);
                let r0 = region_id(world, x0, y0);
                if r0 != region_id(world, x0 + 32, y0) || r0 != region_id(world, x0, y0 + 32) {
                    let z = column(zone, cx * 8, cy * 8).f14.max(0);
                    self.posts.push([x0, y0, z]);
                }
            }
            if 0 <= top && top < self.layers {
                self.set(cx, cy, top, avg);
            }
            // 0x00604470..0x00604b3e: the cells under the top, from the lowest f14.
            let mut k = ((min_f14 / 8) - self.base_z - 1).max(0);
            while k < top {
                let mut sum = [0.0f32; 3];
                let mut count = 0;
                for dx in 0..8 {
                    for dy in 0..8 {
                        for dz in 0..8 {
                            let (x, y, z) = (zx * 256 + cx * 8 + dx, zy * 256 + cy * 8 + dy, (self.base_z + k) * 8 + dz);
                            let b = world.block(x, y, z);
                            let ty = block_type(b);
                            if ty == 0 {
                                continue;
                            }
                            count += 1;
                            let c = if ty == 1 { world.terrain_color(x, y, z) } else { solid_rgb(b) };
                            for m in 0..3 {
                                sum[m] += c[m];
                            }
                        }
                    }
                }
                if count != 0 {
                    let r = 1.0f32 / count as f32;
                    self.set(cx, cy, k, [r * sum[0], r * sum[1], r * sum[2]]);
                }
                k += 1;
            }
        }
    }

    /// The finished tile.
    pub fn finish(self) -> BuiltZoneTile {
        BuiltZoneTile { base_z: self.base_z, size: [32, 32, self.layers], voxels: self.voxels, posts: self.posts }
    }
}

/// The climate points `(rx + dx, ry + dy)`, `dx, dy` in -1..=1, that `buildLandscapeTile`
/// reads, or `None` when it builds nothing (an unnamed world, a region out of range, a missing
/// point).
pub fn region_points(world: &World, rx: i32, ry: i32) -> Option<[[ClimatePoint; 3]; 3]> {
    if !world.has_name || !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) {
        return None;
    }
    let mut out = [[None; 3]; 3];
    for dx in -1..=1 {
        for dy in -1..=1 {
            out[(dx + 1) as usize][(dy + 1) as usize] = Some(*world.climate_point(rx + dx, ry + dy)?);
        }
    }
    Some(out.map(|r| r.map(|p| p.expect("filled above"))))
}

/// A way to run a unit of work with the world readable (the worker takes the read lock around
/// each call).
pub type WorldRead<'a> = dyn FnMut(&mut dyn FnMut(&World)) + 'a;

/// The landscape builder: the state the thread keeps between passes.
#[derive(Debug, Default)]
pub struct LandscapeBuilder {
    /// Zone-tile blobs (the original's save database), by zone.
    store: HashMap<(i32, i32), BuiltZoneTile>,
}

impl LandscapeBuilder {
    /// A builder with an empty store.
    pub fn new() -> LandscapeBuilder {
        LandscapeBuilder::default()
    }

    /// `loadRegion 0x00603230`: the region record (4096 `cube::ZoneTile`s) when missing.
    /// The zone records and cells it copies from the world feed the map overlay widget, not
    /// the draw, and are not kept here.
    pub fn load_region(&mut self, world: &World, tiles: &Mutex<MapTiles>, rx: i32, ry: i32) {
        if !world.has_name {
            return;
        }
        tiles.lock().expect("map tiles").ensure_record(rx, ry);
    }

    /// `buildLandscapeTile 0x006024d0`.
    pub fn region_tile(&mut self, world: &World, tiles: &Mutex<MapTiles>, rx: i32, ry: i32) {
        let points = region_points(world, rx, ry);
        self.region_tile_from(points, tiles, rx, ry);
    }

    /// [`LandscapeBuilder::region_tile`] from the region's 3x3 climate points (all the world
    /// data it reads, [`region_points`]), so the world lock is held only to copy them.
    pub fn region_tile_from(&mut self, points: Option<[[ClimatePoint; 3]; 3]>, tiles: &Mutex<MapTiles>, rx: i32, ry: i32) {
        // 0x0060257c..0x006027e0: no tile object: the `"land" rx "_" ry` blob, when the map
        // database has it, becomes the tile (+0x20 min zone, +0x28 visible, the model, +4..
        // the climate point), and the rebuild below runs whatever its +0x30. With a tile
        // object, only +0x30 rebuilds it.
        let loaded = {
            let mut t = tiles.lock().expect("map tiles");
            if t.region_tile(rx, ry).is_some() {
                false
            } else if let Some(s) = t.take_saved_land(rx, ry) {
                let r = t.ensure_region_tile(rx, ry, s.point, s.elevation);
                r.visible = s.visible;
                r.min_zone = s.min_zone;
                if s.size.iter().all(|&n| n > 0) {
                    let mesh = mesh_tile_voxels(s.size, &s.voxels);
                    t.set_region_model(rx, ry, s.min_zone, s.size, s.voxels, mesh);
                }
                true
            } else {
                false
            }
        };
        let Some(points) = points else { return };
        if !loaded && tiles.lock().expect("map tiles").region_tile(rx, ry).is_some_and(|t| !t.dirty) {
            return;
        }
        let own = points[1][1];
        tiles.lock().expect("map tiles").ensure_region_tile(rx, ry, [own.x, own.y], own.elevation);
        // 0x00602b..: a zone belongs to the region whose climate point is nearest to the
        // warped zone corner; a neighbour wins when its distance is below the truncated own
        // distance.
        let owns = |zx: i32, zy: i32| {
            let (qx, qy) = warped_query(zx << 8, zy << 8);
            let d = point_dist_sq([own.x, own.y], qx, qy);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let p = points[(dx + 1) as usize][(dy + 1) as usize];
                    if point_dist_sq([p.x, p.y], qx, qy) < (d as i32) as f32 {
                        return false;
                    }
                }
            }
            true
        };
        // First pass: bounding box, both ends inclusive.
        let mut bbox: Option<[i32; 4]> = None;
        for zx in rx * 64 - 64..=(rx + 2) * 64 {
            for zy in ry * 64 - 64..=(ry + 2) * 64 {
                if !owns(zx, zy) {
                    continue;
                }
                bbox = Some(match bbox {
                    None => [zx, zy, zx, zy],
                    Some(b) => [b[0].min(zx), b[1].min(zy), b[2].max(zx), b[3].max(zy)],
                });
            }
        }
        let Some([x0, y0, x1, y1]) = bbox else { return };
        let size = [x1 - x0 + 1, y1 - y0 + 1, 1];
        let mut voxels = vec![[0u8; 3]; (size[0] * size[1]) as usize];
        {
            let t = tiles.lock().expect("map tiles");
            // Second pass: upper ends exclusive (as compiled).
            for zx in rx * 64 - 64..(rx + 2) * 64 {
                for zy in ry * 64 - 64..(ry + 2) * 64 {
                    if !owns(zx, zy) {
                        continue;
                    }
                    let g = match t.entry(zx, zy).map(|e| e.flags) {
                        None | Some(0) => 200,
                        Some(f) if f & 1 != 0 => 255,
                        Some(_) => 220,
                    };
                    let (vx, vy) = (zx - x0, zy - y0);
                    if (0..size[0]).contains(&vx) && (0..size[1]).contains(&vy) {
                        voxels[(vy * size[0] + vx) as usize] = [g, g, g];
                    }
                }
            }
        }
        let mesh = mesh_tile_voxels(size, &voxels);
        tiles.lock().expect("map tiles").set_region_model(rx, ry, [x0, y0], size, voxels, mesh);
    }

    /// `buildZoneTile 0x00603a00`.
    pub fn zone_tile(&mut self, world: &World, tiles: &Mutex<MapTiles>, zx: i32, zy: i32) {
        self.zone_tile_with(&mut |f| f(world), tiles, zx, zy);
    }

    /// [`LandscapeBuilder::zone_tile`] with the world reached through `read` per unit: the
    /// checks, the z range, each row of cells ([`ZoneTileBuild`]). A zone unloaded between two
    /// units ends the build without a tile (the zone thread only unloads zones far from the
    /// player, which the next pass no longer asks for).
    pub fn zone_tile_with(&mut self, read: &mut WorldRead<'_>, tiles: &Mutex<MapTiles>, zx: i32, zy: i32) {
        let mut build: Option<ZoneTileBuild> = None;
        read(&mut |world| build = self.zone_tile_start(world, tiles, zx, zy));
        let Some(mut b) = build else { return };
        for cx in 0..32 {
            let mut gone = false;
            read(&mut |world| match world.zone(zx, zy) {
                Some(zone) => b.row(world, zone, cx),
                None => gone = true,
            });
            if gone {
                return;
            }
        }
        self.zone_tile_finish(tiles, zx, zy, b.finish());
    }

    /// The part of `buildZoneTile` before the tile is computed: the site record, the checks,
    /// the reload of a saved tile; the z range of a tile to build.
    fn zone_tile_start(&mut self, world: &World, tiles: &Mutex<MapTiles>, zx: i32, zy: i32) -> Option<ZoneTileBuild> {
        if !world.has_name || !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
            return None;
        }
        let zone = world.zone(zx, zy);
        {
            let mut t = tiles.lock().expect("map tiles");
            // 0x00603a9a..0x00603ac0: with the region record present, the zone's site record
            // is copied from the world region (`0x004a6ad0`: `Region+0x18 + ((zx & 63) * 64 +
            // (zy & 63)) * 16`, 16 bytes) into `ZoneTile+0x10`, every call, before the tile
            // checks. The cell copy that follows (0x00603ac6.., `0x005fb9f0` into the record's
            // cells) is not kept here (see `load_region`).
            if let Some(r) = world.region(zx >> 6, zy >> 6) {
                let rec = r.zones[((zx & 63) * 64 + (zy & 63)) as usize];
                if let Some(e) = t.entry_mut(zx, zy) {
                    e.site = rec;
                }
            }
            let e = t.entry(zx, zy)?;
            if e.tile.is_some() && (!e.dirty || zone.is_none()) {
                return None;
            }
            // 0x00603b1a..0x00603f2c: no model yet: reload the saved tile.
            if e.tile.is_none()
                && let Some(s) = self.store.get(&(zx, zy)).cloned()
            {
                let mesh = mesh_tile_voxels(s.size, &s.voxels);
                t.set_zone_tile(zx, zy, s.size, s.voxels, mesh);
                let e = t.entry_mut(zx, zy).expect("entry");
                e.base_z = s.base_z;
                e.flags |= 2;
                e.fade = FADE_MS;
                e.posts = s.posts;
                if !e.dirty {
                    return None;
                }
            }
            // 0x00603fb8: rebuilding: clear the dirty flag and the posts first.
            if zone.is_some() {
                let e = t.entry_mut(zx, zy).expect("entry");
                e.dirty = false;
                e.posts.clear();
            }
        }
        zone.map(ZoneTileBuild::new)
    }

    /// The part of `buildZoneTile` after the tile is computed: kept, meshed, set.
    fn zone_tile_finish(&mut self, tiles: &Mutex<MapTiles>, zx: i32, zy: i32, built: BuiltZoneTile) {
        self.store.insert((zx, zy), built.clone());
        let mesh = mesh_tile_voxels(built.size, &built.voxels);
        let mut t = tiles.lock().expect("map tiles");
        let had_model = t.entry(zx, zy).is_some_and(|e| e.tile.is_some());
        if t.set_zone_tile(zx, zy, built.size, built.voxels, mesh).is_none() {
            return;
        }
        let e = t.entry_mut(zx, zy).expect("entry");
        e.base_z = built.base_z;
        e.posts = built.posts;
        if !had_model {
            e.fade = FADE_MS;
        }
        e.flags |= 2;
    }

    /// One pass of the thread loop (`0x004695d0..0x00469b50`) around `centre` (zones).
    pub fn iterate(&mut self, world: &World, tiles: &Mutex<MapTiles>, centre: [i32; 2]) {
        self.iterate_with(&mut |f| f(world), tiles, centre);
    }

    /// [`LandscapeBuilder::iterate`] with the world reached through `read`, once per unit of
    /// work (a region record, a landscape tile's climate points, one search for the nearest
    /// zone, the checks and each row of cells of a zone tile): the worker takes the world's
    /// read lock inside `read`, so the zone thread and the frame can write between units.
    ///
    /// Performance note for a future optimiser: the original reads the world with no lock at
    /// all; one lock per unit costs a lock round trip each. A zone tile of a generated world
    /// takes 100..180 ms of CPU (`World::block` and `terrainColor` per voxel under the
    /// surface), which is why it is cut into rows.
    pub fn iterate_with(&mut self, read: &mut WorldRead<'_>, tiles: &Mutex<MapTiles>, centre: [i32; 2]) {
        let [cx, cy] = centre;
        let (rcx, rcy) = (cx / 64, cy / 64);
        for rx in rcx - 3..=rcx + 3 {
            for ry in rcy - 3..=rcy + 3 {
                read(&mut |w| self.load_region(w, tiles, rx, ry));
            }
        }
        for rx in rcx - 3..=rcx + 3 {
            for ry in rcy - 3..=rcy + 3 {
                // Only the climate points under the world lock; the tile outside it.
                let mut points = None;
                read(&mut |w| points = region_points(w, rx, ry));
                self.region_tile_from(points, tiles, rx, ry);
            }
        }
        // 0x00469993: ten times the nearest zone that needs a tile.
        for _ in 0..10 {
            let mut best: Option<((i32, i32), i32)> = None;
            read(&mut |world| {
                for x in cx - 10..cx + 10 {
                    for y in cy - 10..cy + 10 {
                        let need = {
                            let t = tiles.lock().expect("map tiles");
                            match t.entry(x, y) {
                                None => true,
                                Some(e) => (e.tile.is_none() || e.dirty) && (e.flags & 2 != 0 || world.zone(x, y).is_some()),
                            }
                        };
                        if need {
                            let d = (x - cx) * (x - cx) + (y - cy) * (y - cy);
                            if best.is_none_or(|b| d < b.1) {
                                best = Some(((x, y), d));
                            }
                        }
                    }
                }
            });
            if let Some(((x, y), _)) = best {
                self.zone_tile_with(read, tiles, x, y);
            }
        }
    }

    /// `evict 0x005fbed0`: zone tiles more than 10 zones from the centre on either axis and
    /// landscape tiles more than 8 regions away are dropped (their blobs stay in the store;
    /// a landscape tile's, visible byte included, as [`MapTiles::drop_region_tile`] keeps it,
    /// the state 0x006050b0 saved at its last rebuild).
    /// The original also saves and frees region records beyond 8 regions; they are kept here.
    pub fn evict(&mut self, tiles: &Mutex<MapTiles>, centre: [i32; 2]) {
        let [cx, cy] = centre;
        let mut t = tiles.lock().expect("map tiles");
        for (zx, zy) in t.tiled_zones() {
            if (zx - cx).abs() > 10 || (zy - cy).abs() > 10 {
                t.drop_zone_tile(zx, zy);
            }
        }
        let (rcx, rcy) = (cx / 64, cy / 64);
        for (rx, ry) in t.region_tile_keys() {
            if (rx - rcx).abs() > 8 || (ry - rcy).abs() > 8 {
                t.drop_region_tile(rx, ry);
            }
        }
    }
}

enum Msg {
    Centre([i32; 2]),
    Stop,
}

/// The landscape thread as a plain worker: it runs [`LandscapeBuilder::iterate_with`] with a
/// read lock of the world per unit (a region's climate points, a zone search, a row of a zone
/// tile; after the frame thread when it waits, `FrameGate`), [`LandscapeBuilder::evict`] once
/// a second, and sleeps 10 ms. The centre comes through a channel (the latest message wins).
pub struct LandscapeWorker {
    tx: Sender<Msg>,
    handle: Option<JoinHandle<()>>,
}

impl LandscapeWorker {
    /// Starts the worker.
    pub fn spawn(world: Arc<RwLock<World>>, gate: Arc<FrameGate>, tiles: Arc<Mutex<MapTiles>>, centre: [i32; 2]) -> LandscapeWorker {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("landscape".into())
            .spawn(move || run(world, gate, tiles, centre, rx))
            .expect("spawn landscape thread");
        LandscapeWorker { tx, handle: Some(handle) }
    }

    /// New centre (see [`landscape_centre`]).
    pub fn set_centre(&self, centre: [i32; 2]) {
        let _ = self.tx.send(Msg::Centre(centre));
    }

    /// Stops and joins the worker (`GameController+0x800584` cleared).
    pub fn stop(mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LandscapeWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(world: Arc<RwLock<World>>, gate: Arc<FrameGate>, tiles: Arc<Mutex<MapTiles>>, mut centre: [i32; 2], rx: Receiver<Msg>) {
    let mut builder = LandscapeBuilder::new();
    let mut last_evict = Instant::now();
    loop {
        loop {
            match rx.try_recv() {
                Ok(Msg::Centre(c)) => centre = c,
                Ok(Msg::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        // The world's read lock per unit of work (`LandscapeBuilder::iterate_with`).
        builder.iterate_with(
            &mut |f| {
                // After the frame thread when it waits for the lock (`FrameGate`).
                let w = gate.acquire(|| world.read().unwrap_or_else(|e| e.into_inner()));
                let t = Instant::now();
                f(&w);
                drop(w);
                crate::profile::worker_note("World read lock held", t.elapsed());
            },
            &tiles,
            centre,
        );
        if last_evict.elapsed() > Duration::from_millis(1000) {
            builder.evict(&tiles, centre);
            last_evict = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_render::map::SavedLand;

    fn flat_world() -> World {
        let mut w = World::new(1234);
        w.has_name = true;
        let mut z = Zone::new(10, 20);
        for c in z.columns.iter_mut() {
            c.height = 0;
            c.f14 = 10;
            c.blocks = vec![[100, 150, 50, 1]; 10];
        }
        w.insert_zone(Box::new(z));
        w
    }

    #[test]
    fn flat_zone_tile() {
        let w = flat_world();
        let t = build_zone_tile(&w, w.zone(10, 20).unwrap());
        // Blocks 0..=9: z range 9..=10 → one layer of 8-block cells, base cell 1; the surface
        // (z = 9) is cell 1 → layer 0.
        assert_eq!(t.size, [32, 32, 1]);
        assert_eq!(t.base_z, 1);
        assert!(t.voxels.iter().all(|v| *v == [100, 150, 50]));
        assert!(t.posts.is_empty());
    }

    #[test]
    fn builder_builds_loaded_zones_and_rebuilds_dirty_ones() {
        let w = flat_world();
        let tiles = Mutex::new(MapTiles::new());
        let mut b = LandscapeBuilder::new();
        b.iterate(&w, &tiles, [10, 20]);
        {
            let t = tiles.lock().unwrap();
            let e = t.entry(10, 20).unwrap();
            assert_eq!(e.tile.as_ref().unwrap().size, [32, 32, 1]);
            assert_eq!(e.fade, FADE_MS);
            assert_eq!(e.flags & 2, 2);
        }
        let first = tiles.lock().unwrap().entry(10, 20).unwrap().tile.as_ref().unwrap().model;
        // Not dirty: kept.
        b.iterate(&w, &tiles, [10, 20]);
        assert_eq!(tiles.lock().unwrap().entry(10, 20).unwrap().tile.as_ref().unwrap().model, first);
        // Dirty (the zone thread after generateZone): rebuilt, fade not restarted.
        tiles.lock().unwrap().entry_mut(10, 20).unwrap().fade = 0;
        tiles.lock().unwrap().mark_dirty(10, 20);
        b.iterate(&w, &tiles, [10, 20]);
        let t = tiles.lock().unwrap();
        let e = t.entry(10, 20).unwrap();
        assert_ne!(e.tile.as_ref().unwrap().model, first);
        assert_eq!(e.fade, 0);
        assert!(!e.dirty);
    }

    #[test]
    fn eviction_and_reload() {
        let w = flat_world();
        let tiles = Mutex::new(MapTiles::new());
        let mut b = LandscapeBuilder::new();
        b.iterate(&w, &tiles, [10, 20]);
        b.evict(&tiles, [30, 20]);
        assert!(tiles.lock().unwrap().entry(10, 20).unwrap().tile.is_none());
        tiles.lock().unwrap().take_mesh_events();
        b.zone_tile(&w, &tiles, 10, 20);
        let mut t = tiles.lock().unwrap();
        assert!(t.entry(10, 20).unwrap().tile.is_some());
        assert_eq!(t.take_mesh_events().len(), 1);
    }

    fn points_around(rx: i32, ry: i32) -> [[ClimatePoint; 3]; 3] {
        let p = |dx: i32, dy: i32| ClimatePoint {
            x: (rx + dx) * 0x4000 + 0x2000,
            y: (ry + dy) * 0x4000 + 0x2000,
            flag: 0,
            a: 0.5,
            b: 0.5,
            seed: 0,
            elevation: 7,
        };
        [-1, 0, 1].map(|dx| [-1, 0, 1].map(|dy| p(dx, dy)))
    }

    fn saved(visible: bool) -> SavedLand {
        SavedLand { visible, min_zone: [1, 2], size: [1, 1, 1], voxels: vec![[255; 3]], point: [10 * 0x4000 + 0x2000; 2], elevation: 7 }
    }

    #[test]
    fn saved_land_is_loaded_before_the_world_has_points() {
        // 0x006024d0: no tile object, the "land" blob exists: the tile comes from it.
        let tiles = Mutex::new(MapTiles::new());
        tiles.lock().unwrap().stash_saved_land(10, 10, saved(true));
        let mut b = LandscapeBuilder::new();
        b.region_tile_from(None, &tiles, 10, 10);
        let t = tiles.lock().unwrap();
        let r = t.region_tile(10, 10).unwrap();
        assert!(r.visible);
        assert_eq!(r.min_zone, [1, 2]);
        assert_eq!(r.tile.as_ref().unwrap().voxels, vec![[255; 3]]);
        assert!(!r.dirty);
    }

    #[test]
    fn saved_land_is_rebuilt_keeping_its_visible_byte() {
        // The load branch falls through to the rebuild, which never writes +0x28.
        let tiles = Mutex::new(MapTiles::new());
        tiles.lock().unwrap().stash_saved_land(10, 10, saved(true));
        let mut b = LandscapeBuilder::new();
        b.region_tile_from(Some(points_around(10, 10)), &tiles, 10, 10);
        let t = tiles.lock().unwrap();
        let r = t.region_tile(10, 10).unwrap();
        assert!(r.visible);
        assert_ne!(r.min_zone, [1, 2]);
        assert!(r.tile.as_ref().unwrap().voxels.len() > 1);
    }

    #[test]
    fn evicted_landscape_tiles_come_back_visible() {
        let tiles = Mutex::new(MapTiles::new());
        let mut b = LandscapeBuilder::new();
        b.region_tile_from(Some(points_around(10, 10)), &tiles, 10, 10);
        {
            let mut t = tiles.lock().unwrap();
            let r = t.ensure_region_tile(10, 10, [10 * 0x4000 + 0x2000; 2], 7);
            r.visible = true;
        }
        b.evict(&tiles, [30 * 64, 10 * 64]);
        assert!(tiles.lock().unwrap().region_tile(10, 10).is_none());
        b.region_tile_from(Some(points_around(10, 10)), &tiles, 10, 10);
        assert!(tiles.lock().unwrap().region_tile(10, 10).unwrap().visible);
    }

    #[test]
    fn centre() {
        assert_eq!(landscape_centre([100, 200], [512.0, -300.0]), [102, 198]);
    }
}
