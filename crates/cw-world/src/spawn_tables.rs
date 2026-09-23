//! Creature type selection used by generateZone's spawn pass: the climate-driven type lists
//! (`Server.exe 0x005290d0`), the per-type level ranges (`0x0040f0a0`), group sizes
//! (`0x0040efc0`) and the group-member variant roll (`0x0052bfa0`).

use crate::world::World;

/// `levelRange(entityType) -> (lo, hi)`, `Server.exe 0x0040f0a0`.
pub fn level_range(entity_type: i32) -> (i32, i32) {
    match entity_type {
        0x11 | 0x31 | 0x67 | 0x71 | 0x72 | 0x9a => (0x2f, 0x51),
        0x13 | 0x14 | 0x19 | 0x1a | 0x1e | 0x1f | 0x20 | 0x3c | 0x3f | 0x43 | 0x45 | 0x46 => (3, 6),
        0x15 | 0x2e | 0x2f | 0x32 | 0x3a | 0x4b | 0x50 | 0x56 | 0x59 | 0x66 | 0x68 | 0x96 => (0x1f, 0x2f),
        0x1c | 0x3d | 0x5a | 0x9b => (0xe, 0x15),
        0x24 | 0x36 | 0x40 | 0x41 | 0x42 | 0x48 | 0x49 | 0x5b | 0x63 | 0x98 | 0x99 => (0x15, 0x1f),
        0x25 | 0x26 | 0x27 | 0x28 | 0x35 | 0x3b | 0x58 | 0x69 | 0x6a | 0x97 => (9, 0xe),
        0x3e | 0x52 | 0x55 | 0x61 | 0x6e | 0x70 => (0xb4, 0x4e0d),
        0x51 | 0x53 | 0x54 | 0x5e => (0x51, 0xb4),
        0x62 | 0x64 => (6, 9),
        _ => (2, 4),
    }
}

/// `groupSizeRange(entityType) -> (lo, hi)`, `Server.exe 0x0040efc0`.
pub fn group_size_range(entity_type: i32) -> (i32, i32) {
    match entity_type {
        0x15 | 0x1c | 0x2a | 0x32 | 0x37 | 0x3f | 0x45 | 0x46 | 0x57 | 0x58 => (1, 3),
        0x16 | 0x23 | 0x24 | 0x36 | 0x38 | 0x3c | 0x47 | 0x48 | 0x49 | 0x62 | 0x63 | 0x64 | 0x66 | 0x67 | 0x68 | 0x69 | 0x9a => (1, 5),
        _ => (1, 1),
    }
}

impl World {
    /// `groupMemberType(x, y, z, leaderType)`, `Server.exe 0x0052bfa0`: paired types roll a
    /// random member of the pair (consuming one `rand()`); others keep the leader's type.
    pub fn group_member_type(&mut self, leader: i32) -> i32 {
        match leader {
            0 | 1 => self.rng.rand() % 2,
            2 | 3 => self.rng.rand() % 2 + 2,
            4 | 5 => self.rng.rand() % 2 + 4,
            7 | 8 => self.rng.rand() % 2 + 7,
            9 | 10 => self.rng.rand() % 2 + 9,
            0xb | 0xc => self.rng.rand() % 2 + 0xb,
            0xd | 0xe => self.rng.rand() % 2 + 0xd,
            0xf | 0x10 => self.rng.rand() % 2 + 0xf,
            0x16 | 0x17 => self.rng.rand() % 2 + 0x16,
            0x25..=0x28 => self.rng.rand() % 4 + 0x25,
            0x53 | 0x54 => self.rng.rand() % 2 + 0x53,
            other => other,
        }
    }

    /// `pickCreatureType(x, y, h, water)`, `Server.exe 0x005290d0`: builds the list of types
    /// allowed by the column's climate and height and picks one with `rand()`; `0x3c` when the
    /// list is empty (which then consumes no `rand()`).
    pub fn pick_creature_type(&mut self, x: i32, y: i32, h: i32, water: bool) -> i32 {
        let b = self.climate_b(x, y);
        let a = self.climate_a(x, y);
        let c = self.climate_c(x, y);
        let mf = self.mountain_factor(x, y);
        let mut list: Vec<i32> = Vec::new();
        if h < 0 {
            list.extend([0x91, 0x92, 0x93, 0x96, 0x98, 0x99, 0x9b, 0x9a]);
        } else if !water {
            if c <= 0.1 {
                list.push(0x15);
                list.push(0x2e);
                if mf > 0.3 {
                    list.push(0x2f);
                }
                if h > 3 {
                    if b < 0.2 || b >= 0.8 {
                        if b < 0.8 || a >= 0.2 {
                            if b < 0.6 || a < 0.6 {
                                if b < 0.2 {
                                    list.extend([0x1c, 0x38, 0x41, 0x5a, 0x5d, 0x15, 0x31, 0x16, 0x17, 0x23]);
                                }
                            } else {
                                list.extend([
                                    0x36, 0x59, 0x4b, 0x4a, 0x47, 0x40, 0x44, 0x3c, 0x3a, 0x43, 0x53, 0x54, 0x19, 0x66, 0x68, 0x69,
                                    0x58, 0x3e, 0x24, 0x32, 0x16, 0x17,
                                ]);
                            }
                        } else {
                            list.extend([0x36, 0x3d, 0x48, 0x2a, 0x49, 0x42, 0x53, 0x54, 0x66, 0x67, 0x58, 0x3e, 0x24, 0x63]);
                        }
                    } else {
                        list.extend([
                            0x36, 0x4a, 0x1c, 0x38, 0x19, 0x35, 0x37, 0x3c, 0x3f, 0x43, 0x45, 0x46, 0x47, 0x57, 0x58, 0x64, 0x5a,
                            0x5b, 0x5c, 0x23, 0x66, 0x68, 0x69, 0x22, 0x21, 0x1e, 0x1f, 0x20, 0x13, 0x14, 0x1a, 0x1b, 0x62, 0x16,
                            0x17, 0x97,
                        ]);
                    }
                } else {
                    list.extend([0x6a, 0x39, 0x56, 0x19]);
                }
            } else {
                list.extend([0x49, 0x52, 0x55, 0x70, 0x67, 0x3e, 0x6e]);
            }
        } else {
            list.extend([2, 4, 7, 9, 0xb, 0xf, 0x33, 0x30, 0x4c]);
        }
        if list.is_empty() {
            0x3c
        } else {
            let r = self.rng.rand();
            list[(r as u32 % list.len() as u32) as usize]
        }
    }
}
