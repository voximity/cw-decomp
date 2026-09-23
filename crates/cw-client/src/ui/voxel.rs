//! Weapon customization: `cube::VoxelWidget` (vtable 0x0071a664, ctor 0x00587f70, 0x440
//! bytes; panel `GC+0x8008dc`, widget `GC+0x8008f4`) and the GameController code around it.
//!
//! Tier B. The panel opens at the customization bench (static kind 0x4d, R, `update`
//! 0x0049743c). A right/middle click on a weapon (type 3) in the bag or the equipment makes it
//! the target (the inventory's [`UiAction::SetItemTarget`], [`set_target`]). The widget shows
//! the weapon's voxel model with its "spirits" (upgrade cubes stored in the item: up to 32
//! records of 8 bytes at `Item+0x14`, `x, y, z, material, level (i32)`, the count at
//! `Item+0x114`) and a list of the upgrade materials the bag holds ("Upgrades"). Clicking a
//! list cell selects that material; clicking the model then places one cube of it on the face
//! under the cursor and takes one from the bag. Clicking an existing spirit picks it up;
//! the next click moves it. Middle-drag rotates the model. There is no separate save: the
//! item bytes are edited in place.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | reset 0x0058ce20 | [`VoxelWidget::reset`] |
//! | select the hovered list cell 0x0058ce40 | [`VoxelWidget::toggle_selection`] |
//! | click on the model 0x00588250 | [`place`] |
//! | `Item::setSpirit` 0x004c69b0 / `Item::removeSpirit` 0x004c79b0 | [`set_spirit`], [`remove_spirit`] |
//! | max spirits 0x004c7660 | [`max_spirits`] |
//! | `VoxelWidget::update` (slot 1) 0x00588500: list 0x00588500..0x00588c7d, caption item ..0x00588cc4, model and pick ..0x0058a89a, preview and spirits ..0x0058b5c0, list cells, texts | [`build_list`], [`pick`], [`frame`] |
//! | `onMouseDown` 0x0047bcee..0x0047bd3f (left button) | [`left_click`] |
//! | `onMouseDown` 0x0047cd16 (right button prelude) | [`right_click_prelude`] |
//! | `onMouseDown` 0x0047cf6c / 0x0047d99d (target) | [`set_target`] |
//! | `onMouseMove` 0x0047ede4..0x0047ee7c | [`on_mouse_move`] |
//! | `update` 0x0049743c..0x0049747e (R on static 0x4d) | [`open_from_static`] |
//! | `update` 0x0048b4ff..0x0048b627 (cursor caption) | [`cursor_caption`] |
//! | tooltip 0x0047b3e0 (voxel half) | [`tooltip_item`] |

use cw_ui::widget::Gui;
use glam::Vec2;

use super::crafting::{default_item, i32_at, set_vis, shows_level, vis, ItemIcon, ModelDraw, PanelText, WHITE};
use super::inventory::{item_level, same_item, take_one, ItemRef};
use super::members::GcMembers;
use super::{GameView, ItemStack};

/// `cube::VoxelWidget` fields (ctor 0x00587f70).
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelWidget {
    /// `+0x160`: the weapon being customized (an item pointer into the bag or the equipment).
    pub target: Option<ItemRef>,
    /// `+0x164`, `+0x168`, `+0x16c`: model angles in degrees (-90, 0, 90).
    pub angles: [f32; 3],
    /// `+0x170`: the upgrade list rebuilt every frame ([`build_list`]): (count, item).
    pub list: Vec<ItemStack>,
    /// `+0x17c`: the selected list cell, -1 none.
    pub selected: i32,
    /// `+0x180`: the caption item (the selected material, the carried spirit or the hovered
    /// spirit; type 0 none).
    pub caption: Vec<u8>,
    /// `+0x29c/+0x2a0/+0x2a4`: the voxel cell under the cursor (moved to the free neighbour
    /// while placing).
    pub cell: [i32; 3],
    /// `+0x2a8`: the ray hit the model or a spirit.
    pub hit: bool,
    /// `+0x2ac`: the list cell under the cursor, -1 none.
    pub hovered: i32,
    /// `+0x2b0`: its item (the tooltip).
    pub hovered_item: Vec<u8>,
    /// `+0x3c8`: a spirit is picked up (carried).
    pub carrying: bool,
    /// `+0x3cc/+0x3d0/+0x3d4`: the carried spirit's cell.
    pub carried_cell: [i32; 3],
    /// `+0x3d8`: its material.
    pub carried_material: i32,
    /// `+0x3dc`: its level.
    pub carried_level: i32,
}

impl Default for VoxelWidget {
    fn default() -> Self {
        VoxelWidget {
            target: None,
            angles: [-90.0, 0.0, 90.0],
            list: Vec::new(),
            selected: -1,
            caption: default_item(),
            cell: [0; 3],
            hit: false,
            hovered: -1,
            hovered_item: default_item(),
            carrying: false,
            carried_cell: [0; 3],
            carried_material: 0,
            carried_level: 0,
        }
    }
}

impl VoxelWidget {
    /// `0x0058ce20`: no selection, nothing carried.
    pub fn reset(&mut self) {
        self.selected = -1;
        self.carrying = false;
    }

    /// `0x0058ce40`: the hovered list cell becomes the selection, or clears it when it is the
    /// selection already.
    pub fn toggle_selection(&mut self) {
        self.selected = if self.selected == self.hovered { -1 } else { self.hovered };
    }
}

fn target_ref<'a>(game: &'a GameView, t: ItemRef) -> Option<&'a Vec<u8>> {
    match t {
        ItemRef::Equipment(k) => game.equipment.get(k),
        ItemRef::Bag { tab, index } => game.inventory.get(tab as usize)?.get(index as usize).map(|s| &s.item),
    }
}

fn target_mut<'a>(game: &'a mut GameView, t: ItemRef) -> Option<&'a mut Vec<u8>> {
    match t {
        ItemRef::Equipment(k) => game.equipment.get_mut(k),
        ItemRef::Bag { tab, index } => game.inventory.get_mut(tab as usize)?.get_mut(index as usize).map(|s| &mut s.item),
    }
}

/// The weapon ([`VoxelWidget::target`]).
pub fn target_item<'a>(game: &'a GameView, w: &VoxelWidget) -> Option<&'a [u8]> {
    target_ref(game, w.target?).map(|v| &v[..])
}

/// `0x004c7660`: how many spirits an item holds, 32 for a two-handed weapon (sub types 0xf,
/// 0x10, 0x11, 5, 0xa, 0xb, 0x12, 8, 6, 7), else 16.
pub fn max_spirits(item: &[u8]) -> i32 {
    if item[0] == 3 && matches!(item[1], 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7) { 0x20 } else { 0x10 }
}

/// The spirit count, `Item+0x114`.
pub fn spirit_count(item: &[u8]) -> i32 {
    i32_at(item, 0x114)
}

/// Spirit `i`: `(x, y, z, material, level)` from `Item+0x14 + 8i`.
pub fn spirit(item: &[u8], i: usize) -> ([u8; 3], u8, i32) {
    let o = 0x14 + i * 8;
    ([item[o], item[o + 1], item[o + 2]], item[o + 3], i32_at(item, o + 4))
}

/// `Item::setSpirit(x, y, z, material, level)` 0x004c69b0: a spirit at the same `(x, y, z)`
/// bytes gets the material and level; otherwise, below [`max_spirits`], a new record is
/// appended.
pub fn set_spirit(item: &mut [u8], c: [i32; 3], material: u8, level: i32) {
    let n = spirit_count(item);
    let key = [c[0] as u8, c[1] as u8, c[2] as u8];
    for i in 0..n.clamp(0, 32) as usize {
        let o = 0x14 + i * 8;
        if item[o..o + 3] == key {
            item[o + 3] = material;
            item[o + 4..o + 8].copy_from_slice(&level.to_le_bytes());
            return;
        }
    }
    if n < max_spirits(item) {
        let o = 0x14 + n as usize * 8;
        item[o..o + 3].copy_from_slice(&key);
        item[o + 3] = material;
        item[o + 4..o + 8].copy_from_slice(&level.to_le_bytes());
        item[0x114..0x118].copy_from_slice(&(n + 1).to_le_bytes());
    }
}

/// `Item::removeSpirit(x, y, z)` 0x004c79b0: the first spirit whose sign-extended coordinate
/// bytes equal `c` is removed, the later records shifted down.
pub fn remove_spirit(item: &mut [u8], c: [i32; 3]) {
    let n = spirit_count(item);
    for i in 0..n.clamp(0, 32) as usize {
        let o = 0x14 + i * 8;
        let s = [item[o] as i8 as i32, item[o + 1] as i8 as i32, item[o + 2] as i8 as i32];
        if s == c {
            for j in i..(n - 1).clamp(0, 31) as usize {
                let (a, b) = (0x14 + j * 8, 0x14 + (j + 1) * 8);
                item.copy_within(b..b + 8, a);
            }
            item[0x114..0x118].copy_from_slice(&(n - 1).to_le_bytes());
            return;
        }
    }
}

/// The start of `VoxelWidget::update` 0x00588500: the upgrade list, rebuilt from the bag. With
/// a weapon: first the weapon's own material (type 0xb, sub type 0xa, level 1, material
/// `weapon+0xd`) with the summed count of equal bag stacks, when not 0; then for each displayed
/// level `lvl` from `max(1, item_level(weapon) - 10)` to `item_level(weapon)` and each spirit
/// material 0x80..0x83, the sum over bag stacks of type 0xb, sub type 0xe, that material and
/// that displayed level; a non-zero sum adds (type 0xb, sub type 0xe, material, level
/// `(i16)(int)levelCurveInv(lvl * 0.01)`).
pub fn build_list(game: &GameView, weapon: Option<&[u8]>) -> Vec<ItemStack> {
    let mut list = Vec::new();
    let Some(weapon) = weapon else { return list };
    let mut t = default_item();
    t[0] = 0xb;
    t[1] = 0xa;
    t[0xd] = weapon[0xd];
    let mut n = 0i32;
    for tab in &game.inventory {
        for s in tab {
            if same_item(&s.item, &t) {
                n = n.wrapping_add(s.count);
            }
        }
    }
    if n != 0 {
        list.push(ItemStack { count: n, item: t.clone() });
    }
    let top = item_level(weapon);
    let mut lvl = top - 10;
    if lvl < 1 {
        lvl = 1;
    }
    while lvl <= top {
        for m in 0x80..0x84u8 {
            let mut sum = 0i32;
            for tab in &game.inventory {
                for s in tab {
                    if s.item[0] == 0xb && s.item[1] == 0xe && s.item[0xd] == m && item_level(&s.item) == lvl {
                        sum = sum.wrapping_add(s.count);
                    }
                }
            }
            if sum != 0 {
                let mut it = default_item();
                it[0] = 0xb;
                it[1] = 0xe;
                it[0xd] = m;
                let l = cw_world::generate::level_curve_inv(lvl as f32 * 0.01f32) as i32 as i16;
                it[0x10..0x12].copy_from_slice(&l.to_le_bytes());
                list.push(ItemStack { count: sum, item: it });
            }
        }
        lvl += 1;
    }
    list
}

/// The voxel model of the weapon (its `cube::Model`: dimensions `+0x44/+0x48/+0x4c`, 3 bytes
/// per voxel at `+0x30`).
pub trait VoxelModel {
    /// The dimensions.
    fn dims(&self) -> [i32; 3];
    /// `0x004e71d0(voxel, 0)`: the voxel is empty (equal to the empty voxel). Only called in
    /// bounds; outside the model the shared empty voxel `0x0076b340` is used (empty).
    fn is_empty(&self, x: i32, y: i32, z: i32) -> bool;
}

/// The ray walk of 0x0058a3f0..0x0058a89a: `origin` and the unit `dir` are the cursor ray in
/// model voxel space (the renderer's unprojection of the cursor through the model's
/// transform, see [`VoxelFrame::model`]). Starting in the cell `(int)origin` (minus one on each
/// negative axis), each step tests the cell: a solid voxel, or a spirit record whose
/// coordinate bytes equal the cell's low bytes (ignoring the carried one), is a hit (a spirit
/// also becomes the caption item: type 0xb, sub type 0xe, its material and level, rarity 2).
/// Otherwise the walk moves to the nearest cell boundary (x, then y, then z on ties, from a
/// start of 10), stepping the cell and remembering the opposite direction, until 1000 units
/// are travelled. Sets [`VoxelWidget::cell`] and [`VoxelWidget::hit`]; returns the step back
/// towards the viewer (the free neighbour of a hit).
pub fn pick(w: &mut VoxelWidget, weapon: &[u8], model: &dyn VoxelModel, origin: [f32; 3], dir: [f32; 3]) -> [i32; 3] {
    let dims = model.dims();
    let mut cell = [origin[0] as i32, origin[1] as i32, origin[2] as i32];
    for a in 0..3 {
        if origin[a] < 0.0 {
            cell[a] -= 1;
        }
    }
    let mut back = [0i32; 3];
    let mut travelled = 0.0f32;
    w.hit = false;
    loop {
        let inside = (0..3).all(|a| cell[a] >= 0 && cell[a] < dims[a]);
        let empty = !inside || model.is_empty(cell[0], cell[1], cell[2]);
        if !empty {
            w.hit = true;
        }
        let key = [cell[0] as u8, cell[1] as u8, cell[2] as u8];
        let carried = [w.carried_cell[0] as u8, w.carried_cell[1] as u8, w.carried_cell[2] as u8];
        for i in 0..spirit_count(weapon).clamp(0, 32) as usize {
            let (c, m, l) = spirit(weapon, i);
            if w.carrying && c == carried {
                continue;
            }
            if c == key {
                w.hit = true;
                w.caption[0] = 0xb;
                w.caption[1] = 0xe;
                w.caption[0xd] = m;
                w.caption[0x10..0x12].copy_from_slice(&(l as i16).to_le_bytes());
                w.caption[0xc] = 2;
            }
        }
        if w.hit {
            break;
        }
        let mut best = 10.0f32;
        let mut axis = 0usize;
        for a in 0..3 {
            if dir[a] != 0.0 {
                let b = if 0.0 < dir[a] { cell[a] + 1 } else { cell[a] };
                let t = (b as f32 - (origin[a] + dir[a] * travelled)) / dir[a];
                if t < best {
                    best = t;
                    axis = a;
                }
            }
        }
        back = [0; 3];
        if dir[axis] <= 0.0 {
            cell[axis] -= 1;
            back[axis] = 1;
        } else {
            cell[axis] += 1;
            back[axis] = -1;
        }
        travelled += best;
        if !(travelled < 1000.0) {
            break;
        }
    }
    w.cell = cell;
    back
}

/// `0x00588250`, a left click on the model (no list cell hovered).
///
/// - No hit or no weapon: with no widget under the cursor (`over_widget` false, 0x00650ae0)
///   a carried spirit is removed from the weapon; then the selection and the carrying end.
/// - No valid selection: carrying, the spirit moves from its old cell to the hovered one
///   (remove, then set with the saved material and level) and the carrying ends; not
///   carrying, a spirit at the hovered cell is picked up (it stays in the item until moved).
/// - A selection, below 32 spirits (not [`max_spirits`]): a spirit of the selected material
///   and level is set at the hovered cell, and the first bag stack with a count, the same type,
///   sub type and material and the same displayed level loses one.
pub fn place(w: &mut VoxelWidget, game: &mut GameView, over_widget: bool) {
    let Some(t) = w.target.filter(|_| w.hit) else {
        if !over_widget && w.carrying {
            if let Some(item) = w.target.and_then(|t| target_mut(game, t)) {
                remove_spirit(item, w.carried_cell);
            }
            w.carrying = false;
        }
        w.selected = -1;
        w.carrying = false;
        return;
    };
    let sel = w.selected;
    if sel < 0 || sel as usize >= w.list.len() {
        if w.carrying {
            let (old, cell, m, l) = (w.carried_cell, w.cell, w.carried_material as u8, w.carried_level);
            if let Some(item) = target_mut(game, t) {
                remove_spirit(item, old);
                set_spirit(item, cell, m, l);
            }
            w.carrying = false;
            return;
        }
        let Some(item) = target_ref(game, t) else { return };
        let key = [w.cell[0] as u8, w.cell[1] as u8, w.cell[2] as u8];
        for i in 0..spirit_count(item).clamp(0, 32) as usize {
            let (c, m, l) = spirit(item, i);
            if c == key {
                w.carrying = true;
                w.carried_material = i32::from(m);
                w.carried_level = l;
                w.carried_cell = w.cell;
                return;
            }
        }
        return;
    }
    let Some(item) = target_mut(game, t) else { return };
    if spirit_count(item) >= 0x20 {
        return;
    }
    let pick = w.list[sel as usize].item.clone();
    set_spirit(item, w.cell, pick[0xd], i32::from(super::crafting::i16_at(&pick, 0x10)));
    for tab in game.inventory.iter_mut() {
        for s in tab.iter_mut() {
            if s.count != 0 && s.item[0] == pick[0] && s.item[1] == pick[1] && s.item[0xd] == pick[0xd] && item_level(&pick) == item_level(&s.item) {
                take_one(s);
                return;
            }
        }
    }
}

/// `onMouseDown` 0x0047bcee..0x0047bd3f (left button, after the crafting part): with the panel
/// open and a weapon, a hovered list cell toggles the selection ([`VoxelWidget::toggle_selection`]),
/// else the click goes to the model ([`place`]).
pub fn left_click(gui: &Gui, m: &GcMembers, w: &mut VoxelWidget, game: &mut GameView, over_widget: bool) {
    if !(vis(gui, m.voxel_panel) && w.target.is_some()) {
        return;
    }
    if w.hovered >= 0 {
        w.toggle_selection();
    } else {
        place(w, game, over_widget);
    }
}

/// `onMouseDown` 0x0047ccff..0x0047cd39 (right button, before the bag routing): with
/// `GC+0x800704` clear and the panel open, [`VoxelWidget::reset`]. `GC+0x800704` is build
/// mode: written only by the GameController ctor (0 at 0x0045a09b), so always false in this
/// build (see `crate::interact::build_r`). With it set the original would increment the
/// player's build block type `creature+0x18c` instead.
pub fn right_click_prelude(gui: &Gui, m: &GcMembers, w: &mut VoxelWidget, flag_800704: bool) {
    if !flag_800704 && vis(gui, m.voxel_panel) {
        w.reset();
    }
}

/// `onMouseDown` 0x0047cf84 / 0x0047d9be ([`super::UiAction::SetItemTarget`] with
/// `TargetPanel::Voxel`; only weapons are offered): the weapon becomes the target and the
/// widget resets.
pub fn set_target(w: &mut VoxelWidget, item: ItemRef) {
    w.target = Some(item);
    w.reset();
}

/// `onMouseMove` 0x0047ede4 (in the cursor-free branch): with the middle button held
/// (`GC+0xa`) and the panel open, `angles[2] += (x - prev_x) * 0.5` and `angles[0] -= (y -
/// prev_y) * 0.5` (`Engine+0xd4/+0xd8` minus `+0xdc/+0xe0`).
pub fn on_mouse_move(gui: &Gui, m: &GcMembers, w: &mut VoxelWidget, middle_held: bool, delta: Vec2) {
    if middle_held && vis(gui, m.voxel_panel) {
        w.angles[2] = delta.x * 0.5 + w.angles[2];
        w.angles[0] -= delta.y * 0.5;
    }
}

/// R on the customization bench (static kind 0x4d, `update` 0x0049743c; the gameplay side's
/// `Event::OpenStaticPanel { kind: 0x4d }`): the widget resets, the panel flips and the
/// inventory panel follows it.
pub fn open_from_static(gui: &mut Gui, m: &GcMembers, w: &mut VoxelWidget) {
    w.reset();
    let v = !vis(gui, m.voxel_panel);
    set_vis(gui, m.voxel_panel, v);
    set_vis(gui, m.inventory_panel, v);
}

/// `update` 0x0048b4ff..0x0048b627: with the panel open and a caption item (type not 0), the
/// cursor caption (`GC+0x8008a8`, replacing the held-stack count) is the item name followed,
/// when [`shows_level`], by " +" and the displayed level.
pub fn cursor_caption(gui: &Gui, m: &GcMembers, w: &VoxelWidget, item_name: &dyn Fn(&[u8]) -> String) -> Option<String> {
    if !(vis(gui, m.voxel_panel) && w.caption[0] != 0) {
        return None;
    }
    let mut s = item_name(&w.caption);
    if shows_level(&w.caption) {
        s.push_str(" +");
        s.push_str(&item_level(&w.caption).to_string());
    }
    Some(s)
}

/// The voxel half of 0x0047b3e0: with the panel open, the hovered list cell's item.
pub fn tooltip_item<'a>(gui: &Gui, m: &GcMembers, w: &'a VoxelWidget) -> Option<&'a [u8]> {
    (vis(gui, m.voxel_panel) && w.hovered >= 0).then_some(&w.hovered_item[..])
}

/// The inputs of [`frame`].
pub struct VoxelInputs<'a> {
    /// The widget's screen origin (its transform applied to (0, 0)). The original reads the
    /// widget node's Transformation pivot (`+0x38 → +0x19c[+0x170]`, 0x00588caf) through the
    /// node's world matrix (`node+0x48`, divided by w) at 0x00588eb3 (the model centre
    /// `(w · 0.5 + p.x, h · 0.6 + p.y)`) and 0x0058b4d5 (the list base `p + (20, 40)`); with
    /// the shipped `gui.plx` that point equals the widget origin (the node has pivot 0 and an
    /// identity bind/frame), so the caller passes the widget origin.
    pub origin: Vec2,
    /// The widget size.
    pub size: Vec2,
    /// The engine cursor in screen space.
    pub cursor: Vec2,
    /// `Engine+0xe8`: engine time in ms (the hovered spirit pulses).
    pub time_ms: i32,
    /// The weapon's model (`0x004ec400` non-null), when it has one.
    pub model: Option<&'a dyn VoxelModel>,
    /// Its model-cache index ([`crate::interact::item_model_index`]).
    pub model_index: Option<u32>,
    /// The cursor ray in model voxel space (origin, unit direction), see [`pick`].
    pub ray: Option<([f32; 3], [f32; 3])>,
}

/// One spirit cube drawn on the model (`0x004c7250(material)` gives its colour).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpiritCube {
    /// The voxel cell (sign-extended coordinate bytes).
    pub cell: [i32; 3],
    /// Material (the colour key).
    pub material: u8,
    /// 1.1 for the hovered spirit (scaled about its centre), else 1.
    pub scale: f32,
    /// Colour factor: `cos(time_ms * 0.01) * 0.2 + 1` for the hovered spirit, else 1.
    pub brightness: f32,
}

/// The cube previewing a placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewCube {
    /// The free cell next to the hit, or `None`: floating on the ray 100 units from its origin.
    pub cell: Option<[i32; 3]>,
    /// The material (the selection's or the carried spirit's).
    pub material: u8,
}

/// What the customization panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxelFrame {
    /// The weapon model at `(left + w / 2, top + 0.6 h)`, scale `0.25 / max(dimension)`,
    /// translated by `-dims / 2`, rotated by [`VoxelWidget::angles`] (`+0x164`, `+0x168`,
    /// `+0x16c`), projected with a vertical half-angle `pi / 8` over the client aspect.
    pub model: Option<ModelDraw>,
    /// The spirits (the carried one is not drawn).
    pub spirits: Vec<SpiritCube>,
    /// The placement preview.
    pub preview: Option<PreviewCube>,
    /// The upgrade list icons, screen space: cell `i` at `x = (int)((i % 8) * 45 + left +
    /// 20)`, `y = (int)((i / 8) * 45 + top + 40)`, icon at `+20`, scale 0.02 (0.03 hovered or
    /// selected).
    pub list: Vec<ItemIcon>,
    /// Texts: "Upgrades n/max" (with a weapon), the title, the list counts.
    pub texts: Vec<PanelText>,
}

/// `VoxelWidget::update` 0x00588500.
pub fn frame(w: &mut VoxelWidget, game: &GameView, i: &VoxelInputs) -> VoxelFrame {
    let mut f = VoxelFrame::default();
    w.caption = default_item();
    let weapon = target_item(game, w).map(|v| v.to_vec());
    w.list = build_list(game, weapon.as_deref());
    if let Some(weapon) = weapon.as_deref() {
        // 0x00588c7d: the caption item.
        if w.hovered < 0 {
            if w.carrying {
                w.caption[0] = 0xb;
                w.caption[1] = if w.carried_material > 0x7f { 0xe } else { 0xa };
                w.caption[0x10..0x12].copy_from_slice(&(w.carried_level as i16).to_le_bytes());
                w.caption[0xd] = w.carried_material as u8;
            } else if w.selected >= 0 && (w.selected as usize) < w.list.len() {
                w.caption = w.list[w.selected as usize].item.clone();
            }
        }
        let n = w.list.len() as i32;
        if n <= w.selected {
            w.selected = n - 1;
        }
        if let (Some(model), Some(idx)) = (i.model, i.model_index) {
            f.model = Some(ModelDraw {
                item: weapon.to_vec(),
                model: idx,
                center: [i.origin.x + i.size.x * 0.5, i.origin.y + i.size.y * 0.6],
                scale: 0.25,
                angles: w.angles,
            });
            let back = match i.ray {
                Some((o, d)) => pick(w, weapon, model, o, d),
                None => {
                    w.hit = false;
                    [0; 3]
                }
            };
            // 0x0058a8b9: the preview.
            if w.hovered < 0 && (w.carrying || (w.selected >= 0 && (w.selected as usize) < w.list.len())) {
                let cell = if w.hit {
                    for a in 0..3 {
                        w.cell[a] += back[a];
                    }
                    Some(w.cell)
                } else {
                    None
                };
                let material = if w.carrying { w.carried_material as u8 } else { w.list[w.selected as usize].item[0xd] };
                f.preview = Some(PreviewCube { cell, material });
            }
            let carried = [w.carried_cell[0] as u8, w.carried_cell[1] as u8, w.carried_cell[2] as u8];
            let key = [w.cell[0] as u8, w.cell[1] as u8, w.cell[2] as u8];
            for k in 0..spirit_count(weapon).clamp(0, 32) as usize {
                let (c, m, _) = spirit(weapon, k);
                if w.carrying && c == carried {
                    continue;
                }
                let hovered = w.hit && c == key;
                let pulse = (cw_math::cos(f64::from(i.time_ms as f32 * 0.01f32)) as f32) * 0.2f32 + 1.0f32;
                f.spirits.push(SpiritCube {
                    cell: [c[0] as i8 as i32, c[1] as i8 as i32, c[2] as i8 as i32],
                    material: m,
                    scale: if hovered { 1.1 } else { 1.0 },
                    brightness: if hovered { pulse } else { 1.0 },
                });
            }
        }
        // 0x0058b5c0: the list cells.
        w.hovered = -1;
        let (bx, by) = (i.origin.x + 20.0, i.origin.y + 40.0);
        for (k, s) in w.list.iter().enumerate() {
            let k = k as i32;
            let x = ((k % 8 * 45) as f32 + bx) as i32;
            let y = ((k / 8 * 45) as f32 + by) as i32;
            let c = i.cursor;
            if x as f32 <= c.x && c.x < (x + 40) as f32 && y as f32 <= c.y && c.y < (y + 40) as f32 {
                w.hovered = k;
                w.hovered_item = s.item.clone();
            }
            let scale = if w.hovered == k || w.selected == k { 0.03 } else { 0.02 };
            f.list.push(ItemIcon { item: s.item.clone(), center: [(x + 20) as f32, (y + 20) as f32], scale });
        }
        f.texts.push(PanelText::new(format!("Upgrades {}/{}\n", spirit_count(weapon), max_spirits(weapon)), 20.0, i.size.y - 20.0, 10.0, 2.0, WHITE, 0));
    }
    // 0x0058bbf5: the title and the list counts.
    f.texts.push(PanelText::new("Weapon Customization", 15.0, 25.0, 12.0, 3.0, WHITE, 0));
    for (k, s) in w.list.iter().enumerate() {
        let k = k as i32;
        let color = if k == w.selected { [0.0, 1.0, 1.0, 1.0] } else { WHITE };
        f.texts.push(PanelText::new(format!("{}\n", s.count), (k % 8 * 45 + 0x3c) as f32, (k / 8 * 45 + 0x50) as f32, 10.0, 2.0, color, 2));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::super::members::NoPlx;
    use super::super::GameUi;
    use super::*;

    struct Bar;
    impl VoxelModel for Bar {
        fn dims(&self) -> [i32; 3] {
            [4, 1, 1]
        }
        fn is_empty(&self, _x: i32, _y: i32, _z: i32) -> bool {
            false
        }
    }

    fn weapon() -> Vec<u8> {
        let mut it = default_item();
        it[0] = 3;
        it[1] = 0;
        it[0xd] = 1;
        it
    }

    fn material(sub: u8, mat: u8, count: i32) -> ItemStack {
        let mut it = default_item();
        it[0] = 0xb;
        it[1] = sub;
        it[0xd] = mat;
        ItemStack { count, item: it }
    }

    #[test]
    fn spirit_records() {
        let mut it = weapon();
        set_spirit(&mut it, [1, 2, 3], 0x80, 5);
        set_spirit(&mut it, [4, 5, 6], 0x81, 6);
        set_spirit(&mut it, [1, 2, 3], 0x82, 7);
        assert_eq!(spirit_count(&it), 2);
        assert_eq!(spirit(&it, 0), ([1, 2, 3], 0x82, 7));
        remove_spirit(&mut it, [1, 2, 3]);
        assert_eq!(spirit_count(&it), 1);
        assert_eq!(spirit(&it, 0), ([4, 5, 6], 0x81, 6));
        assert_eq!(max_spirits(&it), 16);
    }

    #[test]
    fn pick_walks_to_the_model() {
        let mut w = VoxelWidget::default();
        let it = weapon();
        // From above cell (1, 0, 0) straight down: z steps -1 until it enters the model.
        let back = pick(&mut w, &it, &Bar, [1.5, 0.5, 5.5], [0.0, 0.0, -1.0]);
        assert!(w.hit);
        assert_eq!(w.cell, [1, 0, 0]);
        assert_eq!(back, [0, 0, 1]);
    }

    #[test]
    fn place_and_pay() {
        let game0 = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game0);
        let mut game = GameView::default();
        game.equipment[6] = weapon();
        game.inventory = vec![vec![], vec![], vec![material(0xa, 1, 3)]];
        let mut w = VoxelWidget::default();
        set_target(&mut w, ItemRef::Equipment(6));
        open_from_static(&mut ui.gui, &ui.m, &mut w);
        assert!(vis(&ui.gui, ui.m.voxel_panel) && vis(&ui.gui, ui.m.inventory_panel));
        let inp = VoxelInputs {
            origin: Vec2::ZERO,
            size: Vec2::new(400.0, 624.0),
            cursor: Vec2::new(25.0, 45.0),
            time_ms: 0,
            model: Some(&Bar),
            model_index: Some(1),
            ray: Some(([1.5, 0.5, 5.5], [0.0, 0.0, -1.0])),
        };
        let f = frame(&mut w, &game, &inp);
        assert_eq!(w.list.len(), 1);
        assert_eq!(w.hovered, 0);
        assert_eq!(f.texts[0].text, "Upgrades 0/16\n");
        left_click(&ui.gui, &ui.m, &mut w, &mut game, true);
        assert_eq!(w.selected, 0);
        // Move the cursor off the list: the preview sits on the free cell above the hit.
        // (The preview test reads the previous frame's list hover: one frame late.)
        let inp = VoxelInputs { cursor: Vec2::new(-100.0, -100.0), ..inp };
        assert_eq!(frame(&mut w, &game, &inp).preview, None);
        let f = frame(&mut w, &game, &inp);
        assert_eq!(f.preview, Some(PreviewCube { cell: Some([1, 0, 1]), material: 1 }));
        left_click(&ui.gui, &ui.m, &mut w, &mut game, true);
        assert_eq!(spirit(&game.equipment[6], 0), ([1, 0, 1], 1, 1));
        assert_eq!(game.inventory[2][0].count, 2);
        // Pick the spirit up and move it.
        w.reset();
        let inp2 = VoxelInputs { ray: Some(([1.5, 0.5, 5.5], [0.0, 0.0, -1.0])), ..inp };
        frame(&mut w, &game, &inp2);
        assert!(w.hit && w.cell == [1, 0, 1]);
        left_click(&ui.gui, &ui.m, &mut w, &mut game, true);
        assert!(w.carrying);
        let inp3 = VoxelInputs { ray: Some(([2.5, 0.5, 5.5], [0.0, 0.0, -1.0])), ..inp };
        frame(&mut w, &game, &inp3);
        assert_eq!(w.cell, [2, 0, 1]);
        left_click(&ui.gui, &ui.m, &mut w, &mut game, true);
        assert!(!w.carrying);
        assert_eq!(spirit_count(&game.equipment[6]), 1);
        assert_eq!(spirit(&game.equipment[6], 0).0, [2, 0, 1]);
    }
}
