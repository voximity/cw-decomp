//! The client's save files: `Save/characters.db` and `Save/worlds.db` (`Cube.exe`).
//!
//! Tier A: the blob layouts are the original's, byte for byte (before the zlib stream, whose
//! bytes depend on the zlib build; see `analysis/notes/protocol.md` 2.4).
//!
//! # The two databases
//!
//! The `GameController` constructor 0x00459c40 opens `Save/characters.db` at `GC+0x1001008`
//! and `Save/worlds.db` at `GC+0x1001010` (`cube::Database::open` 0x004497b0, the same
//! one-table SQLite file as the world saves, [`cw_formats::SaveDb`]). In each, the blob `"num"`
//! holds the record count as a raw little-endian `u32` (read with `ByteBuffer::read(4)`
//! 0x0044d620 at 0x00464f9b / 0x004650af) and the records sit under their decimal index
//! (`stringstream << i`).
//!
//! | Blob | Writer | Reader |
//! |---|---|---|
//! | character `i` | `saveCharacter(i, creature)` 0x00487520: the creature copied under the world lock, serialised by 0x0044d790 through a `BlobWriter` (0x0044a8a0, version 1), compressed whole (`ByteBuffer::compress` 0x00449420 = `zlibCompressVec` 0x005fc0d0) | `loadCharacter(i, creature)` 0x004806c0: `getBlobVec`, `ByteBuffer::decompress` 0x004494b0 (`inflate` 0x00449540), 0x0044be40, the position kept |
//! | world `i` | `saveWorld(i, info, lock)` 0x004878a0: not compressed; only the preview voxels are (`zlibCompressVec`) | `loadWorld(i, info)` 0x004809a0 |
//! | `"num"` | 0x004821a0 (create character), 0x004816f0 (delete character), 0x00482530 (create world), 0x00481d30 (delete world), the zone thread 0x0046b1d4 (a server's world added) | the constructor |
//!
//! # The character record (0x0044d790 / 0x0044be40)
//!
//! `creature+X` is `entity+(X-0x10)` below `+0x1178`.
//!
//! | Bytes | Field |
//! |---|---|
//! | 4 | version, 1 (`BlobWriter+4`) |
//! | 0x18 | position `creature+0x10..+0x28` |
//! | 0xc | rotation `creature+0x28..+0x34` |
//! | 4 | HP `+0x16c` |
//! | 4 | XP `+0x194` |
//! | 4 | level `+0x190` |
//! | 1 | class `+0x140` |
//! | 1 | specialization `+0x141` |
//! | 4 | `+0x1198` (the riding stamina) |
//! | 4 | `+0x119c` (not identified) |
//! | 13 × 0x118 | the item slots `+0x300..+0x1138` |
//! | 4 + n | name `+0x1168` (`strlen`, no terminator) |
//! | 0x14 | the style part of the record `*(creature+0x1d28)` (race, gender, face, haircut, hair colour) |
//! | 4 + … | inventory tabs (`+0x11dc`): count, then per tab a count and `count × 0x11c` stacks |
//! | 4, 4 | coins `+0x1304`, platinum `+0x1308` |
//! | 4 + n × 0x118 | the formulas (`record+0x14` list, size `+0x18`) |
//! | 4 + … | the saved positions (`record+0x1c` map, size `+0x20`), in map order: seed, name length, name, 0x18-byte position |
//! | 4, 4 + n | the current world: seed `record+0x24`, name `record+0x28` |
//! | 4 | mana cubes `+0x1164` |
//! | 4, 0x2c | 0xb and the eleven skill levels `+0x1138` (read only when the count is 0xb) |
//!
//! The reader clamps like every `ByteBuffer` reader: a field that does not fit leaves its
//! target unchanged and moves the offset to the end.
//!
//! # `creature+0x1d28`
//!
//! The constructor 0x0044a7e0 (called from the `GameController` constructor 0x004621b8 and from
//! 0x0044be40) builds one 0x40-byte record: race `+0` (i32), gender `+4` (byte), face `+8`,
//! haircut `+0xc`, hair colour `+0x10..+0x13` (`0xffff`, `0xff` from the constructor), the
//! learned formulas `+0x14` (`std::list<Item>`, size `+0x18`), the per-world positions `+0x1c`
//! (`std::map<std::pair<int, std::string>, vec3i64>`, size `+0x20`), the current world's seed
//! `+0x24` and name `+0x28` (`std::string`). The style widget (`character_style.rs`) writes its
//! first 0x14 bytes; `startWorld`, the zone thread, 0x00444a90 and 0x004a14c0 use the rest. Both
//! readings in the port were right about their half: [`crate::ui::Style`] is the first 0x14
//! bytes, [`CharacterRecord::formulas`], [`CharacterRecord::positions`] and the current world
//! the rest.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cw_formats::SaveDb;

/// Bytes of one item (`cube::Item`).
pub const ITEM: usize = 0x118;
/// Bytes of one inventory stack (count + item).
pub const STACK: usize = 0x11c;
/// The item slots the record keeps (`creature+0x300..+0x1138`, entity `+0x2f0..+0x1128`).
pub const ITEM_SLOTS: usize = 13;
/// The entity offset of the first of them.
pub const ITEM_SLOTS_AT: usize = 0x2f0;
/// The key of the record count.
pub const NUM_KEY: &str = "num";
/// `BlobWriter+4` (0x0044a8a0): the version every record starts with.
pub const VERSION: u32 = 1;
/// The skill count the record writes before the levels.
pub const SKILL_COUNT: u32 = 0xb;

/// `Save/characters.db` (`GC+0x1001008`).
pub fn characters_path(game_dir: &Path) -> PathBuf {
    game_dir.join("Save").join("characters.db")
}

/// `Save/worlds.db` (`GC+0x1001010`).
pub fn worlds_path(game_dir: &Path) -> PathBuf {
    game_dir.join("Save").join("worlds.db")
}

/// `Save\world_<name>.db` (`World::load` 0x005a52e0 opens `"Save/world_" + name + ".db"`;
/// `deleteWorld` 0x00481d30 deletes `"Save\\world_" + name + ".db"`).
pub fn world_db_path(game_dir: &Path, name: &str) -> PathBuf {
    game_dir.join("Save").join(format!("world_{name}.db"))
}

/// `Save\map_<name>.db` (`WorldMap::load` 0x005fbc90, `deleteWorld`).
pub fn map_db_path(game_dir: &Path, name: &str) -> PathBuf {
    game_dir.join("Save").join(format!("map_{name}.db"))
}

// ---------------------------------------------------------------------------------------------
// The buffers.

/// `std::vector<char>` appends (0x005870c0 resize + copy).
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

/// `ByteBuffer` reads: `offset + n <= size` or the offset goes to the end and the target
/// keeps its value.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos.checked_add(n).is_some_and(|e| e <= self.data.len()) {
            let s = &self.data[self.pos..self.pos + n];
            self.pos += n;
            Some(s)
        } else {
            self.pos = self.data.len();
            None
        }
    }
    fn u8_into(&mut self, v: &mut u8) {
        if let Some(b) = self.take(1) {
            *v = b[0];
        }
    }
    fn i32_into(&mut self, v: &mut i32) {
        if let Some(b) = self.take(4) {
            *v = i32::from_le_bytes(b.try_into().unwrap());
        }
    }
    fn u32_into(&mut self, v: &mut u32) {
        if let Some(b) = self.take(4) {
            *v = u32::from_le_bytes(b.try_into().unwrap());
        }
    }
    fn bytes_into(&mut self, v: &mut [u8]) {
        if let Some(b) = self.take(v.len()) {
            v.copy_from_slice(b);
        }
    }
    /// A length-prefixed string: the length (0 when it does not fit), then the bytes (left
    /// empty when they do not fit).
    fn string(&mut self) -> Vec<u8> {
        let mut n = 0u32;
        self.u32_into(&mut n);
        self.take(n as usize).map(<[u8]>::to_vec).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------------
// Characters.

/// The key of a saved position: the world's seed and name (`std::pair<int, std::string>`,
/// ordered by seed, then by the name's bytes, as `std::map` orders it).
pub type WorldKey = (i32, Vec<u8>);

/// One saved character: the fields 0x0044d790 writes.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterRecord {
    pub version: u32,
    /// `creature+0x10..+0x28`.
    pub position: [i64; 3],
    /// `creature+0x28..+0x34`.
    pub rotation: [u8; 0xc],
    /// `+0x16c`.
    pub hp: f32,
    /// `+0x194`.
    pub xp: i32,
    /// `+0x190`.
    pub level: i32,
    /// `+0x140`.
    pub class: u8,
    /// `+0x141`.
    pub specialization: u8,
    /// `+0x1198`: the riding stamina (`cw_sim::riding::RidingState::ride_stamina`).
    pub ride_stamina: f32,
    /// `+0x119c`: not identified (kept as read).
    pub f119c: u32,
    /// `+0x300..+0x1138`: the thirteen item slots.
    pub items: Vec<[u8; ITEM]>,
    /// `+0x1168`: the name, without its terminator.
    pub name: Vec<u8>,
    /// `*(creature+0x1d28)+0..+0x14`: race, gender, face, haircut, hair colour.
    pub style: [u8; 0x14],
    /// `+0x11dc`: the inventory tabs, each a list of 0x11c-byte stacks.
    pub tabs: Vec<Vec<[u8; STACK]>>,
    /// `+0x1304`, `+0x1308`.
    pub coins: i32,
    pub platinum: i32,
    /// `*(creature+0x1d28)+0x14`: the learned formulas.
    pub formulas: Vec<[u8; ITEM]>,
    /// `*(creature+0x1d28)+0x1c`: the position kept per world.
    pub positions: BTreeMap<WorldKey, [i64; 3]>,
    /// `*(creature+0x1d28)+0x24`, `+0x28`: the current world.
    pub world_seed: i32,
    pub world_name: Vec<u8>,
    /// `+0x1164`: the mana cubes collected.
    pub mana_cubes: i32,
    /// `+0x1138..+0x1164`: the eleven skill levels.
    pub skills: [i32; 11],
}

impl Default for CharacterRecord {
    /// What the reader starts from (0x00446330 resets the creature; the style record is new,
    /// 0x0044a7e0; four empty tabs, 0x00487380(4)).
    fn default() -> Self {
        let mut style = [0u8; 0x14];
        style[0x10] = 0xff;
        style[0x11] = 0xff;
        style[0x12] = 0xff;
        CharacterRecord {
            version: VERSION,
            position: [0; 3],
            rotation: [0; 0xc],
            hp: 0.0,
            xp: 0,
            level: 1,
            class: 0,
            specialization: 0,
            ride_stamina: 0.0,
            f119c: 0,
            items: vec![[0; ITEM]; ITEM_SLOTS],
            name: Vec::new(),
            style,
            tabs: vec![Vec::new(); 4],
            coins: 0,
            platinum: 0,
            formulas: Vec::new(),
            positions: BTreeMap::new(),
            world_seed: 0,
            world_name: Vec::new(),
            mana_cubes: 0,
            skills: [0; 11],
        }
    }
}

impl CharacterRecord {
    /// The serialiser 0x0044d790 (uncompressed).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.version);
        for p in self.position {
            w.bytes(&p.to_le_bytes());
        }
        w.bytes(&self.rotation);
        w.bytes(&self.hp.to_le_bytes());
        w.i32(self.xp);
        w.i32(self.level);
        w.u8(self.class);
        w.u8(self.specialization);
        w.bytes(&self.ride_stamina.to_le_bytes());
        w.u32(self.f119c);
        for k in 0..ITEM_SLOTS {
            w.bytes(self.items.get(k).map_or(&[0u8; ITEM][..], |i| &i[..]));
        }
        w.u32(self.name.len() as u32);
        w.bytes(&self.name);
        w.bytes(&self.style);
        w.i32(self.tabs.len() as i32);
        for t in &self.tabs {
            w.i32(t.len() as i32);
            for s in t {
                w.bytes(s);
            }
        }
        w.i32(self.coins);
        w.i32(self.platinum);
        w.u32(self.formulas.len() as u32);
        for f in &self.formulas {
            w.bytes(f);
        }
        w.u32(self.positions.len() as u32);
        for ((seed, name), pos) in &self.positions {
            w.i32(*seed);
            w.u32(name.len() as u32);
            w.bytes(name);
            for p in pos {
                w.bytes(&p.to_le_bytes());
            }
        }
        w.i32(self.world_seed);
        w.u32(self.world_name.len() as u32);
        w.bytes(&self.world_name);
        w.i32(self.mana_cubes);
        w.u32(SKILL_COUNT);
        for s in self.skills {
            w.i32(s);
        }
        w.0
    }

    /// The reader 0x0044be40 (uncompressed), starting from [`CharacterRecord::default`].
    pub fn from_bytes(b: &[u8]) -> CharacterRecord {
        let mut c = CharacterRecord::default();
        let mut r = Reader::new(b);
        r.u32_into(&mut c.version);
        let mut pos = [0u8; 0x18];
        r.bytes_into(&mut pos);
        for i in 0..3 {
            c.position[i] = i64::from_le_bytes(pos[i * 8..i * 8 + 8].try_into().unwrap());
        }
        r.bytes_into(&mut c.rotation);
        let mut hp = 0u32;
        r.u32_into(&mut hp);
        c.hp = f32::from_bits(hp);
        r.i32_into(&mut c.xp);
        r.i32_into(&mut c.level);
        r.u8_into(&mut c.class);
        r.u8_into(&mut c.specialization);
        let mut rs = c.ride_stamina.to_bits();
        r.u32_into(&mut rs);
        c.ride_stamina = f32::from_bits(rs);
        r.u32_into(&mut c.f119c);
        for k in 0..ITEM_SLOTS {
            r.bytes_into(&mut c.items[k]);
        }
        // The name: its length (0 when short), then a `memcpy` that only runs when it fits,
        // then the terminator at `+0x1168 + len` either way.
        let mut n = 0u32;
        r.u32_into(&mut n);
        if let Some(s) = r.take(n as usize) {
            c.name = s.to_vec();
        }
        r.bytes_into(&mut c.style);
        // 0x00487380(n): the tabs resized to the count; each tab resized to its count
        // (0x0044d660), filled only when all its stacks fit.
        let mut ntabs = 0i32;
        r.i32_into(&mut ntabs);
        c.tabs = vec![Vec::new(); ntabs.max(0) as usize];
        for t in &mut c.tabs {
            let mut n = 0i32;
            r.i32_into(&mut n);
            *t = vec![[0u8; STACK]; n.max(0) as usize];
            if n > 0
                && let Some(s) = r.take(n as usize * STACK)
            {
                for (k, chunk) in s.chunks_exact(STACK).enumerate() {
                    t[k].copy_from_slice(chunk);
                }
            }
        }
        r.i32_into(&mut c.coins);
        r.i32_into(&mut c.platinum);
        let mut nf = 0i32;
        r.i32_into(&mut nf);
        for _ in 0..nf.max(0) {
            // Each formula starts as a default item (type 0, level 1) and is overwritten when
            // the 0x118 bytes fit.
            let mut f = [0u8; ITEM];
            f[0x10] = 1;
            r.bytes_into(&mut f);
            c.formulas.push(f);
        }
        let mut np = 0i32;
        r.i32_into(&mut np);
        let mut seed = 0i32;
        for _ in 0..np.max(0) {
            r.i32_into(&mut seed);
            let name = r.string();
            let mut p = [0u8; 0x18];
            r.bytes_into(&mut p);
            let pos = [0, 1, 2].map(|i| i64::from_le_bytes(p[i * 8..i * 8 + 8].try_into().unwrap()));
            c.positions.insert((seed, name), pos);
        }
        r.i32_into(&mut c.world_seed);
        c.world_name = r.string();
        r.i32_into(&mut c.mana_cubes);
        let mut ns = 0u32;
        r.u32_into(&mut ns);
        if ns == SKILL_COUNT {
            let mut s = [0u8; 0x2c];
            if let Some(b) = r.take(0x2c) {
                s.copy_from_slice(b);
                for k in 0..11 {
                    c.skills[k] = i32::from_le_bytes(s[k * 4..k * 4 + 4].try_into().unwrap());
                }
            }
        }
        c
    }

    /// The stored blob: the record compressed whole (`ByteBuffer::compress` 0x00449420).
    pub fn to_blob(&self) -> Vec<u8> {
        cw_net::compress::compress(&self.to_bytes())
    }

    /// `ByteBuffer::decompress` 0x004494b0 (whatever inflated before an error is kept) and the
    /// reader.
    pub fn from_blob(blob: &[u8]) -> CharacterRecord {
        let raw = cw_net::compress::decompress(blob).unwrap_or_else(|(partial, _)| partial);
        CharacterRecord::from_bytes(&raw)
    }

    /// The saved position for a world (`record+0x1c` find, 0x0044b880).
    pub fn position_in(&self, seed: i32, name: &str) -> Option<[i64; 3]> {
        self.positions.get(&(seed, name.as_bytes().to_vec())).copied()
    }
}

// ---------------------------------------------------------------------------------------------
// Worlds.

/// One saved world (`cube::WorldInfo`, 0x28 bytes: preview model `+4`, name `+8`, seed
/// `+0x20`, explored `+0x24`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldRecord {
    pub name: Vec<u8>,
    pub seed: i32,
    pub explored: i32,
    /// The preview model (`+4`, a `cube::Model`): its size (`+0x44..+0x4c`) and RGB voxels
    /// (`+0x30`).
    pub preview: Option<([i32; 3], Vec<u8>)>,
}

impl WorldRecord {
    /// The blob of 0x004878a0: name, seed, explored, the model size (zeros without a model),
    /// then with a positive size the compressed voxels (length-prefixed).
    pub fn to_blob(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.name.len() as u32);
        w.bytes(&self.name);
        w.i32(self.seed);
        w.i32(self.explored);
        let size = self.preview.as_ref().map_or([0; 3], |p| p.0);
        for s in size {
            w.i32(s);
        }
        if let Some((s, voxels)) = &self.preview
            && s[0] > 0
            && s[1] > 0
            && s[2] > 0
        {
            let z = cw_net::compress::compress(voxels);
            w.u32(z.len() as u32);
            w.bytes(&z);
        }
        w.0
    }

    /// The reader 0x004809a0.
    pub fn from_blob(b: &[u8]) -> WorldRecord {
        let mut out = WorldRecord::default();
        let mut r = Reader::new(b);
        out.name = r.string();
        r.i32_into(&mut out.seed);
        r.i32_into(&mut out.explored);
        let mut size = [0u8; 0xc];
        let got = r.take(0xc).map(|s| size.copy_from_slice(s)).is_some();
        let s = [0, 1, 2].map(|i| i32::from_le_bytes(size[i * 4..i * 4 + 4].try_into().unwrap()));
        if got && s[0] > 0 && s[1] > 0 && s[2] > 0 {
            let mut n = 0u32;
            r.u32_into(&mut n);
            if n != 0 {
                let z = r.take(n as usize).map(<[u8]>::to_vec).unwrap_or_default();
                let raw = cw_net::compress::decompress(&z).unwrap_or_else(|(partial, _)| partial);
                out.preview = Some((s, raw));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// The databases.

/// `Save/characters.db` or `Save/worlds.db`: `"num"` and the records by index.
pub struct RecordDb {
    pub db: SaveDb,
}

impl RecordDb {
    /// `cube::Database::open` 0x004497b0 (the `Save` folder is WinMain's `_mkdir`).
    pub fn open(path: &Path) -> Result<RecordDb, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let db = SaveDb::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(RecordDb { db })
    }

    /// `"num"`, 0 when missing or short.
    pub fn count(&self) -> i32 {
        match self.db.get(NUM_KEY) {
            Ok(Some(b)) if b.len() >= 4 => i32::from_le_bytes(b[0..4].try_into().unwrap()),
            _ => 0,
        }
    }

    pub fn set_count(&self, n: i32) {
        let _ = self.db.put(NUM_KEY, &n.to_le_bytes());
    }

    pub fn get(&self, index: i32) -> Option<Vec<u8>> {
        self.db.get(&index.to_string()).ok().flatten()
    }

    pub fn put(&self, index: i32, blob: &[u8]) {
        let _ = self.db.put(&index.to_string(), blob);
    }

    /// `deleteCharacter` 0x004816f0: `"num"` rewritten with the new count, the record at
    /// `index` deleted (0x00449720), and every later record moved down one key.
    pub fn remove_shift(&self, index: i32, new_count: i32) {
        self.set_count(new_count);
        let _ = self.db.delete(&index.to_string());
        for j in index..new_count {
            if let Some(b) = self.get(j + 1) {
                self.put(j, &b);
            }
        }
        let _ = self.db.delete(&new_count.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CharacterRecord {
        let mut c = CharacterRecord::default();
        c.position = [1 << 30, -5, 77 << 16];
        c.rotation = [1; 0xc];
        c.hp = 123.5;
        c.xp = 42;
        c.level = 7;
        c.class = 2;
        c.specialization = 1;
        c.ride_stamina = 0.5;
        c.f119c = 9;
        c.items[3][0] = 3;
        c.name = b"Wollay".to_vec();
        c.style[0] = 4;
        c.tabs = vec![vec![[7u8; STACK]; 2], Vec::new(), vec![[1u8; STACK]], Vec::new()];
        c.coins = 1000;
        c.platinum = 3;
        c.formulas = vec![[5u8; ITEM]];
        c.positions.insert((12, b"b".to_vec()), [1, 2, 3]);
        c.positions.insert((12, b"a".to_vec()), [4, 5, 6]);
        c.positions.insert((-1, b"z".to_vec()), [7, 8, 9]);
        c.world_seed = 12;
        c.world_name = b"a".to_vec();
        c.mana_cubes = 4;
        c.skills = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        c
    }

    #[test]
    fn character_layout() {
        let c = sample();
        let b = c.to_bytes();
        assert_eq!(&b[0..4], &1u32.to_le_bytes());
        assert_eq!(&b[4..12], &(1i64 << 30).to_le_bytes());
        // version, position, rotation, hp, xp, level, class, spec, 0x1198, 0x119c.
        let items = 4 + 0x18 + 0xc + 4 + 4 + 4 + 1 + 1 + 4 + 4;
        assert_eq!(items, 0x3e);
        assert_eq!(b[items + 3 * ITEM], 3);
        let name = items + ITEM_SLOTS * ITEM;
        assert_eq!(&b[name..name + 4], &6u32.to_le_bytes());
        assert_eq!(&b[name + 4..name + 10], b"Wollay");
        let style = name + 10;
        assert_eq!(b[style], 4);
        assert_eq!(&b[style + 0x10..style + 0x13], &[0xff; 3]);
        let tabs = style + 0x14;
        assert_eq!(&b[tabs..tabs + 4], &4i32.to_le_bytes());
        assert_eq!(&b[tabs + 4..tabs + 8], &2i32.to_le_bytes());
        // The map order: seed first, then the name bytes.
        let s = String::from_utf8_lossy(&b);
        let (ia, ib, iz) = (s.find("\u{1}\0\0\0a").unwrap(), s.find("\u{1}\0\0\0b").unwrap(), s.find("\u{1}\0\0\0z").unwrap());
        assert!(iz < ia && ia < ib);
        // The tail: mana cubes, 0xb, the skills.
        let n = b.len();
        assert_eq!(&b[n - 0x2c - 4..n - 0x2c], &0xbu32.to_le_bytes());
        assert_eq!(&b[n - 0x2c - 8..n - 0x2c - 4], &4i32.to_le_bytes());
        assert_eq!(&b[n - 4..], &11i32.to_le_bytes());
    }

    #[test]
    fn character_round_trip() {
        let c = sample();
        assert_eq!(CharacterRecord::from_bytes(&c.to_bytes()), c);
        assert_eq!(CharacterRecord::from_blob(&c.to_blob()), c);
        assert_eq!(CharacterRecord::from_bytes(&CharacterRecord::default().to_bytes()), CharacterRecord::default());
    }

    #[test]
    fn truncated_character_keeps_defaults() {
        let c = sample();
        let b = c.to_bytes();
        let r = CharacterRecord::from_bytes(&b[..0x30]);
        assert_eq!(r.position, c.position);
        assert_eq!(r.level, 1);
        assert!(r.name.is_empty());
        // Without the skill count 0xb the skills are not read.
        let mut b2 = b.clone();
        let n = b2.len();
        b2[n - 0x30..n - 0x2c].copy_from_slice(&10u32.to_le_bytes());
        assert_eq!(CharacterRecord::from_bytes(&b2).skills, [0; 11]);
    }

    #[test]
    fn world_round_trip_and_layout() {
        let w = WorldRecord { name: b"Home".to_vec(), seed: 1234, explored: 5, preview: None };
        let b = w.to_blob();
        assert_eq!(b.len(), 4 + 4 + 4 + 4 + 0xc);
        assert_eq!(&b[0..8], &[4, 0, 0, 0, b'H', b'o', b'm', b'e']);
        assert_eq!(&b[8..12], &1234i32.to_le_bytes());
        assert_eq!(WorldRecord::from_blob(&b), w);
        let w = WorldRecord { name: b"x".to_vec(), seed: -3, explored: 0, preview: Some(([2, 1, 1], vec![1, 2, 3, 4, 5, 6])) };
        assert_eq!(WorldRecord::from_blob(&w.to_blob()), w);
    }

    #[test]
    fn record_db_shift() {
        let dir = std::env::temp_dir().join(format!("cw-client-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = RecordDb::open(&dir.join("Save").join("characters.db")).unwrap();
        for i in 0..3 {
            db.put(i, &[i as u8]);
        }
        db.set_count(3);
        db.remove_shift(1, 2);
        assert_eq!(db.count(), 2);
        assert_eq!(db.get(0), Some(vec![0]));
        assert_eq!(db.get(1), Some(vec![2]));
        assert_eq!(db.get(2), None);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
