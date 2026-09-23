//! The client's particle system: the `std::list` of 0xb8-byte particle records at
//! `GameController+0x800740` (`Cube.exe`).
//!
//! Tier B. Particles come from four places, each keeping the original's `rand()` draws in
//! order:
//!
//! | Source | Address | Here |
//! |---|---|---|
//! | ServerUpdate section 2 (`cw_net::packet::Particle`, 0x48 bytes), per record `count` particles | `GameController::update 0x004952a5..0x004955b1` | [`ParticleSystem::spawn_server`] |
//! | falling leaves of tree props (prop kind 0x3c) in the terrain pass | `render 0x004b1e91..0x004b212f` | [`leaf_particle`] (drawn by `scene`) |
//! | a model broken into its surface voxels (killed creatures) | `0x00470d80` | [`ParticleSystem::explode_model`] |
//! | sparks along a magic beam | `0x00471d50` (`0x00471f9b..`) | [`beam_particle`] |
//!
//! Every frame `GameController::update` (`0x00495f25..0x00496329`) trims the list to 5000,
//! advances each particle ([`ParticleSystem::update`]: age, gravity 30 blocks/s², spin,
//! movement with a block collision for flagged particles) and removes those past their
//! lifetime. `render` draws each visible particle as the unit voxel cube `GameController+0x800730`
//! through the CubeShader, double-sided (`0x004b7db6..0x004b8138`,
//! [`ParticleSystem::draw`]).
//!
//! The records are cubes, not quads: position, spin angles and a base matrix place a 1x1x1
//! voxel model scaled by the particle size.

#![allow(dead_code)]
#![allow(clippy::needless_range_loop)]

use cw_math::rand::MsvcRand;
use cw_render::frame::{D3dMatrix, IDENTITY, ModelRef};
use cw_render::mesh::VoxelGrid;
use cw_render::passes::{CameraInputs, ModelDraw, sphere_visible};

use crate::scene::math::{
    INV_FIXED, block_of_fixed, fixed_from_float, float_from_fixed, ftol, pre_multiply, pre_rotate_axis, pre_scale,
    pre_translate, render_translation,
};

/// Most particles the list keeps (`0x00495f41`: `resize(5000)` when larger, which drops the
/// newest, the list tail).
pub const MAX_PARTICLES: usize = 5000;

/// One particle record (0xb8 bytes; constructor `0x00465de0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// `+0x00`: kind. 0 and 4 are lit by the block they are in when drawn; 3 does not move;
    /// 4 has no gravity and passes through blocks of type 7 and 8 (leaves); 1 is recoloured
    /// fire; 2 are beam sparks.
    pub kind: i32,
    /// `+0x04`: age in ms; may start negative (a delay: nothing moves while negative).
    pub age: i32,
    /// `+0x08`: position, world fixed point.
    pub pos: [i64; 3],
    /// `+0x20`: the start position (written by `0x00470d80` only; read by nothing traced).
    pub start: [i64; 3],
    /// `+0x38`: spin angles in degrees (x, y, z).
    pub rotation: [f32; 3],
    /// `+0x44`: colour RGBA.
    pub color: [f32; 4],
    /// `+0x54`: velocity, blocks per second.
    pub velocity: [f32; 3],
    /// `+0x60`: spin velocity, degrees per second.
    pub spin: [f32; 3],
    /// `+0x6c`: base matrix applied after the spin and size (translation zero).
    pub matrix: D3dMatrix,
    /// `+0xac`: edge length in blocks (1.0 from the constructor).
    pub size: f32,
    /// `+0xb0`: collides with blocks while moving.
    pub collides: bool,
    /// `+0xb1`: frozen (not moved; never set by the code traced).
    pub frozen: bool,
    /// `+0xb4`: lifetime in ms (4000 from the constructor).
    pub lifetime: i32,
}

impl Particle {
    /// `0x00465de0`: kind 0, age 0, zero vectors, identity matrix, size 1, flags 0, lifetime
    /// 4000 (the position and colour are not written by the constructor; zero here).
    pub const NEW: Particle = Particle {
        kind: 0,
        age: 0,
        pos: [0; 3],
        start: [0; 3],
        rotation: [0.0; 3],
        color: [0.0; 4],
        velocity: [0.0; 3],
        spin: [0.0; 3],
        matrix: IDENTITY,
        size: 1.0,
        collides: false,
        frozen: false,
        lifetime: 4000,
    };
}

/// `rand() / 32767.0 - 0.5` as the original computes it (`cvtdq2ps`, `divss`, `subss`).
#[inline]
fn centred(rng: &mut MsvcRand) -> f32 {
    (rng.rand() as f32) / 32767.0 - 0.5
}

/// What the particle system reads of the world while moving particles: the block at a
/// position (`cube::World::getBlockFixed 0x0042f860`), as its four bytes.
pub trait BlockSource {
    /// The block at block coordinates.
    fn block(&self, x: i32, y: i32, z: i32) -> [u8; 4];
}

impl BlockSource for cw_world::World {
    fn block(&self, x: i32, y: i32, z: i32) -> [u8; 4] {
        cw_render::light::get_block(self, x, y, z)
    }
}

/// `blockIsSolid 0x0043b480`: any block type but air (0) and water (2).
#[inline]
fn block_is_solid(b: [u8; 4]) -> bool {
    let t = b[3] & 0x1f;
    t != 0 && t != 2
}

/// `(int)(fixed / 65536)` block coordinate of a fixed-point value (`fixedToInt 0x0042f570`,
/// `__alldiv`, truncating).
#[inline]
fn to_block(v: i64) -> i32 {
    (v / 65536) as i32
}

/// `GameController+0x800740`: the particle list, in list order (oldest first).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParticleSystem {
    /// The records.
    pub particles: Vec<Particle>,
}

impl ParticleSystem {
    /// An empty list (the controller constructor, `0x0045a119`).
    pub fn new() -> ParticleSystem {
        ParticleSystem::default()
    }

    /// `std::list::push_back` (`0x004869d0`).
    pub fn push(&mut self, p: Particle) {
        self.particles.push(p);
    }

    /// Number of particles.
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// `GameController::update 0x004952a5..0x004955b1`: `count` (`+0x38`) particles per
    /// server record, each spread around the record's velocity and spinning at random.
    ///
    /// Record layout (`cw_net::packet::Particle`): `+0x00` position (3 x i64), `+0x18`
    /// velocity, `+0x24` colour RGBA, `+0x34` size, `+0x38` count, `+0x3c` kind, `+0x40`
    /// spread. `rand()` per particle: three for the spread (z, y, x), three for the spin
    /// (z, y, x), and one more for the green of a kind-1 (fire) particle.
    pub fn spawn_server(&mut self, rec: &cw_net::packet::Particle, rng: &mut MsvcRand) {
        let b = &rec.0;
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let i = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let q = |o: usize| i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let count = i(0x38);
        let kind = i(0x3c);
        let spread = f(0x40);
        let vel = [f(0x18), f(0x1c), f(0x20)];
        let color = [f(0x24), f(0x28), f(0x2c), f(0x30)];
        let size = f(0x34);
        // 0x0049530a: `count <= 0` spawns nothing.
        for _ in 0..count.max(0) {
            let mut p = Particle::NEW;
            p.kind = kind;
            p.age = 0;
            p.pos = [q(0), q(8), q(0x10)];
            // 0x0049534e..0x004953be: vec3f(x, y, z) takes its arguments pushed z first, so the
            // first rand() is z.
            let z = centred(rng);
            let y = centred(rng);
            let x = centred(rng);
            // 0x004953c3: scaled by `spread * 2` (vec3fScale 0x00451510), added to the
            // record's velocity (0x00412280).
            let k = spread * 2.0;
            p.velocity = [vel[0] + x * k, vel[1] + y * k, vel[2] + z * k];
            p.rotation = [0.0; 3];
            // 0x00495438..0x004954ce: spin, rand() z, y, x, times 100.
            let z = centred(rng);
            let y = centred(rng);
            let x = centred(rng);
            p.spin = [x * 100.0, y * 100.0, z * 100.0];
            p.color = color;
            // 0x004954e3: fire: (1, rand/32767, 0, 1).
            if kind == 1 {
                let g = (rng.rand() as f32) / 32767.0;
                p.color = [1.0, g, 0.0, 1.0];
            }
            p.size = size;
            p.matrix = IDENTITY;
            self.push(p);
        }
    }

    /// `GameController::update 0x00495f25..0x00496329`: trim to [`MAX_PARTICLES`], then per
    /// particle: age += dt; unless the age is negative, the particle is frozen or of kind 3,
    /// apply gravity (not to kind 4), spin, and move (stepping through blocks when it
    /// collides); remove it once its age exceeds its lifetime.
    pub fn update(&mut self, dt_ms: i32, blocks: &dyn BlockSource) {
        // 0x00495f3c: `size() > 5000` → `resize(5000)` (0x004870f0 erases from the tail).
        if self.particles.len() > MAX_PARTICLES {
            self.particles.truncate(MAX_PARTICLES);
        }
        let s = (dt_ms as f32) * 0.001;
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            // 0x00495fa2: `age += dt; js skip`.
            p.age = p.age.wrapping_add(dt_ms);
            if p.age >= 0 && !p.frozen && p.kind != 3 {
                step_particle(p, dt_ms, s, blocks);
            }
            // 0x004962c5: removed when `age > lifetime` (collected, then erased in order).
            if p.age > p.lifetime {
                self.particles.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// `0x00470d80(model, matrix, colour, kind)`: break a model into particles. Every voxel
    /// that is solid and has an empty face neighbour (in the order x, then y, then z
    /// innermost) draws `rand()`; on an even value it becomes a particle of `kind`: a random
    /// delay (`-(rand() % 1000)` ms of age), the voxel's position through `matrix`, a
    /// velocity of twice its offset from the model centre, a random spin (three `rand()`,
    /// x, y, z), the voxel colour times `color`, the matrix's rotation as base matrix,
    /// block collisions on and a 5 s lifetime.
    ///
    /// `matrix` is the render-space world matrix of the model; `render_offset` is
    /// `GameController+0x1d8`/`+0x1e0`, subtracted to get back to world coordinates.
    pub fn explode_model(
        &mut self,
        grid: VoxelGrid<'_>,
        matrix: &D3dMatrix,
        color: [f32; 4],
        kind: i32,
        render_offset: [i64; 2],
        rng: &mut MsvcRand,
    ) {
        let [sx, sy, sz] = grid.size;
        let m = matrix;
        // 0x00470dc0..: the model centre through the matrix.
        let (hx, hy, hz) = ((sx as f32) * 0.5, (sy as f32) * 0.5, (sz as f32) * 0.5);
        let w = 1.0 / (hx * m[0][3] + hy * m[1][3] + hz * m[2][3] + m[3][3]);
        let centre = [
            (hy * m[1][0] + m[0][0] * hx + hz * m[2][0] + m[3][0]) * w,
            (m[0][1] * hx + m[1][1] * hy + hz * m[2][1] + m[3][1]) * w,
            (hx * m[0][2] + hy * m[1][2] + hz * m[2][2] + m[3][2]) * w,
        ];
        // `0x004e71d0(voxel, 0)`: not raw, so the pure red/green/blue marker voxels count as
        // empty too.
        let empty = |x: i32, y: i32, z: i32| cw_render::mesh::voxel_is_empty(grid.at(x, y, z), false);
        for x in 0..sx {
            for y in 0..sy {
                for z in 0..sz {
                    // 0x00470f60: the voxel itself must be solid ...
                    if empty(x, y, z) {
                        continue;
                    }
                    // ... and one of x-1, x+1, y-1, y+1, z-1, z+1 empty (tested in that order).
                    let surface = empty(x - 1, y, z)
                        || empty(x + 1, y, z)
                        || empty(x, y - 1, z)
                        || empty(x, y + 1, z)
                        || empty(x, y, z - 1)
                        || empty(x, y, z + 1);
                    if !surface {
                        continue;
                    }
                    // 0x00471147: `rand() % 2 == 0`.
                    if rng.rand() % 2 != 0 {
                        continue;
                    }
                    let mut p = Particle::NEW;
                    p.kind = kind;
                    // 0x0047127f: `age = -(rand() % 1000)`.
                    p.age = -(rng.rand() % 1000);
                    // 0x00471285..0x00471399: the voxel corner through the matrix.
                    let (fx, fy, fz) = (x as f32, y as f32, z as f32);
                    let wv = 1.0 / (fy * m[1][3] + fx * m[0][3] + fz * m[2][3] + m[3][3]);
                    let px = (m[0][0] * fx + m[1][0] * fy + m[2][0] * fz + m[3][0]) * wv;
                    let py = (m[1][1] * fy + m[0][1] * fx + fz * m[2][1] + m[3][1]) * wv;
                    let pz = (fy * m[1][2] + fx * m[0][2] + m[2][2] * fz + m[3][2]) * wv;
                    let a = fixed_from_float([px, py, pz]);
                    let c = fixed_from_float(centre);
                    // 0x0047140f: `k = ftol(131072.0)`; velocity = ((a - c) * k / 65536) / 65536.
                    let k = ftol(131_072.0);
                    let d = [
                        a[0].wrapping_sub(c[0]).wrapping_mul(k) / 65536,
                        a[1].wrapping_sub(c[1]).wrapping_mul(k) / 65536,
                        a[2].wrapping_sub(c[2]).wrapping_mul(k) / 65536,
                    ];
                    p.velocity = [(d[0] as f32) * INV_FIXED, (d[1] as f32) * INV_FIXED, (d[2] as f32) * INV_FIXED];
                    p.rotation = [0.0; 3];
                    // 0x004715bc..: spin, rand() x, y, z, times 100.
                    let rx = centred(rng);
                    let ry = centred(rng);
                    let rz = centred(rng);
                    p.spin = [rx * 100.0, ry * 100.0, rz * 100.0];
                    // 0x00471686..: voxel colour / 255 times the argument colour; alpha a * 1.
                    let v = grid.at(x, y, z);
                    p.color = [
                        (f32::from(v[0]) / 255.0) * color[0],
                        color[1] * (f32::from(v[1]) / 255.0),
                        color[2] * (f32::from(v[2]) / 255.0),
                        color[3] * 1.0,
                    ];
                    p.size = 1.0;
                    // The base matrix is the model matrix with its translation cleared.
                    p.matrix = *matrix;
                    p.matrix[3][0] = 0.0;
                    p.matrix[3][1] = 0.0;
                    p.matrix[3][2] = 0.0;
                    // 0x0047171b..: the position back in world coordinates.
                    p.pos = [
                        a[0].wrapping_sub(render_offset[0]),
                        a[1].wrapping_sub(render_offset[1]),
                        a[2],
                    ];
                    p.start = p.pos;
                    p.collides = true;
                    p.lifetime = 5000;
                    self.push(p);
                }
            }
        }
    }

    /// `render 0x004b7db6..0x004b8138`: every particle within `max_distance` (the draw
    /// distance times 0.2, `render`'s local at `esp+0x64`) whose cube, pushed out by its size,
    /// is inside the frustum becomes a model draw of the particle cube model:
    ///
    /// - material = colour × `world_material` (`(1, 1, 1, daylight)` or the underwater tint,
    ///   `esp+0x73c`) × `(1, 1, 1, light)`, where `light` is 1, or for kinds 0 and 4
    ///   `max(groundLightAt(pos) / 255, 0.2)`; kinds other than 0 and 4 then get alpha 1;
    /// - world = `T(pos) · Rz(rot.z) · Ry(rot.y) · Rx(rot.x) · S(size) · base · T(-0.5)` read
    ///   right to left (each helper pre-multiplies: translate, then the three axis rotations
    ///   0x004241b0, scale 0x00424730, the base matrix 0x00412400, and the centring 0x00424a60).
    ///
    /// `planes` are the frustum planes of this frame (`GameController+0x1000fa4`, rewritten at
    /// `0x004aee2e` before the creatures are drawn).
    pub fn draw(
        &self,
        camera: &CameraInputs,
        planes: &[[f32; 4]; 6],
        max_distance: f32,
        world_material: [f32; 4],
        light_at: &dyn Fn([i32; 3]) -> u8,
        cube_model: ModelRef,
    ) -> Vec<ModelDraw> {
        let mut out = Vec::new();
        for p in &self.particles {
            // 0x004b7e44: sphere test, margin = size.
            if !sphere_visible(planes, camera, p.pos, p.size, max_distance) {
                continue;
            }
            // 0x004b7e51..0x004b7ec5.
            let mut light = 1.0f32;
            if p.kind == 0 || p.kind == 4 {
                let b = [to_block(p.pos[0]), to_block(p.pos[1]), to_block(p.pos[2])];
                light = f32::from(light_at(b)) / 255.0;
                if 0.2 > light {
                    light = 0.2;
                }
            }
            // 0x004b7f10: (colour * world material) * (1, 1, 1, light).
            let c = p.color;
            let wm = world_material;
            let mut material = [wm[0] * c[0] * 1.0, wm[1] * c[1] * 1.0, wm[2] * c[2] * 1.0, light * (wm[3] * c[3])];
            if p.kind != 0 && p.kind != 4 {
                material[3] = 1.0;
            }
            // 0x004b7f4e..0x004b80c7.
            let mut m = IDENTITY;
            pre_translate(&mut m, render_translation(p.pos, camera.render_offset));
            pre_rotate_axis(&mut m, p.rotation[2], [0.0, 0.0, 1.0]);
            pre_rotate_axis(&mut m, p.rotation[1], [0.0, 1.0, 0.0]);
            pre_rotate_axis(&mut m, p.rotation[0], [1.0, 0.0, 0.0]);
            pre_scale(&mut m, [p.size, p.size, p.size]);
            pre_multiply(&mut m, &p.matrix);
            pre_translate(&mut m, [-0.5, -0.5, -0.5]);
            out.push(ModelDraw {
                origin: 0x004b_80f5,
                model: cube_model,
                world: m,
                material,
                alpha: 1.0,
                double_sided: true,
                shininess: 0.0,
                set_white: None,
                mirrored: false,
            });
        }
        out
    }
}

/// One step of `0x00495fb8..0x004962c1` for a particle that moves this frame.
fn step_particle(p: &mut Particle, dt_ms: i32, s: f32, blocks: &dyn BlockSource) {
    // 0x00495fc3: gravity, except for kind 4: `vz -= dt * 0.001 * 30`.
    if p.kind != 4 {
        p.velocity[2] -= (dt_ms as f32) * 0.001 * 30.0;
    }
    // 0x00495ff5..0x00496033: rotation += spin * s.
    let ds = [p.spin[0] * s, p.spin[1] * s, p.spin[2] * s];
    p.rotation = [p.rotation[0] + ds[0], p.rotation[1] + ds[1], p.rotation[2] + ds[2]];
    if p.collides {
        // 0x00496055..0x004960af: the move is split into n = min(trunc(|v·s|) + 1, 10) steps.
        let mut step = [p.velocity[0] * s, p.velocity[1] * s, p.velocity[2] * s];
        let len = ((step[0] * step[0] + step[1] * step[1] + step[2] * step[2]) as f64).sqrt() as f32;
        let mut n = (len as i32).wrapping_add(1);
        if n > 10 {
            n = 10;
        }
        let inv = 1.0 / (n as f32);
        step = [step[0] * inv, step[1] * inv, step[2] * inv];
        for _ in 0..n.max(0) {
            for axis in 0..3 {
                // 0x00496100..: move one axis, test the block, undo and stop that axis on a hit.
                let d = ftol(step[axis] * 65536.0);
                p.pos[axis] = p.pos[axis].wrapping_add(d);
                let b = blocks.block(block_of_fixed(p.pos[0]), block_of_fixed(p.pos[1]), block_of_fixed(p.pos[2]));
                if !block_is_solid(b) {
                    continue;
                }
                // 0x0049618e: kind 4 passes through block types 8 and 7.
                if p.kind == 4 {
                    let t = b[3] & 0x1f;
                    if t == 8 || t == 7 {
                        continue;
                    }
                }
                p.pos[axis] = p.pos[axis].wrapping_sub(ftol(step[axis] * 65536.0));
                // 0x004961e0: the velocity along the axis is cleared; the step itself is kept,
                // so the remaining sub-steps try (and undo) the move again.
                p.velocity[axis] = 0.0;
                // 0x004961ee: a hit on z stops the particle and its spin.
                if axis == 2 {
                    p.velocity = [0.0; 3];
                    p.spin = [0.0; 3];
                }
            }
        }
    } else {
        // 0x0049626e..0x004962bc: pos += fixed(v * s), then rotation += spin * s a second
        // time (the original adds the spin twice for particles that do not collide).
        let mv = fixed_from_float([p.velocity[0] * s, p.velocity[1] * s, p.velocity[2] * s]);
        p.pos = [p.pos[0].wrapping_add(mv[0]), p.pos[1].wrapping_add(mv[1]), p.pos[2].wrapping_add(mv[2])];
        let ds = [p.spin[0] * s, p.spin[1] * s, p.spin[2] * s];
        p.rotation = [p.rotation[0] + ds[0], p.rotation[1] + ds[1], p.rotation[2] + ds[2]];
    }
}

/// `render 0x004b1f21..0x004b212a`: a leaf falling from a tree prop (kind 0x3c) at `pos`
/// with the prop's colour (`prop+0x2c`). Draws five `rand()`: velocity y, x (z is -0.5),
/// then spin z, y, x. Kind 4, size 0.5, collides, lifetime 20 s.
pub fn leaf_particle(pos: [i64; 3], color: [f32; 3], rng: &mut MsvcRand) -> Particle {
    let mut p = Particle::NEW;
    p.age = 0;
    p.pos = pos;
    // vec3f(x, y, -0.5): pushed z (the constant), then y, then x.
    let y = centred(rng);
    let x = centred(rng);
    p.velocity = [x * 4.0, y * 4.0, -0.5 * 4.0];
    p.rotation = [0.0; 3];
    let z = centred(rng);
    let y = centred(rng);
    let x = centred(rng);
    p.spin = [x * 10.0, y * 10.0, z * 10.0];
    p.color = [color[0], color[1], color[2], 1.0];
    p.size = 0.5;
    p.kind = 4;
    p.matrix = IDENTITY;
    p.lifetime = 20_000;
    p.collides = true;
    p
}

/// `0x00471d50` (`0x00471f9b..0x00472857`): one spark of a magic beam at `pos`, coloured
/// `color` (the beam's first colour, `*param_5`), sized by `size_scale` (`param_12`).
/// Draws four `rand()`: the velocity z (`r * 5 / 32767 + 5`), y and x
/// (`5 - r * 10 / 32767`), then the size (`(r * 0.005 / 32767 + 0.05) * size_scale`).
/// Kind 2, no collision, default lifetime.
pub fn beam_particle(pos: [i64; 3], color: [f32; 4], size_scale: f32, rng: &mut MsvcRand) -> Particle {
    let mut p = Particle::NEW;
    p.kind = 2;
    p.pos = pos;
    let z = ((rng.rand() as f32) * 5.0) / 32767.0 + 5.0;
    let y = 5.0 - ((rng.rand() as f32) * 10.0) / 32767.0;
    let x = 5.0 - ((rng.rand() as f32) * 10.0) / 32767.0;
    p.velocity = [x, y, z];
    p.color = color;
    p.size = (((rng.rand() as f32) * 0.005) / 32767.0 + 0.05) * size_scale;
    p
}

/// The float position of a particle in blocks (tests and debugging).
pub fn position_blocks(p: &Particle) -> [f32; 3] {
    float_from_fixed(p.pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Air;
    impl BlockSource for Air {
        fn block(&self, _: i32, _: i32, _: i32) -> [u8; 4] {
            [0; 4]
        }
    }

    /// Solid ground (type 1) at z < 10.
    struct Floor;
    impl BlockSource for Floor {
        fn block(&self, _: i32, _: i32, z: i32) -> [u8; 4] {
            if z < 10 { [0, 0, 0, 1] } else { [0; 4] }
        }
    }

    fn resting(z: i64) -> Particle {
        Particle { pos: [100 << 16, 100 << 16, z << 16], ..Particle::NEW }
    }

    #[test]
    fn gravity_pulls_down_30_blocks_per_second_squared() {
        let mut s = ParticleSystem::new();
        s.push(resting(50));
        s.update(100, &Air);
        let p = s.particles[0];
        // v = -30 * 0.1 = -3 blocks/s, then moved by v * 0.1 = -0.3 blocks.
        assert!((p.velocity[2] + 3.0).abs() < 1e-5, "{:?}", p.velocity);
        assert_eq!(p.pos[2], (50 << 16) + ftol(-0.3f32 * 65536.0));
    }

    #[test]
    fn kind_4_has_no_gravity() {
        let mut s = ParticleSystem::new();
        s.push(Particle { kind: 4, ..resting(50) });
        s.update(100, &Air);
        assert_eq!(s.particles[0].velocity, [0.0; 3]);
        assert_eq!(s.particles[0].pos[2], 50 << 16);
    }

    #[test]
    fn lifetime_removes_after_exceeded_not_at() {
        let mut s = ParticleSystem::new();
        s.push(Particle { kind: 3, lifetime: 100, ..resting(50) });
        s.update(100, &Air);
        assert_eq!(s.len(), 1, "age == lifetime keeps the particle");
        s.update(1, &Air);
        assert!(s.is_empty());
    }

    #[test]
    fn negative_age_delays_movement() {
        let mut s = ParticleSystem::new();
        s.push(Particle { age: -500, ..resting(50) });
        s.update(100, &Air);
        assert_eq!(s.particles[0].velocity, [0.0; 3]);
        assert_eq!(s.particles[0].age, -400);
    }

    #[test]
    fn colliding_particle_stops_on_the_floor() {
        let mut s = ParticleSystem::new();
        s.push(Particle { collides: true, velocity: [0.0, 0.0, -20.0], spin: [5.0; 3], ..resting(11) });
        for _ in 0..20 {
            s.update(50, &Floor);
        }
        let p = s.particles[0];
        assert!(p.pos[2] >= 10 << 16, "{}", p.pos[2]);
        assert_eq!(p.velocity, [0.0; 3]);
        assert_eq!(p.spin, [0.0; 3]);
    }

    #[test]
    fn trims_to_5000_keeping_the_oldest() {
        let mut s = ParticleSystem::new();
        for i in 0..5003 {
            s.push(Particle { kind: 3, age: i, lifetime: 1 << 30, ..Particle::NEW });
        }
        s.update(0, &Air);
        assert_eq!(s.len(), MAX_PARTICLES);
        assert_eq!(s.particles.last().unwrap().age, 4999);
    }

    #[test]
    fn server_record_draws_six_rands_per_particle_and_seven_for_fire() {
        let mut rec = [0u8; 0x48];
        rec[0x38..0x3c].copy_from_slice(&3i32.to_le_bytes());
        rec[0x34..0x38].copy_from_slice(&0.25f32.to_le_bytes());
        let mut s = ParticleSystem::new();
        let mut rng = MsvcRand::new(7);
        s.spawn_server(&cw_net::packet::Particle(rec), &mut rng);
        let mut reference = MsvcRand::new(7);
        for _ in 0..18 {
            reference.rand();
        }
        assert_eq!(rng.rand(), reference.rand());
        assert_eq!(s.len(), 3);
        assert_eq!(s.particles[0].size, 0.25);

        rec[0x3c..0x40].copy_from_slice(&1i32.to_le_bytes());
        rec[0x38..0x3c].copy_from_slice(&1i32.to_le_bytes());
        let mut rng = MsvcRand::new(7);
        s.spawn_server(&cw_net::packet::Particle(rec), &mut rng);
        let mut reference = MsvcRand::new(7);
        for _ in 0..7 {
            reference.rand();
        }
        assert_eq!(rng.rand(), reference.rand());
        let fire = s.particles.last().unwrap();
        assert_eq!(fire.color[0], 1.0);
        assert_eq!(fire.color[2], 0.0);
    }

    #[test]
    fn explode_uses_only_surface_voxels() {
        // A 3x3x3 solid cube: 26 surface voxels, the centre is interior.
        let voxels = vec![[10u8, 20, 30]; 27];
        let grid = VoxelGrid { size: [3, 3, 3], voxels: &voxels };
        let mut s = ParticleSystem::new();
        let mut rng = MsvcRand::new(1);
        s.explode_model(grid, &IDENTITY, [1.0; 4], 0, [0, 0], &mut rng);
        let mut reference = MsvcRand::new(1);
        let mut expected = 0;
        for _ in 0..26 {
            if reference.rand() % 2 == 0 {
                expected += 1;
                for _ in 0..4 {
                    reference.rand();
                }
            }
        }
        assert_eq!(s.len(), expected);
        assert_eq!(rng.rand(), reference.rand());
        for p in &s.particles {
            assert!(p.age <= 0 && p.age > -1000);
            assert!(p.collides);
            assert_eq!(p.lifetime, 5000);
        }
    }
}
