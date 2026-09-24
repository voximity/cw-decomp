//! Singleplayer, and the map cache `Save/map_<world>.db`.
//!
//! # What the original does (the decision this module records)
//!
//! The shipped Cube.exe has no loopback server. Its singleplayer is the client's own world
//! running the whole `World::tick` as a server does:
//!
//! - `World::World` 0x0058eb00 copies its second argument into `world+0xb4` (0x0058ec09:
//!   `mov al, [ebp+0xc]; mov [ebx+0xb4], al`) and clears `world+0xb8`; the `GameController`
//!   constructor passes 0 (0x00459f31 `push 0`), so the client's world starts as a *server*
//!   world.
//! - The constructor then inserts the local creature into the creature map under id 1
//!   (0x004621f8..0x0046221e, `map[(int64)1] = player`) and calls `World::setLocalPlayer`
//!   0x00487e90 (`[world+0xb8] = player`, 0x0046222c; its only caller).
//! - The only writers of `world+0xb4` in the client are `connect` 0x0046fc50 (`GC+0x398 = 1`
//!   at 0x0046fca7 when it starts connecting, 0 on every failure path), `disconnect`
//!   0x004719f0 (0x00471a5b, 0x00471aad: 0) and the send thread 0x00469c10
//!   (0x0046a869: 0 on a send error); `GC+0x398` is `world+0xb4` (the world sits at `GC+0x2e4`).
//!   The only other writer of `world+0xb8` is the join (0x00470408: the recreated player).
//!   `startWorld` 0x0046f620, the zone thread 0x0046a8a0 and `World::load` 0x005a52e0 write
//!   neither.
//! - `update` 0x00488ee0 calls `World::tick` with a NULL received list (0x0048cf20 `push 0`);
//!   the received records are applied by `update` itself after the tick (`received.rs`).
//! - `GC+0x8006c8` (an in-process `cube::Server`) is only ever zeroed (0x00459fc5), so the
//!   `takeHits`/`takePassives`/`takeShoots` calls at 0x0048ce48 never run.
//!
//! So in singleplayer `world+0xb4 == 0` and `world+0xb8` is the local player: the tick's
//! server passes all run on the client's world — the region activation and mission discovery
//! of the player loop (guarded at `Server.exe 0x005328db`), the statics and creature passes
//! (0x005335e8.., 0x00533bab), the zone spawns (0x00535708), the interactions of the creature
//! loop, the clock with the `time` blob; `World::load` opens `Save/world_<name>.db` for the
//! zone, region and time blobs (0x005a56e4). The local-player blocks of the tick
//! (`world+0xb8`: mount speed, gliding, fall damage, footsteps, the item pickup into the
//! inventory, the hit on the player in `applyHit` 0x004ceb1d) run as well.
//!
//! # What the port does
//!
//! The same: [`crate::controller::Controller`] runs `cw_server::server::world_tick_with` (the
//! server port's tick, whose server passes are gated on [`cw_world::World::is_client`]) on the
//! client's world in 20 ms slices, with `is_client = false` and `local_player = Some(1)` until
//! a connection is made. `connect` sets `is_client`, the join sets `local_player` to the id the
//! server gave, `disconnect` clears `is_client` (the original leaves `world+0xb8` pointing at
//! the player it recreated). Multiplayer runs the same tick with `is_client = true`, which
//! skips the server passes. There is no loopback socket and no second world.
//!
//! # Map persistence (Tier A layouts)
//!
//! `WorldMap::load 0x005fbc90` opens `"Save/map_" + name + ".db"`. Its blobs:
//!
//! - `"discovered"`: the visited-zone counter `WorldMap+0x8000bc` (i32), written by the unload
//!   of a region record 0x00602160 and by `WorldMap::clear` 0x00601f80.
//! - `"reg" rx "_" ry`: a region record, written by 0x00605420 and read by `loadRegion`
//!   0x00603230 ([`RegBlob`]): `WorldMap+0x8000f8` (i32, skipped by the reader), then per
//!   zone (64 × 64, x outer) `ZoneTile+0x30` (flags), `+0x10`, `+0x11`, `+0x12`, `+0x13`
//!   (bytes), `+0x14`, `+0x18` (i32), `+0x1c` (byte), then per cell (8 × 8, x outer) of the
//!   record's copy of the region's cells: `+0x18`, `+0x1c`, `+0x20`, `+0x24`, `+0x28`,
//!   `+0x14` (i32), `+0x00..+0x10`, `+0x10`. The reader overwrites `+0x10..+0x20` of every zone
//!   and the cells from the world's region when that is loaded.
//! - `"tile" zx "_" zy`: a zone tile, written by `buildZoneTile` 0x00603a00 (0x00604bd6..
//!   0x00604e3c) and read by it when the zone has no model (0x00603c94..0x00603f2c)
//!   ([`TileBlob`]): `WorldMap+0x8000f8` (skipped), `ZoneTile+4` (base z), the model size
//!   (3 × i32), with a positive size a length-prefixed zlib stream of the RGB voxels, the
//!   post count and the posts (3 × i32 each).
//! - `"land" rx "_" ry`: a landscape tile, written by 0x006050b0 ([`LandBlob`]): the header,
//!   `LandscapeTile+0x20` (8 bytes, the zone of voxel 0), `+0x28` (byte), the model size and
//!   voxels as above, then `+4..+0x20` (0x1c bytes, the region's climate point).
//!
//! `WorldMap+0x8000f8` has no writer found; the port writes 0 (see the report).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use cw_formats::SaveDb;
use cw_render::map::{MapTiles, SavedLand, mesh_tile_voxels};
use cw_world::World;
use cw_world::region::ZoneRecord;

pub use crate::persist::map_db_path;

/// `WorldMap+0x8000f8`, the first word of every map blob (no writer found; 0).
pub const MAP_HEADER: i32 = 0;

// ---------------------------------------------------------------------------------------------
// The blobs.

fn take<'a>(b: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
    if pos.checked_add(n).is_some_and(|e| e <= b.len()) {
        let s = &b[*pos..*pos + n];
        *pos += n;
        Some(s)
    } else {
        *pos = b.len();
        None
    }
}

fn read_i32(b: &[u8], pos: &mut usize) -> Option<i32> {
    take(b, pos, 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

/// One zone of a `"reg"` blob (14 bytes): `ZoneTile+0x30`, `+0x10..+0x13`, `+0x14`, `+0x18`,
/// `+0x1c`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegZone {
    /// `+0x30`: bit 0 visited, bit 1 has had a tile.
    pub flags: u8,
    /// `+0x10..+0x13`: the site record's kind, sub type and its two padding bytes.
    pub b10: [u8; 4],
    /// `+0x14`: seed.
    pub seed: i32,
    /// `+0x18`: level.
    pub level: i32,
    /// `+0x1c`: the town cell's level roll.
    pub b1c: u8,
}

impl RegZone {
    /// The site part as the world's zone record (`+0x10..+0x20`).
    pub fn site(&self) -> ZoneRecord {
        ZoneRecord { kind: self.b10[0], sub: self.b10[1], seed: self.seed, level: self.level, byte0c: self.b1c }
    }
}

/// One cell of a `"reg"` blob: the first 0x2c bytes of a 0x68-byte cell, in blob order.
pub type RegCell = [u8; 0x2c];

/// The cell bytes of a world cell (`cw_world::region::Cell`, `+0..+0x2c`).
pub fn cell_bytes(c: &cw_world::region::Cell) -> RegCell {
    let mut b = [0u8; 0x2c];
    b[0..8].copy_from_slice(&c.x.to_le_bytes());
    b[8..0x10].copy_from_slice(&c.y.to_le_bytes());
    b[0x10..0x14].copy_from_slice(&c.radius.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&c.height.to_le_bytes());
    b[0x18..0x1c].copy_from_slice(&c.kind.to_le_bytes());
    b[0x1c..0x20].copy_from_slice(&c.variant.to_le_bytes());
    b[0x20..0x24].copy_from_slice(&c.id.to_le_bytes());
    b[0x24..0x28].copy_from_slice(&c.level.to_le_bytes());
    b[0x28..0x2c].copy_from_slice(&c.level_extra.to_le_bytes());
    b
}

/// A region record's blob (`"reg" rx "_" ry`, 0x00605420 / 0x00603230).
#[derive(Clone, Debug, PartialEq)]
pub struct RegBlob {
    pub header: i32,
    /// 4096 zones, index `i * 64 + j` (zone `rx * 64 + i`, `ry * 64 + j`).
    pub zones: Vec<RegZone>,
    /// 64 cells, index `i * 8 + j`.
    pub cells: Vec<RegCell>,
}

impl Default for RegBlob {
    /// A fresh record (ctor 0x005fae00: zones with level 1, zero cells).
    fn default() -> Self {
        RegBlob { header: MAP_HEADER, zones: vec![RegZone { level: 1, ..RegZone::default() }; 4096], cells: vec![[0; 0x2c]; 64] }
    }
}

/// Bytes of a `"reg"` blob.
pub const REG_BLOB_BYTES: usize = 4 + 4096 * 14 + 64 * 0x2c;

impl RegBlob {
    /// 0x00605420.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Vec::with_capacity(REG_BLOB_BYTES);
        w.extend_from_slice(&self.header.to_le_bytes());
        for z in &self.zones {
            w.push(z.flags);
            w.extend_from_slice(&z.b10);
            w.extend_from_slice(&z.seed.to_le_bytes());
            w.extend_from_slice(&z.level.to_le_bytes());
            w.push(z.b1c);
        }
        for c in &self.cells {
            // +0x18, +0x1c, +0x20, +0x24, +0x28, +0x14, +0x00..+0x10, +0x10.
            for o in [0x18, 0x1c, 0x20, 0x24, 0x28, 0x14] {
                w.extend_from_slice(&c[o..o + 4]);
            }
            w.extend_from_slice(&c[0..0x10]);
            w.extend_from_slice(&c[0x10..0x14]);
        }
        w
    }

    /// 0x00603230's reads: every field only when it fits (a short blob leaves the rest as the
    /// fresh record has it).
    pub fn from_bytes(b: &[u8]) -> RegBlob {
        let mut r = RegBlob::default();
        let mut pos = 0;
        if let Some(h) = read_i32(b, &mut pos) {
            r.header = h;
        }
        for z in &mut r.zones {
            if let Some(s) = take(b, &mut pos, 1) {
                z.flags = s[0];
            }
            for k in 0..4 {
                if let Some(s) = take(b, &mut pos, 1) {
                    z.b10[k] = s[0];
                }
            }
            if let Some(v) = read_i32(b, &mut pos) {
                z.seed = v;
            }
            if let Some(v) = read_i32(b, &mut pos) {
                z.level = v;
            }
            if let Some(s) = take(b, &mut pos, 1) {
                z.b1c = s[0];
            }
        }
        for c in &mut r.cells {
            for o in [0x18, 0x1c, 0x20, 0x24, 0x28, 0x14] {
                if let Some(s) = take(b, &mut pos, 4) {
                    c[o..o + 4].copy_from_slice(s);
                }
            }
            if let Some(s) = take(b, &mut pos, 0x10) {
                c[0..0x10].copy_from_slice(s);
            }
            if let Some(s) = take(b, &mut pos, 4) {
                c[0x10..0x14].copy_from_slice(s);
            }
        }
        r
    }
}

/// A model's size and RGB voxels as the map blobs store them.
fn write_model(w: &mut Vec<u8>, size: [i32; 3], voxels: &[[u8; 3]]) {
    for s in size {
        w.extend_from_slice(&s.to_le_bytes());
    }
    if size.iter().all(|&s| s > 0) {
        let raw: Vec<u8> = voxels.iter().flatten().copied().collect();
        let z = cw_net::compress::compress(&raw);
        w.extend_from_slice(&(z.len() as u32).to_le_bytes());
        w.extend_from_slice(&z);
    }
}

fn read_model(b: &[u8], pos: &mut usize) -> ([i32; 3], Option<Vec<[u8; 3]>>) {
    let mut size = [0i32; 3];
    if let Some(s) = take(b, pos, 0xc) {
        for i in 0..3 {
            size[i] = i32::from_le_bytes(s[i * 4..i * 4 + 4].try_into().unwrap());
        }
    }
    if !size.iter().all(|&s| s > 0) {
        return (size, None);
    }
    let n = read_i32(b, pos).unwrap_or(0);
    if n == 0 {
        return (size, None);
    }
    let z = take(b, pos, n as u32 as usize).map(<[u8]>::to_vec).unwrap_or_default();
    let raw = cw_net::compress::decompress(&z).unwrap_or_else(|(p, _)| p);
    (size, Some(raw.as_chunks::<3>().0.to_vec()))
}

/// A zone tile's blob (`"tile" zx "_" zy`, 0x00603a00).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TileBlob {
    pub header: i32,
    /// `ZoneTile+4`.
    pub base_z: i32,
    pub size: [i32; 3],
    pub voxels: Vec<[u8; 3]>,
    /// `ZoneTile+0x20` (count `+0x24`).
    pub posts: Vec<[i32; 3]>,
}

impl TileBlob {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&self.header.to_le_bytes());
        w.extend_from_slice(&self.base_z.to_le_bytes());
        write_model(&mut w, self.size, &self.voxels);
        w.extend_from_slice(&(self.posts.len() as u32).to_le_bytes());
        for p in &self.posts {
            for v in p {
                w.extend_from_slice(&v.to_le_bytes());
            }
        }
        w
    }

    pub fn from_bytes(b: &[u8]) -> TileBlob {
        let mut t = TileBlob::default();
        let mut pos = 0;
        t.header = read_i32(b, &mut pos).unwrap_or(0);
        t.base_z = read_i32(b, &mut pos).unwrap_or(0);
        let (size, voxels) = read_model(b, &mut pos);
        t.size = size;
        t.voxels = voxels.unwrap_or_default();
        let n = read_i32(b, &mut pos).unwrap_or(0);
        for _ in 0..n.max(0) {
            let Some(s) = take(b, &mut pos, 0xc) else { break };
            t.posts.push([0, 1, 2].map(|i| i32::from_le_bytes(s[i * 4..i * 4 + 4].try_into().unwrap())));
        }
        t
    }
}

/// A landscape tile's blob (`"land" rx "_" ry`, 0x006050b0).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LandBlob {
    pub header: i32,
    /// `+0x20`: the zone of voxel 0.
    pub min_zone: [i32; 2],
    /// `+0x28`.
    pub visible: u8,
    pub size: [i32; 3],
    pub voxels: Vec<[u8; 3]>,
    /// `+4..+0x20`: the region's climate point (the port keeps x at `+4`, y at `+8` and the
    /// elevation at `+0x1c`; the other bytes are 0).
    pub point: [u8; 0x1c],
}

impl LandBlob {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&self.header.to_le_bytes());
        w.extend_from_slice(&self.min_zone[0].to_le_bytes());
        w.extend_from_slice(&self.min_zone[1].to_le_bytes());
        w.push(self.visible);
        write_model(&mut w, self.size, &self.voxels);
        w.extend_from_slice(&self.point);
        w
    }

    pub fn from_bytes(b: &[u8]) -> LandBlob {
        let mut t = LandBlob::default();
        let mut pos = 0;
        t.header = read_i32(b, &mut pos).unwrap_or(0);
        t.min_zone[0] = read_i32(b, &mut pos).unwrap_or(0);
        t.min_zone[1] = read_i32(b, &mut pos).unwrap_or(0);
        t.visible = take(b, &mut pos, 1).map_or(0, |s| s[0]);
        let (size, voxels) = read_model(b, &mut pos);
        t.size = size;
        t.voxels = voxels.unwrap_or_default();
        if let Some(s) = take(b, &mut pos, 0x1c) {
            t.point.copy_from_slice(s);
        }
        t
    }
}

/// The key of a region record (`"reg" << rx << "_" << ry`).
pub fn region_key(rx: i32, ry: i32) -> String {
    format!("reg{rx}_{ry}")
}

/// The key of a zone tile (`"tile" << zx << "_" << zy`).
pub fn tile_key(zx: i32, zy: i32) -> String {
    format!("tile{zx}_{zy}")
}

/// The key of a landscape tile (`"land" << rx << "_" << ry`).
pub fn land_key(rx: i32, ry: i32) -> String {
    format!("land{rx}_{ry}")
}

// ---------------------------------------------------------------------------------------------
// The map cache.

/// The parts of the map records the port's [`MapTiles`] does not model: the records' copies
/// of the world's cells and the header, per region, as last loaded or saved.
#[derive(Clone, Debug, Default)]
pub struct MapBlobs {
    pub regions: BTreeMap<(i32, i32), RegBlob>,
}

/// Writes the map cache of the loaded world to `Save/map_<name>.db` (`WorldMap::clear`
/// 0x00601f80: `"discovered"`, then every region record 0x00605420 and landscape tile
/// 0x006050b0; the zone tiles, which the original writes as it builds them, are written here
/// for every zone that has one). A region the world has loaded supplies its site records and
/// cells (as `loadRegion` and `buildZoneTile` copy them). Nothing is written for an unnamed
/// world (`0x005fbc90` returns early when `World+0xa4` is 0).
pub fn save_map(game_dir: &Path, name: &str, tiles: &Mutex<MapTiles>, world: Option<&World>, blobs: &mut MapBlobs) -> Result<(), String> {
    if name.is_empty() {
        return Ok(());
    }
    let path = map_db_path(game_dir, name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let db = SaveDb::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t = tiles.lock().unwrap_or_else(|e| e.into_inner());
    // One transaction for the whole map (Tier C: the original commits each `putBlob`, one
    // journal sync per region and tile, many seconds on a hard disk at shutdown). It is
    // committed whatever the writes return, so a failure keeps the rows before it.
    let began = db.begin().map_err(|e| e.to_string())?;
    let written = (|| -> Result<(), String> {
        db.put("discovered", &t.discovered.to_le_bytes()).map_err(|e| e.to_string())?;
        for rx in 0..0x400 {
            for ry in 0..0x400 {
                if !t.has_record(rx, ry) {
                    continue;
                }
                let mut blob = blobs.regions.get(&(rx, ry)).cloned().unwrap_or_default();
                let region = world.and_then(|w| w.region(rx, ry));
                for i in 0..64 {
                    for j in 0..64 {
                        let Some(e) = t.entry(rx * 64 + i, ry * 64 + j) else { continue };
                        let z = &mut blob.zones[(i * 64 + j) as usize];
                        let site = region.map_or(e.site, |r| r.zones[(i * 64 + j) as usize]);
                        z.flags = e.flags;
                        z.b10[0] = site.kind;
                        z.b10[1] = site.sub;
                        z.seed = site.seed;
                        z.level = site.level;
                        z.b1c = site.byte0c;
                    }
                }
                if let Some(r) = region {
                    for (k, c) in r.cells.iter().enumerate() {
                        blob.cells[k] = cell_bytes(c);
                    }
                }
                db.put(&region_key(rx, ry), &blob.to_bytes()).map_err(|e| e.to_string())?;
                blobs.regions.insert((rx, ry), blob);
            }
        }
        for (zx, zy) in t.tiled_zones() {
            let Some(e) = t.entry(zx, zy) else { continue };
            let Some(tile) = &e.tile else { continue };
            let b = TileBlob { header: MAP_HEADER, base_z: e.base_z, size: tile.size, voxels: tile.voxels.clone(), posts: e.posts.clone() };
            db.put(&tile_key(zx, zy), &b.to_bytes()).map_err(|e| e.to_string())?;
        }
        let land_point = |p: [i32; 2], elevation: i32| {
            let mut point = [0u8; 0x1c];
            point[0..4].copy_from_slice(&p[0].to_le_bytes());
            point[4..8].copy_from_slice(&p[1].to_le_bytes());
            point[0x18..0x1c].copy_from_slice(&elevation.to_le_bytes());
            point
        };
        for (rx, ry) in t.region_tile_keys() {
            let Some(l) = t.region_tile(rx, ry) else { continue };
            let Some(tile) = &l.tile else { continue };
            let b = LandBlob { header: MAP_HEADER, min_zone: l.min_zone, visible: u8::from(l.visible), size: tile.size, voxels: tile.voxels.clone(), point: land_point(l.point, l.elevation) };
            db.put(&land_key(rx, ry), &b.to_bytes()).map_err(|e| e.to_string())?;
        }
        // Landscape tiles evicted this session (0x005fbed0 wrote them through 0x006050b0 before
        // freeing) and blobs loaded but not yet taken.
        for (&(rx, ry), l) in t.saved_land() {
            if t.region_tile(rx, ry).is_some() || !l.size.iter().all(|&n| n > 0) {
                continue;
            }
            let b = LandBlob { header: MAP_HEADER, min_zone: l.min_zone, visible: u8::from(l.visible), size: l.size, voxels: l.voxels.clone(), point: land_point(l.point, l.elevation) };
            db.put(&land_key(rx, ry), &b.to_bytes()).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if began {
        db.commit().map_err(|e| e.to_string())?;
    }
    written
}

/// Reads `Save/map_<name>.db` into a fresh cache: `"discovered"` (`WorldMap::load 0x005fbc90`),
/// every region record (`loadRegion 0x00603230`, done eagerly here: flags and site records
/// into [`MapTiles`], the rest kept in `blobs`), then every zone tile (0x00603c94..0x00603f2c:
/// model, base z, flag 2, the 250 ms fade, the posts), and every landscape tile blob into the
/// saved-land store for `0x006024d0` to take. A missing file leaves the cache empty.
pub fn load_map(game_dir: &Path, name: &str, tiles: &Mutex<MapTiles>, blobs: &mut MapBlobs) {
    let mut t = tiles.lock().unwrap_or_else(|e| e.into_inner());
    *t = MapTiles::new();
    blobs.regions.clear();
    if name.is_empty() {
        return;
    }
    let path = map_db_path(game_dir, name);
    if !path.exists() {
        return;
    }
    let Ok(db) = SaveDb::open(&path) else { return };
    if let Ok(Some(b)) = db.get("discovered")
        && b.len() >= 4
    {
        t.discovered = i32::from_le_bytes(b[0..4].try_into().unwrap());
    }
    let Ok(keys) = db.keys() else { return };
    let parse = |rest: &str| -> Option<(i32, i32)> {
        let (a, b) = rest.split_once('_')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    for k in &keys {
        let Some((rx, ry)) = k.strip_prefix("reg").and_then(parse) else { continue };
        let Ok(Some(blob)) = db.get(k) else { continue };
        let rec = RegBlob::from_bytes(&blob);
        t.ensure_record(rx, ry);
        for i in 0..64 {
            for j in 0..64 {
                let z = rec.zones[(i * 64 + j) as usize];
                if let Some(e) = t.entry_mut(rx * 64 + i, ry * 64 + j) {
                    e.flags = z.flags;
                    e.site = z.site();
                }
            }
        }
        blobs.regions.insert((rx, ry), rec);
    }
    for k in &keys {
        let Some((zx, zy)) = k.strip_prefix("tile").and_then(parse) else { continue };
        let Ok(Some(blob)) = db.get(k) else { continue };
        let b = TileBlob::from_bytes(&blob);
        if b.voxels.is_empty() || t.entry(zx, zy).is_none_or(|e| e.tile.is_some()) {
            continue;
        }
        let mesh = mesh_tile_voxels(b.size, &b.voxels);
        t.set_zone_tile(zx, zy, b.size, b.voxels, mesh);
        if let Some(e) = t.entry_mut(zx, zy) {
            e.base_z = b.base_z;
            e.flags |= 2;
            e.fade = 250;
            e.posts = b.posts;
        }
    }
    // The `"land"` blobs, which `buildLandscapeTile 0x006024d0` reads when it finds a region
    // without a tile object (the visible byte `+0x28` included).
    for k in &keys {
        let Some((rx, ry)) = k.strip_prefix("land").and_then(parse) else { continue };
        let Ok(Some(blob)) = db.get(k) else { continue };
        let b = LandBlob::from_bytes(&blob);
        let int = |o: usize| i32::from_le_bytes(b.point[o..o + 4].try_into().unwrap());
        t.stash_saved_land(rx, ry, SavedLand { visible: b.visible != 0, min_zone: b.min_zone, size: b.size, voxels: b.voxels, point: [int(0), int(4)], elevation: int(0x18) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_blob_layout_and_round_trip() {
        let mut r = RegBlob::default();
        r.header = 7;
        r.zones[1] = RegZone { flags: 3, b10: [4, 5, 0, 0], seed: 0x11223344, level: 9, b1c: 2 };
        let mut c = [0u8; 0x2c];
        for (i, v) in c.iter_mut().enumerate() {
            *v = i as u8;
        }
        r.cells[0] = c;
        let b = r.to_bytes();
        assert_eq!(b.len(), REG_BLOB_BYTES);
        assert_eq!(&b[0..4], &7i32.to_le_bytes());
        // Zone 1 at 4 + 14: flags, +0x10..+0x13, seed, level, +0x1c.
        assert_eq!(&b[18..32], &[3, 4, 5, 0, 0, 0x44, 0x33, 0x22, 0x11, 9, 0, 0, 0, 2]);
        // Cell 0: +0x18 first, then +0x1c .. +0x28, +0x14, +0..+0x10, +0x10.
        let c0 = 4 + 4096 * 14;
        assert_eq!(b[c0], 0x18);
        assert_eq!(b[c0 + 20], 0x14);
        assert_eq!(b[c0 + 24], 0);
        assert_eq!(b[c0 + 40], 0x10);
        assert_eq!(RegBlob::from_bytes(&b), r);
        // A short blob keeps the fresh record's values past its end.
        let short = RegBlob::from_bytes(&b[..20]);
        assert_eq!(short.zones[1].flags, 3);
        assert_eq!(short.zones[1].level, 1);
    }

    #[test]
    fn tile_and_land_round_trip() {
        let t = TileBlob { header: 0, base_z: -2, size: [2, 1, 1], voxels: vec![[1, 2, 3], [4, 5, 6]], posts: vec![[1, 2, 3]] };
        let b = t.to_bytes();
        assert_eq!(&b[4..8], &(-2i32).to_le_bytes());
        assert_eq!(TileBlob::from_bytes(&b), t);
        let empty = TileBlob { size: [0, 0, 0], ..TileBlob::default() };
        assert_eq!(empty.to_bytes().len(), 4 + 4 + 12 + 4);
        let l = LandBlob { header: 0, min_zone: [64, 128], visible: 0, size: [1, 1, 1], voxels: vec![[9, 9, 9]], point: [1; 0x1c] };
        assert_eq!(LandBlob::from_bytes(&l.to_bytes()), l);
    }

    #[test]
    fn map_round_trip() {
        let dir = std::env::temp_dir().join(format!("cw-client-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tiles = Mutex::new(MapTiles::new());
        {
            let mut t = tiles.lock().unwrap();
            t.ensure_record(3, 4);
            assert!(t.mark_visited(3 * 64 + 5, 4 * 64 + 6));
        }
        let mut blobs = MapBlobs::default();
        save_map(&dir, "Home", &tiles, None, &mut blobs).unwrap();
        let back = Mutex::new(MapTiles::new());
        let mut blobs2 = MapBlobs::default();
        load_map(&dir, "Home", &back, &mut blobs2);
        let t = back.lock().unwrap();
        assert_eq!(t.discovered, 1);
        assert_eq!(t.entry(3 * 64 + 5, 4 * 64 + 6).unwrap().flags & 1, 1);
        assert_eq!(t.entry(3 * 64 + 5, 4 * 64 + 7).unwrap().flags, 0);
        assert_eq!(blobs2.regions[&(3, 4)], blobs.regions[&(3, 4)]);
        drop(t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn landscape_tiles_keep_their_visible_byte_across_sessions() {
        let dir = std::env::temp_dir().join(format!("cw-client-land-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let tiles = Mutex::new(MapTiles::new());
        {
            let mut t = tiles.lock().unwrap();
            t.ensure_region_tile(10, 11, [100, 200], 7).visible = true;
            t.set_region_model(10, 11, [640, 704], [1, 1, 1], vec![[255; 3]], mesh_tile_voxels([1, 1, 1], &[[255; 3]]));
            t.ensure_region_tile(20, 21, [300, 400], 8).visible = true;
            t.set_region_model(20, 21, [1280, 1344], [1, 1, 1], vec![[220; 3]], mesh_tile_voxels([1, 1, 1], &[[220; 3]]));
            // Evicted this session (0x005fbed0 saves it before freeing).
            t.drop_region_tile(20, 21);
        }
        save_map(&dir, "Home", &tiles, None, &mut MapBlobs::default()).unwrap();
        let back = Mutex::new(MapTiles::new());
        load_map(&dir, "Home", &back, &mut MapBlobs::default());
        let mut t = back.lock().unwrap();
        // Loaded on demand by 0x006024d0, not eagerly.
        assert!(t.region_tile(10, 11).is_none());
        let l = t.take_saved_land(10, 11).unwrap();
        assert!(l.visible);
        assert_eq!((l.min_zone, l.point, l.elevation, l.size), ([640, 704], [100, 200], 7, [1, 1, 1]));
        assert_eq!(l.voxels, vec![[255; 3]]);
        let l = t.take_saved_land(20, 21).unwrap();
        assert!(l.visible);
        assert_eq!(l.voxels, vec![[220; 3]]);
        drop(t);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_and_paths() {
        assert_eq!(region_key(12, 7), "reg12_7");
        assert_eq!(tile_key(1, 2), "tile1_2");
        assert_eq!(land_key(3, 4), "land3_4");
        assert!(map_db_path(Path::new("g"), "Home").ends_with("map_Home.db"));
    }
}
