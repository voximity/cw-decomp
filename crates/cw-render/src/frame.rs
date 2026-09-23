//! Backend-independent frame description: what `GameController::render` (`Cube.exe
//! 0x004ac260`) asks the device to do, as data.
//!
//! The original is immediate-mode Direct3D 9: one straight-line function that sets render
//! states, CubeShader constants and transforms, then draws. This module is the modern seam
//! between that logic (ported in [`crate::passes`]) and the wgpu backend (`crate::gpu`):
//!
//! - a [`FrameCommands`] is an ordered list of [`Pass`]es;
//! - a pass names what it draws ([`PassKind`]), the original address range it comes from,
//!   its render target, an optional clear and the pipeline state at its start;
//! - each [`Draw`] carries the pipeline state and the shader constants that were current
//!   when the original issued it, so a backend can batch or reorder only where the result
//!   cannot change.
//!
//! Conventions: matrices are [`D3dMatrix`], Direct3D row-major with row vectors (`v * M`),
//! exactly as the original stores and multiplies them. Colours are linear `[r, g, b, a]`
//! floats unless named `argb` (a `D3DCOLOR`). Positions in the frame are *render space*:
//! world fixed-point coordinates (65536 per block) plus the render offset at
//! `GameController+0x1d8`, divided by 65536, so the unit is the block and the world is Z-up.
//!
//! Tier C (see `analysis/notes/porting-brief-client.md`): D3D9-only quirks are dropped here
//! (the -0.135 px half-pixel offset of shaders 06/08, the `textureScale` upload bug of
//! `0x0068cf10`); everything the shaders read is kept.

/// A Direct3D 9 matrix: row-major, row vectors (`v' = v * M`), translation in row 3.
pub type D3dMatrix = [[f32; 4]; 4];

/// The identity [`D3dMatrix`].
pub const IDENTITY: D3dMatrix = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Index of one of the fifteen embedded shaders, as numbered in
/// `analysis/shaders/manifest.json` (`NN_<kind>_<rva>`).
pub type ShaderId = u8;

/// `00`: world vertex shader (CubeShader +4), 8-byte `UBYTE4`+`D3DCOLOR` vertex.
pub const VS_WORLD: ShaderId = 0;
/// `01`: world pixel shader, blocks, creatures, items (bound by `0x00447d10`).
pub const PS_WORLD: ShaderId = 1;
/// `02`: sky background gradient (bound by `0x00447d90`).
pub const PS_SKY: ShaderId = 2;
/// `03`: water (bound by `0x00447dd0`).
pub const PS_WATER: ShaderId = 3;
/// `04`: never bound by the original (dead); listed for completeness.
pub const PS_UNUSED_FOG: ShaderId = 4;
/// `05`: clouds (bound by `0x00447d50`).
pub const PS_CLOUD: ShaderId = 5;
/// `06`: GUI vertex shader (`D3D9Engine+0x194`).
pub const VS_GUI: ShaderId = 6;
/// `07`: GUI pixel shader (`D3D9Engine+0x198`).
pub const PS_GUI: ShaderId = 7;
/// `08`: render-surface screen quad vertex shader.
pub const VS_QUAD: ShaderId = 8;
/// `09`: plain blit with un-premultiply.
pub const PS_BLIT: ShaderId = 9;
/// `10`/`11`/`12`: N×N box downsample, N = 2, 3, 4.
pub const PS_DOWNSAMPLE_2: ShaderId = 10;
/// See [`PS_DOWNSAMPLE_2`].
pub const PS_DOWNSAMPLE_3: ShaderId = 11;
/// See [`PS_DOWNSAMPLE_2`].
pub const PS_DOWNSAMPLE_4: ShaderId = 12;
/// `13`: 9-tap Gaussian blur along y (ps_2_0).
pub const PS_BLUR_V: ShaderId = 13;
/// `14`: 9-tap Gaussian blur along x.
pub const PS_BLUR_H: ShaderId = 14;

/// The programs a draw can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Program {
    /// A pair of the embedded shaders.
    Shaders {
        /// Vertex shader index.
        vs: ShaderId,
        /// Pixel shader index.
        ps: ShaderId,
    },
    /// The D3D9 fixed-function pipeline (`SetVertexShader(NULL)`, `SetPixelShader(NULL)`),
    /// lighting off: vertex colour, optionally modulated by texture stage 0 with the default
    /// `MODULATE` stage state. `fvf` is the vertex format: `0x42` (`XYZ|DIFFUSE`) or `0x142`
    /// (`XYZ|DIFFUSE|TEX1`). The port replaces it with a tiny WGSL "unlit" program.
    FixedFunction {
        /// `D3DFVF_*` bits.
        fvf: u32,
    },
}

impl Program {
    /// CubeShader default, VS 00 + PS 01 (`0x00447d10`).
    pub const WORLD: Program = Program::Shaders { vs: VS_WORLD, ps: PS_WORLD };
    /// VS 00 + PS 02 (`0x00447d90`).
    pub const SKY: Program = Program::Shaders { vs: VS_WORLD, ps: PS_SKY };
    /// VS 00 + PS 03 (`0x00447dd0`).
    pub const WATER: Program = Program::Shaders { vs: VS_WORLD, ps: PS_WATER };
    /// VS 00 + PS 05 (`0x00447d50`).
    pub const CLOUD: Program = Program::Shaders { vs: VS_WORLD, ps: PS_CLOUD };
    /// VS 06 + PS 07 (`0x00688e70`, `0x0068ab70`).
    pub const GUI: Program = Program::Shaders { vs: VS_GUI, ps: PS_GUI };
    /// Fixed function, `XYZ|DIFFUSE`.
    pub const FIXED_COLOR: Program = Program::FixedFunction { fvf: 0x42 };
    /// Fixed function, `XYZ|DIFFUSE|TEX1`.
    pub const FIXED_TEXTURED: Program = Program::FixedFunction { fvf: 0x142 };
}

/// `D3DRS_CULLMODE` values. Note the original's value 2 is `D3DCULL_CW` (clockwise faces
/// are culled), not CCW as `analysis/shaders/README.md` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cull {
    /// `D3DCULL_NONE` (1).
    None,
    /// `D3DCULL_CW` (2): cull faces with clockwise winding in screen space.
    Cw,
    /// `D3DCULL_CCW` (3).
    Ccw,
}

/// `D3DCMPFUNC` subset used by the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareFunc {
    /// `D3DCMP_LESS` (2): set by the HUD model helper `0x00476660`.
    Less,
    /// `D3DCMP_LESSEQUAL` (4): set at the start of every frame (`0x004ac35a`).
    LessEqual,
    /// `D3DCMP_GREATER` (5): GUI drawings `0x0068ab70` (not traced further).
    Greater,
    /// `D3DCMP_ALWAYS` (8).
    Always,
}

/// Depth-buffer state (`D3DRS_ZENABLE`, `ZWRITEENABLE`, `ZFUNC`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepthState {
    /// `D3DRS_ZENABLE`.
    pub test: bool,
    /// `D3DRS_ZWRITEENABLE`.
    pub write: bool,
    /// `D3DRS_ZFUNC`.
    pub func: CompareFunc,
}

/// `D3DBLEND` factors used by the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendFactor {
    /// `D3DBLEND_ZERO` (1).
    Zero,
    /// `D3DBLEND_ONE` (2).
    One,
    /// `D3DBLEND_SRCALPHA` (5).
    SrcAlpha,
    /// `D3DBLEND_INVSRCALPHA` (6).
    InvSrcAlpha,
}

/// `D3DBLENDOP` values used by the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendOp {
    /// `D3DBLENDOP_ADD`.
    Add,
    /// `D3DBLENDOP_SUBTRACT` (caret and inverted rectangles, `0x006897c0`/`0x00689950`).
    Subtract,
}

/// Alpha blending (`ALPHABLENDENABLE`, `SRCBLEND`, `DESTBLEND`, `SEPARATEALPHABLENDENABLE`,
/// `SRCBLENDALPHA`, `DESTBLENDALPHA`, `BLENDOP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlendState {
    /// `D3DRS_ALPHABLENDENABLE`.
    pub enabled: bool,
    /// Colour source factor.
    pub src: BlendFactor,
    /// Colour destination factor.
    pub dst: BlendFactor,
    /// Alpha source factor (`SEPARATEALPHABLENDENABLE` on).
    pub src_alpha: BlendFactor,
    /// Alpha destination factor.
    pub dst_alpha: BlendFactor,
    /// Colour and alpha operation.
    pub op: BlendOp,
}

impl BlendState {
    /// The state `D3D9Engine::beginFrame 0x00688b60` leaves behind and every world pass
    /// inherits: colour `SRCALPHA/INVSRCALPHA`, alpha `ONE/INVSRCALPHA`.
    pub const ALPHA: BlendState = BlendState {
        enabled: true,
        src: BlendFactor::SrcAlpha,
        dst: BlendFactor::InvSrcAlpha,
        src_alpha: BlendFactor::One,
        dst_alpha: BlendFactor::InvSrcAlpha,
        op: BlendOp::Add,
    };
}

/// `D3DTEXTUREADDRESS` subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressMode {
    /// `D3DTADDRESS_WRAP` (1), the device default.
    Wrap,
    /// `D3DTADDRESS_CLAMP` (3).
    Clamp,
    /// `D3DTADDRESS_BORDER` (4).
    Border,
}

/// `D3DTEXTUREFILTERTYPE` subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterMode {
    /// `D3DTEXF_POINT`, the device default for MIN/MAG.
    Point,
    /// `D3DTEXF_LINEAR`.
    Linear,
}

/// Sampler state of stage 0 (fixed function) or of the GUI/render-surface samplers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SamplerState {
    /// `D3DSAMP_ADDRESSU`.
    pub address_u: AddressMode,
    /// `D3DSAMP_ADDRESSV`.
    pub address_v: AddressMode,
    /// `D3DSAMP_MINFILTER`/`MAGFILTER`.
    pub filter: FilterMode,
}

impl SamplerState {
    /// Device default: wrap, point.
    pub const DEFAULT: SamplerState = SamplerState {
        address_u: AddressMode::Wrap,
        address_v: AddressMode::Wrap,
        filter: FilterMode::Point,
    };
}

/// Every piece of fixed state that changes what a draw writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineState {
    /// Bound programs.
    pub program: Program,
    /// Depth state.
    pub depth: DepthState,
    /// Blend state.
    pub blend: BlendState,
    /// Face culling.
    pub cull: Cull,
    /// `D3DRS_COLORWRITEENABLE` mask (0xF = RGBA, 0 = depth only).
    pub color_write: u8,
    /// Stage 0 sampler (fixed-function textured draws).
    pub sampler0: SamplerState,
}

/// Where a pass renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderTarget {
    /// The swap-chain back buffer with its D24S8 depth buffer.
    Backbuffer,
    /// A plasma render surface (`D3D9RenderSurface`, `0x0068bf70`) by the GUI's id.
    Surface(u32),
}

/// A `Clear` call. `None` fields are not cleared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clear {
    /// Colour as `D3DCOLOR` ARGB.
    pub color_argb: Option<u32>,
    /// Depth value.
    pub depth: Option<f32>,
    /// Stencil value.
    pub stencil: Option<u32>,
}

/// One point light as the CubeShader receives it (`setPointLights 0x00448f10`): a
/// position with the radius in `w` (`pointLightPositions`, c0..) and an RGB colour
/// (`pointLightColors`, c10..; the upload pads `w` with 0).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PointLight {
    /// Render-space position (blocks).
    pub position: [f32; 3],
    /// Radius in blocks; a light is off when `radius <= 0`.
    pub radius: f32,
    /// Linear RGB colour.
    pub color: [f32; 3],
}

/// Maximum lights a set holds: the original's arrays are 16 long (a chunk stops accepting
/// lights at 16, `0x004b1180`; the global set is capped at 16, `0x004b12ea`).
pub const LIGHT_SET_CAPACITY: usize = 16;

/// Lights actually uploaded. `setPointLights 0x00448f10` calls
/// `SetVertexShaderConstantF(c0, positions, 10)` and `(c10, colours, 10)`: only the first
/// ten of the sixteen reach the shader, which also only reads ten. This settles the
/// "16 vs 10" question: lights 10..15 are dropped, nothing overlaps.
pub const LIGHTS_UPLOADED: usize = 10;

/// A set of point lights (per chunk, global, or all-zero).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LightSet {
    /// Up to [`LIGHT_SET_CAPACITY`] lights in insertion order; missing slots are zero.
    pub lights: Vec<PointLight>,
}

impl LightSet {
    /// The two constant arrays exactly as uploaded: `(c0..c9, c10..c19)`.
    pub fn upload(&self) -> ([[f32; 4]; LIGHTS_UPLOADED], [[f32; 4]; LIGHTS_UPLOADED]) {
        let mut pos = [[0.0; 4]; LIGHTS_UPLOADED];
        let mut col = [[0.0; 4]; LIGHTS_UPLOADED];
        for (i, l) in self.lights.iter().take(LIGHTS_UPLOADED).enumerate() {
            pos[i] = [l.position[0], l.position[1], l.position[2], l.radius];
            col[i] = [l.color[0], l.color[1], l.color[2], 0.0];
        }
        (pos, col)
    }
}

/// CubeShader constants that change per pass or per phase, not per draw. Each field names
/// its register (CTAB of shader 00/01, `analysis/shaders/README.md`) and its setter.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeUniforms {
    /// View matrix (third argument chain of `setTransforms 0x004482a0`).
    pub view: D3dMatrix,
    /// Projection matrix.
    pub projection: D3dMatrix,
    /// VS c36 `cameraPosition`, render space (`setCameraPosition 0x00448010`, w = 0).
    pub camera_position: [f32; 3],
    /// VS c33 `lightDirection`, normalised by `setLight 0x00448170`.
    pub light_direction: [f32; 3],
    /// VS c34 `lightFrontColor`.
    pub light_front: [f32; 4],
    /// VS c35 `lightBackColor`.
    pub light_back: [f32; 4],
    /// VS c38 `ambientColor`.
    pub ambient: [f32; 4],
    /// VS c37 `darknessColor` (`setDarknessColor 0x00448070`).
    pub darkness: [f32; 4],
    /// VS c30 `fogScale.x` = 1 / fog distance (`setFogScale 0x00448100`; the setter uploads
    /// nothing when the distance is not positive, so the previous value stays).
    pub fog_scale: f32,
    /// VS c31 `fogGradientTranslation.x` = (pitch - 80) / -30
    /// (`setFogGradientTranslation 0x00448090`).
    pub fog_gradient_translation: f32,
    /// PS c0 `skyColor1` (`setSkyAndFogColors 0x00449040`).
    pub sky_color1: [f32; 4],
    /// PS c1 `skyColor2`.
    pub sky_color2: [f32; 4],
    /// PS c2 `fogColor`.
    pub fog_color: [f32; 4],
}

/// CubeShader constants that the original sets per draw (or per small group of draws).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawUniforms {
    /// World matrix; `setTransforms` derives c27 (world, 3 rows), c24 (world·view, 3 rows)
    /// and c20 (world·view·projection, 4 rows) from it and the pass's view/projection.
    pub world: D3dMatrix,
    /// VS c32 `materialColor` (`setMaterialColor 0x00448280`); `a` scales the directional
    /// light (the daylight factor for the world).
    pub material: [f32; 4],
    /// PS c3 `alpha` (`setAlpha 0x00447fb0`).
    pub alpha: f32,
    /// VS c39 `white` (`setWhite 0x00449090`): flash toward white.
    pub white: f32,
    /// PS c4 `shininess` (`setShininess 0x00448fe0`).
    pub shininess: f32,
    /// Index into [`FrameCommands::light_sets`] (VS c0..c19).
    pub light_set: usize,
}

/// A vertex for fixed-function draws (`D3DFVF_XYZ|DIFFUSE[|TEX1]`), stride 16 or 24.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedVertex {
    /// Position (already in the space the pass's fixed-function transforms expect).
    pub position: [f32; 3],
    /// `D3DCOLOR` ARGB.
    pub color_argb: u32,
    /// Texture coordinate (ignored for `fvf = 0x42`).
    pub uv: [f32; 2],
}

/// The 8-byte world vertex used by `DrawPrimitiveUP` for the sky fan (`cube::CubeVertex`
/// ctor `0x00466650`): `UBYTE4` position with the face index in `w`, then a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldVertex {
    /// x, y, z, face (0..5).
    pub position: [u8; 4],
    /// `D3DCOLOR` ARGB.
    pub color_argb: u32,
}

/// Primitive topology of an immediate draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    /// `D3DPT_TRIANGLELIST` (4).
    TriangleList,
    /// `D3DPT_TRIANGLESTRIP` (5).
    TriangleStrip,
    /// `D3DPT_TRIANGLEFAN` (6). wgpu has no fans; the backend expands them.
    TriangleFan,
}

/// Which of a chunk buffer's two index buffers a draw uses (`cube::ChunkBuffer`, vtable
/// `0x006ffdb0`: `+8` and `+0xc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkIndexBuffer {
    /// `ChunkBuffer+8`, primitive count at `+0x14`: solid blocks.
    Opaque,
    /// `ChunkBuffer+0xc`, primitive count at `+0x18`: water surfaces.
    Water,
}

/// Opaque handle of a voxel model (`cube::Model`, drawn by `Model::draw 0x004e6df0`);
/// allocated by whoever owns the model meshes.
pub type ModelRef = u32;

/// Opaque handle of a texture the backend knows (sun, shadow blob, GUI textures).
pub type TextureRef = u32;

/// What a draw puts on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// `DrawPrimitiveUP` with world vertices (the sky fan).
    WorldImmediate {
        /// Topology.
        topology: Topology,
        /// Vertices.
        vertices: Vec<WorldVertex>,
    },
    /// `DrawPrimitiveUP` through the fixed-function pipeline.
    FixedImmediate {
        /// Topology.
        topology: Topology,
        /// Vertices.
        vertices: Vec<FixedVertex>,
        /// Stage-0 texture, `None` for untextured draws.
        texture: Option<TextureRef>,
        /// The fixed-function world, view and projection transforms (`SetTransform`).
        transforms: [D3dMatrix; 3],
    },
    /// One chunk buffer (`DrawIndexedPrimitive(TRIANGLELIST, 0, 0, vertexCount, 0,
    /// primitiveCount)`).
    Chunk {
        /// Index into [`crate::passes::RenderInputs::chunks`].
        chunk: usize,
        /// Index into that chunk's buffers.
        buffer: usize,
        /// Which index buffer.
        index_buffer: ChunkIndexBuffer,
        /// `ChunkBuffer+0x10`.
        vertex_count: u32,
        /// `ChunkBuffer+0x14` or `+0x18`.
        primitive_count: u32,
    },
    /// A voxel model (`Model::draw 0x004e6df0`).
    Model {
        /// Which model.
        model: ModelRef,
    },
    /// The map view drawn by `cube::WorldMap::draw 0x005fc1b0` (full-screen map or
    /// minimap); parameters as the original passes them, content produced by the map
    /// module (it sets its own states: ZENABLE 1, fog scale 1e6/1e9, zero point lights,
    /// fixed light, and toggles ZWRITE for the creature heads).
    Map(MapView),
    /// A batch of the GUI stream, passed through unchanged.
    Gui(Vec<GuiCommand>),
}

/// Arguments of one call of `cube::WorldMap::draw 0x005fc1b0` (`this` = the `WorldMap` at
/// `GameController+0x800d44`, `ret 0x28`). It draws the voxel map as CubeShader models
/// (zone models and landscape tiles cached by the landscape thread `0x00469590`, border
/// posts, mission icons, creature heads); the map module turns this into draws.
#[derive(Clone, Debug, PartialEq)]
pub struct MapView {
    /// p2: centre of the view, world fixed-point.
    pub center: [i64; 3],
    /// p3, p4: screen pixel of the map centre.
    pub center_pixel: [i32; 2],
    /// p5, p6: viewport width and height.
    pub viewport: [i32; 2],
    /// p7: rotation in degrees (x = tilt, y unused, z = yaw).
    pub rotation: [f32; 3],
    /// p8: zoom (`< 0.35` selects the landscape-tile "far" mode).
    pub zoom: f32,
    /// p9: frame time in ms (`GameController+0x8006e8`), advances the fades.
    pub frame_ms: i32,
    /// p10: radius in tiles.
    pub radius: i32,
    /// p11: full map screen (placeholders, mission icons) rather than the minimap.
    pub full: bool,
}

/// One entry of the GUI command stream produced by the plasma engine port (`cw-ui`),
/// between `D3D9Engine::beginFrame 0x00688b60` and `endFrame 0x0068a250`.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum GuiCommand {
    /// A tessellated drawing or widget quad (shaders 06 + 07).
    Draw(GuiDraw),
    /// Start rendering into a render surface (`D3D9RenderSurface` slot 2, `0x0068d290`).
    BeginSurface {
        /// Surface id.
        surface: u32,
        /// Clear before drawing (`clear` slot 9 `0x0068c6e0`).
        clear_argb: Option<u32>,
    },
    /// Return to the previous target (slot 3, `0x0068c770`).
    EndSurface,
    /// Draw a surface as a screen quad (slot 5 `0x0068c7c0`, shaders 08 + 09, linear).
    Blit {
        /// Source surface.
        surface: u32,
        /// Vertex colour (premultiplied), `[r, g, b, a]`.
        color: [f32; 4],
        /// Destination rectangle in target pixels: x, y, width, height (drawQuad 0x00689f30).
        rect: [f32; 4],
        /// The fraction of the source surface that holds the content (uv rectangle
        /// `[0, src_scale]²`): the `scale` argument of `pushSurface` 0x0064edb0 / drawCopy
        /// (slot 5), below 1 for a blur rendered downscaled (0x00632f40: `8 / r` when
        /// `r > 8`). 1 otherwise.
        src_scale: f32,
    },
    /// Separable blur (slot 6 `0x0068cad0`): shader 14 when `horizontal`, else 13;
    /// `blurScale = 0.25 * radius / width` for both directions; blending off; linear/clamp.
    Blur {
        /// Source surface.
        surface: u32,
        /// Axis.
        horizontal: bool,
        /// Radius argument.
        radius: f32,
        /// Destination rectangle in target pixels: x, y, width, height (drawQuad 0x00689f30).
        rect: [f32; 4],
        /// The fraction of the source surface read (see `Blit::src_scale`).
        src_scale: f32,
    },
    /// N×N box downsample (slot 7 `0x0068cf10`, shaders 10–12, point sampling).
    /// `texel_size` is the source texel size, which the port uploads correctly (the
    /// original's `GetPixelShaderConstantF` bug is fixed, see `docs/TODO.md`).
    Downsample {
        /// Source surface.
        surface: u32,
        /// 2, 3 or 4.
        factor: u8,
        /// `(1/width, 1/height)` of the source.
        texel_size: [f32; 2],
        /// Destination rectangle in target pixels: x, y, width, height (drawQuad 0x00689f30).
        rect: [f32; 4],
    },
    /// Draws nothing: a game widget's slot-1 point in `Node::render` 0x00632910, where the
    /// widget's `0x004758c0` item models go (`crate::passes::GuiModel::anchor`). Marks come in
    /// pairs per widget: before its `drawText` calls and after them. `cw_render::passes`
    /// splits the stream at them and strips them; the executor ignores any left over.
    WidgetMark(GuiAnchor),
}

/// A point of the GUI stream a [`GuiCommand::WidgetMark`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GuiAnchor {
    /// The widget (`cw_ui::widget::WidgetId`).
    pub widget: u32,
    /// After the widget's slot-1 `drawText` calls (else before them).
    pub after_texts: bool,
}

/// Shader 06/07 constants and resources of one GUI draw.
#[derive(Clone, Debug, PartialEq)]
pub struct GuiDraw {
    /// Range in the GUI vertex stream (48-byte vertices: FLOAT2 position, FLOAT4 colour,
    /// 3 × FLOAT2 uv).
    pub vertices: std::ops::Range<u32>,
    /// Range in the GUI index stream (32-bit indices, relative to `vertices.start`, the
    /// `BaseVertexIndex` of `DrawIndexedPrimitive`).
    pub indices: std::ops::Range<u32>,
    /// VS c0 `Proj`.
    pub proj: D3dMatrix,
    /// VS c4 `WorldView`.
    pub world_view: D3dMatrix,
    /// VS b0 `widgetDeformationEnabled`.
    pub deformation_enabled: bool,
    /// VS c8 `widgetBindMatrix`.
    pub widget_bind_matrix: D3dMatrix,
    /// VS c12 `inverseWidgetBindMatrix`.
    pub inverse_widget_bind_matrix: D3dMatrix,
    /// VS c16 `MaskMatrix` (2 rows).
    pub mask_matrix: [[f32; 4]; 2],
    /// VS c18 `TextureMatrix` (2 rows).
    pub texture_matrix: [[f32; 4]; 2],
    /// VS c20 `NormalMatrix` (2 rows).
    pub normal_matrix: [[f32; 4]; 2],
    /// VS c22 `widgetBindPos`.
    pub widget_bind_pos: [f32; 2],
    /// VS c23 `widgetBindSize`.
    pub widget_bind_size: [f32; 2],
    /// VS c24 `deformedWidgetPos`.
    pub deformed_widget_pos: [f32; 2],
    /// VS c25 `deformedWidgetSize`.
    pub deformed_widget_size: [f32; 2],
    /// VS c26 `aaOffset`.
    pub aa_offset: f32,
    /// VS c27 `baseColor`.
    pub base_color: [f32; 4],
    /// PS b0 `textureEnabled`.
    pub texture_enabled: bool,
    /// PS c0 `filter`: 0 none, 1 lerp to mask, 2 multiply, 3 mask alpha (picked by
    /// `0x00688e70` from the mask flags).
    pub filter: u8,
    /// PS c1 `textureOpacity`.
    pub texture_opacity: f32,
    /// PS c2 `textureBrightness`.
    pub texture_brightness: f32,
    /// PS c3 `textureContrast`.
    pub texture_contrast: f32,
    /// PS c4 `textureSaturation`.
    pub texture_saturation: f32,
    /// s1 texture.
    pub texture: Option<TextureRef>,
    /// s0 mask texture.
    pub mask: Option<TextureRef>,
    /// s0 mask as a render surface (`Engine+0x4c` after `popSurface` 0x006509f0: the
    /// children of a clipping node, `Node::render` 0x00632910). Takes precedence over
    /// [`GuiDraw::mask`]; sampled with LINEAR/CLAMP at the `MaskMatrix` coordinates.
    pub mask_surface: Option<u32>,
    /// `BLENDOP_SUBTRACT, ONE/ONE` instead of the normal GUI blend (caret, inverted rect).
    pub subtract: bool,
}

/// One draw call with the state that was current when the original issued it.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// Original address of the call (or of the helper that issues it).
    pub origin: u32,
    /// Fixed state.
    pub state: PipelineState,
    /// Index into [`FrameCommands::uniforms`] (the CubeShader per-pass constants current at
    /// the draw; ignored by GUI and fixed-function draws).
    pub uniforms: usize,
    /// Per-draw constants.
    pub draw: DrawUniforms,
    /// What is drawn.
    pub geometry: Geometry,
}

/// What a pass draws. The order of passes in a frame is the original's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassKind {
    /// `Clear` of colour and depth at the top of the frame (`0x004ac312..0x004ac36f`).
    ClearFrame,
    /// Sky gradient fan (`0x004ad684..0x004adede`).
    Sky,
    /// Full-screen map (`0x004adede..0x004ae279`).
    MapScreen,
    /// Night stars (`0x004ae641..0x004aea83`).
    Stars,
    /// Sun sprite (`0x004aea83..0x004aed63`).
    Sun,
    /// Clouds (`0x004b1489..0x004b1a6b`).
    Clouds,
    /// Terrain chunks and their static props (`0x004b1a6b..0x004b21ac`).
    Terrain,
    /// Zone objects, dropped items and props (`0x004b21ac..0x004b35d4`).
    Objects,
    /// Creatures, their equipment, after-images, projectiles (`0x004b35d4..0x004b8bc7`).
    Creatures,
    /// Blob shadows, fixed function (`0x004b8bc7..0x004b9993`).
    Shadows,
    /// Weapon/trail ribbons, fixed function (`0x004b9993..0x004ba13e`) (tentative name).
    Ribbons,
    /// Water surfaces, far to near (`0x004ba13e..0x004ba432`).
    Water,
    /// The far list after the water, far to near, faded (`0x004ba462..0x004ba5d0`).
    FarObjects,
    /// Ghosted creatures: depth-only then colour (`0x004ba5d0..0x004ba9fc`).
    Ghosts,
    /// Depth clear before the HUD overlays (`0x004baa72..0x004baa93`).
    ClearDepth,
    /// Minimap (`0x004baa95..0x004bb075`).
    Minimap,
    /// The plasma GUI (`Engine::render 0x00650980` at `0x004bb07b`).
    Gui,
    /// The item models the game widgets draw inside the GUI pass (`0x004758c0` from their
    /// slot-1 bodies: inventory cells, equipment boxes, the item preview, the panels' icons).
    /// Drawn at the widget's point of the GUI traversal ([`GuiCommand::WidgetMark`]), with
    /// the rest of the GUI stream in a following [`PassKind::Gui`] pass without clear (see
    /// [`crate::passes::GuiModel::anchor`]).
    GuiModels,
    /// 3D HUD models drawn over the GUI (`0x004bb080..0x004bbb1a`).
    HudModels,
}

/// A run of draws into one target. Consecutive passes on the same target with no clear are
/// one wgpu render pass with several pipelines; the split follows the original's phases.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    /// What the pass draws.
    pub kind: PassKind,
    /// Original address range `[start, end)` inside `GameController::render`.
    pub range: (u32, u32),
    /// Render target.
    pub target: RenderTarget,
    /// Clear at the start of the pass.
    pub clear: Option<Clear>,
    /// State at the start of the pass (the state a draw-less pass leaves is this one).
    pub state: PipelineState,
    /// Draws in order.
    pub draws: Vec<Draw>,
}

/// Everything one frame asks of the device, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameCommands {
    /// Passes in the original's order.
    pub passes: Vec<Pass>,
    /// CubeShader per-pass constant blocks referenced by [`Draw::uniforms`].
    pub uniforms: Vec<CubeUniforms>,
    /// Point-light sets referenced by [`DrawUniforms::light_set`]. Set 0 is all zero.
    pub light_sets: Vec<LightSet>,
    /// Frustum planes computed this frame (`0x004aee2e..0x004af353`), to be stored at
    /// `GameController+0x1000fa4` and used by the *next* frame's light-emitter test
    /// (the original tests emitters before it recomputes the planes).
    pub frustum_planes: Option<[[f32; 4]; 6]>,
    /// Projection saved to `GameController+0x800a90` (`0x004aed96`).
    pub projection: Option<D3dMatrix>,
}

impl FrameCommands {
    /// The kinds of the passes, in order.
    pub fn pass_kinds(&self) -> Vec<PassKind> {
        self.passes.iter().map(|p| p.kind).collect()
    }

    /// The first pass of a kind.
    pub fn pass(&self, kind: PassKind) -> Option<&Pass> {
        self.passes.iter().find(|p| p.kind == kind)
    }

    /// The exact vertex-shader constant registers c20..c39 that `setTransforms`,
    /// `setMaterialColor`, `setLight`, … leave for a draw (registers c0..c19 are the point
    /// lights, see [`LightSet::upload`]). Index 0 of the result is c20.
    pub fn vs_constants(&self, draw: &Draw) -> [[f32; 4]; 20] {
        let u = &self.uniforms[draw.uniforms];
        let d = &draw.draw;
        let wv = mat_mul(&d.world, &u.view);
        let wvp = mat_mul(&wv, &u.projection);
        let mut c = [[0.0f32; 4]; 20];
        #[allow(clippy::needless_range_loop)]
        // setTransforms 0x004482a0 uploads transposed matrices (columns as rows):
        // c20..c23 = (W·V·P)^T, c24..c26 = (W·V)^T rows 0..2, c27..c29 = W^T rows 0..2.
        for i in 0..4 {
            c[i] = column(&wvp, i);
        }
        for i in 0..3 {
            c[4 + i] = column(&wv, i);
            c[7 + i] = column(&d.world, i);
        }
        c[10] = [u.fog_scale, 0.0, 0.0, 0.0];
        c[11] = [u.fog_gradient_translation, 0.0, 0.0, 0.0];
        c[12] = d.material;
        c[13] = [u.light_direction[0], u.light_direction[1], u.light_direction[2], 0.0];
        c[14] = u.light_front;
        c[15] = u.light_back;
        c[16] = [u.camera_position[0], u.camera_position[1], u.camera_position[2], 0.0];
        c[17] = u.darkness;
        c[18] = u.ambient;
        c[19] = [d.white, 0.0, 0.0, 0.0];
        c
    }
}

/// `a * b` for [`D3dMatrix`] (row vectors: apply `a` first).
pub fn mat_mul(a: &D3dMatrix, b: &D3dMatrix) -> D3dMatrix {
    let mut r = [[0.0f32; 4]; 4];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + a[i][3] * b[3][j];
        }
    }
    r
}

fn column(m: &D3dMatrix, j: usize) -> [f32; 4] {
    [m[0][j], m[1][j], m[2][j], m[3][j]]
}
