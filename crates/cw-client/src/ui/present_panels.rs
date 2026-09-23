//! The panels' part of [`super::present`]: the select screens, the three `InventoryWidget`s,
//! the crafting preview, identification, adaption and customization panels, the character
//! style and options widgets.
//!
//! Tier B for what is drawn where (every position, size, stroke and colour is the
//! producers', which cite their slot-1 bodies), Tier C for how: each `FontEngine::drawText`
//! 0x00639b30 call becomes one [`WidgetText`] (`drawText(font, text, spacing, lineSpacing, x,
//! y, size, stroke, fill, strokeColour, extrusionColour, flags, wrapWidth, pixelSnap)`,
//! forwarded as is to `ScalableFont::draw` 0x0065bc70) in the widget node's space.
//!
//! | Widget (slot 1) | Texts from |
//! |---|---|
//! | `CharacterPreviewWidget` 0x00425450, `WorldPreviewWidget` 0x00605ae0 | [`super::previews::PreviewFrame::texts`] |
//! | `InventoryWidget` 0x004c2050 (bag, crafting, shop) | [`super::inventory_widget::InventoryFrame::texts`] |
//! | `BlueprintPreviewWidget` 0x0042f910 | [`super::crafting::BlueprintFrame`] |
//! | `EnchantWidget` 0x0044ea30 | [`super::enchant::EnchantFrame::texts`] |
//! | `AdaptionWidget` 0x0040f8f0 | [`super::adaption::AdaptionFrame::texts`] |
//! | `VoxelWidget` 0x00588500 | [`super::voxel::VoxelFrame::texts`] |
//! | `CharacterStyleWidget` 0x00428e40 | [`super::character_style::StyleRow`]s and "Hair color" |
//! | `OptionsWidget` 0x004d0230 | [`super::options_menu::ROWS`] and the values |
//! | `SkillWidget` 0x004dd810 | [`super::skill_panel::widget_texts`] |
//!
//! Not drawn here: the item models of the inventories and the panels' item icons
//! (`0x004758c0`, [`super::gui_models`]), the previews' creature / world models, the
//! panels' inline models (`BlueprintFrame::model`, `VoxelFrame::model`), the style palette's rectangles
//! (engine slot 6 `drawRect`), and the node colours the widgets write into their parts'
//! Displays ([`node_colors`]).

use std::collections::BTreeMap;

use cw_ui::font::TextStyle;
use cw_ui::render::WidgetText;
use cw_ui::widget::{Gui, NodeId, WidgetId};
use glam::{Affine2, Vec2};

use super::crafting::PanelText;
use super::inventory_widget::{InventoryFrame, InventoryWidget, TextDraw};
use super::members::find_node;
use super::previews::PreviewText;
use super::{FrameOutput, GameUi, GameView};

/// The font of every panel text (`L"resource1.dat"`).
pub const PANEL_FONT: &str = "resource1.dat";

const WHITE: [f32; 4] = [1.0; 4];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Writes the panels' part of this frame into the tree: the inventories' selector and scroll
/// thumb, the craft bar, the adaption arrow. (The select screens' `previews::frame` and the
/// panel rules already wrote theirs.)
pub fn apply(ui: &mut GameUi, game: &GameView, out: &FrameOutput) {
    let _ = game;
    // The InventoryWidgets: 0x004c6610(0) places the selector and 0x004c64c0 the scroll thumb
    // whenever the selection, the scroll or the tab changes. The selector is recomputed every
    // frame here from the same state (the result is the same); the thumb only when its
    // rectangle changed, since `dragScroll` 0x004c5bb0 leaves it off the row grid
    // (`InventoryWidget::thumb_written`, see `inventory_input`).
    for which in 0..3 {
        let (frame, w) = match which {
            0 => (&out.bag, ui.m.inventory),
            1 => (&out.crafting, ui.m.crafting),
            _ => (&out.shop, ui.m.shop),
        };
        let (Some(f), Some(w)) = (frame, w) else { continue };
        if !f.visible {
            continue;
        }
        let size = Vec2::new(ui.gui.width(w), ui.gui.height(w));
        let parts = match which {
            0 => ui.m.bag_parts.clone(),
            1 => ui.m.crafting_parts.clone(),
            _ => ui.m.shop_parts.clone(),
        };
        let wdg: &mut InventoryWidget = match which {
            0 => &mut ui.inv.bag,
            1 => &mut ui.inv.crafting,
            _ => &mut ui.inv.shop,
        };
        let (_, sel) = wdg.select(false, size);
        let thumb_button = parts.scroll.and_then(|n| find_node(&ui.gui, n, "scrollbutton")).and_then(|n| ui.gui.nodes[n].widget);
        let thumb = thumb_button.and_then(|b| wdg.scroll_thumb(size, ui.gui.local_size(b).x).map(|r| (b, r)));
        if let (Some(p), Some(n)) = (sel, parts.selector) {
            // `display.visibility = visible`, then `setPosition(pos)` when shown.
            ui.gui.nodes[n].visible = p.visible;
            if p.visible && let Some(sw) = ui.gui.nodes[n].widget {
                ui.gui.set_position(sw, p.pos, true);
            }
        }
        if let Some((_, r)) = thumb
            && wdg.thumb_written != Some(r)
        {
            // 0x0062bb20(x, y, w, h) on the scroll button: position and size.
            ui.inv_place_thumb([0, 2, 3][which]);
        }
    }
    // BlueprintPreviewWidget: the craft bar shown while crafting (`timer >= 0`), its `bar`
    // x-scaled by 0x00434c20.
    if let Some(bp) = &out.craft_preview
        && let Some(v) = bp.bar_visible
        && let Some(n) = ui.m.craft_bar
    {
        ui.gui.nodes[n].visible = v;
    }
    if let (Some(fill), Some(n)) = (out.craft_bar_fill, ui.m.craft_bar) {
        bar_scale(&mut ui.gui, n, fill);
    }
    // AdaptionWidget: the `rightarrow` part.
    if let (Some(a), Some(n)) = (&out.adaption, ui.m.adaption_rightarrow) {
        ui.gui.nodes[n].visible = a.arrow_visible;
    }
}

/// `0x00434c20(timer)`: the `bar` node under the craft bar gets the identity deformation
/// with its first row scaled by `fill / local width` (`fill` = `(width - 8) * timer`,
/// [`super::crafting::bar_fill`]), where the local width is `0x00627d50` of the bar's widget
/// (the craft bar's own when the bar has none; assumed).
fn bar_scale(gui: &mut Gui, craft_bar: NodeId, fill: f32) {
    let Some(bar) = find_node(gui, craft_bar, "bar").filter(|&b| b != craft_bar) else { return };
    let w = gui.nodes[bar].widget.or(gui.nodes[craft_bar].widget);
    let Some(w) = w else { return };
    let lw = gui.local_size(w).x;
    let mut d = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0f32];
    let s = fill / lw;
    if s != 1.0 {
        for v in &mut d[0..4] {
            *v *= s;
        }
    }
    gui.nodes[bar].deformation = d;
    gui.nodes[bar].deform_dirty = true;
}

/// The panels' slot-1 text calls of this frame, by widget.
pub fn widget_texts(ui: &GameUi, game: &GameView, out: &FrameOutput, texts: &mut BTreeMap<WidgetId, Vec<WidgetText>>) {
    let gui = &ui.gui;
    let mut push = |w: WidgetId, t: Vec<WidgetText>| {
        if !t.is_empty() {
            texts.entry(w).or_default().extend(t);
        }
    };
    // The select screens.
    for pf in out.previews.characters.iter().chain(out.previews.worlds.iter()) {
        if let Some(w) = pf.widget {
            push(w, pf.texts.iter().map(|t| preview_text(gui, w, t)).collect());
        }
    }
    // The InventoryWidgets.
    for (f, w) in [(&out.bag, ui.m.inventory), (&out.crafting, ui.m.crafting), (&out.shop, ui.m.shop)] {
        if let (Some(f), Some(w)) = (f, w) {
            push(w, inventory_texts(gui, w, f));
        }
    }
    // BlueprintPreviewWidget.
    if let (Some(bp), Some(w)) = (&out.craft_preview, ui.m.craft_preview) {
        let mut v = panel_texts(gui, w, &bp.texts);
        if let Some(s) = &bp.station_text {
            v.extend(panel_text(gui, w, s));
        }
        push(w, v);
    }
    // EnchantWidget 0x0044ea30 / AdaptionWidget 0x0040f8f0: the description blocks
    // (0x0044ed87, 0x0040fc37 / 0x0040fcb1) come before the panel texts.
    if let (Some(e), Some(w)) = (&out.enchant, ui.m.enchant) {
        let mut v: Vec<WidgetText> = e.description.iter().flat_map(|d| description_texts(ui, game, w, d)).collect();
        v.extend(panel_texts(gui, w, &e.texts));
        push(w, v);
    }
    if let (Some(a), Some(w)) = (&out.adaption, ui.m.adaption) {
        let mut v: Vec<WidgetText> = a.descriptions.iter().flat_map(|d| description_texts(ui, game, w, d)).collect();
        v.extend(panel_texts(gui, w, &a.texts));
        push(w, v);
    }
    if let (Some(v), Some(w)) = (&out.voxel, ui.m.voxel) {
        push(w, panel_texts(gui, w, &v.texts));
    }
    // SkillWidget 0x004dd810.
    if let Some(w) = ui.m.skills {
        push(w, panel_texts(gui, w, &out.skill_texts));
    }
    // CharacterWidget 0x00434e30, while its panel is shown (with the inventory).
    if let Some(w) = ui.m.character
        && gui.parent_widget(w).is_some_and(|p| gui.nodes[gui.widgets[p].node].visible)
    {
        let t = super::character_sheet::texts(&game.player, &game.sheet);
        push(w, t.iter().map(|t| draw_text(gui, w, &t.text, t.pos, t.size, t.stroke, t.color, t.stroke_color, t.flags, t.wrap, super::character_sheet::LINE_SPACING)).collect());
    }
    // CharacterStyleWidget 0x00428e40.
    if !out.style_rows.is_empty() && let Some(w) = ui.m.char_style {
        let lw = gui.local_size(w).x;
        let mut v = Vec::new();
        for r in &out.style_rows {
            v.extend(two_pass(gui, w, r.label, Vec2::new(15.0, r.y), 12.0, 3.0, WHITE, 0));
            v.extend(two_pass(gui, w, &r.value, Vec2::new((lw - 110.0) * 0.5 + 100.0, r.y), 12.0, 3.0, WHITE, 1));
        }
        // "Hair color" centred at (w / 2, 182) above the palette.
        v.extend(two_pass(gui, w, "Hair color", Vec2::new(lw * 0.5, 182.0), 12.0, 3.0, WHITE, 1));
        push(w, v);
    }
    // OptionsWidget 0x004d0230: label at (15, 27 + 30 i), the value centred at
    // `(w - 250) / 2 + 240`, size 12, stroke 3.
    if let (Some(values), Some(w)) = (&out.option_values, ui.m.options) {
        let lw = gui.local_size(w).x;
        let mut v = Vec::new();
        for (i, (label, _)) in super::options_menu::ROWS.iter().enumerate() {
            let y = 27.0 + 30.0 * i as f32;
            v.extend(two_pass(gui, w, label, Vec2::new(15.0, y), 12.0, 3.0, WHITE, 0));
            v.extend(two_pass(gui, w, &values[i], Vec2::new((lw - 250.0) * 0.5 + 240.0, y), 12.0, 3.0, WHITE, 1));
        }
        push(w, v);
    }
}

/// The colours the widgets write into their parts' Displays this frame (the tab icons and
/// `frame`s of the InventoryWidgets, their up/down/scroll buttons, the craft button's
/// `frame`). The renderer reads the Display fill colour of a node from its scene; a per-node
/// override (not available in `cw_ui::render::GuiView` yet) would carry these.
pub fn node_colors(ui: &GameUi, out: &FrameOutput) -> BTreeMap<NodeId, [f32; 4]> {
    let gui = &ui.gui;
    let mut m: BTreeMap<NodeId, [f32; 4]> = out.node_colors.iter().copied().collect();
    for (f, parts) in [(&out.bag, &ui.m.bag_parts), (&out.crafting, &ui.m.crafting_parts), (&out.shop, &ui.m.shop_parts)] {
        let Some(f) = f else { continue };
        for (n, c, name) in [(parts.up, f.up_color, "upbutton"), (parts.down, f.down_color, "downbutton"), (parts.scroll, f.scroll_color, "scrollbutton")] {
            if let (Some(n), Some(c)) = (n, c) {
                m.insert(find_node(gui, n, name).unwrap_or(n), c);
            }
        }
        for (&t, c) in parts.tabs.iter().zip(f.tabs.iter()) {
            m.insert(t, c.icon);
            if let Some(fr) = find_node(gui, t, "frame") {
                m.insert(fr, c.frame);
            }
        }
    }
    if let (Some(bp), Some(b)) = (&out.craft_preview, ui.m.craft_button)
        && let Some(c) = bp.button_color
        && let Some(fr) = find_node(gui, b, "frame")
    {
        m.insert(fr, c);
    }
    m
}

// ---------------------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------------------

/// A widget-local point in the widget node's space (the renderer draws a widget's texts
/// under its node's world matrix; the widget frame is `bind · translate(framePos)` inside it).
fn to_node(gui: &Gui, w: WidgetId, p: Vec2) -> Vec2 {
    let wd = &gui.widgets[w];
    (wd.bind_matrix * Affine2::from_translation(wd.frame_pos)).transform_point2(p)
}

/// One `drawText` 0x00639b30 call.
#[allow(clippy::too_many_arguments)]
fn draw_text(gui: &Gui, w: WidgetId, text: &str, pos: Vec2, size: f32, stroke: f32, color: [f32; 4], stroke_color: [f32; 4], flags: u32, wrap: f32, line_spacing: f32) -> WidgetText {
    WidgetText {
        font: PANEL_FONT.into(),
        text: text.encode_utf16().collect(),
        origin: to_node(gui, w, pos),
        style: TextStyle {
            size,
            stroke_radius: stroke,
            spacing: 0.0,
            line_spacing,
            // -1 (0xbf800000) is "no wrap width"; only the 0x10 flag wraps.
            wrap_width: if wrap < 0.0 { 0.0 } else { wrap },
            flags,
            pixel_snap: true,
        },
        color,
        stroke_color,
    }
}

/// The usual pair: an outline pass (white fill, black stroke of `stroke`), then the fill pass
/// (`color`, no stroke).
#[allow(clippy::too_many_arguments)]
fn two_pass(gui: &Gui, w: WidgetId, text: &str, pos: Vec2, size: f32, stroke: f32, color: [f32; 4], flags: u32) -> [WidgetText; 2] {
    [
        draw_text(gui, w, text, pos, size, stroke, WHITE, BLACK, flags, -1.0, 0.0),
        draw_text(gui, w, text, pos, size, 0.0, color, [0.0; 4], flags, -1.0, 0.0),
    ]
}

/// An item description block (`0x004a28c0`, [`super::item_description::describe`]) in the
/// widget's local space.
fn description_texts(ui: &GameUi, game: &GameView, w: WidgetId, d: &super::enchant::ItemDescription) -> Vec<WidgetText> {
    let v = super::item_description::describe(ui.text.as_deref(), game, &d.item, &d.args());
    v.iter()
        .filter(|t| !t.text.is_empty())
        .map(|t| draw_text(&ui.gui, w, &t.text, t.pos, t.size, t.stroke, t.color, t.stroke_color, t.flags, t.wrap, t.line_spacing))
        .collect()
}

/// A [`PanelText`] (one entry, both passes).
fn panel_text(gui: &Gui, w: WidgetId, t: &PanelText) -> [WidgetText; 2] {
    two_pass(gui, w, &t.text, Vec2::new(t.x, t.y), t.size, t.outline, t.color, t.align)
}

fn panel_texts(gui: &Gui, w: WidgetId, ts: &[PanelText]) -> Vec<WidgetText> {
    ts.iter().flat_map(|t| panel_text(gui, w, t)).collect()
}

/// A [`PreviewText`] (each already one pass; line spacing 2).
fn preview_text(gui: &Gui, w: WidgetId, t: &PreviewText) -> WidgetText {
    draw_text(gui, w, &t.text, t.pos, t.size, t.stroke_radius, t.color, t.stroke_color, t.flags, t.wrap_width, 2.0)
}

/// An [`InventoryFrame`]'s texts (each already one pass).
fn inventory_texts(gui: &Gui, w: WidgetId, f: &InventoryFrame) -> Vec<WidgetText> {
    f.texts.iter().map(|t: &TextDraw| draw_text(gui, w, &t.text, t.pos, t.size, t.stroke_radius, t.color, t.stroke_color, t.flags, t.wrap_width, 0.0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::members::NoPlx;

    #[test]
    fn options_and_style_texts() {
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let game = GameView::default();
        let mut ui = GameUi::new(gui, &mut NoPlx, &game);
        let out = FrameOutput {
            option_values: Some(ui.options.values()),
            style_rows: ui.char_style.rows(),
            ..FrameOutput::default()
        };
        let mut t = BTreeMap::new();
        widget_texts(&ui, &game, &out, &mut t);
        let o = &t[&ui.m.options.unwrap()];
        // 11 rows, label and value, two passes each.
        assert_eq!(o.len(), 44);
        assert_eq!(String::from_utf16_lossy(&o[0].text), "Mode");
        assert_eq!(o[0].origin, Vec2::new(15.0, 27.0));
        assert_eq!(o[0].style.stroke_radius, 3.0);
        assert_eq!(o[1].style.stroke_radius, 0.0);
        let s = &t[&ui.m.char_style.unwrap()];
        assert_eq!(s.len(), 5 * 4 + 2);
        apply(&mut ui, &game, &out);
    }

    /// The tab icons (0x004a1e50 and the ctor's bag / shop tabs) are textures of `gui.plx`'s
    /// scene set on the tab clones, which are registered with that scene. Skipped without
    /// `CW_GAME_DIR`.
    #[test]
    fn tab_icons_textured() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let ui = GameUi::new(gui, &mut plx, &GameView::default());
        assert!(plx.errors.is_empty(), "{:?}", plx.errors);
        let scene = &plx.loaded[0].scene;
        assert_eq!(ui.m.crafting_parts.tabs.len(), 6);
        assert_eq!(ui.m.bag_parts.tabs.len(), 4);
        for &t in ui.m.crafting_parts.tabs.iter().chain(&ui.m.bag_parts.tabs).chain(&ui.m.shop_parts.tabs) {
            assert!(scene.node_objects.contains_key(&t));
            let sh = ui.gui.nodes[t].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<cw_ui::loader::SharedShape>()).unwrap();
            let cw_ui::loader::SceneShape::Mesh(m, _) = &*sh.borrow() else { panic!("tab shape") };
            let tex = m.source.texture.current;
            assert!(tex >= 0 && (tex as usize) < scene.textures.len());
            assert!(scene.textures[tex as usize].name.ends_with(".png"));
        }
        let first = ui.m.crafting_parts.tabs[0];
        let sh = ui.gui.nodes[first].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<cw_ui::loader::SharedShape>()).unwrap();
        let cw_ui::loader::SceneShape::Mesh(m, _) = &*sh.borrow() else { panic!() };
        assert_eq!(scene.textures[m.source.texture.current as usize].name, "craft-melee-weapons.png");
    }
}
