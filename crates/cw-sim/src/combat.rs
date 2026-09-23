//! Combat as the world tick applies it: `World::applyHit` (`Server.exe 0x004cea80`) with its
//! threat bookkeeping (`0x004d5f40`), death, loot (`0x004d2ae0`), kill credit and the mission
//! progress a kill makes; the client's Passive packets as buffs (`0x00411740`); and the
//! per-creature state the original keeps outside the entity block.
//!
//! Every `rand()` here draws from the tick thread's stream, which the caller has swapped into
//! [`World::rng`].

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Hit, Kill, Mission, Passive, Pickup, Sound};
use cw_world::World;
use cw_world::generate::level_curve;
use cw_world::inventory::{Inventory, Item};
use cw_world::settlement::shops::{random_item_528bf0, random_item_52c4e0};
use cw_world::world::PET_TYPES;
use cw_world::zone::Spawn;

use crate::interact::{K, static_record};
use crate::stats::{item_power, max_hp, pow};
use crate::util::{alert_allies, elite, f32_at, i32_at, i64_at, is_ranged, passive_record, pos_of, u16_at, w32, w64, wf32};

fn equipment(e: &EntityData, slot: usize) -> &[u8] {
    let o = 0x2f0 + slot * 0x118;
    &e.0[o..o + 0x118]
}

/// A buff entry of the creature's `+0x1178` list (0x18 bytes): the type byte at +0, a value at
/// +4 and a duration in milliseconds at +8.
pub type Buff = [u8; 0x18];

/// What the original keeps on `cube::Creature` outside the entity block and the tick reads or
/// writes in combat.
#[derive(Debug, Clone, Default)]
pub struct CreatureState {
    /// `+0x13a4`: threat by attacker id.
    pub threat: BTreeMap<i64, f32>,
    /// `+0x13ac`: hits landed by target id.
    pub hits_landed: BTreeMap<i64, f32>,
    /// `+0x1190`: the guard that damage wears down.
    pub block: f32,
    /// `+0x11d0`: the last creature fought.
    pub last_target: i64,
    /// `+0x11c8`: the pet's id.
    pub pet: i64,
    /// `+0x1178`: the buffs.
    pub buffs: Vec<Buff>,
    /// `+0x11dc`: the inventory, from the spawn.
    pub inventory: Inventory,
    /// `+0x1d38`: the spawn's boss marker (`+0x10e8`).
    pub boss: u8,
    /// `+0x13c4`: milliseconds since the behaviour last ran.
    pub idle_ms: i32,
    /// `+0x139c`: skill cooldowns by mode.
    pub cooldowns: BTreeMap<u8, i32>,
    /// `+0x13bc`: the z the creature last stood on, in blocks.
    pub ground_z: f32,
    /// `+0x13b4`: the charge of a held attack.
    pub charge: f32,
    /// `+0x1194`: stamina for gliding and climbing.
    pub stamina: f32,
    /// `+0x1180`: the z offset a step up or down left, eased out over the next ticks.
    pub step_offset: f32,
    /// `+0x11a0`: the normal of the wall the creature touches.
    pub wall_normal: [f32; 3],
    /// `+0x138c`: the aim point, eased towards the ray hit.
    pub aim: [f32; 3],
    /// `+0x1318`: the roll timer before this tick.
    pub prev_roll: i32,
    /// `+0x1468`: statics the creature passes through (zone x, zone y, index).
    pub ignored_statics: BTreeSet<(i32, i32, usize)>,
    /// `+0x13e4`: the behaviour tree (`SpawnAi` cloned from the spawn, or the default list).
    pub ai_root: Option<crate::behaviors_more::AiNode>,
    /// `+0x1408..+0x1464`, `+0x1478`: the path finder's state.
    pub path: crate::path::PathState,
    /// `+0x1184`, `+0x1320`, `+0x13e0`, `+0x13e8`: the combat AI's fields.
    pub ai: crate::combat_ai::CreatureAi,
    /// `+0x11ac`, `+0x11b4`, `+0x11c0`, `+0x1314`, `+0x1320`, `+0x13b8`, `+0x13c0`, `+0x13f4`,
    /// `+0x13f8`: the skill mode machine's fields.
    pub modes: crate::modes::ModeState,
    /// `+0x1198`, `+0x1350`, `+0x1374`, `+0x1398`: riding and the render fields (riding.rs).
    pub riding: crate::riding::RidingState,
    /// `+0x1d2c`, `+0x1d44`, `+0x1d48`: the despawn radius and the pet item as last made
    /// (statics.rs).
    pub extra: crate::statics::CreatureExtra,
}

impl CreatureState {
    /// `+0x1460`: whether the creature follows no path.
    pub fn path_empty(&self) -> bool {
        self.path.waypoints.is_empty()
    }

    /// `Creature::clearPath` 0x00405330: the path lists and the passed statics go.
    pub fn clear_path(&mut self) {
        let mut p = std::mem::take(&mut self.path);
        crate::path::clear_path(&mut p, self);
        self.path = p;
    }
}

impl CreatureState {
    /// The state of a creature made from a zone spawn (`World::tick` 0x00536166, 0x00535f39).
    pub fn from_spawn(s: &Spawn) -> CreatureState {
        CreatureState { inventory: s.inventory.clone(), boss: s.b10e8, ..CreatureState::default() }
    }
}

/// `Server.exe 0x0040f710`: a creature of a peaceful type without the 0x1a00 appearance flags.
pub fn friendly_type(e: &EntityData) -> bool {
    if u16_at(&e.0, 0x6e) & 0x1a00 != 0 {
        return false;
    }
    matches!(
        i32_at(&e.0, 0x54),
        0x5c | 0x4a | 0x22 | 0x19 | 0x37 | 0x35 | 0x57 | 0x44 | 0x43 | 0x5d | 0x6a | 0x6b | 0x23 | 0x3a | 0x39 | 0x93 | 0x91 | 0x92 | 0x16 | 0x17 | 0x62 | 0x38 | 0x1e | 0x1f | 0x20 | 0x13 | 0x1a | 0x1b | 0x21 | 0x64 | 0x14 | 0x59
    )
}

/// `Server.exe 0x004cfcc0`: whether `a` treats `b` as an enemy. Never for hostile type 6 on
/// either side or a type-5 creature against a player; monsters (type 1) against anything of
/// another type or of a different peacefulness; otherwise only when either carries the
/// aggressive flag (`entity+0x114` bit 5).
pub fn is_hostile(a: &EntityData, b: &EntityData) -> bool {
    let (ha, hb) = (a.0[0x50], b.0[0x50]);
    if (ha == 5 && hb == 0) || ha == 6 || hb == 6 {
        return false;
    }
    if ha == 1 && (hb != 1 || friendly_type(a) != friendly_type(b)) {
        return true;
    }
    if hb == 1 && (ha != 1 || friendly_type(a) != friendly_type(b)) {
        return true;
    }
    u16_at(&a.0, 0x114) & 0x20 != 0 || u16_at(&b.0, 0x114) & 0x20 != 0
}

/// `Server.exe 0x0040f8b0`: a two-handed weapon item.
fn two_handed(item: &[u8]) -> bool {
    item[0] == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)
}

/// `Server.exe 0x00409620`: the creature can take damage: hostile types 0, 1, 3, 5, and 6
/// except for the three types 0x8c..0x8e.
fn damageable(e: &EntityData) -> bool {
    match e.0[0x50] {
        0 | 1 | 3 | 5 => true,
        6 => !matches!(i32_at(&e.0, 0x54), 0x8c..=0x8e),
        _ => false,
    }
}

fn sound(pos: [i64; 3], kind: u32) -> Sound {
    let mut b = [0u8; 0x18];
    for (i, p) in pos.iter().enumerate() {
        wf32(&mut b, i * 4, *p as f32 * K);
    }
    w32(&mut b, 0xc, kind);
    wf32(&mut b, 0x10, 1.0);
    wf32(&mut b, 0x14, 1.0);
    Sound(b)
}

/// A pickup record (0x120 bytes): the receiver and the item.
fn pickup_record(player: i64, item: &Item) -> Pickup {
    let mut b = [0u8; 0x120];
    w64(&mut b, 0, player);
    b[8..0x120].copy_from_slice(&item.to_bytes());
    Pickup(b)
}

/// `Creature::addBuff`, `Server.exe 0x00411740`: buffs of types 1, 2, 3, 6, 7, 9, 10 and 11
/// replace an existing buff of the same type; anything else is appended.
pub fn add_buff(state: &mut CreatureState, buff: &Buff) {
    if matches!(buff[0], 1 | 2 | 3 | 6 | 7 | 9 | 10 | 11)
        && let Some(existing) = state.buffs.iter_mut().find(|b| b[0] == buff[0])
    {
        *existing = *buff;
        return;
    }
    state.buffs.push(*buff);
}

/// The client's Passive packets (`World::tick` 0x00532be0): each becomes a buff of its target.
/// They are not relayed.
pub fn apply_passives(entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, passives: &[Passive]) {
    for p in passives {
        let target = i64_at(&p.0, 8);
        if entities.contains_key(&target) {
            let buff: Buff = p.0[0x10..0x28].try_into().unwrap();
            add_buff(states.entry(target).or_default(), &buff);
        }
    }
}

/// Buff durations lose `dt` each tick and expired buffs go (`World::tick` 0x00537ea8).
pub fn tick_buffs(states: &mut BTreeMap<i64, CreatureState>, dt: i32) {
    for s in states.values_mut() {
        s.buffs.retain_mut(|b| {
            let d = i32_at(b, 8).wrapping_sub(dt);
            w32(b, 8, d as u32);
            d > 0
        });
    }
}

/// `Server.exe 0x00412550`: XP over the level's cost raises the level, refilling HP.
pub fn level_up(e: &mut EntityData) {
    let cost = |level: i32| ((1.0f32 - 1.0f32 / ((level as f32 - 1.0f32) * 0.05f32 + 1.0f32)) * 1000.0f32 + 50.0f32) as i32;
    let mut level = i32_at(&e.0, 0x180);
    let mut xp = i32_at(&e.0, 0x184);
    if level <= 0 || xp < cost(level) {
        return;
    }
    while xp >= cost(level) {
        xp -= cost(level);
        level += 1;
        w32(&mut e.0, 0x180, level as u32);
        w32(&mut e.0, 0x184, xp as u32);
        let hp = max_hp(e);
        wf32(&mut e.0, 0x15c, hp);
    }
}

/// `randomConsumable(out, level, rarity)`, `Server.exe 0x0052b3f0`: a fresh item (level 1,
/// everything else 0) of type 0x14 (pet food) whose sub type is the low byte of
/// `world+0x88[rand() % size]` ([`PET_TYPES`], unsigned `div` at 0x0052b457). The level and
/// rarity arguments are unused. One `rand()`.
pub fn random_consumable(world: &mut World) -> Item {
    let r = world.rng.rand() as u32;
    let pet = PET_TYPES[(r % PET_TYPES.len() as u32) as usize];
    Item { item_type: 0x14, sub_type: pet as u8, ..Item::NEW }
}

/// A random offset of up to a block on x and y (two `rand()`), for loot (`1 - rand * 2 / 32767`).
fn scatter(world: &mut World) -> [f32; 3] {
    let x = 1.0f32 - world.rng.rand() as f32 * 2.0f32 / 32767.0f32;
    let y = 1.0f32 - world.rng.rand() as f32 * 2.0f32 / 32767.0f32;
    [x, y, 0.0]
}

fn offset_pos(base: [i64; 3], off: [f32; 3]) -> [i64; 3] {
    let f: Vec<i64> = off.iter().map(|v| (v * 65536.0f32) as i64).collect();
    [base[0].wrapping_add(f[0]), base[1].wrapping_add(f[1]), base[2].wrapping_add(f[2])]
}

/// Drops `item` near `base` with the loot routine's own rotation draw.
fn drop_scattered(world: &mut World, item: Item, base: [i64; 3], scale: f32, dirty: &mut std::collections::BTreeSet<(i32, i32)>) {
    let off = scatter(world);
    let pos = offset_pos(base, off);
    let rot = world.rng.rand() as f32 * 360.0f32 / 32767.0f32;
    if let Some(z) = world.drop_item(item, pos, rot, scale) {
        dirty.insert(z);
    }
}

/// `Server.exe 0x004d2ae0`: the loot of a dead creature, all of it `dropItem`s around it.
#[allow(clippy::too_many_lines)]
fn drop_loot(world: &mut World, e: &EntityData, state: &CreatureState, dirty: &mut std::collections::BTreeSet<(i32, i32)>, verbose: bool) {
    let hostile = e.0[0x50];
    let flags = u16_at(&e.0, 0x6e);
    if hostile == 0 || hostile == 3 || flags & 0x800 != 0 {
        return;
    }
    let base = pos_of(e);
    let etype = i32_at(&e.0, 0x54);
    let level = i32_at(&e.0, 0x180);
    let power = i32::from(e.0[0x198]);
    let _ = world.rng.rand();
    // One worn item, one time in four (always for a boss).
    let worn: Vec<Item> = [6usize, 7, 2, 5, 4, 3, 1].iter().map(|&s| Item::from_bytes(equipment(e, s))).filter(|it| it.item_type != 0).collect();
    if !worn.is_empty() && (world.rng.rand() % 4 == 0 || flags & 0x200 != 0) {
        let off = scatter(world);
        let pick = (world.rng.rand() as u32 % worn.len() as u32) as usize;
        let rot = world.rng.rand() as f32 * 360.0f32 / 32767.0f32;
        let pos = offset_pos(base, off);
        if let Some(z) = world.drop_item(worn[pick], pos, rot, 1.0) {
            dirty.insert(z);
        }
    }
    // Monsters: one time in ten a piece of equipment near the creature's level.
    if (hostile == 1 || etype == 0x8f) && world.rng.rand() % 10 == 0 {
        let lvl = (level - 1 + world.rng.rand() % 2).max(1);
        let armour = world.rng.rand() % 2 == 0;
        let rarity = world.rarity_roll(power, false);
        let mut item = if armour { random_item_528bf0(&mut world.rng, lvl as i16, rarity as u8, -1) } else { random_item_52c4e0(&mut world.rng, lvl as i16, rarity as u8, -1) };
        world.adjust_item_level(&mut item, 0.05, true);
        drop_scattered(world, item, base, 1.0, dirty);
        // 0x004d2ef5..0x004d2ff6: one time in five a bait for a random pet type. The rarity
        // roll (0x004d2f16, `rarityRoll(power, 0)`) and the level are passed to 0x0052b3f0,
        // which ignores both; `adjustLevel(0.05, 1)` (0x004d2f3f) leaves a level-1 item alone.
        if world.rng.rand() % 5 == 0 {
            let _ = world.rarity_roll(power, false);
            let mut item = random_consumable(world);
            world.adjust_item_level(&mut item, 0.05, true);
            if verbose {
                println!("loot: bait for creature type {:#x}", item.sub_type);
            }
            drop_scattered(world, item, base, 1.0, dirty);
        }
    }
    // One to three type-specific drops.
    let n = world.rng.rand() % 3 + 1;
    let mut stop = false;
    for _ in 0..n {
        let mut item = Item::NEW;
        let mut scale = 0.75f32;
        match etype {
            0x3a => {
                item.item_type = 0xb;
                item.sub_type = 2;
                item.material = 0x13;
                scale = 0.8;
            }
            0x46 => {
                item.item_type = 0xb;
                item.sub_type = 0x12;
                scale = 0.8;
            }
            0x47 => {
                item.item_type = 0xb;
                item.sub_type = 0x15;
                scale = 0.8;
            }
            0x78 | 0x79 | 0x7c | 0x7e => {
                item.item_type = 0xb;
                if world.rng.rand() % 2 == 0 {
                    item.sub_type = 5;
                    item.material = 0x15;
                } else {
                    item.sub_type = 1;
                    item.material = 2;
                }
                scale = 0.8;
            }
            0x7a => {
                item.item_type = 0xb;
                item.sub_type = 0x1b;
                scale = 0.8;
            }
            0x7b => {
                item.item_type = 0xb;
                item.sub_type = 0xb;
                item.material = 0x1b;
                scale = 0.8;
            }
            0x7d => {
                item.item_type = 0xb;
                item.sub_type = 6;
                scale = 0.8;
            }
            0x7f => {
                item.item_type = 0xb;
                item.sub_type = 0x14;
                scale = 0.8;
            }
            0x80 => {
                item.item_type = 0xb;
                item.sub_type = 0x17;
                scale = 0.8;
            }
            0x83..=0x85 => {
                item.item_type = 0xb;
                item.material = match etype {
                    0x83 => 0xb,
                    0x84 => 1,
                    _ => 0xc,
                };
                scale = 3.0;
            }
            0x86 => {
                item.item_type = 0xb;
                item.material = 0x11;
                scale = 0.8;
            }
            0x87..=0x8a => {
                item.item_type = 0xb;
                item.rarity = (etype - 0x86) as u8;
                item.material = (etype - 0x87 + 0xd) as u8;
                scale = 3.0;
            }
            0x8b => {
                item.item_type = 0xb;
                item.sub_type = 8;
                item.material = 0x16;
                scale = 0.8;
            }
            _ => {
                stop = true;
            }
        }
        if stop {
            break;
        }
        drop_scattered(world, item, base, scale, dirty);
    }
    if hostile == 1 || etype == 0x8f {
        // One time in fifty a recipe (0x0052a760): a random piece of equipment whose first
        // dword (type, sub type) moves to +8 and whose type becomes 2.
        if world.rng.rand() % 50 == 0 {
            let off = scatter(world);
            let rot = world.rng.rand() as f32 * 360.0f32 / 32767.0f32;
            let rarity = world.rarity_roll(power + 1, false);
            let source = if world.rng.rand() % 2 == 0 { random_item_528bf0(&mut world.rng, level as i16, rarity as u8, -1) } else { random_item_52c4e0(&mut world.rng, level as i16, rarity as u8, -1) };
            let mut item = source;
            item.f8 = u32::from(source.item_type) | (u32::from(source.sub_type) << 8);
            item.item_type = 2;
            let pos = offset_pos(base, off);
            if let Some(z) = world.drop_item(item, pos, rot, 0.75) {
                dirty.insert(z);
            }
        }
        if flags & 0x18 == 0 {
            // Coins: `itemPower(level, power) * 10`, scaled by `1 + rand * 2 / 32767`, tenfold
            // for a boss, split into copper, silver and gold.
            let base_amount = item_power(level as f32, power) * 10.0f32;
            let mut amount = (world.rng.rand() as f32 * 2.0f32 / 32767.0f32 + 1.0f32) * base_amount;
            if flags & 0x200 != 0 {
                amount *= 10.0f32;
            }
            let amount = amount as i32;
            let copper = amount % 100;
            let silver = (amount / 100) % 100;
            let gold = amount / 100 / 100;
            for (count, material) in [(copper, 0xau8), (silver, 0xc), (gold, 0xb)] {
                if count != 0 {
                    let item = Item { item_type: 0xc, material, level: count as u16, ..Item::NEW };
                    drop_scattered(world, item, base, 0.75, dirty);
                }
            }
        } else {
            let r = world.rng.rand() % 1000 + power * 20;
            let mut rarity = if r < 700 {
                0
            } else if r < 950 {
                1
            } else if r < 998 {
                2
            } else {
                3
            };
            if flags & 0x200 != 0 {
                rarity += 1;
                if rarity > 3 {
                    rarity = 3;
                }
            }
            if world.rng.rand() % 20 == 0 && rarity != 0 {
                let item = Item { item_type: 0xe, modifier: world.rng.rand(), level: level as u16, rarity: rarity as u8, ..Item::NEW };
                drop_scattered(world, item, base, 0.75, dirty);
            }
        }
    }
    if hostile == 1 {
        // The inventory, one unit at a time, except stacks of item 1/1.
        for page in &state.inventory.pages {
            for slot in page {
                if slot.count <= 0 || slot.item.item_type == 0 {
                    continue;
                }
                for _ in 0..slot.count {
                    if slot.item.item_type == 1 && slot.item.sub_type == 1 {
                        continue;
                    }
                    drop_scattered(world, slot.item, base, 0.75, dirty);
                }
            }
        }
    }
}

/// `World::applyHit`, `Server.exe 0x004cea80`. Hits on players are relayed for the client to
/// apply; hits on creatures wear their guard, build threat and alert allies, knock back, stun
/// and slow, heal or damage, and on death play the sound, close the creature's static, drop
/// loot, credit kills and XP to the players that fought it, advance the cell's mission, and
/// start the spawn's twenty-minute respawn.
#[allow(clippy::too_many_lines)]
pub fn apply_hit(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, hit: &Hit, out: &mut ServerUpdate, dirty: &mut std::collections::BTreeSet<(i32, i32)>, verbose: bool) {
    let h = &hit.0;
    let (aid, tid) = (i64_at(h, 0), i64_at(h, 8));
    let damage = f32_at(h, 0x10);
    let stun = i32_at(h, 0x18);
    let slow = i32_at(h, 0x1c);
    let hit_type = h[0x45];
    let Some(t0) = entities.get(&tid) else { return };
    // NaN semantics of the original comparison.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(f32_at(&t0.0, 0x15c) > 0.0) {
        return;
    }
    // 0x004ceb1d: a hit on a player is relayed for its client only in a world without a local
    // player (`world+0xb8 == NULL`, a server); the client's singleplayer world applies it here.
    if world.local_player.is_none() && t0.0[0x50] == 0 {
        out.hits.push(hit.clone());
        return;
    }
    let attacker = entities.get(&aid).cloned();
    if damage > 0.0 {
        let t = entities.get_mut(&tid).expect("target");
        let p = pow(2.0, f64::from((i32_at(&t.0, 0x180) - 1 + i32::from(t.0[0x198])) as f32 * 0.25f32)) as f32;
        let st = states.entry(tid).or_default();
        st.block -= damage * 0.25f32 / p;
        if st.block < 0.0 {
            st.block = 0.0;
        }
        if t.0[0x58] == 0x54 {
            t.0[0x58] = 0;
            w32(&mut t.0, 0x11c, 3000);
        }
    }
    if damage >= 0.0 && let Some(att) = &attacker {
        states.entry(aid).or_default().last_target = tid;
        states.entry(tid).or_default().last_target = aid;
        *states.entry(aid).or_default().hits_landed.entry(tid).or_insert(0.0) += 1.0;
        let inc = if is_ranged(att) {
            damage
        } else if i32_at(&att.0, 0x54) == 0x19 {
            damage * 10.0f32
        } else if two_handed(equipment(att, 7)) && att.0[0x131] == 1 {
            damage * 20.0f32
        } else {
            damage * 2.0f32
        };
        let t = states.entry(tid).or_default().threat.entry(aid).or_insert(0.0);
        *t += inc + 0.01f32;
        alert_allies(entities, states, aid, tid, out);
    }
    let tflags = u16_at(&entities[&tid].0, 0x6e);
    if tflags & 0x2000 != 0 {
        // A boss only reacts once a player holds threat on it.
        let engaged = states.entry(tid).or_default().threat.iter().any(|(id, v)| *v > 0.0 && entities.get(id).is_some_and(|e| e.0[0x50] == 0));
        if !engaged {
            return;
        }
    }
    if hit_type == 3 {
        return;
    }
    {
        let t = entities.get_mut(&tid).expect("target");
        if damage >= 0.0 {
            let mut dir = [f32_at(h, 0x38), f32_at(h, 0x3c), f32_at(h, 0x40)];
            if i32_at(&t.0, 0x11c) > 0 && stun < 1 {
                dir = dir.map(|v| v * 0.1f32);
            }
            // 0x004cedea: the knockback lands on a non-player in a server world (`world+0xb4`
            // clear) or on the local player (`world+0xb8`).
            let hostile = t.0[0x50];
            if hostile != 6 && ((!world.is_client && hostile != 0) || world.local_player == Some(tid)) {
                let cur = [f32_at(&t.0, 0x3c), f32_at(&t.0, 0x40), f32_at(&t.0, 0x44)];
                if cur[0] * cur[0] + cur[1] * cur[1] + cur[2] * cur[2] <= dir[1] * dir[1] + dir[0] * dir[0] + dir[2] * dir[2] {
                    for (i, v) in dir.iter().enumerate() {
                        wf32(&mut t.0, 0x3c + i * 4, *v);
                    }
                }
            }
            if stun > 0 {
                t.0[0x24..0x30].fill(0);
            }
        }
        if slow > 0 {
            let cur = i32_at(&t.0, 0x120);
            w32(&mut t.0, 0x120, slow.max(cur) as u32);
        }
    }
    match hit_type {
        1 => {
            let t = entities.get_mut(&tid).expect("target");
            let mp = (f32_at(&t.0, 0x160) + 0.25f32).min(1.0f32);
            wf32(&mut t.0, 0x160, mp);
        }
        4 => {
            // 0x00408230: a mana-shielded rogue specialist recovers mana and gets buff 0xb.
            let t = entities.get_mut(&tid).expect("target");
            if i32_at(&t.0, 0x118) != 0 && t.0[0x130] == 4 && t.0[0x131] == 1 {
                let mp = (f32_at(&t.0, 0x160) + 0.25f32).min(1.0f32);
                wf32(&mut t.0, 0x160, mp);
                let mut buff: Buff = [0; 0x18];
                buff[0] = 0xb;
                w32(&mut buff, 8, 30000);
                add_buff(states.entry(tid).or_default(), &buff);
                out.passives.push(passive_record(tid, tid, &buff));
            }
        }
        _ => {
            {
                let t = entities.get_mut(&tid).expect("target");
                if stun > 0 {
                    let cur = i32_at(&t.0, 0x11c);
                    w32(&mut t.0, 0x11c, stun.max(cur) as u32);
                }
            }
            // 0x004ceef8: in the client's world the rest is only the local player's.
            if world.is_client && world.local_player != Some(tid) {
                return;
            }
            if damage <= 0.0 {
                if damage < 0.0 {
                    let t = entities.get_mut(&tid).expect("target");
                    let mut hp = f32_at(&t.0, 0x15c) - damage;
                    let max = max_hp(t);
                    if max < hp {
                        hp = max;
                    }
                    wf32(&mut t.0, 0x15c, hp);
                    // Healing an enemy's target draws its aggression.
                    if let Some(att) = &attacker {
                        let ids: Vec<i64> = entities.keys().copied().collect();
                        for id in ids {
                            let c = &entities[&id];
                            if !is_hostile(att, c) {
                                continue;
                            }
                            if states.entry(id).or_default().threat.contains_key(&tid) {
                                *states.entry(id).or_default().threat.entry(aid).or_insert(0.0) -= damage * 0.1f32;
                                alert_allies(entities, states, aid, id, out);
                            }
                        }
                    }
                }
            } else if hit_type == 5 {
                let st = states.entry(tid).or_default();
                let mut records = Vec::new();
                for b in &mut st.buffs {
                    if b[0] == 6 {
                        let v = f32_at(b, 4) - damage;
                        if v <= 0.0 {
                            w32(b, 4, 0);
                            w32(b, 8, 0);
                        } else {
                            wf32(b, 4, v);
                        }
                        records.push(passive_record(aid, tid, b));
                    }
                }
                out.passives.extend(records);
            } else {
                let can = damageable(&entities[&tid]) && attacker.as_ref().is_none_or(damageable);
                if can {
                    let t = entities.get_mut(&tid).expect("target");
                    let hp = f32_at(&t.0, 0x15c) - damage;
                    wf32(&mut t.0, 0x15c, hp);
                }
            }
            if f32_at(&entities[&tid].0, 0x15c) <= 0.0 {
                death(world, entities, states, aid, tid, out, dirty, verbose);
            }
        }
    }
}

/// The death of `tid` inside `applyHit` (0x004cf0a3 onward).
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn death(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, aid: i64, tid: i64, out: &mut ServerUpdate, dirty: &mut std::collections::BTreeSet<(i32, i32)>, verbose: bool) {
    let t = entities.get(&tid).expect("target").clone();
    {
        let tm = entities.get_mut(&tid).expect("target");
        wf32(&mut tm.0, 0x15c, 0.0);
        w32(&mut tm.0, 0x5c, 0);
    }
    let tpos = pos_of(&t);
    out.sounds.push(sound(tpos, 0x1b));
    let (zx, zy, sidx) = (i32_at(&t.0, 0x1a0), i32_at(&t.0, 0x1a4), i32_at(&t.0, 0x1a8));
    // The kind-5 static the creature owns closes.
    let owned = world.zone(zx, zy).and_then(|z| z.statics.iter().position(|s| s.kind == 5 && s.tail[2] as i32 == sidx && s.b30 != 0));
    if let Some(k) = owned {
        let z = world.zone_mut(zx, zy).expect("zone");
        let s = &mut z.statics[k];
        let snd = s.toggle(0);
        let rec = static_record(zx, zy, k as i32, s);
        if let Some(kind) = snd {
            out.sounds.push(sound([s.x, s.y, s.z], kind));
        }
        out.statics.push(rec);
    }
    let tstate = states.entry(tid).or_default().clone();
    drop_loot(world, &t, &tstate, dirty, verbose);
    if tstate.pet != 0
        && let Some(p) = entities.get_mut(&tstate.pet)
    {
        wf32(&mut p.0, 0x15c, 0.0);
    }
    // XP: the level curve at level plus power, times twenty, at least one; elites twenty
    // times that, bosses ten times; nothing for flag 0x800.
    let tflags = u16_at(&t.0, 0x6e);
    let mut xp = level_curve((i32_at(&t.0, 0x180) + i32::from(t.0[0x198])) as f32) * 20.0f32;
    if xp < 1.0 {
        xp = 1.0;
    }
    if elite(&t) {
        xp *= 20.0f32;
    }
    if tflags & 0x200 != 0 {
        xp *= 10.0f32;
    }
    if tflags & 0x800 != 0 {
        xp = 0.0;
    }
    // Mission progress in the creature's cell, credited to the first player holding threat.
    let (cx, cy) = (zx / 8, zy / 8);
    let mut completed = false;
    let mut mission_level: i32 = 0;
    let mut mission_b40: u8 = 0;
    if let Some(cell) = world.cell(cx, cy).copied()
        && cell.mission.b41 != 2
        && (tflags & 0x2000 != 0 || (cell.mission.f34 == 5 && tstate.boss != 0))
    {
        let first_player = tstate.threat.iter().find(|(id, v)| **v > 0.0 && entities.get(id).is_some_and(|e| e.0[0x50] == 0)).map(|(id, _)| *id);
        if first_player.is_some() {
            let mut done = false;
            let mut announce = true;
            match cell.mission.f34 {
                1..=5 | 0xd => done = true,
                7..=0xc => {
                    let c = world.cell_mut(cx, cy).expect("cell");
                    c.mission.f44 += 1;
                    c.mission.b41 = 1;
                    if c.mission.f44 >= c.mission.f48 {
                        done = true;
                    }
                }
                _ => announce = false,
            }
            if announce {
                if done {
                    world.cell_mut(cx, cy).expect("cell").mission.b41 = 2;
                }
                let c = *world.cell(cx, cy).expect("cell");
                let r = c.mission_record(cx, cy);
                out.missions.push(Mission { cell_x: r.cell_x, cell_y: r.cell_y, words: r.words, b40: r.b40, b41: r.b41, tail: r.tail });
                mission_level = c.mission.f3c;
                mission_b40 = c.mission.b40;
                if done {
                    completed = true;
                    for x in cx * 8..cx * 8 + 8 {
                        for y in cy * 8..cy * 8 + 8 {
                            if let Some(z) = world.zone_mut(x, y) {
                                let n = z.gen_spawn_count.min(z.spawns.len());
                                z.spawns.truncate(n);
                            }
                        }
                    }
                }
            }
        }
    }
    // Every creature forgets the dead one and its pet; the players that fought a monster get
    // the kill.
    let ids: Vec<i64> = entities.keys().copied().collect();
    let pet = tstate.pet;
    for id in ids {
        {
            let cs = states.entry(id).or_default();
            if cs.last_target == tid || cs.last_target == pet {
                cs.last_target = 0;
            }
            cs.threat.retain(|k, _| *k != tid && *k != pet);
        }
        let c = entities[&id].clone();
        if t.0[0x50] == 1 && c.0[0x50] == 0 && tstate.threat.contains_key(&id) {
            let mut k = [0u8; 0x18];
            w64(&mut k, 0, id);
            w64(&mut k, 8, tid);
            w32(&mut k, 0x10, i32_at(&t.0, 0x54) as u32);
            w32(&mut k, 0x14, xp as i32 as u32);
            out.kills.push(Kill(k));
            // `World::grantXp(kill)` 0x004d61c0: the local player's share (a client world only)
            // and the killer's pet.
            crate::client::world_grant_xp(world, entities, states, &k);
            if completed {
                let lvl = (level_curve((mission_level + i32::from(mission_b40)) as f32) * 50.0f32 + 1.0f32) as u16;
                let reward = Item { item_type: 0xd, level: lvl, ..Item::NEW };
                out.pickups.push(pickup_record(id, &reward));
                // 0x004cfa22: the local player (`world+0xb8`) takes the reward straight into
                // its inventory (`Inventory::addItem(item, -1)`).
                if world.local_player == Some(id) {
                    states.entry(id).or_default().inventory.add_item(reward, -1);
                }
                let class = i32::from(c.0[0x130]);
                let item = if world.rng.rand() % 2 == 0 { random_item_528bf0(&mut world.rng, mission_level as i16, mission_b40, class) } else { random_item_52c4e0(&mut world.rng, mission_level as i16, mission_b40, class) };
                out.pickups.push(pickup_record(id, &item));
                // 0x004cfadc: the same for the item.
                if world.local_player == Some(id) {
                    states.entry(id).or_default().inventory.add_item(item, -1);
                }
            }
        }
    }
    // The spawn respawns in twenty minutes, on this day.
    let day = world.day as u32;
    if let Some(z) = world.zone_mut(zx, zy)
        && sidx >= 0
        && (sidx as usize) < z.spawns.len()
    {
        z.spawns[sidx as usize].f38[0] = 1_200_000;
        z.spawns[sidx as usize].f38[1] = day;
    }
    let _ = aid;
}

/// `Server.exe 0x004d61c0` on a server: the killer's living pet gains the XP and levels up;
/// the killer itself is credited by its client from the kill record.
pub(crate) fn grant_xp(entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, killer: i64, xp: i32) {
    let pet = states.entry(killer).or_default().pet;
    if pet == 0 {
        return;
    }
    if let Some(p) = entities.get_mut(&pet)
        && f32_at(&p.0, 0x15c) > 0.0
    {
        let v = i32_at(&p.0, 0x184).wrapping_add(xp);
        w32(&mut p.0, 0x184, v as u32);
        level_up(p);
        let (pxp, plevel) = (i32_at(&p.0, 0x184), i32_at(&p.0, 0x180));
        if let Some(k) = entities.get_mut(&killer) {
            let o = 0x2f0 + 12 * 0x118;
            w32(&mut k.0, o + 4, pxp as u32);
            k.0[o + 0x10..o + 0x12].copy_from_slice(&(plevel as u16).to_le_bytes());
            // 0x004d6337: the copy the pet pass compares with (`+0x1d48`, statics.rs) follows,
            // so the pet is not made again.
            let item = k.0[o..o + 0x118].to_vec();
            states.entry(killer).or_default().extra.pet_item.copy_from_slice(&item);
        }
    }
}
