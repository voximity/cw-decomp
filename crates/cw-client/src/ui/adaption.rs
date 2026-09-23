//! The adaption panel: `cube::AdaptionWidget` (vtable 0x006fcbd4, ctor 0x0040ecd0, 0x178
//! bytes; panel `GC+0x800900`, widget `GC+0x800904`) and the GameController code around it.
//!
//! Tier B. The panel opens from the adapter NPC (creature class 0x89, `0x004889e0`). A
//! right/middle click on a bag cell or an equipment slot makes that item the target (the
//! inventory's [`UiAction::SetItemTarget`], [`set_target`]). Gear (types 3..9) not yet adapted
//! (`Item+0xe` bit 0 clear; crafted gear has it set) can be raised to the player's level for
//! platinum coins: "Adapt" (left click, bottom-left half) pays, sets the item level to the
//! player's and marks the item adapted. "Goodbye!" (any button released over the bottom-right
//! half) closes the panel.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | target item 0x0040f570 | [`target_item`] |
//! | cost 0x0040f4f0 (also inline in the update) | [`cost`] |
//! | "Adapt" hovered 0x00411340 | [`adapt_hovered`] |
//! | "Goodbye!" hovered 0x00450a00 | [`super::enchant::goodbye_hovered`] |
//! | `AdaptionWidget::update` (slot 1) 0x0040f8f0 | [`frame`] |
//! | `AdaptionWidget::layout` (slot 10) 0x00411410 | [`layout`] |
//! | `0x004889e0`, class 0x89 | [`on_creature_menu`] |
//! | `onMouseDown` 0x0047cd65 (bag cell) / 0x0047d808 (equipment slot) | [`set_target`] |
//! | `onMouseDown` 0x0047c748..0x0047c806 (left button) | [`adapt_click`] |
//! | `onMouseUp` 0x0047de8f..0x0047def9 | [`on_mouse_up`] |

use cw_ui::widget::Gui;
use glam::Vec2;

use super::crafting::{i16_at, i32_at, set_vis, snd, vis, ItemIcon, PanelText, CYAN, GRAY, WHITE};
use super::enchant::{goodbye_hovered, ItemDescription};
use super::inventory::ItemRef;
use super::members::GcMembers;
use super::{GameView, UiAction};

/// `cube::AdaptionWidget` fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptionWidget {
    /// `+0x160` (an equipped item pointer) or `+0x164/+0x168` (a bag cell): the target.
    /// `None` = `+0x160` null and the cell (-1, -1).
    pub target: Option<ItemRef>,
}

impl Default for AdaptionWidget {
    /// The ctor 0x0040ecd0: `+0x160` 0, cell (-1, -1). (`+0x16c` is the GameController,
    /// `+0x170` the `itemframe` node, `+0x174` the `rightarrow` node.)
    fn default() -> Self {
        AdaptionWidget { target: None }
    }
}

/// The player's level, entity `+0x180` (`creature+0x190`).
fn player_level(game: &GameView) -> i32 {
    i32_at(&game.player.0, 0x180)
}

fn target_bytes<'a>(game: &'a GameView, w: &AdaptionWidget) -> Option<&'a [u8]> {
    match w.target? {
        ItemRef::Equipment(k) => game.equipment.get(k).map(|v| &v[..]),
        ItemRef::Bag { tab, index } => {
            if game.inventory.is_empty() || tab < 0 || index < 0 {
                return None;
            }
            let s = game.inventory.get(tab as usize)?.get(index as usize)?;
            (s.count != 0).then_some(&s.item[..])
        }
    }
}

fn target_mut<'a>(game: &'a mut GameView, w: &AdaptionWidget) -> Option<&'a mut Vec<u8>> {
    match w.target? {
        ItemRef::Equipment(k) => game.equipment.get_mut(k),
        ItemRef::Bag { tab, index } => {
            let s = game.inventory.get_mut(tab as usize)?.get_mut(index as usize)?;
            Some(&mut s.item)
        }
    }
}

/// `0x0040f570`: the target item: the equipped item (`+0x160`), else the bag cell when it
/// lies in the bag and its count is not 0; only gear (types 3, 7, 5, 4, 6, 8, 9) not yet
/// adapted (`Item+0xe` bit 0 clear).
pub fn target_item<'a>(game: &'a GameView, w: &AdaptionWidget) -> Option<&'a [u8]> {
    let item = target_bytes(game, w)?;
    if !matches!(item[0], 3 | 7 | 5 | 4 | 6 | 8 | 9) || item[0xe] & 1 != 0 {
        return None;
    }
    Some(item)
}

/// `0x0040f4f0`: the adaption cost in platinum coins. With `L` the item level (`+0x10`, i16),
/// `P` the player's and `r` the rarity: 0 unless `L < P`; else `f = (float)pow(2.0, (double)((r
/// - 1 + L) as f32 * 0.25)) * 2` and `P - L` times `acc = (int)((float)acc + f)`.
pub fn cost(game: &GameView, w: &AdaptionWidget) -> i32 {
    let Some(item) = target_item(game, w) else { return 0 };
    let l = i32::from(i16_at(item, 0x10));
    let p = player_level(game);
    let mut acc = 0i32;
    if l < p {
        let e = (i32::from(item[0xc]) - 1 + l) as f32 * 0.25f32;
        let f = cw_math::pow(2.0, f64::from(e)) as f32 * 2.0f32;
        for _ in 0..(p - l) {
            acc = super::inventory::cvtt(acc as f32 + f);
        }
    }
    acc
}

/// `0x00411340`: a target exists and the local cursor is over "Adapt", `h - 30 < y < h` and
/// `0 < x < w / 2`.
pub fn adapt_hovered(game: &GameView, w: &AdaptionWidget, c: Vec2, size: Vec2) -> bool {
    target_item(game, w).is_some() && size.y - 30.0 < c.y && c.y < size.y && c.x < size.x * 0.5 && 0.0 < c.x
}

/// `0x004889e0` for a creature of class 0x89 (the adapter): the adaption panel opens (the
/// target is kept).
pub fn on_creature_menu(gui: &mut Gui, m: &GcMembers) {
    set_vis(gui, m.adaption_panel, true);
}

/// `onMouseDown` 0x0047cd65 / 0x0047d808 ([`UiAction::SetItemTarget`] with
/// `TargetPanel::Adaption`): a bag cell sets `+0x164/+0x168` and clears `+0x160`; an equipment
/// slot sets `+0x160` and clears the cell.
pub fn set_target(w: &mut AdaptionWidget, item: ItemRef) {
    w.target = Some(item);
}

/// `onMouseUp` 0x0047de8f (any button): with the panel open and the cursor over "Goodbye!",
/// the panel closes and the target is cleared (cell (-1, -1), `+0x160` 0).
pub fn on_mouse_up(gui: &mut Gui, m: &GcMembers, w: &mut AdaptionWidget, c: Vec2, size: Vec2) {
    if vis(gui, m.adaption_panel) && goodbye_hovered(c, size) {
        set_vis(gui, m.adaption_panel, false);
        w.target = None;
    }
}

/// `onMouseDown` 0x0047c748..0x0047c806 (left button, after the inventory part): with the
/// panel open, "Adapt" hovered, a target and at least [`cost`] platinum: platinum `-=` cost,
/// the item's level (`+0x10`) becomes the player's (its low 16 bits), `Item+0xe |= 1`, sound
/// 0x2e. (A cost of 0, an item at or above the player's level, still adapts.)
pub fn adapt_click(gui: &Gui, m: &GcMembers, w: &AdaptionWidget, game: &mut GameView, c: Vec2, size: Vec2) -> Vec<UiAction> {
    let mut out = Vec::new();
    if !(vis(gui, m.adaption_panel) && adapt_hovered(game, w, c, size)) {
        return out;
    }
    if target_item(game, w).is_none() {
        return out;
    }
    let cost = cost(game, w);
    if game.platinum < cost {
        return out;
    }
    game.platinum -= cost;
    let level = player_level(game) as u16;
    if let Some(item) = target_mut(game, w) {
        item[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        item[0xe] |= 1;
    }
    out.push(snd(0x2e));
    out
}

/// What the adaption panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdaptionFrame {
    /// The target's model, centred on the `itemframe` node, scale 0.06 (items with a model).
    pub icon: Option<ItemIcon>,
    /// The target's description at (15, 180), width 300, flag 1; then a copy with the
    /// player's level at (`(int)(w / 2 + 50)`, 180), flag 0.
    pub descriptions: Vec<ItemDescription>,
    /// The `rightarrow` node (`+0x174`) between them: shown with a target.
    pub arrow_visible: bool,
    /// Title, buttons and the cost line.
    pub texts: Vec<PanelText>,
}

/// `AdaptionWidget::update` 0x0040f8f0. `size` is the widget size, `c` the widget-local
/// cursor, `frame_rect` the `itemframe` node's screen position and size. As in
/// [`super::enchant::frame`], the original (0x0040f94d..0x0040fbf2) centres the icon at the
/// AdaptionWidget node's pivot through its world matrix plus the `itemframe` widget's origin
/// relative to its parent widget (0x0062b510) plus half its size; equal to the `itemframe`'s
/// screen origin + size / 2 for the shipped `gui.plx`.
///
/// - A target: its icon and the before/after descriptions, the arrow shown; none: the arrow
///   hidden.
/// - "Adaption" at (15, 25), size 12.
/// - "Adapt" at (w / 3, h - 20): gray without a target, cyan hovered ([`adapt_hovered`]),
///   else white. "Goodbye!" at (2w / 3, h - 20): cyan hovered, else white.
/// - Always: `cost + " Platinum Coins "` at (220, `(int)(h - 50)`), size 10, outline 2.5,
///   right-aligned, (0.5, 0.2, 1, 1); "COST:" at (15, same y), white.
pub fn frame(game: &GameView, w: &AdaptionWidget, c: Vec2, size: Vec2, frame_rect: (Vec2, Vec2)) -> AdaptionFrame {
    let mut f = AdaptionFrame::default();
    let target = target_item(game, w);
    let (wd, h) = (size.x, size.y);
    if let Some(item) = target {
        if crate::interact::item_model_index(item).is_some() {
            let (p, s) = frame_rect;
            f.icon = Some(ItemIcon { item: item.to_vec(), center: [s.x * 0.5 + p.x, s.y * 0.5 + p.y], scale: 0.06 });
        }
        f.descriptions.push(ItemDescription { item: item.to_vec(), x: 0xf, y: 0xb4, width: 300, flag: 1 });
        let mut copy = item.to_vec();
        let level = player_level(game) as u16;
        copy[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        f.descriptions.push(ItemDescription { item: copy, x: (wd * 0.5 + 50.0) as i32, y: 0xb4, width: 300, flag: 0 });
        f.arrow_visible = true;
    }
    f.texts.push(PanelText::new("Adaption", 15.0, 25.0, 12.0, 3.0, WHITE, 0));
    let adapt = if target.is_none() {
        GRAY
    } else if adapt_hovered(game, w, c, size) {
        CYAN
    } else {
        WHITE
    };
    f.texts.push(PanelText::new("Adapt", wd / 3.0, h - 20.0, 12.0, 3.0, adapt, 1));
    let bye = if goodbye_hovered(c, size) { CYAN } else { WHITE };
    f.texts.push(PanelText::new("Goodbye!", wd * 2.0 / 3.0, h - 20.0, 12.0, 3.0, bye, 1));
    let y = (h - 50.0) as i32 as f32;
    f.texts.push(PanelText::new(format!("{} Platinum Coins ", cost(game, w)), 220.0, y, 10.0, 2.5, [0.5, 0.2, 1.0, 1.0], 2));
    f.texts.push(PanelText::new("COST:", 15.0, y, 10.0, 2.0, WHITE, 0));
    f
}

/// `AdaptionWidget::layout` 0x00411410: the `itemframe` node at `((w - fw) / 2, 50)` and the
/// `rightarrow` node at `(w / 2 - 10, 230)`.
pub fn layout(size: Vec2, frame_size: Vec2) -> (Vec2, Vec2) {
    (Vec2::new((size.x - frame_size.x) * 0.5, 50.0), Vec2::new(size.x * 0.5 - 10.0, 230.0))
}

#[cfg(test)]
mod tests {
    use super::super::members::NoPlx;
    use super::super::{GameUi, ItemStack};
    use super::*;

    fn gear(level: i16, rarity: u8) -> Vec<u8> {
        let mut it = super::super::crafting::default_item();
        it[0] = 4;
        it[0xc] = rarity;
        it[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        it
    }

    #[test]
    fn cost_formula() {
        let mut game = GameView::default();
        game.player.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes());
        game.inventory = vec![vec![ItemStack { count: 1, item: gear(3, 1) }]];
        let w = AdaptionWidget { target: Some(ItemRef::Bag { tab: 0, index: 0 }) };
        // (1 - 1 + 3) * 0.25 = 0.75: f = 2^0.75 * 2 = 3.3636; two steps: 3, 6.
        assert_eq!(cost(&game, &w), 6);
        // At or above the player's level: free.
        game.inventory[0][0].item = gear(9, 1);
        assert_eq!(cost(&game, &w), 0);
        // Already adapted: no target.
        game.inventory[0][0].item[0xe] = 1;
        assert!(target_item(&game, &w).is_none());
    }

    #[test]
    fn adapt_flow() {
        let game0 = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game0);
        let mut game = GameView::default();
        game.player.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes());
        game.equipment[1] = gear(3, 1);
        game.platinum = 10;
        let mut w = AdaptionWidget::default();
        on_creature_menu(&mut ui.gui, &ui.m);
        set_target(&mut w, ItemRef::Equipment(1));
        let size = Vec2::new(350.0, 400.0);
        let f = frame(&game, &w, Vec2::new(10.0, 390.0), size, (Vec2::ZERO, Vec2::splat(120.0)));
        assert!(f.arrow_visible);
        assert_eq!(f.texts[1].color, CYAN);
        assert_eq!(f.texts[3].text, "6 Platinum Coins ");
        assert_eq!(i16_at(&f.descriptions[1].item, 0x10), 5);
        let a = adapt_click(&ui.gui, &ui.m, &w, &mut game, Vec2::new(10.0, 390.0), size);
        assert_eq!(a, vec![snd(0x2e)]);
        assert_eq!(game.platinum, 4);
        assert_eq!(i16_at(&game.equipment[1], 0x10), 5);
        assert_eq!(game.equipment[1][0xe] & 1, 1);
        // Adapted: no target any more.
        assert!(target_item(&game, &w).is_none());
        on_mouse_up(&mut ui.gui, &ui.m, &mut w, Vec2::new(300.0, 390.0), size);
        assert!(!vis(&ui.gui, ui.m.adaption_panel));
        assert!(w.target.is_none());
    }
}
