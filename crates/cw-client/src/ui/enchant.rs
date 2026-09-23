//! The identification panel: `cube::EnchantWidget` (vtable 0x006ffc9c, ctor 0x0044e910, 0x174
//! bytes; panel `GC+0x8008f8`, widget `GC+0x8008fc`) and the GameController code around it.
//!
//! Tier B. The panel opens from the identifier NPC (creature class 0x83) through
//! `0x004889e0`; a right/middle click on a bag cell makes it the target (the inventory's
//! [`UiAction::SetItemTarget`], [`set_target`]); only unidentified items (type 0xe) count as
//! a target. "Identify" (left click, bottom-left half) pays `max(1, price / 2)` coins, takes
//! one item off the stack and adds a random weapon or armour piece of the item's level and
//! rarity; "Goodbye!" (any button released over the bottom-right half) closes the panel.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | target item 0x00450960 | [`target_item`] |
//! | cost 0x00450920 | [`cost`] |
//! | "Identify" hovered 0x00450ab0 | [`identify_hovered`] |
//! | "Goodbye!" hovered 0x00450a00 | [`goodbye_hovered`] |
//! | target is unidentified 0x00450b70 | [`target_item`] (the same test) |
//! | `EnchantWidget::update` (slot 1) 0x0044ea30 | [`frame`] |
//! | `EnchantWidget::layout` (slot 10) 0x00450b90 | [`layout`] |
//! | `0x004889e0`, class 0x83 | [`on_creature_menu`] |
//! | `onMouseDown` 0x0047cda4 (bag cell while the panel is open) | [`set_target`] |
//! | `onMouseDown` 0x0047c8ea..0x0047ccf5 (left button) | [`identify_click`] |
//! | `onMouseUp` 0x0047de35..0x0047de8f | [`on_mouse_up`] |
//! | the item-frame tooltip 0x0047b010 | [`frame_tooltip`] |
//! | `update` 0x004964e7..0x00496548 (with the adaption panel) | [`trade_panel_rules`] |

use cw_math::rand::MsvcRand;
use cw_ui::widget::Gui;
use glam::Vec2;

use super::crafting::{i16_at, set_vis, snd, vis, ItemIcon, PanelText, CYAN, GRAY, WHITE};
use super::inventory::{add_item, price, take_one, ItemRef};
use super::members::GcMembers;
use super::{GameView, UiAction};

/// `cube::EnchantWidget` fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnchantWidget {
    /// `+0x160`: target bag tab, -1 none.
    pub tab: i32,
    /// `+0x164`: target index in the tab, -1 none.
    pub index: i32,
    /// `+0x168`: 1 identify (set by the ctor and by 0x004889e0); 0 the disenchant branch of
    /// [`identify_click`], which nothing selects.
    pub mode: i32,
}

impl Default for EnchantWidget {
    /// The ctor 0x0044e910: target (-1, -1), mode 1. (`+0x16c` is the GameController, `+0x170`
    /// the `itemframe` node.)
    fn default() -> Self {
        EnchantWidget { tab: -1, index: -1, mode: 1 }
    }
}

/// The bag stack of the target when `(tab, index)` lies in the bag and its count is not 0.
fn target_stack(game: &GameView, w: &EnchantWidget) -> Option<(usize, usize)> {
    if game.inventory.is_empty() || w.tab < 0 || w.index < 0 {
        return None;
    }
    let (t, i) = (w.tab as usize, w.index as usize);
    let s = game.inventory.get(t)?.get(i)?;
    (s.count != 0).then_some((t, i))
}

/// `0x00450960`: the target item, when the target stack exists ([`target_stack`]) and holds
/// an unidentified item (type 0xe).
pub fn target_item<'a>(game: &'a GameView, w: &EnchantWidget) -> Option<&'a [u8]> {
    let (t, i) = target_stack(game, w)?;
    let item = &game.inventory[t][i].item;
    (item[0] == 0xe).then_some(&item[..])
}

/// `0x00450920`: the identification cost in coins, `max(1, price / 2)` ([`price`]
/// 0x004c76e0), 0 without a target.
pub fn cost(game: &GameView, w: &EnchantWidget) -> i32 {
    match target_item(game, w) {
        None => 0,
        Some(item) => {
            if price(item) / 2 < 1 {
                1
            } else {
                price(item) / 2
            }
        }
    }
}

/// `0x00450a00`: the local cursor `c` is over "Goodbye!", `h - 30 < y < h` and `w / 2 < x <
/// w` (each a `comiss`/`jbe`: NaN is outside). Also the adaption panel's.
pub fn goodbye_hovered(c: Vec2, size: Vec2) -> bool {
    size.y - 30.0 < c.y && c.y < size.y && size.x * 0.5 < c.x && c.x < size.x
}

/// `0x00450ab0`: a target exists and the local cursor is over "Identify", `h - 30 < y < h`
/// and `0 < x < w / 2`.
pub fn identify_hovered(game: &GameView, w: &EnchantWidget, c: Vec2, size: Vec2) -> bool {
    target_item(game, w).is_some() && size.y - 30.0 < c.y && c.y < size.y && c.x < size.x * 0.5 && 0.0 < c.x
}

/// `0x004889e0` for a creature of class 0x83 (the identifier; the gameplay side's
/// `Event::CreatureMenu`): the identification panel opens with no target, mode 1.
pub fn on_creature_menu(gui: &mut Gui, m: &GcMembers, w: &mut EnchantWidget) {
    set_vis(gui, m.enchant_panel, true);
    w.tab = -1;
    w.index = -1;
    w.mode = 1;
}

/// `onMouseDown` 0x0047cda4 ([`UiAction::SetItemTarget`] with `TargetPanel::Enchant`): the
/// clicked bag cell becomes the target (equipment is never offered to this panel).
pub fn set_target(w: &mut EnchantWidget, item: ItemRef) {
    if let ItemRef::Bag { tab, index } = item {
        w.tab = tab;
        w.index = index;
    }
}

/// `onMouseUp` 0x0047de35 (any button): with the panel open and the cursor over "Goodbye!"
/// ([`goodbye_hovered`], widget-local `c`), the panel closes and the target is cleared.
pub fn on_mouse_up(gui: &mut Gui, m: &GcMembers, w: &mut EnchantWidget, c: Vec2, size: Vec2) {
    if vis(gui, m.enchant_panel) && goodbye_hovered(c, size) {
        set_vis(gui, m.enchant_panel, false);
        w.tab = -1;
        w.index = -1;
    }
}

/// `onMouseDown` 0x0047c8ea..0x0047ccf5 (left button, after the adaption and skill parts): with
/// the panel open, "Identify" hovered ([`identify_hovered`]) and at least [`cost`] coins:
///
/// - mode 1 (identify): the target stack must still exist, hold items and be unidentified.
///   The coins drop by the cost, the stack loses one (0x0042f140), then `rand()` picks the
///   generator: odd, the armour one (0x005f51e0, [`random_item_528bf0`]); even, the weapon one
///   (0x005f8ad0, [`random_item_52c4e0`]), with the stack item's level (`+0x10`) and rarity
///   (`+0xc`), category -1, on the client's `rand` stream. The new item goes to the bag
///   (`addItem(item, -1)`), sound 0x2e, "You receive ..." ([`UiAction::ItemReceived`]).
/// - mode 0 (never selected): a stack whose item has a material loses one (the target is
///   cleared when items remain), `rand() % 6 + 5` type-0xa items of that material go to the
///   pickup queue ([`UiAction::QueuePickups`]) with sound 0x2e; then, material or not, the
///   stack loses one more. No coins are taken.
///
/// [`random_item_528bf0`]: cw_world::settlement::shops::random_item_528bf0
/// [`random_item_52c4e0`]: cw_world::settlement::shops::random_item_52c4e0
pub fn identify_click(gui: &Gui, m: &GcMembers, w: &mut EnchantWidget, game: &mut GameView, rng: &mut MsvcRand, c: Vec2, size: Vec2) -> Vec<UiAction> {
    let mut out = Vec::new();
    if !(vis(gui, m.enchant_panel) && identify_hovered(game, w, c, size)) {
        return out;
    }
    if game.coins < cost(game, w) {
        return out;
    }
    let Some((t, i)) = target_stack(game, w) else { return out };
    if target_item(game, w).is_none() {
        return out;
    }
    match w.mode {
        1 => {
            let c = cost(game, w);
            game.coins -= c;
            let stack = &mut game.inventory[t][i];
            take_one(stack);
            let level = i16_at(&stack.item, 0x10);
            let rarity = stack.item[0xc];
            // 0x0047ca4c: `rand() & 0x80000001` (never negative): odd -> armour.
            let r = rng.rand();
            let new = if r & 1 != 0 {
                cw_world::settlement::shops::random_item_528bf0(rng, level, rarity, -1)
            } else {
                cw_world::settlement::shops::random_item_52c4e0(rng, level, rarity, -1)
            };
            let bytes = new.to_bytes().to_vec();
            add_item(game, &bytes, -1);
            out.push(snd(0x2e));
            out.push(UiAction::ItemReceived { item: bytes });
        }
        0 => {
            let stack = &mut game.inventory[t][i];
            if stack.item[0xd] != 0 {
                take_one(stack);
                if stack.count != 0 {
                    w.tab = -1;
                    w.index = -1;
                }
                let n = rng.rand() % 6 + 5;
                let mut piece = super::crafting::default_item();
                piece[0] = 0xa;
                piece[0xd] = game.inventory[t][i].item[0xd];
                out.push(UiAction::QueuePickups { items: vec![piece; n as usize] });
                out.push(snd(0x2e));
            }
            // 0x0047ccb4: the stack at the (original) target loses one more.
            let s = &mut game.inventory[t][i];
            s.count -= 1;
            if s.count <= 0 {
                s.count = 0;
                s.item[0] = 0;
                s.item[1] = 0;
            }
        }
        _ => {}
    }
    out
}

/// `0x0047b010`: the tooltip item of the panel, the target stack's item (count not 0, no type
/// test) while the panel is open and the screen cursor is inside the 120x120 square at
/// `frame_pos`: the panel node's (`GC+0x8008f8`) Transformation pivot (`+0x38 →
/// +0x19c[+0x170]`) through its world matrix (`node+0x48`, divided by w), in screen space
/// (not the `itemframe`, and not the node's translation). Not called; the port's hover
/// source is `item_preview::item_source`.
pub fn frame_tooltip<'a>(gui: &Gui, m: &GcMembers, w: &EnchantWidget, game: &'a GameView, frame_pos: Vec2, cursor: Vec2) -> Option<&'a [u8]> {
    if !vis(gui, m.enchant_panel) {
        return None;
    }
    let (t, i) = target_stack(game, w)?;
    let inside = frame_pos.x <= cursor.x && frame_pos.y <= cursor.y && cursor.x < frame_pos.x + 120.0 && cursor.y < frame_pos.y + 120.0;
    inside.then_some(&game.inventory[t][i].item[..])
}

/// The item description block `0x004a28c0(item, x, y, 1.0, width, flag, 0, 0)`
/// ([`super::item_description::describe`], drawn in the panel widget's space by
/// `present_panels`).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDescription {
    /// The item.
    pub item: Vec<u8>,
    /// Widget-local position.
    pub x: i32,
    /// Widget-local position.
    pub y: i32,
    /// Wrap width (300).
    pub width: i32,
    /// The sixth argument, the name line (`[ebp+0x1c]` of 0x004a28c0): 1 for a panel target,
    /// 0 for the adaption panel's adapted copy; the class line (seventh) is 0 for both panels.
    pub flag: i32,
}

impl ItemDescription {
    /// The arguments of `0x004a28c0` (scale 1, no class line).
    pub fn args(&self) -> super::item_description::DescriptionArgs {
        super::item_description::DescriptionArgs { x: self.x, y: self.y, scale: 1.0, width: self.width, name: self.flag != 0, class_line: false }
    }
}

/// What the identification panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnchantFrame {
    /// The target's model, centred on the `itemframe` node, scale 0.06 (only when the item has
    /// a model, 0x004ec400).
    pub icon: Option<ItemIcon>,
    /// The target's description at (15, 180), width 300.
    pub description: Option<ItemDescription>,
    /// Title, buttons and the cost line.
    pub texts: Vec<PanelText>,
}

/// The cost line of the identification panel (0x0044f9d0..): with a target, at `y = (int)(h -
/// 50)`, size 10: copper `c % 100` + " C " at x 220 (0.8, 0.5, 0, 1), silver `c / 100 % 100`
/// + " S " at 180 (0.7, 0.7, 0.7, 1), gold `c / 100 / 100` + " G " at 140 (1, 0.9, 0, 1), all
/// right-aligned (2), then "COST:" at 15 in white.
fn cost_texts(c: i32, h: f32) -> Vec<PanelText> {
    let y = (h - 50.0) as i32 as f32;
    vec![
        PanelText::new(format!("{} C ", c % 100), 220.0, y, 10.0, 2.0, [0.8, 0.5, 0.0, 1.0], 2),
        PanelText::new(format!("{} S ", (c / 100) % 100), 180.0, y, 10.0, 2.0, [0.7, 0.7, 0.7, 1.0], 2),
        PanelText::new(format!("{} G ", (c / 100) / 100), 140.0, y, 10.0, 2.0, [1.0, 0.9, 0.0, 1.0], 2),
        PanelText::new("COST:", 15.0, y, 10.0, 2.0, WHITE, 0),
    ]
}

/// `EnchantWidget::update` 0x0044ea30. `size` is the widget size, `c` the widget-local cursor,
/// `frame_rect` the `itemframe` node's screen position and size (the icon is drawn at its
/// centre). The original's centre (0x0044ea8d..0x0044ed42) is `(w · 0.5 + (world(pivot).x +
/// rel.x), h · 0.5 + (rel.y + world(pivot).y))`: `world(pivot)` the EnchantWidget node's
/// Transformation pivot (`+0x19c[+0x170]`) through the node's world matrix, `rel` the
/// `itemframe` widget's origin relative to its parent widget (0x0062b510), `w`, `h` its size
/// (0x0062f600 / 0x006291d0). That is the `itemframe` widget's screen origin + size / 2
/// whenever `world(pivot)` is the EnchantWidget's own screen origin, which holds for the
/// shipped `gui.plx` (pivot 0, identity bind/frame).
///
/// - A target: its icon and its description.
/// - "Identification" at (15, 25), size 12.
/// - "Identify" at (w / 3, h - 20), size 12, centred: gray without a target, cyan hovered
///   ([`identify_hovered`]), else white.
/// - "Goodbye!" at (2w / 3, h - 20): cyan hovered ([`goodbye_hovered`]), else white.
/// - A target: the cost ([`cost`]) in coins.
pub fn frame(game: &GameView, w: &EnchantWidget, c: Vec2, size: Vec2, frame_rect: (Vec2, Vec2)) -> EnchantFrame {
    let mut f = EnchantFrame::default();
    let target = target_item(game, w);
    if let Some(item) = target {
        if crate::interact::item_model_index(item).is_some() {
            let (p, s) = frame_rect;
            f.icon = Some(ItemIcon { item: item.to_vec(), center: [s.x * 0.5 + p.x, s.y * 0.5 + p.y], scale: 0.06 });
        }
        f.description = Some(ItemDescription { item: item.to_vec(), x: 0xf, y: 0xb4, width: 300, flag: 1 });
    }
    let (wd, h) = (size.x, size.y);
    f.texts.push(PanelText::new("Identification", 15.0, 25.0, 12.0, 3.0, WHITE, 0));
    let identify = if target.is_none() {
        GRAY
    } else if identify_hovered(game, w, c, size) {
        CYAN
    } else {
        WHITE
    };
    f.texts.push(PanelText::new("Identify", wd / 3.0, h - 20.0, 12.0, 3.0, identify, 1));
    let bye = if goodbye_hovered(c, size) { CYAN } else { WHITE };
    f.texts.push(PanelText::new("Goodbye!", wd * 2.0 / 3.0, h - 20.0, 12.0, 3.0, bye, 1));
    if target.is_some() {
        f.texts.extend(cost_texts(cost(game, w), h));
    }
    f
}

/// `EnchantWidget::layout` 0x00450b90: the `itemframe` node (`+0x170`) at `((w - fw) / 2,
/// 50)`.
pub fn layout(size: Vec2, frame_size: Vec2) -> Vec2 {
    Vec2::new((size.x - frame_size.x) * 0.5, 50.0)
}

/// `update` 0x004964e7..0x00496548: with the identification or the adaption panel open the
/// inventory panel opens; with the inventory panel closed the shop, identification and
/// adaption panels close.
pub fn trade_panel_rules(gui: &mut Gui, m: &GcMembers) {
    if vis(gui, m.enchant_panel) || vis(gui, m.adaption_panel) {
        set_vis(gui, m.inventory_panel, true);
    }
    if !vis(gui, m.inventory_panel) {
        set_vis(gui, m.shop_panel, false);
        set_vis(gui, m.enchant_panel, false);
        set_vis(gui, m.adaption_panel, false);
    }
}

#[cfg(test)]
mod tests {
    use super::super::members::NoPlx;
    use super::super::{GameUi, ItemStack};
    use super::*;

    fn unidentified(level: i16, rarity: u8) -> ItemStack {
        let mut it = super::super::crafting::default_item();
        it[0] = 0xe;
        it[0xc] = rarity;
        it[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        ItemStack { count: 2, item: it }
    }

    #[test]
    fn identify_flow() {
        let game0 = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game0);
        let mut game = GameView::default();
        game.inventory = vec![vec![unidentified(10, 2)]];
        game.coins = 1000;
        let mut w = EnchantWidget::default();
        on_creature_menu(&mut ui.gui, &ui.m, &mut w);
        assert!(vis(&ui.gui, ui.m.enchant_panel));
        let size = Vec2::new(350.0, 285.0);
        let over = Vec2::new(50.0, 270.0);
        // No target: gray, nothing happens.
        let f = frame(&game, &w, over, size, (Vec2::ZERO, Vec2::splat(120.0)));
        assert_eq!(f.texts[1].color, GRAY);
        let mut rng = MsvcRand::new(7);
        assert!(identify_click(&ui.gui, &ui.m, &mut w, &mut game, &mut rng, over, size).is_empty());
        set_target(&mut w, ItemRef::Bag { tab: 0, index: 0 });
        let c = cost(&game, &w);
        assert_eq!(c, (price(&game.inventory[0][0].item) / 2).max(1));
        let f = frame(&game, &w, over, size, (Vec2::ZERO, Vec2::splat(120.0)));
        assert_eq!(f.texts[1].color, CYAN);
        assert_eq!(f.texts.len(), 7);
        let a = identify_click(&ui.gui, &ui.m, &mut w, &mut game, &mut rng, over, size);
        assert_eq!(game.coins, 1000 - c);
        assert_eq!(game.inventory[0][0].count, 1);
        assert!(a.contains(&snd(0x2e)));
        let UiAction::ItemReceived { item } = &a[1] else { panic!() };
        assert!(matches!(item[0], 3..=9));
        assert_eq!(i16_at(item, 0x10), 10);
        // Goodbye closes.
        on_mouse_up(&mut ui.gui, &ui.m, &mut w, Vec2::new(300.0, 270.0), size);
        assert!(!vis(&ui.gui, ui.m.enchant_panel));
        assert_eq!((w.tab, w.index), (-1, -1));
    }

    #[test]
    fn cost_digits() {
        let t = cost_texts(12345, 285.0);
        assert_eq!(t[0].text, "45 C ");
        assert_eq!(t[1].text, "23 S ");
        assert_eq!(t[2].text, "1 G ");
        assert_eq!(t[0].y, 235.0);
    }

    #[test]
    fn panel_rules() {
        let game0 = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game0);
        set_vis(&mut ui.gui, ui.m.adaption_panel, true);
        trade_panel_rules(&mut ui.gui, &ui.m);
        assert!(vis(&ui.gui, ui.m.inventory_panel));
        set_vis(&mut ui.gui, ui.m.inventory_panel, false);
        set_vis(&mut ui.gui, ui.m.adaption_panel, false);
        set_vis(&mut ui.gui, ui.m.enchant_panel, false);
        set_vis(&mut ui.gui, ui.m.shop_panel, true);
        trade_panel_rules(&mut ui.gui, &ui.m);
        assert!(!vis(&ui.gui, ui.m.shop_panel));
    }
}
