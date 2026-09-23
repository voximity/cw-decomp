//! The received `UpdateLists` (`GameController+0x800630`, filled by the receive thread's
//! packet 4) applied to the client's world by `update` 0x00488ee0, after the world tick, and
//! spliced into the tick's own output lists (`esp+0x224`), which the rest of `update` reads
//! (the particles 0x004952a5, the sounds 0x00495630, the send queues 0x0049c02f).
//!
//! The lists are the thirteen slots of `UpdateLists` (0x00423510): hits `+0x630`, sounds
//! `+0x638`, particles `+0x640`, block actions `+0x648`, shoots `+0x650`, zone items `+0x658`,
//! statics `+0x660`, `items_8` `+0x668`, pickups `+0x670`, kills `+0x678`, damage `+0x680`,
//! passives `+0x688`, missions `+0x690`. Tier B, in the original's order:
//!
//! | Range | Slot | Here |
//! |---|---|---|
//! | 0x0048d5f0..0x0048d771 | zone items: the zone's ground item list replaced, spliced out | [`zone_items`] |
//! | 0x0048f242..0x0048f435 | block actions: `clearBlock` 0x00471920 or `setBlock` 0x0044e7a0, then `dropUnsupportedStatics` 0x005a5cb0 over the range, spliced out | [`block_actions`] |
//! | 0x0048f435..0x0048f58d | statics: the zone's static at the index replaced, spliced out | [`statics`] |
//! | 0x0048f58d..0x0048f719 | `items_8`: the zone's list at `zone+0x3c` replaced, spliced out | [`items_8`] |
//! | 0x0048f719..0x0048f858 | pickups: the local player's items (not type 0x19) into its inventory (`addItem` 0x0046ebe0), spliced out | [`pickups`] |
//! | 0x0048f858..0x0048facc | every output pickup: recipes, XP (type 0xd), mana cubes (type 0x19), "You receive" | [`pickup_effects`] |
//! | 0x0048facc..0x0048fbb7 | kills: `World::grantXp` 0x005a0bf0, spliced out | [`kills`] |
//! | 0x0048fbb7..0x0048fcb8 | damage: the threat map `creature+0x13a4` of the first creature | [`damage`] |
//! | 0x0048fcb8..0x0048fdc3 | passives: `Creature::addBuff` 0x00446af0 on the target, spliced out | [`passives`] |
//! | 0x0048fdc3..0x0048fecb | missions: the cell's mission state, spliced out (at the end of the output list) | [`missions`] |
//! | 0x0048fecb..0x0049034a | hits on the local player | [`local_hits`] |
//! | 0x0049034a..0x00490409 | shoots of other creatures: new projectiles (`world+0x14`) | [`shoots`] |
//! | 0x00490409..0x004904b0 | hits of other creatures: onto the output hits | [`other_hits`] |
//! | 0x004904b0..0x004905bd | particles, sounds, missions spliced out | [`apply`] |
//! | 0x004905bd..0x00490651 | the received lists cleared | – |
//! | 0x00490651..0x0049081b | every output mission: the dialog (kind 1) or the quest tag (kind 2) | [`mission_effects`] |

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::packet::{Item8, Passive, ServerUpdate};
use cw_sim::combat::{CreatureState, add_buff};
use cw_sim::projectile::Projectile;
use cw_world::World;
use cw_world::inventory::Item;
use cw_world::zone::GroundItem;

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn i64_at(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn wf32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// What the application asks of the controller (the UI and audio parts of the ranges).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// A chat line (`ChatWidget::print` 0x0043a500), colour in bytes.
    Print { text: String, color: [u8; 3] },
    /// `playSound(id, &GC+0x140, 1, 1)`: at the listener.
    Sound { id: u32 },
    /// 0x004a14c0: the crafting recipes rebuilt (a pickup of the local player).
    RebuildRecipes,
    /// 0x004c6350 / 0x004c64c0: the bag widget refreshed.
    RefreshBag,
    /// The `manacubes` node plays (`0x00636810(GC+0x8007d8, "manacubes")`, every fourth cube).
    ManaCubeAnimation,
    /// 0x004882e0(creature, 1): the dialog of a mission's creature (kind 1).
    Talk { id: i64 },
    /// 0x004906da..0x004907d7: kind 2 on a known cell: `questtag` plays "shine" with the
    /// cell's objective text (0x00477fa0), sound 0x1e.
    QuestComplete { cell: (i32, i32) },
}

/// The client state the application touches (under lock A).
pub struct Target<'a> {
    pub world: &'a mut World,
    pub entities: &'a mut BTreeMap<i64, EntityData>,
    pub states: &'a mut BTreeMap<i64, CreatureState>,
    /// `world+0x14`: the projectiles.
    pub projectiles: &'a mut Vec<Projectile>,
    /// `zone+0x3c` per zone (the `items_8` lists; `cw_world::Zone` has no field for them).
    pub items8: &'a mut BTreeMap<(i32, i32), Vec<Item8>>,
    /// Zones whose blocks changed (their chunks mesh again).
    pub dirty_zones: &'a mut BTreeSet<(i32, i32)>,
    /// `GC+0x8006d0`: the local player's creature id.
    pub local: i64,
    /// The World's `cube::Speech` (`World+0x30`), for the item names.
    pub text: Option<&'a crate::ui::textdb::TextDb>,
}

/// `update` 0x0048d5f0..0x0049081b over the received lists `incoming` and the tick's output
/// `out`: every step of the module table, in order. Returns the effects for the controller.
pub fn apply(t: &mut Target<'_>, incoming: ServerUpdate, out: &mut ServerUpdate) -> Vec<Effect> {
    let mut fx = Vec::new();
    let ServerUpdate { block_actions, hits, particles, sounds, shoots, statics, zone_items: zi, items_8: i8, pickups: pk, kills: kl, damage: dm, passives: ps, missions: ms } = incoming;
    zone_items(t, zi, out);
    self::block_actions(t, block_actions, out);
    self::statics(t, statics, out);
    items_8(t, i8, out);
    pickups(t, pk, out, &mut fx);
    pickup_effects(t, out, &mut fx);
    kills(t, kl, out);
    damage(t, &dm);
    passives(t, ps, out);
    let received_missions = missions(t, ms);
    local_hits(t, &hits, out);
    self::shoots(t, &shoots);
    other_hits(t, hits, out);
    // 0x004904b0..0x004905b8: particles (`0x00457d10`), sounds (`0x00457d60`) and missions
    // (`0x00457c70`) spliced onto the ends of the output lists.
    out.particles.extend(particles);
    out.sounds.extend(sounds);
    out.missions.extend(received_missions);
    mission_effects(t, out, &mut fx);
    fx
}

/// 0x0048d5f0: per record, the zone's ground items (`zone+0x30`) cleared (0x0044be20) and
/// replaced by the record's (`0x00457db0`); `zone+0x74 = 0` (not modelled). The records then
/// go to the output list (`0x00457bd0`).
pub fn zone_items(t: &mut Target<'_>, records: Vec<cw_net::packet::ZoneItems>, out: &mut ServerUpdate) {
    for r in &records {
        if let Some(z) = t.world.zone_mut(r.zone_x, r.zone_y) {
            z.items = r.items.iter().map(|g| ground_item(&g.0)).collect();
        }
    }
    out.zone_items.extend(records);
}

/// A ground item from its 0x148-byte record (the inverse of `cw_sim::interact::ground_item_record`).
pub fn ground_item(b: &[u8]) -> GroundItem {
    GroundItem {
        item: Item::from_bytes(&b[..0x118]),
        x: i64_at(b, 0x118),
        y: i64_at(b, 0x120),
        z: i64_at(b, 0x128),
        rotation: f32_at(b, 0x130),
        f134: f32_at(b, 0x134),
        b138: b[0x138],
        f13c: i32_at(b, 0x13c),
        f140: i32_at(b, 0x140),
        f144: i32_at(b, 0x144),
    }
}

/// 0x0048f242: under the world's second lock (0x0059c480 .. 0x00601ea0): each action with the
/// type byte (`+0xf`) 0 clears the block (`World::clearBlock` 0x00471920), else sets it with
/// its colour and type (`World::setBlock(x, y, z, &rgb, 0)` 0x0044e7a0), tracking the x and y
/// extremes (seeded from the first record); then `World::dropUnsupportedStatics` 0x005a5cb0
/// over them and the records go to the output list (`0x00457b30`). The zones changed are
/// marked for remeshing.
pub fn block_actions(t: &mut Target<'_>, records: Vec<cw_net::packet::BlockAction>, out: &mut ServerUpdate) {
    let Some(first) = records.first() else { return };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for b in &records {
        if b.block[3] == 0 {
            cw_sim::modes::clear_block(t.world, b.x, b.y, b.z);
        } else {
            set_block(t.world, b.x, b.y, b.z, b.block);
        }
        x0 = x0.min(b.x);
        x1 = x1.max(b.x);
        y0 = y0.min(b.y);
        y1 = y1.max(b.y);
        if (0..0x100_0000).contains(&b.x) && (0..0x100_0000).contains(&b.y) {
            t.dirty_zones.insert((b.x / 256, b.y / 256));
        }
    }
    cw_sim::modes::drop_unsupported_props(t.world, x0, y0, x1, y1);
    out.block_actions.extend(records);
}

/// `World::setBlock(x, y, z, block)` on a loaded zone (the zone found from the block).
fn set_block(world: &mut World, x: i32, y: i32, z: i32, block: [u8; 4]) {
    if !(0..0x100_0000).contains(&x) || !(0..0x100_0000).contains(&y) {
        return;
    }
    let (zx, zy) = (x / 256, y / 256);
    let Some(mut zone) = world.remove_zone(zx, zy) else { return };
    cw_world::zone::set_block(world, &mut zone, x, y, z, block);
    world.insert_zone(zone);
}

/// 0x0048f435: a static record whose index (`+8`) is inside the zone's static vector
/// (`zone+0xc`) overwrites that static's fields (`+0x10..`, 0x00468670); `zone+0x74 = 0`.
/// The records go to the output list (`0x00457cc0`).
pub fn statics(t: &mut Target<'_>, records: Vec<cw_net::packet::StaticEntity>, out: &mut ServerUpdate) {
    for r in &records {
        let Some(z) = t.world.zone_mut(r.zone_x, r.zone_y) else { continue };
        if r.index < 0 || r.index as usize >= z.statics.len() {
            continue;
        }
        let s = &mut z.statics[r.index as usize];
        s.kind = r.kind;
        s.x = r.x;
        s.y = r.y;
        s.z = r.z;
        s.rotation = r.rotation;
        s.scale = r.scale.map(f32::from_bits);
        s.b30 = r.b40;
        s.f34 = r.f44;
        s.f38 = r.f48;
        s.f40 = r.user_id as u32;
        s.f44 = (r.user_id >> 32) as u32;
    }
    out.statics.extend(records);
}

/// 0x0048f58d: under the second lock, the zone's `zone+0x3c` list replaced by the record's
/// (`0x00457a60`), `zone+0x74 = 0`; the records go to the output list (`0x00457a90`). The
/// key is the zone's x and y.
pub fn items_8(t: &mut Target<'_>, records: Vec<cw_net::packet::Items8>, out: &mut ServerUpdate) {
    for r in &records {
        let zx = i32::from_le_bytes(r.key[0..4].try_into().unwrap());
        let zy = i32::from_le_bytes(r.key[4..8].try_into().unwrap());
        if t.world.zone(zx, zy).is_some() {
            t.items8.insert((zx, zy), r.items.clone());
        }
    }
    out.items_8.extend(records);
}

/// 0x0048f719: a pickup for the local player (`findEntity(+0) == GC+0x8006d0`) of any type
/// but 0x19 goes into its inventory (`Inventory::addItem(item, -1)` 0x0046ebe0) and the bag
/// widget refreshes; the records go to the output list (`0x00457c20`).
pub fn pickups(t: &mut Target<'_>, records: Vec<cw_net::packet::Pickup>, out: &mut ServerUpdate, fx: &mut Vec<Effect>) {
    for p in &records {
        let who = i64_at(&p.0, 0);
        if who == t.local && t.entities.contains_key(&who) && p.0[8] != 0x19 {
            t.states.entry(who).or_default().inventory.add_item(Item::from_bytes(&p.0[8..0x120]), -1);
            fx.push(Effect::RefreshBag);
        }
    }
    out.pickups.extend(records);
}

/// 0x0048f858..0x0048facc over every output pickup (the tick's and the received) whose
/// creature exists: the local player's rebuild the recipes (0x004a14c0); XP (item type 0xd)
/// for the local player adds `level * 10` (the item's `+0x10`, signed) to its XP and levels
/// up (`Creature::levelUp` 0x00447b00); type 0x19 counts a mana cube on the *local* player
/// (`creature+0x1164`, whoever picked it up), sets its mode 0x6d, prints "You receive a mana
/// cube." and plays sound 0x1e, and every fourth cube plays the `manacubes` node and prints
/// "You gain one skill point."; any other type prints the pickup (0x00480fb0).
pub fn pickup_effects(t: &mut Target<'_>, out: &ServerUpdate, fx: &mut Vec<Effect>) {
    for p in &out.pickups {
        let who = i64_at(&p.0, 0);
        if !t.entities.contains_key(&who) {
            continue;
        }
        if who == t.local {
            fx.push(Effect::RebuildRecipes);
        }
        let item = &p.0[8..0x120];
        if item[0] == 0xd && who == t.local {
            let lvl = i16::from_le_bytes([item[0x10], item[0x11]]) as i32;
            if let Some(e) = t.entities.get_mut(&t.local) {
                let xp = i32_at(&e.0, 0x184).wrapping_add(lvl * 10);
                e.0[0x184..0x188].copy_from_slice(&xp.to_le_bytes());
                cw_sim::combat::level_up(e);
            }
        }
        if item[0] == 0x19 {
            let Some(e) = t.entities.get_mut(&t.local) else { continue };
            let n = i32_at(&e.0, 0x1154).wrapping_add(1);
            e.0[0x1154..0x1158].copy_from_slice(&n.to_le_bytes());
            e.0[0x58] = 0x6d;
            e.0[0x5c..0x60].fill(0);
            fx.push(Effect::Print { text: "You receive a mana cube.\n".into(), color: [100, 150, 255] });
            fx.push(Effect::Sound { id: 0x1e });
            // `n % 4 == 0` with the sign fix-up of `and 0x80000003`.
            if n % 4 == 0 {
                fx.push(Effect::ManaCubeAnimation);
                fx.push(Effect::Print { text: "You gain one skill point.\n".into(), color: [150, 128, 255] });
            }
        } else {
            fx.extend(item_received(t.entities, t.text, t.local, who, item));
        }
    }
}

/// `0x00480fb0(item, creature)`: "You receive " for the local player, "<name> receives " for
/// another creature (its name `creature+0x1168`), each printed in white (0x0043ab30); then the
/// item line and ".\n" in white (0x0048141c, string 0x0070175c):
///
/// - type 0xc: `count << " " << singular(materialKey(Item+0xd))` (0x0048120e..0x00481268;
///   `World::materialKey` 0x0059ff60, `Speech::form` 0x00480e00), white;
/// - else: the counted types (0xd, 0x15, 0xb but not sub 0xe, 0, 0x19, 0x14, 0x18, 0x17)
///   first `count << " X "` (0x00481309, string 0x00701754), then `World::itemName`
///   0x00598a50, then for the other types `" +" << itemLevel` (0x004c76a0, string 0x006fd424);
///   in the item's name colour (0x004c7d20).
///
/// The count is `Item+0x10` as an unsigned short (`movzx`).
pub fn item_received(entities: &BTreeMap<i64, EntityData>, text: Option<&crate::ui::textdb::TextDb>, local: i64, who: i64, item: &[u8]) -> Vec<Effect> {
    let prefix = if who == local {
        "You receive ".to_string()
    } else {
        let name = entities.get(&who).map(|e| {
            let n = &e.0[0x1158..0x1168];
            let end = n.iter().position(|&c| c == 0).unwrap_or(n.len());
            String::from_utf8_lossy(&n[..end]).into_owned()
        });
        format!("{} receives ", name.unwrap_or_default())
    };
    let white = [255, 255, 255];
    let count = u16::from_le_bytes([item[0x10], item[0x11]]);
    let (body, color) = if item[0] == 0xc {
        let empty = crate::ui::textdb::TextDb::default();
        let db = text.unwrap_or(&empty);
        let singular = crate::names::form(db, crate::names::material_key(i32::from(item[0xd])), "singular");
        (format!("{count} {singular}"), white)
    } else {
        let counted = matches!(item[0], 0xd | 0x15 | 0 | 0x19 | 0x14 | 0x18 | 0x17) || (item[0] == 0xb && item[1] != 0xe);
        let mut s = String::new();
        if counted {
            s.push_str(&format!("{count} X "));
        }
        s.push_str(&crate::names::item_name(text, item));
        if !counted {
            s.push_str(&format!(" +{}", crate::ui::inventory::item_level(item)));
        }
        let c = crate::ui::inventory_widget::rarity_color(item);
        (s, [c[0], c[1], c[2]].map(|v| (v * 255.0) as u8))
    };
    vec![
        Effect::Print { text: prefix, color: white },
        Effect::Print { text: body, color },
        Effect::Print { text: ".\n".into(), color: white },
    ]
}

/// 0x0048facc: `World::grantXp(kill)` 0x005a0bf0 for every received kill; spliced out
/// (`0x004579c0`).
pub fn kills(t: &mut Target<'_>, records: Vec<cw_net::packet::Kill>, out: &mut ServerUpdate) {
    for k in &records {
        cw_sim::client::world_grant_xp(t.world, t.entities, t.states, &k.0);
    }
    out.kills.extend(records);
}

/// 0x0048fbb7: when both creatures exist and differ, the first one's threat map
/// (`creature+0x13a4`) gets the value at `+0x10` for the second. Not spliced.
pub fn damage(t: &mut Target<'_>, records: &[cw_net::packet::Damage]) {
    for d in records {
        let (a, b) = (i64_at(&d.0, 0), i64_at(&d.0, 8));
        if !t.entities.contains_key(&a) || !t.entities.contains_key(&b) || a == b {
            continue;
        }
        t.states.entry(a).or_default().threat.insert(b, f32_at(&d.0, 0x10));
    }
}

/// 0x0048fcb8: each passive whose target (`+8`) exists adds its buff (`+0x10`) to it; spliced
/// out (`0x00457a10`).
pub fn passives(t: &mut Target<'_>, records: Vec<Passive>, out: &mut ServerUpdate) {
    for p in &records {
        let target = i64_at(&p.0, 8);
        if t.entities.contains_key(&target) {
            let buff: [u8; 0x18] = p.0[0x10..0x28].try_into().unwrap();
            add_buff(t.states.entry(target).or_default(), &buff);
        }
    }
    out.passives.extend(records);
}

/// 0x0048fdc3: each mission record's state goes into its cell (`cell+0x2c..+0x54` from
/// `+0x10`, 0x00468620), and its region's byte `+8` is cleared (not modelled: the port's
/// region has no such field; see the report). Returns the records for the splice at the end.
pub fn missions(t: &mut Target<'_>, records: Vec<cw_net::packet::Mission>) -> Vec<cw_net::packet::Mission> {
    for m in &records {
        if let Some(c) = t.world.cell_mut(m.cell_x, m.cell_y) {
            let s = &mut c.mission;
            s.f30 = m.words[1];
            s.f34 = m.words[2];
            s.f38 = m.words[3];
            s.f3c = m.words[4];
            s.b40 = m.b40;
            s.b41 = m.b41;
            s.f44 = m.tail[0];
            s.f48 = m.tail[1];
            s.f4c = i64::from(m.tail[2] as u32) | (i64::from(m.tail[3]) << 32);
        }
    }
    records
}

/// 0x0048fecb..0x0049034a: the received hits whose target (`+8`) is the local player:
///
/// - type 1 (a guarded hit) with damage above 0: the velocity becomes the hit's direction
///   (`+0x38`) and the hit flash (`creature+0x1184`) 1; then (type 1 again) the velocity once
///   more, `MP += +0x164`, `+0x134 += +0x164`, MP capped at 1 and `+0x134` at MP, and `+0x164`
///   loses `damage / guardStrength` (0x0043e190), floored at -1;
/// - type 5 (absorbed): every type-6 buff loses the damage (0 and its duration 0 at or below 0)
///   and goes out as a passive of the player to itself;
/// - otherwise, with HP above 0: positive damage wears the guard (`creature+0x1190 -=
///   damage * 0.25 / 2^((level - 1) * 0.25)`, floored at 0), a stun (`+0x18`) raises the stun
///   time, and HP loses the damage, clamped to `0..=maxHp`.
pub fn local_hits(t: &mut Target<'_>, hits: &[cw_net::packet::Hit], out: &mut ServerUpdate) {
    let me = t.local;
    for h in hits {
        let h = &h.0;
        if i64_at(h, 8) != me {
            continue;
        }
        let Some(e) = t.entities.get_mut(&me) else { continue };
        let st = t.states.entry(me).or_default();
        let ty = h[0x45];
        let dmg = f32_at(h, 0x10);
        let dir = [f32_at(h, 0x38), f32_at(h, 0x3c), f32_at(h, 0x40)];
        if ty != 3 && ty == 1 && dmg > 0.0 {
            for (i, v) in dir.iter().enumerate() {
                wf32(&mut e.0, 0x24 + i * 4, *v);
            }
            st.ai.hit_flash = 1.0;
        }
        if ty == 1 {
            for (i, v) in dir.iter().enumerate() {
                wf32(&mut e.0, 0x24 + i * 4, *v);
            }
            let g = cw_sim::combat_ai::block_strength(e);
            let x = dmg / g;
            let mp = f32_at(&e.0, 0x164) + f32_at(&e.0, 0x160);
            wf32(&mut e.0, 0x160, mp);
            let v134 = f32_at(&e.0, 0x164) + f32_at(&e.0, 0x134);
            wf32(&mut e.0, 0x134, v134);
            if f32_at(&e.0, 0x160) > 1.0f32 {
                wf32(&mut e.0, 0x160, 1.0);
            }
            let mp = f32_at(&e.0, 0x160);
            if f32_at(&e.0, 0x134) > mp {
                wf32(&mut e.0, 0x134, mp);
            }
            let v = f32_at(&e.0, 0x164) - x;
            wf32(&mut e.0, 0x164, v);
            if -1.0f32 > f32_at(&e.0, 0x164) {
                wf32(&mut e.0, 0x164, -1.0);
            }
        } else if ty == 5 {
            let mut recs = Vec::new();
            for b in &mut st.buffs {
                if b[0] != 6 {
                    continue;
                }
                let v = f32_at(b, 4) - dmg;
                wf32(b, 4, v);
                // 0x004900f2: `comiss 0, v; jb` keeps a positive value (NaN is cleared).
                if !(0.0f32 < v) {
                    wf32(b, 4, 0.0);
                    b[8..12].fill(0);
                }
                let mut p = [0u8; 0x28];
                p[0..8].copy_from_slice(&me.to_le_bytes());
                p[8..16].copy_from_slice(&me.to_le_bytes());
                p[0x10..0x28].copy_from_slice(b);
                recs.push(Passive(p));
            }
            out.passives.extend(recs);
        } else if f32_at(&e.0, 0x15c) > 0.0f32 {
            if dmg > 0.0f32 {
                let x = dmg * 0.25f32;
                let level = i32_at(&e.0, 0x180).wrapping_sub(1);
                let y = level as f32 * 0.25f32;
                let p = cw_sim::stats::pow(2.0, f64::from(y)) as f32;
                st.block -= x / p;
                if 0.0f32 > st.block {
                    st.block = 0.0;
                }
            }
            let stun = i32_at(h, 0x18);
            if stun > 0 {
                let cur = i32_at(&e.0, 0x11c);
                e.0[0x11c..0x120].copy_from_slice(&stun.max(cur).to_le_bytes());
            }
            let hp = f32_at(&e.0, 0x15c) - dmg;
            wf32(&mut e.0, 0x15c, hp);
            if 0.0f32 > f32_at(&e.0, 0x15c) {
                wf32(&mut e.0, 0x15c, 0.0);
            }
            let max = cw_sim::stats::max_hp(e);
            if f32_at(&e.0, 0x15c) > max {
                wf32(&mut e.0, 0x15c, max);
            }
        }
    }
}

/// 0x0049034a: shoots of other creatures (`+0 != ` the local player's id) become projectiles
/// of the client's world (`world+0x14`, 0x004861a0).
pub fn shoots(t: &mut Target<'_>, shoots: &[cw_net::packet::Shoot]) {
    for s in shoots {
        if i64_at(&s.0, 0) != t.local {
            t.projectiles.push(cw_sim::projectile::projectile_from_shoot(s));
        }
    }
}

/// 0x00490409: hits of other creatures go onto the output hits (0x00486290).
pub fn other_hits(t: &mut Target<'_>, hits: Vec<cw_net::packet::Hit>, out: &mut ServerUpdate) {
    for h in hits {
        if i64_at(&h.0, 0) != t.local {
            out.hits.push(h);
        }
    }
}

/// 0x00490651: every output mission with its `+0x18` word set: kind (`+0x25`) 1 opens the
/// dialog of the creature at `+8` (0x004882e0(creature, 1); the port's record keeps no id
/// there, so it never finds one), kind 2 on a known cell plays the quest tag and sound 0x1e.
pub fn mission_effects(t: &mut Target<'_>, out: &ServerUpdate, fx: &mut Vec<Effect>) {
    for m in &out.missions {
        let b = m.to_bytes();
        if i32_at(&b, 0x18) == 0 {
            continue;
        }
        if b[0x25] == 1 {
            let id = i64_at(&b, 8);
            if t.entities.contains_key(&id) {
                fx.push(Effect::Talk { id });
            }
        }
        if b[0x25] == 2 && t.world.cell(m.cell_x, m.cell_y).is_some() {
            fx.push(Effect::QuestComplete { cell: (m.cell_x, m.cell_y) });
            fx.push(Effect::Sound { id: 0x1e });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_net::packet::{Hit, Pickup};

    fn target_parts() -> (World, BTreeMap<i64, EntityData>, BTreeMap<i64, CreatureState>) {
        let mut w = World::new(1);
        w.local_player = Some(1);
        let mut entities = BTreeMap::new();
        let mut e = EntityData::new_creature();
        e.0[0x50] = 0;
        entities.insert(1, e);
        let mut states = BTreeMap::new();
        states.insert(1, CreatureState { inventory: cw_world::inventory::Inventory { pages: vec![Vec::new(); 4], gold: 0, f12c: 0 }, ..CreatureState::default() });
        (w, entities, states)
    }

    #[test]
    fn local_pickups_and_xp() {
        let (mut w, mut entities, mut states) = target_parts();
        let (mut proj, mut i8, mut dirty) = (Vec::new(), BTreeMap::new(), BTreeSet::new());
        let mut t = Target { world: &mut w, entities: &mut entities, states: &mut states, projectiles: &mut proj, items8: &mut i8, dirty_zones: &mut dirty, local: 1, text: None };
        let mut p = [0u8; 0x120];
        p[0..8].copy_from_slice(&1i64.to_le_bytes());
        p[8] = 1; // a consumable
        p[8 + 0x10] = 1;
        let mut xp = p;
        xp[8] = 0xd;
        xp[8 + 0x10] = 3;
        let u = ServerUpdate { pickups: vec![Pickup(p), Pickup(xp)], ..ServerUpdate::default() };
        let mut out = ServerUpdate::default();
        let fx = apply(&mut t, u, &mut out);
        assert_eq!(out.pickups.len(), 2);
        assert!(fx.contains(&Effect::RebuildRecipes));
        assert_eq!(i32_at(&entities[&1].0, 0x184), 30);
        // The consumable is a stack; the type-0xd item counts into `+0x12c` by its level.
        let count: i32 = states[&1].inventory.pages.iter().flatten().map(|s| s.count).sum();
        assert_eq!(count, 1);
        assert_eq!(states[&1].inventory.f12c, 3);
    }

    #[test]
    fn hits_on_the_local_player() {
        let (mut w, mut entities, mut states) = target_parts();
        wf32(&mut entities.get_mut(&1).unwrap().0, 0x15c, 100.0);
        let hp0 = 100.0;
        let (mut proj, mut i8, mut dirty) = (Vec::new(), BTreeMap::new(), BTreeSet::new());
        let mut t = Target { world: &mut w, entities: &mut entities, states: &mut states, projectiles: &mut proj, items8: &mut i8, dirty_zones: &mut dirty, local: 1, text: None };
        let mut h = [0u8; 0x48];
        h[0..8].copy_from_slice(&7i64.to_le_bytes());
        h[8..16].copy_from_slice(&1i64.to_le_bytes());
        h[0x10..0x14].copy_from_slice(&10.0f32.to_le_bytes());
        h[0x18..0x1c].copy_from_slice(&500i32.to_le_bytes());
        let mut other = h;
        other[8..16].copy_from_slice(&9i64.to_le_bytes());
        let u = ServerUpdate { hits: vec![Hit(h), Hit(other)], ..ServerUpdate::default() };
        let mut out = ServerUpdate::default();
        apply(&mut t, u, &mut out);
        assert_eq!(f32_at(&entities[&1].0, 0x15c), hp0 - 10.0);
        assert_eq!(i32_at(&entities[&1].0, 0x11c), 500);
        // Both hits come from creature 7, not the local player: both go out.
        assert_eq!(out.hits.len(), 2);
    }
}
