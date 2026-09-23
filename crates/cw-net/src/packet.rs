//! Packet ids, payload layouts and framing (`analysis/notes/protocol.md`, section 2).
//!
//! Every packet is a little-endian `u32` id followed by a payload whose length the id implies;
//! there is no outer length and no checksum. Records that the world tick fills and the server
//! relays unchanged (hits, shoots, particles, ...) are kept as raw byte arrays of the original
//! size; the records built by the network code itself (block actions, static entities,
//! missions) have typed accessors with the layouts confirmed in the binary.

use crate::bytebuf::ByteBuffer;
use crate::compress::{compress, decompress};
use crate::entity::{ENTITY_SIZE, EntityData};

/// Packet ids, both directions (`receiveLoop` jump table `0x0042671c`; send sites in `sendLoop`
/// and `acceptLoop`).
pub mod id {
    pub const ENTITY_UPDATE: u32 = 0;
    pub const UPDATE_FINISHED: u32 = 2;
    pub const VERSION_MISMATCH: u32 = 3;
    pub const SERVER_UPDATE: u32 = 4;
    pub const CURRENT_TIME: u32 = 5;
    pub const INTERACT: u32 = 6;
    pub const HIT: u32 = 7;
    pub const PASSIVE: u32 = 8;
    pub const SHOOT: u32 = 9;
    pub const CHAT: u32 = 10;
    pub const ZONE_DISCOVERED: u32 = 11;
    pub const REGION_DISCOVERED: u32 = 12;
    pub const SEED: u32 = 0x0f;
    pub const JOIN: u32 = 0x10;
    pub const CLIENT_VERSION: u32 = 0x11;
    pub const SERVER_FULL: u32 = 0x12;
}

/// The protocol version the server accepts (`acceptLoop`, compared at `0x004256c5`).
pub const PROTOCOL_VERSION: u32 = 3;

macro_rules! raw_record {
    ($(#[$doc:meta])* $name:ident, $size:expr) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(pub [u8; $size]);

        impl $name {
            pub const SIZE: usize = $size;

            pub fn from_slice(b: &[u8]) -> Option<Self> {
                b.try_into().ok().map(Self)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self([0; $size])
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({} bytes)", stringify!($name), $size)
            }
        }
    };
}

raw_record!(/// Client packet 6 payload (0x12c bytes), queued on the player creature for the tick.
    Interact, 0x12c);
raw_record!(/// Client packet 7 and ServerUpdate section 2 element (0x48 bytes), relayed unchanged.
    Hit, 0x48);
raw_record!(/// Client packet 8 and ServerUpdate section 12 element (0x28 bytes).
    Passive, 0x28);
raw_record!(/// Client packet 9 and ServerUpdate section 5 element (0x70 bytes).
    Shoot, 0x70);
raw_record!(/// ServerUpdate section 3 element (0x48 bytes); the first 24 bytes are the i64 position the distance filter reads.
    Particle, 0x48);
raw_record!(/// ServerUpdate section 4 element (0x18 bytes); the first 12 bytes are the f32 block position the distance filter reads.
    Sound, 0x18);
raw_record!(/// ServerUpdate section 10 element (0x18 bytes).
    Kill, 0x18);
raw_record!(/// ServerUpdate section 11 element (0x18 bytes).
    Damage, 0x18);
raw_record!(/// ServerUpdate section 9 element (0x120 bytes): i64 entity id and a 0x118-byte item.
    Pickup, 0x120);
raw_record!(/// ServerUpdate section 7 element (0x148 bytes): the zone's ground item record (`cube::GroundItem`).
    GroundItem, 0x148);
raw_record!(/// ServerUpdate section 8 element (16 bytes); its producer in the world tick is not identified.
    Item8, 0x10);

/// ServerUpdate section 1 element (0x14 bytes, the `zone+0x68` modified block): position, block,
/// day placed (-1 permanent). Writer `0x00421b10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockAction {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: [u8; 4],
    pub time: i32,
}

impl BlockAction {
    pub const SIZE: usize = 0x14;

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

/// ServerUpdate section 6 element (0x58 bytes), built for a discovered zone from the zone's
/// static record (`0x00427b40..0x00427c25`). The original leaves `+0x0c`, `+0x14`, `+0x41..0x43`
/// and `+0x4c` as uninitialised stack; they are written as zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StaticEntity {
    /// `+0x00`, `+0x04`: the zone of the request.
    pub zone_x: i32,
    pub zone_y: i32,
    /// `+0x08`: index of the record in the zone's static vector.
    pub index: i32,
    /// `+0x10`: static kind (`s+0`).
    pub kind: u32,
    /// `+0x18`: position in 16.16 fixed blocks (`s+8`).
    pub x: i64,
    pub y: i64,
    pub z: i64,
    /// `+0x30`: rotation (`s+0x20`).
    pub rotation: i32,
    /// `+0x34`: scale (`s+0x24`), as bits.
    pub scale: [u32; 3],
    /// `+0x40`: the byte at `s+0x30`.
    pub b40: u8,
    /// `+0x44`, `+0x48`: `s+0x34`, `s+0x38`.
    pub f44: u32,
    pub f48: u32,
    /// `+0x50`: `s+0x40..0x48` (cuwo `user_id`).
    pub user_id: i64,
}

impl StaticEntity {
    pub const SIZE: usize = 0x58;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0x00..0x04].copy_from_slice(&self.zone_x.to_le_bytes());
        b[0x04..0x08].copy_from_slice(&self.zone_y.to_le_bytes());
        b[0x08..0x0c].copy_from_slice(&self.index.to_le_bytes());
        b[0x10..0x14].copy_from_slice(&self.kind.to_le_bytes());
        b[0x18..0x20].copy_from_slice(&self.x.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&self.y.to_le_bytes());
        b[0x28..0x30].copy_from_slice(&self.z.to_le_bytes());
        b[0x30..0x34].copy_from_slice(&self.rotation.to_le_bytes());
        for (i, s) in self.scale.iter().enumerate() {
            b[0x34 + 4 * i..0x38 + 4 * i].copy_from_slice(&s.to_le_bytes());
        }
        b[0x40] = self.b40;
        b[0x44..0x48].copy_from_slice(&self.f44.to_le_bytes());
        b[0x48..0x4c].copy_from_slice(&self.f48.to_le_bytes());
        b[0x50..0x58].copy_from_slice(&self.user_id.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let i32_at = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let i64_at = |o: usize| i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        Self {
            zone_x: i32_at(0),
            zone_y: i32_at(4),
            index: i32_at(8),
            kind: u32_at(0x10),
            x: i64_at(0x18),
            y: i64_at(0x20),
            z: i64_at(0x28),
            rotation: i32_at(0x30),
            scale: [u32_at(0x34), u32_at(0x38), u32_at(0x3c)],
            b40: b[0x40],
            f44: u32_at(0x44),
            f48: u32_at(0x48),
            user_id: i64_at(0x50),
        }
    }
}

/// ServerUpdate section 13 element (0x38 bytes), built for a discovered region from a cell
/// (`0x00427dd2..0x00427e6c`). `+0x26..0x28` is uninitialised stack in the original, zero here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mission {
    /// `+0x00`, `+0x04`: cell coordinates (`region * 8 + cell`).
    pub cell_x: i32,
    pub cell_y: i32,
    /// `+0x10`: `cell+0x2c..0x40`, five words (the first is the word before the mission state).
    pub words: [i32; 5],
    /// `+0x24`, `+0x25`: `cell+0x40`, `cell+0x41` (progress).
    pub b40: u8,
    pub b41: u8,
    /// `+0x28`: `cell+0x44..0x54`, four words.
    pub tail: [i32; 4],
}

impl Mission {
    pub const SIZE: usize = 0x38;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..4].copy_from_slice(&self.cell_x.to_le_bytes());
        b[4..8].copy_from_slice(&self.cell_y.to_le_bytes());
        for (i, w) in self.words.iter().enumerate() {
            b[0x10 + 4 * i..0x14 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        b[0x24] = self.b40;
        b[0x25] = self.b41;
        for (i, w) in self.tail.iter().enumerate() {
            b[0x28 + 4 * i..0x2c + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        b
    }

    pub fn from_bytes(b: &[u8]) -> Self {
        let i = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Self {
            cell_x: i(0),
            cell_y: i(4),
            words: [i(0x10), i(0x14), i(0x18), i(0x1c), i(0x20)],
            b40: b[0x24],
            b41: b[0x25],
            tail: [i(0x28), i(0x2c), i(0x30), i(0x34)],
        }
    }
}

/// The ground items of one zone in ServerUpdate section 7: the zone key (`i32 x, i32 y` as one
/// 8-byte copy) and the items (`0x00421dc0`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZoneItems {
    pub zone_x: i32,
    pub zone_y: i32,
    pub items: Vec<GroundItem>,
}

/// One entry of ServerUpdate section 8: an 8-byte key and 16-byte records (`0x00421a30`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Items8 {
    pub key: [u8; 8],
    pub items: Vec<Item8>,
}

/// The decompressed body of packet 4, ServerUpdate: thirteen `u32 count` + elements sections in
/// this order (`sendLoop 0x00424d22..0x00425011`; the order differs from the `UpdateLists`
/// slot order).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerUpdate {
    pub block_actions: Vec<BlockAction>,
    pub hits: Vec<Hit>,
    pub particles: Vec<Particle>,
    pub sounds: Vec<Sound>,
    pub shoots: Vec<Shoot>,
    pub statics: Vec<StaticEntity>,
    pub zone_items: Vec<ZoneItems>,
    pub items_8: Vec<Items8>,
    pub pickups: Vec<Pickup>,
    pub kills: Vec<Kill>,
    pub damage: Vec<Damage>,
    pub passives: Vec<Passive>,
    pub missions: Vec<Mission>,
}

impl ServerUpdate {
    pub fn is_empty(&self) -> bool {
        *self == ServerUpdate::default()
    }

    /// The uncompressed body.
    pub fn write_body(&self, buf: &mut ByteBuffer) {
        fn section<T>(buf: &mut ByteBuffer, items: &[T], bytes: impl Fn(&T) -> Vec<u8>) {
            buf.write_u32(items.len() as u32);
            for it in items {
                buf.write(&bytes(it));
            }
        }
        section(buf, &self.block_actions, |b| b.to_bytes().to_vec());
        section(buf, &self.hits, |h| h.0.to_vec());
        section(buf, &self.particles, |p| p.0.to_vec());
        section(buf, &self.sounds, |s| s.0.to_vec());
        section(buf, &self.shoots, |s| s.0.to_vec());
        section(buf, &self.statics, |s| s.to_bytes().to_vec());
        buf.write_u32(self.zone_items.len() as u32);
        for z in &self.zone_items {
            buf.write_i32(z.zone_x);
            buf.write_i32(z.zone_y);
            section(buf, &z.items, |g| g.0.to_vec());
        }
        buf.write_u32(self.items_8.len() as u32);
        for e in &self.items_8 {
            buf.write(&e.key);
            section(buf, &e.items, |i| i.0.to_vec());
        }
        section(buf, &self.pickups, |p| p.0.to_vec());
        section(buf, &self.kills, |k| k.0.to_vec());
        section(buf, &self.damage, |d| d.0.to_vec());
        section(buf, &self.passives, |p| p.0.to_vec());
        section(buf, &self.missions, |m| m.to_bytes().to_vec());
    }

    pub fn body(&self) -> Vec<u8> {
        let mut buf = ByteBuffer::new();
        self.write_body(&mut buf);
        buf.into_vec()
    }

    /// Parses an uncompressed body; `None` when it is truncated.
    pub fn read_body(body: &[u8]) -> Option<ServerUpdate> {
        let mut r = ByteBuffer::from_vec(body.to_vec());
        fn section<T>(r: &mut ByteBuffer, size: usize, make: impl Fn(&[u8]) -> T) -> Option<Vec<T>> {
            let n = r.read_u32()? as usize;
            let mut out = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                out.push(make(r.read(size)?));
            }
            Some(out)
        }
        let block_actions = section(&mut r, BlockAction::SIZE, BlockAction::from_bytes)?;
        let hits = section(&mut r, Hit::SIZE, |b| Hit::from_slice(b).unwrap())?;
        let particles = section(&mut r, Particle::SIZE, |b| Particle::from_slice(b).unwrap())?;
        let sounds = section(&mut r, Sound::SIZE, |b| Sound::from_slice(b).unwrap())?;
        let shoots = section(&mut r, Shoot::SIZE, |b| Shoot::from_slice(b).unwrap())?;
        let statics = section(&mut r, StaticEntity::SIZE, StaticEntity::from_bytes)?;
        let nz = r.read_u32()? as usize;
        let mut zone_items = Vec::with_capacity(nz.min(1 << 16));
        for _ in 0..nz {
            let zone_x = r.read_i32()?;
            let zone_y = r.read_i32()?;
            let items = section(&mut r, GroundItem::SIZE, |b| GroundItem::from_slice(b).unwrap())?;
            zone_items.push(ZoneItems { zone_x, zone_y, items });
        }
        let n8 = r.read_u32()? as usize;
        let mut items_8 = Vec::with_capacity(n8.min(1 << 16));
        for _ in 0..n8 {
            let key: [u8; 8] = r.read(8)?.try_into().unwrap();
            let items = section(&mut r, Item8::SIZE, |b| Item8::from_slice(b).unwrap())?;
            items_8.push(Items8 { key, items });
        }
        let pickups = section(&mut r, Pickup::SIZE, |b| Pickup::from_slice(b).unwrap())?;
        let kills = section(&mut r, Kill::SIZE, |b| Kill::from_slice(b).unwrap())?;
        let damage = section(&mut r, Damage::SIZE, |b| Damage::from_slice(b).unwrap())?;
        let passives = section(&mut r, Passive::SIZE, |b| Passive::from_slice(b).unwrap())?;
        let missions = section(&mut r, Mission::SIZE, Mission::from_bytes)?;
        Some(ServerUpdate { block_actions, hits, particles, sounds, shoots, statics, zone_items, items_8, pickups, kills, damage, passives, missions })
    }
}

/// A packet the server sends.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerPacket {
    /// `acceptLoop 0x0042576f`: the bare u32 3, then the socket is closed.
    VersionMismatch,
    /// `u32 0x12` (unreachable in the original, see the note).
    ServerFull,
    /// `u32 0x10, u32 0, i64 id, 0x1168 bytes`.
    Join { id: i64, entity: Box<EntityData> },
    /// `u32 0x0f, u32 seed`.
    Seed(u32),
    /// Packet 0: `i64 id` and the masked delta, compressed together.
    EntityUpdate { id: i64, delta: Vec<u8> },
    /// `u32 2`.
    UpdateFinished,
    /// `u32 5, u32 day, u32 time of day (ms)`.
    CurrentTime { day: u32, time: u32 },
    /// Packet 4: the compressed body.
    ServerUpdate(Box<ServerUpdate>),
    /// `u32 10, i64 sender, u32 n, n UTF-16 code units`.
    Chat { sender: i64, text: Vec<u16> },
}

impl ServerPacket {
    /// The bytes as the original's `send` calls emit them, appended to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            ServerPacket::VersionMismatch => out.extend_from_slice(&id::VERSION_MISMATCH.to_le_bytes()),
            ServerPacket::ServerFull => out.extend_from_slice(&id::SERVER_FULL.to_le_bytes()),
            ServerPacket::Join { id: eid, entity } => {
                out.extend_from_slice(&id::JOIN.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&eid.to_le_bytes());
                out.extend_from_slice(&entity.0);
            }
            ServerPacket::Seed(seed) => {
                out.extend_from_slice(&id::SEED.to_le_bytes());
                out.extend_from_slice(&seed.to_le_bytes());
            }
            ServerPacket::EntityUpdate { id: eid, delta } => {
                let mut body = Vec::with_capacity(8 + delta.len());
                body.extend_from_slice(&eid.to_le_bytes());
                body.extend_from_slice(delta);
                let z = compress(&body);
                out.extend_from_slice(&id::ENTITY_UPDATE.to_le_bytes());
                out.extend_from_slice(&(z.len() as u32).to_le_bytes());
                out.extend_from_slice(&z);
            }
            ServerPacket::UpdateFinished => out.extend_from_slice(&id::UPDATE_FINISHED.to_le_bytes()),
            ServerPacket::CurrentTime { day, time } => {
                out.extend_from_slice(&id::CURRENT_TIME.to_le_bytes());
                out.extend_from_slice(&day.to_le_bytes());
                out.extend_from_slice(&time.to_le_bytes());
            }
            ServerPacket::ServerUpdate(u) => {
                let z = compress(&u.body());
                out.extend_from_slice(&id::SERVER_UPDATE.to_le_bytes());
                out.extend_from_slice(&(z.len() as u32).to_le_bytes());
                out.extend_from_slice(&z);
            }
            ServerPacket::Chat { sender, text } => {
                out.extend_from_slice(&id::CHAT.to_le_bytes());
                out.extend_from_slice(&sender.to_le_bytes());
                out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                for cu in text {
                    out.extend_from_slice(&cu.to_le_bytes());
                }
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// A packet the server receives (`receiveLoop 0x00426020`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPacket {
    /// `0x11`: the client's protocol version; only meaningful as the first packet.
    Version(u32),
    /// Packet 0: the compressed `i64 id` + delta. The id is ignored by the original; the
    /// update always applies to the connection's own creature.
    EntityUpdate { compressed: Vec<u8> },
    /// Ids 1..5 and every id above 12: no payload is read, the packet is dropped.
    Ignored(u32),
    Interact(Box<Interact>),
    Hit(Box<Hit>),
    Passive(Box<Passive>),
    Shoot(Box<Shoot>),
    /// Packet 10: `i32 n` then `n` UTF-16 code units (nothing more when `n <= 0`).
    Chat(Vec<u16>),
    ZoneDiscovered { x: i32, y: i32 },
    RegionDiscovered { x: i32, y: i32 },
}

impl ClientPacket {
    /// Decodes an entity update payload into `(id, delta bytes)`.
    pub fn decode_entity_update(compressed: &[u8]) -> Option<(i64, Vec<u8>)> {
        let body = decompress(compressed).ok()?;
        if body.len() < 8 {
            return None;
        }
        let eid = i64::from_le_bytes(body[..8].try_into().unwrap());
        Some((eid, body[8..].to_vec()))
    }
}

/// Result of [`parse_client_packet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parse {
    /// A whole packet and the number of bytes it used.
    Packet(ClientPacket, usize),
    /// More bytes are needed.
    NeedMore,
    /// The original ends the connection's receive loop: an entity update with `len == 0`
    /// (`0x0042615f`).
    Fatal,
}

/// Parses one client packet from the front of `buf`, with the original's framing rules.
pub fn parse_client_packet(buf: &[u8]) -> Parse {
    fn need(buf: &[u8], n: usize) -> Option<&[u8]> {
        buf.get(..n)
    }
    let Some(head) = need(buf, 4) else { return Parse::NeedMore };
    let pid = u32::from_le_bytes(head.try_into().unwrap());
    let rest = &buf[4..];
    let fixed = |size: usize| need(rest, size).map(|b| (b, 4 + size));
    match pid {
        id::CLIENT_VERSION => match need(rest, 4) {
            Some(b) => Parse::Packet(ClientPacket::Version(u32::from_le_bytes(b.try_into().unwrap())), 8),
            None => Parse::NeedMore,
        },
        id::ENTITY_UPDATE => {
            let Some(l) = need(rest, 4) else { return Parse::NeedMore };
            let len = u32::from_le_bytes(l.try_into().unwrap()) as usize;
            if len == 0 {
                return Parse::Fatal;
            }
            match need(&rest[4..], len) {
                Some(b) => Parse::Packet(ClientPacket::EntityUpdate { compressed: b.to_vec() }, 8 + len),
                None => Parse::NeedMore,
            }
        }
        1..=5 => Parse::Packet(ClientPacket::Ignored(pid), 4),
        id::INTERACT => match fixed(Interact::SIZE) {
            Some((b, n)) => Parse::Packet(ClientPacket::Interact(Box::new(Interact::from_slice(b).unwrap())), n),
            None => Parse::NeedMore,
        },
        id::HIT => match fixed(Hit::SIZE) {
            Some((b, n)) => Parse::Packet(ClientPacket::Hit(Box::new(Hit::from_slice(b).unwrap())), n),
            None => Parse::NeedMore,
        },
        id::PASSIVE => match fixed(Passive::SIZE) {
            Some((b, n)) => Parse::Packet(ClientPacket::Passive(Box::new(Passive::from_slice(b).unwrap())), n),
            None => Parse::NeedMore,
        },
        id::SHOOT => match fixed(Shoot::SIZE) {
            Some((b, n)) => Parse::Packet(ClientPacket::Shoot(Box::new(Shoot::from_slice(b).unwrap())), n),
            None => Parse::NeedMore,
        },
        id::CHAT => {
            let Some(l) = need(rest, 4) else { return Parse::NeedMore };
            let n = i32::from_le_bytes(l.try_into().unwrap());
            if n <= 0 {
                return Parse::Packet(ClientPacket::Chat(Vec::new()), 8);
            }
            let bytes = n as usize * 2;
            match need(&rest[4..], bytes) {
                Some(b) => {
                    let text = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                    Parse::Packet(ClientPacket::Chat(text), 8 + bytes)
                }
                None => Parse::NeedMore,
            }
        }
        id::ZONE_DISCOVERED | id::REGION_DISCOVERED => match need(rest, 8) {
            Some(b) => {
                let x = i32::from_le_bytes(b[0..4].try_into().unwrap());
                let y = i32::from_le_bytes(b[4..8].try_into().unwrap());
                let p = if pid == id::ZONE_DISCOVERED { ClientPacket::ZoneDiscovered { x, y } } else { ClientPacket::RegionDiscovered { x, y } };
                Parse::Packet(p, 12)
            }
            None => Parse::NeedMore,
        },
        _ => Parse::Packet(ClientPacket::Ignored(pid), 4),
    }
}

/// Encodes a client packet as the client would send it (for tests and a future client).
pub fn encode_client_packet(p: &ClientPacket, out: &mut Vec<u8>) {
    match p {
        ClientPacket::Version(v) => {
            out.extend_from_slice(&id::CLIENT_VERSION.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        ClientPacket::EntityUpdate { compressed } => {
            out.extend_from_slice(&id::ENTITY_UPDATE.to_le_bytes());
            out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            out.extend_from_slice(compressed);
        }
        ClientPacket::Ignored(pid) => out.extend_from_slice(&pid.to_le_bytes()),
        ClientPacket::Interact(r) => {
            out.extend_from_slice(&id::INTERACT.to_le_bytes());
            out.extend_from_slice(&r.0);
        }
        ClientPacket::Hit(r) => {
            out.extend_from_slice(&id::HIT.to_le_bytes());
            out.extend_from_slice(&r.0);
        }
        ClientPacket::Passive(r) => {
            out.extend_from_slice(&id::PASSIVE.to_le_bytes());
            out.extend_from_slice(&r.0);
        }
        ClientPacket::Shoot(r) => {
            out.extend_from_slice(&id::SHOOT.to_le_bytes());
            out.extend_from_slice(&r.0);
        }
        ClientPacket::Chat(text) => {
            out.extend_from_slice(&id::CHAT.to_le_bytes());
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            for cu in text {
                out.extend_from_slice(&cu.to_le_bytes());
            }
        }
        ClientPacket::ZoneDiscovered { x, y } | ClientPacket::RegionDiscovered { x, y } => {
            let pid = if matches!(p, ClientPacket::ZoneDiscovered { .. }) { id::ZONE_DISCOVERED } else { id::REGION_DISCOVERED };
            out.extend_from_slice(&pid.to_le_bytes());
            out.extend_from_slice(&x.to_le_bytes());
            out.extend_from_slice(&y.to_le_bytes());
        }
    }
}

/// The client's entity update payload: `i64 id` and a delta, compressed.
pub fn encode_entity_update_payload(eid: i64, delta: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + delta.len());
    body.extend_from_slice(&eid.to_le_bytes());
    body.extend_from_slice(delta);
    compress(&body)
}

/// Size of the Join packet: id, zero word, entity id and the entity block.
pub const JOIN_SIZE: usize = 4 + 4 + 8 + ENTITY_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_update_body_round_trips_in_wire_order() {
        let mut u = ServerUpdate::default();
        u.block_actions.push(BlockAction { x: 1, y: 2, z: 3, block: [4, 5, 6, 7], time: -1 });
        u.hits.push(Hit([9; 0x48]));
        u.statics.push(StaticEntity { zone_x: 32768, zone_y: 32769, index: 2, kind: 0x41, x: 1 << 16, y: 2 << 16, z: 3 << 16, rotation: 1, scale: [0x3f800000; 3], b40: 1, f44: 0, f48: 0, user_id: -1 });
        u.zone_items.push(ZoneItems { zone_x: 5, zone_y: 6, items: vec![GroundItem([1; 0x148])] });
        u.items_8.push(Items8 { key: [1, 2, 3, 4, 5, 6, 7, 8], items: vec![Item8([2; 16])] });
        u.missions.push(Mission { cell_x: 4096, cell_y: 4097, words: [1, 2, 3, 4, 5], b40: 6, b41: 7, tail: [8, 9, 10, 11] });
        let body = u.body();
        // Section order: block actions first, then hits, particles, sounds, shoots, statics ...
        assert_eq!(&body[..4], &1u32.to_le_bytes());
        assert_eq!(&body[4 + 0x14..8 + 0x14], &1u32.to_le_bytes());
        assert_eq!(ServerUpdate::read_body(&body), Some(u.clone()));
        assert_eq!(ServerUpdate::read_body(&body[..body.len() - 1]), None);
        let empty = ServerUpdate::default().body();
        assert_eq!(empty, vec![0; 13 * 4]);
    }

    #[test]
    fn client_framing_rules() {
        let mut bytes = Vec::new();
        encode_client_packet(&ClientPacket::Version(3), &mut bytes);
        encode_client_packet(&ClientPacket::Ignored(2), &mut bytes);
        encode_client_packet(&ClientPacket::Chat(vec![0x48, 0x69]), &mut bytes);
        encode_client_packet(&ClientPacket::ZoneDiscovered { x: 7, y: -1 }, &mut bytes);
        encode_client_packet(&ClientPacket::Ignored(99), &mut bytes);
        let mut off = 0;
        let mut got = Vec::new();
        while off < bytes.len() {
            match parse_client_packet(&bytes[off..]) {
                Parse::Packet(p, n) => {
                    got.push(p);
                    off += n;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            got,
            vec![ClientPacket::Version(3), ClientPacket::Ignored(2), ClientPacket::Chat(vec![0x48, 0x69]), ClientPacket::ZoneDiscovered { x: 7, y: -1 }, ClientPacket::Ignored(99)]
        );
        assert_eq!(parse_client_packet(&[10, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]), Parse::Packet(ClientPacket::Chat(Vec::new()), 8));
        assert_eq!(parse_client_packet(&[0, 0, 0, 0, 0, 0, 0, 0]), Parse::Fatal);
        assert_eq!(parse_client_packet(&[7, 0, 0, 0, 1, 2]), Parse::NeedMore);
        assert_eq!(parse_client_packet(&[7, 0]), Parse::NeedMore);
    }

    #[test]
    fn entity_update_payload_round_trips() {
        let payload = encode_entity_update_payload(42, &[1, 2, 3]);
        assert_eq!(ClientPacket::decode_entity_update(&payload), Some((42, vec![1, 2, 3])));
        let mut bytes = Vec::new();
        ServerPacket::EntityUpdate { id: 42, delta: vec![1, 2, 3] }.encode(&mut bytes);
        assert_eq!(&bytes[..4], &0u32.to_le_bytes());
        let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 8 + len);
        assert_eq!(ClientPacket::decode_entity_update(&bytes[8..]), Some((42, vec![1, 2, 3])));
    }

    #[test]
    fn join_and_small_server_packets() {
        let j = ServerPacket::Join { id: 5, entity: Box::new(EntityData::ZERO) }.to_bytes();
        assert_eq!(j.len(), JOIN_SIZE);
        assert_eq!(&j[..16], &[0x10, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(ServerPacket::VersionMismatch.to_bytes(), vec![3, 0, 0, 0]);
        assert_eq!(ServerPacket::Seed(26879).to_bytes(), [[0x0f, 0, 0, 0], 26879u32.to_le_bytes()].concat());
        assert_eq!(ServerPacket::CurrentTime { day: 1, time: 2 }.to_bytes(), vec![5, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(ServerPacket::Chat { sender: 1, text: vec![0x41] }.to_bytes(), vec![10, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0x41, 0]);
    }
}
