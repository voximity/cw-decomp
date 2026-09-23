//! `plasma::Drawing` (Cube.exe vtable 0x0071f2c0) and the CPU half of
//! `plasma::D3D9Drawing` (vtable 0x007225f8): the triangle soup a shape hands to the GPU.
//!
//! Data flow in the original, kept visible here:
//!
//! ```text
//! SmoothMeshShape::buildDrawings 0x00648d60
//!   fills Drawing.{positions, tex_coords, colors, colors2, contours}
//!   -> Drawing::tessellate 0x00675690            (GLU, here lyon)      -> indices
//!   -> Drawing::tessellateWithFringe 0x00673090  (extrusion walls)     -> indices
//!   -> Drawing::computeOutlines 0x00674160       (boundary loops)      -> outlines
//! D3D9Drawing::update 0x0068b7f0
//!   -> Drawing::update 0x00675800                (bounds)
//!   -> Drawing::buildRenderArrays 0x006758f0     (AA fringe vertices + quads, reversed IB)
//!   -> 48-byte vertices: pos, colour, n0, n1, uv (the D3D9 VB/IB)
//! ```
//!
//! Names: the original's `tessellateWithFringe` (named before this port) actually builds the
//! *extrusion* side walls; the anti-aliasing fringe the GUI vertex shader 06 extrudes is built
//! later by 0x006758f0. This module calls them [`Drawing::tessellate_extrusion`] and
//! [`Drawing::build_buffers`].

use lyon::math::point;
use lyon::path::Path;
use lyon::tessellation::{
    FillGeometryBuilder, FillOptions, FillRule, FillTessellator, FillVertex, GeometryBuilder,
    GeometryBuilderError, VertexId, VertexSource,
};

/// A 2D vector as the original stores it (`plasma::Vector<2,float>`, 8 bytes).
pub type Vec2 = [f32; 2];
/// An RGBA colour (`plasma::Vector<4,float>`, 16 bytes), straight (not premultiplied) alpha.
pub type Vec4 = [f32; 4];
/// A row-major 4×4 matrix as `plasma::Matrix<float>` lays it out (16 floats, translation in
/// elements 12..14, so a point transforms as `x*m[0] + y*m[4] + m[12]`).
pub type Mat4 = [f32; 16];

/// The 48-byte GUI vertex of `D3D9Engine::init` 0x0068a350's vertex declaration, as written
/// by `D3D9Drawing::update` 0x0068b7f0.
///
/// | Offset | D3D usage | Source array |
/// |---|---|---|
/// | 0 | FLOAT2 POSITION0 | Drawing+0xc4 (render positions) |
/// | 8 | FLOAT4 COLOR0 | Drawing+0xdc |
/// | 24 | FLOAT2 TEXCOORD0 | Drawing+0xe8, first half: edge normal `n0` |
/// | 32 | FLOAT2 TEXCOORD1 | Drawing+0xe8, second half: edge normal `n1` |
/// | 40 | FLOAT2 TEXCOORD2 | Drawing+0xd0 (texture coordinate) |
///
/// Vertex shader 06 extrudes a vertex only when both `n0` and `n1` are non-zero: each is
/// normalised as `n/(|n|+1e-5)`, the pair's bisector is normalised the same way, and the
/// vertex moves along it by `aaOffset / max(dot(n0̂, bisector), 0.7)` (z += 0.5). Original
/// fill vertices carry zero normals and stay put; fringe vertices carry the outward edge
/// normals and have alpha 0, giving a one-`aaOffset`-wide anti-aliasing ramp.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct GuiVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    pub n0: [f32; 2],
    pub n1: [f32; 2],
    pub uv: [f32; 2],
}

const _: () = assert!(std::mem::size_of::<GuiVertex>() == 48);

/// The GPU-ready buffers of one drawing (the D3D9 VB at +0x128 / IB at +0x12c, and their
/// CPU copies at +0x130/+0x134). Indices are a triangle list (`D3DPT_TRIANGLELIST`, INDEX32).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawingBuffers {
    pub vertices: Vec<GuiVertex>,
    pub indices: Vec<u32>,
}

/// `plasma::Drawing` (ctor 0x00672a00, size 0x124 of the 0x140 `D3D9Drawing`): the
/// authoring-space triangle mesh of one fill, stroke or extrusion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    /// +0x04: vertex positions.
    pub positions: Vec<Vec2>,
    /// +0x10: texture coordinates (become TEXCOORD2).
    pub tex_coords: Vec<Vec2>,
    /// +0x1c: vertex colours.
    pub colors: Vec<Vec4>,
    /// +0x28: second colour. Only the extrusion drawing fills it (extrusion back colours);
    /// `tessellateWithFringe` gives the extruded copy of each outline vertex this colour.
    /// The GLU combine callback interpolates it only while it is non-empty.
    pub colors2: Vec<Vec4>,
    /// +0x34: triangle-list indices.
    pub indices: Vec<u32>,
    /// +0x40: input contours for [`Drawing::tessellate`] (indices into `positions`).
    pub contours: Vec<Vec<u32>>,
    /// +0x100: boundary loops of the triangle mesh ([`Drawing::compute_outlines`]); these
    /// drive the AA fringe, hit testing and the extrusion walls.
    pub outlines: Vec<Vec<u32>>,
    /// +0x110/+0x114: bounds minimum, +0x118/+0x11c: bounds maximum ([`Drawing::update`]).
    pub bounds_min: Vec2,
    pub bounds_max: Vec2,
    /// +0xbc: dirty flags. `SmoothMeshShape::buildDrawings` seeds it from the shape's
    /// +0xc0c; bit 3 asks for [`Drawing::compute_outlines`] before upload. 0xf = all dirty.
    pub flags: u32,
    /// +0x64 (byte): set on the fill drawing when the shape fills its outline curves
    /// (`SmoothMeshShape.flags` bit 0 set, bit 2 clear). Its reader was not traced.
    pub curve_fill: bool,
    /// The `D3D9Drawing` VB/IB (+0x128/+0x12c): the last [`Drawing::build_buffers`] result,
    /// reused until the drawing changes ([`Drawing::cached_buffers`]).
    pub buffers: BuffersCache,
}

/// The GPU buffers `D3D9Drawing::update` 0x0068b7f0 keeps between frames (it rebuilds them
/// only after the shape's `buildDrawings`). Keyed by a fingerprint of the drawing (array
/// lengths and the vertex positions' bits) and emptied by the mutators of [`Drawing`]
/// (`clear`, `tessellate`, `tessellate_extrusion`, `compute_outlines`) and by
/// `SmoothMeshShape::buildDrawings`. A clone starts empty; equality ignores it.
#[derive(Default)]
pub struct BuffersCache(std::sync::Mutex<Option<(u64, std::sync::Arc<DrawingBuffers>)>>);

impl BuffersCache {
    /// Forget the buffers (the drawing changed).
    pub fn invalidate(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

impl Clone for BuffersCache {
    fn clone(&self) -> Self {
        BuffersCache::default()
    }
}

impl PartialEq for BuffersCache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for BuffersCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BuffersCache")
    }
}

impl Drawing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties every per-vertex array and the index/contour lists (the `_Orphan_all` +
    /// `end = begin` sequences at the top of each `buildDrawings` branch).
    pub fn clear(&mut self) {
        self.buffers.invalidate();
        self.positions.clear();
        self.tex_coords.clear();
        self.colors.clear();
        self.colors2.clear();
        self.indices.clear();
        self.contours.clear();
    }

    /// `Drawing::update` 0x00675800 (vtable slot 1): recompute the bounds from `positions`
    /// (all zero when empty). The trailing 0x00674f80 only shrinks vector capacities.
    pub fn update(&mut self) {
        let Some(first) = self.positions.first() else {
            self.bounds_min = [0.0, 0.0];
            self.bounds_max = [0.0, 0.0];
            return;
        };
        self.bounds_min = *first;
        self.bounds_max = *first;
        for p in &self.positions[1..] {
            if p[0] < self.bounds_min[0] {
                self.bounds_min[0] = p[0];
            }
            if p[1] < self.bounds_min[1] {
                self.bounds_min[1] = p[1];
            }
            // `max <= v && v != max`, i.e. v > max without NaN surprises.
            if self.bounds_max[0] <= p[0] && p[0] != self.bounds_max[0] {
                self.bounds_max[0] = p[0];
            }
            if self.bounds_max[1] <= p[1] && p[1] != self.bounds_max[1] {
                self.bounds_max[1] = p[1];
            }
        }
    }

    /// `Drawing::tessellate` 0x00675690: triangulate `contours` with the NONZERO winding
    /// rule and replace `indices` (the original resets +0x38 = +0x34 first).
    ///
    /// The original runs GLU (`gluNewTess`, `GLU_TESS_WINDING_NONZERO`, the no-op
    /// EDGE_FLAG callback 0x00675660 forcing plain triangles, VERTEX_DATA 0x00675670
    /// pushing the vertex index, COMBINE_DATA 0x006751d0 appending a new vertex). This port
    /// uses lyon's fill tessellator:
    ///
    /// - An output vertex that is an input endpoint reuses that endpoint's index. When
    ///   lyon merges coincident endpoints the first one wins.
    /// - An intersection vertex is appended like GLU's combine callback does: position
    ///   from lyon, and uv/colour/colour2 as the mean of its source edges' linear
    ///   interpolations (GLU weights up to four vertices by distance; the difference is
    ///   sub-pixel in colour and invisible in position).
    /// - The triangulation itself (which diagonals, triangle order) differs from GLU.
    ///   Every triangle is oriented like GLU's: counter-clockwise (positive cross product)
    ///   when the input's total signed area is non-negative, clockwise otherwise. The
    ///   outline and fringe passes rely on consistent orientation.
    pub fn tessellate(&mut self) {
        self.buffers.invalidate();
        self.indices.clear();
        if self.contours.is_empty() {
            return;
        }
        let mut builder = Path::builder();
        // Endpoint id (assigned in path order by lyon) -> vertex index.
        let mut endpoint_to_index: Vec<u32> = Vec::new();
        let mut area = 0.0f64;
        for contour in &self.contours {
            if contour.is_empty() {
                continue;
            }
            for (k, &idx) in contour.iter().enumerate() {
                let p = self.positions[idx as usize];
                let pt = point(p[0], p[1]);
                let id = if k == 0 { builder.begin(pt) } else { builder.line_to(pt) };
                let id = id.0 as usize;
                if endpoint_to_index.len() <= id {
                    endpoint_to_index.resize(id + 1, u32::MAX);
                }
                endpoint_to_index[id] = idx;
                let q = self.positions[contour[(k + 1) % contour.len()] as usize];
                area += p[0] as f64 * q[1] as f64 - q[0] as f64 * p[1] as f64;
            }
            builder.end(true);
        }
        let path = builder.build();
        let mut out = TessOutput {
            drawing: self,
            endpoint_to_index: &endpoint_to_index,
            triangles: Vec::new(),
        };
        let options = FillOptions::tolerance(0.01).with_fill_rule(FillRule::NonZero);
        let mut tess = FillTessellator::new();
        // A failed tessellation leaves no triangles, like a GLU error would.
        if tess.tessellate_with_ids(path.id_iter(), &path, None, &options, &mut out).is_err() {
            out.triangles.clear();
        }
        let triangles = std::mem::take(&mut out.triangles);
        let want_ccw = area >= 0.0;
        for [a, b, c] in triangles {
            let (pa, pb, pc) = (
                self.positions[a as usize],
                self.positions[b as usize],
                self.positions[c as usize],
            );
            let cross = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pc[0] - pa[0]) * (pb[1] - pa[1]);
            if (cross >= 0.0) == want_ccw {
                self.indices.extend_from_slice(&[a, b, c]);
            } else {
                self.indices.extend_from_slice(&[a, c, b]);
            }
        }
    }

    /// 0x00674160: rebuild `outlines` as the boundary loops of the triangle mesh.
    ///
    /// Every directed triangle edge (a,b) goes into an ordered set unless its reverse (b,a)
    /// is already there, in which case the reverse is removed; what remains are the
    /// boundary edges. They are then collected into a `map<int, list<int>>` from start to
    /// end vertex (in triangle order) and chained: each loop starts at the smallest start
    /// vertex left in the map and follows first-inserted successors until it returns to
    /// its start or runs out.
    pub fn compute_outlines(&mut self) {
        self.buffers.invalidate();
        self.outlines = boundary_loops(&self.indices, None);
    }

    /// `tessellateWithFringe` 0x00673090 (misnamed; see the module docs): build the side
    /// walls of an extruded shape.
    ///
    /// `m` is the `SmoothMeshShape.extrusionMatrix` key (row-major, 2D points transform as
    /// `(x*m[0] + y*m[4] + m[12], x*m[1] + y*m[5] + m[13]) / (x*m[3] + y*m[7] + m[15])`).
    ///
    /// 1. [`Drawing::tessellate`] the contours and [`Drawing::compute_outlines`], then
    ///    **drop the fill triangles** (indices cleared): only the walls are drawn.
    /// 2. For each outline loop of three or more vertices, append two vertices per loop
    ///    point: the point itself with `colors[i]` and uv (0,0), and its image under `m`
    ///    with `colors2[i]` and uv (0,0). The shoelace sum over all loops accumulates the
    ///    signed area (not reset between loops).
    /// 3. Each loop edge (i → i+1) becomes a heap entry keyed by how far the projected
    ///    edge midpoint lies along the extrusion direction (0x00672600 / 0x006721d0 are
    ///    MSVC `push_heap`/`_Adjust_heap` with that comparator). Popping the max first
    ///    orders the walls back to front.
    /// 4. A popped edge (a,b) with a+1, b+1 its extruded copies emits the quad
    ///    `(a, a+1, b), (b, a+1, b+1)` only when `cross(p[b]-p[a], p[a+1]-p[a]) * area < 0`
    ///    (a back-face test).
    /// 5. [`Drawing::compute_outlines`] again, now around the walls.
    pub fn tessellate_extrusion(&mut self, m: &Mat4) {
        self.buffers.invalidate();
        self.tessellate();
        self.compute_outlines();
        self.indices.clear();

        let mut area = 0.0f32; // local_ac
        let mut heap: Vec<(u32, u32)> = Vec::new();
        let outlines = self.outlines.clone();
        for loop_ in &outlines {
            let n = loop_.len();
            if n <= 2 {
                continue;
            }
            let base = self.positions.len() as u32; // local_d0
            for i in 0..n {
                let idx = loop_[i] as usize;
                let nxt = loop_[(i + 1) % n] as usize;
                let p = self.positions[idx];
                let q = self.positions[nxt];
                area = (p[0] * q[1] - p[1] * q[0]) + area;
                // Inner copy.
                self.positions.push(p);
                self.colors.push(self.colors[idx]);
                self.tex_coords.push([0.0, 0.0]);
                // Extruded copy.
                let x = m[4] * p[1] + m[0] * p[0] + m[12];
                let y = m[5] * p[1] + m[1] * p[0] + m[13];
                let w = 1.0 / (m[3] * p[0] + m[7] * p[1] + m[15]);
                self.positions.push([x * w, y * w]);
                let c2 = self.colors2.get(idx).copied().unwrap_or([0.0; 4]);
                self.colors.push(c2);
                self.tex_coords.push([0.0, 0.0]);
            }
            for i in 0..n {
                let a = base + 2 * i as u32;
                let b = base + 2 * ((i + 1) % n) as u32;
                heap.push((a, b));
                let len = heap.len();
                let val = heap[len - 1];
                push_heap(&mut heap, len - 1, 0, val, &self.positions, m);
            }
        }
        // Keep colors2 parallel to the vertex arrays for later passes.
        if !self.colors2.is_empty() {
            self.colors2.resize(self.positions.len(), [0.0; 4]);
        }
        while !heap.is_empty() {
            let (a, b) = heap[0];
            let pa = self.positions[a as usize];
            let pa1 = self.positions[a as usize + 1];
            let pb = self.positions[b as usize];
            let e1 = [pa1[0] - pa[0], pa1[1] - pa[1]];
            let e2 = [pb[0] - pa[0], pb[1] - pa[1]];
            if (e2[0] * e1[1] - e2[1] * e1[0]) * area < 0.0 {
                self.indices.extend_from_slice(&[a, a + 1, b, b, a + 1, b + 1]);
            }
            let len = heap.len();
            if len > 1 {
                // _Pop_heap: move the max to the back, sift the old back element down.
                let val = heap[len - 1];
                heap[len - 1] = heap[0];
                adjust_heap(&mut heap, 0, len - 1, val, &self.positions, m);
            }
            heap.pop();
        }
        self.compute_outlines();
    }

    /// `D3D9Drawing::update` 0x0068b7f0 minus the D3D calls: [`Drawing::update`] (bounds),
    /// then 0x006758f0 (the render arrays with the anti-aliasing fringe), then the 48-byte
    /// interleave.
    ///
    /// 0x006758f0, in order:
    /// 1. Render arrays start as copies of positions/colours/uvs with zero normals and a
    ///    copy of the indices.
    /// 2. If there are outlines: `A` = Σ over triangles of `cross(p1-p0, p2-p0)`.
    /// 3. For each outline loop of three or more vertices, for each point P (index c):
    ///    - `nPrev = (-A·d.y, A·d.x)` with `d = P - P_prev` and `nNext = (A·e.y, -A·e.x)`
    ///      with `e = P - P_next` (both `A` × the left normal of the loop direction, so
    ///      `-n` points outward whatever the orientation). A normal shorter than 1e-6
    ///      walks to the next neighbour (at most 10 steps). **Original bug kept:** the
    ///      retry for `nNext` uses `-A`, flipping that normal.
    ///    - A zero normal takes the other one.
    ///    - `dot(nNext, nPrev) >= 0`: one fringe vertex at P, colour alpha × 0, uv copied,
    ///      `n0 = -nPrev`, `n1 = -nNext`. The ring gets `[c, new]`.
    ///    - Otherwise (a sharp turn) two fringe vertices, one with both normals `-nPrev`
    ///      and one with both `-nNext`; the ring gets `[c, new1, -1, -1, c, new2]`.
    /// 4. For each consecutive ring pair `(in_k, out_k)`, `(in_k+1, out_k+1)` with both
    ///    inner entries valid: triangles `(in_k, out_k, in_k+1), (in_k+1, out_k, out_k+1)`.
    ///    The `-1` pairs leave the wedge at a sharp turn open.
    /// 5. The whole index list is reversed (`std::reverse`): fringe first, and every
    ///    triangle's winding flips.
    /// [`Drawing::build_buffers`] of a copy, cached between frames (`D3D9Drawing::update`
    /// 0x0068b7f0 runs only after a rebuild; see [`BuffersCache`]).
    pub fn cached_buffers(&self) -> std::sync::Arc<DrawingBuffers> {
        let fp = self.fingerprint();
        if let Some((f, b)) = &*self.buffers.0.lock().unwrap_or_else(|e| e.into_inner()) {
            if *f == fp {
                return std::sync::Arc::clone(b);
            }
        }
        let b = std::sync::Arc::new(self.clone().build_buffers());
        *self.buffers.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((fp, std::sync::Arc::clone(&b)));
        b
    }

    /// The cache key: every array length and an FNV-1a hash of the positions' bits.
    fn fingerprint(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: u64| {
            h ^= v;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for n in [self.positions.len(), self.tex_coords.len(), self.colors.len(), self.colors2.len(), self.indices.len(), self.outlines.len()] {
            mix(n as u64);
        }
        for p in &self.positions {
            mix(u64::from(p[0].to_bits()) | (u64::from(p[1].to_bits()) << 32));
        }
        h
    }

    pub fn build_buffers(&mut self) -> DrawingBuffers {
        self.update();
        let mut pos = self.positions.clone(); // +0xc4
        let mut col = self.colors.clone(); // +0xdc
        let mut uv = self.tex_coords.clone(); // +0xd0
        uv.resize(pos.len(), [0.0, 0.0]);
        col.resize(pos.len(), [0.0; 4]);
        let mut normals: Vec<[f32; 4]> = vec![[0.0; 4]; pos.len()]; // +0xe8
        let mut idx = self.indices.clone(); // +0xf4

        if !self.outlines.is_empty() {
            // 2. Twice the signed area of the fill (local_154).
            let mut a2 = 0.0f32;
            for t in self.indices.chunks_exact(3) {
                let p0 = self.positions[t[0] as usize];
                let p1 = self.positions[t[1] as usize];
                let p2 = self.positions[t[2] as usize];
                a2 = a2 + ((p1[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (p1[1] - p0[1]));
            }
            let neg = -a2; // fVar23
            for loop_ in &self.outlines {
                let n = loop_.len();
                if n <= 2 {
                    continue;
                }
                let mut ring: Vec<i64> = Vec::new();
                for i in 0..n {
                    let c = loop_[i] as usize;
                    let p = self.positions[c];
                    let mut prev = (n + i - 1) % n;
                    let next0 = (i + 1) % n;
                    // nPrev from P - P_prev.
                    let q = self.positions[loop_[prev] as usize];
                    let (mut dx, mut dy) = (p[0] - q[0], p[1] - q[1]);
                    let mut np = [dy * neg - 0.0, 0.0 - dx * neg, dx * 0.0 - dy * 0.0];
                    let mut tries = 0;
                    while len3(np) < 1e-12 && prev != i && tries < 10 {
                        prev = (n + prev - 1) % n;
                        let q = self.positions[loop_[prev] as usize];
                        dx = p[0] - q[0];
                        dy = p[1] - q[1];
                        np = [dy * neg - 0.0, 0.0 - dx * neg, dx * 0.0 - dy * 0.0];
                        tries += 1;
                    }
                    // nNext from P - P_next, with +A.
                    let mut next = next0;
                    let q = self.positions[loop_[next] as usize];
                    let (ex, ey) = (p[0] - q[0], p[1] - q[1]);
                    let mut nn = [ey * a2 - 0.0, 0.0 - ex * a2, ex * 0.0 - ey * 0.0];
                    let mut tries = 0;
                    while len3(nn) < 1e-12 && next != i && tries < 10 {
                        next = (next + 1) % n;
                        let q = self.positions[loop_[next] as usize];
                        let (ex, ey) = (p[0] - q[0], p[1] - q[1]);
                        // Original bug: the retry uses -A (fVar23), not +A.
                        nn = [ey * neg - 0.0, 0.0 - ex * neg, ex * 0.0 - ey * 0.0];
                        tries += 1;
                    }
                    if len3(np) <= 0.0 {
                        np = nn;
                    }
                    if len3(nn) <= 0.0 {
                        nn = np;
                    }
                    let c_col = col[c];
                    let fringe_col = [c_col[0], c_col[1], c_col[2], c_col[3] * 0.0];
                    let c_uv = uv[c];
                    if 0.0 <= nn[0] * np[0] + nn[1] * np[1] + nn[2] * np[2] {
                        ring.push(c as i64);
                        ring.push(pos.len() as i64);
                        pos.push(p);
                        col.push(fringe_col);
                        uv.push(c_uv);
                        normals.push([np[0] * -1.0, np[1] * -1.0, nn[0] * -1.0, nn[1] * -1.0]);
                    } else {
                        ring.push(c as i64);
                        ring.push(pos.len() as i64);
                        pos.push(p);
                        col.push(fringe_col);
                        uv.push(c_uv);
                        normals.push([np[0] * -1.0, np[1] * -1.0, np[0] * -1.0, np[1] * -1.0]);
                        ring.push(-1);
                        ring.push(-1);
                        ring.push(c as i64);
                        ring.push(pos.len() as i64);
                        pos.push(p);
                        col.push(fringe_col);
                        uv.push(c_uv);
                        normals.push([nn[0] * -1.0, nn[1] * -1.0, nn[0] * -1.0, nn[1] * -1.0]);
                    }
                }
                let m = ring.len();
                let mut k = 0;
                while k < m {
                    if ring[k] >= 0 && ring[(k + 2) % m] >= 0 {
                        let r = |j: usize| ring[j % m] as u32;
                        idx.extend_from_slice(&[r(k), r(k + 1), r(k + 2), r(k + 2), r(k + 1), r(k + 3)]);
                    }
                    k += 2;
                }
            }
        }
        idx.reverse();

        let vertices = (0..pos.len())
            .map(|i| GuiVertex {
                pos: pos[i],
                color: col[i],
                n0: [normals[i][0], normals[i][1]],
                n1: [normals[i][2], normals[i][3]],
                uv: uv[i],
            })
            .collect();
        DrawingBuffers { vertices, indices: idx }
    }

    /// `Drawing::containsPoint` 0x006747f0: bounds check, then an even-odd crossing test
    /// against every outline loop (point in the drawing's own space). An outline index out
    /// of range answers false.
    pub fn contains_point(&self, p: Vec2) -> bool {
        let (x, y) = (p[0], p[1]);
        if !(self.bounds_min[0] <= x
            && self.bounds_min[1] <= y
            && (x < self.bounds_max[0] || x == self.bounds_max[0])
            && (y < self.bounds_max[1] || y == self.bounds_max[1]))
        {
            return false;
        }
        let nverts = self.positions.len();
        let mut inside = false;
        for loop_ in &self.outlines {
            let n = loop_.len();
            for i in 0..n {
                let a = loop_[i] as usize;
                let b = loop_[(i + 1) % n] as usize;
                if a >= nverts || b >= nverts {
                    return false;
                }
                let pa = self.positions[a];
                let pb = self.positions[b];
                if ((pa[1] < y && y <= pb[1]) || (pb[1] < y && y <= pa[1]))
                    && ((y - pa[1]) / (pb[1] - pa[1])) * (pb[0] - pa[0]) + pa[0] < x
                {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// `Drawing::expandBounds` 0x00674690: grow `(min, max)` by every position transformed
    /// through `m` (with the perspective divide). `first` starts true and is cleared by the
    /// first point, which initialises both corners.
    pub fn expand_bounds(&self, min: &mut Vec2, max: &mut Vec2, m: &Mat4, first: &mut bool) {
        for p in &self.positions {
            let (x, y) = (p[0], p[1]);
            let w = 1.0 / (x * m[3] + y * m[7] + m[15]);
            let tx = w * (m[4] * y + m[0] * x + m[12]);
            let ty = w * (m[1] * x + m[5] * y + m[13]);
            if *first {
                *min = [tx, ty];
                *max = [tx, ty];
                *first = false;
            } else {
                if !(min[0] <= tx) {
                    min[0] = tx;
                }
                if !(min[1] <= ty) {
                    min[1] = ty;
                }
                if !(tx < max[0] || tx == max[0]) {
                    max[0] = tx;
                }
                if !(ty < max[1] || ty == max[1]) {
                    max[1] = ty;
                }
            }
        }
    }

    /// `Drawing::intersectsRect` 0x00674970 (really a circle test): does the circle of
    /// `radius` around the screen point `p` touch the drawing? `to_screen` maps drawing
    /// space to screen space; `to_local` maps back and scales the radius for the bounds
    /// pre-check (by the longer of its first two basis vectors). True when any outline
    /// edge comes within `radius`, else the even-odd crossing result.
    ///
    /// The original's early exits for the bounds test and for out-of-range indices leave
    /// the return register holding the security-cookie check's value; this port answers
    /// false there.
    pub fn intersects_circle(&self, p: Vec2, radius: f32, to_screen: &Mat4, to_local: &Mat4) -> bool {
        let m5 = to_local;
        let ax = m5[4] * 0.0 + m5[0];
        let bx = m5[0] * 0.0 + m5[4];
        let ay = m5[5] * 0.0 + m5[1];
        let by = m5[1] * 0.0 + m5[5];
        let len_b = ((by * by + bx * bx) as f64).sqrt() as f32;
        let len_a = ((ay * ay + ax * ax) as f64).sqrt() as f32;
        let scale = if len_b <= len_a { len_a } else { len_b };
        let r_local = scale * radius;
        let (x, y) = (p[0], p[1]);
        let w = 1.0 / (m5[3] * x + m5[7] * y + m5[15]);
        let lx = (x * m5[0] + y * m5[4] + m5[12]) * w;
        let ly = (x * m5[1] + y * m5[5] + m5[13]) * w;
        let lx_lo = lx - r_local;
        let ly_lo = ly - r_local;
        if !(self.bounds_min[0] <= lx + r_local
            && self.bounds_min[1] <= ly + r_local
            && (lx_lo < self.bounds_max[0] || lx_lo == self.bounds_max[0])
            && (ly_lo < self.bounds_max[1] || ly_lo == self.bounds_max[1]))
        {
            return false;
        }
        let m4 = to_screen;
        let nverts = self.positions.len();
        let mut inside = false;
        for loop_ in &self.outlines {
            let n = loop_.len();
            for i in 0..n {
                let a = loop_[i] as usize;
                let b = loop_[(i + 1) % n] as usize;
                if a >= nverts || b >= nverts {
                    return false;
                }
                let pa = self.positions[a];
                let pb = self.positions[b];
                let wa = 1.0 / (m4[7] * pa[1] + pa[0] * m4[3] + m4[15]);
                let sax = (m4[0] * pa[0] + m4[4] * pa[1] + m4[12]) * wa;
                let say = (m4[1] * pa[0] + m4[5] * pa[1] + m4[13]) * wa;
                let wb = 1.0 / (pb[0] * m4[3] + pb[1] * m4[7] + m4[15]);
                let sbx = (pb[0] * m4[0] + pb[1] * m4[4] + m4[12]) * wb;
                let sby = (pb[0] * m4[1] + pb[1] * m4[5] + m4[13]) * wb;
                let (px, py) = (x - sax, y - say);
                let (ex, ey) = (sbx - sax, sby - say);
                let l2 = ey * ey + ex * ex;
                let d2 = if 1e-20 <= l2 {
                    let t = (py * ey + px * ex) / l2;
                    if 0.0 < t {
                        let (qx, qy) = if t < 1.0 { (px - ex * t, py - ey * t) } else { (x - sbx, y - sby) };
                        qy * qy + qx * qx
                    } else {
                        py * py + px * px
                    }
                } else {
                    py * py + px * px
                };
                if d2 <= radius * radius {
                    return true;
                }
                if ((say < y && y <= sby) || (sby < y && y <= say))
                    && ((y - say) / (sby - say)) * (sbx - sax) + sax < x
                {
                    inside = !inside;
                }
            }
        }
        inside
    }
}

fn len3(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// Boundary loops of a triangle list (0x00674160 and, with `prefer_start`, the face-set
/// variant inside `SmoothMeshShape::setFaces` 0x0066b200).
///
/// `prefer_start`: see [`chain_loops`].
pub(crate) fn boundary_loops(triangles: &[u32], prefer_start: Option<u32>) -> Vec<Vec<u32>> {
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for t in triangles.chunks_exact(3) {
        for k in 0..3 {
            edges.push((t[k], t[(k + 1) % 3]));
        }
    }
    boundary_loops_from_edges(&edges, prefer_start)
}

/// Shared by [`boundary_loops`] and the shape topology: cancel opposite directed edges,
/// then chain what is left.
pub(crate) fn boundary_loops_from_edges(edges: &[(u32, u32)], prefer_start: Option<u32>) -> Vec<Vec<u32>> {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    let mut set: BTreeSet<(u32, u32)> = BTreeSet::new();
    for &(a, b) in edges {
        if !set.remove(&(b, a)) {
            set.insert((a, b));
        }
    }
    // Second pass in the same edge order: map a -> successors for every edge still in the
    // set (a duplicate directed edge is pushed twice, as the original's list does).
    let mut succ: BTreeMap<u32, VecDeque<u32>> = BTreeMap::new();
    for &(a, b) in edges {
        if set.contains(&(a, b)) {
            succ.entry(a).or_default().push_back(b);
        }
    }
    chain_loops(succ, prefer_start)
}

/// Chain a successor map into loops (the tail of 0x00674160 and 0x0066b200). Each loop
/// starts at `prefer_start` while that vertex is still a key, else at the smallest key.
pub(crate) fn chain_loops(
    mut succ: std::collections::BTreeMap<u32, std::collections::VecDeque<u32>>,
    prefer_start: Option<u32>,
) -> Vec<Vec<u32>> {
    let mut loops = Vec::new();
    while let Some((&smallest, _)) = succ.iter().next() {
        let start = match prefer_start {
            Some(s) if succ.contains_key(&s) => s,
            _ => smallest,
        };
        let mut contour = Vec::new();
        let mut cur = start;
        loop {
            let Some(list) = succ.get_mut(&cur) else { break };
            let next = list.pop_front().expect("non-empty successor list");
            if list.is_empty() {
                succ.remove(&cur);
            }
            contour.push(cur);
            cur = next;
            if next == start {
                break;
            }
        }
        loops.push(contour);
    }
    loops
}

/// Heap key of an extrusion wall entry (the comparator inlined in 0x00672600 and
/// 0x006721d0): project the edge midpoint through `m`, and measure how far the projection
/// lies along its own displacement direction, relative to the projected origin.
fn wall_key(e: (u32, u32), pos: &[Vec2], m: &Mat4) -> f32 {
    let pa = pos[e.0 as usize];
    let pb = pos[e.1 as usize];
    let sx = pa[0] + pb[0];
    let sy = pa[1] + pb[1];
    let mx = sx * 0.5;
    let my = sy * 0.5;
    let px = m[4] * my + mx * m[0] + m[12];
    let py = m[1] * mx + m[5] * my + m[13];
    let w = 1.0 / (m[3] * mx + m[7] * my + m[15]);
    let (px, py) = (px * w, py * w);
    let mut dx = px - mx;
    let mut dy = py - my;
    let l2 = dy * dy + dx * dx;
    if 0.0 < l2 {
        let l = (l2 as f64).sqrt() as f32;
        dx *= 1.0 / l;
        dy *= 1.0 / l;
    }
    (py - m[13]) * dy + (px - m[12]) * dx
}

/// MSVC `_Push_heap` (0x00672600): sift `val` up from `hole` towards `top`.
fn push_heap(h: &mut [(u32, u32)], mut hole: usize, top: usize, val: (u32, u32), pos: &[Vec2], m: &Mat4) {
    let kv = wall_key(val, pos, m);
    while top < hole {
        let parent = (hole - 1) / 2;
        if kv <= wall_key(h[parent], pos, m) {
            break;
        }
        h[hole] = h[parent];
        hole = parent;
    }
    h[hole] = val;
}

/// MSVC `_Adjust_heap` (0x006721d0): move the hole at `hole` down to a leaf, taking the
/// larger child (the right one unless it is less than the left), then push `val`.
fn adjust_heap(h: &mut [(u32, u32)], mut hole: usize, bottom: usize, val: (u32, u32), pos: &[Vec2], m: &Mat4) {
    let top = hole;
    let mut child = 2 * hole + 2;
    while child < bottom {
        if wall_key(h[child], pos, m) < wall_key(h[child - 1], pos, m) {
            child -= 1;
        }
        h[hole] = h[child];
        hole = child;
        child = 2 * child + 2;
    }
    if child == bottom {
        h[hole] = h[bottom - 1];
        hole = bottom - 1;
    }
    push_heap(h, hole, top, val, pos, m);
}

/// Receives lyon's output, mapping endpoints back to the drawing's vertex indices and
/// appending intersection vertices (the GLU combine callback 0x006751d0).
struct TessOutput<'a> {
    drawing: &'a mut Drawing,
    endpoint_to_index: &'a [u32],
    triangles: Vec<[u32; 3]>,
}

impl GeometryBuilder for TessOutput<'_> {
    fn add_triangle(&mut self, a: VertexId, b: VertexId, c: VertexId) {
        self.triangles.push([a.0, b.0, c.0]);
    }
}

impl FillGeometryBuilder for TessOutput<'_> {
    fn add_fill_vertex(&mut self, vertex: FillVertex) -> Result<VertexId, GeometryBuilderError> {
        let mut edges: Vec<(u32, u32, f32)> = Vec::new();
        for src in vertex.sources() {
            match src {
                VertexSource::Endpoint { id } => {
                    if let Some(&idx) = self.endpoint_to_index.get(id.0 as usize) {
                        if idx != u32::MAX {
                            return Ok(VertexId(idx));
                        }
                    }
                }
                VertexSource::Edge { from, to, t } => {
                    let f = self.endpoint_to_index.get(from.0 as usize).copied().unwrap_or(u32::MAX);
                    let g = self.endpoint_to_index.get(to.0 as usize).copied().unwrap_or(u32::MAX);
                    if f != u32::MAX && g != u32::MAX {
                        edges.push((f, g, t));
                    }
                }
            }
        }
        // Combine: new vertex at the end of every per-vertex array.
        let d = &mut *self.drawing;
        let p = vertex.position();
        let new_index = d.positions.len() as u32;
        let k = edges.len().max(1) as f32;
        let lerp2 = |v: &[Vec2]| -> Vec2 {
            let mut acc = [0.0f32; 2];
            for &(f, g, t) in &edges {
                let a = v.get(f as usize).copied().unwrap_or_default();
                let b = v.get(g as usize).copied().unwrap_or_default();
                for c in 0..2 {
                    acc[c] += a[c] + (b[c] - a[c]) * t;
                }
            }
            [acc[0] / k, acc[1] / k]
        };
        let lerp4 = |v: &[Vec4]| -> Vec4 {
            let mut acc = [0.0f32; 4];
            for &(f, g, t) in &edges {
                let a = v.get(f as usize).copied().unwrap_or_default();
                let b = v.get(g as usize).copied().unwrap_or_default();
                for c in 0..4 {
                    acc[c] += a[c] + (b[c] - a[c]) * t;
                }
            }
            [acc[0] / k, acc[1] / k, acc[2] / k, acc[3] / k]
        };
        let uv = lerp2(&d.tex_coords);
        let col = lerp4(&d.colors);
        let has_c2 = !d.colors2.is_empty();
        let col2 = if has_c2 { Some(lerp4(&d.colors2)) } else { None };
        d.positions.push([p.x, p.y]);
        d.tex_coords.push(uv);
        d.colors.push(col);
        if let Some(c2) = col2 {
            d.colors2.push(c2);
        }
        Ok(VertexId(new_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(d: &mut Drawing, x0: f32, y0: f32, s: f32, ccw: bool) -> Vec<u32> {
        let base = d.positions.len() as u32;
        let pts = [[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]];
        for p in pts {
            d.positions.push(p);
            d.colors.push([1.0, 0.5, 0.25, 1.0]);
            d.tex_coords.push([p[0], p[1]]);
        }
        let mut c = vec![base, base + 1, base + 2, base + 3];
        if !ccw {
            c.reverse();
        }
        c
    }

    fn signed_area2(d: &Drawing, t: &[u32]) -> f32 {
        let (a, b, c) = (d.positions[t[0] as usize], d.positions[t[1] as usize], d.positions[t[2] as usize]);
        (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])
    }

    #[test]
    fn gui_vertex_layout() {
        assert_eq!(std::mem::size_of::<GuiVertex>(), 48);
        assert_eq!(std::mem::offset_of!(GuiVertex, color), 8);
        assert_eq!(std::mem::offset_of!(GuiVertex, n0), 24);
        assert_eq!(std::mem::offset_of!(GuiVertex, n1), 32);
        assert_eq!(std::mem::offset_of!(GuiVertex, uv), 40);
    }

    #[test]
    fn tessellate_square() {
        let mut d = Drawing::new();
        let c = square(&mut d, 0.0, 0.0, 10.0, true);
        d.contours.push(c);
        d.tessellate();
        assert_eq!(d.positions.len(), 4, "no new vertices for a simple square");
        assert_eq!(d.indices.len(), 6, "two triangles");
        let mut total = 0.0;
        for t in d.indices.chunks_exact(3) {
            let a = signed_area2(&d, t);
            assert!(a > 0.0, "CCW input gives CCW triangles");
            total += a;
        }
        assert!((total - 200.0).abs() < 1e-3);
    }

    #[test]
    fn tessellate_clockwise_square_keeps_orientation() {
        let mut d = Drawing::new();
        let c = square(&mut d, 0.0, 0.0, 10.0, false);
        d.contours.push(c);
        d.tessellate();
        assert_eq!(d.indices.len(), 6);
        for t in d.indices.chunks_exact(3) {
            assert!(signed_area2(&d, t) < 0.0);
        }
    }

    #[test]
    fn tessellate_square_with_hole_nonzero() {
        let mut d = Drawing::new();
        let outer = square(&mut d, 0.0, 0.0, 10.0, true);
        let hole = square(&mut d, 3.0, 3.0, 4.0, false);
        d.contours.push(outer);
        d.contours.push(hole);
        d.tessellate();
        assert_eq!(d.positions.len(), 8);
        // A square annulus: 8 triangles, area 100 - 16.
        assert_eq!(d.indices.len(), 24);
        let mut total = 0.0;
        for t in d.indices.chunks_exact(3) {
            let a = signed_area2(&d, t);
            assert!(a > 0.0);
            total += a;
        }
        assert!((total / 2.0 - 84.0).abs() < 1e-3);
        d.compute_outlines();
        assert_eq!(d.outlines.len(), 2);
        assert!(d.outlines.iter().all(|l| l.len() == 4));
    }

    #[test]
    fn tessellate_same_winding_hole_is_filled_under_nonzero() {
        let mut d = Drawing::new();
        let outer = square(&mut d, 0.0, 0.0, 10.0, true);
        let inner = square(&mut d, 3.0, 3.0, 4.0, true);
        d.contours.push(outer);
        d.contours.push(inner);
        d.tessellate();
        let total: f32 = d.indices.chunks_exact(3).map(|t| signed_area2(&d, t)).sum();
        // NONZERO: winding 2 inside the inner square is still filled.
        assert!((total / 2.0 - 100.0).abs() < 1e-3);
    }

    #[test]
    fn tessellate_self_intersection_adds_combined_vertex() {
        // A bow tie: the crossing becomes a combined vertex with interpolated attributes.
        let mut d = Drawing::new();
        for p in [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]] {
            d.positions.push(p);
            d.colors.push([p[0] / 10.0, 0.0, 0.0, 1.0]);
            d.tex_coords.push(p);
        }
        d.contours.push(vec![0, 1, 2, 3]);
        d.tessellate();
        assert_eq!(d.positions.len(), 5);
        let c = d.positions[4];
        assert!((c[0] - 5.0).abs() < 1e-4 && (c[1] - 5.0).abs() < 1e-4);
        assert!((d.colors[4][0] - 0.5).abs() < 1e-4);
        assert_eq!(d.indices.len(), 6);
    }

    #[test]
    fn fringe_on_square() {
        let mut d = Drawing::new();
        let c = square(&mut d, 0.0, 0.0, 10.0, true);
        d.contours.push(c);
        d.tessellate();
        d.compute_outlines();
        assert_eq!(d.outlines, vec![vec![0, 1, 2, 3]]);
        let b = d.build_buffers();
        // 4 fill vertices + one fringe vertex per convex corner.
        assert_eq!(b.vertices.len(), 8);
        // 2 fill triangles + 2 per outline edge.
        assert_eq!(b.indices.len(), 6 + 4 * 6);
        for v in &b.vertices[..4] {
            assert_eq!((v.n0, v.n1), ([0.0; 2], [0.0; 2]));
            assert_eq!(v.color[3], 1.0);
        }
        // Corner (0,0): outward normals point to -y (edge from (0,10)) and -x... check
        // they point away from the square's centre and are non-zero.
        for v in &b.vertices[4..] {
            assert_eq!(v.color[3], 0.0);
            assert_eq!(&v.color[..3], &[1.0, 0.5, 0.25]);
            let centre = [5.0, 5.0];
            let out = [v.pos[0] - centre[0], v.pos[1] - centre[1]];
            assert!(v.n0 != [0.0; 2] && v.n1 != [0.0; 2]);
            assert!(v.n0[0] * out[0] + v.n0[1] * out[1] > 0.0);
            assert!(v.n1[0] * out[0] + v.n1[1] * out[1] > 0.0);
        }
        // First fringe vertex (corner 0 = (0,0)): prev edge (0,10)->(0,0) outward normal
        // is -x, next edge (0,0)->(10,0) outward normal is -y; scaled by the area 200.
        let v = b.vertices[4];
        assert_eq!(v.pos, [0.0, 0.0]);
        assert_eq!(v.n0, [-2000.0, 0.0]);
        assert_eq!(v.n1, [0.0, -2000.0]);
        // Reversed index buffer: the fringe comes first.
        assert!(b.indices[..24].iter().any(|&i| i >= 4));
    }

    #[test]
    fn fringe_triangle_count_for_ngon() {
        // Regular n-gons with n >= 5 turn by less than 90 degrees: one fringe vertex per corner
        // (a triangle turns by 120 and splits every corner, see fringe_splits_sharp_turn).
        for n in [5usize, 6, 8, 16] {
            let mut d = Drawing::new();
            for k in 0..n {
                let a = k as f32 / n as f32 * std::f32::consts::TAU;
                d.positions.push([a.cos() * 10.0, a.sin() * 10.0]);
                d.colors.push([1.0; 4]);
                d.tex_coords.push([0.0; 2]);
            }
            d.contours.push((0..n as u32).collect());
            d.tessellate();
            d.compute_outlines();
            let b = d.build_buffers();
            assert_eq!(b.vertices.len(), 2 * n, "n={n}");
            assert_eq!(b.indices.len(), 3 * (n - 2) + 6 * n, "n={n}");
        }
    }

    #[test]
    fn fringe_splits_sharp_turn() {
        // A thin spike: the tip turns by more than 90 degrees -> two fringe vertices there
        // and no quad across the gap.
        let mut d = Drawing::new();
        for p in [[0.0, 0.0], [10.0, 0.0], [0.0, 1.0]] {
            d.positions.push(p);
            d.colors.push([1.0; 4]);
            d.tex_coords.push([0.0; 2]);
        }
        d.contours.push(vec![0, 1, 2]);
        d.tessellate();
        d.compute_outlines();
        let b = d.build_buffers();
        // Corner (0,0) is 90 degrees (dot == 0 -> not split); the two acute corners split.
        assert_eq!(b.vertices.len(), 3 + 1 + 2 + 2);
        let split: Vec<_> = b.vertices[3..].iter().filter(|v| v.n0 == v.n1).collect();
        assert_eq!(split.len(), 4);
        // Ring pairs: 3 corners -> 1 + 2 + 2 valid pairs plus 2 gaps; one quad per edge.
        assert_eq!(b.indices.len(), 3 + 3 * 6);
    }

    #[test]
    fn extrusion_walls() {
        let mut d = Drawing::new();
        let c = square(&mut d, 0.0, 0.0, 10.0, true);
        d.colors2 = vec![[0.0, 0.0, 0.0, 1.0]; 4];
        d.contours.push(c);
        // Translate by (3, 4).
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        m[12] = 3.0;
        m[13] = 4.0;
        d.tessellate_extrusion(&m);
        assert_eq!(d.positions.len(), 4 + 8);
        assert_eq!(d.positions[5], [3.0, 4.0]);
        assert_eq!(d.colors[5], [0.0, 0.0, 0.0, 1.0]);
        // Only the walls facing away from the extrusion direction survive: two of four.
        assert_eq!(d.indices.len(), 2 * 6);
        assert!(!d.outlines.is_empty());
    }

    #[test]
    fn contains_point_even_odd() {
        let mut d = Drawing::new();
        let outer = square(&mut d, 0.0, 0.0, 10.0, true);
        let hole = square(&mut d, 3.0, 3.0, 4.0, false);
        d.contours.push(outer);
        d.contours.push(hole);
        d.tessellate();
        d.compute_outlines();
        d.update();
        assert!(d.contains_point([1.0, 1.0]));
        assert!(!d.contains_point([5.0, 5.0]));
        assert!(!d.contains_point([11.0, 5.0]));
    }
}
