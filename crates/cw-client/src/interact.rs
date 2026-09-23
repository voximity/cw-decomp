//! The interaction half of `cube::GameController::update` (`Cube.exe 0x00488ee0`): what the
//! crosshair is on (creatures, statics, ground items, the aim point of the ray through the
//! world), the Interact packets the keys queue on the player (`creature+0x130c`, sent as
//! client packet 6 and run by the world tick's interaction switch), the pickups, the quick-item
//! menu and the consumables.
//!
//! Range map (Cube.exe), in frame order:
//!
//! | Range | Here |
//! |---|---|
//! | 0x00491f16..0x0049213b (the quick-item widget; Q at 0x004920ae) | [`quick_use_q`] |
//! | 0x0049662f..0x004966c9 | [`drop_held_item`] |
//! | 0x00496922..0x00496f37 | [`talk_or_mount`] |
//! | 0x0049736c..0x004975bb | [`r_key`] (the respawn is [`crate::player::LocalPlayer::respawn`]) |
//! | 0x004975c1..0x004976e7 | [`e_key`] |
//! | 0x004976e7..0x00497723 | [`t_key`] |
//! | 0x00497952..0x00497ea7 | [`quick_menu`] |
//! | 0x0049797d..0x004979c7 | [`build_r`] |
//! | 0x00498409..0x00499e9b | [`aim`] |
//! | `useItem` 0x004a2780 | [`use_item`] |
//! | `hasRoomFor` 0x0043e4a0 | [`has_room`] |
//! | `quickItems` 0x0047ae10 | [`quick_items`] |
//! | `canPickUp` 0x0043e550 | [`can_pick_up`] |
//! | `ModelCache::modelForItem` 0x004ec400 (with 0x004ec370, 0x0051be60) | [`item_model_index`] |
//!
//! The aim ray is the shared `World::sweep` (`Cube.exe 0x005a35d0`, `Server.exe 0x004d6730`,
//! [`cw_sim::path::sweep`]) and the visibility tests the shared `World::lineOfSight`
//! (`Cube.exe 0x0059ee90`, [`cw_sim::path::line_of_sight`]).

#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::too_many_arguments, clippy::too_many_lines, clippy::nonminimal_bool, clippy::collapsible_if, clippy::assign_op_pattern, clippy::needless_late_init)]
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_net::packet::Interact;
use cw_sim::combat::CreatureState;
use cw_world::World;
use cw_world::inventory::{Inventory, Item};

use crate::input::ControllerBytes;
use crate::player::{Camera, Event, LocalPlayer, Mat4, UiState, add3, blocks3, ent, f32_at, fix3, fixed_dot, i32_at, i64_at, len_sq, length, normalize, pos_of, sub3, wi32, wi64, K};

/// `cube::Interact::Interact` 0x00459530 with its item, target and type: the 0x12c-byte packet
/// (item at `+0`, target `(zone x, zone y, index)` at `+0x118`, type at `+0x128`).
pub fn interact_packet(kind: u8, target: Option<[i32; 3]>, item: Option<&[u8]>) -> Interact {
    let mut b = [0u8; Interact::SIZE];
    // Constructor: an empty item (+0x10 = 1, the level), target (-1, -1, -1), +0x12a = 1.
    b[0x10] = 1;
    b[0x118..0x124].fill(0xff);
    b[0x12a] = 1;
    if let Some(it) = item {
        b[..Item::SIZE].copy_from_slice(&it[..Item::SIZE]);
    }
    if let Some(t) = target {
        for (i, v) in t.iter().enumerate() {
            b[0x118 + i * 4..0x11c + i * 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    b[0x128] = kind;
    Interact(b)
}

// ---------------------------------------------------------------------------------------------
// Items and the inventory.

/// `0x0043e550` (`canPickUp`): a type-0x19 item (a key) only when it matches the player's
/// `entity+0x1154` (above 100 reduced to `100 + n % 100`).
pub fn can_pick_up(e: &EntityData, item: &[u8]) -> bool {
    if item[0] != 0x19 {
        return true;
    }
    let mut v = i32_at(&e.0, 0x1154);
    if v > 100 {
        v = v + (v / 100) * -100 + 100;
    }
    i32_at(item, 4) == v
}

/// `0x0043e4a0` (`hasRoomFor`): consumables (1), type 0xb and type 0x14 items stack; the count
/// of equal items held by the cursor and in every inventory page must stay below 50 (0x14:
/// none held).
pub fn has_room(inv: &Inventory, held_count: i32, held_item: &[u8], item: &[u8]) -> bool {
    let t = item[0];
    if t == 1 || t == 0xb || t == 0x14 {
        let it = Item::from_bytes(item);
        let mut n = 0;
        if Inventory::same_item(&Item::from_bytes(held_item), &it) {
            n = held_count;
        }
        for page in &inv.pages {
            for s in page {
                if Inventory::same_item(&it, &s.item) {
                    n += s.count;
                }
            }
        }
        if (t == 0x14 && 0 < n) || 0x31 < n {
            return false;
        }
    }
    true
}

/// `0x0047ae10`: the quick items, `(page, slot)` of every non-empty slot holding a consumable
/// (type 1) the player's level allows, pages then slots in order.
pub fn quick_items(e: &EntityData, inv: &Inventory) -> Vec<(usize, usize)> {
    let level = i32_at(&e.0, ent::LEVEL);
    let mut out = Vec::new();
    for (p, page) in inv.pages.iter().enumerate() {
        for (s, slot) in page.iter().enumerate() {
            if slot.count != 0 && slot.item.item_type == 1 && i32::from(slot.item.level as i16) <= level {
                out.push((p, s));
            }
        }
    }
    out
}

/// `0x004a2780` (`useItem`): a consumable, when not stunned: sub type 7 is queued as an
/// Interact of type 1 (the tick turns it into a placed creature), anything else becomes the
/// item being consumed (`entity+0x1d8`) in mode 0x50 (sub types 1, 2: drinks) or 0x51, and the
/// roll stops. The stack loses one; an emptied slot loses its item.
pub fn use_item(p: &mut LocalPlayer, e: &mut EntityData, inv: &mut Inventory, page: usize, slot: usize) {
    let s = &mut inv.pages[page][slot];
    if s.item.item_type != 1 || i32_at(&e.0, ent::STUN) > 0 {
        return;
    }
    let bytes = s.item.to_bytes();
    if s.item.sub_type != 7 {
        e.0[ent::CONSUMING..ent::CONSUMING + Item::SIZE].copy_from_slice(&bytes);
        e.0[ent::MODE] = if matches!(s.item.sub_type, 1 | 2) { 0x50 } else { 0x51 };
        wi32(&mut e.0, ent::MODE_TIME, 0);
        wi32(&mut e.0, ent::ROLL, 0);
    } else {
        p.interact_queue.push(interact_packet(1, None, Some(&bytes)));
    }
    s.count -= 1;
    if s.count <= 0 {
        s.item.sub_type = 0;
        s.item.item_type = 0;
        s.count = 0;
    }
}

/// The quick-item index wrapped into the list (0x00491f6f: a negative index is first raised by
/// whole list lengths, unsigned arithmetic as the original).
fn wrap_index(i: i32, n: usize) -> usize {
    let n = n as u32;
    let mut v = i as u32;
    if i < 0 {
        let q = (i.wrapping_neg() as u32) / n;
        v = ((q + 1).wrapping_mul(n).wrapping_add(i as u32)) % n;
    }
    (v % n) as usize
}

/// 0x00491f16..0x0049213b: the quick-item list; empty, the menu closes. With the cursor
/// captured, Q (edge) uses the selected item, or plays sound 0x32 when the level is too low.
pub fn quick_use_q(p: &mut LocalPlayer, e: &mut EntityData, st: &mut CreatureState, input: &ControllerBytes, ui: &UiState) {
    let list = quick_items(e, &st.inventory);
    if list.is_empty() {
        p.quick_open = false;
    }
    if !list.is_empty() && !ui.cursor_free {
        let (pg, sl) = list[wrap_index(p.quick_index, list.len())];
        if input.key_q() && !p.latches.q {
            let lv = i32::from(st.inventory.pages[pg][sl].item.level as i16);
            if i32_at(&e.0, ent::LEVEL) < lv {
                p.events.push(Event::Sound { id: 0x32, pos: p.camera.eye, volume: 1.0, pitch: 1.0 });
            } else {
                use_item(p, e, &mut st.inventory, pg, sl);
            }
        }
        // 0x0049266a: the latch is only kept while the list shows.
        p.latches.q = input.key_q();
    }
}

/// 0x00497952..0x0049797d, before the build-mode test: the navigation latch survives only while
/// the menu is open, and W or S close it.
pub fn quick_menu_prelude(p: &mut LocalPlayer, input: &ControllerBytes) {
    if !p.quick_open {
        p.quick_nav_latch = false;
    }
    if input.forward() || input.back() {
        p.quick_open = false;
    }
}

/// 0x004979cc..0x00497ea7: the quick-item menu (Tab), after [`quick_menu_prelude`] and
/// [`build_r`]. W or S closes it. A (or the stick left)
/// and D (or right) move the selection with sound 0x55, repeating every 200 ms while held; with
/// neither, the navigation latch is set. R or E (edge) uses the selection (sound 0x32 when the
/// level is too low) and closes the menu. Returns true when the menu consumed the frame's
/// movement (the original jumps past the movement input).
pub fn quick_menu(p: &mut LocalPlayer, e: &mut EntityData, st: &mut CreatureState, input: &ControllerBytes, dt: i32) -> bool {
    if !p.quick_open {
        return false;
    }
    let latch = p.quick_nav_latch;
    let left = input.left() || input.left_stick[0] < 0;
    let right = input.right() || 0 < input.left_stick[0];
    let mut r0 = if left { p.quick_repeat[0] } else { 0 };
    let mut r1 = if right { p.quick_repeat[1] } else { 0 };
    if !left && !right {
        p.quick_nav_latch = true;
    } else if latch {
        if left && r0 <= 0 {
            p.quick_index -= 1;
            r0 = 200;
            p.events.push(Event::Sound { id: 0x55, pos: p.camera.eye, volume: 1.0, pitch: 1.0 });
        }
        if right && r1 <= 0 {
            p.quick_index += 1;
            r1 = 200;
            p.events.push(Event::Sound { id: 0x55, pos: p.camera.eye, volume: 1.0, pitch: 1.0 });
        }
    } else {
        p.quick_repeat = [r0, r1];
        return quick_menu_use(p, e, st, input);
    }
    r0 -= dt;
    if r0 < 0 {
        r0 = 0;
    }
    r1 -= dt;
    if r1 < 0 {
        r1 = 0;
    }
    p.quick_repeat = [r0, r1];
    quick_menu_use(p, e, st, input)
}

/// 0x00497b0b..0x00497ea2: the use part of [`quick_menu`].
fn quick_menu_use(p: &mut LocalPlayer, e: &mut EntityData, st: &mut CreatureState, input: &ControllerBytes) -> bool {
    let list = quick_items(e, &st.inventory);
    if list.is_empty() {
        return true;
    }
    let (pg, sl) = list[wrap_index(p.quick_index, list.len())];
    if (input.key_r() && !p.latches.r) || (input.key_e() && !p.latches.e) {
        let lv = i32::from(st.inventory.pages[pg][sl].item.level as i16);
        if lv > i32_at(&e.0, ent::LEVEL) {
            p.events.push(Event::Sound { id: 0x32, pos: p.camera.eye, volume: 1.0, pitch: 1.0 });
            return true;
        }
        use_item(p, e, &mut st.inventory, pg, sl);
        p.quick_open = false;
    }
    true
}

/// 0x0049662f..0x004966c9: a left click (edge: `GC+4` down, the latch 0x0076b13b up) with a
/// stack on the cursor (`creature+0x11e8 != 0`), the inventory panel visible (0x0047fa10 on
/// `GC+0x8008bc`) and no widget under the cursor (0x00650ae0 returns 0) drops ONE item: an
/// Interact (ctor 0x00459530) carrying a copy of the held item (0x0042c5e0 from
/// `creature+0x11ec`) is queued on `creature+0x130c` (0x00486100), then [`take_one`]
/// (0x0042f140) takes one off the held stack. The server drops one item per packet.
///
/// The packet's type byte (`+0x128`) is not written on this path: the Interact ctor leaves
/// it alone, so the original sends whatever its stack slot (`[esp+0x4074]` of `update`) held.
/// The port sends 6 (drop), the value the server's interaction switch expects for a drop.
pub fn drop_held_item(p: &mut LocalPlayer, input: &ControllerBytes, ui: &UiState) {
    if input.left_mouse() && !p.latches.left_mouse && p.held_count != 0 && ui.inventory_open && !ui.widget_hovered {
        let item = p.held_item;
        p.interact_queue.push(interact_packet(6, None, Some(&item)));
        take_one(&mut p.held_count, &mut p.held_item);
    }
    p.latches.left_mouse = input.left_mouse();
}

/// `0x0042f140` (`ItemStack::takeOne`, `dec [ecx]; cmp [ecx], 0; jg`): one off the count; at
/// zero or below the count is 0 and the item's type and sub type (`Item+0`, `+1`, the word at
/// stack `+4`) are cleared; the rest of the item is kept. (`ui::inventory::take_one` is the
/// same function on a [`crate::ui::ItemStack`].)
pub fn take_one(count: &mut i32, item: &mut [u8]) {
    *count = count.wrapping_sub(1);
    if *count <= 0 {
        *count = 0;
        item[0] = 0;
        item[1] = 0;
    }
}

/// 0x00496922..0x00496f37 without the prompts: R (edge) on the aimed creature. A villager
/// (hostile 3), with the cursor captured: talk (Interact 2 with its spawn triple
/// `entity+0x1a0`, 0x004969bf..0x004969fa; then its speech bubble, `0x004882e0(creature, 0)`
/// at 0x00496a4f, through [`Event::Talk`] and `ui::bubbles`). A ridable pet of the player's (hostile 5, a type
/// `0x00444760` accepts, the player's `entity+0x112c` set, the pet's owner the player, not
/// already riding): mode 0x6a (mount).
pub fn talk_or_mount(p: &mut LocalPlayer, entities: &mut BTreeMap<i64, EntityData>, input: &ControllerBytes, ui: &UiState) {
    let pressed = input.key_r() && !p.latches.r;
    let Some(c) = entities.get(&p.aimed_creature).cloned() else { return };
    let hostile = c.0[ent::HOSTILE];
    if hostile == 3 && !ui.cursor_free {
        if pressed {
            let t = [i32_at(&c.0, 0x1a0), i32_at(&c.0, 0x1a4), i32_at(&c.0, 0x1a8)];
            p.interact_queue.push(interact_packet(2, Some(t), None));
            p.events.push(Event::Talk { id: p.aimed_creature });
        }
        return;
    }
    if hostile == 5 && ridable_type(i32_at(&c.0, 0x54)) {
        let Some(me) = entities.get_mut(&p.id) else { return };
        if i32_at(&me.0, 0x112c) != 0 && i64_at(&c.0, 0x188) == p.id && me.0[ent::MODE] != 0x6a && pressed {
            me.0[ent::MODE] = 0x6a;
            wi32(&mut me.0, ent::MODE_TIME, 0);
        }
    }
}

/// `0x00444760`: the creature types that can be ridden.
pub fn ridable_type(t: i32) -> bool {
    matches!(t, 0x13 | 0x14 | 0x16 | 0x17 | 0x19..=0x28 | 0x3f..=0x43 | 0x4a | 0x4b | 0x62 | 99 | 100 | 0x66..=0x69 | 0x97)
}

/// 0x0049736c..0x004975bb (alive; the dead case is the respawn): R (edge) with the quick menu
/// closed. `0x004889e0` first (a creature menu for the aimed creature), then the aimed static:
/// a bed (0x2d) opens the map, 0x4d and the crafting kinds (0x41, 0x47..0x4c) open their
/// panels, anything else is used (Interact 3 with its triple) and examined ([`Event::Examine`]).
pub fn r_key(p: &mut LocalPlayer, world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, input: &ControllerBytes) {
    if p.quick_open || !(input.key_r() && !p.latches.r) {
        return;
    }
    // 0x00497383: dead, R respawns.
    if entities.get(&p.id).is_some_and(|e| 0.0 >= f32_at(&e.0, ent::HP)) {
        p.respawn(world, entities, states);
        return;
    }
    // 0x004889e0: the aimed creature (`+0x800a70`) becomes the menu creature.
    if p.aimed_creature != 0 && entities.contains_key(&p.aimed_creature) {
        p.menu_creature = p.aimed_creature;
        let class = entities[&p.aimed_creature].0[ent::CLASS];
        if matches!(class, 0x80..=0x83 | 0x89) {
            p.events.push(Event::CreatureMenu { id: p.aimed_creature });
        }
    } else {
        p.menu_creature = 0;
        // `0x0046fac0` (a creature with no owner that the player then owns) is not ported.
    }
    if p.aimed_static[0] < 0 {
        return;
    }
    let Some(s) = static_at(world, p.aimed_static) else { return };
    match s.kind {
        0x2d => p.events.push(Event::OpenMapFromBed),
        0x4d => p.events.push(Event::OpenStaticPanel { kind: 0x4d }),
        0x41 | 0x47..=0x4c => p.events.push(Event::OpenCraftingPanel { kind: s.kind }),
        _ => {
            // 0x004974d6..0x00497511: Interact 3 with the triple; 0x0049755e: `0x00488030(static,
            // 0)`, the examine bubble (the `+0x800940` branch between them is dead).
            p.interact_queue.push(interact_packet(3, Some(p.aimed_static), None));
            p.events.push(Event::Examine { kind: s.kind });
        }
    }
}

/// `0x005a0910`: the static of a (zone x, zone y, index) triple.
pub fn static_at(world: &World, t: [i32; 3]) -> Option<&cw_world::zone::Static> {
    let z = world.zone(t[0], t[1])?;
    if t[2] < 0 {
        return None;
    }
    z.statics.get(t[2] as usize)
}

/// `0x0059fb90`: the ground item of a triple.
pub fn ground_item_at(world: &World, t: [i32; 3]) -> Option<&cw_world::zone::GroundItem> {
    let z = world.zone(t[0], t[1])?;
    if t[2] < 0 {
        return None;
    }
    z.items.get(t[2] as usize)
}

/// 0x004975c1..0x004976e7: E (edge) on the aimed ground item picks it up (Interact 5) when the
/// stacks have room, else "inventory full" and sound 0x31.
pub fn e_key(p: &mut LocalPlayer, world: &World, e: &EntityData, st: &CreatureState, input: &ControllerBytes) {
    if !(input.key_e() && !p.latches.e) || p.aimed_item[0] < 0 {
        return;
    }
    let Some(g) = ground_item_at(world, p.aimed_item) else { return };
    let item = g.item.to_bytes();
    if has_room(&st.inventory, p.held_count, &p.held_item, &item) {
        p.interact_queue.push(interact_packet(5, Some(p.aimed_item), None));
    } else {
        p.events.push(Event::InventoryFull);
        p.events.push(Event::Sound { id: 0x31, pos: pos_of(e), volume: 1.0, pitch: 1.0 });
    }
}

/// 0x004976e7..0x00497723: T (edge) calls the pet (Interact 8).
pub fn t_key(p: &mut LocalPlayer, input: &ControllerBytes) {
    if input.key_t() && !p.latches.t {
        p.interact_queue.push(interact_packet(8, None, None));
    }
}

/// 0x0049797d..0x004979c7: in build mode R (edge) builds (Interact 7). Returns true in build
/// mode (the movement input is skipped).
///
/// Build mode is the byte `GC+0x800704` ([`LocalPlayer::build_mode`]). A scan of every
/// instruction of Cube.exe finds one write, the constructor's `mov byte [ebx+0x800704], 0`
/// (0x0045a09b), and five reads: here (0x0049797d), the right-click prelude of `onMouseDown`
/// (0x0047ccff: in build mode a right click increments the player's `creature+0x18c`, the
/// build block type, instead of the customization pick; `ui::voxel::right_click_prelude`),
/// 0x0049b18d and 0x0049b5b1 in `update` (the build aim and the build placement), and
/// `render` 0x004b3338 (the `build-cursor.cub` model, name at 0x00700c4c, coloured by
/// `creature+0x18d`). Nothing sets it, so it is a disabled developer mode: always false in
/// this build.
pub fn build_r(p: &mut LocalPlayer, input: &ControllerBytes) -> bool {
    if !p.build_mode {
        return false;
    }
    if input.key_r() && !p.latches.r {
        p.interact_queue.push(interact_packet(7, None, None));
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The aim.

/// The result of the aim pass beyond the `LocalPlayer` fields it sets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Aim {
    /// `[esp+0x14d8]`: where the crosshair meets the world (fixed), relative to nothing; the
    /// ray-hit field of the player is this minus its position.
    pub point: [i64; 3],
    /// `[esp+0xd4]`: the sweep's distance (negative when it found nothing).
    pub ray: f32,
    /// The creatures whose projected box holds the crosshair (the map value's `+8` byte, which
    /// the name plates read).
    pub highlighted: BTreeSet<i64>,
    /// `[esp+0x7c]`/`[esp+0x34]`: the creature on the crosshair nearest to the player (the
    /// target a charging mode 0x22 takes, [`aim_targets`]).
    pub nearest_on_crosshair: i64,
}

/// The item-model sizes the item pass projects (`cube::Model` `+0x44`, `+0x48`, `+0x4c`),
/// supplied by the render side's model cache ([`item_model_index`] gives the index).
pub trait ItemModels {
    fn size(&self, model_index: u32) -> Option<[i32; 3]>;
}

/// 0x00498409..0x00499e9b: the aim. The ray from the camera target along the view (`world
/// sweep`, 60 blocks through air, statics included) gives the aim point (100 blocks when it
/// hits nothing). The lock-on, the aimed creature, static and item are cleared, then:
/// creatures (lit enough to be seen, in sight of the eye) whose box projects around the screen
/// centre: the nearest to the eye is aimed; the nearest within 4 blocks of the player is the
/// fallback; an enemy moves the aim point to its distance along the ray. Statics of the 3x3
/// cells around the player within 4 blocks, and ground items of the 3x3 zones around
/// `center_zone` within 4 blocks, compete by distance to the eye (an item there is no room for
/// counts 16 more).
pub fn aim(p: &mut LocalPlayer, world: &World, entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, models: &dyn ItemModels, dt: i32, center_zone: [i32; 2]) -> Aim {
    let cam = p.camera.clone();
    let proj = p.projection;
    let mut dir = cam.model.mul_dir([0.0, 0.0, 1.0]);
    normalize(&mut dir);
    let mut out = Aim { point: add3(cam.target, fix3([dir[0] * 100.0, dir[1] * 100.0, dir[2] * 100.0])), ..Aim::default() };
    let t = cw_sim::path::sweep(world, cam.target, dir, 60.0, false, true);
    out.ray = t;
    if t >= 0.0 {
        out.point = add3(cam.target, fix3([dir[0] * t, dir[1] * t, dir[2] * t]));
    }
    // 0x004984fb: the resets.
    p.lock_target = 0;
    p.f8006d4 = 0;
    p.aimed_creature = 0;
    p.aimed_static = [-1, -1, 0];
    p.aimed_item = [-1, -1, 0];
    let Some(player) = entities.get(&p.id).cloned() else { return out };
    let ppos = pos_of(&player);
    let mut near_creature = 0i64;
    let mut near_static = [-1, -1, 0];
    let mut near_item = [-1, -1, 0];
    let mut best_cam = -1.0f32;
    let mut best_enemy = -1.0f32;
    let mut best_near = -1.0f32;
    let mut best_item = -1.0f32;
    let mut best_any = -1.0f32;
    let mut nearest_any = 0i64;
    // 0x004985a0: the daylight, `(1 - (t * 2 / 86400000 - 1)^4)^5`.
    let tod = world.time_of_day as f32 * 2.0f32 / 86_400_000.0f32 - 1.0f32;
    let day = cw_math::pow(f64::from(1.0f32 - cw_math::pow(f64::from(tod), 4.0) as f32), 5.0) as f32;
    let in_frame = |corner_world_rel: [f32; 3], flags: &mut [bool; 4]| {
        let v = cam.view_shake.transform_point(corner_world_rel);
        if v[2] > 0.0 {
            let s = proj.transform_point(v);
            flags[0] |= 0.0 > s[0];
            flags[1] |= 0.0 > s[1];
            flags[2] |= s[0] > 0.0;
            flags[3] |= s[1] > 0.0;
        }
    };
    let pmode = player.0[ent::MODE];
    let (guard, haste) = cw_sim::util::guard_haste(states, p.id);
    let charging = pmode == 0x22 && i32_at(&player.0, ent::MODE_TIME) < cw_sim::skills::skill_total_time(&player, guard, haste);
    // 0x00498690: the creatures.
    for (id, c) in entities {
        if *id == p.id {
            continue;
        }
        if charging && matches!(c.0[ent::HOSTILE], 6 | 1) {
            continue;
        }
        let cpos = pos_of(c);
        let light = ground_light_at(world, cpos);
        let lf = (light as f32 / 255.0f32) * day;
        let mut range = 256.0f32;
        if 0.3f32 > lf {
            range = lf * 0.9f32 / 0.3f32 * 256.0f32 + 25.6f32;
        }
        let cst = states.get(id).cloned().unwrap_or_default();
        let render = cst.riding.render_pos;
        let foot = add3(render, fix3([0.0, 0.0, -cst.step_offset]));
        if !cw_sim::path::line_of_sight(world, cam.eye, foot, false, range) {
            continue;
        }
        let sc = crate::player::vec3_at(&c.0, 0x70);
        let size = [sc[0] + 1.0f32, sc[1] + 1.0f32, sc[2] + 1.0f32];
        let mut f = [false; 4];
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    let off = [crate::player::fix(i as f32 * size[0] - size[0] * 0.5f32), crate::player::fix(j as f32 * size[1] - size[1] * 0.5f32), crate::player::fix(k as f32 * size[2] - size[2] * 0.5f32)];
                    let corner = add3(add3(add3(foot, [cam.origin[0], cam.origin[1], 0]), off), [0, 0, 0]);
                    in_frame(blocks3(corner), &mut f);
                }
            }
        }
        let dp = sub3(ppos, render);
        let d_player = fixed_dot(dp, dp) as f32 * K;
        if 16.0f32 > d_player && (0.0 > best_near || best_near > d_player) {
            best_near = d_player;
            near_creature = *id;
        }
        if f.iter().all(|x| *x) {
            out.highlighted.insert(*id);
            let de = sub3(cam.eye, render);
            let d_cam = fixed_dot(de, de) as f32 * K;
            if 0.0 > best_cam || best_cam > d_cam {
                best_cam = d_cam;
                p.aimed_creature = *id;
            }
            if cw_sim::util::is_enemy(&player, c) && (0.0 > best_enemy || best_enemy > d_cam) {
                best_enemy = d_cam;
            }
            if 0.0 > best_any || best_any > d_player {
                best_any = d_player;
                nearest_any = *id;
            }
        }
    }
    // 0x00498d48: an enemy on the crosshair pulls the aim point to its distance along the ray
    // from the eye (not while mode 0x31 runs).
    if best_enemy >= 0.0 && pmode != 0x31 {
        let d = crate::player::sqrt_f(best_enemy);
        let cand = add3(cam.eye, fix3([dir[0] * d, dir[1] * d, dir[2] * d]));
        let v = blocks3(sub3(cand, ppos));
        if v[1] * dir[1] + v[0] * dir[0] + v[2] * dir[2] > 0.0 {
            out.point = cand;
        }
    }
    // 0x00498e22: the charged-skill target ([`aim_targets`] writes it).
    out.nearest_on_crosshair = nearest_any;
    // 0x00498e7c: the poison modes 0x1e..0x21 record the aim point during the wind-up and hold
    // it after.
    if matches!(pmode, 0x1e..=0x21) {
        let w = cw_sim::skills::skill_windup(&player, guard, haste, -1);
        let st = states.entry(p.id).or_default();
        if i32_at(&player.0, ent::MODE_TIME) - dt <= w {
            st.modes.poison_origin = out.point;
        } else {
            out.point = st.modes.poison_origin;
        }
    }
    // 0x00498ed1: the statics of the 3x3 cells around the player.
    let cx = ((ppos[0] / 0x10000) as i32) / 8;
    let cy = ((ppos[1] / 0x10000) as i32) / 8;
    for x in cx - 1..=cx + 1 {
        for y in cy - 1..=cy + 1 {
            if x < 0 || y < 0 || x > 0x1f_ffff || y > 0x1f_ffff {
                continue;
            }
            let (zx, zy) = (x >> 5, y >> 5);
            let Some(zone) = world.zone(zx, zy) else { continue };
            for (idx, s) in zone.statics.iter().enumerate() {
                // The cell list (`zone+0xac`, 32x32 cells of 8 blocks): the statics whose
                // position falls in the cell, in zone order.
                if ((s.x >> 16) as i32) / 8 != x || ((s.y >> 16) as i32) / 8 != y {
                    continue;
                }
                let spos = [s.x, s.y, s.z];
                let dp = sub3(ppos, spos);
                let d_player = fixed_dot(dp, dp) as f32 * K;
                if d_player > 16.0f32 {
                    continue;
                }
                let mid = fix3([0.0, 0.0, s.scale[2] * 0.5f32]);
                let de = sub3(sub3(cam.eye, spos), mid);
                let d_eye = fixed_dot(de, de) as f32 * K;
                let triple = [zx, zy, idx as i32];
                if 0.0 > best_near || best_near > d_player {
                    best_near = d_player;
                    near_creature = 0;
                    near_static = triple;
                }
                if !(0.0 > best_cam || best_cam > d_eye) {
                    continue;
                }
                if !cw_sim::path::line_of_sight(world, cam.eye, add3(spos, mid), false, 200.0) {
                    continue;
                }
                let mut dims = s.scale;
                if s.rotation % 2 != 0 {
                    dims.swap(0, 1);
                }
                let mut f = [false; 4];
                for i in 0..2 {
                    for j in 0..2 {
                        for k in 0..2 {
                            let off = [crate::player::fix(i as f32 * dims[0] - dims[0] * 0.5f32), crate::player::fix(j as f32 * dims[1] - dims[1] * 0.5f32), crate::player::fix(k as f32 * dims[2])];
                            let corner = add3(add3(spos, [cam.origin[0], cam.origin[1], 0]), off);
                            in_frame(blocks3(corner), &mut f);
                        }
                    }
                }
                if f.iter().all(|x| *x) {
                    p.aimed_creature = 0;
                    best_cam = d_eye;
                    p.aimed_static = triple;
                }
            }
        }
    }
    // 0x00499538: nothing is aimed at while dead.
    if !(0.0 < f32_at(&player.0, ent::HP)) {
        p.aimed_creature = 0;
        p.aimed_static = [-1, -1, 0];
    }
    let my_inventory = states.get(&p.id).map(|s| s.inventory.clone()).unwrap_or_default();
    // 0x00499594: the ground items of the 3x3 zones around `center_zone` (`GameController+0x2bc`).
    for zx in center_zone[0] - 1..=center_zone[0] + 1 {
        for zy in center_zone[1] - 1..=center_zone[1] + 1 {
            let Some(zone) = world.zone(zx, zy) else { continue };
            for (idx, g) in zone.items.iter().enumerate() {
                let item = g.item.to_bytes();
                if !can_pick_up(&player, &item) {
                    continue;
                }
                let gpos = [g.x, g.y, g.z];
                let mut d_near = len_sq(blocks3(sub3(ppos, gpos)));
                if d_near > 16.0f32 || len_sq(blocks3(sub3(ppos, gpos))) > 100.0f32 {
                    continue;
                }
                let mut d_eye = len_sq(blocks3(sub3(cam.eye, gpos)));
                if !has_room(&my_inventory, p.held_count, &p.held_item, &item) {
                    d_eye = d_eye + 16.0f32;
                    d_near = d_near + 16.0f32;
                }
                let triple = [zx, zy, idx as i32];
                if 0.0 > best_item || best_item > d_near {
                    best_item = d_near;
                    near_static = [-1, -1, 0];
                    near_item = triple;
                    near_creature = 0;
                }
                if !(0.0 > best_cam || best_cam > d_eye) {
                    continue;
                }
                if !cw_sim::path::line_of_sight(world, cam.eye, gpos, false, 200.0) {
                    continue;
                }
                let Some(model) = item_model_index(&item).and_then(|m| models.size(m)) else { continue };
                let m = item_matrix(&cam, g, model);
                let mut f = [false; 4];
                for a in 0..2 {
                    for b in 0..2 {
                        for c in 0..2 {
                            let v = [(model[0].wrapping_mul(a)) as f32, (model[1].wrapping_mul(b)) as f32, (model[2].wrapping_mul(c)) as f32];
                            in_frame(m.transform_point(v), &mut f);
                        }
                    }
                }
                if f.iter().all(|x| *x) {
                    best_cam = d_eye;
                    p.aimed_item = triple;
                    p.aimed_creature = 0;
                    p.aimed_static = [-1, -1, 0];
                }
            }
        }
    }
    // 0x00499d1c: the fallbacks within reach.
    if p.aimed_creature == 0 && p.aimed_static[0] < 0 {
        if near_creature != 0 {
            p.aimed_creature = near_creature;
        }
        if near_static[0] >= 0 {
            p.aimed_static = near_static;
        }
    }
    if p.aimed_item[0] < 0 && near_item[0] >= 0 {
        p.aimed_item = near_item;
    }
    // 0x00499db3: a lock-on nearer than the ray's hit moves the aim point onto it.
    if t >= 0.0 && p.lock_target != 0 {
        if let Some(l) = entities.get(&p.lock_target) {
            let d = blocks3(sub3(pos_of(l), cam.target));
            if t * t > len_sq(d) {
                let n = length(d);
                out.point = add3(cam.target, fix3([dir[0] * n, dir[1] * n, dir[2] * n]));
            }
        }
    }
    out
}

/// The charged-skill target and the Shift self-target of 0x00498e22..0x00498e7c, applied to the
/// player block (split from [`aim`] so the aim pass can stay read-only on the entities).
pub fn aim_targets(p: &LocalPlayer, e: &mut EntityData, states: &BTreeMap<i64, CreatureState>, input: &ControllerBytes, nearest_on_crosshair: i64) {
    let (guard, haste) = cw_sim::util::guard_haste(states, p.id);
    if e.0[ent::MODE] == 0x22 && nearest_on_crosshair != 0 && i32_at(&e.0, ent::MODE_TIME) < cw_sim::skills::skill_windup(e, guard, haste, -1) {
        wi64(&mut e.0, ent::TARGET, nearest_on_crosshair);
    }
    if input.shift() {
        wi64(&mut e.0, ent::TARGET, p.id);
    }
}

/// The matrix of a ground item's model (0x0049984a..0x00499a34): at its position relative to
/// the render origin, scaled by `+0x134`; weapons and armour (types 3, 4) turned by the item's
/// rotation and laid down (`+0.5 x` up, 90 degrees about y, centred in z); types 0xc and 0xd
/// laid down without the rotation; the rest turned and centred in x and y.
fn item_matrix(cam: &Camera, g: &cw_world::zone::GroundItem, size: [i32; 3]) -> Mat4 {
    let mut m = Mat4::IDENTITY;
    let x = g.x.wrapping_add(cam.origin[0]) as f32 * K;
    let y = g.y.wrapping_add(cam.origin[1]) as f32 * K;
    let z = g.z as f32 * K;
    m.translate([x, y, z]);
    m.scale(g.f134, g.f134, g.f134);
    let t = g.item.item_type;
    let zoff;
    match t {
        3 | 4 | 0xc | 0xd => {
            if matches!(t, 3 | 4) {
                m.rotate_z(g.rotation);
            }
            m.translate([0.0, 0.0, size[0] as f32 * 0.5f32]);
            m.rotate_y(90.0);
            zoff = size[2] as f32 * -0.5f32;
        }
        _ => {
            m.rotate_z(g.rotation);
            zoff = 0.0;
        }
    }
    m.translate([size[0] as f32 * -0.5f32, size[1] as f32 * -0.5f32, zoff]);
    // The corners go through this matrix, then (in [`aim`]) through `view_shake` and the
    // projection, each `0x004248a0` with its own `w` divide.
    m
}

/// `groundLightAt` 0x004718b0 (`Server.exe 0x004d24a0`) at a fixed position (each axis `/65536`
/// truncating): a type-0xd block 255, air or water its first byte (at least 5), else 0.
pub fn ground_light_at(world: &World, p: [i64; 3]) -> u8 {
    let b = world.block((p[0] / 65536) as i32, (p[1] / 65536) as i32, (p[2] / 65536) as i32);
    let t = b[3] & 0x1f;
    if t == 0xd {
        return 0xff;
    }
    if t != 0 && t != 2 {
        return 0;
    }
    b[0].max(5)
}

// ---------------------------------------------------------------------------------------------
// Item models.

/// `0x004ec370`: the model of a consumable by sub type.
fn consumable_model(sub: u8) -> u32 {
    match sub {
        0 => 0x354,
        1 => 0x351,
        2 => 0x352,
        3 => 0x353,
        4 => 0x95b,
        5 => 0x82c,
        6 => 0x959,
        7 => 0x9f2,
        8 => 0x91f,
        9 => 0x91d,
        _ => 0x34b,
    }
}

/// `0x0051be60(sub, modifier, rarity, material)`: the model of a weapon.
pub fn weapon_model(sub: u8, modifier: i32, rarity: u8, material: u8) -> u32 {
    let r = |base: i32| (if (rarity as i32) < 4 { rarity as i32 } else { 4 }) + base;
    let m11 = modifier % 0xb;
    let v = match sub {
        0 => {
            if material == 5 {
                0x80b
            } else if material == 7 {
                0x818
            } else {
                r(899 + m11 * 5)
            }
        }
        1 => {
            if material == 5 {
                0x80c
            } else if material == 7 {
                0x81a
            } else {
                r((m11 + 0x162) * 5)
            }
        }
        2 => {
            if material == 2 {
                r(0x298)
            } else if material == 7 {
                0x819
            } else {
                r(0x261 + m11 * 5)
            }
        }
        3 => r(0x3ba + m11 * 5),
        4 => r(0x3f1 + m11 * 5),
        5 => r((m11 + 0x178) * 5),
        6 => r(0x2a2 + m11 * 5),
        7 => r(0x2d9 + m11 * 5),
        8 => r(0x310 + m11 * 5),
        9 => 0x348,
        10 => {
            if material == 5 {
                r(0x25c)
            } else {
                r(0x1b2 + m11 * 5)
            }
        }
        0xb => r(0x1e9 + m11 * 5),
        0xc => {
            let m6 = (modifier % 6) * 5;
            if material != 0xb { r(0x23e + m6) } else { r(0x220 + m6) }
        }
        0xd => {
            if material == 2 {
                r(0x45f)
            } else {
                r(0x428 + m11 * 5)
            }
        }
        0xe => 0x347,
        0xf => {
            if material == 5 {
                0x790
            } else if material == 7 {
                0x78f
            } else {
                r((m11 + 0x16d) * 5)
            }
        }
        0x10 => match material {
            5 => 0x7c9,
            7 => 0x7c8,
            0x12 => 0x7ca,
            _ => r(0x791 + m11 * 5),
        },
        0x11 => match material {
            2 => 0x7cb,
            5 => 0x805,
            7 => 0x804,
            _ => r(0x7cc + m11 * 5),
        },
        0x12 => 0x91b,
        0x13 => 0x803,
        0x14 => 0x34a,
        _ => 899,
    };
    v as u32
}

/// `ModelCache::modelForItem` 0x004ec400 (the task's "unresolved `0x004ec400`": it is not speech
/// drawing): the index of an item's model in the model cache (`GameController+0x300`, a vector
/// of `cube::Model*`), or `None` for items without one. The fixed-offset cases of the original
/// (`*(vector + off)`) are `off / 4`.
pub fn item_model_index(item: &[u8]) -> Option<u32> {
    let t = item[0];
    let sub = item[1];
    let modifier = i32::from_le_bytes(item[4..8].try_into().unwrap());
    let rarity = item[0xc];
    let material = item[0xd];
    let r4 = u32::from(if rarity > 3 { 4 } else { rarity });
    let m5 = (modifier as u32) % 5 * 5;
    let at = |off: u32| Some(off / 4);
    // The armour switch of type 7 (also reached from type 4 material 6 with a sub type above 6).
    let type7 = |material: u8| -> Option<u32> {
        match material {
            5 => at(0x201c),
            7 => at(0x2050),
            0xb => at(0x203c),
            0x12 => at(0x1324),
            0x13 => at(0x2510),
            0x14 => at(0x2528),
            0x16 => at(0x24dc),
            0x17 => at(0xa7c),
            0x19 => Some(r4 + m5 + 0x69e),
            0x1a => Some(r4 + m5 + 0x568),
            0x1b => Some(r4 + m5 + 0x5cc),
            _ => Some(r4 + m5 + 0x47d),
        }
    };
    match t {
        1 => Some(consumable_model(sub)),
        2 => at(0x25d4),
        3 => Some(weapon_model(sub, modifier, rarity, material)),
        4 => match material {
            5 => at(0x2018),
            6 => match sub {
                0 => at(0x1334),
                1 => at(0x1338),
                2 => at(0x133c),
                3 => at(0x1340),
                4 => at(0x1344),
                5 => at(0x1348),
                6 => at(0x134c),
                _ => type7(material),
            },
            7 => at(0x204c),
            0xb => at(0x2038),
            0x12 => at(0x1320),
            0x13 => at(0x2504),
            0x14 => at(0x251c),
            0x16 => at(0x24e0),
            0x17 => at(0xa78),
            0x19 => Some(m5 + 0x685 + r4),
            0x1a => Some(m5 + 0x54f + r4),
            0x1b => Some(m5 + 0x5b3 + r4),
            _ => Some(r4 + m5 + 0x464),
        },
        5 => match material {
            1 => Some(m5 + 0x496 + r4),
            5 => at(0x2024),
            7 => at(0x2058),
            0xb => at(0x2044),
            0x12 => at(0x1328),
            0x13 => at(0x2500),
            0x14 => at(0x2518),
            0x16 => at(0x24e8),
            0x17 => at(0xa80),
            0x19 => Some(r4 + m5 + 0x6d0),
            0x1a => Some(m5 + 0x59a + r4),
            0x1b => Some(m5 + 0x5fe + r4),
            _ => at(0x1c),
        },
        6 => match material {
            1 => Some(m5 + 0x4af + r4),
            5 => at(0x2020),
            7 => at(0x2054),
            0xb => at(0x2040),
            0x12 => at(0x132c),
            0x13 => at(0x2508),
            0x14 => at(0x2520),
            0x16 => at(0x24e4),
            0x17 => at(0xa84),
            0x19 => Some(m5 + 0x6b7 + r4),
            0x1a => Some(m5 + 0x581 + r4),
            0x1b => Some(m5 + 0x5e5 + r4),
            _ => at(0x6c4),
        },
        7 => type7(material),
        8 => {
            if material != 0xc { Some(r4 + m5 + 0x61c) } else { Some(m5 + 0x63a + r4) }
        }
        9 => {
            if material != 0xc { Some(m5 + 0x653 + r4) } else { Some(r4 + m5 + 0x66c) }
        }
        10 => at(0x206c),
        0xb => match sub {
            0 => match material {
                1 => at(0x2484),
                2 => at(0x24f4),
                0xb => at(0x248c),
                0xc => at(0x2488),
                0xd => at(0x2490),
                0xe => at(0x2494),
                0xf => at(0x2498),
                0x10 => at(0x249c),
                0x11 => at(0x24f0),
                _ => None,
            },
            1 => at(0x24f4),
            2 => at(0x24f8),
            5 => at(0x2568),
            6 => at(0x24c0),
            7 => at(0x2514),
            8 => at(0x24d4),
            9 => at(0x24ec),
            10 => match material {
                1 => at(0x24a0),
                2 => at(0x24ac),
                0xb => at(0x24a8),
                0xc => at(0x24a4),
                _ => None,
            },
            0xb => at(0x21c0),
            0xc => at(0x2554),
            0xd => at(0x2480),
            0xe => match material as i8 {
                -0x7f => at(0x24b8),
                -0x7e => at(0x24b4),
                -0x7d => at(0x24bc),
                _ => at(0x24b0),
            },
            0xf => at(0xd2c),
            0x10 => at(0x2470),
            0x11 => at(0x2478),
            0x12 => at(0x255c),
            0x13 => at(0xd30),
            0x14 => at(0x20c4),
            0x15 => at(0x2560),
            0x16 => at(0xd34),
            0x17 => at(0xd3c),
            0x18 => at(0xd38),
            0x19 => at(0xd40),
            0x1a => at(0x2558),
            0x1b => at(0x20ac),
            _ => None,
        },
        0xc => match material {
            0xb => at(0x2098),
            0xc => at(0x2094),
            _ => at(0x2090),
        },
        0xd => at(0x24d4),
        0xe => {
            let v = i32::from(rarity) - 1;
            // The original tests `-1 < v && 3 < v`.
            if 3 < v {
                return Some(0x911);
            }
            let v = if v < 0 { 0 } else { v };
            Some(v as u32 + 0x90e)
        }
        0xf => at(0x24fc),
        0x10 => Some((modifier as u32 & 1) + 0x909),
        0x11 => Some((modifier as u32 & 3) + 0x841),
        0x12 => {
            if sub != 1 { Some((modifier as u32) % 3 + 0x845) } else { Some((modifier as u32) % 3 + 0x848) }
        }
        0x13 => match sub {
            0x18 => at(0x21d0),
            0x19 => at(0x21c4),
            _ => at(0x2080),
        },
        0x14 => match sub {
            0x13 => at(0xdac),
            0x16 => at(0xd90),
            0x17 => at(0xd94),
            0x19 => at(0xdb4),
            0x1a => at(0xdbc),
            0x1b => at(0xd9c),
            0x1e => at(0xd6c),
            0x21 => at(0xd68),
            0x22 => at(0xd7c),
            0x23 => at(0xd64),
            0x24 => at(0xde4),
            0x25 => at(0xd54),
            0x26 => at(0xd58),
            0x27 => at(0xd60),
            0x28 => at(0xd5c),
            0x32 => at(0xd98),
            0x35 => at(0xd80),
            0x37 => at(0xdb0),
            0x38 => at(0xd84),
            0x39 => at(0xde8),
            0x3a => at(0xdec),
            0x3b => at(0xdf0),
            0x3c => at(0xdf4),
            0x3d => at(0xdf8),
            0x3e => at(0xdfc),
            0x3f => at(0xdd0),
            0x40 => at(0xdd8),
            0x41 => at(0xddc),
            0x42 => at(0xdd4),
            0x43 => at(0xda8),
            0x4a => at(0xde0),
            0x4b => at(0xdb8),
            0x56 => at(0xdc0),
            0x57 => at(0xd78),
            0x58 => at(0xe00),
            0x5a => at(0xd88),
            0x5b => at(0xd8c),
            0x5c => at(0xd70),
            0x5d => at(0xd74),
            0x62 => at(0xdc8),
            99 => at(0xdc4),
            0x66 => at(0x25a0),
            0x67 => at(0xe04),
            0x68 => at(0xda0),
            0x69 => at(0xda4),
            0x6a => at(0xdcc),
            0x97 => at(0xe08),
            _ => at(0x2478),
        },
        0x15 => match sub {
            3 => at(0x25d8),
            4 | 5 => at(0xd4c),
            6 => at(0x27d8),
            7 => at(0x984),
            8 => at(0x24ec),
            9 => at(0x256c),
            _ => Some((u32::from(sub) & 3) + 0x821),
        },
        0x17 => {
            if sub == 1 { at(0x27d0) } else { at(0x27cc) }
        }
        0x18 => at(0x2604),
        0x19 => at(0x2818),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interact_layout() {
        let it = interact_packet(5, Some([3, 4, 7]), None);
        assert_eq!(it.0[0x128], 5);
        assert_eq!(i32_at(&it.0, 0x118), 3);
        assert_eq!(i32_at(&it.0, 0x120), 7);
        assert_eq!(it.0[0x10], 1);
        let it = interact_packet(8, None, None);
        assert_eq!(i32_at(&it.0, 0x11c), -1);
    }

    #[test]
    fn wrap_negative_index() {
        assert_eq!(wrap_index(-1, 3), 2);
        assert_eq!(wrap_index(-4, 3), 2);
        assert_eq!(wrap_index(4, 3), 1);
    }

    #[test]
    fn models() {
        let mut item = [0u8; Item::SIZE];
        item[0] = 1;
        item[1] = 7;
        assert_eq!(item_model_index(&item), Some(0x9f2));
        item[0] = 0x19;
        assert_eq!(item_model_index(&item), Some(0x2818 / 4));
        assert_eq!(weapon_model(0x14, 0, 0, 0), 0x34a);
    }
}
