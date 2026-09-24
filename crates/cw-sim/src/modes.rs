//! The skill and animation mode machine of `World::tick` (`Server.exe 0x00537bc1..0x0053e1fc`):
//! the two-level dispatch on the creature's mode (`entity+0x58`) through the byte table
//! `0x00548a8c` and the jump table `0x00548a44`, one function per case, and the active-buff
//! effects of the buff loop (the type-1 stun clear and the type-4 poison tick).
//!
//! In `update_creature` (update.rs) the machine runs after the timers block (slow, make-blue,
//! speed-up, show-patch) and before the combo timer and the cooldowns (0x00537d01), at the
//! point marked "jump table 0x548a44". Every case ends at one of the joins 0x00537cef /
//! 0x00537cf5 / 0x00537cfb / 0x00537d01 / 0x0053e0c5 / 0x0053b968, which differ only in the
//! registers they reload: here each is a `return`.
//!
//! Client and server: `world+0xb4` ([`World::is_client`]) is 0 and `world+0xb8`
//! ([`World::local_player`]) NULL on a server, so there the standard guard `(b4 == 0 &&
//! hostile != 0) || c == b8` ([`Cx::auth`]) is `hostile != 0`, case 15 (mode 0x69, the local
//! player's home teleport) does nothing and `computeLighting` after a terrain destruction runs.
//! In the client's world the guard admits only the local player (case 0's own guard,
//! [`Cx::generic_guard`], admits every creature there), the terrain destruction, the
//! applied heals and life steal of other creatures, the poison ticks and cases 10 and 16 are
//! the server's, and case 15 runs for the local player (`client::home_teleport`).
//!
//! Offsets: the transcriptions use creature offsets (`c+X`); the entity block is `c+0x10`, so
//! `c+X` is `entity+(X-0x10)`. Fields above `c+0x1178` live in [`CreatureState`] (combat.rs) or
//! in [`ModeState`] here.
//!
//! Sources: `pseudo_537bc1.md`, `pseudo_539c02.md`, `pseudo_53bc86.md` (transcriptions of the
//! disassembly) plus the helpers decompiled for this port (cited where used).

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums its operand order.
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    clippy::manual_clamp,
    clippy::assign_op_pattern,
    clippy::excessive_precision,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::float_cmp
)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::packet::{BlockAction, Hit, Particle, Passive, Shoot, Sound};
use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_world::appearance::Appearance;
use cw_world::zone::Spawn;
use cw_world::World;

use crate::behavior::Behavior;
use crate::behaviors_more::{AiNode, CompanionBehavior};
use crate::combat_ai::CombatState;
use crate::combat::{Buff, CreatureState, add_buff, apply_hit};
use crate::skills::{attack_speed, skill_duration, skill_level_factor, skill_total_time, skill_windup};
use crate::stats::{item_power, max_hp, pow};
use crate::util::{K, WEAPON, add3, block_fix, block_type, chance_roll, diff_blocks, f32_at, fix, fix3, floor_div_fix, has_buff, i32_at, i64_at, is_enemy, len_sq3, level_pow, mana_cost, normalize3, pos_at, pos_blocks, rotate_z, set_pos, set_vec3f, strike_items, u16_at, vec3f_at, w16, w32, w64, wf32, wi32};



// ---------------------------------------------------------------------------------------------
// Dispatch tables.

/// The jump table at `0x00548a44` (18 targets), read from the binary. Index 17 is the default
/// (0x0053bc80: `esi = c; goto 0x00537d01`).
pub const MODE_JUMP_TABLE: [u32; 18] = [
    0x538117, 0x537fb4, 0x53ce3a, 0x53b973, 0x53ba28, 0x53a9f2, 0x53d7e1, 0x537f28, 0x53b24f, 0x53d3f4, 0x53caf5, 0x53afb9, 0x53b568, 0x53ad3c, 0x53ae68, 0x537be2, 0x53cba1, 0x53bc80,
];

/// The byte table at `0x00548a8c` (0x6e entries), indexed by `mode - 1`, read from the binary.
pub const MODE_INDEX_TABLE: [u8; 110] = [
    0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 2, 2, 3, 3, 2, 3, 4, 17, 0, 0, 0, //
    0, 5, 17, 3, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, //
    8, 9, 0, 17, 17, 0, 9, 17, 0, 0, 3, 0, 0, 0, 3, 3, //
    0, 0, 0, 0, 0, 0, 17, 0, 10, 0, 0, 0, 0, 0, 17, 17, //
    17, 17, 17, 17, 17, 0, 0, 0, 11, 11, 0, 12, 0, 4, 4, 9, //
    17, 17, 17, 17, 13, 17, 14, 0, 15, 17, 17, 6, 17, 16,
];

// ---------------------------------------------------------------------------------------------
// State kept on the creature outside the entity block.

/// Creature fields the mode machine reads or writes that live outside the entity block and are
/// not in [`CreatureState`] yet. One per creature.
#[derive(Debug, Clone, Default)]
pub struct ModeState {
    /// `creature+0x11ac`: `std::set<i64>` of the ids hit in the current swing / beam.
    pub hit_set: BTreeSet<i64>,
    /// `creature+0x11b4`: `std::set<i64>` of the ids that dodged (were rolling) in the current
    /// swing, notified with a type-4 Hit when the window ends.
    pub pending_set: BTreeSet<i64>,
    /// `creature+0x11c0` (i64): the id of the creature this one rides (the client's
    /// `applyMovementInput` moves the mount instead of the rider); the beam and the
    /// projectiles never hit it. Cleared by the tick when the mount dies (riding.rs).
    pub mount: i64,
    /// `creature+0x1314` (i32): a copy of the hit counter (`entity+0x60`) made when a
    /// multi-hit period starts and before the window.
    pub hit_count_copy: i32,
    /// `creature+0x1320` (vec3i64): the "attacker position" the poison tick passes to
    /// `creatureAttack` (meaning unknown; nothing in this range writes it).
    pub poison_origin: [i64; 3],
    /// `creature+0x13b8` (u8): something landed in the current swing (blocks the end-of-swing
    /// attack-speed rescale).
    pub landed: u8,
    /// `creature+0x13c0` (u8): the swing consumed a type-0xb buff (forces the crit flag).
    pub free_cast: u8,
    /// `creature+0x13f4` (i32): how mode 0x5c dresses the pet (1: own look, 2: the look of
    /// `creature+0x11d0`, else a fresh appearance of a type from `summon_types`).
    pub summon_look: i32,
    /// `creature+0x13f8` (`std::vector<i32>`): the entity types mode 0x5c picks from (0x25 when
    /// empty).
    pub summon_types: Vec<i32>,
    /// Not in the original: pets made by mode 0x5c this tick as `(pet id, owner id)`. The pet's
    /// entity and `CreatureState` (with its behaviour tree) are made here; the caller gives it a
    /// default `ModeState`.
    pub summoned: Vec<(i64, i64)>,
    /// Not in the original: pets tamed by mode 0x6e this tick as `(pet id, owner id)`.
    /// Informational (for logging or the caller's bookkeeping).
    pub tamed: Vec<(i64, i64)>,
}

/// A live projectile (the 0x70-byte Shoot record the tick pushes on `world+0x14`, the world's
/// projectile list, and on `out+0x20`). Constructor `0x00422890`: `+0..+0x10` = -1, `+0x10` =
/// 0, `+0x30..+0x3c` = 0, radius 0.5, `f50` 1.0, knockback 1.0, the rest 0; the fields it leaves
/// uninitialised are 0 here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projectile {
    /// `+0x00`: the shooter's id.
    pub owner: i64,
    /// `+0x08`: 12 bytes copied from `entity+0x1a0` (home zone x, y and the spawn index); -1, -1
    /// and 0 when not copied.
    pub zone: [i32; 3],
    /// `+0x14`: uninitialised by the constructor.
    pub f14: u32,
    /// `+0x18`: position (16.16 fixed).
    pub pos: [i64; 3],
    /// `+0x30..+0x3c`: zeroed by the constructor.
    pub f30: [u32; 3],
    /// `+0x3c`: velocity (blocks per second).
    pub vel: [f32; 3],
    /// `+0x48`: damage.
    pub damage: f32,
    /// `+0x4c`: radius (0.5).
    pub radius: f32,
    /// `+0x50`: (1.0).
    pub f50: f32,
    /// `+0x54`: knockback / charge (1.0).
    pub knockback: f32,
    /// `+0x58`.
    pub f58: f32,
    /// `+0x5c` (u8): a charged or special shot.
    pub flag5c: u8,
    /// `+0x60`: kind (0 spell, 1 arrow/bolt, 2 mode-0x1a spell, 3 heal/boomerang, 4 mode 0x6c).
    pub kind: i32,
    /// `+0x64` (u8): 2 = healing shot.
    pub sub: u8,
    /// `+0x68`: age in ms.
    pub age: i32,
    /// `+0x6c`.
    pub f6c: u32,
}

impl Projectile {
    /// `Shoot::Shoot` 0x00422890.
    pub fn new() -> Projectile {
        Projectile { owner: -1, zone: [-1, -1, 0], f14: 0, pos: [0; 3], f30: [0; 3], vel: [0.0; 3], damage: 0.0, radius: 0.5, f50: 1.0, knockback: 1.0, f58: 0.0, flag5c: 0, kind: 0, sub: 0, age: 0, f6c: 0 }
    }

    /// The 0x70-byte network record.
    pub fn to_shoot(&self) -> Shoot {
        let mut b = [0u8; 0x70];
        w64(&mut b, 0, self.owner);
        for (i, v) in self.zone.iter().enumerate() {
            w32(&mut b, 8 + i * 4, *v as u32);
        }
        w32(&mut b, 0x14, self.f14);
        for (i, v) in self.pos.iter().enumerate() {
            w64(&mut b, 0x18 + i * 8, *v);
        }
        for (i, v) in self.f30.iter().enumerate() {
            w32(&mut b, 0x30 + i * 4, *v);
        }
        set_vec3f(&mut b, 0x3c, self.vel);
        wf32(&mut b, 0x48, self.damage);
        wf32(&mut b, 0x4c, self.radius);
        wf32(&mut b, 0x50, self.f50);
        wf32(&mut b, 0x54, self.knockback);
        wf32(&mut b, 0x58, self.f58);
        b[0x5c] = self.flag5c;
        w32(&mut b, 0x60, self.kind as u32);
        b[0x64] = self.sub;
        w32(&mut b, 0x68, self.age as u32);
        w32(&mut b, 0x6c, self.f6c);
        Shoot(b)
    }
}

impl Default for Projectile {
    fn default() -> Self {
        Projectile::new()
    }
}

// ---------------------------------------------------------------------------------------------
// Byte helpers (duplicated from physics.rs).

/// `normalized` 0x00412670: `v * (1 / (float)sqrt((double)(x*x + y*y + z*z)))`.
fn normalized(v: [f32; 3]) -> [f32; 3] {
    let s = 1.0f32 / (f64::from(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() as f32);
    [v[0] * s, v[1] * s, v[2] * s]
}

/// `0x004e1520(out, s, v)` / `0x004079f0`: `v * s` per component.
fn scale3(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

/// `lengthSq2Fixed` 0x0041ce90: `(x*x)/65536 + (y*y)/65536` in i64 (wrapping multiply,
/// truncating divide).
fn len_sq2_fixed(d: [i64; 3]) -> i64 {
    (d[0].wrapping_mul(d[0]) / 65536).wrapping_add(d[1].wrapping_mul(d[1]) / 65536)
}

/// `fixedLess` 0x004dade0: `a < __ftol2(f * 65536)`.
fn fixed_less(a: i64, f: f32) -> bool {
    a < (f * 65536.0f32) as i64
}

/// `Server.exe 0x0040f2b0` (`hasTwoHandedWeapon`): the right hand holds a weapon of sub type
/// 0xf, 0x10, 0x11, 5, 10, 11, 0x12, 8, 6 or 7. Duplicated from combat_ai.rs (`heavy_weapon`).
fn two_handed(e: &EntityData) -> bool {
    e.0[WEAPON] == 3 && matches!(e.0[WEAPON + 1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)
}

/// `Server.exe 0x00411800` (`shootOrigin`): the point a creature with appearance flag 4
/// breathes from. Duplicated from combat_ai.rs (`mouth_pos`).
fn shoot_origin(e: &EntityData) -> [i64; 3] {
    let pos = pos_at(&e.0);
    if e.0[0x6e] & 4 == 0 {
        return pos;
    }
    let m: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let a = f32_at(&e.0, 0x20) * 0.0174532924f32;
    let c = cw_math::cos(f64::from(a)) as f32;
    let s = cw_math::sin(f64::from(a)) as f32;
    let a0 = m[4] * s + m[0] * c;
    let b0 = m[4] * c - m[0] * s;
    let b1 = m[5] * c - m[1] * s;
    let a1 = m[5] * s + m[1] * c;
    let a2 = m[6] * s + m[2] * c;
    let b2 = m[6] * c - m[2] * s;
    let a3 = m[7] * s + m[3] * c;
    let h = f32_at(&e.0, 0x74) * 0.5f32;
    let z = f32_at(&e.0, 0x78) * 0.35f32;
    let b3 = m[7] * c - m[3] * s;
    let w = ((h * b3 + a3 * 0.0f32) + m[11] * z) + m[15];
    let x = ((h * b0 + a0 * 0.0f32) + m[8] * z) + m[12];
    let y = ((h * b1 + a1 * 0.0f32) + m[9] * z) + m[13];
    let inv = 1.0f32 / w;
    let zz = ((h * b2 + a2 * 0.0f32) + m[10] * z) + m[14];
    let v = [inv * x, inv * y, inv * zz];
    [pos[0].wrapping_add(fix(v[0])), pos[1].wrapping_add(fix(v[1])), pos[2].wrapping_add(fix(v[2]))]
}

// ---------------------------------------------------------------------------------------------
// Creature stats resolved for this port.

/// `Server.exe 0x00409d10` (stdcall `(mode)`, ecx unused): the damage multiplier of a mode.
/// Decoded from the byte table 0x00409d90 (index `mode - 5`, 0x44 entries) and the jump table
/// 0x00409d70: 5 and 0x1a → 0.5; 0x15 and 0x48 → 0.1; 0x1e and 0x20 → 0.4; 0x1f and 0x21 →
/// 0.6; 0x25, 0x2b, 0x39, 0x3a and 0x44 → 2; 0x37 → 0.25; everything else 1.
pub fn damage_multiplier(mode: i32) -> f32 {
    match mode {
        5 | 0x1a => 0.5f32,
        0x15 | 0x48 => 0.1f32,
        0x1e | 0x20 => 0.4f32,
        0x1f | 0x21 => 0.6f32,
        0x25 | 0x2b | 0x39 | 0x3a | 0x44 => 2.0f32,
        0x37 => 0.25f32,
        _ => 1.0f32,
    }
}

/// `Server.exe 0x00414550`: the strength of a weapon item (type 3). Duplicated from
/// combat_ai.rs (`item_block`).
pub fn item_weapon_power(item: &[u8]) -> f32 {
    if item[0] != 3 {
        return 0.0;
    }
    let lv = i32_at(item, 0x114) as f32 * 0.1f32 + f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
    let p = || item_power(lv, i32::from(item[0xc]));
    match item[1] {
        3 | 4 | 0xd => p() * 2.0f32,
        5 => p() * 4.0f32,
        0xf | 0x10 | 0x11 | 0xa | 0xb | 0x12 | 8 | 6 | 7 => p() * 8.0f32,
        _ => p() * 4.0f32,
    }
}

/// The shared body of `baseDamage` 0x00408f70 and `magicPower` 0x00411ad0 (both decompiled):
/// `entity+0x170 * 2^0 * levelPow`; plus each struck weapon's strength (0x00409270 /
/// 0x00414550) when the mode strikes with any; else, without weapons, `levelPow *
/// 2^(power/4) * 2` with appearance flag 8 and `* 8` with flag 0x10.
fn power_sum(e: &EntityData) -> f32 {
    let p1 = level_pow(e);
    let p2 = pow(2.0, f64::from(0.25f32 * 0.0f32)) as f32;
    let mut v = f32_at(&e.0, 0x170) * p2 * p1;
    let items = strike_items(e);
    if !items.is_empty() {
        for o in items {
            v = item_weapon_power(&e.0[o..o + 0x118]) + v;
        }
        return v;
    }
    let flags = u16_at(&e.0, 0x6e);
    let pw = || pow(2.0, f64::from(f32::from(e.0[0x198]) * 0.25f32)) as f32;
    if flags & 8 != 0 {
        let a = level_pow(e);
        let b = pw();
        v = a * b * 2.0f32 + v;
    }
    if flags & 0x10 != 0 {
        let a = level_pow(e);
        let b = pw();
        v = b * a * 8.0f32 + v;
    }
    v
}

/// `Server.exe 0x00408f70` (`baseDamage`): [`power_sum`] times 50, 2, 5 or 3 in modes 0x57,
/// 0x5b, 0x5d, 0x60.
pub fn base_damage(e: &EntityData) -> f32 {
    let v = power_sum(e);
    match e.0[0x58] {
        0x57 => v * 50.0f32,
        0x5b => v * 2.0f32,
        0x5d => v * 5.0f32,
        0x60 => v * 3.0f32,
        _ => v,
    }
}

/// `Server.exe 0x00411ad0` (`magicPower`): [`power_sum`] without the per-mode factor.
pub fn magic_power(e: &EntityData) -> f32 {
    power_sum(e)
}

/// `Server.exe 0x00410f90(level, rarity)`: `itemPower(level, rarity) / 2^3`.
fn item_power_8(level: f32, rarity: i32) -> f32 {
    item_power(level, rarity) / (pow(2.0, 3.0) as f32)
}

/// `Server.exe 0x00413ac0`: the block/crit chance an item adds: 0.05 (0.1 for a two-handed
/// weapon or a chest item) times `1 - (dword+4 % 21) / 20` (+1 for material 0xb) times
/// `itemPower(level) / 8`; 0 below 0.001 or for item types other than 3..9.
pub fn item_chance(item: &[u8]) -> f32 {
    let t = item[0];
    if !matches!(t, 8 | 9 | 3 | 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let mut k = 0.05f32;
    if (t == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)) || t == 4 {
        k = 0.1f32;
    }
    let r = u32::from_le_bytes(item[4..8].try_into().unwrap()) % 21;
    let mut f = 1.0f32 - ((f64::from(r) + 0.0) as f32) / 20.0f32;
    if item[0xd] == 0xb {
        f = f + 1.0f32;
    }
    let lv = f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
    let v = item_power_8(lv, i32::from(item[0xc])) * k * f;
    if 0.001f32 > v {
        return 0.0;
    }
    v
}

/// `Server.exe 0x00409ac0` (`blockChance`): `(levelPow * 2^0 / 2^3) * 0.1` plus
/// [`item_chance`] of slot 6 and 7 (type 3), 2 (type 4), 3 (6), 4 (5), 5 (7), 1 (8), 8 and 9
/// (type 9), in that order.
pub fn block_chance(e: &EntityData) -> f32 {
    let p1 = level_pow(e);
    let p2 = pow(2.0, f64::from(0.25f32 * 0.0f32)) as f32;
    let p3 = pow(2.0, 3.0) as f32;
    let mut v = ((p1 * p2) / p3) * 0.1f32;
    for (slot, ty) in [(6usize, 3u8), (7, 3), (2, 4), (3, 6), (4, 5), (5, 7), (1, 8), (8, 9), (9, 9)] {
        let o = 0x2f0 + slot * 0x118;
        if e.0[o] == ty {
            v = item_chance(&e.0[o..o + 0x118]) + v;
        }
    }
    v
}

/// `Server.exe 0x0040f520` (`rollBlock`, used as the crit roll): chance 1 with a type-0xb buff,
/// else `blockChance + guard * 0.15`; one `rand()`; `rand / 32767 < chance`.
fn roll_block(world: &mut World, e: &EntityData, st: &CreatureState) -> bool {
    let chance = if has_buff(st, 0xb) { 1.0f32 } else { block_chance(e) + st.block * 0.15f32 };
    let r = world.rng.rand();
    (r as f32) / 32767.0f32 < chance
}

/// `Server.exe 0x0040f8f0(c, mode)` (`knockback`): 0 for hostile types 5 and 3 and modes
/// 0x1e..0x21; else by mode a constant or a constant over the attack speed.
fn knockback(e: &EntityData, st: &CreatureState, mode: i32) -> f32 {
    let h = e.0[0x50];
    if h == 5 || h == 3 {
        return 0.0;
    }
    let asp = || attack_speed(e, st.block, has_buff(st, 0xc));
    match mode {
        1 | 2 | 9 => 6.0f32 / asp(),
        3 | 4 | 0x3e => 3.0f32 / asp(),
        5 => 4.0,
        6 | 7 | 0x12 | 0x13 => 3.0f32 / asp(),
        0xb | 0x57 => 10.0,
        0xd | 0xe | 0xf => 6.0f32 / asp(),
        0x11 | 0x14 => 6.0,
        0x15 | 0x58 => skill_level_factor(e, 0x15, -1) * 30.0f32 + 5.0f32,
        0x1e..=0x21 => 0.0,
        0x36 => 5.0,
        0x3c | 0x3d => 6.0f32 / asp(),
        0x41 | 0x42 => 10.0f32 / asp(),
        _ => 2.0f32 / asp(),
    }
}

// ---------------------------------------------------------------------------------------------
// Records.

fn sound_rec(pos: [i64; 3], kind: u32, pitch: f32, volume: f32) -> Sound {
    let mut b = [0u8; 0x18];
    set_vec3f(&mut b, 0, pos_blocks(pos));
    w32(&mut b, 0xc, kind);
    wf32(&mut b, 0x10, pitch);
    wf32(&mut b, 0x14, volume);
    Sound(b)
}

/// A Hit record (0x48 bytes) as `0x00422a90` leaves it with the fields set: attacker +0, target
/// +8, damage +0x10, crit +0x14, position +0x20, hit type +0x45.
fn hit_rec(attacker: i64, target: i64, damage: f32, crit: bool, pos: [i64; 3], kind: u8) -> Hit {
    let mut h = [0u8; 0x48];
    w64(&mut h, 0, attacker);
    w64(&mut h, 8, target);
    wf32(&mut h, 0x10, damage);
    h[0x14] = u8::from(crit);
    for (i, p) in pos.iter().enumerate() {
        w64(&mut h, 0x20 + i * 8, *p);
    }
    h[0x45] = kind;
    Hit(h)
}

/// A particle record (0x48 bytes; ctor 0x004c8510: +0x3c = 0, +0x40 = 3.0): position +0,
/// velocity +0x18, colour +0x24, size +0x34, count +0x38, kind +0x3c, spread +0x40.
fn particle_rec(pos: [i64; 3], vel: [f32; 3], color: [f32; 4], size: f32, count: i32, kind: i32, spread: f32) -> Particle {
    let mut p = [0u8; 0x48];
    for (i, v) in pos.iter().enumerate() {
        w64(&mut p, i * 8, *v);
    }
    set_vec3f(&mut p, 0x18, vel);
    for (i, c) in color.iter().enumerate() {
        wf32(&mut p, 0x24 + i * 4, *c);
    }
    wf32(&mut p, 0x34, size);
    wi32(&mut p, 0x38, count);
    wi32(&mut p, 0x3c, kind);
    wf32(&mut p, 0x40, spread);
    Particle(p)
}

/// A PassivePacket (0x28 bytes, ctor 0x004063d0): `+0` and `+8` the creature's id, `+0x10` the
/// buff.
fn passive_rec(id: i64, buff: &Buff) -> Passive {
    let mut b = [0u8; 0x28];
    w64(&mut b, 0, id);
    w64(&mut b, 8, id);
    b[0x10..0x28].copy_from_slice(buff);
    Passive(b)
}

fn buff(ty: u8, value: f32, duration: i32) -> Buff {
    let mut b = [0u8; 0x18];
    b[0] = ty;
    wf32(&mut b, 4, value);
    wi32(&mut b, 8, duration);
    b
}

/// `World::tick` 0x0053e357..0x0053e4c8: an idle creature (mode 0) notifies every id of its
/// pending set (`creature+0x11b4`) that is not in its hit set (`+0x11ac`) and still exists:
/// `onMultiHit` 0x00408230 on it, then a zero-damage type-4 Hit record at its position (not
/// applied). Both sets clear afterwards.
pub fn idle_flush(entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, out: &mut ServerUpdate) {
    let (pending, hit) = {
        let ms = &states.entry(id).or_default().modes;
        (ms.pending_set.clone(), ms.hit_set.clone())
    };
    // std::set<i64> iteration: ascending ids.
    for tid in pending {
        if hit.contains(&tid) {
            continue;
        }
        let Some(t) = entities.get_mut(&tid) else { continue };
        // 0x00408230: a rolling class-4 spec-1 target gains 0.25 MP and buff 0xb for 30 s.
        if i32_at(&t.0, 0x118) != 0 && t.0[0x130] == 4 && t.0[0x131] == 1 {
            let mp = f32_at(&t.0, 0x160) + 0.25f32;
            wf32(&mut t.0, 0x160, if !(mp <= 1.0) { 1.0 } else { mp });
            let b = buff(0xb, 0.0, 30000);
            add_buff(states.entry(tid).or_default(), &b);
            out.passives.push(passive_rec(tid, &b));
        }
        let tpos = pos_at(&entities[&tid].0);
        out.hits.push(hit_rec(id, tid, 0.0, false, tpos, 4));
    }
    let ms = &mut states.entry(id).or_default().modes;
    ms.hit_set.clear();
    ms.pending_set.clear();
}

// ---------------------------------------------------------------------------------------------
// The machine's context.

struct Cx<'a> {
    world: &'a mut World,
    entities: &'a mut BTreeMap<i64, EntityData>,
    states: &'a mut BTreeMap<i64, CreatureState>,
    ms: &'a mut ModeState,
    projectiles: &'a mut Vec<crate::projectile::Projectile>,
    dirty: &'a mut BTreeSet<(i32, i32)>,
    out: &'a mut ServerUpdate,
    id: i64,
    dt: i32,
}

impl Cx<'_> {
    fn e(&self) -> &EntityData {
        &self.entities[&self.id]
    }

    fn em(&mut self) -> &mut EntityData {
        self.entities.get_mut(&self.id).expect("creature")
    }

    fn st(&mut self) -> &mut CreatureState {
        self.states.entry(self.id).or_default()
    }

    fn gh(&self) -> (f32, bool) {
        self.states.get(&self.id).map_or((0.0, false), |s| (s.block, has_buff(s, 0xc)))
    }

    fn st_ref(&self) -> CreatureState {
        // Performance note: a clone to read the state next to the entity; only buffs and the
        // guard are read through it.
        self.states.get(&self.id).cloned().unwrap_or_default()
    }

    /// `skillWindup(c, -1)` 0x00407db0.
    fn w(&self) -> i32 {
        let (g, h) = self.gh();
        skill_windup(self.e(), g, h, -1)
    }

    /// `skillDuration(c, -1)` 0x00411d60.
    fn d(&self) -> i32 {
        let (g, h) = self.gh();
        skill_duration(self.e(), g, h, -1)
    }

    /// `skillTotalTime(c)` 0x004084b0.
    fn t(&self) -> i32 {
        let (g, h) = self.gh();
        skill_total_time(self.e(), g, h)
    }

    /// `attackSpeed(c)` 0x00412150.
    fn aspd(&self) -> f32 {
        let (g, h) = self.gh();
        attack_speed(self.e(), g, h)
    }

    fn mt(&self) -> i32 {
        i32_at(&self.e().0, 0x5c)
    }

    fn set_mt(&mut self, v: i32) {
        wi32(&mut self.em().0, 0x5c, v);
    }

    fn mode(&self) -> u8 {
        self.e().0[0x58]
    }

    fn pos(&self) -> [i64; 3] {
        pos_at(&self.e().0)
    }

    fn f(&self, o: usize) -> f32 {
        f32_at(&self.e().0, o)
    }

    fn set_f(&mut self, o: usize, v: f32) {
        wf32(&mut self.em().0, o, v);
    }

    fn i(&self, o: usize) -> i32 {
        i32_at(&self.e().0, o)
    }

    fn set_i(&mut self, o: usize, v: i32) {
        wi32(&mut self.em().0, o, v);
    }

    fn mp(&self) -> f32 {
        self.f(0x160)
    }

    fn set_mp(&mut self, v: f32) {
        self.set_f(0x160, v);
    }

    fn charge(&self) -> f32 {
        self.states.get(&self.id).map_or(0.0, |s| s.charge)
    }

    fn set_charge(&mut self, v: f32) {
        self.st().charge = v;
    }

    fn rand(&mut self) -> i32 {
        self.world.rng.rand()
    }

    /// `P(k, b)`: `((float)rand() * k) / 32767 + b`.
    fn rpitch(&mut self, k: f32, b: f32) -> f32 {
        let r = self.rand() as f32;
        (r * k) / 32767.0f32 + b
    }

    /// A Sound at the creature's position (`out+8`, 0x00428590).
    fn sound(&mut self, kind: u32, pitch: f32) {
        let p = self.pos();
        self.out.sounds.push(sound_rec(p, kind, pitch, 1.0));
    }

    fn mana_cost(&self, mode: i32) -> f32 {
        let st = self.states.get(&self.id).cloned().unwrap_or_default();
        mana_cost(self.e(), &st, mode, -1)
    }

    /// The standard guard `(world+0xb4 == 0 && hostile != 0) || c == world+0xb8` (e.g.
    /// 0x00538117): on a server a non-player creature, in the client's world the local player.
    fn auth(&self) -> bool {
        (!self.world.is_client && self.e().0[0x50] != 0) || self.world.local_player == Some(self.id)
    }

    /// Case 0's own guard `world+0xb4 != 0 || hostile != 0 || c == world+0xb8` (0x00538117,
    /// `Cube.exe 0x00612357`: the first `jne` skips the other two tests, unlike the standard
    /// guard's): on a server a non-player creature, in the client's world every creature, so
    /// another player's swing sounds and particles play locally (its hits stay the server's:
    /// 0x0053a6cc, 0x0053a941 and `creatureAttack`'s local-player return).
    fn generic_guard(&self) -> bool {
        self.world.is_client || self.e().0[0x50] != 0 || self.world.local_player == Some(self.id)
    }

    /// `world+0xb4 == 0 || c == world+0xb8` (0x0053a6cc): the server, or the local player.
    fn server_or_local(&self) -> bool {
        !self.world.is_client || self.world.local_player == Some(self.id)
    }

    /// `world+0xb4 == 0 || (c == world+0xb8 && other == world+0xb8)` (0x0053a941, 0x0053acf7):
    /// the heal a creature gives another is applied on the server, or on the client when both
    /// are the local player.
    fn applies_to(&self, other: i64) -> bool {
        !self.world.is_client || (self.world.local_player == Some(self.id) && self.world.local_player == Some(other))
    }

    /// `(mt + dt) / 100 != mt / 100` (signed truncating divides).
    fn crosses_100(&self) -> bool {
        let mt = self.mt();
        mt.wrapping_add(self.dt) / 100 != mt / 100
    }

    /// `mt <= W && mt + dt > W`: the wind-up is crossed this tick.
    fn crosses_windup(&self) -> bool {
        self.mt() <= self.w() && self.mt().wrapping_add(self.dt) > self.w()
    }

    /// `addBuff` 0x00411740 on the creature and its PassivePacket on `out+0x58` (0x00411040).
    fn add_buff_packet(&mut self, b: Buff) {
        add_buff(self.st(), &b);
        let id = self.id;
        self.out.passives.push(passive_rec(id, &b));
    }

    /// `findBuff(c, ty)` 0x0040ef90 then a zero-duration buff of that type replacing it (the
    /// "consume on next tick" pattern).
    fn consume_buff(&mut self, ty: u8) {
        if has_buff(self.st(), ty) {
            self.add_buff_packet(buff(ty, 0.0, 0));
        }
    }

    fn roll_block(&mut self) -> bool {
        let st = self.st_ref();
        let e = self.entities[&self.id].clone();
        roll_block(self.world, &e, &st)
    }

    fn attack_speed_rescale(&mut self) {
        // attackSpeed reads the hit counter, so the two calls around its reset differ.
        let a = self.aspd();
        self.set_i(0x60, 0);
        let b = self.aspd();
        let mt = self.mt();
        self.set_mt(((a / b) * mt as f32) as i32);
    }

    /// Pushes a projectile on the world's list (`world+0x14`) and the network list.
    fn shoot(&mut self, sh: &Projectile) {
        self.projectiles.push(crate::projectile::projectile_from_shoot(&sh.to_shoot()));
        self.out.shoots.push(sh.to_shoot());
    }

    fn home_zone(&self) -> [i32; 3] {
        [self.i(0x1a0), self.i(0x1a4), self.i(0x1a8)]
    }

    fn apply(&mut self, h: &Hit) {
        apply_hit(self.world, self.entities, self.states, h, self.out, self.dirty, false);
    }
}

// ---------------------------------------------------------------------------------------------
// Entry.

/// The mode machine of `World::tick` for creature `id` (`Server.exe 0x00537bc1..0x0053bc86`,
/// with the cases reached from there up to 0x0053e0cb): `idx = byte[0x548a8c + mode - 1]`,
/// `jmp [0x548a44 + idx*4]` for modes 1..=0x6e, nothing for mode 0 or above 0x6e. Runs
/// between the timers and the combo timer of [`crate::update::update_creature`]. `ms` is the
/// creature's [`ModeState`]; `projectiles` the world's live projectile list (`world+0x14`);
/// `dirty` the zones `applyHit` marks.
pub fn run_mode_machine(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, ms: &mut ModeState, projectiles: &mut Vec<crate::projectile::Projectile>, dirty: &mut BTreeSet<(i32, i32)>, id: i64, dt: i32, out: &mut ServerUpdate) {
    let Some(e) = entities.get(&id) else { return };
    let mode = u32::from(e.0[0x58]);
    // 0x00537bce: `(unsigned)(mode - 1) > 0x6d` -> default.
    if mode.wrapping_sub(1) > 0x6d {
        return;
    }
    let idx = MODE_INDEX_TABLE[(mode - 1) as usize];
    let mut cx = Cx { world, entities, states, ms, projectiles, dirty, out, id, dt };
    match idx {
        0 => case_generic(&mut cx),
        1 => case_mode8(&mut cx),
        2 => case_spell(&mut cx),
        3 => case_charge_tick(&mut cx),
        4 => case_beam(&mut cx),
        5 => case_mode22(&mut cx),
        6 => case_ranged(&mut cx),
        7 => case_mode2f(&mut cx),
        8 => case_teleport(&mut cx),
        9 => case_burst(&mut cx),
        10 => case_mode49(&mut cx),
        11 => case_mode59(&mut cx),
        12 => case_summon(&mut cx),
        13 => case_mode65(&mut cx),
        14 => case_mode67(&mut cx),
        // 0x00537be2, mode 0x69: the local player's home teleport (`c == world+0xb8`, else the
        // default).
        15 => {
            if cx.world.local_player == Some(cx.id) {
                let w = cx.w();
                let dt = cx.dt;
                let e = cx.entities.get_mut(&cx.id).expect("creature");
                crate::client::home_teleport(cx.world, e, w, dt);
            }
        }
        16 => case_tame(&mut cx),
        // 0x0053bc80: default.
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Case 0: the generic melee state machine (0x00538117 .. 0x0053a9ed).

/// Case idx 0 (0x00538117): modes 1-7, 9-21, 30-33, 51, 54, 57, 58, 60-62, 65-70, 72, 74-78,
/// 86-88, 91, 93, 104. Start and wind-up sounds and particles, the hit window with its
/// multi-hit periods, the dodge flush, the attack-speed rescale, the MP cost and charge, the
/// reach and centre, the big-creature terrain destruction and the creature-hit loop.
fn case_generic(cx: &mut Cx) {
    // 0x00538117: non-player creatures on the server, every creature in the client's world.
    if !cx.generic_guard() {
        return;
    }
    let dt = cx.dt;
    // 0x00538138: the class-4 spec-1 roll start.
    let m0 = cx.mode();
    if cx.e().0[0x130] == 4 && cx.e().0[0x131] == 1 && matches!(m0, 0x11 | 5 | 0x14) && cx.mt() == 0 {
        cx.set_i(0x118, 600);
    }
    // 0x00538169: the wind-up, kept in edi until the 0x005384b3 block.
    let w0 = cx.w();
    let mode = cx.mode();
    if mode == 0x57 || mode == 0x58 {
        // L_55C 0x0053855c.
        let mt = cx.mt();
        if mt < cx.w() && cx.crosses_100() {
            let p = cx.rpitch(0.25, 1.0);
            cx.sound(0x24, p);
        }
        // 0x00538619
        if !(cx.mt().wrapping_sub(dt) >= cx.w()) && !(cx.mt() < cx.w()) {
            // W <= mt < W + dt
            let pos = cx.pos();
            let p = particle_rec(pos, [0.0, 0.0, 2.0], [0.0; 4], 0.25, 80, 1, 20.0);
            cx.sound(0x51, 1.0);
            cx.out.particles.push(p);
        }
    } else if (0x1e..=0x21).contains(&mode) {
        // L_255 0x00538255.
        let mt = cx.mt();
        if mt < cx.w() && cx.crosses_100() {
            let p = cx.rpitch(0.25, 1.0);
            cx.sound(0x24, p);
        }
        // 0x00538312
        if !(cx.mt() >= cx.w()) && !(cx.mt().wrapping_add(dt) < cx.w()) {
            // The wind-up is reached this tick. The colour stays uninitialised stack (0 here)
            // unless spec 1.
            let e = cx.e();
            let pos = add3(pos_at(&e.0), fix3(vec3f_at(&e.0, 0x150)));
            let spec1 = e.0[0x131] == 1;
            let (kind, color) = if spec1 { (2, [0.0, 0.2, 1.0, 1.0]) } else { (1, [0.0; 4]) };
            let p = particle_rec(pos, [0.0, 0.0, 2.0], color, 0.25, 20, kind, 8.0);
            cx.sound(if spec1 { 0x29 } else { 0x26 }, 1.0);
            cx.out.particles.push(p);
        }
    } else if cx.mt() == 0 && two_handed(cx.e()) && cx.mode() != 0x5b {
        // 0x005381dc: the two-hander's start sound.
        let p = cx.rpitch(0.1, 1.0);
        cx.sound(0x11, p);
    }
    // L_4B3 0x005384b3: the wind-up-boundary sound.
    {
        let mt = cx.mt();
        if !(mt > w0) && !(mt.wrapping_add(dt) <= w0) && cx.mode() != 0x5b {
            let pos = cx.pos();
            let mut pitch = cx.rpitch(0.2, 0.9);
            let m = cx.mode();
            let kind = match m {
                0xa => 0x10,
                0xb => 0x30,
                0x36 => 0xe,
                _ => {
                    // 0x00538797
                    let p15 = pitch * 1.5f32;
                    pitch = p15;
                    let m = cx.mode();
                    if two_handed(cx.e()) || m == 0x44 || m == 0x5d || m == 0x45 {
                        pitch = p15 * 0.5f32;
                    }
                    0xf
                }
            };
            cx.out.sounds.push(sound_rec(pos, kind, pitch, 1.0));
        }
    }
    // L_826 0x00538826: the mid-swing sound of 0x39/0x3a/0x3c/0x4a.
    let m = cx.mode();
    if m == 0x39 || m == 0x3c || m == 0x3a || m == 0x4a {
        if cx.mt() < cx.w() + cx.d() / 2 && cx.mt().wrapping_add(dt) >= cx.w() + cx.d() / 2 {
            let p = cx.rpitch(0.25, 1.0);
            cx.sound(0xc, p);
        }
    }
    // 0x00538903: the hit window.
    let mut hit_start = (cx.d() / 2 - 100) + cx.w();
    let mut hit_end = cx.w() + cx.d() / 2;
    if hit_start < cx.w() {
        hit_start = cx.w();
    }
    if hit_end >= cx.w() + cx.d() {
        hit_end = (cx.w() - 1) + cx.d();
    }
    if cx.mode() == 0x44 || cx.mode() == 0x45 {
        hit_start = cx.d() / 3 + cx.w();
    }
    if cx.mode() == 0x5d {
        hit_start = cx.d() / 5 + cx.w();
        hit_end = cx.w() + cx.d();
    }
    // 0x00538a72: the multi-hit modes.
    let mut reset_hits = false;
    let m = cx.mode();
    if matches!(m, 0x48 | 0x56 | 0x1e | 0x1f | 0x20 | 0x21 | 0xb | 0x5 | 0x5b) {
        if cx.mt() < cx.w() || cx.mt() >= cx.w() + cx.d() {
            // 0x00538dfd
            let w = cx.w();
            hit_start = w;
            hit_end = w;
        } else {
            let mut period = if m == 0x5b { 1000 } else { 250 };
            if m == 0xb {
                period = cx.d() / 3;
            }
            if cx.mode() == 0x5 {
                period = cx.d() / 3;
            }
            if cx.mode() == 0x1e || cx.mode() == 0x20 {
                period = cx.d() / 6;
            }
            if cx.mode() == 0x1f || cx.mode() == 0x21 {
                period = cx.d() / 12;
            }
            if period == 0 {
                // The original's idiv faults here (an attack speed that makes the duration
                // shorter than the divisor); nothing sensible to reproduce.
                return;
            }
            hit_start = (cx.mt() - cx.w()) / period * period + cx.w();
            hit_end = hit_start + 100;
            let k_next = (cx.mt().wrapping_add(dt) - cx.w()) / period * period + cx.w();
            if k_next != hit_start && k_next < cx.w() + cx.d() {
                cx.ms.hit_count_copy = cx.i(0x60);
                reset_hits = true;
                cx.ms.hit_set.clear();
                cx.ms.pending_set.clear();
            }
            if cx.mt().wrapping_add(dt) > hit_end {
                reset_hits = true;
            }
            if cx.mode() != 5 {
                let mut play = true;
                if cx.mt().wrapping_sub(dt) >= cx.w() {
                    let a = (cx.mt().wrapping_add(dt) - cx.w()) / period;
                    let b = (cx.mt() - cx.w()) / period;
                    if b == a {
                        play = false;
                    }
                }
                if play {
                    let p = cx.rpitch(0.25, 1.0);
                    cx.sound(0xf, p);
                    if cx.mode() == 0x5b {
                        let p = cx.rpitch(0.1, 0.5);
                        cx.sound(0x52, p);
                    }
                }
            }
        }
    }
    // L_E12 0x00538e12: before the window, the dodgers are notified and the sets clear.
    if cx.mt() <= cx.w() {
        cx.ms.hit_count_copy = cx.i(0x60);
        flush_pending(cx);
        cx.ms.hit_set.clear();
        cx.ms.pending_set.clear();
        cx.ms.landed = 0;
    }
    // L_FC0 0x00538fc0
    if cx.mode() == 0x4a && cx.mt() < cx.w() + cx.d() / 2 && cx.mt().wrapping_add(dt) >= cx.w() + cx.d() / 2 {
        let p = cx.rpitch(0.1, 0.5);
        cx.sound(0x52, p);
    }
    // 0x0053908c
    if cx.mt() >= hit_end {
        flush_pending(cx);
        cx.ms.pending_set.clear();
    }
    // 0x00539208: the end of the swing rescales the mode time by the attack speed.
    if cx.ms.landed == 0 && cx.mt().wrapping_add(dt) >= cx.w() + cx.d() {
        if cx.mt() < cx.t() && cx.i(0x118) == 0 {
            cx.attack_speed_rescale();
        }
    }
    // 0x005392b7: MP cost and charge.
    let m = cx.mode();
    if m == 0x1f || m == 0x21 {
        // L_514 0x00539514
        if !(cx.mt() > cx.w()) && !(cx.mt().wrapping_add(dt) <= cx.w()) {
            cx.set_charge(0.1);
            let cost = cx.mana_cost(i32::from(cx.mode()));
            let mp = cx.mp() - cost;
            cx.set_mp(mp);
            if 0.0 > cx.mp() {
                cx.set_mp(0.0);
            }
        }
    } else if cx.mt() == 0 {
        cx.ms.free_cast = 0;
        cx.set_charge(0.0);
        let cost = cx.mana_cost(i32::from(cx.mode()));
        if cost > 0.0 {
            let cost = cx.mana_cost(i32::from(cx.mode()));
            cx.set_charge(cost);
            let mp = cx.mp() - cost;
            cx.set_mp(mp);
        } else {
            // 0x0053940b
            let m = cx.mode();
            if m == 0x11 || m == 0x14 || m == 0x5 {
                let mp = cx.mp();
                cx.set_charge(mp);
                cx.set_mp(0.0);
                if has_buff(cx.st(), 0xb) {
                    cx.add_buff_packet(buff(0xb, 0.0, 0));
                    cx.ms.free_cast = 1;
                }
            } else if m == 0x36 || m == 0x15 || m == 0x57 || m == 0x58 {
                cx.set_charge(1.0);
            }
        }
    }
    // L_362 0x00539362
    let charged = cx.f(0x134);
    if charged > 0.0 {
        cx.set_charge(charged);
        let mp = cx.mp() - charged;
        cx.set_mp(mp);
        cx.ms.landed = 0;
        if cx.mode() == 0xb || cx.mode() == 0x5 {
            let c = cx.charge() * 0.5f32;
            cx.set_charge(c);
        }
    }
    cx.set_f(0x134, 0.0);
    if 0.0 > cx.mp() {
        cx.set_mp(0.0);
    }
    let has_charge = cx.charge() > 0.0 || cx.mode() == 0xb;
    // L_5AB 0x005395ab: inside the hit window.
    let mt = cx.mt();
    if mt <= hit_start || mt > hit_end {
        return;
    }
    let (center, reach) = hit_centre(cx);
    // 0x005398d2: big creatures (scale.x > 4) break the blocks in reach (at most 8); the
    // server's only (0x005398de, `world+0xb4`).
    let mut destroyed = false;
    let m = cx.mode();
    if !cx.world.is_client && cx.f(0x70) > 4.0 && !(m == 0x57 || m == 0x5b || m == 0x4a) {
        destroyed = destroy_terrain(cx, center, reach);
    }
    if destroyed {
        // 0x00539c3b
        let p = cx.rpitch(0.4, 0.5);
        cx.sound(2, p);
        // 0x00539cbb: without a local player (`world+0xb8`, NULL on the server) the light pass
        // runs, then the unsupported props drop. Argument order pushed: 0, 8, int(cy+R),
        // int(cx+R), int(cy-R), int(cx-R).
        let y1 = floor_div_fix(center[1].wrapping_add(fix(reach)));
        let x1 = floor_div_fix(center[0].wrapping_add(fix(reach)));
        let y0 = floor_div_fix(center[1].wrapping_sub(fix(reach)));
        let x0 = floor_div_fix(center[0].wrapping_sub(fix(reach)));
        if cx.world.local_player.is_none() {
            compute_lighting(cx.world, x0, y0, x1, y1, 8);
        }
        drop_unsupported_props(cx.world, x0, y0, x1, y1);
    }
    hit_creatures(cx, center, reach, has_charge, reset_hits);
}

/// The dodge flush (0x00538e3d..0x00538f95 and 0x005390a1..0x005391ef): every id of the
/// pending set (`+0x11b4`) not in the hit set (`+0x11ac`) that still exists gets
/// `onMultiHit` 0x00408230 and a zero-damage type-4 Hit record (not applied).
fn flush_pending(cx: &mut Cx) {
    let ids: Vec<i64> = cx.ms.pending_set.iter().copied().collect();
    for tid in ids {
        if cx.ms.hit_set.contains(&tid) {
            continue;
        }
        let Some(t) = cx.entities.get(&tid) else { continue };
        let tpos = pos_at(&t.0);
        on_multi_hit(cx, tid);
        let h = hit_rec(cx.id, tid, 0.0, false, tpos, 4);
        cx.out.hits.push(h);
    }
}

/// `onMultiHit` 0x00408230(target, out): a rolling class-4 spec-1 target gains 0.25 MP (capped
/// at 1 the `!(x <= 1)` way) and a 30 s type-0xb buff with its PassivePacket.
fn on_multi_hit(cx: &mut Cx, tid: i64) {
    let t = cx.entities.get_mut(&tid).expect("target");
    if i32_at(&t.0, 0x118) != 0 && t.0[0x130] == 4 && t.0[0x131] == 1 {
        let mp = f32_at(&t.0, 0x160) + 0.25f32;
        wf32(&mut t.0, 0x160, if !(mp <= 1.0) { 1.0 } else { mp });
        let b = buff(0xb, 0.0, 30000);
        add_buff(cx.states.entry(tid).or_default(), &b);
        cx.out.passives.push(passive_rec(tid, &b));
    }
}

/// 0x005395ab..0x005398c0: the hit's reach and centre by mode.
fn hit_centre(cx: &mut Cx) -> ([i64; 3], f32) {
    let e = cx.e();
    let yaw = f32_at(&e.0, 0x20);
    let sx = f32_at(&e.0, 0x70);
    let mut reach = sx * 1.5f32;
    let mut fwd = 1.5f32;
    let m = e.0[0x58];
    if matches!(m, 0xd | 0xe | 2 | 1 | 9 | 4 | 3 | 7 | 6 | 0x12 | 0x13) {
        reach = sx * 2.0f32;
        fwd = 1.0;
    }
    if m == 0x14 || m == 0x15 {
        reach = reach * 2.0f32;
    }
    if m == 0x44 || m == 0x46 {
        fwd = 0.0;
    }
    if m == 0x56 || m == 0x5d || m == 0x68 {
        reach = reach * 2.0f32;
        fwd = 0.0;
    }
    if m == 0x57 || m == 0x58 {
        reach = 8.0;
        fwd = 0.0;
    }
    if m == 0x1e || m == 0x20 {
        reach = 3.0;
        fwd = 0.0;
    }
    if m == 0x1f || m == 0x21 {
        reach = 5.0;
        fwd = 0.0;
    }
    if m == 0x5b || m == 0x4a {
        reach = 20.0;
        fwd = 0.0;
    }
    if matches!(m, 0x48 | 0x4e | 0x4d | 0x4c) {
        reach = reach * 1.2f32;
        fwd = 0.0;
    }
    let off = [0.0f32, sx * fwd, 0.0];
    let pos = pos_at(&e.0);
    let mut center = add3(pos, fix3(rotate_z(yaw, off)));
    if (0x1e..=0x21).contains(&m) {
        center = add3(pos, fix3(vec3f_at(&e.0, 0x150)));
    }
    if m == 0x4b || m == 0x45 {
        center[2] = center[2].wrapping_sub(fix((f32_at(&e.0, 0x78) - sx) * 0.5f32));
    }
    if m == 0xb || m == 0x3d || m == 0x36 {
        center = pos;
        reach = reach * 3.0f32;
    }
    (center, reach)
}

/// 0x005398d2..0x00539c35: the sphere of blocks around `center` (radius `min(8, (int)reach)`,
/// x outer, z inner) whose flag bit 0x20 is set break: a particle in the block's colour,
/// `World::clearBlock` 0x00530470 and a BlockAction record. Returns whether any broke.
fn destroy_terrain(cx: &mut Cx, center: [i64; 3], reach: f32) -> bool {
    let mut destroyed = false;
    let mut r = reach as i32;
    if r > 8 {
        r = 8;
    }
    if -r > r {
        return false;
    }
    for dx in -r..=r {
        for dy in -r..=r {
            for dz in -r..=r {
                let rr = r * r;
                let p = add3(center, [i64::from(dx) << 16, i64::from(dy) << 16, i64::from(dz) << 16]);
                let d = diff_blocks(p, center);
                if rr as f32 > len_sq3(d) {
                    let blk = block_fix(cx.world, p);
                    // 0x005306c0: (blk[3] >> 5) & 1.
                    if (blk[3] >> 5) & 1 != 0 {
                        // The colour is computed b2, b1, b0 in the original; each is independent.
                        let color = [f32::from(blk[0]) / 255.0f32, f32::from(blk[1]) / 255.0f32, f32::from(blk[2]) / 255.0f32, 1.0];
                        cx.out.particles.push(particle_rec(p, [0.0, 0.0, 10.0], color, 0.5, 3, 0, 3.0));
                        clear_block(cx.world, floor_div_fix(p[0]), floor_div_fix(p[1]), floor_div_fix(p[2]));
                        // 0x004c64f0: `P / 65536`, truncating.
                        let day = cx.world.day;
                        cx.out.block_actions.push(BlockAction { x: (p[0] / 65536) as i32, y: (p[1] / 65536) as i32, z: (p[2] / 65536) as i32, block: [0, 0, 0, 0], time: day });
                        destroyed = true;
                    }
                }
            }
        }
    }
    destroyed
}

/// `World::clearBlock` 0x00530470 (decompiled): for a block inside a loaded zone and inside its
/// column's array (`height <= z < height + len`), `Column::setBlockRaw(z - height, 0x005842d4 or
/// 0x005842d8)` (both `00 00 00 00`).
pub fn clear_block(world: &mut World, bx: i32, by: i32, bz: i32) {
    if !(0..0x1000000).contains(&bx) || !(0..0x1000000).contains(&by) {
        return;
    }
    let Some(zone) = world.zone_mut(bx / 256, by / 256) else { return };
    let col = zone.column_mut(bx, by);
    let h = col.height;
    if h <= bz && bz < col.blocks.len() as i32 + h {
        col.set_raw(bz - h, [0, 0, 0, 0]);
    }
}

/// `World::computeLighting(x0, y0, x1, y1, 8, NULL)` 0x004d1a70 after a terrain destruction.
/// The port of the light pass (`World::post_process_blocks`) works on one zone at a time; the
/// original with a NULL zone reads every loaded zone's columns, so across zone borders the
/// propagation here can differ (neighbours outside the zone read as the "below" block).
fn compute_lighting(world: &mut World, x0: i32, y0: i32, x1: i32, y1: i32, margin: i32) {
    let (ax0, ay0, ax1, ay1) = (x0 - margin, y0 - margin, x1 + margin, y1 + margin);
    let mut zones = Vec::new();
    for zx in ax0.div_euclid(256)..=(ax1 - 1).div_euclid(256) {
        for zy in ay0.div_euclid(256)..=(ay1 - 1).div_euclid(256) {
            zones.push((zx, zy));
        }
    }
    for (zx, zy) in zones {
        if let Some(mut z) = world.remove_zone(zx, zy) {
            world.post_process_blocks(&mut z, x0, y0, x1, y1, margin);
            world.insert_zone(z);
        }
    }
}

/// `Server.exe 0x004d9160(world, x0, y0, x1, y1)` (decompiled): for each loaded zone of the
/// block range (each bound clamped at 0, then `/ 256`), the props (`zone+4` list) with flag bit
/// 2 (`+0x38 & 2`) whose block one below their position (`getBlockFixed(x, y, z - 1.0)`) is
/// air are removed.
pub fn drop_unsupported_props(world: &mut World, x0: i32, y0: i32, x1: i32, y1: i32) {
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.max(0), y1.max(0));
    for zx in (x0 >> 8)..=(x1 >> 8) {
        for zy in (y0 >> 8)..=(y1 >> 8) {
            if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
                continue;
            }
            let Some(zone) = world.zone(zx, zy) else { continue };
            let keep: Vec<bool> = zone.props.iter().map(|p| !(p.flags & 2 != 0 && block_type(block_fix(world, [p.x, p.y, p.z.wrapping_sub(0x10000)])) == 0)).collect();
            if keep.iter().all(|k| *k) {
                continue;
            }
            let zone = world.zone_mut(zx, zy).expect("zone");
            let mut it = keep.into_iter();
            zone.props.retain(|_| it.next().unwrap_or(true));
        }
    }
}

/// 0x00539e44..0x0053a9ed: the creature-hit loop of the hit window.
fn hit_creatures(cx: &mut Cx, center: [i64; 3], r: f32, has_charge: bool, reset_hits: bool) {
    let id = cx.id;
    let mut hp_term = 1.0f32;
    let m = cx.mode();
    if m == 0x3c || m == 0x0b || m == 0x3e {
        hp_term = 0.0;
    }
    let mut mp_given = false;
    if cx.mode() == 0x1f && !(cx.mt() < cx.w()) && has_buff(cx.st(), 9) {
        cx.add_buff_packet(buff(9, 0.0, 0));
    }
    // 0x00539f38: over the world's creature map (players included), in id order. The creature
    // itself is not skipped (it only passes `isEnemy` against itself with the aggressive flag).
    // Performance note: the id snapshot and entity clones keep the borrows simple.
    let ids: Vec<i64> = cx.entities.keys().copied().collect();
    for oid in ids {
        let Some(other) = cx.entities.get(&oid).cloned() else { continue };
        if 0.0 >= f32_at(&other.0, 0x15c) {
            continue;
        }
        if cx.ms.hit_set.contains(&oid) {
            continue;
        }
        // 0x00539fdd
        let reach = f32_at(&other.0, 0x70) * 0.7f32 + r;
        let mut origin = cx.pos();
        if (0x1e..=0x21).contains(&cx.mode()) {
            origin = add3(center, [0, 0, fix(r)]);
        }
        // 0x0053a096
        let rr = reach * reach;
        let opos = pos_at(&other.0);
        let d = [center[0].wrapping_sub(opos[0]), center[1].wrapping_sub(opos[1]), center[2].wrapping_sub(opos[2])];
        if !fixed_less(len_sq2_fixed(d), rr) {
            continue;
        }
        let dz = ((center[2].wrapping_sub(opos[2])) as f32 * K).abs();
        let mut t = f32_at(&other.0, 0x78) * 0.5f32;
        t = t + cx.f(0x78);
        t = t + r;
        if !(t > dz) {
            continue;
        }
        // 0x0053a18d
        if !crate::path::line_of_sight(cx.world, origin, opos, true, 200.0) {
            continue;
        }
        // 0x0053a1b7
        if is_enemy(cx.e(), &other) {
            if i32_at(&other.0, 0x118) != 0 {
                // The target rolls: it is notified at the end of the window.
                cx.ms.pending_set.insert(oid);
                continue;
            }
            // 0x0053a201
            let mut dir = diff_blocks(opos, cx.pos());
            dir[2] = 0.0;
            let l2 = len_sq3(dir);
            if l2 > 0.01f32 {
                normalize3(&mut dir);
            }
            dir[2] = 0.25;
            let crit = if cx.ms.free_cast != 0 { true } else { cx.roll_block() };
            // 0x0053a296
            let st = cx.st_ref();
            let kb = knockback(cx.e(), &st, i32::from(cx.mode()));
            let s = (f64::from(cx.f(0x70) / f32_at(&other.0, 0x70))).sqrt() as f32;
            dir = scale3(dir, s);
            dir = scale3(dir, kb);
            // 0x0053a2ee: damage.
            let m = cx.mode();
            let mut damage;
            if matches!(m, 0x57 | 0x58 | 0x1e | 0x1f | 0x20 | 0x21) {
                let a = damage_multiplier(i32::from(m));
                let b = magic_power(cx.e());
                let t2 = a * b;
                let rn = cx.rand() as f32;
                let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
                let one = 0.0f32 + 1.0f32;
                x = x * t2;
                damage = x * one;
            } else {
                let ch = cx.charge();
                let a = damage_multiplier(i32::from(m));
                let b = base_damage(cx.e());
                let mut p = a * b;
                let mut q = ch * ch;
                q = q * 5.0f32;
                q = q + hp_term;
                p = p * q;
                let rn = cx.rand() as f32;
                let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
                x = x + 0.0f32;
                damage = x * p;
            }
            if crit {
                damage = damage * 2.0f32;
            }
            // 0x0053a45c
            let first_hit = cx.ms.hit_set.is_empty() && cx.i(0x11c) <= 0;
            cx.ms.hit_set.insert(oid);
            cx.ms.landed = 1;
            let m = cx.mode();
            let special = (0x1e..=0x21).contains(&m);
            let charge = cx.charge();
            // 0x0053a52b: creatureAttack(target, attacker, damage, crit, flag2c04, charge,
            // &c.pos, &dir, out, &dirty, special, mode, 0, 1).
            let ok = crate::combat_ai::creature_attack(cx.world, cx.entities, cx.states, oid, Some(id), damage, crit, has_charge, charge, 0, dir, cx.out, cx.dirty, false, special, i32::from(m), 0, true);
            if ok {
                // 0x0053a538
                if cx.mode() == 0x1e {
                    let e = cx.e().clone();
                    if chance_roll(cx.world, &e, 0.25) && cx.e().0[0x131] == 0 {
                        cx.add_buff_packet(buff(9, 0.0, 30000));
                        cx.sound(0x2f, 1.0);
                    }
                }
                // 0x0053a659
                if first_hit {
                    let c = cx.i(0x60) + 1;
                    cx.set_i(0x60, c);
                    // 0x004103a0: an in-order walk over the cooldowns with no stores.
                    cx.set_i(0x64, 0);
                }
                // 0x0053a679
                if has_charge && cx.e().0[0x130] == 4 && cx.e().0[0x131] == 0 {
                    let g = cx.charge() + cx.st().block;
                    cx.st().block = g;
                    if g > 1.0 {
                        cx.st().block = 1.0;
                    }
                }
                // 0x0053a6c6: the MP gain, once per window, on the server or for the local
                // player (0x0053a6cc); every other path sets the flag too (0x0053a7c5).
                if cx.server_or_local() && !mp_given && !has_charge && cx.mode() != 0x1e && cx.mode() != 0x20 {
                    let rn = cx.rand() as f32;
                    let mut x = 1.0f32 - (rn * 2.0f32) / 32767.0f32;
                    x = x * 0.05f32;
                    x = x + 0.1f32;
                    let t = cx.t();
                    let mut gain = ((t as f32) / 500.0f32) * x;
                    let r2 = cx.rand();
                    if r2 % 8 == 0 {
                        gain = gain * 1.5f32;
                    }
                    let mp = cx.mp() + gain;
                    cx.set_mp(mp);
                    if mp > 1.0 {
                        cx.set_mp(1.0);
                    }
                }
                // 0x0053a7c5: every path of a landed attack sets the flag.
                mp_given = true;
            }
        }
        // ALLY 0x0053a7d1: modes 0x20/0x21 of spec 1 heal allies in reach while a hit period
        // starts.
        let m = cx.mode();
        if (m == 0x21 || m == 0x20) && reset_hits {
            let Some(other) = cx.entities.get(&oid).cloned() else { continue };
            if !is_enemy(cx.e(), &other) && cx.e().0[0x131] == 1 {
                let rn = cx.rand() as f32;
                let x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
                let mut v = magic_power(cx.e());
                v = v * x;
                let one = 0.0f32 + 1.0f32;
                v = v * one;
                let dmg = v * -0.1f32;
                let crit = cx.roll_block();
                let h = hit_rec(id, oid, dmg, crit, pos_at(&other.0), 0);
                cx.out.hits.push(h.clone());
                cx.sound(0x2a, 1.0);
                // 0x0053a941
                if cx.applies_to(oid) {
                    cx.apply(&h);
                }
            }
        }
    }
    // 0x0053a9b1: the rest (a skillTotalTime call with its result discarded) has no effect.
}

// ---------------------------------------------------------------------------------------------
// The other cases, in jump-table order.

/// Case idx 1, mode 8 (0x00537fb4): the wind-up sound 0x10 and the charge tick 0x37 every
/// 100 ms, pitched by the charged MP.
fn case_mode8(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    if cx.mt() <= cx.w() && cx.mt().wrapping_add(cx.dt) > cx.w() {
        let p = cx.rpitch(0.25, 1.0);
        cx.sound(0x10, p);
    }
    // 0x00538071
    if !cx.crosses_100() {
        return;
    }
    let p = cx.f(0x134) * 0.5f32 + 1.0f32;
    cx.sound(0x37, p);
}

/// Case idx 2, modes 0x16 / 0x17 / 0x1a (0x0053ce3a): the spell projectile at the wind-up
/// crossing (mode 0x1a again at W + 300 when charged): MP cost, Shoot kind 0 / 2, the type-0xa
/// buff consumed, sound 0x16 / 0xf.
fn case_spell(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    let dt = cx.dt;
    if cx.crosses_windup() {
        let cost = cx.mana_cost(i32::from(cx.mode()));
        if cost > 0.0 {
            let c2 = cx.mana_cost(i32::from(cx.mode()));
            let mp = cx.mp() - c2;
            cx.set_mp(mp);
        } else {
            let mp = cx.mp() - cx.f(0x134);
            cx.set_mp(mp);
        }
        if 0.0 > cx.mp() {
            cx.set_mp(0.0);
        }
        let ch = cx.f(0x134);
        cx.set_charge(ch);
        cx.set_f(0x134, 0.0);
    }
    // 0x0053cf16
    let fire = if cx.mt() <= cx.w() && cx.mt().wrapping_add(dt) > cx.w() {
        true
    } else {
        // 0x0053cf44: the second shot of mode 0x1a.
        if cx.mode() != 0x1a || !(cx.charge() > 0.0) {
            return;
        }
        if cx.mt() > cx.w() + 300 {
            return;
        }
        if cx.mt().wrapping_add(dt) <= cx.w() + 300 {
            return;
        }
        true
    };
    if !fire {
        return;
    }
    // FIRE 0x0053cf9b
    let mut sh = Projectile::new();
    sh.vel = scale3(normalized(vec3f_at(&cx.e().0, 0x150)), 100.0);
    let m = cx.mode();
    let ch = cx.charge();
    sh.age = 0;
    sh.radius = 0.5;
    let kb;
    if m == 0x1a {
        let mut t0 = ch * 2.0f32;
        let t1 = ch * 0.2f32;
        t0 = t0 + 2.0f32;
        kb = t1;
        sh.knockback = t1;
        sh.radius = t0;
    } else {
        kb = ch;
        sh.knockback = ch;
    }
    let cost = cx.mana_cost(i32::from(m));
    if cost > 0.0 {
        sh.knockback = cx.mana_cost(i32::from(cx.mode()));
        sh.f58 = sh.knockback;
    } else {
        sh.f58 = kb;
    }
    sh.flag5c = u8::from(cx.charge() > 0.0 || cx.mode() == 0x17);
    sh.pos = cx.pos();
    sh.owner = cx.id;
    sh.zone = cx.home_zone();
    if sh.flag5c != 0 {
        cx.consume_buff(0xa);
    }
    // 0x0053d1a8
    let m = cx.mode();
    if m == 0x1a {
        sh.f50 = cx.f(0xa0);
        sh.kind = 2;
    } else {
        sh.kind = 0;
    }
    let mul_a = if m == 0x1a { 1.0f32 } else { 5.0f32 };
    let mut add = 1.0f32;
    if sh.knockback > 0.0 {
        add = 0.1;
    }
    if m == 0x17 {
        add = 0.0;
    }
    let ch = cx.charge();
    let a = damage_multiplier(i32::from(m));
    let b = base_damage(cx.e());
    let mut p = a * b;
    let mut q = ch * ch;
    q = q * mul_a;
    q = q + add;
    p = p * q;
    let rn = cx.rand() as f32;
    let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
    x = x + 0.0f32;
    sh.damage = x * p;
    cx.shoot(&sh);
    let m = cx.mode();
    let (kind, pitch) = if m == 0x16 || m == 0x17 {
        let rn = cx.rand() as f32;
        let y = (rn * 0.5f32) / 32767.0f32;
        if sh.flag5c != 0 { (0x16, y + 1.0f32) } else { (0xf, y + 2.0f32) }
    } else if m == 0x1a {
        // 0x0053d32c: double precision (0.51, 32767.0 and the widened f32 0.7).
        let rn = f64::from(cx.rand());
        (0xf, ((rn * 0.51) / 32767.0 + 0.699999988079071) as f32)
    } else {
        // Unreachable for this case's modes.
        (0x16, cx.rpitch(0.5, 1.0))
    };
    cx.sound(kind, pitch);
}

/// Case idx 3, modes 0x18 0x19 0x1b 0x24 0x3b 0x3f 0x40 (0x0053b973): the charge tick 0x37
/// every 100 ms, pitched by the charged MP.
fn case_charge_tick(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    // `edx` still holds dt from the pre-dispatch code (assumed by the transcription).
    if !cx.crosses_100() {
        return;
    }
    let p = cx.f(0x134) * 0.5f32 + 1.0f32;
    cx.sound(0x37, p);
}

/// Case idx 4, modes 0x1c / 0x5e / 0x5f (0x0053ba28 and its tail 0x0053bc8b..0x0053caf0): the
/// beam. Mode 0x1c drains MP continuously; 0x5e / 0x5f pay at the wind-up. The beam marches
/// from `shootOrigin` along the ray hit every 100 ms of the active part (0x1c, 0x5f) and at the
/// crossing, attacking every enemy it touches. The `mode == 0x68` branches are dead (mode 0x68
/// dispatches to case 0) and left out.
fn case_beam(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    let dt = cx.dt;
    let dt_f = dt as f32;
    // 0x0053bb21
    if cx.mode() == 0x1c {
        let mp = cx.mp() - dt_f * 2e-4f32;
        cx.set_mp(mp);
    } else if !(cx.mt() > cx.w()) && !(cx.mt().wrapping_add(dt) <= cx.w()) {
        // 0x0053bb83
        let c = cx.mana_cost(i32::from(cx.mode()));
        let mp = cx.mp() - c;
        cx.set_mp(mp);
        cx.consume_buff(9);
    }
    // CLAMP 0x0053bc66
    if 0.0 > cx.mp() {
        cx.set_mp(0.0);
        return;
    }
    // T 0x0053ba9d
    if cx.mt().wrapping_add(dt) < cx.w() {
        return;
    }
    if cx.mt() >= cx.w() + cx.d() {
        return;
    }
    let crossing = !(cx.mt() > cx.w()) && cx.mt().wrapping_add(dt) > cx.w();
    if !crossing {
        // PERIODIC 0x0053bc91
        if cx.mode() != 0x5f && cx.mode() != 0x1c {
            return;
        }
        let a = (cx.mt().wrapping_add(dt) - cx.w()) / 100;
        let b = (cx.mt() - cx.w()) / 100;
        if b == a {
            return;
        }
        if cx.mt().wrapping_add(dt) <= cx.w() {
            return;
        }
    }
    // BEAM 0x0053bd1c
    let id = cx.id;
    let ray = vec3f_at(&cx.e().0, 0x150);
    let mut dir = ray;
    let l2 = len_sq3(dir);
    if l2 > 0.0 {
        normalize3(&mut dir);
    }
    let mut cursor = shoot_origin(cx.e());
    let n = normalized(ray);
    let reach = crate::path::sweep(cx.world, cursor, n, 200.0, false, true);
    cx.ms.hit_set.clear();
    cx.ms.pending_set.clear();
    let mut hit_any = false;
    // 0x0053bdfa: the class-3 spec-1 heal shoot at the beam's end, on the crossing tick.
    if cx.e().0[0x130] == 3 && cx.e().0[0x131] == 1 && cx.crosses_windup() {
        heal_shoot(cx, add3(cursor, fix3(scale3(normalized(ray), reach))));
    }
    // 0x0053bf9e: march in 1-block steps.
    if reach > 0.0 {
        let mut k = 0i32;
        let mut kf = 0.0f32;
        let _ = kf;
        loop {
            let ids: Vec<i64> = cx.entities.keys().copied().collect();
            for oid in ids {
                let Some(other) = cx.entities.get(&oid).cloned() else { continue };
                if oid == id {
                    continue;
                }
                if oid == cx.ms.mount {
                    continue;
                }
                if 0.0 >= f32_at(&other.0, 0x15c) {
                    continue;
                }
                if cx.ms.hit_set.contains(&oid) {
                    continue;
                }
                if i32_at(&other.0, 0x118) != 0 {
                    continue;
                }
                // 0x0053c097
                let mut r = f32_at(&other.0, 0x70) * 0.5f32;
                r = r + 1.0f32;
                let rr = r * r;
                let opos = pos_at(&other.0);
                let d = [cursor[0].wrapping_sub(opos[0]), cursor[1].wrapping_sub(opos[1]), cursor[2].wrapping_sub(opos[2])];
                if !fixed_less(len_sq2_fixed(d), rr) {
                    continue;
                }
                let dz = ((cursor[2].wrapping_sub(opos[2])) as f32 * K).abs();
                let mut h = f32_at(&other.0, 0x78) * 0.5f32;
                h = h + 1.0f32;
                if !(h > dz) {
                    continue;
                }
                // 0x0053c189
                cx.ms.hit_set.insert(oid);
                let crit = cx.roll_block();
                // kbDir ([ebp-0x2a8]) is computed here and never used: no side effects.
                let rnd = cx.rpitch(0.05, 1.0);
                let mp = magic_power(cx.e());
                let mut dmg = mp * 1.5f32;
                dmg = dmg * rnd;
                if crit {
                    dmg = dmg * 2.0f32;
                }
                if !is_enemy(cx.e(), &other) {
                    continue;
                }
                let atk_dir = [0.0f32; 3];
                let m = cx.mode();
                let factor = if m == 0x5f { 0.1f32 } else { 0.0 };
                let special = m == 0x5f || m == 0x5e || m == 0x1c;
                let flag5 = m == 0x5f;
                // 0x0053c488: creatureAttack(other, c, dmg, crit, flag5, factor, &cursor,
                // &atkDir, out, &dirty, special, 0, 0, 1).
                let ok = crate::combat_ai::creature_attack(cx.world, cx.entities, cx.states, oid, Some(id), dmg, crit, flag5, factor, 0, atk_dir, cx.out, cx.dirty, false, special, 0, 0, true);
                if ok && cx.e().0[0x131] == 2 {
                    // Life steal: a Hit on itself with the negated damage (pushed, not applied
                    // through applyHit) and the HP added directly on the server.
                    dmg = -dmg;
                    let h = hit_rec(id, id, dmg, crit, cx.pos(), 0);
                    cx.out.hits.push(h);
                    // 0x0053c529: the server's (`world+0xb4`).
                    if !cx.world.is_client {
                        let hp = cx.f(0x15c) - dmg;
                        cx.set_f(0x15c, hp);
                        let mx = max_hp(cx.e());
                        if cx.f(0x15c) > mx {
                            let mx2 = max_hp(cx.e());
                            cx.set_f(0x15c, mx2);
                        }
                    }
                }
                // 0x0053c58d
                if cx.e().0[0x130] == 3 && cx.e().0[0x131] == 1 && cx.crosses_windup() {
                    heal_shoot(cx, cursor);
                }
                hit_any = true;
            }
            // 0x0053c723
            cursor = add3(cursor, fix3(dir));
            k += 1;
            kf = k as f32;
            if !(reach > kf) {
                break;
            }
        }
        if hit_any {
            let c = cx.i(0x60) + 1;
            cx.set_i(0x60, c);
            cx.set_i(0x64, 0);
        }
    }
    // 0x0053c797: a miss sparks at the beam's end and rescales the mode time.
    if cx.ms.hit_set.is_empty() {
        let v = scale3(normalized(ray), reach);
        let pos = add3(cx.pos(), fix3(v));
        // The colour is a vec3f (1.0, 0.2, 0.5) copied as 16 bytes: the alpha is stale stack
        // in the original (0 here).
        let p = particle_rec(pos, [dir[0] * -1.0f32, dir[1] * -1.0f32, dir[2] * -1.0f32], [1.0, 0.2, 0.5, 0.0], 0.1, 5, 1, 3.0);
        cx.out.particles.push(p);
        cx.attack_speed_rescale();
    }
    // 0x0053c8e6
    if cx.mode() == 0x5e && cx.crosses_windup() {
        let p = cx.rpitch(0.1, 1.0);
        cx.sound(0x28, p);
        return;
    }
    if cx.mode() == 0x5e {
        return;
    }
    if !cx.crosses_100() {
        return;
    }
    let p = cx.rpitch(0.25, 1.0);
    cx.sound(0x24 + u32::from(hit_any), p);
}

/// The class-3 spec-1 heal shoot of the beam (0x0053bdfa and 0x0053c58d): kind 3, sub 2,
/// damage `magicPower * 0.1`, radius 4, at `pos`, no velocity.
fn heal_shoot(cx: &mut Cx, pos: [i64; 3]) {
    let mut sh = Projectile::new();
    sh.kind = 3;
    sh.flag5c = 0;
    sh.knockback = 0.0;
    sh.f58 = 0.0;
    let m = magic_power(cx.e());
    sh.owner = cx.id;
    sh.damage = m * 0.1f32;
    sh.radius = 4.0;
    sh.sub = 2;
    sh.pos = pos;
    sh.f50 = 1.0;
    sh.age = 0;
    sh.vel = [0.0; 3];
    cx.shoot(&sh);
}

/// Case idx 5, mode 0x22 (0x0053a9f2): the channelled heal: eight pulses over the duration,
/// each costing MP and healing the target (`entity+0x190`, else itself) by `magicPower * 2 *
/// rnd` through a negative Hit.
fn case_mode22(cx: &mut Cx) {
    let id = cx.id;
    let t = cx.mt();
    if t < cx.w() && cx.crosses_100() {
        let p = cx.rpitch(0.25, 1.0);
        cx.sound(0x24, p);
    }
    // 0x0053aaab
    if !cx.auth() {
        return;
    }
    if cx.mt() <= cx.w() {
        return;
    }
    if cx.mt() > cx.w() + cx.d() {
        return;
    }
    let per = (cx.d() as f32 * 0.125f32) as i32;
    if per == 0 {
        // The original's idiv faults.
        return;
    }
    let a = (cx.mt().wrapping_add(cx.dt) - cx.w()) / per;
    let b = (cx.mt() - cx.w()) / per;
    if b == a {
        return;
    }
    let cost = cx.mana_cost(i32::from(cx.mode()));
    if !(cx.mp() >= cost) {
        return;
    }
    let c2 = cx.mana_cost(i32::from(cx.mode()));
    let mp = cx.mp() - c2;
    cx.set_mp(mp);
    let tid0 = i64_at(&cx.e().0, 0x190);
    let tid = if cx.entities.contains_key(&tid0) { tid0 } else { id };
    let tpos = pos_at(&cx.entities[&tid].0);
    let rn = cx.rand() as f32;
    let x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
    let mut v = magic_power(cx.e());
    v = v * -2.0f32;
    let dmg = v * x;
    let crit = cx.roll_block();
    let h = hit_rec(id, tid, dmg, crit, tpos, 0);
    cx.out.hits.push(h.clone());
    cx.sound(0x29, 1.0);
    // 0x0053acf7
    if cx.applies_to(tid) {
        cx.apply(&h);
    }
}

/// Case idx 6, modes 0x25-0x2e and 0x6c (0x0053d7e1): the ranged weapon shot: charge sounds,
/// MP at the wind-up, one Shoot at the wind-up (0x2d / 0x2e: every D/3), kind 1 (4 for 0x6c).
fn case_ranged(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    let dt = cx.dt;
    let mt = cx.mt();
    if mt <= cx.w() - 200 && cx.crosses_100() {
        let p = cx.rpitch(0.25, 1.0);
        cx.sound(0x24, p);
    }
    // 0x0053d8c6
    let mut fire_t = cx.w();
    if cx.mode() == 0x2e || cx.mode() == 0x2d {
        let per = cx.d() / 3;
        if per == 0 {
            // The original's idiv faults.
            return;
        }
        let kk = (cx.mt().wrapping_add(dt) - cx.w()) / per;
        fire_t = per * kk;
        fire_t = fire_t + cx.w();
        if fire_t < cx.w() {
            fire_t = cx.w();
        }
    }
    // 0x0053d93e
    if cx.crosses_windup() && len_sq3(vec3f_at(&cx.e().0, 0x150)) > 0.0 {
        let c = cx.mana_cost(i32::from(cx.mode()));
        let mp = cx.mp() - c;
        cx.set_mp(mp);
        if 0.0 > cx.mp() {
            cx.set_mp(0.0);
        }
    }
    // 0x0053d9c6
    let mt = cx.mt();
    if mt >= cx.w() + cx.d() {
        return;
    }
    if mt > fire_t {
        return;
    }
    if mt.wrapping_add(dt) <= fire_t {
        return;
    }
    let ray = vec3f_at(&cx.e().0, 0x150);
    if !(len_sq3(ray) > 0.0) {
        return;
    }
    let mut sh = Projectile::new();
    let spd = if cx.e().0[WEAPON + 1] == 0xc { 100.0f32 } else { 50.0f32 };
    sh.vel = scale3(normalized(ray), spd);
    sh.age = 0;
    let charged = cx.f(0x134);
    sh.radius = charged * 4.0f32 + 0.5f32;
    sh.pos = cx.pos();
    sh.knockback = charged;
    sh.f50 = charged + 0.5f32;
    if cx.i(0x54) == 0x65 {
        sh.pos[1] = sh.pos[1].wrapping_add(fix(cx.f(0x74)));
        sh.pos[2] = sh.pos[2].wrapping_add(fix(cx.f(0x78) * 0.5f32));
    }
    sh.owner = cx.id;
    sh.zone = cx.home_zone();
    if cx.mode() == 0x6c {
        sh.kind = 4;
        let a = damage_multiplier(i32::from(cx.mode()));
        let b = base_damage(cx.e());
        let p = a * b;
        let rn = cx.rand() as f32;
        let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
        let one = 0.0f32 + 1.0f32;
        sh.f50 = 4.0;
        sh.knockback = 1.0;
        sh.flag5c = 1;
        x = x * p;
        x = x * one;
        x = x * 5.0f32;
        sh.damage = x;
        sh.vel = scale3(normalized(ray), 100.0);
    } else {
        sh.kind = 1;
        let m = cx.mode();
        let a = damage_multiplier(i32::from(m));
        let b = magic_power(cx.e());
        let p = a * b;
        let rn = cx.rand() as f32;
        let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
        let one = 0.0f32 + 1.0f32;
        if matches!(m, 0x26 | 0x27 | 0x28 | 0x2c | 0x29 | 0x2a) {
            x = x * p;
            x = x * one;
        } else if m == 0x2e || m == 0x2d {
            sh.knockback = 0.25;
            x = x * p;
            x = x * one;
            sh.f50 = sh.f50 + 0.5f32;
        } else {
            sh.knockback = 1.0;
            x = x * p;
            x = x * one;
            sh.f50 = sh.f50 + 1.0f32;
        }
        sh.damage = x;
    }
    // 0x0053decd
    if cx.e().0[0x130] == 3 {
        sh.sub = u8::from(cx.e().0[0x131] == 1) + 1;
    }
    let m = cx.mode();
    sh.flag5c = u8::from(m == 0x2e || m == 0x2d || m == 0x25 || m == 0x2b);
    cx.projectiles.push(crate::projectile::projectile_from_shoot(&sh.to_shoot()));
    if cx.f(0x134) > 0.0 {
        let mp = cx.mp() - cx.f(0x134);
        cx.set_mp(mp);
        if 0.0 > cx.mp() {
            cx.set_mp(0.0);
        }
        cx.set_f(0x134, 0.0);
    }
    if sh.flag5c != 0 {
        cx.consume_buff(9);
    }
    cx.out.shoots.push(sh.to_shoot());
    let pos = cx.pos();
    let m = cx.mode();
    let (kind, pitch) = if sh.sub == 2 {
        (0x29, if m == 0x2c || m == 0x29 || m == 0x2a { 1.25f32 } else { 1.0 })
    } else {
        (0x26, if m == 0x26 || m == 0x27 || m == 0x28 { 2.0f32 } else { 1.0 })
    };
    cx.out.sounds.push(sound_rec(pos, kind, pitch, 0.9));
}

/// Case idx 7, modes 0x2f / 0x30 (0x00537f28): sound 0xe on the first tick (no hostile test).
fn case_mode2f(cx: &mut Cx) {
    let mt = cx.mt();
    if mt > 0 {
        return;
    }
    if mt.wrapping_add(cx.dt) <= 0 {
        return;
    }
    let p = cx.rpitch(0.25, 1.0);
    cx.sound(0xe, p);
}

/// Case idx 8, mode 0x31 (0x0053b24f): the teleport to the free spot nearest the ray hit (a
/// 9x9x9 box search, -2..6 in z), then down onto the ground or water.
fn case_teleport(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    if cx.mt() > cx.w() {
        return;
    }
    if cx.mt().wrapping_add(cx.dt) <= cx.w() {
        return;
    }
    let base = add3(cx.pos(), fix3(vec3f_at(&cx.e().0, 0x150)));
    let scale = vec3f_at(&cx.e().0, 0x70);
    let mut best = 400.0f32;
    let mut best_pos = base;
    for bx in -4i64..=4 {
        for by in -4i64..=4 {
            for bz in -2i64..=6 {
                let cand = add3(base, [bx << 16, by << 16, bz << 16]);
                if crate::path::box_collides(cx.world, cand, scale, false) {
                    continue;
                }
                let d = len_sq3(diff_blocks(cand, base));
                if best > d {
                    best = d;
                    best_pos = cand;
                }
            }
        }
    }
    // 0x0053b415
    if !(400.0f32 > best) {
        return;
    }
    let mut pos = best_pos;
    set_pos(&mut cx.em().0, pos);
    // 0x0053b440: settle down.
    loop {
        let below = [pos[0], pos[1], pos[2].wrapping_sub(1 << 16)];
        if crate::path::box_collides(cx.world, below, scale, false) {
            break;
        }
        let mut z = pos[2].wrapping_sub(fix(scale[2] * 0.5f32));
        z = z.wrapping_sub(1 << 16);
        let blk = block_fix(cx.world, [pos[0], pos[1], z]);
        if block_type(blk) == 2 {
            break;
        }
        pos[2] = pos[2].wrapping_sub(1 << 16);
        set_pos(&mut cx.em().0, pos);
    }
    // 0x0053b547
    cx.st().ground_z = pos[2] as f32 * K;
}

/// Case idx 9, modes 0x32 / 0x37 / 0x60 (0x0053d3f4): the charge is taken at the wind-up,
/// then a Shoot (kind 0, speed 150, flag 1) each D/4 ms of the duration.
fn case_burst(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    let dt = cx.dt;
    if cx.crosses_windup() {
        let ch = cx.f(0x134);
        cx.set_charge(ch);
        let mp = cx.mp() - cx.f(0x134);
        cx.set_mp(mp);
        cx.set_f(0x134, 0.0);
        cx.consume_buff(0xa);
    }
    // 0x0053d518
    let per = cx.d() / 4;
    if cx.mt() < cx.w() {
        return;
    }
    let mt = cx.mt();
    if mt >= cx.w() + cx.d() {
        return;
    }
    if per == 0 {
        // The original's idiv faults.
        return;
    }
    // Not relative to the wind-up.
    if mt.wrapping_add(dt) / per == mt / per {
        return;
    }
    let ray = vec3f_at(&cx.e().0, 0x150);
    if !(len_sq3(ray) > 0.0) {
        return;
    }
    let p = cx.rpitch(0.5, 1.0);
    cx.sound(0x16, p);
    let mut sh = Projectile::new();
    sh.vel = scale3(normalized(ray), 150.0);
    sh.age = 0;
    sh.radius = 0.5;
    sh.knockback = cx.charge() * 0.25f32;
    sh.f58 = 0.0;
    sh.flag5c = 1;
    sh.pos = cx.pos();
    sh.owner = cx.id;
    sh.zone = cx.home_zone();
    sh.kind = 0;
    let add = if cx.mode() == 0x37 { 0.1f32 } else { 1.0f32 };
    let ch = cx.charge();
    let a = damage_multiplier(i32::from(cx.mode()));
    let b = base_damage(cx.e());
    let mut p = a * b;
    let mut q = ch * ch;
    q = q * 5.0f32;
    q = q + add;
    p = p * q;
    let rn = cx.rand() as f32;
    let mut x = 1.25f32 - (rn * 0.5f32) / 32767.0f32;
    x = x + 0.0f32;
    sh.damage = x * p;
    cx.shoot(&sh);
}

/// Case idx 10, mode 0x49 (0x0053caf5): sound 0x2b at the wind-up (no hostile test on the
/// server).
fn case_mode49(cx: &mut Cx) {
    // 0x0053caf5: the server's (`world+0xb4`; the client goes to the join 0x00537d0a).
    if cx.world.is_client {
        return;
    }
    if cx.mt() >= cx.w() {
        return;
    }
    if cx.mt().wrapping_add(cx.dt) < cx.w() {
        return;
    }
    let p = cx.rpitch(0.25, 0.6);
    cx.sound(0x2b, p);
}

/// Case idx 11, modes 0x59 / 0x5a (0x0053afb9): the boomerang-like Shoot (kind 3, radius 8,
/// sub 1 / 2) at the wind-up crossing.
fn case_mode59(cx: &mut Cx) {
    let t = cx.mt();
    if t < cx.w() && cx.crosses_100() {
        let p = cx.rpitch(0.25, 1.0);
        cx.sound(0x24, p);
    }
    // 0x0053b072
    if !cx.auth() {
        return;
    }
    if cx.mt() > cx.w() {
        return;
    }
    if cx.mt().wrapping_add(cx.dt) <= cx.w() {
        return;
    }
    let p = cx.rpitch(0.25, 1.0);
    cx.sound(0x26, p);
    let mut sh = Projectile::new();
    sh.kind = 3;
    sh.flag5c = 0;
    sh.knockback = 0.0;
    sh.f58 = 0.0;
    let a = damage_multiplier(i32::from(cx.mode()));
    let b = magic_power(cx.e());
    sh.owner = cx.id;
    let mut dmg = a * b;
    dmg = dmg * 2.0f32;
    sh.damage = dmg;
    sh.radius = 8.0;
    sh.sub = u8::from(cx.mode() == 0x5a) + 1;
    sh.pos = cx.pos();
    sh.f50 = 1.0;
    sh.age = 0;
    sh.vel = [0.0; 3];
    cx.shoot(&sh);
}

/// Case idx 12, mode 0x5c (0x0053b568): summon a pet (at most three per owner) with id one below
/// the smallest id in the world.
fn case_summon(cx: &mut Cx) {
    if !cx.auth() {
        return;
    }
    if cx.mt() > cx.w() {
        return;
    }
    if cx.mt().wrapping_add(cx.dt) <= cx.w() {
        return;
    }
    let id = cx.id;
    let mut owned = 0;
    let mut min_id = 0i64;
    for (k, o) in cx.entities.iter() {
        if i64_at(&o.0, 0x188) == id {
            owned += 1;
        }
        if *k < min_id {
            min_id = *k;
        }
    }
    if !cx.entities.is_empty() && owned > 2 {
        return;
    }
    // 0x0053b6b7
    let new_id = min_id.wrapping_sub(1);
    // `Creature::Creature(&id)` 0x00406400: the entity block of a new creature. The creature
    // fields outside the block are the defaults of CreatureState / ModeState.
    let mut pet = EntityData::constructed();
    pet.0[0x50] = if cx.e().0[0x50] == 0 { 3 } else { 1 };
    let ty = if cx.ms.summon_types.is_empty() {
        0x25
    } else {
        let r = cx.rand() as u32;
        cx.ms.summon_types[(r % cx.ms.summon_types.len() as u32) as usize]
    };
    match cx.ms.summon_look {
        1 => {
            let e = cx.e();
            wi32(&mut pet.0, 0x54, i32_at(&e.0, 0x54));
            pet.0[0x2f0..0x1128].copy_from_slice(&e.0[0x2f0..0x1128]);
            pet.0[0x68..0x114].copy_from_slice(&e.0[0x68..0x114]);
        }
        2 => {
            let src_id = cx.states.get(&id).map_or(0, |s| s.last_target);
            let src = cx.entities.get(&src_id).unwrap_or(&cx.entities[&id]);
            wi32(&mut pet.0, 0x54, i32_at(&src.0, 0x54));
            pet.0[0x2f0..0x1128].copy_from_slice(&src.0[0x2f0..0x1128]);
            pet.0[0x68..0x114].copy_from_slice(&src.0[0x68..0x114]);
        }
        _ => {
            wi32(&mut pet.0, 0x54, ty);
            // `Creature::initAppearance(&type, &appearance, NULL)` 0x0040a840 over the
            // constructor's appearance defaults.
            let mut sp = Spawn { entity_type: ty, appearance: Appearance::NEW, ..Spawn::NEW };
            cx.world.init_appearance(&mut sp);
            pet.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&sp.appearance.to_bytes());
        }
    }
    // 0x0053b7ee
    let f = u16_at(&pet.0, 0x6e);
    w16(&mut pet.0, 0x6e, (f & 0xfdff) | 0x800);
    wi32(&mut pet.0, 0x180, cx.i(0x180));
    // maxHp of the summoner (ecx = c), as the original.
    let hp = max_hp(cx.e());
    wf32(&mut pet.0, 0x15c, hp);
    set_pos(&mut pet.0, cx.pos());
    w64(&mut pet.0, 0x188, id);
    cx.entities.insert(new_id, pet);
    // The behaviour (0x0053b82b): Sequence[Combat(20), Companion(owner), RandomWalk].
    let st = CreatureState { ai_root: Some(AiNode::Sequence(vec![AiNode::Base(Behavior::Combat(CombatState::new(20.0))), AiNode::Companion(CompanionBehavior { leader: id }), AiNode::Base(Behavior::RandomWalk { timer: 0 })])), ..CreatureState::default() };
    cx.states.insert(new_id, st);
    cx.ms.summoned.push((new_id, id));
}

/// Case idx 13, mode 0x65 (0x0053ad3c): a 10 s type-1 buff (stun immunity) on the first tick.
fn case_mode65(cx: &mut Cx) {
    if !cx.auth() || cx.mt() != 0 {
        return;
    }
    cx.sound(0x19, 1.0);
    let b = buff(1, 0.0, 10000);
    add_buff(cx.st(), &b);
    if cx.i(0x11c) > 0 {
        cx.set_i(0x11c, 0);
    }
    let id = cx.id;
    cx.out.passives.push(passive_rec(id, &b));
}

/// Case idx 14, mode 0x67 (0x0053ae68): a 30 s type-6 shield buff worth `magicPower * 4` on the
/// first tick.
fn case_mode67(cx: &mut Cx) {
    if !cx.auth() || cx.mt() != 0 {
        return;
    }
    cx.sound(0x19, 1.0);
    let v = magic_power(cx.e()) * 4.0f32;
    let b = buff(6, v, 30000);
    add_buff(cx.st(), &b);
    if cx.i(0x11c) > 0 {
        cx.set_i(0x11c, 0);
    }
    let id = cx.id;
    cx.out.passives.push(passive_rec(id, &b));
}

/// Case idx 16, mode 0x6e (0x0053cba1): being fed. At the wind-up, when the feeder
/// (`creature+0x11d0`) holds pet food (slot 12 type 0x14) for this creature's type: sound 0x2c,
/// a type-6 Hit, the type-7 buff's progress + 0.2; at 1.0 the creature becomes a pet
/// (`World::tame`). No hostile test on the server.
fn case_tame(cx: &mut Cx) {
    // 0x0053cba1: the server's (`world+0xb4`; the client goes to the join 0x00537d0a).
    if cx.world.is_client {
        return;
    }
    if cx.mt() >= cx.w() {
        return;
    }
    if cx.mt().wrapping_add(cx.dt) < cx.w() {
        return;
    }
    let id = cx.id;
    let fid = cx.states.get(&id).map_or(0, |s| s.last_target);
    let Some(f) = cx.entities.get(&fid) else { return };
    let food = 0x2f0 + 12 * 0x118;
    if f.0[food] != 0x14 {
        return;
    }
    if u32::from(f.0[food + 1]) != cx.i(0x54) as u32 {
        return;
    }
    let p = cx.rpitch(0.2, 0.9);
    cx.sound(0x2c, p);
    let h = hit_rec(id, fid, 0.0, false, cx.pos(), 6);
    cx.out.hits.push(h);
    let mut prog = 0.0f32;
    for b in &cx.st().buffs {
        if b[0] == 7 {
            prog = f32_at(b, 4);
        }
    }
    prog = prog + 0.2f32;
    let c = cx.i(0x60) + 1;
    cx.set_i(0x60, c);
    cx.set_i(0x64, 0);
    if prog >= 1.0 {
        cx.em().0[0x50] = 5;
        prog = 1.0;
        tame(cx.entities, cx.states, fid, id);
        cx.ms.tamed.push((id, fid));
    }
    let mut b = buff(7, prog, 5000);
    let owner = i64_at(&cx.e().0, 0x188);
    w64(&mut b, 0x10, owner);
    // 0x004ce9f0: addBuff and the PassivePacket.
    cx.add_buff_packet(b);
}

/// `World::tame(owner, pet)` 0x00522580 (decompiled): the pet's behaviour becomes
/// `Sequence[Combat(20), Companion(owner)]` (`creature+0x13e4`, the old tree dropped); its multipliers (`entity+0x168..0x178`) reset to (100, 1, 1, 1, 1),
/// type 0x19 gets (300, _, 0.1, 5, 5), types 0x56 and 0x68 get class/spec 0x103 / 0x102. With
/// the owner in the world: the pet's level is capped at the owner's, its parent is the owner,
/// its HP multiplier grows by `skillFactor(owner skill 0) * 0.5 + 1` and the owner's pet
/// (`creature+0x11c8`) is the pet.
pub fn tame(entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, owner: i64, pet: i64) {
    {
        let st = states.entry(pet).or_default();
        st.ai_root = Some(AiNode::Sequence(vec![AiNode::Base(Behavior::Combat(CombatState::new(20.0))), AiNode::Companion(CompanionBehavior { leader: owner })]));
    }
    let owner_e = entities.get(&owner).cloned();
    let Some(p) = entities.get_mut(&pet) else { return };
    wf32(&mut p.0, 0x168, 100.0);
    wf32(&mut p.0, 0x16c, 1.0);
    wf32(&mut p.0, 0x170, 1.0);
    wf32(&mut p.0, 0x174, 1.0);
    wf32(&mut p.0, 0x178, 1.0);
    match i32_at(&p.0, 0x54) {
        0x19 => {
            wf32(&mut p.0, 0x168, 300.0);
            wf32(&mut p.0, 0x174, 5.0);
            wf32(&mut p.0, 0x178, 5.0);
            wf32(&mut p.0, 0x170, 0.1);
        }
        0x56 => w16(&mut p.0, 0x130, 0x103),
        0x68 => w16(&mut p.0, 0x130, 0x102),
        _ => {}
    }
    let Some(o) = owner_e else { return };
    if i32_at(&o.0, 0x180) < i32_at(&p.0, 0x180) {
        wi32(&mut p.0, 0x180, i32_at(&o.0, 0x180));
    }
    w64(&mut p.0, 0x188, owner);
    // 0x00407c80: `k < 1 ? 0 : 1 - 1 / (k * 0.1 + 1)` of the owner's skill 0 (x87).
    let k = i32_at(&o.0, 0x1128);
    let f = if k < 1 { 0.0f32 } else { (1.0f64 - 1.0f64 / (f64::from(k as f32) * f64::from(0.1f32) + 1.0f64)) as f32 };
    let m = (f * 0.5f32 + 1.0f32) * f32_at(&p.0, 0x168);
    wf32(&mut p.0, 0x168, m);
    states.entry(owner).or_default().pet = pet;
}

// ---------------------------------------------------------------------------------------------
// The buff loop's active effects (0x0053e0d0 .. 0x0053e1b5).

/// The effect of an active buff in the tick's buff loop (`0x0053e0d0`, entered after `b.duration
/// -= dt` left `remaining > 0`): type 1 clears the stun (`entity+0x11c > 0` → 0); type 4
/// (poison) makes its source attack the holder for the buff's value each time the remaining
/// duration crosses a multiple of **200** ms (`0x51eb851f sar 7`; update.rs's comment says 400).
/// The source may be gone: the original passes NULL; this port passes `None` then.
///
/// update.rs calls this for each surviving buff after its duration was lowered.
pub fn active_buff_effect(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, b: &Buff, dt: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>) {
    let remaining = i32_at(b, 8);
    match b[0] {
        1 => {
            // 0x0053e1a1
            if let Some(e) = entities.get_mut(&id) {
                if i32_at(&e.0, 0x11c) > 0 {
                    wi32(&mut e.0, 0x11c, 0);
                }
            }
        }
        4 => {
            // 0x0053e0e9: the server's (`world+0xb4 == 0`).
            if world.is_client {
                return;
            }
            if remaining.wrapping_add(dt) / 200 == remaining / 200 {
                return;
            }
            let src_id = i64_at(b, 0x10);
            let src = if entities.contains_key(&src_id) { Some(src_id) } else { None };
            // 0x0053e19a: creatureAttack(c, src, value, 0, 0, 0.0, &c[+0x1320], &(0,0,0),
            // out, &dirty, 1, 0, 0, 0); the result is ignored.
            let _ = crate::combat_ai::creature_attack(world, entities, states, id, src, f32_at(b, 4), false, false, 0.0, 0, [0.0; 3], out, dirty, false, true, 0, 0, false);
        }
        _ => {}
    }
}

