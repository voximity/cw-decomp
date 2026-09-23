//! Crafting: the recipe tabs of the crafting `InventoryWidget` (type 2, `GC+0x800958`, data
//! `GC+0x800adc`), `cube::BlueprintPreviewWidget` (vtable 0x006fd364, ctor 0x0042f190, 0x3c8
//! bytes, panel `GC+0x800ad4`, widget `GC+0x800964`), the craft timer of `update` and the craft
//! itself. Also the small draw types the other trade panels ([`super::enchant`],
//! [`super::adaption`], [`super::voxel`]) share.
//!
//! Tier B. Drawing is described as data ([`BlueprintFrame`]); the renderer draws it.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | recipe table `World::recipeFor(item, &recipe)` 0x0059cff0 (always returns true), ingredient push 0x005a0d80 | [`recipe_for`] |
//! | recipe tabs rebuild 0x004a14c0 (sort 0x00455c30 / 0x00454b50, fill 0x004a19d0) | [`refresh`] |
//! | left click with the crafting panel open, after `InventoryWidget` 0x004c6610 (`onMouseDown` 0x0047bab6..0x0047bcee) | [`on_recipe_click`] |
//! | R on a crafting station (`update` 0x0049756a..0x004975b6) | [`open_from_station`] |
//! | C / menu button 2 (0x00488bd0: toggle, then 0x004a14c0) | [`toggle`] |
//! | `update` 0x0048c677..0x0048c6c1: the preview panel follows the crafting panel | [`preview_visibility_rule`] |
//! | `update` 0x0048f183..0x0048f242: the craft timer | [`update_timer`] |
//! | the craft 0x004709c0 (sounds 0x31, 0x59, 0x5a) | [`craft`] |
//! | `BlueprintPreviewWidget::update` (slot 1) 0x0042f910 | [`preview_frame`] |
//! | `BlueprintPreviewWidget::layout` (slot 10) 0x004348f0 | [`preview_layout`] |
//! | progress bar 0x00434c20 | [`bar_fill`] |
//! | tooltip items 0x0047b340 (recipe cell) / 0x0047b3e0 (preview) | [`recipe_tooltip`], [`BlueprintPreview::tooltip_item`] |
//!
//! # State
//!
//! The recipes are the player's learned formulas ([`GameView::formulas`], `*(creature+0x1d28)
//! + 0x14`). Each becomes, through [`recipe_for`], a [`Recipe`] (0x128 bytes: result item,
//! station `+0x118`, ingredient vector `+0x11c` of 0x11c-byte `Item + count` records) and lands
//! in one of six tabs by the result type: 0 weapons (3), 1 armour (4..7), 2 accessories (8, 9),
//! 3 consumables (1) other than sub types 1/2, 4 consumables of sub type 1/2, 5 ingredients
//! (0xb). Types 2, 0xa and the rest get no tab.

use cw_math::rand::MsvcRand;
use cw_ui::widget::{Gui, NodeId};
use glam::Vec2;

use super::inventory::{add_item, has_room, item_level, knows_formula, same_item};
use super::inventory_widget::InventoryWidget;
use super::members::GcMembers;
use super::{GameView, ItemStack, UiAction};

/// The size of `cube::Item`.
pub const ITEM_SIZE: usize = cw_net::entity::ITEM_SIZE;

// ---------------------------------------------------------------------------------------
// Shared draw data and helpers (also used by enchant, adaption and voxel)
// ---------------------------------------------------------------------------------------

/// One text draw of a panel: `plasma` text 0x00639b30 (font `resource1.dat`), which the
/// original calls twice per text, an outline pass with `outline` > 0 then a fill pass with
/// outline 0 and the colour below. Positions are widget-local unless a field says otherwise.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelText {
    /// The text.
    pub text: String,
    /// Position (widget-local).
    pub x: f32,
    /// Position (widget-local).
    pub y: f32,
    /// Font size.
    pub size: f32,
    /// Outline width of the first pass.
    pub outline: f32,
    /// Fill colour (RGBA).
    pub color: [f32; 4],
    /// The alignment flags argument of 0x00639b30 (0 left, 1 and 0x11 centred, 2 right, 0x10
    /// used for the station line; the meaning of the bits is the font code's).
    pub align: u32,
}

impl PanelText {
    pub(crate) fn new(text: impl Into<String>, x: f32, y: f32, size: f32, outline: f32, color: [f32; 4], align: u32) -> Self {
        PanelText { text: text.into(), x, y, size, outline, color, align }
    }
}

/// White.
pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The hovered-button colour (0, 1, 1, 1).
pub const CYAN: [f32; 4] = [0.0, 1.0, 1.0, 1.0];
/// A disabled button (0.7, 0.7, 0.7, 1).
pub const GRAY: [f32; 4] = [0.7, 0.7, 0.7, 1.0];

/// A 3D item model drawn in a panel (`cube::Model::draw` 0x004e6df0 with `CubeShader`).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDraw {
    /// The item.
    pub item: Vec<u8>,
    /// Index into the model cache (`GC+0x300`, [`crate::interact::item_model_index`]).
    pub model: u32,
    /// Screen position of the model centre.
    pub center: [f32; 2],
    /// Scale numerator: the model is scaled by `scale / max(dimension)`.
    pub scale: f32,
    /// Euler angles in degrees (the widget's `+0x160/+0x164/+0x168`).
    pub angles: [f32; 3],
}

/// An item icon drawn with `0x004758c0(x, y, GC+0x800a1c rotation, scale, item, 0)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemIcon {
    /// The item.
    pub item: Vec<u8>,
    /// Icon centre (the coordinate space is the caller's, see the frame struct).
    pub center: [f32; 2],
    /// Scale argument.
    pub scale: f32,
}

/// Node visibility (`Node+0x3c` Display attribute); a missing node reads as hidden.
pub(crate) fn vis(gui: &Gui, n: Option<NodeId>) -> bool {
    n.is_some_and(|n| gui.nodes[n].visible)
}

/// Writes a node's visibility (0x00411a90); missing nodes are ignored.
pub(crate) fn set_vis(gui: &mut Gui, n: Option<NodeId>, v: bool) {
    if let Some(n) = n {
        gui.nodes[n].visible = v;
    }
}

pub(crate) fn snd(id: u32) -> UiAction {
    UiAction::PlaySound { id, volume: 1.0, pitch: 1.0 }
}

pub(crate) fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// `0x00434870`: whether an item's name is followed by its level: not for types 0xc, 0xd,
/// 0x15, 0, 0x19, 0x14, 0x18, 0x17, nor type 0xb unless its sub type is 0xe.
pub fn shows_level(item: &[u8]) -> bool {
    let t = item[0];
    !(matches!(t, 0xc | 0xd | 0x15) || (t == 0xb && item[1] != 0xe) || matches!(t, 0 | 0x19 | 0x14 | 0x18 | 0x17))
}

/// `Item::Item` 0x0042f3e0 / the inline item resets: zero with level (`+0x10`) 1.
pub fn default_item() -> Vec<u8> {
    let mut v = vec![0u8; ITEM_SIZE];
    v[0x10] = 1;
    v
}

// ---------------------------------------------------------------------------------------
// Recipes
// ---------------------------------------------------------------------------------------

/// One ingredient (0x11c bytes: the item, then the count at `+0x118`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ingredient {
    /// The item.
    pub item: Vec<u8>,
    /// `+0x118`: how many are needed.
    pub count: i32,
}

/// A recipe (0x128 bytes; `BlueprintPreviewWidget+0x16c`, `GC+0x1000e7c`).
#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    /// `+0`: the result item.
    pub item: Vec<u8>,
    /// `+0x118`: the station needed (see [`Station`]); 0 none.
    pub station: i32,
    /// `+0x11c`: the ingredients.
    pub ingredients: Vec<Ingredient>,
}

impl Default for Recipe {
    /// `0x0042f360`: default item (level 1), station 0, no ingredients.
    fn default() -> Self {
        Recipe { item: default_item(), station: 0, ingredients: Vec::new() }
    }
}

/// What a recipe's station value (`Recipe+0x118`) requires ([`BlueprintFrame::station_text`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Station {
    /// 1: water. Satisfied when the player's physics flags (`creature+0x5c`, entity `+0x4c`)
    /// have bit 1 set, or the block `0x0042f860(pos.x, pos.y, pos.z - (i64)(height * 0.5 *
    /// 65536))` (`height` = `creature+0x88`) has `(b[3] & 0x1f) == 3`. "Requires water".
    Water,
    /// A static entity of `kind` near the player: the 3x3 world cells around
    /// `(pos / 65536) / 8` (0x0042f640) are searched for a static whose kind (`+0`) matches.
    /// Kinds 0x47..0x4b count when the squared distance in blocks (fixed-point difference /
    /// 65536) is below 16; kinds 0x4c and 0x41 when the length 0x00424860 of the difference
    /// vector (0x0042c7a0) is below 16.
    Static {
        /// Static kind.
        kind: u32,
        /// The line shown when it is missing.
        text: &'static str,
    },
}

/// The station switch of 0x0042f910 (jump table 0x00434770 over `station - 1`, 1..9).
pub fn station(station: i32) -> Option<Station> {
    let s = |kind, text| Some(Station::Static { kind, text });
    match station {
        1 => Some(Station::Water),
        3 => s(0x47, "Requires Furnace"),
        4 => s(0x48, "Requires Anvil"),
        5 => s(0x49, "Requires Spinning Wheel"),
        6 => s(0x4a, "Requires Loom"),
        7 => s(0x4b, "Requires Saw"),
        8 => s(0x4c, "Requires Workbench"),
        9 => s(0x41, "Requires Campfire"),
        // 2 and everything outside 1..9: nothing required.
        _ => None,
    }
}

/// `World::recipeFor(item, &out)` 0x0059cff0: the ingredients of `item`. The result item is a
/// copy of `item`; `station` keeps the caller's value (0 for a fresh [`Recipe`]) unless a case
/// sets it. Every ingredient is pushed (0x005a0d80) from one template that the cases edit in
/// place: type 0xb, sub type, material `+0xd`, rarity `+0xc`, level 1, count `+0x118`. The
/// original always returns true.
pub fn recipe_for(item: &[u8]) -> Recipe {
    let mut r = Recipe { item: item[..ITEM_SIZE].to_vec(), station: 0, ingredients: Vec::new() };
    // The template (0x0059d014..0x0059d06c).
    let mut t = default_item();
    t[0] = 0xb;
    let mut count = 0i32;
    let push = |r: &mut Recipe, t: &[u8], count: i32| r.ingredients.push(Ingredient { item: t.to_vec(), count });
    let mat = item[0xd];
    match item[0] {
        // 0x0059d3bb: consumables by sub type (table 0x0059d600).
        1 => match item[1] {
            1 => {
                count = 1;
                t[1] = 0x16;
                push(&mut r, &t, count);
                t[1] = 0x1a;
                push(&mut r, &t, count);
            }
            2 => {
                count = 1;
                t[1] = 0x17;
                push(&mut r, &t, count);
                t[1] = 0xc;
                t[0xd] = 0x18;
                push(&mut r, &t, count);
            }
            4 => {
                t[1] = 0x14;
                count = 1;
                push(&mut r, &t, count);
                r.station = 9;
            }
            5 => {
                count = 1;
                t[1] = 0x1b;
                push(&mut r, &t, count);
            }
            6 => {
                count = 1;
                t[1] = 0xf;
                push(&mut r, &t, count);
                t[1] = 0x15;
                push(&mut r, &t, count);
                r.station = 9;
            }
            8 => {
                count = 1;
                t[1] = 0x11;
                push(&mut r, &t, count);
            }
            9 => {
                t[1] = 0x10;
                count = 1;
                push(&mut r, &t, count);
                r.station = 9;
            }
            _ => {}
        },
        // 0x0059d083: weapons by sub type (table 0x0059d590).
        3 => match item[1] {
            0..=3 | 0xd => {
                count = 8;
                t[1] = 0xa;
                t[0xd] = 1;
                push(&mut r, &t, count);
                r.station = 4;
            }
            4 => {
                count = 6;
                t[1] = 0xa;
                t[0xd] = 1;
                push(&mut r, &t, count);
                count = 2;
                t[1] = 9;
                t[0xd] = 0x1b;
                push(&mut r, &t, count);
                r.station = 4;
            }
            5 | 0xf..=0x11 => {
                count = 0x10;
                t[1] = 0xa;
                t[0xd] = 1;
                push(&mut r, &t, count);
                r.station = 4;
            }
            6 => {
                count = 0xf;
                t[1] = 0xa;
                t[0xd] = 2;
                push(&mut r, &t, count);
                count = 1;
                t[1] = 9;
                t[0xd] = 0x1a;
                push(&mut r, &t, count);
                r.station = 8;
            }
            7 => {
                count = 6;
                t[1] = 0xa;
                t[0xd] = 1;
                push(&mut r, &t, count);
                count = 9;
                t[1] = 0xa;
                t[0xd] = 2;
                push(&mut r, &t, count);
                count = 1;
                t[1] = 9;
                t[0xd] = 0x1a;
                push(&mut r, &t, count);
                r.station = 8;
            }
            8 | 0xa | 0xb => {
                count = 0x10;
                t[1] = 0xa;
                t[0xd] = 2;
                push(&mut r, &t, count);
                r.station = 8;
            }
            0xc => {
                t[0xd] = mat;
                count = 8;
                t[1] = 0xa;
                r.station = 4;
                push(&mut r, &t, count);
            }
            _ => {}
        },
        // 0x0059d1f6 / 0x0059d24d / 0x0059d244: armour, 0x14 / 6 / 8 pieces of the material
        // (material 1, iron: sub 0xa at the anvil; else sub 9 at the loom).
        4..=7 => {
            count = match item[0] {
                4 => 0x14,
                5 => 6,
                _ => 8,
            };
            if mat == 1 {
                t[1] = 0xa;
                t[0xd] = 1;
                push(&mut r, &t, count);
                r.station = 4;
            } else {
                t[0xd] = mat;
                t[1] = 9;
                push(&mut r, &t, count);
                r.station = 6;
            }
        }
        // 0x0059d256: amulets, 6 of the material; station untouched.
        8 => {
            count = 6;
            t[1] = 0xa;
            t[0xd] = mat;
            push(&mut r, &t, count);
        }
        // 0x0059d284: rings, 3 of the material.
        9 => {
            t[0xd] = mat;
            count = 3;
            t[1] = 0xa;
            push(&mut r, &t, count);
        }
        // 0x0059d2b2: ingredients by sub type - 9 (tables 0x0059d5ec / 0x0059d5d8).
        0xb => match item[1] {
            9 => {
                let set = match mat {
                    0x19 => Some((6u8, 0u8)),
                    0x1a => Some((5, 0x15)),
                    0x1b => Some((0xb, 0x1b)),
                    _ => None,
                };
                if let Some((sub, m)) = set {
                    t[1] = sub;
                    t[0xd] = m;
                    count = 1;
                    push(&mut r, &t, count);
                    r.station = 5;
                }
            }
            0xa => {
                count = 1;
                if mat == 2 {
                    t[1] = 1;
                    r.station = 7;
                } else {
                    t[1] = 0;
                    r.station = 3;
                }
                t[0xd] = mat;
                push(&mut r, &t, count);
            }
            0x16 => {
                t[1] = 0x18;
                t[0xd] = 0;
                count = 1;
                push(&mut r, &t, count);
                r.station = 9;
            }
            0x1a => {
                count = 1;
                t[1] = 0xc;
                t[0xd] = 0x18;
                push(&mut r, &t, count);
                r.station = 1;
            }
            _ => {}
        },
        // 2, 0xa and the rest: no ingredients.
        _ => {}
    }
    // 0x0059d4f3: rarity 1..4 adds two ingredients of sub type 0, rarity r, material 0xc + r
    // (the rarity/material word 0x0d01 / 0x0e02 / 0x0f03 / 0x1004).
    let rarity = item[0xc];
    if (1..=4).contains(&rarity) {
        t[0xc] = rarity;
        t[0xd] = 0xc + rarity;
        count = 2;
        t[1] = 0;
        push(&mut r, &t, count);
    }
    let _ = count;
    r
}

/// The recipe tab of a result (`0x004a161d` table 0x004a19b8 over `type - 1`).
pub fn recipe_tab(item: &[u8]) -> Option<usize> {
    match item[0] {
        3 => Some(0),
        4..=7 => Some(1),
        8 | 9 => Some(2),
        // 0x004a163c: `(sub - 1) as u8 <= 1`.
        1 => Some(if item[1].wrapping_sub(1) <= 1 { 4 } else { 3 }),
        0xb => Some(5),
        _ => None,
    }
}

/// The sort key of the recipe tabs (0x00454b50): `(rarity + level * 6) * 1000` (int).
fn sort_key(item: &[u8]) -> i32 {
    (i32::from(item[0xc]) + i32::from(i16_at(item, 0x10)) * 6).wrapping_mul(1000)
}

// ---------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------

/// `cube::BlueprintPreviewWidget` fields (ctor 0x0042f190).
#[derive(Clone, Debug, PartialEq)]
pub struct BlueprintPreview {
    /// `+0x160`: model pitch in degrees (-120).
    pub pitch: f32,
    /// `+0x164`: model roll (0).
    pub roll: f32,
    /// `+0x168`: model yaw, `+= dt_ms * 0.02` per frame.
    pub yaw: f32,
    /// `+0x16c` (item), `+0x284` (station), `+0x288` (ingredients): the recipe shown.
    pub recipe: Recipe,
    /// `+0x2a8`: the ingredient under the cursor, -1 none.
    pub hovered_ingredient: i32,
    /// `+0x2ac`: the tooltip item (the hovered ingredient or the hovered result); its type byte
    /// is cleared every frame, so type 0 means none ([`BlueprintPreview::tooltip_item`]).
    pub tooltip: Vec<u8>,
    /// `+0x3c4`: the craft button is enabled and hovered (read by `update` through
    /// 0x0047f1c0).
    pub button_hot: bool,
}

impl Default for BlueprintPreview {
    fn default() -> Self {
        BlueprintPreview {
            pitch: -120.0,
            roll: 0.0,
            yaw: 0.0,
            recipe: Recipe::default(),
            hovered_ingredient: -1,
            tooltip: default_item(),
            button_hot: false,
        }
    }
}

impl BlueprintPreview {
    /// The preview half of 0x0047b3e0: with the preview panel open, the tooltip item when its
    /// type is not 0.
    pub fn tooltip_item(&self, preview_panel_open: bool) -> Option<&[u8]> {
        (preview_panel_open && self.tooltip[0] != 0).then_some(&self.tooltip[..])
    }
}

/// The crafting state of the GameController and the preview widget.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftingState {
    /// `GC+0x800964`: the preview widget.
    pub preview: BlueprintPreview,
    /// `GC+0x800adc`: the recipe tabs the crafting `InventoryWidget` shows (its `+0x160`):
    /// cells of (craftable count, result item).
    pub tabs: Vec<Vec<ItemStack>>,
    /// `GC+0x1000e78`: the craft timer, -1 idle, 0..1 while crafting.
    pub timer: f32,
    /// `GC+0x1000e7c`: the recipe being crafted (copied from the preview when the timer
    /// starts).
    pub crafting: Recipe,
}

impl Default for CraftingState {
    fn default() -> Self {
        CraftingState { preview: BlueprintPreview::default(), tabs: Vec::new(), timer: -1.0, crafting: Recipe::default() }
    }
}

/// `0x0047b340`: the tooltip item of the crafting widget, the recipe cell under the cursor
/// (`hovered` = the widget's `+0x184/+0x188`) when the crafting panel is open, the cell
/// exists, its count is not negative and its type is not 0.
pub fn recipe_tooltip<'a>(state: &'a CraftingState, crafting_panel_open: bool, hovered: (i32, i32)) -> Option<&'a [u8]> {
    if !crafting_panel_open || hovered.0 < 0 || hovered.1 < 0 {
        return None;
    }
    let s = state.tabs.get(hovered.0 as usize)?.get(hovered.1 as usize)?;
    (s.count >= 0 && s.item[0] != 0).then_some(&s.item[..])
}

/// How many of `item` the bag holds (the sums of 0x004a19d0 and 0x0042f910: every stack whose
/// item equals, count 0 included).
fn have_count(game: &GameView, item: &[u8], skip_empty: bool) -> i32 {
    let mut n = 0i32;
    for tab in &game.inventory {
        for s in tab {
            if skip_empty && s.count == 0 {
                continue;
            }
            if same_item(&s.item, item) {
                n = n.wrapping_add(s.count);
            }
        }
    }
    n
}

/// 0x004a19d0(tab, recipes): tab `tab` of [`CraftingState::tabs`] (created up to `tab + 1`
/// tabs, 0x00487380) is cleared and gets one cell per recipe: the result item with the count
/// `min over ingredients of (held / needed)` (0 without ingredients). The recipe at the
/// crafting widget's selection (`+0x18c/+0x190`) is copied into the preview.
fn fill_tab(state: &mut CraftingState, game: &GameView, tab: usize, recipes: &[Recipe], selected: (i32, i32)) {
    if state.tabs.len() < tab + 1 {
        state.tabs.resize(tab + 1, Vec::new());
    }
    state.tabs[tab].clear();
    for (i, r) in recipes.iter().enumerate() {
        let mut best = -1i32;
        for ing in &r.ingredients {
            // `need` is never 0 in the recipe table; the original would fault on it.
            let q = if ing.count != 0 { have_count(game, &ing.item, false) / ing.count } else { 0 };
            if best < 0 || q < best {
                best = q;
            }
        }
        let count = if r.ingredients.is_empty() { 0 } else { best };
        state.tabs[tab].push(ItemStack { count, item: r.item.clone() });
        if tab as i32 == selected.0 && i as i32 == selected.1 {
            state.preview.recipe = r.clone();
        }
    }
}

/// `0x004a1e50` (the ctor's only call, 0x004644f7): the icon textures of the crafting
/// widget's tabs, one parentless clone of the GUI root's `tab` per entry (`Node::clone(0)`
/// 0x006326d0, the texture written into the clone shape's `texture` key `+0x7f8[+0x7cc]`,
/// 0x0064ac00 per shape), handed to `setTabs` 0x004c6140 in this order: the weapon and armour
/// icons of the local player's class (`creature+0x140`: 1 `GC+0x8006a8` / `+0x800698`,
/// 2 `+0x8006ac` / `+0x80069c`, 3 `+0x8006b4` / `+0x8006a4`, 4 `+0x8006b0` / `+0x8006a0`;
/// another class has neither), then `+0x8006b8`, `+0x8006bc`, `+0x8006c0`, `+0x8006c4`. The
/// fields hold the textures the ctor loads at 0x00463929..0x00463d82 (`0x00486a20`), whose
/// names are given here. The six match the recipe tabs of [`refresh`] (weapons, armour,
/// accessories, cooking, alchemy, ingredients).
pub fn crafting_tab_icons(class: u8) -> Vec<&'static str> {
    let mut out = Vec::new();
    let pair = match class {
        1 => Some(("craft-melee-weapons.png", "craft-heavy-armor.png")),
        2 => Some(("craft-ranged-weapons.png", "craft-medium-armor.png")),
        3 => Some(("craft-magic-weapons.png", "craft-light-armor.png")),
        4 => Some(("craft-rogue-weapons.png", "craft-rogue-armor.png")),
        _ => None,
    };
    if let Some((w, a)) = pair {
        out.push(w);
        out.push(a);
    }
    out.extend(["craft-amulets.png", "craft-cooking.png", "craft-alchemy.png", "inventory-ingredients.png"]);
    out
}

/// `0x004a14c0`: rebuilds the recipe tabs from the learned formulas ([`GameView::formulas`],
/// list order) and the bag, then (0x004c6350 / 0x004c64c0, the lead's) the crafting widget
/// refreshes. Each formula's recipe ([`recipe_for`]) goes to its [`recipe_tab`]; each tab is
/// sorted by `std::sort` with "a before b" when `(rarity + level * 6) * 1000` of b is less
/// than a's (descending), then filled ([`fill_tab`]). `selected` is the crafting widget's
/// `(+0x18c, +0x190)`.
pub fn refresh(state: &mut CraftingState, game: &GameView, selected: (i32, i32)) {
    let mut bins: [Vec<Recipe>; 6] = Default::default();
    for f in &game.formulas {
        let r = recipe_for(f);
        if let Some(t) = recipe_tab(&f[..]) {
            bins[t].push(r);
        }
    }
    for b in bins.iter_mut() {
        cw_math::sort::msvc_sort(b, |a, b| sort_key(&b.item) < sort_key(&a.item));
    }
    for (t, b) in bins.iter().enumerate() {
        fill_tab(state, game, t, b, selected);
    }
}

/// `onMouseDown` 0x0047bab6..0x0047bcee (left button, crafting panel open), after the crafting
/// widget's click 0x004c6610 has set its selection `selected` (`+0x18c/+0x190`): a selection
/// inside [`CraftingState::tabs`] shows its recipe ([`recipe_for`] of the cell's item, which
/// always succeeds); an invalid one shows the default recipe (level-1 empty item).
pub fn on_recipe_click(gui: &Gui, m: &GcMembers, state: &mut CraftingState, selected: (i32, i32)) {
    if !vis(gui, m.crafting_panel) {
        return;
    }
    let (t, i) = selected;
    let cell = if t >= 0 && i >= 0 { state.tabs.get(t as usize).and_then(|tab| tab.get(i as usize)) } else { None };
    state.preview.recipe = match cell {
        Some(c) => recipe_for(&c.item),
        None => Recipe::default(),
    };
}

/// C and menu button 2 (0x00488bd0): flips the crafting panel, then rebuilds the recipes
/// ([`refresh`]); `flow::toggle_crafting` is the key/menu path and does both.
pub fn toggle(gui: &mut Gui, m: &GcMembers, state: &mut CraftingState, game: &GameView, crafting_widget: &InventoryWidget) {
    let v = !vis(gui, m.crafting_panel);
    set_vis(gui, m.crafting_panel, v);
    refresh(state, game, (crafting_widget.selected_tab, crafting_widget.selected_index));
}

/// R on a crafting station (`update` 0x0049756a..0x004975b6, kinds 0x41, 0x47..0x4c; the
/// gameplay side's `Event::OpenCraftingPanel`): the crafting panel flips; the campfire (0x41)
/// selects recipe tab 3, the furnace, spinning wheel and saw (0x47, 0x49, 0x4b) tab 5
/// (`InventoryWidget+0x1b4` through 0x0046eb80); then [`refresh`].
pub fn open_from_station(gui: &mut Gui, m: &GcMembers, state: &mut CraftingState, game: &GameView, crafting_widget: &mut InventoryWidget, kind: u32) {
    let v = !vis(gui, m.crafting_panel);
    set_vis(gui, m.crafting_panel, v);
    // Jump table 0x0049d508 / 0x0049d4fc over kind - 0x41.
    match kind {
        0x41 => crafting_widget.tab = 3,
        0x47 | 0x49 | 0x4b => crafting_widget.tab = 5,
        _ => {}
    }
    refresh(state, game, (crafting_widget.selected_tab, crafting_widget.selected_index));
}

/// `update` 0x0048c677..0x0048c6c1: when both panels exist, the preview panel is visible
/// exactly when the crafting panel is (the selection read through 0x00487e60 is unused).
pub fn preview_visibility_rule(gui: &mut Gui, m: &GcMembers) {
    if m.craft_preview_panel.is_some() && m.crafting_panel.is_some() {
        let v = vis(gui, m.crafting_panel);
        set_vis(gui, m.craft_preview_panel, v);
    }
}

/// `update` 0x0048f183..0x0048f242, with the preview panel visible: when the craft button is
/// hot ([`BlueprintPreview::button_hot`]), the left button is held (`GC+4`) and the timer is
/// idle (`0 > timer`, a `comiss`: NaN does not start), the timer starts at 0 and the preview's
/// recipe is copied (0x004685e0). A running timer (`timer >= 0`, NaN skips) advances by
/// `dt_ms * 0.001`; reaching 1 it goes back to -1 and [`craft`] runs. Returns the craft's
/// actions; the bar fill is [`bar_fill`] of the new timer (0x00434c20).
pub fn update_timer(gui: &Gui, m: &GcMembers, state: &mut CraftingState, game: &mut GameView, rng: &mut MsvcRand, dt_ms: i32, left_held: bool) -> Vec<UiAction> {
    let mut out = Vec::new();
    if !(m.craft_preview_panel.is_some() && vis(gui, m.craft_preview_panel)) {
        return out;
    }
    if state.preview.button_hot && left_held && 0.0 > state.timer {
        state.timer = 0.0;
        state.crafting = state.preview.recipe.clone();
    }
    // `comiss timer, 0; jb`: NaN (unordered) skips.
    if state.timer >= 0.0 {
        let t = dt_ms as f32 * 0.001f32 + state.timer;
        state.timer = t;
        if t >= 1.0 {
            state.timer = -1.0;
            out.extend(craft(state, game, rng));
        }
    }
    out
}

/// `0x00434c20(timer)` on the preview: the craft bar's fill, `(bar_width - 8) * max(timer, 0)`
/// in pixels (the bar child `+0x2a4` is x-scaled to it over its own width).
pub fn bar_fill(timer: f32, bar_width: f32) -> f32 {
    let p = if 0.0 > timer { 0.0 } else { timer };
    (bar_width - 8.0) * p
}

/// The craft 0x004709c0, on [`CraftingState::crafting`].
///
/// Without room for the result ([`has_room`]): "You can't carry more of these items.\n" in
/// (1, 0.2, 0.2, 1) and sound 0x31. Otherwise, for each ingredient in order, `need` = its
/// count, and for each bag tab, the stacks equal to it: when `need < count` the stack loses
/// `need` and the tab is left (the next tab is still searched with the same `need`);
/// otherwise the stack is emptied (count 0, default item) and `need` is reduced by the
/// already-zeroed count, i.e. not at all (the original's bug, kept). Then the result: types 3,
/// 4, 7, 6, 5, 8, 9 get `Item+0xe |= 1`; sound 0x59; an ingredient result (0xb) other than sub
/// type 0x1a gets, one time in five, `+1` level (`+2` when a second `rand() % 5 == 0`, `+3`
/// on a third) and sound 0x5a. The result is added to the bag (`addItem(item, -1)`), the bag
/// widget refreshes (0x004c6350 / 0x004c64c0), "You receive ..." is printed (0x00480fb0,
/// [`UiAction::ItemReceived`]) and the recipes are rebuilt ([`UiAction::RebuildRecipes`]).
pub fn craft(state: &mut CraftingState, game: &mut GameView, rng: &mut MsvcRand) -> Vec<UiAction> {
    let mut out = Vec::new();
    let mut item = state.crafting.item.clone();
    if !has_room(game, &item) {
        out.push(UiAction::Print { text: "You can't carry more of these items.\n".into(), color: [1.0, 0.2, 0.2, 1.0] });
        out.push(snd(0x31));
        return out;
    }
    for ing in &state.crafting.ingredients {
        let need = ing.count;
        for tab in game.inventory.iter_mut() {
            for s in tab.iter_mut() {
                if same_item(&s.item, &ing.item) {
                    if need < s.count {
                        s.count -= need;
                        break;
                    }
                    s.count = 0;
                    s.item = default_item();
                    // `need -= count` with the count already 0; `need == 0` ends the tab.
                    if need == 0 {
                        break;
                    }
                }
            }
        }
    }
    if matches!(item[0], 3 | 4 | 7 | 6 | 5 | 8 | 9) {
        item[0xe] |= 1;
    }
    out.push(snd(0x59));
    if item[0] == 0xb && item[1] != 0x1a && rng.rand() % 5 == 0 {
        let mut bonus: i16 = 1;
        if rng.rand() % 5 == 0 {
            bonus = 2;
            if rng.rand() % 5 == 0 {
                bonus = 3;
            }
        }
        let l = i16_at(&item, 0x10).wrapping_add(bonus);
        item[0x10..0x12].copy_from_slice(&l.to_le_bytes());
        out.push(snd(0x5a));
    }
    add_item(game, &item, -1);
    out.push(UiAction::ItemReceived { item });
    out.push(UiAction::RebuildRecipes);
    out
}

// ---------------------------------------------------------------------------------------
// BlueprintPreviewWidget::update 0x0042f910
// ---------------------------------------------------------------------------------------

/// The inputs of [`preview_frame`].
pub struct PreviewInputs<'a> {
    /// The client state (bag, formulas).
    pub game: &'a GameView,
    /// The widget's screen rectangle origin (0x0062dc20: left, top).
    pub origin: Vec2,
    /// The widget size (width 0x0062f600, height 0x006291d0 / 0x00627ce0).
    pub size: Vec2,
    /// The cursor in widget space (0x006294d0).
    pub cursor_local: Vec2,
    /// The engine cursor in screen space (`Engine+0xd4/+0xd8`).
    pub cursor_screen: Vec2,
    /// `Engine+0xe4`: the frame time in ms.
    pub dt_ms: i32,
    /// Whether the station of a recipe is present ([`station`]; the controller evaluates it
    /// against the world).
    pub station_ok: &'a dyn Fn(Station) -> bool,
    /// The item name (`0x00598a50`, the world's text database).
    pub item_name: &'a dyn Fn(&[u8]) -> String,
    /// The preview has a craft button (`+0x29c`, the `craftbutton` node).
    pub has_button: bool,
    /// `Button::isHovered` 0x006294c0 on the craft button.
    pub button_hovered: bool,
    /// The preview has a craft bar (`+0x2a0`, the `craftbar` node).
    pub has_bar: bool,
    /// `GC+0x1000e78` ([`CraftingState::timer`]).
    pub timer: f32,
}

/// One ingredient cell of the preview.
#[derive(Clone, Debug, PartialEq)]
pub struct IngredientCell {
    /// The icon, centre in screen space (`cell + 20`), scale 0.025 (0.0375 hovered).
    pub icon: ItemIcon,
    /// The hover rectangle in screen space: `x..x+40`, `y..y+40`.
    pub rect: [f32; 4],
    /// Held count ([`CraftingState`] sum over the bag, empty stacks skipped).
    pub have: i32,
    /// Needed count.
    pub need: i32,
}

/// What the preview shows this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlueprintFrame {
    /// The result model, at `(left + w / 2, top + 110)`, scale `0.06` (0.09 when the result
    /// is hovered) over the largest model dimension. `None` when the item has no model: the
    /// original then draws nothing at all and every other field stays empty.
    pub model: Option<ModelDraw>,
    /// The ingredient icons (only for a learned formula).
    pub ingredients: Vec<IngredientCell>,
    /// Texts: the name ("Unknown" for a formula not learned) and, for a learned one, each
    /// ingredient's `have/need`.
    pub texts: Vec<PanelText>,
    /// "Requires ..." at (15, h - 40), size 9, red (1, 0.25, 0.25, 1), when the station is
    /// missing.
    pub station_text: Option<PanelText>,
    /// The craft button's `frame` child colour (0x00633d70 "frame", 0x0040f8e0 / 0x004288e0):
    /// (0, 1, 1, 1) enabled and hovered, (0, 1, 0, 1) enabled, (1, 1, 1, 1) disabled. `None`
    /// without a button.
    pub button_color: Option<[f32; 4]>,
    /// The craft bar is shown (`timer >= 0`); `None` without a bar.
    pub bar_visible: Option<bool>,
}

/// `BlueprintPreviewWidget::update` 0x0042f910 (slot 1). Mutates the widget: the yaw, the
/// hovered ingredient, the tooltip item and the button's hot flag.
///
/// 1. The model: `modelForItem(result)`, or model 0x95c (`GC+0x304 + 0x2570`) for a formula
///    not learned (0x00444a90); no model: nothing more happens this frame.
/// 2. `yaw += dt_ms * 0.02`; hovered ingredient -1; the tooltip's type byte 0.
/// 3. The cursor over the result (`w/2 - 60 < x < w/2 + 60`, `50 < y < 170`, local): the
///    tooltip is the result and the model is drawn 1.5 times larger.
/// 4. A learned formula: the ingredients on a grid of `cols = (int)((w - 10) / 45)` columns,
///    cell `i` at `x = (int)((i % cols) * 45 + left + 10)`, `y = (int)((i / cols) * 45 + (h -
///    130 + top) + 10)` in screen space; the cursor inside `x..x+40, y..y+40` hovers it (index
///    and tooltip).
/// 5. The name: not learned, "Unknown" at (15, 65) size 15; learned, the item name plus, when
///    [`shows_level`], " " and the displayed level ([`item_level`]), centred at (w/2, 30),
///    size 12, wrapped to `w`; then per ingredient `have/need` at (`(i % cols) * 45 + 50`,
///    `(int)(h - 130 + (i / cols) * 45 + 55)`), size 10, right-aligned, green (0, 1, 0, 1)
///    when enough, else white (and the craft is blocked).
/// 6. The station ([`station`]) missing: "Requires ..." and the craft is blocked.
/// 7. The button: enabled when not blocked and the formula is learned; hot when also
///    hovered.
/// 8. The bar: shown while `timer >= 0`.
pub fn preview_frame(p: &mut BlueprintPreview, i: &PreviewInputs) -> BlueprintFrame {
    let mut f = BlueprintFrame::default();
    let item = p.recipe.item.clone();
    let known = knows_formula(i.game, &item);
    let model = if known { crate::interact::item_model_index(&item) } else { Some(0x95c) };
    let Some(model) = model else { return f };
    let (w, h) = (i.size.x, i.size.y);
    let (left, top) = (i.origin.x, i.origin.y);
    // 0x0042fd72: the yaw advances.
    p.yaw = i.dt_ms as f32 * 0.02f32 + p.yaw;
    p.hovered_ingredient = -1;
    p.tooltip[0] = 0;
    let mut scale = 0.06f32;
    let c = i.cursor_local;
    if w * 0.5 - 60.0 < c.x && 50.0 < c.y && c.x < w * 0.5 + 60.0 && c.y < 170.0 {
        p.tooltip = item.clone();
        scale *= 1.5;
    }
    f.model = Some(ModelDraw { item: item.clone(), model, center: [left + w * 0.5, top + 110.0], scale, angles: [p.pitch, p.roll, p.yaw] });
    // 0x004309be: the grid.
    let cols = ((w - 10.0) / 45.0) as i32;
    // Performance note for a future optimiser: `cols` 0 would fault in the original (idiv).
    let cols = cols.max(1);
    let grid_top = (h - 130.0) + top;
    if known {
        for (k, ing) in p.recipe.ingredients.iter().enumerate() {
            let k = k as i32;
            let x = ((k % cols * 45) as f32 + left + 10.0) as i32;
            let y = ((k / cols * 45) as f32 + grid_top + 10.0) as i32;
            let cs = i.cursor_screen;
            let mut s = 0.025f32;
            if x as f32 <= cs.x && cs.x < (x + 40) as f32 && y as f32 <= cs.y && cs.y < (y + 40) as f32 {
                p.hovered_ingredient = k;
                p.tooltip = ing.item.clone();
                s = 0.0375;
            }
            f.ingredients.push(IngredientCell {
                icon: ItemIcon { item: ing.item.clone(), center: [(x + 20) as f32, (y + 20) as f32], scale: s },
                rect: [x as f32, y as f32, (x + 40) as f32, (y + 40) as f32],
                have: 0,
                need: ing.count,
            });
        }
    }
    let mut blocked_ingredients = false;
    if !known {
        f.texts.push(PanelText::new("Unknown", 15.0, 65.0, 15.0, 3.0, WHITE, 0));
    } else {
        let mut name = (i.item_name)(&item);
        if shows_level(&item) {
            name.push(' ');
            name.push_str(&item_level(&item).to_string());
        }
        f.texts.push(PanelText::new(name, w * 0.5, 30.0, 12.0, 3.0, WHITE, 0x11));
        for (k, ing) in p.recipe.ingredients.iter().enumerate() {
            let have = have_count(i.game, &ing.item, true);
            let k = k as i32;
            let x = (k % cols * 45 + 0x32) as f32;
            let y = ((h - 130.0) + (k / cols * 45) as f32 + 55.0) as i32 as f32;
            let color = if have < ing.count {
                blocked_ingredients = true;
                WHITE
            } else {
                [0.0, 1.0, 0.0, 1.0]
            };
            // `have << "/" << need << endl`.
            f.texts.push(PanelText::new(format!("{have}/{}\n", ing.count), x, y, 10.0, 2.0, color, 2));
            if let Some(cell) = f.ingredients.get_mut(k as usize) {
                cell.have = have;
            }
        }
    }
    let mut blocked_station = false;
    if let Some(st) = station(p.recipe.station) {
        if !(i.station_ok)(st) {
            blocked_station = true;
            let text = match st {
                Station::Water => "Requires water",
                Station::Static { text, .. } => text,
            };
            f.station_text = Some(PanelText::new(text, 15.0, h - 40.0, 9.0, 3.0, [1.0, 0.25, 0.25, 1.0], 0x10));
        }
    }
    // 0x00431f28: the craft button.
    if i.has_button {
        if !blocked_station && !blocked_ingredients && known {
            if i.button_hovered {
                f.button_color = Some(CYAN);
                p.button_hot = true;
            } else {
                f.button_color = Some([0.0, 1.0, 0.0, 1.0]);
                p.button_hot = false;
            }
        } else {
            f.button_color = Some(WHITE);
            p.button_hot = false;
        }
    }
    // 0x00434711: the bar.
    if i.has_bar {
        f.bar_visible = Some(i.timer >= 0.0);
    }
    f
}

/// Where `BlueprintPreviewWidget::layout` 0x004348f0 puts the children, from the widget size
/// and the children's sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewLayout {
    /// `itemshadow` (`+0x298`, reparented to the widget's node when needed): `((w - sw) / 2,
    /// 90)`.
    pub shadow: Vec2,
    /// `craftbutton` (`+0x29c`): `(w - bw - 20, h - bh - 20)`.
    pub button: Vec2,
    /// `craftbar` (`+0x2a0`, with a button): position `(20, h - bh - 20)`, size `(w - bw - 40
    /// - 10, bh)` (0x0062bb20).
    pub bar: (Vec2, Vec2),
}

/// `BlueprintPreviewWidget::layout` 0x004348f0. The tree applies it through cw-ui's
/// `Gui::layout` (the widget's `+0x298/+0x29c/+0x2a0` members registered by
/// `members::construct`, run when the attach 0x00631460 sizes the widget to its panel); this
/// is the same geometry as a value, for callers and tests.
pub fn preview_layout(size: Vec2, shadow_size: Vec2, button_size: Vec2) -> PreviewLayout {
    let (w, h) = (size.x, size.y);
    PreviewLayout {
        shadow: Vec2::new((w - shadow_size.x) * 0.5, 90.0),
        button: Vec2::new(w - button_size.x - 20.0, h - button_size.y - 20.0),
        bar: (Vec2::new(20.0, h - button_size.y - 20.0), Vec2::new(w - button_size.x - 40.0 - 10.0, button_size.y)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(t: u8, sub: u8, mat: u8, rarity: u8, level: i16) -> Vec<u8> {
        let mut v = default_item();
        v[0] = t;
        v[1] = sub;
        v[0xd] = mat;
        v[0xc] = rarity;
        v[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        v
    }

    fn ing(sub: u8, mat: u8) -> Vec<u8> {
        item(0xb, sub, mat, 0, 1)
    }

    #[test]
    fn recipe_table() {
        // A sword: 8 iron bars (0xb/0xa material 1) at the anvil.
        let r = recipe_for(&item(3, 0, 1, 0, 1));
        assert_eq!(r.station, 4);
        assert_eq!(r.ingredients, vec![Ingredient { item: ing(0xa, 1), count: 8 }]);
        // A bow (3/6): 15 wood (material 2), 1 thread of 0x1a, at the workbench.
        let r = recipe_for(&item(3, 6, 2, 0, 1));
        assert_eq!(r.station, 8);
        assert_eq!(r.ingredients.len(), 2);
        assert_eq!((r.ingredients[1].item[1], r.ingredients[1].item[0xd], r.ingredients[1].count), (9, 0x1a, 1));
        // Cloth armour: 20 of its material (sub 9) at the loom; rarity 2 adds 2 of (0, 2, 0xe).
        let r = recipe_for(&item(4, 0, 0x19, 2, 1));
        assert_eq!(r.station, 6);
        assert_eq!(r.ingredients[0], Ingredient { item: ing(9, 0x19), count: 0x14 });
        let mut gem = ing(0, 0xe);
        gem[0xc] = 2;
        assert_eq!(r.ingredients[1], Ingredient { item: gem, count: 2 });
        // Ingredient 0xb/0x1a: water (station 1).
        assert_eq!(recipe_for(&item(0xb, 0x1a, 0, 0, 1)).station, 1);
        // A formula result without a case: nothing.
        assert!(recipe_for(&item(2, 0, 0, 0, 1)).ingredients.is_empty());
    }

    #[test]
    fn tabs_sorted_and_counted() {
        let mut game = GameView::default();
        game.formulas = vec![item(3, 0, 1, 0, 1), item(3, 1, 1, 0, 5), item(4, 0, 1, 0, 1), item(2, 0, 0, 0, 1)];
        game.inventory = vec![vec![], vec![], vec![ItemStack { count: 17, item: ing(0xa, 1) }]];
        let mut st = CraftingState::default();
        refresh(&mut st, &game, (0, 0));
        assert_eq!(st.tabs.len(), 6);
        // Weapons: level 5 first (descending key), 17 / 8 = 2 craftable.
        assert_eq!(st.tabs[0].len(), 2);
        assert_eq!(i16_at(&st.tabs[0][0].item, 0x10), 5);
        assert_eq!(st.tabs[0][0].count, 2);
        // Armour: 17 / 20 = 0.
        assert_eq!(st.tabs[1][0].count, 0);
        // The selection (0, 0) is shown in the preview.
        assert_eq!(i16_at(&st.preview.recipe.item, 0x10), 5);
    }

    #[test]
    fn craft_consumes_and_rolls() {
        let mut game = GameView::default();
        game.inventory = vec![vec![], vec![], vec![ItemStack { count: 10, item: ing(0xa, 1) }]];
        let mut st = CraftingState::default();
        st.crafting = recipe_for(&item(3, 0, 1, 0, 1));
        let mut rng = MsvcRand::new(1);
        let a = craft(&mut st, &mut game, &mut rng);
        assert_eq!(game.inventory[2][0].count, 2);
        assert!(a.contains(&snd(0x59)));
        // The weapon went to tab 0 with the crafted flag.
        assert_eq!(game.inventory[0][0].item[0], 3);
        assert_eq!(game.inventory[0][0].item[0xe] & 1, 1);
        assert!(a.contains(&UiAction::RebuildRecipes));
        // Not enough: the stack is emptied (the original does not check ingredients here).
        let a = craft(&mut st, &mut game, &mut rng);
        assert_eq!(game.inventory[2][0].count, 0);
        assert!(a.contains(&snd(0x59)));
    }

    #[test]
    fn timer_runs_to_craft() {
        use super::super::members::NoPlx;
        let game0 = GameView::default();
        let mut ui = super::super::GameUi::new(Gui::new(), &mut NoPlx, &game0);
        let mut game = GameView::default();
        let mut st = CraftingState::default();
        let mut rng = MsvcRand::new(1);
        set_vis(&mut ui.gui, ui.m.craft_preview_panel, true);
        st.preview.recipe = recipe_for(&item(1, 5, 0, 0, 1));
        st.preview.button_hot = true;
        assert!(update_timer(&ui.gui, &ui.m, &mut st, &mut game, &mut rng, 500, true).is_empty());
        assert_eq!(st.timer, 0.5);
        let a = update_timer(&ui.gui, &ui.m, &mut st, &mut game, &mut rng, 500, false);
        assert_eq!(st.timer, -1.0);
        assert!(a.contains(&snd(0x59)));
        assert_eq!(bar_fill(0.5, 108.0), 50.0);
    }

    /// The attach sizes the preview widget to its panel and cw-ui's layout puts the parts where
    /// [`preview_layout`] says. Skipped without `CW_GAME_DIR`.
    #[test]
    fn preview_parts_follow_the_layout() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else { return };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let ui = super::super::GameUi::new(gui, &mut plx, &GameView::default());
        let w = ui.m.craft_preview.unwrap();
        let size = Vec2::new(ui.gui.width(w), ui.gui.height(w));
        assert_eq!(size, Vec2::new(350.0, 330.0));
        let b = ui.m.craft_button.and_then(|n| ui.gui.nodes[n].widget).unwrap();
        let s = ui.m.craft_itemshadow.and_then(|n| ui.gui.nodes[n].widget).unwrap();
        let bs = Vec2::new(ui.gui.width(b), ui.gui.height(b));
        let l = preview_layout(size, Vec2::new(ui.gui.width(s), ui.gui.height(s)), bs);
        // (The positions round-trip through the node translations: compare to 1e-3.)
        let near = |a: Vec2, b: Vec2| (a - b).abs().max_element() < 1e-3;
        assert!(near(ui.gui.get_position(b), l.button), "{:?} {:?}", ui.gui.get_position(b), l.button);
        assert!(near(ui.gui.get_position(s), l.shadow));
        if let Some(bar) = ui.m.craft_bar.and_then(|n| ui.gui.nodes[n].widget) {
            assert!(near(ui.gui.get_position(bar), l.bar.0));
        }
        // The InventoryWidgets fill their panels too (the recipe grid has rows).
        let c = ui.m.crafting.unwrap();
        assert_eq!(Vec2::new(ui.gui.width(c), ui.gui.height(c)), Vec2::new(350.0, 285.0));
    }

    #[test]
    fn preview_button_state() {
        let mut game = GameView::default();
        let sword = item(3, 0, 1, 0, 1);
        game.formulas = vec![sword.clone()];
        game.inventory = vec![vec![], vec![], vec![ItemStack { count: 9, item: ing(0xa, 1) }]];
        let mut p = BlueprintPreview { recipe: recipe_for(&sword), ..Default::default() };
        let name = |_: &[u8]| "Sword".to_string();
        let near = |_: Station| true;
        let far = |_: Station| false;
        let mut inp = PreviewInputs {
            game: &game,
            origin: Vec2::new(100.0, 100.0),
            size: Vec2::new(350.0, 330.0),
            cursor_local: Vec2::new(-1.0, -1.0),
            cursor_screen: Vec2::new(-1.0, -1.0),
            dt_ms: 16,
            station_ok: &near,
            item_name: &name,
            has_button: true,
            button_hovered: true,
            has_bar: true,
            timer: -1.0,
        };
        let f = preview_frame(&mut p, &inp);
        assert_eq!(f.button_color, Some(CYAN));
        assert!(p.button_hot);
        assert_eq!(f.texts[0].text, "Sword 1");
        assert_eq!(f.texts[1].text, "9/8\n");
        assert_eq!(f.bar_visible, Some(false));
        // The ingredient cell: cols = 7, first cell at (110, 100 + 200 + 10).
        assert_eq!(f.ingredients[0].rect, [110.0, 310.0, 150.0, 350.0]);
        inp.station_ok = &far;
        let f = preview_frame(&mut p, &inp);
        assert_eq!(f.station_text.as_ref().unwrap().text, "Requires Anvil");
        assert_eq!(f.button_color, Some(WHITE));
        assert!(!p.button_hot);
        // Hovering the ingredient cell.
        inp.station_ok = &near;
        inp.cursor_screen = Vec2::new(120.0, 320.0);
        preview_frame(&mut p, &inp);
        assert_eq!(p.hovered_ingredient, 0);
        assert_eq!(p.tooltip_item(true).unwrap()[1], 0xa);
    }
}
