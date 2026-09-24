//! The world save database (`Save/world_<name>.db`, `cube::Database` at `world+0xac`) as the
//! generator reads and writes it.
//!
//! Blobs are plain little-endian records written through the writer object of
//! `FUN_0041d800` (`[vector*, version = 1, pos]`) and read back through the same object over a
//! loaded vector: every read is bounds-checked, and one that does not fit leaves its target
//! untouched and moves the position to the end ([`BlobReader`]).
//!
//! | key | writer | reader | contents |
//! |---|---|---|---|
//! | `time` | the world tick (`0x005322d0`, every ten seconds of play) | `World::load` `0x004d83a0` | day `+0x800160`, time of day `+0x80015c` |
//! | `zone<zx>_<zy>` | `World::saveZone` `0x004d81b0` | the end of `generateZone` (`0x00521bed`) | [`Zone::save_blob`] / [`World::apply_zone_blob`] |
//! | `mission<rx*8+cx>_<ry*8+cy>` | `World::saveEntities` `0x004d7c50` | `createRegion` phase J | [`MissionState`] |
//! | `monster<rx*8+cx>_<ry*8+cy>` | `World::saveEntities` | `createRegion` phase J | [`MonsterState`] |
//!
//! The database is only consulted for a named world (`world+0xa4 != 0`) with `world+0xb4`
//! clear, which is every server world.

use std::sync::{Arc, Mutex};

use cw_formats::SaveDb;

use crate::fixed::to_block;
use crate::inventory::{Inventory, Item};
use crate::surface::Block;
use crate::world::World;
use crate::zone::{GroundItem, Zone, set_block};

/// Key of the world clock blob.
pub const TIME_KEY: &str = "time";

/// `"zone" << zx << "_" << zy` (`0x00521b78`).
pub fn zone_key(zx: i32, zy: i32) -> String {
    format!("zone{zx}_{zy}")
}

/// `"mission" << a << "_" << b` with `a = rx * 8 + cx`, `b = ry * 8 + cy`.
pub fn mission_key(a: i32, b: i32) -> String {
    format!("mission{a}_{b}")
}

/// `"monster" << a << "_" << b`, same indices as [`mission_key`].
pub fn monster_key(a: i32, b: i32) -> String {
    format!("monster{a}_{b}")
}

/// The blob writer of `FUN_0041d800`: appends raw little-endian fields; `version` (1) is the
/// first field every serializer writes.
#[derive(Debug, Default)]
pub struct BlobWriter {
    pub buf: Vec<u8>,
}

impl BlobWriter {
    /// The version the writer constructor stores (`arg1[1] = 1`).
    pub const VERSION: u32 = 1;

    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// The blob reader of `FUN_0041d800` over a loaded vector. Each accessor returns `None` when
/// the field does not fit, after moving the position to the end of the data, so every later
/// read fails too; the callers leave the destination field as it was.
#[derive(Debug)]
pub struct BlobReader<'a> {
    data: &'a [u8],
    pos: usize,
    /// The version field the deserializers read first (`arg1[2]`).
    pub version: u32,
}

impl<'a> BlobReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, version: 0 }
    }

    /// The next `n` bytes, or `None` (and the position at the end) when they do not fit.
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n <= self.data.len() {
            let s = &self.data[self.pos..self.pos + n];
            self.pos += n;
            Some(s)
        } else {
            self.pos = self.data.len();
            None
        }
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.take(4).map(|s| f32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Option<i64> {
        self.take(8).map(|s| i64::from_le_bytes(s.try_into().unwrap()))
    }

    /// Reads the version field into `self.version` (the first read of every deserializer).
    pub fn read_version(&mut self) {
        if let Some(v) = self.u32() {
            self.version = v;
        }
    }
}

/// One player-modified block, the 20-byte element of the vector at `zone+0x68`: the block
/// position, the block, and the day it was placed (`-1` for permanent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifiedBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: Block,
    pub time: i32,
}

impl ModifiedBlock {
    pub const SIZE: usize = 20;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..4].copy_from_slice(&self.x.to_le_bytes());
        b[4..8].copy_from_slice(&self.y.to_le_bytes());
        b[8..12].copy_from_slice(&self.z.to_le_bytes());
        b[12..16].copy_from_slice(&self.block);
        b[16..20].copy_from_slice(&self.time.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let i = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Self { x: i(0), y: i(4), z: i(8), block: b[12..16].try_into().unwrap(), time: i(16) }
    }
}

/// `FUN_0041d270`: the ground-item vector comparison of the zone deserializer. Two items are
/// equal when `Item::operator==` holds and the position, rotation, `+0x134`, `+0x138`,
/// `+0x13c` and `+0x140` agree (`+0x144` is not compared; the float compares are IEEE, so a
/// NaN never matches).
pub fn ground_items_equal(a: &[GroundItem], b: &[GroundItem]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            Inventory::same_item(&p.item, &q.item)
                && p.x == q.x
                && p.y == q.y
                && p.z == q.z
                && p.rotation == q.rotation
                && p.f134 == q.f134
                && p.b138 == q.b138
                && p.f13c == q.f13c
                && p.f140 == q.f140
        })
}

impl Zone {
    /// `Zone::serialize(writer)`, `Server.exe 0x0041faa0`: the `zone<zx>_<zy>` blob. The ground
    /// items are only written for a dirty zone (`+0x75`); an untouched zone writes a zero
    /// count. Then the modified blocks, the `+0x38`/`+0x3c` words of every spawn and the
    /// `+0x30` byte of every static.
    pub fn save_blob(&self) -> Vec<u8> {
        let mut w = BlobWriter::new();
        w.u32(BlobWriter::VERSION);
        if self.dirty {
            w.u32(self.items.len() as u32);
            for it in &self.items {
                w.bytes(&it.item.to_bytes());
                w.i64(it.x);
                w.i64(it.y);
                w.i64(it.z);
                w.f32(it.rotation);
                w.f32(it.f134);
                w.u8(it.b138);
                w.i32(it.f13c);
                w.i32(it.f140);
                w.i32(it.f144);
            }
        } else {
            w.u32(0);
        }
        w.u32(self.modified.len() as u32);
        for m in &self.modified {
            w.bytes(&m.to_bytes());
        }
        w.u32(self.spawns.len() as u32);
        for s in &self.spawns {
            w.u32(s.f38[0]);
            w.u32(s.f38[1]);
        }
        w.u32(self.statics.len() as u32);
        for s in &self.statics {
            w.u8(s.b30);
        }
        w.into_bytes()
    }
}

impl World {
    /// The blob stored under `key`, when a save database is attached and holds it.
    pub fn saved_blob(&self, key: &str) -> Option<Vec<u8>> {
        self.save.as_ref().and_then(|db| db.lock().ok()?.get(key).ok().flatten())
    }

    /// Attach a save database (`World::load` opens `Save/world_<name>.db`).
    pub fn attach_save(&mut self, db: SaveDb) {
        self.save = Some(Arc::new(Mutex::new(db)));
    }

    /// `World::load`'s read of the `time` blob: the day into `+0x800160`, the time of day into
    /// `+0x80015c`; a short blob leaves the later field alone.
    pub fn load_time_blob(&mut self, blob: &[u8]) {
        let mut r = BlobReader::new(blob);
        if let Some(v) = r.i32() {
            self.day = v;
        }
        if let Some(v) = r.i32() {
            self.time_of_day = v;
        }
    }

    /// The `time` blob the world tick writes (`0x0053254d`): day, then time of day.
    pub fn time_blob(&self) -> Vec<u8> {
        let mut w = BlobWriter::new();
        w.i32(self.day);
        w.i32(self.time_of_day);
        w.into_bytes()
    }

    /// `World::saveZone(zone)`, `Server.exe 0x004d81b0`: for a named world, writes the zone
    /// blob when the zone is dirty or has modified blocks. Returns whether a blob was written.
    pub fn save_zone(&self, zone: &Zone) -> cw_formats::Result<bool> {
        self.save_target().save_zone(zone)
    }

    /// `World::saveEntities(rx, ry)`, `Server.exe 0x004d7c50`: for a named world and a region
    /// whose missions came from the database (`region+0x15a18`), writes the mission and monster
    /// blobs of all 64 cells, cell by cell in `cx * 8 + cy` order.
    pub fn save_region_entities(&self, rx: i32, ry: i32) -> cw_formats::Result<bool> {
        if !(0..0x400).contains(&rx) || !(0..0x400).contains(&ry) {
            return Ok(false);
        }
        let Some(region) = self.region(rx, ry) else { return Ok(false) };
        self.save_target().save_region_entities(rx, ry, region)
    }

    /// Where this world saves, detached from it: a zone or region taken out of the world
    /// ([`World::remove_zone`], [`World::take_region`]) can be saved without holding the world
    /// (the client's zone thread saves after releasing the world lock).
    pub fn save_target(&self) -> SaveTarget {
        SaveTarget { has_name: self.has_name, db: self.save.clone() }
    }
}

/// One save-database write taken out of the world, to be done later on any thread
/// ([`World::time_save`]).
pub struct DeferredPut {
    db: Arc<Mutex<SaveDb>>,
    key: String,
    value: Vec<u8>,
}

impl DeferredPut {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// `putBlob` of the blob into the database it was taken for.
    pub fn write(self) -> cw_formats::Result<()> {
        self.db.lock().expect("save database").put(&self.key, &self.value)
    }
}

impl World {
    /// The tick's `time` blob write (0x0053254d) for the attached database, when there is one.
    pub fn time_save(&self) -> Option<DeferredPut> {
        let db = self.save.as_ref()?;
        Some(DeferredPut { db: Arc::clone(db), key: TIME_KEY.to_string(), value: self.time_blob() })
    }
}

/// A world's name flag and save database ([`World::save_target`]).
#[derive(Clone, Default)]
pub struct SaveTarget {
    has_name: bool,
    db: Option<Arc<Mutex<SaveDb>>>,
}

impl SaveTarget {
    /// Runs `f` with the database in one transaction (Tier C: the original commits each
    /// `putBlob`, one journal sync each): the saves `f` makes, each taking the database lock on
    /// its own, join it and commit together at the end. The lock is not held while `f` runs.
    pub fn batch<T>(&self, f: impl FnOnce() -> T) -> T {
        let Some(db) = &self.db else { return f() };
        let began = db.lock().expect("save database").begin().unwrap_or(false);
        let r = f();
        if began {
            let _ = db.lock().expect("save database").commit();
        }
        r
    }

    /// [`World::save_zone`].
    pub fn save_zone(&self, zone: &Zone) -> cw_formats::Result<bool> {
        if !self.has_name || !(zone.dirty || !zone.modified.is_empty()) {
            return Ok(false);
        }
        let Some(db) = &self.db else { return Ok(false) };
        db.lock().expect("save database").put(&zone_key(zone.x, zone.y), &zone.save_blob())?;
        Ok(true)
    }

    /// [`World::save_region_entities`] for region `(rx, ry)`, given as `region`.
    pub fn save_region_entities(&self, rx: i32, ry: i32, region: &crate::region::Region) -> cw_formats::Result<bool> {
        if !self.has_name {
            return Ok(false);
        }
        let Some(db) = &self.db else { return Ok(false) };
        if !region.missions_active {
            return Ok(false);
        }
        // The 128 writes go in one transaction (Tier C: the original commits each `putBlob`,
        // 128 journal syncs, seconds on a hard disk); the same rows result.
        db.lock().expect("save database").batch(|db| {
            for cx in 0..8 {
                for cy in 0..8 {
                    let cell = &region.cells[(cx * 8 + cy) as usize];
                    db.put(&mission_key(rx * 8 + cx, ry * 8 + cy), &cell.mission.to_blob())?;
                    db.put(&monster_key(rx * 8 + cx, ry * 8 + cy), &cell.monster.to_blob())?;
                }
            }
            Ok(true)
        })
    }
}

impl World {
    /// `Zone::deserialize(reader, world, zone)`, `Server.exe 0x0041ee20`, applied at the end of
    /// `generateZone` when the zone has a blob: replaces the ground items with the saved ones
    /// that have not expired (marking the zone dirty when that changes anything), replays the
    /// unexpired modified blocks and drops the props they left floating, then restores the
    /// spawn words and static bytes when the counts still match.
    pub fn apply_zone_blob(&mut self, zone: &mut Zone, blob: &[u8]) {
        let parsed = ZoneBlob::parse(blob, Some(zone.spawns.len()), Some(zone.statics.len()));
        let expired = |time: i32| !(time >= self.day - 3 || time < 0);

        // Ground items.
        let items: Vec<GroundItem> = parsed.items.into_iter().filter(|it| !expired(it.f144)).collect();
        if !ground_items_equal(&items, &zone.items) {
            zone.dirty = true;
            zone.items = items;
        }

        // Modified blocks.
        for m in &parsed.blocks {
            if !expired(m.time) {
                set_block(self, zone, m.x, m.y, m.z, m.block);
                zone.modified.push(*m);
            }
        }
        if !parsed.blocks.is_empty() {
            // Props flagged 2 (placed on a block) whose supporting block is now air are removed.
            let keep: Vec<bool> = zone
                .props
                .iter()
                .map(|p| p.flags & 2 == 0 || zone_block_fixed(zone, p.x, p.y, p.z - 0x10000)[3] & 0x1f != 0)
                .collect();
            let mut k = keep.into_iter();
            zone.props.retain(|_| k.next().unwrap());
        }

        // Spawn words +0x38/+0x3c and static bytes +0x30, only when the counts still matched.
        if let Some(words) = parsed.spawn_words {
            for (s, (a, b)) in zone.spawns.iter_mut().zip(words) {
                if let Some(v) = a {
                    s.f38[0] = v;
                }
                if let Some(v) = b {
                    s.f38[1] = v;
                }
            }
        }
        if let Some(bytes) = parsed.static_bytes {
            for (s, b) in zone.statics.iter_mut().zip(bytes) {
                if let Some(v) = b {
                    s.b30 = v;
                }
            }
        }
    }
}

/// A parsed `zone<zx>_<zy>` blob, read exactly as `Zone::deserialize` reads it: short reads
/// leave the constructor defaults in place, and the spawn and static sections are only read
/// when their counts equal the expected ones (`None` expects any count), since the original
/// skips the reads otherwise and the following section then starts at the wrong offset, as
/// it does here.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneBlob {
    pub version: u32,
    /// Every item record, expired ones included.
    pub items: Vec<GroundItem>,
    /// Every modified block, expired ones included (a short record reads as zeros).
    pub blocks: Vec<ModifiedBlock>,
    /// The `+0x38`/`+0x3c` words per spawn, when the count matched.
    pub spawn_words: Option<Vec<(Option<u32>, Option<u32>)>>,
    /// The `+0x30` byte per static, when the count matched and was positive.
    pub static_bytes: Option<Vec<Option<u8>>>,
}

impl ZoneBlob {
    pub fn parse(blob: &[u8], spawn_count: Option<usize>, static_count: Option<usize>) -> ZoneBlob {
        let mut r = BlobReader::new(blob);
        r.read_version();
        let n = r.i32().unwrap_or(0).max(0) as usize;
        let mut items = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let mut it = GroundItem::NEW;
            if let Some(raw) = r.take(Item::SIZE) {
                it.item = Item::from_bytes(raw);
            }
            if let Some(pos) = r.take(24) {
                it.x = i64::from_le_bytes(pos[0..8].try_into().unwrap());
                it.y = i64::from_le_bytes(pos[8..16].try_into().unwrap());
                it.z = i64::from_le_bytes(pos[16..24].try_into().unwrap());
            }
            if let Some(v) = r.f32() {
                it.rotation = v;
            }
            if let Some(v) = r.f32() {
                it.f134 = v;
            }
            if let Some(v) = r.u8() {
                it.b138 = v;
            }
            if let Some(v) = r.i32() {
                it.f13c = v;
            }
            if let Some(v) = r.i32() {
                it.f140 = v;
            }
            if let Some(v) = r.i32() {
                it.f144 = v;
            }
            items.push(it);
        }
        let n = r.i32().unwrap_or(0).max(0) as usize;
        let mut blocks = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            blocks.push(r.take(ModifiedBlock::SIZE).map(ModifiedBlock::from_bytes).unwrap_or(ModifiedBlock { x: 0, y: 0, z: 0, block: [0; 4], time: 0 }));
        }
        let n = r.i32().unwrap_or(0);
        let spawn_words = if n >= 0 && spawn_count.is_none_or(|c| c == n as usize) {
            Some((0..n).map(|_| (r.u32(), r.u32())).collect())
        } else {
            None
        };
        let n = r.i32().unwrap_or(0);
        let static_bytes = if n > 0 && static_count.is_none_or(|c| c == n as usize) {
            Some((0..n).map(|_| r.u8()).collect())
        } else {
            None
        };
        ZoneBlob { version: r.version, items, blocks, spawn_words, static_bytes }
    }
}

/// `getBlockFixed` (`0x00406050` then `0x004061f0`) on a zone under construction: the 16.16
/// position converted with [`to_block`], read with [`Zone::block`].
fn zone_block_fixed(zone: &Zone, x: i64, y: i64, z: i64) -> Block {
    zone.block(to_block(x), to_block(y), to_block(z))
}

/// The mission state of a cell, `cell+0x2c..+0x54` (the first word at `+0x2c` is not
/// serialised). Written by `FUN_0041f880`, read by `FUN_0041ebc0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MissionState {
    /// `+0x30`
    pub f30: i32,
    /// `+0x34`: 1 marks the cell's boss with appearance flag 0x2000 (`spawnCellNpc`).
    pub f34: i32,
    /// `+0x38`
    pub f38: i32,
    /// `+0x3c`
    pub f3c: i32,
    /// `+0x40`
    pub b40: u8,
    /// `+0x41`: mission progress byte the world tick advances.
    pub b41: u8,
    /// `+0x44`
    pub f44: i32,
    /// `+0x48`
    pub f48: i32,
    /// `+0x4c`
    pub f4c: i64,
}

impl MissionState {
    /// All zero, as `cube::Cell::Cell` leaves it.
    pub const EMPTY: MissionState = MissionState { f30: 0, f34: 0, f38: 0, f3c: 0, b40: 0, b41: 0, f44: 0, f48: 0, f4c: 0 };

    pub fn to_blob(&self) -> Vec<u8> {
        let mut w = BlobWriter::new();
        w.u32(BlobWriter::VERSION);
        w.i32(self.f30);
        w.i32(self.f34);
        w.i32(self.f38);
        w.i32(self.f3c);
        w.u8(self.b40);
        w.u8(self.b41);
        w.i32(self.f44);
        w.i32(self.f48);
        w.i64(self.f4c);
        w.into_bytes()
    }

    /// `FUN_0041ebc0(reader, cell + 0x2c)`: each field is only overwritten when it fits.
    pub fn read_blob(&mut self, blob: &[u8]) {
        let mut r = BlobReader::new(blob);
        r.read_version();
        if let Some(v) = r.i32() {
            self.f30 = v;
        }
        if let Some(v) = r.i32() {
            self.f34 = v;
        }
        if let Some(v) = r.i32() {
            self.f38 = v;
        }
        if let Some(v) = r.i32() {
            self.f3c = v;
        }
        if let Some(v) = r.u8() {
            self.b40 = v;
        }
        if let Some(v) = r.u8() {
            self.b41 = v;
        }
        if let Some(v) = r.i32() {
            self.f44 = v;
        }
        if let Some(v) = r.i32() {
            self.f48 = v;
        }
        if let Some(v) = r.i64() {
            self.f4c = v;
        }
    }
}

/// The saved monster (boss) of a cell, `cell+0x54..+0x68`. Written by `FUN_0041f9e0`, read by
/// `FUN_0041ed50`. `spawnCellNpc` places the creature at the centre of the zone named by
/// `zone` when `kind` is non-zero.
///
/// `cube::Cell::Cell` zeroes `+0x44..+0x5c` but never `+0x60`, so in the original the zone
/// word of a cell without a monster is heap garbage (and `saveEntities` writes it out as
/// such); it is 0 here and only meaningful when `kind` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonsterState {
    /// `+0x54`: entity type; 0 means no saved monster.
    pub kind: i32,
    /// `+0x58`: level.
    pub level: i32,
    /// `+0x5c`: the spawn's `+0x58` byte (rarity base).
    pub b5c: u8,
    /// `+0x60`: the zone, `zx` in the low word and `zy` in the high word.
    pub zone: i64,
}

impl MonsterState {
    /// All zero, as `cube::Cell::Cell` leaves it.
    pub const EMPTY: MonsterState = MonsterState { kind: 0, level: 0, b5c: 0, zone: 0 };

    /// The zone the monster belongs to, as `(zx, zy)`.
    pub fn zone_xy(&self) -> (i32, i32) {
        (self.zone as i32, (self.zone >> 32) as i32)
    }

    /// Packs `(zx, zy)` into the `+0x60` word pair.
    pub fn pack_zone(zx: i32, zy: i32) -> i64 {
        (i64::from(zy) << 32) | i64::from(zx as u32)
    }

    pub fn to_blob(&self) -> Vec<u8> {
        let mut w = BlobWriter::new();
        w.u32(BlobWriter::VERSION);
        w.i32(self.kind);
        w.i32(self.level);
        w.u8(self.b5c);
        w.i64(self.zone);
        w.into_bytes()
    }

    /// `FUN_0041ed50(reader, cell + 0x54)`.
    pub fn read_blob(&mut self, blob: &[u8]) {
        let mut r = BlobReader::new(blob);
        r.read_version();
        if let Some(v) = r.i32() {
            self.kind = v;
        }
        if let Some(v) = r.i32() {
            self.level = v;
        }
        if let Some(v) = r.u8() {
            self.b5c = v;
        }
        if let Some(v) = r.i64() {
            self.zone = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_stops_at_the_end_and_leaves_fields_alone() {
        let mut r = BlobReader::new(&[1, 0, 0, 0, 7, 0]);
        r.read_version();
        assert_eq!(r.version, 1);
        assert_eq!(r.i32(), None);
        assert_eq!(r.u8(), None);
        let mut m = MissionState { f30: 5, ..Default::default() };
        m.read_blob(&[1, 0, 0, 0, 9, 0, 0, 0, 2]);
        assert_eq!(m.f30, 9);
        assert_eq!(m.f34, 0);
    }

    #[test]
    fn cell_blobs_round_trip() {
        let m = MissionState { f30: 1, f34: 2, f38: 3, f3c: 4, b40: 5, b41: 6, f44: 7, f48: 8, f4c: -9 };
        let mut back = MissionState::default();
        back.read_blob(&m.to_blob());
        assert_eq!(back, m);
        assert_eq!(m.to_blob().len(), 4 + 16 + 2 + 8 + 8);
        let s = MonsterState { kind: 0x8f, level: 621, b5c: 3, zone: MonsterState::pack_zone(32860, 32779) };
        let mut back = MonsterState::default();
        back.read_blob(&s.to_blob());
        assert_eq!(back, s);
        assert_eq!(back.zone_xy(), (32860, 32779));
    }

    #[test]
    fn modified_block_bytes_round_trip() {
        let m = ModifiedBlock { x: -3, y: 70000, z: 12, block: [1, 2, 3, 0x84], time: -1 };
        assert_eq!(ModifiedBlock::from_bytes(&m.to_bytes()), m);
    }
}
