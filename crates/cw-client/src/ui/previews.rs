//! The character and world select screens: `cube::CharacterPreviewWidget` (vtable
//! 0x006fcedc, ctor 0x00424e80, 0x168 bytes, slot 1 0x00425450), `cube::WorldPreviewWidget`
//! (vtable 0x0071e09c, ctor 0x00605a20, 0x16c bytes, slot 1 0x00605ae0), their builders
//! 0x0049d650 / 0x004a23d0, and the vertical carousel that `GameController::update`
//! 0x00488ee0 runs over them (0x00489d4f..0x0048b2cb, Ghidra lines ~636..1180 of the
//! decompilation), plus the drag scrolling of `onMouseMove` 0x0047ea00 and the reset in
//! `onMouseDown` 0x0047b600.
//!
//! Tier B. The previews are real nodes of the widget tree: each entry is a `blackwidget`
//! clone (600x220) in the select screen root with the preview widget as its content. The
//! carousel moves those nodes (`Widget::setPosition` 0x0062a650), selects the hovered entry
//! (`GC+0x800a0c` / `GC+0x800a10`) and places the screen's Select / 20x20 / Delete buttons
//! next to the selected entry. What each preview draws (texts, the 3D model) is returned as
//! data in [`PreviewsFrame`].
//!
//! # Address map
//!
//! | Range | Port |
//! |---|---|
//! | 0x0049d650 | [`rebuild_character_previews`] |
//! | 0x004a23d0 | [`rebuild_world_previews`] |
//! | 0x00489d4f..0x0048a818 | [`frame`], character carousel ([`carousel`], [`character_buttons`]) |
//! | 0x0048a818..0x0048b2cb | [`frame`], world carousel ([`carousel`], [`world_buttons`]) |
//! | 0x0047ea2b..0x0047ecc3 | [`on_mouse_move`] |
//! | 0x0047b63f, 0x0047b70f | [`on_mouse_down`] |
//! | 0x00425450 | [`character_preview_content`], [`character_preview_update`], [`PreviewModels::creatures`] ([`preview_view`], [`preview_projection`]) |
//! | 0x00605ae0 | [`world_preview_content`], [`world_preview_yaw_step`], [`walking_pose_step`], [`PreviewModels::world_cards`] ([`world_model_matrix`]) |
//!
//! The models are drawn after the GUI (`cw_render::passes::GuiCreature`), not at the widget's
//! slot-1 point inside the traversal as the original does: whatever the GUI draws later over
//! a card (the Select button, the cursor) is drawn under the model here.

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_ui::widget::{Gui, NodeId, WidgetId, WidgetSource};
use glam::{IVec2, Vec2};

use super::members::{GcMembers, clone_subtree};
use super::{CharacterEntry, GameView, WorldEntry};
use crate::player::{lerp_factor, lerp_scalar};

/// The font every preview text uses (`L"resource1.dat"`).
pub const PREVIEW_FONT: &str = "resource1.dat";
/// `FontEngine::drawText` 0x00639b30 arguments shared by every preview text: spacing 0,
/// line spacing 2.0, pixel snap 1.
pub const TEXT_SPACING: f32 = 0.0;
/// See [`TEXT_SPACING`].
pub const TEXT_LINE_SPACING: f32 = 2.0;

/// The size of each preview panel (`0x0062c570(600, 220, 1)` in 0x0049d650 / 0x004a23d0).
pub const PANEL_SIZE: Vec2 = Vec2::new(600.0, 220.0);
/// The panel's initial x (`setPosition(300, y, 1)`).
pub const PANEL_X: f32 = 300.0;
/// The first panel's y (`local_18 = 0x32`).
pub const PANEL_Y0: i32 = 50;
/// The y step between panels (`+= 0xe6`).
pub const PANEL_STEP: i32 = 230;

// ---------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------

/// One `CharacterPreviewWidget` (0x168 bytes): the base `plasma::Widget` (0x160 bytes)
/// plus +0x160 the `cube::Creature*` it shows (null for "New character") and +0x164 the
/// GameController.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterPreview {
    /// The `blackwidget` clone stored in `GC+0x800978` (its widget, node+0x40, is the one
    /// the carousel moves and hover-tests).
    pub panel: NodeId,
    /// The preview widget (the panel's content).
    pub widget: Option<WidgetId>,
    /// +0x160: index into `GC+0x800984` (the saved characters), `None` for "New character".
    pub character: Option<usize>,
}

/// One `WorldPreviewWidget` (0x16c bytes): +0x160 the `cube::WorldInfo*` (null for "New
/// world"), +0x164 the model yaw in degrees (float, 0 at construction), +0x168 the
/// GameController.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldPreview {
    /// The `blackwidget` clone stored in `GC+0x8009c0`.
    pub panel: NodeId,
    /// The preview widget.
    pub widget: Option<WidgetId>,
    /// +0x160: index into `GC+0x8009dc` (the saved worlds), `None` for "New world".
    pub world: Option<usize>,
    /// +0x164: the world model's yaw (degrees).
    pub yaw: f32,
}

/// The carousel state of the two select screens (GameController fields and two statics).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Previews {
    /// `GC+0x800978`: the character previews, top to bottom (the panel nodes are mirrored
    /// into [`GcMembers::character_previews`]).
    pub characters: Vec<CharacterPreview>,
    /// `GC+0x8009c0`: the world previews.
    pub worlds: Vec<WorldPreview>,
    /// `GC+0x8009cc`: `std::map<Node*, unsigned>`, preview panel → world index.
    pub world_index_of_panel: BTreeMap<NodeId, i32>,
    /// `GC+0x8009d4`: `std::map<int, Node*>`, world index → preview panel.
    pub world_panel_of_index: BTreeMap<i32, NodeId>,
    /// `GC+0x800a04`: the character list's scroll velocity (float; drag adds, decays with
    /// `lerpRepeat(0.005)`; zeroed by every mouse press and whenever the screen is off).
    pub character_velocity: f32,
    /// `GC+0x800a08`: the world list's scroll velocity.
    pub world_velocity: f32,
    /// Static 0x0076b04c: the smoothed cursor y of `onMouseMove` (reset to the cursor y by
    /// `onMouseDown`).
    pub smoothed_cursor_y: f32,
    /// Statics 0x0076b090 (the last `timeGetTime()` of `onMouseMove`) and 0x0076b094 bit 0
    /// (initialised): `None` before the first move.
    pub last_move_time: Option<u32>,
}

// ---------------------------------------------------------------------------------------
// Small Gui helpers (the plasma calls the original makes)
// ---------------------------------------------------------------------------------------

fn visible(gui: &Gui, n: Option<NodeId>) -> bool {
    n.is_some_and(|n| gui.nodes[n].visible)
}

/// `0x00411a90(v)` on a node.
fn set_visible(gui: &mut Gui, n: Option<NodeId>, v: bool) {
    if let Some(n) = n {
        gui.nodes[n].visible = v;
    }
}

/// `node+0x40`: the node's widget.
fn widget_of(gui: &Gui, n: Option<NodeId>) -> Option<WidgetId> {
    n.and_then(|n| gui.nodes[n].widget)
}

/// `Node::setParent` 0x00636950: detaches `n` from its parent and, when `parent` is given,
/// appends it as the last (topmost) child.
fn set_parent(gui: &mut Gui, n: NodeId, parent: Option<NodeId>) {
    if let Some(old) = gui.nodes[n].parent {
        gui.nodes[old].children.retain(|&c| c != n);
    }
    gui.nodes[n].parent = parent;
    if let Some(p) = parent {
        gui.nodes[p].children.push(n);
    }
}

/// `Node::destroyChildren` 0x00632870: every child is deleted (`0x006504e0`). The arena
/// keeps the slots; they are only unlinked here.
fn destroy_children(gui: &mut Gui, n: NodeId) {
    let children = std::mem::take(&mut gui.nodes[n].children);
    for c in children {
        gui.nodes[c].parent = None;
        gui.nodes[c].visible = false;
    }
}

/// `0x0062f660` / `0x0062f630`: the widget's position y / x.
fn pos(gui: &Gui, w: WidgetId) -> Vec2 {
    gui.get_position(w)
}

/// `W / 2` as `cdq; sub eax, edx; sar eax, 1` (toward zero).
fn half(v: i32) -> i32 {
    v / 2
}

// ---------------------------------------------------------------------------------------
// Rebuild (0x0049d650 / 0x004a23d0)
// ---------------------------------------------------------------------------------------

/// One preview panel: `blackwidget` cloned into `root` (0x00636040 plus the attribute copies
/// 0x00636b70 / 0x006368e0), sized 600x220 (0x0062c570), the preview widget attached as its
/// content (0x00631460), placed at (300, y) (0x0062a650). Returns (panel, preview widget).
fn build_panel(gui: &mut Gui, m: &GcMembers, root: Option<NodeId>, class: cw_ui::widget::GameWidgetClass, y: i32) -> (NodeId, WidgetId) {
    let panel = match m.blackwidget {
        Some(t) => clone_subtree(gui, t, root),
        None => gui.add_plain_node(root, "blackwidget"),
    };
    // Assumption: the copied Display attribute makes the clone visible (the template is
    // hidden; the screen root's visibility gates the whole list).
    gui.nodes[panel].visible = true;
    if gui.nodes[panel].widget.is_none() {
        gui.add_widget(panel, &WidgetSource::default());
    }
    let pw = gui.nodes[panel].widget.unwrap();
    gui.set_size(pw, PANEL_SIZE, true);
    // `Engine::createNode(0, 0, 0, 0, L"")` 0x0064f4e0 + the widget ctor on it.
    let content = gui.add_plain_node(Some(panel), "");
    let w = gui.add_game_widget(content, &WidgetSource::default(), cw_ui::widget::GameWidget::new(class));
    // `Node::attachWidget(blackwidget name, widget, 1)` 0x00631460: the preview widget fills
    // the panel (`setRect(0, 0, getSize(panel))`), so its slot-1 texts (origin at the widget's
    // top-left) land inside the card.
    super::members::attach_content(gui, Some(panel), w);
    gui.set_position_xy(pw, PANEL_X, y as f32, true);
    (panel, w)
}

/// `GameController::rebuildCharacterPreviews` `Cube.exe 0x0049d650`: one preview per saved
/// character (`GC+0x800984`) plus "New character" last, in the character select screen
/// (`GC+0x800888`). Fills [`Previews::characters`] and [`GcMembers::character_previews`].
pub fn rebuild_character_previews(gui: &mut Gui, m: &mut GcMembers, st: &mut Previews, game: &GameView) {
    // 0x0049d67a: detach Select / 20x20 / Delete.
    for b in [m.char_select, m.char_small, m.char_delete].into_iter().flatten() {
        set_parent(gui, b, None);
    }
    let root = m.char_select_root;
    if let Some(r) = root {
        destroy_children(gui, r);
    }
    st.characters.clear();
    m.character_previews.clear();
    let mut y = PANEL_Y0;
    // `for (i = 0; i < count + 1; i++)`, the count re-read every pass.
    let mut i = 0usize;
    while i < game.characters.len() + 1 {
        let (panel, w) = build_panel(gui, m, root, cw_ui::widget::GameWidgetClass::CharacterPreview, y);
        let character = (i < game.characters.len()).then_some(i);
        st.characters.push(CharacterPreview { panel, widget: Some(w), character });
        m.character_previews.push(panel);
        y += PANEL_STEP;
        i += 1;
    }
    // 0x0049d8bb: `0x00635700` refreshes the subtree; the buttons go back on top.
    for b in [m.char_select, m.char_small, m.char_delete].into_iter().flatten() {
        set_parent(gui, b, root);
    }
}

/// Whether a world is a server's (`name.substr(0, 7) == "online_"`, 0x004a24d7: a name of
/// fewer than 7 bytes is not).
pub fn is_online_world(name: &str) -> bool {
    name.as_bytes().starts_with(b"online_")
}

/// `GameController::rebuildWorldPreviews` `Cube.exe 0x004a23d0`: one preview per saved world
/// (`GC+0x8009dc`) whose `online_` prefix matches the multiplayer flag (`GC+0x8009b0`),
/// plus "New world" last, in the world select screen (`GC+0x80088c`). Fills
/// [`Previews::worlds`] and the two maps.
pub fn rebuild_world_previews(gui: &mut Gui, m: &GcMembers, st: &mut Previews, game: &GameView) {
    for b in [m.world_select, m.world_small, m.world_delete].into_iter().flatten() {
        set_parent(gui, b, None);
    }
    let root = m.world_select_root;
    if let Some(r) = root {
        destroy_children(gui, r);
    }
    st.worlds.clear();
    st.world_index_of_panel.clear();
    st.world_panel_of_index.clear();
    let mut y = PANEL_Y0;
    let mut i = 0usize;
    while i < game.worlds.len() + 1 {
        let world = (i < game.worlds.len()).then_some(i);
        let listed = match world {
            Some(k) => game.multiplayer == is_online_world(&game.worlds[k].name),
            None => true,
        };
        if listed {
            let (panel, w) = build_panel(gui, m, root, cw_ui::widget::GameWidgetClass::WorldPreview, y);
            st.worlds.push(WorldPreview { panel, widget: Some(w), world, yaw: 0.0 });
            st.world_index_of_panel.insert(panel, i as i32);
            st.world_panel_of_index.insert(i as i32, panel);
            y += PANEL_STEP;
        }
        i += 1;
    }
    for b in [m.world_select, m.world_small, m.world_delete].into_iter().flatten() {
        set_parent(gui, b, root);
    }
}

// ---------------------------------------------------------------------------------------
// The carousel (update 0x00489d4f..0x0048b2cb)
// ---------------------------------------------------------------------------------------

/// The shared scroll pass of both lists (0x00489df4..0x0048a3a9 for characters,
/// 0x0048a8b9..0x0048ae8c for worlds). `panels` are the list's nodes top to bottom;
/// `client` is `(GC+0x11c, GC+0x120)`; `velocity` is `GC+0x800a04` / `+0x800a08`.
/// Calls `on_hover(k)` for every panel whose widget is under the cursor (0x006294c0), in
/// list order. The caller has checked that the left button is up and the list non-empty.
pub fn carousel(gui: &mut Gui, panels: &[NodeId], client: IVec2, dt: i32, velocity: f32, mut on_hover: impl FnMut(&mut Gui, usize)) {
    let ws: Vec<WidgetId> = panels.iter().filter_map(|&p| gui.nodes[p].widget).collect();
    if ws.len() != panels.len() || ws.is_empty() {
        return;
    }
    let (first, last) = (ws[0], ws[ws.len() - 1]);
    let (cw, ch) = (client.x, client.y);
    // 0x00489df4: the first panel's y, the top target 10 and the bottom target
    // `(H - 10) - lastHeight`.
    let mut first_y = pos(gui, first).y;
    let mut top = 10.0f32;
    let mut bottom = (ch - 10) as f32 - gui.height(last);
    // 0x00489e6b: the list's extent `(int)((last.y + last.h) - first.y)` (cvttss2si).
    let span = ((pos(gui, last).y + gui.height(last)) - pos(gui, first).y) as i32;
    if span < ch - 20 {
        // A short list is centred: top `H/2 - span/2`, bottom `(span/2 + H/2) - last.h`.
        let (h2, s2) = (half(ch), half(span));
        top = (h2 - s2) as f32;
        bottom = ((s2 + h2) as f32) - gui.height(last);
    }
    let bottom_target = bottom;
    // Re-centres every panel horizontally and shifts it by `delta` (0x00489ff0).
    let shift = |gui: &mut Gui, delta: f32| {
        for &w in &ws {
            let y = pos(gui, w).y + delta;
            let x = half(cw) as f32 - gui.width(w) * 0.5;
            gui.set_position_xy(w, x, y, true);
        }
    };
    // 0x00489f5c: `first.y > top` (comiss/jbe: NaN skips): ease the first panel up to the
    // top target and move the list with it.
    if first_y > top {
        lerp_scalar(&mut first_y, top, dt, 0.01);
        let delta = first_y - pos(gui, first).y;
        shift(gui, delta);
    }
    // 0x0048a0b6: `top > first.y && bottom > last.y`: ease the last panel down to the bottom
    // target.
    let mut last_y = pos(gui, last).y;
    let first_now = pos(gui, first).y;
    if top > first_now && bottom > last_y {
        lerp_scalar(&mut last_y, bottom_target, dt, 0.01);
        let delta = last_y - pos(gui, last).y;
        shift(gui, delta);
    }
    // 0x0048a252: the velocity step and the hover selection.
    for (k, &w) in ws.iter().enumerate() {
        let y = pos(gui, w).y + (velocity * 0.001) * dt as f32;
        let x = half(cw) as f32 - gui.width(w) * 0.5;
        gui.set_position_xy(w, x, y, true);
        if gui.widget_under_cursor(w, gui.widgets[w].node) {
            on_hover(gui, k);
        }
    }
}

/// Places a screen's Select / 20x20 / Delete buttons around the selected panel's widget
/// `sel` (0x0048a3ed..0x0048a6e2, 0x0048aed5..0x0048b190): Select at the bottom-right corner
/// inset 20, the small button at the top-right corner inset 20 (both shown), Delete 20 px
/// right of the panel at its top (visibility untouched: the small button's handler shows it).
fn place_buttons(gui: &mut Gui, sel: WidgetId, select: Option<NodeId>, small: Option<NodeId>, delete: Option<NodeId>) {
    let p = pos(gui, sel);
    let bottom = p.y + gui.height(sel);
    let right = p.x + gui.width(sel);
    if let Some(b) = widget_of(gui, select) {
        let y = (bottom - gui.height(b)) - 20.0;
        let x = (right - gui.width(b)) - 20.0;
        gui.set_position_xy(b, x, y, true);
    }
    set_visible(gui, select, true);
    let p = pos(gui, sel);
    let right = p.x + gui.width(sel);
    if let Some(b) = widget_of(gui, small) {
        let y = pos(gui, sel).y + 20.0;
        let x = (right - gui.width(b)) - 20.0;
        gui.set_position_xy(b, x, y, true);
    }
    set_visible(gui, small, true);
    if let Some(b) = widget_of(gui, delete) {
        let y = pos(gui, sel).y;
        let x = (pos(gui, sel).x + gui.width(sel)) + 20.0;
        gui.set_position_xy(b, x, y, true);
    }
}

/// The tail shared by both screens (0x0048a717 / 0x0048b1c5): the "New" entry hides Delete
/// and the small button and captions Select "Create", else "Select" (0x00636ad0 on the
/// Select node).
fn select_caption(gui: &mut Gui, is_new: bool, select: Option<NodeId>, small: Option<NodeId>, delete: Option<NodeId>) {
    if is_new {
        set_visible(gui, delete, false);
        set_visible(gui, small, false);
    }
    // `Node::setText` 0x00636ad0 on the Select node (every TextShape of the clone; a
    // TextShape whose string is already `c` is left alone).
    if let Some(n) = select {
        let c = if is_new { "Create" } else { "Select" };
        super::present_hud::set_all_text(gui, n, c);
    }
}

/// The character screen's button pass (0x0048a3af..0x0048a7c8).
pub fn character_buttons(gui: &mut Gui, m: &GcMembers, st: &Previews, game: &GameView) {
    let sel = game.selected_character;
    // `!leftDown && sel >= 0 && sel < previews.size()`.
    let target = if !gui.left_down && sel >= 0 && (sel as usize) < st.characters.len() {
        gui.nodes[st.characters[sel as usize].panel].widget
    } else {
        None
    };
    match target {
        Some(w) => place_buttons(gui, w, m.char_select, m.char_small, m.char_delete),
        None => {
            set_visible(gui, m.char_select, false);
            set_visible(gui, m.char_small, false);
            set_visible(gui, m.char_delete, false);
        }
    }
    // `sel >= characters.size()` (signed compare, `jl`).
    select_caption(gui, sel >= game.characters.len() as i32, m.char_select, m.char_small, m.char_delete);
}

/// The world screen's button pass (0x0048ae92..0x0048b276): the selected panel is looked
/// up in `GC+0x8009d4` (`operator[]`, which inserts a null entry on a miss; the lookup here
/// does not insert).
pub fn world_buttons(gui: &mut Gui, m: &GcMembers, st: &Previews, game: &GameView) {
    let sel = game.selected_world;
    let target = if !gui.left_down {
        st.world_panel_of_index.get(&sel).and_then(|&p| gui.nodes[p].widget)
    } else {
        None
    };
    match target {
        Some(w) => place_buttons(gui, w, m.world_select, m.world_small, m.world_delete),
        None => {
            set_visible(gui, m.world_select, false);
            set_visible(gui, m.world_small, false);
            set_visible(gui, m.world_delete, false);
        }
    }
    select_caption(gui, sel >= game.worlds.len() as i32, m.world_select, m.world_small, m.world_delete);
}

// ---------------------------------------------------------------------------------------
// Preview content (slot 1 bodies, text part)
// ---------------------------------------------------------------------------------------

/// One `FontEngine::drawText` 0x00639b30 call of a preview (font [`PREVIEW_FONT`], spacing
/// 0, line spacing 2, pixel snap 1). Each label is drawn twice: a stroke pass (radius 3,
/// black stroke) then a fill pass (radius 0).
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewText {
    /// The text.
    pub text: String,
    /// Origin in the preview widget's space.
    pub pos: Vec2,
    /// Font size.
    pub size: f32,
    /// Stroke radius (3 for the stroke pass, 0 for the fill pass).
    pub stroke_radius: f32,
    /// Fill colour (RGBA).
    pub color: [f32; 4],
    /// Stroke colour (RGBA).
    pub stroke_color: [f32; 4],
    /// `cw_ui::font::align` flags (5 = centred both ways, 0x10 = wrap).
    pub flags: u32,
    /// Wrap width (180, or -1 when unused).
    pub wrap_width: f32,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// The name colour (0.25, 1, 1, 1).
const NAME_COLOR: [f32; 4] = [0.25, 1.0, 1.0, 1.0];
/// The specialization / seed colour (0.5, 0.5, 1, 1).
const DETAIL_COLOR: [f32; 4] = [0.5, 0.5, 1.0, 1.0];

/// The two passes of one label.
fn label(out: &mut Vec<PreviewText>, text: &str, pos: Vec2, size: f32, fill: [f32; 4], flags: u32, wrap: f32) {
    out.push(PreviewText {
        text: text.to_owned(),
        pos,
        size,
        stroke_radius: 3.0,
        color: WHITE,
        stroke_color: BLACK,
        flags,
        wrap_width: wrap,
    });
    out.push(PreviewText {
        text: text.to_owned(),
        pos,
        size,
        stroke_radius: 0.0,
        color: fill,
        stroke_color: [0.0; 4],
        flags,
        wrap_width: wrap,
    });
}

/// The race word of 0x00426ffa, by entity type (13/14 frogman and 6 have none: nothing is
/// appended, an original gap kept here).
pub fn race_name(entity_type: i32) -> Option<&'static str> {
    Some(match entity_type {
        0 | 1 => "Elf",
        2 | 3 => "Human",
        4 | 5 => "Goblin",
        7 | 8 => "Lizardman",
        9 | 10 => "Dwarf",
        11 | 12 => "Orc",
        15 | 16 => "Undead",
        _ => return None,
    })
}

/// The class word of 0x00427062 (entity+0x130).
pub fn class_name(class: u8) -> Option<&'static str> {
    Some(match class {
        1 => "Warrior",
        2 => "Ranger",
        3 => "Mage",
        4 => "Rogue",
        _ => return None,
    })
}

/// The specialization word of 0x004273f7 (class, then entity+0x131 = 0 or 1).
pub fn specialization_name(class: u8, spec: u8) -> Option<&'static str> {
    let pair = match class {
        1 => ["Berserker", "Guardian"],
        2 => ["Sniper", "Scout"],
        3 => ["Fire Mage", "Water Mage"],
        4 => ["Assassin", "Ninja"],
        _ => return None,
    };
    match spec {
        0 => Some(pair[0]),
        1 => Some(pair[1]),
        _ => None,
    }
}

/// `CharacterPreviewWidget::update` `Cube.exe 0x00425450`, text part. `width` is the
/// widget's width (0x0062f600). "New character" is centred at `((int)(width/2), 30)`;
/// otherwise the name (200, 30), `"LVL <level> <race> <class>"` (200, 55) and
/// `"Specialization: <name>"` (200, 75).
pub fn character_preview_content(entry: Option<&CharacterEntry>, width: f32) -> Vec<PreviewText> {
    let mut out = Vec::new();
    match entry {
        None => {
            // `(float)(int)(width * 0.5)`.
            let x = ((width * 0.5) as i32) as f32;
            label(&mut out, "New character", Vec2::new(x, 30.0), 14.0, WHITE, 5, 180.0);
        }
        Some(c) => {
            label(&mut out, &c.name, Vec2::new(200.0, 30.0), 14.0, NAME_COLOR, 0x10, 180.0);
            let mut s = format!("LVL {} ", c.level);
            if let Some(r) = race_name(c.entity_type) {
                s.push_str(r);
            }
            s.push(' ');
            if let Some(k) = class_name(c.class) {
                s.push_str(k);
            }
            label(&mut out, &s, Vec2::new(200.0, 55.0), 12.0, WHITE, 0, -1.0);
            let mut s = String::from("Specialization: ");
            if let Some(k) = specialization_name(c.class, c.specialization) {
                s.push_str(k);
            }
            label(&mut out, &s, Vec2::new(200.0, 75.0), 10.0, DETAIL_COLOR, 0, -1.0);
        }
    }
    out
}

/// `wostream << float` with the default flags (precision 6, `%g`).
pub fn format_float_g(v: f32) -> String {
    let v = f64::from(v);
    if v == 0.0 {
        return "0".into();
    }
    if !v.is_finite() {
        return if v.is_nan() { "nan".into() } else if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    // %g: scientific when exp < -4 or exp >= precision; trailing zeros removed.
    let s = format!("{:.5e}", v);
    let (mant, e) = s.split_once('e').unwrap();
    let e: i32 = e.parse().unwrap();
    if !(-4..6).contains(&e) {
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        return format!("{mant}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    let decimals = (5 - e).max(0) as usize;
    let f = format!("{:.*}", decimals, v);
    if f.contains('.') { f.trim_end_matches('0').trim_end_matches('.').to_string() } else { f }
}

/// `WorldPreviewWidget::update` `Cube.exe 0x00605ae0`, text part: "New world" centred, or the
/// name (200, 30), `"Explored: <(float)explored> km²"` (200, 55), `"Seed: <seed>"` (200, 80).
pub fn world_preview_content(entry: Option<&WorldEntry>, width: f32) -> Vec<PreviewText> {
    let mut out = Vec::new();
    match entry {
        None => {
            let x = ((width * 0.5) as i32) as f32;
            label(&mut out, "New world", Vec2::new(x, 30.0), 14.0, WHITE, 5, 180.0);
        }
        Some(w) => {
            label(&mut out, &w.name, Vec2::new(200.0, 30.0), 14.0, NAME_COLOR, 0x10, 180.0);
            let s = format!("Explored: {} km\u{b2}", format_float_g(w.explored as f32));
            label(&mut out, &s, Vec2::new(200.0, 55.0), 10.0, WHITE, 0, -1.0);
            let s = format!("Seed: {}", w.seed);
            label(&mut out, &s, Vec2::new(200.0, 80.0), 10.0, DETAIL_COLOR, 0, -1.0);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Preview model pose (slot 1 bodies, simulation part)
// ---------------------------------------------------------------------------------------

/// The creature-only fields the character preview animates (entity bytes are edited in
/// place).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PreviewPose {
    /// `creature+0x137c`: the body yaw the renderer draws (degrees).
    pub yaw: f32,
    /// `creature+0x118c`: the animation clock (`+= frame_ms · 0.0075`).
    pub anim_time: f32,
    /// `creature+0x1188`: an animation blend (0.2 while walking, eased to 0 otherwise).
    pub blend: f32,
}

/// The float lerp factor inlined in both slot 1 bodies: `f = 0; n times f = (1 - f)·0.01 + f`
/// in single precision (unlike 0x004ac150, which is double).
fn lerp_factor_f32(n: i32) -> f32 {
    let mut f = 0.0f32;
    let mut i = 0;
    while i < n {
        f = (1.0 - f) * 0.01 + f;
        i += 1;
    }
    f
}

/// `0x00605950(p, target, n, rate)`: `n` times `p += (target - p)·rate` per axis.
pub fn ease_vec3(p: &mut [f32; 3], target: [f32; 3], n: i32, rate: f32) {
    let mut i = 0;
    while i < n {
        let d = [target[0] - p[0], target[1] - p[1], target[2] - p[2]];
        p[0] += d[0] * rate;
        p[1] = d[1] * rate + p[1];
        p[2] = d[2] * rate + p[2];
        i += 1;
    }
}

/// The not-hovered yaw ease of both slot 1 bodies (0x00425a6c..0x00425c26): turns `yaw`
/// toward 0 by the shortest way, `yaw += asin(clamp(sin 0·cos y − cos 0·sin y)) · (f·180/π)`.
pub fn ease_yaw_to_zero(yaw: f32, frame_ms: i32) -> f32 {
    let f = lerp_factor_f32(frame_ms);
    // `(float)((double)(yaw / 180) · π)`.
    let rad = ((f64::from(yaw / 180.0)) * std::f64::consts::PI) as f32;
    let s = cw_math::sin(f64::from(rad)) as f32;
    let c = cw_math::cos(f64::from(rad)) as f32;
    let rad0 = ((f64::from(0.0f32 / 180.0)) * std::f64::consts::PI) as f32;
    let s0 = cw_math::sin(f64::from(rad0)) as f32;
    let c0 = cw_math::cos(f64::from(rad0)) as f32;
    let mut v = (f64::from(s0) * f64::from(c) - f64::from(c0) * f64::from(s)) as f32;
    // `if (v > 1) v = 1; else if (-1 > v) v = -1;`
    if v > 1.0 {
        v = 1.0;
    } else if -1.0 > v {
        v = -1.0;
    }
    // libm_sse2_asin_precise; the platform asin here.
    let a = f64::from(f64::from(v).asin() as f32);
    let k = (f64::from(f) * 180.0) / std::f64::consts::PI;
    yaw + (a * k) as f32
}

/// `CharacterPreviewWidget::update` `Cube.exe 0x00425450`, simulation part, on the shown
/// creature (entity block `e` = creature+0x10, `pose` its creature-only fields). Zeroes
/// entity+0x118 (roll time), +0x11c (stun time), +0x114 (flags, u16), +0x4c (physics flags),
/// +0x58 (mode); advances the animation clock; hovered (0x006294c0 on the preview widget):
/// spins (`yaw -= ms·0.05`), blend 0.2, acceleration (entity+0x30) eased to (0, 8, 0) and
/// velocity (entity+0x24) to (0, 3, 0) at 0.01 per ms — the walk cycle; else the yaw eases
/// back to 0, the blend to 0, acceleration and velocity to 0.
pub fn character_preview_update(e: &mut EntityData, pose: &mut PreviewPose, hovered: bool, frame_ms: i32) {
    let b = &mut e.0;
    b[0x118..0x11c].fill(0);
    b[0x11c..0x120].fill(0);
    b[0x114..0x116].fill(0);
    b[0x4c..0x50].fill(0);
    pose.anim_time = frame_ms as f32 * 0.0075 + pose.anim_time;
    b[0x58] = 0;
    let (accel_target, vel_target) = if hovered {
        pose.yaw -= frame_ms as f32 * 0.05;
        pose.blend = 0.2;
        ([0.0, 8.0, 0.0], [0.0, 3.0, 0.0])
    } else {
        pose.yaw = ease_yaw_to_zero(pose.yaw, frame_ms);
        // 0x00425c2e: `blend = (0 - blend)·0.01 + blend`, `frame_ms` times.
        let mut i = 0;
        while i < frame_ms {
            pose.blend = (0.0 - pose.blend) * 0.01 + pose.blend;
            i += 1;
        }
        ([0.0; 3], [0.0; 3])
    };
    let mut acc = crate::player::vec3_at(b, 0x30);
    ease_vec3(&mut acc, accel_target, frame_ms, 0.01);
    crate::player::set_vec3(b, 0x30, acc);
    let mut vel = crate::player::vec3_at(b, 0x24);
    ease_vec3(&mut vel, vel_target, frame_ms, 0.01);
    crate::player::set_vec3(b, 0x24, vel);
}

/// `WorldPreviewWidget::update` `Cube.exe 0x00605ae0`, the world model's yaw (+0x164), only
/// run when the `WorldInfo` has a model (`WorldInfo+4 != 0`): hovered spins
/// (`yaw -= ms·0.05`), else eases back to 0. The model is drawn at yaw + 45°, scaled 0.05,
/// centred on its size (`model+0x44..+0x4c · -0.5`).
pub fn world_preview_yaw_step(yaw: &mut f32, hovered: bool, frame_ms: i32) {
    *yaw = if hovered { *yaw - frame_ms as f32 * 0.05 } else { ease_yaw_to_zero(*yaw, frame_ms) };
}

/// The selected character walking on its current world's preview (0x00605ae0, when the
/// world's seed and name equal [`GameView::current_world`]; the creature is
/// `characters[GC+0x800a0c]`, 0x00487e30): clock `+= ms·0.0075`, the render position copied
/// from the position (`creature+0x1350.. = creature+0x10..`, the caller's), blend 0.2,
/// acceleration eased to (0, 8, 0) and velocity to (0, 3, 0) at 0.01 per ms.
pub fn walking_pose_step(e: &mut EntityData, pose: &mut PreviewPose, frame_ms: i32) {
    pose.anim_time = frame_ms as f32 * 0.0075 + pose.anim_time;
    pose.blend = 0.2;
    let b = &mut e.0;
    let mut acc = crate::player::vec3_at(b, 0x30);
    ease_vec3(&mut acc, [0.0, 8.0, 0.0], frame_ms, 0.01);
    crate::player::set_vec3(b, 0x30, acc);
    let mut vel = crate::player::vec3_at(b, 0x24);
    ease_vec3(&mut vel, [0.0, 3.0, 0.0], frame_ms, 0.01);
    crate::player::set_vec3(b, 0x24, vel);
}

// ---------------------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------------------

/// What one preview shows this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewFrame {
    /// The panel node (a `blackwidget` clone; the renderer draws it from the tree).
    pub panel: NodeId,
    /// The preview widget.
    pub widget: Option<WidgetId>,
    /// The panel widget's position and size (after this frame's carousel step).
    pub pos: Vec2,
    /// See [`PreviewFrame::pos`].
    pub size: Vec2,
    /// The preview widget is under the cursor (drives the pose: call
    /// [`character_preview_update`] / [`world_preview_yaw_step`] with it).
    pub hovered: bool,
    /// Index into the saved characters / worlds; `None` for the "New" entry.
    pub index: Option<usize>,
    /// The text draws, in order, in the preview widget's space.
    pub texts: Vec<PreviewText>,
    /// Worlds only: the selected character (`GC+0x800a0c`, when in range) is drawn walking
    /// on this world (its seed and name are the player's current world).
    pub walking_character: Option<usize>,
}

/// The 3D model draw of both previews, in the preview widget's frame (for the renderer):
/// model centred at widget origin + (100, 110), perspective with `1/tan(22.5°)` and the
/// viewport aspect, camera pitch −95° (−1.6580628 rad) and yaw 45°, distance 17; light
/// direction (0.5, 1, 1.5)/√3.5 for characters, (0.5, 0.6, 3.5)/√12.86 for worlds, colours
/// (0.2, 0.3, 0.4, 1) and (0.4, 0.4, 0.4, 1); fog 1e9 (0x4e6e6b28). The creature is drawn
/// by 0x004128f0 with `GC+0x800580`, the world model by 0x004e6df0 ([`PreviewModels`]).
pub const MODEL_OFFSET: Vec2 = Vec2::new(100.0, 110.0);

/// The select screens this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewsFrame {
    /// The character previews (when the character select screen is visible).
    pub characters: Vec<PreviewFrame>,
    /// The world previews (when the world select screen is visible).
    pub worlds: Vec<PreviewFrame>,
}

fn preview_frame(gui: &Gui, panel: NodeId, widget: Option<WidgetId>, index: Option<usize>, texts: Vec<PreviewText>) -> PreviewFrame {
    let pw = gui.nodes[panel].widget;
    PreviewFrame {
        panel,
        widget,
        pos: pw.map_or(Vec2::ZERO, |w| gui.get_position(w)),
        size: pw.map_or(Vec2::ZERO, |w| gui.get_size(w)),
        hovered: widget.is_some_and(|w| gui.widget_under_cursor(w, gui.widgets[w].node)),
        index,
        texts,
        walking_character: None,
    }
}

/// The select-screen part of `GameController::update` `Cube.exe 0x00488ee0`
/// (0x00489d4f..0x0048b2cb), after [`super::flow::frame_rules`] (which owns the camera orbit
/// of these screens). `dt` is update's `dt` in ms, `client` `(GC+0x11c, GC+0x120)`.
///
/// Character screen, while it and not "Please wait..." is visible (else `GC+0x800a04 = 0`):
/// with the left button up and a non-empty list, the [`carousel`] (hover selects
/// `GC+0x800a0c`, hiding Delete when the selection changes); then [`character_buttons`];
/// then `GC+0x800a04` decays toward 0 (`lerpRepeat(dt, 0.005)`). The world screen is the
/// same with `GC+0x8009c0`, `GC+0x800a10` (through the panel → index map) and `GC+0x800a08`.
pub fn frame(gui: &mut Gui, m: &GcMembers, st: &mut Previews, game: &mut GameView, dt: i32, client: IVec2) -> PreviewsFrame {
    let wait = visible(gui, m.wait);
    // 0x00489d4f: the character screen.
    if visible(gui, m.char_select_root) && !wait {
        if !gui.left_down && !st.characters.is_empty() {
            let panels: Vec<NodeId> = st.characters.iter().map(|p| p.panel).collect();
            let delete = m.char_delete;
            let sel = &mut game.selected_character;
            carousel(gui, &panels, client, dt, st.character_velocity, |gui, k| {
                // 0x0048a343: `if (k != sel) delete.hide(); sel = k`.
                if k as i32 != *sel {
                    set_visible(gui, delete, false);
                }
                *sel = k as i32;
            });
        }
        character_buttons(gui, m, st, game);
        // 0x0048a7d8: `lerp(&GC+0x800a04, 0, dt, 0.005)`.
        lerp_scalar(&mut st.character_velocity, 0.0, dt, 0.005);
    } else {
        st.character_velocity = 0.0;
    }
    // 0x0048a818: the world screen.
    if visible(gui, m.world_select_root) && !wait {
        if !gui.left_down && !st.worlds.is_empty() {
            let panels: Vec<NodeId> = st.worlds.iter().map(|p| p.panel).collect();
            let delete = m.world_delete;
            let map = &st.world_index_of_panel;
            let sel = &mut game.selected_world;
            carousel(gui, &panels, client, dt, st.world_velocity, |gui, k| {
                // 0x0048ae23: `idx = map[panel]` (an absent key reads 0 after insertion).
                let idx = map.get(&panels[k]).copied().unwrap_or(0);
                if idx != *sel {
                    set_visible(gui, delete, false);
                }
                *sel = idx;
            });
        }
        world_buttons(gui, m, st, game);
        lerp_scalar(&mut st.world_velocity, 0.0, dt, 0.005);
    } else {
        st.world_velocity = 0.0;
    }
    // The widgets' slot 1 (content), for the visible screens.
    let mut out = PreviewsFrame::default();
    if visible(gui, m.char_select_root) {
        for p in &st.characters {
            let width = p.widget.map_or(0.0, |w| gui.width(w));
            let texts = character_preview_content(p.character.and_then(|i| game.characters.get(i)), width);
            out.characters.push(preview_frame(gui, p.panel, p.widget, p.character, texts));
        }
    }
    if visible(gui, m.world_select_root) {
        let sel_char = game.selected_character;
        let walker = (sel_char >= 0 && (sel_char as usize) < game.characters.len()).then_some(sel_char as usize);
        for p in &st.worlds {
            let width = p.widget.map_or(0.0, |w| gui.width(w));
            let entry = p.world.and_then(|i| game.worlds.get(i));
            let texts = world_preview_content(entry, width);
            let mut f = preview_frame(gui, p.panel, p.widget, p.world, texts);
            if let Some(w) = entry {
                if w.seed == game.current_world.seed && w.name == game.current_world.name {
                    f.walking_character = walker;
                }
            }
            out.worlds.push(f);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Preview models (slot 1 bodies, draw part)
// ---------------------------------------------------------------------------------------

/// The walker's offset from the world card's pivot (0x00605ae0: `(x - 100, y + 100)`).
pub const WALKER_OFFSET: Vec2 = Vec2::new(-100.0, 100.0);
/// `Cube.exe 0x004269e1`: the `0x004128f0` call of `CharacterPreviewWidget::update`.
pub const CHARACTER_DRAW_SITE: u32 = 0x0042_69e1;
/// `Cube.exe 0x00607e31`: the `0x004128f0` call of `WorldPreviewWidget::update` (the walker).
pub const WALKER_DRAW_SITE: u32 = 0x0060_7e31;

/// The view matrix both preview widgets build before `0x004128f0` (0x00425d5a..0x00425f4b,
/// 0x00607862..0x00607a50), row-vector convention: identity with row 3 = (0, 0, 17, 1);
/// rows 1/2 turned by the pitch −1.6580628 rad (−95°), rows 0/1 by the yaw 0.7853982 rad
/// (45°), both with `cos`/`sin` in double narrowed to float; then row 3 += `zf`·row 2 with
/// `zf = (float)(−z) · 1/65536` (the 64-bit negation of the creature's z, `creature+0x20`,
/// `fild` to float). The creature's x/y are taken out through the pose's origin arguments.
pub fn preview_view(z: i64) -> [[f32; 4]; 4] {
    let c = (-1.6580628156661987f64).cos() as f32;
    let s = (-1.6580628156661987f64).sin() as f32;
    let s0 = s * 0.0;
    let c0 = c * 0.0;
    let row2 = [c0 - s0, c0 - s, c - s0, c0 - s0];
    // The pitched row 1 (`puStack_8bc`, `fStack_8ac`, `s + c0`, 0).
    let r1 = [s0 + c0, c + s0, s + c0];
    let cy = 0.7853981852531433f64.cos() as f32;
    let sy = 0.7853981852531433f64.sin() as f32;
    let cy0 = cy * 0.0;
    let sy0 = sy * 0.0;
    let row0 = [sy * r1[0] + cy, sy * r1[1] + cy0, sy * r1[2] + cy0, sy * r1[0] + cy0];
    let row1 = [cy * r1[0] - sy, cy * r1[1] - sy0, cy * r1[2] - sy0, cy * r1[0] - sy0];
    let zf = (z.wrapping_neg() as f32) * 1.525_878_9e-5;
    let base = [0.0, 0.0, 17.0, 1.0];
    let mut row3 = [0.0f32; 4];
    for j in 0..4 {
        row3[j] = ((row1[j] * 0.0 + row0[j] * 0.0) + zf * row2[j]) + base[j];
    }
    [row0, row1, row2, row3]
}

/// The projection of both preview widgets (0x00425f4b..0x004261b8): `P · T` with the
/// perspective `P` (x `−(1/tan(π/8))/aspect`, y `1/tan(π/8)`, `P[2] = (0, 0, 1.0001, 1)`,
/// `P[3] = (0, 0, −0.10001, 0)`; the aspect `Engine+0x10c / Engine+0x110`) and the NDC shift
/// `T` (row 3 `(tx, ty, 0, 1)`, `tx = ((px − W/2)/W)·2`, `ty = ((py − H/2)/H)·−2`) that puts
/// the view axis on the pixel `p` (the widget's pivot plus the model offset).
pub fn preview_projection(viewport: IVec2, p: Vec2) -> [[f32; 4]; 4] {
    let (w, h) = (viewport.x as f32, viewport.y as f32);
    let f = 1.0 / (0.39269909262657166f64.tan() as f32);
    let px = -(f / (w / h));
    let tx = ((p.x - w * 0.5) / w) * 2.0;
    let ty = ((p.y - h * 0.5) / h) * -2.0;
    // 0x3f800347 and 0xbdccd20b.
    let (a, b) = (f32::from_bits(0x3f80_0347), f32::from_bits(0xbdcc_d20b));
    [[px, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [tx, ty, a, 1.0], [0.0, 0.0, b, 0.0]]
}

/// The light direction of the character draws: `(0.5, 1, 1.5) · (1 / (float)√3.5)`.
pub fn character_light() -> [f32; 3] {
    let k = 1.0 / (3.5f64.sqrt() as f32);
    [k * 0.5, k, k * 1.5]
}

/// The screen point a preview widget draws around: its node's Transformation pivot through
/// the node's world matrix (`widget+0x148`: the pivot `+0x19c[+0x170]` times `node+0x48..`).
pub fn widget_pivot(gui: &Gui, w: WidgetId) -> Vec2 {
    let n = gui.widgets[w].node;
    gui.node_world(n).transform_point2(gui.nodes[n].pivot)
}

/// A saved character's creature as the select screens keep it (`GC+0x800984[i]`): the entity
/// block, the preview fields of the creature and its pose object (`creature+0x1498`).
#[derive(Clone, Debug)]
pub struct PreviewCreature {
    /// The entity block (`creature+0x10`).
    pub entity: EntityData,
    /// `creature+0x137c`, `+0x118c`, `+0x1188`.
    pub pose: PreviewPose,
    /// The pose object.
    pub state: cw_render::pose::PoseState,
}

/// The creatures of the saved characters, in `GC+0x800984` order.
#[derive(Clone, Debug, Default)]
pub struct PreviewModels {
    /// One per saved character.
    pub creatures: Vec<PreviewCreature>,
    /// The fingerprint of the world preview models last handed to the renderer.
    uploaded: Option<u64>,
    /// How many world preview handles were handed out.
    uploaded_count: usize,
}

impl PreviewModels {
    /// Replaces the creatures (the saved characters were loaded, saved or deleted). A
    /// creature at the same index with the same name keeps its preview fields and pose
    /// object (the original's creature objects live as long as the list entry).
    pub fn set_characters(&mut self, entities: Vec<EntityData>) {
        let old = std::mem::take(&mut self.creatures);
        self.creatures = entities
            .into_iter()
            .enumerate()
            .map(|(i, entity)| match old.get(i) {
                Some(c) if c.entity.0[0x1158..0x1168] == entity.0[0x1158..0x1168] => PreviewCreature { entity, pose: c.pose, state: c.state.clone() },
                _ => PreviewCreature { entity, pose: PreviewPose::default(), state: cw_render::pose::PoseState::default() },
            })
            .collect();
    }

    /// The character cards' slot 1 (0x00425450), draw part, after [`frame`]: for each visible
    /// card with a creature, [`character_preview_update`] (hovered = the card's widget under
    /// the cursor, 0x006294c0) then the pose and draw at the card's pivot + [`MODEL_OFFSET`].
    /// `frame_ms` is `Engine+0xe4`. The world cards are [`PreviewModels::world_cards`].
    pub fn creatures(&mut self, gui: &Gui, frame: &PreviewsFrame, frame_ms: i32, models: &dyn cw_render::pose::ModelSource) -> Vec<cw_render::passes::GuiCreature> {
        let mut out = Vec::new();
        for p in &frame.characters {
            let (Some(i), Some(w)) = (p.index, p.widget) else { continue };
            let Some(c) = self.creatures.get_mut(i) else { continue };
            character_preview_update(&mut c.entity, &mut c.pose, p.hovered, frame_ms);
            let at = widget_pivot(gui, w) + MODEL_OFFSET;
            out.push(pose_draw(c, gui.viewport, at, frame_ms, models, CHARACTER_DRAW_SITE));
        }
        out
    }
}

/// The model handles of the world previews (`WorldInfo+4`), one per saved world index,
/// below the particle cube and the map's tile handles.
pub const WORLD_PREVIEW_BASE: cw_render::frame::ModelRef = 0x3fff_fe00;
/// `Cube.exe 0x00607033`: the world model's `Model::draw` 0x004e6df0 in 0x00605ae0.
pub const WORLD_DRAW_SITE: u32 = 0x0060_7033;

/// The light direction of the world model: `(0.5, 0.6, 3.5) · (1 / (float)√12.86)`.
pub fn world_light() -> [f32; 3] {
    let k = 1.0 / (12.859999656677246f64.sqrt() as f32);
    [k * 0.5, k * 0.6, k * 3.5]
}

/// The world matrix of the world model (0x0060755e..0x00607780, row-vector convention):
/// identity; row 3 += 17·row 2; rows 1/2 turned by −2.0943952 rad (−120°); rows 0/1 turned by
/// `(yaw + 45)·0.017453292` rad and rows 0..2 scaled by 0.05; row 3 += the model centring
/// `(−sx/2, −sy/2, −sz/2)` through rows 0..2. It is passed as the view, the world being the
/// identity (0x00607025).
pub fn world_model_matrix(yaw: f32, size: [i32; 3]) -> [[f32; 4]; 4] {
    let mut m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    for j in 0..4 {
        m[3][j] = ((m[1][j] * 0.0 + m[0][j] * 0.0) + m[2][j] * 17.0) + m[3][j];
    }
    let rot = |m: &mut [[f32; 4]; 4], a: usize, b: usize, c: f32, s: f32| {
        for j in 0..4 {
            let (ra, rb) = (m[a][j], m[b][j]);
            m[a][j] = ra * c + rb * s;
            m[b][j] = rb * c - ra * s;
        }
    };
    let (c, s) = ((-2.094395160675049f64).cos() as f32, (-2.094395160675049f64).sin() as f32);
    rot(&mut m, 1, 2, c, s);
    let r = (yaw + 45.0) * 0.017_453_292;
    let (c, s) = (f64::from(r).cos() as f32, f64::from(r).sin() as f32);
    rot(&mut m, 0, 1, c, s);
    for row in m.iter_mut().take(3) {
        for v in row.iter_mut() {
            *v *= 0.05;
        }
    }
    let h = [size[0] as f32 * -0.5, size[1] as f32 * -0.5, size[2] as f32 * -0.5];
    for j in 0..4 {
        m[3][j] = m[0][j] * h[0] + m[1][j] * h[1] + m[2][j] * h[2] + m[3][j];
    }
    m
}

/// A cheap fingerprint of the saved worlds' preview models (count, sizes, a byte sum).
/// Performance note for a future optimiser: this walks every preview's voxels each frame the
/// world screen shows; a version counter bumped by the controller's writers would avoid it.
fn previews_fingerprint(models: &[Option<([i32; 3], Vec<u8>)>]) -> u64 {
    let mut h = models.len() as u64;
    for m in models {
        h = h.wrapping_mul(0x100_0000_01b3);
        if let Some((s, v)) = m {
            h ^= (s[0] as u64) << 40 ^ (s[1] as u64) << 20 ^ s[2] as u64;
            h = h.wrapping_add(v.iter().fold(v.len() as u64, |a, &b| a.wrapping_mul(31).wrapping_add(u64::from(b))));
        }
    }
    h
}

impl PreviewModels {
    /// The mesh uploads for the saved worlds' preview models (`WorldInfo+4`, built by the
    /// world save 0x004878a0 from the player's zone model): every handle again when the
    /// models changed since the last call, `None` for a handle that has no model now.
    pub fn world_meshes(&mut self, models: &[Option<([i32; 3], Vec<u8>)>]) -> Vec<(cw_render::frame::ModelRef, Option<cw_render::mesh::ModelMesh>)> {
        let fp = previews_fingerprint(models);
        if self.uploaded == Some(fp) {
            return Vec::new();
        }
        self.uploaded = Some(fp);
        let n = models.len().max(self.uploaded_count);
        self.uploaded_count = models.len();
        (0..n)
            .map(|i| {
                let mesh = models.get(i).and_then(|m| m.as_ref()).filter(|(s, v)| s.iter().all(|&d| d > 0) && v.len() == (s[0] * s[1] * s[2]) as usize * 3).map(|(s, v)| {
                    let vox: Vec<[u8; 3]> = v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                    cw_render::map::mesh_tile_voxels(*s, &vox)
                });
                (WORLD_PREVIEW_BASE + i as u32, mesh)
            })
            .collect()
    }

    /// The world cards' slot 1 (0x00605ae0), draw part, card by card: when the world has a
    /// preview model (`WorldInfo+4 != 0`), its yaw step ([`world_preview_yaw_step`], hovered
    /// = 0x006294c0 on the card's widget) and the model at the pivot + [`MODEL_OFFSET`]; then,
    /// on the player's current world, the selected character walking at the pivot +
    /// [`WALKER_OFFSET`].
    pub fn world_cards(
        &mut self,
        gui: &Gui,
        frame: &PreviewsFrame,
        st: &mut Previews,
        models: &[Option<([i32; 3], Vec<u8>)>],
        frame_ms: i32,
        source: &dyn cw_render::pose::ModelSource,
    ) -> Vec<cw_render::passes::GuiCreature> {
        let mut out = Vec::new();
        for (k, p) in frame.worlds.iter().enumerate() {
            let Some(w) = p.widget else { continue };
            if let Some(i) = p.index
                && let Some(Some((size, _))) = models.get(i)
            {
                if let Some(wp) = st.worlds.get_mut(k) {
                    world_preview_yaw_step(&mut wp.yaw, p.hovered, frame_ms);
                    let at = widget_pivot(gui, w) + MODEL_OFFSET;
                    // 0x00607025: `setTransforms(world = identity, view = the model matrix,
                    // projection)`.
                    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
                    let d = cw_render::passes::ModelDraw::new(WORLD_DRAW_SITE, WORLD_PREVIEW_BASE + i as u32, identity, [1.0; 4]);
                    out.push(cw_render::passes::GuiCreature {
                        origin: WORLD_DRAW_SITE,
                        view: world_model_matrix(wp.yaw, *size),
                        projection: preview_projection(gui.viewport, at),
                        light_direction: world_light(),
                        parts: vec![d],
                    });
                }
            }
            if let Some(i) = p.walking_character
                && let Some(c) = self.creatures.get_mut(i)
            {
                walker_step(c, frame_ms);
                let at = widget_pivot(gui, w) + WALKER_OFFSET;
                out.push(pose_draw(c, gui.viewport, at, frame_ms, source, WALKER_DRAW_SITE));
            }
        }
        out
    }
}

/// 0x00605ae0's walker update on `characters[GC+0x800a0c]` (0x0060721d..0x006073c3): the
/// flags (`+0x124`, u16), physics flags (`+0x5c`), roll and stun times (`+0x128`, `+0x12c`)
/// and mode (`+0x68`) zeroed, the render yaw (`+0x137c`) set to 0, then
/// [`walking_pose_step`].
fn walker_step(c: &mut PreviewCreature, frame_ms: i32) {
    let b = &mut c.entity.0;
    b[0x114..0x116].fill(0);
    b[0x4c..0x50].fill(0);
    b[0x118..0x11c].fill(0);
    b[0x11c..0x120].fill(0);
    b[0x58] = 0;
    c.pose.yaw = 0.0;
    walking_pose_step(&mut c.entity, &mut c.pose, frame_ms);
}

/// `0x004128f0(GC+0x800580, view, projection, GC+0x300, frame_ms, (1, 1, 1, 1), −x, −y, 0,
/// 0)` with the widget's matrices: the render position is the creature's position (the
/// walker copies it at 0x0060726d; the saved characters are never moved), the origin its
/// negated x/y, the render yaw [`PreviewPose::yaw`], the walk cycle its blend and clock.
fn pose_draw(c: &mut PreviewCreature, viewport: IVec2, at: Vec2, frame_ms: i32, models: &dyn cw_render::pose::ModelSource, origin: u32) -> cw_render::passes::GuiCreature {
    let pos = crate::scene::entity_pos(&c.entity);
    let inputs = cw_render::pose::PoseInputs {
        models,
        dt_ms: frame_ms,
        color: [1.0; 4],
        render_pos: pos,
        render_rot: [0.0, 0.0, c.pose.yaw],
        origin: [pos[0].wrapping_neg(), pos[1].wrapping_neg()],
        step_z: 0.0,
        walk: cw_render::pose::WalkCycle { blend: c.pose.blend, phase: c.pose.anim_time },
        mounted: false,
        pet: None,
        combo: 0,
        charge: 0.0,
        guard: 0.0,
        haste: false,
        hide: 0,
        anim_mode: None,
        hit_flash: 0.0,
    };
    let time = i32::from_le_bytes(c.entity.0[0x5c..0x60].try_into().unwrap());
    let pose = cw_render::pose::build_pose_with(&c.entity, time, &inputs, &mut c.state);
    c.pose.blend = pose.walk_blend;
    let parts = pose
        .parts
        .iter()
        .map(|p| {
            let mut d = cw_render::passes::ModelDraw::new(origin, p.model, crate::controller::to_d3d(&p.matrix.to_cols_array()), p.color);
            d.shininess = p.shine;
            d.mirrored = p.mirrored;
            d
        })
        .collect();
    cw_render::passes::GuiCreature {
        origin,
        view: preview_view(pos[2]),
        projection: preview_projection(viewport, at),
        light_direction: character_light(),
        parts,
    }
}

// ---------------------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------------------

/// `onMouseDown` 0x0047b600, preview part: the smoothed cursor y is reset to the engine
/// cursor y (Engine+0xd8, before the press is injected, 0x0047b63f) and the character
/// list's velocity is zeroed (0x0047b70f, every press; the world list's is not — an
/// original asymmetry).
pub fn on_mouse_down(gui: &Gui, st: &mut Previews) {
    st.smoothed_cursor_y = gui.cursor.y;
    st.character_velocity = 0.0;
}

/// `onMouseMove` 0x0047ea00, preview part. `now_ms` is `timeGetTime()`; `inject` performs
/// the engine move `0x00652c10(x, y)`. Updates the smoothed cursor y (every move:
/// `s = (1-f)·s + f·cursorY`, `f = lerpRepeat(dt, 0.01)`); then, when a select screen is
/// visible and the left button is down, injects the move, drags that list by the smoothed
/// delta (re-centring x) and adds `((cursorY − prevCursorY)·dt)·2` to its velocity, and
/// returns true: the rest of `onMouseMove` (camera) is skipped. Returns false (nothing
/// injected) otherwise; the caller then runs the normal path.
pub fn on_mouse_move(gui: &mut Gui, m: &GcMembers, st: &mut Previews, now_ms: u32, client: IVec2, inject: impl FnOnce(&mut Gui)) -> bool {
    // 0x0047ea2b: the static timer (initialised on the first call).
    let last = *st.last_move_time.get_or_insert(now_ms);
    let dt = now_ms.wrapping_sub(last) as i32;
    st.last_move_time = Some(now_ms);
    let old = st.smoothed_cursor_y;
    let f = lerp_factor(dt, 0.01);
    st.smoothed_cursor_y = (1.0 - f) * old + gui.cursor.y * f;
    let delta = st.smoothed_cursor_y - old;
    let (panels, world) = if visible(gui, m.char_select_root) && gui.left_down {
        (st.characters.iter().map(|p| p.panel).collect::<Vec<_>>(), false)
    } else if visible(gui, m.world_select_root) && gui.left_down {
        (st.worlds.iter().map(|p| p.panel).collect::<Vec<_>>(), true)
    } else {
        return false;
    };
    inject(gui);
    for p in panels {
        if let Some(w) = gui.nodes[p].widget {
            let y = pos(gui, w).y + delta;
            let x = half(client.x) as f32 - gui.width(w) * 0.5;
            gui.set_position_xy(w, x, y, true);
        }
    }
    // 0x0047eb7b: `v = ((Engine+0xd8 − Engine+0xe0)·(float)dt)·2 + v`.
    let add = ((gui.cursor.y - gui.prev_cursor.y) * dt as f32) * 2.0;
    if world {
        st.world_velocity = add + st.world_velocity;
    } else {
        st.character_velocity = add + st.character_velocity;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::GameUi;
    use super::super::members::NoPlx;
    use super::*;

    fn setup(chars: usize, worlds: &[&str]) -> (GameUi, GameView, Previews) {
        let mut game = GameView::default();
        for i in 0..chars {
            game.characters.push(CharacterEntry {
                name: format!("C{i}"),
                level: 3,
                class: 1,
                specialization: 1,
                entity_type: 2,
            });
        }
        for (i, n) in worlds.iter().enumerate() {
            game.worlds.push(WorldEntry { name: (*n).into(), seed: i as i32, explored: 0 });
        }
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game);
        let wait = ui.m.wait;
        ui.set_visible(wait, false);
        let mut st = Previews::default();
        rebuild_character_previews(&mut ui.gui, &mut ui.m, &mut st, &game);
        rebuild_world_previews(&mut ui.gui, &ui.m, &mut st, &game);
        (ui, game, st)
    }

    #[test]
    fn rebuild_lists_saved_entries_plus_new() {
        let (ui, _game, st) = setup(2, &["a", "online_b", "c"]);
        assert_eq!(st.characters.len(), 3);
        assert_eq!(st.characters[2].character, None);
        assert_eq!(ui.m.character_previews.len(), 3);
        // Singleplayer: "online_b" is skipped; the maps skip index 1.
        assert_eq!(st.worlds.iter().map(|w| w.world).collect::<Vec<_>>(), vec![Some(0), Some(2), None]);
        assert_eq!(st.world_index_of_panel[&st.worlds[1].panel], 2);
        assert_eq!(st.world_panel_of_index[&3], st.worlds[2].panel);
        // Panels at (300, 50 + 230k); the buttons are the screen's last children.
        let w = ui.gui.nodes[st.characters[1].panel].widget.unwrap();
        assert_eq!(ui.gui.get_position(w), Vec2::new(300.0, 280.0));
        let kids = &ui.gui.nodes[ui.m.char_select_root.unwrap()].children;
        assert_eq!(kids[kids.len() - 1], ui.m.char_delete.unwrap());
        assert!(is_online_world("online_x") && !is_online_world("online") && !is_online_world("x"));
    }

    #[test]
    fn carousel_centres_short_list_and_selects_hover() {
        let (mut ui, mut game, mut st) = setup(1, &[]);
        let m = ui.m.clone();
        ui.gui.nodes[m.start_root.unwrap()].visible = false;
        ui.gui.nodes[m.char_select_root.unwrap()].visible = true;
        let client = IVec2::new(1280, 720);
        // Two panels spanning 230 + 220 = 450 px (< 700): centred, eased toward top = 360 - 225.
        for _ in 0..200 {
            frame(&mut ui.gui, &m, &mut st, &mut game, 16, client);
        }
        let w0 = ui.gui.nodes[st.characters[0].panel].widget.unwrap();
        let p = ui.gui.get_position(w0);
        assert_eq!(p.x, 640.0 - 300.0);
        assert!((p.y - 135.0).abs() < 0.5, "{p}");
        // Hover the second panel: it becomes the selection, "Create" caption, Delete hidden.
        game.selected_character = 0;
        ui.gui.nodes[m.char_delete.unwrap()].visible = true;
        let w1 = ui.gui.nodes[st.characters[1].panel].widget.unwrap();
        // Without gui.plx the panel has no `blackwidget` shape to hit; give its widget the
        // rect test instead.
        ui.gui.widgets[w1].kind = cw_ui::widget::WidgetKind::Game(cw_ui::widget::GameWidget::new(
            cw_ui::widget::GameWidgetClass::Sprite,
        ));
        ui.gui.cursor = ui.gui.get_position(w1) + Vec2::new(10.0, 10.0);
        // The `button2` template's caption TextShape (absent without gui.plx).
        let caption = ui.gui.add_plain_node(m.char_select, "text");
        ui.gui.nodes[caption].text = Some("Select".encode_utf16().collect());
        let f = frame(&mut ui.gui, &m, &mut st, &mut game, 16, client);
        assert_eq!(game.selected_character, 1);
        assert!(!ui.gui.nodes[m.char_delete.unwrap()].visible);
        assert_eq!(ui.gui.nodes[caption].text.as_deref().map(String::from_utf16_lossy).as_deref(), Some("Create"));
        assert!(ui.gui.nodes[m.char_select.unwrap()].visible);
        assert_eq!(f.characters.len(), 2);
        assert_eq!(f.characters[1].texts[0].text, "New character");
        assert_eq!(f.characters[0].texts[2].text, "LVL 3 Human Warrior");
        assert_eq!(f.characters[0].texts[4].text, "Specialization: Guardian");
    }

    #[test]
    fn velocity_decays_and_resets() {
        let (mut ui, mut game, mut st) = setup(0, &[]);
        let m = ui.m.clone();
        st.character_velocity = 100.0;
        // Screen hidden: zeroed.
        frame(&mut ui.gui, &m, &mut st, &mut game, 16, IVec2::new(800, 600));
        assert_eq!(st.character_velocity, 0.0);
        ui.gui.nodes[m.char_select_root.unwrap()].visible = true;
        st.character_velocity = 100.0;
        frame(&mut ui.gui, &m, &mut st, &mut game, 16, IVec2::new(800, 600));
        assert!(st.character_velocity < 100.0 && st.character_velocity > 90.0);
        on_mouse_down(&ui.gui, &mut st);
        assert_eq!(st.character_velocity, 0.0);
    }

    #[test]
    fn drag_adds_velocity() {
        let (mut ui, _game, mut st) = setup(1, &[]);
        let m = ui.m.clone();
        ui.gui.nodes[m.char_select_root.unwrap()].visible = true;
        assert!(!on_mouse_move(&mut ui.gui, &m, &mut st, 1000, IVec2::new(800, 600), |_| {}));
        ui.gui.left_down = true;
        let moved = on_mouse_move(&mut ui.gui, &m, &mut st, 1010, IVec2::new(800, 600), |g| {
            g.prev_cursor = g.cursor;
            g.cursor.y += 5.0;
        });
        assert!(moved);
        assert_eq!(st.character_velocity, 5.0 * 10.0 * 2.0);
    }

    /// 0x00425450: the creature's origin (x/y taken out by the pose origin, z by the view)
    /// lands 17 units in front of the camera and on the card pixel `pivot + (100, 110)`.
    #[test]
    fn preview_camera_puts_the_creature_on_its_card() {
        let z = 37 * 65536 + 12345i64;
        let v = preview_view(z);
        let zf = z as f32 / 65536.0;
        let p = [0.0f32, 0.0, zf, 1.0];
        let view: Vec<f32> = (0..4).map(|j| (0..4).map(|k| p[k] * v[k][j]).sum()).collect();
        assert!(view[0].abs() < 1e-3 && view[1].abs() < 1e-3, "{view:?}");
        assert!((view[2] - 17.0).abs() < 1e-3, "{view:?}");
        let viewport = IVec2::new(1280, 720);
        let at = Vec2::new(440.0, 245.0);
        let pr = preview_projection(viewport, at);
        let clip: Vec<f32> = (0..4).map(|j| (0..4).map(|k| view[k] * pr[k][j]).sum()).collect();
        let ndc = Vec2::new(clip[0] / clip[3], clip[1] / clip[3]);
        let px = Vec2::new((ndc.x + 1.0) * 0.5 * 1280.0, (1.0 - ndc.y) * 0.5 * 720.0);
        assert!((px - at).length() < 0.01, "{px}");
        // Pitch -95 degrees about x, then yaw 45 degrees: the view's up axis is almost the
        // world's z (the camera looks down on the creature from slightly above).
        assert!(v[2][1] > 0.99 && v[2][2].abs() < 0.1, "{:?}", v[2]);
        let l = character_light();
        assert!((l[0] * l[0] + l[1] * l[1] + l[2] * l[2] - 1.0).abs() < 1e-6);
    }

    /// 0x00605ae0: the world model's centre sits 17 units in front of the camera, and a changed
    /// list of preview models is handed to the renderer again (a removed one as `None`).
    #[test]
    fn world_model_view_and_uploads() {
        let size = [8, 6, 4];
        let m = world_model_matrix(30.0, size);
        let c = [4.0f32, 3.0, 2.0, 1.0];
        let v: Vec<f32> = (0..4).map(|j| (0..4).map(|k| c[k] * m[k][j]).sum()).collect();
        assert!(v[0].abs() < 1e-5 && v[1].abs() < 1e-5 && (v[2] - 17.0).abs() < 1e-5, "{v:?}");
        let mut pm = PreviewModels::default();
        let vox = vec![200u8; 8 * 6 * 4 * 3];
        let list = vec![Some((size, vox.clone())), None];
        let up = pm.world_meshes(&list);
        assert_eq!(up.iter().map(|(r, m)| (*r, m.is_some())).collect::<Vec<_>>(), vec![(WORLD_PREVIEW_BASE, true), (WORLD_PREVIEW_BASE + 1, false)]);
        assert!(pm.world_meshes(&list).is_empty());
        let up = pm.world_meshes(&[None]);
        assert_eq!(up.iter().map(|(r, m)| (*r, m.is_some())).collect::<Vec<_>>(), vec![(WORLD_PREVIEW_BASE, false), (WORLD_PREVIEW_BASE + 1, false)]);
        let l = world_light();
        assert!((l[0] * l[0] + l[1] * l[1] + l[2] * l[2] - 1.0).abs() < 1e-5);
    }

    /// The draw part of slot 1: a card with a saved character poses its creature (spinning
    /// while hovered) and yields one GUI creature at its pivot; the "New" card none.
    #[test]
    fn character_cards_pose_their_creatures() {
        struct NoModels;
        impl cw_render::pose::ModelSource for NoModels {
            fn item_model(&self, _item: &[u8]) -> Option<u32> {
                None
            }
            fn model_size(&self, _model: u32) -> [i32; 3] {
                [4, 4, 4]
            }
            fn model_count(&self) -> usize {
                0x1000
            }
        }
        let (mut ui, mut game, mut st) = setup(1, &[]);
        let m = ui.m.clone();
        ui.gui.viewport = IVec2::new(1280, 720);
        ui.gui.nodes[m.start_root.unwrap()].visible = false;
        ui.gui.nodes[m.char_select_root.unwrap()].visible = true;
        let f = frame(&mut ui.gui, &m, &mut st, &mut game, 16, IVec2::new(1280, 720));
        let mut models = PreviewModels::default();
        models.set_characters(vec![crate::controller::fresh_player_entity()]);
        let mut f2 = f.clone();
        f2.characters[0].hovered = true;
        let out = models.creatures(&ui.gui, &f2, 100, &NoModels);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].origin, CHARACTER_DRAW_SITE);
        assert!(out[0].parts.iter().all(|p| p.origin == CHARACTER_DRAW_SITE));
        assert_eq!(models.creatures[0].pose.yaw, -5.0);
        let w = f.characters[0].widget.unwrap();
        let at = widget_pivot(&ui.gui, w) + MODEL_OFFSET;
        assert_eq!(out[0].projection, preview_projection(ui.gui.viewport, at));
        assert!(out[0].projection.iter().flatten().all(|v| v.is_finite()));
        // Renamed/replaced list: the pose resets.
        let mut e = crate::controller::fresh_player_entity();
        e.0[0x1158] = b'Z';
        models.set_characters(vec![e]);
        assert_eq!(models.creatures[0].pose.yaw, 0.0);
    }

    #[test]
    fn texts_and_names() {
        assert_eq!(race_name(13), None);
        assert_eq!(race_name(0), Some("Elf"));
        assert_eq!(specialization_name(3, 1), Some("Water Mage"));
        let w = WorldEntry { name: "Home".into(), seed: 7, explored: 12 };
        let t = world_preview_content(Some(&w), 600.0);
        assert_eq!(t[2].text, "Explored: 12 km\u{b2}");
        assert_eq!(t[4].text, "Seed: 7");
        assert_eq!(t[5].color, DETAIL_COLOR);
        let t = world_preview_content(None, 601.0);
        assert_eq!(t[0].pos, Vec2::new(300.0, 30.0));
        assert_eq!(format_float_g(1234567.0), "1.23457e+06");
        assert_eq!(format_float_g(0.5), "0.5");
    }

    #[test]
    fn pose_spins_when_hovered_and_eases_back() {
        let mut e = EntityData::new_creature();
        let mut p = PreviewPose::default();
        character_preview_update(&mut e, &mut p, true, 100);
        assert_eq!(p.yaw, -5.0);
        assert_eq!(p.blend, 0.2);
        let v = crate::player::vec3_at(&e.0, 0x24);
        assert!(v[1] > 1.0 && v[1] < 3.0);
        for _ in 0..100 {
            character_preview_update(&mut e, &mut p, false, 50);
        }
        assert!(p.yaw.abs() < 0.01 && p.blend < 0.01);
        let mut y = 350.0;
        for _ in 0..200 {
            world_preview_yaw_step(&mut y, false, 50);
        }
        // Shortest way: 350 turns up to 360, not down to 0.
        assert!((y - 360.0).abs() < 0.01, "{y}");
    }
}

/// With the game's `.plx` files (skipped without `CW_GAME_DIR`): the select screen's
/// hover and hit tests on the real, resized (deformed) `blackwidget` and `button2` clones.
#[cfg(test)]
mod real {
    use super::super::{FrameInputs, GameUi, UiAction};
    use super::*;
    use cw_math::rand::MsvcRand;
    use cw_ui::widget::{UiEvent, event};

    fn select_screen() -> Option<(GameUi, GameView)> {
        let dir = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from)?;
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = IVec2::new(1280, 720);
        let mut game = GameView::default();
        game.characters.push(CharacterEntry { name: "A".into(), level: 1, class: 1, specialization: 0, entity_type: 2 });
        game.load_distance = 100.0;
        let mut ui = GameUi::new(gui, &mut plx, &game);
        let gc = ui.m.on_resize_widgets(&ui.gui);
        cw_ui::widget::game_controller_on_resize(&mut ui.gui, &gc, 1280, 720);
        ui.handle_own_action(&game, &UiAction::RefreshPreviews);
        let m = ui.m.clone();
        ui.set_visible(m.start_root, false);
        ui.set_visible(m.char_select_root, true);
        Some((ui, game))
    }

    /// One pass of the message loop: the free cursor's slot 6 (`Engine::mouseMove`), then the
    /// UI frame and its events.
    fn step(ui: &mut GameUi, game: &mut GameView, c: Vec2) {
        ui.gui.inject_mouse_move(c.x, c.y, None);
        let mut rng = MsvcRand::new(1);
        let mut inp = FrameInputs::basic(&mut rng, 1.0, 16, IVec2::new(1280, 720));
        let _ = ui.frame(game, &mut inp);
        let ev = std::mem::take(&mut ui.gui.events);
        let _ = ui.apply(&ev, game);
    }

    fn texts(gui: &Gui, n: NodeId, out: &mut Vec<String>) {
        if let Some(t) = &gui.nodes[n].text {
            out.push(String::from_utf16_lossy(t));
        }
        for &c in &gui.nodes[n].children {
            texts(gui, c, out);
        }
    }

    /// The carousel's hover test (0x006294c0 on each card, 0x0048a33e) must find the
    /// "New character" card as soon as the cursor is on it: the Select button moves there
    /// with the caption "Create" and the small and Delete buttons hide (0x0048a717); back on
    /// the saved character's card it is "Select" again. Before the undeform fix the resized
    /// card's shape was missed and the selection stayed on the first card.
    #[test]
    fn hovering_the_new_card_shows_create() {
        let Some((mut ui, mut game)) = select_screen() else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let m = ui.m.clone();
        for _ in 0..300 {
            step(&mut ui, &mut game, Vec2::new(5.0, 5.0));
        }
        assert_eq!(game.selected_character, 0);
        let centre = |ui: &GameUi, k: usize| {
            let w = ui.gui.nodes[ui.previews.characters[k].panel].widget.unwrap();
            ui.gui.widget_world(w).translation + ui.gui.get_size(w) * 0.5
        };
        let (c0, c1) = (centre(&ui, 0), centre(&ui, 1));
        step(&mut ui, &mut game, c1);
        assert_eq!(game.selected_character, 1);
        let sel = ui.gui.nodes[m.char_select.unwrap()].widget.unwrap();
        let card = ui.gui.nodes[ui.previews.characters[1].panel].widget.unwrap();
        let (sp, cp) = (ui.gui.get_position(sel), ui.gui.get_position(card));
        assert!(ui.visible(m.char_select));
        assert!(sp.y > cp.y && sp.y < cp.y + ui.gui.height(card), "{sp} {cp}");
        assert!(!ui.visible(m.char_small) && !ui.visible(m.char_delete));
        let mut t = Vec::new();
        texts(&ui.gui, m.char_select.unwrap(), &mut t);
        assert!(t.iter().any(|s| s == "Create"), "{t:?}");
        // Out and in again, many times: the state follows the last hovered card.
        for _ in 0..5 {
            step(&mut ui, &mut game, Vec2::new(5.0, 5.0));
            step(&mut ui, &mut game, c1);
        }
        assert_eq!(game.selected_character, 1);
        step(&mut ui, &mut game, c0);
        assert_eq!(game.selected_character, 0);
        let mut t = Vec::new();
        texts(&ui.gui, m.char_select.unwrap(), &mut t);
        assert!(t.iter().any(|s| s == "Select"), "{t:?}");
        assert!(ui.visible(m.char_small));
    }

    /// The Back button (a `button2` clone resized to 200x66): every point of its rect is a
    /// hit, so hovering fires its ENTER and a click its LEFT_RELEASE. Before the undeform fix
    /// half of it was dead: the button seemed to stop answering until the cursor came back
    /// over its caption.
    #[test]
    fn back_button_answers_over_its_whole_rect() {
        let Some((mut ui, mut game)) = select_screen() else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let m = ui.m.clone();
        step(&mut ui, &mut game, Vec2::new(640.0, 700.0));
        let back = ui.gui.nodes[m.back.unwrap()].widget.unwrap();
        let o = ui.gui.widget_world(back).translation;
        let size = ui.gui.get_size(back);
        for iy in 0..6 {
            for ix in 0..10 {
                let p = o + Vec2::new((ix as f32 + 0.5) * size.x / 10.0, (iy as f32 + 0.5) * size.y / 6.0);
                // Away (nothing under the cursor), then onto the point.
                ui.gui.inject_mouse_move(640.0, 700.0, None);
                ui.gui.events.clear();
                ui.gui.inject_mouse_move(p.x, p.y, None);
                assert!(ui.is_hovered(back), "not hovered at {p}");
                let entered = ui.gui.events.iter().any(|e| matches!(e, UiEvent::State { name, .. } if *name == cw_ui::widget::button_state::ENTER));
                assert!(entered, "no ENTER at {p}");
                ui.gui.events.clear();
                ui.gui.inject_left_down(None);
                ui.gui.inject_left_up();
                let clicked = ui.gui.events.iter().any(|e| matches!(e, UiEvent::Signal { widget, event: ev } if *widget == back && *ev == event::LEFT_RELEASE));
                assert!(clicked, "no click at {p}");
            }
        }
    }
}
