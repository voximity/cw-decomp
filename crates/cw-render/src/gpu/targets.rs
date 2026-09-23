//! `plasma::D3D9RenderSurface` (vtable 0x00722618, size 0x4c): offscreen colour + depth
//! targets and the screen-quad draws that read them.
//!
//! | Slot | Original | Here |
//! |---|---|---|
//! | 1 | resize 0x0068d350 (RT texture A8R8G8B8 + D24S8 depth, only on a size change) | [`RenderSurface::resize`] |
//! | 2/3 | begin 0x0068d290 / end 0x0068c770 (save, set, restore the RT) | [`RenderSurface::begin_pass`] (a wgpu render pass) |
//! | 5 | drawCopy 0x0068c7c0 (VS 08 + PS 09, s0 LINEAR) | [`SurfaceFilter::Copy`] |
//! | 6 | drawBlurred 0x0068cad0 (VS 08 + PS 14/13, LINEAR/CLAMP, blending off) | [`SurfaceFilter::BlurHorizontal`] / [`SurfaceFilter::BlurVertical`], [`blur`] |
//! | 7 | drawScaled 0x0068cf10 (VS 08 + PS 09/10/11/12, s0 POINT) | [`SurfaceFilter::Downsample`] |
//! | 8/9 | clearRect 0x0068c0c0 / clear 0x0068c6e0 | the `clear` argument of `begin_pass` |
//!
//! The quad is drawQuad 0x00689f30: four 48-byte vertices (x,y), (x+w,y), (x+w,y+h), (x,y+h)
//! with the uv in TEXCOORD2 and one colour, indices 0,1,2,2,3,0, projected by the GUI
//! orthographic matrix ([`gui_projection`]) of the target's size.
//!
//! Simplified: the originals' own uv arithmetic (drawCopy's scale-and-snap, drawBlurred's
//! `+0.5` texel-centre term in the uv origin, a D3D9 half-texel habit) is left to the
//! caller, who passes the uv rectangle directly; the +0.5 is dropped with the -0.135 px
//! offset (docs/TODO.md "Renderer quirks").

use wgpu::util::DeviceExt;

use super::pipelines::{
    gui_projection, BlurPsConstants, DownsamplePsConstants, GuiVertex, PipelineKind, Pipelines,
    ScreenVsConstants, DEPTH_FORMAT,
};
use super::textures::Samplers;

/// Index list of drawQuad 0x00689f30 (INDEX32 IB at D3D9Engine+0x2c8).
pub const QUAD_INDICES: [u32; 6] = [0, 1, 2, 2, 3, 0];

/// The four vertices drawQuad 0x00689f30 writes into the dynamic VB at D3D9Engine+0x2c4.
pub fn quad_vertices(
    pos: [f32; 2],
    size: [f32; 2],
    uv: [f32; 2],
    uv_size: [f32; 2],
    color: [f32; 4],
) -> [GuiVertex; 4] {
    let v = |x: f32, y: f32, u: f32, w: f32| GuiVertex {
        position: [x + 0.0, y + 0.0],
        color,
        normal0: [0.0; 2],
        normal1: [0.0; 2],
        uv: [u, w],
    };
    [
        v(pos[0], pos[1], uv[0], uv[1]),
        v(pos[0] + size[0], pos[1], uv[0] + uv_size[0], uv[1]),
        v(pos[0] + size[0], pos[1] + size[1], uv[0] + uv_size[0], uv[1] + uv_size[1]),
        v(pos[0], pos[1] + size[1], uv[0], uv[1] + uv_size[1]),
    ]
}

/// An offscreen render target (`plasma::D3D9RenderSurface`): colour (+0x38 texture,
/// +0x3c surface) and D24S8 depth (+0x40), size at +0x2c/+0x30.
#[derive(Debug)]
pub struct RenderSurface {
    /// Colour texture (RENDER_ATTACHMENT, TEXTURE_BINDING, COPY_SRC/DST).
    pub color: wgpu::Texture,
    /// Colour view.
    pub color_view: wgpu::TextureView,
    /// Depth-stencil texture ([`DEPTH_FORMAT`]).
    pub depth: wgpu::Texture,
    /// Depth view.
    pub depth_view: wgpu::TextureView,
    /// Width (+0x2c).
    pub width: u32,
    /// Height (+0x30).
    pub height: u32,
    format: wgpu::TextureFormat,
}

fn create_targets(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Texture, wgpu::TextureView) {
    let size = wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 };
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("render surface"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let depth = create_depth_texture(device, size.width, size.height);
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    (color, color_view, depth, depth_view)
}

/// A [`DEPTH_FORMAT`] attachment of the given size.
pub fn create_depth_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    create_depth_texture_ms(device, width, height, 1)
}

/// A [`DEPTH_FORMAT`] attachment with `samples` samples per pixel (the back buffer's depth
/// with the anti-aliasing option).
pub fn create_depth_texture_ms(device: &wgpu::Device, width: u32, height: u32, samples: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: samples.max(1),
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

impl RenderSurface {
    /// Create a surface of `width` x `height` with colour `format` (use the pipelines'
    /// [`Pipelines::color_format`] so every pipeline can render into it).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> Self {
        let (color, color_view, depth, depth_view) = create_targets(device, format, width, height);
        Self { color, color_view, depth, depth_view, width, height, format }
    }

    /// resize 0x0068d350: recreate the textures only when the size changes. Returns whether
    /// they were recreated (the original returns 1 then, 0 otherwise).
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        if width == self.width && height == self.height {
            return false;
        }
        *self = Self::new(device, self.format, width, height);
        true
    }

    /// begin 0x0068d290: a render pass into this surface, optionally cleared
    /// (clear 0x0068c6e0) to `clear`; depth is cleared to 1 whenever colour is.
    pub fn begin_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        clear: Option<wgpu::Color>,
    ) -> wgpu::RenderPass<'e> {
        begin_pass(encoder, &self.color_view, &self.depth_view, clear)
    }
}

/// Begin a render pass on any colour + depth pair (the back buffer or a surface).
pub fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    let (load, depth_load) = match clear {
        Some(c) => (wgpu::LoadOp::Clear(c), wgpu::LoadOp::Clear(1.0)),
        None => (wgpu::LoadOp::Load, wgpu::LoadOp::Load),
    };
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("plasma pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations { load: depth_load, store: wgpu::StoreOp::Store }),
            stencil_ops: Some(wgpu::Operations {
                load: if clear.is_some() { wgpu::LoadOp::Clear(0) } else { wgpu::LoadOp::Load },
                store: wgpu::StoreOp::Store,
            }),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Which render-surface draw to make.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceFilter {
    /// drawCopy 0x0068c7c0 / drawScaled filter 1: PS 09 with LINEAR (copy) sampling.
    Copy,
    /// drawScaled filter 1 with POINT sampling.
    CopyPoint,
    /// drawScaled filters 2, 3, 4: N x N box downsample (PS 10/11/12), POINT sampling.
    Downsample(u32),
    /// drawBlurred flag true: PS 14 with `blurScale = 0.25 * radius / width`.
    BlurHorizontal {
        /// Blur radius in source pixels.
        radius: f32,
    },
    /// drawBlurred flag false: PS 13.
    BlurVertical {
        /// Blur radius in source pixels.
        radius: f32,
    },
}

/// Destination rectangle and source uv rectangle of a surface draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadRect {
    /// Top-left corner in target pixels.
    pub pos: [f32; 2],
    /// Size in target pixels.
    pub size: [f32; 2],
    /// Source uv origin.
    pub uv: [f32; 2],
    /// Source uv extent.
    pub uv_size: [f32; 2],
    /// Vertex colour (drawBlurred uses white).
    pub color: [f32; 4],
}

impl QuadRect {
    /// The whole source onto the whole target, white.
    pub fn full(target_width: u32, target_height: u32) -> Self {
        Self {
            pos: [0.0, 0.0],
            size: [target_width as f32, target_height as f32],
            uv: [0.0, 0.0],
            uv_size: [1.0, 1.0],
            color: [1.0; 4],
        }
    }
}

/// Record one screen-quad draw of `source` into `pass`, whose target is
/// `target_width` x `target_height`.
///
/// Performance note for a future optimiser: this creates a vertex, index and two uniform
/// buffers plus two bind groups per call, as the original rewrote its dynamic VB and
/// constants per draw; a per-frame ring buffer would avoid the allocations.
#[allow(clippy::too_many_arguments)]
pub fn draw_surface(
    device: &wgpu::Device,
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &Pipelines,
    samplers: &Samplers,
    source: &RenderSurface,
    target_width: u32,
    target_height: u32,
    rect: QuadRect,
    filter: SurfaceFilter,
) {
    let (kind, sampler, ps_constant): (PipelineKind, &wgpu::Sampler, [f32; 4]) = match filter {
        SurfaceFilter::Copy => (PipelineKind::Blit, &samplers.linear_clamp, [0.0; 4]),
        SurfaceFilter::CopyPoint => (PipelineKind::Blit, &samplers.point_clamp, [0.0; 4]),
        SurfaceFilter::Downsample(n) => {
            let kind = match n {
                2 => PipelineKind::Downsample2,
                3 => PipelineKind::Downsample3,
                4 => PipelineKind::Downsample4,
                _ => PipelineKind::Blit,
            };
            // 0x0068cf10: (1/width, 1/height, 0, 0) of the source surface; uploaded here
            // (the original called GetPixelShaderConstantF by mistake).
            let scale = DownsamplePsConstants {
                texture_scale: [1.0 / source.width as f32, 1.0 / source.height as f32, 0.0, 0.0],
            };
            (kind, &samplers.point_clamp, scale.texture_scale)
        }
        SurfaceFilter::BlurHorizontal { radius } | SurfaceFilter::BlurVertical { radius } => {
            let kind = if matches!(filter, SurfaceFilter::BlurHorizontal { .. }) {
                PipelineKind::BlurHorizontal
            } else {
                PipelineKind::BlurVertical
            };
            // 0x0068cad0: (radius * 0.25) / width, for both directions.
            let c = BlurPsConstants { blur_scale: [(radius * 0.25) / source.width as f32, 0.0, 0.0, 0.0] };
            (kind, &samplers.linear_clamp, c.blur_scale)
        }
    };

    let vertices = quad_vertices(rect.pos, rect.size, rect.uv, rect.uv_size, rect.color);
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("surface quad"),
        contents: bytemuck::cast_slice(&vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("surface quad indices"),
        contents: bytemuck::cast_slice(&QUAD_INDICES),
        usage: wgpu::BufferUsages::INDEX,
    });
    let vs = ScreenVsConstants { proj: gui_projection(target_width, target_height) };
    let vs_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("screen vs constants"),
        contents: bytemuck::bytes_of(&vs),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let ps_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("screen ps constants"),
        contents: bytemuck::bytes_of(&ps_constant),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("screen constants"),
        layout: &pipelines.layouts.screen_uniforms,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: vs_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: ps_buf.as_entire_binding() },
        ],
    });
    let texture = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("screen texture"),
        layout: &pipelines.layouts.screen_texture,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&source.color_view),
            },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    });

    pass.set_pipeline(pipelines.get(kind));
    pass.set_bind_group(0, &uniforms, &[0, 0]);
    pass.set_bind_group(1, &texture, &[]);
    pass.set_vertex_buffer(0, vb.slice(..));
    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
    pass.draw_indexed(0..6, 0, 0..1);
}

/// A separable blur of `source` into `target` through `scratch` (horizontal pass into
/// `scratch`, vertical pass into `target`), each a full-surface drawBlurred 0x0068cad0.
/// `scratch` must be the size of `source`. Both passes overwrite (blending is off).
#[allow(clippy::too_many_arguments)]
pub fn blur(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &Pipelines,
    samplers: &Samplers,
    source: &RenderSurface,
    scratch: &RenderSurface,
    target: &RenderSurface,
    radius: f32,
) {
    {
        let mut pass = scratch.begin_pass(encoder, Some(wgpu::Color::TRANSPARENT));
        draw_surface(
            device,
            &mut pass,
            pipelines,
            samplers,
            source,
            scratch.width,
            scratch.height,
            QuadRect::full(scratch.width, scratch.height),
            SurfaceFilter::BlurHorizontal { radius },
        );
    }
    let mut pass = target.begin_pass(encoder, Some(wgpu::Color::TRANSPARENT));
    draw_surface(
        device,
        &mut pass,
        pipelines,
        samplers,
        scratch,
        target.width,
        target.height,
        QuadRect::full(target.width, target.height),
        SurfaceFilter::BlurVertical { radius },
    );
}

/// An N x N downsample of `source` into `target` (drawScaled 0x0068cf10, filter N).
/// `target` should be `source / n` in each dimension.
pub fn downsample(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &Pipelines,
    samplers: &Samplers,
    source: &RenderSurface,
    target: &RenderSurface,
    n: u32,
) {
    let mut pass = target.begin_pass(encoder, Some(wgpu::Color::TRANSPARENT));
    draw_surface(
        device,
        &mut pass,
        pipelines,
        samplers,
        source,
        target.width,
        target.height,
        QuadRect::full(target.width, target.height),
        SurfaceFilter::Downsample(n),
    );
}
