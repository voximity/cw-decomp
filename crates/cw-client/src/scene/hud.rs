//! HUD models (`Cube.exe 0x004bb080..0x004bbb1a`): the 3D models `GameController::render`
//! draws in screen space over the GUI with `0x00476660`, and the compass of the map screen
//! (`0x004ae21b`).
//!
//! `0x00476660(this = GC, x, y, rotation: *vec3f, scale, model: *Model, depth)` draws nothing
//! for a null model; `cw_render::passes::hud_model_matrices` is its transform (rotation in
//! degrees, model centred on its size, `T(ndc(x, y), depth) · perspective`). The material is
//! the last `setMaterialColor` before the call.
//!
//! The HUD branch (`0x004bb080..0x004bb0ad`) runs when the GUI root `GC+0x800884` is visible
//! and the engine's root widget (`0x00487490(GC+0x800710)` = `Engine+0xb4`) is visible. It
//! then sets `ZENABLE=1`, binds the cube shader, clears the depth buffer (`Clear(0, 0,
//! ZBUFFER, 0, 1.0, 0)`), sets the HUD light (see [`HUD_LIGHT`]) and zeroes the 16 point
//! lights, and draws, in order:
//!
//! 1. `0x004bb3e5`: the local player's head (appearance `0x7c`), white, at `(50, 60)`;
//! 2. `0x004bb4f2`: the player's hair (appearance `0x7e`) in its hair colour, same place;
//! 3. `0x004bb610`: the player's pet's head (or body when it has no head), white, at `(50, 210)`;
//! 4. `0x004bb6cf`: the compass, model `0x9ff`, white, at `(w - 60, 40)`, turned by the
//!    camera yaw;
//! 5. `0x004bb90c` / `0x004bba1e`: head and hair of up to three other players (the three
//!    friend life bars `GC+0x800778`), at `(270 + 220 i, 45)`.
//!
//! The portraits wobble: rotation `(-120 + 20 cos(t/2000 + φ), 0, 20 + 20 sin(t/1000 + φ))`
//! with `t` the engine clock (`Engine+0xe8`) and `φ = 43 i` for the i-th other player.

use cw_net::EntityData;
use cw_render::passes::HudModel;

use super::math::{cosf, sinf};
use super::{ModelInfo, SceneCtx};

/// Appearance offsets in the entity block (`creature+0x10+X`).
mod off {
    /// `+0x50` hostile type (`creature+0x60`): 0 for players.
    pub const HOSTILE: usize = 0x50;
    /// `+0x6a..0x6d` hair colour r, g, b (`creature+0x7a`).
    pub const HAIR_RGB: usize = 0x6a;
    /// `+0x7c` head model (`creature+0x8c`).
    pub const HEAD_MODEL: usize = 0x7c;
    /// `+0x7e` hair model (`creature+0x8e`).
    pub const HAIR_MODEL: usize = 0x7e;
    /// `+0x84` body model (`creature+0x94`).
    pub const BODY_MODEL: usize = 0x84;
}

/// The compass model index (`0x004120c0(GC+0x300, 0x9ff)`; `0x004ae189` reads the same slot
/// inline: null when the vector holds `<= 0x9ff` entries).
pub const COMPASS_MODEL: i32 = 0x9ff;

/// `setLight 0x00448170` of `0x004bb259` (front, back, direction before the setter
/// normalises it, ambient). The HUD pass also zeroes the 16 point lights (`0x004bb32a`).
/// Not carried by [`HudModel`]: see the report.
pub const HUD_LIGHT: HudLight = HudLight {
    front: [1.0, 1.0, 1.0, 1.0],
    back: [0.2, 0.3, 0.4, 1.0],
    direction: [0.0, 0.5, -1.0],
    ambient: [0.4, 0.4, 0.4, 1.0],
};

/// Arguments of a `setLight 0x00448170` call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudLight {
    /// First argument, `lightFrontColor`.
    pub front: [f32; 4],
    /// Second argument, `lightBackColor`.
    pub back: [f32; 4],
    /// Third argument, the direction. At `0x004bb1e8` it is `vec3f(0, 0.5, -1).normalized()`
    /// (`0x00427870`) and the setter normalises again.
    pub direction: [f32; 3],
    /// Fourth argument, `ambientColor`.
    pub ambient: [f32; 4],
}

/// Controller and engine fields the HUD models read that `SceneState` does not have yet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudExtra {
    /// `0x0047fa10(0x00487490(GC+0x800710))`: the engine's root widget (`Engine+0xb4`) is
    /// visible.
    pub engine_root_visible: bool,
    /// `0x0043a490(GC+0x800710)` = `Engine+0xe8`: the engine clock in ms (`ui::GameView::
    /// engine_time_ms`).
    pub engine_time_ms: i32,
    /// `std::vector::size(GC+0x800778)`: the friend life bars, resized to 3 by the
    /// controller's constructor (`0x0046128c`).
    pub friend_frames: i32,
}

impl HudExtra {
    /// The fields as `SceneState` carries them (`engine_root_visible`, `engine_time_ms`,
    /// `friend_frames`).
    pub fn placeholder(ctx: &SceneCtx) -> HudExtra {
        HudExtra {
            engine_root_visible: ctx.gc.engine_root_visible,
            engine_time_ms: ctx.gc.engine_time_ms,
            friend_frames: ctx.gc.friend_frames,
        }
    }
}

fn i16_at(e: &EntityData, o: usize) -> i16 {
    i16::from_le_bytes([e.0[o], e.0[o + 1]])
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// One `0x00476660` call: nothing for a null model.
#[allow(clippy::too_many_arguments)]
fn hud_call(
    out: &mut Vec<HudModel>,
    origin: u32,
    screen: [f32; 2],
    rotation: [f32; 3],
    scale: f32,
    model: Option<ModelInfo>,
    depth: f32,
    material: [f32; 4],
) {
    // 0x00476660: `if (model != 0)`.
    if let Some(m) = model {
        out.push(HudModel { origin, screen, rotation, scale, model: m.handle, model_size: m.size, depth, material });
    }
}

/// `setMaterialColor(vec4(r/255, g/255, b/255, 1))` of the hair colour (`0x004bb3ea..0x004bb471`,
/// `0x004bb911..0x004bb994`).
fn hair_material(e: &EntityData) -> [f32; 4] {
    let c = |i: usize| e.0[off::HAIR_RGB + i] as i32 as f32 / 255.0;
    [c(0), c(1), c(2), 1.0]
}

/// The portrait wobble `vec3f(cos((t·0.0005 + φ))·20, 0, sin((t·0.001 + φ))·20)`
/// (`0x004bb0f0..0x004bb18b`, `0x004bb78b..0x004bb848`): the sums are done in double and
/// narrowed (`cvtpd2ps`); the sine is evaluated first.
fn wobble(t: i32, phase: i32) -> [f32; 3] {
    let s = sinf((t as f64 * 0.001 + phase as f64) as f32) * 20.0;
    let c = cosf((t as f64 * 0.0005 + phase as f64) as f32) * 20.0;
    [c, 0.0, s]
}

/// `0x00412280`: `out = other + this` (the wobble plus the base rotation).
fn add(this: [f32; 3], other: [f32; 3]) -> [f32; 3] {
    [other[0] + this[0], other[1] + this[1], other[2] + this[2]]
}

/// `0x004bb080..0x004bbb1a` with the placeholder [`HudExtra`]. The caller runs it only when
/// `GC+0x800884` (the HUD) is visible (`ScenePieces::hud_visible`); the check is repeated.
pub fn hud_models(ctx: &SceneCtx) -> Vec<HudModel> {
    hud_models_with(ctx, &HudExtra::placeholder(ctx))
}

/// `0x004bb080..0x004bbb1a`: the HUD models in draw order.
pub fn hud_models_with(ctx: &SceneCtx, x: &HudExtra) -> Vec<HudModel> {
    let mut out = Vec::new();
    // 0x004bb080..0x004bb0a7: GUI root and engine root visible.
    if !ctx.gc.gui.hud || !x.engine_root_visible {
        return out;
    }
    let m = ctx.models;
    let t = x.engine_time_ms;
    let player = ctx.entities.get(&ctx.gc.player);

    // 0x004bb0f0..0x004bb18b: the player's wobble (phase 0).
    let w = wobble(t, 0);

    // Unresolved: the original dereferences the player creature `GC+0x8006d0` unconditionally;
    // the port skips the player's portrait and pet when the player is not in the map.
    if let Some(p) = player {
        // 0x004bb32f..0x004bb3e5: white, head, rotation (-120, 0, 20) + wobble, scale 0.0035.
        let rot = add([-120.0, 0.0, 20.0], w);
        hud_call(
            &mut out,
            0x004b_b3e5,
            [50.0, 60.0],
            rot,
            0.0035,
            m.model(i16_at(p, off::HEAD_MODEL) as i32),
            0.0,
            WHITE,
        );
        // 0x004bb3ea..0x004bb4f2: hair colour, hair.
        let rot = add([-120.0, 0.0, 20.0], w);
        hud_call(
            &mut out,
            0x004b_b4f2,
            [50.0, 60.0],
            rot,
            0.0035,
            m.model(i16_at(p, off::HAIR_MODEL) as i32),
            0.0,
            hair_material(p),
        );
        // 0x004bb4f7..0x004bb610: the pet (`creature+0x11c8`, `World::findEntity`): white,
        // its head, or its body when the head index is negative, or nothing.
        let pet_id = ctx.states.get(&ctx.gc.player).map(|s| s.pet);
        if let Some(pet) = pet_id.and_then(|id| ctx.entities.get(&id)) {
            let head = i16_at(pet, off::HEAD_MODEL);
            let index = if head >= 0 {
                Some(head)
            } else {
                let body = i16_at(pet, off::BODY_MODEL);
                if body >= 0 { Some(body) } else { None }
            };
            if let Some(i) = index {
                let rot = add([-140.0, 0.0, 20.0], w);
                hud_call(&mut out, 0x004b_b610, [50.0, 210.0], rot, 0.0035, m.model(i as i32), 0.0, WHITE);
            }
        }
    }

    // 0x004bb615..0x004bb6cf: white, the compass.
    out.extend(compass(ctx, 0x004b_b6cf));

    // 0x004bb6d4..0x004bba7c: the other players, in map order.
    let mut index = 0; // esp+0xe4
    let mut phase = 0; // esp+0xe8, +43 per drawn player
    let mut sx = 270; // esp+0x3c, +220 per drawn player
    for (&id, e) in ctx.entities.iter() {
        // 0x004bb75d: `index >= size(GC+0x800778)` ends the loop.
        if index >= x.friend_frames {
            break;
        }
        // 0x004bb775: players only (hostile type 0), not the local player.
        if e.0[off::HOSTILE] != 0 || id == ctx.gc.player {
            continue;
        }
        let w = wobble(t, phase);
        let rot = add([-120.0, 0.0, 20.0], w);
        // 0x004bb84d..0x004bb90c: white, head, scale 0.0025, at (x, 45).
        hud_call(
            &mut out,
            0x004b_b90c,
            [sx as f32, 45.0],
            rot,
            0.0025,
            m.model(i16_at(e, off::HEAD_MODEL) as i32),
            0.0,
            WHITE,
        );
        // 0x004bb911..0x004bba1e: hair colour, hair.
        let rot = add([-120.0, 0.0, 20.0], w);
        hud_call(
            &mut out,
            0x004b_ba1e,
            [sx as f32, 45.0],
            rot,
            0.0025,
            m.model(i16_at(e, off::HAIR_MODEL) as i32),
            0.0,
            hair_material(e),
        );
        // 0x004bba23..0x004bba40 (the `+0x21` counter at esp+0xe0 is not read).
        index += 1;
        phase += 0x2b;
        sx += 0xdc;
    }
    // 0x004bba82..0x004bbab7: setMaterialColor(white), then the spin of `GC+0x800a1c`
    // ([`item_preview_spin`], a controller-state write the caller performs).
    out
}

/// The compass (`0x004bb615..0x004bb6cf`, `0x004ae009..0x004ae21b`): model `0x9ff`, white,
/// rotation `(-120, 0, -yaw)` (`GC+0x1ac`), scale 0.003, at `(w - 60, 40)`, depth 0.
fn compass(ctx: &SceneCtx, origin: u32) -> Option<HudModel> {
    let mut out = Vec::new();
    let rot = [-120.0, 0.0, -ctx.gc.camera.yaw];
    let sx = (ctx.gc.screen[0] - 0x3c) as f32;
    hud_call(&mut out, origin, [sx, 40.0], rot, 0.003, ctx.models.model(COMPASS_MODEL), 0.0, WHITE);
    out.pop()
}

/// The compass of the map screen, `0x004ae21b` (in `0x004adede..0x004ae279`, when the map is
/// open, `GC+0x8006e4`): after the map (`0x005fc1b0`), `setMaterialColor(white)`, a
/// `setLight` ([`MAP_COMPASS_LIGHT`]) and `Clear(0, 0, ZBUFFER, 0xff6464ff, 1.0, 0)` (depth only): see the
/// report. Same model, rotation, scale and place as the HUD compass.
pub fn map_compass(ctx: &SceneCtx) -> Option<HudModel> {
    compass(ctx, 0x004a_e21b)
}

/// `setLight` of `0x004ae15e` (before the map compass): front white, back (0.2, 0.3, 0.4),
/// ambient 0.4, direction `(0, 0.5, -1)·k` with `k = 1/sqrt(0·0 + 0.25 + 1)` (computed
/// inline in float, `sqrt` in double).
pub const MAP_COMPASS_LIGHT: HudLight = HudLight {
    front: [1.0, 1.0, 1.0, 1.0],
    back: [0.2, 0.3, 0.4, 1.0],
    direction: [0.0, 0.5, -1.0],
    ambient: [0.4, 0.4, 0.4, 1.0],
};

/// `0x004bbabc..0x004bbb16`: the rotation `GC+0x800a1c` the item preview of `0x004758c0`
/// (`0x004bb01d`) uses advances every HUD frame: `x = 225`, `y = 0`,
/// `z += dt·0.002·5·3` (`dt` = `GC+0x8006e8`, in that multiplication order).
pub fn item_preview_spin(rot: [f32; 3], dt_ms: i32) -> [f32; 3] {
    [225.0, 0.0, dt_ms as f32 * 0.002 * 5.0 * 3.0 + rot[2]]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use cw_render::frame::ModelRef;
    use cw_render::mesh::VoxelGrid;
    use cw_world::inventory::Item;

    use super::super::{SceneModels, SceneState};
    use super::*;

    struct Models;
    impl SceneModels for Models {
        fn model(&self, index: i32) -> Option<ModelInfo> {
            if index < 0 {
                return None;
            }
            Some(ModelInfo { handle: index as u32, size: [10, 12, 14] })
        }
        fn prop_model(&self, _kind: u32) -> Option<ModelInfo> {
            None
        }
        fn prop_model_count(&self) -> usize {
            0
        }
        fn item_model(&self, _item: &Item) -> Option<ModelInfo> {
            None
        }
        fn voxels(&self, _model: ModelRef) -> Option<VoxelGrid<'_>> {
            None
        }
        fn particle_cube(&self) -> ModelRef {
            0
        }
    }

    fn with_ctx<R>(gc: &SceneState, entities: &BTreeMap<i64, EntityData>, f: impl FnOnce(&SceneCtx) -> R) -> R {
        let world = cw_world::World::new(0);
        let states = BTreeMap::new();
        let ctx = SceneCtx {
            world: &world,
            entities,
            states: &states,
            gc,
            models: &Models,
            projectiles: &[],
            planes: [[0.0; 4]; 6],
            daylight: 1.0,
            underwater: false,
            world_material: [1.0; 4],
            draw_distance: 100.0,
            prop_far: 20.0,
            prop_near: 18.0,
            static_far: 30.0,
            static_near: 27.0,
            particle_range: 20.0,
            dt_ms: 16,
        };
        f(&ctx)
    }

    fn player(head: i16, hair: i16, hostile: u8) -> EntityData {
        let mut e = EntityData::default();
        e.0[off::HEAD_MODEL..off::HEAD_MODEL + 2].copy_from_slice(&head.to_le_bytes());
        e.0[off::HAIR_MODEL..off::HAIR_MODEL + 2].copy_from_slice(&hair.to_le_bytes());
        e.0[off::HAIR_RGB] = 255;
        e.0[off::HAIR_RGB + 1] = 0;
        e.0[off::HAIR_RGB + 2] = 51;
        e.0[off::HOSTILE] = hostile;
        e
    }

    #[test]
    fn compass_follows_camera_yaw() {
        let mut gc = SceneState::default();
        gc.screen = [1280, 720];
        let entities = BTreeMap::new();
        for yaw in [0.0f32, 37.5, -90.0, 270.0] {
            gc.camera.yaw = yaw;
            let c = with_ctx(&gc, &entities, map_compass).unwrap();
            assert_eq!(c.rotation, [-120.0, 0.0, -yaw]);
            assert_eq!(c.screen, [1220.0, 40.0]);
            assert_eq!(c.scale, 0.003);
            assert_eq!(c.model, 0x9ff);
            assert_eq!(c.origin, 0x004a_e21b);
        }
    }

    #[test]
    fn hud_layout_1280x720() {
        let mut gc = SceneState::default();
        gc.screen = [1280, 720];
        gc.player = 5;
        gc.gui.hud = true;
        gc.camera.yaw = 10.0;
        let mut entities = BTreeMap::new();
        entities.insert(1, player(3, 4, 0));
        entities.insert(2, player(6, 7, 1)); // hostile: skipped
        entities.insert(5, player(1, 2, 0)); // the local player
        for id in 6..10 {
            entities.insert(id, player(8, 9, 0));
        }
        let h = with_ctx(&gc, &entities, |ctx| {
            hud_models_with(ctx, &HudExtra { engine_root_visible: true, engine_time_ms: 0, friend_frames: 3 })
        });
        let got: Vec<(u32, [f32; 2], u32)> = h.iter().map(|m| (m.origin, m.screen, m.model)).collect();
        assert_eq!(
            got,
            vec![
                (0x004b_b3e5, [50.0, 60.0], 1),
                (0x004b_b4f2, [50.0, 60.0], 2),
                (0x004b_b6cf, [1220.0, 40.0], 0x9ff),
                (0x004b_b90c, [270.0, 45.0], 3),
                (0x004b_ba1e, [270.0, 45.0], 4),
                (0x004b_b90c, [490.0, 45.0], 8),
                (0x004b_ba1e, [490.0, 45.0], 9),
                (0x004b_b90c, [710.0, 45.0], 8),
                (0x004b_ba1e, [710.0, 45.0], 9),
            ]
        );
        // t = 0, phase 0: wobble (20, 0, 0).
        assert_eq!(h[0].rotation, [-100.0, 0.0, 20.0]);
        assert_eq!(h[0].scale, 0.0035);
        assert_eq!(h[1].material, [1.0, 0.0, 51.0 / 255.0, 1.0]);
        assert_eq!(h[2].rotation, [-120.0, 0.0, -10.0]);
        assert_eq!(h[3].scale, 0.0025);
        // Second friend: phase 43.
        assert_eq!(h[5].rotation, add([-120.0, 0.0, 20.0], wobble(0, 43)));

        // Hidden HUD: nothing.
        gc.gui.hud = false;
        assert!(with_ctx(&gc, &entities, hud_models).is_empty());
    }

    #[test]
    fn spin() {
        assert_eq!(item_preview_spin([1.0, 2.0, 3.0], 100), [225.0, 0.0, 100.0f32 * 0.002 * 5.0 * 3.0 + 3.0]);
    }
}
