//! The world tick's interactions and statics: the statics pass over the zones around players
//! (`World::tick` 0x00533129..0x00533b56), the per-creature switch on the queued Interact
//! packets (0x005362f5..0x00537804, jump table 0x005489e0 on `interact+0x128`), the HP clamp
//! that follows it (0x00537856), and the tail that marks and re-announces the zones whose
//! items or statics changed (0x00546efc..0x0054710b).
//!
//! Every `rand()` here draws from the tick thread's stream, which the caller has swapped into
//! [`World::rng`].

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{GroundItem as GroundItemRecord, Hit, Interact, Pickup, Sound, StaticEntity, ZoneItems};
use cw_world::World;
use cw_world::inventory::Item;
use cw_world::zone::{GroundItem, Spawn, Static};

use crate::stats::{item_power, max_hp};
use crate::util::{f32_at, i32_at, pos_of, u16_at, w32, w64};

/// 2^-16, the fixed-point block scale.
pub const K: f32 = 1.5258789e-05;

fn set_pos(e: &mut EntityData, p: [i64; 3]) {
    for (i, v) in p.iter().enumerate() {
        w64(&mut e.0, i * 8, *v);
    }
}

/// `vec3f` to 16.16 fixed, `__ftol2(f * 65536.0f)` per axis (0x00402510).
fn fixed3(v: [f32; 3]) -> [i64; 3] {
    v.map(|f| (f * 65536.0f32) as i64)
}

/// `vec3i64` difference converted to f32 blocks and squared (0x00402c50, 0x00402550,
/// 0x004021b0): each axis rounded once, `x*x + y*y + z*z`.
pub fn dist_sq(a: [i64; 3], b: [i64; 3]) -> f32 {
    let d: Vec<f32> = (0..3).map(|i| a[i].wrapping_sub(b[i]) as f32 * K).collect();
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `vec3i64::lengthSq` in fixed point (0x00406260, 0x004056c0): per axis the 64-bit product
/// divided by 65536, summed with wrapping adds.
fn fixed_len_sq(v: [i64; 3]) -> i64 {
    v.iter().fold(0i64, |acc, &c| acc.wrapping_add(c.wrapping_mul(c) / 65536))
}

/// A sound record (0x18 bytes, constructor 0x004c8530): position in f32 blocks, kind, pitch,
/// volume.
fn sound(pos: [i64; 3], kind: u32, pitch: f32, volume: f32) -> Sound {
    let mut b = [0u8; 0x18];
    for (i, p) in pos.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&(*p as f32 * K).to_le_bytes());
    }
    w32(&mut b, 0xc, kind);
    b[0x10..0x14].copy_from_slice(&pitch.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&volume.to_le_bytes());
    Sound(b)
}

/// The wire form of a ground item (`cube::GroundItem`, 0x148 bytes).
pub fn ground_item_record(it: &GroundItem) -> GroundItemRecord {
    let mut b = [0u8; 0x148];
    b[..0x118].copy_from_slice(&it.item.to_bytes());
    b[0x118..0x120].copy_from_slice(&it.x.to_le_bytes());
    b[0x120..0x128].copy_from_slice(&it.y.to_le_bytes());
    b[0x128..0x130].copy_from_slice(&it.z.to_le_bytes());
    b[0x130..0x134].copy_from_slice(&it.rotation.to_le_bytes());
    b[0x134..0x138].copy_from_slice(&it.f134.to_le_bytes());
    b[0x138] = it.b138;
    b[0x13c..0x140].copy_from_slice(&it.f13c.to_le_bytes());
    b[0x140..0x144].copy_from_slice(&it.f140.to_le_bytes());
    b[0x144..0x148].copy_from_slice(&it.f144.to_le_bytes());
    GroundItemRecord(b)
}

/// The static record (0x58 bytes, 0x00422d70): the addressing triple and the static's fields.
pub fn static_record(zx: i32, zy: i32, index: i32, s: &Static) -> StaticEntity {
    StaticEntity {
        zone_x: zx,
        zone_y: zy,
        index,
        kind: s.kind,
        x: s.x,
        y: s.y,
        z: s.z,
        rotation: s.rotation,
        scale: [s.scale[0].to_bits(), s.scale[1].to_bits(), s.scale[2].to_bits()],
        b40: s.b30,
        f44: s.f34,
        f48: s.f38,
        user_id: i64::from(s.f40) | (i64::from(s.f44) << 32),
    }
}

/// The zone-items record of a loaded zone (0x00422c00).
pub fn zone_items_record(world: &World, zx: i32, zy: i32) -> Option<ZoneItems> {
    let zone = world.zone(zx, zy)?;
    Some(ZoneItems { zone_x: zone.x, zone_y: zone.y, items: zone.items.iter().map(ground_item_record).collect() })
}

/// The two seats: kinds 0x10 and 0x12 sit (mode 0x53, facing `rotation + 2`), kinds 0x13, 0x44
/// and 0x45 lie (mode 0x54, facing `rotation`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Seat {
    Sit,
    Lie,
}

fn seat_kind(kind: u32) -> Option<Seat> {
    match kind {
        0x10 | 0x12 => Some(Seat::Sit),
        0x13 | 0x44 | 0x45 => Some(Seat::Lie),
        _ => None,
    }
}

/// Puts a player on a seat (0x005331da, 0x005332c0 and the interaction cases 0x00536955,
/// 0x00536a4d): the static's position raised by half the player's z scale plus the static's z
/// scale, velocity cleared, yaw from the static's rotation, and the mode when `set_mode`.
fn seat(e: &mut EntityData, s: &Static, kind: Seat, set_mode: bool) {
    let lift = f32_at(&e.0, 0x78) * 0.5f32 + s.scale[2];
    let off = fixed3([0.0, 0.0, lift]);
    set_pos(e, [s.x.wrapping_add(off[0]), s.y.wrapping_add(off[1]), s.z.wrapping_add(off[2])]);
    e.0[0x24..0x30].fill(0);
    let quarter = match kind {
        Seat::Sit => s.rotation.wrapping_add(2),
        Seat::Lie => s.rotation,
    };
    let yaw = quarter.wrapping_mul(90) as f32;
    e.0[0x20..0x24].copy_from_slice(&yaw.to_le_bytes());
    if set_mode {
        e.0[0x58] = match kind {
            Seat::Sit => 0x53,
            Seat::Lie => 0x54,
        };
    }
}

/// The ground-items pass (0x00532ca2..0x00533097) over one zone around players: every item's
/// `+0x140` and `+0x13c` countdowns lose `dt`, and an item whose `+0x13c` countdown just
/// ran out lands with a sound at its position (kind 0x3a for coins, type 0xc, else 0x3b;
/// pitch `1 + rand * 0.1 / 32767`). A type-0x10 item first snaps to a neighbouring wall
/// (0x00532d13, [`crate::statics::snap_wall_item`]).
pub fn items_pass(world: &mut World, zx: i32, zy: i32, dt: i32, out: &mut ServerUpdate) {
    let n = world.zone(zx, zy).map_or(0, |z| z.items.len());
    for k in 0..n {
        // 0x00532d13
        if let Some(mut g) = world.zone(zx, zy).and_then(|z| z.items.get(k).cloned())
            && g.item.item_type == 0x10
        {
            crate::statics::snap_wall_item(world, &mut g);
            if let Some(z) = world.zone_mut(zx, zy) {
                z.items[k] = g;
            }
        }
        let (pos, kind, expired) = {
            let Some(z) = world.zone_mut(zx, zy) else { return };
            let it = &mut z.items[k];
            if it.f140 > 0 {
                it.f140 -= dt;
                if it.f140 < 0 {
                    it.f140 = 0;
                }
            }
            let mut expired = false;
            if it.f13c > 0 {
                it.f13c -= dt;
                if it.f13c <= 0 {
                    it.f13c = 0;
                    expired = true;
                }
            }
            ([it.x, it.y, it.z], it.item.item_type, expired)
        };
        // 0x00532fdb: the landing sound is the server's (`world+0xb4 == 0`).
        if expired && !world.is_client {
            let pitch = world.rng.rand() as f32 * 0.1f32 / 32767.0f32 + 1.0f32;
            out.sounds.push(sound(pos, 0x3a + u32::from(kind != 0xc), pitch, 1.0));
        }
    }
}

/// The statics pass (0x00533129..0x00533b56) over the zones around players: every static's
/// millisecond timer advances by `dt`; an occupied seat keeps its player in place or releases
/// it (user gone, stunned, more than four blocks away, accelerating, or no longer sitting);
/// kinds 6, 7 and 8 close two seconds after opening and open again when a player is within 30
/// blocks. Records of changed statics go out. Then the open traps act (0x00533887,
/// [`crate::statics::static_effect`]; `play_ms` is the world's millisecond counter
/// `world+0x8000bc`, already advanced by this tick's `dt`).
#[allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments)] // `!(x > y)` also keeps a NaN seated, as the original's `comiss` does
pub fn statics_pass(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, crate::combat::CreatureState>, active: &BTreeSet<(i32, i32)>, dt: i32, play_ms: i32, out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    let players: Vec<[i64; 3]> = entities.values().filter(|e| e.0[0x50] == 0).map(pos_of).collect();
    for &(zx, zy) in active {
        items_pass(world, zx, zy, dt, out);
        let n = world.zone(zx, zy).map_or(0, |z| z.statics.len());
        for k in 0..n {
            let Some(mut s) = world.zone(zx, zy).map(|z| z.statics[k].clone()) else { break };
            s.f34 = s.f34.wrapping_add(dt as u32);
            let user = i64::from(s.f40) | (i64::from(s.f44) << 32);
            if user != 0 {
                let mut release = true;
                if let Some(e) = entities.get_mut(&user)
                    && i32_at(&e.0, 0x11c) <= 0
                {
                    let kind = seat_kind(s.kind);
                    if let Some(kind) = kind {
                        seat(e, &s, kind, true);
                    }
                    let d = dist_sq(pos_of(e), [s.x, s.y, s.z]);
                    let accel = [f32_at(&e.0, 0x30), f32_at(&e.0, 0x34), f32_at(&e.0, 0x38)];
                    let accel_sq = accel[0] * accel[0] + accel[1] * accel[1] + accel[2] * accel[2];
                    if !(d > 16.0f32) && !(accel_sq > 0.0f32) && matches!(e.0[0x58], 0x53 | 0x54) {
                        release = false;
                        if let Some(kind) = kind {
                            seat(e, &s, kind, false);
                        }
                    }
                }
                // 0x005335e8 / 0x00533645: the release (and everything after it for this
                // static) is the server's; the client goes to the next static (0x00533b17).
                if release && world.is_client {
                    if let Some(z) = world.zone_mut(zx, zy) {
                        z.statics[k] = s;
                    }
                    continue;
                }
                if release {
                    s.f40 = 0;
                    s.f44 = 0;
                    out.statics.push(static_record(zx, zy, k as i32, &s));
                }
            }
            // 0x005336b5: the toggles and the traps are the server's.
            if world.is_client {
                if let Some(z) = world.zone_mut(zx, zy) {
                    z.statics[k] = s;
                }
                continue;
            }
            if matches!(s.kind, 6..=8) && (s.f34 as i32) > 2000 {
                let toggle_to = if s.b30 != 0 {
                    Some(0u8)
                } else if players.iter().any(|p| fixed_len_sq([p[0].wrapping_sub(s.x), p[1].wrapping_sub(s.y), p[2].wrapping_sub(s.z)]) < (900i64 << 16)) {
                    Some(1u8)
                } else {
                    None
                };
                if let Some(open) = toggle_to {
                    if let Some(kind) = s.toggle(open) {
                        out.sounds.push(sound([s.x, s.y, s.z], kind, 1.0, 1.0));
                    }
                    out.statics.push(static_record(zx, zy, k as i32, &s));
                }
            }
            let placed = s.clone();
            if let Some(z) = world.zone_mut(zx, zy) {
                z.statics[k] = s;
            }
            // 0x00533887: an open trap's periodic effect (statics.rs).
            crate::statics::static_effect(world, entities, states, &placed, play_ms, dt, out, dirty, verbose);
        }
    }
}

/// `Creature::canPickUp(item)`, `Server.exe 0x00409660`: any item but a type-0x19 one, which
/// must match the word at `entity+0x1154` (reduced to `100 + n % 100` above 100).
fn can_pick_up(e: &EntityData, item: &[u8]) -> bool {
    if item[0] != 0x19 {
        return true;
    }
    let mut v = i32_at(&e.0, 0x1154);
    if v > 100 {
        v = v % 100 + 100;
    }
    i32_at(item, 4) == v
}

/// `Item::healAmount`, `Server.exe 0x00413be0`: type-1 consumables heal `itemPower * 200` for
/// sub types 1, 2, 4, 5, 6 and `itemPower * 100` for 8 and 9.
pub fn heal_amount(item: &[u8]) -> f32 {
    if item[0] != 1 {
        return 0.0;
    }
    let power = || item_power(f32::from(u16_at(item, 0x10) as i16), i32::from(item[0xc]));
    match item[1] {
        1 | 2 | 4 | 5 | 6 => power() * 200.0f32,
        8 | 9 => power() * 100.0f32,
        _ => 0.0,
    }
}

/// `Server.exe 0x004d7ae0`, run when the players sleep: every creature that is not a player
/// and not hostile type 5, whose home zone is loaded and whose spawn index is valid, resets
/// its spawn's `+0x38` counter and is deleted.
fn remove_daily_creatures(world: &mut World, entities: &mut BTreeMap<i64, EntityData>) {
    let mut gone = Vec::new();
    for (id, e) in entities.iter() {
        if e.0[0x50] == 0 || e.0[0x50] == 5 {
            continue;
        }
        let (zx, zy, idx) = (i32_at(&e.0, 0x1a0), i32_at(&e.0, 0x1a4), i32_at(&e.0, 0x1a8));
        if let Some(z) = world.zone_mut(zx, zy)
            && idx >= 0
            && (idx as usize) < z.spawns.len()
        {
            z.spawns[idx as usize].f38[0] = 0;
            gone.push(*id);
        }
    }
    for id in gone {
        entities.remove(&id);
        world.creature_ids.remove(&id);
    }
}

/// The interaction switch for one player's queued packets, in the order they arrived. In the
/// client's world (`world+0xb4`) every case but "use a static" is the server's
/// ([`client_interact`]); `states` is read there only.
#[allow(clippy::too_many_lines)]
pub fn handle_interacts(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, crate::combat::CreatureState>, player: i64, packets: &[Interact], out: &mut ServerUpdate, dirty: &mut BTreeSet<(i32, i32)>, verbose: bool) {
    for it in packets {
        if world.is_client {
            client_interact(world, entities, states, player, it, out);
            continue;
        }
        let Some(e) = entities.get(&player) else { return };
        let ppos = pos_of(e);
        let target = [i32_at(&it.0, 0x118), i32_at(&it.0, 0x11c), i32_at(&it.0, 0x120)];
        match it.0[0x128] {
            // Consume: a type-1 sub-type-7 item becomes a type-0x90 creature at the player;
            // anything else heals the player by its amount, with a sound and a negative hit.
            1 => {
                let item = &it.0[..0x118];
                if item[0] == 1 && item[1] == 7 {
                    let min = entities.keys().fold(0i64, |m, &id| if id < m { id } else { m });
                    let id = min.wrapping_sub(1);
                    let mut c = EntityData::constructed();
                    set_pos(&mut c, ppos);
                    w32(&mut c.0, 0x180, i32::from(u16_at(item, 0x10) as i16) as u32);
                    c.0[0x50] = 6;
                    w32(&mut c.0, 0x54, 0x90);
                    let mut sp = Spawn { entity_type: 0x90, ..Spawn::NEW };
                    world.init_appearance(&mut sp);
                    c.0[0x68..0x68 + cw_world::appearance::Appearance::SIZE].copy_from_slice(&sp.appearance.to_bytes());
                    entities.insert(id, c);
                } else {
                    let heal = heal_amount(item);
                    let e = entities.get_mut(&player).expect("player");
                    let hp = f32_at(&e.0, 0x15c) + heal;
                    e.0[0x15c..0x160].copy_from_slice(&hp.to_le_bytes());
                    out.sounds.push(sound(ppos, 0x2c, 1.0, 1.0));
                    let mut h = [0u8; 0x48];
                    w64(&mut h, 8, player);
                    h[0x10..0x14].copy_from_slice(&(-heal).to_le_bytes());
                    for (i, p) in ppos.iter().enumerate() {
                        w64(&mut h, 0x20 + i * 8, *p);
                    }
                    out.hits.push(Hit(h));
                }
            }
            // Talk: the creature addressed by (zone x, zone y, spawn index). An innkeeper
            // (class 0x84) at night puts the world to sleep: next day, 07:00, a sound, the
            // daily creatures removed and every region's missions re-armed.
            2 => {
                let mut sleep = false;
                for c in entities.values() {
                    if [i32_at(&c.0, 0x1a0), i32_at(&c.0, 0x1a4), i32_at(&c.0, 0x1a8)] != target {
                        continue;
                    }
                    let t = world.time_of_day;
                    if c.0[0x130] == 0x84 && (t >= 0x3dc_c500 || t <= 0x149_9700) {
                        sleep = true;
                        break;
                    }
                    if u16_at(&c.0, 0x6e) & 0x80 != 0 {
                        break;
                    }
                }
                if sleep {
                    world.day = world.day.wrapping_add(1);
                    world.time_of_day = 0x180_8580;
                    out.sounds.push(sound(ppos, 0x1d, 1.0, 0.75));
                    remove_daily_creatures(world, entities);
                    world.clear_region_activation();
                }
            }
            // Use a static.
            3 => {
                {
                    let e = entities.get_mut(&player).expect("player");
                    if e.0[0x58] == 0x4f {
                        e.0[0x58] = 0;
                    }
                }
                let (zx, zy, idx) = (target[0], target[1], target[2]);
                let (statics_len, mut s) = {
                    let Some(zone) = world.zone(zx, zy) else { continue };
                    if idx < 0 || idx as usize >= zone.statics.len() {
                        continue;
                    }
                    (zone.statics.len(), zone.statics[idx as usize].clone())
                };
                match s.kind {
                    // A bed is used through the client's own world only.
                    0x2d => continue,
                    0x10 | 0x12 | 0x13 | 0x44 | 0x45 => {
                        let e = entities.get_mut(&player).expect("player");
                        if (s.f40 == 0 && s.f44 == 0) && matches!(e.0[0x58], 0 | 0x53 | 0x54) {
                            s.f40 = player as u32;
                            s.f44 = (player >> 32) as u32;
                            if let Some(kind) = seat_kind(s.kind) {
                                seat(e, &s, kind, true);
                            }
                            out.statics.push(static_record(zx, zy, idx, &s));
                            e.0[0x30..0x3c].fill(0);
                        }
                    }
                    _ => {
                        // A kind-0xa static in state 2 spills its inventory around it.
                        if s.kind == 0xa && s.b30 == 2 {
                            for page in 0..s.inventory.pages.len() {
                                for slot in 0..s.inventory.pages[page].len() {
                                    while s.inventory.pages[page][slot].count != 0 {
                                        let ox = 2.0f32 - world.rng.rand() as f32 * 4.0f32 / 32767.0f32;
                                        let oy = 2.0f32 - world.rng.rand() as f32 * 4.0f32 / 32767.0f32;
                                        let off = fixed3([ox, oy, 1.0]);
                                        let rot = world.rng.rand() as f32 * 360.0f32 / 32767.0f32;
                                        let pos = [s.x.wrapping_add(off[0]), s.y.wrapping_add(off[1]), s.z.wrapping_add(off[2])];
                                        let item = s.inventory.pages[page][slot].item;
                                        if let Some(z) = world.drop_item(item, pos, rot, 1.0) {
                                            dirty.insert(z);
                                        }
                                        // Slot::takeOne (0x00405550): the stack shrinks by one and
                                        // empties its item at zero.
                                        let sl = &mut s.inventory.pages[page][slot];
                                        sl.count -= 1;
                                        if sl.count <= 0 {
                                            sl.count = 0;
                                            sl.item.item_type = 0;
                                            sl.item.sub_type = 0;
                                        }
                                    }
                                }
                            }
                        }
                        if matches!(s.kind, 9 | 1 | 2 | 3 | 0xa) {
                            let open = u8::from(s.b30 == 0);
                            if let Some(kind) = s.toggle(open) {
                                out.sounds.push(sound([s.x, s.y, s.z], kind, 1.0, 1.0));
                            }
                            out.statics.push(static_record(zx, zy, idx, &s));
                        }
                        if s.kind == 9 {
                            let linked = s.tail[2] as i32;
                            let open = u8::from(s.b30 != 0);
                            if linked >= 0 && (linked as usize) < statics_len && linked != idx {
                                let mut l = world.zone(zx, zy).expect("zone").statics[linked as usize].clone();
                                if let Some(kind) = l.toggle(open) {
                                    out.sounds.push(sound([l.x, l.y, l.z], kind, 1.0, 1.0));
                                }
                                // The record names the requested index, not the linked one.
                                out.statics.push(static_record(zx, zy, idx, &l));
                                if let Some(z) = world.zone_mut(zx, zy) {
                                    z.statics[linked as usize] = l;
                                }
                            } else if linked == idx {
                                if let Some(kind) = s.toggle(open) {
                                    out.sounds.push(sound([s.x, s.y, s.z], kind, 1.0, 1.0));
                                }
                                out.statics.push(static_record(zx, zy, idx, &s));
                            }
                        }
                    }
                }
                if let Some(z) = world.zone_mut(zx, zy) {
                    z.statics[idx as usize] = s;
                }
            }
            4 => {}
            // Pick up a ground item within four blocks (0x00536506, server worlds only): the
            // zone is re-announced, a pickup record and a sound go out, the item leaves the zone.
            5 => {
                let (zx, zy, idx) = (target[0], target[1], target[2]);
                let Some(zone) = world.zone(zx, zy) else { continue };
                if idx < 0 || idx as usize >= zone.items.len() {
                    continue;
                }
                let g = zone.items[idx as usize].clone();
                let item = g.item.to_bytes();
                if !can_pick_up(e, &item) {
                    continue;
                }
                if 16.0f32 < dist_sq(ppos, [g.x, g.y, g.z]) {
                    continue;
                }
                dirty.insert((zx, zy));
                let mut p = [0u8; 0x120];
                w64(&mut p, 0, player);
                p[8..0x120].copy_from_slice(&item);
                out.pickups.push(Pickup(p));
                // 0x0053664a: the local player (`world+0xb8`, never set on a server) puts the
                // item straight into its inventory (`creature+0x11dc`, `Inventory::addItem(item,
                // -1)` 0x00427000), except type 0x19 (0x00536652).
                if world.local_player == Some(player) && g.item.item_type != 0x19 {
                    states.entry(player).or_default().inventory.add_item(g.item, -1);
                }
                let pitch = world.rng.rand() as f32 * 0.1f32 / 32767.0f32 + 1.0f32;
                out.sounds.push(sound(ppos, 0x2d, pitch, 1.0));
                if let Some(z) = world.zone_mut(zx, zy) {
                    z.items.remove(idx as usize);
                }
            }
            // Drop the packet's item at the player's feet.
            6 => {
                let item = Item::from_bytes(&it.0[..0x118]);
                if let Some(z) = world.drop_item(item, ppos, 0.0, 1.0) {
                    dirty.insert(z);
                }
                out.sounds.push(sound(ppos, 0x39, 1.0, 1.0));
            }
            7 => {
                if verbose {
                    println!("[{player}] build interaction not ported");
                }
            }
            // Call the pet: no pets yet.
            8 => {}
            _ => {}
        }
    }
}

/// One queued interaction in the client's world (`world+0xb4 != 0`), the interaction switch
/// 0x005362f5 (jump table 0x005489e0): kinds 1 (0x00536e64), 2 (0x0053630d), 5 (0x00536506),
/// 6 (0x0053778c), 7 (0x005370bd) and 8 (0x00536459) are guarded to the server and do
/// nothing; kind 3, use a static (0x0053670a): a sneaking creature (mode 0x4f) stands up
/// with its guard (`creature+0x1190`) at 0; a bed (kind 0x2d) takes the local player
/// (`world+0xb8`, 0x005367af) to the static its `+0x178` names in the zone of the static
/// nearest to the player (`World::nearestStatic` 0x004d9410 within 4 blocks), two blocks up,
/// with sound 0x2f; a free seat (kinds 0x10, 0x12, 0x13, 0x44, 0x45; mode 0, 0x53 or 0x54)
/// only zeroes the acceleration (0x00536916 -> 0x00536b64: the seating is the server's);
/// anything else is the server's (0x00536ba3).
pub fn client_interact(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, crate::combat::CreatureState>, player: i64, it: &Interact, out: &mut ServerUpdate) {
    if it.0[0x128] != 3 {
        return;
    }
    let Some(e) = entities.get_mut(&player) else { return };
    // 0x00536710
    if e.0[0x58] == 0x4f {
        e.0[0x58] = 0;
        states.entry(player).or_default().block = 0.0;
    }
    let target = [i32_at(&it.0, 0x118), i32_at(&it.0, 0x11c), i32_at(&it.0, 0x120)];
    let (zx, zy, idx) = (target[0], target[1], target[2]);
    // 0x0053674e: `getZone` 0x00406290, the index against the static count (0x0041cb40).
    let s = {
        let Some(zone) = world.zone(zx, zy) else { return };
        if idx < 0 || idx as usize >= zone.statics.len() {
            return;
        }
        zone.statics[idx as usize].clone()
    };
    if s.kind == 0x2d {
        // 0x005367af: the local player only.
        if world.local_player != Some(player) {
            return;
        }
        let ppos = pos_of(&entities[&player]);
        let (nzx, nzy, _) = crate::client::nearest_static(world, ppos);
        let Some(zone) = world.zone(nzx, nzy) else { return };
        let link = s.tail[2] as i32;
        if link < 0 || link as usize >= zone.statics.len() {
            return;
        }
        let l = &zone.statics[link as usize];
        let dest = [l.x.wrapping_add(0), l.y.wrapping_add(0), l.z.wrapping_add(2 << 16)];
        // 0x00536828: sound 0x2f at the player (pitch and volume 1), then the move (0x00402cb0,
        // 0x00402a40).
        out.sounds.push(sound(ppos, 0x2f, 1.0, 1.0));
        set_pos(entities.get_mut(&player).expect("player"), dest);
        return;
    }
    // 0x005368ce: a free seat.
    if matches!(s.kind, 0x10 | 0x12 | 0x13 | 0x45 | 0x44) && s.f40 == 0 && s.f44 == 0 {
        let e = entities.get_mut(&player).expect("player");
        if matches!(e.0[0x58], 0 | 0x53 | 0x54) {
            // 0x00536b64
            e.0[0x30..0x3c].fill(0);
        }
    }
}

/// `Creature::maxHp` clamp after the interactions (0x00537856..0x0053788b), for every creature.
pub fn clamp_hp(entities: &mut BTreeMap<i64, EntityData>) {
    for e in entities.values_mut() {
        let max = max_hp(e);
        if f32_at(&e.0, 0x15c) > max {
            e.0[0x15c..0x160].copy_from_slice(&max.to_le_bytes());
        }
    }
}

/// The tail of the tick (0x00546efc..0x0054710b): every zone whose items changed is marked
/// dirty and its item list re-announced; every zone with a static record goes dirty for its
/// statics.
pub fn tail(world: &mut World, dirty: &BTreeSet<(i32, i32)>, out: &mut ServerUpdate) {
    for &(zx, zy) in dirty {
        if let Some(z) = world.zone_mut(zx, zy) {
            z.dirty = true;
        }
        if let Some(rec) = zone_items_record(world, zx, zy) {
            out.zone_items.push(rec);
        }
    }
    let zones: Vec<(i32, i32)> = out.statics.iter().map(|s| (s.zone_x, s.zone_y)).collect();
    for (zx, zy) in zones {
        if let Some(z) = world.zone_mut(zx, zy) {
            z.statics_dirty = true;
        }
    }
}
