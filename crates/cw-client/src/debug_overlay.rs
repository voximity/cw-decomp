//! The debug overlay: a port-only testing aid, not in Cube.exe (egui 0.36 on egui-winit and
//! egui-wgpu).
//!
//! # Usage
//!
//! Compiled only with the `debug-overlay` feature (off by default; without it the binary has
//! no egui code and behaves as before):
//!
//! ```text
//! cargo build --release -p cw-client --features debug-overlay
//! ```
//!
//! **F3** shows or hides it (F3 has no binding in the game: the key table `0x0047e95c` only
//! maps F1). It is off at start. While it shows, the system cursor is visible and released
//! (mouse look is suspended; the game's own cursor still follows the mouse on free-cursor
//! screens), and clicks, wheel turns and typing that land on the overlay's window do not reach
//! the game. Movement keys still reach the game unless a text field has the focus.
//!
//! - **State**: position (blocks), zone, region, world seed and name, FPS, creature count.
//! - **Player**: level and XP (set, or "Add XP" which then runs the kill's `levelUp`
//!   `cw_sim::combat::level_up`), HP and MP, full heal, god mode. God mode refills HP to
//!   `maxHp` every frame after the tick (it stays on while the overlay is hidden); a hit that
//!   takes more than the full HP within one frame's tick still kills.
//! - **Skills**: the eleven skill levels (`entity+0x1128`). The game stores no point pool: the
//!   cap is `manaCubes / 4 + level * 2 - 2` (`SkillWidget` 0x0047b8c5), so "+1 point" adds
//!   four mana cubes (`entity+0x1154`) and "−1 point" removes four. Mana cubes are also what
//!   the mana cube pickups compare against (`interact.rs`).
//! - **Items**: a preset from the dictionary's item keys, or type / sub type by number, with
//!   level, rarity, material, modifier and count. "Give" puts it into the inventory with
//!   `Inventory::addItem(item, -1)` (0x00427000), the call the tick's pickup makes
//!   (`cw_sim::interact`, 0x0053664a), so the inventory widgets refresh from the creature's
//!   state as after a pickup.
//! - **Teleport**: to block coordinates, to a zone's centre, or to the zone under the world map
//!   cursor. Open the map (M), point at a zone outside the overlay's window (the zone shown
//!   follows the cursor while it is not over the overlay), then press "Teleport to map
//!   cursor". The position is written as the bed teleport writes it (`creature+0x10`,
//!   0x0047df65; its zone centre formula), with the velocity cleared and a height of the
//!   world's base height + 2 blocks at the target (or 0 as the bed does, or a typed value).
//!
//! Every write goes to the client's creature map and states under the world lock, then lock A
//! (the order of the tick in `Controller::simulate`). In singleplayer that world is the
//! authoritative one (`singleplayer.rs`); connected to a server, the changes are only this
//! client's (the server keeps its own creature and sends it back on the next update).
//!
//! Nothing runs while the overlay is hidden except god mode when it was turned on, and the
//! renderer draws the overlay only when there is something to paint.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use cw_net::EntityData;
use cw_render::gpu::overlay::{OverlayPass, OverlayTarget};
use cw_sim::combat::CreatureState;
use cw_world::inventory::{Inventory, Item};
use egui_wgpu::wgpu;
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

use crate::controller::{Controller, FrameSink};
use crate::player::{ent, f32_at, i32_at, set_pos, wf32};

/// `entity+0x1128`: the eleven skill levels.
const SKILLS_AT: usize = 0x1128;
/// `entity+0x1154`: the mana cubes.
const MANA_CUBES_AT: usize = 0x1154;

fn w32(e: &mut EntityData, o: usize, v: i32) {
    e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---------------------------------------------------------------------------------------------
// Pure parts.

/// What the item form describes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemSpec {
    pub item_type: u8,
    pub sub_type: u8,
    pub level: u16,
    pub rarity: u8,
    pub material: u8,
    pub modifier: i32,
    pub count: u32,
}

impl Default for ItemSpec {
    fn default() -> Self {
        // An iron sword of level 1.
        ItemSpec { item_type: 3, sub_type: 0, level: 1, rarity: 0, material: 1, modifier: 0, count: 1 }
    }
}

impl ItemSpec {
    /// The item, from the constructor defaults (`Item::NEW`).
    pub fn build(&self) -> Item {
        Item {
            item_type: self.item_type,
            sub_type: self.sub_type,
            level: self.level.max(1),
            rarity: self.rarity,
            material: self.material,
            modifier: self.modifier,
            ..Item::NEW
        }
    }
}

/// `Inventory::addItem` 0x00427000 takes the item's level as the count for these types (and
/// stores them at level 1): coins and the second currency, ingredients but sub type 0xe, and
/// types 0x14, 0x15, 0x17, 0x18, 0x19.
pub fn level_is_count(item_type: u8, sub_type: u8) -> bool {
    matches!(item_type, 0xc | 0xd | 0x15 | 0x19 | 0x14 | 0x18 | 0x17) || (item_type == 0xb && sub_type != 0xe)
}

/// Puts `spec.count` of the item into `inv` through `Inventory::addItem(item, -1)`: once with
/// the count as the level for the types that count by level, else once per item (stackable
/// types merge into one slot, others take a slot each).
pub fn give_items(inv: &mut Inventory, spec: &ItemSpec) {
    let mut item = spec.build();
    if spec.count == 0 {
        return;
    }
    if level_is_count(item.item_type, item.sub_type) {
        item.level = spec.count.min(i16::MAX as u32) as u16;
        inv.add_item(item, -1);
    } else {
        for _ in 0..spec.count.min(1000) {
            inv.add_item(item, -1);
        }
    }
}

/// A block coordinate as 16.16 fixed point (`creature+0x10` units).
pub fn block_to_fixed(b: f64) -> i64 {
    (b * 65536.0).floor() as i64
}

/// A fixed-point coordinate in blocks.
pub fn fixed_to_block(p: i64) -> f64 {
    p as f64 / 65536.0
}

/// The zone (256 blocks) of a fixed-point coordinate, rounding toward −∞.
pub fn zone_of(p: i64) -> i32 {
    (p >> 24) as i32
}

/// The region (64 zones) of a zone coordinate, rounding toward −∞.
pub fn region_of(zone: i32) -> i32 {
    zone.div_euclid(64)
}

/// The zone centre the bed teleport writes (`onMouseUp` 0x0047df65..0x0047df8e,
/// `map_screen::on_release`): `((z · 256 + 128) << 16)` with a 32-bit product.
pub fn zone_centre(zx: i32, zy: i32) -> [i64; 2] {
    let f = |z: i32| i64::from(z.wrapping_shl(8).wrapping_add(0x80)) << 16;
    [f(zx), f(zy)]
}

/// The world map's stored matrices (`WorldMap+0x44` view, `WorldMap+4` projection; row-major,
/// `v * M`) and the viewport, as the map overlay projects with them (`ui::map_overlay`).
#[derive(Debug, Clone, Copy)]
pub struct MapCamera {
    pub view: cw_render::frame::D3dMatrix,
    pub projection: cw_render::frame::D3dMatrix,
    pub width: f32,
    pub height: f32,
}

impl MapCamera {
    fn combined(&self) -> glam::Mat4 {
        // A row-major `v * M` matrix has the memory layout of glam's column-major `M * v`.
        let v = glam::Mat4::from_cols_array_2d(&self.view);
        let p = glam::Mat4::from_cols_array_2d(&self.projection);
        p * v
    }

    /// The cursor (pixels) on the map, as blocks relative to the map's origin (the player plus
    /// the pan; what the overlay subtracts before projecting), on the plane `z = 0` of that
    /// space. `None` when the ray misses the plane.
    pub fn unproject(&self, cursor: [f32; 2]) -> Option<[f32; 2]> {
        if self.width <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let inv = self.combined().inverse();
        if !inv.is_finite() {
            return None;
        }
        let nx = (cursor[0] - self.width * 0.5) / (self.width * 0.5);
        let ny = (self.height * 0.5 - cursor[1]) / (self.height * 0.5);
        let a = inv * glam::Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * glam::Vec4::new(nx, ny, 0.5, 1.0);
        if a.w == 0.0 || b.w == 0.0 {
            return None;
        }
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        let dz = a.z - b.z;
        if dz.abs() < 1e-9 {
            return None;
        }
        let t = a.z / dz;
        let p = a + (b - a) * t;
        p.is_finite().then_some([p.x, p.y])
    }
}

/// The zone under the map cursor: the player's position plus the pan plus the unprojected
/// offset, in zones.
pub fn map_cursor_zone(cam: &MapCamera, cursor: [f32; 2], player: [i64; 3], pan: [f32; 3]) -> Option<[i32; 2]> {
    let rel = cam.unproject(cursor)?;
    let bx = fixed_to_block(player[0]) + f64::from(pan[0]) + f64::from(rel[0]);
    let by = fixed_to_block(player[1]) + f64::from(pan[1]) + f64::from(rel[1]);
    Some([zone_of(block_to_fixed(bx)), zone_of(block_to_fixed(by))])
}

/// Where a teleport puts the player's feet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TeleportZ {
    /// `World::baseHeight` at the target + 2 blocks.
    Ground,
    /// 0, as the bed teleport writes.
    Zero,
    /// A height in blocks.
    Blocks(f32),
}

/// One change the overlay asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Give(ItemSpec),
    SetLevel(i32),
    SetXp(i32),
    /// XP added, then `levelUp` as a kill runs it.
    AddXp(i32),
    SetHp(f32),
    SetMp(f32),
    FullHeal,
    SetSkill(usize, i32),
    ResetSkills,
    /// Skill points (× 4 mana cubes), negative to remove.
    AddSkillPoints(i32),
    /// Blocks x, y.
    Teleport([f64; 2], TeleportZ),
}

/// Applies `a` to the local player `id`; `base_height` is `World::baseHeight` at a block.
/// Returns whether the skill levels changed.
pub fn apply_action(entities: &mut std::collections::BTreeMap<i64, EntityData>, states: &mut std::collections::BTreeMap<i64, CreatureState>, id: i64, a: &Action, base_height: &dyn Fn(i32, i32) -> f32) -> bool {
    if let Action::Give(spec) = a {
        // The tick's pickup: `states.entry(player).or_default().inventory.add_item(item, -1)`.
        give_items(&mut states.entry(id).or_default().inventory, spec);
        return false;
    }
    let Some(e) = entities.get_mut(&id) else { return false };
    match *a {
        Action::Give(_) => {}
        Action::SetLevel(l) => w32(e, ent::LEVEL, l.max(1)),
        Action::SetXp(x) => w32(e, ent::XP, x.max(0)),
        Action::AddXp(x) => {
            let v = i32_at(&e.0, ent::XP).wrapping_add(x);
            w32(e, ent::XP, v);
            cw_sim::combat::level_up(e);
        }
        Action::SetHp(h) => wf32(&mut e.0, ent::HP, h),
        Action::SetMp(m) => wf32(&mut e.0, ent::MP, m),
        Action::FullHeal => {
            let hp = cw_sim::stats::max_hp(e);
            wf32(&mut e.0, ent::HP, hp);
            wf32(&mut e.0, ent::MP, 1.0);
        }
        Action::SetSkill(k, l) => {
            if k < 11 {
                w32(e, SKILLS_AT + 4 * k, l.max(0));
                return true;
            }
        }
        Action::ResetSkills => {
            for k in 0..11 {
                w32(e, SKILLS_AT + 4 * k, 0);
            }
            return true;
        }
        Action::AddSkillPoints(n) => {
            let v = i32_at(&e.0, MANA_CUBES_AT).saturating_add(n.saturating_mul(4)).max(0);
            w32(e, MANA_CUBES_AT, v);
            return true;
        }
        Action::Teleport([bx, by], z) => {
            let (fx, fy) = (block_to_fixed(bx), block_to_fixed(by));
            let fz = match z {
                TeleportZ::Zero => 0,
                TeleportZ::Blocks(b) => block_to_fixed(f64::from(b)),
                TeleportZ::Ground => block_to_fixed(f64::from(base_height(bx.floor() as i32, by.floor() as i32)) + 2.0),
            };
            set_pos(e, [fx, fy, fz]);
            for i in 0..3 {
                wf32(&mut e.0, ent::VEL + 4 * i, 0.0);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// The renderer side (lives in `app::ExecSink`).

/// One frame of the overlay for the executor.
pub struct OverlayPaint {
    primitives: Vec<egui::ClippedPrimitive>,
    textures: egui::TexturesDelta,
    pixels_per_point: f32,
}

/// The egui renderer, created on the sink's device at the first paint.
#[derive(Default)]
pub struct OverlayRenderer {
    renderer: Option<egui_wgpu::Renderer>,
    pending: Option<OverlayPaint>,
    /// Textures egui released, freed after the frame that last drew them was submitted.
    to_free: Vec<egui::TextureId>,
}

impl OverlayRenderer {
    /// The paint of this frame. A paint the executor did not draw (a skipped frame) passes its
    /// texture updates on to this one.
    pub fn submit(&mut self, mut paint: OverlayPaint) {
        if let Some(mut old) = self.pending.take() {
            let newer = std::mem::take(&mut paint.textures);
            old.textures.append(newer);
            paint.textures = std::mem::take(&mut old.textures);
        }
        self.pending = Some(paint);
    }

    /// The pass for `Executor::render_with_overlay`, when there is something to paint.
    pub fn pass(&mut self) -> Option<&mut dyn OverlayPass> {
        if self.pending.is_some() { Some(self) } else { None }
    }
}

impl Drop for OverlayRenderer {
    fn drop(&mut self) {
        if let Some(p) = &mut self.pending {
            p.textures.clear();
        }
    }
}

impl OverlayPass for OverlayRenderer {
    fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &OverlayTarget<'_>) -> Vec<wgpu::CommandBuffer> {
        let Some(mut p) = self.pending.take() else { return Vec::new() };
        let r = self.renderer.get_or_insert_with(|| egui_wgpu::Renderer::new(device, target.format, egui_wgpu::RendererOptions::default()));
        for id in self.to_free.drain(..) {
            r.free_texture(&id);
        }
        for (id, deltas) in &p.textures.set {
            for d in deltas {
                r.update_texture(device, queue, *id, d);
            }
        }
        self.to_free.extend(p.textures.free.iter().copied());
        p.textures.clear();
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [target.width, target.height], pixels_per_point: p.pixels_per_point };
        let pre = r.update_buffers(device, queue, encoder, &p.primitives, &screen);
        let mut rp = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("debug overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    // The frame is already in the surface texture: load, never clear.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        r.render(&mut rp, &p.primitives, &screen);
        drop(rp);
        pre
    }
}

// ---------------------------------------------------------------------------------------------
// The UI side (lives in `app::App`).

/// What the panels show, read once per frame.
#[derive(Default)]
struct Snapshot {
    pos: [i64; 3],
    level: i32,
    xp: i32,
    hp: f32,
    max_hp: f32,
    mp: f32,
    skills: [i32; 11],
    mana_cubes: i32,
    creatures: usize,
    world: Option<(i32, String)>,
    map_open: bool,
    map_zone: Option<[i32; 2]>,
    connected: bool,
    text: Option<Arc<crate::ui::textdb::TextDb>>,
    has_player: bool,
}

/// The overlay's form fields.
struct Form {
    item: ItemSpec,
    level: i32,
    xp: i32,
    add_xp: i32,
    hp: f32,
    mp: f32,
    target_block: [f64; 2],
    target_zone: [i32; 2],
    z_mode: usize,
    z_blocks: f32,
}

impl Default for Form {
    fn default() -> Self {
        Form { item: ItemSpec::default(), level: 1, xp: 0, add_xp: 100, hp: 100.0, mp: 1.0, target_block: [0.0; 2], target_zone: [0; 2], z_mode: 0, z_blocks: 0.0 }
    }
}

impl Form {
    fn z(&self) -> TeleportZ {
        match self.z_mode {
            1 => TeleportZ::Zero,
            2 => TeleportZ::Blocks(self.z_blocks),
            _ => TeleportZ::Ground,
        }
    }
}

/// The overlay: egui's context and winit state, the form and the toggles.
#[derive(Default)]
pub struct DebugOverlay {
    egui: Option<(egui::Context, egui_winit::State)>,
    visible: bool,
    /// The system cursor was made visible for the overlay.
    cursor_shown: bool,
    god_mode: bool,
    form: Form,
    /// The last zone under the map cursor while the pointer was not over the overlay.
    map_zone: Option<[i32; 2]>,
    /// Mouse buttons whose press egui took (their release is not passed to the game either).
    swallowed_buttons: u8,
    last_frame: Option<Instant>,
    fps: f32,
}

fn button_bit(b: winit::event::MouseButton) -> u8 {
    match b {
        winit::event::MouseButton::Left => 1,
        winit::event::MouseButton::Right => 2,
        winit::event::MouseButton::Middle => 4,
        _ => 8,
    }
}

impl DebugOverlay {
    /// The egui state for the window (`resumed`).
    pub fn attach(&mut self, window: &Window) {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, window, Some(window.scale_factor() as f32), window.theme(), None);
        self.egui = Some((ctx, state));
    }

    /// The overlay shows (the cursor is released for it).
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// A window event, before the game sees it. Returns true when the game must not see it:
    /// F3 (the toggle), and while the overlay shows, the presses, wheel turns and keys egui
    /// takes (a button's release follows its press).
    pub fn on_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput { event: k, .. } = event
            && k.physical_key == PhysicalKey::Code(KeyCode::F3)
        {
            if k.state == ElementState::Pressed && !k.repeat {
                self.visible = !self.visible;
            }
            return true;
        }
        let Some((ctx, state)) = self.egui.as_mut() else { return false };
        if !self.visible {
            self.swallowed_buttons = 0;
            return false;
        }
        let consumed = state.on_window_event(window, event).consumed;
        // egui-winit reports Tab as always consumed; the game's Tab (the quick-item wheel) is
        // only taken while an overlay field has the keyboard.
        let keyboard = ctx.egui_wants_keyboard_input();
        match event {
            WindowEvent::MouseInput { state: s, button, .. } => {
                let bit = button_bit(*button);
                if *s == ElementState::Pressed {
                    if consumed {
                        self.swallowed_buttons |= bit;
                    }
                    consumed
                } else {
                    let swallowed = self.swallowed_buttons & bit != 0;
                    self.swallowed_buttons &= !bit;
                    swallowed
                }
            }
            WindowEvent::MouseWheel { .. } => consumed,
            // Releases go through so no key stays held in the game.
            WindowEvent::KeyboardInput { event: k, .. } => keyboard && k.state == ElementState::Pressed,
            _ => false,
        }
    }

    /// Once per frame after `update`, before `render`: god mode, then (shown) the UI, its
    /// actions and the paint handed to the sink. `gui_scale` is `App::gui_scale` (egui's
    /// pixels per point, as the game's GUI is scaled).
    pub fn frame(&mut self, window: &Window, c: &mut Controller, sink: &mut dyn FrameSink, gui_scale: f64) {
        let now = Instant::now();
        if let Some(l) = self.last_frame {
            let dt = now.duration_since(l).as_secs_f32();
            if dt > 0.0 {
                let f = 1.0 / dt;
                self.fps = if self.fps == 0.0 { f } else { self.fps * 0.95 + f * 0.05 };
            }
        }
        self.last_frame = Some(now);
        if self.god_mode {
            apply_actions(c, &[Action::FullHeal]);
        }
        // The system cursor over the window: shown for the overlay, hidden again after (the
        // game draws `cursor.plx` and hides the system one, `App::update_grab`).
        if self.visible {
            window.set_cursor_visible(true);
            self.cursor_shown = true;
        } else if self.cursor_shown {
            window.set_cursor_visible(false);
            self.cursor_shown = false;
        }
        if !self.visible {
            return;
        }
        let Some((ctx, state)) = self.egui.as_mut() else { return };
        let ctx = ctx.clone();
        let zoom = (gui_scale / window.scale_factor().max(1e-3)) as f32;
        if (ctx.zoom_factor() - zoom).abs() > 1e-4 {
            ctx.set_zoom_factor(zoom);
        }
        let over_egui = ctx.is_pointer_over_egui();
        let snap = snapshot(c);
        if snap.map_open && !over_egui && snap.map_zone.is_some() {
            self.map_zone = snap.map_zone;
        }
        let input = state.take_egui_input(window);
        let mut actions = Vec::new();
        let mut open = true;
        let full = ctx.run_ui(input, |ui| {
            egui::Window::new("Debug overlay (F3)").open(&mut open).default_width(320.0).show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels(ui, &snap, &mut self.form, &mut self.god_mode, self.map_zone, self.fps, &mut actions);
                });
            });
        });
        if !open {
            self.visible = false;
        }
        if let Some((_, state)) = self.egui.as_mut() {
            state.handle_platform_output(window, full.platform_output);
        }
        apply_actions(c, &actions);
        let primitives = ctx.tessellate(full.shapes, full.pixels_per_point);
        let paint = OverlayPaint { primitives, textures: full.textures_delta, pixels_per_point: full.pixels_per_point };
        match sink.debug_overlay() {
            Some(r) => r.submit(paint),
            None => {
                let mut p = paint;
                p.textures.clear();
            }
        }
    }
}

/// The state the panels show.
fn snapshot(c: &Controller) -> Snapshot {
    let id = c.player.id;
    let mut s = Snapshot { world: c.shared.lock_world_cs().loaded.clone(), map_open: c.game.map_open, text: c.ui.text.clone(), connected: c.net.connected.load(Ordering::SeqCst), ..Snapshot::default() };
    {
        let nw = c.net.world.lock().unwrap_or_else(|e| e.into_inner());
        s.creatures = nw.entities.len();
        if let Some(e) = nw.entities.get(&id) {
            s.has_player = true;
            s.pos = [0, 1, 2].map(|i| i64::from_le_bytes(e.0[i * 8..i * 8 + 8].try_into().unwrap()));
            s.level = i32_at(&e.0, ent::LEVEL);
            s.xp = i32_at(&e.0, ent::XP);
            s.hp = f32_at(&e.0, ent::HP);
            s.mp = f32_at(&e.0, ent::MP);
            s.max_hp = cw_sim::stats::max_hp(e);
            for k in 0..11 {
                s.skills[k] = i32_at(&e.0, SKILLS_AT + 4 * k);
            }
            s.mana_cubes = i32_at(&e.0, MANA_CUBES_AT);
        }
    }
    if s.map_open
        && let (Some(view), Some(projection)) = (c.map_state.stored_view, c.map_state.stored_projection)
    {
        let cam = MapCamera { view, projection, width: c.screen[0] as f32, height: c.screen[1] as f32 };
        s.map_zone = map_cursor_zone(&cam, c.ui.gui.cursor.to_array(), s.pos, c.map_pan);
    }
    s
}

/// The actions, under the world lock then lock A (the tick's order), then the view refreshed
/// from the creature (`Controller::mirror_to_view`, what every frame's UI pass starts from).
fn apply_actions(c: &mut Controller, actions: &[Action]) {
    if actions.is_empty() {
        return;
    }
    let id = c.player.id;
    let mut skills = false;
    {
        let world = c.shared.write_world();
        let mut nw = c.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let nw = &mut *nw;
        let base_height = |x: i32, y: i32| world.base_height(x, y);
        for a in actions {
            skills |= apply_action(&mut nw.entities, &mut nw.states, id, a, &base_height);
        }
    }
    c.mirror_to_view();
    if skills {
        // The skill panel's working copy (reloaded as opening it does, 0x00488c70).
        let p = c.game.player.clone();
        c.ui.skills.load_from(&p);
    }
}

const Z_MODES: [&str; 3] = ["base height + 2", "0 (as the bed)", "blocks"];

fn panels(ui: &mut egui::Ui, s: &Snapshot, f: &mut Form, god: &mut bool, map_zone: Option<[i32; 2]>, fps: f32, out: &mut Vec<Action>) {
    let b = s.pos.map(fixed_to_block);
    let (zx, zy) = (zone_of(s.pos[0]), zone_of(s.pos[1]));
    egui::CollapsingHeader::new("State").default_open(true).show(ui, |ui| {
        ui.label(format!("Position: {:.2}, {:.2}, {:.2}", b[0], b[1], b[2]));
        ui.label(format!("Zone: {zx}, {zy}   Region: {}, {}", region_of(zx), region_of(zy)));
        match &s.world {
            Some((seed, name)) => ui.label(format!("World: {name:?}, seed {seed}")),
            None => ui.label("World: none loaded"),
        };
        ui.label(format!("FPS: {fps:.0}   Creatures: {}", s.creatures));
        if s.connected {
            ui.colored_label(egui::Color32::YELLOW, "Connected: changes are this client's only.");
        }
        if !s.has_player {
            ui.colored_label(egui::Color32::YELLOW, "No local player creature.");
        }
    });
    egui::CollapsingHeader::new("Player").default_open(true).show(ui, |ui| {
        ui.label(format!("Level {}  XP {}  HP {:.0}/{:.0}  MP {:.2}", s.level, s.xp, s.hp, s.max_hp, s.mp));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.level).range(1..=10000).prefix("level "));
            if ui.button("Set level").clicked() {
                out.push(Action::SetLevel(f.level));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.xp).range(0..=i32::MAX).prefix("XP "));
            if ui.button("Set XP").clicked() {
                out.push(Action::SetXp(f.xp));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.add_xp).range(0..=1_000_000).prefix("+XP "));
            if ui.button("Add XP (levels up)").clicked() {
                out.push(Action::AddXp(f.add_xp));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.hp).range(0.0..=1.0e7).prefix("HP "));
            if ui.button("Set HP").clicked() {
                out.push(Action::SetHp(f.hp));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.mp).range(0.0..=1.0).speed(0.01).prefix("MP "));
            if ui.button("Set MP").clicked() {
                out.push(Action::SetMp(f.mp));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Full heal").clicked() {
                out.push(Action::FullHeal);
            }
            ui.checkbox(god, "God mode (HP refilled every frame)");
        });
    });
    egui::CollapsingHeader::new("Skills").show(ui, |ui| {
        let spent: i32 = s.skills.iter().sum();
        let cap = s.mana_cubes / 4 + s.level * 2 - 2;
        ui.label(format!("Points: {spent}/{cap}   Mana cubes: {}", s.mana_cubes));
        ui.horizontal(|ui| {
            if ui.button("+1 point").clicked() {
                out.push(Action::AddSkillPoints(1));
            }
            if ui.button("-1 point").clicked() {
                out.push(Action::AddSkillPoints(-1));
            }
            if ui.button("Reset skills").clicked() {
                out.push(Action::ResetSkills);
            }
        });
        egui::Grid::new("skills").show(ui, |ui| {
            for k in 0..11 {
                ui.label(format!("Skill {k}"));
                let mut v = s.skills[k];
                if ui.add(egui::DragValue::new(&mut v).range(0..=100)).changed() {
                    out.push(Action::SetSkill(k, v));
                }
                ui.end_row();
            }
        });
    });
    egui::CollapsingHeader::new("Items").show(ui, |ui| {
        let it = &mut f.item;
        egui::ComboBox::from_id_salt("item preset").selected_text(crate::names::item_key(i32::from(it.item_type), i32::from(it.sub_type))).height(300.0).show_ui(ui, |ui| {
            for &((t, sub), key) in crate::names::ITEM_KEYS {
                if ui.selectable_label(i32::from(it.item_type) == t && i32::from(it.sub_type) == sub, format!("{key} ({t}, {sub})")).clicked() {
                    it.item_type = t as u8;
                    it.sub_type = sub as u8;
                }
            }
        });
        egui::Grid::new("item").show(ui, |ui| {
            ui.label("Type / sub");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut it.item_type));
                ui.add(egui::DragValue::new(&mut it.sub_type));
            });
            ui.end_row();
            ui.label("Level");
            ui.add(egui::DragValue::new(&mut it.level).range(1..=10000));
            ui.end_row();
            ui.label("Rarity");
            ui.add(egui::DragValue::new(&mut it.rarity).range(0..=4));
            ui.end_row();
            ui.label("Material");
            ui.add(egui::DragValue::new(&mut it.material));
            ui.end_row();
            ui.label("Modifier");
            ui.add(egui::DragValue::new(&mut it.modifier));
            ui.end_row();
            ui.label("Count");
            ui.add(egui::DragValue::new(&mut it.count).range(1..=999));
            ui.end_row();
        });
        let name = crate::names::item_name(s.text.as_deref(), &it.build().to_bytes());
        ui.label(format!("Name: {name}"));
        if level_is_count(it.item_type, it.sub_type) {
            ui.label("(this type counts by level: the count becomes its level, stored at level 1)");
        }
        if ui.button("Give").clicked() {
            out.push(Action::Give(*it));
        }
    });
    egui::CollapsingHeader::new("Teleport").default_open(true).show(ui, |ui| {
        egui::ComboBox::from_label("Height").selected_text(Z_MODES[f.z_mode.min(2)]).show_ui(ui, |ui| {
            for (i, m) in Z_MODES.iter().enumerate() {
                ui.selectable_value(&mut f.z_mode, i, *m);
            }
        });
        if f.z_mode == 2 {
            ui.add(egui::DragValue::new(&mut f.z_blocks).prefix("z "));
        }
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.target_block[0]).prefix("x "));
            ui.add(egui::DragValue::new(&mut f.target_block[1]).prefix("y "));
            if ui.button("Here").on_hover_text("Fill in the current position").clicked() {
                f.target_block = [b[0], b[1]];
            }
            if ui.button("Go (blocks)").clicked() {
                out.push(Action::Teleport(f.target_block, f.z()));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut f.target_zone[0]).prefix("zx "));
            ui.add(egui::DragValue::new(&mut f.target_zone[1]).prefix("zy "));
            if ui.button("Go (zone centre)").clicked() {
                let c = zone_centre(f.target_zone[0], f.target_zone[1]);
                out.push(Action::Teleport([fixed_to_block(c[0]), fixed_to_block(c[1])], f.z()));
            }
        });
        let live = if s.map_open { s.map_zone.map_or("off the map".to_string(), |z| format!("{}, {}", z[0], z[1])) } else { "map closed (M)".to_string() };
        ui.label(format!("Map cursor zone: {live}"));
        ui.add_enabled_ui(map_zone.is_some(), |ui| {
            let text = map_zone.map_or("Teleport to map cursor".to_string(), |z| format!("Teleport to map cursor ({}, {})", z[0], z[1]));
            if ui.button(text).clicked()
                && let Some([x, y]) = map_zone
            {
                let c = zone_centre(x, y);
                out.push(Action::Teleport([fixed_to_block(c[0]), fixed_to_block(c[1])], f.z()));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn item_spec_builds_from_the_constructor_defaults() {
        let spec = ItemSpec { item_type: 4, sub_type: 0, level: 12, rarity: 3, material: 2, modifier: 77, count: 1 };
        let it = spec.build();
        assert_eq!((it.item_type, it.sub_type, it.level, it.rarity, it.material, it.modifier), (4, 0, 12, 3, 2, 77));
        assert_eq!(it.num_spirits, 0);
        assert_eq!(it.flags, 0);
        // Level 0 is not an item level.
        assert_eq!(ItemSpec { level: 0, ..spec }.build().level, 1);
    }

    #[test]
    fn equipment_takes_one_slot_each() {
        let mut inv = Inventory::NEW;
        give_items(&mut inv, &ItemSpec { item_type: 3, count: 3, ..ItemSpec::default() });
        // Weapons go to page 0 (`pageForItem` 0x004282f0), not stackable.
        assert_eq!(inv.pages[0].len(), 3);
        assert!(inv.pages[0].iter().all(|s| s.count == 1 && s.item.item_type == 3));
    }

    #[test]
    fn stackable_items_merge() {
        let mut inv = Inventory::NEW;
        give_items(&mut inv, &ItemSpec { item_type: 1, sub_type: 0, level: 5, material: 0, count: 4, ..ItemSpec::default() });
        let page = Inventory::page_for(1) as usize;
        assert_eq!(inv.pages[page].len(), 1);
        assert_eq!(inv.pages[page][0].count, 4);
        assert_eq!(inv.pages[page][0].item.level, 5);
    }

    #[test]
    fn level_counted_types_use_the_count_as_level() {
        let mut inv = Inventory::NEW;
        // Coins of material 0xa (copper): straight into the gold.
        give_items(&mut inv, &ItemSpec { item_type: 0xc, sub_type: 0, level: 1, material: 0xa, count: 250, ..ItemSpec::default() });
        assert_eq!(inv.gold, 250);
        assert!(inv.items().next().is_none());
        // An ingredient (type 0xb, sub not 0xe): one slot of `count`, stored at level 1.
        give_items(&mut inv, &ItemSpec { item_type: 0xb, sub_type: 2, level: 9, material: 0, count: 7, ..ItemSpec::default() });
        let s = &inv.pages[2][0];
        assert_eq!((s.count, s.item.level), (7, 1));
        assert!(!level_is_count(0xb, 0xe));
        assert!(level_is_count(0x14, 0));
        assert!(!level_is_count(3, 0));
    }

    #[test]
    fn coordinates_round_toward_minus_infinity() {
        assert_eq!(block_to_fixed(1.5), 0x18000);
        assert_eq!(block_to_fixed(-0.5), -0x8000);
        assert_eq!(fixed_to_block(-0x8000), -0.5);
        assert_eq!(zone_of(block_to_fixed(255.9)), 0);
        assert_eq!(zone_of(block_to_fixed(256.0)), 1);
        assert_eq!(zone_of(block_to_fixed(-0.1)), -1);
        assert_eq!(zone_of(block_to_fixed(-256.0)), -1);
        assert_eq!(zone_of(block_to_fixed(-256.1)), -2);
        assert_eq!(region_of(63), 0);
        assert_eq!(region_of(64), 1);
        assert_eq!(region_of(-1), -1);
        assert_eq!(region_of(-64), -1);
        assert_eq!(region_of(-65), -2);
    }

    #[test]
    fn zone_centre_matches_the_bed_teleport() {
        for (zx, zy) in [(3, -2), (0, 0), (-1, 511), (0x0080_0000, 7)] {
            let mut open = true;
            let mut pick = crate::map_screen::TeleportPick { hovered: [zx, zy], selected: [zx, zy] };
            let p = crate::map_screen::on_release(0, &mut open, true, &mut pick).unwrap();
            assert_eq!(zone_centre(zx, zy), [p[0], p[1]]);
        }
        let c = zone_centre(3, -2);
        assert_eq!((zone_of(c[0]), zone_of(c[1])), (3, -2));
    }

    /// The map overlay's projection (`ui::map_overlay::project`, 0x004c9680): `v * view`,
    /// divide by w, `* projection`, divide by w, to pixels.
    fn project(cam: &MapCamera, v: [f32; 3]) -> [f32; 2] {
        let t = |m: &cw_render::frame::D3dMatrix, v: [f32; 3]| {
            let col = |j: usize| v[1] * m[1][j] + v[0] * m[0][j] + v[2] * m[2][j] + m[3][j];
            let inv = 1.0 / col(3);
            [col(0) * inv, col(1) * inv, col(2) * inv]
        };
        let n = t(&cam.projection, t(&cam.view, v));
        [cam.width * 0.5 * n[0] + cam.width * 0.5, n[1] * (-cam.height * 0.5) + cam.height * 0.5]
    }

    #[allow(deprecated)] // glam 0.33 moved these to `glam::camera`; same matrices.
    fn test_camera() -> MapCamera {
        // A tilted camera above the origin looking down, left-handed as D3D (`D3DXMatrixLookAtLH`
        // / `PerspectiveFovLH`); glam's column arrays are the row-major D3D rows.
        let view = glam::Mat4::look_at_lh(glam::Vec3::new(10.0, -60.0, 120.0), glam::Vec3::new(5.0, 3.0, 0.0), glam::Vec3::Z);
        let proj = glam::Mat4::perspective_lh(1.0, 16.0 / 9.0, 1.0, 5000.0);
        MapCamera { view: view.to_cols_array_2d(), projection: proj.to_cols_array_2d(), width: 1600.0, height: 900.0 }
    }

    #[test]
    fn unproject_inverts_the_overlay_projection() {
        let cam = test_camera();
        for p in [[0.0f32, 0.0], [40.0, -25.0], [-70.0, 60.0], [5.0, 3.0]] {
            let px = project(&cam, [p[0], p[1], 0.0]);
            let back = cam.unproject(px).expect("hit");
            assert!((back[0] - p[0]).abs() < 0.05 && (back[1] - p[1]).abs() < 0.05, "{p:?} -> {px:?} -> {back:?}");
        }
    }

    #[test]
    fn map_cursor_zone_adds_the_player_and_the_pan() {
        let cam = test_camera();
        let px = project(&cam, [40.0, -25.0, 0.0]);
        // Player at block (1000, -10), pan (300, 0): the point is (1340, -35).
        let player = [block_to_fixed(1000.0), block_to_fixed(-10.0), 0];
        assert_eq!(map_cursor_zone(&cam, px, player, [300.0, 0.0, 0.0]), Some([5, -1]));
    }

    #[test]
    fn actions_write_the_player() {
        let id = 1;
        let mut ents = BTreeMap::new();
        let mut e = EntityData::ZERO;
        w32(&mut e, ent::LEVEL, 3);
        wf32(&mut e.0, ent::VEL, 5.0);
        ents.insert(id, e);
        let mut states = BTreeMap::new();
        let ground = |_x: i32, _y: i32| 10.0f32;
        assert!(!apply_action(&mut ents, &mut states, id, &Action::Teleport([100.5, -3.0], TeleportZ::Ground), &ground));
        let e = &ents[&id];
        let pos = [0, 1, 2].map(|i| i64::from_le_bytes(e.0[i * 8..i * 8 + 8].try_into().unwrap()));
        assert_eq!(pos, [block_to_fixed(100.5), block_to_fixed(-3.0), block_to_fixed(12.0)]);
        assert_eq!(f32_at(&e.0, ent::VEL), 0.0);
        apply_action(&mut ents, &mut states, id, &Action::Teleport([0.0, 0.0], TeleportZ::Zero), &ground);
        assert_eq!(i64::from_le_bytes(ents[&id].0[16..24].try_into().unwrap()), 0);

        assert!(apply_action(&mut ents, &mut states, id, &Action::AddSkillPoints(2), &ground));
        assert_eq!(i32_at(&ents[&id].0, MANA_CUBES_AT), 8);
        apply_action(&mut ents, &mut states, id, &Action::AddSkillPoints(-5), &ground);
        assert_eq!(i32_at(&ents[&id].0, MANA_CUBES_AT), 0);
        assert!(apply_action(&mut ents, &mut states, id, &Action::SetSkill(4, 6), &ground));
        assert_eq!(i32_at(&ents[&id].0, SKILLS_AT + 16), 6);
        apply_action(&mut ents, &mut states, id, &Action::ResetSkills, &ground);
        assert_eq!(i32_at(&ents[&id].0, SKILLS_AT + 16), 0);

        apply_action(&mut ents, &mut states, id, &Action::SetLevel(20), &ground);
        apply_action(&mut ents, &mut states, id, &Action::SetXp(7), &ground);
        assert_eq!((i32_at(&ents[&id].0, ent::LEVEL), i32_at(&ents[&id].0, ent::XP)), (20, 7));
        apply_action(&mut ents, &mut states, id, &Action::SetHp(42.0), &ground);
        apply_action(&mut ents, &mut states, id, &Action::SetMp(0.5), &ground);
        assert_eq!((f32_at(&ents[&id].0, ent::HP), f32_at(&ents[&id].0, ent::MP)), (42.0, 0.5));
        apply_action(&mut ents, &mut states, id, &Action::FullHeal, &ground);
        assert_eq!(f32_at(&ents[&id].0, ent::HP), cw_sim::stats::max_hp(&ents[&id]));

        apply_action(&mut ents, &mut states, id, &Action::Give(ItemSpec { item_type: 3, count: 2, ..ItemSpec::default() }), &ground);
        assert_eq!(states[&id].inventory.pages[0].len(), 2);
    }

    #[test]
    fn add_xp_levels_up_as_a_kill_does() {
        let id = 1;
        let mut ents = BTreeMap::new();
        let mut e = EntityData::ZERO;
        w32(&mut e, ent::LEVEL, 1);
        ents.insert(id, e);
        let mut states = BTreeMap::new();
        // Level 1 costs `(1 - 1/1) * 1000 + 50` = 50.
        apply_action(&mut ents, &mut states, id, &Action::AddXp(60), &|_, _| 0.0);
        assert_eq!(i32_at(&ents[&id].0, ent::LEVEL), 2);
        assert_eq!(i32_at(&ents[&id].0, ent::XP), 10);
    }

    /// The overlay pass draws over the frame without clearing it (`LoadOp::Load`), on a
    /// headless device (skipped without an adapter).
    #[test]
    fn overlay_pass_loads_the_frame_and_draws_over_it() {
        let Some((_instance, _adapter, device, queue)) = cw_render::gpu::device::headless() else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let (w, h) = (64u32, 64u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        // The "game frame": cleared to red.
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::RED), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
        // An egui frame with a blue square in the top-left 16x16 points (2 pixels per point).
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(32.0, 32.0))), ..Default::default() };
        ctx.set_pixels_per_point(2.0);
        let full = ctx.run_ui(input, |ui| {
            ui.painter().rect_filled(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(8.0, 8.0)), 0.0, egui::Color32::BLUE);
        });
        let primitives = ctx.tessellate(full.shapes, full.pixels_per_point);
        let mut r = OverlayRenderer::default();
        r.submit(OverlayPaint { primitives, textures: full.textures_delta, pixels_per_point: full.pixels_per_point });
        let pass = r.pass().expect("a paint is pending");
        let pre = pass.encode(&device, &queue, &mut enc, &OverlayTarget { view: &view, format, width: w, height: h });
        assert!(r.pass().is_none(), "the paint is consumed");
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(row * h), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(pre.into_iter().chain([enc.finish()]));
        buf.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buf.get_mapped_range(..).expect("mapped").to_vec();
        let px = |x: u32, y: u32| {
            let o = (y * row + x * 4) as usize;
            [data[o], data[o + 1], data[o + 2], data[o + 3]]
        };
        assert_eq!(px(4, 4), [0, 0, 255, 255], "the overlay drew");
        assert_eq!(px(40, 40), [255, 0, 0, 255], "the frame under it was kept");
    }
}
