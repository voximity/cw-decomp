//! The item models the game widgets draw inside the GUI pass: every call of `GameController
//! 0x004758c0(x, y, float3* rotation, scale, Item*, depth)` from a widget's slot-1 body, as
//! [`GuiModel`]s for `cw_render::passes` (Tier B for which item, where, how big, in which
//! colour; Tier C for the drawing).
//!
//! | Caller | Models | Here |
//! |---|---|---|
//! | `SpriteWidget::update` 0x0051c3d0 (the 12 equipment boxes `GC+0x800a28`, their `+0x164` set by `update` 0x0048c7fc..0x0048cbb4) | the equipped item at the box centre, scale `width · 0.0006`, white | [`equipment_boxes`] |
//! | `SpriteWidget::update` 0x0051c3d0 (the quick-item button's background `GC+0x800a18`, `+0x164` set by 0x004921ee) | the selected quick item | [`quick_item`] |
//! | `InventoryWidget::update` 0x004c2050 (bag, crafting, shop) | the cells (0x004c2577) and the cursor stack (0x004c595a) | [`InventoryFrame::models`](super::inventory_widget::InventoryFrame), `held` |
//! | `BlueprintPreviewWidget::update` 0x0042f910 | the ingredient icons (0x00430bd6), white | `BlueprintFrame::ingredients` |
//! | `EnchantWidget::update` 0x0044ea30, `AdaptionWidget::update` 0x0040f8f0 | the target's icon, white | `EnchantFrame::icon`, `AdaptionFrame::icon` |
//! | `VoxelWidget::update` 0x00588500 | the upgrade list (0x0058b736), white | `VoxelFrame::list` |
//! | `PreviewWidget::update` 0x004d50a0 | the previewed item at `(x + 150, y + 150)`, scale 0.1, depth -0.05, white | [`super::item_preview::PreviewFrame::model`] |
//!
//! Every caller passes `GC+0x800a1c` as the rotation, the value it has while the GUI draws
//! (the HUD spin 0x004bbabc runs after the GUI pass). The model of `0x004758c0` is
//! `modelForItem 0x004ec400(item)`, or for an item of type 0 the model vector's entry `0x95c`
//! (`GC+0x304 + 0x2570`); its shininess `0x004c7be0`; after it, the spirit cubes of
//! `0x00471b60(item, world, ..., (1, 1, 1, 1), 0)`.
//!
//! Order: the original draws each widget's models at its point of the GUI traversal, inside
//! the widget's slot 1 (`Node::render` 0x00632910: the node's shape, slot 1, then the
//! children), with the GUI's depth test off before and after (render state 7). So whatever
//! the GUI draws later covers the model: the widget's own texts drawn after it (the
//! inventory's stack counts), later siblings (the quick-item button's `count` text after its
//! `background` sprite), later panels. Each [`GuiModel`] carries its widget as its
//! [`GuiModel::anchor`] (before or after the widget's texts) and `cw_render::passes` draws it
//! at that [`cw_render::frame::GuiCommand::WidgetMark`] of the GUI stream.

use cw_render::frame::{GuiAnchor, ModelRef};
use cw_render::passes::GuiModel;
use cw_ui::widget::{Gui, NodeId, WidgetId};
use glam::Vec2;

use super::inventory::slot;
use super::{FrameOutput, GameUi, GameView};

/// `ModelCache` lookups the models need.
pub trait GuiModelSource {
    /// The size in voxels of model `index` of the model vector (`GC+0x300`), `None` when
    /// the vector has no such entry.
    fn size(&self, index: u32) -> Option<[i32; 3]>;
    /// The unit cube of the spirit cubes (`0x00471b60`).
    fn spirit_cube(&self) -> ModelRef;
}

/// What the widgets' `0x004758c0` calls read besides the frame output.
pub struct GuiModelInputs<'a> {
    /// `GC+0x800a1c` before this frame's spin.
    pub rotation: [f32; 3],
    /// The model cache.
    pub models: &'a dyn GuiModelSource,
}

/// `0x004c7be0(item)`: 1 for the metal materials (1, 0xb, 0xc, 0x16), else 0.
pub fn item_shininess(item: &[u8]) -> f32 {
    if matches!(item[0xd], 1 | 0xb | 0xc | 0x16) { 1.0 } else { 0.0 }
}

/// `0x004758c0`'s model choice: type 0 → entry `0x95c` (`GC+0x304 + 0x2570`) when the vector
/// holds it, else `modelForItem` 0x004ec400.
pub fn gui_item_model(item: &[u8], models: &dyn GuiModelSource) -> Option<(u32, [i32; 3])> {
    let idx = if item[0] == 0 { 0x95c } else { crate::interact::item_model_index(item)? };
    Some((idx, models.size(idx)?))
}

/// One `0x004758c0` call; `None` when the item has no model.
pub fn item_model(origin: u32, center: Vec2, scale: f32, depth: f32, item: &[u8], material: [f32; 4], inp: &GuiModelInputs) -> Option<GuiModel> {
    let (model, model_size) = gui_item_model(item, inp.models)?;
    // 0x00471b60: the spirit cubes (count `item+0x114`, cube `i` at `item+0x14 + 8i`: signed
    // x, y, z, material), coloured `0x004c7250(material, (1, 1, 1, 1), 0)`.
    let mut spirits = Vec::new();
    let count = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap());
    for i in 0..count.max(0) as usize {
        let o = 0x14 + i * 8;
        let Some(c) = item.get(o..o + 4) else { break };
        let off = [c[0] as i8 as f32, c[1] as i8 as f32, c[2] as i8 as f32];
        spirits.push((off, crate::scene::material_color(c[3], [1.0; 4], 0.0)));
    }
    Some(GuiModel {
        origin,
        screen: center.to_array(),
        rotation: inp.rotation,
        scale,
        ring_turn: item[0] == 9,
        model,
        model_size,
        depth,
        material,
        shininess: item_shininess(item),
        spirits,
        spirit_model: inp.models.spirit_cube(),
        anchor: None,
    })
}

/// The model drawn in widget `w`'s slot 1, before its texts or after them
/// ([`GuiModel::anchor`]).
fn anchored(g: Option<GuiModel>, w: Option<WidgetId>, after_texts: bool) -> Option<GuiModel> {
    g.map(|g| GuiModel { anchor: w.map(|w| GuiAnchor { widget: w as u32, after_texts }), ..g })
}

/// A node and all its ancestors are visible (the GUI traversal reaches its widgets).
pub fn shown(gui: &Gui, n: NodeId) -> bool {
    let mut cur = Some(n);
    while let Some(c) = cur {
        if !gui.nodes[c].visible {
            return false;
        }
        cur = gui.nodes[c].parent;
    }
    true
}

/// `SpriteWidget::update` 0x0051c3d0 with an item (`+0x164` not null, type not 0): the centre
/// is the node's Transformation pivot (`+0x38 → +0x19c[+0x170]`) through the node's world
/// matrix (`node+0x48`, divided by w; screen space) plus half the widget's local size
/// (0x00627d50 / 0x00627ce0), the scale `width · 0.0006`, the material white (0x00448280),
/// depth 0.
fn sprite(gui: &Gui, w: WidgetId, item: &[u8], inp: &GuiModelInputs) -> Option<GuiModel> {
    let node = gui.widgets[w].node;
    if !shown(gui, node) || item[0] == 0 {
        return None;
    }
    let size = gui.local_size(w);
    // 0x0051c57d..0x0051c635: `(w · 0.5 + world(pivot).x, h · 0.5 + world(pivot).y)`.
    let p = gui.node_world(node).transform_point2(gui.nodes[node].pivot);
    let c = Vec2::new(size.x * 0.5f32 + p.x, size.y * 0.5f32 + p.y);
    // The only draw of the widget's slot 1 (no text).
    anchored(item_model(0x0051_c6a7, c, size.x * 0.0006f32, 0.0, item, [1.0; 4], inp), Some(w), false)
}

/// The equipment boxes: box `i` (`GC+0x800a28[i]`) shows equipment slot
/// [`slot::HOVER_ORDER`]`[i]` (`update` 0x0048c7fc..0x0048cbb4: `+0x164 = creature + 0x990,
/// 0xaa8, 0xbc0, 0xcd8, 0x418, 0x878, 0x530, 0x760, 0x648, 0xdf0, 0xf08, 0x1020`).
pub fn equipment_boxes(ui: &GameUi, game: &GameView, inp: &GuiModelInputs) -> Vec<GuiModel> {
    let mut out = Vec::new();
    for (i, &w) in ui.m.equipment_sprites.iter().enumerate() {
        let Some(&k) = slot::HOVER_ORDER.get(i) else { break };
        let Some(item) = game.equipment.get(k) else { continue };
        out.extend(sprite(&ui.gui, w, item, inp));
    }
    out
}

/// The quick-item button's background sprite (`GC+0x800a18`, 0x004921c4..0x004921f7): the
/// selected quick item ([`super::item_preview::quick_items`] at the selection index), or none
/// when there are no quick items or the cursor is free (0x00491f5f..0x00491f69 jump to
/// 0x0049221c, where 0x00492299 sets `+0x160`, `+0x164` = 0).
pub fn quick_item(ui: &GameUi, game: &GameView, inp: &GuiModelInputs) -> Option<GuiModel> {
    let w = ui.m.background_sprite?;
    let list = super::item_preview::quick_items(game);
    if list.is_empty() || super::flow::is_cursor_free(ui, game) {
        return None;
    }
    let (t, i) = list[super::item_preview::wrap_index(game.quick_index, list.len())];
    sprite(&ui.gui, w, &game.inventory[t][i].item, inp)
}

/// Every `0x004758c0` draw of this frame, in the order of the module docs.
pub fn gui_models(ui: &GameUi, game: &GameView, out: &FrameOutput, inp: &GuiModelInputs) -> Vec<GuiModel> {
    let mut v = equipment_boxes(ui, game, inp);
    v.extend(quick_item(ui, game, inp));
    // The InventoryWidgets 0x004c2050: the colour 0x00448280 of each cell (the cursor stack
    // keeps the last one set).
    // The cells come before the widget's texts (0x004c2577, the first `drawText` at
    // 0x004c2862), the cursor stack after the last (0x004c595a after 0x004c5543).
    let mut material = [1.0f32; 4];
    for (f, w) in [(&out.bag, ui.m.inventory), (&out.crafting, ui.m.crafting), (&out.shop, ui.m.shop)] {
        let Some(f) = f else { continue };
        for m in f.models.iter().chain(f.held.iter()) {
            if let Some(c) = m.color {
                material = c;
            }
            let (origin, after) = if m.color.is_some() { (0x004c_2577, false) } else { (0x004c_595a, true) };
            v.extend(anchored(item_model(origin, m.pos, m.scale, 0.0, &m.item, material, inp), w, after));
        }
    }
    // Each of these widgets draws its models before its first `drawText`: 0x00430bd6 before
    // 0x00430efb, 0x0044ed42 before 0x0044ef69, 0x0040fbf2 before 0x0040fec7, 0x0058b736
    // before 0x0058b9da.
    let icon = |origin: u32, i: &super::crafting::ItemIcon, w: Option<WidgetId>| anchored(item_model(origin, Vec2::from(i.center), i.scale, 0.0, &i.item, [1.0; 4], inp), w, false);
    if let Some(bp) = &out.craft_preview {
        v.extend(bp.ingredients.iter().filter_map(|c| icon(0x0043_0bd6, &c.icon, ui.m.craft_preview)));
    }
    if let Some(e) = &out.enchant {
        v.extend(e.icon.as_ref().and_then(|i| icon(0x0044_ed42, i, ui.m.enchant)));
    }
    if let Some(a) = &out.adaption {
        v.extend(a.icon.as_ref().and_then(|i| icon(0x0040_fbf2, i, ui.m.adaption)));
    }
    if let Some(vx) = &out.voxel {
        v.extend(vx.list.iter().filter_map(|i| icon(0x0058_b736, i, ui.m.voxel)));
    }
    // PreviewWidget 0x004d5d02: white (0x00448280), depth -0.05, scale 0.1; before the
    // preview's texts (0x004d6042..).
    if let Some((p, item)) = &out.preview.model {
        v.extend(anchored(item_model(0x004d_5d02, *p, 0.1, -0.05, item, [1.0; 4], inp), ui.m.preview_widget, false));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Models;
    impl GuiModelSource for Models {
        fn size(&self, index: u32) -> Option<[i32; 3]> {
            (index < 0x1000).then_some([10, 20, 5])
        }
        fn spirit_cube(&self) -> ModelRef {
            7
        }
    }

    #[test]
    fn item_model_fields() {
        let inp = GuiModelInputs { rotation: [225.0, 0.0, 12.0], models: &Models };
        let mut ring = super::super::crafting::default_item();
        ring[0] = 9;
        ring[0xd] = 1;
        ring[0x114..0x118].copy_from_slice(&1i32.to_le_bytes());
        ring[0x14..0x18].copy_from_slice(&[1, 0xff, 2, 0]);
        let g = item_model(1, Vec2::new(5.0, 6.0), 0.03, 0.0, &ring, [1.0; 4], &inp).unwrap();
        assert!(g.ring_turn);
        assert_eq!(g.shininess, 1.0);
        assert_eq!(g.spirits.len(), 1);
        assert_eq!(g.spirits[0].0, [1.0, -1.0, 2.0]);
        assert_eq!((g.rotation, g.screen, g.model_size, g.spirit_model), ([225.0, 0.0, 12.0], [5.0, 6.0], [10, 20, 5], 7));
        // Type 0: model 0x95c.
        let empty = super::super::crafting::default_item();
        let mut e = empty.clone();
        e[0] = 0;
        assert_eq!(gui_item_model(&e, &Models).map(|m| m.0), Some(0x95c));
    }

    /// `SpriteWidget::update` 0x0051c57d..0x0051c635 reads the node's Transformation
    /// **pivot** (`+0x38 -> +0x19c[+0x170]`) through the node's world matrix (`+0x48`, divided
    /// by w) and adds half the widget's local size: the centre is `world(pivot) + size / 2`,
    /// not the node's origin.
    #[test]
    fn sprite_centre_uses_the_pivot() {
        use cw_ui::widget::{NodeSource, WidgetSource};
        let mut gui = Gui::new();
        let n = gui.add_node(None, NodeSource { name: "box".into(), visible: true, translation: Vec2::new(100.0, 200.0), pivot: Vec2::new(5.0, 7.0), ..Default::default() });
        let w = gui.add_widget(n, &WidgetSource { frame_size: Vec2::new(40.0, 40.0), bind_size: Vec2::new(40.0, 40.0), ..Default::default() });
        let inp = GuiModelInputs { rotation: [0.0; 3], models: &Models };
        let mut ring = super::super::crafting::default_item();
        ring[0] = 9;
        let g = sprite(&gui, w, &ring, &inp).unwrap();
        assert_eq!(g.anchor, Some(GuiAnchor { widget: w as u32, after_texts: false }));
        let size = gui.local_size(w);
        let want = Vec2::new(105.0, 207.0) + size * 0.5;
        assert_eq!(Vec2::from(g.screen), want);
        assert_eq!(g.scale, size.x * 0.0006f32);
    }

    /// Each model is anchored to the widget whose slot 1 draws it, before or after that
    /// widget's texts: the inventory cells before the stack counts (0x004c2577 precedes
    /// every `drawText` 0x00639b30 of `InventoryWidget::update`, the first at 0x004c2862),
    /// the cursor stack after them (0x004c595a follows the last, 0x004c5543), the preview
    /// model before the preview texts (0x004d5d02, then 0x004d6042..).
    #[test]
    fn models_anchor_to_their_widgets() {
        use super::super::inventory_widget::{InventoryFrame, ItemModel};
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let game = GameView::default();
        let mut ui = super::super::GameUi::new(gui, &mut super::super::members::NoPlx, &game);
        let widget = |ui: &mut super::super::GameUi, name: &str| {
            let n = ui.gui.add_plain_node(None, name);
            ui.gui.add_widget(n, &cw_ui::widget::WidgetSource::default())
        };
        let bag = widget(&mut ui, "inventory");
        let prev = widget(&mut ui, "preview");
        ui.m.inventory = Some(bag);
        ui.m.preview_widget = Some(prev);
        let mut ring = super::super::crafting::default_item();
        ring[0] = 9;
        let cell = ItemModel { pos: Vec2::new(10.0, 10.0), scale: 0.03, color: Some([1.0; 4]), item: ring.clone() };
        let held = ItemModel { color: None, scale: 0.05, ..cell.clone() };
        let mut out = FrameOutput::default();
        out.bag = Some(InventoryFrame { visible: true, models: vec![cell], held: Some(held), ..Default::default() });
        out.preview.model = Some((Vec2::new(5.0, 5.0), ring));
        let inp = GuiModelInputs { rotation: [0.0; 3], models: &Models };
        let v = gui_models(&ui, &game, &out, &inp);
        let a = |w: WidgetId, after_texts: bool| Some(GuiAnchor { widget: w as u32, after_texts });
        let got: Vec<(u32, Option<GuiAnchor>)> = v.iter().map(|g| (g.origin, g.anchor)).collect();
        assert_eq!(got, vec![(0x004c_2577, a(bag, false)), (0x004c_595a, a(bag, true)), (0x004d_5d02, a(prev, false))]);
    }

    /// The identification, adaption and customization widgets (0x0044ea30, 0x0040f8f0,
    /// 0x00588500) place their models from the widget node's pivot through its world matrix;
    /// the port's callers (`GameUi` `node_rect` / `widget_origin`) pass the widget's screen
    /// origin instead. Both agree for the shipped `gui.plx`; this pins that. Skipped without
    /// `CW_GAME_DIR`.
    #[test]
    fn panel_pivots_are_widget_origins() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let ui = super::super::GameUi::new(gui, &mut plx, &GameView::default());
        let m = &ui.m;
        let ws: Vec<WidgetId> = [m.enchant, m.adaption, m.voxel, m.background_sprite].into_iter().flatten().chain(m.equipment_sprites.iter().copied()).collect();
        assert!(ws.len() >= 4);
        for w in ws {
            let n = ui.gui.widgets[w].node;
            let p = ui.gui.node_world(n).transform_point2(ui.gui.nodes[n].pivot);
            let o = ui.gui.widget_world(w).transform_point2(Vec2::ZERO);
            assert!((p - o).length() < 1e-3, "{} {p:?} {o:?}", ui.gui.nodes[n].name);
        }
    }
}
