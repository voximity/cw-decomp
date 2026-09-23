//! The creature pass of `GameController::render` (`Cube.exe 0x004b35d4..0x004b8bc7`) minus
//! the creature bodies (the pose `0x004128f0` calls at `0x004b5cf3`/`0x004b5db2`, the
//! after-images and the ghost deferral, which `cw_render::passes::build_frame` draws from the
//! `CreatureInput`s another module builds) and minus the particle draw `0x004b7db6..0x004b8138`
//! ([`crate::particles::ParticleSystem::draw`]).
//!
//! Tier B: operation order, comparison directions, truncations and every `rand()` draw follow
//! the assembly. The only `rand()` consumers of this range are the ten model explosions
//! `0x00470d80` of a creature that died ([`dying_creature`]); the four beam calls
//! (`0x00471d50`) all pass `param_15 = 0` (no sparks) and draw none.
//!
//! # Map of the original
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x004b35d4..0x004b3801` | airships (`GC+0x2f0`, model 0xa03) | [`airship_draw`] |
//! | `0x004b3801..0x004b3990` | the creature list (`0x004c1100`), cleared by panels, sorted far → near (`0x004aba90`) | [`creature_list`] |
//! | `0x004b39e0..0x004b4124` | a dead creature: ten explosions when `creature+0x1d10` is set, the flash of type 0x90 | [`dying_creature`] |
//! | `0x004b4129..0x004b454e` | modes 0x1e..0x21: cube whirls `0x004bc760` at the aim point | [`living_creature`], [`whirl_4bc760`] |
//! | `0x004b454e..0x004b467b` | mode 0x22: fire whirl `0x004bbd80` around the target | [`living_creature`], [`whirl_4bbd80`] |
//! | `0x004b467b..0x004b4e19` | modes 0x1c, 0x5e, 0x5f: the ray (particle cube stretched to the `World::sweep` hit) | [`ray_draw`] |
//! | `0x004b4e1b..0x004b4e6a` | visibility and white-clear test of the creature | [`living_creature`] |
//! | `0x004b4e70..0x004b5aaf` | body light and tints (for [`body_material`]); buffs 4 (whirl), 7 (model 0x9f8), 8 (model 0x9f9) | [`buff_models`] |
//! | `0x004b5aaf..0x004b5db7` | body, after-images, ghost deferral; each pose draw sets `creature+0x1d10` (0x0041293b) | drawn from `CreatureInput`; the flag in [`PassEffects::posed`] |
//! | `0x004b5db7..0x004b5ef3` | spirit cubes `0x00471b60` of weapons, chest and shoulders | [`spirit_cubes`] |
//! | `0x004b5ef8..0x004b62fc` | stun stars (model 0x9f5) / mode 0x54 (model 0x9f7) | [`stun_models`] |
//! | `0x004b6300..0x004b68f5` | modes 0x25/0x26/0x2b/0x2c: beam `0x00471d50` | [`living_creature`] |
//! | `0x004b68f5..0x004b6d47` | buffs 6 (`0x004c04c0`), 9, 10, 11 (`0x004bbd80`) | [`buff_whirls`] |
//! | `0x004b6d47..0x004b6e85` | modes 0x18/0x19: beam `0x00471d50` | [`living_creature`] |
//! | `0x004b6e85..0x004b7085` | modes 0x57/0x58: four fire whirls | [`living_creature`] |
//! | `0x004b7085..0x004b72cb` | mode 0x65: model 0x42b | [`mode_65_model`] |
//! | `0x004b72cb..0x004b7685` | hostility 3: the marker above the head (0x90b/0x90c/0x90d/0xa00) | [`marker_model`] |
//! | `0x004b76c4..0x004b7db6` | path debug cubes (`GC+0x1001004`) | [`path_debug`] |
//! | `0x004b7db6..0x004b8138` | particles (CULLMODE NONE) | [`crate::particles::ParticleSystem::draw`] |
//! | `0x004b8138..0x004b8bb9` | projectiles: models 0x348/0x835/boomerang item, beams of kinds 0 and 1 | [`projectile_pass`] |
//!
//! [`creature_pass`] covers everything up to `0x004b7db6`; [`projectile_pass`] is the part
//! after the particles and must be called by the parent after
//! `ParticleSystem::draw` (it is not called from [`creature_pass`]).

use std::collections::{BTreeMap, BTreeSet};

use cw_math::rand::MsvcRand;
use cw_net::EntityData;
use cw_render::frame::D3dMatrix;
use cw_render::passes::{ModelDraw, sphere_visible};
use cw_sim::combat::CreatureState;
use cw_sim::util::has_buff;
use cw_world::inventory::Item;

use super::beam::{BeamArgs, draw_beam};
use super::math::*;
use super::{FlashLight, ModelInfo, SceneCtx, ScenePieces, entity_pos, equip, f32_at, i32_at, i64_at, material_color, u16_at};
use crate::particles::ParticleSystem;

/// Size of the particle cube `GC+0x800730` (`Model+0x44..+0x4c`). `SceneModels::particle_cube`
/// only gives the handle; the model is the 1x1x1 cube (the particle draw centres it with
/// -0.5 too).
const PARTICLE_CUBE_SIZE: [i32; 3] = [1, 1, 1];

/// `3.141592653589793` (`0x006fce08`, a double).
const PI_D: f64 = std::f64::consts::PI;

// ---------------------------------------------------------------------------------------
// Inputs the SceneCtx does not have yet
// ---------------------------------------------------------------------------------------

/// One airship of `World+0xc` / `GC+0x2f0` (the object behind the map node's `+8`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Airship {
    /// `+0x10`: position, world fixed point (the visibility test).
    pub position: [i64; 3],
    /// `+0x80`: the drawn position, world fixed point.
    pub render_pos: [i64; 3],
    /// `+0x98`: yaw in degrees.
    pub yaw: f32,
}

/// The part models and matrices the pose object at `creature+0x1498` keeps from its last
/// frame (`pose+0xbc..+0xe8` model pointers, `pose+0xec..+0x52c` one render-space world matrix
/// per part, the same matrices `0x004128f0` passed to `setTransforms`, centring included).
/// Only the slots this pass reads are here.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PoseParts {
    /// `creature+0x1554` (`pose+0xbc`): head.
    pub head: Option<ModelInfo>,
    /// `creature+0x1558` (`pose+0xc0`): hair.
    pub hair: Option<ModelInfo>,
    /// `creature+0x155c` (`pose+0xc4`): body (chest armour or appearance body).
    pub body: Option<ModelInfo>,
    /// `creature+0x1560` (`pose+0xc8`): upper arms.
    pub arm: Option<ModelInfo>,
    /// `creature+0x1564` (`pose+0xcc`): hands.
    pub hands: Option<ModelInfo>,
    /// `creature+0x1570` (`pose+0xd8`): feet.
    pub feet: Option<ModelInfo>,
    /// `creature+0x1574` (`pose+0xdc`): right weapon (slot 7).
    pub weapon_r: Option<ModelInfo>,
    /// `creature+0x1578` (`pose+0xe0`): left weapon (slot 6).
    pub weapon_l: Option<ModelInfo>,
    /// `creature+0x157c` (`pose+0xe4`): shoulder armour (slot 5).
    pub shoulder: Option<ModelInfo>,
    /// `creature+0x1604` (`pose+0x16c`): body matrix.
    pub body_m: D3dMatrix,
    /// `creature+0x1644` (`pose+0x1ac`): head matrix (also the hair's).
    pub head_m: D3dMatrix,
    /// `creature+0x16c4` (`pose+0x22c`): left upper arm.
    pub arm_l_m: D3dMatrix,
    /// `creature+0x1704` (`pose+0x26c`): right upper arm.
    pub arm_r_m: D3dMatrix,
    /// `creature+0x1744` (`pose+0x2ac`): right weapon.
    pub weapon_r_m: D3dMatrix,
    /// `creature+0x1784` (`pose+0x2ec`): left weapon.
    pub weapon_l_m: D3dMatrix,
    /// `creature+0x17c4` (`pose+0x32c`): left hand.
    pub hand_l_m: D3dMatrix,
    /// `creature+0x1804` (`pose+0x36c`): right hand.
    pub hand_r_m: D3dMatrix,
    /// `creature+0x1844` (`pose+0x3ac`): shoulder armour.
    pub shoulder_m: D3dMatrix,
    /// `creature+0x1944` (`pose+0x4ac`): left foot.
    pub foot_l_m: D3dMatrix,
    /// `creature+0x1984` (`pose+0x4ec`): right foot.
    pub foot_r_m: D3dMatrix,
}

/// What the pass reads that `SceneCtx` does not carry yet (see the report: each field should
/// move into `SceneState`/`SceneCtx`).
pub struct PassInputs<'a> {
    /// Per creature, the pose's parts of its last frame (`creature+0x1554..`, `+0x1604..`).
    pub parts: &'a BTreeMap<i64, PoseParts>,
    /// `creature+0x1d10 != 0`: the creature died and has not exploded yet.
    pub explode_pending: &'a BTreeSet<i64>,
    /// `esp+0x34` of `render`: `timeGetTime()` at the start of the frame (`0x004ac2aa`), the
    /// clock of the fire whirls and the stun stars.
    pub real_time_ms: i32,
    /// `0x00602440(GC+0x800d44, zx, zy)`: the per-zone map record, `Some(record+0x30 & 1)` when
    /// it exists, `None` for a null record.
    pub zone_discovered: &'a dyn Fn(i32, i32) -> Option<bool>,
    /// `GC+0x1001004`: draw the path finder's nodes (a debug switch).
    pub show_paths: bool,
    /// The airships (cw-sim has none).
    pub airships: &'a [Airship],
}

/// What the pass writes back into the controller.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PassEffects {
    /// Flash lights pushed to `GC+0x800748` (`0x004863d0` at `0x004b4118`).
    pub flash_lights: Vec<FlashLight>,
    /// Creatures whose `creature+0x1d10` the pass cleared (every creature of the list, in
    /// order: `0x004b411d` and `0x004b4129`).
    pub explode_cleared: Vec<i64>,
    /// Creatures whose pose object `creature+0x1498` was drawn this frame (the body
    /// 0x004b5db2, the after-images 0x004b5cf3 or the ghost deferral drawn at
    /// 0x004ba8fe/0x004ba9a5), in order. Each call of `0x004128f0` sets `pose+0x878`
    /// (`creature+0x1d10`) to 1 at 0x0041293b, after this frame's clear at 0x004b4129.
    pub posed: Vec<i64>,
    /// Per posed creature, the body material `esp+0x7f4` ([`body_material`], ground light in
    /// the alpha) that 0x004b5cf3 and 0x004b5db2 pass to `0x004128f0` as the pose colour.
    pub body_materials: BTreeMap<i64, [f32; 4]>,
}

impl PassEffects {
    /// The pass's writes to the `creature+0x1d10` bytes: every creature of the list cleared
    /// (0x004b411d, 0x004b4129), then set again by its pose draw (0x0041293b). Per creature the
    /// clear comes first in the original, so applying all clears before all sets is the same.
    pub fn apply_flags(&self, pending: &mut BTreeSet<i64>) {
        for id in &self.explode_cleared {
            pending.remove(id);
        }
        pending.extend(self.posed.iter().copied());
    }
}

impl PoseParts {
    /// What one call of `0x004128f0` leaves in the pose object: the part model pointers
    /// `pose+0xbc..+0xe8` are set on every call (0x00412960..0x00412bc6, null when the slot
    /// has no model, some cleared by the animations), and each part matrix is rebuilt in place
    /// and handed to `setTransforms` only when its part is drawn (the head: 0x00420433 tests
    /// `pose+0xbc`, 0x00420440 copies the base into `pose+0x1ac`, 0x004206b7 draws with it), so
    /// an undrawn part keeps last frame's matrix. `info` maps a model index to its model.
    ///
    /// Simplification: the pointer of a slot is taken from the parts drawn, so a slot whose
    /// model is set but whose part the animation hid (`hide` bits) reads as null here.
    pub fn record_pose(&mut self, pose: &cw_render::pose::Pose, info: impl Fn(u32) -> Option<ModelInfo>) {
        use cw_render::pose::PartSlot as S;
        self.head = None;
        self.hair = None;
        self.body = None;
        self.arm = None;
        self.hands = None;
        self.feet = None;
        self.weapon_r = None;
        self.weapon_l = None;
        self.shoulder = None;
        for p in &pose.parts {
            let a = p.matrix.to_cols_array();
            let m: D3dMatrix = [[a[0], a[1], a[2], a[3]], [a[4], a[5], a[6], a[7]], [a[8], a[9], a[10], a[11]], [a[12], a[13], a[14], a[15]]];
            let model = info(p.model);
            match p.slot {
                // `pose+0xbc`, matrix `+0x1ac`.
                S::Head => (self.head, self.head_m) = (model, m),
                // `pose+0xc0` (its matrix `+0x1ec` is not read by the creature pass).
                S::Hair => self.hair = model,
                // `pose+0xc4`, `+0x16c`.
                S::Body => (self.body, self.body_m) = (model, m),
                // `pose+0xc8`, `+0x22c` / `+0x26c`.
                S::UpperArmLeft => (self.arm, self.arm_l_m) = (model, m),
                S::UpperArmRight => (self.arm, self.arm_r_m) = (model, m),
                // `pose+0xcc`, `+0x32c` / `+0x36c`.
                S::HandLeft => (self.hands, self.hand_l_m) = (model, m),
                S::HandRight => (self.hands, self.hand_r_m) = (model, m),
                // `pose+0xd8`, `+0x4ac` / `+0x4ec`.
                S::FootLeft => (self.feet, self.foot_l_m) = (model, m),
                S::FootRight => (self.feet, self.foot_r_m) = (model, m),
                // `pose+0xdc`, `+0x2ac`; `pose+0xe0`, `+0x2ec`.
                S::WeaponRight => (self.weapon_r, self.weapon_r_m) = (model, m),
                S::WeaponLeft => (self.weapon_l, self.weapon_l_m) = (model, m),
                // `pose+0xe4`, `+0x3ac`.
                S::ShoulderArmor => (self.shoulder, self.shoulder_m) = (model, m),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------------------

/// `0x004b35d4..0x004b7db6`, with placeholder inputs for what `SceneCtx` lacks (no airships, no
/// pose parts, nothing pending to explode, `real_time_ms = clock_ms`, no zone records, path
/// debug off). The flash lights and flag clears are dropped. See [`creature_pass_with`].
pub fn creature_pass(ctx: &SceneCtx, particles: &mut ParticleSystem, rng: &mut MsvcRand, out: &mut ScenePieces) {
    let parts = BTreeMap::new();
    let pending = BTreeSet::new();
    let none = |_: i32, _: i32| None;
    let inp = PassInputs {
        parts: &parts,
        explode_pending: &pending,
        real_time_ms: ctx.gc.clock_ms,
        zone_discovered: &none,
        show_paths: false,
        airships: &[],
    };
    let mut fx = PassEffects::default();
    creature_pass_with(ctx, &inp, particles, rng, out, &mut fx);
}

/// `0x004b35d4..0x004b7db6`: airships, the creature list and order, and per creature (far →
/// near) everything but its body, then the path debug cubes. Model draws go to
/// `out.creature_extras` in the original's order.
pub fn creature_pass_with(
    ctx: &SceneCtx,
    inp: &PassInputs,
    particles: &mut ParticleSystem,
    rng: &mut MsvcRand,
    out: &mut ScenePieces,
    fx: &mut PassEffects,
) {
    // 0x004b35d4..0x004b3801.
    for a in inp.airships {
        out.creature_extras.extend(airship_draw(ctx, a));
    }
    // 0x004b3801..0x004b3990.
    let order = creature_list(ctx);
    out.creature_order = order.clone();
    // 0x004b39e0..0x004b76be.
    for &id in &order {
        let Some(e) = ctx.entities.get(&id) else { continue };
        let st = ctx.states.get(&id);
        // 0x004b39ee: `comiss 0, hp; jb alive` (NaN is alive).
        let hp = f32_at(e, 0x15c);
        if 0.0 < hp || hp.is_nan() {
            living_creature(ctx, inp, id, e, st, particles, rng, &mut out.creature_extras, fx);
        } else {
            dying_creature(ctx, inp, id, e, st, particles, rng, fx);
        }
    }
    // 0x004b76c4..0x004b7db6.
    if inp.show_paths {
        path_debug(ctx, &order, &mut out.creature_extras);
    }
}

/// `0x004b35d4..0x004b37c3`: one airship: visible within the draw distance (margin 64), model
/// 0xa03, `T(render position) · Rz(yaw) · T(-size / 2)`, the world material.
pub fn airship_draw(ctx: &SceneCtx, a: &Airship) -> Option<ModelDraw> {
    if !sphere_visible(&ctx.planes, &ctx.gc.camera, a.position, 64.0, ctx.draw_distance) {
        return None;
    }
    let model = ctx.models.model(0xa03)?;
    let mut m = identity();
    pre_translate(&mut m, render_translation(a.render_pos, ctx.gc.camera.render_offset));
    pre_rotate_z(&mut m, a.yaw);
    pre_translate(&mut m, half_neg(model.size));
    Some(draw(0x004b_37be, model.handle, m, ctx.world_material))
}

/// `0x004b3801..0x004b3990`: every creature with its squared render distance from the camera
/// (`creature+0x1350 - GC+0x140`, blocks²), in map order; emptied when panel 874, 888, 894 or
/// 88c is open (`0x0044be20`); then `0x004aba90` sorts it far → near (predicate
/// `a.dist > b.dist`).
pub fn creature_list(ctx: &SceneCtx) -> Vec<i64> {
    let cam = ctx.gc.camera.position;
    let mut list: Vec<(i64, f32)> = Vec::new();
    for (&id, e) in ctx.entities {
        let d = length_sq(float_from_fixed(sub3(render_pos(ctx, id, e), cam)));
        list.push((id, d));
    }
    if ctx.gc.gui.hides_creatures() {
        list.clear();
    }
    cw_math::sort::msvc_sort(&mut list, |a, b| a.1 > b.1);
    list.into_iter().map(|(id, _)| id).collect()
}

// ---------------------------------------------------------------------------------------
// Per creature
// ---------------------------------------------------------------------------------------

/// `creature+0x1350` (the render position), the entity position without a state.
fn render_pos(ctx: &SceneCtx, id: i64, e: &EntityData) -> [i64; 3] {
    ctx.states.get(&id).map_or(entity_pos(e), |s| s.riding.render_pos)
}

/// `esp+0xbc`: `GC+0x1d0 × 0.3`, the distance of the creature effects.
fn effect_range(ctx: &SceneCtx) -> f32 {
    ctx.gc.view_distance * 0.3
}

/// Whether the body of a creature (the pose `0x004128f0` at 0x004b5db2, its after-images
/// and the ghost deferral) is drawn this frame. The body draw sits inside the living branch of
/// the creature loop: 0x004b39ee `comiss 0, hp; jb 0x004b4129` sends `hp > 0` (and NaN) there;
/// `hp <= 0` runs [`dying_creature`] and jumps to the loop end 0x004b7685, so a dead creature
/// has no body at all. Inside the living branch 0x004b4e5b drops every creature but the local
/// player on a white-cleared frame. (The sphere test 0x004b4e1b is done by the frame builder.)
pub fn body_drawn(e: &EntityData, id: i64, player: i64, white_clear: bool) -> bool {
    // 0x004b39ee: `comiss 0, hp; jb alive` (NaN is alive).
    let hp = f32_at(e, 0x15c);
    if !(0.0 < hp || hp.is_nan()) {
        return false;
    }
    // 0x004b4e5b
    !(white_clear && id != player)
}

/// `0x004b39ee..0x004b4124`: a creature with `hp <= 0`. When its explosion is pending
/// (`creature+0x1d10`) and it is visible (render position lowered by the step offset
/// `+0x1180`, margin its height, three times for mode 0x6a, within `GC+0x1d0 × 0.3`), its
/// parts burst into particles: head, hair (unless appearance flag 0x400; hair colour), body,
/// the two hands, the two upper arms, the two feet and the right weapon, each
/// `0x00470d80(model, part matrix, colour, kind 0)` (`rand()` per surface voxel); a creature
/// of type 0x90 also flashes (colour (1, 0.8, 0.5), radius 16, 800 ms). The flag is cleared
/// either way; nothing else of the creature is drawn.
#[allow(clippy::too_many_arguments)]
pub fn dying_creature(
    ctx: &SceneCtx,
    inp: &PassInputs,
    id: i64,
    e: &EntityData,
    st: Option<&CreatureState>,
    particles: &mut ParticleSystem,
    rng: &mut MsvcRand,
    fx: &mut PassEffects,
) {
    // 0x004b3a06.
    let mut radius = f32_at(e, 0x78);
    if e.0[0x58] == 0x6a {
        radius *= 3.0;
    }
    // 0x004b3a31.
    if inp.explode_pending.contains(&id) {
        let step = st.map_or(0.0, |s| s.step_offset);
        let centre = add3(render_pos(ctx, id, e), fixed_from_float([0.0, 0.0, -step]));
        if sphere_visible(&ctx.planes, &ctx.gc.camera, centre, radius, effect_range(ctx)) {
            if let Some(p) = inp.parts.get(&id) {
                let white = [1.0f32; 4];
                // 0x004b3b6f: head.
                explode(ctx, particles, rng, p.head, &p.head_m, white);
                // 0x004b3c50: hair, with the hair colour (`entity+0x6a..0x6c` / 255).
                if u16_at(e, 0x6e) & 0x400 == 0 {
                    let hc = [f32::from(e.0[0x6a]) / 255.0, f32::from(e.0[0x6b]) / 255.0, f32::from(e.0[0x6c]) / 255.0, 1.0];
                    explode(ctx, particles, rng, p.hair, &p.head_m, hc);
                }
                // 0x004b3cd9: body; 0x004b3d62, 0x004b3deb: hands; 0x004b3e74, 0x004b3efd:
                // upper arms; 0x004b3f86, 0x004b400f: feet; 0x004b4098: right weapon.
                explode(ctx, particles, rng, p.body, &p.body_m, white);
                explode(ctx, particles, rng, p.hands, &p.hand_l_m, white);
                explode(ctx, particles, rng, p.hands, &p.hand_r_m, white);
                explode(ctx, particles, rng, p.arm, &p.arm_l_m, white);
                explode(ctx, particles, rng, p.arm, &p.arm_r_m, white);
                explode(ctx, particles, rng, p.feet, &p.foot_l_m, white);
                explode(ctx, particles, rng, p.feet, &p.foot_r_m, white);
                explode(ctx, particles, rng, p.weapon_r, &p.weapon_r_m, white);
            }
            // 0x004b409d: type 0x90 flashes at its entity position.
            if i32_at(e, 0x54) == 0x90 {
                fx.flash_lights.push(FlashLight {
                    position: entity_pos(e),
                    color: [1.0, 0.8, 0.5],
                    radius: 16.0,
                    age: 0,
                    duration: 800,
                });
            }
        }
    }
    // 0x004b411d.
    fx.explode_cleared.push(id);
}

/// `0x00470d80(model, matrix, …, colour, kind 0)`; a null model draws nothing (and no `rand()`).
fn explode(ctx: &SceneCtx, particles: &mut ParticleSystem, rng: &mut MsvcRand, model: Option<ModelInfo>, m: &D3dMatrix, color: [f32; 4]) {
    if let Some(mi) = model
        && let Some(grid) = ctx.models.voxels(mi.handle)
    {
        particles.explode_model(grid, m, color, 0, ctx.gc.camera.render_offset, rng);
    }
}

/// `0x004b4129..0x004b7685`: a living creature, everything but the body, in order.
#[allow(clippy::too_many_arguments)]
pub fn living_creature(
    ctx: &SceneCtx,
    inp: &PassInputs,
    id: i64,
    e: &EntityData,
    st: Option<&CreatureState>,
    particles: &mut ParticleSystem,
    rng: &mut MsvcRand,
    out: &mut Vec<ModelDraw>,
    fx: &mut PassEffects,
) {
    let gc = ctx.gc;
    // 0x004b4129: the explosion flag of a living creature is dropped.
    fx.explode_cleared.push(id);
    let mode = e.0[0x58];
    let mt = i32_at(e, 0x5c);
    let (guard, haste) = st.map_or((0.0, false), |s| (s.block, has_buff(s, 0xc)));
    let total = || cw_sim::skills::skill_total_time(e, guard, haste);
    let windup = || cw_sim::skills::skill_windup(e, guard, haste, -1);
    let range = effect_range(ctx);
    let aim = st.map_or([0.0; 3], |s| s.aim);
    let step = st.map_or(0.0, |s| s.step_offset);
    let class = e.0[0x131];
    let rpos = render_pos(ctx, id, e);
    let epos = entity_pos(e);
    let parts = inp.parts.get(&id);
    let origin = gc.poses.get(&id).map_or([0; 2], |p| p.origin);

    // 0x004b4130..0x004b454e: modes 0x1e..0x21, two whirls at the aim point.
    if matches!(mode, 0x1e..=0x21) && mt < total() {
        let at = add3(epos, fixed_from_float(aim));
        if sphere_visible(&ctx.planes, &gc.camera, at, 4.0, range) {
            let mtf = mt as f32;
            let mut twist = (mtf / windup() as f32) * 0.1;
            let mut radius = (mtf * 2.0) / total() as f32;
            let mut count = 0x2b;
            // 0x004b4226: `cmp mt, windup; jle`.
            if mt > windup() {
                twist *= 2.0;
                count = 0x56;
                radius *= 2.0;
            }
            let (ca, cb) = if class == 1 {
                ([0.0, 0.1, 1.0, 1.0], [0.5, 0.7, 1.0, 1.0])
            } else {
                ([1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0])
            };
            // 0x004b4430.
            let p = add3(sub3(epos, fixed_from_float([0.0, 0.0, radius * 0.5])), fixed_from_float(aim));
            let size = ((mt as f32) * 0.4) / total() as f32;
            out.extend(whirl_4bc760(ctx, p, ca, cb, inp.real_time_ms, radius, size, twist, count));
            // 0x004b4549: modes 0x1f and 0x21 add a second, wider one.
            if mode == 0x1f || mode == 0x21 {
                let p = add3(sub3(epos, fixed_from_float([0.0, 0.0, radius])), fixed_from_float(aim));
                let size = ((mt as f32) * 0.4) / total() as f32;
                out.extend(whirl_4bc760(ctx, p, ca, cb, inp.real_time_ms, radius * 2.0, size, twist * 0.5, count * 2));
            }
        }
    }

    // 0x004b454e..0x004b467b: mode 0x22, a whirl around the target (`entity+0x190`).
    let target = i64_at(e, 0x190);
    if target != 0
        && mode == 0x22
        && mt < total()
        && let Some(te) = ctx.entities.get(&target)
    {
        let trp = render_pos(ctx, target, te);
        if sphere_visible(&ctx.planes, &gc.camera, trp, f32_at(te, 0x78), range) {
            out.extend(whirl_4bbd80(ctx, trp, [0.0, 0.0, 1.0, 1.0], [0.25, 0.75, 1.0, 1.0], mt, f32_at(te, 0x70), 0.1, 0x14));
        }
    }

    // 0x004b467b..0x004b4e19: the ray of modes 0x1c (while `entity+0x160 > 0`), 0x5e, 0x5f.
    let ray_mode = (mode == 0x1c && f32_at(e, 0x160) > 0.0) || ((mode == 0x5f || mode == 0x5e) && mt < total());
    let mut ray_drawn = false;
    if ray_mode
        && !(mt < windup())
        && !(i32_at(e, 0x11c) > 0)
        && sphere_visible(&ctx.planes, &gc.camera, epos, 200.0, range)
    {
        out.push(ray_draw(ctx, e, st, parts, origin, windup()));
        ray_drawn = true;
    }
    // 0x004b4e1b: otherwise the creature itself must be visible (margin its height).
    if !ray_drawn && !sphere_visible(&ctx.planes, &gc.camera, epos, f32_at(e, 0x78), range) {
        return;
    }
    // 0x004b4e5b: a white-cleared frame shows only the local player.
    if gc.white_clear && id != gc.player {
        return;
    }

    // 0x004b4e70..0x004b53d5 (body light) and the buff tints: the body material.
    let body_mat = body_material(ctx, e, st);
    // 0x004b53d5..0x004b5aaf: the first buff loop.
    if let Some(s) = st {
        for b in &s.buffs {
            buff_models(ctx, e, rpos, step, b, inp.real_time_ms, out);
        }
    }
    // 0x004b5aaf..0x004b5db7: the body (drawn from `CreatureInput`, not here). Every path
    // through this range calls `0x004128f0` on `creature+0x1498` (after-images 0x004b5cf3,
    // the body 0x004b5db2, or the ghost list 0x004b5d53 drawn at 0x004ba8fe/0x004ba9a5), and
    // 0x0041293b sets `creature+0x1d10` = 1: the explosion flag the dead branch reads next
    // frame (0x004b3a31).
    fx.posed.push(id);
    fx.body_materials.insert(id, body_mat);
    // 0x004b5db7..0x004b5ef3: spirit cubes of the equipment.
    if let Some(p) = parts {
        spirit_cubes(ctx, e, p, body_mat, out);
    }
    // 0x004b5ef8..0x004b62fc.
    let stun = i32_at(e, 0x11c);
    stun_models(ctx, e, mode, mt, stun, inp.real_time_ms, out);
    // 0x004b6300: `cmp stun, 0; jg 0x004b7085`.
    if !(stun > 0) {
        // 0x004b630d..0x004b68f5: modes 0x25/0x26/0x2b/0x2c during the wind-up: a beam.
        if matches!(mode, 0x25 | 0x26 | 0x2b | 0x2c) && mt < windup() {
            let colors = if class == 1 {
                [[0.0, 0.0, 1.0, 1.0], [0.0, 0.25, 1.0, 1.0], [0.25, 0.5, 1.0, 1.0]]
            } else {
                [[1.0, 0.0, 0.0, 1.0], [1.0, 0.25, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0]]
            };
            let mut k = (mt as f32) / windup() as f32;
            if mode == 0x26 {
                k *= 0.5;
            }
            let start = beam_origin(e, parts, origin);
            let n = normalized(vec3f_at(e, 0x150));
            let args = BeamArgs {
                start,
                dir: [n[0] * k, n[1] * k, n[2] * k],
                angle: f32_at(e, 0x20),
                colors,
                time: mt,
                p11: 1.0,
                size: k,
                p13: 1.0,
                segments: 0x50,
                sparks: false,
            };
            // 0x004b68f0.
            out.extend(draw_beam(ctx, &args, particles, rng));
        }
        // 0x004b68f5..0x004b6d47: the second buff loop.
        if let Some(s) = st {
            for b in &s.buffs {
                out.extend(buff_whirls(ctx, rpos, step, b, inp.real_time_ms));
            }
        }
        // 0x004b6d47..0x004b6e85: modes 0x18/0x19: a beam along the ray, one block ahead.
        if mode == 0x18 || mode == 0x19 {
            let n = normalized(vec3f_at(e, 0x150));
            let k = f32_at(e, 0x134) * 0.5;
            let args = BeamArgs {
                start: add3(epos, fixed_from_float(n)),
                dir: n,
                angle: f32_at(e, 0x20),
                colors: [[0.5, 0.5, 1.0, 1.0], [0.75, 0.75, 1.0, 1.0], [0.9, 0.9, 1.0, 1.0]],
                time: mt,
                p11: k,
                size: k,
                p13: 1.0,
                segments: 0x14,
                sparks: false,
            };
            // 0x004b6e7c.
            out.extend(draw_beam(ctx, &args, particles, rng));
        }
        // 0x004b6e85..0x004b7085: modes 0x57/0x58: four fire whirls stacked up.
        if (mode == 0x57 || mode == 0x58) && mt < total() {
            let mut k = (mt as f32) / windup() as f32;
            // 0x004b6ed5: `comiss k, 1; jbe`.
            if k > 1.0 {
                k = 20.0 - k * 19.0;
                if 0.0 > k {
                    k = 0.0;
                }
            }
            let a = k * 4.0;
            let b = k * 0.5;
            let mut i = 0i32;
            let mut count = 0x14i32;
            loop {
                let p = sub3(epos, fixed_from_float([0.0, 0.0, (step + 1.0) + a]));
                let radius = ((i as f32) * 2.0) * k + 1.0;
                // 0x004b705c.
                out.extend(whirl_4bbd80(ctx, p, [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0], inp.real_time_ms, radius, b, count));
                i += 1;
                count += 0xd;
                if count >= 0x48 {
                    break;
                }
            }
        }
    }
    // 0x004b7085..0x004b72cb.
    if mode == 0x65 && mt < total() {
        out.extend(mode_65_model(ctx, e, step, mt, total(), body_mat));
    }
    // 0x004b72cb..0x004b7685.
    if e.0[0x50] == 3 {
        out.extend(marker_model(ctx, inp, e, step));
    }
}

/// The body material of `0x004b4e70..0x004b5b44`: `(1, 1, 1, light)` with `light` the ground
/// light `0x004718b0` 0.1 below the entity position / 255; × (0.5, 0.5, 1.5, 1) while
/// `entity+0x124 > 0`; × (1, 0.5, 0.5, 1) per buff of type 1 or 2 and × (0.5, 1.5, 0.5, 1) per
/// buff of type 4 (`0x004127c0`); then × the world material (`0x00412120`); and with appearance
/// flag 0x200 × `(0x004460f0 + 1) · 0.5`. The spirit cubes and model 0x42b use it.
pub fn body_material(ctx: &SceneCtx, e: &EntityData, st: Option<&CreatureState>) -> [f32; 4] {
    let p = entity_pos(e);
    // 0x004b4e70..0x004b4ed9: `groundLightAt(Millisecs(x), Millisecs(y), Millisecs(z - 0.1))`.
    let z = p[2].wrapping_sub(ftol(0.1f32 * 65536.0));
    let b = cw_render::light::get_block(ctx.world, (p[0] / 65536) as i32, (p[1] / 65536) as i32, (z / 65536) as i32);
    let light = f32::from(cw_render::light::prop_light(b)) / 255.0;
    let mut c = [1.0f32, 1.0, 1.0, light];
    let mul = |c: &mut [f32; 4], k: [f32; 4]| {
        *c = [k[0] * c[0], k[1] * c[1], k[2] * c[2], k[3] * c[3]];
    };
    // 0x004b5391.
    if i32_at(e, 0x124) > 0 {
        mul(&mut c, [0.5, 0.5, 1.5, 1.0]);
    }
    if let Some(s) = st {
        for b in &s.buffs {
            if b[0] == 1 || b[0] == 2 {
                mul(&mut c, [1.0, 0.5, 0.5, 1.0]);
            }
            if b[0] == 4 {
                mul(&mut c, [0.5, 1.5, 0.5, 1.0]);
            }
        }
    }
    // 0x004b5ac6.
    let wm = ctx.world_material;
    let mut m = [wm[0] * c[0], wm[1] * c[1], wm[2] * c[2], wm[3] * c[3]];
    // 0x004b5acb.
    if u16_at(e, 0x6e) & 0x200 != 0 {
        let k = if e.0[0x50] == 1 { [0.8f32, 0.0, 0.5, 1.0] } else { [1.0; 4] };
        let s = [(k[0] + 1.0) * 0.5, (k[1] + 1.0) * 0.5, (k[2] + 1.0) * 0.5, (k[3] + 1.0) * 0.5];
        mul(&mut m, s);
    }
    m
}

/// `0x004b53d5..0x004b5a6f`, one buff of the first loop: type 4 a green whirl `0x004bbd80`
/// at the render position (step offset removed; its clock `100000 - buff+8`, radius the local
/// player's width, the original's choice); type 7 model 0x9f8 and type 8 model 0x9f9 above the
/// creature (half its height plus 0.25, step offset removed).
fn buff_models(ctx: &SceneCtx, e: &EntityData, rpos: [i64; 3], step: f32, b: &[u8; 0x18], _real: i32, out: &mut Vec<ModelDraw>) {
    let gc = ctx.gc;
    let player_width = ctx.entities.get(&gc.player).map_or(0.0, |p| f32_at(p, 0x70));
    // 0x004b547b: type 4.
    if b[0] == 4 {
        let p = sub3(rpos, fixed_from_float([0.0, 0.0, step]));
        let time = 100_000i32.wrapping_sub(i32::from_le_bytes(b[8..12].try_into().unwrap()));
        out.extend(whirl_4bbd80(ctx, p, [0.0, 1.0, 0.0, 1.0], [0.5, 1.0, 0.0, 1.0], time, player_width, 0.1, 8));
    }
    // The raised position of 0x004b5623 / 0x004b587a: z + (h/2) + 0.25 - step, each `ftol`.
    let raised = || {
        let p = entity_pos(e);
        let z = p[2]
            .wrapping_add(ftol(f32_at(e, 0x78) * 0.5 * 65536.0))
            .wrapping_add(ftol(0.25f32 * 65536.0))
            .wrapping_sub(ftol(step * 65536.0));
        render_translation([p[0], p[1], z], gc.camera.render_offset)
    };
    // 0x004b55be: type 7, model 0x9f8 turned toward the camera with a sway.
    if b[0] == 7
        && let Some(model) = ctx.models.model(0x9f8)
    {
        let mut m = identity();
        pre_translate(&mut m, raised());
        let s = f32::from_le_bytes(b[4..8].try_into().unwrap()) * 0.1 + 0.02;
        pre_scale(&mut m, [s, s, s]);
        let sway = cosf((gc.clock_ms as f32) * 0.001) * 40.0;
        pre_rotate_z(&mut m, sway + (gc.camera.yaw + 180.0));
        pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
        out.push(draw(0x004b_5803, model.handle, m, [1.0; 4]));
    }
    // 0x004b5810: type 8, model 0x9f9 spinning, pulsing with the creature's width.
    if b[0] == 8
        && let Some(model) = ctx.models.model(0x9f9)
    {
        let mut m = identity();
        pre_translate(&mut m, raised());
        let k = cosf((gc.clock_ms as f32) * 0.005) * 0.1 + 1.0;
        let s = k * (f32_at(e, 0x70) * 0.07);
        pre_scale(&mut m, [s, s, s]);
        pre_rotate_z(&mut m, (gc.clock_ms as f32) * 0.02);
        pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
        out.push(draw(0x004b_5a66, model.handle, m, [1.0; 4]));
    }
}

/// `0x004b5db7..0x004b5ee0`: the spirit cubes `0x00471b60` of the right weapon (slot 7,
/// matrix `+0x1744`), the left weapon (slot 6, `+0x1784`), the chest (slot 2, body matrix) and
/// the shoulders (slot 5, `+0x1844`), each only while that part has a model; the colour base is
/// the body material and the strength the charge `entity+0x60 / maxBlock`.
fn spirit_cubes(ctx: &SceneCtx, e: &EntityData, p: &PoseParts, body_mat: [f32; 4], out: &mut Vec<ModelDraw>) {
    let charge = (i32_at(e, 0x60) as f32) / (cw_sim::skills::max_block(e) as f32);
    if p.weapon_r.is_some() {
        out.extend(spirit_cubes_471b60(ctx, equip(e, 7), &p.weapon_r_m, body_mat, charge));
    }
    if p.weapon_l.is_some() {
        out.extend(spirit_cubes_471b60(ctx, equip(e, 6), &p.weapon_l_m, body_mat, charge));
    }
    if p.body.is_some() {
        out.extend(spirit_cubes_471b60(ctx, equip(e, 2), &p.body_m, body_mat, charge));
    }
    if p.shoulder.is_some() {
        out.extend(spirit_cubes_471b60(ctx, equip(e, 5), &p.shoulder_m, body_mat, charge));
    }
}

/// `0x00471b60(item, matrix, view, projection, base, strength)`: for an item (type byte not 0),
/// one particle cube per spirit cube (`item+0x14 + 8i`: signed x, y, z, material; count
/// `item+0x114`), at `matrix · T(x, y, z)`, coloured `material_color(material, base, strength)`
/// (`0x004c7250`). The original also sets the shininess of `0x004c7be0(item)` around the loop,
/// which `ModelDraw` does not carry.
pub fn spirit_cubes_471b60(ctx: &SceneCtx, item: &[u8], matrix: &D3dMatrix, base: [f32; 4], strength: f32) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    if item[0] == 0 {
        return out;
    }
    let count = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap());
    for i in 0..count.max(0) as usize {
        let o = 0x14 + i * 8;
        let Some(c) = item.get(o..o + 4) else { break };
        let (x, y, z) = (c[0] as i8 as f32, c[1] as i8 as f32, c[2] as i8 as f32);
        let mut m = *matrix;
        pre_translate(&mut m, [x, y, z]);
        let color = material_color(c[3], base, strength);
        out.push(draw(0x0047_1cfc, ctx.models.particle_cube(), m, color));
    }
    out
}

/// `0x004b5ef8..0x004b62fc`: while `entity+0x11c > -3000` (stunned or just recovered) model
/// 0x9f5 spins half a height above the creature (scale width × 0.1, turn
/// `(realTime % 360000) × 0.2`); otherwise, in mode 0x54, model 0x9f7 a tenth of the height up
/// (scale `(cos(mt·0.005)·0.1 + 1) · width · 0.07`, turned to the camera with a sway, not
/// centred).
fn stun_models(ctx: &SceneCtx, e: &EntityData, mode: u8, mt: i32, stun: i32, real: i32, out: &mut Vec<ModelDraw>) {
    let gc = ctx.gc;
    let p = entity_pos(e);
    let up = |k: f32| {
        let z = p[2].wrapping_add(ftol(f32_at(e, 0x78) * k * 65536.0));
        render_translation([p[0], p[1], z], gc.camera.render_offset)
    };
    // 0x004b5ef8: `cmp stun, -3000; jle`.
    if stun > -3000 {
        let Some(model) = ctx.models.model(0x9f5) else { return };
        let mut m = identity();
        pre_translate(&mut m, up(0.5));
        let s = f32_at(e, 0x70) * 0.1;
        pre_scale(&mut m, [s, s, s]);
        pre_rotate_z(&mut m, ((real % 360_000) as f32) * 0.2);
        pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
        out.push(draw(0x004b_60dd, model.handle, m, [1.0; 4]));
    } else if mode == 0x54 {
        let Some(model) = ctx.models.model(0x9f7) else { return };
        let mut m = identity();
        pre_translate(&mut m, up(0.1));
        let k = cosf((mt as f32) * 0.005) * 0.1 + 1.0;
        let s = k * (f32_at(e, 0x70) * 0.07);
        pre_scale(&mut m, [s, s, s]);
        let yaw = gc.camera.yaw + 180.0;
        pre_rotate_z(&mut m, yaw + cosf((mt as f32) * 0.001) * 40.0);
        out.push(draw(0x004b_62f5, model.handle, m, [1.0; 4]));
    }
}

/// `0x004b68f5..0x004b6d09`, one buff of the second loop, at the render position without the
/// step offset, clock `realTime`, radius from the local player's width: type 6 the blue
/// `0x004c04c0` (40 cubes, width + 0.5, size 0.05); types 9 (red → yellow), 10 (magenta → pink),
/// 11 (black → violet) `0x004bbd80` (20 cubes, size 0.1).
fn buff_whirls(ctx: &SceneCtx, rpos: [i64; 3], step: f32, b: &[u8; 0x18], real: i32) -> Vec<ModelDraw> {
    let w = ctx.entities.get(&ctx.gc.player).map_or(0.0, |p| f32_at(p, 0x70));
    let p = sub3(rpos, fixed_from_float([0.0, 0.0, step]));
    match b[0] {
        6 => whirl_4c04c0(ctx, p, [0.2, 0.2, 1.0, 1.0], [0.2, 0.75, 1.0, 1.0], real, w + 0.5, 0.05, 0x28),
        9 => whirl_4bbd80(ctx, p, [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0], real, w, 0.1, 0x14),
        10 => whirl_4bbd80(ctx, p, [1.0, 0.0, 1.0, 1.0], [1.0, 0.5, 1.0, 1.0], real, w, 0.1, 0x14),
        11 => whirl_4bbd80(ctx, p, [0.0, 0.0, 0.0, 1.0], [0.5, 0.0, 1.0, 1.0], real, w, 0.1, 0x14),
        _ => Vec::new(),
    }
}

/// `0x004b709f..0x004b72c6`: mode 0x65, model 0x42b above the creature (half its height plus
/// 0.25, step offset removed), turned by the yaw, `0.8 / size x` scaled in over the first
/// 200 ms and out over the last 200 ms, in the body material.
fn mode_65_model(ctx: &SceneCtx, e: &EntityData, step: f32, mt: i32, total: i32, body_mat: [f32; 4]) -> Option<ModelDraw> {
    let model = ctx.models.model(0x42b)?;
    let mut m = identity();
    pre_translate(&mut m, above_head(ctx, e, step));
    pre_rotate_axis(&mut m, f32_at(e, 0x20), [0.0, 0.0, 1.0]);
    let s0 = 0.8 / (model.size[0] as f32);
    let mut s = s0;
    if mt < 200 {
        s = ((mt as f32) / 200.0) * s0;
    }
    // 0x004b71e7: `cmp mt, total - 200; jle`.
    if mt > total.wrapping_sub(200) {
        s *= (total.wrapping_sub(mt) as f32) / 200.0;
    }
    pre_scale(&mut m, [s, s, s]);
    pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
    Some(draw(0x004b_72c6, model.handle, m, body_mat))
}

/// `fromFixed(pos + (render offset, ftol((h · 0.5 + 0.25 - step) · 65536)))`, the translation of
/// models 0x42b and the markers (`0x004b70c7..0x004b7151`).
fn above_head(ctx: &SceneCtx, e: &EntityData, step: f32) -> [f32; 3] {
    let dz = ftol(((f32_at(e, 0x78) * 0.5 + 0.25) - step) * 65536.0);
    let off = ctx.gc.camera.render_offset;
    float_from_fixed(add3(entity_pos(e), [off[0], off[1], dz]))
}

/// `0x004b72cb..0x004b7685`: the marker above a creature of hostility 3. The model comes from
/// `entity+0x130` (0x80..0x82 → 0x90d, 0x83 → 0x90c, 0x89 → 0xa00); with a home zone
/// (`entity+0x1cc`, `+0x1d0`, x not negative) whose zone record (`0x004a6ad0`) or else cell
/// (`getCell(zx/8, zy/8)`) has a mission within two levels of the local player, it becomes
/// 0x90b; its colour is white when the zone map record (`0x00602440`) is marked, else
/// (0.25, 1, 0.25). Scaled to 0.8 blocks wide and turned by the yaw.
fn marker_model(ctx: &SceneCtx, inp: &PassInputs, e: &EntityData, step: f32) -> Option<ModelDraw> {
    let mut color = [1.0f32; 4];
    let race = e.0[0x130];
    let mut model = None;
    if matches!(race, 0x80..=0x82) {
        model = ctx.models.model(0x90d);
    }
    if race == 0x83 {
        model = ctx.models.model(0x90c);
    }
    if race == 0x89 {
        model = ctx.models.model(0xa00);
    }
    let zx = i32_at(e, 0x1cc);
    // 0x004b738a: `test zx, zx; js`.
    if zx >= 0 {
        let zy = i32_at(e, 0x1d0);
        let known = (inp.zone_discovered)(zx, zy);
        let level = ctx.entities.get(&ctx.gc.player).map(|p| i32_at(p, 0x180));
        let in_range = |l: i32| level.is_some_and(|pl| !(l < pl.wrapping_sub(2)) && !(l > pl.wrapping_add(2)));
        // 0x004b73c8: the zone record, else the cell.
        let zone = zone_record(ctx.world, zx, zy);
        let mut quest = false;
        if let Some(z) = zone
            && z.kind != 0
            && in_range(z.level)
        {
            quest = true;
        }
        if !quest
            && let Some(c) = ctx.world.cell(zx / 8, zy / 8)
            && c.kind != 0
            && in_range(c.level)
        {
            quest = true;
        }
        if quest {
            model = ctx.models.model(0x90b);
        }
        // 0x004b7459.
        model?;
        // 0x004b7461.
        color = if known == Some(true) { [1.0, 1.0, 1.0, 1.0] } else { [0.25, 1.0, 0.25, 1.0] };
    }
    let model = model?;
    let mut m = identity();
    pre_translate(&mut m, above_head(ctx, e, step));
    pre_rotate_axis(&mut m, f32_at(e, 0x20), [0.0, 0.0, 1.0]);
    let s = 0.8 / (model.size[0] as f32);
    pre_scale(&mut m, [s, s, s]);
    pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
    Some(draw(0x004b_7680, model.handle, m, color))
}

/// `0x004a6ad0(zx, zy)`: the 16-byte zone record of region `(zx/64, zy/64)` at
/// `+0x18 + ((zx%64)·64 + zy%64)·16`, for `0 <= zx, zy < 0x10000`.
fn zone_record(world: &cw_world::World, zx: i32, zy: i32) -> Option<cw_world::region::ZoneRecord> {
    if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
        return None;
    }
    let r = world.region(zx / 64, zy / 64)?;
    r.zones.get(((zx % 64) * 64 + zy % 64) as usize).copied()
}

// ---------------------------------------------------------------------------------------
// The ray and the beam origins
// ---------------------------------------------------------------------------------------

/// A point of a part in world coordinates: the part matrix applied to `v`, back from render
/// space with the pose's render offset (`fixed(point) + (-origin x, -origin y, 0)`, the
/// `0x00412260` negations of `creature+0x1d18`/`+0x1d20`).
fn part_point(m: &D3dMatrix, v: [f32; 3], origin: [i64; 2]) -> [i64; 3] {
    let p = fixed_from_float(transform_point(m, v));
    add3([origin[0].wrapping_neg(), origin[1].wrapping_neg(), 0], p)
}

/// The centre of a model: `vec3f(size x, y, z) · 0.5` (`0x00451510`).
fn half(mi: ModelInfo) -> [f32; 3] {
    [(mi.size[0] as f32) * 0.5, (mi.size[1] as f32) * 0.5, (mi.size[2] as f32) * 0.5]
}

/// `(size x, y, z) · -0.5`: the usual centring.
fn half_neg(size: [i32; 3]) -> [f32; 3] {
    [(size[0] as f32) * -0.5, (size[1] as f32) * -0.5, (size[2] as f32) * -0.5]
}

/// `0x004b64c0..0x004b6864`: where the beam of modes 0x25.. starts: the shoot origin
/// (`0x00446bb0`), or the head centre (appearance flag 4 with a head model), or for a slot-7
/// weapon of sub type 0xc on an odd `entity+0x60` the left hand (its centre, or its matrix
/// origin without a hand model), or else the right hand's centre when there is a hand model.
/// Without pose parts the shoot origin.
fn beam_origin(e: &EntityData, parts: Option<&PoseParts>, origin: [i64; 2]) -> [i64; 3] {
    let so = shoot_origin(e);
    let Some(p) = parts else { return so };
    if u16_at(e, 0x6e) & 4 != 0
        && let Some(h) = p.head
    {
        return part_point(&p.head_m, half(h), origin);
    }
    // 0x004b65c1: `entity+0x60 % 2 != 0` (C remainder).
    if equip(e, 7)[1] == 0xc && i32_at(e, 0x60) % 2 != 0 {
        return match p.hands {
            Some(h) => part_point(&p.hand_l_m, half(h), origin),
            None => part_point(&p.hand_l_m, [0.0; 3], origin),
        };
    }
    match p.hands {
        Some(h) => part_point(&p.hand_r_m, half(h), origin),
        None => so,
    }
}

/// `0x004b46ce..0x004b4e14`: the ray. From the shoot origin along the normalised aim
/// (`creature+0x138c`) `World::sweep` finds the end (at most 100 blocks, stopping at solid
/// blocks and statics); the drawn ray starts at the head centre (flag 4 with a head model), the
/// right hand's centre, or the right hand's matrix origin. A particle cube stretched from start
/// to end: `T(start) · rotation (0, 1, 0) → end - start · S(w, length · k, w) · T(-0.5, 0,
/// -0.5)` with `w` 0.25 in mode 0x5f else 0.1 and `k = min((mt - windup) · 0.01, 1)`; the
/// colour pulses with the world clock by class (1: blue-white, 2: green, else yellow).
fn ray_draw(ctx: &SceneCtx, e: &EntityData, st: Option<&CreatureState>, parts: Option<&PoseParts>, origin: [i64; 2], windup: i32) -> ModelDraw {
    let gc = ctx.gc;
    let aim = st.map_or([0.0; 3], |s| s.aim);
    let dir = normalized(aim);
    let from = shoot_origin(e);
    // 0x004b4743: `World::sweep(origin, dir, 100, 0, 1)`.
    let d = cw_sim::path::sweep(ctx.world, from, dir, 100.0, false, true);
    let end = add3(from, fixed_from_float([dir[0] * d, dir[1] * d, dir[2] * d]));
    let start = match parts {
        None => from,
        Some(p) => {
            if u16_at(e, 0x6e) & 4 != 0
                && let Some(h) = p.head
            {
                part_point(&p.head_m, half(h), origin)
            } else if let Some(h) = p.hands {
                part_point(&p.hand_r_m, half(h), origin)
            } else {
                part_point(&p.hand_r_m, [0.0; 3], origin)
            }
        }
    };
    let mut m = identity();
    pre_translate(&mut m, render_translation(start, gc.camera.render_offset));
    rotate_from_to(&mut m, [0.0, 1.0, 0.0], float_from_fixed(sub3(end, start)));
    let w = if e.0[0x58] == 0x5f { 0.25 } else { 0.1 };
    let kk = (i32_at(e, 0x5c).wrapping_sub(windup) as f32) * 0.01;
    // 0x004b4b52: `comiss 1, k; jbe`.
    let k = if 1.0 > kk { kk } else { 1.0 };
    let len = length(float_from_fixed(sub3(start, end)));
    pre_scale(&mut m, [w, len * k, w]);
    pre_translate(&mut m, [-0.5, 0.0, -0.5]);
    // 0x004b4c3b: the colour by class, `0x0040e420` cosines of the world clock.
    let t = gc.clock_ms as f32;
    let color = match e.0[0x131] {
        1 => {
            let c = cosf(t * 0.01) * 0.3 + 0.3;
            let d = cosf(t * 0.01) * 0.3 + 0.4;
            [d, c, 1.0, 1.0]
        }
        2 => {
            let b = cosf(t * 0.01) * 0.3 + 0.4;
            let d = cosf(t * 0.01) * 0.3 + 0.3;
            [d, 1.0, b, 1.0]
        }
        _ => {
            let c = (((cosf(t * 0.03) * 0.3) as f64) + 0.7) as f32;
            [1.0, c, 0.4, 1.0]
        }
    };
    draw(0x004b_4e14, ctx.models.particle_cube(), m, color)
}

/// `Creature::shootOrigin` `0x00446bb0` (`Server.exe 0x00411800`; the same code as
/// `cw_sim::modes`'s private `shoot_origin`): the entity position, or for appearance flag 4 the
/// mouth `Rz(yaw) · (width_y · 0.5, 0, height · 0.35)` with the original's full products.
fn shoot_origin(e: &EntityData) -> [i64; 3] {
    let pos = entity_pos(e);
    if e.0[0x6e] & 4 == 0 {
        return pos;
    }
    let m: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let a = f32_at(e, 0x20) * 0.017_453_292_f32;
    let c = cw_math::cos(f64::from(a)) as f32;
    let s = cw_math::sin(f64::from(a)) as f32;
    let a0 = m[4] * s + m[0] * c;
    let b0 = m[4] * c - m[0] * s;
    let b1 = m[5] * c - m[1] * s;
    let a1 = m[5] * s + m[1] * c;
    let a2 = m[6] * s + m[2] * c;
    let b2 = m[6] * c - m[2] * s;
    let a3 = m[7] * s + m[3] * c;
    let h = f32_at(e, 0x74) * 0.5f32;
    let z = f32_at(e, 0x78) * 0.35f32;
    let b3 = m[7] * c - m[3] * s;
    let w = ((h * b3 + a3 * 0.0f32) + m[11] * z) + m[15];
    let x = ((h * b0 + a0 * 0.0f32) + m[8] * z) + m[12];
    let y = ((h * b1 + a1 * 0.0f32) + m[9] * z) + m[13];
    let inv = 1.0f32 / w;
    let zz = ((h * b2 + a2 * 0.0f32) + m[10] * z) + m[14];
    add3(pos, fixed_from_float([inv * x, inv * y, inv * zz]))
}

// ---------------------------------------------------------------------------------------
// The debug paths
// ---------------------------------------------------------------------------------------

/// A block cube of the path debug: `T(bx + 0.5, by + 0.5, bz) · S(s) · T(-0.5, -0.5, 0)` in
/// render space (x and y through `fromBlock + ftol(0.5·65536) + offset`, z the block as a
/// float).
fn block_cube(ctx: &SceneCtx, b: [i32; 3], s: f32, color: [f32; 4], origin: u32) -> ModelDraw {
    let off = ctx.gc.camera.render_offset;
    let h = ftol(0.5f32 * 65536.0);
    let x = ((i64::from(b[0]) * 65536).wrapping_add(h).wrapping_add(off[0]) as f32) * INV_FIXED;
    let y = ((i64::from(b[1]) * 65536).wrapping_add(h).wrapping_add(off[1]) as f32) * INV_FIXED;
    let mut m = identity();
    pre_translate(&mut m, [x, y, b[2] as f32]);
    pre_scale(&mut m, [s, s, s]);
    pre_translate(&mut m, [(PARTICLE_CUBE_SIZE[0] as f32) * -0.5, (PARTICLE_CUBE_SIZE[1] as f32) * -0.5, 0.0]);
    draw(origin, ctx.models.particle_cube(), m, color)
}

/// `floorDivFix 0x0042f100`: `v / 65536`, minus one for every negative `v` (even an exact
/// multiple, as compiled).
fn floor_div_fix(v: i64) -> i32 {
    if v < 0 { ((v / 65536) as i32).wrapping_sub(1) } else { (v / 65536) as i32 }
}

/// `0x004b76c4..0x004b7db6` (`GC+0x1001004`): per creature of the list, a light blue cube (scale
/// 0.2) on every node the path finder reached (`creature+0x140c`), a red one (0.25) on every
/// waypoint (`+0x1460`), and with waypoints a yellow one on the goal (`+0x1440`) and a green one
/// on the start (`+0x1428`).
pub fn path_debug(ctx: &SceneCtx, order: &[i64], out: &mut Vec<ModelDraw>) {
    for id in order {
        let Some(st) = ctx.states.get(id) else { continue };
        let path = &st.path;
        for k in path.nodes.keys() {
            out.push(block_cube(ctx, *k, 0.2, [0.2, 0.7, 1.0, 1.0], 0x004b_7926));
        }
        for k in &path.waypoints {
            out.push(block_cube(ctx, *k, 0.25, [1.0, 0.1, 0.15, 1.0], 0x004b_838f));
        }
        if !path.waypoints.is_empty() {
            let b = |p: [i64; 3]| [floor_div_fix(p[0]), floor_div_fix(p[1]), floor_div_fix(p[2])];
            out.push(block_cube(ctx, b(path.goal), 0.25, [1.0, 1.0, 0.15, 1.0], 0x004b_7b9e));
            out.push(block_cube(ctx, b(path.start), 0.25, [0.15, 1.0, 0.15, 1.0], 0x004b_7d6e));
        }
    }
}

// ---------------------------------------------------------------------------------------
// Projectiles
// ---------------------------------------------------------------------------------------

/// `0x004b8138..0x004b8bb9`, after the particles: every projectile of `GC+0x2f8` whose sphere
/// (margin `+0x4c`) is within the props' far distance:
///
/// - kind 0: model 0x348, and with `+0x54 != 0` a blue beam one block ahead along the velocity
///   (`0x004b8546`);
/// - kind 1 (fireball): a red beam (blue for an owner of class 1) along the velocity, no model
///   (`0x004b8b77`);
/// - kind 2: the owner's slot-7 item model when it is a weapon of sub type 8 (a boomerang),
///   else 0x348, tilted up to 90° about y and spun about x with the age;
/// - kind 4: model 0x835, turned about `velocity × z` by half the age.
///
/// Models: `T(pos) · orientation · S(scale / 14) · T(-size / 2)`, material `(1, 1, 1, light) ×`
/// the world material with `light` the ground light at the projectile / 255. Kind 0 (and 2's
/// fallback path is not affected) turns the model's y axis onto the velocity (`0x004c12f0`).
/// Call after `ParticleSystem::draw` (the original draws the particles in between).
pub fn projectile_pass(ctx: &SceneCtx, particles: &mut ParticleSystem, rng: &mut MsvcRand, out: &mut ScenePieces) {
    let gc = ctx.gc;
    for p in ctx.projectiles {
        // 0x004b81b5.
        if !sphere_visible(&ctx.planes, &gc.camera, p.pos, p.radius, ctx.prop_far) {
            continue;
        }
        let model = match p.kind {
            // 0x004b83dd.
            0 => {
                let model = ctx.models.model(0x348);
                if p.knockback != 0.0 {
                    let k = p.knockback;
                    let n = normalized(p.vel);
                    let args = BeamArgs {
                        start: add3(p.pos, fixed_from_float(n)),
                        dir: n,
                        angle: 0.0,
                        colors: [[0.5, 0.5, 1.0, 1.0], [0.75, 0.75, 1.0, 1.0], [0.9, 0.9, 1.0, 1.0]],
                        time: p.age,
                        p11: k * 0.5,
                        size: k,
                        p13: 1.0,
                        segments: 0x14,
                        sparks: false,
                    };
                    // 0x004b8546.
                    out.creature_extras.extend(draw_beam(ctx, &args, particles, rng));
                }
                model
            }
            // 0x004b88fc.
            1 => {
                let n = normalized(p.vel);
                let v = [n[0] * p.scale, n[1] * p.scale, n[2] * p.scale];
                let dir = [v[0] * 6.0, v[1] * 6.0, v[2] * 6.0];
                let owner_class = ctx.entities.get(&p.owner_id).map(|o| o.0[0x131]);
                let colors = if owner_class == Some(1) {
                    [[0.0, 0.0, 1.0, 1.0], [0.0, 0.25, 1.0, 1.0], [0.25, 0.5, 1.0, 1.0]]
                } else {
                    [[1.0, 0.0, 0.0, 1.0], [1.0, 0.25, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0]]
                };
                let args = BeamArgs {
                    start: sub3(p.pos, fixed_from_float([dir[0] * 0.5, dir[1] * 0.5, dir[2] * 0.5])),
                    dir,
                    angle: 0.0,
                    colors,
                    time: p.age,
                    p11: 0.1,
                    size: p.scale * 5.0,
                    p13: 0.75,
                    segments: 0x14,
                    sparks: false,
                };
                // 0x004b8b77.
                out.creature_extras.extend(draw_beam(ctx, &args, particles, rng));
                continue;
            }
            // 0x004b86eb.
            2 => match ctx.entities.get(&p.owner_id) {
                Some(o) if equip(o, 7)[0] == 3 && equip(o, 7)[1] == 8 => ctx.models.item_model(&Item::from_bytes(equip(o, 7))),
                _ => ctx.models.model(0x348),
            },
            // 0x004b8763.
            4 => ctx.models.model(0x835),
            _ => continue,
        };
        if let Some(model) = model {
            out.creature_extras.push(projectile_draw(ctx, p, model));
        }
    }
}

/// `0x004b855d..0x004b88f2`: the model of one projectile.
pub fn projectile_draw(ctx: &SceneCtx, p: &cw_sim::projectile::Projectile, model: ModelInfo) -> ModelDraw {
    let off = ctx.gc.camera.render_offset;
    // 0x004b8569: kind 1 is full bright (never reached: kind 1 has no model).
    let light = if p.kind == 1 {
        1.0
    } else {
        let b = cw_render::light::get_block(ctx.world, (p.pos[0] / 65536) as i32, (p.pos[1] / 65536) as i32, (p.pos[2] / 65536) as i32);
        f32::from(cw_render::light::prop_light(b)) / 255.0
    };
    let wm = ctx.world_material;
    let material = [1.0 * wm[0], 1.0 * wm[1], 1.0 * wm[2], light * wm[3]];
    let mut m = identity();
    pre_translate(&mut m, float_from_fixed(add3(p.pos, [off[0], off[1], 0])));
    match p.kind {
        // 0x004b868e.
        2 => {
            let mut a = (p.age as f32) * 0.5;
            if a > 90.0 {
                a = 90.0;
            }
            pre_rotate_y(&mut m, a);
            pre_rotate_x(&mut m, (p.age.wrapping_neg() as f32) * 1.5);
        }
        // 0x004b8781.
        4 => {
            let axis = cross(p.vel, [0.0, 0.0, 1.0]);
            pre_rotate_axis(&mut m, (p.age as f32) * 0.5, axis);
        }
        // 0x004b87dd.
        _ => {
            if length_sq(p.vel) > 0.0 {
                rotate_from_to(&mut m, [0.0, 1.0, 0.0], normalized(p.vel));
            }
        }
    }
    let s = p.scale / 14.0;
    pre_scale(&mut m, [s, s, s]);
    pre_translate(&mut m, half_neg(model.size));
    draw(0x004b_88f2, model.handle, m, material)
}

// ---------------------------------------------------------------------------------------
// The cube whirls 0x004bc760, 0x004bbd80, 0x004c04c0
// ---------------------------------------------------------------------------------------

/// The frame the three whirls share for one cube at phase `t` (0..1): the product of three
/// rotations by `40t`, `30t`, `10t` degrees (rows 0..2, the inlined identity products written
/// as Ghidra lists them, zero terms included), rows 0..2 scaled by `scale` unless it is exactly
/// 1, `translation` in row 3, then `T(-cube size / 2)`.
fn whirl_matrix(t: f32, scale: f32, translation: [f32; 3]) -> D3dMatrix {
    let a1 = t * 40.0 * DEG;
    let c1 = cosf(a1);
    let s1 = sinf(a1);
    let f10 = c1 * 0.0;
    let f12 = s1 * 0.0;
    let f19 = f10 + f12;
    let f20 = f10 - f12;
    let a2 = t * 30.0 * DEG;
    let c2 = cosf(a2);
    let s2 = sinf(a2);
    let f14 = c2 - f20 * s2;
    let f24 = c2 * 0.0;
    let l84 = f20 * c2 + s2;
    let f15 = f24 - (f10 - s1) * s2;
    let l9c0 = s2 * 0.0;
    let l94 = (f10 - s1) * c2 + l9c0;
    let f22 = f24 - (c1 - f12) * s2;
    let f24b = f24 - f20 * s2;
    let f25 = (c1 - f12) * c2 + l9c0;
    let l9c = f20 * c2 + l9c0;
    let a3 = t * 10.0 * DEG;
    let c3 = cosf(a3);
    let s3 = sinf(a3);
    let l68 = f19 * s3 + f14 * c3;
    let l58 = f19 * c3 - f14 * s3;
    let l64 = (c1 + f12) * s3 + f15 * c3;
    let l54 = (c1 + f12) * c3 - f15 * s3;
    let l60 = (s1 + f10) * s3 + f22 * c3;
    let l50 = (s1 + f10) * c3 - f22 * s3;
    let l5c = f19 * s3 + f24b * c3;
    let l4c = f19 * c3 - f24b * s3;
    let mut m: D3dMatrix = [
        [l68, l64, l60, l5c],
        [l58, l54, l50, l4c],
        [l84, l94, f25, l9c],
        [translation[0], translation[1], translation[2], 1.0],
    ];
    // `ucomiss scale, 1; jnp`: skipped only for exactly 1 (NaN scales).
    if scale != 1.0 {
        for row in m.iter_mut().take(3) {
            for v in row.iter_mut() {
                *v *= scale;
            }
        }
    }
    pre_translate(&mut m, half_neg(PARTICLE_CUBE_SIZE));
    m
}

/// The shared loop state of the whirls: per cube `i`, the phase `t = (time_i % 2000) / 2000`
/// with `time_i = time + 443 i`, the size wobble `cos(13 i + d1)` and the colour lerp.
struct Whirl {
    d1: f64,
    b: f32,
}

impl Whirl {
    /// `((time0 % 500) / 500 · 2) · π` (double) and `(time0 % 10000) · 0.0001`, computed once.
    fn new(t500: i32, time0: i32) -> Whirl {
        let d1 = (((t500 % 500) as f32 / 500.0) * 2.0) as f64 * PI_D;
        let b = ((time0 % 10_000) as f32) * 0.0001;
        Whirl { d1, b }
    }

    /// The cube's scale `(cos(13i + d1) · 0.5 + 1) · size · (1 - u²)`, `u = (t - 0.5) · 2`.
    fn scale(&self, i: i32, t: f32, size: f32) -> f32 {
        let u = (t - 0.5) * 2.0;
        let arg = ((i.wrapping_mul(13) as f64) + self.d1) as f32;
        let c = cosf(arg);
        (c * 0.5 + 1.0) * size * (1.0 - u * u)
    }

    /// `((i / 10 + b) · 2) · π` (double), the base angle around the centre.
    fn angle(&self, i: i32) -> f64 {
        ((((i as f32) / 10.0 + self.b) * 2.0) as f64) * PI_D
    }
}

fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let k = 1.0 - t;
    [a[0] * k + b[0] * t, a[1] * k + b[1] * t, a[2] * k + b[2] * t, a[3] * k + b[3] * t]
}

/// The render-space position of the whirl's centre (`(pos + render offset) / 65536`, z
/// `pos / 65536`, each through the x87 `fild`/`fstp` then `× 1/65536`).
fn whirl_centre(ctx: &SceneCtx, pos: [i64; 3]) -> [f32; 3] {
    render_translation(pos, ctx.gc.camera.render_offset)
}

/// `0x004bc760(pos, colour a, colour b, time, view, projection, radius, size, twist, count)`:
/// `count` particle cubes spiralling out of `pos`: cube `i` at phase `t` sits `t · radius` out
/// at angle `t² · 4π · twist + (i/10 + b) · 2π` and `2 t · radius` up, coloured `a → b`.
#[allow(clippy::too_many_arguments)]
pub fn whirl_4bc760(ctx: &SceneCtx, pos: [i64; 3], a: [f32; 4], b: [f32; 4], time: i32, radius: f32, size: f32, twist: f32, count: i32) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    let w = Whirl::new(time, time);
    let c = whirl_centre(ctx, pos);
    let mut tm = time;
    for i in 0..count.max(0) {
        let t = ((tm % 2000) as f32) / 2000.0;
        let sc = w.scale(i, t, size);
        let phi = ((((t * t) * 4.0) as f64 * PI_D) * f64::from(twist) + w.angle(i)) as f32;
        let color = lerp_color(a, b, t);
        let r = t * radius;
        let tr = [c[0] + cosf(phi) * r, c[1] + sinf(phi) * r, c[2] + r * 2.0];
        out.push(draw(0x004b_d114, ctx.models.particle_cube(), whirl_matrix(t, sc, tr), color));
        tm = tm.wrapping_add(0x1bb);
    }
    out
}

/// `0x004bbd80(pos, colour a, colour b, time, view, projection, radius, size, count)`: `count`
/// cubes on a circle of `radius` around `pos` rising `2 t · radius`, coloured `a → b`.
#[allow(clippy::too_many_arguments)]
pub fn whirl_4bbd80(ctx: &SceneCtx, pos: [i64; 3], a: [f32; 4], b: [f32; 4], time: i32, radius: f32, size: f32, count: i32) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    let w = Whirl::new(time, time);
    let c = whirl_centre(ctx, pos);
    let mut tm = time;
    for i in 0..count.max(0) {
        let t = ((tm % 2000) as f32) / 2000.0;
        let sc = w.scale(i, t, size);
        let phi = w.angle(i) as f32;
        let color = lerp_color(a, b, t);
        let tr = [c[0] + cosf(phi) * radius, c[1] + sinf(phi) * radius, c[2] + (t * radius) * 2.0];
        out.push(draw(0x004b_c6fe, ctx.models.particle_cube(), whirl_matrix(t, sc, tr), color));
        tm = tm.wrapping_add(0x1bb);
    }
    out
}

/// `0x004c04c0(pos, colour a, colour b, time, view, projection, radius, size, count)`: `count`
/// cubes on a sphere of `radius` around `pos`: latitude `t · π` (height `cos(tπ) · radius`,
/// ring `sin(tπ) · radius`), coloured `a → b`; its size wobble runs on `time / 8`.
#[allow(clippy::too_many_arguments)]
pub fn whirl_4c04c0(ctx: &SceneCtx, pos: [i64; 3], a: [f32; 4], b: [f32; 4], time: i32, radius: f32, size: f32, count: i32) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    let w = Whirl::new(time / 8, time);
    let c = whirl_centre(ctx, pos);
    let mut tm = time;
    for i in 0..count.max(0) {
        let t = ((tm % 2000) as f32) / 2000.0;
        let tpi = ((t as f64) * PI_D) as f32;
        let up = cosf(tpi) * radius;
        let ring = sinf(tpi) * radius;
        let sc = w.scale(i, t, size);
        let phi = w.angle(i) as f32;
        let color = lerp_color(a, b, t);
        let tr = [c[0] + cosf(phi) * ring, c[1] + sinf(phi) * ring, c[2] + up];
        out.push(draw(0x004c_0e7c, ctx.models.particle_cube(), whirl_matrix(t, sc, tr), color));
        tm = tm.wrapping_add(0x1bb);
    }
    out
}

// ---------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------

/// A model draw with alpha 1, culled (the pass does not change the alpha or the cull mode
/// around these draws).
fn draw(origin: u32, model: cw_render::frame::ModelRef, world: D3dMatrix, material: [f32; 4]) -> ModelDraw {
    ModelDraw { origin, model, world, material, alpha: 1.0, double_sided: false, shininess: 0.0, set_white: None, mirrored: false }
}

fn vec3f_at(e: &EntityData, o: usize) -> [f32; 3] {
    [f32_at(e, o), f32_at(e, o + 4), f32_at(e, o + 8)]
}

/// `vec3f::normalized 0x00427870`: `v · (1 / sqrt((x² + y²) + z²))`.
fn normalized(v: [f32; 3]) -> [f32; 3] {
    let inv = 1.0 / (f64::from(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() as f32);
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

/// `vec3fLength 0x00423f20`.
fn length(v: [f32; 3]) -> f32 {
    f64::from(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() as f32
}

/// `0x00412390`: `a × b` in the original's term order.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], b[0] * a[2] - a[0] * b[2], a[0] * b[1] - b[0] * a[1]]
}

/// `0x004244f0`: `M = Ry(degrees) · M` (rows 0 and 2).
fn pre_rotate_y(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let r0 = m[0][j];
        let r2 = m[2][j];
        m[0][j] = r0 * c - r2 * s;
        m[2][j] = r2 * c + r0 * s;
    }
}

/// `0x004243d0`: `M = Rx(degrees) · M` (rows 1 and 2).
fn pre_rotate_x(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let r1 = m[1][j];
        let r2 = m[2][j];
        m[1][j] = r2 * s + r1 * c;
        m[2][j] = r2 * c - r1 * s;
    }
}

/// `0x004c12f0(from, to)`: rotate `M` so that `from` turns onto `to`: about `to × from`-ordered
/// cross product (`(to.z·from.y - to.y·from.z, to.x·from.z - to.z·from.x, to.y·from.x -
/// to.x·from.y)`) by `acos(to·from / (|to|·|from|))` degrees (`0x004241b0`); nothing when either
/// length or the cross product's squared length is below 0.0001.
pub fn rotate_from_to(m: &mut D3dMatrix, from: [f32; 3], to: [f32; 3]) {
    let la = f64::from(from[0] * from[0] + from[1] * from[1] + from[2] * from[2]).sqrt() as f32;
    // `comisd 0.0001, |la|; ja skip` (NaN goes on).
    if 0.0001f64 > f64::from(la.abs()) {
        return;
    }
    let lb = f64::from(to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt() as f32;
    if 0.0001f64 > f64::from(lb.abs()) {
        return;
    }
    let cx = to[2] * from[1] - to[1] * from[2];
    let cy = to[0] * from[2] - to[2] * from[0];
    let cz = to[1] * from[0] - to[0] * from[1];
    let c2 = (cy * cy + cx * cx) + cz * cz;
    if 0.0001f64 > f64::from(c2.abs()) {
        return;
    }
    let dot = (to[0] * from[0] + to[1] * from[1]) + to[2] * from[2];
    let cosang = dot / (lb * la);
    let ang = (cw_math::acos(f64::from(cosang)) as f32) * 57.295_78;
    pre_rotate_axis(m, ang, [cx, cy, cz]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneModels, SceneState};
    use cw_render::frame::{IDENTITY, ModelRef};
    use cw_render::mesh::VoxelGrid;

    const B: i64 = 65536;
    const ALL_VISIBLE: [[f32; 4]; 6] = [[0.0, 0.0, 0.0, 1.0]; 6];

    /// Models 1..=9 are solid cubes of edge `index` (so each explosion has its own particle
    /// count); 0x348 is 2x4x6; the particle cube is handle 100.
    struct Models {
        voxels: Vec<Vec<[u8; 3]>>,
    }

    impl Models {
        fn new() -> Models {
            let voxels = (0..=9).map(|n: usize| vec![[200u8, 100, 50]; n * n * n]).collect();
            Models { voxels }
        }
    }

    impl SceneModels for Models {
        fn model(&self, index: i32) -> Option<ModelInfo> {
            match index {
                0x348 => Some(ModelInfo { handle: 0x348, size: [2, 4, 6] }),
                0xa03 => Some(ModelInfo { handle: 0xa03, size: [10, 20, 4] }),
                _ => None,
            }
        }
        fn prop_model(&self, _: u32) -> Option<ModelInfo> {
            None
        }
        fn prop_model_count(&self) -> usize {
            0
        }
        fn item_model(&self, _: &Item) -> Option<ModelInfo> {
            None
        }
        fn voxels(&self, model: ModelRef) -> Option<VoxelGrid<'_>> {
            let n = model as usize;
            if (1..=9).contains(&n) {
                Some(VoxelGrid { size: [n as i32; 3], voxels: &self.voxels[n] })
            } else {
                None
            }
        }
        fn particle_cube(&self) -> ModelRef {
            100
        }
    }

    fn put(e: &mut EntityData, o: usize, b: &[u8]) {
        e.0[o..o + b.len()].copy_from_slice(b);
    }

    fn creature(pos: [i64; 3], hp: f32) -> EntityData {
        let mut e = EntityData::ZERO;
        put(&mut e, 0, &(pos[0] * B).to_le_bytes());
        put(&mut e, 8, &(pos[1] * B).to_le_bytes());
        put(&mut e, 0x10, &(pos[2] * B).to_le_bytes());
        put(&mut e, 0x70, &1.0f32.to_le_bytes());
        put(&mut e, 0x78, &2.0f32.to_le_bytes());
        put(&mut e, 0x15c, &hp.to_le_bytes());
        e
    }

    fn ctx<'a>(
        world: &'a cw_world::World,
        entities: &'a BTreeMap<i64, EntityData>,
        states: &'a BTreeMap<i64, CreatureState>,
        gc: &'a SceneState,
        models: &'a Models,
        projectiles: &'a [cw_sim::projectile::Projectile],
    ) -> SceneCtx<'a> {
        SceneCtx {
            world,
            entities,
            states,
            gc,
            models,
            projectiles,
            planes: ALL_VISIBLE,
            daylight: 1.0,
            underwater: false,
            world_material: [1.0, 1.0, 1.0, 1.0],
            draw_distance: 122.4,
            prop_far: 20.0,
            prop_near: 18.0,
            static_far: 36.72,
            static_near: 33.048,
            particle_range: 24.48,
            dt_ms: 16,
        }
    }

    /// A pose with a distinct model per slot: head 1, hair 2, body 3, arm 4, hands 5, feet 6,
    /// right weapon 7; each matrix a translation to tell them apart.
    fn parts() -> PoseParts {
        let mi = |n: u32| Some(ModelInfo { handle: n, size: [n as i32; 3] });
        let t = |x: f32| {
            let mut m = IDENTITY;
            m[3] = [x, 0.0, 0.0, 1.0];
            m
        };
        PoseParts {
            head: mi(1),
            hair: mi(2),
            body: mi(3),
            arm: mi(4),
            hands: mi(5),
            feet: mi(6),
            weapon_r: mi(7),
            head_m: t(1.0),
            body_m: t(2.0),
            arm_l_m: t(3.0),
            arm_r_m: t(4.0),
            weapon_r_m: t(5.0),
            hand_l_m: t(6.0),
            hand_r_m: t(7.0),
            foot_l_m: t(8.0),
            foot_r_m: t(9.0),
            ..PoseParts::default()
        }
    }

    fn run_explosion(flags: u16, ty: i32) -> (ParticleSystem, MsvcRand, PassEffects) {
        let world = cw_world::World::new(1);
        let mut entities = BTreeMap::new();
        let mut e = creature([10, 10, 10], 0.0);
        put(&mut e, 0x6e, &flags.to_le_bytes());
        put(&mut e, 0x54, &ty.to_le_bytes());
        e.0[0x6a] = 255;
        entities.insert(5, e);
        let mut states = BTreeMap::new();
        let mut st = CreatureState::default();
        st.riding.render_pos = [10 * B, 10 * B, 10 * B];
        states.insert(5, st);
        let gc = SceneState::default();
        let models = Models::new();
        let c = ctx(&world, &entities, &states, &gc, &models, &[]);
        let mut pm = BTreeMap::new();
        pm.insert(5, parts());
        let mut pending = BTreeSet::new();
        pending.insert(5);
        let none = |_: i32, _: i32| None;
        let inp = PassInputs { parts: &pm, explode_pending: &pending, real_time_ms: 0, zone_discovered: &none, show_paths: false, airships: &[] };
        let mut particles = ParticleSystem::new();
        let mut rng = MsvcRand::new(77);
        let mut out = ScenePieces::default();
        let mut fx = PassEffects::default();
        creature_pass_with(&c, &inp, &mut particles, &mut rng, &mut out, &mut fx);
        assert_eq!(out.creature_order, vec![5]);
        assert!(out.creature_extras.is_empty());
        (particles, rng, fx)
    }

    /// The expected result: the explosions in the original's order on a fresh stream.
    fn expected(models: &[(u32, D3dMatrix, [f32; 4])]) -> (ParticleSystem, MsvcRand) {
        let m = Models::new();
        let mut particles = ParticleSystem::new();
        let mut rng = MsvcRand::new(77);
        for (n, mat, col) in models {
            particles.explode_model(m.voxels(*n).unwrap(), mat, *col, 0, [0, 0], &mut rng);
        }
        (particles, rng)
    }

    /// 0x004b39ee / 0x004b4e5b: a creature with `hp <= 0` gets no body (the dead branch jumps
    /// to the loop end 0x004b7685); NaN HP counts as alive; a white-cleared frame keeps only
    /// the local player's body.
    #[test]
    fn body_is_drawn_only_for_living_creatures() {
        let alive = creature([0, 0, 0], 10.0);
        let dead = creature([0, 0, 0], 0.0);
        let below = creature([0, 0, 0], -5.0);
        let nan = creature([0, 0, 0], f32::NAN);
        assert!(body_drawn(&alive, 1, 1, false));
        assert!(!body_drawn(&dead, 1, 1, false));
        assert!(!body_drawn(&below, 2, 1, false));
        assert!(body_drawn(&nan, 2, 1, false));
        assert!(body_drawn(&alive, 1, 1, true));
        assert!(!body_drawn(&alive, 2, 1, true));
    }

    /// One frame of the pass over `entities` (camera at the origin, everything in the frustum),
    /// its effects applied to `pending` as `gather_scene` does.
    fn frame(entities: &BTreeMap<i64, EntityData>, gc: &SceneState, pending: &mut BTreeSet<i64>, particles: &mut ParticleSystem, rng: &mut MsvcRand) -> PassEffects {
        let world = cw_world::World::new(1);
        let states = BTreeMap::new();
        let models = Models::new();
        let c = ctx(&world, entities, &states, gc, &models, &[]);
        let pm: BTreeMap<i64, PoseParts> = entities.keys().map(|&id| (id, parts())).collect();
        let none = |_: i32, _: i32| None;
        let snapshot = pending.clone();
        let inp = PassInputs { parts: &pm, explode_pending: &snapshot, real_time_ms: 0, zone_discovered: &none, show_paths: false, airships: &[] };
        let mut out = ScenePieces::default();
        let mut fx = PassEffects::default();
        creature_pass_with(&c, &inp, particles, rng, &mut out, &mut fx);
        fx.apply_flags(pending);
        fx
    }

    /// 0x004b4e70..0x004b5ac6 and 0x004b5d81..0x004b5db2: the body pose `0x004128f0` gets the
    /// body material `esp+0x7f4` as its colour, whose alpha is the block light byte 0 at
    /// `(x, y, z - 0.1)` / 255 (`0x004718b0`) times the world material's alpha (the daylight,
    /// `0x00412120`). A creature standing in shade (light 51) gets alpha 0.2 · daylight.
    #[test]
    fn a_posed_creature_carries_its_body_material_with_the_block_light() {
        const ZX: i32 = 0x8000;
        let mut world = cw_world::World::new(1);
        let mut z = cw_world::zone::Zone::new(ZX, ZX);
        for c in z.columns.iter_mut() {
            c.height = 10;
            // z = 10: air with light byte 0 = 51; z = 11: air with light 255.
            c.blocks = vec![[51, 0, 0, 0], [255, 0, 0, 0]];
        }
        world.insert_zone(Box::new(z));
        let base = i64::from(ZX) * 256;
        let mut entities = BTreeMap::new();
        let mut e = creature([0, 0, 0], 10.0);
        put(&mut e, 0, &((base + 5) * B).to_le_bytes());
        put(&mut e, 8, &((base + 5) * B).to_le_bytes());
        // z = 10.2: 0.1 below is block 10.
        put(&mut e, 0x10, &(10 * B + B / 5).to_le_bytes());
        entities.insert(1, e);
        let states = BTreeMap::new();
        let mut gc = SceneState::default();
        gc.camera.position = [(base + 5) * B, (base + 5) * B, 10 * B];
        let models = Models::new();
        let mut c = ctx(&world, &entities, &states, &gc, &models, &[]);
        c.world_material = [1.0, 1.0, 1.0, 0.5];
        let pm: BTreeMap<i64, PoseParts> = BTreeMap::new();
        let none = |_: i32, _: i32| None;
        let pending = BTreeSet::new();
        let inp = PassInputs { parts: &pm, explode_pending: &pending, real_time_ms: 0, zone_discovered: &none, show_paths: false, airships: &[] };
        let mut out = ScenePieces::default();
        let mut fx = PassEffects::default();
        let (mut particles, mut rng) = (ParticleSystem::new(), MsvcRand::new(1));
        creature_pass_with(&c, &inp, &mut particles, &mut rng, &mut out, &mut fx);
        assert_eq!(fx.posed, vec![1]);
        let light = 51.0f32 / 255.0;
        assert_eq!(fx.body_materials.get(&1), Some(&[1.0, 1.0, 1.0, 0.5 * light]));
    }

    /// 0x0041293b: the pose draw `0x004128f0` sets `pose+0x878` (`creature+0x1d10`) on every
    /// call; the creature loop reaches it (0x004b5cf3/0x004b5db2, or the ghost deferral drawn at
    /// 0x004ba8fe/0x004ba9a5) only for a living creature that passed the sphere test 0x004b4e1b
    /// and the white-clear test 0x004b4e5b.
    #[test]
    fn only_a_drawn_living_creature_is_posed() {
        let mut entities = BTreeMap::new();
        entities.insert(1, creature([10, 10, 10], 10.0));
        entities.insert(2, creature([1000, 0, 0], 10.0));
        entities.insert(3, creature([5, 5, 5], 0.0));
        let gc = SceneState::default();
        let mut pending = BTreeSet::new();
        let (mut particles, mut rng) = (ParticleSystem::new(), MsvcRand::new(1));
        let fx = frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        assert_eq!(fx.posed, vec![1]);
        assert_eq!(pending, BTreeSet::from([1]));
        // 0x004b4e5b: a white-cleared frame poses only the local player.
        let gc = SceneState { white_clear: true, player: 3, ..SceneState::default() };
        let mut entities = BTreeMap::new();
        entities.insert(1, creature([10, 10, 10], 10.0));
        entities.insert(3, creature([5, 5, 5], 10.0));
        let fx = frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        assert_eq!(fx.posed, vec![3]);
        assert_eq!(pending, BTreeSet::from([3]));
    }

    /// The flag set by last frame's body draw is what 0x004b3a31 reads: a creature bursts on
    /// the first frame it is dead (0x004b3a31..0x004b4098) and the clear 0x004b411d keeps it
    /// from bursting again.
    #[test]
    fn a_creature_explodes_once_on_the_frame_after_its_last_body_draw() {
        let gc = SceneState::default();
        let mut entities = BTreeMap::new();
        entities.insert(5, creature([10, 10, 10], 10.0));
        let mut pending = BTreeSet::new();
        let (mut particles, mut rng) = (ParticleSystem::new(), MsvcRand::new(1));
        frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        assert!(particles.particles.is_empty());
        put(entities.get_mut(&5).unwrap(), 0x15c, &0.0f32.to_le_bytes());
        frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        let n = particles.particles.len();
        assert!(n > 0);
        assert!(pending.is_empty());
        frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        assert_eq!(particles.particles.len(), n);
    }

    /// A creature culled by 0x004b4e1b on its last living frame was never posed, so it has no
    /// flag and does not burst when it comes into view dead.
    #[test]
    fn a_creature_that_died_unseen_does_not_explode() {
        let gc = SceneState::default();
        let mut entities = BTreeMap::new();
        entities.insert(5, creature([1000, 0, 0], 10.0));
        let mut pending = BTreeSet::new();
        let (mut particles, mut rng) = (ParticleSystem::new(), MsvcRand::new(1));
        frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        entities.insert(5, creature([10, 10, 10], 0.0));
        frame(&entities, &gc, &mut pending, &mut particles, &mut rng);
        assert!(particles.particles.is_empty());
    }

    /// 0x00412960..0x00412bc6 set the part model pointers `pose+0xbc..+0xe8` on every call;
    /// the part matrices (`pose+0x1ac` for the head, 0x00420440..0x004206b7) are rebuilt in
    /// place only when their part is drawn and otherwise keep their last value.
    #[test]
    fn pose_parts_record_the_drawn_parts() {
        use cw_render::pose::{PartSlot, PartTransform, Pose};
        let info = |n: u32| Some(ModelInfo { handle: n, size: [n as i32; 3] });
        let t = |x: f32| glam::Mat4::from_translation(glam::Vec3::new(x, 0.0, 0.0));
        let part = |slot, model, x| PartTransform { slot, model, matrix: t(x), color: [1.0; 4], shine: 0.0, mirrored: false };
        let pose = |parts| Pose { parts, root: glam::Mat4::IDENTITY, trails: [[[0.0; 3]; 16]; 4], walk_blend: 0.0 };
        let mut p = PoseParts::default();
        p.record_pose(
            &pose(vec![
                part(PartSlot::Head, 1, 1.0),
                part(PartSlot::Hair, 2, 2.0),
                part(PartSlot::Body, 3, 3.0),
                part(PartSlot::UpperArmLeft, 4, 4.0),
                part(PartSlot::UpperArmRight, 4, 5.0),
                part(PartSlot::WeaponRight, 7, 6.0),
                part(PartSlot::HandLeft, 5, 7.0),
                part(PartSlot::HandRight, 5, 8.0),
                part(PartSlot::FootLeft, 6, 9.0),
                part(PartSlot::FootRight, 6, 10.0),
            ]),
            info,
        );
        let mi = |n: u32| Some(ModelInfo { handle: n, size: [n as i32; 3] });
        assert_eq!((p.head, p.hair, p.body, p.arm, p.hands, p.feet, p.weapon_r), (mi(1), mi(2), mi(3), mi(4), mi(5), mi(6), mi(7)));
        let x = |m: &D3dMatrix| m[3][0];
        assert_eq!(
            [x(&p.head_m), x(&p.body_m), x(&p.arm_l_m), x(&p.arm_r_m), x(&p.weapon_r_m), x(&p.hand_l_m), x(&p.hand_r_m), x(&p.foot_l_m), x(&p.foot_r_m)],
            [1.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]
        );
        // Next frame: no weapon and no hair; the weapon matrix stays.
        p.record_pose(&pose(vec![part(PartSlot::Head, 1, 11.0)]), info);
        assert_eq!((p.head, p.hair, p.weapon_r), (mi(1), None, None));
        assert_eq!((x(&p.head_m), x(&p.weapon_r_m)), (11.0, 6.0));
    }

    #[test]
    fn dying_creature_explodes_ten_parts_in_order() {
        let p = parts();
        let w = [1.0; 4];
        let hair = [1.0, 0.0, 0.0, 1.0];
        let order = [
            (1, p.head_m, w),
            (2, p.head_m, hair),
            (3, p.body_m, w),
            (5, p.hand_l_m, w),
            (5, p.hand_r_m, w),
            (4, p.arm_l_m, w),
            (4, p.arm_r_m, w),
            (6, p.foot_l_m, w),
            (6, p.foot_r_m, w),
            (7, p.weapon_r_m, w),
        ];
        let (want_p, want_rng) = expected(&order);
        let (got_p, got_rng, fx) = run_explosion(0, 0);
        assert_eq!(got_rng, want_rng);
        assert_eq!(got_p.particles.len(), want_p.particles.len());
        assert_eq!(got_p.particles, want_p.particles);
        assert!(fx.flash_lights.is_empty());
        assert_eq!(fx.explode_cleared, vec![5]);
    }

    #[test]
    fn bald_creature_skips_the_hair_and_type_0x90_flashes() {
        let p = parts();
        let w = [1.0; 4];
        let order = [
            (1, p.head_m, w),
            (3, p.body_m, w),
            (5, p.hand_l_m, w),
            (5, p.hand_r_m, w),
            (4, p.arm_l_m, w),
            (4, p.arm_r_m, w),
            (6, p.foot_l_m, w),
            (6, p.foot_r_m, w),
            (7, p.weapon_r_m, w),
        ];
        let (want_p, want_rng) = expected(&order);
        let (got_p, got_rng, fx) = run_explosion(0x400, 0x90);
        assert_eq!(got_rng, want_rng);
        assert_eq!(got_p.particles, want_p.particles);
        assert_eq!(fx.flash_lights.len(), 1);
        let f = fx.flash_lights[0];
        assert_eq!((f.position, f.color, f.radius, f.duration), ([10 * B, 10 * B, 10 * B], [1.0, 0.8, 0.5], 16.0, 800));
    }

    #[test]
    fn creatures_are_ordered_far_to_near_and_hidden_by_panels() {
        let world = cw_world::World::new(1);
        let mut entities = BTreeMap::new();
        entities.insert(1, creature([1, 0, 0], 10.0));
        entities.insert(2, creature([5, 0, 0], 10.0));
        entities.insert(3, creature([3, 0, 0], 10.0));
        let states = BTreeMap::new();
        let mut gc = SceneState::default();
        let models = Models::new();
        {
            let c = ctx(&world, &entities, &states, &gc, &models, &[]);
            assert_eq!(creature_list(&c), vec![2, 3, 1]);
        }
        gc.gui.w888 = true;
        let c = ctx(&world, &entities, &states, &gc, &models, &[]);
        assert!(creature_list(&c).is_empty());
    }

    #[test]
    fn projectile_along_y_is_translated_scaled_and_centred() {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let mut gc = SceneState::default();
        gc.camera.render_offset = [-8 * B, -8 * B];
        let models = Models::new();
        let p = cw_sim::projectile::Projectile {
            pos: [10 * B, 10 * B, 5 * B],
            vel: [0.0, 3.0, 0.0],
            scale: 14.0,
            radius: 0.5,
            ..Default::default()
        };
        let projectiles = [p];
        let c = ctx(&world, &entities, &states, &gc, &models, &projectiles);
        let mut out = ScenePieces::default();
        projectile_pass(&c, &mut ParticleSystem::new(), &mut MsvcRand::new(1), &mut out);
        assert_eq!(out.creature_extras.len(), 1);
        let d = &out.creature_extras[0];
        assert_eq!(d.model, 0x348);
        // Velocity along +y: no rotation; T(2, 2, 5) · S(1) · T(-1, -2, -3).
        assert_eq!(d.world[3], [1.0, 0.0, 2.0, 1.0]);
        assert_eq!(d.world[0], [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(d.world[1], [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn projectile_along_x_turns_its_y_axis_onto_the_velocity() {
        let mut m = IDENTITY;
        rotate_from_to(&mut m, [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        let y = transform_vector(&m, [0.0, 1.0, 0.0]);
        assert!((y[0] - 1.0).abs() < 1e-6 && y[1].abs() < 1e-6 && y[2].abs() < 1e-6);
    }

    #[test]
    fn airship_transform() {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let gc = SceneState::default();
        let models = Models::new();
        let c = ctx(&world, &entities, &states, &gc, &models, &[]);
        let a = Airship { position: [0; 3], render_pos: [100 * B, 50 * B, 30 * B], yaw: 0.0 };
        let d = airship_draw(&c, &a).unwrap();
        assert_eq!(d.origin, 0x004b_37be);
        assert_eq!(d.world[3], [95.0, 40.0, 28.0, 1.0]);
    }

    #[test]
    fn whirls_draw_count_cubes_coloured_from_a_to_b() {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let gc = SceneState::default();
        let models = Models::new();
        let c = ctx(&world, &entities, &states, &gc, &models, &[]);
        let a = [1.0, 0.0, 0.0, 1.0];
        let b = [0.0, 0.0, 1.0, 1.0];
        let v = whirl_4bbd80(&c, [0; 3], a, b, 0, 1.0, 0.1, 20);
        assert_eq!(v.len(), 20);
        // Cube 0 at time 0: t = 0, colour a, on the circle at angle 0 (b = 0), scale
        // (cos(d1=0)·0.5 + 1)·0.1·(1 - 1) = 0.
        assert_eq!(v[0].material, a);
        assert_eq!(v[0].world[3][0], 1.0 + 0.0 * -0.5);
        assert_eq!(whirl_4bc760(&c, [0; 3], a, b, 1234, 1.0, 0.1, 0.5, 43).len(), 43);
        assert_eq!(whirl_4c04c0(&c, [0; 3], a, b, 1234, 1.0, 0.05, 40).len(), 40);
        // Cube 1 of the 4bbd80 whirl: time 443, t = 0.2215, colour lerp.
        let t = 443.0f32 / 2000.0;
        assert_eq!(v[1].material, lerp_color(a, b, t));
    }

    #[test]
    fn spirit_cubes_follow_the_item() {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let gc = SceneState::default();
        let models = Models::new();
        let c = ctx(&world, &entities, &states, &gc, &models, &[]);
        let mut item = [0u8; 0x118];
        item[0] = 3;
        item[0x114] = 2;
        item[0x14..0x18].copy_from_slice(&[1, 2, 0xff, 0x80]);
        item[0x1c..0x20].copy_from_slice(&[0, 0, 0, 1]);
        let v = spirit_cubes_471b60(&c, &item, &IDENTITY, [1.0; 4], 0.5);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].world[3], [1.0, 2.0, -1.0, 1.0]);
        assert_eq!(v[0].material, material_color(0x80, [1.0; 4], 0.5));
        assert_eq!(v[1].material, material_color(1, [1.0; 4], 0.5));
        item[0] = 0;
        assert!(spirit_cubes_471b60(&c, &item, &IDENTITY, [1.0; 4], 0.5).is_empty());
    }
}
