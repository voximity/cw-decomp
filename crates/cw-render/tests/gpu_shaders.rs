//! Validation of the WGSL rewrites (naga) and of the pipelines (wgpu, headless when an
//! adapter exists). No window is opened.

use std::collections::HashMap;
use std::mem::size_of;

use cw_render::gpu::pipelines::{
    BlurPsConstants, DownsamplePsConstants, GuiPsConstants, GuiVsConstants, PipelineKind,
    Pipelines, ScreenVsConstants, WorldPsConstants, WorldVsConstants,
};
use cw_render::gpu::wgsl::{Stage, SHADERS};

fn parse(index: usize) -> naga::Module {
    let info = &SHADERS[index];
    naga::front::wgsl::parse_str(info.source).unwrap_or_else(|e| {
        panic!("shader {:02} ({}) does not parse:\n{}", info.index, info.name, e.emit_to_string(info.source))
    })
}

fn validate(module: &naga::Module) -> Result<naga::valid::ModuleInfo, String> {
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(module)
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn every_shader_parses_and_validates() {
    assert_eq!(SHADERS.len(), 15);
    for (i, info) in SHADERS.iter().enumerate() {
        assert_eq!(info.index as usize, i);
        let module = parse(i);
        if let Err(e) = validate(&module) {
            panic!("shader {:02} ({}) fails validation: {e}", info.index, info.name);
        }
        let (name, stage) = match info.stage {
            Stage::Vertex => ("vs_main", naga::ShaderStage::Vertex),
            Stage::Fragment => ("fs_main", naga::ShaderStage::Fragment),
        };
        assert!(
            module.entry_points.iter().any(|ep| ep.name == name && ep.stage == stage),
            "shader {:02} lacks entry point {name}",
            info.index
        );
    }
}

fn struct_size(module: &naga::Module, name: &str) -> u32 {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
            (Some(n), naga::TypeInner::Struct { span, .. }) if n == name => Some(*span),
            _ => None,
        })
        .unwrap_or_else(|| panic!("struct {name} not found"))
}

/// The Rust register structs must have exactly the WGSL uniform layout.
#[test]
fn constant_structs_match_wgsl_layout() {
    let cases: [(usize, &str, usize); 8] = [
        (0, "WorldVsConstants", size_of::<WorldVsConstants>()),
        (1, "WorldPsConstants", size_of::<WorldPsConstants>()),
        (6, "GuiVsConstants", size_of::<GuiVsConstants>()),
        (7, "GuiPsConstants", size_of::<GuiPsConstants>()),
        (8, "ScreenVsConstants", size_of::<ScreenVsConstants>()),
        (10, "DownsamplePsConstants", size_of::<DownsamplePsConstants>()),
        (13, "BlurPsConstants", size_of::<BlurPsConstants>()),
        (14, "BlurPsConstants", size_of::<BlurPsConstants>()),
    ];
    for (index, name, rust) in cases {
        let module = parse(index);
        assert_eq!(struct_size(&module, name) as usize, rust, "{name} in shader {index:02}");
    }
    // One vec4 per D3D9 register (plus the bool register padded to a vec4).
    assert_eq!(size_of::<WorldVsConstants>(), 40 * 16);
    assert_eq!(size_of::<WorldPsConstants>(), 5 * 16);
    assert_eq!(size_of::<GuiVsConstants>(), 28 * 16 + 16);
    assert_eq!(size_of::<GuiPsConstants>(), 5 * 16 + 16);
}

fn io_locations(module: &naga::Module, stage: naga::ShaderStage, outputs: bool) -> HashMap<u32, naga::Handle<naga::Type>> {
    let ep = module.entry_points.iter().find(|e| e.stage == stage).unwrap();
    let mut map = HashMap::new();
    let mut add = |binding: &Option<naga::Binding>, ty: naga::Handle<naga::Type>| {
        if let Some(naga::Binding::Location { location, .. }) = binding {
            map.insert(*location, ty);
        }
    };
    let mut visit = |ty: naga::Handle<naga::Type>, binding: &Option<naga::Binding>| {
        match &module.types[ty].inner {
            naga::TypeInner::Struct { members, .. } => {
                for m in members {
                    add(&m.binding, m.ty);
                }
            }
            _ => add(binding, ty),
        }
    };
    if outputs {
        let r = ep.function.result.as_ref().unwrap();
        visit(r.ty, &r.binding);
    } else {
        for a in &ep.function.arguments {
            visit(a.ty, &a.binding);
        }
    }
    map
}

/// Every fragment input of each pipeline is written by its vertex shader with the same type.
#[test]
fn vertex_outputs_feed_pixel_inputs() {
    for kind in PipelineKind::ALL {
        let (vs, ps) = kind.shader_indices();
        let vm = parse(vs as usize);
        let pm = parse(ps as usize);
        let outs = io_locations(&vm, naga::ShaderStage::Vertex, true);
        let ins = io_locations(&pm, naga::ShaderStage::Fragment, false);
        for (loc, ty) in ins {
            let out = outs.get(&loc).unwrap_or_else(|| panic!("{kind:?}: VS {vs:02} does not write location {loc}"));
            assert_eq!(vm.types[*out].inner, pm.types[ty].inner, "{kind:?}: location {loc} type");
        }
    }
}

/// Create every pipeline and run the blur/downsample chain on a headless adapter.
/// Skipped (passes) when no adapter is available.
#[test]
fn pipelines_build_on_headless_adapter() {
    use cw_render::gpu::{block_on, device, targets, textures};

    let Some((_instance, adapter, device, queue)) = device::headless() else {
        eprintln!("no GPU adapter available; skipping the wgpu pipeline test");
        return;
    };
    eprintln!("adapter: {:?}", adapter.get_info());
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let pipelines = Pipelines::new(&device, format);
    for kind in PipelineKind::ALL {
        let _ = pipelines.get(kind);
    }
    let samplers = textures::Samplers::new(&device, &queue);
    let tex = textures::GpuTexture::from_rgba8(&device, &queue, "t", 2, 2, &[255; 16]);
    let _sampler = textures::create_sampler(&device, textures::TextureSampling::default());
    let _ = tex;

    let src = targets::RenderSurface::new(&device, format, 64, 32);
    let scratch = targets::RenderSurface::new(&device, format, 64, 32);
    let dst = targets::RenderSurface::new(&device, format, 64, 32);
    let mut small = targets::RenderSurface::new(&device, format, 16, 8);
    assert!(!small.resize(&device, 16, 8));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _clear = src.begin_pass(&mut encoder, Some(wgpu::Color { r: 1.0, g: 0.5, b: 0.25, a: 1.0 }));
    }
    targets::blur(&device, &mut encoder, &pipelines, &samplers, &src, &scratch, &dst, 4.0);
    for n in [2, 4] {
        assert!(small.resize(&device, 64 / n, 32 / n));
        targets::downsample(&device, &mut encoder, &pipelines, &samplers, &src, &small, n);
    }
    queue.submit([encoder.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if let Some(e) = block_on(scope.pop()) {
        panic!("wgpu validation error: {e}");
    }
}

/// The 2x2 downsample (shader 10 with the fixed textureScale) averages each source block.
/// Skipped when no adapter is available.
#[test]
fn downsample2_averages_blocks() {
    use cw_render::gpu::{device, targets, textures};

    let Some((_instance, _adapter, device, queue)) = device::headless() else {
        eprintln!("no GPU adapter available; skipping");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipelines = Pipelines::new(&device, format);
    let samplers = textures::Samplers::new(&device, &queue);
    let (w, h) = (4u32, 2u32);
    let src = targets::RenderSurface::new(&device, format, w, h);
    // Left block: red channel 0,40,80,120 -> 60. Right block: green 200,100,0,100 -> 100.
    let mut texels = vec![0u8; (w * h * 4) as usize];
    let set = |t: &mut Vec<u8>, x: u32, y: u32, rgba: [u8; 4]| {
        let i = ((y * w + x) * 4) as usize;
        t[i..i + 4].copy_from_slice(&rgba);
    };
    set(&mut texels, 0, 0, [0, 0, 0, 255]);
    set(&mut texels, 1, 0, [40, 0, 0, 255]);
    set(&mut texels, 0, 1, [80, 0, 0, 255]);
    set(&mut texels, 1, 1, [120, 0, 0, 255]);
    set(&mut texels, 2, 0, [0, 200, 0, 255]);
    set(&mut texels, 3, 0, [0, 100, 0, 255]);
    set(&mut texels, 2, 1, [0, 0, 0, 255]);
    set(&mut texels, 3, 1, [0, 100, 0, 255]);
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &src.color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &texels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let dst = targets::RenderSurface::new(&device, format, 2, 1);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    targets::downsample(&device, &mut encoder, &pipelines, &samplers, &src, &dst, 2);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &dst.color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(1) },
        },
        wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = readback.slice(..).get_mapped_range().unwrap().to_vec();
    let px = &data[..8];
    let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 1;
    assert!(close(px[0], 60) && close(px[1], 0) && close(px[3], 255), "left {:?}", &px[..4]);
    assert!(close(px[4], 0) && close(px[5], 100) && close(px[7], 255), "right {:?}", &px[4..]);
}
