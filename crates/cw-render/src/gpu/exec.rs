//! The executor: turns a [`FrameCommands`] (the data form of `GameController::render`
//! `Cube.exe 0x004ac260`, built by [`crate::passes::build_frame`]) into wgpu work.
//!
//! Tier C. The original drew immediately on one IDirect3DDevice9; here a frame is walked
//! twice:
//!
//! 1. **prepare**: every draw's constant registers are written into one CPU-side uniform
//!    ring (one block per draw at the device's `min_uniform_buffer_offset_alignment`, bound
//!    with dynamic offsets as `pipelines.rs` set the layouts up), immediate geometry
//!    (`DrawPrimitiveUP`: the sky fan, stars, sun, decals, ribbons, the render-surface
//!    quads) is appended to per-frame vertex streams (fans and strips expanded to lists),
//!    the render pipelines the draws' [`PipelineState`]s need are created and cached, and
//!    the pass/target sequence is flattened into a list of operations;
//! 2. **upload** the ring and the streams with one `write_buffer` each (plus the GUI stream
//!    of [`Resources::set_gui_stream`]);
//! 3. **encode** the operations: a wgpu render pass per target change or clear (frame passes
//!    on the same target without a clear share one render pass), pipelines and bind groups
//!    per draw.
//!
//! State mapping (D3D9 → wgpu), per draw from its [`PipelineState`]:
//!
//! - `ZENABLE = 0` disables the depth test *and* the depth write (D3D9 semantics), so the
//!   depth state is `(test ? func : Always, test && write)`;
//! - `CULLMODE` CW/CCW: front faces are clockwise (`FrontFace::Cw`) and `D3DCULL_CW` culls
//!   them (`Face::Front`), `D3DCULL_CCW` culls the back (`pipelines.rs`, "Culling");
//! - blending: the colour factors, the separate alpha factors and the operation as given;
//! - `COLORWRITEENABLE` bits are wgpu's `ColorWrites` bits (R 1, G 2, B 4, A 8);
//! - the GUI and render-surface draws use the fixed pipelines of [`Pipelines`] (their state
//!   never varies in the original: `beginFrame 0x00688b60`, `0x006897c0`, `0x0068cad0`).
//!
//! The fixed-function draws (FVF 0x42/0x142) get a small dedicated pipeline
//! (`shaders/fixed.wgsl`): the D3D9 stage-0 defaults (colour MODULATE, alpha SELECTARG1 of
//! the texture; diffuse alone without a texture), stage-0 sampler from
//! [`PipelineState::sampler0`] (`BORDER` → `ClampToBorder` with border colour 0 when the
//! adapter has `ADDRESS_MODE_CLAMP_TO_BORDER`, else clamp to edge).
//!
//! Per-controller API: [`Resources`] owns what outlives a frame (chunk and model meshes,
//! textures, render surfaces, the GUI stream); [`Executor::render`] draws one frame into the
//! window, [`Executor::encode`] into any colour/depth pair (tests, screenshots).

use std::collections::HashMap;
use std::mem::size_of;
use std::num::NonZeroU64;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::device::GpuContext;
use super::pipelines::{
    BlurPsConstants, DEPTH_FORMAT, DownsamplePsConstants, GuiPsConstants, GuiVertex, GuiVsConstants, PipelineKind,
    Pipelines, ScreenVsConstants, WORLD_VERTEX_LAYOUT, WorldPsConstants, WorldVsConstants, create_shader_module,
    gui_projection,
};
use super::targets::{RenderSurface, quad_vertices};
use super::textures::{GpuTexture, Samplers, TextureSampling, create_sampler};
use super::wgsl;
use crate::frame::*;
use crate::mesh::{ChunkBuild, ModelMesh, build_cub_mesh};
use crate::passes::ChunkBufferInput;

/// The fixed-function replacement program (not one of the fifteen original shaders).
pub const FIXED_WGSL: &str = include_str!("shaders/fixed.wgsl");

// ---------------------------------------------------------------------------------------
// GPU-side formats of the fixed-function draws
// ---------------------------------------------------------------------------------------

/// `D3DFVF_XYZ|DIFFUSE|TEX1` (stride 0x18); FVF 0x42 draws use the same layout with the uv
/// ignored.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FixedGpuVertex {
    /// `D3DFVF_XYZ`.
    pub position: [f32; 3],
    /// `D3DFVF_DIFFUSE`, the D3DCOLOR's bytes in memory order (B, G, R, A).
    pub color: [u8; 4],
    /// `D3DFVF_TEX1`.
    pub uv: [f32; 2],
}

impl From<&FixedVertex> for FixedGpuVertex {
    fn from(v: &FixedVertex) -> Self {
        FixedGpuVertex { position: v.position, color: v.color_argb.to_le_bytes(), uv: v.uv }
    }
}

const FIXED_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 12, shader_location: 1 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
];

/// Layout of [`FixedGpuVertex`].
pub const FIXED_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 24,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &FIXED_ATTRIBUTES,
};

/// Constants of `fixed.wgsl`: `WORLD·VIEW·PROJECTION` as four registers (columns) and the
/// "stage 0 has a texture" flag.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FixedConstants {
    /// `wvp` registers.
    pub wvp: [[f32; 4]; 4],
    /// x: textured.
    pub flags: [f32; 4],
}

/// The registers of a D3D matrix as uploaded (register i = column i, `v * M` in WGSL).
fn registers(m: &D3dMatrix) -> [[f32; 4]; 4] {
    let c = |j: usize| [m[0][j], m[1][j], m[2][j], m[3][j]];
    [c(0), c(1), c(2), c(3)]
}

/// Expand a primitive to a triangle list (wgpu has no fans; strips are expanded too so every
/// immediate draw shares one pipeline topology). Strips alternate the winding back.
pub fn triangle_list_indices(topology: Topology, n: usize) -> Vec<usize> {
    let mut out = Vec::new();
    if n < 3 {
        return out;
    }
    match topology {
        Topology::TriangleList => out.extend(0..n - n % 3),
        Topology::TriangleFan => {
            for i in 1..n - 1 {
                out.extend([0, i, i + 1]);
            }
        }
        Topology::TriangleStrip => {
            for i in 0..n - 2 {
                if i % 2 == 0 {
                    out.extend([i, i + 1, i + 2]);
                } else {
                    out.extend([i + 1, i, i + 2]);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------------------

/// A mesh on the GPU: one vertex buffer of 8-byte world vertices and one index buffer.
#[derive(Debug)]
pub struct GpuMesh {
    /// Vertex buffer.
    pub vertices: wgpu::Buffer,
    /// Index buffer.
    pub indices: wgpu::Buffer,
    /// Indices in the buffer.
    pub index_count: u32,
    /// `Uint16` for models (`cube::Sprite` INDEX16), `Uint32` for chunks (INDEX32).
    pub index_format: wgpu::IndexFormat,
}

fn upload_mesh<I: Pod>(device: &wgpu::Device, label: &str, vertices: &[crate::mesh::WorldVertex], indices: &[I], format: wgpu::IndexFormat) -> Option<GpuMesh> {
    if vertices.is_empty() || indices.is_empty() {
        return None;
    }
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    // create_buffer_init pads an odd number of 16-bit indices to the copy alignment.
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    Some(GpuMesh { vertices: vb, indices: ib, index_count: indices.len() as u32, index_format: format })
}

/// The model meshes by [`ModelRef`] (`cube::Sprite` VB `+0x34`, IB `+0x38`, built by
/// `cube::Sprite::buildMesh 0x004e7870` = [`crate::mesh::build_model_mesh`]). A model whose
/// mesh is empty is remembered as present with nothing to draw.
#[derive(Debug, Default)]
pub struct ModelCache {
    meshes: HashMap<ModelRef, Option<GpuMesh>>,
}

impl ModelCache {
    /// An empty cache.
    pub fn new() -> ModelCache {
        ModelCache::default()
    }

    /// Whether `id` has been uploaded (possibly as an empty mesh).
    pub fn contains(&self, id: ModelRef) -> bool {
        self.meshes.contains_key(&id)
    }

    /// Number of models held.
    pub fn len(&self) -> usize {
        self.meshes.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty()
    }

    /// The GPU mesh of `id` (`None` when absent or empty).
    pub fn get(&self, id: ModelRef) -> Option<&GpuMesh> {
        self.meshes.get(&id).and_then(|m| m.as_ref())
    }

    /// Upload (or replace) a built mesh: map tiles ([`crate::map::MapTiles::mesh`]), world
    /// models ([`crate::mesh::build_world_model_mesh`]), anything already meshed.
    pub fn insert_mesh(&mut self, device: &wgpu::Device, id: ModelRef, mesh: &ModelMesh) {
        let gpu = upload_mesh(device, "model", &mesh.vertices, &mesh.indices, wgpu::IndexFormat::Uint16);
        self.meshes.insert(id, gpu);
    }

    /// Mesh a parsed `.cub` model (tint 0 as `cube::Sprite`'s constructor leaves it) and
    /// upload it.
    pub fn insert_cub(&mut self, device: &wgpu::Device, id: ModelRef, model: &cw_formats::CubModel, raw: bool) {
        self.insert_mesh(device, id, &build_cub_mesh(model, [0, 0, 0], raw));
    }

    /// Parse a `.cub` blob (as stored in `data1.db`) and upload it.
    pub fn insert_cub_blob(&mut self, device: &wgpu::Device, id: ModelRef, bytes: &[u8], raw: bool) -> cw_formats::Result<()> {
        let model = cw_formats::CubModel::parse(bytes)?;
        self.insert_cub(device, id, &model, raw);
        Ok(())
    }

    /// Build and upload `id` unless it is cached. `build` returning `None` leaves it absent
    /// (tried again next time). Returns whether the model is now cached.
    pub fn ensure(&mut self, device: &wgpu::Device, id: ModelRef, build: impl FnOnce() -> Option<ModelMesh>) -> bool {
        if self.contains(id) {
            return true;
        }
        match build() {
            Some(mesh) => {
                self.insert_mesh(device, id, &mesh);
                true
            }
            None => false,
        }
    }

    /// Release a model.
    pub fn remove(&mut self, id: ModelRef) {
        self.meshes.remove(&id);
    }
}

/// One `cube::ChunkBuffer` on the GPU: the slab's vertex buffer and its two index buffers
/// (`+8` opaque, `+0xc` water).
#[derive(Debug)]
pub struct GpuChunkBuffer {
    /// Vertex buffer (`n * 8` bytes).
    pub vertices: Option<wgpu::Buffer>,
    /// `+8`: opaque faces.
    pub opaque: Option<wgpu::Buffer>,
    /// `+0xc`: water faces.
    pub water: Option<wgpu::Buffer>,
    /// Index counts of the two buffers.
    pub opaque_indices: u32,
    /// See `opaque_indices`.
    pub water_indices: u32,
}

/// The inputs `build_frame` needs of a chunk's buffers ([`ChunkBufferInput`], `+0x10..+0x1c`),
/// in the same order as [`Resources::upload_chunk`] stores them.
pub fn chunk_buffer_inputs(build: &ChunkBuild) -> Vec<ChunkBufferInput> {
    build
        .buffers
        .iter()
        .map(|b| ChunkBufferInput {
            z_base: b.base_z,
            vertex_count: b.vertices.len() as u32,
            opaque_primitives: (b.indices.len() / 3) as u32,
            water_primitives: (b.indices2.len() / 3) as u32,
            has_opaque: !b.indices.is_empty(),
            has_water: !b.indices2.is_empty(),
        })
        .collect()
}

/// A texture with the sampler state `D3D9Texture::bind 0x0068bc90` gives it.
#[derive(Debug)]
pub struct TextureEntry {
    /// The texture.
    pub texture: GpuTexture,
    /// Its sampler state (GUI slots).
    pub sampling: TextureSampling,
    sampler: wgpu::Sampler,
}

/// A growable GPU buffer rewritten every frame.
#[derive(Debug)]
struct Stream {
    buffer: wgpu::Buffer,
    capacity: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl Stream {
    fn new(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages) -> Stream {
        let capacity = 4096;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: capacity,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Stream { buffer, capacity, usage, label }
    }

    /// Upload `data`; returns whether the buffer was recreated (bind groups must follow).
    ///
    /// Performance note for a future optimiser: the whole stream is copied every frame (the
    /// original rewrote its dynamic buffers per draw); a persistently mapped ring or
    /// change tracking would avoid it.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> bool {
        let mut grown = false;
        let need = (data.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if need > self.capacity {
            self.capacity = need.next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            grown = true;
        }
        if !data.is_empty() {
            if data.len() as u64 == need {
                queue.write_buffer(&self.buffer, 0, data);
            } else {
                let mut padded = data.to_vec();
                padded.resize(need as usize, 0);
                queue.write_buffer(&self.buffer, 0, &padded);
            }
        }
        grown
    }
}

/// Everything the frames draw that outlives one frame: chunk and model meshes, textures,
/// render surfaces and the GUI stream (the original's `ChunkBuffer`s, `cube::Sprite` buffers,
/// `plasma::D3D9Texture`s, `plasma::D3D9RenderSurface`s and the `D3D9Engine` dynamic VB).
#[derive(Debug)]
pub struct Resources {
    /// Chunk buffers by ring index ([`crate::passes::RenderInputs::chunks`] index, the
    /// `chunk` of [`Geometry::Chunk`]).
    chunks: HashMap<usize, Vec<GpuChunkBuffer>>,
    /// Model meshes.
    pub models: ModelCache,
    textures: HashMap<TextureRef, TextureEntry>,
    surfaces: HashMap<u32, RenderSurface>,
    gui_vertices: Vec<GuiVertex>,
    gui_indices: Vec<u32>,
    gui_vb: Stream,
    gui_ib: Stream,
    color_format: wgpu::TextureFormat,
    /// The GUI's viewport (`Engine+0x10c` / `+0x110`) when it differs from the back buffer:
    /// the render-surface quads drawn into the back buffer (drawQuad 0x00689f30) are then
    /// projected over this size, like the GUI draws ([`Resources::set_gui_viewport`]).
    gui_viewport: Option<(u32, u32)>,
}

impl Resources {
    /// Empty resources whose render surfaces use `color_format` (the executor's format).
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Resources {
        Resources {
            chunks: HashMap::new(),
            models: ModelCache::new(),
            textures: HashMap::new(),
            surfaces: HashMap::new(),
            gui_vertices: Vec::new(),
            gui_indices: Vec::new(),
            gui_vb: Stream::new(device, "gui vertices", wgpu::BufferUsages::VERTEX),
            gui_ib: Stream::new(device, "gui indices", wgpu::BufferUsages::INDEX),
            color_format,
            gui_viewport: None,
        }
    }

    /// Upload the slabs of a meshed chunk (`buildChunkMesh 0x0049d910`, the VB and the two
    /// INDEX32 IBs of `0x004a08e6..0x004a0cbc`) at ring index `ring_index`, replacing what
    /// was there. Pair it with [`chunk_buffer_inputs`] for the frame's `ChunkInput::buffers`.
    pub fn upload_chunk(&mut self, device: &wgpu::Device, ring_index: usize, build: &ChunkBuild) {
        let buffers = build
            .buffers
            .iter()
            .map(|b| {
                let init = |label: &str, contents: &[u8], usage| {
                    (!contents.is_empty()).then(|| {
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
                    })
                };
                GpuChunkBuffer {
                    vertices: init("chunk vertices", bytemuck::cast_slice(&b.vertices), wgpu::BufferUsages::VERTEX),
                    opaque: init("chunk opaque", bytemuck::cast_slice(&b.indices), wgpu::BufferUsages::INDEX),
                    water: init("chunk water", bytemuck::cast_slice(&b.indices2), wgpu::BufferUsages::INDEX),
                    opaque_indices: b.indices.len() as u32,
                    water_indices: b.indices2.len() as u32,
                }
            })
            .collect();
        self.chunks.insert(ring_index, buffers);
    }

    /// Release the buffers at a ring index.
    pub fn remove_chunk(&mut self, ring_index: usize) {
        self.chunks.remove(&ring_index);
    }

    /// Whether a ring index has buffers.
    pub fn has_chunk(&self, ring_index: usize) -> bool {
        self.chunks.contains_key(&ring_index)
    }

    /// Register a texture (sun `GC+0x8006e0`, shadow `GC+0x8006dc`, GUI textures) with its
    /// sampler state.
    pub fn add_texture(&mut self, device: &wgpu::Device, id: TextureRef, texture: GpuTexture, sampling: TextureSampling) {
        let sampler = create_sampler(device, sampling);
        self.textures.insert(id, TextureEntry { texture, sampling, sampler });
    }

    /// Release a texture.
    pub fn remove_texture(&mut self, id: TextureRef) {
        self.textures.remove(&id);
    }

    /// Create or resize render surface `id` (`D3D9RenderSurface::resize 0x0068d350`: only
    /// on a size change). Returns whether its textures were (re)created.
    pub fn ensure_surface(&mut self, device: &wgpu::Device, id: u32, width: u32, height: u32) -> bool {
        match self.surfaces.get_mut(&id) {
            Some(s) => s.resize(device, width, height),
            None => {
                self.surfaces.insert(id, RenderSurface::new(device, self.color_format, width, height));
                true
            }
        }
    }

    /// A render surface.
    pub fn surface(&self, id: u32) -> Option<&RenderSurface> {
        self.surfaces.get(&id)
    }

    /// Release a render surface.
    pub fn remove_surface(&mut self, id: u32) {
        self.surfaces.remove(&id);
    }

    /// This frame's GUI stream (the `cw-ui` tessellation between `beginFrame 0x00688b60` and
    /// `endFrame 0x0068a250`), which [`GuiDraw::vertices`]/[`GuiDraw::indices`] index.
    /// Uploaded by the next [`Executor::encode`].
    pub fn set_gui_stream(&mut self, vertices: Vec<GuiVertex>, indices: Vec<u32>) {
        self.gui_vertices = vertices;
        self.gui_indices = indices;
    }

    /// The size the GUI stream was laid out for (its `GuiDraw::proj` and the render surfaces'
    /// size). When it differs from the back buffer the whole GUI is stretched over the back
    /// buffer, the quads of the surfaces included (text then looks blurred: cw-client draws
    /// the stream at the back buffer's size, `cw_ui::render::GuiView::scale`). `None` (the
    /// default) is the back buffer's own size.
    pub fn set_gui_viewport(&mut self, size: Option<(u32, u32)>) {
        self.gui_viewport = size;
    }
}

// ---------------------------------------------------------------------------------------
// The executor
// ---------------------------------------------------------------------------------------

/// Where a frame is drawn: the back buffer (colour + its D24S8 depth) or any other pair.
#[derive(Clone, Copy, Debug)]
pub struct FrameTarget<'a> {
    /// Colour view (format = [`Executor::color_format`]).
    pub color: &'a wgpu::TextureView,
    /// Depth-stencil view ([`DEPTH_FORMAT`]).
    pub depth: &'a wgpu::TextureView,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Samples per pixel of `color`/`depth` (1 without anti-aliasing).
    pub samples: u32,
    /// Where a multisampled `color` resolves to (the surface texture), `None` when `samples`
    /// is 1.
    pub resolve: Option<&'a wgpu::TextureView>,
}

impl<'a> FrameTarget<'a> {
    /// A single-sampled colour/depth pair.
    pub fn new(color: &'a wgpu::TextureView, depth: &'a wgpu::TextureView, width: u32, height: u32) -> Self {
        FrameTarget { color, depth, width, height, samples: 1, resolve: None }
    }
}

/// Wall-clock times of the last [`Executor::render`] (the port's diagnostics).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTimings {
    /// `get_current_texture` (blocks while the swap chain has no free image).
    pub acquire: std::time::Duration,
    /// Prepare, stream uploads, encode, `finish`.
    pub encode: std::time::Duration,
    /// `queue.submit` (the backend's command submission and staging copies).
    pub submit: std::time::Duration,
    /// Capture (when requested) and `present`.
    pub present: std::time::Duration,
}

/// Counters of one encoded frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// wgpu render passes begun.
    pub render_passes: u32,
    /// Draw calls issued.
    pub draws: u32,
    /// Draws skipped because their chunk, model, texture or surface is not in the
    /// [`Resources`] (or a map placeholder, or a surface read while it is the target).
    pub skipped: u32,
    /// Render pipelines in the cache after the frame.
    pub pipelines: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Back,
    Surface(u32),
}

#[derive(Clone, Copy, Debug)]
enum WorldGeom {
    Chunk { chunk: usize, buffer: usize, water: bool, index_count: u32 },
    Model(ModelRef),
    Immediate { first: u32, count: u32 },
}

#[derive(Clone, Debug)]
enum Op {
    Begin { target: Target, color: Option<wgpu::Color>, depth: Option<f32>, stencil: Option<u32> },
    World { key: PipelineState, samples: u32, vs: u32, ps: u32, geom: WorldGeom },
    Fixed { key: PipelineState, samples: u32, sampler: SamplerState, off: u32, first: u32, count: u32, texture: Option<TextureRef> },
    Gui { subtract: bool, vs: u32, ps: u32, base_vertex: i32, indices: Range<u32>, mask: Option<TextureRef>, mask_surface: Option<u32>, texture: Option<TextureRef> },
    Screen { kind: PipelineKind, point: bool, source: u32, vs: u32, ps: u32, first: u32 },
}

/// The CPU side of the uniform ring: blocks at the dynamic-offset alignment.
struct Ring {
    data: Vec<u8>,
    align: usize,
}

impl Ring {
    fn push<T: Pod>(&mut self, v: &T) -> u32 {
        let off = self.data.len();
        self.data.extend_from_slice(bytemuck::bytes_of(v));
        let end = self.data.len().next_multiple_of(self.align);
        self.data.resize(end, 0);
        off as u32
    }
}

fn argb_color(argb: u32) -> wgpu::Color {
    let c = |s: u32| f64::from((argb >> s) & 0xff) / 255.0;
    wgpu::Color { r: c(16), g: c(8), b: c(0), a: c(24) }
}

fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    match f {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::InvSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
    }
}

fn compare(f: CompareFunc) -> wgpu::CompareFunction {
    match f {
        CompareFunc::Less => wgpu::CompareFunction::Less,
        CompareFunc::LessEqual => wgpu::CompareFunction::LessEqual,
        CompareFunc::Greater => wgpu::CompareFunction::Greater,
        CompareFunc::Always => wgpu::CompareFunction::Always,
    }
}

/// The key a pipeline is cached by: the state minus what is not baked into a pipeline.
fn pipeline_key(s: &PipelineState) -> PipelineState {
    PipelineState { sampler0: SamplerState::DEFAULT, ..*s }
}

/// The frame's GPU work: pipelines, the uniform ring, the per-frame streams.
#[derive(Debug)]
pub struct Executor {
    color_format: wgpu::TextureFormat,
    /// GUI and render-surface pipelines (their state never varies) and the bind group
    /// layouts.
    pub pipelines: Pipelines,
    samplers: Samplers,
    world_modules: HashMap<u8, wgpu::ShaderModule>,
    fixed_module: wgpu::ShaderModule,
    world_layout: wgpu::PipelineLayout,
    fixed_uniform_layout: wgpu::BindGroupLayout,
    fixed_texture_layout: wgpu::BindGroupLayout,
    fixed_layout: wgpu::PipelineLayout,
    cache: HashMap<(PipelineState, u32), wgpu::RenderPipeline>,
    /// The GUI/render-surface pipelines for a multisampled back buffer, by sample count
    /// ([`Executor::set_sample_count`]).
    ms_pipelines: HashMap<u32, Pipelines>,
    stage0_samplers: HashMap<SamplerState, wgpu::Sampler>,
    white: GpuTexture,
    align: u32,
    ring: Stream,
    ring_groups: Option<RingGroups>,
    world_stream: Stream,
    fixed_stream: Stream,
    quad_stream: Stream,
    /// The times of the last [`Executor::render`].
    pub timings: FrameTimings,
}

#[derive(Debug)]
struct RingGroups {
    world: wgpu::BindGroup,
    gui: wgpu::BindGroup,
    screen: wgpu::BindGroup,
    fixed: wgpu::BindGroup,
}

impl Executor {
    /// Compile the programs and create the fixed pipelines for `color_format` (the non-sRGB
    /// [`GpuContext::color_format`], or the format of an offscreen target).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, color_format: wgpu::TextureFormat) -> Executor {
        let pipelines = Pipelines::new(device, color_format);
        let samplers = Samplers::new(device, queue);
        let world_modules = [0u8, 1, 2, 3, 4, 5]
            .into_iter()
            .map(|i| (i, create_shader_module(device, &wgsl::SHADERS[i as usize])))
            .collect();
        let fixed_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fixed function"),
            source: wgpu::ShaderSource::Wgsl(FIXED_WGSL.into()),
        });
        let world_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world (executor)"),
            bind_group_layouts: &[Some(&pipelines.layouts.world_uniforms)],
            immediate_size: 0,
        });
        let fixed_uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fixed constants"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(size_of::<FixedConstants>() as u64),
                },
                count: None,
            }],
        });
        let fixed_texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fixed stage 0"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let fixed_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fixed function"),
            bind_group_layouts: &[Some(&fixed_uniform_layout), Some(&fixed_texture_layout)],
            immediate_size: 0,
        });
        let white = GpuTexture::from_rgba8(device, queue, "white", 1, 1, &[255; 4]);
        let align = device.limits().min_uniform_buffer_offset_alignment.max(16);
        Executor {
            color_format,
            pipelines,
            samplers,
            world_modules,
            fixed_module,
            world_layout,
            fixed_uniform_layout,
            fixed_texture_layout,
            fixed_layout,
            cache: HashMap::new(),
            ms_pipelines: HashMap::new(),
            stage0_samplers: HashMap::new(),
            white,
            align,
            ring: Stream::new(device, "uniform ring", wgpu::BufferUsages::UNIFORM),
            ring_groups: None,
            world_stream: Stream::new(device, "immediate world vertices", wgpu::BufferUsages::VERTEX),
            fixed_stream: Stream::new(device, "immediate fixed vertices", wgpu::BufferUsages::VERTEX),
            quad_stream: Stream::new(device, "surface quads", wgpu::BufferUsages::VERTEX),
            timings: FrameTimings::default(),
        }
    }

    /// The colour format the pipelines were built for.
    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    /// Draw one frame into the window: acquire the back buffer, [`Executor::encode`],
    /// submit, present (`EndScene` + `Present 0x004c85f0`). `None` when no back buffer could
    /// be acquired (the frame is skipped, as a lost D3D9 device skipped it).
    pub fn render(&mut self, ctx: &mut GpuContext, frame: &FrameCommands, res: &mut Resources) -> Option<FrameStats> {
        self.render_with_overlay(ctx, frame, res, None)
    }

    /// [`Executor::render`] with a port-only [`super::overlay::OverlayPass`] recorded after the
    /// frame onto the surface texture (the client's debug overlay; `None` is `render`).
    pub fn render_with_overlay(&mut self, ctx: &mut GpuContext, frame: &FrameCommands, res: &mut Resources, overlay: Option<&mut dyn super::overlay::OverlayPass>) -> Option<FrameStats> {
        let t0 = std::time::Instant::now();
        let back = ctx.acquire();
        self.timings = FrameTimings { acquire: t0.elapsed(), ..FrameTimings::default() };
        let back = back?;
        let t1 = std::time::Instant::now();
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let target = match &ctx.msaa_view {
            // The anti-aliasing option: draw multisampled, resolve into the surface texture.
            Some(ms) => FrameTarget {
                color: ms,
                depth: &ctx.depth_view,
                width: ctx.config.width,
                height: ctx.config.height,
                samples: ctx.sample_count(),
                resolve: Some(&back.view),
            },
            None => FrameTarget::new(&back.view, &ctx.depth_view, ctx.config.width, ctx.config.height),
        };
        let stats = self.encode(&ctx.device, &ctx.queue, &mut encoder, target, frame, res);
        // Port-only debug overlay (cw-client `debug_overlay.rs`): last, onto the resolved surface
        // texture, loading the frame.
        let pre = overlay.map_or_else(Vec::new, |o| {
            let t = super::overlay::OverlayTarget { view: &back.view, format: ctx.color_format(), width: ctx.config.width, height: ctx.config.height };
            o.encode(&ctx.device, &ctx.queue, &mut encoder, &t)
        });
        let cmd = encoder.finish();
        let t2 = std::time::Instant::now();
        self.timings.encode = t2 - t1;
        ctx.queue.submit(pre.into_iter().chain([cmd]));
        let t3 = std::time::Instant::now();
        self.timings.submit = t3 - t2;
        ctx.capture_if_requested(&back);
        ctx.present(back);
        self.timings.present = t3.elapsed();
        Some(stats)
    }

    /// The GUI and render-surface pipelines for a back buffer with `samples` samples per
    /// pixel (the anti-aliasing option, `0x0076b1e4 * 2`): built now so the first frame does
    /// not stall. Render surfaces stay single-sampled. `samples` is what
    /// [`GpuContext::set_sample_count`] returned.
    pub fn set_sample_count(&mut self, device: &wgpu::Device, samples: u32) {
        self.ensure_ms_pipelines(device, samples);
    }

    fn ensure_ms_pipelines(&mut self, device: &wgpu::Device, samples: u32) {
        if samples > 1 && !self.ms_pipelines.contains_key(&samples) {
            let p = Pipelines::with_samples(device, self.color_format, samples, self.pipelines.layouts.clone());
            self.ms_pipelines.insert(samples, p);
        }
    }

    fn gui_pipelines(&self, samples: u32) -> &Pipelines {
        if samples > 1 { self.ms_pipelines.get(&samples).unwrap_or(&self.pipelines) } else { &self.pipelines }
    }

    fn pipeline(&mut self, device: &wgpu::Device, key: PipelineState, samples: u32) -> bool {
        if self.cache.contains_key(&(key, samples)) {
            return true;
        }
        let (layout, vs, fs, buffer) = match key.program {
            Program::Shaders { vs: VS_WORLD, ps } if self.world_modules.contains_key(&ps) && ps != VS_WORLD => {
                (&self.world_layout, &self.world_modules[&VS_WORLD], &self.world_modules[&ps], WORLD_VERTEX_LAYOUT)
            }
            Program::FixedFunction { .. } => (&self.fixed_layout, &self.fixed_module, &self.fixed_module, FIXED_VERTEX_LAYOUT),
            _ => return false,
        };
        let cull = match key.cull {
            Cull::None => None,
            Cull::Cw => Some(wgpu::Face::Front),
            Cull::Ccw => Some(wgpu::Face::Back),
        };
        let b = key.blend;
        let blend = b.enabled.then(|| {
            let op = match b.op {
                BlendOp::Add => wgpu::BlendOperation::Add,
                BlendOp::Subtract => wgpu::BlendOperation::Subtract,
            };
            wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: blend_factor(b.src), dst_factor: blend_factor(b.dst), operation: op },
                alpha: wgpu::BlendComponent {
                    src_factor: blend_factor(b.src_alpha),
                    dst_factor: blend_factor(b.dst_alpha),
                    operation: op,
                },
            }
        });
        let d = key.depth;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame state"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: vs,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(buffer)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: cull,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // D3D9: ZENABLE = FALSE turns off the test and the write.
                depth_write_enabled: Some(d.test && d.write),
                depth_compare: Some(if d.test { compare(d.func) } else { wgpu::CompareFunction::Always }),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: fs,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.color_format,
                    blend,
                    write_mask: wgpu::ColorWrites::from_bits_truncate(u32::from(key.color_write)),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.cache.insert((key, samples), pipeline);
        true
    }

    fn stage0_sampler(&mut self, device: &wgpu::Device, s: SamplerState) {
        if self.stage0_samplers.contains_key(&s) {
            return;
        }
        let border = device.features().contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
        let addr = |a: AddressMode| match a {
            AddressMode::Wrap => wgpu::AddressMode::Repeat,
            AddressMode::Clamp => wgpu::AddressMode::ClampToEdge,
            AddressMode::Border if border => wgpu::AddressMode::ClampToBorder,
            AddressMode::Border => wgpu::AddressMode::ClampToEdge,
        };
        let (u, v) = (addr(s.address_u), addr(s.address_v));
        let filter = match s.filter {
            FilterMode::Point => wgpu::FilterMode::Nearest,
            FilterMode::Linear => wgpu::FilterMode::Linear,
        };
        let uses_border = u == wgpu::AddressMode::ClampToBorder || v == wgpu::AddressMode::ClampToBorder;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stage 0"),
            address_mode_u: u,
            address_mode_v: v,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            // D3DSAMP_BORDERCOLOR is never set: the default 0x00000000.
            border_color: uses_border.then_some(wgpu::SamplerBorderColor::TransparentBlack),
            ..Default::default()
        });
        self.stage0_samplers.insert(s, sampler);
    }

    /// Record `frame` into `encoder`, drawing into `target`. Uploads the uniform ring, the
    /// immediate streams and the GUI stream through `queue` (the writes land before the
    /// encoder's commands when it is submitted after this call).
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        frame: &FrameCommands,
        res: &mut Resources,
    ) -> FrameStats {
        let mut stats = FrameStats::default();
        let mut ring = Ring { data: Vec::new(), align: self.align as usize };
        let mut world_verts: Vec<crate::mesh::WorldVertex> = Vec::new();
        let mut fixed_verts: Vec<FixedGpuVertex> = Vec::new();
        let mut quad_verts: Vec<GuiVertex> = Vec::new();
        let mut ops: Vec<Op> = Vec::new();

        // ---- 1. prepare --------------------------------------------------------------
        self.ensure_ms_pipelines(device, target.samples);
        // Samples per pixel of a target: the back buffer's, 1 for every render surface.
        let samples_of = |t: Option<Target>| match t {
            Some(Target::Surface(_)) => 1,
            _ => target.samples.max(1),
        };
        let size_of_target = |t: Target, res: &Resources| -> Option<(u32, u32)> {
            match t {
                Target::Back => Some((target.width, target.height)),
                Target::Surface(id) => res.surfaces.get(&id).map(|s| (s.width, s.height)),
            }
        };
        let mut current: Option<Target> = None;
        for pass in &frame.passes {
            let t = match pass.target {
                RenderTarget::Backbuffer => Target::Back,
                RenderTarget::Surface(id) => Target::Surface(id),
            };
            if size_of_target(t, res).is_none() {
                stats.skipped += pass.draws.len() as u32;
                continue;
            }
            if pass.clear.is_some() || current != Some(t) {
                let c = pass.clear.unwrap_or(Clear { color_argb: None, depth: None, stencil: None });
                ops.push(Op::Begin { target: t, color: c.color_argb.map(argb_color), depth: c.depth, stencil: c.stencil });
                current = Some(t);
            }
            for draw in &pass.draws {
                match &draw.geometry {
                    Geometry::Gui(cmds) => {
                        self.prepare_gui(cmds, &mut ring, &mut quad_verts, &mut ops, &mut current, res, target, &mut stats);
                    }
                    Geometry::Map(_) => stats.skipped += 1,
                    Geometry::FixedImmediate { topology, vertices, texture, transforms } => {
                        let key = pipeline_key(&draw.state);
                        let samples = samples_of(current);
                        if !self.pipeline(device, key, samples) {
                            stats.skipped += 1;
                            continue;
                        }
                        self.stage0_sampler(device, draw.state.sampler0);
                        let wvp = mat_mul(&mat_mul(&transforms[0], &transforms[1]), &transforms[2]);
                        let textured = texture.is_some() && matches!(draw.state.program, Program::FixedFunction { fvf } if fvf & 0x100 != 0);
                        let off = ring.push(&FixedConstants { wvp: registers(&wvp), flags: [f32::from(u8::from(textured)), 0.0, 0.0, 0.0] });
                        let first = fixed_verts.len() as u32;
                        for i in triangle_list_indices(*topology, vertices.len()) {
                            fixed_verts.push(FixedGpuVertex::from(&vertices[i]));
                        }
                        let count = fixed_verts.len() as u32 - first;
                        ops.push(Op::Fixed {
                            key,
                            samples,
                            sampler: draw.state.sampler0,
                            off,
                            first,
                            count,
                            texture: if textured { *texture } else { None },
                        });
                    }
                    geometry => {
                        let key = pipeline_key(&draw.state);
                        let samples = samples_of(current);
                        if !self.pipeline(device, key, samples) || matches!(key.program, Program::FixedFunction { .. }) {
                            stats.skipped += 1;
                            continue;
                        }
                        let geom = match geometry {
                            Geometry::Chunk { chunk, buffer, index_buffer, primitive_count, .. } => WorldGeom::Chunk {
                                chunk: *chunk,
                                buffer: *buffer,
                                water: *index_buffer == ChunkIndexBuffer::Water,
                                index_count: primitive_count.saturating_mul(3),
                            },
                            Geometry::Model { model } => WorldGeom::Model(*model),
                            Geometry::WorldImmediate { topology, vertices } => {
                                let first = world_verts.len() as u32;
                                for i in triangle_list_indices(*topology, vertices.len()) {
                                    let v = &vertices[i];
                                    world_verts.push(crate::mesh::WorldVertex { pos: v.position, color: v.color_argb });
                                }
                                WorldGeom::Immediate { first, count: world_verts.len() as u32 - first }
                            }
                            _ => unreachable!("handled above"),
                        };
                        let (vs, ps) = world_constants(frame, draw);
                        let vs = ring.push(&vs);
                        let ps = ring.push(&ps);
                        ops.push(Op::World { key, samples, vs, ps, geom });
                    }
                }
            }
        }

        // ---- 2. upload -----------------------------------------------------------------
        if ring.data.is_empty() {
            ring.data.resize(self.align as usize, 0);
        }
        // The largest binding must fit after the last offset.
        let tail = size_of::<WorldVsConstants>().max(size_of::<GuiVsConstants>());
        ring.data.resize(ring.data.len() + tail.next_multiple_of(self.align as usize), 0);
        if self.ring.upload(device, queue, &ring.data) || self.ring_groups.is_none() {
            self.ring_groups = Some(self.ring_groups(device));
        }
        self.world_stream.upload(device, queue, bytemuck::cast_slice(&world_verts));
        self.fixed_stream.upload(device, queue, bytemuck::cast_slice(&fixed_verts));
        self.quad_stream.upload(device, queue, bytemuck::cast_slice(&quad_verts));
        res.gui_vb.upload(device, queue, bytemuck::cast_slice(&res.gui_vertices));
        res.gui_ib.upload(device, queue, bytemuck::cast_slice(&res.gui_indices));

        // ---- 3. encode -----------------------------------------------------------------
        let res: &Resources = res;
        let groups = self.ring_groups.as_ref().expect("created above");
        let mut pass: Option<wgpu::RenderPass<'static>> = None;
        let mut current: Option<Target> = None;
        let mut fixed_groups: HashMap<(Option<TextureRef>, SamplerState), wgpu::BindGroup> = HashMap::new();
        for op in &ops {
            match op {
                Op::Begin { target: t, color, depth, stencil } => {
                    drop(pass.take());
                    let (cv, dv) = match t {
                        Target::Back => (target.color, target.depth),
                        Target::Surface(id) => {
                            let s = &res.surfaces[id];
                            (&s.color_view, &s.depth_view)
                        }
                    };
                    let rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("frame pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: cv,
                            depth_slice: None,
                            resolve_target: if *t == Target::Back { target.resolve } else { None },
                            ops: wgpu::Operations {
                                load: color.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: dv,
                            depth_ops: Some(wgpu::Operations {
                                load: depth.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: Some(wgpu::Operations {
                                load: stencil.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                                store: wgpu::StoreOp::Store,
                            }),
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass = Some(rp.forget_lifetime());
                    current = Some(*t);
                    stats.render_passes += 1;
                }
                Op::World { key, samples, vs, ps, geom } => {
                    let Some(p) = pass.as_mut() else { continue };
                    let (vb, ib) = match *geom {
                        WorldGeom::Chunk { chunk, buffer, water, index_count } => {
                            let Some(b) = res.chunks.get(&chunk).and_then(|c| c.get(buffer)) else {
                                stats.skipped += 1;
                                continue;
                            };
                            let (ib, n) = if water { (&b.water, b.water_indices) } else { (&b.opaque, b.opaque_indices) };
                            let (Some(vb), Some(ib)) = (&b.vertices, ib) else {
                                stats.skipped += 1;
                                continue;
                            };
                            (vb.slice(..), Some((ib.slice(..), wgpu::IndexFormat::Uint32, index_count.min(n))))
                        }
                        WorldGeom::Model(m) => {
                            let Some(mesh) = res.models.get(m) else {
                                // An absent model, or an empty one (nothing to draw).
                                if !res.models.contains(m) {
                                    stats.skipped += 1;
                                }
                                continue;
                            };
                            (mesh.vertices.slice(..), Some((mesh.indices.slice(..), mesh.index_format, mesh.index_count)))
                        }
                        WorldGeom::Immediate { first, count } => {
                            if count == 0 {
                                continue;
                            }
                            let stride = 8u64;
                            (self.world_stream.buffer.slice(u64::from(first) * stride..u64::from(first + count) * stride), None)
                        }
                    };
                    p.set_pipeline(&self.cache[&(*key, *samples)]);
                    p.set_bind_group(0, &groups.world, &[*vs, *ps]);
                    p.set_vertex_buffer(0, vb);
                    match ib {
                        Some((ib, format, n)) => {
                            p.set_index_buffer(ib, format);
                            p.draw_indexed(0..n, 0, 0..1);
                        }
                        None => {
                            let WorldGeom::Immediate { count, .. } = *geom else { unreachable!() };
                            p.draw(0..count, 0..1);
                        }
                    }
                    stats.draws += 1;
                }
                Op::Fixed { key, samples, sampler, off, first, count, texture } => {
                    let Some(p) = pass.as_mut() else { continue };
                    if *count == 0 {
                        continue;
                    }
                    let view = match texture {
                        Some(id) => match res.textures.get(id) {
                            Some(t) => &t.texture.view,
                            None => {
                                stats.skipped += 1;
                                continue;
                            }
                        },
                        None => &self.white.view,
                    };
                    let bg = fixed_groups.entry((*texture, *sampler)).or_insert_with(|| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("fixed stage 0"),
                            layout: &self.fixed_texture_layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&self.stage0_samplers[sampler]),
                                },
                            ],
                        })
                    });
                    p.set_pipeline(&self.cache[&(*key, *samples)]);
                    p.set_bind_group(0, &groups.fixed, &[*off]);
                    p.set_bind_group(1, &*bg, &[]);
                    let stride = 24u64;
                    p.set_vertex_buffer(0, self.fixed_stream.buffer.slice(u64::from(*first) * stride..u64::from(first + count) * stride));
                    p.draw(0..*count, 0..1);
                    stats.draws += 1;
                }
                Op::Gui { subtract, vs, ps, base_vertex, indices, mask, mask_surface, texture } => {
                    let Some(p) = pass.as_mut() else { continue };
                    if indices.is_empty() {
                        continue;
                    }
                    if indices.end as usize > res.gui_indices.len() {
                        stats.skipped += 1;
                        continue;
                    }
                    // Performance note for a future optimiser: one texture bind group per GUI
                    // draw; a cache keyed by (mask, texture) would save the creations.
                    let slot = |t: &Option<TextureRef>| match t.and_then(|id| res.textures.get(&id)) {
                        Some(e) => (&e.texture.view, &e.sampler),
                        None => (&self.samplers.dummy_texture, &self.samplers.linear_clamp),
                    };
                    let (mv, ms) = match mask_surface {
                        // popSurface 0x006509f0 left the clipping node's level surface in
                        // Engine+0x4c; bound as s0 (LINEAR/CLAMP) by 0x0068ab70.
                        Some(id) => {
                            if current == Some(Target::Surface(*id)) {
                                stats.skipped += 1;
                                continue;
                            }
                            match res.surfaces.get(id) {
                                Some(s) => (&s.color_view, &self.samplers.linear_clamp),
                                None => {
                                    stats.skipped += 1;
                                    continue;
                                }
                            }
                        }
                        None => slot(mask),
                    };
                    let (tv, ts) = slot(texture);
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("gui textures"),
                        layout: &self.pipelines.layouts.gui_textures,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(mv) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(ms) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(tv) },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(ts) },
                        ],
                    });
                    let kind = if *subtract { PipelineKind::GuiSubtract } else { PipelineKind::Gui };
                    p.set_pipeline(self.gui_pipelines(samples_of(current)).get(kind));
                    p.set_bind_group(0, &groups.gui, &[*vs, *ps]);
                    p.set_bind_group(1, &bg, &[]);
                    p.set_vertex_buffer(0, res.gui_vb.buffer.slice(..));
                    p.set_index_buffer(res.gui_ib.buffer.slice(..), wgpu::IndexFormat::Uint32);
                    p.draw_indexed(indices.clone(), *base_vertex, 0..1);
                    stats.draws += 1;
                }
                Op::Screen { kind, point, source, vs, ps, first } => {
                    let Some(p) = pass.as_mut() else { continue };
                    let Some(src) = res.surfaces.get(source) else {
                        stats.skipped += 1;
                        continue;
                    };
                    if current == Some(Target::Surface(*source)) {
                        // A surface cannot be sampled while it is the render target.
                        stats.skipped += 1;
                        continue;
                    }
                    let sampler = if *point { &self.samplers.point_clamp } else { &self.samplers.linear_clamp };
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("screen texture"),
                        layout: &self.pipelines.layouts.screen_texture,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src.color_view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                        ],
                    });
                    p.set_pipeline(self.gui_pipelines(samples_of(current)).get(*kind));
                    p.set_bind_group(0, &groups.screen, &[*vs, *ps]);
                    p.set_bind_group(1, &bg, &[]);
                    let stride = 48u64;
                    p.set_vertex_buffer(0, self.quad_stream.buffer.slice(u64::from(*first) * stride..u64::from(first + 6) * stride));
                    p.draw(0..6, 0..1);
                    stats.draws += 1;
                }
            }
        }
        drop(pass);
        stats.pipelines = self.cache.len() as u32;
        stats
    }

    /// The GUI stream of one [`Geometry::Gui`] draw: render-surface switches, widget draws
    /// and the surface quads (`D3D9RenderSurface` slots 2, 3, 5, 6, 7).
    #[allow(clippy::too_many_arguments)]
    fn prepare_gui(
        &mut self,
        cmds: &[GuiCommand],
        ring: &mut Ring,
        quads: &mut Vec<GuiVertex>,
        ops: &mut Vec<Op>,
        current: &mut Option<Target>,
        res: &Resources,
        target: FrameTarget<'_>,
        stats: &mut FrameStats,
    ) {
        let size = |t: Target| match t {
            Target::Back => res.gui_viewport.unwrap_or((target.width, target.height)),
            Target::Surface(id) => res.surfaces.get(&id).map_or((1, 1), |s| (s.width, s.height)),
        };
        let mut stack: Vec<Target> = Vec::new();
        // Nesting depth of BeginSurface calls on surfaces that do not exist: their content is
        // dropped until the matching EndSurface.
        let mut missing = 0u32;
        for cmd in cmds {
            match cmd {
                GuiCommand::BeginSurface { surface, clear_argb } => {
                    if missing > 0 || !res.surfaces.contains_key(surface) {
                        missing += 1;
                        continue;
                    }
                    stack.push(current.unwrap_or(Target::Back));
                    let t = Target::Surface(*surface);
                    // clear 0x0068c6e0 clears depth with the colour.
                    ops.push(Op::Begin {
                        target: t,
                        color: clear_argb.map(argb_color),
                        depth: clear_argb.map(|_| 1.0),
                        stencil: clear_argb.map(|_| 0),
                    });
                    *current = Some(t);
                }
                GuiCommand::EndSurface => {
                    if missing > 0 {
                        missing -= 1;
                        continue;
                    }
                    let t = stack.pop().unwrap_or(Target::Back);
                    ops.push(Op::Begin { target: t, color: None, depth: None, stencil: None });
                    *current = Some(t);
                }
                // A position marker for the GUI item models (`crate::passes`); draws nothing.
                GuiCommand::WidgetMark(_) => {}
                _ if missing > 0 => stats.skipped += 1,
                GuiCommand::Draw(g) => {
                    let m = registers;
                    let vs = GuiVsConstants {
                        proj: m(&g.proj),
                        world_view: m(&g.world_view),
                        widget_bind_matrix: m(&g.widget_bind_matrix),
                        inverse_widget_bind_matrix: m(&g.inverse_widget_bind_matrix),
                        mask_matrix: g.mask_matrix,
                        texture_matrix: g.texture_matrix,
                        normal_matrix: g.normal_matrix,
                        widget_bind_pos: [g.widget_bind_pos[0], g.widget_bind_pos[1], 0.0, 0.0],
                        widget_bind_size: [g.widget_bind_size[0], g.widget_bind_size[1], 0.0, 0.0],
                        deformed_widget_pos: [g.deformed_widget_pos[0], g.deformed_widget_pos[1], 0.0, 0.0],
                        deformed_widget_size: [g.deformed_widget_size[0], g.deformed_widget_size[1], 0.0, 0.0],
                        aa_offset: [g.aa_offset, 0.0, 0.0, 0.0],
                        base_color: g.base_color,
                        widget_deformation_enabled: [u32::from(g.deformation_enabled), 0, 0, 0],
                    };
                    let ps = GuiPsConstants {
                        filter: [f32::from(g.filter), 0.0, 0.0, 0.0],
                        texture_opacity: [g.texture_opacity, 0.0, 0.0, 0.0],
                        texture_brightness: [g.texture_brightness, 0.0, 0.0, 0.0],
                        texture_contrast: [g.texture_contrast, 0.0, 0.0, 0.0],
                        texture_saturation: [g.texture_saturation, 0.0, 0.0, 0.0],
                        texture_enabled: [u32::from(g.texture_enabled), 0, 0, 0],
                    };
                    let vs = ring.push(&vs);
                    let ps = ring.push(&ps);
                    ops.push(Op::Gui {
                        subtract: g.subtract,
                        vs,
                        ps,
                        base_vertex: g.vertices.start as i32,
                        indices: g.indices.clone(),
                        mask: g.mask,
                        mask_surface: g.mask_surface,
                        texture: g.texture,
                    });
                }
                GuiCommand::Blit { surface, color, rect, src_scale } => {
                    self.quad(ring, quads, ops, size(current.unwrap_or(Target::Back)), *surface, PipelineKind::Blit, false, [0.0; 4], *color, *rect, *src_scale);
                }
                GuiCommand::Blur { surface, horizontal, radius, rect, src_scale } => {
                    let width = res.surfaces.get(surface).map_or(1, |s| s.width) as f32;
                    // 0x0068cad0: (radius · 0.25) / width for both directions.
                    let c = BlurPsConstants { blur_scale: [(radius * 0.25) / width, 0.0, 0.0, 0.0] };
                    let kind = if *horizontal { PipelineKind::BlurHorizontal } else { PipelineKind::BlurVertical };
                    self.quad(ring, quads, ops, size(current.unwrap_or(Target::Back)), *surface, kind, false, c.blur_scale, [1.0; 4], *rect, *src_scale);
                }
                GuiCommand::Downsample { surface, factor, texel_size, rect } => {
                    let kind = match factor {
                        2 => PipelineKind::Downsample2,
                        3 => PipelineKind::Downsample3,
                        4 => PipelineKind::Downsample4,
                        _ => PipelineKind::Blit,
                    };
                    let c = DownsamplePsConstants { texture_scale: [texel_size[0], texel_size[1], 0.0, 0.0] };
                    self.quad(ring, quads, ops, size(current.unwrap_or(Target::Back)), *surface, kind, true, c.texture_scale, [1.0; 4], *rect, 1.0);
                }
            }
        }
        // An unbalanced stream leaves the GUI where it ended; return to the frame's target
        // like the original's endFrame would have.
        if let Some(&bottom) = stack.first() {
            ops.push(Op::Begin { target: bottom, color: None, depth: None, stencil: None });
            *current = Some(bottom);
        }
    }

    /// One drawQuad 0x00689f30 of a render surface into the current target.
    #[allow(clippy::too_many_arguments)]
    fn quad(
        &self,
        ring: &mut Ring,
        quads: &mut Vec<GuiVertex>,
        ops: &mut Vec<Op>,
        target_size: (u32, u32),
        source: u32,
        kind: PipelineKind,
        point: bool,
        ps_constant: [f32; 4],
        color: [f32; 4],
        rect: [f32; 4],
        src_scale: f32,
    ) {
        let vs = ring.push(&ScreenVsConstants { proj: gui_projection(target_size.0, target_size.1) });
        let ps = ring.push(&ps_constant);
        let v = quad_vertices([rect[0], rect[1]], [rect[2], rect[3]], [0.0, 0.0], [src_scale, src_scale], color);
        let first = quads.len() as u32;
        // QUAD_INDICES 0, 1, 2, 2, 3, 0 expanded.
        quads.extend([v[0], v[1], v[2], v[2], v[3], v[0]]);
        ops.push(Op::Screen { kind, point, source, vs, ps, first });
    }

    fn ring_groups(&self, device: &wgpu::Device) -> RingGroups {
        let buf = &self.ring.buffer;
        let binding = |size: usize| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: buf, offset: 0, size: NonZeroU64::new(size as u64) })
        };
        let l = &self.pipelines.layouts;
        let two = |layout: &wgpu::BindGroupLayout, label: &str, a: usize, b: usize| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: binding(a) },
                    wgpu::BindGroupEntry { binding: 1, resource: binding(b) },
                ],
            })
        };
        RingGroups {
            world: two(&l.world_uniforms, "world ring", size_of::<WorldVsConstants>(), size_of::<WorldPsConstants>()),
            gui: two(&l.gui_uniforms, "gui ring", size_of::<GuiVsConstants>(), size_of::<GuiPsConstants>()),
            screen: two(&l.screen_uniforms, "screen ring", size_of::<ScreenVsConstants>(), size_of::<BlurPsConstants>()),
            fixed: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fixed ring"),
                layout: &self.fixed_uniform_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: binding(size_of::<FixedConstants>()) }],
            }),
        }
    }
}

/// The CubeShader registers of one draw: VS c0..c39 (point lights of its set through
/// [`LightSet::upload`], then [`FrameCommands::vs_constants`]) and PS c0..c4.
///
/// Performance note for a future optimiser: every draw gets its own 640 + 80 byte block even
/// when only the world matrix changed; splitting per-pass and per-draw registers would cut
/// the ring by an order of magnitude.
pub fn world_constants(frame: &FrameCommands, draw: &Draw) -> (WorldVsConstants, WorldPsConstants) {
    let (pos, col) = frame.light_sets.get(draw.draw.light_set).map(LightSet::upload).unwrap_or_default();
    let c = frame.vs_constants(draw);
    let mut regs = [[0.0f32; 4]; 40];
    regs[..10].copy_from_slice(&pos);
    regs[10..20].copy_from_slice(&col);
    regs[20..].copy_from_slice(&c);
    let vs: WorldVsConstants = bytemuck::cast(regs);
    let u = &frame.uniforms[draw.uniforms];
    let ps = WorldPsConstants {
        sky_color1: u.sky_color1,
        sky_color2: u.sky_color2,
        fog_color: u.fog_color,
        alpha: [draw.draw.alpha, 0.0, 0.0, 0.0],
        shininess: [draw.draw.shininess, 0.0, 0.0, 0.0],
    };
    (vs, ps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fans_and_strips_expand() {
        assert_eq!(triangle_list_indices(Topology::TriangleFan, 4), vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(triangle_list_indices(Topology::TriangleStrip, 5), vec![0, 1, 2, 2, 1, 3, 2, 3, 4]);
        assert_eq!(triangle_list_indices(Topology::TriangleList, 7), (0..6).collect::<Vec<_>>());
        assert!(triangle_list_indices(Topology::TriangleFan, 2).is_empty());
    }

    #[test]
    fn argb_clear_colour() {
        let c = argb_color(0xff00_70ff);
        assert_eq!((c.r, c.a), (0.0, 1.0));
        assert!((c.g - 112.0 / 255.0).abs() < 1e-12);
        assert_eq!(c.b, 1.0);
    }

    #[test]
    fn fixed_vertex_is_24_bytes() {
        assert_eq!(size_of::<FixedGpuVertex>(), 24);
        let v = FixedGpuVertex::from(&FixedVertex { position: [1.0, 2.0, 3.0], color_argb: 0x11223344, uv: [0.5, 0.25] });
        assert_eq!(v.color, [0x44, 0x33, 0x22, 0x11]);
    }
}
