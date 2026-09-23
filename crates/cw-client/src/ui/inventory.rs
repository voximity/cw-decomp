//! The inventory half of `GameController::onMouseDown` `Cube.exe 0x0047b600` and the item
//! helpers it calls.
//!
//! The bag is `creature+0x11dc`, a `cube::Inventory` (the same class as the server's,
//! `Inventory::addItem` 0x0046ebe0 is byte-identical to `Server.exe 0x00427000`): a vector of
//! tabs, each a vector of 0x11c-byte stacks (`i32 count` + `cube::Item`), the cursor stack at
//! `+0xc/+0x10` (`creature+0x11e8/+0x11ec`, [`GameView::held`]), coins at `+0x128`
//! (`creature+0x1304`) and platinum at `+0x12c` (`creature+0x1308`). The equipment is
//! `creature+0x300 + k * 0x118` for k = 0..11 (entity slots 1..12, [`slot`]).
//!
//! # Range map of `onMouseDown` (Tier B)
//!
//! | Range | Here |
//! |---|---|
//! | 0x0047bd6c..0x0047c54d (left button, `GC+0x8008f0` set: the equipment slots) | [`equipment_left_click`] |
//! | 0x0047c54d..0x0047c748 (left button, inventory open: merge / swap with the cursor) | [`bag_left_click`] |
//! | 0x0047c8f0..0x0047dd8d (right or middle button) | [`right_click`] and its parts [`use_or_equip`], [`sell_from_bag`], [`sell_equipment`], [`unequip`], [`buy`], [`buy_back`] |
//!
//! The left-button crafting selection (0x0047bb65, `InventoryWidget` 0x004c6610 on the
//! crafting widget, then the craft preview), the customization, adaption, skill and enchant
//! parts of `onMouseDown` are not here (see their modules).
//!
//! # Item helpers
//!
//! | Address | Here |
//! |---|---|
//! | `canEquip(creature, item)` 0x0043e420 | [`can_equip`] |
//! | item level 0x004c76a0 (and 0x0043ca60) | [`item_level`], [`level_fraction`] |
//! | class check 0x004c6f20 | [`class_allows`] |
//! | `Item::operator==` 0x0042f4a0 | [`same_item`] |
//! | `Item::isTwoHanded` 0x00444820 | [`is_two_handed`] |
//! | stackable 0x0047f9f0 | [`stackable`] |
//! | page for item 0x0047fa30 | [`page_for`] |
//! | price 0x004c76e0 | [`price`] |
//! | `Item::Item` 0x0042f3e0 | [`empty_item`] |
//! | stack take-one 0x0042f140 | [`take_one`] |
//! | `Inventory::addItem` 0x0046ebe0 | [`add_item`] |
//! | `hasRoomFor` 0x0043e4a0 | [`has_room`] |
//! | known formula 0x00444a90 | [`knows_formula`] |

use cw_net::entity::ITEM_SIZE;
use cw_world::inventory::{Inventory, Item};

use super::inventory_widget::{InventoryWidget, ShopData};
use super::{GameView, ItemStack, UiAction};

/// Creature equipment offsets (`creature+...`) and the index `k` into
/// [`GameView::equipment`]: `k = (offset - 0x418) / 0x118`.
pub mod slot {
    /// +0x418: amulet (item type 8).
    pub const NECK: usize = 0;
    /// +0x530: chest (type 4).
    pub const CHEST: usize = 1;
    /// +0x648: feet (type 6).
    pub const FEET: usize = 2;
    /// +0x760: hands (type 5).
    pub const HANDS: usize = 3;
    /// +0x878: shoulder (type 7).
    pub const SHOULDER: usize = 4;
    /// +0x990: left weapon.
    pub const LEFT: usize = 5;
    /// +0xaa8: right weapon.
    pub const RIGHT: usize = 6;
    /// +0xbc0: left ring (type 9, middle click).
    pub const LEFT_RING: usize = 7;
    /// +0xcd8: right ring (type 9, right click).
    pub const RIGHT_RING: usize = 8;
    /// +0xdf0: lamp (type 0x18).
    pub const LAMP: usize = 9;
    /// +0xf08: type 0x17 (equipping resets the current mode).
    pub const SPECIAL: usize = 10;
    /// +0x1020: pet (types 0x13 / 0x14).
    pub const PET: usize = 11;

    /// The order `0x0047b1b0` tests the equipment slot nodes for the hover
    /// (`Button::isHovered` 0x006294c0 on each): the first hovered one is "the slot under
    /// the cursor". Only while `GC+0x8008f0` is set.
    pub const HOVER_ORDER: [usize; 12] =
        [LEFT, RIGHT, LEFT_RING, RIGHT_RING, NECK, SHOULDER, CHEST, HANDS, FEET, LAMP, SPECIAL, PET];
}

// ---------------------------------------------------------------------------------------
// Item helpers
// ---------------------------------------------------------------------------------------

/// `cvttss2si`: truncation toward zero, `0x80000000` for NaN and out-of-range values.
pub fn cvtt(f: f32) -> i32 {
    if f.is_nan() || f >= 2147483648.0 || f < -2147483648.0 { i32::MIN } else { f as i32 }
}

fn level_of(item: &[u8]) -> i16 {
    i16::from_le_bytes([item[0x10], item[0x11]])
}

/// `0x0043ca60`: `1 - 1 / ((x - 1) * 0.05 + 1)` in single precision.
pub fn level_fraction(x: f32) -> f32 {
    1.0f32 - 1.0f32 / ((x - 1.0f32) * 0.05f32 + 1.0f32)
}

/// `0x004c76a0`: the displayed item level, `(int)(0x0043ca60((float)(i16)Item+0x10) * 100 +
/// 1)`.
pub fn item_level(item: &[u8]) -> i32 {
    cvtt(level_fraction(f32::from(level_of(item))) * 100.0f32 + 1.0f32)
}

/// The creature half of `canEquip` 0x0043e420: `(int)((1 - 1 / (((float)level - 1) * 0.05 +
/// 1)) * 100 + 1)`, `level` = `creature+0x190` (entity `+0x180`).
pub fn level_cap(level: i32) -> i32 {
    let l = level as f32;
    cvtt((1.0f32 - 1.0f32 / ((l - 1.0f32) * 0.05f32 + 1.0f32)) * 100.0f32 + 1.0f32)
}

/// `0x004c6f20(item, class)`: whether class `class` (1 warrior, 2 ranger, 3 mage, 4 rogue)
/// may use `item`. Type 2 (a formula) tests its product: a copy with the type byte replaced
/// by `Item+8` and `Item+8..+0xc` zeroed (a product type of 2 is allowed). Weapons (3) by sub
/// type, armour (4..7) by material; everything else is allowed.
pub fn class_allows(item: &[u8], class: i32) -> bool {
    match item[0] {
        2 => {
            let mut c = item[..ITEM_SIZE].to_vec();
            c[0] = item[8];
            c[8..0xc].fill(0);
            if c[0] == 2 {
                return true;
            }
            class_allows(&c, class)
        }
        3 => match item[1] {
            // Table 0x004c706c over 0x004c7058.
            0..=2 | 0xd | 0xf..=0x11 => class == 1,
            3..=5 => class == 4,
            6..=8 | 0xe => class == 2,
            0xa..=0xc => class == 3,
            _ => true,
        },
        4..=7 => match item[0xd] {
            // Table 0x004c7094 over 0x004c7080 (index material - 1).
            1 | 5 | 0x12 | 0x16 => class == 1,
            0x13 | 0x1a => class == 2,
            0x17 | 0x19 => class == 3,
            0x1b => class == 4,
            _ => true,
        },
        _ => true,
    }
}

/// `canEquip(creature, item)` 0x0043e420: the item level ([`item_level`]) is at most
/// [`level_cap`] of the creature's level (entity `+0x180`) and its class (entity `+0x130`)
/// may use it ([`class_allows`]).
pub fn can_equip(player: &cw_net::EntityData, item: &[u8]) -> bool {
    let level = i32::from_le_bytes(player.0[0x180..0x184].try_into().unwrap());
    let cap = level_cap(level);
    if item_level(item) > cap {
        return false;
    }
    class_allows(item, i32::from(player.0[0x130]))
}

/// `Item::isTwoHanded` 0x00444820.
pub fn is_two_handed(item: &[u8]) -> bool {
    item[0] == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)
}

/// `Item::operator==` 0x0042f4a0 (the same code as `Server.exe 0x004078f0`,
/// [`Inventory::same_item`]): every field and the first four bytes of each spirit.
pub fn same_item(a: &[u8], b: &[u8]) -> bool {
    Inventory::same_item(&Item::from_bytes(a), &Item::from_bytes(b))
}

/// `0x0047f9f0`: the item types that show a stack count (1, 0xa, 0xc, 0xd, 0xb, 0x15); the
/// same set as `Item::isStackable` ([`Inventory::stackable`]).
pub fn stackable(item: &[u8]) -> bool {
    matches!(item[0], 1 | 0xa | 0xc | 0xd | 0xb | 0x15)
}

/// `0x0047fa30`: the bag tab an item belongs to (0 equipment, 1 items, 2 ingredients, 3
/// pets); [`Inventory::page_for`].
pub fn page_for(item: &[u8]) -> i32 {
    Inventory::page_for(item[0])
}

/// `0x004c76e0`: the shop price (buying and selling), at least 1:
/// `(int)(itemPower((float)(i16)level, rarity) * 10 * k)` with `k` 2 (types 3, 4), 1 (5, 6,
/// 9), 1.5 (7, 8), 100 (0x17), else 0.2, doubled for a two-handed weapon.
pub fn price(item: &[u8]) -> i32 {
    let mut k = match item[0] {
        3 | 4 => 2.0f32,
        5 | 6 | 9 => 1.0f32,
        7 | 8 => 1.5f32,
        0x17 => 100.0f32,
        _ => 0.2f32,
    };
    if is_two_handed(item) {
        k *= 2.0f32;
    }
    let p = cw_sim::stats::item_power(f32::from(level_of(item)), i32::from(item[0xc]));
    let v = cvtt(p * 10.0f32 * k);
    if v < 1 { 1 } else { v }
}

/// `Item::Item` 0x0042f3e0: every field zero, level (`+0x10`) 1.
pub fn empty_item() -> Vec<u8> {
    let mut v = vec![0u8; ITEM_SIZE];
    v[0x10] = 1;
    v
}

/// `0x0042f140`: one off the stack; below one the count is 0 and the item's type and sub type
/// are cleared.
pub fn take_one(stack: &mut ItemStack) {
    stack.count = stack.count.wrapping_sub(1);
    if stack.count < 1 {
        stack.count = 0;
        stack.item[0] = 0;
        stack.item[1] = 0;
    }
}

/// `cube::Inventory::addItem(item, page)` 0x0046ebe0 on the player's inventory
/// (`creature+0x11dc`): `page == -1` picks the tab from the type ([`page_for`]). Types 0xc,
/// 0xd, 0x15, 0xb (sub type other than 0xe), 0, 0x19, 0x14, 0x18, 0x17 count their level as
/// the number of items and are stored with level 1. Coins (0xc) of material 0xa/0xb/0xc add
/// 1/10000/100 per item to [`GameView::coins`]; type 0xd adds to [`GameView::platinum`].
/// A stackable item joins the first equal stack; otherwise it takes the first empty slot of
/// the tab, or is appended.
pub fn add_item(game: &mut GameView, item: &[u8], page: i32) {
    let page = if page == -1 { page_for(item) } else { page };
    let t = item[0];
    if t == 0 {
        return;
    }
    if game.inventory.len() as i32 <= page {
        game.inventory.resize(page as usize + 1, Vec::new());
    }
    let mut copy = item[..ITEM_SIZE].to_vec();
    let mut n: i32 = 1;
    let by_level = matches!(t, 0xc | 0xd | 0x15) || (t == 0xb && item[1] != 0xe) || matches!(t, 0 | 0x19 | 0x14 | 0x18 | 0x17);
    if by_level {
        n = i32::from(level_of(item));
        copy[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
    }
    if t == 0xc {
        match item[0xd] {
            0xa => {
                game.coins = game.coins.wrapping_add(n);
                return;
            }
            0xb => {
                game.coins = game.coins.wrapping_add(n.wrapping_mul(10000));
                return;
            }
            0xc => {
                game.coins = game.coins.wrapping_add(n.wrapping_mul(100));
                return;
            }
            _ => {}
        }
    }
    if t == 0xd {
        game.platinum = game.platinum.wrapping_add(n);
        return;
    }
    let tab = &mut game.inventory[page as usize];
    let mut first_empty = None;
    for (i, s) in tab.iter_mut().enumerate() {
        if s.count == 0 && first_empty.is_none() {
            first_empty = Some(i);
        }
        if Inventory::stackable(t) && same_item(&s.item, &copy) {
            s.count = s.count.wrapping_add(n);
            return;
        }
    }
    if let Some(i) = first_empty {
        tab[i].item = copy;
        tab[i].count = n;
        return;
    }
    tab.push(ItemStack { count: n, item: copy });
}

/// `hasRoomFor(creature, item)` 0x0043e4a0: consumables (1), ingredients (0xb) and pet food
/// (0x14) stack up to 49 equal items (the cursor stack and every bag stack counted; pet food:
/// none may be held). Everything else always fits. (`crate::interact::has_room` is the same
/// helper over the player's `cw_world` inventory.)
pub fn has_room(game: &GameView, item: &[u8]) -> bool {
    let t = item[0];
    if t == 1 || t == 0xb || t == 0x14 {
        let mut n: i32 = 0;
        if same_item(&game.held.item, item) {
            n = game.held.count;
        }
        for tab in &game.inventory {
            for s in tab {
                if same_item(&s.item, item) {
                    n = n.wrapping_add(s.count);
                }
            }
        }
        if (t == 0x14 && 0 < n) || 0x31 < n {
            return false;
        }
    }
    true
}

/// `0x00444a90(creature, formula)`: the formula is in the learned list
/// (`*(creature+0x1d28)+0x14`, [`GameView::formulas`]); a creature without the record knows
/// none.
pub fn knows_formula(game: &GameView, formula: &[u8]) -> bool {
    game.formulas.iter().any(|f| same_item(formula, f))
}

/// The cursor caption of `update` 0x0048b40c: the held count when a stack is held and its
/// item shows counts ([`stackable`]), else empty.
pub fn cursor_caption(held: &ItemStack) -> String {
    if held.count != 0 && stackable(&held.item) { held.count.to_string() } else { String::new() }
}

// ---------------------------------------------------------------------------------------
// Click handlers
// ---------------------------------------------------------------------------------------

fn snd(id: u32) -> UiAction {
    UiAction::PlaySound { id, volume: 1.0, pitch: 1.0 }
}

/// The error line of the shop (0x0047db30 / 0x0047dc66), `0x0043ab30` with (1, 0.2, 0.2, 1).
fn cant_carry() -> UiAction {
    UiAction::Print { text: "You can't carry more of these items.\n".into(), color: [1.0, 0.2, 0.2, 1.0] }
}

/// The panel whose target a right/middle click sets ([`UiAction::SetItemTarget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPanel {
    /// `AdaptionWidget` (`GC+0x800904`).
    Adaption,
    /// `EnchantWidget` (`GC+0x8008fc`), the identification panel.
    Enchant,
    /// `VoxelWidget` (`GC+0x8008f4`), weapon customization; the original then calls
    /// 0x0058ce20 on it.
    Voxel,
}

/// An item of the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemRef {
    /// Bag cell `(tab, index)`.
    Bag { tab: i32, index: i32 },
    /// Equipment slot `k` ([`slot`]).
    Equipment(usize),
}

/// The GameController panels the handlers test (`Node+0x3c` Display attribute), and the
/// `GC+0x8008f0` byte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Panels {
    /// `GC+0x8008bc` inventory panel.
    pub inventory: bool,
    /// `GC+0x8008c4` shop panel.
    pub shop: bool,
    /// `GC+0x8008dc` weapon customization panel.
    pub customization: bool,
    /// `GC+0x8008f8` identification (enchant) panel.
    pub identification: bool,
    /// `GC+0x800900` adaption panel.
    pub adaption: bool,
    /// `GC+0x8008f0`: the equipment slots are live ([`crate::ui::GameUi::flag_8008f0`]).
    pub equipment: bool,
}

/// `0x0047b450`: the bag cell under the cursor, `(tab, index)` of the bag widget's hover
/// (`+0x184/+0x188`) when the inventory panel is open, the cell exists and its count is not
/// zero.
pub fn bag_cell_under_cursor(game: &GameView, bag: &InventoryWidget, inventory_open: bool) -> Option<(usize, usize)> {
    if !inventory_open || bag.hovered_tab < 0 || bag.hovered_index < 0 {
        return None;
    }
    let (t, i) = (bag.hovered_tab as usize, bag.hovered_index as usize);
    let s = game.inventory.get(t)?.get(i)?;
    (s.count != 0).then_some((t, i))
}

/// `0x0047b550`: the shop cell under the cursor, `(tab, index)` of the shop widget's hover
/// in the shop pages (`GC+0x800c0c`) when the shop panel is open (no count test).
pub fn shop_cell_under_cursor(shop: &ShopData, shop_widget: &InventoryWidget, shop_open: bool) -> Option<(usize, usize)> {
    if !shop_open || shop_widget.hovered_tab < 0 || shop_widget.hovered_index < 0 {
        return None;
    }
    let (t, i) = (shop_widget.hovered_tab as usize, shop_widget.hovered_index as usize);
    shop.pages.get(t)?.get(i)?;
    Some((t, i))
}

/// The inputs of the click handlers besides the state they edit.
#[derive(Clone, Copy, Debug)]
pub struct ClickContext<'a> {
    /// The open panels.
    pub panels: Panels,
    /// The bag widget (`GC+0x800954`), hover and tab.
    pub bag: &'a InventoryWidget,
    /// The shop widget (`GC+0x80095c`), hover and tab.
    pub shop_widget: &'a InventoryWidget,
    /// The equipment slot under the cursor ([`slot::HOVER_ORDER`], `0x0047b1b0`), only
    /// meaningful while [`Panels::equipment`].
    pub hovered_equipment: Option<usize>,
    /// `GC+0x29` = `Controller` key[VK_SHIFT] (`+0x19 + 0x10`): shift held.
    pub shift: bool,
}

/// `onMouseDown(button)` 0x0047b600, the left-button inventory parts in their order: the
/// equipment slots (0x0047bd6c, while `GC+0x8008f0`) then the bag cell (0x0047c54d). The
/// crafting selection before them and the adaption/skill/enchant parts after them are other
/// modules'.
pub fn left_click(game: &mut GameView, ctx: &ClickContext) -> Vec<UiAction> {
    let mut out = Vec::new();
    if ctx.panels.equipment {
        let s = ctx.hovered_equipment.filter(|_| ctx.panels.equipment);
        out.extend(equipment_left_click(game, s));
    }
    out.extend(bag_left_click(game, ctx.bag, ctx.panels.inventory, ctx.shift));
    out
}

fn swap_with_held(game: &mut GameView, k: usize) {
    // 0x0040ee70 copy, 0x0042c5e0 slot = held, 0x0042c5e0 held = copy.
    std::mem::swap(&mut game.equipment[k], &mut game.held.item);
}

/// 0x0047bd6c..0x0047c54d: a left click with `GC+0x8008f0` set on equipment slot `hovered`
/// (`0x0047b1b0`, `None` when none is hovered or the flag is clear) swaps the slot with the
/// cursor's item when the cursor is empty or holds a matching type. Refused (sound 0x32)
/// when the held item cannot be equipped ([`can_equip`]) over a slot.
///
/// Right hand: an empty cursor or a weapon; a two-handed weapon first sends a non-empty left
/// hand to the bag. Left hand: a two-handed weapon other than a bow/crossbow is refused (0x32
/// played, nothing swapped); an empty cursor or a weapon swaps, a two-handed weapon in either
/// the cursor or the right hand first sends a non-empty right hand to the bag. Pet: a single
/// pet or pet food swaps; a cursor of several pet foods puts one into the slot (the old pet
/// back to the bag) unless it is the same item, which ends the click silently. Other slots:
/// their item type. After a swap the cursor count is 1 for an item or drops by one (to 0) for
/// nothing, and sound 0x57 (holding something) or 0x58 plays.
pub fn equipment_left_click(game: &mut GameView, hovered: Option<usize>) -> Vec<UiAction> {
    let mut out = Vec::new();
    let held_type = game.held.item[0];
    if !(held_type == 0 || can_equip(&game.player, &game.held.item) || hovered.is_none()) {
        out.push(snd(0x32));
        return out;
    }
    let Some(s) = hovered else {
        // No slot: every test below fails and the click falls through to 0x0047c54d.
        return out;
    };
    let mut swapped = false;
    let (count, ty) = (game.held.count, game.held.item[0]);
    if s == slot::RIGHT && (count == 0 || ty == 3) {
        // 0x0047bdca: a two-handed weapon empties the left hand first.
        if ty == 3 && is_two_handed(&game.held.item) && game.equipment[slot::LEFT][0] != 0 {
            let old = game.equipment[slot::LEFT].clone();
            add_item(game, &old, -1);
            game.equipment[slot::LEFT] = empty_item();
        }
        swap_with_held(game, slot::RIGHT);
        swapped = true;
    }
    if s == slot::LEFT {
        let h = &game.held.item;
        if h[0] == 3 && is_two_handed(h) && h[1] != 6 && h[1] != 7 {
            out.push(snd(0x32));
        } else if game.held.count == 0 || h[0] == 3 {
            let r = &game.equipment[slot::RIGHT];
            if ((h[0] == 3 && is_two_handed(h)) || (r[0] == 3 && is_two_handed(r))) && r[0] != 0 {
                let old = r.clone();
                add_item(game, &old, -1);
                game.equipment[slot::RIGHT] = empty_item();
            }
            swap_with_held(game, slot::LEFT);
            swapped = true;
        }
    }
    let (count, ty) = (game.held.count, game.held.item[0]);
    if s == slot::PET && (count == 0 || ty == 0x13 || ty == 0x14) {
        if count < 2 || ty != 0x14 {
            swap_with_held(game, slot::PET);
            swapped = true;
        } else if !same_item(&game.held.item, &game.equipment[slot::PET]) {
            // 0x0047c13a: one pet food into the slot, the old pet to the bag.
            if game.equipment[slot::PET][0] != 0 {
                let old = game.equipment[slot::PET].clone();
                add_item(game, &old, -1);
            }
            game.equipment[slot::PET] = game.held.item.clone();
            take_one(&mut game.held);
        }
    }
    // 0x0047c190..: the single-type slots, in the original's order.
    let typed = [
        (slot::NECK, 8u8),
        (slot::LEFT_RING, 9),
        (slot::RIGHT_RING, 9),
        (slot::SHOULDER, 7),
        (slot::SPECIAL, 0x17),
        (slot::LAMP, 0x18),
        (slot::CHEST, 4),
        (slot::HANDS, 5),
    ];
    for (k, t) in typed {
        if s == k && (game.held.count == 0 || game.held.item[0] == t) {
            swap_with_held(game, k);
            swapped = true;
        }
    }
    if s == slot::FEET && (game.held.count == 0 || game.held.item[0] == 6) {
        swap_with_held(game, slot::FEET);
    } else if !swapped {
        return out;
    }
    // 0x0047c4c9: the cursor count after the swap.
    if game.held.item[0] == 0 {
        take_one(&mut game.held);
    } else {
        game.held.count = 1;
    }
    out.push(snd(if game.held.count == 0 { 0x58 } else { 0x57 }));
    out
}

/// 0x0047c54d..0x0047c748: a left click on the bag widget's hovered cell (`+0x184/+0x188`,
/// any count) with the inventory open. A held stack of another tab's type is refused (sound
/// 0x32); a held stackable item equal to the cell's joins it (no cap, no sound). Otherwise,
/// unless shift is held (`GC+0x29`), the cell and the cursor stack swap (`0x0044a690`) and
/// sound 0x57 (now holding) or 0x58 plays at the listener.
pub fn bag_left_click(game: &mut GameView, bag: &InventoryWidget, inventory_open: bool, shift: bool) -> Vec<UiAction> {
    let mut out = Vec::new();
    if !inventory_open || bag.hovered_tab < 0 || bag.hovered_index < 0 {
        return out;
    }
    let (t, i) = (bag.hovered_tab as usize, bag.hovered_index as usize);
    if t >= game.inventory.len() || i >= game.inventory[t].len() {
        return out;
    }
    let n = game.held.count;
    if n != 0 {
        if page_for(&game.held.item) != bag.tab {
            out.push(snd(0x32));
            return out;
        }
        if stackable(&game.held.item) && same_item(&game.held.item, &game.inventory[t][i].item) {
            let c = &mut game.inventory[t][i];
            c.count = c.count.wrapping_add(n);
            game.held.count = 0;
            game.held.item[0] = 0;
            game.held.item[1] = 0;
            return out;
        }
    }
    if !shift {
        std::mem::swap(&mut game.inventory[t][i], &mut game.held);
        out.push(snd(if game.held.count == 0 { 0x58 } else { 0x57 }));
    }
    out
}

/// `onMouseDown` 0x0047c8f0..0x0047dd8d: the right (`button == 1`) or middle (`button == 2`)
/// click. A bag cell under the cursor ([`bag_cell_under_cursor`]) goes, by the open panels,
/// to the adaption target, the enchant target, a sale ([`sell_from_bag`]), the voxel target
/// (weapons, else sound 0x31) or [`use_or_equip`]. Without a bag cell, a non-empty equipment
/// slot under the cursor goes to the adaption target, a sale ([`sell_equipment`]), the voxel
/// target or back to the bag ([`unequip`]); without one either, a shop cell buys
/// ([`buy_back`] on the shop's tab 1, else [`buy`]). Every path ends with the recipe rebuild
/// 0x004a14c0 ([`UiAction::RebuildRecipes`]).
///
/// The right-button prelude 0x0047bab1 runs before this in [`super::GameUi::on_mouse_down_with`]
/// ([`super::voxel::right_click_prelude`]: with `GC+0x800704` clear and the customization
/// panel open, `VoxelWidget` 0x0058ce20; `GC+0x800704` is the build mode, never set in this
/// build, whose branch would do `creature+0x18c += 1`).
pub fn right_click(game: &mut GameView, shop: &mut ShopData, ctx: &ClickContext, button: i32) -> Vec<UiAction> {
    let mut out = Vec::new();
    if button != 1 && button != 2 {
        return out;
    }
    let p = ctx.panels;
    match bag_cell_under_cursor(game, ctx.bag, p.inventory) {
        Some((t, i)) => {
            let (tab, index) = (t as i32, i as i32);
            if p.adaption {
                out.push(UiAction::SetItemTarget { panel: TargetPanel::Adaption, item: ItemRef::Bag { tab, index } });
            } else if p.identification {
                out.push(UiAction::SetItemTarget { panel: TargetPanel::Enchant, item: ItemRef::Bag { tab, index } });
            } else if p.shop {
                out.extend(sell_from_bag(game, shop, t, i));
            } else if p.customization {
                if game.inventory[t][i].item[0] == 3 {
                    out.push(UiAction::SetItemTarget { panel: TargetPanel::Voxel, item: ItemRef::Bag { tab, index } });
                } else {
                    out.push(snd(0x31));
                }
            } else {
                out.extend(use_or_equip(game, t, i, button));
            }
        }
        None => {
            let eq = if p.equipment { ctx.hovered_equipment } else { None };
            match eq.filter(|&k| game.equipment[k][0] != 0) {
                Some(k) => {
                    if p.adaption {
                        out.push(UiAction::SetItemTarget { panel: TargetPanel::Adaption, item: ItemRef::Equipment(k) });
                    } else if p.shop {
                        out.extend(sell_equipment(game, shop, k));
                    } else if p.customization {
                        if game.equipment[k][0] == 3 {
                            out.push(UiAction::SetItemTarget { panel: TargetPanel::Voxel, item: ItemRef::Equipment(k) });
                        } else {
                            out.push(snd(0x31));
                        }
                    } else {
                        out.extend(unequip(game, k));
                    }
                }
                None => {
                    if let Some((st, si)) = shop_cell_under_cursor(shop, ctx.shop_widget, p.shop) {
                        if ctx.shop_widget.tab == 1 {
                            out.extend(buy_back(game, shop, ctx.shop_widget.hovered_index, st, si));
                        } else {
                            out.extend(buy(game, shop, st, si));
                        }
                    }
                }
            }
        }
    }
    // 0x0047dd86.
    out.push(UiAction::RebuildRecipes);
    out
}

/// Moves the equipped item in `k` back to the bag (`addItem`, an empty slot adds nothing) and
/// equips `item` there.
fn swap_in(game: &mut GameView, k: usize, item: Vec<u8>) {
    let old = std::mem::replace(&mut game.equipment[k], item);
    add_item(game, &old, -1);
}

/// 0x0047cfc0..0x0047d7c6: right (`button == 1`) or middle (`button == 2`) click on bag cell
/// `(tab, index)` with no trade panel open: the item is equipped, used or learned.
///
/// Refused (sound 0x32) when [`can_equip`] fails; nothing for an empty stack. Then, testing
/// the stack's type again after each step as the original does:
/// - weapons (3): one off the stack; a shield (sub 0xd/0xe) to the left hand (a two-handed
///   right weapon back to the bag first); right click on a bow/crossbow (6/7): left hand,
///   right hand emptied; middle click on a one-handed weapon: left hand; otherwise the right
///   hand, a two-handed weapon also emptying the left. Sound 0x58.
/// - pets (0x13), or pet food (0x14) not equal to the equipped pet: into the pet slot (old
///   pet to the bag first), then one off the stack. Sound 0x58.
/// - types 4, 7, 8, 9 (right click: right ring, middle: left ring), 0x17 (also clears the
///   current mode, `creature+0x68`), 0x18, 5, 6: one off, swapped into their slot. Sound 0x58.
/// - ingredients (0xb) with more than one: two off, one copy with level + 1 added back
///   (`addItem` counts a non-0xe ingredient's level as items). Sound 0x58.
/// - consumables (1): [`UiAction::UseItem`] (`useItem` 0x004a2780 under the world lock).
/// - formulas (2): the product (type byte from `Item+8`, `+8..+0xc` zeroed) is learned
///   ([`GameView::formulas`]) and one taken off, sound 0x2f; an already known one plays 0x31.
pub fn use_or_equip(game: &mut GameView, tab: usize, index: usize, button: i32) -> Vec<UiAction> {
    let mut out = Vec::new();
    let Some(stack) = game.inventory.get(tab).and_then(|t| t.get(index)).cloned() else {
        return out;
    };
    if !can_equip(&game.player, &stack.item) {
        out.push(snd(0x32));
        return out;
    }
    if stack.count == 0 {
        return out;
    }
    let ty = |game: &GameView| game.inventory[tab][index].item[0];
    let empty = empty_item();
    if ty(game) == 3 {
        let copy = game.inventory[tab][index].item.clone();
        take_one(&mut game.inventory[tab][index]);
        let sub = copy[1];
        // 0x0047d038: `equipped` is false only on the skip to 0x0047d1fc (no sound).
        let mut equipped = true;
        if sub == 0xd || sub == 0xe {
            // 0x0047d159: a shield; a two-handed right weapon goes back first.
            if is_two_handed(&game.equipment[slot::RIGHT]) {
                swap_in(game, slot::RIGHT, empty.clone());
            }
            swap_in(game, slot::LEFT, copy);
        } else if button == 1 && (sub == 6 || sub == 7) {
            // 0x0047d064: bow / crossbow on a right click: left hand, right hand emptied.
            swap_in(game, slot::LEFT, copy);
            swap_in(game, slot::RIGHT, empty.clone());
        } else if button != 1 && !is_two_handed(&copy) {
            // 0x0047d0cc: a middle click on a one-handed weapon: left hand (any other
            // button skips to 0x0047d1fc).
            if button == 2 {
                swap_in(game, slot::LEFT, copy);
            } else {
                equipped = false;
            }
        } else {
            // 0x0047d0ea: right hand; a two-handed weapon also empties the left.
            let two = is_two_handed(&copy);
            swap_in(game, slot::RIGHT, copy);
            if two {
                swap_in(game, slot::LEFT, empty.clone());
            }
        }
        if equipped {
            out.push(snd(0x58));
        }
    }
    // 0x0047d1fc: pets and pet food (the pet slot's old item goes to the bag first).
    let t = ty(game);
    if t == 0x13 || (t == 0x14 && !same_item(&game.inventory[tab][index].item, &game.equipment[slot::PET])) {
        let copy = game.inventory[tab][index].item.clone();
        swap_in(game, slot::PET, copy);
        take_one(&mut game.inventory[tab][index]);
        out.push(snd(0x58));
    }
    // 0x0047d2af..0x0047d6af: the single-slot types.
    let singles: [(u8, usize); 8] = [
        (4, slot::CHEST),
        (7, slot::SHOULDER),
        (8, slot::NECK),
        (9, if button == 1 { slot::RIGHT_RING } else { slot::LEFT_RING }),
        (0x17, slot::SPECIAL),
        (0x18, slot::LAMP),
        (5, slot::HANDS),
        (6, slot::FEET),
    ];
    for (want, k) in singles {
        if ty(game) == want {
            let copy = game.inventory[tab][index].item.clone();
            take_one(&mut game.inventory[tab][index]);
            swap_in(game, k, copy);
            if want == 0x17 {
                // `creature+0x68 = 0`: the current mode (entity+0x58).
                game.player.0[0x58] = 0;
            }
            out.push(snd(0x58));
        }
    }
    // 0x0047d6af: two ingredients make one of the next level.
    if ty(game) == 0xb && game.inventory[tab][index].count > 1 {
        let mut copy = game.inventory[tab][index].item.clone();
        let l = level_of(&copy).wrapping_add(1);
        copy[0x10..0x12].copy_from_slice(&l.to_le_bytes());
        take_one(&mut game.inventory[tab][index]);
        take_one(&mut game.inventory[tab][index]);
        add_item(game, &copy, -1);
        out.push(snd(0x58));
    }
    // 0x0047d717: consumables and formulas.
    match ty(game) {
        1 => out.push(UiAction::UseItem { tab: tab as i32, index: index as i32 }),
        2 => {
            let s = &game.inventory[tab][index];
            let mut f = s.item.clone();
            f[0] = s.item[8];
            f[8..0xc].fill(0);
            if knows_formula(game, &f) {
                out.push(snd(0x31));
            } else {
                game.formulas.push(f);
                take_one(&mut game.inventory[tab][index]);
                out.push(snd(0x2f));
            }
        }
        _ => {}
    }
    out
}

/// Appends a sold item to the buy-back list (`GC+0x800d3c`, count 1) and drops the oldest
/// entry beyond 10 (0x0047ceee / 0x0047d90e).
fn push_buyback(shop: &mut ShopData, item: Vec<u8>) {
    shop.buyback.push(ItemStack { count: 1, item });
    if shop.buyback.len() > 10 {
        shop.buyback.remove(0);
    }
}

/// 0x0047cdf5..0x0047cf67: with the shop open, a right/middle click on a bag cell sells one
/// item: one off the stack, coins + [`price`], sound 0x3a at the player (volume 0.75), the
/// item into the buy-back list, the shop pages rebuilt.
pub fn sell_from_bag(game: &mut GameView, shop: &mut ShopData, tab: usize, index: usize) -> Vec<UiAction> {
    let copy = game.inventory[tab][index].item.clone();
    take_one(&mut game.inventory[tab][index]);
    game.coins = game.coins.wrapping_add(price(&copy));
    let out = vec![UiAction::PlaySound { id: 0x3a, volume: 0.75, pitch: 1.0 }, UiAction::RefreshShop];
    push_buyback(shop, copy);
    out
}

/// 0x0047d867..0x0047d998: with the shop open, a right/middle click on a non-empty equipment
/// slot sells it: coins + [`price`], sound 0x3a (0.75), the item into the buy-back list, the
/// shop rebuilt, then the slot's type and sub type cleared.
pub fn sell_equipment(game: &mut GameView, shop: &mut ShopData, k: usize) -> Vec<UiAction> {
    let copy = game.equipment[k].clone();
    game.coins = game.coins.wrapping_add(price(&copy));
    push_buyback(shop, copy);
    game.equipment[k][0] = 0;
    game.equipment[k][1] = 0;
    vec![UiAction::PlaySound { id: 0x3a, volume: 0.75, pitch: 1.0 }, UiAction::RefreshShop]
}

/// 0x0047d9da..0x0047da2b: with no trade panel open, a right/middle click on a non-empty
/// equipment slot puts it back into the bag (`addItem`) and clears the slot's type and sub
/// type; sound 0x58.
pub fn unequip(game: &mut GameView, k: usize) -> Vec<UiAction> {
    let copy = game.equipment[k].clone();
    add_item(game, &copy, -1);
    game.equipment[k][0] = 0;
    game.equipment[k][1] = 0;
    vec![snd(0x58)]
}

/// 0x0047dc3d..0x0047dd70: buying shop cell `(tab, index)` (the vendor's tab): nothing when
/// the [`price`] exceeds the coins; "You can't carry more of these items." (sound 0x31) when
/// [`has_room`] fails; else the item is added (`addItem`), paid for, and sound 0x39 plays at
/// the player.
pub fn buy(game: &mut GameView, shop: &ShopData, tab: usize, index: usize) -> Vec<UiAction> {
    let item = shop.pages[tab][index].item.clone();
    if price(&item) > game.coins {
        return Vec::new();
    }
    if !has_room(game, &item) {
        return vec![cant_carry(), snd(0x31)];
    }
    add_item(game, &item, -1);
    game.coins = game.coins.wrapping_sub(price(&item));
    vec![snd(0x39)]
}

/// 0x0047da47..0x0047dc38: buying back on the shop's tab 1. The hovered index `hovered`
/// counts from the newest entry (the tab shows the list newest first): entry `len - 1 -
/// hovered` of the list. For each of its `count` items: stop with "You can't carry more of
/// these items." (sound 0x31) when [`has_room`] fails; buy it when the shown item's [`price`]
/// (shop cell `(tab, index)`) is within the coins. The entry loses the bought items and is
/// removed at 0; sound 0x39 and the shop rebuilt. No entry: nothing.
pub fn buy_back(game: &mut GameView, shop: &mut ShopData, hovered: i32, tab: usize, index: usize) -> Vec<UiAction> {
    let mut out = Vec::new();
    let n = shop.buyback.len() as i32;
    let pos = n - 1 - hovered;
    if hovered < 0 || pos < 0 || pos >= n {
        return out;
    }
    let pos = pos as usize;
    let shown = shop.pages[tab][index].item.clone();
    let item = shop.buyback[pos].item.clone();
    let mut bought = 0;
    let mut i = 0;
    if shop.buyback[pos].count >= 1 {
        loop {
            if !has_room(game, &item) {
                out.push(cant_carry());
                out.push(snd(0x31));
                break;
            }
            if price(&shown) <= game.coins {
                add_item(game, &item, -1);
                game.coins = game.coins.wrapping_sub(price(&item));
                bought += 1;
            }
            i += 1;
            if shop.buyback[pos].count <= i {
                break;
            }
        }
    }
    shop.buyback[pos].count -= bought;
    if shop.buyback[pos].count == 0 {
        shop.buyback.remove(pos);
    }
    out.push(snd(0x39));
    out.push(UiAction::RefreshShop);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(ty: u8, sub: u8, count: i32) -> ItemStack {
        let mut s = ItemStack::empty();
        s.count = count;
        s.item[0] = ty;
        s.item[1] = sub;
        s.item[0x10] = 1;
        s
    }

    fn game() -> GameView {
        let mut g = GameView::default();
        // Level 10 warrior.
        g.player.0[0x180..0x184].copy_from_slice(&10i32.to_le_bytes());
        g.player.0[0x130] = 1;
        g.inventory = vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        g
    }

    fn sounds(a: &[UiAction]) -> Vec<u32> {
        a.iter().filter_map(|x| if let UiAction::PlaySound { id, .. } = x { Some(*id) } else { None }).collect()
    }

    #[test]
    fn level_and_class() {
        assert_eq!(item_level(&stack(3, 0, 1).item), 1);
        assert_eq!(level_cap(1), 1);
        // (1 - 1/1.45) * 100 + 1 = 32.03.
        assert_eq!(level_cap(10), 32);
        let mut sword = stack(3, 0, 1).item;
        assert!(class_allows(&sword, 1) && !class_allows(&sword, 3));
        sword[1] = 0xa; // staff
        assert!(class_allows(&sword, 3) && !class_allows(&sword, 1));
        let mut chest = stack(4, 0, 1).item;
        chest[0xd] = 0x1b;
        assert!(class_allows(&chest, 4) && !class_allows(&chest, 2));
        // A formula tests its product type (Item+8).
        let mut f = stack(2, 0xa, 1).item;
        f[8] = 3;
        assert!(class_allows(&f, 3) && !class_allows(&f, 2));
        let g = game();
        let mut hi = stack(3, 0, 1).item;
        hi[0x10..0x12].copy_from_slice(&100i16.to_le_bytes());
        assert!(!can_equip(&g.player, &hi));
        assert!(can_equip(&g.player, &stack(3, 0, 1).item));
    }

    #[test]
    fn prices() {
        // itemPower(1, 0) = 1: 10 * 2 = 20 for a sword, 40 two-handed, 2 for a potion.
        assert_eq!(price(&stack(3, 0, 1).item), 20);
        assert_eq!(price(&stack(3, 0xf, 1).item), 40);
        assert_eq!(price(&stack(1, 0, 1).item), 2);
    }

    #[test]
    fn add_item_stacks_and_coins() {
        let mut g = game();
        add_item(&mut g, &stack(1, 2, 1).item, -1);
        add_item(&mut g, &stack(1, 2, 1).item, -1);
        assert_eq!(g.inventory[1].len(), 1);
        assert_eq!(g.inventory[1][0].count, 2);
        // A non-stackable weapon takes a new slot; an emptied slot is reused first.
        add_item(&mut g, &stack(3, 0, 1).item, -1);
        add_item(&mut g, &stack(3, 0, 1).item, -1);
        assert_eq!(g.inventory[0].len(), 2);
        g.inventory[0][0] = ItemStack::empty();
        add_item(&mut g, &stack(3, 1, 1).item, -1);
        assert_eq!(g.inventory[0][0].item[1], 1);
        // Gold coins: level 3 -> 3 * 10000.
        let mut c = stack(0xc, 0, 1).item;
        c[0xd] = 0xb;
        c[0x10] = 3;
        add_item(&mut g, &c, -1);
        assert_eq!(g.coins, 30000);
        // Ingredients count their level and are stored at level 1.
        let mut ing = stack(0xb, 1, 1).item;
        ing[0x10] = 4;
        add_item(&mut g, &ing, -1);
        assert_eq!((g.inventory[2][0].count, g.inventory[2][0].item[0x10]), (4, 1));
    }

    #[test]
    fn merge_and_swap() {
        let mut g = game();
        g.inventory[1] = vec![stack(1, 2, 3), stack(1, 5, 1)];
        let mut bag = InventoryWidget::new(0, None);
        bag.tab = 1;
        bag.hovered_tab = 1;
        bag.hovered_index = 0;
        // Pick up: the cursor takes the stack.
        let a = bag_left_click(&mut g, &bag, true, false);
        assert_eq!(sounds(&a), vec![0x57]);
        assert_eq!(g.held.count, 3);
        assert_eq!(g.inventory[1][0].count, 0);
        // Merge onto an equal stack.
        g.inventory[1][0] = stack(1, 2, 2);
        let a = bag_left_click(&mut g, &bag, true, false);
        assert!(a.is_empty());
        assert_eq!(g.inventory[1][0].count, 5);
        assert_eq!((g.held.count, g.held.item[0]), (0, 0));
        // Swap with a different item.
        g.held = stack(1, 7, 1);
        bag.hovered_index = 1;
        let a = bag_left_click(&mut g, &bag, true, false);
        assert_eq!(sounds(&a), vec![0x57]);
        assert_eq!((g.held.item[1], g.inventory[1][1].item[1]), (5, 7));
        // Shift: no swap.
        let a = bag_left_click(&mut g, &bag, true, true);
        assert!(a.is_empty());
        // Wrong tab.
        g.held = stack(3, 0, 1);
        let a = bag_left_click(&mut g, &bag, true, false);
        assert_eq!(sounds(&a), vec![0x32]);
    }

    #[test]
    fn equipment_swap() {
        let mut g = game();
        g.held = stack(3, 0xf, 1); // greatsword
        g.equipment[slot::LEFT] = stack(3, 0xd, 1).item; // shield
        // Two-handed into the right hand: the shield goes to the bag.
        let a = equipment_left_click(&mut g, Some(slot::RIGHT));
        assert_eq!(sounds(&a), vec![0x58]);
        assert_eq!(g.equipment[slot::RIGHT][1], 0xf);
        assert_eq!(g.equipment[slot::LEFT][0], 0);
        assert_eq!(g.inventory[0][0].item[1], 0xd);
        assert_eq!(g.held.count, 0);
        // Pick the greatsword up again.
        let a = equipment_left_click(&mut g, Some(slot::RIGHT));
        assert_eq!(sounds(&a), vec![0x57]);
        assert_eq!((g.held.count, g.held.item[1]), (1, 0xf));
        // A two-handed sword is refused in the left hand.
        let a = equipment_left_click(&mut g, Some(slot::LEFT));
        assert_eq!(sounds(&a), vec![0x32]);
        assert_eq!(g.held.item[1], 0xf);
        // Wrong type for the neck: nothing.
        let a = equipment_left_click(&mut g, Some(slot::NECK));
        assert!(a.is_empty());
    }

    #[test]
    fn ingredient_split_and_formula() {
        let mut g = game();
        let mut ing = stack(0xb, 0xe, 3);
        ing.item[0x10] = 2;
        g.inventory[2] = vec![ing];
        let a = use_or_equip(&mut g, 2, 0, 1);
        assert_eq!(sounds(&a), vec![0x58]);
        assert_eq!(g.inventory[2][0].count, 1);
        assert_eq!(g.inventory[2][1].item[0x10], 3);
        let mut f = stack(2, 0, 2);
        f.item[8] = 3;
        g.inventory[1] = vec![f];
        let a = use_or_equip(&mut g, 1, 0, 1);
        assert_eq!(sounds(&a), vec![0x2f]);
        assert_eq!(g.formulas.len(), 1);
        assert_eq!(g.formulas[0][0], 3);
        let a = use_or_equip(&mut g, 1, 0, 1);
        assert_eq!(sounds(&a), vec![0x31]);
        g.inventory[1] = vec![stack(1, 2, 1)];
        let a = use_or_equip(&mut g, 1, 0, 1);
        assert_eq!(a, vec![UiAction::UseItem { tab: 1, index: 0 }]);
    }

    #[test]
    fn equip_rules() {
        let mut g = game();
        g.inventory[0] = vec![stack(3, 0, 1), stack(3, 0xf, 1), stack(3, 0xd, 1)];
        use_or_equip(&mut g, 0, 0, 1);
        assert_eq!(g.equipment[slot::RIGHT][0], 3);
        assert_eq!(g.inventory[0][0].count, 0);
        use_or_equip(&mut g, 0, 1, 1);
        assert_eq!(g.equipment[slot::RIGHT][1], 0xf);
        assert!(g.inventory[0].iter().any(|s| s.count == 1 && s.item[0] == 3 && s.item[1] == 0));
        use_or_equip(&mut g, 0, 2, 1);
        assert_eq!(g.equipment[slot::LEFT][1], 0xd);
        assert_eq!(g.equipment[slot::RIGHT][0], 0);
        assert_eq!(g.equipment[slot::RIGHT][0x10], 1);
    }

    #[test]
    fn sell_and_buy_back() {
        let mut g = game();
        let mut shop = ShopData::default();
        g.inventory[0] = vec![stack(3, 0, 2)];
        for _ in 0..2 {
            let a = sell_from_bag(&mut g, &mut shop, 0, 0);
            assert_eq!(a[0], UiAction::PlaySound { id: 0x3a, volume: 0.75, pitch: 1.0 });
        }
        assert_eq!(g.coins, 40);
        assert_eq!(g.inventory[0][0].count, 0);
        assert_eq!(shop.buyback.len(), 2);
        for _ in 0..10 {
            g.inventory[0][0] = stack(3, 1, 1);
            sell_from_bag(&mut g, &mut shop, 0, 0);
        }
        assert_eq!(shop.buyback.len(), 10);
        assert_eq!(shop.buyback[0].item[1], 1);
        // Tab 1 shows the list newest first.
        shop.refresh(1, None);
        assert_eq!(shop.pages[1].len(), 10);
        // Buy back the newest (hovered 0 = last entry).
        g.coins = 20;
        let a = buy_back(&mut g, &mut shop, 0, 1, 0);
        assert_eq!(sounds(&a), vec![0x39]);
        assert_eq!(g.coins, 0);
        assert_eq!(shop.buyback.len(), 9);
        // Not enough coins: nothing bought, the entry stays, the sound still plays.
        shop.refresh(1, None);
        let a = buy_back(&mut g, &mut shop, 0, 1, 0);
        assert_eq!(sounds(&a), vec![0x39]);
        assert_eq!(shop.buyback.len(), 9);
        // Vendor tab: too expensive, nothing at all.
        shop.pages[0] = vec![stack(3, 0, 1)];
        assert!(buy(&mut g, &shop, 0, 0).is_empty());
        g.coins = 25;
        assert_eq!(sounds(&buy(&mut g, &shop, 0, 0)), vec![0x39]);
        assert_eq!(g.coins, 5);
    }

    #[test]
    fn room_limit() {
        let mut g = game();
        g.inventory[1] = vec![stack(1, 2, 49)];
        assert!(has_room(&g, &stack(1, 2, 1).item));
        g.held = stack(1, 2, 1);
        assert!(!has_room(&g, &stack(1, 2, 1).item));
        let mut s = ShopData::default();
        s.pages = vec![vec![stack(1, 2, 1)]];
        g.coins = 100;
        let a = buy(&mut g, &s, 0, 0);
        assert_eq!(sounds(&a), vec![0x31]);
    }
}
