//! `start.plx` through `render_gui` and the wgpu executor, headless, read back. Checks the
//! logo's clipping (the blue gradient only inside the letters, `Node::render` 0x00632910)
//! and, with `CW_SHOT=<path.ppm>`, writes the image. Skipped without `CW_GAME_DIR` or a GPU.

use cw_render::frame::*;
use cw_render::gpu::exec::{Executor, FrameTarget, Resources};
use cw_render::gpu::targets::RenderSurface;
use cw_render::gpu::textures::{GpuTexture, TextureAddress, TextureFilter, TextureSampling};
use cw_render::gpu::{block_on, device};
use cw_ui::font::{DiskFonts, FontEngine, ENGINE_FONT_SEARCH_PATH};
use cw_ui::loader::{load, LoadOptions};
use cw_ui::render::{render_gui, scene_texture_ref, GlyphAtlas, GuiFonts, GuiView, GLYPH_ATLAS_TEXTURE};

#[test]
fn start_screen_pixels() {
    let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
        eprintln!("CW_GAME_DIR not set; skipped");
        return;
    };
    let Some((_i, _a, device, queue)) = device::headless() else {
        eprintln!("no GPU; skipped");
        return;
    };
    let (w, h) = (1280u32, 720u32);
    let doc = cw_formats::plx::parse(&std::fs::read(dir.join("start.plx")).unwrap()).unwrap();
    let (mut gui, scene) = load(&doc, LoadOptions::default());
    gui.viewport = glam::IVec2::new(w as i32, h as i32);
    let mut engine = FontEngine::new();
    engine.add_search_path(ENGINE_FONT_SEARCH_PATH);
    let files = DiskFonts { base: dir.clone() };
    let mut atlas = GlyphAtlas::default();
    let view = GuiView { width: w, height: h, ..Default::default() };
    let f = render_gui(&gui, &[&scene], GuiFonts { engine: &mut engine, files: &files }, &mut atlas, &view);

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let back = RenderSurface::new(&device, format, w, h);
    let mut exec = Executor::new(&device, &queue, format);
    let mut res = Resources::new(&device, format);
    let lin = TextureSampling { min: TextureFilter::Linear, mag: TextureFilter::Linear, address_u: TextureAddress::Wrap, address_v: TextureAddress::Wrap };
    for (ti, t) in scene.textures.iter().enumerate() {
        let n = (t.width.max(0) * t.height.max(0)) as usize;
        let px = if t.pixels.len() >= n * 4 {
            t.pixels[..n * 4].to_vec()
        } else if t.pixels.len() >= n * 3 && n > 0 {
            cw_render::gpu::textures::rgb8_to_rgba8(&t.pixels[..n * 3])
        } else {
            continue;
        };
        let g = GpuTexture::from_rgba8(&device, &queue, "plx", t.width as u32, t.height as u32, &px);
        res.add_texture(&device, scene_texture_ref(0, ti), g, lin);
    }
    if !atlas.is_empty() {
        let g = GpuTexture::from_rgba8(&device, &queue, "atlas", atlas.width, atlas.height, &atlas.rgba);
        res.add_texture(&device, GLYPH_ATLAS_TEXTURE, g, lin);
    }
    for s in f.surfaces() {
        res.ensure_surface(&device, s, w, h);
    }
    res.set_gui_stream(f.vertices.clone(), f.indices.clone());
    let state = PipelineState {
        program: Program::GUI,
        depth: DepthState { test: false, write: false, func: CompareFunc::Always },
        blend: BlendState::ALPHA,
        cull: Cull::None,
        color_write: 0xf,
        sampler0: SamplerState::DEFAULT,
    };
    let draw = Draw {
        origin: 0x004bb07b,
        state,
        uniforms: 0,
        draw: DrawUniforms { world: IDENTITY, material: [1.0; 4], alpha: 1.0, white: 0.0, shininess: 0.0, light_set: 0 },
        geometry: Geometry::Gui(f.commands.clone()),
    };
    let frame = FrameCommands {
        passes: vec![Pass {
            kind: PassKind::Gui,
            range: (0x004bb07b, 0x004bb080),
            target: RenderTarget::Backbuffer,
            clear: Some(Clear { color_argb: Some(0xff00_0000), depth: Some(1.0), stencil: Some(0) }),
            state,
            draws: vec![draw],
        }],
        uniforms: vec![],
        light_sets: vec![],
        frustum_planes: None,
        projection: None,
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let stats = exec.encode(&device, &queue, &mut enc, FrameTarget::new(&back.color_view, &back.depth_view, w, h), &frame, &mut res);
    let row = w * 4;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &back.color, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    if let Some(e) = block_on(scope.pop()) {
        panic!("wgpu validation error: {e}");
    }
    let data = buf.slice(..).get_mapped_range().unwrap().to_vec();
    eprintln!("{stats:?}");
    assert_eq!(stats.skipped, 0);
    if let Some(p) = std::env::var_os("CW_SHOT") {
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for px in data.chunks(4) {
            out.extend_from_slice(&px[..3]);
        }
        std::fs::write(p, out).unwrap();
    }
}
