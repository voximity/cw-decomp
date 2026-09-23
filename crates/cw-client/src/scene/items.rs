//! Ground items of the objects pass (`Cube.exe 0x004b21f3..0x004b2c50`, inside
//! `GameController::render 0x004ac260`), their spirit cubes (`0x00471b60`), the item model
//! lookup `ModelCache::modelForItem 0x004ec400` (with `0x004ec370` and `0x0051be60`) and the
//! build-mode target marker (`0x004b3338..0x004b35d4`).
//!
//! Ground items never go to the far list: an item that fails the visibility test is not drawn
//! at all (`0x004b23e6 je 0x004b2c50`).

use cw_render::frame::D3dMatrix;
use cw_render::passes::{ModelDraw, sphere_visible};
use cw_world::inventory::Item;
use cw_world::zone::{GroundItem, Zone};

use super::math::*;
use super::{FarEntry, ModelInfo, SceneCtx, entity_pos, material_color};
use crate::interact::{can_pick_up, ground_light_at};

// ---------------------------------------------------------------------------------------
// The ground-item loop
// ---------------------------------------------------------------------------------------

/// `GC+0x800a78` (`vec3i`: zone x, zone y, index in the zone's item list): the ground item under
/// the crosshair, written by the update's aim pass (`crate::player::LocalPlayer::aimed_item`).
///
/// `SceneState::aimed_item` (`[-1, -1, -1]` matches no item: zone coordinates are never
/// negative).
fn aimed_item(ctx: &SceneCtx) -> [i32; 3] {
    ctx.gc.aimed_item
}

/// `0x004b21f3..0x004b2c50`: the ground items of one zone (`zone+0x30`, list order), drawn into
/// `objects`. Per item (its index counts every item, skipped or not, from 0):
///
/// - skipped while claimed (`+0x140 != 0`), when it has no model (the original has no null
///   check), when the local player cannot pick it up (`canPickUp 0x0043e550`: a type-0x19 item
///   only for its level), or when its sphere is not visible;
/// - the model is `0x004ec400` ([`item_model_index`], `ctx.models.item_model`);
/// - a dropped item bobs while its countdown `+0x13c` is positive:
///   `z += 5 · (1 - ((t - 250) / 250)²)` blocks;
/// - visibility: `0x0047f760` at the (bobbed) position raised by `size z · 0.5 · scale`, margin
///   `size x · 0.7 · scale`, within `static_far` (`esp+0x4c`);
/// - material: white with alpha `groundLightAt(pos) / 255 · daylight` (at the item's
///   unbobbed position), alpha 1 for glowing items (type 0xb sub 0x13, type 0x12);
/// - transform: see [`item_matrix`];
/// - highlight: `+ (0.25, 0.25, 0.25, 0.25)` when `(zone x, zone y, index)` equals
///   `GC+0x800a78` ([`aimed_item`]);
/// - then the spirit cubes ([`item_spirits`]).
///
/// The shader alpha is 1 throughout (every near prop and static drawn before sets 1).
/// `far` is unused: the items have no far path.
pub fn zone_items(ctx: &SceneCtx, zone: (i32, i32), z: &Zone, objects: &mut Vec<ModelDraw>, _far: &mut Vec<FarEntry>) {
    let gc = ctx.gc;
    // `GC+0x8006d0`: the local player creature (always present in the original).
    let Some(player) = ctx.entities.get(&gc.player) else { return };
    let aimed = aimed_item(ctx);
    let mut index: i32 = -1;
    for it in &z.items {
        // 0x004b2270: the index counts every item.
        index += 1;
        // 0x004b2282.
        if it.f140 != 0 {
            continue;
        }
        // 0x004b2296: `0x004ec400` (returns the `cube::Model*`; no null check follows).
        let Some(model) = ctx.models.item_model(&it.item) else { continue };
        let [sx, _, sz] = model.size;
        // 0x004b22a1: margin = size x · 0.7 · scale.
        let margin = (sx as f32) * 0.7 * it.f134;
        // 0x004b22d1: the position, bobbing while `+0x13c > 0`.
        let mut pos = [it.x, it.y, it.z];
        if it.f13c > 0 {
            pos[2] = pos[2].wrapping_add(bob_offset(it.f13c));
        }
        // 0x004b233b: `canPickUp(player, item)`.
        if !can_pick_up(player, &it.item.to_bytes()) {
            continue;
        }
        // 0x004b234f: the centre raised by `size z · 0.5 · scale` (double precision, `0x00459c00`).
        let half = ftol_f64((sz as f64) * 0.5 * (it.f134 as f64) * 65536.0);
        let centre = add3(pos, [0, 0, half]);
        if !sphere_visible(&ctx.planes, &gc.camera, centre, margin, ctx.static_far) {
            continue;
        }
        // 0x004b24d3: the light at the item's own (unbobbed) position.
        let light = ground_light_at(ctx.world, [it.x, it.y, it.z]);
        let mut material = [1.0, 1.0, 1.0, (light as i32 as f32) / 255.0 * ctx.daylight];
        // 0x004b2565: glowing items are full bright.
        let t = it.item.item_type;
        if (t == 0xb && it.item.sub_type == 0x13) || t == 0x12 {
            material[3] = 1.0;
        }
        // 0x004b23ec..0x004b2b5f.
        let (world, k) = item_matrix(it, pos, model.size, gc.clock_ms, gc.camera.render_offset);
        if let Some(k) = k {
            // 0x004b29b3: `vec4 *= k` (all four components).
            material = [material[0] * k, k * material[1], material[2] * k, k * material[3]];
        }
        // 0x004b2b64..0x004b2bdc: the highlight.
        if [zone.0, zone.1, index] == aimed {
            material = [material[0] + 0.25, material[1] + 0.25, material[2] + 0.25, material[3] + 0.25];
        }
        // 0x004b2be1..0x004b2c1b.
        objects.push(ModelDraw { origin: 0x004b_2c1b, model: model.handle, world, material, alpha: 1.0, double_sided: false, shininess: item_shininess(&it.item), set_white: None, mirrored: false });
        // 0x004b2c4b.
        objects.extend(item_spirits(ctx, &it.item, &world, material));
    }
}

/// `0x004c7be0(item)`: the shininess of an item's draws, 1.0 for materials 1, 0xb, 0xc and
/// 0x16 (the metals), else 0.
pub fn item_shininess(item: &Item) -> f32 {
    if matches!(item.material, 1 | 0xb | 0xc | 0x16) { 1.0 } else { 0.0 }
}

/// `__ftol2` of a double (truncation toward zero).
#[inline]
fn ftol_f64(v: f64) -> i64 {
    v as i64
}

/// `0x004b22d6..0x004b2336`: the bob of a dropped item, fixed point:
/// `fixed64FromFloat((1 - ((t - 250) / 250)²) · 5)` (`0x0042c580`: `ftol(v · 65536)`).
pub fn bob_offset(t: i32) -> i64 {
    let a = (t.wrapping_sub(250) as f32) / 250.0;
    let v = (1.0 - a * a) * 5.0;
    ftol(v * 65536.0)
}

/// `0x004b23ec..0x004b2b5f`: the world matrix of a ground item at `pos` (already bobbed), and
/// the factor the material is multiplied by (coins only).
///
/// `M = T(render pos) · S(scale)`, then per item type (the switch at `0x004b2589`, table
/// `0x004bbba4`/`0x004bbbb4`):
///
/// - 3, 4 (weapons, armour; `0x004b28b6`): `Rz(rotation) · T(0, 0, sx/2) · Ry(90) ·
///   T(-sx/2, -sy/2, -sz/2)` (laid down);
/// - 0xc, 0xd (coins; `0x004b2943`): as 3/4 with rotation `clock · 0.2 + rotation`; the
///   material is scaled by `(cos(2·clock·0.2°)·0.5 + 0.5)² + 1` (a glint);
/// - 0x19 (`0x004b280b`): `Rz(clock · 0.02) · T(0, 0, sz) · Rx(45) · Ry(45) ·
///   T(-sx/2, -sy/2, -sz/2)` (a spinning, tilted cube);
/// - others (`0x004b29df`): `Rz(rotation) · T(-sx/2, -sy/2, 0)`;
///
/// then, while the countdown `+0x13c` is non-zero, a tumble about the model's centre:
/// `T(sx/2, sy/2, sz/2) · Rx(t) · Ry(0.7 t) · T(-sx/2, -sy/2, -sz/2)`.
/// (Factors read left to right as applied to the model: each is `M = F · M`.)
pub fn item_matrix(it: &GroundItem, pos: [i64; 3], size: [i32; 3], clock: i32, render_offset: [i64; 2]) -> (D3dMatrix, Option<f32>) {
    let [sx, sy, sz] = size;
    let mut m = identity();
    // 0x004b23f8..0x004b24a6.
    pre_translate(&mut m, render_translation(pos, render_offset));
    // 0x004b24ce.
    pre_scale(&mut m, [it.f134, it.f134, it.f134]);
    let mut k = None;
    // 0x004b2589: `switch (type - 3)`.
    let zoff = match it.item.item_type {
        // 0x004b28b6.
        3 | 4 => {
            lay_down(&mut m, it.rotation, sx);
            (sz as f32) * -0.5
        }
        // 0x004b2943.
        0xc | 0xd => {
            let a = (clock.wrapping_add(clock) as f32) * 0.2;
            let r = ((a as f64) * std::f64::consts::PI / 180.0) as f32;
            // 0x004b297c: `0x0040e420` = cos of the float widened.
            let c = cosf(r);
            let s = c * 0.5 + 0.5;
            k = Some(s * s + 1.0);
            // 0x004b29b8.
            let rot = (clock as f32) * 0.2 + it.rotation;
            lay_down(&mut m, rot, sx);
            (sz as f32) * -0.5
        }
        // 0x004b280b.
        0x19 => {
            pre_rotate_z(&mut m, (clock as f32) * 0.02);
            pre_translate(&mut m, [0.0, 0.0, sz as f32]);
            pre_rotate_x(&mut m, 45.0);
            pre_rotate_y(&mut m, 45.0);
            (sz as f32) * -0.5
        }
        // 0x004b29df.
        _ => {
            pre_rotate_z(&mut m, it.rotation);
            0.0
        }
    };
    // 0x004b2a01.
    pre_translate(&mut m, [(sx as f32) * -0.5, (sy as f32) * -0.5, zoff]);
    // 0x004b2a49: the tumble.
    if it.f13c != 0 {
        pre_translate(&mut m, [(sx as f32) * 0.5, (sy as f32) * 0.5, (sz as f32) * 0.5]);
        pre_rotate_x(&mut m, it.f13c as f32);
        pre_rotate_y(&mut m, (it.f13c as f32) * 0.7);
        pre_translate(&mut m, [(sx as f32) * -0.5, (sy as f32) * -0.5, (sz as f32) * -0.5]);
    }
    (m, k)
}

/// `0x004b28be..0x004b291b`: `Rz(rotation) · T(0, 0, sx/2) · Ry(90)`.
fn lay_down(m: &mut D3dMatrix, rotation: f32, sx: i32) {
    pre_rotate_z(m, rotation);
    pre_translate(m, [0.0, 0.0, (sx as f32) * 0.5]);
    pre_rotate_y(m, 90.0);
}

/// `0x004243d0`: `M = Rx(degrees) · M` (rows 1 and 2).
fn pre_rotate_x(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let a = m[1][j];
        m[1][j] = m[2][j] * s + a * c;
        m[2][j] = m[2][j] * c - a * s;
    }
}

/// `0x004244f0`: `M = Ry(degrees) · M` (rows 0 and 2).
fn pre_rotate_y(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let a = m[0][j];
        m[0][j] = a * c - m[2][j] * s;
        m[2][j] = m[2][j] * c + a * s;
    }
}

/// `0x00471b60(item, world, view, projection, material, 0.0)`: the spirit cubes of an item
/// (nothing for an empty item, type 0): per cube `i < +0x114`, the particle cube
/// (`GC+0x800730`) at `world · T(x, y, z)` (the cube's signed bytes `+0x14 + 8i ..`), material
/// `0x004c7250(cube material +0x17 + 8i, material, 0)`.
///
/// The shininess is `setShininess(0x004c7be0(item))` before, 0 after ([`item_shininess`]).
fn item_spirits(ctx: &SceneCtx, item: &Item, world: &D3dMatrix, material: [f32; 4]) -> Vec<ModelDraw> {
    let mut out = Vec::new();
    if item.item_type == 0 {
        return out;
    }
    let count = item.num_spirits as i32;
    let mut i = 0i32;
    while i < count {
        let o = (i as usize) * 8;
        let Some(c) = item.spirits.get(o..o + 4) else { break };
        let mut m = *world;
        pre_translate(&mut m, [(c[0] as i8) as f32, (c[1] as i8) as f32, (c[2] as i8) as f32]);
        out.push(ModelDraw {
            origin: 0x0047_1cfc,
            model: ctx.models.particle_cube(),
            world: m,
            material: material_color(c[3], material, 0.0),
            alpha: 1.0,
            double_sided: false,
            shininess: item_shininess(item),
            set_white: None,
            mirrored: false,
        });
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------
// ModelCache::modelForItem 0x004ec400
// ---------------------------------------------------------------------------------------

/// `ModelCache::modelForItem 0x004ec400`: the index of an item's model in the model vector
/// (`GameController+0x300`), `None` for items without one (types 0, 0x16, above 0x19 and the
/// sub types/materials the switches leave out). Implementers of
/// [`super::SceneModels::item_model`] call `models.model(item_model_index(item)?)`.
///
/// The original returns the `cube::Model*`: the constant cases read `*(begin + off)` after
/// checking `off < size in bytes` (here index `off / 4`, bounds-checked by `model`), the
/// computed ones call `0x004120c0(index)`. Armour indices use the modifier as unsigned
/// (`div`), weapons ([`weapon_model`]) as signed (`idiv`). Rarity is clamped to 4.
///
/// Case addresses (jump table `0x004ede04`, index `type - 1`) are cited per arm.
pub fn item_model_index(item: &Item) -> Option<i32> {
    let sub = item.sub_type;
    let material = item.material;
    let modifier = item.modifier;
    let r4 = if item.rarity > 3 { 4 } else { item.rarity as i32 };
    // `(uint)modifier % 5 * 5`.
    let m5 = ((modifier as u32) % 5 * 5) as i32;
    let at = |off: i32| Some(off / 4);
    // 0x004ec783: type 7 (also the default of type 4 material 6).
    let type7 = || match material {
        5 => at(0x201c),
        7 => at(0x2050),
        0xb => at(0x203c),
        0x12 => at(0x1324),
        0x13 => at(0x2510),
        0x14 => at(0x2528),
        0x16 => at(0x24dc),
        0x17 => at(0xa7c),
        0x19 => Some(m5 + 0x69e + r4),
        0x1a => Some(r4 + m5 + 0x568),
        0x1b => Some(r4 + m5 + 0x5cc),
        _ => Some(m5 + 0x47d + r4),
    };
    match item.item_type {
        // 0x004ec41e: consumables.
        1 => Some(consumable_model(sub)),
        // 0x004ec439.
        2 => at(0x25d4),
        // 0x004ec45b: weapons.
        3 => Some(weapon_model(sub, modifier, item.rarity as i32, material)),
        // 0x004ec483: chest armour (switch on material, table 0x004ede68/0x004ede9c).
        4 => match material {
            5 => at(0x2018),
            // 0x004ec5f4: material 6 by sub type.
            6 => match sub {
                0 => at(0x1334),
                1 => at(0x1338),
                2 => at(0x133c),
                3 => at(0x1340),
                4 => at(0x1344),
                5 => at(0x1348),
                6 => at(0x134c),
                _ => type7(),
            },
            7 => at(0x204c),
            0xb => at(0x2038),
            0x12 => at(0x1320),
            0x13 => at(0x2504),
            0x14 => at(0x251c),
            0x16 => at(0x24e0),
            0x17 => at(0xa78),
            0x19 => Some(m5 + 0x685 + r4),
            0x1a => Some(m5 + 0x54f + r4),
            0x1b => Some(m5 + 0x5b3 + r4),
            _ => Some(r4 + m5 + 0x464),
        },
        // 0x004eca51 (table 0x004edf18/0x004edf4c).
        5 => match material {
            1 => Some(m5 + 0x496 + r4),
            5 => at(0x2024),
            7 => at(0x2058),
            0xb => at(0x2044),
            0x12 => at(0x1328),
            0x13 => at(0x2500),
            0x14 => at(0x2518),
            0x16 => at(0x24e8),
            0x17 => at(0xa80),
            0x19 => Some(r4 + m5 + 0x6d0),
            0x1a => Some(m5 + 0x59a + r4),
            0x1b => Some(m5 + 0x5fe + r4),
            _ => at(0x1c),
        },
        // 0x004ecc6a (table 0x004edf68/0x004edf9c).
        6 => match material {
            1 => Some(m5 + 0x4af + r4),
            5 => at(0x2020),
            7 => at(0x2054),
            0xb => at(0x2040),
            0x12 => at(0x132c),
            0x13 => at(0x2508),
            0x14 => at(0x2520),
            0x16 => at(0x24e4),
            0x17 => at(0xa84),
            0x19 => Some(m5 + 0x6b7 + r4),
            0x1a => Some(m5 + 0x581 + r4),
            0x1b => Some(m5 + 0x5e5 + r4),
            _ => at(0x6c4),
        },
        // 0x004ec783 (table 0x004eded0/0x004edf00).
        7 => type7(),
        // 0x004ec981.
        8 => {
            if material != 0xc {
                Some(r4 + m5 + 0x61c)
            } else {
                Some(m5 + 0x63a + r4)
            }
        }
        // 0x004ec9e9.
        9 => {
            if material != 0xc {
                Some(m5 + 0x653 + r4)
            } else {
                Some(r4 + m5 + 0x66c)
            }
        }
        // 0x004edbb5.
        0xa => at(0x206c),
        // 0x004ed640: by sub type (table 0x004ee100).
        0xb => match sub {
            // 0x004ed81f: by material (table 0x004ee170/0x004ee198).
            0 => match material {
                1 => at(0x2484),
                2 => at(0x24f4),
                0xb => at(0x248c),
                0xc => at(0x2488),
                0xd => at(0x2490),
                0xe => at(0x2494),
                0xf => at(0x2498),
                0x10 => at(0x249c),
                0x11 => at(0x24f0),
                _ => None,
            },
            1 => at(0x24f4),
            2 => at(0x24f8),
            5 => at(0x2568),
            6 => at(0x24c0),
            7 => at(0x2514),
            8 => at(0x24d4),
            9 => at(0x24ec),
            // 0x004ed969 (table 0x004ee1ac/0x004ee1c0).
            0xa => match material {
                1 => at(0x24a0),
                2 => at(0x24ac),
                0xb => at(0x24a8),
                0xc => at(0x24a4),
                _ => None,
            },
            0xb => at(0x21c0),
            0xc => at(0x2554),
            0xd => at(0x2480),
            // Signed char compares: -0x7f, -0x7e, -0x7d are 0x81, 0x82, 0x83.
            0xe => match material {
                0x81 => at(0x24b8),
                0x82 => at(0x24b4),
                0x83 => at(0x24bc),
                _ => at(0x24b0),
            },
            0xf => at(0xd2c),
            0x10 => at(0x2470),
            0x11 => at(0x2478),
            0x12 => at(0x255c),
            0x13 => at(0xd30),
            0x14 => at(0x20c4),
            0x15 => at(0x2560),
            0x16 => at(0xd34),
            0x17 => at(0xd3c),
            0x18 => at(0xd38),
            0x19 => at(0xd40),
            0x1a => at(0x2558),
            0x1b => at(0x20ac),
            _ => None,
        },
        // 0x004ed571.
        0xc => match material {
            0xb => at(0x2098),
            0xc => at(0x2094),
            _ => at(0x2090),
        },
        // 0x004ed5da.
        0xd => at(0x24d4),
        // 0x004edcef: `v = rarity - 1`; `v >= 0 && v > 3` → 3; negative → 0.
        0xe => {
            let v = item.rarity as i32 - 1;
            let v = v.clamp(0, 3);
            Some(v + 0x90e)
        }
        // 0x004ed61e.
        0xf => at(0x24fc),
        // 0x004edd2d.
        0x10 => Some((modifier & 1) + 0x909),
        // 0x004edd46.
        0x11 => Some((modifier & 3) + 0x841),
        // 0x004edd5f: unsigned `div` by 3.
        0x12 => {
            let r = ((modifier as u32) % 3) as i32;
            if sub != 1 { Some(r + 0x845) } else { Some(r + 0x848) }
        }
        // 0x004ece88.
        0x13 => match sub {
            0x18 => at(0x21d0),
            0x19 => at(0x21c4),
            _ => at(0x2080),
        },
        // 0x004ecef1: by sub type (table 0x004edfb8/0x004ee078).
        0x14 => match sub {
            0x13 => at(0xdac),
            0x16 => at(0xd90),
            0x17 => at(0xd94),
            0x19 => at(0xdb4),
            0x1a => at(0xdbc),
            0x1b => at(0xd9c),
            0x1e => at(0xd6c),
            0x21 => at(0xd68),
            0x22 => at(0xd7c),
            0x23 => at(0xd64),
            0x24 => at(0xde4),
            0x25 => at(0xd54),
            0x26 => at(0xd58),
            0x27 => at(0xd60),
            0x28 => at(0xd5c),
            0x32 => at(0xd98),
            0x35 => at(0xd80),
            0x37 => at(0xdb0),
            0x38 => at(0xd84),
            0x39 => at(0xde8),
            0x3a => at(0xdec),
            0x3b => at(0xdf0),
            0x3c => at(0xdf4),
            0x3d => at(0xdf8),
            0x3e => at(0xdfc),
            0x3f => at(0xdd0),
            0x40 => at(0xdd8),
            0x41 => at(0xddc),
            0x42 => at(0xdd4),
            0x43 => at(0xda8),
            0x4a => at(0xde0),
            0x4b => at(0xdb8),
            0x56 => at(0xdc0),
            0x57 => at(0xd78),
            0x58 => at(0xe00),
            0x5a => at(0xd88),
            0x5b => at(0xd8c),
            0x5c => at(0xd70),
            0x5d => at(0xd74),
            0x62 => at(0xdc8),
            0x63 => at(0xdc4),
            0x66 => at(0x25a0),
            0x67 => at(0xe04),
            0x68 => at(0xda0),
            0x69 => at(0xda4),
            0x6a => at(0xdcc),
            0x97 => at(0xe08),
            _ => at(0x2478),
        },
        // 0x004edbd7: by sub type (table 0x004ee1cc).
        0x15 => match sub {
            3 => at(0x25d8),
            4 | 5 => at(0xd4c),
            6 => at(0x27d8),
            7 => at(0x984),
            8 => at(0x24ec),
            9 => at(0x256c),
            // 0x004edcd0: `(sub & 0x80000003) + 0x821` (sub is a zero-extended byte).
            _ => Some((sub as i32 & 3) + 0x821),
        },
        // 0x004edd9a.
        0x17 => {
            if sub == 1 { at(0x27d0) } else { at(0x27cc) }
        }
        // 0x004edddf.
        0x18 => at(0x2604),
        // 0x004ed5fc.
        0x19 => at(0x2818),
        // 0x004ed704: type 0x16, and types 0 and above 0x19 (`ja`).
        _ => None,
    }
}

/// `0x004ec370(sub)`: the model index of a consumable.
fn consumable_model(sub: u8) -> i32 {
    match sub {
        0 => 0x354,
        1 => 0x351,
        2 => 0x352,
        3 => 0x353,
        4 => 0x95b,
        5 => 0x82c,
        6 => 0x959,
        7 => 0x9f2,
        8 => 0x91f,
        9 => 0x91d,
        _ => 0x34b,
    }
}

/// `0x0051be60(sub, modifier, rarity, material)`: the model index of a weapon (`modifier % 11`
/// and `% 6` signed; rarity clamped to 4).
fn weapon_model(sub: u8, modifier: i32, rarity: i32, material: u8) -> i32 {
    let r = if rarity < 4 { rarity } else { 4 };
    let m11 = modifier % 0xb;
    match sub {
        0 => match material {
            5 => 0x80b,
            7 => 0x818,
            _ => r + 899 + m11 * 5,
        },
        1 => match material {
            5 => 0x80c,
            7 => 0x81a,
            _ => (m11 + 0x162) * 5 + r,
        },
        2 => match material {
            2 => r + 0x298,
            7 => 0x819,
            _ => r + 0x261 + m11 * 5,
        },
        3 => r + 0x3ba + m11 * 5,
        4 => r + 0x3f1 + m11 * 5,
        5 => (m11 + 0x178) * 5 + r,
        6 => r + 0x2a2 + m11 * 5,
        7 => r + 0x2d9 + m11 * 5,
        8 => r + 0x310 + m11 * 5,
        9 => 0x348,
        0xa => {
            if material == 5 {
                r + 0x25c
            } else {
                r + 0x1b2 + m11 * 5
            }
        }
        0xb => r + 0x1e9 + m11 * 5,
        0xc => {
            let m6 = (modifier % 6) * 5;
            if material != 0xb { r + 0x23e + m6 } else { r + 0x220 + m6 }
        }
        0xd => {
            if material == 2 {
                r + 0x45f
            } else {
                r + 0x428 + m11 * 5
            }
        }
        0xe => 0x347,
        0xf => match material {
            5 => 0x790,
            7 => 0x78f,
            _ => (m11 + 0x16d) * 5 + r,
        },
        0x10 => match material {
            5 => 0x7c9,
            7 => 0x7c8,
            0x12 => 0x7ca,
            _ => r + 0x791 + m11 * 5,
        },
        0x11 => match material {
            2 => 0x7cb,
            5 => 0x805,
            7 => 0x804,
            _ => r + 0x7cc + m11 * 5,
        },
        0x12 => 0x91b,
        0x13 => 0x803,
        0x14 => 0x34a,
        _ => 899,
    }
}

// ---------------------------------------------------------------------------------------
// The build-mode target marker
// ---------------------------------------------------------------------------------------

/// `0x00598840(World, key)`: `std::map<int, cube::Model*>` lookup in `World+0x800154` (the
/// build-mode block models), keyed by the local player's `entity+0x17d` (signed byte).
///
/// [`super::SceneModels::build_model`].
fn build_model(ctx: &SceneCtx, key: i32) -> Option<ModelInfo> {
    ctx.models.build_model(key)
}

/// `0x004b3338..0x004b35d4` (while `GC+0x800704`, build mode): the model `0x00598840`
/// ([`build_model`]) of the local player's `entity+0x17d` at the block the aim ray hits,
/// see [`target_marker_draw`].
pub fn target_marker(ctx: &SceneCtx) -> Vec<ModelDraw> {
    let gc = ctx.gc;
    let Some(e) = ctx.entities.get(&gc.player) else { return Vec::new() };
    // 0x004b3351: `movsx creature+0x18d`.
    let key = (e.0[0x17d] as i8) as i32;
    let Some(model) = build_model(ctx, key) else { return Vec::new() };
    vec![target_marker_draw(e, model, gc.clock_ms, gc.camera.render_offset)]
}

/// The marker draw of `0x004b336a..0x004b35cf`:
///
/// - block `B = (pos + fromFloat(ray hit entity+0x150) + fromFloat(0.5, 0.5, 0.5)) / 65536`
///   (`vec3i::fromFixed 0x00450f60`, `__alldiv`: truncation);
/// - `M = T(fromFixed(fromBlock(B) + (offset x + f, offset y + f, f)))` with `f =
///   ftol(0.01 · 65536)` (`0x004122e0`, a small lift against z-fighting) `· Rz(entity+0x17c ·
///   90) · T(-(sx / 2), -(sy / 2), 0)` (integer halves);
/// - material `(1, 1, 1, (cos(clock · 0.005) + 1) · 0.25 + 0.5)` (a pulse), alpha 1 (set at
///   `0x004b32e8`).
pub fn target_marker_draw(e: &cw_net::EntityData, model: ModelInfo, clock: i32, render_offset: [i64; 2]) -> ModelDraw {
    let f32_at = |o: usize| f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap());
    // 0x004b336a: `fromFloat(0.5, 0.5, 0.5)`.
    let half = fixed_from_float([0.5, 0.5, 0.5]);
    // 0x004b339d: `fromFloat(creature+0x160)` (entity+0x150, the ray hit).
    let hit = fixed_from_float([f32_at(0x150), f32_at(0x154), f32_at(0x158)]);
    // 0x004b33de: `(pos + hit) + half`, then `/ 65536` per axis.
    let p = add3(add3(entity_pos(e), hit), half);
    let block = [(p[0] / 65536) as i32, (p[1] / 65536) as i32, (p[2] / 65536) as i32];
    // 0x004b3403..0x004b3461: the render offset lifted by 0.01 blocks.
    let f = ftol(0.01 * 65536.0);
    let off = [render_offset[0].wrapping_add(f), render_offset[1].wrapping_add(f), f];
    // 0x004b3477: `vec3i64::fromBlock` (`<< 16`) + offset.
    let b = [(block[0] as i64) << 16, (block[1] as i64) << 16, (block[2] as i64) << 16];
    let mut m = identity();
    // 0x004b34a6.
    pre_translate(&mut m, float_from_fixed(add3(b, off)));
    // 0x004b34d8: `movsx creature+0x18c` · 90.
    pre_rotate_z(&mut m, (((e.0[0x17c] as i8) as i32) as f32) * 90.0);
    // 0x004b34e1..0x004b3526: integer halves, negated.
    let [sx, sy, _] = model.size;
    pre_translate(&mut m, [(-(sx / 2)) as f32, (-(sy / 2)) as f32, 0.0]);
    // 0x004b354e..0x004b35c8.
    let c = cosf((clock as f32) * 0.005);
    let a = (c + 1.0) * 0.25 + 0.5;
    ModelDraw { origin: 0x004b_35cf, model: model.handle, world: m, material: [1.0, 1.0, 1.0, a], alpha: 1.0, double_sided: false, shininess: 0.0, set_white: None, mirrored: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(t: u8, sub: u8, material: u8, rarity: u8, modifier: i32) -> Item {
        let mut it = Item::NEW;
        it.item_type = t;
        it.sub_type = sub;
        it.material = material;
        it.rarity = rarity;
        it.modifier = modifier;
        it
    }

    /// Values read off the asm: type 2 reads `*(begin + 0x25d4)` (0x004ec44f) = index 0x975;
    /// type 1 sub 0 is `0x004ec370(0)` = 0x354; type 0x10 is `(modifier & 1) + 0x909`; armour
    /// type 4 iron-like default `rarity + (modifier % 5) * 5 + 0x464`; weapon sub 0 material 1
    /// rarity 2 modifier 12: `2 + 899 + (12 % 11) * 5`; type 0x16 has no model.
    #[test]
    fn item_model_index_known_items() {
        assert_eq!(item_model_index(&item(2, 0, 0, 0, 0)), Some(0x975));
        assert_eq!(item_model_index(&item(1, 0, 0, 0, 0)), Some(0x354));
        assert_eq!(item_model_index(&item(0x10, 0, 0, 0, 3)), Some(0x90a));
        assert_eq!(item_model_index(&item(4, 0, 1, 7, 13)), Some(4 + 15 + 0x464));
        assert_eq!(item_model_index(&item(3, 0, 1, 2, 12)), Some(2 + 899 + 5));
        assert_eq!(item_model_index(&item(0x16, 0, 0, 0, 0)), None);
        assert_eq!(item_model_index(&item(0, 0, 0, 0, 0)), None);
        assert_eq!(item_model_index(&item(0x1a, 0, 0, 0, 0)), None);
        // 0x004edcef: rarity 0 → 0x90e, rarity 9 → 0x911.
        assert_eq!(item_model_index(&item(0xe, 0, 0, 0, 0)), Some(0x90e));
        assert_eq!(item_model_index(&item(0xe, 0, 0, 9, 0)), Some(0x911));
    }

    /// Two independent transcriptions of 0x004ec400 (this one from the Ghidra/capstone listing,
    /// `crate::interact`'s from the update's port) agree everywhere.
    #[test]
    fn item_model_index_matches_interact() {
        for t in 0..=0x1bu8 {
            for sub in 0..=255u8 {
                for material in 0..=255u8 {
                    let it = item(t, sub, material, 2, 7);
                    let a = item_model_index(&it);
                    let b = crate::interact::item_model_index(&it.to_bytes()).map(|v| v as i32);
                    assert_eq!(a, b, "type {t:#x} sub {sub:#x} material {material:#x}");
                }
            }
        }
        for t in 0..=0x1bu8 {
            for sub in 0..0x14u8 {
                for material in [0u8, 1, 2, 5, 6, 7, 0xb, 0xc, 0x12, 0x19, 0x1a, 0x1b] {
                    for rarity in 0..7u8 {
                        for modifier in [0, 1, 4, 5, 10, 11, 17, -1, -12, i32::MIN, i32::MAX] {
                            let it = item(t, sub, material, rarity, modifier);
                            let a = item_model_index(&it);
                            let b = crate::interact::item_model_index(&it.to_bytes()).map(|v| v as i32);
                            assert_eq!(a, b, "type {t:#x} sub {sub:#x} mat {material:#x} r {rarity} m {modifier}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn bob_peaks_at_250() {
        assert_eq!(bob_offset(250), 5 * 65536);
        // ((125 - 250) / 250)² = 0.25 → 3.75 blocks.
        assert_eq!(bob_offset(125), 245_760);
        assert_eq!(bob_offset(500), 0);
    }

    fn ground(t: u8) -> GroundItem {
        let mut g = GroundItem::NEW;
        g.item.item_type = t;
        g.f134 = 1.0;
        g
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    /// Default type: centred in x and y on the item's position, standing on it; a quarter turn
    /// of `+0x130` maps the model's +x to +y.
    #[test]
    fn default_item_is_centred_and_turned() {
        let mut g = ground(0xb);
        let pos = [10 << 16, 20 << 16, 30 << 16];
        let (m, k) = item_matrix(&g, pos, [2, 4, 6], 0, [0, 0]);
        assert!(k.is_none());
        assert!(close(transform_point(&m, [1.0, 2.0, 0.0]), [10.0, 20.0, 30.0]));
        g.rotation = 90.0;
        let (m, _) = item_matrix(&g, pos, [2, 4, 6], 0, [0, 0]);
        // Model centre + 1 in x → world + 1 in y.
        assert!(close(transform_point(&m, [2.0, 2.0, 0.0]), [10.0, 21.0, 30.0]));
    }

    /// Weapons are laid down: the model's +z axis ends up along world -x (Ry(90)), the model's
    /// centre `sx/2` above the ground point.
    #[test]
    fn weapon_is_laid_down() {
        let g = ground(3);
        let (m, _) = item_matrix(&g, [0, 0, 0], [2, 2, 10], 0, [0, 0]);
        let c = transform_point(&m, [1.0, 1.0, 5.0]);
        assert!(close(c, [0.0, 0.0, 1.0]), "{c:?}");
        let tip = transform_point(&m, [1.0, 1.0, 10.0]);
        assert!((tip[2] - 1.0).abs() < 1e-4 && (tip[0].abs() - 5.0).abs() < 1e-4, "{tip:?}");
    }

    /// Coins glint: at clock 0 the factor is `(1·0.5 + 0.5)² + 1 = 2`.
    #[test]
    fn coin_glint_factor() {
        let g = ground(0xc);
        let (_, k) = item_matrix(&g, [0, 0, 0], [2, 2, 2], 0, [0, 0]);
        assert_eq!(k, Some(2.0));
        // clock 450: 2·450·0.2 = 180° → cos = -1 → (0)² + 1.
        let (_, k) = item_matrix(&g, [0, 0, 0], [2, 2, 2], 450, [0, 0]);
        assert!((k.unwrap() - 1.0).abs() < 1e-6);
    }

    /// The tumble keeps the model's centre fixed.
    #[test]
    fn tumble_is_about_the_centre() {
        let mut g = ground(0xb);
        let (m0, _) = item_matrix(&g, [0, 0, 0], [2, 4, 6], 0, [0, 0]);
        g.f13c = 37;
        let (m1, _) = item_matrix(&g, [0, 0, 0], [2, 4, 6], 0, [0, 0]);
        assert!(close(transform_point(&m0, [1.0, 2.0, 3.0]), transform_point(&m1, [1.0, 2.0, 3.0])));
    }

    #[test]
    fn marker_sits_on_the_hit_block() {
        let mut e = cw_net::EntityData::ZERO;
        e.0[0..8].copy_from_slice(&(5i64 << 16).to_le_bytes());
        e.0[8..16].copy_from_slice(&(6i64 << 16).to_le_bytes());
        e.0[16..24].copy_from_slice(&(7i64 << 16).to_le_bytes());
        e.0[0x150..0x154].copy_from_slice(&2.2f32.to_le_bytes());
        let model = ModelInfo { handle: 9, size: [3, 3, 1] };
        let d = target_marker_draw(&e, model, 0, [0, 0]);
        // Block (7.7 → 7, 6.5 → 6, 7.5 → 7), lifted 655/65536; minus (1, 1, 0).
        let o = transform_point(&d.world, [0.0; 3]);
        let lift = 655.0 / 65536.0;
        assert!(close(o, [6.0 + lift, 5.0 + lift, 7.0 + lift]), "{o:?}");
        assert_eq!(d.material, [1.0, 1.0, 1.0, 1.0]);
    }
}
