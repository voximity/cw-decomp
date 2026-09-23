//! The rest of `World::tick`'s pass over the zones around players and the creature map before
//! the spawns (`Server.exe 0x005322d0`, 0x00532d13 and 0x00533887..0x00535702), transcribed
//! in `analysis/notes/functions/005322d0_pseudo_533b5c.md`:
//!
//! - the type-0x10 ground items that snap to a neighbouring wall (0x00532d13, called from the
//!   items pass in [`crate::interact::items_pass`]);
//! - the periodic effects of open kind-6..8 statics, the traps (0x00533887, called from
//!   [`crate::interact::statics_pass`]);
//! - the villagers' schedule positions and the spawns' kill countdown (0x00533c7b);
//! - the despawn of creatures far from every player (0x00533dc3..0x005341d9);
//! - the players' pets made from the pet item (0x005341d9..0x005347c7, named worlds only);
//! - pets whose owner has another pet (0x005347c7..0x00534911);
//! - dead creatures: the placed bomb's explosion (type 0x90), the pet's revival after 20 s and
//!   the removal of other dead creatures after 1 s (0x00534911..0x005354d9).
//!
//! The airships' removal (0x005354d9..0x00535702, `world+0xc`) is not ported (no airships).
//! Every `rand()` draws from the tick thread's stream, which the caller has swapped into
//! [`World::rng`].

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments, clippy::assign_op_pattern, clippy::needless_range_loop)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{BlockAction, Particle, Sound};
use cw_world::World;
use cw_world::appearance::Appearance;
use cw_world::zone::{GroundItem, Spawn, SpawnAi, Static};

use crate::combat::CreatureState;
use crate::combat_ai::creature_attack;
use crate::modes::{base_damage, clear_block, drop_unsupported_props, tame};
use crate::stats::{max_hp, pow};
use crate::util::{blocks, block_fix, diff_blocks, fix, floor_div_fix, i32_at, i64_at, is_enemy, len_sq3, normalize3, pos_at, set_pos, solid, to_block, u16_at, w16, w32, w64, wf32, f32_at};

/// The equipment slot of the pet item (`entity+0x1010`, `creature+0x1020`).
const PET_SLOT: usize = 0x2f0 + 12 * 0x118;

/// The creature fields this module keeps outside the entity block.
#[derive(Debug, Clone)]
pub struct CreatureExtra {
    /// `creature+0x1d2c`: the spawn's activation radius (`spawn+0x08`, copied at 0x00535f1b;
    /// 200.0 from the constructor 0x00406400); the despawn distance is this plus 20 blocks.
    pub spawn_radius: f32,
    /// `creature+0x1d44`: a player's pet-master skill (`entity+0x1128`) as the pet was last made.
    pub pet_skill: i32,
    /// `creature+0x1d48`: a player's pet item (0x118 bytes) as the pet was last made (`grantXp`
    /// 0x004d61c0 refreshes it with the pet's new XP and level).
    pub pet_item: Vec<u8>,
}

impl Default for CreatureExtra {
    fn default() -> Self {
        CreatureExtra { spawn_radius: 200.0, pet_skill: 0, pet_item: vec![0; 0x118] }
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers.

/// `vec3i64::lengthSq` in fixed point (0x00406260): per axis the 64-bit product divided by
/// 65536, summed. (Duplicated from interact.rs.)
fn fixed_len_sq(v: [i64; 3]) -> i64 {
    v.iter().fold(0i64, |acc, &c| acc.wrapping_add(c.wrapping_mul(c) / 65536))
}

/// `vec3f::fromFixed(a - b)` squared (0x00402c50, 0x00402550, 0x004021b0).
fn dist_sq(a: [i64; 3], b: [i64; 3]) -> f32 {
    len_sq3(diff_blocks(a, b))
}

/// A particle record (0x48 bytes; constructor 0x004c8510: `+0x3c` = 0, `+0x40` = 3.0).
fn particle(pos: [i64; 3], vel: [f32; 3], color: [f32; 4], size: f32, count: u32, p3c: u32) -> Particle {
    let mut p = [0u8; 0x48];
    for (i, v) in pos.iter().enumerate() {
        p[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in vel.iter().enumerate() {
        p[0x18 + i * 4..0x1c + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    for (i, c) in color.iter().enumerate() {
        p[0x24 + i * 4..0x28 + i * 4].copy_from_slice(&c.to_le_bytes());
    }
    p[0x34..0x38].copy_from_slice(&size.to_le_bytes());
    p[0x38..0x3c].copy_from_slice(&count.to_le_bytes());
    p[0x3c..0x40].copy_from_slice(&p3c.to_le_bytes());
    p[0x40..0x44].copy_from_slice(&3.0f32.to_le_bytes());
    Particle(p)
}

/// A sound record (0x18 bytes; constructor 0x004c8530: pitch and volume 1.0).
fn sound(pos: [i64; 3], kind: u32, pitch: f32) -> Sound {
    let mut b = [0u8; 0x18];
    for i in 0..3 {
        b[i * 4..i * 4 + 4].copy_from_slice(&blocks(pos[i]).to_le_bytes());
    }
    b[0xc..0x10].copy_from_slice(&kind.to_le_bytes());
    b[0x10..0x14].copy_from_slice(&pitch.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&1.0f32.to_le_bytes());
    Sound(b)
}

/// `Item::operator==` 0x004078f0 (`!=` is 0x00415b00): type, sub type, modifier (+4), +0xe,
/// +8, rarity (+0xc), level (+0x10), material (+0xd), the spirit count (+0x114) and, for each
/// spirit, its material byte (+3) and its x, y, z bytes (0x004079c0); the spirit levels are not
/// compared. The count is capped at 32 here (the original would read past the item).
pub fn item_eq(a: &[u8], b: &[u8]) -> bool {
    let u32_at = |x: &[u8], o: usize| u32::from_le_bytes(x[o..o + 4].try_into().unwrap());
    if !(a[0] == b[0] && a[1] == b[1] && u32_at(a, 4) == u32_at(b, 4) && a[0xe] == b[0xe] && u32_at(a, 8) == u32_at(b, 8) && a[0xc] == b[0xc] && a[0x10..0x12] == b[0x10..0x12] && a[0xd] == b[0xd]) {
        return false;
    }
    let n = u32_at(a, 0x114) as i32;
    if n != u32_at(b, 0x114) as i32 {
        return false;
    }
    for i in 0..n.clamp(0, 32) as usize {
        let o = 0x14 + i * 8;
        if a[o + 3] != b[o + 3] || a[o..o + 3] != b[o..o + 3] {
            return false;
        }
    }
    true
}

/// `EntityData::reset` 0x00411360 on an entity block: level 1, XP 0, velocities and
/// accelerations 0, multipliers (100, 1, 1) at 0x168/0x16c/0x170 (0x174 and 0x178 kept), mode
/// and its time 0, stun -3000, the timers 0, the equipment zeroed, flags 0, the combat fields
/// 0, the parent id 0, home zone (-1, -1, 0), class 0, the default appearance (its byte 5 is
/// not copied by 0x00407730), name and skills zeroed. Position, rotation, hostile type, type
/// and HP are kept.
pub fn reset_entity(e: &mut EntityData) {
    let b = &mut e.0;
    w32(b, 0x180, 1);
    w32(b, 0x184, 0);
    for o in (0x24..0x48).step_by(4) {
        w32(b, o, 0);
    }
    wf32(b, 0x168, 100.0);
    wf32(b, 0x170, 1.0);
    wf32(b, 0x16c, 1.0);
    b[0x58] = 0;
    w32(b, 0x5c, 0);
    w32(b, 0x11c, 0xffff_f448);
    for o in [0x120, 0x124, 0x128, 0x12c] {
        w32(b, o, 0);
    }
    b[0x2f0..0x2f0 + 0xe38].fill(0);
    w16(b, 0x114, 0);
    w16(b, 0x17c, 0);
    for o in [0x60, 0x64, 0x134, 0x160, 0x164] {
        w32(b, o, 0);
    }
    w64(b, 0x188, 0);
    w32(b, 0x190, 0);
    w32(b, 0x194, 0);
    b[0x198] = 0;
    for o in (0x138..0x150).step_by(4) {
        w32(b, o, 0);
    }
    w32(b, 0x1a0, 0xffff_ffff);
    w32(b, 0x1a4, 0xffff_ffff);
    w32(b, 0x1a8, 0);
    w32(b, 0x1cc, 0xffff_ffff);
    w32(b, 0x1d0, 0xffff_ffff);
    w32(b, 0x1d4, 0);
    b[0x1c8] = 0;
    w16(b, 0x130, 0);
    // 0x00406970 then 0x00407730: the default appearance, byte 5 left as it was.
    let a = Appearance::NEW.to_bytes();
    let keep5 = b[0x68 + 5];
    b[0x68..0x68 + Appearance::SIZE].copy_from_slice(&a[..Appearance::SIZE]);
    b[0x68 + 5] = keep5;
    b[0x1158..0x1168].fill(0);
    b[0x1128..0x1154].fill(0);
    w32(b, 0x1154, 0);
}

/// The creature part of `Creature::reset` 0x004110d0 (after [`reset_entity`]): the behaviour
/// tree goes, the mount, last target and pet ids, the hit-counter copy, the previous roll time,
/// threat, hit map and cooldowns, the hit flash, guard 0, stamina 1, render smoothing, the
/// ground z, the charge, the free-cast flag, the inventory pages, the path and the buffs.
/// (`+0x1d3c` = 1, `+0x1d40`, `+0x13c8..+0x13d0`, `+0x11d8`, `+0x1404` and the walk-cycle
/// fields have no counterpart here.)
pub fn reset_state(st: &mut CreatureState) {
    st.ai_root = None;
    st.modes.mount = 0;
    st.last_target = 0;
    st.pet = 0;
    st.modes.hit_count_copy = 0;
    st.prev_roll = 0;
    st.threat.clear();
    st.hits_landed.clear();
    st.cooldowns.clear();
    st.ai.f13e0 = 0;
    st.ai.hit_flash = 0.0;
    st.block = 0.0;
    st.stamina = 1.0;
    st.riding.smoothing = 0;
    st.ground_z = 0.0;
    st.charge = 0.0;
    st.modes.free_cast = 0;
    st.inventory.pages.clear();
    // 0x00405330 and the path fields +0x1478.. (0x004110d0).
    st.clear_path();
    st.buffs.clear();
}

/// Deletes a creature (its destructor through the vtable and the map erase 0x0040a1d0).
fn delete_creature(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64) {
    entities.remove(&id);
    states.remove(&id);
    world.creature_ids.remove(&id);
}

/// The ids of the players (hostile type 0) in creature-map order: the list the tick builds at
/// 0x00532809 (`[ebp-0x2bd4]`).
fn player_ids(entities: &BTreeMap<i64, EntityData>) -> Vec<i64> {
    entities.iter().filter(|(_, e)| e.0[0x50] == 0).map(|(k, _)| *k).collect()
}

// ---------------------------------------------------------------------------------------------
// 0x00532d13: wall items.

/// 0x00532d13..0x00532f83, in the ground-items pass: a type-0x10 item (a wall-mounted item)
/// probes the blocks one block away on -y, +y, -x and +x in that order (`getBlockFixed`
/// 0x00406050, each probe seeing the previous move); against a solid block it moves to 0.2
/// block from that side of its block (`(v >> 16 << 16) + 0.2`, or `((v >> 16) + 1 << 16) -
/// 0.2`) and turns away from it (rotation 180, 0, 90, 270).
pub fn snap_wall_item(world: &World, it: &mut GroundItem) {
    if it.item.item_type != 0x10 {
        return;
    }
    let one = 1i64 << 16;
    // 0x00405640 (`v >> 16`), 0x004cde40 (`n << 16`), 0x00401530 / 0x004014b0 (`± fix(f)`).
    if solid(block_fix(world, [it.x, it.y.wrapping_sub(one), it.z])) {
        it.y = ((it.y >> 16) << 16).wrapping_add(fix(0.2f32));
        it.rotation = 180.0;
    }
    if solid(block_fix(world, [it.x, it.y.wrapping_add(one), it.z])) {
        it.y = (((it.y >> 16) << 16).wrapping_add(one)).wrapping_sub(fix(0.2f32));
        it.rotation = 0.0;
    }
    if solid(block_fix(world, [it.x.wrapping_sub(one), it.y, it.z])) {
        it.x = ((it.x >> 16) << 16).wrapping_add(fix(0.2f32));
        it.rotation = 90.0;
    }
    if solid(block_fix(world, [it.x.wrapping_add(one), it.y, it.z])) {
        it.x = (((it.x >> 16) << 16).wrapping_add(one)).wrapping_sub(fix(0.2f32));
        it.rotation = 270.0;
    }
}

// ---------------------------------------------------------------------------------------------
// 0x00533887: traps.

/// 0x00533887..0x00533b17: an open (`+0x30`) static of kind 6 or 7 acts every 100 ms of play
/// time (`play_ms / 100` differs from `(play_ms + dt) / 100`, `play_ms` being the world's
/// millisecond counter `world+0x8000bc` already advanced this tick); an open kind 8 acts when
/// its timer `+0x34` is exactly 0 (the tick it opened). Every creature within 5 blocks (fixed
/// point `lengthSq < 25 << 16`) that is not a monster (hostile 1) is hit without an attacker:
/// knockback `normalize(xy) * 20` with z = 5 (× 2 for kind 8), damage `2^((spawnLevel - 1) *
/// 0.25) * 10` (× 5 for kind 8; `spawnLevel` 0x004d2340 at the static), magic for kind 6
/// (`creatureAttack` 0x004cfd50 with stun factor 0 and `show`).
pub fn static_effect(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, s: &Static, play_ms: i32, dt: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    if s.b30 == 0 {
        return;
    }
    let fire = match s.kind {
        // 0x0053389d: `0x51eb851f` / `sar 6`, both truncating.
        6 | 7 => play_ms.wrapping_add(dt) / 100 != play_ms / 100,
        8 => s.f34 == 0,
        _ => false,
    };
    if !fire {
        return;
    }
    let spos = [s.x, s.y, s.z];
    let ids: Vec<i64> = entities.keys().copied().collect();
    for oid in ids {
        let Some(o) = entities.get(&oid) else { continue };
        let opos = pos_at(&o.0);
        // 0x0053393e: 0x00402c50 (s - o), 0x00406260, 0x00402d10.
        let d2 = fixed_len_sq([spos[0].wrapping_sub(opos[0]), spos[1].wrapping_sub(opos[1]), spos[2].wrapping_sub(opos[2])]);
        if !(d2 < (25i64 << 16)) {
            continue;
        }
        if o.0[0x50] == 1 {
            continue;
        }
        // 0x00533984: `vec3f::fromFixed(o - s)`, z cleared, normalised when longer than 0.
        let mut d = diff_blocks(opos, spos);
        d[2] = 0.0;
        if len_sq3(d) > 0.0f32 {
            normalize3(&mut d);
        }
        d = [d[0] * 20.0f32, d[1] * 20.0f32, d[2] * 20.0f32];
        d[2] = 5.0;
        if s.kind == 8 {
            d = [d[0] * 2.0f32, d[1] * 2.0f32, d[2] * 2.0f32];
        }
        // 0x00533a3f: spawnLevel(x, y) - 1, times 0.25, `pow(2, .)` (0x004055a0) times 10.
        let lvl = world.spawn_level(s.x, s.y).wrapping_sub(1);
        let mut dmg = (pow(2.0, f64::from(lvl as f32 * 0.25f32)) as f32) * 10.0f32;
        if s.kind == 8 {
            dmg *= 5.0f32;
        }
        let _ = creature_attack(world, entities, states, oid, None, dmg, false, false, 0.0, 0, d, out, dirty, verbose, s.kind == 6, 0, 0, true);
    }
}

// ---------------------------------------------------------------------------------------------
// 0x00533c7b..0x00535702: the spawns and the creature map.

/// 0x00533c7b..0x00533dc3 (server; the client runs 0x00533bbf instead): for every spawn of
/// the zones around players, the schedule (`spawn+0x10a0`, a villager's) moves the spawn to
/// each entry whose time the time of day has reached (the last such entry wins), and the
/// kill countdown `spawn+0x38` loses `dt`.
pub fn spawn_schedules(world: &mut World, active: &BTreeSet<(i32, i32)>, dt: i32) {
    let tod = world.time_of_day;
    for &(zx, zy) in active {
        let Some(z) = world.zone_mut(zx, zy) else { continue };
        for s in &mut z.spawns {
            update_spawn(s, tod, dt);
        }
    }
}

fn update_spawn(s: &mut Spawn, tod: i32, dt: i32) {
    if let Some(SpawnAi::Villager { schedule }) = &s.ai {
        for e in schedule {
            // 0x00533d25: `cmp tod, [entry+0x18]; jl` skips.
            if tod >= e.time {
                s.x = e.pos[0];
                s.y = e.pos[1];
                s.z = e.pos[2];
            }
        }
    }
    s.f38[0] = (s.f38[0] as i32).wrapping_sub(dt) as u32;
}

/// 0x00533dc3..0x005341d9: a living creature that is neither a player nor a pet is deleted
/// when every player is farther than its spawn radius plus 20 blocks (squared distances; with
/// no player it stays), unless a player holds positive threat on it (a threat entry with a
/// nonzero id of a player) while it is within 512 blocks of its spawn position
/// (`entity+0x1b0`). A deleted creature leaves every creature's threat map first (the scan
/// sees those removals); the deletions follow the scan.
pub fn despawn(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) {
    let players: Vec<[i64; 3]> = player_ids(entities).iter().map(|k| pos_at(&entities[k].0)).collect();
    let mut gone = Vec::new();
    let ids: Vec<i64> = entities.keys().copied().collect();
    for id in ids {
        let o = &entities[&id];
        let h = o.0[0x50];
        if h == 0 || h == 5 {
            continue;
        }
        // 0x00533e3b: comiss/jbe, NaN skips.
        if !(f32_at(&o.0, 0x15c) > 0.0f32) {
            continue;
        }
        let pos = pos_at(&o.0);
        let home = [i64_at(&o.0, 0x1b0), i64_at(&o.0, 0x1b8), i64_at(&o.0, 0x1c0)];
        let mut kept = false;
        if let Some(st) = states.get(&id) {
            for (tid, thr) in &st.threat {
                if *tid != 0
                    && let Some(t) = entities.get(tid)
                    && t.0[0x50] == 0
                    && *thr > 0.0f32
                    && 262_144.0f32 > dist_sq(pos, home)
                {
                    kept = true;
                    break;
                }
            }
        }
        if kept {
            continue;
        }
        // 0x00533f56: the nearest player, starting from -1.
        let mut min = -1.0f32;
        for p in &players {
            let d = dist_sq(pos, *p);
            if 0.0f32 > min || min > d {
                min = d;
            }
        }
        let r = states.get(&id).map_or(200.0, |s| s.extra.spawn_radius) + 20.0f32;
        if min > r * r {
            gone.push(id);
            // 0x00534073: erased from every creature's threat map (0x00530560).
            for st in states.values_mut() {
                st.threat.remove(&id);
            }
        }
    }
    // 0x00534136
    for id in gone {
        delete_creature(world, entities, states, id);
    }
}

/// Puts a player's pet item into its pet (0x005343b4..0x00534478 and 0x005345f9..0x005346bc
/// after the reset): type from the item's sub type, level from its i16 at +0x10, XP from +4,
/// the name from the spirits' material bytes (when fewer than 16), the owner, then
/// `Creature::initAppearance(&type, &appearance, NULL)` 0x0040a840 over the default appearance
/// and `World::tame(owner, pet)` 0x00522580. `new_pet` also sets hostile 5, the owner's
/// position and `maxHp` before the name.
fn dress_pet(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, owner: i64, pet: i64, item: &[u8], new_pet: bool) {
    let owner_pos = pos_at(&entities[&owner].0);
    let p = entities.get_mut(&pet).expect("pet");
    if new_pet {
        w64(&mut p.0, 0x188, owner);
        p.0[0x50] = 5;
    }
    w32(&mut p.0, 0x54, u32::from(item[1]));
    w32(&mut p.0, 0x180, i32::from(i16::from_le_bytes([item[0x10], item[0x11]])) as u32);
    w32(&mut p.0, 0x184, u32::from_le_bytes(item[4..8].try_into().unwrap()));
    if new_pet {
        set_pos(&mut p.0, owner_pos);
        let hp = max_hp(p);
        wf32(&mut p.0, 0x15c, hp);
    }
    // 0x005343e7: the name from the spirits (`item+0x17 + i * 8`), NUL-terminated, only for
    // a count below 16 (a negative count would write before the name in the original; here
    // nothing is written).
    let n = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap());
    if (0..16).contains(&n) {
        for i in 0..n as usize {
            p.0[0x1158 + i] = item[0x17 + i * 8];
        }
        p.0[0x1158 + n as usize] = 0;
    }
    if !new_pet {
        w64(&mut p.0, 0x188, owner);
        // 0x00534446: the appearance back to the defaults (0x00406970 / 0x00407730).
        let a = Appearance::NEW.to_bytes();
        let keep5 = p.0[0x68 + 5];
        p.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&a[..Appearance::SIZE]);
        p.0[0x68 + 5] = keep5;
    }
    // The appearance is the default one on both paths (the reset put it there), with its
    // byte 5 as it was.
    let ty = i32_at(&p.0, 0x54);
    let appearance = Appearance { b5: p.0[0x68 + 5], ..Appearance::NEW };
    let mut sp = Spawn { entity_type: ty, appearance, ..Spawn::NEW };
    world.init_appearance(&mut sp);
    let p = entities.get_mut(&pet).expect("pet");
    p.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&sp.appearance.to_bytes()[..Appearance::SIZE]);
    tame(entities, states, owner, pet);
}

/// 0x005341d9..0x005347c7 (named worlds only, `world+0xa4`): the players' pets. A player whose
/// pet slot (12) holds a pet item (type 0x13) keeps its pet (`creature+0x11c8`); a lost one is
/// looked up among the pets (hostile 5) whose owner is the player. When the pet-master skill
/// or the item changed since the pet was last made (`+0x1d44`, `+0x1d48`) the pet is reset and
/// made again from the item; with no pet and not gliding, a new creature (id one below the
/// lowest id, and below 0) becomes the pet. Without a pet item the pet is dropped (its HP 0)
/// unless it carries a type-7 buff (being fed).
pub fn player_pets(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) {
    // 0x005341df: `sub_419f80(world+0x94)` is `world+0xa4 == 0`.
    if !world.has_name {
        return;
    }
    for pid in player_ids(entities) {
        let Some(p) = entities.get(&pid) else { continue };
        let item: Vec<u8> = p.0[PET_SLOT..PET_SLOT + 0x118].to_vec();
        let skill0 = i32_at(&p.0, 0x1128);
        let flags = u16_at(&p.0, 0x114);
        if item[0] != 0x13 {
            // 0x005346c6
            let pet = states.get(&pid).map_or(0, |s| s.pet);
            let found = entities.contains_key(&pet);
            if found && states.get(&pet).is_some_and(|s| s.buffs.iter().any(|b| b[0] == 7)) {
                continue;
            }
            states.entry(pid).or_default().pet = 0;
            if found && let Some(e) = entities.get_mut(&pet) {
                wf32(&mut e.0, 0x15c, 0.0);
            }
            continue;
        }
        // 0x00534252
        let mut pet = states.entry(pid).or_default().pet;
        if pet == 0 || !entities.contains_key(&pet) {
            pet = 0;
            for (k, o) in entities.iter() {
                if o.0[0x50] == 5 && i64_at(&o.0, 0x188) == pid {
                    pet = *k;
                    break;
                }
            }
            states.entry(pid).or_default().pet = pet;
        }
        // 0x00534346
        let mut updated = false;
        if pet != 0 && entities.contains_key(&pet) {
            let ex = &states.entry(pid).or_default().extra;
            if ex.pet_skill != skill0 || !item_eq(&item, &ex.pet_item) {
                updated = true;
                reset_entity(entities.get_mut(&pet).expect("pet"));
                reset_state(states.entry(pet).or_default());
                dress_pet(world, entities, states, pid, pet, &item, false);
            }
        }
        // 0x0053447d: the skill and item as the pet now is.
        {
            let ex = &mut states.entry(pid).or_default().extra;
            ex.pet_skill = skill0;
            ex.pet_item.copy_from_slice(&item);
        }
        if updated || states.entry(pid).or_default().pet != 0 || flags & 0x10 != 0 {
            continue;
        }
        // 0x005344cd: a new pet, id one below the lowest (at most -1).
        let min = entities.keys().fold(0i64, |m, &k| if k < m { k } else { m });
        let id = min.wrapping_sub(1);
        states.entry(pid).or_default().pet = id;
        // `Creature::Creature(&id)` 0x00406400, then `Creature::reset` 0x004110d0.
        let mut e = EntityData::constructed();
        reset_entity(&mut e);
        entities.insert(id, e);
        states.insert(id, CreatureState::default());
        world.creature_ids.insert(id);
        dress_pet(world, entities, states, pid, id, &item, true);
    }
}

/// 0x005347c7..0x00534911: a pet (hostile 5) whose owner exists but has another pet is reset
/// and left for dead: HP 0, hostile 3, mode time 5000, no owner (the dead-creature pass below
/// deletes it in the same tick). Before that, for the local player (`world+0xb8`, NULL on a
/// server; 0x00534818..0x0053486f) holding a pet item (slot 12 type 0x13) whose pet
/// (`creature+0x11c8`) exists, the item takes the pet's level (the i16 at item+0x10) and XP
/// (item+4).
pub fn orphan_pets(world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) {
    let ids: Vec<i64> = entities.keys().copied().collect();
    for id in ids {
        // 0x00534818: `findEntity(o+0x11c8)`, then `o == world+0xb8 && pet`.
        let pet_id = states.get(&id).map_or(0, |s| s.pet);
        if world.local_player == Some(id)
            && let Some(pet) = entities.get(&pet_id)
        {
            let (level, xp) = (u16_at(&pet.0, 0x180), i32_at(&pet.0, 0x184));
            let it = 0x2f0 + 12 * 0x118;
            let o = entities.get_mut(&id).expect("creature");
            if o.0[it] == 0x13 {
                w16(&mut o.0, it + 0x10, level);
                w32(&mut o.0, it + 4, xp as u32);
            }
        }
        let o = &entities[&id];
        if o.0[0x50] != 5 {
            continue;
        }
        let owner = i64_at(&o.0, 0x188);
        if !entities.contains_key(&owner) || states.get(&owner).map_or(0, |s| s.pet) == id {
            continue;
        }
        let e = entities.get_mut(&id).expect("pet");
        reset_entity(e);
        wf32(&mut e.0, 0x15c, 0.0);
        e.0[0x50] = 3;
        w32(&mut e.0, 0x5c, 5000);
        w64(&mut e.0, 0x188, 0);
        reset_state(states.entry(id).or_default());
    }
}

/// 0x005349a5..0x00535266: a dead type-0x90 creature (the placed bomb) explodes on the first
/// tick after its death (mode time 0): a particle burst, every block with flag bit 0x20
/// (0x005306c0) within 8 blocks breaks (a particle in its colour, `World::clearBlock`
/// 0x00530470, a BlockAction record), then, when any broke, a sound 1 and the light pass and
/// the unsupported props (server); and every enemy creature (`isEnemy` 0x004d18c0) that is not
/// the bomb's mount, alive, not rolling and not yet in its hit set, within `(scale.x * 0.7 +
/// 8)` blocks horizontally (fixed point) and `scale.z * 0.5 + bomb.scale.z + 8` vertically,
/// takes `baseDamage * 25` (doubled on `rand() % 4 == 0`, a critical) with knockback
/// `normalize(xy) * 10` and z 2.5, magic, strong, through `creatureAttack` 0x004cfd50.
fn explode(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    let c = entities[&id].clone();
    let cpos = pos_at(&c.0);
    // 0x005349a5: the burst. The colour is a vec3 copied as 16 bytes (0x0041cb60): its fourth
    // float is stack garbage in the original; 1.0 here.
    out.particles.push(particle(cpos, [0.0, 0.0, 20.0], [1.0, 0.2, 0.5, 1.0], 0.25, 20, 1));
    // 0x00534a5a: dx outer, dy, dz inner, each -8..=8.
    let mut destroyed = false;
    for dx in -8i32..=8 {
        for dy in -8i32..=8 {
            for dz in -8i32..=8 {
                // 0x004d99d0: the block offset `<< 16`; 0x00402cb0.
                let p = [cpos[0].wrapping_add(i64::from(dx) << 16), cpos[1].wrapping_add(i64::from(dy) << 16), cpos[2].wrapping_add(i64::from(dz) << 16)];
                if !(64.0f32 > dist_sq(p, cpos)) {
                    continue;
                }
                let blk = block_fix(world, p);
                if (blk[3] >> 5) & 1 == 0 {
                    continue;
                }
                // 0x00534b65: a particle in the block's colour.
                let color = [f32::from(blk[0]) / 255.0f32, f32::from(blk[1]) / 255.0f32, f32::from(blk[2]) / 255.0f32, 1.0];
                out.particles.push(particle(p, [0.0, 0.0, 10.0], color, 0.5, 3, 0));
                // 0x00534c5e: z through 0x00405510, y and x through 0x00405640 (`>> 16`).
                clear_block(world, (p[0] >> 16) as i32, (p[1] >> 16) as i32, floor_div_fix(p[2]));
                // 0x00534c92: BlockAction at `vec3i64ToBlock` 0x00405450, block 0, day.
                let day = world.day;
                out.block_actions.push(BlockAction { x: to_block(p[0]), y: to_block(p[1]), z: to_block(p[2]), block: [0, 0, 0, 0], time: day });
                destroyed = true;
            }
        }
    }
    if destroyed {
        // 0x00534d4a
        let pitch = world.rng.rand() as f32 * 0.4f32 / 32767.0f32 + 0.5f32;
        out.sounds.push(sound(cpos, 1, pitch));
        // 0x00534da4 (world+0xb8 == NULL, so not with a local player): `computeLighting(x -
        // 8, y - 8, x + 8, y + 8, 8, NULL)` 0x004d1a70, then (under the world lock) 0x004d9160
        // over the same range.
        let eight = 8i64 << 16;
        let (x0, y0) = ((cpos[0].wrapping_sub(eight) >> 16) as i32, (cpos[1].wrapping_sub(eight) >> 16) as i32);
        let (x1, y1) = ((cpos[0].wrapping_add(eight) >> 16) as i32, (cpos[1].wrapping_add(eight) >> 16) as i32);
        if world.local_player.is_none() {
            compute_lighting(world, x0, y0, x1, y1, 8);
        }
        drop_unsupported_props(world, x0, y0, x1, y1);
    }
    // 0x00534ed5: the creatures around.
    let mount = states.get(&id).map_or(0, |s| s.modes.mount);
    let ids: Vec<i64> = entities.keys().copied().collect();
    for oid in ids {
        if oid == id {
            continue;
        }
        let Some(o) = entities.get(&oid) else { continue };
        let Some(c) = entities.get(&id) else { return };
        if !is_enemy(c, o) {
            continue;
        }
        if oid == mount {
            continue;
        }
        // 0x00534f88: comiss 0, hp; jae skips (NaN goes on).
        if 0.0f32 >= f32_at(&o.0, 0x15c) {
            continue;
        }
        if i32_at(&o.0, 0x118) != 0 {
            continue;
        }
        if states.get(&id).is_some_and(|s| s.modes.hit_set.contains(&oid)) {
            continue;
        }
        let opos = pos_at(&o.0);
        let oscale = [f32_at(&o.0, 0x70), f32_at(&o.0, 0x74), f32_at(&o.0, 0x78)];
        // 0x00534fe5: `lengthSq2Fixed(o - c)` (0x0041ce90) < `fix(r²)` (0x004dade0).
        let r = oscale[0] * 0.7f32 + 8.0f32;
        let r2 = r * r;
        let dxy = [opos[0].wrapping_sub(cpos[0]), opos[1].wrapping_sub(cpos[1])];
        let l2 = (dxy[0].wrapping_mul(dxy[0]) / 65536).wrapping_add(dxy[1].wrapping_mul(dxy[1]) / 65536);
        if !(l2 < fix(r2)) {
            continue;
        }
        // 0x00535064: |c.z - o.z| in blocks (0x00401490, 0x00401420, fabs 0x00401ca0).
        let dz = blocks(cpos[2].wrapping_sub(opos[2])).abs();
        let cz = f32_at(&c.0, 0x78);
        if !((oscale[2] * 0.5f32 + cz) + 8.0f32 > dz) {
            continue;
        }
        // 0x005350e6
        let mut dir = diff_blocks(opos, cpos);
        dir[2] = 0.0;
        if len_sq3(dir) > 0.01f32 {
            normalize3(&mut dir);
        }
        dir[2] = 0.25;
        // 0x0053514d: signed `rand() % 4 == 0`.
        let crit = world.rng.rand() % 4 == 0;
        dir = [dir[0] * 10.0f32, dir[1] * 10.0f32, dir[2] * 10.0f32];
        let mut dmg = base_damage(c) * 25.0f32;
        if crit {
            dmg *= 2.0f32;
        }
        states.entry(id).or_default().modes.hit_set.insert(oid);
        let _ = creature_attack(world, entities, states, oid, Some(id), dmg, crit, true, 0.0, 0, dir, out, dirty, verbose, true, 0, 0, true);
    }
}

/// `World::computeLighting(x0, y0, x1, y1, margin, NULL)` 0x004d1a70 as modes.rs ports it
/// (duplicated from modes.rs, which keeps its copy private): the light pass of each zone of
/// the range, one zone at a time.
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

/// 0x00534911..0x005354d9: every dead creature (`0 >= HP`, NaN skipped) that is not a player:
/// a type-0x90 one with mode time 0 explodes ([`explode`]); the mode time then counts `dt`. A
/// pet whose owner exists and is not dead (`HP >= 0`) revives after 20 s at its owner with full
/// HP, no last target and no threat (the owner's hit map clears); any other dead creature is
/// deleted after 1 s, leaving every creature's threat and hit maps first.
pub fn dead_creatures(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, dt: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    let mut gone = Vec::new();
    let ids: Vec<i64> = entities.keys().copied().collect();
    for id in ids {
        let Some(o) = entities.get(&id) else { continue };
        // 0x00534974: comiss 0, hp; jb skips a living one (and NaN).
        if !(0.0f32 >= f32_at(&o.0, 0x15c)) {
            continue;
        }
        if o.0[0x50] == 0 {
            continue;
        }
        if i32_at(&o.0, 0x5c) == 0 && i32_at(&o.0, 0x54) == 0x90 {
            explode(world, entities, states, id, out, dirty, verbose);
        }
        // 0x00535272
        let Some(o) = entities.get_mut(&id) else { continue };
        let mt = i32_at(&o.0, 0x5c).wrapping_add(dt);
        w32(&mut o.0, 0x5c, mt as u32);
        if o.0[0x50] == 5 {
            let owner = i64_at(&o.0, 0x188);
            let Some(ow) = entities.get(&owner) else { continue };
            if mt < 20000 {
                continue;
            }
            // 0x005352b8: comiss hp, 0; jb skips (NaN too).
            if !(f32_at(&ow.0, 0x15c) >= 0.0f32) {
                continue;
            }
            let opos = pos_at(&ow.0);
            let o = entities.get_mut(&id).expect("pet");
            let hp = max_hp(o);
            wf32(&mut o.0, 0x15c, hp);
            set_pos(&mut o.0, opos);
            let st = states.entry(id).or_default();
            st.last_target = 0;
            st.threat.clear();
            states.entry(owner).or_default().hits_landed.clear();
        } else if mt >= 1000 {
            gone.push(id);
            // 0x0053534c: erased from every creature's threat and hit maps.
            for st in states.values_mut() {
                st.threat.remove(&id);
                st.hits_landed.remove(&id);
            }
        }
    }
    // 0x0053542f
    for id in gone {
        delete_creature(world, entities, states, id);
    }
}

/// 0x00533b5c..0x00535702 on a server, after the zones' items and statics: the spawns'
/// schedules and countdowns, the despawn, the players' pets, the orphaned pets and the dead
/// creatures, in the original's order. `active` is the zone set around players. In the
/// client's world (`world+0xb4`, 0x00533bab) all of it is skipped and only the pets' owners
/// are refreshed ([`client_pet_owners`]).
pub fn creature_pass(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, active: &BTreeSet<(i32, i32)>, dt: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    if world.is_client {
        client_pet_owners(entities, states);
        if world.fix_pet_leveling {
            sync_local_pet_item(world, entities, states);
        }
        return;
    }
    spawn_schedules(world, active, dt);
    despawn(world, entities, states);
    player_pets(world, entities, states);
    orphan_pets(world, entities, states);
    dead_creatures(world, entities, states, dt, out, dirty, verbose);
    // 0x005354d9..0x00535702: airships (`world+0xc`) with no player within 512 blocks are
    // deleted: not ported (no airships).
}

/// Not in the original (see `World::fix_pet_leveling`): the write-back of 0x00534818, which
/// the server's `orphan_pets` does for the local player, run on a client for its own player:
/// the pet item in slot 12 (type 0x13) takes the pet's level (item+0x10, i16) and XP (item+4).
pub fn sync_local_pet_item(world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>) {
    let Some(id) = world.local_player else { return };
    let pet_id = states.get(&id).map_or(0, |s| s.pet);
    let Some(pet) = entities.get(&pet_id) else { return };
    let (level, xp) = (u16_at(&pet.0, 0x180), i32_at(&pet.0, 0x184));
    let it = 0x2f0 + 12 * 0x118;
    let Some(o) = entities.get_mut(&id) else { return };
    if o.0[it] == 0x13 {
        w16(&mut o.0, it + 0x10, level);
        w32(&mut o.0, it + 4, xp as u32);
    }
}

/// 0x00533bbf..0x00533c76, the client's replacement for the creature pass (`world+0xb4 != 0`):
/// every pet (hostile 5) whose owner (`entity+0x188`) exists becomes that owner's pet
/// (`creature+0x11c8`), in creature-map order; then the tick goes on at the spawns
/// (0x00535702, server only as well).
pub fn client_pet_owners(entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) {
    for (id, c) in entities {
        if c.0[0x50] != 5 {
            continue;
        }
        let owner = i64_at(&c.0, 0x188);
        if entities.contains_key(&owner) {
            states.entry(owner).or_default().pet = *id;
        }
    }
}
