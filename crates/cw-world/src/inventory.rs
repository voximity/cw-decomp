//! `cube::Inventory` (0x130 bytes at `cube::Spawn+0xf6c` and `cube::Static+0x48`; constructor
//! `Server.exe 0x00406ef0`) and `Inventory::addItem`, `Server.exe 0x00427000`.
//!
//! Pages are vectors of 0x11c-byte slots (`i32` count at `+0`, the 0x118-byte item at `+4`).
//! Coins (type 0xc) and a second currency (type 0xd) are counted instead of stored.

/// An equipment item as embedded in `cube::Spawn` (0x118 bytes; the cuwo `ItemData` layout).
/// Constructor: the per-item part of `FUN_00406ad0` (`Server.exe 0x00406ad0`), [`Item::NEW`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    /// `+0x00` item type, 0.
    pub item_type: u8,
    /// `+0x01` sub type, 0.
    pub sub_type: u8,
    /// `+0x04` modifier (a raw `rand()` for equipment), 0.
    pub modifier: i32,
    /// `+0x08` 0 (cuwo `minus_modifier`).
    pub f8: u32,
    /// `+0x0c` rarity, 0.
    pub rarity: u8,
    /// `+0x0d` material, 0.
    pub material: u8,
    /// `+0x0e` flags, 0.
    pub flags: u8,
    /// `+0x10` level, 1.
    pub level: u16,
    /// `+0x14..+0x114` 32 spirit cubes of 8 bytes, zeroed.
    pub spirits: [u8; 0x100],
    /// `+0x114` spirit count, 0.
    pub num_spirits: u32,
}

impl Item {
    /// Size of the original structure.
    pub const SIZE: usize = 0x118;

    /// Constructor defaults (bytes +2, +3, +0xf, +0x12, +0x13 are not written; 0 here).
    pub const NEW: Item = Item {
        item_type: 0,
        sub_type: 0,
        modifier: 0,
        f8: 0,
        rarity: 0,
        material: 0,
        flags: 0,
        level: 1,
        spirits: [0; 0x100],
        num_spirits: 0,
    };

    /// The original little-endian layout.
    pub fn to_bytes(&self) -> [u8; Item::SIZE] {
        let mut b = [0u8; Item::SIZE];
        b[0] = self.item_type;
        b[1] = self.sub_type;
        b[4..8].copy_from_slice(&self.modifier.to_le_bytes());
        b[8..0xc].copy_from_slice(&self.f8.to_le_bytes());
        b[0xc] = self.rarity;
        b[0xd] = self.material;
        b[0xe] = self.flags;
        b[0x10..0x12].copy_from_slice(&self.level.to_le_bytes());
        b[0x14..0x114].copy_from_slice(&self.spirits);
        b[0x114..0x118].copy_from_slice(&self.num_spirits.to_le_bytes());
        b
    }

    /// The constructor-initialised fields of a raw 0x118-byte item (the bytes the
    /// constructor leaves alone are ignored).
    pub fn from_bytes(b: &[u8]) -> Item {
        let mut it = Item::NEW;
        it.item_type = b[0];
        it.sub_type = b[1];
        it.modifier = i32::from_le_bytes(b[4..8].try_into().unwrap());
        it.f8 = u32::from_le_bytes(b[8..0xc].try_into().unwrap());
        it.rarity = b[0xc];
        it.material = b[0xd];
        it.flags = b[0xe];
        it.level = u16::from_le_bytes(b[0x10..0x12].try_into().unwrap());
        it.spirits.copy_from_slice(&b[0x14..0x114]);
        it.num_spirits = u32::from_le_bytes(b[0x114..0x118].try_into().unwrap());
        it
    }

    /// A merged `*(u16*)item = sub << 8 | type` store.
    pub(crate) fn set_type(&mut self, type_sub: u16) {
        self.item_type = type_sub as u8;
        self.sub_type = (type_sub >> 8) as u8;
    }
}

/// One inventory slot: the stack count and the item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub count: i32,
    pub item: Item,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Inventory {
    /// `+0`: the pages, each a vector of slots.
    pub pages: Vec<Vec<Slot>>,
    /// `+0x128`: gold.
    pub gold: i32,
    /// `+0x12c`: the type-0xd currency.
    pub f12c: i32,
}

impl Inventory {
    pub const NEW: Inventory = Inventory { pages: Vec::new(), gold: 0, f12c: 0 };

    /// `pageForItem(item)`, `Server.exe 0x004282f0`.
    pub fn page_for(item_type: u8) -> i32 {
        match item_type {
            3..=9 | 0x17 | 0x18 => 0,
            0xb => 2,
            0x13 | 0x14 => 3,
            _ => 1,
        }
    }

    /// `Item::isStackable`, `Server.exe 0x004282d0`.
    pub fn stackable(item_type: u8) -> bool {
        matches!(item_type, 1 | 0xa | 0xc | 0xd | 0xb | 0x15)
    }

    /// `Item::operator==`, `Server.exe 0x004078f0`: every field but the level-independent
    /// padding, then the first four bytes of each of the `num_spirits` spirit slots (the
    /// original reads past the item beyond 32; the comparison stops there here).
    pub fn same_item(a: &Item, b: &Item) -> bool {
        a.item_type == b.item_type
            && a.sub_type == b.sub_type
            && a.modifier == b.modifier
            && a.flags == b.flags
            && a.f8 == b.f8
            && a.rarity == b.rarity
            && a.level == b.level
            && a.material == b.material
            && a.num_spirits == b.num_spirits
            && (0..(a.num_spirits as i32).clamp(0, 32) as usize).all(|i| a.spirits[i * 8..i * 8 + 4] == b.spirits[i * 8..i * 8 + 4])
    }

    /// `Inventory::addItem(item, page)`, `Server.exe 0x00427000`; `page == -1` picks the page
    /// from the item type. Checked against the decompilation with `0x004282f0`, `0x004282d0` and
    /// `0x004078f0`/`0x004079c0` (the page resize `0x0041f770`, the 0x118-byte copy `0x00402a70`
    /// and the slot push `0x00428630` are the vector/copy helpers); the original returns nothing
    /// meaningful.
    pub fn add_item(&mut self, item: Item, page: i32) {
        let page = if page == -1 { Self::page_for(item.item_type) } else { page };
        if item.item_type == 0 {
            return;
        }
        if self.pages.len() as i32 <= page {
            self.pages.resize(page as usize + 1, Vec::new());
        }
        let mut copy = item;
        let mut n: i32 = 1;
        let t = item.item_type;
        let stack_by_level = matches!(t, 0xc | 0xd | 0x15)
            || (t == 0xb && item.sub_type != 0xe)
            || matches!(t, 0 | 0x19 | 0x14 | 0x18 | 0x17);
        if stack_by_level {
            n = i32::from(copy.level as i16);
            copy.level = 1;
        }
        if t == 0xc {
            match item.material {
                0xa => {
                    self.gold = self.gold.wrapping_add(n);
                    return;
                }
                0xb => {
                    self.gold = self.gold.wrapping_add(n.wrapping_mul(10000));
                    return;
                }
                0xc => {
                    self.gold = self.gold.wrapping_add(n.wrapping_mul(100));
                    return;
                }
                _ => {}
            }
        }
        if t == 0xd {
            self.f12c = self.f12c.wrapping_add(n);
            return;
        }
        let slots = &mut self.pages[page as usize];
        let mut first_empty: Option<usize> = None;
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.count == 0 && first_empty.is_none() {
                first_empty = Some(i);
            }
            if Self::stackable(t) && Self::same_item(&slot.item, &copy) {
                slot.count = slot.count.wrapping_add(n);
                return;
            }
        }
        if let Some(i) = first_empty {
            slots[i].item = copy;
            slots[i].count = n;
            return;
        }
        slots.push(Slot { count: n, item: copy });
    }

    /// Every item in page order, for callers that only need the contents.
    pub fn items(&self) -> impl Iterator<Item = &Slot> {
        self.pages.iter().flatten()
    }
}
