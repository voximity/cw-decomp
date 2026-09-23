//! Ground decals (`Cube.exe 0x004b8bc7..0x004b9993`) and weapon ribbons (`0x004b9993..0x004ba13e`)
//! of `GameController::render`.
//!
//! # Ground decals (the "shadow" pass)
//!
//! Fixed function, FVF `0x142` (XYZ|DIFFUSE|TEX1, stride 0x18), texture `GC+0x8006dc`,
//! `ZWRITEENABLE = 0`, sampler 0 `ADDRESSU = ADDRESSV = 4` (**`D3DTADDRESS_BORDER`**, not clamp:
//! the cells at the edge of a decal reach past the texture and read the border colour),
//! projection `esp+0x74c`, view `esp+0x79c`, and one `SetTransform(WORLD)` per quad (a
//! translation to the quad's block column in render space). Each quad is
//! `DrawPrimitiveUP(TRIANGLEFAN, 2, v, 0x18)`. Two sources, in this order:
//!
//! 1. `0x004b8c9a..0x004b92d5`: projectiles of kind 3 (the lingering areas): a disc of radius
//!    `+0x4c` projected onto the ground, one quad per block column within `trunc(radius + 1)`,
//!    tinted orange (`+0x64 == 1`) or blue, pulsing with the age.
//! 2. `0x004b92d5..0x004b9980`: creature blob shadows: a 5x5 block patch under the creature
//!    (under its render position moved by the pose's root offset), texture spread over 3 blocks,
//!    opaque black vertices. Skipped while one of the widgets `GC+0x800874/888/88c/894` is
//!    visible.
//!
//! Every quad lies on the top face of the first solid block found going down from the centre
//! block's z (at most 100 steps below it; a column with none gets no quad).
//!
//! # Ribbons
//!
//! `SetTexture(0, NULL)`, `CULLMODE = NONE`, `LIGHTING = FALSE`, FVF `0x42` (stride 0x10),
//! world = identity, view `esp+0x79c` (set per creature, the same matrix); `ZWRITEENABLE` stays
//! 0 from the decals (it is set back to 1 at `0x004ba171`). Per melee creature in its attack
//! animation: `DrawPrimitiveUP(TRIANGLESTRIP, 30, v, 0x10)` of the right weapon's two trails
//! (32 vertices), and a second strip for the left weapon when the entity's slot 6 holds a weapon.
//! White, the alpha rising to the middle of the trail and falling to its ends.

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_render::frame::FixedVertex;

use super::math::*;
use super::{SceneCtx, entity_pos, equip};

/// What the two passes read of the pose object at `creature+0x1498` beyond [`super::PoseView`].
///
/// The public entry points take them from `SceneState::poses` ([`super::PoseView`]).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PoseFx {
    /// `pose+0xac` (`creature+0x1544`): the root offset (`PoseState::root_off`), rotated by the
    /// entity yaw for the blob shadow's centre.
    pub root_off: [f32; 3],
    /// `pose+0x56c`, `+0x62c`, `+0x6ec`, `+0x7ac` (`creature+0x1a04`, `+0x1ac4`, `+0x1b84`,
    /// `+0x1c44`): `PoseState::trails` (right tip, right base, left tip, left base), 16 points
    /// each, newest first, stored relative to `render_pos + render offset` in x and y.
    pub trails: [[[f32; 3]; 16]; 4],
}

// ---------------------------------------------------------------------------------------
// Entity fields (creature offset = entity offset + 0x10)
// ---------------------------------------------------------------------------------------

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

/// `creature+0x16c`: HP.
const HP: usize = 0x15c;
/// `creature+0x80`: the scale vector (3 × f32).
const SCALE: usize = 0x70;
/// `creature+0x28`: the orientation (3 × f32); `[2]` is the yaw.
const ORIENTATION: usize = 0x18;
/// `creature+0x6c`: time in the current mode, ms.
const MODE_TIME: usize = 0x5c;
/// `creature+0x70`: the hit counter.
const HIT_COUNTER: usize = 0x60;

/// `creature+0x1350`: the render position (the entity position without a state).
fn render_pos(ctx: &SceneCtx, id: i64, e: &EntityData) -> [i64; 3] {
    ctx.states.get(&id).map_or_else(|| entity_pos(e), |s| s.riding.render_pos)
}

/// `Cube.exe 0x00444520` (`cube::Creature::isRanged`, the same test as `Server.exe
/// 0x0040f5a0`, which `cw_sim::util::is_ranged` ports but keeps crate-private): class 3, types
/// 0x75/0x56/0x68, or a weapon of sub type 6, 7, 8, 10, 11 or 12 in slot 7.
fn is_ranged(e: &EntityData) -> bool {
    let w = equip(e, 7);
    if e.0[0x130] == 3 || matches!(i32_at(e, 0x54), 0x75 | 0x56) || (w[0] == 3 && matches!(w[1], 10..=12)) || i32_at(e, 0x54) == 0x68 {
        return true;
    }
    w[0] == 3 && matches!(w[1], 6 | 7 | 8 | 10 | 11)
}

// ---------------------------------------------------------------------------------------
// Ground decals
// ---------------------------------------------------------------------------------------

/// `vec3i::fromFixed 0x00450f60`: `__alldiv(v, 0x10000)` per axis (truncating toward zero).
fn block_of(p: [i64; 3]) -> [i32; 3] {
    [(p[0] / 65536) as i32, (p[1] / 65536) as i32, (p[2] / 65536) as i32]
}

/// `blockIsSolid 0x0043b480`: type (`b[3] & 0x1f`) neither 0 (air) nor 2 (water).
fn is_solid(b: [u8; 4]) -> bool {
    let t = b[3] & 0x1f;
    t != 0 && t != 2
}

/// The ground search of both decal loops (`0x004b8e50..0x004b8ece`, `0x004b94d0..0x004b9555`):
/// `World::getBlock 0x0042f7e0(x, y, z, 0)` from `z` down until a solid block, giving up
/// after 100 steps (`++n > 100`). Returns the z above the solid block.
fn ground_top(ctx: &SceneCtx, x: i32, y: i32, z: i32) -> Option<i32> {
    let mut z = z;
    let mut n = 0;
    while !is_solid(cw_render::light::get_block(ctx.world, x, y, z)) {
        n += 1;
        z = z.wrapping_sub(1);
        if n > 100 {
            return None;
        }
    }
    Some(z.wrapping_add(1))
}

/// `int64FromBlock 0x00412080` + render offset (`0x004122c0`), to float (`0x004120f0`): the
/// render-space coordinate of a block boundary.
fn block_render(b: i32, offset: i64) -> f32 {
    ((i64::from(b) << 16).wrapping_add(offset) as f32) * INV_FIXED
}

/// One decal quad: the fan `(x, y)`, `(x+1, y)`, `(x+1, y+1)`, `(x, y+1)` at height `z` (block
/// units relative to the column `b`), moved by the quad's world matrix `0x00424a60(bx, by, dz)`
/// (`M = T · I`, so the fixed-function product is `p + t`).
#[allow(clippy::too_many_arguments)]
fn quad(t: [f32; 3], dx: i32, dy: i32, z: i32, color: u32, u: [f32; 2], v: [f32; 2]) -> [FixedVertex; 4] {
    let p = |x: i32, y: i32| [x as f32 + t[0], y as f32 + t[1], z as f32 + t[2]];
    [
        FixedVertex { position: p(dx, dy), color_argb: color, uv: [u[0], v[0]] },
        FixedVertex { position: p(dx.wrapping_add(1), dy), color_argb: color, uv: [u[1], v[0]] },
        FixedVertex { position: p(dx.wrapping_add(1), dy.wrapping_add(1)), color_argb: color, uv: [u[1], v[1]] },
        FixedVertex { position: p(dx, dy.wrapping_add(1)), color_argb: color, uv: [u[0], v[1]] },
    ]
}

/// `0x004b8c9a..0x004b92d5`: the ground disc of every kind-3 projectile (list `GC+0x2f8`, in
/// order).
fn projectile_decals(ctx: &SceneCtx, out: &mut Vec<[FixedVertex; 4]>) {
    let off = ctx.gc.camera.render_offset;
    for p in ctx.projectiles {
        // 0x004b8ce2: kind 3 only; `0x0047f760(pos, radius, esp+0x4c)`.
        if p.kind != 3 {
            continue;
        }
        if !cw_render::passes::sphere_visible(&ctx.planes, &ctx.gc.camera, p.pos, p.radius, ctx.static_far) {
            continue;
        }
        let b = block_of(p.pos);
        let size = p.radius;
        // 0x004b8d42: 1 / (size · 2).
        let inv = 1.0f32 / (size * 2.0);
        // 0x004b8d60: a pulse of ±50 from the age (`0x0040e420` = (float)cos((double)x)).
        let c = ((cosf((p.age as f32) * 0.01)) * -50.0) as i32;
        let mut color = ((0xffff_ff96u32.wrapping_sub(c as u32) & 0xff) << 8) | 0xff00_00ff;
        if p.sub == 1 {
            color = ((50u32.wrapping_sub(c as u32)) << 8) & 0xffff_ff00 | 0xffff_0000;
        }
        // 0x004b8dc1: r = trunc(size + 1); x outer, y inner, both -r..=r.
        let r = (size + 1.0) as i32;
        let mut dx = r.wrapping_neg();
        while dx <= r {
            let mut dy = r.wrapping_neg();
            while dy <= r {
                if let Some(zt) = ground_top(ctx, b[0].wrapping_add(dx), b[1].wrapping_add(dy), b[2]) {
                    // 0x004b8ece: u, v of the cell corner: (size + d - frac) / (2·size).
                    let fx = (p.pos[0].wrapping_sub(i64::from(b[0]) << 16) as f32) * INV_FIXED;
                    let u0 = ((size + dx as f32) - fx) * inv;
                    let fy = (p.pos[1].wrapping_sub(i64::from(b[1]) << 16) as f32) * INV_FIXED;
                    let v0 = ((size + dy as f32) - fy) * inv;
                    let u1 = u0 + inv;
                    let v1 = v0 + inv;
                    // 0x004b918a: world = T(bx + offset x, by + offset y, 0.01).
                    let t = [block_render(b[0], off[0]), block_render(b[1], off[1]), 0.01];
                    // 0x004b9234: DrawPrimitiveUP(TRIANGLEFAN, 2).
                    out.push(quad(t, dx, dy, zt, color, [u0, u1], [v0, v1]));
                }
                dy = dy.wrapping_add(1);
            }
            dx = dx.wrapping_add(1);
        }
    }
}

/// `0x004b92d5..0x004b9980`: the blob shadow of every living, visible creature (the creature
/// map `World+4`, id order). `pose_of` gives the pose fields the original reads at
/// `creature+0x1544`.
fn creature_shadows(ctx: &SceneCtx, pose_of: &dyn Fn(i64) -> PoseFx, out: &mut Vec<[FixedVertex; 4]>) {
    // 0x004b92d5: nothing while one of the four widgets is open.
    if ctx.gc.gui.hides_creatures() {
        return;
    }
    let off = ctx.gc.camera.render_offset;
    for (&id, e) in ctx.entities {
        // 0x004b9380: `0 >= hp` skips (NaN passes).
        if 0.0 >= f32_at(e, HP) {
            continue;
        }
        let rp = render_pos(ctx, id, e);
        // 0x004b9390: `0x0047f760(render_pos, scale.z, GC+0x1d0·0.3·0.5)`.
        let dist = ctx.gc.view_distance * 0.3 * 0.5;
        if !cw_render::passes::sphere_visible(&ctx.planes, &ctx.gc.camera, rp, f32_at(e, SCALE + 8), dist) {
            continue;
        }
        // 0x004b93d5: a white clear shows only the player's.
        if ctx.gc.white_clear && id != ctx.gc.player {
            continue;
        }
        // 0x004b93ec: centre = render_pos + fixed(Rz(yaw) · root offset).
        let mut m = identity();
        pre_rotate_z(&mut m, f32_at(e, ORIENTATION + 8));
        let v = transform_vector(&m, pose_of(id).root_off);
        let b = block_of(add3(rp, fixed_from_float(v)));
        // 0x004b947a: x outer, y inner, both -2..=2.
        for dx in -2i32..=2 {
            for dy in -2i32..=2 {
                let Some(zt) = ground_top(ctx, b[0].wrapping_add(dx), b[1].wrapping_add(dy), b[2]) else {
                    continue;
                };
                // 0x004b9555: frac = (render_pos + ftol(v·65536) - block) / 65536 (`0x004122e0`,
                // `0x0043abc0`, `0x004120f0`); u = (d + 1.5 - frac) / 3.
                let a = dx as f32 + 1.5;
                let fx = (rp[0].wrapping_add(ftol(v[0] * 65536.0)).wrapping_sub(i64::from(b[0]) << 16) as f32) * INV_FIXED;
                let u0 = (a - fx) * 0.333_333_34;
                let fy = (rp[1].wrapping_add(ftol(v[1] * 65536.0)).wrapping_sub(i64::from(b[1]) << 16) as f32) * INV_FIXED;
                let v0 = ((dy as f32 + 1.5) - fy) * 0.333_333_34;
                let u1 = u0 + 0.333_333_34;
                let v1 = v0 + 0.333_333_34;
                // 0x004b9851: world = T(bx + offset x, by + offset y, 0.02).
                let t = [block_render(b[0], off[0]), block_render(b[1], off[1]), 0.02];
                // 0x004b98f9: DrawPrimitiveUP(TRIANGLEFAN, 2), opaque black.
                out.push(quad(t, dx, dy, zt, 0xff00_0000, [u0, u1], [v0, v1]));
            }
        }
    }
}

/// The pose fields of a creature from `SceneState::poses` (zero for a creature without one).
fn pose_fx(ctx: &SceneCtx, id: i64) -> PoseFx {
    ctx.gc.poses.get(&id).map_or_else(PoseFx::default, |p| PoseFx { root_off: p.root_off, trails: p.trails })
}

/// `0x004b8bc7..0x004b9993`: the textured ground decals (projectile areas, then creature blob
/// shadows), each a fan of 4 vertices already in render space (the per-quad world translation
/// folded in: `build_frame` draws them with world = identity).
pub fn shadows(ctx: &SceneCtx) -> Vec<[FixedVertex; 4]> {
    shadows_with(ctx, &|id| pose_fx(ctx, id))
}

/// [`shadows`] with the pose fields supplied (see [`PoseFx`]).
pub fn shadows_with(ctx: &SceneCtx, pose_of: &dyn Fn(i64) -> PoseFx) -> Vec<[FixedVertex; 4]> {
    let mut out = Vec::new();
    projectile_decals(ctx, &mut out);
    creature_shadows(ctx, pose_of, &mut out);
    out
}

// ---------------------------------------------------------------------------------------
// Ribbons
// ---------------------------------------------------------------------------------------

/// The per-point alpha weight (`0x004b9e11`): `t = i / 15`, `x = ((1 - t) - 0.5) · 2`,
/// `(1 - x²)²` (0 at both ends, 1 in the middle).
fn ribbon_weight(i: i32) -> f32 {
    let t = (i as f32) / 15.0;
    let mut x = 1.0f32 - t;
    x -= 0.5;
    x *= 2.0;
    x *= x;
    let k = 1.0f32 - x;
    k * k
}

/// `(a << 24) | (r << 16) | (g << 8) | b` built as the original does (`shl 8; or`).
fn argb(a: i32, rgb: [u8; 3]) -> u32 {
    let mut c = a as u32;
    c = (c << 8) | u32::from(rgb[0]);
    c = (c << 8) | u32::from(rgb[1]);
    c = (c << 8) | u32::from(rgb[2]);
    c
}

/// One strip (`0x004b9e11..0x004b9f6f`, `0x004b9fa0..0x004ba0fe`): for `i` in 0..16, vertex
/// `2i` = `base[i] - t` with alpha `trunc(k·50)` and colour `c1`, vertex `2i + 1` =
/// `tip[i] - t` with alpha `trunc(k·tip_alpha)` and colour `c2` (`0x004121c0` is `a - b`).
fn strip(base: &[[f32; 3]; 16], tip: &[[f32; 3]; 16], t: [f32; 3], tip_alpha: f32, c1: [u8; 3], c2: [u8; 3]) -> Vec<FixedVertex> {
    let mut v = Vec::with_capacity(32);
    for i in 0..16 {
        let k = ribbon_weight(i as i32);
        let sub = |p: [f32; 3]| [p[0] - t[0], p[1] - t[1], p[2] - t[2]];
        v.push(FixedVertex { position: sub(base[i]), color_argb: argb((k * 50.0) as i32, c1), uv: [0.0; 2] });
        v.push(FixedVertex { position: sub(tip[i]), color_argb: argb((k * tip_alpha) as i32, c2), uv: [0.0; 2] });
    }
    v
}

/// `0x004b9993..0x004ba13e`: the weapon ribbons, each a triangle strip of 32 vertices (30
/// primitives) in render space. Creatures in the map's order: alive, visible
/// (`0x0047f760(render_pos, scale.y, GC+0x1d0·0.3)`), not ranged (`0x00444520`) and with the
/// mode time at most the mode's whole animation (`skillTotalTime 0x0043d1a0`): the right
/// ribbon, then the left one when entity slot 6 (`creature+0x990`) holds a weapon (type 3).
pub fn ribbons(ctx: &SceneCtx) -> Vec<Vec<FixedVertex>> {
    ribbons_with(ctx, &|id| pose_fx(ctx, id))
}

/// [`ribbons`] with the pose fields supplied (see [`PoseFx`]).
pub fn ribbons_with(ctx: &SceneCtx, pose_of: &dyn Fn(i64) -> PoseFx) -> Vec<Vec<FixedVertex>> {
    let mut out = Vec::new();
    let off = ctx.gc.camera.render_offset;
    for (&id, e) in ctx.entities {
        if 0.0 >= f32_at(e, HP) {
            continue;
        }
        let rp = render_pos(ctx, id, e);
        if !cw_render::passes::sphere_visible(&ctx.planes, &ctx.gc.camera, rp, f32_at(e, SCALE + 4), ctx.gc.view_distance * 0.3) {
            continue;
        }
        // 0x004b9a76.
        if is_ranged(e) {
            continue;
        }
        // 0x004b9a85: `mode_time > skillTotalTime` skips.
        let (guard, haste) = guard_haste(ctx.states, id);
        if i32_at(e, MODE_TIME) > cw_sim::skills::skill_total_time(e, guard, haste) {
            continue;
        }
        // 0x004b9af2: t = (-(render_pos.x) - offset.x, -(render_pos.y) - offset.y, 0) / 65536
        // (`0x00412260` negate, `0x00412200` subtract, `0x004120f0`).
        let t = [
            (rp[0].wrapping_neg().wrapping_sub(off[0]) as f32) * INV_FIXED,
            (rp[1].wrapping_neg().wrapping_sub(off[1]) as f32) * INV_FIXED,
            0.0,
        ];
        // 0x004b9b84..0x004b9d74: two colours are mixed from the hit counter
        // (s = min(entity+0x60 · 0.1, 1): c1 = (200,100,255)·s + (100,255,255)·(1-s),
        // c2 = (255,50,100)·s + (255,255,255)·(1-s)) and then both overwritten with
        // (255,255,255) before use; the mix has no other effect.
        let _hit_counter = i32_at(e, HIT_COUNTER);
        let c1 = [255u8; 3];
        let c2 = [255u8; 3];
        let pose = pose_of(id);
        // 0x004b9e11..0x004b9f6f: right weapon, base `pose+0x62c`, tip `pose+0x56c` (alpha 200).
        out.push(strip(&pose.trails[1], &pose.trails[0], t, 200.0, c1, c2));
        // 0x004b9f71: left weapon when slot 6 holds a weapon: base `pose+0x7ac`, tip
        // `pose+0x6ec` (alpha 255).
        if equip(e, 6)[0] == 3 {
            out.push(strip(&pose.trails[3], &pose.trails[2], t, 255.0, c1, c2));
        }
    }
    out
}

/// `cw_sim::util::guard_haste`: the guard (`+0x1190`) and haste buff (type 0xc) the skill
/// timings read.
fn guard_haste(states: &BTreeMap<i64, cw_sim::combat::CreatureState>, id: i64) -> (f32, bool) {
    cw_sim::util::guard_haste(states, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneModels, SceneState, ModelInfo};
    use cw_render::frame::ModelRef;
    use cw_render::mesh::VoxelGrid;
    use cw_sim::combat::CreatureState;
    use cw_world::World;
    use cw_world::inventory::Item;
    use cw_world::zone::Zone;

    struct NoModels;

    impl SceneModels for NoModels {
        fn model(&self, _: i32) -> Option<ModelInfo> {
            None
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

    const ALL_VISIBLE: [[f32; 4]; 6] = [[0.0, 0.0, 0.0, 1.0]; 6];
    const B: i64 = 65536;
    const ZX: i32 = 0x8000;

    /// Solid ground below z = 10 over one zone.
    fn flat_world() -> World {
        let mut world = World::new(1);
        let mut z = Zone::new(ZX, ZX);
        for c in z.columns.iter_mut() {
            c.height = 10;
        }
        world.insert_zone(Box::new(z));
        world
    }

    fn put_f32(e: &mut EntityData, o: usize, v: f32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn ctx<'a>(
        world: &'a World,
        entities: &'a BTreeMap<i64, EntityData>,
        states: &'a BTreeMap<i64, CreatureState>,
        gc: &'a SceneState,
        projectiles: &'a [cw_sim::projectile::Projectile],
    ) -> SceneCtx<'a> {
        SceneCtx {
            world,
            entities,
            states,
            gc,
            models: &NoModels,
            projectiles,
            planes: ALL_VISIBLE,
            daylight: 1.0,
            underwater: false,
            world_material: [1.0; 4],
            draw_distance: 122.4,
            prop_far: 20.0,
            prop_near: 18.0,
            static_far: 36.72,
            static_near: 33.048,
            particle_range: 24.48,
            dt_ms: 16,
        }
    }

    fn base() -> i64 {
        i64::from(ZX) * 256 + 128
    }

    #[test]
    fn blob_shadow_on_flat_ground() {
        let world = flat_world();
        let mut gc = SceneState::default();
        let bx = base();
        gc.camera.position = [bx * B, bx * B, 20 * B];
        gc.camera.render_offset = [-bx * B, -bx * B];
        // A creature standing at the middle of block (bx, bx, 10), no pose offset.
        let pos = [bx * B + B / 2, bx * B + B / 2, 10 * B];
        let mut e = EntityData::ZERO;
        put_f32(&mut e, HP, 100.0);
        put_f32(&mut e, SCALE + 8, 2.0);
        let mut entities = BTreeMap::new();
        entities.insert(1, e);
        let mut states = BTreeMap::new();
        let mut st = CreatureState::default();
        st.riding.render_pos = pos;
        states.insert(1, st);
        let c = ctx(&world, &entities, &states, &gc, &[]);
        let q = shadows(&c);
        assert_eq!(q.len(), 25);
        // The centre cell (dx = dy = 0 is the 13th, x outer): corner at render (0, 0), on top of
        // the ground (z 10) + 0.02; uv (0 + 1.5 - 0.5) / 3.
        let m = &q[12];
        assert_eq!(m[0].position, [0.0, 0.0, 10.02]);
        assert_eq!(m[2].position, [1.0, 1.0, 10.02]);
        assert_eq!(m[0].color_argb, 0xff00_0000);
        let third = 0.333_333_34f32;
        assert_eq!(m[0].uv, [1.0 * third, 1.0 * third]);
        assert_eq!(m[1].uv, [1.0 * third + third, 1.0 * third]);
        assert_eq!(m[3].uv, [1.0 * third, 1.0 * third + third]);
        // The first cell is (-2, -2).
        assert_eq!(q[0][0].position, [-2.0, -2.0, 10.02]);
        assert_eq!(q[1][0].position, [-2.0, -1.0, 10.02]);
        // A widget hides them all.
        let mut gc2 = gc.clone();
        gc2.gui.w888 = true;
        assert!(shadows(&ctx(&world, &entities, &states, &gc2, &[])).is_empty());
    }

    #[test]
    fn healing_area_decal() {
        let world = flat_world();
        let mut gc = SceneState::default();
        let bx = base();
        gc.camera.position = [bx * B, bx * B, 20 * B];
        gc.camera.render_offset = [-bx * B, -bx * B];
        let p = cw_sim::projectile::Projectile {
            pos: [bx * B, bx * B, 12 * B],
            radius: 1.5,
            kind: 3,
            sub: 2,
            age: 0,
            ..Default::default()
        };
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let ps = [p];
        let q = shadows(&ctx(&world, &entities, &states, &gc, &ps));
        // r = trunc(2.5) = 2: 5x5 cells; cos(0)·-50 = -50: green 150 + 50.
        assert_eq!(q.len(), 25);
        assert_eq!(q[0][0].color_argb, 0xff00_c8ff);
        assert_eq!(q[12][0].position, [0.0, 0.0, 10.01]);
        // u = (1.5 + 0 - 0) / 3.
        assert_eq!(q[12][0].uv, [1.5 * (1.0 / 3.0f32), 1.5 * (1.0 / 3.0f32)]);
    }

    #[test]
    fn right_ribbon_from_trails() {
        let world = World::new(1);
        let mut gc = SceneState::default();
        gc.camera.position = [100 * B, 100 * B, 20 * B];
        gc.camera.render_offset = [-90 * B, -90 * B];
        let mut e = EntityData::ZERO;
        put_f32(&mut e, HP, 100.0);
        let mut entities = BTreeMap::new();
        entities.insert(5, e.clone());
        let mut states = BTreeMap::new();
        let mut st = CreatureState::default();
        st.riding.render_pos = [100 * B, 100 * B, 16 * B];
        states.insert(5, st);
        let mut pose = PoseFx::default();
        for i in 0..16 {
            pose.trails[0][i] = [1.0, i as f32, 17.0];
            pose.trails[1][i] = [0.0, i as f32, 16.5];
        }
        let c = ctx(&world, &entities, &states, &gc, &[]);
        let r = ribbons_with(&c, &|_| pose);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].len(), 32);
        // Relative trails + (render_pos + offset) = + (10, 10, 0).
        assert_eq!(r[0][0].position, [10.0, 10.0, 16.5]);
        assert_eq!(r[0][1].position, [11.0, 10.0, 17.0]);
        // The ends are transparent; i = 7 has k = (1 - (1/15)²)².
        assert_eq!(r[0][0].color_argb, 0x00ff_ffff);
        let k = ribbon_weight(7);
        assert_eq!(r[0][15].color_argb, (((k * 200.0) as u32) << 24) | 0xff_ffff);
        assert_eq!(r[0][14].color_argb, (((k * 50.0) as u32) << 24) | 0xff_ffff);
        // A left weapon adds the second strip.
        let mut e2 = e;
        e2.0[0x2f0 + 6 * 0x118] = 3;
        entities.insert(5, e2);
        let c = ctx(&world, &entities, &states, &gc, &[]);
        assert_eq!(ribbons_with(&c, &|_| pose).len(), 2);
    }
}
