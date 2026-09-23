//! The 0x1168-byte entity data block (`creature+0x10`, cuwo's `EntityData`) and its masked
//! delta encoding: `writeEntityDelta` (`Server.exe 0x00416210`) and `readEntityDelta`
//! (`Server.exe 0x00415dd0`). Packet 0 in both directions carries `i64 id` followed by this
//! delta; the Join packet carries the raw block.
//!
//! The writer visits 48 fields in a fixed order (bit *n* of the u64 mask is field *n*), tests
//! each with a field-specific comparator (or takes it when `full` is set), then appends the
//! mask and the raw bytes of every changed field, in bit order, from `cur`. The reader mirrors
//! it with bounds-checked copies: a field that does not fit is left untouched and the cursor
//! moves to the end. Two pairs of fields are out of memory order (bits 41/42 and 45/46), and
//! the u32 at `0x1ac` plus the structure padding never travel in a delta.

use crate::bytebuf::ByteBuffer;

/// Size of the entity data block.
pub const ENTITY_SIZE: usize = 0x1168;

/// How the writer decides that a field changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// Bitwise equality of the field's bytes (integer fields, `0x00415090` and friends).
    Bytes,
    /// `n` single-precision floats compared with `==` (`0x00414bc0`, `0x00414fd0`, `0x00414c60`):
    /// NaN never equals, `+0.0` equals `-0.0`.
    F32(usize),
    /// `Appearance::operator==`, `Server.exe 0x00415750`.
    Appearance,
    /// `Item::operator==`, `Server.exe 0x004078f0`.
    Item,
    /// The 13 equipment items, `Server.exe 0x004159b0`.
    Equipment,
    /// `strcmp` of the 16-byte name, `Server.exe 0x00415b20`.
    Name,
    /// 11 u32 skills, `Server.exe 0x00415be0`.
    Skills,
}

/// One delta field: `(offset, size, comparator)`, in bit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub offset: usize,
    pub size: usize,
    pub compare: Compare,
}

const fn f(offset: usize, size: usize, compare: Compare) -> Field {
    Field { offset, size, compare }
}

/// The 48 fields in mask-bit order (`analysis/notes/protocol.md`, section 2.3).
pub const FIELDS: [Field; 48] = [
    f(0x000, 24, Compare::Bytes),       // 0 position
    f(0x018, 12, Compare::F32(3)),      // 1 body roll, pitch, yaw
    f(0x024, 12, Compare::F32(3)),      // 2 velocity
    f(0x030, 12, Compare::F32(3)),      // 3 acceleration
    f(0x03c, 12, Compare::F32(3)),      // 4 extra velocity
    f(0x048, 4, Compare::F32(1)),       // 5 look pitch
    f(0x04c, 4, Compare::Bytes),        // 6 physics flags
    f(0x050, 1, Compare::Bytes),        // 7 hostile type
    f(0x054, 4, Compare::Bytes),        // 8 entity type
    f(0x058, 1, Compare::Bytes),        // 9 current mode
    f(0x05c, 4, Compare::Bytes),        // 10 mode start time
    f(0x060, 4, Compare::Bytes),        // 11 hit counter
    f(0x064, 4, Compare::Bytes),        // 12 last hit time
    f(0x068, 0xac, Compare::Appearance), // 13 appearance
    f(0x114, 2, Compare::Bytes),        // 14 flags
    f(0x118, 4, Compare::Bytes),        // 15 roll time
    f(0x11c, 4, Compare::Bytes),        // 16 stun time
    f(0x120, 4, Compare::Bytes),        // 17 slowed time
    f(0x124, 4, Compare::Bytes),        // 18 make-blue time
    f(0x128, 4, Compare::Bytes),        // 19 speed-up time
    f(0x12c, 4, Compare::F32(1)),       // 20 show-patch time
    f(0x130, 1, Compare::Bytes),        // 21 class
    f(0x131, 1, Compare::Bytes),        // 22 specialization
    f(0x134, 4, Compare::F32(1)),       // 23 charged MP
    f(0x138, 12, Compare::F32(3)),      // 24 not used 1..3
    f(0x144, 12, Compare::F32(3)),      // 25 not used 4..6
    f(0x150, 12, Compare::F32(3)),      // 26 ray hit
    f(0x15c, 4, Compare::F32(1)),       // 27 HP
    f(0x160, 4, Compare::F32(1)),       // 28 MP
    f(0x164, 4, Compare::F32(1)),       // 29 block power
    f(0x168, 20, Compare::F32(5)),      // 30 multipliers
    f(0x17c, 1, Compare::Bytes),        // 31 not used 7
    f(0x17d, 1, Compare::Bytes),        // 32 not used 8
    f(0x180, 4, Compare::Bytes),        // 33 level
    f(0x184, 4, Compare::Bytes),        // 34 current XP
    f(0x188, 8, Compare::Bytes),        // 35 parent owner
    f(0x190, 8, Compare::Bytes),        // 36 unknown 1, 2
    f(0x198, 1, Compare::Bytes),        // 37 power base
    f(0x19c, 4, Compare::Bytes),        // 38 unknown 4
    f(0x1a0, 12, Compare::Bytes),       // 39 start chunk
    f(0x1b0, 24, Compare::Bytes),       // 40 spawn position
    f(0x1cc, 12, Compare::Bytes),       // 41 not used 20
    f(0x1c8, 1, Compare::Bytes),        // 42 not used 19
    f(0x1d8, 0x118, Compare::Item),     // 43 consumable
    f(0x2f0, 0xe38, Compare::Equipment), // 44 equipment
    f(0x1158, 16, Compare::Name),       // 45 name
    f(0x1128, 44, Compare::Skills),     // 46 skills
    f(0x1154, 4, Compare::Bytes),       // 47 mana cubes
];

/// Size of one item (`cube::Item`).
pub const ITEM_SIZE: usize = 0x118;

/// The entity data as raw bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct EntityData(pub [u8; ENTITY_SIZE]);

impl EntityData {
    pub const ZERO: EntityData = EntityData([0; ENTITY_SIZE]);

    /// The block of a freshly constructed creature, `EntityData::EntityData`
    /// (`Server.exe 0x00407020`) alone, as a creature the tick makes from a zone spawn starts.
    /// The appearance bytes at `0x68..0x114` are left zero here; the original puts
    /// `Appearance::Appearance` defaults there, which the caller supplies (they live in
    /// `cw-world`).
    pub fn constructed() -> EntityData {
        let mut e = EntityData::ZERO;
        let b = &mut e.0;
        let w32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        // Constructor: hostile type 3, stun time -3000, HP 500, the five multipliers
        // (100, 1, 1, 1, 1), level 1, unknown4 and start chunk -1, not_used_20 (-1, -1, 0),
        // the consumable at level 1, the thirteen equipment items at level 1.
        b[0x50] = 3;
        w32(b, 0x11c, 0xffff_f448);
        w32(b, 0x15c, 0x43fa_0000);
        w32(b, 0x168, 0x42c8_0000);
        for o in [0x16c, 0x170, 0x174, 0x178] {
            w32(b, o, 0x3f80_0000);
        }
        w32(b, 0x180, 1);
        w32(b, 0x19c, 0xffff_ffff);
        w32(b, 0x1a0, 0xffff_ffff);
        w32(b, 0x1a4, 0xffff_ffff);
        w32(b, 0x1cc, 0xffff_ffff);
        w32(b, 0x1d0, 0xffff_ffff);
        b[0x1e8] = 1;
        for slot in 0..13 {
            b[0x2f0 + slot * ITEM_SIZE + 0x10] = 1;
        }
        e
    }

    /// [`EntityData::constructed`] followed by `EntityData::reset` (`0x00411360`, from
    /// `Creature::init`), as `acceptLoop` leaves a player before filling its fields: level 1,
    /// XP 0, the multipliers, stun -3000 and the -1 words again, class 0, and the equipment
    /// zeroed (levels included).
    pub fn new_creature() -> EntityData {
        let mut e = EntityData::constructed();
        let b = &mut e.0;
        let w32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        w32(b, 0x168, 0x42c8_0000);
        w32(b, 0x16c, 0x3f80_0000);
        w32(b, 0x170, 0x3f80_0000);
        for slot in 0..13 {
            b[0x2f0 + slot * ITEM_SIZE + 0x10] = 0;
        }
        e
    }

    pub fn from_slice(bytes: &[u8]) -> Option<EntityData> {
        bytes.try_into().ok().map(EntityData)
    }

    pub fn as_bytes(&self) -> &[u8; ENTITY_SIZE] {
        &self.0
    }
}

impl std::fmt::Debug for EntityData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityData({} bytes)", ENTITY_SIZE)
    }
}

impl Default for EntityData {
    fn default() -> Self {
        Self::ZERO
    }
}

#[inline]
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

#[inline]
fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

/// `n` floats at `o` compared with `==` (`0x00415710` for three).
#[inline]
fn f32s_equal(a: &[u8], b: &[u8], o: usize, n: usize) -> bool {
    (0..n).all(|i| f32_at(a, o + 4 * i) == f32_at(b, o + 4 * i))
}

/// `Appearance::operator==`, `Server.exe 0x00415750`, over the 0xac-byte appearance at `o` in
/// both blocks. Bytes `+5` and `+0xa9..` (padding) are not compared.
pub fn appearance_equal(a: &[u8], b: &[u8], o: usize) -> bool {
    (0..5).all(|i| a[o + i] == b[o + i])
        && u16_at(a, o + 6) == u16_at(b, o + 6)
        && f32s_equal(a, b, o + 8, 3)
        && (0..8).all(|i| u16_at(a, o + 0x14 + 2 * i) == u16_at(b, o + 0x14 + 2 * i))
        && f32s_equal(a, b, o + 0x24, 10)
        && f32s_equal(a, b, o + 0x4c, 3)
        && f32s_equal(a, b, o + 0x58, 3)
        && f32s_equal(a, b, o + 0x64, 3)
        && f32s_equal(a, b, o + 0x70, 3)
        && f32s_equal(a, b, o + 0x7c, 3)
        && f32s_equal(a, b, o + 0x88, 3)
        && f32s_equal(a, b, o + 0xa0, 3)
        && f32s_equal(a, b, o + 0x94, 3)
}

/// `Item::operator==`, `Server.exe 0x004078f0`, over the item at `o`: type, sub-type,
/// modifier, flags, `+8`, rarity, level (u16), material, the spirit count (signed), then the
/// first four bytes of each spirit slot (`+0x14 + 8 i`) for `count` slots. The original reads
/// past the item when `count` exceeds 32; here the comparison stops at the end of the block.
pub fn item_equal(a: &[u8], b: &[u8], o: usize) -> bool {
    if a[o] != b[o]
        || a[o + 1] != b[o + 1]
        || a[o + 4..o + 8] != b[o + 4..o + 8]
        || a[o + 0xe] != b[o + 0xe]
        || a[o + 8..o + 0xc] != b[o + 8..o + 0xc]
        || a[o + 0xc] != b[o + 0xc]
        || a[o + 0x10..o + 0x12] != b[o + 0x10..o + 0x12]
        || a[o + 0xd] != b[o + 0xd]
        || a[o + 0x114..o + 0x118] != b[o + 0x114..o + 0x118]
    {
        return false;
    }
    let count = i32::from_le_bytes(a[o + 0x114..o + 0x118].try_into().unwrap());
    for i in 0..count.max(0) as usize {
        let s = o + 0x14 + 8 * i;
        if s + 4 > a.len() {
            break;
        }
        if a[s + 3] != b[s + 3] || a[s..s + 3] != b[s..s + 3] {
            return false;
        }
    }
    true
}

/// `Server.exe 0x004159b0`: all 13 equipment items equal (any order; no side effects).
pub fn equipment_equal(a: &[u8], b: &[u8], o: usize) -> bool {
    (0..13).all(|k| item_equal(a, b, o + k * ITEM_SIZE))
}

/// The byte `strcmp` of `Server.exe 0x00415b20` over the 16-byte name: equal when the bytes
/// match up to and including the first NUL. Without a NUL the original keeps comparing past the
/// block; here the 16 bytes decide.
pub fn name_equal(a: &[u8], b: &[u8], o: usize) -> bool {
    for i in 0..16 {
        if a[o + i] != b[o + i] {
            return false;
        }
        if a[o + i] == 0 {
            return true;
        }
    }
    true
}

/// Whether `field` differs between `prev` and `cur` according to its comparator.
pub fn field_changed(field: &Field, prev: &[u8], cur: &[u8]) -> bool {
    let (o, n) = (field.offset, field.size);
    let equal = match field.compare {
        Compare::Bytes | Compare::Skills => prev[o..o + n] == cur[o..o + n],
        Compare::F32(k) => f32s_equal(prev, cur, o, k),
        Compare::Appearance => appearance_equal(prev, cur, o),
        Compare::Item => item_equal(prev, cur, o),
        Compare::Equipment => equipment_equal(prev, cur, o),
        Compare::Name => name_equal(prev, cur, o),
    };
    !equal
}

/// `writeEntityDelta(buf, prev, cur, full)`, `Server.exe 0x00416210`: appends the u64 mask and
/// the changed fields of `cur` to `buf`; returns the buffer size afterwards.
pub fn write_delta(buf: &mut ByteBuffer, prev: &EntityData, cur: &EntityData, full: bool) -> usize {
    let mut mask: u64 = 0;
    let mut changed: Vec<&Field> = Vec::new();
    for (bit, field) in FIELDS.iter().enumerate() {
        if full || field_changed(field, &prev.0, &cur.0) {
            mask |= 1u64 << bit;
            changed.push(field);
        }
    }
    buf.write_u64(mask);
    for field in changed {
        buf.write(&cur.0[field.offset..field.offset + field.size]);
    }
    buf.len()
}

/// `readEntityDelta(buf, target)`, `Server.exe 0x00415dd0`: reads the mask (0 when fewer than 8
/// bytes remain, cursor at the end) and copies each flagged field into `target`; a field that
/// does not fit is skipped with the cursor at the end. Returns the buffer size.
pub fn read_delta(buf: &mut ByteBuffer, target: &mut EntityData) -> usize {
    let mask = buf.read_u64().unwrap_or(0);
    for (bit, field) in FIELDS.iter().enumerate() {
        if mask & (1u64 << bit) == 0 {
            continue;
        }
        if let Some(bytes) = buf.read(field.size) {
            target.0[field.offset..field.offset + field.size].copy_from_slice(bytes);
        }
    }
    buf.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_entities_produce_an_empty_mask() {
        let e = EntityData::ZERO;
        let mut b = ByteBuffer::new();
        assert_eq!(write_delta(&mut b, &e, &e, false), 8);
        assert_eq!(b.data, [0; 8]);
        let mut full = ByteBuffer::new();
        // The 48 fields cover 4434 of the 4456 bytes (padding and 0x1ac are never sent).
        assert_eq!(write_delta(&mut full, &e, &e, true), 8 + 4434);
        assert_eq!(&full.data[..8], &0x0000_ffff_ffff_ffffu64.to_le_bytes());
    }

    #[test]
    fn float_compare_semantics() {
        let mut prev = EntityData::ZERO;
        let mut cur = EntityData::ZERO;
        // -0.0 against +0.0 is unchanged.
        cur.0[0x48..0x4c].copy_from_slice(&(-0.0f32).to_le_bytes());
        assert!(!field_changed(&FIELDS[5], &prev.0, &cur.0));
        // NaN always counts as changed, even against itself.
        prev.0[0x48..0x4c].copy_from_slice(&f32::NAN.to_le_bytes());
        cur.0[0x48..0x4c].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(field_changed(&FIELDS[5], &prev.0, &cur.0));
    }

    #[test]
    fn read_applies_only_fields_that_fit() {
        let mut cur = EntityData::ZERO;
        cur.0[0x180..0x184].copy_from_slice(&7i32.to_le_bytes()); // level
        cur.0[0x1154..0x1158].copy_from_slice(&9u32.to_le_bytes()); // mana cubes
        let mut b = ByteBuffer::new();
        write_delta(&mut b, &EntityData::ZERO, &cur, false);
        // Drop the last byte: the mana cubes no longer fit.
        b.data.pop();
        b.pos = 0;
        let mut target = EntityData::ZERO;
        read_delta(&mut b, &mut target);
        assert_eq!(&target.0[0x180..0x184], &7i32.to_le_bytes());
        assert_eq!(&target.0[0x1154..0x1158], &[0; 4]);
        assert_eq!(b.pos, b.len());
    }
}
