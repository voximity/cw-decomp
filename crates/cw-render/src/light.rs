//! Block light for the world meshes (Tier A: the port computes the same bytes as Cube.exe).
//!
//! Two pieces:
//!
//! - [`compute_light`], `cube::World::computeLight` (`Cube.exe 0x0059a0e0`): the flood fill
//!   that stores a light level in bytes 0..2 of every air and water block (the colour bytes of
//!   those blocks carry no colour in the client). Zone generation runs it over the whole zone
//!   (`Cube.exe 0x005ede84`, zone as the column hint), `buildChunkMesh` over the chunk plus a
//!   margin, and the world tick after block edits (`Cube.exe 0x0060f071`).
//! - [`vertex_light`], `computeVertexLight` (`Cube.exe 0x004c1510`): the colour and light level
//!   of one corner of a block face, from the light bytes of the 6x6 blocks in front of it.
//!
//! Light bytes of an air/water block after [`compute_light`]:
//!
//! | byte | meaning |
//! |---|---|
//! | 0 | the final light level the mesher reads |
//! | 1 | scratch: this sweep's value |
//! | 2 | the previous sweep's value (255 = open to the sky) |
//!
//! Block types (`byte3 & 0x1f`) that matter here: 0 air, 2 water, 0xd a light source (counts
//! as 255); everything else is opaque. The `0x40` flag only keeps an air block at `z <= 0` from
//! reading as water (see [`get_block`]).

use cw_world::World;
use cw_world::surface::Block;
use cw_world::zone::{AIR_BLOCK, BELOW_BLOCK, Column, WATER_BLOCK, Zone};

/// Where [`compute_light`] and the meshers find block columns: `cube::World::getColumn(x, y,
/// hint)` (`Cube.exe 0x004347a0`). With no hint the world's zones are searched; with a zone hint
/// only that zone's columns exist (zone generation lights a zone before it is inserted).
pub trait ColumnAccess {
    /// The column holding block `(bx, by)`, if its zone is loaded.
    fn column(&self, bx: i32, by: i32) -> Option<&Column>;
    /// The same column, mutable.
    fn column_mut(&mut self, bx: i32, by: i32) -> Option<&mut Column>;
}

/// `getColumn(x, y, 0)`: bounds `0..0x1000000`, then the zone at `(x / 256, y / 256)`.
impl ColumnAccess for World {
    fn column(&self, bx: i32, by: i32) -> Option<&Column> {
        World::column(self, bx, by)
    }

    fn column_mut(&mut self, bx: i32, by: i32) -> Option<&mut Column> {
        if !(0..0x100_0000).contains(&bx) || !(0..0x100_0000).contains(&by) {
            return None;
        }
        Some(self.zone_mut(bx / 256, by / 256)?.column_mut(bx, by))
    }
}

/// `getColumn(x, y, zone)` with a zone hint: only the columns of that zone exist.
pub struct ZoneColumns<'a>(pub &'a mut Zone);

impl ColumnAccess for ZoneColumns<'_> {
    fn column(&self, bx: i32, by: i32) -> Option<&Column> {
        if !(0..0x100_0000).contains(&bx) || !(0..0x100_0000).contains(&by) || !self.0.contains(bx, by) {
            return None;
        }
        Some(self.0.column(bx, by))
    }

    fn column_mut(&mut self, bx: i32, by: i32) -> Option<&mut Column> {
        if !(0..0x100_0000).contains(&bx) || !(0..0x100_0000).contains(&by) || !self.0.contains(bx, by) {
            return None;
        }
        Some(self.0.column_mut(bx, by))
    }
}

/// `cube::World::getBlock(x, y, z, hint)` (`Cube.exe 0x0042f7e0`), also inlined in
/// `buildChunkMesh`, `computeVertexLight` and `computeLight`: no column reads as the "below"
/// block (200,200,200, type 1), under the column too; above the stored blocks is air for `z > 0`
/// and water (255,255,255, 0x82) for `z <= 0`; a stored air block at `z <= 0` without the 0x40
/// flag also reads as water. The static blocks are the same values as the server's
/// (initialisers `Cube.exe 0x006f9720`/`0x006f9750`/`0x006f9780` and their copies at
/// `0x006f9b10..`, `0x006f9cc0..`, `0x006faad0..`).
#[inline]
pub fn get_block<C: ColumnAccess + ?Sized>(cols: &C, x: i32, y: i32, z: i32) -> Block {
    let Some(col) = cols.column(x, y) else { return BELOW_BLOCK };
    block_in_column(col, z)
}

/// [`get_block`] once the column is known.
#[inline]
pub fn block_in_column(col: &Column, z: i32) -> Block {
    if z < col.height {
        return BELOW_BLOCK;
    }
    if col.height + col.blocks.len() as i32 <= z {
        return if z > 0 { AIR_BLOCK } else { WATER_BLOCK };
    }
    let b = col.blocks[(z - col.height) as usize];
    if b[3] & 0x1f == 0 && z < 1 && b[3] & 0x40 == 0 {
        return WATER_BLOCK;
    }
    b
}

/// True for the block types light passes through: air (0) and water (2).
#[inline]
fn is_clear(t: u8) -> bool {
    t == 0 || t == 2
}

/// The light a neighbour contributes in [`compute_light`]'s sweeps: 255 for a light source
/// (type 0xd), the previous sweep's value (byte 2, at least 5) for air and water, 0 otherwise.
#[inline]
fn sweep_light(b: Block) -> u32 {
    let t = b[3] & 0x1f;
    if t == 0xd {
        0xff
    } else if is_clear(t) {
        u32::from(b[2].max(5))
    } else {
        0
    }
}

/// `cube::World::computeLight(x0, y0, x1, y1, margin, hint)`, `Cube.exe 0x0059a0e0`.
///
/// Over the columns `[x0 - margin, x1 + margin) x [y0 - margin, y1 + margin)` (x outer, y inner;
/// missing columns are skipped):
///
/// 1. `0x0059a110`: walking each column from its top block down, air and water blocks above the
///    first opaque block get bytes 1 and 2 = 255 (sky), the others 0. Byte 0 is kept.
/// 2. `0x0059a1c2`, 16 times: every air/water block whose byte 2 is not 255 gets byte 1 =
///    `max(neighbour light) * 85 / 100` over its six neighbours (x-1, x+1, y-1, y+1, z-1, z+1,
///    read through [`get_block`], contribution from [`sweep_light`]); then, as a second pass over
///    the same columns, byte 2 = byte 1 for every air/water block (a Jacobi sweep).
/// 3. After the 16th sweep (the loop exit past `0x0059a75f`): over `[x0, x1) x [y0, y1)` only,
///    byte 0 = byte 2 for every air/water block.
///
/// Performance note for a future optimiser: each block re-looks-up its four side columns, as the
/// original does; 16 full sweeps of the region dominate chunk rebuilds.
pub fn compute_light<C: ColumnAccess + ?Sized>(cols: &mut C, x0: i32, y0: i32, x1: i32, y1: i32, margin: i32) {
    compute_light_in_steps::<C>(&mut |step| step(cols), x0, y0, x1, y1, margin);
}

/// A way to run one step of [`compute_light_in_steps`] with the columns writable.
pub type ColumnsStep<'a, C> = dyn FnMut(&mut dyn FnMut(&mut C)) + 'a;

/// [`compute_light`] with the columns reached through `with` once per step: the sky pass, each
/// of the sixteen sweeps (update and copy), the publish. A caller can take its lock per step
/// (the chunk mesher, so the frame never waits for a whole relight); with no change to the
/// columns between steps the result is exactly [`compute_light`]'s.
pub fn compute_light_in_steps<C: ColumnAccess + ?Sized>(
    with: &mut ColumnsStep<'_, C>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    margin: i32,
) {
    let (xa, xb) = (x0 - margin, x1 + margin);
    let (ya, yb) = (y0 - margin, y1 + margin);

    // 1. Sky pass, 0x0059a110..0x0059a1bc.
    with(&mut |cols| sky_pass(cols, xa, xb, ya, yb));

    // 2. Sixteen sweeps, 0x0059a1c2..0x0059a6d6 (update) and 0x0059a6d6.. (copy).
    let mut scratch: Vec<Option<u8>> = Vec::new();
    for _ in 0..16 {
        with(&mut |cols| sweep(cols, &mut scratch, xa, xb, ya, yb));
    }

    // 3. Publish (no margin).
    with(&mut |cols| publish(cols, x0, x1, y0, y1));
}

/// Step 1 of [`compute_light`].
fn sky_pass<C: ColumnAccess + ?Sized>(cols: &mut C, xa: i32, xb: i32, ya: i32, yb: i32) {
    for x in xa..xb {
        for y in ya..yb {
            let Some(col) = cols.column_mut(x, y) else { continue };
            let mut sky = true;
            for b in col.blocks.iter_mut().rev() {
                if is_clear(b[3] & 0x1f) {
                    let v = if sky { 0xff } else { 0 };
                    b[1] = v;
                    b[2] = v;
                } else {
                    sky = false;
                }
            }
        }
    }
}

/// One of the sixteen sweeps of [`compute_light`] (update, then copy).
fn sweep<C: ColumnAccess + ?Sized>(cols: &mut C, scratch: &mut Vec<Option<u8>>, xa: i32, xb: i32, ya: i32, yb: i32) {
    for x in xa..xb {
        for y in ya..yb {
            let Some(col) = cols.column(x, y) else { continue };
            let height = col.height;
            scratch.clear();
            for (i, b) in col.blocks.iter().enumerate() {
                if !is_clear(b[3] & 0x1f) || b[2] == 0xff {
                    scratch.push(None);
                    continue;
                }
                let z = height + i as i32;
                // 0x0059a205..0x0059a67b: the maximum over the six neighbours; the original
                // stops early at 255, which does not change the maximum.
                let mut m = 0u32;
                for (nx, ny, nz) in [(x - 1, y, z), (x + 1, y, z), (x, y - 1, z), (x, y + 1, z), (x, y, z - 1), (x, y, z + 1)] {
                    m = m.max(sweep_light(get_block(cols, nx, ny, nz)));
                }
                // 0x0059a67e: `(max * 0x55) / 100` (signed division; max is never negative).
                scratch.push(Some((m * 0x55 / 100) as u8));
            }
            let col = cols.column_mut(x, y).expect("column seen above");
            for (b, v) in col.blocks.iter_mut().zip(scratch.iter()) {
                if let Some(v) = v {
                    b[1] = *v;
                }
            }
        }
    }
    // 0x0059a6d6..0x0059a74e: byte 2 = byte 1.
    for x in xa..xb {
        for y in ya..yb {
            let Some(col) = cols.column_mut(x, y) else { continue };
            for b in &mut col.blocks {
                if is_clear(b[3] & 0x1f) {
                    b[2] = b[1];
                }
            }
        }
    }
}

/// Step 3 of [`compute_light`].
fn publish<C: ColumnAccess + ?Sized>(cols: &mut C, x0: i32, x1: i32, y0: i32, y1: i32) {
    for x in x0..x1 {
        for y in y0..y1 {
            let Some(col) = cols.column_mut(x, y) else { continue };
            for b in &mut col.blocks {
                if is_clear(b[3] & 0x1f) {
                    b[0] = b[2];
                }
            }
        }
    }
}

/// The half-open sample range along one axis for a face normal component: the block in front
/// (`0..1` or `-1..0`) along the normal, `-3..3` across it (`Cube.exe 0x004c155d..0x004c15f4`).
#[inline]
fn sample_range(n: i32) -> (i32, i32) {
    if n > 0 {
        (0, 1)
    } else if n < 0 {
        (-1, 0)
    } else {
        (-3, 3)
    }
}

/// `if (0 > v) 0 else if (v > 1) 1 else v`: the `comiss`/`jbe` clamp of `Cube.exe 0x004c18ca`,
/// which lets NaN through.
#[inline]
#[allow(clippy::manual_clamp)] // `f32::clamp` would differ on NaN
fn clamp01(v: f32) -> f32 {
    if 0.0 > v {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    }
}

/// `computeVertexLight(out, x, y, z, color, normal, passthrough)`, `Cube.exe 0x004c1510` (a
/// `GameController` method; it reads blocks through the world at `GC+0x2e4`).
///
/// `(x, y, z)` is a face corner in world blocks, `color` the face's RGBA in 0..1, `normal` the
/// face normal. With `passthrough` the colour comes back unchanged (no caller in
/// `buildChunkMesh` sets it).
///
/// Samples the blocks `(x + dx, y + dy, z + dz)` over [`sample_range`] of each normal component
/// (6x6 blocks in the face plane, one deep in front of it), x outer, y, z inner. Each sample is
/// weighted `(3 - max(|dx + .5|, |dy + .5|, |dz + .5|)) / 3` and contributes:
///
/// | block | light | open |
/// |---|---|---|
/// | type 0xd | 255 | 1 |
/// | air, water | `max(byte0, 5)` | 1 |
/// | anything else | 5 | 0 |
///
/// Output alpha = `sum(light * w) / (sum(w) * 255)`; `open = sum(open * w) / sum(w)`. The RGB
/// is the input colour where the corner is open, and where it is enclosed, a darkened
/// desaturated copy of it: `c' = clamp01(10 * 0.1 * c - 0.9 * lum)` with
/// `lum = 0.1 * (0.59 g + 0.3 r + 0.11 b)` on the clamped input, mixed as
/// `c * open + c' * (1 - open)` with the unclamped input `c`. Every operation is single
/// precision in the original's order.
pub fn vertex_light(world: &World, x: i32, y: i32, z: i32, color: [f32; 4], normal: [i32; 3], passthrough: bool) -> [f32; 4] {
    if passthrough {
        return color;
    }
    let (xa, xb) = sample_range(normal[0]);
    let (ya, yb) = sample_range(normal[1]);
    let (za, zb) = sample_range(normal[2]);

    let mut alpha = 0.0f32; // EBP-0x3c
    let mut open = 0.0f32; // EBP-0x24
    let mut sum_w = 0.0f32; // XMM2 / EBP-0x30
    let mut sum_light = 0.0f32; // XMM4 / EBP-0x20
    let mut sum_open = 0.0f32; // XMM3 / EBP-0x38
    if xa < xb {
        for dx in xa..xb {
            for dy in ya..yb {
                for dz in za..zb {
                    // 0x004c1660..0x004c1737: getBlock inlined.
                    let b = get_block(world, x + dx, y + dy, z + dz);
                    // 0x004c1737..0x004c1780
                    let t = b[3] & 0x1f;
                    let (light, o) = if t == 0xd {
                        (0xffu8, 1.0f32)
                    } else if is_clear(t) {
                        (b[0].max(5), 1.0)
                    } else {
                        (5, 0.0)
                    };
                    // 0x004c1780..0x004c17e5: the largest |d + 0.5| (float -> double -> andpd ->
                    // float, which is the float absolute value).
                    let mut m = (dx as f32 + 0.5).abs();
                    let my = (dy as f32 + 0.5).abs();
                    if my > m {
                        m = my;
                    }
                    let mz = (dz as f32 + 0.5).abs();
                    if mz > m {
                        m = mz;
                    }
                    // 0x004c17e5..0x004c182d
                    let w = (3.0f32 - m) / 3.0f32;
                    sum_light += f32::from(light) * w; // (addition commutes exactly)
                    sum_w += w;
                    sum_open += w * o;
                }
            }
        }
        // 0x004c1885
        if sum_w > 0.0 {
            alpha = sum_light / (sum_w * 255.0);
            open = sum_open / sum_w;
        }
    }

    // 0x004c18b5..0x004c19ac: the enclosed tint.
    let r = clamp01(color[0]) * 0.1f32;
    let g = clamp01(color[1]) * 0.1f32;
    let b = clamp01(color[2]) * 0.1f32;
    let lum = (g * 0.59f32 + r * 0.3f32 + b * 0.11f32) * -9.0f32;
    let rt = clamp01(r * 10.0f32 + lum);
    let gt = clamp01(g * 10.0f32 + lum);
    let bt = clamp01(b * 10.0f32 + lum);
    // 0x004c19ac..0x004c19fe
    let inv = 1.0f32 - open;
    [color[0] * open + inv * rt, color[1] * open + inv * gt, color[2] * open + inv * bt, alpha]
}

/// The light level `buildChunkMesh` stores into a prop of the chunk (`prop+0x28`, as a float;
/// `Cube.exe 0x004a0e04..0x004a0e49`): 255 for a light-source block, `max(byte0, 5)` for air
/// and water, 0 otherwise (unlike [`vertex_light`], whose opaque samples count 5).
pub fn prop_light(b: Block) -> u8 {
    let t = b[3] & 0x1f;
    if t == 0xd {
        0xff
    } else if is_clear(t) {
        b[0].max(5)
    } else {
        0
    }
}
