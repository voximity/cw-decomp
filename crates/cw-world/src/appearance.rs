//! `cube::Creature::initAppearance`, `Server.exe 0x0040a840`, and the stats/equipment
//! initialisation `FUN_004fb480` that follows it in every generator. See
//! `analysis/notes/functions/0040a840_Creature_initAppearance.md` and `analysis/notes/porting-brief.md`.
//!
//! Float constants the original stores as immediates are written as `h(bits)` so the port keeps
//! their exact bit patterns (several are not the nearest float to their decimal value, e.g.
//! 0.96 as 0x3f75c290 and 12.6 as 0x41499999); decimals are the values Ghidra prints for the
//! `vec3` temporaries, which round-trip to the immediates' bits.

pub use crate::inventory::Item;
use crate::world::World;
use crate::zone::Spawn;

/// An `f32` from its bit pattern.
const fn h(bits: u32) -> f32 {
    f32::from_bits(bits)
}

/// The creature appearance block (0xac bytes at `cube::Spawn+0x74`), the cuwo `AppearanceData`
/// layout. Constructor `FUN_00406970` (`Server.exe 0x00406970`) is [`Appearance::NEW`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    /// `+0x00` u16, 0 from the constructor (cuwo `not_used_1/2`).
    pub f0: u16,
    /// `+0x02..+0x05` hair colour r, g, b; 0xff each from the constructor.
    pub hair_color: [u8; 3],
    /// `+0x05` padding byte; the constructor does not write it (0 here).
    pub b5: u8,
    /// `+0x06` appearance flags, 0 from the constructor; initAppearance only ORs bits in.
    pub flags: u16,
    /// `+0x08` overall scale (x, y, z), 1.0 each.
    pub scale: [f32; 3],
    /// `+0x14` head model, 0xffff.
    pub head_model: u16,
    /// `+0x16` hair model, 0xffff.
    pub hair_model: u16,
    /// `+0x18` hand model, 0xffff.
    pub hand_model: u16,
    /// `+0x1a` foot model, 0xffff.
    pub foot_model: u16,
    /// `+0x1c` body model, 0xffff.
    pub body_model: u16,
    /// `+0x1e` tail model, 0xffff.
    pub tail_model: u16,
    /// `+0x20` shoulder model, 0xffff.
    pub shoulder_model: u16,
    /// `+0x22` wing model, 0xffff.
    pub wing_model: u16,
    /// `+0x24` head scale, 0x3f8147ae (1.01).
    pub head_scale: f32,
    /// `+0x28` body scale, 1.0.
    pub body_scale: f32,
    /// `+0x2c` hand scale, 1.0.
    pub hand_scale: f32,
    /// `+0x30` foot scale, 0x3f7ae148 (0.98).
    pub foot_scale: f32,
    /// `+0x34` shoulder scale, 1.0.
    pub shoulder_scale: f32,
    /// `+0x38` weapon scale, 0x3f733333 (0.95).
    pub weapon_scale: f32,
    /// `+0x3c` back scale, 0x3f4ccccd (0.8).
    pub back_scale: f32,
    /// `+0x40` unknown scale, 1.0.
    pub unknown_scale: f32,
    /// `+0x44` wing scale, 1.0.
    pub wing_scale: f32,
    /// `+0x48` body pitch, 0.
    pub body_pitch: f32,
    /// `+0x4c` arm pitch, 0 (`+0x4c..+0x58` are written as one vec3).
    pub arm_pitch: f32,
    /// `+0x50` arm roll, 0.
    pub arm_roll: f32,
    /// `+0x54` arm yaw, 0.
    pub arm_yaw: f32,
    /// `+0x58` feet pitch, 0.
    pub feet_pitch: f32,
    /// `+0x5c` wing pitch, 0.
    pub wing_pitch: f32,
    /// `+0x60` back pitch, 0.
    pub back_pitch: f32,
    /// `+0x64` body offset, (0, 0, -5).
    pub body_offset: [f32; 3],
    /// `+0x70` head offset, (0, 0.5, 5).
    pub head_offset: [f32; 3],
    /// `+0x7c` hand offset, (6, 0, 0).
    pub hand_offset: [f32; 3],
    /// `+0x88` foot offset, (3, 1, -10.5).
    pub foot_offset: [f32; 3],
    /// `+0x94` back offset, (0, -8, 2).
    pub back_offset: [f32; 3],
    /// `+0xa0` wing offset, (0, 0, 0).
    pub wing_offset: [f32; 3],
}

impl Appearance {
    /// Size of the original structure.
    pub const SIZE: usize = 0xac;

    /// `FUN_00406970`, `Server.exe 0x00406970`.
    pub const NEW: Appearance = Appearance {
        f0: 0,
        hair_color: [0xff; 3],
        b5: 0,
        flags: 0,
        scale: [1.0; 3],
        head_model: 0xffff,
        hair_model: 0xffff,
        hand_model: 0xffff,
        foot_model: 0xffff,
        body_model: 0xffff,
        tail_model: 0xffff,
        shoulder_model: 0xffff,
        wing_model: 0xffff,
        head_scale: h(0x3f8147ae),
        body_scale: 1.0,
        hand_scale: 1.0,
        foot_scale: h(0x3f7ae148),
        shoulder_scale: 1.0,
        weapon_scale: h(0x3f733333),
        back_scale: h(0x3f4ccccd),
        unknown_scale: 1.0,
        wing_scale: 1.0,
        body_pitch: 0.0,
        arm_pitch: 0.0,
        arm_roll: 0.0,
        arm_yaw: 0.0,
        feet_pitch: 0.0,
        wing_pitch: 0.0,
        back_pitch: 0.0,
        body_offset: [0.0, 0.0, -5.0],
        head_offset: [0.0, 0.5, 5.0],
        hand_offset: [6.0, 0.0, 0.0],
        foot_offset: [3.0, 1.0, -10.5],
        back_offset: [0.0, -8.0, 2.0],
        wing_offset: [0.0; 3],
    };

    /// The original little-endian layout.
    pub fn to_bytes(&self) -> [u8; Appearance::SIZE] {
        let mut b = [0u8; Appearance::SIZE];
        let mut put = |off: usize, bytes: &[u8]| b[off..off + bytes.len()].copy_from_slice(bytes);
        put(0x00, &self.f0.to_le_bytes());
        put(0x02, &self.hair_color);
        put(0x05, &[self.b5]);
        put(0x06, &self.flags.to_le_bytes());
        let models = [
            self.head_model,
            self.hair_model,
            self.hand_model,
            self.foot_model,
            self.body_model,
            self.tail_model,
            self.shoulder_model,
            self.wing_model,
        ];
        for (i, m) in models.iter().enumerate() {
            put(0x14 + 2 * i, &m.to_le_bytes());
        }
        let floats = [
            (0x08, &self.scale[..]),
            (0x24, &[self.head_scale][..]),
            (0x28, &[self.body_scale][..]),
            (0x2c, &[self.hand_scale][..]),
            (0x30, &[self.foot_scale][..]),
            (0x34, &[self.shoulder_scale][..]),
            (0x38, &[self.weapon_scale][..]),
            (0x3c, &[self.back_scale][..]),
            (0x40, &[self.unknown_scale][..]),
            (0x44, &[self.wing_scale][..]),
            (0x48, &[self.body_pitch][..]),
            (0x4c, &[self.arm_pitch, self.arm_roll, self.arm_yaw][..]),
            (0x58, &[self.feet_pitch][..]),
            (0x5c, &[self.wing_pitch][..]),
            (0x60, &[self.back_pitch][..]),
            (0x64, &self.body_offset[..]),
            (0x70, &self.head_offset[..]),
            (0x7c, &self.hand_offset[..]),
            (0x88, &self.foot_offset[..]),
            (0x94, &self.back_offset[..]),
            (0xa0, &self.wing_offset[..]),
        ];
        for (off, vals) in floats {
            for (i, v) in vals.iter().enumerate() {
                put(off + 4 * i, &v.to_le_bytes());
            }
        }
        b
    }

    /// The inverse of [`Appearance::to_bytes`] (the first 0xac bytes of `b`).
    pub fn from_bytes(b: &[u8]) -> Appearance {
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let v3 = |o: usize| [f(o), f(o + 4), f(o + 8)];
        Appearance {
            f0: u16_at(0),
            hair_color: [b[2], b[3], b[4]],
            b5: b[5],
            flags: u16_at(6),
            scale: v3(0x08),
            head_model: u16_at(0x14),
            hair_model: u16_at(0x16),
            hand_model: u16_at(0x18),
            foot_model: u16_at(0x1a),
            body_model: u16_at(0x1c),
            tail_model: u16_at(0x1e),
            shoulder_model: u16_at(0x20),
            wing_model: u16_at(0x22),
            head_scale: f(0x24),
            body_scale: f(0x28),
            hand_scale: f(0x2c),
            foot_scale: f(0x30),
            shoulder_scale: f(0x34),
            weapon_scale: f(0x38),
            back_scale: f(0x3c),
            unknown_scale: f(0x40),
            wing_scale: f(0x44),
            body_pitch: f(0x48),
            arm_pitch: f(0x4c),
            arm_roll: f(0x50),
            arm_yaw: f(0x54),
            feet_pitch: f(0x58),
            wing_pitch: f(0x5c),
            back_pitch: f(0x60),
            body_offset: v3(0x64),
            head_offset: v3(0x70),
            hand_offset: v3(0x7c),
            foot_offset: v3(0x88),
            back_offset: v3(0x94),
            wing_offset: v3(0xa0),
        }
    }

    /// The original's merged `*(u32*)(app+0x1a) = body << 16 | foot` store.
    fn set_foot_body(&mut self, foot: u16, body: u16) {
        self.foot_model = foot;
        self.body_model = body;
    }

    /// `scale /= k` component-wise in float (`divss`).
    fn div_scale(&mut self, k: f32) {
        for s in &mut self.scale {
            *s /= k;
        }
    }

    /// `scale *= k` component-wise in float (`mulss`).
    fn mul_scale(&mut self, k: f32) {
        for s in &mut self.scale {
            *s *= k;
        }
    }
}

/// `FUN_0040f8b0` (`Server.exe 0x0040f8b0`): the item is a weapon (type 3) of one of the
/// sub types 5, 6, 7, 8, 0xa, 0xb, 0xf, 0x10, 0x11, 0x12.
pub fn is_weapon_kind(item: &Item) -> bool {
    item.item_type == 3 && matches!(item.sub_type, 5 | 6 | 7 | 8 | 0xa | 0xb | 0xf | 0x10 | 0x11 | 0x12)
}

impl World {
    /// `Creature::initAppearance(&spawn.entity_type, &spawn.appearance, NULL)`, `Server.exe 0x0040a840`.
    ///
    /// The `info == NULL` path the generators use: a random hair colour and two random
    /// variant numbers (`rand() % 100`) stand in for the client's character-creation data, and
    /// the entity type is not rewritten. Fields a case does not write keep their previous value.
    /// The body is [`init_appearance_body`].
    pub fn init_appearance(&mut self, spawn: &mut Spawn) {
        // Prologue (info == NULL): hair b, g, r then variant A, B.
        let r1 = self.rng.rand(); // 0x0040a88b
        let r2 = self.rng.rand(); // 0x0040a8a4
        let r3 = self.rng.rand(); // 0x0040a8b8
        let hair = [r3 as u8, r2 as u8, r1 as u8];
        let v1 = self.rng.rand() % 100; // 0x0040a8dc, local_20 (info[2])
        let v2 = self.rng.rand() % 100; // 0x0040a8e9, local_1c (info[3])

        init_appearance_body(&mut self.rng, &mut spawn.entity_type, &mut spawn.appearance, hair, v1, v2);
    }
}

/// The character-creation record `initAppearance` reads when it is given one (`info`, the
/// first 0x14 bytes of the client's style record `*(creature+0x1d28)`): race, gender, the
/// two variants (face, haircut) and the hair colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppearanceInfo {
    /// `+0`: race 0..7.
    pub race: i32,
    /// `+4`: gender (bit 0).
    pub gender: u8,
    /// `+8`: variant A (the face).
    pub variant_a: i32,
    /// `+0xc`: variant B (the haircut).
    pub variant_b: i32,
    /// `+0x10..+0x12`: hair colour.
    pub hair: [u8; 3],
}

/// `initAppearance(&entityType, &appearance, info)` with a record (`Cube.exe 0x0043f7c0`, the
/// client's copy of `Server.exe 0x0040a840`, called when a character is loaded): the entity
/// type is rewritten from the race and gender (0x0043f82f..: race 0 → 2, 1 → 0, 2 → 9, 3 → 0xb,
/// 4 → 4, 5 → 7, 6 → 0xf, 7 → 0xd, plus `gender & 1`; other races keep the type), then the
/// same switch with the record's hair colour and variants. Cases of non-player types still
/// draw from `rng` as in the generator path.
pub fn init_appearance_with_info(rng: &mut cw_math::rand::MsvcRand, entity_type: &mut i32, appearance: &mut Appearance, info: &AppearanceInfo) {
    let g = i32::from(info.gender & 1);
    let base = match info.race {
        0 => Some(2),
        1 => Some(0),
        2 => Some(9),
        3 => Some(0xb),
        4 => Some(4),
        5 => Some(7),
        6 => Some(0xf),
        7 => Some(0xd),
        _ => None,
    };
    if let Some(b) = base {
        *entity_type = b + g;
    }
    init_appearance_body(rng, entity_type, appearance, info.hair, info.variant_a, info.variant_b);
}

/// The switch of `initAppearance` (`Server.exe 0x0040a840` / `Cube.exe 0x0043f7c0`) after the
/// prologue: `hair` and the variants `v1`, `v2` from the record (or the prologue's draws).
pub fn init_appearance_body(rng: &mut cw_math::rand::MsvcRand, entity_type: &mut i32, appearance: &mut Appearance, hair: [u8; 3], v1: i32, v2: i32) {
    {
        let a = appearance;
        a.scale = [h(0x3f75c290), h(0x3f75c290), h(0x400a3d71)];

        // Case bodies end in one of these tails of the original:
        //   `return`        : nothing more;
        //   `break`         : flags |= 0x20 (0x0040af34);
        //   explicit `flags |= X` before returning.
        match *entity_type {
            0x00 => {
                a.hair_color = hair;
                a.head_model = (v1 % 4 + 0x4d4) as u16;
                a.hair_model = (v2 % 10 + 0x500) as u16;
                a.set_foot_body(0x1b0, 1);
                a.hand_model = 0x1ae;
            }
            0x01 => {
                a.hair_color = hair;
                a.head_model = (v1 % 6 + 0x4d8) as u16;
                a.hair_model = (v2 % 10 + 0x50a) as u16;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x1ae;
            }
            0x02 | 0x03 => {
                a.hair_color = hair;
                let (head, hair_m, limit) = if *entity_type == 2 {
                    (v1 % 6 + 0x4de, v2 % 15 + 0x4e4, 0x4e0)
                } else {
                    (v1 % 6 + 0x4f3, v2 % 7 + 0x4f9, 0x4f5)
                };
                a.head_model = head as u16;
                a.hair_model = hair_m as u16;
                a.set_foot_body(0x1b0, 1);
                // Signed short compare `limit < head`.
                a.hand_model = 0x1ae + u16::from(limit < a.head_model as i16);
            }
            0x04 | 0x05 => {
                a.hair_color = hair;
                let (head, hair_m) = if *entity_type == 4 {
                    (v1 % 5 + 0x4b, v2 % 6 + 0x50)
                } else {
                    (v1 % 5 + 0x56, v2 % 6 + 0x5b)
                };
                a.head_model = head as u16;
                a.hair_model = hair_m as u16;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x61;
                a.weapon_scale = h(0x3f99999a);
                a.div_scale(1.2);
            }
            0x06 => {
                let r1 = rng.rand(); // 0x0040ad3d
                let r2 = rng.rand(); // 0x0040ad56
                let r3 = rng.rand(); // 0x0040ad6a
                a.hair_color = [r3 as u8, r2 as u8, r1 as u8];
                a.head_model = (rng.rand() % 3 + 0x94b) as u16; // 0x0040ad8e
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x1af;
                a.head_offset = [0.0, 3.0, 5.0];
                a.head_scale = h(0x3f666666);
                a.weapon_scale = h(0x3f99999a);
            }
            0x07 | 0x08 => {
                a.hair_color = hair;
                a.head_model = if *entity_type == 7 { v1 % 2 + 0x62 } else { v1 % 5 + 0x6a } as u16;
                a.hair_model = (v2 % 6 + 100) as u16;
                a.set_foot_body(0x71, 0x70);
                a.hand_model = 0x6f;
            }
            0x09 | 0x0a => {
                a.hair_color = hair;
                if *entity_type == 9 {
                    a.head_model = (v1 % 5 + 0x11a) as u16;
                    a.hair_model = (v2 % 3 + 0x11f) as u16;
                    a.set_foot_body(0x1b0, 0x12c);
                } else {
                    a.head_model = (v1 % 5 + 0x122) as u16;
                    a.hair_model = (v2 % 5 + 0x127) as u16;
                    a.set_foot_body(0x1b0, 0x12d);
                }
                a.hand_model = 0x1ae;
                a.weapon_scale = h(0x3f99999a);
                a.head_scale = h(0x3f666666);
                a.div_scale(1.2);
            }
            0x0b => {
                a.hair_color = hair;
                a.head_model = (v1 % 5 + 0x514) as u16;
                a.hair_model = (v2 % 10 + 0x51e) as u16;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x12e;
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f666666);
                a.scale = [h(0x3f851eb8), h(0x3f851eb8), h(0x4015c28f)];
            }
            0x0c => {
                a.hair_color = hair;
                a.head_model = (v1 % 5 + 0x519) as u16;
                a.hair_model = (v2 % 4 + 0x528) as u16;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x12e;
                a.unknown_scale = h(0x3f8ccccd);
                a.head_scale = h(0x3f4ccccd);
                a.body_scale = h(0x3f733333);
                a.head_offset = [0.0, 1.5, 4.0];
                a.scale = [h(0x3f851eb8), h(0x3f851eb8), h(0x4015c28f)];
            }
            0x0d => {
                a.hair_color = [0xff; 3];
                a.head_model = (v1 % 5 + 0x52c) as u16;
                a.hair_model = (v2 % 5 + 0x531) as u16;
                a.set_foot_body(0x1b0, 1);
                a.hand_model = 0x53e;
            }
            0x0e => {
                a.hair_color = [0xff; 3];
                a.head_model = (v1 % 4 + 0x536) as u16;
                a.hair_model = (v2 % 4 + 0x53a) as u16;
                a.set_foot_body(0x1b0, 1);
                a.hand_model = 0x53e;
            }
            0x0f | 0x10 => {
                a.hair_color = hair;
                if *entity_type == 0xf {
                    a.head_model = (v1 % 6 + 0x12f) as u16;
                    a.hair_model = (v2 % 6 + 0x135) as u16;
                } else {
                    a.head_model = (v1 % 6 + 0x13b) as u16;
                    a.hair_model = (v2 % 6 + 0x141) as u16;
                }
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x147;
                a.head_scale = h(0x3f666666);
            }
            0x11 => {
                a.head_model = 0x148;
                a.hair_model = 0x149;
                a.set_foot_body(0x14c, 0x14b);
                a.hand_model = 0x14a;
                a.head_scale = h(0x3f4ccccd);
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (0.0, -90.0, 0.0);
                a.body_offset = [0.0, 0.0, -2.0];
                a.head_offset = [0.0, 4.0, 7.0];
                a.hand_offset = [5.0, 3.0, 0.0];
                a.flags |= 0x428;
            }
            0x12 => {
                a.head_model = 0xb;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x1ae;
            }
            0x13 | 0x14 => {
                if *entity_type == 0x13 {
                    a.head_model = 0x163;
                    a.set_foot_body(0x165, 0x162);
                    a.hand_model = 0x164;
                    a.tail_model = 0x166;
                } else {
                    a.head_model = 0x168;
                    a.set_foot_body(0x16a, 0x167);
                    a.hand_model = 0x169;
                    a.tail_model = 0x16b;
                }
                // 0x0040d4cf
                a.head_scale = h(0x3f666666);
                a.scale = [0.8, 0.8, 1.0];
                a.head_offset = [0.0, 9.0, 6.0];
                a.body_offset = [0.0, 0.0, 2.0];
                a.hand_offset = [3.0, 4.0, -2.5];
                a.foot_offset = [3.0, -4.0, -1.0];
                tail_d582(a, -8.0, -1.0);
            }
            0x15 => {
                a.head_model = 0x16d;
                a.set_foot_body(0x16f, 0x16c);
                a.hand_model = 0x16e;
                a.tail_model = 0x170;
                a.head_scale = h(0x3f333333);
                a.head_offset = [0.0, 12.5, 3.0];
                a.body_offset = [0.0, 0.0, 1.0];
                a.hand_offset = [3.0, 2.5, -4.5];
                a.foot_offset = [3.0, -4.0, -4.0];
                a.foot_scale = h(0x3f4ccccd);
                a.hand_scale = h(0x3f4ccccd);
                a.body_scale = h(0x3f99999a);
                a.back_offset = [0.0, -8.0, -2.5];
                a.back_scale = h(0x3f800000);
                a.back_pitch = h(0xc1f00000);
                a.scale = [2.8, 2.8, 4.0];
                a.flags |= 0x31;
            }
            0x16 | 0x17 => {
                if *entity_type == 0x16 {
                    a.head_model = 0x172;
                    a.set_foot_body(0x174, 0x171);
                    a.hand_model = 0x173;
                } else {
                    a.head_model = 0x176;
                    a.set_foot_body(0x178, 0x175);
                    a.hand_model = 0x177;
                }
                // 0x0040d713
                a.head_scale = h(0x3f4ccccd);
                a.head_offset = [0.0, 9.0, 5.0];
                a.body_offset = [0.0, 0.0, -4.0];
                a.hand_offset = [3.0, 4.0, -8.5];
                a.foot_offset = [3.0, -4.0, -9.0];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f800000);
                a.flags |= 0x31;
            }
            0x18 => {
                a.scale = [0.8, 0.8, 1.2];
                a.body_model = 0x874;
                a.body_offset = [0.0, 0.0, -1.5];
                a.flags |= 0x20;
            }
            0x19 => {
                a.head_model = 0x872;
                a.set_foot_body(0x873, 0x871);
                a.hand_model = 0x873;
                a.head_scale = h(0x3f266666);
                a.head_offset = [0.0, 13.0, 0.5];
                a.body_offset = [0.0, 0.0, 3.5];
                a.hand_offset = [3.0, 4.0, -3.5];
                a.foot_offset = [3.0, -4.0, -3.5];
                a.scale = [0.8, 0.8, 1.0];
                a.foot_scale = h(0x3f4ccccd);
                a.hand_scale = h(0x3f4ccccd);
                // 0x0040d597 with local_c = 4.5
                tail_d59e(a, -8.0, 4.5);
            }
            0x1a => {
                a.head_model = 0x17a;
                a.set_foot_body(0x17c, 0x179);
                a.hand_model = 0x17b;
                a.tail_model = 0x17d;
                a.head_scale = h(0x3f666666);
                a.head_offset = [0.0, 9.0, -1.0];
                a.body_offset = [0.0, 0.0, -5.5];
                a.hand_offset = [3.0, 4.0, -8.5];
                a.foot_offset = [3.0, -4.1, -8.5];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f800000);
                a.body_scale = h(0x3f866666);
                a.back_offset = [0.0, -10.0, 3.0];
                a.back_scale = h(0x3f4ccccd);
                a.back_pitch = h(0xc2f00000);
                a.scale = [1.04, 1.04, 2.34];
                a.flags |= 0x31;
            }
            0x1b => {
                a.head_model = 0x17f;
                a.set_foot_body(0x181, 0x17e);
                a.hand_model = 0x180;
                a.tail_model = 0x182;
                a.head_scale = h(0x3f666666);
                a.head_offset = [0.0, 9.0, 5.0];
                a.body_offset = [0.0, 0.0, 0.5];
                a.hand_offset = [3.0, 4.0, -2.5];
                a.foot_offset = [3.0, -4.1, -2.5];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f800000);
                a.body_scale = h(0x3f866666);
                a.back_offset = [0.0, -7.0, 2.0];
                a.back_scale = h(0x3f4ccccd);
                a.back_pitch = h(0xc2f00000);
                a.scale = [0.8, 0.8, 0.7];
                a.flags |= 0x31;
            }
            0x1c => {
                a.head_model = 0x184;
                a.set_foot_body(0x186, 0x183);
                a.hand_model = 0x185;
                a.tail_model = 0x187;
                a.head_offset = [0.0, 11.0, -1.0];
                a.body_offset = [0.0, 0.0, -3.5];
                a.hand_offset = [3.0, 3.0, -7.5];
                a.foot_offset = [3.0, -4.1, -7.5];
                a.back_offset = [0.0, -5.5, -1.0];
                a.scale = [1.52, 1.52, 3.23];
                a.body_scale = h(0x3f99999a);
                a.head_scale = h(0x3f4ccccd);
                a.foot_scale = h(0x3f666666);
                a.hand_scale = h(0x3f666666);
                a.flags |= 0x31;
            }
            0x1d => {
                a.head_model = 0x189;
                a.set_foot_body(0x18b, 0x188);
                a.hand_model = 0x18a;
                a.tail_model = 0x18c;
                a.head_scale = h(0x3f19999a);
                a.head_offset = [0.0, 14.0, -1.0];
                a.body_offset = [0.0, 0.0, 0.0];
                a.hand_offset = [3.0, 5.0, -8.5];
                a.foot_offset = [3.0, -6.0, -9.0];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f8ccccd);
                tail_d59e(a, -11.0, -4.0);
            }
            0x1e..=0x20 => {
                let (head, foot, body, hand, tail) = match *entity_type {
                    0x1e => (0x18e, 0x190, 0x18d, 0x18f, 0x191),
                    0x1f => (0x193, 0x195, 0x192, 0x194, 0x196),
                    _ => (0x198, 0x19a, 0x197, 0x199, 0x19b),
                };
                a.head_model = head;
                a.set_foot_body(foot, body);
                a.hand_model = hand;
                // 0x0040dcda
                a.tail_model = tail;
                a.head_scale = h(0x3f666666);
                a.head_offset = [0.0, 9.0, -1.0];
                a.body_offset = [0.0, 0.0, -4.0];
                a.hand_offset = [3.0, 4.0, -8.5];
                a.foot_offset = [3.0, -4.1, -8.5];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f800000);
                a.back_offset = [0.0, -8.0, -1.0];
                a.back_scale = h(0x3f8ccccd);
                a.back_pitch = h(0xc1f00000);
                a.flags |= 0x31;
            }
            0x21 | 0x22 => {
                let head_y = if *entity_type == 0x21 {
                    a.head_model = 0x19d;
                    a.set_foot_body(0x19e, 0x19c);
                    a.hand_model = 0x19e;
                    a.scale = [0.8, 0.8, 0.8];
                    9.0
                } else {
                    a.head_model = 0x1a0;
                    a.set_foot_body(0x1a1, 0x19f);
                    a.hand_model = 0x1a1;
                    a.scale = [1.2, 1.2, 1.2];
                    4.0
                };
                // 0x0040de38
                a.head_offset = [0.0, head_y, 1.9];
                a.body_offset = [0.0, 0.0, 1.0];
                a.hand_offset = [3.0, 4.0, -4.2];
                a.foot_offset = [3.0, -4.0, -4.2];
                a.body_scale = h(0x3f99999a);
                a.head_scale = h(0x3f59999a);
                tail_d582(a, -8.0, -1.0);
            }
            0x23 | 0x24 => {
                if *entity_type == 0x23 {
                    a.set_foot_body(0x9b, 0x9a);
                } else {
                    a.set_foot_body(0x9d, 0x9c);
                }
                // 0x0040c70d
                a.hand_offset = [9.0, 0.0, 8.0];
                a.foot_offset = [5.0, 1.0, -2.5];
                a.head_offset = [0.0, 1.0, 8.0];
                a.body_offset = [0.0, 0.0, 2.0];
                a.hand_scale = h(0x3f666666);
                a.scale = [1.0, 1.0, 1.1];
                a.flags |= 0x30;
            }
            0x25..=0x28 => {
                a.body_model = (0x95f + (*entity_type - 0x25)) as u16;
                a.body_offset = [0.0; 3];
                a.body_scale = h(0x3fc00000);
                a.scale = [0.8, 0.8, 0.8];
                a.flags |= 0x29;
            }
            0x29 => {
                a.head_model = 0x963;
                a.hair_model = 0x964;
                a.head_offset = [0.0; 3];
                a.flags |= 2;
                a.head_scale = h(0x3f4ccccd);
                a.scale = [1.4, 1.4, 2.5];
                a.flags |= 0x429;
            }
            0x2a => {
                a.head_model = 0x965;
                a.hand_model = 0x966;
                a.head_offset = [0.0; 3];
                a.hand_offset = [6.0, 0.0, 2.0];
                a.flags |= 2;
                a.head_scale = h(0x3f4ccccd);
                a.hand_scale = h(0x3f000000);
                a.scale = [1.4, 1.4, 2.5];
                a.flags |= 0x28;
            }
            0x2b => {
                a.head_model = 3;
                a.set_foot_body(0x1b0, 4);
                a.hand_model = 0x1ae;
                a.flags |= 0x20;
            }
            0x2c => {
                a.head_model = 10;
                a.hair_model = 0xffff;
                a.set_foot_body(0x1b0, 0);
                a.hand_model = 0x1ae;
            }
            0x2d => {
                a.head_model = 5;
                a.set_foot_body(0x1b0, 6);
                a.hand_model = 0x1ae;
                a.head_offset = [0.0, 3.0, 4.0];
                a.head_scale = h(0x3f4ccccd);
                a.flags |= 0x20;
            }
            0x2e => {
                let r1 = rng.rand(); // 0x0040c7c1
                let r2 = rng.rand(); // 0x0040c7d4
                let r3 = rng.rand(); // 0x0040c7ec
                a.hair_color = [r3 as u8, r2 as u8, r1 as u8];
                a.head_model = 0x3c;
                a.set_foot_body(0x3f, 0x3d);
                a.hand_model = 0x3e;
                a.head_scale = h(0x3f1c28f6);
                a.head_offset = [0.0, 4.0, 6.0];
                a.body_offset = [0.0, 0.0, -2.0];
                a.hand_offset = [7.5, 0.0, 3.0];
                a.scale = [2.4, 2.4, 5.3999996];
                a.flags |= 0x20;
            }
            0x2f => {
                a.head_model = 0x40;
                a.hand_model = 0x41;
                a.foot_model = 0x42;
                a.weapon_scale = h(0x3f266666);
                a.head_scale = h(0x3f4ccccd);
                a.foot_scale = h(0x3f59999a);
                a.hand_scale = h(0x3f59999a);
                a.head_offset = [0.0, 0.0, 2.0];
                a.hand_offset = [7.5, 0.0, 5.0];
                a.body_offset = [0.0, 0.0, -1.0];
                a.foot_offset = [3.0, 1.0, -6.5];
                a.scale = [2.4, 2.4, 3.6000001];
                a.flags |= 0x20;
            }
            0x30 => {
                a.head_model = 0xc;
                a.set_foot_body(0xf, 0xd);
                a.hand_model = 0xe;
                a.div_scale(1.2);
                a.flags |= 0x20;
            }
            0x31 => {
                a.head_model = 0x10;
                a.set_foot_body(0x13, 0x11);
                a.hand_model = 0x12;
                a.head_offset = [0.0, 1.5, 4.0];
                a.head_scale = h(0x3f99999a);
                a.div_scale(1.2);
                a.flags |= 0x20;
            }
            0x32 => {
                a.head_model = 0x14;
                a.set_foot_body(0x17, 0x15);
                a.hand_model = 0x16;
                a.head_offset = [0.0, h(0x40600000), h(0x40000000)];
                a.foot_offset = [h(0x40600000), h(0x3f800000), h(0xc1080000)];
                a.body_offset = [0.0, 0.0, h(0xc0800000)];
                a.head_scale = h(0x3f866666);
                a.body_scale = h(0x3f4ccccd);
                a.scale = [h(0x3f23d70b), h(0x3f23d70b), h(0x3fae147b)];
                a.flags |= 0x28;
            }
            0x33 => {
                a.head_model = 0x94f;
                a.set_foot_body(0x94e, 0xe2);
                a.hand_model = 0x950;
                a.flags |= 0x20;
            }
            0x34 => {
                a.head_model = 0x952;
                a.set_foot_body(0x951, 0x954);
                a.hand_model = 0x953;
                a.hand_offset = [h(0x40c00000), 0.0, h(0x3f800000)];
                a.body_offset = [0.0, 0.0, h(0xc0800000)];
                a.flags |= 0x20;
            }
            0x35 | 0x36 => {
                if *entity_type == 0x35 {
                    a.head_model = 0xe7;
                    a.set_foot_body(0xe8, 0xe6);
                    a.wing_model = 0xe9;
                } else {
                    a.head_model = 0xeb;
                    a.set_foot_body(0xec, 0xea);
                    a.wing_model = 0xee;
                    a.hand_model = 0xed;
                }
                // 0x0040e022
                a.foot_offset = [3.0, -1.0, -7.0];
                a.flags |= 2;
                a.body_pitch = h(0xc2340000);
                a.feet_pitch = h(0x41f00000);
                a.wing_pitch = h(0x42340000);
                a.wing_offset = [1.0, -6.0, 1.0];
                a.flags |= 0x30;
            }
            0x37 | 0x3a => {
                if *entity_type == 0x37 {
                    a.head_model = 0xff;
                    a.set_foot_body(0x100, 0xfe);
                    a.wing_model = 0x101;
                } else if rng.rand() % 2 == 0 {
                    // rand at 0x0040e409
                    // head 0x10f branch
                    a.head_model = 0x10f;
                    a.set_foot_body(0x110, 0x10e);
                    a.wing_model = 0x111;
                } else {
                    a.head_model = 0x10b;
                    a.set_foot_body(0x10c, 0x10a);
                    a.wing_model = 0x10d;
                }
                // 0x0040e287
                a.foot_offset = [3.0, -4.0, -10.0];
                a.head_offset = [0.0, 3.0, 6.0];
                tail_e2c7(a);
            }
            0x38 => {
                a.head_model = 0x103;
                a.set_foot_body(0x104, 0x102);
                a.wing_model = 0x105;
                a.foot_offset = [3.0, 0.0, -10.0];
                a.head_offset = [0.0, 3.0, 2.0];
                a.body_scale = h(0x3fa66666);
                a.flags |= 2;
                a.body_pitch = 0.0;
                a.wing_pitch = h(0x42340000);
                a.wing_offset = [3.0, -2.0, -5.0];
                a.scale = [0.8, 0.8, 1.65];
                a.flags |= 0x30;
            }
            0x39 => {
                a.head_model = 0x107;
                a.set_foot_body(0x108, 0x106);
                a.wing_model = 0x109;
                a.foot_offset = [3.0, -4.0, -10.0];
                a.head_offset = [0.0, 5.0, 8.0];
                tail_e2c7(a);
            }
            0x3b => {
                a.head_model = 0x113;
                a.set_foot_body(0x114, 0x112);
                a.wing_model = 0x115;
                a.foot_offset = [3.0, -4.0, -10.0];
                tail_e2c7(a);
            }
            0x3c..=0x3e => {
                let (wing_y, wing_z) = match *entity_type {
                    0x3d => {
                        a.head_model = 0xf7;
                        a.set_foot_body(0xf8, 0xf6);
                        a.wing_model = 0xf9;
                        a.foot_offset = [3.0, -1.0, -5.0];
                        (-5.0, -1.0)
                    }
                    t => {
                        if t == 0x3c {
                            a.head_model = 0xf0;
                            a.set_foot_body(0xf1, 0xef);
                            a.wing_model = 0xf2;
                        } else {
                            a.head_model = 0xfb;
                            a.set_foot_body(0xfc, 0xfa);
                            a.wing_model = 0xfd;
                        }
                        // 0x0040e0bc
                        a.foot_offset = [3.0, -1.0, -7.0];
                        (-6.0, 1.0)
                    }
                };
                // 0x0040e0f2
                a.flags |= 2;
                a.body_pitch = h(0xc2340000);
                a.feet_pitch = h(0x41f00000);
                a.wing_pitch = h(0x42340000);
                a.wing_offset = [1.0, wing_y, wing_z];
                a.head_offset = [0.0, 3.0, 4.0];
                a.flags |= 0x30;
            }
            0x3f..=0x42 => {
                let (foot, body) = match *entity_type {
                    0x3f => (0x14e, 0x14d),
                    0x40 => (0x150, 0x14f),
                    0x41 => (0x152, 0x151),
                    _ => (0x154, 0x153),
                };
                a.set_foot_body(foot, body);
                // 0x0040e48d
                a.hand_model = 0xffff;
                a.body_offset = [0.0, 3.0, 6.0];
                a.foot_offset = [4.0, 0.0, -1.5];
                a.scale = [1.2, 1.2, 2.6999998];
                a.foot_scale = h(0x3fc00000);
                a.flags |= 0x28;
            }
            0x43 => {
                a.set_foot_body(0x156, 0x155);
                a.head_model = 0x157;
                a.body_offset = [0.0, 0.0, 4.0];
                a.head_offset = [0.0, 4.0, 8.0];
                a.foot_offset = [4.0, 0.0, -6.0];
                a.scale = [1.2, 1.2, 3.75];
                a.body_scale = h(0x3f333333);
                a.head_scale = h(0x3f000000);
                a.foot_scale = h(0x3fc00000);
                a.flags |= 0x28;
            }
            0x44 => {
                a.head_model = 0x73;
                a.set_foot_body(0x74, 0x72);
                a.hand_model = 0x75;
                a.hand_offset = [8.0, 0.0, 2.0];
                a.foot_offset = [5.0, 1.0, -8.5];
                a.head_offset = [0.0, 1.0, 2.0];
                a.body_offset = [0.0, 0.0, -6.0];
                a.hand_scale = h(0x3f99999a);
                a.flags |= 0x20;
            }
            0x45 | 0x46 => {
                if *entity_type == 0x45 {
                    a.head_model = 0x77;
                    a.set_foot_body(0x78, 0x76);
                    a.hand_model = 0x79;
                    a.hand_offset = [6.0, 3.0, 0.0];
                    a.foot_offset = [3.0, 2.0, -10.0];
                    a.head_offset = [0.0, 1.0, 7.0];
                    a.body_offset = [0.0, 0.0, -4.0];
                    a.hand_scale = h(0x3f4ccccd);
                    a.foot_scale = h(0x3f333333);
                } else {
                    a.set_foot_body(0x7b, 0x7a);
                    a.hand_model = 0x7c;
                    a.hand_offset = [6.0, 3.0, 0.0];
                    a.foot_offset = [3.0, 2.0, -8.0];
                    a.head_offset = [0.0, 1.0, 7.0];
                    a.body_offset = [0.0, 0.0, 5.0];
                }
                // 0x0040be16
                a.scale = [1.2, 1.2, 2.6999998];
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (-45.0, 45.0, -45.0);
                a.flags |= 0x28;
            }
            0x47 | 0x48 => {
                if *entity_type == 0x47 {
                    a.set_foot_body(0x7e, 0x7d);
                } else {
                    a.set_foot_body(0x80, 0x7f);
                }
                // 0x0040bef4
                a.foot_offset = [3.0, 0.0, -12.0];
                a.head_offset = [0.0, 0.0, 0.0];
                a.body_offset = [0.0, 0.0, 1.0];
                a.scale = [1.2, 1.2, 2.6999998];
                a.flags |= 0x30;
            }
            0x49 => {
                a.set_foot_body(0x82, 0x81);
                a.foot_offset = [3.0, 0.0, -11.0];
                a.head_offset = [0.0, 0.0, -12.0];
                a.scale = [1.2, 1.2, 2.6999998];
                a.flags |= 0x30;
            }
            0x4a => {
                a.head_model = 0x1a3;
                a.set_foot_body(0x1a4, 0x1a2);
                a.hand_model = 0x1a4;
                a.tail_model = 0x1a5;
                a.scale = [1.0, 1.0, 1.0];
                a.head_offset = [0.0, 8.5, 1.5];
                a.body_offset = [0.0, 0.0, 0.6];
                a.hand_offset = [4.0, 1.0, -3.7];
                a.foot_offset = [4.0, -3.0, -3.7];
                a.body_scale = h(0x3f666666);
                a.head_scale = h(0x3f3ae148);
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3f800000);
                a.back_offset = [0.0, -8.5, -2.9];
                a.back_scale = h(0x3f4ccccd);
                a.back_pitch = h(0xc2a00000);
                a.flags |= 0x31;
            }
            0x4b => {
                a.head_model = 0x1a7;
                a.set_foot_body(0x1a8, 0x1a6);
                a.hand_model = 0x1a8;
                a.scale = [1.0, 1.0, 1.0];
                a.head_offset = [0.0, 17.5, 2.5];
                a.body_offset = [0.0, -9.0, 2.6];
                a.hand_offset = [6.0, 9.0, -1.7];
                a.foot_offset = [6.0, -9.0, -1.7];
                a.body_scale = h(0x40000000);
                a.head_scale = h(0x3f8ccccd);
                a.foot_scale = h(0x3fcccccd);
                a.hand_scale = h(0x3fcccccd);
                a.flags |= 0x31;
            }
            0x4c..=0x4f => {
                let k = (*entity_type - 0x4c) as u16 * 4;
                a.head_model = 0xa0 + k;
                a.set_foot_body(0xa1 + k, 0xa2 + k);
                // 0x0040b9b4
                a.hand_model = 0xa3 + k;
                a.hand_offset = [6.0, 0.0, -2.0];
                a.body_scale = h(0x3f666666);
                a.flags |= 0x20;
            }
            0x50 => {
                a.head_model = 0xb0;
                a.set_foot_body(0xb1, 0xb2);
                a.hand_model = 0xb3;
                a.flags |= 0x20;
            }
            0x51 => {
                a.head_model = 0xbe;
                a.set_foot_body(0xbf, 0xc0);
                a.hand_model = 0xc1;
                a.head_scale = h(0x3f4ccccd);
                a.head_offset = [0.0, 1.8, 6.0];
                a.foot_scale = h(0x3f4ccccd);
                a.foot_offset = [3.5, 1.0, -10.8];
                a.mul_scale(1.5);
                a.flags |= 0x20;
            }
            0x52 => {
                a.head_model = 0xc2;
                a.set_foot_body(0xc3, 0xc4);
                a.hand_model = 0xc5;
                a.shoulder_model = 0xc6;
                a.shoulder_scale = h(0x3f266666);
                a.head_scale = h(0x3f000000);
                a.foot_scale = h(0x3f666666);
                a.hand_scale = h(0x3f333333);
                a.body_scale = h(0x3f666666);
                a.weapon_scale = h(0x3f666666);
                a.hand_offset = [7.0, 0.0, 1.5];
                a.head_offset = [0.0, 3.0, 4.5];
                a.foot_offset = [3.0, 1.0, -7.5];
                a.body_offset = [0.0, 0.0, -2.0];
                a.scale = [3.2, 3.2, 5.8];
                a.flags |= 0x20;
            }
            0x53 | 0x54 => {
                let base = if *entity_type == 0x53 { 0xb4 } else { 0xb7 };
                a.head_model = (rng.rand() % 3 + base) as u16; // 0x0040ba48 / 0x0040baa7
                // 0x0040ba5d
                a.set_foot_body(0xba, 0xbb);
                a.hand_model = 0xbc + u16::from(a.head_model as i16 != base as i16);
                a.head_scale = h(0x3f666666);
                a.head_offset = [0.0, 0.8, 6.0];
                a.flags |= 0x20;
            }
            0x55 => {
                a.head_model = 0xc8;
                a.hair_model = 0xc7;
                a.set_foot_body(0xc9, 0xca);
                a.hand_model = 0xcb;
                a.hand_offset = [8.0, 5.0, 0.0];
                a.body_scale = h(0x3f666666);
                a.arm_pitch = h(0xc2b40000);
                a.arm_roll = h(0x41f00000);
                a.flags |= 0x428;
            }
            0x56 => {
                a.set_foot_body(0x9f, 0x9e);
                a.foot_offset = [5.0, 1.0, -5.5];
                a.body_offset = [0.0, 0.0, 2.0];
                a.scale = [0.8, 0.8, 1.0];
                a.flags |= 0x10;
            }
            0x57 => {
                a.head_model = 0x84;
                a.set_foot_body(0x85, 0x83);
                a.hand_model = 0x86;
                a.hand_offset = [9.0, 0.0, 2.0];
                a.foot_offset = [5.0, 1.0, -8.5];
                a.head_offset = [0.0, 1.0, 2.0];
                a.body_offset = [0.0, 0.0, -6.0];
                a.arm_roll = h(0xc1f00000);
                a.hand_scale = h(0x3f666666);
                a.flags |= 0x28;
                a.div_scale(1.2);
            }
            0x58 | 0x59 => {
                if *entity_type == 0x58 {
                    a.head_model = 0x88;
                    a.set_foot_body(0x89, 0x87);
                    a.hand_model = 0x8a;
                } else {
                    a.head_model = 0x8c;
                    a.set_foot_body(0x8d, 0x8b);
                    a.hand_model = 0x8e;
                }
                a.hand_offset = [9.0, 0.0, 2.0];
                a.foot_offset = [5.0, 1.0, -8.5];
                a.head_offset = [0.0, 1.0, 3.0];
                a.body_offset = [0.0, 0.0, -5.0];
                a.arm_roll = h(0xc1f00000);
                a.hand_scale = h(0x3f666666);
                if *entity_type == 0x59 {
                    a.head_scale = h(0x3f99999a);
                }
                // 0x0040c33c
                a.flags |= 0x28;
                a.mul_scale(1.1);
            }
            0x5a | 0x5b => {
                if *entity_type == 0x5a {
                    a.head_model = 0x90;
                    a.set_foot_body(0x91, 0x8f);
                    a.hand_model = 0x92;
                } else {
                    a.head_model = 0x94;
                    a.set_foot_body(0x95, 0x93);
                    a.hand_model = 0x96;
                }
                a.hand_offset = [7.0, 0.0, 0.0];
                a.foot_offset = [5.0, 1.0, -8.5];
                a.head_offset = [0.0, 1.0, 4.0];
                a.body_offset = [0.0, -5.0, -5.0];
                a.arm_roll = h(0xc1f00000);
                a.hand_scale = h(0x3f400000);
                a.foot_scale = h(0x3f4ccccd);
                a.head_scale = h(0x3f99999a);
                // 0x0040c344
                a.flags |= 0x28;
                a.mul_scale(if *entity_type == 0x5a { 0.7 } else { 0.9 });
            }
            0x5c => {
                a.head_model = 0x97;
                a.wing_model = 0x99;
                a.foot_model = 0x98;
                a.hand_offset = [9.0, 0.0, 2.0];
                a.foot_offset = [5.0, 0.0, -11.5];
                a.head_offset = [0.0, 0.0, -2.0];
                a.hand_scale = h(0x3f666666);
                a.body_pitch = h(0xc1200000);
                a.wing_pitch = h(0x42a00000);
                a.wing_offset = [3.0, -5.0, 0.0];
                a.flags |= 0x2a;
                a.div_scale(1.2);
            }
            0x5d => {
                a.head_model = 0xcd;
                a.set_foot_body(0xce, 0xcc);
                a.hand_model = 0xcf;
                a.hand_offset = [8.0, 0.0, 2.0];
                a.foot_offset = [5.0, 1.0, -8.5];
                a.head_offset = [0.0, 1.0, 2.0];
                a.body_offset = [0.0, 0.0, -6.0];
                a.hand_scale = h(0x3f666666);
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (-60.0, -45.0, 0.0);
                a.flags |= 0x28;
                a.div_scale(1.2);
            }
            0x5e => {
                a.head_model = 0x877;
                a.set_foot_body(0x87a, 0x879);
                a.hand_model = 0x878;
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f4ccccd);
                a.head_offset = [0.0, h(0x40400000), h(0x40400000)];
                a.hand_offset = [h(0x40a00000), h(0x40800000), 0.0];
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (0.0, h(0xc2340000), 0.0);
                a.hand_scale = h(0x3f19999a);
                a.scale = [h(0x4019999a), h(0x4019999a), h(0x40accccc)];
                a.flags |= 0x28;
            }
            0x5f => {
                a.head_model = 0xe1;
                a.set_foot_body(0x1b0, 0xe0);
                a.hand_model = 0x1ae;
                a.head_scale = h(0x3f4ccccd);
                a.flags |= 0x20;
            }
            0x60 => {
                a.head_model = 0xe3;
                a.hair_model = 0xffff;
                a.set_foot_body(0xe4, 0xe2);
                a.hand_model = 0xe5;
                a.hand_offset = [6.0, 3.0, -2.0];
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (0.0, -90.0, 0.0);
                a.flags |= 0x28;
            }
            0x61 => {
                a.head_model = 0x87b;
                a.hair_model = 0x87c;
                a.set_foot_body(0x1b0, 0x5b7);
                a.hand_model = 0x87d;
                a.flags |= 0x402;
                a.wing_model = 0x115;
                a.wing_scale = h(0x40000000);
                a.wing_pitch = h(0x42700000);
            }
            0x62 => {
                a.head_model = 0x159;
                a.set_foot_body(0x15a, 0x158);
                a.hand_model = 0x15a;
                a.tail_model = 0x15b;
                a.head_scale = h(0x3f4ccccd);
                a.scale = [1.4399999, 1.4399999, 1.6];
                a.head_offset = [0.0, 8.0, 6.0];
                a.body_offset = [0.0, 0.0, 3.0];
                a.hand_offset = [3.0, 3.0, -3.25];
                a.foot_offset = [3.0, -5.0, -3.25];
                a.foot_scale = h(0x3f4ccccd);
                a.hand_scale = h(0x3f4ccccd);
                a.back_offset = [0.0, -7.0, -4.0];
                a.back_scale = h(0x3f800000);
                a.body_scale = h(0x3f666666);
                a.flags |= 0x31;
            }
            0x63 | 0x64 => {
                if *entity_type == 0x63 {
                    a.head_model = 0x15d;
                    a.set_foot_body(0x15e, 0x15c);
                    a.hand_model = 0x15e;
                    a.head_offset = [0.0, 12.0, 4.0];
                    a.body_offset = [0.0, 0.0, 1.0];
                    a.hand_offset = [3.0, 3.0, -5.75];
                    a.foot_offset = [3.0, -4.0, -5.75];
                    a.body_scale = h(0x3f733333);
                } else {
                    a.head_model = 0x160;
                    a.set_foot_body(0x161, 0x15f);
                    a.hand_model = 0x161;
                    a.head_offset = [0.0, 10.0, 1.0];
                    a.body_offset = [0.0, 0.0, -2.0];
                    a.hand_offset = [3.0, 3.0, -6.75];
                    a.foot_offset = [3.0, -4.0, -6.75];
                    a.body_scale = h(0x3f99999a);
                }
                a.head_scale = h(0x3f266666);
                a.scale = [1.8, 1.8, 3.0];
                a.foot_scale = h(0x3f400000);
                a.hand_scale = h(0x3f4ccccd);
                a.flags |= 0x31;
            }
            0x65 => {
                a.head_model = 0x1aa;
                a.set_foot_body(0x1ab, 0x1a9);
                a.hand_model = 0x1ab;
                a.tail_model = 0x1ad;
                a.head_scale = h(0x3f028f5c);
                a.head_offset = [0.0, 12.5, 0.5];
                a.body_offset = [0.0, 0.0, -2.0];
                a.hand_offset = [3.0, 4.0, 1.0];
                a.foot_offset = [3.0, -4.0, 1.0];
                a.foot_scale = h(0x3f800000);
                a.hand_scale = h(0x3fc00000);
                a.back_offset = [0.0, -16.0, 4.0];
                a.flags |= 0x33;
            }
            0x66..=0x69 => {
                let (head, foot) = match *entity_type {
                    0x66 => (0xd8, 0xd9),
                    0x67 => (0xda, 0xdb),
                    0x68 => (0xdc, 0xdd),
                    _ => (0xde, 0xdf),
                };
                a.head_model = head;
                a.foot_model = foot;
                a.foot_offset = [3.5, 1.0, -2.5];
                a.head_offset = [0.0, 1.0, 1.0];
                a.head_scale = if *entity_type == 0x66 { h(0x3f400000) } else { h(0x3f666666) };
                a.scale = [1.6, 1.6, 1.6];
                // 0x68 ends at 0x0040cab2 (flags |= 0x29), the others at 0x0040ecea.
                a.flags |= if *entity_type == 0x68 { 0x29 } else { 0x31 };
            }
            0x6a | 0x6b => {
                if *entity_type == 0x6a {
                    a.head_model = 0xd1;
                    a.set_foot_body(0xd2, 0xd0);
                    a.hand_model = 0xd3;
                } else {
                    a.head_model = 0xd5;
                    a.set_foot_body(0xd6, 0xd4);
                    a.hand_model = 0xd7;
                }
                a.hand_offset = [10.0, 0.0, 8.0];
                a.foot_offset = [6.0, 1.0, -5.5];
                a.head_offset = [0.0, 1.0, 5.0];
                a.body_offset = [0.0, 0.0, 1.0];
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (90.0, 30.0, 0.0);
                a.head_scale = h(0x3f400000);
                a.hand_scale = h(0x3f99999a);
                a.scale = if *entity_type == 0x6a { [0.8, 0.8, 1.2] } else { [4.8, 4.8, 7.2000003] };
                a.flags |= 0x28;
            }
            0x6c | 0x72 => {
                if *entity_type == 0x6c {
                    a.head_model = 0x18;
                    a.set_foot_body(0x1c, 0x19);
                    a.hand_model = 0x1b;
                    a.shoulder_model = 0x1a;
                } else {
                    a.head_model = 0x37;
                    a.set_foot_body(0x3b, 0x38);
                    a.hand_model = 0x3a;
                    a.shoulder_model = 0x39;
                }
                // 0x0040b27c
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f19999a);
                a.foot_scale = h(0x3f400000);
                a.shoulder_scale = h(0x3f666666);
                a.hand_offset = [h(0x41000000), h(0x40400000), 0.0];
                a.head_offset = [0.0, h(0x40400000), h(0x40400000)];
                a.scale = [h(0x40b33333), h(0x40b33333), h(0x41499999)];
                a.flags |= 0x20;
            }
            0x6d => {
                a.head_model = 0x1e;
                a.hair_model = 0x1d;
                a.set_foot_body(0x22, 0x1f);
                a.hand_model = 0x21;
                a.shoulder_model = 0x20;
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f19999a);
                a.foot_scale = h(0x3f400000);
                a.shoulder_scale = h(0x3f666666);
                a.hand_offset = [h(0x41000000), h(0x40400000), 0.0];
                a.head_offset = [0.0, h(0x40400000), h(0x40400000)];
                a.scale = [h(0x40b33333), h(0x40b33333), h(0x41499999)];
                a.flags |= 0x420;
            }
            0x6e => {
                a.head_model = 0x23;
                a.set_foot_body(0x27, 0x24);
                a.hand_model = 0x26;
                a.shoulder_model = 0x25;
                a.unknown_scale = h(0x3f8ccccd);
                a.head_scale = h(0x3f19999a);
                a.foot_scale = h(0x3f400000);
                a.shoulder_scale = h(0x3f59999a);
                a.hand_offset = [h(0x41000000), h(0x40400000), 0.0];
                a.head_offset = [0.0, h(0x40400000), h(0x3f800000)];
                a.scale = [h(0x404ccccd), h(0x404ccccd), h(0x40e66666)];
                a.flags |= 0x20;
            }
            0x6f..=0x71 => {
                match *entity_type {
                    0x6f => {
                        a.head_model = 0x28;
                        a.set_foot_body(0x2c, 0x29);
                        a.hand_model = 0x2b;
                        a.shoulder_model = 0x2a;
                        a.unknown_scale = h(0x3f99999a);
                        a.head_scale = h(0x3f19999a);
                    }
                    t => {
                        if t == 0x70 {
                            a.head_model = 0x2d;
                            a.set_foot_body(0x31, 0x2e);
                            a.hand_model = 0x30;
                            a.shoulder_model = 0x2f;
                        } else {
                            a.head_model = 0x32;
                            a.set_foot_body(0x36, 0x33);
                            a.hand_model = 0x35;
                            a.shoulder_model = 0x34;
                        }
                        // 0x0040b3f8
                        a.unknown_scale = h(0x3f99999a);
                        a.head_scale = h(0x3f1c28f6);
                    }
                }
                // 0x0040b39e
                a.foot_scale = h(0x3f4147ae);
                a.shoulder_scale = h(0x3f666666);
                a.hand_offset = [h(0x41000000), h(0x40400000), 0.0];
                a.head_offset = [0.0, h(0x40800000), h(0x40400000)];
                a.scale = [h(0x40b33333), h(0x40b33333), h(0x41499999)];
                a.flags |= 0x20;
            }
            0x73 => {
                a.head_model = 0x43;
                a.set_foot_body(0x47, 0x44);
                a.hand_model = 0x46;
                a.shoulder_model = 0x45;
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f19999a);
                a.foot_scale = h(0x3f400000);
                a.shoulder_scale = h(0x3f666666);
                a.hand_offset = [h(0x41000000), h(0x40400000), 0.0];
                a.head_offset = [0.0, 3.0, 3.0];
                a.scale = [4.8, 4.8, 10.799999];
                a.flags |= 0x20;
            }
            0x74 => {
                let r = rng.rand(); // 0x0040ea26
                let s = (r as f32 * 3.0) / 32767.0 + 7.0;
                a.scale = [s, s, s * 1.12];
                a.head_model = 0x48;
                a.set_foot_body(0x4a, 0x49);
                a.hand_model = 0x4a;
                a.body_scale = h(0x3f333333);
                a.head_scale = h(0x3ecccccd);
                a.head_offset = [0.0, 8.0, 2.0];
                a.body_offset = [0.0, 0.0, 1.0];
                a.back_offset = [0.0, -4.0, -3.0];
                a.foot_scale = h(0x3f000000);
                a.hand_scale = h(0x3f000000);
                a.hand_offset = [3.0, 3.0, -4.1];
                // 0x0040eccd
                a.foot_offset = [2.9, -3.0, -4.1];
                a.flags |= 0x31;
            }
            0x75 => {
                a.head_model = 0x117;
                a.body_model = 0x116;
                a.hand_model = 0x118;
                a.foot_model = 0xffff;
                a.shoulder_model = 0x119;
                a.body_offset = [0.0, 0.0, 3.0];
                a.hand_offset = [6.0, 5.0, 6.0];
                a.head_offset = [0.0, 1.0, 11.0];
                a.head_scale = h(0x3ecccccd);
                a.hand_scale = h(0x3f400000);
                (a.arm_pitch, a.arm_roll, a.arm_yaw) = (0.0, -90.0, 0.0);
                a.scale = [6.4, 6.4, 14.4];
                a.flags |= 0x20;
            }
            0x76 => {
                a.head_model = 0x86a;
                a.body_model = (rng.rand() % 2 + 0x86b) as u16; // 0x0040b631
                a.hand_model = 0x86d;
                a.foot_model = 0x86e;
                a.unknown_scale = h(0x3f99999a);
                a.head_scale = h(0x3f333333);
                a.shoulder_scale = h(0x3f666666);
                a.hand_offset = [8.0, 0.0, 5.0];
                a.head_offset = [0.0, 4.0, 8.0];
                a.foot_scale = h(0x3f4ccccd);
                a.hand_scale = h(0x3fc00000);
                a.foot_offset = [3.0, 1.0, -9.5];
                a.body_offset = [0.0, 0.0, 2.0];
                a.scale = [4.0, 4.0, 10.0];
                a.flags |= 0x2c;
            }
            0x77 => {
                a.scale = [9.0, 9.0, 10.08];
                a.head_model = (rng.rand() % 5 + 0x861) as u16; // 0x0040eb59
                a.body_model = (rng.rand() % 9 + 0x855) as u16; // 0x0040eb6d
                let hand_foot = (rng.rand() % 3 + 0x85e) as u16; // 0x0040eb81
                a.hand_model = hand_foot;
                a.foot_model = hand_foot;
                a.tail_model = (rng.rand() % 2 + 0x866) as u16; // 0x0040eb99
                a.back_scale = h(0x3f333333);
                a.body_scale = h(0x3f333333);
                let r = rng.rand(); // 0x0040ebbe
                a.head_scale = (r as f32 * 0.1) / 32767.0 + 0.4;
                let r = rng.rand(); // 0x0040ebf2
                a.head_offset = [0.0, 8.0, (r as f32 * 0.5) / 32767.0 - 1.0];
                a.body_offset = [0.0, 0.0, 1.0];
                a.back_offset = [0.0, -4.0, -3.0];
                a.foot_scale = h(0x3f000000);
                a.hand_scale = h(0x3f000000);
                let r = rng.rand(); // 0x0040ec77
                a.hand_offset = [r as f32 / 32767.0 + 3.0, 3.0, -4.1];
                let r = rng.rand(); // 0x0040ecaf
                a.foot_offset = [r as f32 / 32767.0 + 3.0, -3.0, -4.1];
                a.flags |= 0x31;
            }
            0x78..=0x7c | 0x7e..=0x80 | 0x8f => {
                // Body-only statics (0x0040e601 / 0x0040e608 / 0x0040e60c).
                let (scale, body, z) = match *entity_type {
                    0x78 => ([2.0, 2.0, 2.0], 0x827, 0.0),
                    0x79 => ([2.0, 2.0, 2.0], 0x828, 0.0),
                    0x7a => ([2.0, 2.0, 2.0], 0x82a, 0.0),
                    0x7b => ([0.5, 0.5, 1.5], 0x86f, 0.0),
                    0x7c => {
                        let r = rng.rand() % 2; // 0x0040e7dc
                        ([2.0, 2.0, 2.0], 0x82e - u16::from(r != 0), 2.0)
                    }
                    0x7e => ([2.0, 2.0, 2.0], 0x82f, 2.0),
                    0x7f => ([1.0, 1.0, 1.0], 0x830, 0.0),
                    0x80 => ([1.5, 1.5, 4.0], 0x906, 0.0),
                    _ => {
                        let r = rng.rand() % 4; // 0x0040e8ac
                        ([1.5, 1.5, 1.25], 0x841 + r as u16, 0.0)
                    }
                };
                a.scale = scale;
                a.body_model = body;
                a.body_offset = [0.0, 0.0, z];
            }
            0x7d => {
                a.scale = [0.5, 0.5, 1.7];
                a.body_model = 0x829;
                a.body_offset = [0.0; 3];
                a.body_scale = h(0x3fc00000);
            }
            0x81 => {
                a.body_model = 0x832;
                a.body_offset = [0.0, 0.0, -4.0];
            }
            0x82 => {
                a.body_model = 0x833;
                a.scale = [3.0, 3.0, 12.0];
                a.body_offset = [0.0, 0.0, -4.0];
            }
            0x83..=0x8b => {
                // 0x0040e918
                a.body_model = (0x834 + (*entity_type - 0x83)) as u16;
                a.scale = [2.0, 2.0, 2.0];
                a.body_offset = [0.0, 0.0, -4.0];
            }
            0x8c => {
                a.body_model = 0x83d;
                a.scale = [0.8, 0.8, 4.0];
                a.body_offset = [0.0; 3];
                a.body_scale = h(0x40000000);
                a.flags |= 0x20;
            }
            0x8d | 0x8e => {
                // 0x0040e9e4
                let (body, z) = if *entity_type == 0x8d { (0x83e, 2.0) } else { (0x83f, 2.5) };
                a.body_model = body;
                a.scale = [1.5, 1.5, z];
                a.body_offset = [0.0; 3];
                a.flags |= 0x20;
            }
            0x90 => {
                a.scale = [0.8, 0.8, 0.8];
                a.body_model = 0x9f2;
                a.body_offset = [0.0, 0.0, 1.0];
                a.body_scale = h(0x3f4ccccd);
            }
            0x91 | 0x92 => {
                // 0x0040cbe1
                a.body_model = if *entity_type == 0x91 { 0x96a } else { 0x96b };
                a.body_offset = [0.0; 3];
                let r = rng.rand(); // 0x0040cc06
                let s = ((r as f32 * 0.5) / 32767.0 + 1.2) * 0.8;
                a.scale = [s, s, s];
                a.body_scale = h(0x3f666666);
                a.flags |= 0x131;
            }
            0x93 => {
                a.body_model = 0x972;
                a.body_offset = [0.0; 3];
                a.scale = [0.8, 0.8, 2.0];
                a.flags |= 0x131;
            }
            0x94 | 0x95 => {
                a.hair_color = hair;
                if *entity_type == 0x94 {
                    a.head_model = (v1 % 3 + 0x53f) as u16;
                    a.hair_model = (v2 % 3 + 0x542) as u16;
                    a.body_model = 0x545;
                } else {
                    a.head_model = (v1 % 3 + 0x547) as u16;
                    a.hair_model = (v2 % 3 + 0x54a) as u16;
                    a.body_model = 0x54d;
                }
                a.hand_model = 0x546;
            }
            0x96 | 0x99..=0x9b => {
                // 0x0040cc7e
                a.body_model = match *entity_type {
                    0x96 => 0x96c,
                    0x99 => 0x96f,
                    0x9a => 0x970,
                    _ => 0x971,
                };
                a.body_scale = h(0x3f99999a);
                a.body_offset = [0.0; 3];
                a.scale = [2.4, 2.4, 6.0];
                a.flags |= 0x131;
            }
            0x97 => {
                a.head_model = 0xf3;
                a.wing_model = 0xf4;
                a.foot_model = 0xf5;
                a.flags |= 2;
                a.foot_offset = [3.0, 4.0, -8.0];
                a.body_pitch = h(0xc2340000);
                a.wing_pitch = h(0x42340000);
                a.head_scale = h(0x3f99999a);
                a.wing_scale = h(0x3f19999a);
                a.foot_scale = h(0x3f800000);
                a.scale = [1.0, 1.0, 1.875];
                a.wing_offset = [6.5, 3.0, -1.0];
                a.head_offset = [0.0, 3.0, 0.0];
                a.flags |= 0x30;
            }
            0x98 => {
                a.head_model = 0x96d;
                a.hair_model = 0x96e;
                a.head_scale = h(0x3f99999a);
                a.body_offset = [0.0; 3];
                a.head_offset = [0.0; 3];
                a.scale = [2.4, 2.4, 6.0];
                a.flags |= 0x531;
            }
            _ => {}
        }
    }
}

impl World {

    /// `FUN_004fb480(spawn, 0)`, `Server.exe 0x004fb480`: the second initialisation the
    /// generators run on each spawn. Picks class, specialization and starting equipment for
    /// humanoid types, sets the per-type combat floats (`+0xf58..+0xf68`) and skill lists, and
    /// finally draws the modifiers of six equipment slots.
    ///
    /// Equipment slots are `spawn.equipment[k]` at `+0x120 + k * 0x118`: 1 neck (+0x238), 2 chest
    /// (+0x350), 3 feet (+0x468), 4 hands (+0x580), 5 shoulder (+0x698), 6 left weapon (+0x7b0),
    /// 7 right weapon (+0x8c8), 8 and 9 rings (+0x9e0, +0xaf8).
    pub fn init_creature(&mut self, spawn: &mut Spawn) {
        self.init_creature_flag(spawn, false);
    }

    /// `FUN_004fb480(spawn, flag)`: with `flag` set (the tick's type-5 mission reroll,
    /// 0x00535e0f) the weapon levels are set and the function jumps from 0x004fbb85 straight
    /// to the six modifier draws (0x004fbef6), skipping the per-type combat floats and skill
    /// lists and the 0x200 boost.
    pub fn init_creature_flag(&mut self, spawn: &mut Spawn, flag: bool) {
        let flags = spawn.appearance.flags;
        if flags & 0x40 != 0 {
            return;
        }
        let class = spawn.f30 as u8;
        if (0x80..=0x89).contains(&class) {
            // Class byte 0x80..=0x89 (not produced by the generators): chest armour only, 0x004fbf3e.
            let chest = &mut spawn.equipment[2];
            chest.set_type(4);
            chest.material = 6;
            if class == 0x88 {
                chest.sub_type = (self.rng.rand() % 5 + 2) as u8; // 0x004fbf4e
            }
            // 0x004fbf77
            spawn.f_f60 = h(0x3dcccccd);
            spawn.f_f5c = h(0x3e99999a);
            spawn.f_f64 = h(0x41200000);
            spawn.f_f68 = h(0x41200000);
            return;
        }

        let level = spawn.level as u16;
        let eq = &mut spawn.equipment;
        eq[6].set_type(0);
        eq[7].item_type = 0;
        eq[7].sub_type = 0;
        eq[5].set_type(0);
        eq[2].set_type(0);
        eq[3].set_type(0);
        eq[4].set_type(0);
        eq[1].set_type(0);

        let et = spawn.entity_type;
        if flags & 0x18 != 0 || (spawn.f28 != 6 && et == 0x76) {
            // 0x004fbb5c
            eq[6].set_type(0);
            eq[7].item_type = 0;
            eq[7].sub_type = 0;
        } else if spawn.f28 == 6 {
            // straight to 0x004fbb6f
        } else if et == 0x75 {
            spawn.v10ac.push(0x5c);
            spawn.f10b8 = 0;
            spawn.v10bc.push(0x11);
            spawn.v10bc.push(0x60);
        } else if et == 0x6c || et == 0x72 || et == 0x2e {
            if et != 0x2e {
                // 0x004fbb1e
                spawn.v10ac.push(0x5b);
                spawn.v10ac.push(0x5d);
            }
            // 0x004fbb50
            eq[7].item_type = 3;
            eq[7].sub_type = 0x11;
            // 0x004fbb02
            eq[7].material = 2;
            eq[6].set_type(0);
        } else if et == 0x73 {
            spawn.v10ac.push(0x56);
            spawn.v10ac.push(0x5d);
            eq[7].sub_type = 4;
            eq[7].item_type = 3;
            eq[7].material = 1;
            eq[6].set_type(0x403);
            eq[6].material = 1;
        } else if et == 0x6d {
            spawn.v10ac.push(0x56);
            spawn.v10ac.push(0x5d);
            spawn.v10ac.push(0x5b);
            eq[7].sub_type = 2;
            eq[7].item_type = 3;
            eq[7].material = 7;
            eq[6].set_type(0x203);
            eq[6].material = 7;
        } else if et == 0x51 {
            spawn.v10ac.push(0x57);
            eq[7].item_type = 3;
            eq[7].sub_type = 0xc;
            eq[7].material = 0xb;
            eq[6].set_type(0xc03);
            eq[6].material = 0xb;
        } else if et == 0x52 {
            eq[7].item_type = 3;
            eq[7].material = 1;
            eq[6].set_type(3);
            eq[6].material = 1;
            // 0x004fbb68
            eq[7].sub_type = 0;
        } else if matches!(et, 0x2f | 0x6f | 0x70 | 0x71) {
            // nothing
        } else if et == 0x2d || et == 0x2b {
            spawn.v10ac.push(0x5f);
            spawn.f30 = (spawn.f30 & 0x00ff) | (2 << 8);
            eq[7].item_type = 3;
            eq[7].sub_type = (self.rng.rand() % 2 + 10) as u8; // 0x004fbaec
            // 0x004fbb02
            eq[7].material = 2;
            eq[6].set_type(0);
        } else {
            let class = if et == 0x61 {
                self.rng.rand(); // 0x004fb758, result overwritten by 4
                4u8
            } else {
                (self.rng.rand() % 4 + 1) as u8 // 0x004fb758
            };
            let spec = match class {
                1 => {
                    let spec = self.rng.rand() % 2; // 0x004fb78a
                    match self.rng.rand() % 3 {
                        // rand at 0x004fb79b
                        0 => {
                            eq[7].item_type = 3;
                            let r = self.rng.rand(); // 0x004fb80b
                            eq[7].material = 1;
                            eq[6].set_type(0);
                            eq[7].sub_type = (r % 3 + 0xf) as u8;
                        }
                        1 => {
                            eq[7].item_type = 3;
                            let r = self.rng.rand(); // 0x004fb7dd
                            eq[7].material = 1;
                            eq[6].item_type = 3;
                            eq[7].sub_type = (r % 3) as u8;
                            eq[6].sub_type = (self.rng.rand() % 3) as u8; // 0x004fb7fa
                            eq[6].material = 1;
                        }
                        _ => {
                            eq[7].item_type = 3;
                            let r = self.rng.rand(); // 0x004fb7b6
                            eq[7].material = 1;
                            eq[6].set_type(0xd03);
                            eq[7].sub_type = (r % 3) as u8;
                            eq[6].material = 1;
                        }
                    }
                    spec
                }
                2 => {
                    let spec = self.rng.rand() % 2; // 0x004fb833
                    eq[7].item_type = 3;
                    let r = self.rng.rand(); // 0x004fb847
                    eq[7].material = 2;
                    eq[7].sub_type = (r % 3 + 6) as u8;
                    spec
                }
                3 => {
                    let spec = self.rng.rand() % 2; // 0x004fb866
                    match self.rng.rand() % 3 {
                        // rand at 0x004fb877
                        t @ (0 | 1) => {
                            eq[7].sub_type = if t == 0 { 10 } else { 0xb };
                            // 0x004fb8c2
                            eq[7].item_type = 3;
                            eq[7].material = 2;
                            eq[6].set_type(0);
                        }
                        _ => {
                            eq[7].item_type = 3;
                            eq[7].sub_type = 0xc;
                            eq[7].material = 0xb;
                            eq[6].set_type(0xc03);
                            eq[6].material = 0xb;
                        }
                    }
                    spec
                }
                _ => {
                    let spec = self.rng.rand() % 2; // 0x004fb8d6
                    eq[7].item_type = 3;
                    let r = self.rng.rand(); // 0x004fb8ea
                    eq[7].material = 1;
                    let sub = (r % 3 + 3) as u8;
                    eq[7].sub_type = sub;
                    if sub == 5 {
                        eq[6].set_type(0);
                        eq[6].material = 0;
                    } else {
                        eq[6].item_type = 3;
                        eq[6].sub_type = sub;
                        eq[6].material = 1;
                    }
                    spec
                }
            };
            spawn.f30 = u16::from(class) | ((spec as u16) << 8);

            if flags & 0x20 == 0 {
                let material = match eq[7].sub_type {
                    10..=12 => 0x19,
                    6..=8 => 0x1a,
                    3..=5 => 0x1b,
                    _ => 1,
                };
                eq[2].item_type = 4;
                eq[2].level = level;
                eq[2].material = material;
                if self.rng.rand() % 2 != 0 {
                    // rand at 0x004fb997
                    eq[5].level = level;
                    eq[5].item_type = 7;
                    eq[5].material = material;
                }
                if self.rng.rand() % 2 != 0 {
                    // rand at 0x004fb9c2
                    eq[4].level = level;
                    eq[4].item_type = 5;
                    eq[4].material = material;
                }
                if self.rng.rand() % 2 != 0 {
                    // rand at 0x004fb9ed
                    eq[3].level = level;
                    eq[3].item_type = 6;
                    eq[3].material = material;
                }
                for slot in [1, 9, 8] {
                    // rand at 0x004fba18 (material 0x004fba38), 0x004fba52 (0x004fba72), 0x004fba8c (0x004fbab0)
                    if self.rng.rand() % 10 == 0 {
                        eq[slot].item_type = if slot == 1 { 8 } else { 9 };
                        eq[slot].level = level;
                        eq[slot].material = 12 - u8::from(self.rng.rand() % 2 != 0);
                    }
                }
            }
        }

        // 0x004fbb6f
        eq[7].level = level;
        eq[6].level = level;

        // 0x004fbb85: the flag-0 part.
        if !flag {
            let mut push_10ac = None;
            match et {
                0x11 | 0x5e | 0x61 => {
                    spawn.f_f58 = h(0x43c80000);
                    spawn.f_f60 = h(0x3fc00000);
                    spawn.f_f64 = h(0x40400000);
                    spawn.f_f68 = h(0x40400000);
                    push_10ac = Some(0x5d);
                }
                0x15 => {
                    spawn.f_f58 = h(0x437a0000);
                    spawn.f_f60 = h(0x40000000);
                    spawn.v10ac.push(0x45);
                    spawn.f_f68 = h(0x40400000);
                    spawn.f_f64 = h(0x40400000);
                }
                0x19 => {
                    spawn.f_f58 = h(0x43480000);
                    spawn.f_f60 = h(0x3f000000);
                    spawn.f_f68 = h(0x40400000);
                    spawn.f_f64 = h(0x40400000);
                }
                0x2e | 0x52 | 0x51 => {
                    // 0x004fbc98
                    spawn.f_f58 = h(0x437a0000);
                    spawn.f_f64 = h(0x40400000);
                    spawn.f_f68 = h(0x40400000);
                    push_10ac = Some(if et == 0x51 { 0x5c } else { 0x56 });
                }
                0x2f | 0x58 => {
                    spawn.f_f58 = h(0x437a0000);
                    spawn.f_f60 = h(0x40400000);
                    spawn.f_f5c *= 0.5;
                    spawn.f_f68 = h(0x40400000);
                    spawn.f_f64 = h(0x40400000);
                }
                0x55 => {
                    spawn.f_f58 = h(0x447a0000);
                    spawn.f_f60 = h(0x40000000);
                    spawn.f_f68 = h(0x40400000);
                    spawn.f_f64 = h(0x40400000);
                }
                0x56 => spawn.f30 = 0x103,
                0x6b..=0x6d | 0x6f..=0x74 | 0x76 => {
                    spawn.f_f58 = h(0x451c4000);
                    spawn.f_f60 = h(0x40c00000);
                    spawn.f_f64 = h(0x40a00000);
                    spawn.f_f68 = h(0x40a00000);
                    spawn.f_f5c = if is_weapon_kind(&spawn.equipment[7]) { h(0x3f400000) } else { h(0x3f000000) };
                }
                0x75 => {
                    spawn.f_f58 = h(0x451c4000);
                    spawn.f_f60 = h(0x40a00000);
                    spawn.f_f64 = h(0x40800000);
                    spawn.f_f68 = h(0x40a00000);
                }
                0x77 => {
                    spawn.f_f5c = h(0x3f000000);
                    spawn.f_f58 = h(0x451c4000);
                    spawn.f_f60 = h(0x40c00000);
                    spawn.f_f64 = h(0x41200000);
                    spawn.f_f68 = h(0x41200000);
                }
                _ => {}
            }
            if let Some(skill) = push_10ac {
                spawn.v10ac.push(skill);
            }

            if flags & 0x200 != 0 {
                let f58 = spawn.f_f58 * 15.0;
                spawn.f_f58 = f58;
                if f58 > 10000.0 {
                    spawn.f_f58 = h(0x461c4000);
                }
                spawn.f_f60 *= 3.0;
                spawn.f_f5c *= 0.75;
                spawn.f_f64 += 2.0;
                spawn.f_f68 += 2.0;
                const CHOICES: [i32; 5] = [0x5c, 0x5d, 0x56, 0x59, 0x57];
                let r = self.rng.rand(); // 0x004fbe92
                spawn.v10ac.push(CHOICES[(r as u32 % 5) as usize]);
                spawn.f10b8 = self.rng.rand() % 3; // 0x004fbeb2
                spawn.v10bc.push(0x11);
                spawn.v10bc.push(0x60);
            }
        }

        // 0x004fbef6: rand at 0x004fbef6, efe, f06, f0e, f16, f1e
        let eq = &mut spawn.equipment;
        eq[6].modifier = self.rng.rand();
        eq[7].modifier = self.rng.rand();
        eq[5].modifier = self.rng.rand();
        eq[2].modifier = self.rng.rand();
        eq[4].modifier = self.rng.rand();
        eq[3].modifier = self.rng.rand();
    }
}

/// Tail at 0x0040d582: foot and hand scale 1, then the back part of 0x0040d59e.
fn tail_d582(a: &mut Appearance, back_y: f32, back_z: f32) {
    a.foot_scale = h(0x3f800000);
    a.hand_scale = h(0x3f800000);
    tail_d59e(a, back_y, back_z);
}

/// Tail at 0x0040d59e: back offset (0, y, z), back scale 1, back pitch -30, flags |= 0x31.
fn tail_d59e(a: &mut Appearance, back_y: f32, back_z: f32) {
    a.back_offset = [0.0, back_y, back_z];
    a.back_scale = h(0x3f800000);
    a.back_pitch = h(0xc1f00000);
    a.flags |= 0x31;
}

/// Tail at 0x0040e2c7 (after the pvVar8 store): wings spread, body pitch -45.
fn tail_e2c7(a: &mut Appearance) {
    a.flags |= 2;
    a.body_pitch = h(0xc2340000);
    a.wing_offset = [3.0, -5.0, 0.0];
    a.wing_pitch = h(0x42340000);
    a.flags |= 0x30;
}
