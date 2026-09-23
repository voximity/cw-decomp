//! The consumable modes and the sneaking guard of `World::tick`
//! (`Server.exe 0x005322d0`): eating and drinking (modes 0x50/0x51, 0x0053e6bd..0x0053e9a3,
//! every non-player on a server) and the exposure of a sneaking creature (mode 0x4f,
//! 0x0053f37a..0x0053fa2a), which sets its guard (`creature+0x1190`). Transcribed in
//! `analysis/notes/functions/005322d0_pseudo_53e1fc.md`; the drink's end (0x0053ffaa) is in
//! physics.rs.

// The comparisons keep the original's NaN behaviour and the sums its operand order.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::assign_op_pattern, clippy::needless_range_loop)]

use std::collections::BTreeSet;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Hit, Sound};
use cw_world::World;

use crate::combat::CreatureState;
use crate::skills::skill_level_factor;
use crate::stats::{item_stat, max_hp, pow};
use crate::util::{blocks, f32_at, i32_at, length3, pos_at, u16_at, vec3f_at, w64, wf32};

/// `consumableDuration` 0x00413aa0: type 1 items last 3000 ms (sub type 1) or 10000 ms; others 0.
fn consumable_duration(item: &[u8]) -> i32 {
    if item[0] != 1 {
        0
    } else if item[1] == 1 {
        3000
    } else {
        10000
    }
}

/// `Item::manaAmount` 0x00414200: type-1 sub types 4 and 6 restore `itemStat(level, rarity) *
/// 1.5` (0x00410f90 with the item's i16 level at +0x10 and its rarity at +0xc); others 0.
fn mana_amount(item: &[u8]) -> f32 {
    if item[0] == 1 && (item[1] == 4 || item[1] == 6) {
        let level = i16::from_le_bytes([item[0x10], item[0x11]]);
        item_stat(f32::from(level), i32::from(item[0xc])) * 1.5f32
    } else {
        0.0
    }
}

/// A sound record (0x18 bytes, constructor 0x004c8530: pitch and volume 1.0).
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

/// 0x0053e6bd..0x0053e9a3: the consumable modes. On a server (`world+0xb4 == 0`) every
/// non-player (hostile type != 0) runs them; players never do. In mode 0x50 (eat) or 0x51 (drink), with `prev = modeTime - dt`:
/// every 200 ms of the item's duration (`prev < duration` and `prev / 200 != now / 200`, both
/// truncating) a share `rate = 200 / duration` (halved for non-players) of the item's heal
/// (`Item::healAmount` 0x00413be0) goes to the HP (a negative-damage Hit from attacker -1,
/// capped at `maxHp`) and a share of its mana amount scaled by `2^(level * 0.25)` to the
/// accumulator at `entity+0x12c`, which resets to 0 (with sound 0x2f when it crosses zero
/// upward) once it is not negative, and every 400 ms a sound 0x2c. `e` is the creature's
/// entity block, `id` its id. In the client's world (`world+0xb4`) only the local player
/// (`world+0xb8`) runs them.
pub fn consumable_modes(world: &World, e: &mut EntityData, id: i64, dt: i32, out: &mut ServerUpdate) {
    // 0x0053e6bd: `cmp byte [world+0xb4], 0; jne 0x53e6d2; cmp byte [c+0x60], 0; jne
    // 0x53e6de; 0x53e6d2: cmp c, [world+0xb8]; jne skip`: with +0xb4 clear a non-player goes
    // on, a player only when it is the local player (+0xb8, NULL on a server). So on a server
    // every creature but the players runs it (the villagers' and monsters' potions of
    // `CombatBehavior` 0x00402fec/0x0040329c; observed on Server.exe as attacker -1 heals);
    // on the client only the local player.
    if !((!world.is_client && e.0[0x50] != 0) || world.local_player == Some(id)) {
        return;
    }
    let mode = e.0[0x58];
    if mode != 0x50 && mode != 0x51 {
        return;
    }
    let item: Vec<u8> = e.0[0x1d8..0x1d8 + 0x118].to_vec();
    let now = i32_at(&e.0, 0x5c);
    let prev = now.wrapping_sub(dt);
    let pos = pos_at(&e.0);
    if prev < consumable_duration(&item) && prev / 200 != now / 200 {
        // 0x0053e741: `200.0f / (float)duration` (cvtdq2ps).
        let mut rate = 200.0f32 / consumable_duration(&item) as f32;
        if e.0[0x50] != 0 {
            rate *= 0.5f32;
        }
        let heal = crate::interact::heal_amount(&item) * rate;
        // 0x0053e79c: NaN skips.
        if heal > 0.0f32 {
            // Hit constructor 0x00422a90 zeroes +0x14 and +0x18..+0x46.
            let mut h = [0u8; 0x48];
            w64(&mut h, 0, -1);
            w64(&mut h, 8, id);
            h[0x10..0x14].copy_from_slice(&(-heal).to_le_bytes());
            for (i, p) in pos.iter().enumerate() {
                w64(&mut h, 0x20 + i * 8, *p);
            }
            out.hits.push(Hit(h));
            let hp = heal + f32_at(&e.0, 0x15c);
            wf32(&mut e.0, 0x15c, hp);
            // 0x0053e83b: `maxHp` (0x0040fda0) spilled to f32.
            let max = max_hp(e);
            if f32_at(&e.0, 0x15c) > max {
                wf32(&mut e.0, 0x15c, max);
            }
        }
        // 0x0053e864: the mana part.
        let mana = mana_amount(&item);
        let p = pow(2.0, f64::from(i32_at(&e.0, 0x180) as f32 * 0.25f32)) as f32;
        let inc = (mana / p) * rate;
        let acc = inc + f32_at(&e.0, 0x12c);
        wf32(&mut e.0, 0x12c, acc);
        // 0x0053e8c8: jb skips on `acc < 0` and on NaN.
        if !(acc < 0.0f32) {
            if 0.0f32 > acc - inc {
                out.sounds.push(sound(pos, 0x2f, 1.0));
            }
            wf32(&mut e.0, 0x12c, 0.0);
        }
        // 0x0053e921: every 400 ms of the mode a sound 0x2c (inside the 200 ms test: its two
        // failures jump to 0x0053e99d, past this).
        let mt = i32_at(&e.0, 0x5c);
        if mt.wrapping_sub(dt) / 400 != mt / 400 {
            out.sounds.push(sound(pos, 0x2c, 1.0));
        }
    }
}

/// The zones the tick collects around the players at its start (`std::set<Zone*>` at
/// `[ebp-0x2bf0]`, 0x00532815..0x005328d5): the loaded zones of the 3x3 around each player,
/// zone coordinates `(pos / 65536) / 256` (both truncating). The original orders the set by
/// address; coordinate order stands in (as in cw-server).
pub fn player_zones<'a>(world: &World, players: impl Iterator<Item = &'a EntityData>) -> BTreeSet<(i32, i32)> {
    let mut set = BTreeSet::new();
    for p in players {
        let pos = pos_at(&p.0);
        let (zx, zy) = (((pos[0] / 65536) as i32) / 256, ((pos[1] / 65536) as i32) / 256);
        for x in zx - 1..=zx + 1 {
            for y in zy - 1..=zy + 1 {
                if world.zone(x, y).is_some() {
                    set.insert((x, y));
                }
            }
        }
    }
    set
}

/// `World::lightAt` 0x004d5c80: the light at a 16.16 position (each axis divided by 65536,
/// truncating): a type-13 block 255, air or water its first byte (at least 5), anything else
/// 0; over 255.
pub fn light_at(world: &World, pos: [i64; 3]) -> f32 {
    let b = world.block((pos[0] / 65536) as i32, (pos[1] / 65536) as i32, (pos[2] / 65536) as i32);
    let t = b[3] & 0x1f;
    let v = if t == 0xd {
        0xff
    } else if t == 0 || t == 2 {
        b[0].max(5)
    } else {
        0
    };
    f32::from(v) / 255.0f32
}

/// `vec3f::fromFixed(a - b)` squared (0x00402c50, 0x00402550, 0x004021b0).
fn dist_sq(a: [i64; 3], b: [i64; 3]) -> f32 {
    let d = [blocks(a[0].wrapping_sub(b[0])), blocks(a[1].wrapping_sub(b[1])), blocks(a[2].wrapping_sub(b[2]))];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// 0x0053f3d1..0x0053fa2a: the guard of a sneaking creature (mode 0x4f; the speed factor is
/// applied by the caller). The exposure starts at the light at the creature times the darkness
/// `(1 - ((time * 2 / 86400000 - 1)^2))^2`; within 16 blocks of it, each glowing prop (flag 1,
/// `zone+4`), kind-0x32 static and type-0x12 ground item of the zones around the players adds
/// `(1 - d²/256)² * 0.5`; the sum is capped at 1; then every creature carrying flag 0x200
/// within 10 blocks (the creature itself included) adds `(1 - d²/100)²`. The guard moves by
/// `((1 - (0.9 - s * 0.5) * exposure) - (0.5 - s * 0.5) * |vel|) * dt * 5e-4` (`s` the 0x4f
/// skill factor) and is clamped to 0..1. `others` are the other creatures (id order), `flags`
/// the creature's own `entity+0x114` as the movement holds it.
pub fn sneak_guard(world: &World, others: &[(i64, EntityData)], id: i64, flags: u16, e: &EntityData, st: &mut CreatureState, dt_f: f32) {
    let pos = pos_at(&e.0);
    // 0x0053f3d1: `sub_41cae0` is the time of day (`world+0x80015c`).
    let tod = world.time_of_day as f32 * 2.0f32 / 86_400_000.0f32 - 1.0f32;
    let p1 = pow(f64::from(tod), 2.0) as f32;
    let dark = pow(f64::from(1.0f32 - p1), 2.0) as f32;
    let mut exposure = light_at(world, pos) * dark;
    // Performance note for a future optimiser: the zone set is rebuilt per sneaking creature;
    // the original builds it once at the start of the tick.
    let players = others.iter().map(|(_, o)| o).chain(std::iter::once(e)).filter(|o| o.0[0x50] == 0);
    let zones = player_zones(world, players);
    for (zx, zy) in zones {
        let Some(z) = world.zone(zx, zy) else { continue };
        // 0x0053f4a0: the props.
        for p in &z.props {
            if p.flags & 1 != 0 {
                let d = dist_sq([p.x, p.y, p.z], pos);
                if 256.0f32 > d {
                    let k = 1.0f32 - d * 0.00390625f32;
                    exposure = (k * k) * 0.5f32 + exposure;
                }
            }
        }
        // 0x0053f595: the statics of kind 0x32.
        for s in &z.statics {
            if s.kind == 0x32 {
                let d = dist_sq([s.x, s.y, s.z], pos);
                if 256.0f32 > d {
                    let k = 1.0f32 - d * 0.00390625f32;
                    exposure = (k * k) * 0.5f32 + exposure;
                }
            }
        }
        // 0x0053f690: the ground items of type 0x12.
        for it in &z.items {
            if it.item.item_type == 0x12 {
                let d = dist_sq([it.x, it.y, it.z], pos);
                if 256.0f32 > d {
                    let k = 1.0f32 - d * 0.00390625f32;
                    exposure = (k * k) * 0.5f32 + exposure;
                }
            }
        }
    }
    // 0x0053f7cd
    if exposure > 1.0f32 {
        exposure = 1.0;
    }
    // 0x0053f7ea: every creature of the map (this one included) with flag 0x200.
    let mut list: Vec<(i64, u16, [i64; 3])> = others.iter().map(|(k, o)| (*k, u16_at(&o.0, 0x114), pos_at(&o.0))).collect();
    let at = list.partition_point(|(k, _, _)| *k < id);
    list.insert(at, (id, flags, pos));
    for (_, f, opos) in list {
        if f & 0x200 != 0 {
            let d = dist_sq(opos, pos);
            if 100.0f32 > d {
                let k = 1.0f32 - d / 100.0f32;
                exposure = k * k + exposure;
            }
        }
    }
    // 0x0053f8e8
    let s1 = skill_level_factor(e, 0x4f, -1);
    let hide = 1.0f32 - (0.9f32 - s1 * 0.5f32) * exposure;
    let s2 = skill_level_factor(e, 0x4f, -1);
    let m2 = 0.5f32 - s2 * 0.5f32;
    let vlen = length3(vec3f_at(&e.0, 0x24));
    let g = ((hide - m2 * vlen) * (dt_f * 5e-4f32)) + st.block;
    st.block = g;
    if 0.0f32 > g {
        st.block = 0.0;
    }
    // 0x0053fa21
    if st.block > 1.0f32 {
        st.block = 1.0;
    }
}
