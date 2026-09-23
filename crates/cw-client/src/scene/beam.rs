//! The magic beam `Cube.exe 0x00471d50` (15 KB): a helix of particle cubes along a direction,
//! three cubes per segment, with sparks.
//!
//! The original inlines every matrix product (the identity, translation, axis rotation and
//! the x/y/z rotations of `mat4.h`) into one straight-line body; the port calls the matrix
//! helpers of [`super::math`] instead. Each inlined product only adds terms that are exact
//! zeros (`x·0`) or multiplies by exact ones, so the helpers produce the same floats for
//! finite inputs (the sign of a zero can differ). The trigonometry keeps the original's
//! float/double mix (`sin`/`cos`/`sqrt` of a float widened to double and narrowed back; the
//! helix phases are computed in double).
//!
//! # Map
//!
//! | Range | Piece |
//! |---|---|
//! | `0x00471dab..0x00472141` | the frame: `v ⟂ dir`, `w = v × dir`, both normalised then scaled by `|dir|`; the unit direction |
//! | `0x0047214a..0x004722c8` | `sparks`: march the unit direction from `start` until a solid block (at most 20 steps); the beam's span becomes `dir × steps` |
//! | `0x004722c8..0x00472526` | per segment: its point `start + span·t` |
//! | `0x0047252c..0x00472877` | the spark: `rand() % 8 == 0`, then the 200 ms boundary test, then [`beam_particle`] |
//! | `0x00472881..0x00473903` | cube 1 (colour 0) |
//! | `0x00473908..0x00474a0b` | cube 2 (colour 1) |
//! | `0x00474a10..0x00475861` | cube 3 (colour 2) |

use cw_math::rand::MsvcRand;
use cw_render::frame::D3dMatrix;
use cw_render::passes::ModelDraw;

use super::SceneCtx;
use super::math::*;
use crate::particles::{ParticleSystem, beam_particle};

/// The arguments of `0x00471d50(this = GameController, …)`, by parameter (`ret 0x38`).
///
/// `param_9`/`param_10` (the view and projection matrices for `setTransforms`) are not
/// carried: the port's passes supply them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeamArgs {
    /// `param_2`: start, world fixed point.
    pub start: [i64; 3],
    /// `param_3`: direction and length, blocks.
    pub dir: [f32; 3],
    /// `*(float *)(param_4 + 8)`: an angle in degrees (the third float of the vector
    /// `param_4` points to).
    pub angle: f32,
    /// `*param_5`, `*param_6`, `*param_7`: the colours of the three cubes of a segment
    /// (`param_5` also colours the sparks).
    pub colors: [[f32; 4]; 3],
    /// `param_8`: a time in ms (phase of the helix and of the segments).
    pub time: i32,
    /// `param_11`.
    pub p11: f32,
    /// `param_12`: size scale.
    pub size: f32,
    /// `param_13`.
    pub p13: f32,
    /// `param_14`: number of segments.
    pub segments: i32,
    /// `param_15`: stop the beam at the first solid block (up to 20 steps) and throw sparks.
    pub sparks: bool,
}

/// `0x006fce08`.
const PI: f64 = std::f64::consts::PI;

/// The block containing a fixed-point coordinate as `0x0042f860` computes it: negative
/// values are moved down by one block before the truncating division (so an exact negative
/// multiple of 65536 lands one block lower than a floor would).
fn block_of(v: i64) -> i32 {
    let v = if v < 0 { v.wrapping_sub(0x10000) } else { v };
    (v / 0x10000) as i32
}

/// `M = Rx(degrees) · M` as the original inlines it (`mat4.h`'s x rotation): per column,
/// `m1 = c·m1 + s·m2`, `m2 = c·m2 - s·m1`.
pub(super) fn pre_rotate_x(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let (a, b) = (m[1][j], m[2][j]);
        m[1][j] = c * a + s * b;
        m[2][j] = c * b - s * a;
    }
}

/// `M = Ry(degrees) · M` as the original inlines it: per column, `m0 = c·m0 - s·m2`,
/// `m2 = s·m0 + c·m2`.
pub(super) fn pre_rotate_y(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let (a, b) = (m[0][j], m[2][j]);
        m[0][j] = c * a - s * b;
        m[2][j] = s * a + c * b;
    }
}

/// `trig · (v · k)` per axis, optionally halved first: `trig · ((v · k) · 0.5)`.
fn scaled(trig: f32, v: [f32; 3], k: f32, half: bool) -> [f32; 3] {
    let f = |x: f32| {
        let vk = x * k;
        trig * if half { vk * 0.5 } else { vk }
    };
    [f(v[0]), f(v[1]), f(v[2])]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// The centre of one cube: the segment point `p` on the first circle (radius `k·|dir|`, angle
/// `a`, in the plane of `u`, `w`, both of length `|dir|`) plus a second circle (angle `a·b_mul`, radius `k·|dir|`
/// or half that): `(C2 + S2) + ((P + C) + S)` in the original's order.
fn helix_point(p: [f32; 3], u: [f32; 3], w: [f32; 3], k: f32, a: f32, b_mul: f32, half: bool) -> [f32; 3] {
    let s = scaled(sinf(a), w, k, false);
    let c = scaled(cosf(a), u, k, false);
    let q = add(add(p, c), s);
    let b = a * b_mul;
    let s2 = scaled(sinf(b), w, k, half);
    let c2 = scaled(cosf(b), u, k, half);
    add(add(c2, s2), q)
}

/// The matrix of one cube: `T(point) · R(axis z, angle) · Rx(rx) · Ry(rx) · Rz(rz) ·
/// S(scale) · T(-0.5)` (pre-multiplied in that order), the scale being
/// `(cos(a·0.9)·0.1 + 0.2) · (k + 0.2) · size`.
fn cube_matrix(point: [f32; 3], angle: f32, rx: f32, rz: f32, a: f32, k: f32, size: f32) -> D3dMatrix {
    let mut m = identity();
    pre_translate(&mut m, point);
    pre_rotate_axis(&mut m, angle, [0.0, 0.0, 1.0]);
    pre_rotate_x(&mut m, rx);
    pre_rotate_y(&mut m, rx);
    pre_rotate_z(&mut m, rz);
    let s = (cosf(a * 0.9) * 0.1 + 0.2) * (k + 0.2) * size;
    pre_scale(&mut m, [s, s, s]);
    // `row3 -= row0·0.5 + row1·0.5 + row2·0.5`: the unit cube centred.
    pre_translate(&mut m, [-0.5, -0.5, -0.5]);
    m
}

/// `0x00471d50`: the beam's cubes (particle cube model `GC+0x800730`) and, when `sparks`,
/// its sparks pushed to `particles` (`rand()` per segment).
///
/// Per segment `i` (`t = (i + 1 + (time % n)/n) / n` along the span):
///
/// - with `sparks`: one `rand()`; when it is a multiple of 8 and
///   `(clock + i·200/n)/200 != (dt + clock + i·200/n)/200` (the world clock `GC+0x8003a0`
///   and the frame time `GC+0x8006e8`), a spark at the segment point
///   ([`beam_particle`]: four more `rand()`);
/// - three cubes on a double helix around the span, coloured `colors[0..3]`, their sizes
///   growing by 1 % from one to the next.
///
/// The draws carry alpha 1: the function does not set the alpha itself; its caller in the
/// objects pass (`0x004b2ec6`) sets 1 just before the call. Other callers inherit whatever
/// alpha is current (see the report).
pub fn draw_beam(ctx: &SceneCtx, a: &BeamArgs, particles: &mut ParticleSystem, rng: &mut MsvcRand) -> Vec<ModelDraw> {
    let gc = ctx.gc;
    let mut out = Vec::new();
    let [dx, dy, dz] = a.dir;
    // 0x00471dab: a vector perpendicular to the direction, (0, 0, 1) × dir.
    let zdz = dz * 0.0;
    let zdx = dx * 0.0;
    let zdy = dy * 0.0;
    let mut vx = zdz - dy;
    let mut vy = dx - zdz;
    let mut vz = zdy - zdx;
    let len2 = dx * dx + dy * dy + dz * dz;
    // 0x00471e29: `comiss 1e-5, len2; jbe`: a (nearly) zero direction takes (0, 1, 0) × dir.
    if 1e-5 > len2 {
        vy = zdx - zdz;
        vx = dz - zdy;
        vz = zdy - dx;
    }
    let inv = 1.0 / (((vy * vy + vx * vx + vz * vz) as f64).sqrt() as f32);
    vz *= inv;
    vy *= inv;
    vx *= inv;
    // 0x00471ef5: w = v × dir, normalised.
    let mut w = [vy * dz - vz * dy, vz * dx - dz * vx, dy * vx - dx * vy];
    let inv = 1.0 / (((w[1] * w[1] + w[0] * w[0] + w[2] * w[2]) as f64).sqrt() as f32);
    w = [w[0] * inv, w[1] * inv, w[2] * inv];
    // 0x00471fd8: both scaled by |dir|.
    let l = (len2 as f64).sqrt() as f32;
    let u = [vx * l, vy * l, vz * l];
    let w = [w[0] * l, w[1] * l, w[2] * l];
    // 0x00472076: the unit direction.
    let inv = 1.0 / (((dy * dy + dx * dx + dz * dz) as f64).sqrt() as f32);
    let unit = [inv * dx, dy * inv, dz * inv];
    let mut span = a.dir;
    // 0x0047214a..0x004722c8: march until a block that is neither air nor water.
    if a.sparks {
        let mut pos = a.start;
        let mut steps = 0.0f32;
        for _ in 0..0x14 {
            let b = cw_render::light::get_block(ctx.world, block_of(pos[0]), block_of(pos[1]), block_of(pos[2]));
            let t = b[3] & 0x1f;
            if t != 0 && t != 2 {
                break;
            }
            let step = [ftol(unit[0] * 65536.0), ftol(unit[1] * 65536.0), ftol(unit[2] * 65536.0)];
            pos = add3(pos, step);
            steps += 1.0;
        }
        span = [dx * steps, dy * steps, dz * steps];
    }
    // 0x004722c8.
    if a.segments <= 0 {
        return out;
    }
    let n = a.segments as f32;
    let phase = ((a.time % a.segments) as f32) / n;
    let clockf = a.time as f32;
    let cube = ctx.models.particle_cube();
    let draw = |m: D3dMatrix, color: [f32; 4]| ModelDraw {
        origin: 0x0047_1d50,
        model: cube,
        world: m,
        material: color,
        alpha: 1.0,
        double_sided: false,
        shininess: 0.0,
        set_white: None,
        mirrored: false,
    };
    for i in 0..a.segments {
        let t = ((i as f32) + 1.0 + phase) / n;
        let off = [ftol((span[0] * t) * 65536.0), ftol((span[1] * t) * 65536.0), ftol((span[2] * t) * 65536.0)];
        let point = add3(a.start, off);
        let p = render_translation(point, gc.camera.render_offset);
        // 0x0047252c: the spark.
        if a.sparks && rng.rand() % 8 == 0 {
            let k = i.wrapping_mul(200) / a.segments;
            let clock = gc.clock_ms;
            if clock.wrapping_add(k) / 200 != ctx.dt_ms.wrapping_add(clock).wrapping_add(k) / 200 {
                particles.push(beam_particle(point, a.colors[0], a.size, rng));
            }
        }
        // 0x00472881: cube 1.
        let t2 = t + 0.1;
        let mut k = t2 * a.p11;
        let mut phase_d = (t2 as f64) * PI;
        let a1 = ((phase_d * 2.0 * (l as f64) - ((clockf * 0.005) as f64) * PI) * (a.p13 as f64)) as f32;
        let tl = t2 * l;
        let rx = tl * 360.0;
        let rz = tl * -360.0;
        let pt = helix_point(p, u, w, k, a1, -0.3, false);
        out.push(draw(cube_matrix(pt, a.angle, rx, rz, a1, k, a.size), a.colors[0]));
        // 0x00473908: cube 2.
        k *= 1.01;
        phase_d = phase_d * 3.0 * (l as f64);
        let a2 = ((phase_d - ((clockf * 0.004) as f64) * PI) * (a.p13 as f64)) as f32;
        let pt = helix_point(p, u, w, k, a2, 0.3, true);
        out.push(draw(cube_matrix(pt, a.angle, rx * 2.0, rz * 2.0, a2, k, a.size), a.colors[1]));
        // 0x00474a10: cube 3.
        k *= 1.01;
        phase_d = (phase_d - ((clockf * 0.006) as f64) * PI) * (a.p13 as f64);
        let a3 = phase_d as f32;
        let pt = helix_point(p, u, w, k, a3, 0.3, true);
        out.push(draw(cube_matrix(pt, a.angle, rx * 3.0, rz * 3.0, a3, k, a.size), a.colors[2]));
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

    use crate::scene::{ModelInfo, SceneModels, SceneState};

    struct Cube;
    impl SceneModels for Cube {
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
            7
        }
    }

    fn args(sparks: bool, segments: i32) -> BeamArgs {
        BeamArgs {
            start: [1000 << 16, 1000 << 16, 5000 << 16],
            dir: [8.0, 0.0, 0.0],
            angle: 0.0,
            colors: [[1.0, 0.0, 0.0, 1.0], [1.0, 0.25, 0.0, 1.0], [1.0, 0.5, 0.0, 1.0]],
            time: 1234,
            p11: 0.2,
            size: 4.0,
            p13: 2.0,
            segments,
            sparks,
        }
    }

    fn with_ctx<R>(clock: i32, dt: i32, f: impl FnOnce(&SceneCtx) -> R) -> R {
        let world = cw_world::World::new(1);
        let entities = BTreeMap::new();
        let states = BTreeMap::new();
        let gc = SceneState { clock_ms: clock, ..SceneState::default() };
        let ctx = SceneCtx {
            world: &world,
            entities: &entities,
            states: &states,
            gc: &gc,
            models: &Cube,
            projectiles: &[],
            planes: [[0.0, 0.0, 0.0, 1.0]; 6],
            daylight: 1.0,
            underwater: false,
            world_material: [1.0; 4],
            draw_distance: 100.0,
            prop_far: 20.0,
            prop_near: 18.0,
            static_far: 30.0,
            static_near: 27.0,
            particle_range: 20.0,
            dt_ms: dt,
        };
        f(&ctx)
    }

    /// The rand() draws of the sparks replayed by hand: one per segment, four more per
    /// spark thrown.
    fn expected(seed: u32, segments: i32, clock: i32, dt: i32) -> (usize, MsvcRand) {
        let mut r = MsvcRand::new(seed);
        let mut sparks = 0;
        for i in 0..segments {
            if r.rand() % 8 == 0 {
                let k = i * 200 / segments;
                if (clock + k) / 200 != (dt + clock + k) / 200 {
                    for _ in 0..4 {
                        r.rand();
                    }
                    sparks += 1;
                }
            }
        }
        (sparks, r)
    }

    #[test]
    fn spark_draws_match_a_replay() {
        // The sky above z = 5000 is air, so the march takes all 20 steps.
        for (seed, segments, clock, dt) in [(1u32, 5, 190, 20), (7, 8, 0, 16), (12345, 40, 999, 250), (99, 3, 150, 60)] {
            with_ctx(clock, dt, |ctx| {
                let mut rng = MsvcRand::new(seed);
                let mut ps = ParticleSystem::new();
                let draws = draw_beam(ctx, &args(true, segments), &mut ps, &mut rng);
                assert_eq!(draws.len(), 3 * segments as usize);
                let (n, mut replay) = expected(seed, segments, clock, dt);
                assert_eq!(ps.len(), n, "seed {seed}");
                assert_eq!(rng.rand(), replay.rand(), "stream position after seed {seed}");
                for p in &ps.particles {
                    assert_eq!(p.kind, 2);
                    assert_eq!(p.color, [1.0, 0.0, 0.0, 1.0]);
                }
            });
        }
    }

    #[test]
    fn a_boundary_every_frame_throws_a_spark_per_multiple_of_eight() {
        // dt = 200 crosses a 200 ms boundary for every segment.
        with_ctx(0, 200, |ctx| {
            let mut rng = MsvcRand::new(3);
            let mut ps = ParticleSystem::new();
            draw_beam(ctx, &args(true, 30), &mut ps, &mut rng);
            let mut r = MsvcRand::new(3);
            let mut n = 0;
            for _ in 0..30 {
                if r.rand() % 8 == 0 {
                    n += 1;
                    for _ in 0..4 {
                        r.rand();
                    }
                }
            }
            assert_eq!(ps.len(), n);
        });
    }

    #[test]
    fn no_sparks_no_rand() {
        with_ctx(0, 200, |ctx| {
            let mut rng = MsvcRand::new(5);
            let mut ps = ParticleSystem::new();
            let draws = draw_beam(ctx, &args(false, 40), &mut ps, &mut rng);
            assert_eq!(draws.len(), 120);
            assert!(ps.particles.is_empty());
            assert_eq!(rng.rand(), MsvcRand::new(5).rand());
            assert!(draws.iter().all(|d| d.model == 7 && d.alpha == 1.0));
            assert_eq!(draws[1].material, [1.0, 0.25, 0.0, 1.0]);
        });
    }

    #[test]
    fn cubes_follow_the_span() {
        // Without sparks the span is `dir`: segment i's first cube is within the helix
        // radius (two circles of radius k·|dir| = 8k, k = (t + 0.1)·0.2) of start + dir·t.
        with_ctx(0, 16, |ctx| {
            let mut rng = MsvcRand::new(5);
            let mut ps = ParticleSystem::new();
            let a = args(false, 4);
            let draws = draw_beam(ctx, &a, &mut ps, &mut rng);
            for i in 0..4 {
                let t = ((i as f32) + 1.0 + (1234 % 4) as f32 / 4.0) / 4.0;
                let c = transform_point(&draws[3 * i].world, [0.5, 0.5, 0.5]);
                let expect = [1000.0 + 8.0 * t, 1000.0, 5000.0];
                let r = 2.0 * (t + 0.1) * 0.2 * 8.0 + 1e-3;
                let d = [c[0] - expect[0], c[1] - expect[1], c[2] - expect[2]];
                assert!(d[0].abs() < 1e-2, "segment {i}: {c:?}");
                assert!((d[1] * d[1] + d[2] * d[2]).sqrt() <= r * 1.01, "segment {i}: {c:?}");
            }
        });
    }
}
