//! The item preview: which item the `preview` node (`GC+0x8008e4`, with its
//! `cube::PreviewWidget` `GC+0x8008e8`) shows, where the two preview nodes go, when
//! `equippedpreview` (`GC+0x8008ec`) shows, the Tab quick-item selector's texts, and the text
//! calls of `PreviewWidget::update` `Cube.exe 0x004d50a0`.
//!
//! Tier B.
//!
//! # Map
//!
//! | Range | Here |
//! |---|---|
//! | `update` 0x0048d771..0x0048d7a9: the aimed ground item (`GC+0x800a78`, 0x0059fb90) | [`item_source`] |
//! | `update` 0x0048d7ad..0x0048d99a: the Tab list (0x0047ae10) or the hover sources 0x0047b450, 0x0047b1b0, 0x0047b340, 0x0047b550, 0x0047b3e0, 0x0047b010 | [`item_source`] |
//! | `update` 0x0048d99e: `preview` visible with an item | `GameUi::frame` |
//! | `update` 0x0048ec97..0x0048eee2: the placement | [`place_previews`] |
//! | `update` 0x0048eee6..0x0048f14d: `Preview+0x160`, the equipped list `+0x174` | [`equipped_list`] |
//! | `update` 0x0048f14d..0x0048f17e: `equippedpreview` visible = list not empty and `preview` visible | `GameUi::frame` |
//! | `update` 0x00497b0b..0x00497f22: the selector's `count` / `name` | [`selector_texts`] |
//! | `PreviewWidget::update` 0x004d50a0 | [`widget_texts`] |
//!
//! `equippedpreview` has a visibility writer (0x0048f17e): it is hidden whenever nothing is
//! previewed (the list is only built for an item), and shown only while an item of a type
//! the player has equipped is previewed.
//!
//! The item description block `0x004a28c0` is [`super::item_description`]; the item model
//! `0x004758c0` the widget draws at `(x + 150, y + 150)` scale 0.1 is [`PreviewFrame::model`],
//! drawn through [`super::gui_models`].

use cw_ui::font::TextStyle;
use cw_ui::render::WidgetText;
use cw_ui::widget::NodeId;
use glam::Vec2;

use super::inventory::{item_level, price, slot};
use super::members::find_node;
use super::present_hud::{place, set_named_text};
use super::{GameUi, GameView};

/// Where the previewed item comes from (the original keeps only the item pointer; the
/// equipped-list exclusions compare it with the equipment slots' addresses).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewSource {
    /// Nothing.
    #[default]
    None,
    /// The aimed ground item (`GC+0x800a78`, 0x0059fb90).
    Ground,
    /// The Tab quick-item selection (0x0047ae10 at `GC+0x800a44`): bag `(tab, index)`.
    QuickItem(usize, usize),
    /// The hovered bag cell (0x0047b450).
    Bag(usize, usize),
    /// The hovered equipment slot (0x0047b1b0, [`slot`] index).
    Equipment(usize),
    /// The hovered recipe cell (0x0047b340).
    Recipe(usize, usize),
    /// The hovered shop cell (0x0047b550).
    Shop(usize, usize),
    /// The craft preview's tooltip item (0x0047b3e0, `BlueprintPreviewWidget+0x2ac`).
    Blueprint,
    /// The customization list's hovered spirit (0x0047b3e0, `VoxelWidget+0x2b0`).
    Voxel,
    /// The identification panel's item under the cursor (0x0047b010).
    Enchant(usize, usize),
}

/// `cube::PreviewWidget` fields (`GC+0x8008e8`).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewWidget {
    /// `+0x160`: the item shown (`None` = null).
    pub item: Option<Vec<u8>>,
    /// Where `+0x160` points.
    pub source: PreviewSource,
    /// `+0x168`: the hovered skill button (-1 none).
    pub skill: i32,
    /// `+0x16c`: its level.
    pub level: i32,
    /// `+0x170`: the hovered specialization button (-1 none).
    pub spec: i32,
    /// `+0x174`: the equipped items of the previewed item's type (`std::list<Item*>`), as
    /// equipment slot indices.
    pub equipped: Vec<usize>,
}

impl Default for PreviewWidget {
    fn default() -> Self {
        PreviewWidget { item: None, source: PreviewSource::None, skill: -1, level: 0, spec: -1, equipped: Vec::new() }
    }
}

/// `0x0047ae10`: the quick items, `(tab, index)` of every stack with a count, a consumable
/// (type 1) and an item level (`Item+0x10`, i16) not above the player's (`creature+0x190`).
pub fn quick_items(game: &GameView) -> Vec<(usize, usize)> {
    let level = i32::from_le_bytes(game.player.0[0x180..0x184].try_into().unwrap());
    let mut out = Vec::new();
    for (t, tab) in game.inventory.iter().enumerate() {
        for (i, s) in tab.iter().enumerate() {
            let lv = i32::from(i16::from_le_bytes([s.item[0x10], s.item[0x11]]));
            if s.count != 0 && s.item[0] == 1 && lv <= level {
                out.push((t, i));
            }
        }
    }
    out
}

/// The selection index `GC+0x800a44` wrapped into a list of `n` (0x0048d7e6..0x0048d810: a
/// negative index is first raised by whole lengths, unsigned arithmetic).
pub fn wrap_index(i: i32, n: usize) -> usize {
    let n = n as u32;
    let mut v = i as u32;
    if i < 0 {
        let q = (i.wrapping_neg() as u32) / n;
        v = ((q + 1).wrapping_mul(n).wrapping_add(i as u32)) % n;
    }
    (v % n) as usize
}

/// `update` 0x0048d771..0x0048d99a. Starts from the aimed ground item. With the Tab menu open
/// (`GC+0x800a40`) and quick items present, the selected quick item (no hover flag).
/// Otherwise the first hover source that answers, each setting the hover flag
/// (`[esp+0x17]`): the bag cell under the cursor with a count (0x0047b450, inventory panel
/// open); the equipment slot under the cursor holding an item (0x0047b1b0, `GC+0x8008f0`,
/// test order [`slot::HOVER_ORDER`]); the recipe cell (0x0047b340); the shop cell (0x0047b550,
/// no count test); the craft preview's tooltip item, or with that panel closed the
/// customization list's spirit (0x0047b3e0); the identification item within 120 px of the
/// panel's origin (0x0047b010). Returns the item and the hover flag.
pub fn item_source(ui: &GameUi, game: &GameView, ground: Option<&[u8]>) -> (PreviewSource, Option<Vec<u8>>, bool) {
    let m = &ui.m;
    let mut src = PreviewSource::None;
    let mut item: Option<Vec<u8>> = None;
    if let Some(g) = ground {
        src = PreviewSource::Ground;
        item = Some(g.to_vec());
    }
    if ui.flag_800a40 {
        let list = quick_items(game);
        if !list.is_empty() {
            let (t, i) = list[wrap_index(game.quick_index, list.len())];
            return (PreviewSource::QuickItem(t, i), Some(game.inventory[t][i].item.clone()), false);
        }
        return (src, item, false);
    }
    // 0x0047b450.
    if ui.visible(m.inventory_panel) {
        let (t, i) = (ui.inv.bag.hovered_tab, ui.inv.bag.hovered_index);
        if t >= 0 && i >= 0 {
            if let Some(s) = game.inventory.get(t as usize).and_then(|tab| tab.get(i as usize)) {
                if s.count != 0 {
                    return (PreviewSource::Bag(t as usize, i as usize), Some(s.item.clone()), true);
                }
            }
        }
    }
    // 0x0047b1b0: box `i` (`GC+0x800a28[i]`) is slot `HOVER_ORDER[i]`.
    if ui.flag_8008f0 {
        let k = slot::HOVER_ORDER.iter().enumerate().find(|&(i, _)| m.equipment_sprites.get(i).is_some_and(|&w| ui.is_hovered(w))).map(|(_, &k)| k);
        if let Some(k) = k {
            if let Some(e) = game.equipment.get(k) {
                if e[0] != 0 {
                    return (PreviewSource::Equipment(k), Some(e.clone()), true);
                }
            }
        }
    }
    // 0x0047b340.
    let hovered_recipe = (ui.inv.crafting.hovered_tab, ui.inv.crafting.hovered_index);
    if let Some(it) = super::crafting::recipe_tooltip(&ui.crafting, ui.visible(m.crafting_panel), hovered_recipe) {
        let (t, i) = (hovered_recipe.0 as usize, hovered_recipe.1 as usize);
        return (PreviewSource::Recipe(t, i), Some(it.to_vec()), true);
    }
    // 0x0047b550.
    if ui.visible(m.shop_panel) {
        let (t, i) = (ui.inv.shop.hovered_tab, ui.inv.shop.hovered_index);
        if t >= 0 && i >= 0 {
            if let Some(s) = ui.inv.shop_data.pages.get(t as usize).and_then(|tab| tab.get(i as usize)) {
                return (PreviewSource::Shop(t as usize, i as usize), Some(s.item.clone()), true);
            }
        }
    }
    // 0x0047b3e0.
    if ui.visible(m.craft_preview_panel) {
        if let Some(it) = ui.crafting.preview.tooltip_item(true) {
            return (PreviewSource::Blueprint, Some(it.to_vec()), true);
        }
    } else if ui.visible(m.voxel_panel) && ui.voxel.hovered >= 0 {
        return (PreviewSource::Voxel, Some(ui.voxel.hovered_item.clone()), true);
    }
    // 0x0047b010: the target stack (count not 0) while the cursor lies in the 120x120 square
    // at the panel's origin: the panel node's pivot (`Transformation+0x19c`) pushed through
    // the node's world matrix, which lands on the panel's widget origin in the shipped
    // gui.plx (the translation would be 250 px off).
    if ui.visible(m.enchant_panel) {
        let (t, i) = (ui.enchant.tab, ui.enchant.index);
        if t >= 0 && i >= 0 {
            if let Some(s) = game.inventory.get(t as usize).and_then(|tab| tab.get(i as usize)) {
                if s.count != 0 {
                    let o = m.enchant_panel.map_or(Vec2::ZERO, |n| {
                        ui.gui.node_world(n).transform_point2(ui.gui.nodes[n].pivot)
                    });
                    let c = ui.gui.cursor;
                    if o.x <= c.x && o.y <= c.y && c.x < o.x + 120.0 && c.y < o.y + 120.0 {
                        return (PreviewSource::Enchant(t as usize, i as usize), Some(s.item.clone()), true);
                    }
                }
            }
        }
    }
    (src, item, false)
}

/// `GameController::GameController` 0x00465a0a..0x00465a2f, its last change to the node
/// tree (after the chat widget, before `Engine` 0x006526b0): `n->setParent(n->getParent())`
/// (0x00434a80 reads `Node+0x28`; `setParent` 0x00636950 detaches and calls `addChild`
/// 0x00630be0, a `std::list` push_back on `Node+0x2c`) for `equippedpreview`, then
/// `preview`. Both move to the end of their parent's child list, after every panel the ctor
/// cloned into the GUI root, so `Node::render` 0x00632910 (forward list walk) draws them
/// on top, `preview` last.
pub fn raise_previews(gui: &mut cw_ui::widget::Gui, equipped_preview: Option<NodeId>, preview: Option<NodeId>) {
    for n in [equipped_preview, preview].into_iter().flatten() {
        if let Some(p) = gui.nodes[n].parent {
            gui.add_child(p, n);
        }
    }
}

/// `update` 0x0048ec97..0x0048eee2: with the hover flag, `preview` at the cursor + (20, 20)
/// and `equippedpreview` at the cursor + (−235, 20) (0x00480dd0 the engine cursor, 0x00468f20
/// add); otherwise `preview` at `(W − 320, H − 320)` and `equippedpreview` at
/// `(W − 320, H − 575)` (0x004279e0 / 0x004279f0, the engine's width and height). Each minus
/// its pivot (0x00468df0), then the Transformation's slot 1 (`evaluate`).
pub fn place_previews(ui: &mut GameUi, hovered: bool) {
    let (w, h) = (ui.gui.viewport.x, ui.gui.viewport.y);
    let c = ui.gui.cursor;
    let (p, e) = if hovered {
        ([c.x + 20.0, c.y + 20.0], [c.x + -235.0, c.y + 20.0])
    } else {
        ([(w - 0x140) as f32, (h - 0x140) as f32], [(w - 0x140) as f32, (h - 0x23f) as f32])
    };
    if let Some(n) = ui.m.preview {
        place(&mut ui.gui, n, p);
    }
    if let Some(n) = ui.m.equipped_preview {
        place(&mut ui.gui, n, e);
    }
}

/// `update` 0x0048ef0f..0x0048f14d: for a previewed item, the equipped items whose type byte
/// equals its type, in the order right weapon, left weapon (both skipped when the item is
/// itself a weapon slot), pet, shoulder, chest, hands, feet, neck, right ring, left ring
/// (both rings skipped when the item is a ring slot); a slot is also skipped when it is the
/// item itself. The lamp and the special slot are not compared.
pub fn equipped_list(game: &GameView, source: PreviewSource, item: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let is = |k: usize| source == PreviewSource::Equipment(k);
    let ty = |k: usize| game.equipment.get(k).map_or(0, |e| e[0]);
    let weapon = is(slot::RIGHT) || is(slot::LEFT);
    let ring = is(slot::RIGHT_RING) || is(slot::LEFT_RING);
    for k in [slot::RIGHT, slot::LEFT] {
        if !weapon && ty(k) == item[0] {
            out.push(k);
        }
    }
    for k in [slot::PET, slot::SHOULDER, slot::CHEST, slot::HANDS, slot::FEET, slot::NECK] {
        if !is(k) && ty(k) == item[0] {
            out.push(k);
        }
    }
    for k in [slot::RIGHT_RING, slot::LEFT_RING] {
        if !ring && ty(k) == item[0] {
            out.push(k);
        }
    }
    out
}

/// The Tab selector's texts (`update` 0x00497b0b..0x00497f22, while the quick-item menu is
/// open): with quick items, `count` = the selected stack's count and `name` = the item name
/// (`World::itemName` 0x00598a50) `<< " +" <<` its level (0x004c76a0), each through
/// `Node::setChildText(name, text, 1)` 0x00636a00; without, `count` = "" (the name keeps its
/// last text).
pub fn selector_texts(ui: &mut GameUi, game: &GameView) {
    let Some(sel) = ui.m.selector else { return };
    if !ui.flag_800a40 {
        return;
    }
    let list = quick_items(game);
    if list.is_empty() {
        set_named_text(&mut ui.gui, sel, "count", "");
        return;
    }
    let (t, i) = list[wrap_index(game.quick_index, list.len())];
    let s = &game.inventory[t][i];
    set_named_text(&mut ui.gui, sel, "count", &s.count.to_string());
    let name = format!("{} +{}", crate::names::item_name(ui.text.as_deref(), &s.item), item_level(&s.item));
    set_named_text(&mut ui.gui, sel, "name", &name);
}

/// What `PreviewWidget::update` 0x004d50a0 draws besides its texts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewFrame {
    /// The item model `0x004758c0(x + 150, y + 150, GC+0x800a1c, 0.1, item, -0.05)` with the
    /// depth test on (render states 7 = 1, 0x17 = 2), when the item has a model
    /// (0x004ec400); `(x, y)` is the `preview` node's pivot (`Transformation+0x19c[+0x170]`)
    /// through its world matrix (`node+0x48`, divided by w), i.e. in screen space. Not drawn
    /// by the port's renderer yet.
    pub model: Option<(Vec2, Vec<u8>)>,
}

fn text(t: &str, origin: Vec2, size: f32, stroke: f32, flags: u32, color: [f32; 4], stroke_color: [f32; 4]) -> WidgetText {
    WidgetText {
        font: super::present_hud::FONT.into(),
        text: t.encode_utf16().collect(),
        origin,
        style: TextStyle { size, stroke_radius: stroke, spacing: 0.0, line_spacing: 0.0, wrap_width: 0.0, flags, pixel_snap: true },
        color,
        stroke_color,
    }
}

/// `PreviewWidget::update` 0x004d50a0 (the `preview` node's texts, in the node's space).
/// `(x, y)` below is the node's Transformation **pivot** (`+0x38 → +0x19c[+0x170]`, read
/// at 0x004d5d33, and at 0x004d52e8 / 0x004d5392 before the tooltips), not its translation
/// (`+0x94[+0x68]`); the widget's slot 1 runs inside `Node::render` 0x00632910 (0x006334a7)
/// under the node's world transform, so these are node-space coordinates. gui.plx's `preview` has pivot
/// (49.2, -115.8) and its box spans (45.2, -119.8)..(353.2, 188.2), so `pivot + (14, 25)`
/// is the box's inner top-left.
///
/// - The four `star1..4` children hidden, then with an item the one of its rarity
///   (`Item+0xc`, 1..4) shown.
/// - Without an item: the specialization tooltip `0x004a62c0(class, +0x170, x + 14, y + 25,
///   1, 275)` when `+0x170 >= 0`, then the skill tooltip `0x004a5710(+0x168, +0x16c, ...)`
///   when `+0x168 >= 0` ([`super::tooltip::layout_tooltip`]: each word an outline pass,
///   stroke 3, white on black, then the fill pass in its colour).
/// - With an item: the description block `0x004a28c0(item, (int)(x + 14), (int)(y + 25), 1,
///   280, 1, 1, _)` (0x004d5d8c, [`super::item_description::describe`]); with the shop open
///   over a bag stack (0x0047f940) the price right-aligned from `((int)(x + 290), (int)(y +
///   280))`, copper, silver, gold, each shown when not 0 and 40 px left of the previous (size
///   10, stroke 2, (0.8, 0.5, 0) / (0.7, 0.7, 0.7) / (1, 0.9, 0)); with equipped items of the
///   same type, "Currently equipped" at `(x - 190, y + 20)` size 8 (stroke 3, white) and
///   their blocks `0x004a28c0(equipped, (int)(x - 240), (int)(y + 10 + 30 + 130 k), 1, 230,
///   1, 1, _)` (0x004d6b3f..0x004d6bd5, the `+130` accumulated in float).
pub fn widget_texts(ui: &GameUi, game: &GameView) -> Vec<WidgetText> {
    let mut out = Vec::new();
    let Some(n) = ui.m.preview else { return out };
    if !ui.gui.nodes[n].visible {
        return out;
    }
    // 0x004d5d33 (0x004d52e8, 0x004d5392 for the tooltips): `Transformation+0x19c[+0x170]`,
    // the pivot. The positions are already node space; nothing is subtracted below.
    let tr = ui.gui.nodes[n].pivot;
    let white = [1.0; 4];
    let black = [0.0, 0.0, 0.0, 1.0];
    let p = ui.preview.clone();
    match &p.item {
        None => {
            let db = ui.text.as_ref();
            let class = game.player.0[0x130];
            let mut lines = Vec::new();
            if p.spec >= 0 {
                if let Some(db) = db {
                    lines.push(super::tooltip::specialization_tooltip_lines(db, class, p.spec));
                }
            }
            if p.skill >= 0 {
                if let Some(db) = db {
                    lines.push(super::tooltip::skill_tooltip_lines(db, &game.player, p.skill, p.level, class, ui.skills.spec));
                }
            }
            let fonts = &ui.fonts;
            for l in lines {
                let x0 = super::inventory::cvtt(tr.x + 14.0f32);
                let y0 = super::inventory::cvtt(tr.y + 25.0f32);
                let words = super::tooltip::layout_tooltip(&l, x0, y0, super::tooltip::TOOLTIP_WIDTH, &mut |t, s| fonts.measure(t, s, 0.0));
                for w in words {
                    let o = Vec2::new(w.x, w.y);
                    out.push(text(&w.text, o, w.size, super::tooltip::TOOLTIP_OUTLINE, 0, white, black));
                    out.push(text(&w.text, o, w.size, 0.0, 0, w.color, [0.0; 4]));
                }
            }
        }
        Some(item) => {
            // 0x004d5d3b..0x004d5d8c: the description block.
            let db = ui.text.as_deref();
            let args = super::item_description::DescriptionArgs {
                x: super::inventory::cvtt(tr.x + 14.0f32),
                y: super::inventory::cvtt(tr.y + 25.0f32),
                scale: 1.0,
                width: 0x118,
                name: true,
                class_line: true,
            };
            let d = super::item_description::describe(db, game, item, &args);
            out.extend(super::item_description::widget_texts(&d, Vec2::ZERO));
            // 0x0047f940: the shop panel open and the hovered bag cell holding a stack.
            let selling = ui.visible(ui.m.shop_panel)
                && matches!(p.source, PreviewSource::Bag(..))
                && ui.inv.bag.hovered_tab >= 0
                && ui.inv.bag.hovered_index >= 0;
            if selling {
                let v = price(item);
                let (gold, silver, copper) = ((v / 100) / 100, (v / 100) % 100, v % 100);
                let mut x = super::inventory::cvtt(tr.x + 290.0f32);
                let y = super::inventory::cvtt(tr.y + 280.0f32);
                for (amount, suffix, color) in [
                    (copper, " C ", [0.8, 0.5, 0.0, 1.0]),
                    (silver, " S ", [0.7, 0.7, 0.7, 1.0]),
                    (gold, " G ", [1.0, 0.9, 0.0, 1.0]),
                ] {
                    if amount != 0 {
                        let o = Vec2::new(x as f32, y as f32);
                        let t = format!("{amount}{suffix}");
                        out.push(text(&t, o, 10.0, 2.0, 2, white, black));
                        out.push(text(&t, o, 10.0, 0.0, 2, color, [0.0; 4]));
                        x -= 0x28;
                    }
                }
            }
            if !p.equipped.is_empty() {
                // 0x004d6a00 (and 0x004d6b1f): `(x - 190, y + 20)` in float (no truncation).
                let o = Vec2::new(tr.x - 190.0f32, tr.y + 20.0f32);
                out.push(text("Currently equipped", o, 8.0, 3.0, 0, white, black));
                out.push(text("Currently equipped", o, 8.0, 0.0, 0, white, [0.0; 4]));
                // 0x004d6b3f..0x004d6bd5: one block per equipped item.
                let x = super::inventory::cvtt(tr.x - 240.0f32);
                let mut yb = tr.y + 10.0f32;
                for &k in &p.equipped {
                    let Some(e) = game.equipment.get(k) else { continue };
                    let args = super::item_description::DescriptionArgs {
                        x,
                        y: super::inventory::cvtt(yb + 30.0f32),
                        scale: 1.0,
                        width: 0xe6,
                        name: true,
                        class_line: true,
                    };
                    let d = super::item_description::describe(db, game, e, &args);
                    out.extend(super::item_description::widget_texts(&d, Vec2::ZERO));
                    yb = yb + 130.0f32;
                }
            }
        }
    }
    out
}

/// The non-text part of `PreviewWidget::update` 0x004d50a0 (the widget's pass, after
/// `update` placed the node): the four `star1..4` children hidden, then with an item the
/// one of its rarity (`Item+0xc`, 1..4) shown; the item model (0x004ec400, 0x004758c0).
pub fn widget_frame(ui: &mut GameUi) -> PreviewFrame {
    let mut f = PreviewFrame::default();
    let Some(n) = ui.m.preview else { return f };
    let stars: Vec<Option<NodeId>> = ["star1", "star2", "star3", "star4"].iter().map(|s| find_node(&ui.gui, n, s)).collect();
    for s in stars.iter().flatten() {
        ui.gui.nodes[*s].visible = false;
    }
    let Some(item) = ui.preview.item.clone() else { return f };
    let r = item[0xc];
    if (1..=4).contains(&r) {
        if let Some(s) = stars[(r - 1) as usize] {
            ui.gui.nodes[s].visible = true;
        }
    }
    if crate::interact::item_model_index(&item).is_some() {
        // 0x004d5c4d..0x004d5d02: the pivot through the node's world matrix, + 150.
        let w = ui.gui.node_world(n).transform_point2(ui.gui.nodes[n].pivot);
        f.model = Some((w + Vec2::new(150.0, 150.0), item));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ItemStack;

    fn stack(ty: u8, count: i32, level: i16) -> ItemStack {
        let mut s = ItemStack::empty();
        s.count = count;
        s.item[0] = ty;
        s.item[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        s
    }

    #[test]
    fn quick_items_and_wrap() {
        let mut g = GameView::default();
        g.player.0[0x180..0x184].copy_from_slice(&3i32.to_le_bytes());
        g.inventory = vec![vec![stack(1, 2, 1), stack(3, 1, 1)], vec![stack(1, 0, 1), stack(1, 5, 9), stack(1, 1, 3)]];
        assert_eq!(quick_items(&g), vec![(0, 0), (1, 2)]);
        assert_eq!(wrap_index(-1, 2), 1);
        assert_eq!(wrap_index(5, 2), 1);
    }

    #[test]
    fn selector_count_and_name() {
        use cw_ui::widget::NodeSource;
        let mut ui = GameUi::default();
        let sel = ui.gui.add_plain_node(None, "selector");
        let count = ui.gui.add_node(Some(sel), NodeSource { name: "count".into(), visible: true, text: Some("1".into()), ..Default::default() });
        let name = ui.gui.add_node(Some(sel), NodeSource { name: "name".into(), visible: true, text: Some("x".into()), ..Default::default() });
        ui.m.selector = Some(sel);
        ui.flag_800a40 = true;
        let mut g = GameView::default();
        g.player.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes());
        g.inventory = vec![vec![stack(1, 3, 2)]];
        selector_texts(&mut ui, &g);
        assert_eq!(ui.gui.nodes[count].text, Some("3".encode_utf16().collect()));
        let n = String::from_utf16_lossy(ui.gui.nodes[name].text.as_ref().unwrap());
        assert!(n.ends_with(" +2") || n.contains(" +"), "{n}");
        // No quick item: the count is cleared, the name kept.
        g.inventory = vec![vec![stack(3, 1, 1)]];
        selector_texts(&mut ui, &g);
        assert_eq!(ui.gui.nodes[count].text, Some(Vec::new()));
        assert_eq!(String::from_utf16_lossy(ui.gui.nodes[name].text.as_ref().unwrap()), n);
    }

    /// gui.plx's `preview` node: pivot (49.175, -115.824), box shape (45.2, -119.8) ..
    /// (353.2, 188.2) in node space. 0x004d50a0 reads `(x, y)` from the Transformation's
    /// pivot (`+0x19c[+0x170]`), not its translation (`+0x94[+0x68]`), and draws under the
    /// node's world transform, so the description starts at `pivot + (14, 25)` in node space,
    /// inside the box.
    #[test]
    fn preview_texts_start_at_pivot_offset() {
        use cw_ui::widget::NodeSource;
        let mut ui = GameUi::default();
        let n = ui.gui.add_node(
            None,
            NodeSource { name: "preview".into(), visible: true, translation: Vec2::new(550.8, 515.8), pivot: Vec2::new(49.175, -115.824), ..Default::default() },
        );
        ui.m.preview = Some(n);
        let mut item = vec![0u8; 0x118];
        item[0] = 3;
        ui.preview.item = Some(item);
        let g = GameView::default();
        let t = widget_texts(&ui, &g);
        assert!(!t.is_empty());
        // (int)(49.175 + 14) = 63, (int)(-115.824 + 25) = -90 (truncation toward zero).
        assert_eq!(t[0].origin, Vec2::new(63.0, -90.0));
        for w in &t {
            assert!(w.origin.x >= 45.0 && w.origin.x <= 353.0, "{:?}", w.origin);
            assert!(w.origin.y >= -120.0 && w.origin.y <= 188.0, "{:?}", w.origin);
        }
        // The model at the pivot through the node's world matrix, + 150.
        let f = widget_frame(&mut ui);
        let m = f.model.map(|(p, _)| p);
        if let Some(p) = m {
            assert!((p - Vec2::new(550.8 + 49.175 + 150.0, 515.8 - 115.824 + 150.0)).length() < 1e-3, "{p:?}");
        }
    }

    /// The GameController ctor's last tree change (0x00465a0a..0x00465a2f) re-attaches
    /// `equippedpreview`, then `preview`, to their own parent, which appends each one to the
    /// end of the child list. `Node::render` 0x00632910 walks the list forward, so both draw
    /// after the inventory panel and every other panel (built earlier in the ctor), with
    /// `preview` on top.
    #[test]
    fn previews_draw_after_the_panels() {
        struct GuiPlx;
        impl crate::ui::members::PlxLoader for GuiPlx {
            fn load(&mut self, gui: &mut cw_ui::widget::Gui, file: &str, parent: cw_ui::widget::NodeId) {
                if file == "gui.plx" {
                    // gui.plx order: the previews come before the ctor's panels.
                    gui.add_plain_node(Some(parent), "preview");
                    gui.add_plain_node(Some(parent), "equippedpreview");
                    gui.add_plain_node(Some(parent), "blackwidget");
                }
            }
        }
        let mut gui = cw_ui::widget::Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let ui = GameUi::new(gui, &mut GuiPlx, &GameView::default());
        let (p, e, inv) = (ui.m.preview.unwrap(), ui.m.equipped_preview.unwrap(), ui.m.inventory_panel.unwrap());
        let parent = ui.gui.nodes[p].parent.unwrap();
        let kids = &ui.gui.nodes[parent].children;
        assert_eq!(&kids[kids.len() - 2..], &[e, p]);
        // Pre-order (draw order) over the whole tree: the inventory panel, then the previews.
        let mut order = Vec::new();
        let mut stack = vec![ui.gui.root.unwrap()];
        while let Some(n) = stack.pop() {
            order.push(n);
            stack.extend(ui.gui.nodes[n].children.iter().rev().copied());
        }
        let at = |n| order.iter().position(|&x| x == n).unwrap();
        assert!(at(inv) < at(e) && at(e) < at(p), "inventory {} equipped {} preview {}", at(inv), at(e), at(p));
    }

    #[test]
    fn equipped_comparisons() {
        let mut g = GameView::default();
        g.equipment[slot::RIGHT][0] = 3;
        g.equipment[slot::LEFT][0] = 3;
        g.equipment[slot::CHEST][0] = 4;
        let mut sword = vec![0u8; 0x118];
        sword[0] = 3;
        assert_eq!(equipped_list(&g, PreviewSource::Bag(0, 0), &sword), vec![slot::RIGHT, slot::LEFT]);
        assert!(equipped_list(&g, PreviewSource::Equipment(slot::LEFT), &sword).is_empty());
        let mut chest = vec![0u8; 0x118];
        chest[0] = 4;
        assert!(equipped_list(&g, PreviewSource::Equipment(slot::CHEST), &chest).is_empty());
        assert_eq!(equipped_list(&g, PreviewSource::Shop(0, 0), &chest), vec![slot::CHEST]);
    }
}
