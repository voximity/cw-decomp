//! World and model meshes (Tier A: the same vertices and indices as Cube.exe, on the CPU).
//!
//! - [`build_chunk_mesh`], `buildChunkMesh` (`Cube.exe 0x0049d910`), called by the chunk mesh
//!   thread (`Cube.exe 0x004690a0`) for one 32x32-column chunk once [`chunk_ready`] holds.
//! - [`build_model_mesh`], `cube::Sprite::buildMesh` (`Cube.exe 0x004e7870`), for the voxel
//!   models (`.cub`) of creatures, items and props.
//!
//! Both emit the 8-byte [`WorldVertex`]. One quad per visible face, four vertices, indices
//! `0 1 2 0 2 3`; no greedy merging or strips.
//!
//! Map of `buildChunkMesh` (0x0049d910..0x004a1350):
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | 0x0049d910..0x0049db10 | clear the scratch vectors 0x0076b098/0x0076b0a8/0x0076b0b4, lock | - |
//! | 0x0049db10..0x0049db89 | relight the chunk and a margin when dirty or at a zone edge | [`build_chunk_mesh`] |
//! | 0x0049db89..0x0049e04d | pass 1: the z range of the exposed blocks | [`exposed_z_range`] |
//! | 0x0049e04d..0x0049e089 | slab count `(max - min) / 250 + 1` | [`build_chunk_mesh`] |
//! | 0x0049e089..0x004a08e6 | pass 2: colours and faces per block | [`mesh_column`] |
//! | 0x004a08e6..0x004a0cbc | per slab: VB (`n * 8`, MANAGED) and two INDEX32 IBs | [`ChunkMesh`] |
//! | 0x004a0cbc..0x004a0e8e | the zone's props inside the chunk get their light level | [`chunk_props`] |
//! | 0x004a0e8e..0x004a1350 | bounds, radius, `cube::ChunkBuffer` list, chunk fields | [`ChunkBuild`] |

use bytemuck::{Pod, Zeroable};
use cw_world::World;
use cw_world::surface::Block;
use cw_world::zone::{Column, Prop};

use crate::light::{block_in_column, compute_light_in_steps, get_block, vertex_light};

/// Slab height: a chunk's vertical extent is cut into buffers of 250 blocks so that the local z
/// fits the vertex's byte (`Cube.exe 0x0049e04f`, the `0x10624dd3` division by 250).
pub const SLAB_HEIGHT: i32 = 250;

/// Chunk side in blocks (`buildChunkMesh` multiplies the chunk coordinates by 0x20).
pub const CHUNK_SIZE: i32 = 32;

/// The 8-byte world vertex, `cube::CubeVertex` (`Cube.exe 0x00466650`): D3DDECLTYPE_UBYTE4
/// position then D3DCOLOR, as the vertex buffers hold it.
///
/// - `pos`: local x, y, z (block corner relative to the chunk origin and the slab base; a model
///   vertex is in voxel units) and the face index in w: 0 +x, 1 -x, 2 +y, 3 -y, 4 +z, 5 -z.
/// - `color`: a D3DCOLOR, `A << 24 | R << 16 | G << 8 | B` (bytes B, G, R, A in memory), each
///   channel `(int)(c * 255.0f) & 0xff` with truncation; A is the light level for world vertices
///   and the ambient-occlusion blend's alpha (about 1) for models.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct WorldVertex {
    pub pos: [u8; 4],
    pub color: u32,
}

impl WorldVertex {
    /// Red, green, blue, alpha of [`WorldVertex::color`].
    pub fn rgba(&self) -> [u8; 4] {
        let c = self.color;
        [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
    }

    /// The face index in `pos[3]`.
    pub fn face(&self) -> u8 {
        self.pos[3]
    }
}

/// `cvttss2si`: truncation toward zero, 0x80000000 for NaN and out-of-range values.
#[inline]
fn cvttss2si(v: f32) -> i32 {
    if v.is_nan() || v >= 2_147_483_648.0 || v < -2_147_483_648.0 { i32::MIN } else { v as i32 }
}

/// The face index `cube::CubeVertex` derives from the normal (`Cube.exe 0x004666f5..`): +x 0,
/// -x 1, +y 2, -y 3, +z 4, -z 5; any other vector keeps the initial 1.
fn face_index(n: [i32; 3]) -> u8 {
    match n {
        [1, 0, 0] => 0,
        [-1, 0, 0] => 1,
        [0, 1, 0] => 2,
        [0, -1, 0] => 3,
        [0, 0, 1] => 4,
        [0, 0, -1] => 5,
        _ => 1,
    }
}

/// `cube::CubeVertex::CubeVertex(pos, normal, color)`, `Cube.exe 0x00466650`: position bytes are
/// the low bytes of the integers, the colour channels are multiplied by 255 in single precision
/// and truncated.
pub fn cube_vertex(pos: [i32; 3], normal: [i32; 3], color: [f32; 4]) -> WorldVertex {
    let ch = |v: f32| cvttss2si(v * 255.0f32) as u32;
    let a = ch(color[3]);
    let r = ch(color[0]) & 0xff;
    let g = ch(color[1]) & 0xff;
    let b = ch(color[2]) & 0xff;
    let c = (((a << 8 | r) << 8 | g) << 8) | b;
    WorldVertex { pos: [pos[0] as u8, pos[1] as u8, pos[2] as u8, face_index(normal)], color: c }
}

/// One `cube::ChunkBuffer` (vtable 0x006ffdb0, 0x20 bytes): the geometry of one 250-block slab of
/// a chunk. Only slabs with vertices get one (`Cube.exe 0x004a1290`).
///
/// | Offset | Field |
/// |---|---|
/// | +4 | VB of `vertices` |
/// | +8 | IB of `indices` (null when empty) |
/// | +0xc | IB of `indices2` (null when empty) |
/// | +0x10 | vertex count |
/// | +0x14 | `indices.len() / 3` |
/// | +0x18 | `indices2.len() / 3` |
/// | +0x1c | [`ChunkMesh::base_z`] |
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChunkMesh {
    /// World z of local z 0.
    pub base_z: i32,
    pub vertices: Vec<WorldVertex>,
    /// Opaque faces (every block type but water).
    pub indices: Vec<u32>,
    /// The second index buffer: every face of water blocks (type 2) and the extra top quad of
    /// type-3 blocks under open air (colour (0, 0, 1)). Drawn in a separate pass.
    pub indices2: Vec<u32>,
}

/// Everything `buildChunkMesh` leaves on the `cube::Chunk` (0x268 bytes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChunkBuild {
    /// Chunk coordinates (`chunk+0x18`, `+0x1c`), in units of 32 blocks.
    pub cx: i32,
    pub cy: i32,
    /// The slab buffers in slab order (the list at `chunk+0x10`).
    pub buffers: Vec<ChunkMesh>,
    /// Lowest and highest exposed block z from pass 1 (see [`exposed_z_range`]).
    pub min_z: i32,
    pub max_z: i32,
    /// `chunk+0x20..0x38`: `(x0, y0, min_z)` in 16.16 fixed blocks.
    pub bounds_min: [i64; 3],
    /// `chunk+0x38..0x50`: `(x0 + 33, y0 + 33, max_z + 1)` in 16.16 fixed blocks.
    pub bounds_max: [i64; 3],
    /// `chunk+0x23c`: vertex count over the non-empty slabs.
    pub vertex_count: usize,
    /// The chunk's props with their light level (`chunk+0x248` list), see [`chunk_props`].
    pub props: Vec<Prop>,
}

/// `Cube.exe 0x0046f490`: the mesh thread builds a chunk only when its zone is loaded and, for a
/// chunk on a zone's first or last row/column of chunks (local block 0 or 0xe0), also the zone
/// across that edge and, at a corner, the diagonal zone and both side zones.
pub fn chunk_ready(world: &World, cx: i32, cy: i32) -> bool {
    if cx < 0 || cy < 0 {
        return false;
    }
    let (x0, y0) = (cx.wrapping_mul(CHUNK_SIZE), cy.wrapping_mul(CHUNK_SIZE));
    if x0 >= 0x100_0000 || y0 >= 0x100_0000 {
        return false;
    }
    let (zx, zy) = (x0 / 256, y0 / 256);
    if world.zone(zx, zy).is_none() {
        return false;
    }
    let (lx, ly) = (x0 & 0xff, y0 & 0xff);
    let dx = if lx == 0xe0 { 1 } else if lx == 0 { -1 } else { 0 };
    let dy = if ly == 0xe0 { 1 } else if ly == 0 { -1 } else { 0 };
    if dx == 0 && dy == 0 {
        return true;
    }
    // The neighbour chunk's zone, computed from chunk coordinates as the original does.
    let zone_of = |cx: i32, cy: i32| {
        let (x, y) = (cx * CHUNK_SIZE, cy * CHUNK_SIZE);
        world.zone(div256(x), div256(y))
    };
    if zone_of(cx + dx, cy + dy).is_none() {
        return false;
    }
    if dx == 0 || dy == 0 {
        return true;
    }
    zone_of(cx + dx, cy).is_some() && world.zone(zx, div256((cy + dy) * CHUNK_SIZE)).is_some()
}

/// `(v + ((v >> 31) & 0xff)) >> 8`: the original's signed division by 256 (truncating).
#[inline]
fn div256(v: i32) -> i32 {
    (v + ((v >> 31) & 0xff)) >> 8
}

/// The lowest column base among a column and its four side neighbours, and the column's top
/// (at least 1): the z range `lo - 1 .. top` both passes walk (`Cube.exe 0x0049dc9a..0x0049de41`
/// and `0x0049e1a0..0x0049e36b`).
fn column_z_range(world: &World, x: i32, y: i32, col: &Column) -> (i32, i32) {
    let mut top = col.height + col.blocks.len() as i32;
    if top < 1 {
        top = 1;
    }
    let mut lo = col.height;
    for (nx, ny) in [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)] {
        if let Some(c) = world.column(nx, ny)
            && c.height < lo
        {
            lo = c.height;
        }
    }
    (lo, top)
}

/// Whether a face of a block of type `t` toward a neighbour block is drawn: toward air always,
/// toward water unless the block is water itself (`Cube.exe 0x0049dedf..` in pass 1 and every
/// face test of pass 2).
#[inline]
fn face_open(t: u8, nb: Block) -> bool {
    let nt = nb[3] & 0x1f;
    nt == 0 || (t != 2 && nt == 2)
}

/// Pass 1 of `buildChunkMesh`, `Cube.exe 0x0049db89..0x0049e04d`: the lowest and highest z of
/// the blocks with at least one drawn face, over the chunk's columns (x outer, y inner, z up).
///
/// Quirk kept: before walking a column, the highest z is also raised to the top of the column
/// at `(x, y + 1)` (`0x0049de26`, the register still holding that neighbour). The original
/// dereferences a null column there when `(x, y + 1)` lies in an unloaded zone; [`chunk_ready`]
/// keeps that from happening and the port skips the update instead.
pub fn exposed_z_range(world: &World, x0: i32, y0: i32) -> (i32, i32) {
    let mut r = ExposedZ::default();
    for x in x0..x0 + CHUNK_SIZE {
        exposed_z_row(world, &mut r, x, y0);
    }
    (r.min_z, r.max_z)
}

/// The running state of pass 1 ([`exposed_z_range`]) between rows of columns.
#[derive(Clone, Copy, Debug)]
struct ExposedZ {
    first: bool,
    min_z: i32,
    max_z: i32,
}

impl Default for ExposedZ {
    fn default() -> Self {
        ExposedZ { first: true, min_z: 0, max_z: 0 }
    }
}

/// Pass 1 over the chunk's columns `(x, y0..y0 + 32)` (one step of the x loop).
fn exposed_z_row(world: &World, r: &mut ExposedZ, x: i32, y0: i32) {
    for y in y0..y0 + CHUNK_SIZE {
        let Some(col) = world.column(x, y) else { continue };
        let (lo, top) = column_z_range(world, x, y, col);
        if let Some(c) = world.column(x, y + 1) {
            let t = c.height + c.blocks.len() as i32;
            if t > r.max_z {
                r.max_z = t;
            }
        }
        for z in lo - 1..top {
            let t = block_in_column(col, z)[3] & 0x1f;
            if t == 0 {
                continue;
            }
            // 0x0049decb..0x0049dfd3: hidden when no neighbour shows a face.
            let exposed = [(x - 1, y, z), (x + 1, y, z), (x, y - 1, z), (x, y + 1, z), (x, y, z - 1), (x, y, z + 1)]
                .into_iter()
                .any(|(nx, ny, nz)| face_open(t, get_block(world, nx, ny, nz)));
            if !exposed {
                continue;
            }
            // 0x0049dfd5..0x0049dffb
            if r.first || z < r.min_z {
                r.min_z = z;
                if r.first || z > r.max_z {
                    r.max_z = z;
                }
            } else if z > r.max_z {
                r.max_z = z;
            }
            r.first = false;
        }
    }
}

/// The four corners of each face as offsets from the block, in the original's vertex order,
/// with the face normal. Order of the faces as tested: -x, +x, -y, +y, -z, +z.
const FACES: [([i32; 3], [[i32; 3]; 4]); 6] = [
    ([-1, 0, 0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]),
    ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
    ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
    ([0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]),
    ([0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
    ([0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]),
];

/// Which index buffer a quad goes to.
#[derive(Clone, Copy)]
enum Target {
    Opaque,
    Second,
}

/// One quad: the six indices first (`FUN_0046eeb0`, or inlined for the x faces), then the four
/// vertices, each lit by [`vertex_light`] at its world corner (`Cube.exe 0x0049ef4c..` for -x).
#[allow(clippy::too_many_arguments)]
fn emit_quad(world: &World, slab: &mut ChunkMesh, target: Target, face: usize, world_pos: [i32; 3], local: [i32; 3], color: [f32; 4]) {
    let (normal, corners) = FACES[face];
    let base = slab.vertices.len() as u32;
    let idx = match target {
        Target::Opaque => &mut slab.indices,
        Target::Second => &mut slab.indices2,
    };
    idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    for c in corners {
        let lit = vertex_light(world, world_pos[0] + c[0], world_pos[1] + c[1], world_pos[2] + c[2], color, normal, false);
        slab.vertices.push(cube_vertex([local[0] + c[0], local[1] + c[1], local[2] + c[2]], normal, lit));
    }
}

/// The face colour of a block (`Cube.exe 0x0049e426..0x0049e9c8`), RGBA in 0..1.
///
/// Normally the block's bytes / 255. Blocks under the column's base (they read as the "below"
/// block) blend the column's bottom block into the terrain colour: with
/// `f = min((base - z) * 0.1, 1)`, the terrain colour alone when the bottom block is air or
/// water (or the column is empty), else `bottom * (1 - f) / 255 + terrain * f / 255`.
fn block_color(world: &World, x: i32, y: i32, z: i32, col: &Column, b: Block) -> [f32; 4] {
    let mut color = [f32::from(b[0]) / 255.0f32, f32::from(b[1]) / 255.0f32, f32::from(b[2]) / 255.0f32, 1.0f32];
    if z < col.height {
        let mut f = (col.height - z) as f32 * 0.1f32;
        if f > 1.0 {
            f = 1.0;
        }
        // 0x0049e52f: an empty column reads the static (255, 255, 255, 0) of blockAt.
        let bottom = col.blocks.first().copied().unwrap_or([255, 255, 255, 0]);
        let bt = bottom[3] & 0x1f;
        // `cube::World::terrainColor`, Cube.exe 0x005f9620 (the server's 0x0052d030).
        let tc = world.terrain_color(x, y, z);
        if bt == 0 || bt == 2 {
            color = [tc[0] / 255.0f32, tc[1] / 255.0f32, tc[2] / 255.0f32, 1.0];
        } else {
            let inv = 1.0f32 - f;
            let part = |c: u8, t: f32| (t * f) / 255.0f32 + (f32::from(c) * inv) / 255.0f32;
            color = [part(bottom[0], tc[0]), part(bottom[1], tc[1]), part(bottom[2], tc[2]), 1.0];
        }
    }
    color
}

/// Pass 2 for one column (`Cube.exe 0x0049e0df..0x004a08a5`): every block of the column's z
/// range that is not air, with its faces tested in the order -x, +x, -y, +y, -z, +z.
#[allow(clippy::too_many_arguments)]
fn mesh_column(world: &World, slabs: &mut [ChunkMesh], x0: i32, y0: i32, x: i32, y: i32, col: &Column, min_z: i32) {
    let (lo, top) = column_z_range(world, x, y, col);
    for z in lo - 1..top {
        // 0x0049e380: slab of this z (truncating division).
        let slab_index = (z - min_z) / SLAB_HEIGHT;
        let base_z = slab_index * SLAB_HEIGHT + min_z;
        let b = block_in_column(col, z);
        let t = b[3] & 0x1f;
        if t == 0 {
            continue;
        }
        let mut color = block_color(world, x, y, z, col, b);
        let mut target = Target::Opaque;
        if t == 2 {
            // 0x0049e9c8..0x0049ebd5: water takes a blue tint lit by its blue byte (the
            // propagated light of a water block, see `light::compute_light`).
            let bl = color[2];
            let inv = 1.0f32 - bl;
            color = [0.1f32 * inv + 0.3f32 * bl, 0.2f32 * inv + 0.4f32 * bl, 1.0f32 * inv + 1.0f32 * bl, 1.0f32 * inv + 1.0f32 * bl];
            target = Target::Second;
        }
        // Every face lies in the slab of its block; exposed blocks are always within
        // [min_z, max_z], so the index is in range whenever a face is emitted.
        let wp = [x, y, z];
        let local = [x - x0, y - y0, z - base_z];
        let neighbours = [(x - 1, y, z), (x + 1, y, z), (x, y - 1, z), (x, y + 1, z), (x, y, z - 1), (x, y, z + 1)];
        for (face, (nx, ny, nz)) in neighbours.into_iter().enumerate() {
            let nb = get_block(world, nx, ny, nz);
            if !face_open(t, nb) {
                continue;
            }
            let slab = &mut slabs[slab_index as usize];
            if face == 5 && t == 3 && nb[3] & 0x1f == 0 {
                // 0x004a01f0..0x004a0588: a type-3 block under open air (not water) gets an extra
                // top quad in the second buffer, coloured (0, 0, 1, 1), before its normal one.
                emit_quad(world, slab, Target::Second, face, wp, local, [0.0, 0.0, 1.0, 1.0]);
            }
            emit_quad(world, slab, target, face, wp, local, color);
        }
    }
}

/// The chunk's props (`Cube.exe 0x004a0cbc..0x004a0e8e`): every prop of the zone holding the
/// chunk origin whose position, `trunc(trunc(p * 0.03125) / 65536)` per axis (the double
/// multiply, `_ftol`, `__alldiv` of the original), falls in this chunk gets its `f28` set to the
/// [`crate::light::prop_light`] of the block it stands in; the chunk keeps copies, in list order.
pub fn chunk_props(world: &mut World, cx: i32, cy: i32) -> Vec<Prop> {
    let (x0, y0) = (cx * CHUNK_SIZE, cy * CHUNK_SIZE);
    let (zx, zy) = (div256(x0), div256(y0));
    let Some(zone) = world.zone(zx, zy) else { return Vec::new() };
    let chunk_of = |v: i64| (((v as f64) * 0.03125) as i64 / 0x10000) as i32;
    let lights: Vec<(usize, f32)> = zone
        .props
        .iter()
        .enumerate()
        .filter(|(_, p)| chunk_of(p.y) == cy && chunk_of(p.x) == cx)
        .map(|(i, p)| {
            let b = get_block(world, (p.x / 0x10000) as i32, (p.y / 0x10000) as i32, (p.z / 0x10000) as i32);
            (i, f32::from(crate::light::prop_light(b)))
        })
        .collect();
    let zone = world.zone_mut(zx, zy).expect("zone seen above");
    lights
        .into_iter()
        .map(|(i, l)| {
            zone.props[i].f28 = l;
            zone.props[i].clone()
        })
        .collect()
}

/// `buildChunkMesh(cx, cy)`, `Cube.exe 0x0049d910`: the mesh of chunk `(cx, cy)` (blocks
/// `[32 cx, 32 cx + 32) x [32 cy, 32 cy + 32)`), or `None` for coordinates the original rejects
/// (negative or past block 0xffffff).
///
/// `dirty` is the chunk's `+0x74` flag (set after block edits). When it is set, or when the chunk
/// touches a zone edge (local block 0 or 0xe0 on either axis), the chunk and a margin (4 blocks
/// when dirty, 16 otherwise) are relit first with [`compute_light`]; other chunks rely on the
/// light zone generation computed.
///
/// Faces are culled against the neighbouring blocks through `getBlock`, across chunk and zone
/// borders alike: a neighbour in an unloaded zone reads as the opaque "below" block, so faces
/// toward it are dropped and its light samples count as opaque. [`chunk_ready`] says which
/// zones the mesh thread waits for.
pub fn build_chunk_mesh(world: &mut World, cx: i32, cy: i32, dirty: bool) -> Option<ChunkBuild> {
    build_chunk_mesh_with(&mut DirectAccess(world), cx, cy, dirty)
}

/// How [`build_chunk_mesh_with`] reaches the world: once per unit of work (each row of columns
/// of each relight step, each row of columns of pass 1, each column of pass 2, the props), so a
/// caller can take its lock per unit instead of for the whole build.
pub trait WorldAccess {
    /// Runs `f` with the world readable.
    fn read(&mut self, f: &mut dyn FnMut(&World));
    /// Runs `f` with the world writable.
    fn write(&mut self, f: &mut dyn FnMut(&mut World));
}

/// [`WorldAccess`] over a world the caller already holds.
pub struct DirectAccess<'a>(pub &'a mut World);

impl WorldAccess for DirectAccess<'_> {
    fn read(&mut self, f: &mut dyn FnMut(&World)) {
        f(self.0)
    }
    fn write(&mut self, f: &mut dyn FnMut(&mut World)) {
        f(self.0)
    }
}

/// [`build_chunk_mesh`] with the world reached through `access` per unit of work (the relight,
/// then pass 1 one row of 32 columns and pass 2 one column at a time, then the props). Between
/// two units the world may change (the original's mesher holds no lock at all while it builds;
/// Tier C threading); with no change in between the result is exactly [`build_chunk_mesh`]'s.
///
/// The relight is reached once per row of columns of each of its steps
/// ([`compute_light_in_steps`]), so no unit under the write lock covers more than 64 columns.
pub fn build_chunk_mesh_with(access: &mut dyn WorldAccess, cx: i32, cy: i32, dirty: bool) -> Option<ChunkBuild> {
    if cx < 0 || cy < 0 {
        return None;
    }
    let (x0, y0) = (cx.wrapping_mul(CHUNK_SIZE), cy.wrapping_mul(CHUNK_SIZE));
    if x0 > 0xff_ffff || y0 > 0xff_ffff {
        return None;
    }

    // 0x0049db10..0x0049db89
    let (lx, ly) = (x0 & 0xff, y0 & 0xff);
    if dirty || lx == 0 || ly == 0 || lx == 0xe0 || ly == 0xe0 {
        let r = if dirty { 4 } else { 16 };
        // One unit per row of columns of each relight step.
        compute_light_in_steps::<World>(&mut |step| access.write(step), x0 - r, y0 - r, x0 + CHUNK_SIZE + r, y0 + CHUNK_SIZE + r, 0);
    }

    // Pass 1 (`exposed_z_range`), one row of columns per unit.
    let mut range = ExposedZ::default();
    for x in x0..x0 + CHUNK_SIZE {
        access.read(&mut |w| exposed_z_row(w, &mut range, x, y0));
    }
    let (min_z, max_z) = (range.min_z, range.max_z);
    // 0x0049e04d: signed division by 250.
    let slab_count = ((max_z - min_z) / SLAB_HEIGHT + 1) as usize;
    let mut slabs: Vec<ChunkMesh> =
        (0..slab_count).map(|i| ChunkMesh { base_z: min_z + i as i32 * SLAB_HEIGHT, ..Default::default() }).collect();

    // Pass 2, one column per unit (x outer, y inner, as the original's loops).
    for x in x0..x0 + CHUNK_SIZE {
        for y in y0..y0 + CHUNK_SIZE {
            access.read(&mut |w| {
                if let Some(col) = w.column(x, y) {
                    mesh_column(w, &mut slabs, x0, y0, x, y, col, min_z);
                }
            });
        }
    }

    let mut props = Vec::new();
    access.write(&mut |world| props = chunk_props(world, cx, cy));

    // 0x004a0e8e..0x004a1350: only slabs with vertices get a ChunkBuffer.
    let buffers: Vec<ChunkMesh> = slabs.into_iter().filter(|s| !s.vertices.is_empty()).collect();
    let vertex_count = buffers.iter().map(|b| b.vertices.len()).sum();
    let fx = |v: i32| i64::from(v) << 16;
    Some(ChunkBuild {
        cx,
        cy,
        buffers,
        min_z,
        max_z,
        bounds_min: [fx(x0), fx(y0), fx(min_z)],
        bounds_max: [fx(x0 + CHUNK_SIZE + 1), fx(y0 + CHUNK_SIZE + 1), fx(max_z + 1)],
        vertex_count,
        props,
    })
}

/// A voxel model's mesh, as `cube::Sprite::buildMesh` (`Cube.exe 0x004e7870`) leaves it: one VB
/// of [`WorldVertex`] (`sprite+0x34`) and one INDEX16 IB (`sprite+0x38`); `+0x3c` holds the
/// vertex count and `+0x40` the triangle count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelMesh {
    pub vertices: Vec<WorldVertex>,
    pub indices: Vec<u16>,
}

/// The value of an empty voxel, and of every voxel outside the model: the static at
/// `Cube.exe 0x0076b340`. It is zero-initialised and model editing code stores out-of-range
/// writes into it (`0x005faff2`, `0x00603152`, ...), so in the original it can briefly hold
/// another colour; the port assumes the (0, 0, 0) it holds otherwise.
pub const EMPTY_VOXEL: [u8; 3] = [0, 0, 0];

/// `Cube.exe 0x004e71d0(voxel, raw)`: empty is (0, 0, 0); unless `raw` (`sprite+0x55`), the pure
/// red, green and blue marker voxels (attachment points, collected by `setVoxels 0x004e7650`)
/// count as empty too.
pub fn voxel_is_empty(v: [u8; 3], raw: bool) -> bool {
    if v == EMPTY_VOXEL {
        return true;
    }
    if raw {
        return false;
    }
    v == [255, 0, 0] || v == [0, 255, 0] || v == [0, 0, 255]
}

/// A voxel grid in the layout of `sprite+0x30`: `size` along x, y, z and RGB triples indexed
/// `(z * size_y + y) * size_x + x` (the `.cub` file order).
#[derive(Clone, Copy)]
pub struct VoxelGrid<'a> {
    pub size: [i32; 3],
    pub voxels: &'a [[u8; 3]],
}

impl VoxelGrid<'_> {
    /// The voxel at `(x, y, z)`, [`EMPTY_VOXEL`] outside the grid.
    pub fn at(&self, x: i32, y: i32, z: i32) -> [u8; 3] {
        let [sx, sy, sz] = self.size;
        if x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz {
            return EMPTY_VOXEL;
        }
        self.voxels[((z * sy + y) * sx + x) as usize]
    }

    /// `Cube.exe 0x004eb8d0(x, y, z, normal)`: the fraction of empty voxels (exactly
    /// [`EMPTY_VOXEL`]) among the 6x6 in front of a face corner, the same sample box as
    /// [`crate::light::vertex_light`], unweighted: `count as f32 / total as f32`, 0 for an
    /// empty box.
    pub fn occlusion(&self, x: i32, y: i32, z: i32, normal: [i32; 3]) -> f32 {
        let range = |n: i32| if n > 0 { (0, 1) } else if n < 0 { (-1, 0) } else { (-3, 3) };
        let (xa, xb) = range(normal[0]);
        let (ya, yb) = range(normal[1]);
        let (za, zb) = range(normal[2]);
        let (mut total, mut empty) = (0i32, 0i32);
        if xa < xb {
            for dx in xa..xb {
                for dy in ya..yb {
                    total += zb - za;
                    for dz in za..zb {
                        if self.at(x + dx, y + dy, z + dz) == EMPTY_VOXEL {
                            empty += 1;
                        }
                    }
                }
            }
            if total > 0 {
                return empty as f32 / total as f32;
            }
        }
        0.0
    }
}

/// `cube::Sprite::buildMesh`, `Cube.exe 0x004e7870`: the mesh of a voxel model.
///
/// Voxels are visited x outer, y, z inner; a voxel that [`voxel_is_empty`] (with `raw`) is
/// skipped. A face is drawn when the neighbour voxel is exactly [`EMPTY_VOXEL`] (x and y faces
/// compare the bytes; z faces call `0x004e71d0` with `raw` = 1, the same test), so hidden marker
/// voxels still occlude. Faces in the order -x, +x, -y, +y, -z, +z with the corners of the world
/// mesher. Each corner's colour blends the voxel toward the model tint (`sprite+0x5c..0x5e`,
/// zero from the constructor) by the corner's [`VoxelGrid::occlusion`] `ao`:
/// `c * ao + (1 - ao) * tint` per channel, alpha `1 * ao + (1 - ao) * 1`.
///
/// The index buffer is filled afterwards, `4i, 4i+1, 4i+2, 4i, 4i+2, 4i+3` per quad in 16-bit
/// arithmetic (it wraps past 16384 quads as the original's shorts do).
pub fn build_model_mesh(grid: VoxelGrid<'_>, tint: [u8; 3], raw: bool) -> ModelMesh {
    let mut vertices = Vec::new();
    let [sx, sy, sz] = grid.size;
    let tint = [f32::from(tint[0]) / 255.0f32, f32::from(tint[1]) / 255.0f32, f32::from(tint[2]) / 255.0f32];
    for x in 0..sx {
        for y in 0..sy {
            for z in 0..sz {
                let v = grid.at(x, y, z);
                if voxel_is_empty(v, raw) {
                    continue;
                }
                let c = [f32::from(v[0]) / 255.0f32, f32::from(v[1]) / 255.0f32, f32::from(v[2]) / 255.0f32];
                for (normal, corners) in FACES {
                    let nb = grid.at(x + normal[0], y + normal[1], z + normal[2]);
                    if nb != EMPTY_VOXEL {
                        continue;
                    }
                    for k in corners {
                        let p = [x + k[0], y + k[1], z + k[2]];
                        let ao = grid.occlusion(p[0], p[1], p[2], normal);
                        let inv = 1.0f32 - ao;
                        let col = [c[0] * ao + inv * tint[0], c[1] * ao + inv * tint[1], c[2] * ao + inv * tint[2], ao * 1.0f32 + inv * 1.0f32];
                        vertices.push(cube_vertex(p, normal, col));
                    }
                }
            }
        }
    }
    let quads = vertices.len() / 4;
    let mut indices = Vec::with_capacity(quads * 6);
    for i in 0..quads {
        let s = (i as u16).wrapping_mul(4);
        indices.extend_from_slice(&[s, s.wrapping_add(1), s.wrapping_add(2), s, s.wrapping_add(2), s.wrapping_add(3)]);
    }
    ModelMesh { vertices, indices }
}

/// [`build_model_mesh`] for a parsed `.cub` file (sizes and voxels in file order).
pub fn build_cub_mesh(model: &cw_formats::CubModel, tint: [u8; 3], raw: bool) -> ModelMesh {
    let size = model.size.map(|s| s as i32);
    build_model_mesh(VoxelGrid { size, voxels: &model.voxels }, tint, raw)
}

/// [`build_model_mesh`] for a world model table entry (`cw_world::model::Model`, whose `raw` is
/// `sprite+0x55`).
pub fn build_world_model_mesh(model: &cw_world::model::Model, tint: [u8; 3]) -> ModelMesh {
    build_model_mesh(VoxelGrid { size: model.size, voxels: &model.voxels }, tint, model.raw)
}
