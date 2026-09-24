//! Chunk and model meshing on synthetic input, with hand-checked face counts and light values.

use cw_render::light::{ZoneColumns, compute_light, get_block};
use cw_render::mesh::{ChunkBuild, VoxelGrid, WorldVertex, build_chunk_mesh, build_model_mesh, chunk_ready};
use cw_world::World;
use cw_world::zone::Zone;

const ZX: i32 = 128;
const ZY: i32 = 128;
/// A chunk in the middle of the zone (local block 96: no zone edge, no relight).
const CX: i32 = ZX * 8 + 3;
const CY: i32 = ZY * 8 + 3;
const X0: i32 = CX * 32;
const Y0: i32 = CY * 32;
/// The pair of blocks at `(BX, BY, 5)`, `(BX + 1, BY, 5)`; the lone block at `(BX + 5, BY + 5, 10)`.
const BX: i32 = X0 + 10;
const BY: i32 = Y0 + 10;

/// A flat zone: every column has its base at 1 and no stored blocks, so z <= 0 is the opaque
/// "below" block and z >= 1 is air.
fn flat_zone(zx: i32, zy: i32) -> Box<Zone> {
    let mut zone = Box::new(Zone::new(zx, zy));
    for c in &mut zone.columns {
        c.height = 1;
    }
    zone
}

fn put(zone: &mut Zone, x: i32, y: i32, z: i32, b: [u8; 4]) {
    let col = zone.column_mut(x, y);
    let h = col.height;
    col.set_raw(z - h, b);
}

/// The three-block world, lit the way zone generation lights a zone (`Cube.exe 0x005ede84`).
fn three_block_world(extra: impl FnOnce(&mut Zone)) -> World {
    let mut world = World::new(1234);
    let mut zone = flat_zone(ZX, ZY);
    put(&mut zone, BX, BY, 5, [255, 0, 255, 1]);
    put(&mut zone, BX + 1, BY, 5, [255, 0, 255, 1]);
    put(&mut zone, BX + 5, BY + 5, 10, [255, 255, 255, 1]);
    extra(&mut zone);
    let (x0, y0) = (ZX * 256, ZY * 256);
    compute_light(&mut ZoneColumns(&mut zone), x0, y0, x0 + 256, y0 + 256, 0);
    world.insert_zone(zone);
    world
}

fn faces(build: &ChunkBuild) -> [usize; 6] {
    let mut n = [0; 6];
    for b in &build.buffers {
        for v in b.vertices.chunks(4) {
            assert!(v.iter().all(|w| w.face() == v[0].face()), "a quad has one face index");
            n[v[0].face() as usize] += 1;
        }
    }
    n
}

/// The vertex at local `(x, y, z)` with face index `face`.
fn vertex_at(build: &ChunkBuild, x: i32, y: i32, z: i32, face: u8) -> WorldVertex {
    let b = &build.buffers[0];
    let lz = z - b.base_z;
    *b.vertices
        .iter()
        .find(|v| v.pos == [(x - X0) as u8, (y - Y0) as u8, lz as u8, face])
        .unwrap_or_else(|| panic!("no vertex at ({x}, {y}, {z}) face {face}"))
}

#[test]
fn light_propagates_under_blocks() {
    let world = three_block_world(|_| {});
    // Air under an opaque block, next to open columns: 255 * 85 / 100.
    assert_eq!(get_block(&world, BX + 5, BY + 5, 9)[0], 216);
    assert_eq!(get_block(&world, BX + 5, BY + 5, 1)[0], 216);
    assert_eq!(get_block(&world, BX, BY, 4)[0], 216);
    // Nothing above the lone block is stored: it reads as the static air block.
    assert_eq!(get_block(&world, BX + 5, BY + 5, 11), [255, 255, 255, 0]);
}

#[test]
fn three_blocks_face_counts() {
    let mut world = three_block_world(|_| {});
    assert!(chunk_ready(&world, CX, CY));
    let build = build_chunk_mesh(&mut world, CX, CY, false).unwrap();
    // One slab: exposed z from 0 (the ground's top faces) to 10, raised to 11 by the (x, y + 1)
    // column top quirk (the lone block's column seen from its -y neighbour).
    assert_eq!((build.min_z, build.max_z), (0, 11));
    assert_eq!(build.buffers.len(), 1);
    let b = &build.buffers[0];
    assert_eq!(b.base_z, 0);
    // 32 * 32 ground tops, 10 faces for the pair (the shared ones culled), 6 for the lone block.
    assert_eq!(b.vertices.len(), 4 * (1024 + 10 + 6));
    assert_eq!(b.indices.len(), 6 * 1040);
    assert!(b.indices2.is_empty());
    assert_eq!(build.vertex_count, b.vertices.len());
    // Per face index: +x, -x, +y, -y, +z, -z.
    assert_eq!(faces(&build), [2, 2, 3, 3, 1027, 3]);
    // Quads index their own four vertices as 0 1 2 0 2 3.
    for (q, i) in b.indices.chunks(6).enumerate() {
        let s = 4 * q as u32;
        assert_eq!(i, [s, s + 1, s + 2, s, s + 2, s + 3]);
    }
    assert_eq!(build.bounds_min, [i64::from(X0) << 16, i64::from(Y0) << 16, 0]);
    assert_eq!(build.bounds_max, [i64::from(X0 + 33) << 16, i64::from(Y0 + 33) << 16, 12 << 16]);
}

#[test]
fn three_blocks_light_values() {
    let mut world = three_block_world(|_| {});
    let build = build_chunk_mesh(&mut world, CX, CY, false).unwrap();

    // Top of the lone block: every sample is sky air (255), so the corner is fully open and
    // keeps the block colour; the light is 255 up to float rounding of sum(255 w) / (255 sum w).
    let v = vertex_at(&build, BX + 5, BY + 5, 11, 4);
    let [r, g, b, a] = v.rgba();
    assert_eq!([r, g, b], [255, 255, 255]);
    assert!(a >= 254, "light {a}");

    // Bottom of the lone block at its (0, 0) corner: of the 6x6 samples at z = 9 only the one
    // under the block is 216, with weight (3 - 0.5) / 3. The weights sum to 38 / 3 (4 samples at
    // 2.5/3, 12 at 1.5/3, 20 at 0.5/3), so light = (255 * 38/3 - 39 * 2.5/3) / (255 * 38/3)
    // = 3197.5 / 3230 = 0.98994, times 255 = 252.4.
    let v = vertex_at(&build, BX + 5, BY + 5, 10, 5);
    assert_eq!(v.rgba(), [255, 255, 255, 252]);

    // Bottom of the pair at (BX, BY): the samples under both blocks are 216, weights 2.5/3 and
    // 1.5/3: (3230 - 39 * 4/3) / 3230 = 0.98390, times 255 = 250.9. Colour (255, 0, 255).
    let v = vertex_at(&build, BX, BY, 5, 5);
    assert_eq!(v.rgba(), [255, 0, 255, 250]);
}

#[test]
fn water_and_type3_go_to_the_second_buffer() {
    let (wx, wy) = (BX + 12, BY + 12);
    let (tx, ty) = (BX + 15, BY + 3);
    let mut world = three_block_world(|zone| {
        put(zone, wx, wy, 3, [0, 0, 0, 2]);
        put(zone, tx, ty, 2, [0, 255, 0, 3]);
    });
    let build = build_chunk_mesh(&mut world, CX, CY, false).unwrap();
    let b = &build.buffers[0];
    // Six water faces plus the type-3 block's extra top quad.
    assert_eq!(b.indices2.len(), 6 * 7);
    // Opaque: ground, pair, lone block, the type-3 block's six faces.
    assert_eq!(b.indices.len(), 6 * (1040 + 6));
    // Water under the sky has light 255 in its colour bytes, so its colour is
    // (0.3, 0.4, 1, 1) * 1 + (0.1, 0.2, 1, 1) * 0: 0.3f * 255 = 76.5, 0.4f * 255 = 102.
    let v = vertex_at(&build, wx, wy, 4, 4);
    let [r, g, bl, _] = v.rgba();
    assert_eq!([r, g, bl], [76, 102, 255]);
    // The type-3 block's two top quads: the blue one first (second buffer), then its colour.
    let tops: Vec<_> = b.vertices.iter().filter(|v| v.pos == [(tx - X0) as u8, (ty - Y0) as u8, 3, 4]).collect();
    assert_eq!(tops.len(), 2);
    assert_eq!(&tops[0].rgba()[..3], &[0, 0, 255]);
    assert_eq!(&tops[1].rgba()[..3], &[0, 255, 0]);
}

#[test]
fn faces_toward_an_unloaded_zone_are_culled() {
    // A chunk on the zone's last chunk column (local 0xe0): it needs zone ZX + 1.
    let (cx, cy) = (ZX * 8 + 7, CY);
    let edge_x = ZX * 256 + 255;
    let mut world = three_block_world(|zone| put(zone, edge_x, BY, 5, [10, 20, 30, 1]));
    assert!(!chunk_ready(&world, cx, cy));
    let alone = build_chunk_mesh(&mut world, cx, cy, false).unwrap();
    // +x of the edge block faces the missing zone: read as the opaque "below" block, dropped.
    assert_eq!(faces(&alone)[0], 0);
    assert_eq!(faces(&alone)[1..], [1, 1, 1, 1024 + 1, 1]);

    world.insert_zone(flat_zone(ZX + 1, ZY));
    assert!(chunk_ready(&world, cx, cy));
    let joined = build_chunk_mesh(&mut world, cx, cy, false).unwrap();
    assert_eq!(faces(&joined), [1, 1, 1, 1, 1024 + 1, 1]);
}

#[test]
fn chunk_mesh_is_deterministic() {
    let mut w1 = three_block_world(|_| {});
    let mut w2 = three_block_world(|_| {});
    let a = build_chunk_mesh(&mut w1, CX, CY, true).unwrap();
    let b = build_chunk_mesh(&mut w2, CX, CY, true).unwrap();
    let c = build_chunk_mesh(&mut w1, CX, CY, true).unwrap();
    for other in [&b, &c] {
        assert_eq!(a.buffers.len(), other.buffers.len());
        for (x, y) in a.buffers.iter().zip(&other.buffers) {
            assert_eq!(bytemuck::cast_slice::<_, u8>(&x.vertices), bytemuck::cast_slice::<_, u8>(&y.vertices));
            assert_eq!(x.indices, y.indices);
            assert_eq!(x.indices2, y.indices2);
        }
    }
    assert_eq!(std::mem::size_of::<WorldVertex>(), 8);
}

#[test]
fn vertex_bytes_are_d3d_order() {
    let v = cw_render::mesh::cube_vertex([1, 2, 3], [0, 0, -1], [1.0, 0.5, 0.0, 1.0]);
    // UBYTE4 position with the face in w, then D3DCOLOR as B, G, R, A bytes.
    assert_eq!(bytemuck::bytes_of(&v), &[1, 2, 3, 5, 0, 127, 255, 255]);
}

#[test]
fn model_single_voxel() {
    let voxels = [[255, 0, 255]];
    let m = build_model_mesh(VoxelGrid { size: [1, 1, 1], voxels: &voxels }, [0, 0, 0], false);
    assert_eq!(m.vertices.len(), 24);
    assert_eq!(m.indices.len(), 36);
    assert_eq!(&m.indices[6..12], &[4, 5, 6, 4, 6, 7]);
    // Nothing around: every corner is fully unoccluded, so the voxel colour comes through.
    for v in &m.vertices {
        assert_eq!(v.rgba(), [255, 0, 255, 255]);
    }
    let faces: Vec<u8> = m.vertices.chunks(4).map(|q| q[0].face()).collect();
    assert_eq!(faces, [1, 0, 3, 2, 5, 4]);
}

#[test]
fn model_markers_hidden_unless_raw() {
    let voxels = [[255, 0, 0]];
    let grid = VoxelGrid { size: [1, 1, 1], voxels: &voxels };
    assert!(build_model_mesh(grid, [0, 0, 0], false).vertices.is_empty());
    assert_eq!(build_model_mesh(grid, [0, 0, 0], true).vertices.len(), 24);
}

#[test]
fn model_occlusion() {
    // 2 x 1 x 2 grid (x fastest, then y, then z): voxels at (0, 0, 0) and (1, 0, 1).
    let e = [0, 0, 0];
    let w = [255, 255, 255];
    let voxels = [w, e, e, w];
    let m = build_model_mesh(VoxelGrid { size: [2, 1, 2], voxels: &voxels }, [0, 0, 0], false);
    assert_eq!(m.vertices.len(), 2 * 24);
    // The +z face of (0, 0, 0), corner (1, 0, 1): of the 36 samples at z = 1 one is the voxel
    // (1, 0, 1), so ao = 35 / 36 and the colour is 255 * 35/36 = 247.9 toward a black tint.
    let v = m.vertices.iter().find(|v| v.pos == [1, 0, 1, 4]).unwrap();
    assert_eq!(&v.rgba()[..3], &[247, 247, 247]);
}

/// The client's mesher takes the `World` write lock once per step of the relight, so a step
/// bounds how long the frame thread can wait for it: each step writes one row of columns (one
/// `x`) at most.
#[test]
fn relight_steps_write_one_row_each() {
    use cw_render::light::{ColumnAccess, compute_light_in_steps};
    use cw_world::zone::Column;
    use std::collections::BTreeSet;

    struct Rows<'a> {
        world: &'a mut World,
        written: BTreeSet<i32>,
    }
    impl ColumnAccess for Rows<'_> {
        fn column(&self, bx: i32, by: i32) -> Option<&Column> {
            ColumnAccess::column(&*self.world, bx, by)
        }
        fn column_mut(&mut self, bx: i32, by: i32) -> Option<&mut Column> {
            self.written.insert(bx);
            ColumnAccess::column_mut(&mut *self.world, bx, by)
        }
    }

    let mut world = three_block_world(|_| {});
    let reference = {
        let mut w = three_block_world(|_| {});
        compute_light(&mut w, X0 - 16, Y0 - 16, X0 + 48, Y0 + 48, 0);
        w
    };
    let mut rows = Rows { world: &mut world, written: BTreeSet::new() };
    let mut steps = 0;
    compute_light_in_steps::<Rows<'_>>(
        &mut |step| {
            rows.written.clear();
            step(&mut rows);
            assert!(rows.written.len() <= 1, "a step wrote rows {:?}", rows.written);
            steps += 1;
        },
        X0 - 16,
        Y0 - 16,
        X0 + 48,
        Y0 + 48,
        0,
    );
    // Sky pass and 16 x (update, copy) over 64 rows, the publish over 64 rows.
    assert_eq!(steps, 64 + 16 * 2 * 64 + 64);
    for x in X0 - 16..X0 + 48 {
        for y in Y0 - 16..Y0 + 48 {
            assert_eq!(world.column(x, y).map(|c| c.blocks.clone()), reference.column(x, y).map(|c| c.blocks.clone()), "column ({x}, {y})");
        }
    }
}

/// The client's mesher takes the `World` read lock once per unit of a build: a row of columns
/// of pass 1, a single column of pass 2 (meshing a whole row of 32 columns held the lock for up
/// to a millisecond), the props under the write lock; the result is the direct build's.
#[test]
fn chunk_build_units() {
    use cw_render::mesh::{WorldAccess, build_chunk_mesh_with};
    struct Counting<'a> {
        world: &'a mut World,
        reads: usize,
        writes: usize,
    }
    impl WorldAccess for Counting<'_> {
        fn read(&mut self, f: &mut dyn FnMut(&World)) {
            self.reads += 1;
            f(self.world)
        }
        fn write(&mut self, f: &mut dyn FnMut(&mut World)) {
            self.writes += 1;
            f(self.world)
        }
    }
    let mut w = three_block_world(|_| {});
    let direct = build_chunk_mesh(&mut three_block_world(|_| {}), CX, CY, false).unwrap();
    let mut access = Counting { world: &mut w, reads: 0, writes: 0 };
    let b = build_chunk_mesh_with(&mut access, CX, CY, false).unwrap();
    // An inner chunk, not dirty: no relight.
    assert_eq!((access.reads, access.writes), (32 + 32 * 32, 1));
    assert_eq!(b.buffers.len(), direct.buffers.len());
    for (x, y) in b.buffers.iter().zip(&direct.buffers) {
        assert_eq!(bytemuck::cast_slice::<_, u8>(&x.vertices), bytemuck::cast_slice::<_, u8>(&y.vertices));
    }
}
