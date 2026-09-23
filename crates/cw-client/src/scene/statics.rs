//! Zone statics and ground items of the objects pass (`Cube.exe 0x004b21ac..0x004b32ed`),
//! `drawStatic 0x004be760` and the camp-fire flames `0x004bbd80`.
//!
//! # Map
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x004b21ac..0x004b21f3` | skipped when white-cleared; zone bounds `[esp+0x28]..[esp+0xa4]` × `[esp+0xa8]..[esp+0xd8]` (computed at `0x004b0756..0x004b0835` for the lights: `zones_around(player, draw_distance)`) | [`zone_objects`] |
//! | `0x004b21f3..0x004b2c8e` | per zone: its ground items | `super::items::zone_items` |
//! | `0x004b2c8e..0x004b3292` | per zone: its statics, in vector order | [`zone_objects`] |
//! | `0x004b2e0f..0x004b3009` | kind 6 with `+0x30` set: a beam | [`super::beam::draw_beam`] |
//! | `0x004b3097..0x004b3164` | near statics: world material, +0.1 when aimed at (`GC+0x800a84`), `0x004be760` | [`draw_static`] |
//! | `0x004b3169..0x004b325c` | kind 0x41: the fire | [`fire`] |
//! | `0x004b3053..0x004b3092` | far statics to the far list (`0x004c1190`) | [`FarEntry`] |

use cw_math::rand::MsvcRand;
use cw_render::frame::D3dMatrix;
use cw_render::passes::{ModelDraw, sphere_visible};
use cw_world::zone::Static;

use super::beam::{BeamArgs, draw_beam, pre_rotate_x};
use super::math::*;
use super::{FarEntry, FarObject, ModelInfo, SceneCtx, entity_pos, zones_around};
use crate::particles::ParticleSystem;

/// The static model table `GC+0x800724` (78 slots, filled by the controller constructor at
/// `0x00462c04..`): per static kind, the index into the model vector `GC+0x300`
/// ([`super::SceneModels::model`]), `-1` for the three null slots (6, 14, 70).
const STATIC_MODELS: [i32; 0x4e] = [
    0x9b1, 0x9b2, 0x9b3, 0x9b4, 0x9b5, 0x9b6, -1, 0x9b7, 0x9b8, 0x9b9, // 0..9
    0x9ba, 0x9bb, 0x9bc, 0x9bd, -1, 0x9bf, 0x9c1, 0x9c0, 0x9c2, 0x9c3, // 10..19
    0x9c4, 0x9c6, 0x9c7, 0x9c8, 0x9c9, 0x9ca, 0x9cb, 0x9cc, 0x9cd, 0x9c5, // 20..29
    0x9ce, 0x9cf, 0x9d0, 0x9d1, 0x9d2, 0x9d3, 0x9d4, 0x9d5, 0x9d6, 0x9d7, // 30..39
    0x9d8, 0x9d9, 0x9da, 0x9db, 0x9dc, 0x9dd, 0x81f, 0x9de, 0x9df, 0x9e0, // 40..49
    0x9e1, 0x9e2, 0x9e3, 0x9e4, 0x9e5, 0x9e6, 0x912, 0x913, 0x914, 0x915, // 50..59
    0x916, 0x917, 0x918, 0x919, 0x91a, 0x9fa, 0x9fb, 0x9fc, 0x9fd, 0x9fe, // 60..69
    -1, 0x9e7, 0x9e8, 0x9ec, 0x9ed, 0x9e9, 0x9ea, 0x9eb, // 70..77
];

/// `GC+0x800724[kind]` (`None` for a null slot, an unloaded model or a kind outside the
/// table).
fn static_model(ctx: &SceneCtx, kind: i32) -> Option<ModelInfo> {
    let index = *STATIC_MODELS.get(usize::try_from(kind).ok()?)?;
    if index < 0 { None } else { ctx.models.model(index) }
}

/// `GC+0x800a84`: the static under the crosshair (zone x, zone y, index), written by
/// `GameController::update` (`0x00496b1e`, `0x004973eb`; `player::LocalPlayer::aimed_static`).
/// `SceneState::aimed_static` (the constructor's `(-1, -1, 0)`, `0x0045a534`, matches no
/// static).
fn aimed_static(ctx: &SceneCtx) -> [i32; 3] {
    ctx.gc.aimed_static
}

/// `[esp+0x34]` of `render`: `timeGetTime()` read at the top of the frame (`0x004ac2aa`),
/// the clock of the fire flames.
/// `SceneState::frame_time_ms`.
fn frame_time_ms(ctx: &SceneCtx) -> i32 {
    ctx.gc.frame_time_ms
}

/// `0x004b21ac..0x004b32ed`: the zones around the local player: ground items, then statics.
///
/// Nothing is drawn when the frame is white-cleared (`0x004b21c7`) or the local player's
/// creature is missing (the original reads its position through `GC+0x8006d0`). Zones are
/// walked x outer, y inner, both inclusive. Per zone, the ground items
/// (`super::items::zone_items`), then every static in vector order whose kind is inside the
/// static model table (78, `0x004b2cfa..0x004b2d11`, whether or not the slot has a model) and
/// whose sphere (centre raised by `scale z / 2`, radius `max(scale) + 1`) is visible within
/// `static_far`:
///
/// - kind 6 with byte `+0x30` set: a beam along the static's facing (8 blocks, centred on
///   it, 40 segments, red to orange, no sparks), [`draw_beam`];
/// - squared distance above `static_near²`: to the far list;
/// - otherwise [`draw_static`] with the world material (+0.1 on each component for the
///   static under the crosshair), and for kind 0x41 (camp fire) the flames [`fire`].
///
/// No `rand()` is drawn here (the statics' beams have no sparks); the ground items may.
pub fn zone_objects(
    ctx: &SceneCtx,
    particles: &mut ParticleSystem,
    rng: &mut MsvcRand,
    far: &mut Vec<FarEntry>,
    objects: &mut Vec<ModelDraw>,
) {
    let gc = ctx.gc;
    // 0x004b21c7.
    if gc.white_clear {
        return;
    }
    let Some(player) = ctx.entities.get(&gc.player) else { return };
    let (xs, ys) = zones_around(entity_pos(player), ctx.draw_distance);
    let cam = gc.camera.position;
    for zx in xs[0]..=xs[1] {
        for zy in ys[0]..=ys[1] {
            // 0x004b2214: `World::getZone`.
            let Some(zone) = ctx.world.zone(zx, zy) else { continue };
            // 0x004b2225..0x004b2c8e.
            super::items::zone_items(ctx, (zx, zy), zone, objects, far);
            // 0x004b2c95..0x004b3292.
            for (index, s) in zone.statics.iter().enumerate() {
                let kind = s.kind as i32;
                // 0x004b2cfa: `js` and `size()` of `GC+0x800724`.
                if kind < 0 || kind >= STATIC_MODELS.len() as i32 {
                    continue;
                }
                // 0x004b2d17: radius max(scale) + 1 (`comiss`: NaN keeps the previous).
                let [sx, sy, sz] = s.scale;
                let mut m = sx;
                if sy > m {
                    m = sy;
                }
                if sz > m {
                    m = sz;
                }
                let radius = m + 1.0;
                // 0x004b2d95: `0x00459c00`: `ftol((double)z · 0.5 · 65536)`.
                let pos = [s.x, s.y, s.z];
                let centre = add3(pos, [0, 0, ((sz as f64) * 0.5 * 65536.0) as i64]);
                // 0x004b2df0: `0x0047f760`.
                if !sphere_visible(&ctx.planes, &gc.camera, centre, radius, ctx.static_far) {
                    continue;
                }
                // 0x004b2dfd: the beam.
                if s.kind == 6 && s.b30 != 0 {
                    objects.extend(draw_beam(ctx, &static_beam(s, gc.clock_ms), particles, rng));
                }
                // 0x004b300e: squared distance of the position (not the centre).
                let dist2 = length_sq(float_from_fixed(sub3(pos, cam)));
                // 0x004b304e: `comiss dist2, near²; jbe near`.
                if dist2 > ctx.static_near * ctx.static_near {
                    far.push(FarEntry { object: FarObject::Static { zone: (zx, zy), index }, dist2 });
                    continue;
                }
                // 0x004b3097: the world material, brightened when aimed at (`0x00468870`
                // compares the vec3i `(zone x, zone y, index)` built by `0x0042c500` with
                // `GC+0x800a84`; `0x004289e0` adds (0.1, 0.1, 0.1, 0.1)).
                let mut material = ctx.world_material;
                if aimed_static(ctx) == [zx, zy, index as i32] {
                    material = [0.1 + material[0], 0.1 + material[1], 0.1 + material[2], 0.1 + material[3]];
                }
                // The shader's alpha before the call: the last draw's (see `draw_static_with`).
                let alpha = objects.last().map_or(1.0, |d| d.alpha);
                let draws = draw_static_with(ctx, s, dist2, ctx.static_far, ctx.static_near, material, alpha);
                let alpha = draws.last().map_or(alpha, |d| d.alpha);
                objects.extend(draws);
                // 0x004b3169: camp fire: flames at the centre, red to yellow, radius 0.8,
                // size 0.2, 20 cubes.
                if s.kind == 0x41 {
                    let centre = add3(pos, fixed_from_float([0.0, 0.0, sz * 0.5]));
                    objects.extend(fire(
                        ctx,
                        centre,
                        [1.0, 0.0, 0.0, 1.0],
                        [1.0, 1.0, 0.0, 1.0],
                        frame_time_ms(ctx),
                        0.8,
                        0.2,
                        0x14,
                        alpha,
                    ));
                }
            }
        }
    }
    // 0x004b32da: `setAlpha(1)` (state only).
}

/// `0x004b2e14..0x004b3009`: the beam of a kind-6 static: direction `(8, 0, 0)` turned by
/// the rotation quadrant (1: `(0, 8, 0)`, 2: `(-8, 0, 0)`, 3: `(0, -8, 0)`), starting half a
/// beam behind the static, angle 0, colours red `(1, 0, 0)`, `(1, 0.25, 0)`, `(1, 0.5, 0)`,
/// time = the world clock (`0x00488b80`: `World+0x8000bc`), 0.2, size 4, 2, 40 segments, no
/// sparks.
fn static_beam(s: &Static, clock: i32) -> BeamArgs {
    let dir = match s.rotation {
        1 => [0.0, 8.0, 0.0],
        2 => [-8.0, 0.0, 0.0],
        3 => [0.0, -8.0, 0.0],
        _ => [8.0, 0.0, 0.0],
    };
    // 0x004b2ee3: `vec3fScale(dir, 0.5)`, `vec3i64::fromFloat`, `pos - half`.
    let half = fixed_from_float([dir[0] * 0.5, dir[1] * 0.5, dir[2] * 0.5]);
    BeamArgs {
        start: sub3([s.x, s.y, s.z], half),
        dir,
        angle: 0.0,
        colors: [[1.0, 0.0, 0.0, 1.0], [1.0, 0.25, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0]],
        time: clock,
        p11: 0.2,
        size: 4.0,
        p13: 2.0,
        segments: 40,
        sparks: false,
    }
}

/// `0x004be760(static, dist2, far, near, …, material)`: one static, as the far list draws it
/// (`0x004ba54a`: the world material, no highlight).
///
/// See [`draw_static_with`]. The alpha a chest's base (kind 10) is drawn with is the
/// shader's current alpha in the original (the previous far entry's fade); here it is this
/// static's own fade.
pub fn draw_static(ctx: &SceneCtx, s: &Static, _zone: (i32, i32), _index: usize, dist2: f32, far: f32, near: f32) -> Vec<ModelDraw> {
    draw_static_with(ctx, s, dist2, far, near, ctx.world_material, fade(dist2, far, near))
}

/// The fade of `0x004c0403`: 1 within `near` (`comiss dist2, near²; jbe`), then
/// `1 - (sqrt(dist2) - near) / (far - near)`.
fn fade(dist2: f32, far: f32, near: f32) -> f32 {
    if dist2 > near * near { 1.0 - (((dist2 as f64).sqrt() as f32) - near) / (far - near) } else { 1.0 }
}

/// How far a door, chest, lever or lift is open (`0x004bee9d` and its copies): with byte
/// `+0x30` clear (opening), `min(+0x34 · 0.001, 1)`; set (closing), `max(1 - +0x34 · k, 0)`
/// with `k` = 0.001 or 0.01 depending on the kind. `+0x34` is read as a signed int (ms).
fn open_amount(s: &Static, closing: f32) -> f32 {
    let t = s.f34 as i32 as f32;
    if s.b30 == 0 {
        let f = t * 0.001;
        if f > 1.0 { 1.0 } else { f }
    } else {
        let f = 1.0 - t * closing;
        if 0.0 > f { 0.0 } else { f }
    }
}

/// `0x004be760` with the material and the shader's current alpha (used only by the base of a
/// chest) given.
///
/// The model is `GC+0x800724[kind]` ([`STATIC_MODELS`]); a static whose slot is null draws
/// nothing. The light is the block at the static's position (`x/65536` etc., truncating;
/// `prop_light`: 255 for a light block, `max(byte 0, 5)` in air or water, else 0) and the
/// material is `(r, g, b, a · light/255)`, with alpha component 1 for lamp posts (0x32,
/// 0x33). Transform, pre-multiplied left to right:
///
/// `T(0, 0, lift) · T(position) · S(scale x / size x) · Rz(90° · quadrant) ·
/// T(-size x / 2, -size y / 2, 0) · [open]`, with:
///
/// - kinds 5, 7, 8 (lifts, `lift` = `scale z · 0.8 · f`, `-(scale z · f) - 1`,
///   `scale z · f`);
/// - kinds 1, 2, 3 (doors): `T(0, size y / 2, 0) · Rz(90° · f) · T(0, -size y / 2, 0)`;
/// - kind 10 (chest): the base, model slot 10, drawn first with the plain transform; the
///   lid, model slot 11: `T(0, 0, 9) · Rx(80° · f) · T(0, 0, -1)`;
/// - kind 9 (lever): `T(0, size y / 2, -0.5) · Rx(60° · f - 30°) · T(0, -size y / 2, 0)`.
///
/// The main draw's alpha is the fade ([`fade`]).
fn draw_static_with(ctx: &SceneCtx, s: &Static, dist2: f32, far: f32, near: f32, material: [f32; 4], alpha_state: f32) -> Vec<ModelDraw> {
    let gc = ctx.gc;
    let mut out = Vec::new();
    let kind = s.kind as i32;
    let pos = [s.x, s.y, s.z];
    let rt = render_translation(pos, gc.camera.render_offset);
    let quadrant = (s.rotation.wrapping_mul(90)) as f32;
    // 0x004bed49 / 0x004c030e: `World::getBlock(x / 65536, y / 65536, z / 65536)`.
    let light = cw_render::light::prop_light(cw_render::light::get_block(
        ctx.world,
        (s.x / 65536) as i32,
        (s.y / 65536) as i32,
        (s.z / 65536) as i32,
    ));
    let lit = (light as f32) / 255.0;
    // The plain placement: T(position) · S · Rz(quadrant) · T(-size/2).
    let place = |m: &mut D3dMatrix, model: &ModelInfo| {
        let sc = s.scale[0] / (model.size[0] as f32);
        pre_translate(m, rt);
        pre_scale(m, [sc, sc, sc]);
        pre_rotate_z(m, quadrant);
        pre_translate(m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
    };
    let mut model = static_model(ctx, kind);
    // 0x004be78a..0x004bee42: the chest's base (the original does not test the slot).
    if kind == 10 {
        if let Some(base) = model {
            let mut m = identity();
            place(&mut m, &base);
            out.push(ModelDraw {
                origin: 0x004b_e760,
                model: base.handle,
                world: m,
                material: [material[0], material[1], material[2], lit * material[3]],
                alpha: alpha_state,
                double_sided: false,
                shininess: 0.0,
                set_white: None,
                mirrored: false,
            });
        }
        // 0x004bee47: `GC+0x800724[11]`.
        model = static_model(ctx, 11);
    }
    let Some(model) = model else { return out };
    let mut m = identity();
    // 0x004bee94..0x004bf2d0: lifts.
    let lift = match kind {
        5 => Some(s.scale[2] * 0.8 * open_amount(s, 0.001)),
        7 => Some(-(s.scale[2] * open_amount(s, 0.01)) - 1.0),
        8 => Some(s.scale[2] * open_amount(s, 0.01)),
        _ => None,
    };
    if let Some(t) = lift {
        pre_translate(&mut m, [0.0, 0.0, t]);
    }
    place(&mut m, &model);
    let half_y = (model.size[1] as f32) * 0.5;
    let neg_half_y = (model.size[1] as f32) * -0.5;
    // Doors turn about their hinge edge.
    if kind == 1 || kind == 2 || kind == 3 {
        let f = open_amount(s, 0.001);
        pre_translate(&mut m, [0.0, half_y, 0.0]);
        pre_rotate_z(&mut m, f * 90.0);
        pre_translate(&mut m, [0.0, neg_half_y, 0.0]);
    }
    // 0x004bfc5c: the chest lid.
    if kind == 10 {
        let f = open_amount(s, 0.001);
        pre_translate(&mut m, [0.0, 0.0, 9.0]);
        pre_rotate_x(&mut m, f * 80.0);
        pre_translate(&mut m, [0.0, 0.0, -1.0]);
    }
    // 0x004bff9a: the lever.
    if kind == 9 {
        let f = open_amount(s, 0.001);
        pre_translate(&mut m, [0.0, half_y, -0.5]);
        pre_rotate_x(&mut m, f * 60.0 - 30.0);
        pre_translate(&mut m, [0.0, neg_half_y, 0.0]);
    }
    // 0x004c039b: the material; lamp posts are full bright.
    let mut mat = [material[0], material[1], material[2], material[3] * lit];
    if kind == 0x32 || kind == 0x33 {
        mat[3] = 1.0;
    }
    out.push(ModelDraw {
        origin: 0x004b_e760,
        model: model.handle,
        world: m,
        material: mat,
        alpha: fade(dist2, far, near),
        double_sided: false,
        shininess: 0.0,
        set_white: None,
        mirrored: false,
    });
    out
}

/// `0x004bbd80(center, color_a, color_b, time, …, radius, size, count)`: flames, `count`
/// particle cubes (`GC+0x800730`) rising and fading from `color_a` to `color_b` over 2 s
/// each, staggered by 443 ms.
///
/// Cube `i` (`t = time + 443 i`, `f = (t mod 2000) / 2000`): at
/// `centre + (cos a · radius, sin a · radius, f · radius · 2)` with
/// `a = (i / 10 + (time mod 10000) · 0.0001) · 2π`, turned by `Rx(40° f)`, `Ry(30° f)`,
/// `Rz(10° f)` (the rotation block is the original's inlined product, zero terms dropped:
/// they are exact zeros), scaled by `(cos(13 i + (time mod 500)/500 · 2π) · 0.5 + 1) · size ·
/// (1 - (2f - 1)²)`, centred, coloured `color_a · (1 - f) + color_b · f`. The alpha is the
/// shader's current one (`alpha`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire(
    ctx: &SceneCtx,
    centre: [i64; 3],
    color_a: [f32; 4],
    color_b: [f32; 4],
    time: i32,
    radius: f32,
    size: f32,
    count: i32,
    alpha: f32,
) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    if count <= 0 {
        return out;
    }
    const PI: f64 = std::f64::consts::PI;
    // 0x004bbdb8: from the time at the call.
    let base = ((((time % 500) as f32) / 500.0 * 2.0) as f64) * PI;
    let phase = ((time % 10000) as f32) * 0.0001;
    let r = render_translation(centre, ctx.gc.camera.render_offset);
    let cube = ctx.models.particle_cube();
    // The particle cube is 1×1×1 (`Model+0x44..+0x4c` of `GC+0x800730`).
    let cube_size = [1.0f32; 3];
    let mut t = time;
    let mut n13 = 0i32;
    for i in 0..count {
        let f = ((t % 2000) as f32) / 2000.0;
        let g = (f - 0.5) * 2.0;
        let c0 = cosf(((n13 as f64) + base) as f32);
        let scale = (c0 * 0.5 + 1.0) * size * (1.0 - g * g);
        let a = ((((i as f32) / 10.0 + phase) * 2.0) as f64 * PI) as f32;
        // 0x004bbf09: the colour.
        let inv = 1.0 - f;
        let color = [
            color_a[0] * inv + color_b[0] * f,
            color_a[1] * inv + color_b[1] * f,
            color_a[2] * inv + color_b[2] * f,
            color_a[3] * inv + color_b[3] * f,
        ];
        // 0x004bbffd: the translation row.
        let (ca, sa) = (cosf(a), sinf(a));
        let tr = [r[0] + ca * radius, r[1] + sa * radius, r[2] + f * radius * 2.0];
        // 0x004bc16e..0x004bc4c9: Rx(40 f), Ry(30 f), Rz(10 f) (inlined).
        let (c1, s1) = (cosf(f * 40.0 * DEG), sinf(f * 40.0 * DEG));
        let (c2, s2) = (cosf(f * 30.0 * DEG), sinf(f * 30.0 * DEG));
        let (c3, s3) = (cosf(f * 10.0 * DEG), sinf(f * 10.0 * DEG));
        let s1s2 = s1 * s2;
        let c1s2 = c1 * s2;
        let mut m: D3dMatrix = [
            [c2 * c3, c1 * s3 + s1s2 * c3, s1 * s3 - c1s2 * c3, 0.0],
            [-(c2 * s3), c1 * c3 - s1s2 * s3, s1 * c3 + c1s2 * s3, 0.0],
            [s2, -(s1 * c2), c1 * c2, 0.0],
            [tr[0], tr[1], tr[2], 1.0],
        ];
        // 0x004bc4d6: `ucomiss scale, 1; jnp`: scaled unless exactly 1.
        pre_scale(&mut m, [scale, scale, scale]);
        pre_translate(&mut m, [cube_size[0] * -0.5, cube_size[1] * -0.5, cube_size[2] * -0.5]);
        out.push(ModelDraw { origin: 0x004b_bd80, model: cube, world: m, material: color, alpha, double_sided: false, shininess: 0.0, set_white: None, mirrored: false });
        t = t.wrapping_add(0x1bb);
        n13 = n13.wrapping_add(0xd);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use cw_render::frame::ModelRef;
    use cw_render::mesh::VoxelGrid;
    use cw_world::inventory::Item;

    use crate::scene::{SceneModels, SceneState};

    /// Every model index is a 10×20×30 model whose handle is the index.
    struct Models;
    impl SceneModels for Models {
        fn model(&self, index: i32) -> Option<ModelInfo> {
            Some(ModelInfo { handle: index as ModelRef, size: [10, 20, 30] })
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
        fn voxels(&self, _: ModelRef) -> Option<VoxelGrid<'_>> {
            None
        }
        fn particle_cube(&self) -> ModelRef {
            1
        }
    }

    fn with_ctx<R>(f: impl FnOnce(&SceneCtx) -> R) -> R {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let mut gc = SceneState::default();
        gc.camera.render_offset = [-(1000 << 16), -(2000 << 16)];
        let ctx = SceneCtx {
            world: &world,
            entities: &entities,
            states: &states,
            gc: &gc,
            models: &Models,
            projectiles: &[],
            planes: [[0.0, 0.0, 0.0, 1.0]; 6],
            daylight: 1.0,
            underwater: false,
            world_material: [1.0, 1.0, 1.0, 0.5],
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

    fn statik(kind: u32, rotation: i32) -> Static {
        let mut s = Static::NEW;
        s.kind = kind;
        s.x = 1000 << 16;
        s.y = 2000 << 16;
        s.z = 5000 << 16;
        s.rotation = rotation;
        s.scale = [5.0, 5.0, 5.0];
        s
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn plain_static_is_centred_scaled_and_turned() {
        with_ctx(|ctx| {
            // Kind 4, quadrant 1: model 0x9b5, 10×20×30 voxels scaled by 5/10.
            let d = draw_static(ctx, &statik(4, 1), (0, 0), 0, 4.0, 30.0, 27.0);
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].model, 0x9b5);
            assert_eq!(d[0].alpha, 1.0);
            let m = &d[0].world;
            // The model's centre (5, 10, 0) lands on the static (render space (0, 0, 5000)).
            assert!(close(transform_point(m, [5.0, 10.0, 0.0]), [0.0, 0.0, 5000.0]));
            // Voxel +x becomes +y after the quarter turn, at half size.
            let p = transform_point(m, [6.0, 10.0, 0.0]);
            assert!(close(p, [0.0, 0.5, 5000.0]), "{p:?}");
        });
    }

    #[test]
    fn door_opens_about_its_edge_and_fades() {
        with_ctx(|ctx| {
            let mut s = statik(1, 0);
            s.b30 = 0;
            s.f34 = 1000; // fully open: 90°
            let d = draw_static(ctx, &s, (0, 0), 0, 28.5 * 28.5, 30.0, 27.0);
            assert_eq!(d.len(), 1);
            assert!((d[0].alpha - 0.5).abs() < 1e-5);
            let m = &d[0].world;
            // The hinge (model point (0, size y / 2)) stays put; the opposite edge swings a
            // quarter turn: (2.5, 0) closed, (-2.5, 5) open (half scale).
            let hinge = transform_point(m, [0.0, 10.0, 0.0]);
            assert!(close(hinge, [-2.5, 0.0, 5000.0]), "{hinge:?}");
            let free = transform_point(m, [10.0, 10.0, 0.0]);
            assert!(close(free, [-2.5, 5.0, 5000.0]), "{free:?}");
        });
    }

    #[test]
    fn chest_draws_base_then_lid_and_lamps_are_bright() {
        with_ctx(|ctx| {
            let d = draw_static(ctx, &statik(10, 0), (0, 0), 0, 1.0, 30.0, 27.0);
            assert_eq!(d.iter().map(|d| d.model).collect::<Vec<_>>(), vec![0x9ba, 0x9bb]);
            let d = draw_static(ctx, &statik(0x32, 0), (0, 0), 0, 1.0, 30.0, 27.0);
            assert_eq!(d[0].material[3], 1.0);
            // Beams (kind 6) have no model.
            assert!(draw_static(ctx, &statik(6, 0), (0, 0), 0, 1.0, 30.0, 27.0).is_empty());
        });
    }

    #[test]
    fn beam_of_a_static_is_centred_on_it() {
        let s = statik(6, 3);
        let b = static_beam(&s, 77);
        assert_eq!(b.dir, [0.0, -8.0, 0.0]);
        assert_eq!(b.start, [1000 << 16, (2000 << 16) + (4 << 16), 5000 << 16]);
        assert_eq!((b.segments, b.sparks, b.time), (40, false, 77));
    }

    #[test]
    fn fire_has_twenty_cubes_around_the_centre() {
        with_ctx(|ctx| {
            let d = fire(ctx, [1000 << 16, 2000 << 16, 5000 << 16], [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0], 123_456, 0.8, 0.2, 20, 1.0);
            assert_eq!(d.len(), 20);
            for x in &d {
                let c = transform_point(&x.world, [0.5, 0.5, 0.5]);
                assert!((c[0] * c[0] + c[1] * c[1]).sqrt() <= 0.8 + 1e-4, "{c:?}");
                assert!(c[2] >= 5000.0 && c[2] <= 5001.6 + 1e-3, "{c:?}");
                assert_eq!(x.material[0], 1.0);
            }
        });
    }
}
