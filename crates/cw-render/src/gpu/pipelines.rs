//! Vertex layouts, constant-register structs, bind group layouts and render pipelines.
//!
//! The original binds shaders and render states piecemeal on one IDirect3DDevice9; wgpu bakes
//! them into pipelines, so every (vertex shader, pixel shader, blend/depth/cull) combination
//! the original reaches is one [`PipelineKind`]. States and their sources:
//!
//! | Kind | VS+PS | Blend | Depth test / write | Cull | Original |
//! |---|---|---|---|---|---|
//! | `World` | 00+01 | SRCALPHA/INVSRCALPHA | LessEqual / on | CCW (back) | `0x00447d10`, RS 19=5, 20=6 at `0x004aedd5` |
//! | `WorldGhostDepth` | 00+01 | colour writes off | LessEqual / on | CCW | COLORWRITEENABLE=0 at `0x004ba890` |
//! | `Sky` | 00+02 | SRCALPHA/INVSRCALPHA | off / off | none | `0x00447d90` at `0x004adeb1`, ZENABLE off |
//! | `Water` | 00+03 | SRCALPHA/INVSRCALPHA | LessEqual / on | none | `0x00447dd0` at `0x004ba157` |
//! | `UnusedPs04` | 00+04 | as `World` | as `World` | CCW | never bound (dead) |
//! | `Clouds` | 00+05 | SRCALPHA/INVSRCALPHA | LessEqual / on | CCW | `0x00447d50` at `0x004b1489` |
//! | `Gui` | 06+07 | colour SRCALPHA/INVSRCALPHA, alpha ONE/INVSRCALPHA | off | none | beginFrame `0x00688b60` |
//! | `GuiSubtract` | 06+07 | colour SUBTRACT ONE/ONE, alpha ONE/INVSRCALPHA | off | none | `0x006897c0`, `0x00689950` |
//! | `Blit` | 08+09 | as `Gui` | off | none | drawCopy `0x0068c7c0`, drawScaled `0x0068cf10` filter 1 |
//! | `Downsample2/3/4` | 08+10/11/12 | as `Gui` | off | none | drawScaled `0x0068cf10` filters 2-4 |
//! | `BlurVertical` / `BlurHorizontal` | 08+13 / 08+14 | none (ALPHABLENDENABLE 0) | off | none | drawBlurred `0x0068cad0` |
//!
//! Depth function: D3D9's default ZFUNC is LESSEQUAL and the ghosted-creature colour pass
//! draws the same triangles again after the depth-only pass, which only passes with
//! LessEqual. The static scan also saw ZFUNC=LESS in `0x004758c0`/`0x00476660`/`0x004d50a0`
//! and GREATER in `0x0068ab70` (with Z off in the GUI); those are left to `passes.rs`.
//!
//! Culling: D3DCULL_CCW culls counter-clockwise screen-space triangles, i.e. front faces
//! are clockwise: `FrontFace::Cw` + `Face::Back`. D3D9 and wgpu share the y-up NDC and the
//! [0, 1] depth range, so the original's matrices can be uploaded unchanged.
//!
//! Every pipeline carries a depth-stencil state with [`DEPTH_FORMAT`] (the original's D24S8)
//! so each can be used in any pass: the back buffer and every render surface have a depth
//! attachment, as in the original (`D3D9RenderSurface::resize` 0x0068d350 creates one).
//!
//! Uniform bindings use dynamic offsets: the original changes constant registers between
//! draws (world matrix per chunk, alpha per creature); one buffer with per-draw offsets
//! (aligned to `min_uniform_buffer_offset_alignment`) replaces SetVertexShaderConstantF.

use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};

use super::wgsl;

/// Depth-stencil format of every pass: D3DFMT_D24S8 (`0x4b` in 0x0068d350).
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

// ---------------------------------------------------------------------------------------
// Vertex formats
// ---------------------------------------------------------------------------------------

/// `cube::CubeVertex`: the 8-byte world vertex of chunks and models (declaration made in
/// `cube::CubeShader::init` 0x00447e10; written by `buildChunkMesh` 0x0049d910 and
/// `cube::Model::buildMesh` 0x004e7870).
///
/// - `position`: D3DDECLTYPE_UBYTE4 POSITION0 @0: block x, y, z inside the mesh and the face
///   index (0:+X 1:-X 2:+Y 3:-Y 4:+Z 5:-Z) in w. Fetched as `Uint8x4` (unnormalised).
/// - `color`: D3DDECLTYPE_D3DCOLOR COLOR0 @4, stored exactly as the original's DWORD
///   `0xAARRGGBB` in little-endian memory order, i.e. bytes `[B, G, R, A]`. Fetched as
///   `Unorm8x4` and swizzled `.zyxw` in vertex shader 00. A = light level (light/255).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct WorldVertex {
    /// x, y, z, face index.
    pub position: [u8; 4],
    /// D3DCOLOR bytes in memory order: B, G, R, A.
    pub color: [u8; 4],
}

impl WorldVertex {
    /// Build from a position/face and a D3DCOLOR DWORD (`0xAARRGGBB`).
    pub fn new(x: u8, y: u8, z: u8, face: u8, d3dcolor: u32) -> Self {
        Self { position: [x, y, z, face], color: d3dcolor.to_le_bytes() }
    }
}

/// The GUI vertex of `plasma::D3D9Drawing::update` 0x0068b7f0 and of drawQuad 0x00689f30
/// (declaration made in `D3D9Engine::init` 0x0068a350), 48 bytes:
/// FLOAT2 POSITION0 @0, FLOAT4 COLOR0 @8, FLOAT2 TEXCOORD0 @24, TEXCOORD1 @32, TEXCOORD2 @40.
/// TEXCOORD0/1 are the anti-aliasing edge normals, TEXCOORD2 the texture coordinate.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GuiVertex {
    /// POSITION0 (D3D9 expands it to (x, y, 0, 1)).
    pub position: [f32; 2],
    /// COLOR0, RGBA floats.
    pub color: [f32; 4],
    /// TEXCOORD0: first edge normal (fringe vertices), else 0.
    pub normal0: [f32; 2],
    /// TEXCOORD1: second edge normal (fringe vertices), else 0.
    pub normal1: [f32; 2],
    /// TEXCOORD2: texture coordinate.
    pub uv: [f32; 2],
}

const WORLD_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint8x4, offset: 0, shader_location: 0 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 4, shader_location: 1 },
];

const GUI_ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 2 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 32, shader_location: 3 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 40, shader_location: 4 },
];

/// The screen quad reads POSITION0, COLOR0 and TEXCOORD2 of the same 48-byte vertex.
const SCREEN_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 8, shader_location: 1 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 40, shader_location: 4 },
];

/// Layout of [`WorldVertex`] (stride 8).
pub const WORLD_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 8,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &WORLD_ATTRIBUTES,
};

/// Layout of [`GuiVertex`] for shader 06 (stride 48, all five attributes).
pub const GUI_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 48,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &GUI_ATTRIBUTES,
};

/// Layout of the render-surface quad for shader 08 ([`GuiVertex`], position/colour/uv only).
pub const SCREEN_VERTEX_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 48,
    step_mode: wgpu::VertexStepMode::Vertex,
    attributes: &SCREEN_ATTRIBUTES,
};

// ---------------------------------------------------------------------------------------
// Constant registers. One `[f32; 4]` per D3D9 register, in register order, so the CPU side
// writes the same values the original passed to Set*ShaderConstantF. A matrix is its
// registers in order (register i = column i of the WGSL matrix; the shader computes v * M).
// ---------------------------------------------------------------------------------------

/// Vertex shader 00 constants c0..c39 (`WorldVsConstants` in `vs00_world.wgsl`). Set by
/// the `cube::CubeShader` uniform setters 0x00448010..0x00449090.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WorldVsConstants {
    /// `pointLightPositions` c0..c9: xyz position, w radius (light on when w > 0).
    pub point_light_positions: [[f32; 4]; 10],
    /// `pointLightColors` c10..c19 (xyz).
    pub point_light_colors: [[f32; 4]; 10],
    /// `worldViewProjMatrix` c20..c23.
    pub world_view_proj_matrix: [[f32; 4]; 4],
    /// `worldViewMatrix` c24..c26.
    pub world_view_matrix: [[f32; 4]; 3],
    /// `worldMatrix` c27..c29.
    pub world_matrix: [[f32; 4]; 3],
    /// `fogScale` c30 (x).
    pub fog_scale: [f32; 4],
    /// `fogGradientTranslation` c31 (x).
    pub fog_gradient_translation: [f32; 4],
    /// `materialColor` c32.
    pub material_color: [f32; 4],
    /// `lightDirection` c33 (xyz).
    pub light_direction: [f32; 4],
    /// `lightFrontColor` c34.
    pub light_front_color: [f32; 4],
    /// `lightBackColor` c35.
    pub light_back_color: [f32; 4],
    /// `cameraPosition` c36 (xyz).
    pub camera_position: [f32; 4],
    /// `darknessColor` c37.
    pub darkness_color: [f32; 4],
    /// `ambientColor` c38.
    pub ambient_color: [f32; 4],
    /// `white` c39 (x).
    pub white: [f32; 4],
}

/// Pixel shader 01..05 constants c0..c4 (`WorldPsConstants` in `world_common.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WorldPsConstants {
    /// `skyColor1` c0.
    pub sky_color1: [f32; 4],
    /// `skyColor2` c1.
    pub sky_color2: [f32; 4],
    /// `fogColor` c2.
    pub fog_color: [f32; 4],
    /// `alpha` c3 (x), set by 0x00447fb0.
    pub alpha: [f32; 4],
    /// `shininess` c4 (x).
    pub shininess: [f32; 4],
}

/// Vertex shader 06 constants c0..c27 and b0 (`GuiVsConstants` in `vs06_gui.wgsl`), set by
/// bindWidgetShader 0x00688e70, uploadMatrices 0x0068a800 and D3D9Drawing::draw 0x0068ab70.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GuiVsConstants {
    /// `Proj` c0..c3.
    pub proj: [[f32; 4]; 4],
    /// `WorldView` c4..c7.
    pub world_view: [[f32; 4]; 4],
    /// `widgetBindMatrix` c8..c11.
    pub widget_bind_matrix: [[f32; 4]; 4],
    /// `inverseWidgetBindMatrix` c12..c15.
    pub inverse_widget_bind_matrix: [[f32; 4]; 4],
    /// `MaskMatrix` c16..c17.
    pub mask_matrix: [[f32; 4]; 2],
    /// `TextureMatrix` c18..c19.
    pub texture_matrix: [[f32; 4]; 2],
    /// `NormalMatrix` c20..c21.
    pub normal_matrix: [[f32; 4]; 2],
    /// `widgetBindPos` c22 (xy).
    pub widget_bind_pos: [f32; 4],
    /// `widgetBindSize` c23 (xy).
    pub widget_bind_size: [f32; 4],
    /// `deformedWidgetPos` c24 (xy).
    pub deformed_widget_pos: [f32; 4],
    /// `deformedWidgetSize` c25 (xy).
    pub deformed_widget_size: [f32; 4],
    /// `aaOffset` c26 (x).
    pub aa_offset: [f32; 4],
    /// `baseColor` c27.
    pub base_color: [f32; 4],
    /// `widgetDeformationEnabled` b0 in x (0 or 1); y, z, w padding.
    pub widget_deformation_enabled: [u32; 4],
}

/// Pixel shader 07 constants c0..c4 and b0 (`GuiPsConstants` in `ps07_gui.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GuiPsConstants {
    /// `filter` c0 (x): 0 none, 1 lerp to mask, 2 multiply, 3 alpha mask (int as float).
    pub filter: [f32; 4],
    /// `textureOpacity` c1 (x).
    pub texture_opacity: [f32; 4],
    /// `textureBrightness` c2 (x).
    pub texture_brightness: [f32; 4],
    /// `textureContrast` c3 (x). Also the colour scale when no texture is bound.
    pub texture_contrast: [f32; 4],
    /// `textureSaturation` c4 (x).
    pub texture_saturation: [f32; 4],
    /// `textureEnabled` b0 in x (0 or 1).
    pub texture_enabled: [u32; 4],
}

/// Vertex shader 08 constants c0..c3 (`ScreenVsConstants` in `vs08_screen.wgsl`), uploaded
/// by drawQuad 0x00689f30 from D3D9Engine+0x264.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ScreenVsConstants {
    /// `Proj` c0..c3.
    pub proj: [[f32; 4]; 4],
}

/// Pixel shader 10..12 constant c0 (`DownsamplePsConstants`): `textureScale` in xy, the
/// source texel size (1/width, 1/height) as 0x0068cf10 computes it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct DownsamplePsConstants {
    /// `textureScale` c0 (xy).
    pub texture_scale: [f32; 4],
}

/// Pixel shader 13/14 constant c0 (`BlurPsConstants`): `blurScale` in x
/// (`0.25 * radius / width`, 0x0068cad0).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct BlurPsConstants {
    /// `blurScale` c0 (x).
    pub blur_scale: [f32; 4],
}

/// The orthographic GUI projection of beginFrame 0x00688b60 (D3D9Engine+0x264) for a
/// `width` x `height` target, as the four registers drawQuad 0x00689f30 uploads (the
/// D3DX row-major matrix transposed by 0x004490f0): x' = 2x/w - 1, y' = 1 - 2y/h,
/// z' = 1e-7 (1 - z).
pub fn gui_projection(width: u32, height: u32) -> [[f32; 4]; 4] {
    let w = width as f32;
    let h = height as f32;
    // _33 = 0xb3d6bf93 (+0x28c) and _43 = 0x33d6bf93 (+0x29c), about -/+1e-7.
    let z = f32::from_bits(0x33d6_bf93);
    [
        [2.0 / (w - 0.0), 0.0, 0.0, (w + 0.0) / (0.0 - w)],
        [0.0, 2.0 / (0.0 - h), 0.0, (h + 0.0) / (h - 0.0)],
        [0.0, 0.0, -z, z],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

// ---------------------------------------------------------------------------------------
// Bind group layouts
// ---------------------------------------------------------------------------------------

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages, size: usize) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The bind group layouts shared by the pipelines.
///
/// - world, group 0: binding 0 [`WorldVsConstants`] (vertex), binding 1 [`WorldPsConstants`]
///   (fragment), both with dynamic offsets.
/// - GUI, group 0: binding 0 [`GuiVsConstants`], binding 1 [`GuiPsConstants`] (dynamic).
///   Group 1: binding 0/1 mask texture/sampler (s0), binding 2/3 texture/sampler (s1).
/// - screen, group 0: binding 0 [`ScreenVsConstants`], binding 1 a 16-byte pixel constant
///   ([`DownsamplePsConstants`] or [`BlurPsConstants`]; unused by the blit) (dynamic).
///   Group 1: binding 0/1 source texture/sampler (s0).
#[derive(Clone, Debug)]
pub struct BindGroupLayouts {
    /// World constants (group 0 of the world pipelines).
    pub world_uniforms: wgpu::BindGroupLayout,
    /// GUI constants (group 0 of the GUI pipelines).
    pub gui_uniforms: wgpu::BindGroupLayout,
    /// GUI mask + texture (group 1 of the GUI pipelines).
    pub gui_textures: wgpu::BindGroupLayout,
    /// Screen-quad constants (group 0 of the render-surface pipelines).
    pub screen_uniforms: wgpu::BindGroupLayout,
    /// Screen-quad source texture (group 1 of the render-surface pipelines).
    pub screen_texture: wgpu::BindGroupLayout,
}

impl BindGroupLayouts {
    /// Create the five layouts.
    pub fn new(device: &wgpu::Device) -> Self {
        use std::mem::size_of;
        let v = wgpu::ShaderStages::VERTEX;
        let f = wgpu::ShaderStages::FRAGMENT;
        let world_uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world constants"),
            entries: &[
                uniform_entry(0, v, size_of::<WorldVsConstants>()),
                uniform_entry(1, f, size_of::<WorldPsConstants>()),
            ],
        });
        let gui_uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui constants"),
            entries: &[
                uniform_entry(0, v, size_of::<GuiVsConstants>()),
                uniform_entry(1, f, size_of::<GuiPsConstants>()),
            ],
        });
        let gui_textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui textures"),
            entries: &[texture_entry(0), sampler_entry(1), texture_entry(2), sampler_entry(3)],
        });
        let screen_uniforms = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen constants"),
            entries: &[
                uniform_entry(0, v, size_of::<ScreenVsConstants>()),
                uniform_entry(1, f, size_of::<BlurPsConstants>()),
            ],
        });
        let screen_texture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("screen texture"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        Self { world_uniforms, gui_uniforms, gui_textures, screen_uniforms, screen_texture }
    }
}

// ---------------------------------------------------------------------------------------
// Pipelines
// ---------------------------------------------------------------------------------------

/// Every pipeline the original's state combinations need (see the module table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineKind {
    /// 00+01: terrain, creatures, items, models (bind default 0x00447d10).
    World,
    /// 00+01 with colour writes off: the ghosted-creature depth pre-pass (0x004ba890).
    WorldGhostDepth,
    /// 00+02: sky background fan, depth off (0x00447d90).
    Sky,
    /// 00+03: water, culling off, depth write on (0x00447dd0).
    Water,
    /// 00+04: the dead pixel shader, kept so its translation is exercised.
    UnusedPs04,
    /// 00+05: clouds (0x00447d50).
    Clouds,
    /// 06+07: GUI widgets and shapes (0x00688e70, 0x0068ab70).
    Gui,
    /// 06+07 with BLENDOP SUBTRACT, ONE/ONE (0x006897c0 caret, 0x00689950 inverted rect).
    GuiSubtract,
    /// 08+09: render-surface copy.
    Blit,
    /// 08+10: 2x2 downsample.
    Downsample2,
    /// 08+11: 3x3 downsample.
    Downsample3,
    /// 08+12: 4x4 downsample.
    Downsample4,
    /// 08+13: vertical blur.
    BlurVertical,
    /// 08+14: horizontal blur.
    BlurHorizontal,
}

impl PipelineKind {
    /// All kinds, in declaration order.
    pub const ALL: [PipelineKind; 14] = [
        PipelineKind::World,
        PipelineKind::WorldGhostDepth,
        PipelineKind::Sky,
        PipelineKind::Water,
        PipelineKind::UnusedPs04,
        PipelineKind::Clouds,
        PipelineKind::Gui,
        PipelineKind::GuiSubtract,
        PipelineKind::Blit,
        PipelineKind::Downsample2,
        PipelineKind::Downsample3,
        PipelineKind::Downsample4,
        PipelineKind::BlurVertical,
        PipelineKind::BlurHorizontal,
    ];

    /// (vertex shader index, pixel shader index) of the original.
    pub fn shader_indices(self) -> (u8, u8) {
        use PipelineKind::*;
        match self {
            World | WorldGhostDepth => (0, 1),
            Sky => (0, 2),
            Water => (0, 3),
            UnusedPs04 => (0, 4),
            Clouds => (0, 5),
            Gui | GuiSubtract => (6, 7),
            Blit => (8, 9),
            Downsample2 => (8, 10),
            Downsample3 => (8, 11),
            Downsample4 => (8, 12),
            BlurVertical => (8, 13),
            BlurHorizontal => (8, 14),
        }
    }

    fn family(self) -> Family {
        match self.shader_indices().0 {
            0 => Family::World,
            6 => Family::Gui,
            _ => Family::Screen,
        }
    }

    fn label(self) -> &'static str {
        use PipelineKind::*;
        match self {
            World => "world 00+01",
            WorldGhostDepth => "world ghost depth 00+01",
            Sky => "sky 00+02",
            Water => "water 00+03",
            UnusedPs04 => "unused 00+04",
            Clouds => "clouds 00+05",
            Gui => "gui 06+07",
            GuiSubtract => "gui subtract 06+07",
            Blit => "blit 08+09",
            Downsample2 => "downsample2 08+10",
            Downsample3 => "downsample3 08+11",
            Downsample4 => "downsample4 08+12",
            BlurVertical => "blur v 08+13",
            BlurHorizontal => "blur h 08+14",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    World,
    Gui,
    Screen,
}

/// World blend: ALPHABLENDENABLE with SRCALPHA/INVSRCALPHA and no separate alpha, so the
/// alpha channel uses the same factors (GameController::render, RS 19=5, 20=6).
pub const WORLD_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// GUI blend of beginFrame 0x00688b60: SRCALPHA/INVSRCALPHA, SEPARATEALPHABLENDENABLE with
/// SRCBLENDALPHA ONE, DESTBLENDALPHA INVSRCALPHA. Render surfaces therefore hold
/// premultiplied colour, which the screen shaders un-premultiply.
pub const GUI_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// 0x006897c0/0x00689950: BLENDOP SUBTRACT with SRCBLEND ONE, DESTBLEND ONE (src - dst).
/// They do not touch the separate alpha states, which stay ONE/INVSRCALPHA/ADD.
pub const GUI_SUBTRACT_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Subtract,
    },
    alpha: GUI_BLEND.alpha,
};

/// Every render pipeline, created once for a colour target format.
#[derive(Debug)]
pub struct Pipelines {
    /// The layouts the bind groups must be created with.
    pub layouts: BindGroupLayouts,
    /// Colour format the pipelines were built for (non-sRGB).
    pub color_format: wgpu::TextureFormat,
    /// Samples per pixel of the targets these pipelines draw into.
    pub samples: u32,
    pipelines: Vec<(PipelineKind, wgpu::RenderPipeline)>,
}

/// Compile one WGSL module.
pub fn create_shader_module(device: &wgpu::Device, info: &wgsl::ShaderInfo) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(info.name),
        source: wgpu::ShaderSource::Wgsl(info.source.into()),
    })
}

impl Pipelines {
    /// Build all [`PipelineKind`]s for `color_format` (must be a non-sRGB format to keep the
    /// original's gamma-space output) and [`DEPTH_FORMAT`].
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        Self::with_samples(device, color_format, 1, BindGroupLayouts::new(device))
    }

    /// The same pipelines for a target with `samples` samples per pixel (the anti-aliasing
    /// option, `D3DPRESENT_PARAMETERS.MultiSampleType` of 0x004c8940), sharing `layouts` so
    /// bind groups work with either set.
    pub fn with_samples(device: &wgpu::Device, color_format: wgpu::TextureFormat, samples: u32, layouts: BindGroupLayouts) -> Self {
        let modules: Vec<wgpu::ShaderModule> =
            wgsl::SHADERS.iter().map(|s| create_shader_module(device, s)).collect();

        let world_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&layouts.world_uniforms)],
            immediate_size: 0,
        });
        let gui_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gui"),
            bind_group_layouts: &[Some(&layouts.gui_uniforms), Some(&layouts.gui_textures)],
            immediate_size: 0,
        });
        let screen_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("screen"),
            bind_group_layouts: &[Some(&layouts.screen_uniforms), Some(&layouts.screen_texture)],
            immediate_size: 0,
        });

        let pipelines = PipelineKind::ALL
            .iter()
            .map(|&kind| {
                let (vs, ps) = kind.shader_indices();
                let family = kind.family();
                let (layout, buffer) = match family {
                    Family::World => (&world_layout, WORLD_VERTEX_LAYOUT),
                    Family::Gui => (&gui_layout, GUI_VERTEX_LAYOUT),
                    Family::Screen => (&screen_layout, SCREEN_VERTEX_LAYOUT),
                };
                let state = PassState::of(kind);
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(kind.label()),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module: &modules[vs as usize],
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[Some(buffer)],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Cw,
                        cull_mode: state.cull,
                        unclipped_depth: false,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        conservative: false,
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(state.depth_write),
                        depth_compare: Some(state.depth_compare),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState { count: samples.max(1), ..Default::default() },
                    fragment: Some(wgpu::FragmentState {
                        module: &modules[ps as usize],
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: color_format,
                            blend: state.blend,
                            write_mask: state.write_mask,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                (kind, pipeline)
            })
            .collect();

        Self { layouts, color_format, samples: samples.max(1), pipelines }
    }

    /// The pipeline of `kind`.
    pub fn get(&self, kind: PipelineKind) -> &wgpu::RenderPipeline {
        &self
            .pipelines
            .iter()
            .find(|(k, _)| *k == kind)
            .expect("every PipelineKind is built in Pipelines::new")
            .1
    }
}

/// Fixed-function state of one [`PipelineKind`] (D3DRS_* equivalents).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassState {
    /// ALPHABLENDENABLE + SRCBLEND/DESTBLEND/BLENDOP (+ separate alpha).
    pub blend: Option<wgpu::BlendState>,
    /// COLORWRITEENABLE.
    pub write_mask: wgpu::ColorWrites,
    /// ZENABLE/ZFUNC (Always when Z is off).
    pub depth_compare: wgpu::CompareFunction,
    /// ZWRITEENABLE.
    pub depth_write: bool,
    /// CULLMODE (None = D3DCULL_NONE, Back = D3DCULL_CCW with clockwise fronts).
    pub cull: Option<wgpu::Face>,
}

impl PassState {
    /// The state table of the module documentation.
    pub fn of(kind: PipelineKind) -> Self {
        use PipelineKind::*;
        let world = PassState {
            blend: Some(WORLD_BLEND),
            write_mask: wgpu::ColorWrites::ALL,
            depth_compare: wgpu::CompareFunction::LessEqual,
            depth_write: true,
            cull: Some(wgpu::Face::Back),
        };
        let overlay = PassState {
            blend: Some(GUI_BLEND),
            write_mask: wgpu::ColorWrites::ALL,
            depth_compare: wgpu::CompareFunction::Always,
            depth_write: false,
            cull: None,
        };
        match kind {
            World | Clouds | UnusedPs04 => world,
            WorldGhostDepth => PassState { write_mask: wgpu::ColorWrites::empty(), ..world },
            // Screen fan with ZENABLE off. Culling is off here although the original leaves
            // CULLMODE at CCW: the fan's winding is not known and it covers the screen anyway.
            Sky => PassState {
                depth_compare: wgpu::CompareFunction::Always,
                depth_write: false,
                cull: None,
                ..world
            },
            Water => PassState { cull: None, ..world },
            Gui | Blit | Downsample2 | Downsample3 | Downsample4 => overlay,
            GuiSubtract => PassState { blend: Some(GUI_SUBTRACT_BLEND), ..overlay },
            BlurVertical | BlurHorizontal => PassState { blend: None, ..overlay },
        }
    }
}
