//! The HUD half of [`super::present`]: what `GameController::update` `Cube.exe 0x00488ee0`
//! writes into the `gui.plx` HUD nodes every frame (from [`FrameOutput`], [`GameUi::hud`]
//! and the widget states), and the text calls of the HUD game widgets' slot 1.
//!
//! Tier B. Node writes follow the original's helpers:
//!
//! | Helper | Here |
//! |---|---|
//! | position: `Transformation.translation = (x, y) − pivot` (0x0040ef70 pivot, 0x00468df0 subtract, 0x004288b0 store, slot 1 `evaluate`) | [`place`] |
//! | fill: `Transformation.deformation = identity · scale(f, 1, 1)` of a `bar` node (0x0042f590, 0x00423e70, 0x00424730) | [`set_fill`] |
//! | `Node::setChildText(name, text, 1)` 0x00636a00: the first node named `name` on each branch gets the text on itself and every TextShape below (0x00636ad0) | [`set_named_text`] |
//! | `Node::setText` 0x00636ad0 | [`set_all_text`] |
//! | `setVisible` 0x00411a90 (Display visibility key) | `Node::visible` |
//! | `setState` 0x00636810 | queued in [`HudState::pending_states`], played by [`play_scene_writes`] |
//!
//! # Map of `update` ranges applied here
//!
//! | Range | Nodes |
//! |---|---|
//! | 0x0048cbc3..0x0048cc0d | cursor `+0x8008a8`: visible = `isCursorFree`, at the engine cursor (no pivot); its caption (0x0048b3cd.., 0x00636ad0) |
//! | 0x0048d054..0x0048d51c | `smallquesttag` (visible, "show" + `objective`), `questbar` (visible, `bar` fill) |
//! | 0x004908bf, 0x004966e2..0x00496886 | the dialog `SpeechWidget` clock and page step; the four bubbles' page step |
//! | 0x00490a93..0x00493d72 | the bars ([`super::hud`]) |
//! | 0x00494147 | `combopoints` visible exactly while it animates |
//! | 0x00494161..0x004944fe | `landscape` at `(W − 260, 90)` with its `landscape` (area name, 0x004e5320) / `landscapedetail` (site name, 0x004e5c10) texts ([`super::hud::HudState::landscape`]), the `landname` banner ([`super::hud::LandnameState`]) |
//! | 0x004944fe..0x00494885 | `info` at `(W − 15, 20)`; `star1`..`star4` under `landscape` hidden; the `info` text ([`super::hud::HudState::info`]) |
//! | 0x00492727..0x004927d5, 0x00497b0b.. | the Tab `selector`: position, visibility, `count` / `name` ([`super::item_preview::selector_texts`]) |
//! | 0x004927da..0x00492977, 0x00496890.. | `crosshair` / `zoomcrosshair` visibility, position, text |
//! | 0x00499e9b..0x0049b18d | the nameplates in `+0x8007b0` |
//! | 0x004ae220 / 0x004ae334 | the map overlay root `+0x8008a0` visible with the map ([`super::map_overlay::overlay_roots`]) |
//!
//! Widget text calls ([`widget_texts`]): `ChatWidget::update` 0x00439730 (size 10, stroke 2:
//! a black stroke pass with a transparent fill, then the coloured fill, `resource1.dat`,
//! pixel-snapped), the dialog `SpeechWidget` 0x004e5f90 answer options (size 14: white with
//! a black outline of 3, then white or cyan), `MapOverlayWidget::update` 0x004c9680
//! (`drawText` 0x00639b30: line spacing 2, centred, wrap −1, outline 3 then the label colour).
//! `ObjectiveWidget` and `StatisticsWidget` draw nothing in this build. `PreviewWidget::update`
//! 0x004d50a0's texts come from [`super::item_preview::widget_texts`]; the preview and skill
//! nodes themselves are written by `GameUi::frame` (`item_preview`, `skill_panel`).

use std::cell::RefCell;
use std::collections::BTreeMap;

use cw_ui::font::{DiskFonts, FontEngine, TextStyle, ENGINE_FONT_SEARCH_PATH};
use cw_ui::loader::{SceneShape, SharedShape};
use cw_ui::render::WidgetText;
use cw_ui::widget::{GameWidget, GameWidgetClass, Gui, NodeId, UiEvent, WidgetId, WidgetSource, node_flags};
use glam::Vec2;

use super::hud::HudNodes;
use super::members::{clone_subtree, find_node};
use super::plx_files::GamePlxLoader;
use super::{FrameOutput, GameUi, GameView};

/// The font of every HUD text call (`resource1.dat`, `ChatWidget+0x184`, `SpeechWidget+0x1a8`).
pub const FONT: &str = "resource1.dat";

// ---------------------------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------------------------

/// `translation = (x, y) − pivot` (0x00468df0, 0x004288b0).
pub fn place(gui: &mut Gui, n: NodeId, pos: [f32; 2]) {
    let p = gui.nodes[n].pivot;
    gui.nodes[n].translation = Vec2::new(pos[0] - p.x, pos[1] - p.y);
}

/// `Transformation.deformation = scale(f, 1, 1)` (row-major 4×4, 0x00423e70 / 0x00424730).
pub fn set_fill(gui: &mut Gui, n: Option<NodeId>, f: f32) {
    let Some(n) = n else { return };
    let mut m = glam::Mat4::IDENTITY.to_cols_array();
    m[0] = f;
    gui.nodes[n].deformation = m;
    gui.nodes[n].deform_dirty = true;
}

fn shared(gui: &Gui, n: NodeId) -> Option<SharedShape> {
    gui.nodes[n].shape.as_ref()?.as_any()?.downcast_ref::<SharedShape>().cloned()
}

/// 0x00636ad0 on one node: a TextShape whose string differs takes `text` (and rebuilds).
fn set_own_text(gui: &mut Gui, n: NodeId, text: &[u16]) {
    if gui.nodes[n].text.is_none() {
        return;
    }
    if gui.nodes[n].text.as_deref() != Some(text) {
        gui.nodes[n].text = Some(text.to_vec());
        if let Some(s) = shared(gui, n) {
            if let SceneShape::Text(t) = &mut *s.borrow_mut() {
                t.string.current = text.to_vec();
                t.sync();
                t.shape.dirty = false;
            }
        }
        gui.events.push(UiEvent::TextRebuild { node: n });
    }
}

/// `Node::setText` 0x00636ad0: the node and every descendant (enabled nodes only) whose
/// shape is a TextShape take `text`.
pub fn set_all_text(gui: &mut Gui, n: NodeId, text: &str) {
    let t: Vec<u16> = text.encode_utf16().collect();
    set_all_text_u16(gui, n, &t);
}

fn set_all_text_u16(gui: &mut Gui, n: NodeId, t: &[u16]) {
    if gui.nodes[n].flags & node_flags::DISABLED != 0 {
        return;
    }
    set_own_text(gui, n, t);
    for c in gui.nodes[n].children.clone() {
        set_all_text_u16(gui, c, t);
    }
}

/// `Node::setChildText(name, text, 1)` 0x00636a00: on each branch the first node named
/// `name` (this node included) gets [`set_all_text`]; its subtree is not searched further.
pub fn set_named_text(gui: &mut Gui, n: NodeId, name: &str, text: &str) {
    let t: Vec<u16> = text.encode_utf16().collect();
    named_text(gui, n, name, &t);
}

fn named_text(gui: &mut Gui, n: NodeId, name: &str, t: &[u16]) {
    if gui.nodes[n].flags & node_flags::DISABLED != 0 {
        return;
    }
    if gui.nodes[n].name == name {
        set_all_text_u16(gui, n, t);
        return;
    }
    for c in gui.nodes[n].children.clone() {
        named_text(gui, c, name, t);
    }
}

/// The colour of every TextShape in the subtree (`shape+0xb4`, 0x00457930 on
/// `combopoints`; 0x00458cf0 on each `info` node of a nameplate).
pub fn set_text_color(gui: &mut Gui, n: NodeId, color: [f32; 4]) {
    if let Some(s) = shared(gui, n) {
        if let SceneShape::Text(t) = &mut *s.borrow_mut() {
            t.color.current = color;
            t.shape.source.color = glam::Vec4::from_array(color);
        }
    }
    for c in gui.nodes[n].children.clone() {
        set_text_color(gui, c, color);
    }
}

fn named_nodes(gui: &Gui, n: NodeId, name: &str, out: &mut Vec<NodeId>) {
    if gui.nodes[n].name == name {
        out.push(n);
    }
    for &c in &gui.nodes[n].children {
        named_nodes(gui, c, name, out);
    }
}

fn set_vis(gui: &mut Gui, n: Option<NodeId>, v: bool) {
    if let Some(n) = n {
        gui.nodes[n].visible = v;
    }
}

/// The first `count` children visible, the rest hidden (the mana cubes 0x00490da2, the
/// riding cubes 0x00491817).
fn show_first(gui: &mut Gui, n: NodeId, count: i32) {
    for (i, c) in gui.nodes[n].children.clone().into_iter().enumerate() {
        gui.nodes[c].visible = (i as i32) < count;
    }
}

/// Moves `child` (a child of `parent`) to directly after `anchor` in the child list (draw
/// order), or to the end when `anchor` is not a child.
fn move_after(gui: &mut Gui, parent: NodeId, child: NodeId, anchor: Option<NodeId>) {
    let ch = &mut gui.nodes[parent].children;
    ch.retain(|&c| c != child);
    match anchor.and_then(|a| ch.iter().position(|&c| c == a)) {
        Some(i) => ch.insert(i + 1, child),
        None => ch.push(child),
    }
}

/// A clone of `src` into `parent` placed after `after` (the original clones in ctor order;
/// the clones follow the template's position in the GUI root here).
fn clone_after(gui: &mut Gui, src: NodeId, parent: NodeId, after: NodeId) -> NodeId {
    let c = clone_subtree(gui, src, Some(parent));
    move_after(gui, parent, c, Some(after));
    c
}

/// Restores a reused clone from its template: visibility, transformation, text and a fresh
/// copy of each shape, node by node (the clone was made from the template, so the enabled
/// children pair up).
fn restore(gui: &mut Gui, dst: NodeId, src: NodeId) {
    let (vis, tr, piv, rot, def, text) = {
        let s = &gui.nodes[src];
        (s.visible, s.translation, s.pivot, s.rotation, s.deformation, s.text.clone())
    };
    let shape = gui.nodes[src].shape.as_ref().and_then(|s| s.clone_shape());
    let d = &mut gui.nodes[dst];
    d.visible = vis;
    d.translation = tr;
    d.pivot = piv;
    d.rotation = rot;
    d.deformation = def;
    d.text = text;
    if shape.is_some() {
        d.shape = shape;
    }
    let sc: Vec<NodeId> = gui.nodes[src].children.iter().copied().filter(|&c| gui.nodes[c].flags & node_flags::DISABLED == 0).collect();
    let dc = gui.nodes[dst].children.clone();
    for (a, b) in dc.into_iter().zip(sc) {
        restore(gui, a, b);
    }
}

// ---------------------------------------------------------------------------------------
// The ctor's HUD nodes
// ---------------------------------------------------------------------------------------

/// The HUD parts of the ctor 0x00459c40 that `members::construct` does not build: the `bar`
/// lookups, the pet and party clones (0x00460f82, 0x00461211, 0x0046128c), the nameplate
/// container (0x00461741), `crosshair`/`zoomcrosshair` moved to the end of the GUI root and
/// the zoom crosshair hidden (0x0046176a..0x00461788), the four speech bubbles and the
/// deletion of their template (0x00461ab3..0x00461c21), the deletion of the `menubutton`
/// template (0x00460e82), the map overlay root and widget (0x0045c090..0x0045c11b).
pub fn build_nodes(ui: &mut GameUi) -> HudNodes {
    let m = ui.m.clone();
    let gui = &mut ui.gui;
    let mut h = HudNodes::default();
    let Some(g) = m.gui_root else { return h };
    let find = |gui: &Gui, n: Option<NodeId>, name: &str| n.and_then(|n| find_node(gui, n, name));
    // Experience bar and the pet's clone (0x00460e97..0x00460fd5).
    h.xp_bar_fill = find(gui, m.experience_bar, "bar");
    if let Some(xp) = m.experience_bar {
        let c = clone_after(gui, xp, g, xp);
        h.pet_xp = Some(c);
        h.pet_xp_fill = find_node(gui, c, "bar");
    }
    // `questbar` under `smallquesttag`, hidden (0x00461072..0x00461105).
    h.quest_bar = find(gui, m.smallquesttag, "questbar");
    h.quest_fill = find(gui, h.quest_bar, "bar");
    set_vis(gui, h.quest_bar, false);
    // Life bar, the pet's clone and the three party clones (0x00461116..0x00461353).
    h.life_fill = find(gui, m.life_bar, "bar");
    if let Some(lb) = m.life_bar {
        let c = clone_after(gui, lb, g, lb);
        h.pet_life = Some(c);
        h.pet_life_fill = find_node(gui, c, "bar");
        let mut after = c;
        for _ in 0..3 {
            let p = clone_after(gui, lb, g, after);
            after = p;
            h.party.push(p);
            h.party_fill.push(find_node(gui, p, "bar"));
        }
    }
    h.mp_fill = find(gui, m.mp_bar, "bar");
    h.charge_fill = find(gui, m.mp_bar, "chargebar");
    h.hp_fill = find(gui, m.hp_bar, "bar");
    h.cast_fill = find(gui, m.cast_bar, "bar");
    h.stamina_fill = find(gui, m.stamina_bar, "bar");
    // 0x00461741: the nameplate container, then the crosshairs re-attached after it
    // (0x00636950 moves a node to the end of its new parent).
    let plates = gui.add_plain_node(Some(g), "");
    h.plates = Some(plates);
    for c in [m.crosshair, m.zoomcrosshair].into_iter().flatten() {
        if let Some(p) = gui.nodes[c].parent {
            gui.nodes[p].children.retain(|&x| x != c);
        }
        gui.nodes[c].parent = Some(g);
        gui.nodes[g].children.push(c);
    }
    set_vis(gui, m.zoomcrosshair, false);
    // 0x00461a68..0x00461c21: four bubbles cloned from `speech` (Display and
    // Transformation copied), each with a SpeechWidget, hidden; then the template deleted
    // (hidden and disabled here).
    if let Some(t) = find_node(gui, g, "speech") {
        for _ in 0..4 {
            let c = clone_subtree(gui, t, Some(g));
            // 0x00461b7b..0x00461bbc: `new SpeechWidget(gui, clone, gc)` (0x004e5c90), whose
            // `0x00627c00` makes it 250x200 (the width the text wraps against).
            let w = match gui.nodes[c].widget {
                Some(w) => w,
                None => {
                    let w = gui.add_game_widget(c, &super::speech::widget_source(), GameWidget::new(GameWidgetClass::Speech));
                    super::speech::init_widget(gui, w);
                    w
                }
            };
            gui.nodes[c].visible = false;
            h.bubbles.push((c, w));
        }
        gui.nodes[t].visible = false;
        gui.nodes[t].flags |= node_flags::DISABLED;
    }
    // (0x00460e82, the `menubutton` template deleted: `members::construct`.)
    // 0x0045c090..0x0045c11b: the map overlay root (under the engine root, hidden until the
    // map opens) and the MapOverlayWidget's node.
    if let Some(top) = gui.root {
        let r = gui.add_plain_node(Some(top), "");
        gui.nodes[r].visible = false;
        let n = gui.add_plain_node(Some(r), "");
        let w = gui.add_game_widget(n, &WidgetSource::default(), GameWidget::new(GameWidgetClass::MapOverlay));
        h.overlay_root = Some(r);
        h.overlay_node = Some(n);
        h.overlay = Some(w);
    }
    h
}

// ---------------------------------------------------------------------------------------
// Text measure
// ---------------------------------------------------------------------------------------

/// The UI's text measure: a `FontEngine` over the game folder's font files, owned by
/// [`GameUi`] (`GameUi::fonts`) and created on the first measure (the original's engine
/// loads `resource1.dat` in the ctor, 0x0045ddb8).
pub struct TextMeasure {
    /// The folder the font files are read from (`DiskFonts::base`).
    base: std::path::PathBuf,
    /// The engine and its file source, created on first use. Interior mutability because the
    /// layouts take a `&dyn Fn` measure while other fields of the UI are borrowed mutably.
    cache: RefCell<Option<(FontEngine, DiskFonts)>>,
}

impl TextMeasure {
    /// A measure over the fonts of `base`.
    pub fn new(base: std::path::PathBuf) -> Self {
        TextMeasure { base, cache: RefCell::new(None) }
    }

    /// `ScalableFont::measure` 0x0065e720 (`x1 − x0` of the ink box, line start −1, pixel
    /// snapped, identity transform) of `text` in [`FONT`]; 0 without the font file.
    pub fn measure(&self, text: &str, size: f32, stroke: f32) -> f32 {
        let mut f = self.cache.borrow_mut();
        let (engine, files) = f.get_or_insert_with(|| {
            let mut e = FontEngine::new();
            e.add_search_path(ENGINE_FONT_SEARCH_PATH);
            (e, DiskFonts { base: self.base.clone() })
        });
        let Some(font) = engine.get_font(files, FONT) else { return 0.0 };
        let t: Vec<u16> = text.encode_utf16().collect();
        let style = TextStyle { size, stroke_radius: stroke, spacing: 0.0, line_spacing: 0.0, wrap_width: 0.0, flags: 0, pixel_snap: true };
        let (a, b) = font.bounds(&t, &glam::Mat4::IDENTITY.to_cols_array(), &style, -1, false);
        b.x - a.x
    }

    /// The chat's measure (size 10, stroke 2): what `ChatWidget::print` 0x0043a500 and the
    /// update body wrap and advance with.
    pub fn measure_chat(&self, text: &str) -> f32 {
        self.measure(text, super::chat::FONT_SIZE, 2.0)
    }
}

impl Default for TextMeasure {
    fn default() -> Self {
        TextMeasure::new(crate::controller::default_game_dir())
    }
}

impl std::fmt::Debug for TextMeasure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextMeasure").field("base", &self.base).field("loaded", &self.cache.borrow().is_some()).finish()
    }
}

// ---------------------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------------------

/// Writes this frame's HUD state into the tree (see the module map). Runs after
/// `GameUi::frame` in `update`.
pub fn apply(ui: &mut GameUi, game: &GameView, out: &FrameOutput) {
    if ui.hud_state.nodes.is_none() {
        let n = build_nodes(ui);
        ui.hud_state.nodes = Some(n);
    }
    let h = ui.hud_state.nodes.clone().unwrap_or_default();
    let m = ui.m.clone();
    let dt = ui.gui.frame_ms;
    let (w, _hh) = (ui.gui.viewport.x, ui.gui.viewport.y);

    // 0x0048cbc3: the cursor.
    if let Some(c) = m.cursor {
        let free = super::flow::is_cursor_free(ui, game);
        ui.gui.nodes[c].visible = free;
        ui.gui.nodes[c].translation = ui.gui.cursor;
        set_all_text(&mut ui.gui, c, &out.cursor_caption);
    }

    // 0x0048d054..0x0048d51c: the small quest tag and its bar.
    if let Some(q) = &out.quest_tag {
        if let Some(show) = &q.small_show {
            if let Some(t) = m.smallquesttag {
                ui.hud_state.pending_states.push((t, "show"));
                set_named_text(&mut ui.gui, t, "objective", show);
            }
        }
        if let Some(v) = q.small_visible {
            set_vis(&mut ui.gui, m.smallquesttag, v);
        }
        if let Some(bar) = &q.quest_bar {
            set_vis(&mut ui.gui, h.quest_bar, bar.is_some());
            if let Some(f) = bar {
                set_fill(&mut ui.gui, h.quest_fill, *f);
            }
        }
    }

    // 0x0049081b..0x004908ae: the bubbles' clocks; 0x004908bf: the dialog's.
    super::bubbles::ensure(&mut ui.bubbles, h.bubbles.len());
    super::bubbles::advance_clocks(&mut ui.bubbles, dt);
    ui.speech.advance(dt);

    // 0x00490a93..0x00490cf5: the experience bar at (100, 75).
    if let Some(xp) = m.experience_bar {
        let t = ui.hud.xp_text.clone();
        set_named_text(&mut ui.gui, xp, "text", &t);
        place(&mut ui.gui, xp, [100.0, 75.0]);
        set_fill(&mut ui.gui, h.xp_bar_fill, ui.hud.xp_fill);
    }
    if let Some(b) = out.hud_bars.clone() {
        apply_bars(ui, &h, &b);
    }
    // 0x00494147: `combopoints` shows while its "hit" state plays.
    set_vis(&mut ui.gui, m.combopoints, ui.hud_state.combopoints_animating);
    // 0x00494161: the region banner node at (W − 260, 90) (no pivot).
    if let Some(l) = m.landscape {
        ui.gui.nodes[l].translation = Vec2::new((w - 0x104) as f32, 90.0);
        // 0x004942b1..0x00494348: `setChildText(L"landscape", area, 1)` (the 0x004e5320
        // result, `esp+0x1564`) then `setChildText(L"landscapedetail", site, 1)` (the
        // 0x004e5c10 result, `esp+0x1594`) (0x00636a00). The first matches `landscape`
        // itself, so every text below it (the detail pair too) takes the area name before
        // the second call gives the size-12 detail pair the site name.
        if let Some((area, site)) = ui.hud_state.landscape.clone() {
            set_named_text(&mut ui.gui, l, "landscape", &area);
            set_named_text(&mut ui.gui, l, "landscapedetail", &site);
        }
    }
    // 0x0049434d..0x004944fe: `landname` (its names were set with the landscape texts,
    // `hud::landscape_names`).
    let lf = ui.hud_state.landname.step(dt);
    if let Some(ln) = m.landname {
        if let Some((name, detail)) = &lf.texts {
            set_named_text(&mut ui.gui, ln, "name", name);
            set_named_text(&mut ui.gui, ln, "detail", detail);
        }
        ui.gui.nodes[ln].visible = lf.visible;
        if let Some(c) = lf.color {
            ui.hud_state.pending_fill.push((ln, c));
        }
    }
    // 0x004944fe..0x00494554: `info` +0x800860 at (W − 15, 20) (no pivot). Its text (the
    // time / temperature / humidity stream of 0x00494554..0x00494713, `hud::info_text`) is
    // written after the stars.
    if let Some(n) = m.info {
        ui.gui.nodes[n].translation = Vec2::new((w - 0xf) as f32, 20.0);
    }
    // 0x00494722..0x0049483a: `findNode(L"star1".."star4")` on `landscape`, `setVisible(0)`.
    if let Some(l) = m.landscape {
        for s in super::hud::LANDSCAPE_STARS {
            if let Some(n) = find_node(&ui.gui, l, s) {
                ui.gui.nodes[n].visible = false;
            }
        }
    }
    // 0x0049483f..0x00494885: `setChildText(L"info" (0x006ffee0), stream.str(), 1)` on
    // +0x800860 (the stream's `str()` 0x00411bc0): the outline node and its fill child.
    if let (Some(n), Some(t)) = (m.info, ui.hud_state.info.clone()) {
        set_named_text(&mut ui.gui, n, "info", &t);
    }

    // The skill and specialization buttons and the item preview (`preview`, `equippedpreview`)
    // were placed and shown by `GameUi::frame` (`skill_panel`, `item_preview`,
    // 0x0048d771..0x0048f17e).

    // 0x00492727..0x004927d5: the quick-item `selector` (+0x8008e0) at (W/2, H/2 − 200)
    // minus its pivot, shown while the Tab menu (`GC+0x800a40`) is open; its `count` and
    // `name` texts (0x00497b0b..).
    if let Some(n) = m.selector {
        let (vw, vh) = (ui.gui.viewport.x, ui.gui.viewport.y);
        place(&mut ui.gui, n, [(vw / 2) as f32, (vh / 2 - 200) as f32]);
        ui.gui.nodes[n].visible = ui.flag_800a40;
    }
    super::item_preview::selector_texts(ui, game);
    // 0x00491f65..0x0049228f: the quick-item button's stack count.
    super::quick_item::apply_button_count(ui, game);

    // 0x004927da..0x00492977 and 0x00496890..: the crosshairs.
    if let Some(c) = &out.crosshair {
        if let Some(n) = m.crosshair {
            ui.gui.nodes[n].visible = c.crosshair;
            place(&mut ui.gui, n, c.position);
        }
        if let Some(n) = m.zoomcrosshair {
            ui.gui.nodes[n].visible = c.zoom;
            place(&mut ui.gui, n, c.position);
        }
        if let Some(n) = m.crosshair {
            set_all_text(&mut ui.gui, n, &c.text);
        }
    }

    // 0x004966e2..0x004967ed: the bubbles' page step (`ui::bubbles`); 0x00496805..0x00496886:
    // the dialog's.
    let hidden = super::bubbles::page_steps(&mut ui.bubbles);
    for (&(n, _), hide) in h.bubbles.iter().zip(hidden) {
        if hide {
            ui.gui.nodes[n].visible = false;
        }
    }
    if ui.speech.page_step() {
        set_vis(&mut ui.gui, m.speech_node, false);
    }
    ui.hud_state.speech_options = if out.speech_options && ui.visible(m.speech_node) {
        let height = m.speech.map_or(0.0, |s| ui.gui.height(s));
        let pivot = m.speech_node.map_or(Vec2::ZERO, |n| ui.gui.nodes[n].pivot);
        ui.speech.option_layout(height, pivot.to_array(), &|t| ui.fonts.measure(t, super::speech::OPTION_SIZE, 0.0))
    } else {
        Vec::new()
    };

    // 0x00499e9b..0x0049b18d: the nameplates.
    apply_plates(ui, &h, out);

    // 0x004ae220 / 0x004ae334: the overlay root follows the map.
    set_vis(&mut ui.gui, h.overlay_root, game.map_open);

    // `ChatWidget::update` 0x00439730 (its node is always drawn with the GUI root).
    let height = m.chat.map_or(0.0, |c| ui.gui.height(c));
    ui.hud_state.chat_draws = ui.chat.update(height, game.engine_time_ms, &|t| ui.fonts.measure_chat(t));
}

/// The bars of `update` 0x00490cf5..0x00493d72 ([`super::hud::hud_bars`]).
fn apply_bars(ui: &mut GameUi, h: &HudNodes, b: &super::hud::HudBars) {
    let m = ui.m.clone();
    let gui = &mut ui.gui;
    // 0x00490cf5: mana cubes at (100, 130).
    if let Some(n) = m.manacube_bar {
        place(gui, n, b.manacube.position);
        show_first(gui, n, b.manacube.shown);
    }
    // 0x00490e37: the player's life frame.
    if let Some(n) = m.life_bar {
        life_frame(gui, n, h.life_fill, &b.player);
    }
    // 0x004911d1: the pet.
    match &b.pet {
        Some(p) => {
            set_vis(gui, h.pet_life, true);
            set_vis(gui, h.pet_xp, true);
            set_vis(gui, m.riding_bar, true);
            if let Some(n) = h.pet_life {
                life_frame(gui, n, h.pet_life_fill, &p.life);
            }
            if let Some(n) = h.pet_xp {
                set_named_text(gui, n, "text", &p.xp_text);
                place(gui, n, [100.0, 225.0]);
                set_fill(gui, h.pet_xp_fill, p.xp_fill);
            }
            if let Some(n) = m.riding_bar {
                place(gui, n, [100.0, 290.0]);
                show_first(gui, n, p.riding_cubes);
            }
        }
        None => {
            // 0x004918e1.
            set_vis(gui, h.pet_life, false);
            set_vis(gui, h.pet_xp, false);
            set_vis(gui, m.riding_bar, false);
        }
    }
    // 0x00491902: the party slots, all hidden, then one per member.
    for (i, &n) in h.party.iter().enumerate() {
        gui.nodes[n].visible = false;
        if let Some(Some(f)) = b.party.get(i) {
            gui.nodes[n].visible = true;
            life_frame(gui, n, h.party_fill.get(i).copied().flatten(), f);
        }
    }
    // 0x00492977: `mpbar`.
    if let Some(n) = m.mp_bar {
        set_named_text(gui, n, "text", &b.mp.text);
        place(gui, n, b.mp.position);
        set_fill(gui, h.mp_fill, b.mp.fill);
        set_fill(gui, h.charge_fill, b.mp.charge_fill);
    }
    // 0x00492bd4: `hpbar`.
    if let Some(n) = m.hp_bar {
        let (t, f) = (ui.hud.life_text.clone(), ui.hud.life_fill);
        set_named_text(gui, n, "text", &t);
        place(gui, n, b.hp_position);
        set_fill(gui, h.hp_fill, f);
    }
    // 0x00492ddd: `staminabar`.
    if let Some(n) = m.stamina_bar {
        gui.nodes[n].visible = b.stamina.is_some();
        if let Some(s) = &b.stamina {
            place(gui, n, s.position);
            set_fill(gui, h.stamina_fill, s.fill);
        }
    }
    // 0x00492f1d: `castbar`.
    if let Some(n) = m.cast_bar {
        gui.nodes[n].visible = b.cast.is_some();
        if let Some(c) = &b.cast {
            place(gui, n, c.position);
            set_named_text(gui, n, "name", &c.name);
            set_fill(gui, h.cast_fill, c.fill);
        }
    }
    // 0x00493b0f: `combopoints` on a change of the hit counter.
    if let (Some(n), Some(c)) = (m.combopoints, &b.combo) {
        set_all_text(gui, n, &c.text);
        set_text_color(gui, n, c.color);
        if c.play_hit {
            ui.hud_state.pending_states.push((n, "hit"));
        }
    }
}

/// A life frame (`lifebar` or a clone): `text`, `info`, `class`, position, `bar` fill
/// (0x00490e37..0x004911cc).
fn life_frame(gui: &mut Gui, n: NodeId, fill: Option<NodeId>, f: &super::hud::LifeFrame) {
    set_named_text(gui, n, "text", &f.text);
    place(gui, n, f.position);
    set_named_text(gui, n, "info", &f.info);
    set_named_text(gui, n, "class", &f.class);
    set_fill(gui, fill, f.fill);
}

/// The nameplates (0x00499e9b..0x0049b18d): the container is emptied (0x00632870) and each
/// plate is a clone of its `:small` template (0x00636040) with its position, the
/// `rarename` children's visibility and text, the `info` text and colour, the `text` and the
/// `bar`/`bar2` fills.
fn apply_plates(ui: &mut GameUi, h: &HudNodes, out: &FrameOutput) {
    let Some(container) = h.plates else { return };
    let mut pool = ui.hud_state.nodes.as_ref().map(|n| n.plate_pool.clone()).unwrap_or_default();
    let gui = &mut ui.gui;
    for p in pool.iter().flatten() {
        gui.nodes[*p].visible = false;
    }
    let mut used = [0usize; 4];
    let mut order = Vec::new();
    for p in &out.nameplates {
        let k = p.kind.member_index() - 4;
        let Some(tmpl) = ui.m.target_bars[p.kind.member_index()] else { continue };
        let n = if used[k] < pool[k].len() {
            let n = pool[k][used[k]];
            restore(gui, n, tmpl);
            n
        } else {
            let n = clone_subtree(gui, tmpl, Some(container));
            pool[k].push(n);
            n
        };
        used[k] += 1;
        // The template is detached in the original (0x00636950(0)), not hidden: its clones
        // show.
        gui.nodes[n].visible = true;
        order.push(n);
        if let Some(v) = p.rare_name_visible {
            for c in gui.nodes[n].children.clone() {
                if gui.nodes[c].name == "rarename" {
                    gui.nodes[c].visible = v;
                }
            }
        }
        place(gui, n, p.position);
        set_named_text(gui, n, "rarename", &p.rare_name);
        set_named_text(gui, n, "info", &p.info);
        if let Some(c) = p.info_color {
            let mut infos = Vec::new();
            named_nodes(gui, n, "info", &mut infos);
            for i in infos {
                set_text_color(gui, i, c);
            }
        }
        set_named_text(gui, n, "text", &p.text);
        let bar = find_node(gui, n, "bar");
        set_fill(gui, bar, p.bar);
        let bar2 = find_node(gui, n, "bar2");
        set_fill(gui, bar2, p.bar2);
    }
    // Draw order: this frame's plates in order, then the idle clones (hidden).
    let mut rest: Vec<NodeId> = gui.nodes[container].children.iter().copied().filter(|c| !order.contains(c)).collect();
    order.append(&mut rest);
    gui.nodes[container].children = order;
    if let Some(n) = ui.hud_state.nodes.as_mut() {
        n.plate_pool = pool;
    }
}

// ---------------------------------------------------------------------------------------
// Scene writes
// ---------------------------------------------------------------------------------------

/// `Node::isAnimating` 0x006364f0 inputs of [`apply`] (`combopoints`, 0x00494147), from the
/// loaded scenes. Call before [`apply`].
pub fn animation_inputs(ui: &mut GameUi, plx: &GamePlxLoader) {
    ui.hud_state.combopoints_animating = ui.m.combopoints.is_some_and(|n| plx.loaded.iter().any(|l| l.scene.is_animating(&ui.gui, n)));
}

/// Plays the `setState` calls and Display writes [`apply`] queued on the `.plx` scenes
/// (the scene that built a node owns its attributes). Call after [`apply`].
pub fn play_scene_writes(ui: &mut GameUi, plx: &mut GamePlxLoader) {
    for (n, name) in std::mem::take(&mut ui.hud_state.pending_states) {
        for l in plx.loaded.iter_mut() {
            if l.scene.node_objects.contains_key(&n) {
                l.scene.set_state(&ui.gui, n, name, 0);
            }
        }
    }
    for (n, c) in std::mem::take(&mut ui.hud_state.pending_fill) {
        for l in plx.loaded.iter_mut() {
            if let Some(&(_, di, _)) = l.scene.node_objects.get(&n) {
                if let Some(d) = l.scene.displays.get_mut(di) {
                    d.fill_color.current = c;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// widget_texts
// ---------------------------------------------------------------------------------------

fn text_call(text: &str, origin: [f32; 2], style: TextStyle, color: [f32; 4], stroke_color: [f32; 4]) -> WidgetText {
    WidgetText { font: FONT.into(), text: text.encode_utf16().collect(), origin: Vec2::from_array(origin), style, color, stroke_color }
}

fn style(size: f32, stroke: f32, line_spacing: f32, flags: u32, wrap: f32) -> TextStyle {
    TextStyle { size, stroke_radius: stroke, spacing: 0.0, line_spacing, wrap_width: wrap, flags, pixel_snap: true }
}

/// The HUD game widgets' slot-1 text calls (see the module docs).
pub fn widget_texts(ui: &GameUi, game: &GameView, out: &FrameOutput, texts: &mut BTreeMap<WidgetId, Vec<WidgetText>>) {
    // ChatWidget 0x00439730: each item a black stroke pass (fill (0,0,0,0), stroke
    // (0,0,0,1)) then the fill pass (colour, stroke colour 0), size 10, stroke 2.
    if let Some(w) = ui.m.chat {
        let mut v = Vec::new();
        for d in &ui.hud_state.chat_draws {
            let st = style(super::chat::FONT_SIZE, 2.0, 0.0, 0, 0.0);
            v.push(text_call(&d.text, d.pos.to_array(), st, [0.0; 4], [0.0, 0.0, 0.0, 1.0]));
            v.push(text_call(&d.text, d.pos.to_array(), st, d.color, [0.0; 4]));
        }
        if !v.is_empty() {
            texts.entry(w).or_default().extend(v);
        }
    }
    // SpeechWidget 0x004e5f90: the answer options (white with a black outline of 3, then
    // white or cyan (0, 1, 1, 1) without stroke).
    if let Some(w) = ui.m.speech {
        let mut v = Vec::new();
        for (t, x, y, hovered) in &ui.hud_state.speech_options {
            v.push(text_call(t, [*x, *y], style(super::speech::OPTION_SIZE, super::speech::OPTION_OUTLINE, 0.0, 0, 0.0), [1.0; 4], [0.0, 0.0, 0.0, 1.0]));
            let c = if *hovered { [0.0, 1.0, 1.0, 1.0] } else { [1.0; 4] };
            v.push(text_call(t, [*x, *y], style(super::speech::OPTION_SIZE, 0.0, 0.0, 0, 0.0), c, [0.0, 0.0, 0.0, 1.0]));
        }
        if !v.is_empty() {
            texts.entry(w).or_default().extend(v);
        }
    }
    // The speech bubbles' SpeechWidget 0x004e5f90: the runs of the page laid out by
    // 0x004e65a0 (`ui::bubbles::layout`), each an outline pass (the run's colour, black
    // stroke of 3) then a fill pass (no stroke), size 14, in the node's space.
    if let Some(h) = ui.hud_state.nodes.as_ref() {
        for (i, &(n, w)) in h.bubbles.iter().enumerate() {
            let Some(b) = ui.bubbles.get(i) else { continue };
            if !super::gui_models::shown(&ui.gui, n) {
                continue;
            }
            let pivot = ui.gui.nodes[n].pivot;
            let runs = super::bubbles::layout(b, super::bubbles::bubble_width(&ui.gui, w), pivot, &|t| ui.fonts.measure(t, super::speech::OPTION_SIZE, 0.0));
            let mut v = Vec::new();
            for r in runs {
                v.push(text_call(&r.text, r.origin, style(super::speech::OPTION_SIZE, super::speech::OPTION_OUTLINE, 0.0, 0, 0.0), r.color, [0.0, 0.0, 0.0, 1.0]));
                v.push(text_call(&r.text, r.origin, style(super::speech::OPTION_SIZE, 0.0, 0.0, 0, 0.0), r.color, [0.0; 4]));
            }
            if !v.is_empty() {
                texts.entry(w).or_default().extend(v);
            }
        }
    }
    // MapOverlayWidget 0x004c9680 (0x004ca2d8 / 0x004ca3c0): `drawText(font, text, spacing 0,
    // line spacing 2, x, y, size, stroke, colour, stroke colour, 0, flags 1, wrap −1, snap)`.
    if let (Some(w), Some(f)) = (ui.hud_state.nodes.as_ref().and_then(|n| n.overlay), &out.map_overlay) {
        let mut v = Vec::new();
        for l in &f.labels {
            let a = style(l.size, l.outline, 2.0, cw_ui::font::align::H_CENTER, -1.0);
            v.push(text_call(&l.text, l.position, a, [1.0; 4], [0.0, 0.0, 0.0, 1.0]));
            let b = style(l.size, 0.0, 2.0, cw_ui::font::align::H_CENTER, -1.0);
            v.push(text_call(&l.text, l.position, b, l.color, [0.0; 4]));
        }
        texts.entry(w).or_default().extend(v);
    }
    // PreviewWidget 0x004d50a0.
    if let Some(w) = ui.m.preview_widget {
        let v = super::item_preview::widget_texts(ui, game);
        if !v.is_empty() {
            texts.entry(w).or_default().extend(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_ui::widget::NodeSource;

    fn text_node(gui: &mut Gui, parent: Option<NodeId>, name: &str) -> NodeId {
        gui.add_node(parent, NodeSource { name: name.into(), visible: true, text: Some(String::new()), ..Default::default() })
    }

    /// The ctor's banner (0x0045adae..0x0045b55a) is four TextShape nodes; `update` puts it at
    /// (W − 260, 90), writes the area name (0x004e5320) then the site name (0x004e5c10) (0x004942b1..0x00494348) and hides the
    /// cloned stars (0x00494722..0x0049483a).
    #[test]
    fn landscape_banner_texts_and_stars() {
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let game = GameView::default();
        let mut ui = GameUi::new(gui, &mut crate::ui::members::NoPlx, &game);
        let l = ui.m.landscape.unwrap();
        let kids = ui.gui.nodes[l].children.clone();
        assert_eq!(kids.len(), 2);
        let (fill, detail) = (kids[0], kids[1]);
        assert_eq!(ui.gui.nodes[fill].name, "landscape");
        assert_eq!(ui.gui.nodes[detail].name, "landscapedetail");
        assert_eq!(ui.gui.nodes[detail].translation, Vec2::new(0.0, 20.0));
        let detail_fill = ui.gui.nodes[detail].children[0];
        for n in [l, fill, detail, detail_fill] {
            let sh = shared(&ui.gui, n).expect("TextShape");
            let SceneShape::Text(t) = &*sh.borrow() else { panic!("not a TextShape") };
            assert_eq!(String::from_utf16_lossy(&t.string.current), "Hyara Planes");
            let size = if n == l || n == fill { 20.0 } else { 12.0 };
            assert_eq!(t.shape.source.size, size);
            let stroke = if n == l || n == detail { 3.0 } else { 0.0 };
            assert_eq!(t.shape.source.stroke_radius, stroke);
        }
        // Stand-ins for the gui.plx star clones.
        let star = ui.gui.add_plain_node(Some(l), "star3");
        ui.hud_state.landscape = Some(("Lands of Kro".into(), "Trade District".into()));
        ui.hud_state.info = Some("TIME 13:05  TEMP 25 \u{b0}C  HUM 45%".into());
        apply(&mut ui, &game, &FrameOutput::default());
        assert_eq!(ui.gui.nodes[l].translation, Vec2::new(1020.0, 90.0));
        let text = |ui: &GameUi, n: NodeId| ui.gui.nodes[n].text.as_ref().map(|t| String::from_utf16_lossy(t));
        assert_eq!(text(&ui, l).as_deref(), Some("Lands of Kro"));
        assert_eq!(text(&ui, fill).as_deref(), Some("Lands of Kro"));
        assert_eq!(text(&ui, detail).as_deref(), Some("Trade District"));
        assert_eq!(text(&ui, detail_fill).as_deref(), Some("Trade District"));
        let sh = shared(&ui.gui, detail_fill).unwrap();
        let SceneShape::Text(t) = &*sh.borrow() else { panic!() };
        assert_eq!(String::from_utf16_lossy(&t.string.current), "Trade District");
        assert!(!ui.gui.nodes[star].visible);
        // 0x0045bd3f..: `info` (size 12, right-aligned, outline 2) and its fill child; the
        // text of 0x0049483f..0x00494885 on both, at (W − 15, 20).
        let info = ui.m.info.unwrap();
        let info_fill = ui.gui.nodes[info].children[0];
        assert_eq!(ui.gui.nodes[info].translation, Vec2::new(1265.0, 20.0));
        for (n, stroke) in [(info, 2.0), (info_fill, 0.0)] {
            assert_eq!(ui.gui.nodes[n].name, "info");
            assert_eq!(text(&ui, n).as_deref(), Some("TIME 13:05  TEMP 25 \u{b0}C  HUM 45%"));
            let sh = shared(&ui.gui, n).expect("TextShape");
            let SceneShape::Text(t) = &*sh.borrow() else { panic!("not a TextShape") };
            assert_eq!(t.shape.source.size, 12.0);
            assert_eq!(t.shape.source.stroke_radius, stroke);
            assert_eq!(t.shape.source.flags, cw_ui::font::align::RIGHT);
        }
    }

    #[test]
    fn named_text_and_fill() {
        let mut gui = Gui::new();
        let root = gui.add_plain_node(None, "lifebar");
        let t = text_node(&mut gui, Some(root), "text");
        let deep = gui.add_plain_node(Some(root), "frame");
        let t2 = text_node(&mut gui, Some(deep), "text");
        let info = text_node(&mut gui, Some(root), "info");
        let bar = gui.add_plain_node(Some(root), "bar");
        set_named_text(&mut gui, root, "text", "5/10");
        assert_eq!(gui.nodes[t].text, Some("5/10".encode_utf16().collect()));
        assert_eq!(gui.nodes[t2].text, Some("5/10".encode_utf16().collect()));
        assert_eq!(gui.nodes[info].text, Some(Vec::new()));
        set_fill(&mut gui, Some(bar), 0.5);
        assert_eq!(gui.nodes[bar].deformation[0], 0.5);
        assert_eq!(gui.nodes[bar].deformation[5], 1.0);
        gui.nodes[root].pivot = Vec2::new(3.0, 4.0);
        place(&mut gui, root, [100.0, 30.0]);
        assert_eq!(gui.nodes[root].translation, Vec2::new(97.0, 26.0));
    }

    #[test]
    fn plates_reuse_clones() {
        let mut gui = Gui::new();
        let top = gui.add_plain_node(None, "");
        let g = gui.add_plain_node(Some(top), "");
        let tmpl = gui.add_node(Some(g), NodeSource { name: "friendlifebar:small".into(), visible: false, ..Default::default() });
        text_node(&mut gui, Some(tmpl), "text");
        let rn = gui.add_node(Some(tmpl), NodeSource { name: "rarename".into(), visible: true, text: Some(String::new()), ..Default::default() });
        gui.add_plain_node(Some(tmpl), "bar");
        let mut ui = GameUi { gui, ..Default::default() };
        ui.m.gui_root = Some(g);
        ui.m.target_bars[5] = Some(tmpl);
        let plate = super::super::target::PlateFrame {
            id: 1,
            kind: super::super::target::PlateKind::Friend,
            position: [10.0, 60.0],
            rare_name_visible: Some(false),
            rare_name: "Bob".into(),
            info_color: None,
            info: String::new(),
            text: "1/2".into(),
            bar: 0.5,
            bar2: 0.5,
        };
        let mut out = FrameOutput::default();
        out.nameplates = vec![plate.clone(), plate];
        let view = GameView::default();
        apply(&mut ui, &view, &out);
        let h = ui.hud_state.nodes.clone().unwrap();
        let c = h.plates.unwrap();
        assert_eq!(h.plate_pool[1].len(), 2);
        let n0 = ui.gui.nodes.len();
        apply(&mut ui, &view, &out);
        // Reused: no new nodes.
        assert_eq!(ui.gui.nodes.len(), n0);
        let first = ui.gui.nodes[c].children[0];
        assert!(ui.gui.nodes[first].visible);
        assert_eq!(ui.gui.nodes[first].translation, Vec2::new(10.0, 60.0));
        let r = find_node(&ui.gui, first, "rarename").unwrap();
        assert!(!ui.gui.nodes[r].visible);
        assert_eq!(ui.gui.nodes[r].text, Some("Bob".encode_utf16().collect()));
        // The template keeps its own state.
        assert!(ui.gui.nodes[rn].visible);
        out.nameplates.truncate(1);
        apply(&mut ui, &view, &out);
        let second = ui.gui.nodes[c].children[1];
        assert!(!ui.gui.nodes[second].visible);
    }

    #[test]
    fn widget_texts_of_chat() {
        let mut ui = GameUi::default();
        let n = ui.gui.add_plain_node(None, "");
        let w = ui.gui.add_widget(n, &WidgetSource::default());
        ui.m.chat = Some(w);
        ui.hud_state.chat_draws = vec![super::super::chat::ChatDraw { text: "hi".into(), pos: Vec2::new(3.0, 19.0), color: [1.0, 0.0, 0.0, 1.0] }];
        let mut t = BTreeMap::new();
        widget_texts(&ui, &GameView::default(), &FrameOutput::default(), &mut t);
        let v = &t[&w];
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].stroke_color, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(v[1].color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(v[1].style.size, 10.0);
    }

    /// The four bubble clones carry a SpeechWidget of the ctor's size (0x004e5c90 /
    /// 0x00627c00: 250 wide), so 0x004e65a0 wraps at 220 and a villager's sentence takes a
    /// few lines, not one per word. Skipped without `CW_GAME_DIR`.
    #[test]
    fn bubble_widgets_are_250_wide_and_wrap_sentences() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let mut ui = GameUi::new(gui, &mut plx, &GameView::default());
        ui.fonts = TextMeasure::new(dir.clone());
        apply(&mut ui, &GameView::default(), &FrameOutput::default());
        let h = ui.hud_state.nodes.clone().unwrap();
        assert_eq!(h.bubbles.len(), 4);
        for &(_, w) in &h.bubbles {
            assert_eq!(super::super::bubbles::bubble_width(&ui.gui, w), 250.0);
        }
        let mut b = super::super::speech::SpeechWidget::default();
        let mut line = Vec::new();
        super::super::textdb::push_words(&mut line, "Have you ever been to the Hyara Planes? I heard there are ogres.", super::super::textdb::WHITE);
        b.set_lines(&vec![line]);
        b.elapsed_ms = 100_000;
        let w = h.bubbles[0].1;
        let runs = super::super::bubbles::layout(&b, super::super::bubbles::bubble_width(&ui.gui, w), Vec2::ZERO, &|t| ui.fonts.measure(t, super::super::speech::OPTION_SIZE, 0.0));
        let mut rows: Vec<f32> = runs.iter().map(|r| r.origin[1]).collect();
        rows.dedup();
        assert!(rows.len() >= 2 && rows.len() <= 4, "{} lines: {runs:?}", rows.len());
        assert!(runs.len() > 2 * rows.len());
    }
}
