//! The executor on a synthetic frame (one chunk slab, one model, particles, stars, sun,
//! decals, ribbons, a GUI quad drawn into a render surface, the blur/downsample chain, HUD
//! models, the map hook) on the headless adapter. Skipped when no adapter exists.

use cw_render::frame::*;
use cw_render::gpu::exec::{Executor, FrameTarget, Resources, chunk_buffer_inputs};
use cw_render::gpu::pipelines::GuiVertex;
use cw_render::gpu::targets::RenderSurface;
use cw_render::gpu::textures::{GpuTexture, TextureAddress, TextureFilter, TextureSampling};
use cw_render::gpu::{block_on, device};
use cw_render::map::{MapModels, MapScene, MapState, MapTiles};
use cw_render::mesh::{ChunkBuild, ChunkMesh, cube_vertex};
use cw_render::passes::*;

const B: i64 = 65536;

fn translation(t: [f32; 3]) -> D3dMatrix {
    let mut m = IDENTITY;
    m[3][0] = t[0];
    m[3][1] = t[1];
    m[3][2] = t[2];
    m
}

/// A chunk slab with one top quad (opaque) and one water quad.
fn chunk_build() -> ChunkBuild {
    let c = [0.5, 0.8, 0.3, 1.0];
    let top = |z: i32| {
        [
            cube_vertex([0, 0, z], [0, 0, 1], c),
            cube_vertex([0, 32, z], [0, 0, 1], c),
            cube_vertex([32, 32, z], [0, 0, 1], c),
            cube_vertex([32, 0, z], [0, 0, 1], c),
        ]
    };
    let mut vertices = top(10).to_vec();
    vertices.extend(top(12));
    ChunkBuild {
        cx: 0,
        cy: 0,
        buffers: vec![ChunkMesh { base_z: 0, vertices, indices: vec![0, 1, 2, 0, 2, 3], indices2: vec![4, 5, 6, 4, 6, 7] }],
        min_z: 10,
        max_z: 12,
        bounds_min: [0, 0, 10 * B],
        bounds_max: [33 * B, 33 * B, 13 * B],
        vertex_count: 8,
        props: vec![],
    }
}

fn camera() -> CameraInputs {
    let mut view = IDENTITY;
    // Look along +y: x right, z up.
    view[1] = [0.0, 0.0, 1.0, 0.0];
    view[2] = [0.0, 1.0, 0.0, 0.0];
    view[3] = [-16.0, -20.0, -2.0, 1.0];
    CameraInputs {
        position: [16 * B, 2 * B, 20 * B],
        render_offset: [0, 0],
        view,
        sky_view: view,
        pitch: 80.0,
        yaw: 0.0,
        narrow_fov: false,
        fog_distance: 100.0,
    }
}

fn gui_ortho(w: f32, h: f32) -> D3dMatrix {
    [[2.0 / w, 0.0, 0.0, 0.0], [0.0, -2.0 / h, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [-1.0, 1.0, 0.0, 1.0]]
}

fn gui_draw(w: f32, h: f32) -> GuiDraw {
    GuiDraw {
        vertices: 0..4,
        indices: 0..6,
        proj: gui_ortho(w, h),
        world_view: IDENTITY,
        deformation_enabled: false,
        widget_bind_matrix: IDENTITY,
        inverse_widget_bind_matrix: IDENTITY,
        mask_matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
        texture_matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
        normal_matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
        widget_bind_pos: [0.0; 2],
        widget_bind_size: [w, h],
        deformed_widget_pos: [0.0; 2],
        deformed_widget_size: [w, h],
        aa_offset: 0.0,
        base_color: [1.0; 4],
        texture_enabled: true,
        filter: 0,
        texture_opacity: 1.0,
        texture_brightness: 0.0,
        texture_contrast: 1.0,
        texture_saturation: 1.0,
        texture: Some(10),
        mask: None,
        mask_surface: None,
        subtract: false,
    }
}

fn gui_quad(w: f32, h: f32) -> Vec<GuiVertex> {
    let v = |x: f32, y: f32, u: f32, t: f32| GuiVertex { position: [x, y], color: [1.0, 0.5, 0.25, 1.0], normal0: [0.0; 2], normal1: [0.0; 2], uv: [u, t] };
    vec![v(4.0, 4.0, 0.0, 0.0), v(w - 4.0, 4.0, 1.0, 0.0), v(w - 4.0, h - 4.0, 1.0, 1.0), v(4.0, h - 4.0, 0.0, 1.0)]
}

#[test]
fn executor_draws_a_synthetic_frame() {
    let Some((_instance, adapter, device, queue)) = device::headless() else {
        eprintln!("no GPU adapter available; skipping the executor test");
        return;
    };
    eprintln!("adapter: {:?}", adapter.get_info());
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (w, h) = (128u32, 96u32);
    let back = RenderSurface::new(&device, format, w, h);

    let mut exec = Executor::new(&device, &queue, format);
    let mut res = Resources::new(&device, format);

    // Chunk slab at ring index 0, a 2x2x2 model, textures, render surfaces.
    let build = chunk_build();
    res.upload_chunk(&device, 0, &build);
    let cub = cw_formats::CubModel { size: [2, 2, 2], voxels: vec![[200, 100, 50]; 8] };
    res.models.insert_cub(&device, 7, &cub, false);
    res.models.insert_cub(&device, 1000, &cub, false); // the particle cube stand-in
    let sampling = TextureSampling {
        min: TextureFilter::Linear,
        mag: TextureFilter::Linear,
        address_u: TextureAddress::Border,
        address_v: TextureAddress::Border,
    };
    for id in [1u32, 2, 10] {
        let t = GpuTexture::from_rgba8(&device, &queue, "t", 2, 2, &[255, 255, 255, 200].repeat(4));
        res.add_texture(&device, id, t, sampling);
    }
    assert!(res.ensure_surface(&device, 1, 64, 32));
    assert!(res.ensure_surface(&device, 3, 64, 32));
    assert!(res.ensure_surface(&device, 2, 32, 16));
    assert!(!res.ensure_surface(&device, 2, 32, 16));
    res.set_gui_stream(gui_quad(64.0, 32.0), vec![0, 1, 2, 2, 3, 0]);

    let gui = vec![
        GuiCommand::BeginSurface { surface: 1, clear_argb: Some(0) },
        GuiCommand::Draw(gui_draw(64.0, 32.0)),
        GuiCommand::EndSurface,
        GuiCommand::BeginSurface { surface: 3, clear_argb: Some(0) },
        GuiCommand::Blur { surface: 1, horizontal: true, radius: 4.0, rect: [0.0, 0.0, 64.0, 32.0], src_scale: 1.0 },
        GuiCommand::EndSurface,
        GuiCommand::BeginSurface { surface: 2, clear_argb: Some(0) },
        GuiCommand::Downsample { surface: 3, factor: 2, texel_size: [1.0 / 64.0, 1.0 / 32.0], rect: [0.0, 0.0, 32.0, 16.0] },
        GuiCommand::EndSurface,
        GuiCommand::Blur { surface: 2, horizontal: false, radius: 4.0, rect: [0.0, 0.0, 64.0, 32.0], src_scale: 1.0 },
        GuiCommand::Blit { surface: 2, color: [1.0; 4], rect: [64.0, 0.0, 32.0, 16.0], src_scale: 1.0 },
    ];

    let world = cw_world::World::new(0);
    let tiles = MapTiles::new();
    let models = MapModels::default();
    let state = MapState::default();
    let map = MapScene { tiles: &tiles, models: &models, world: &world, creatures: &[], player_position: [16 * B, 16 * B, 0], state: &state };

    let cam = camera();
    let mut particle = ModelDraw::new(0x004b_80f5, 1000, translation([16.0, 12.0, 14.0]), [1.0; 4]);
    particle.double_sided = true;
    let mut shiny = ModelDraw::new(0x0047_1cfc, 7, translation([15.0, 12.0, 14.0]), [1.0; 4]);
    shiny.shininess = 1.0;
    let fv = |x: f32, y: f32, u: f32, v: f32| FixedVertex { position: [x, y, 10.5], color_argb: 0xff00_0000, uv: [u, v] };
    let hud = HudModel {
        origin: 0x004b_b6cf,
        screen: [100.0, 40.0],
        rotation: [-120.0, 0.0, 0.0],
        scale: 0.003,
        model: 7,
        model_size: [2, 2, 2],
        depth: 0.0,
        material: [1.0; 4],
    };
    let flat = FlatWorld;
    let inputs = RenderInputs {
        mode: FrameMode::World,
        screen: [w as i32, h as i32],
        white_clear: false,
        time_of_day_ms: 1_000_000, // night: stars
        clock_ms: 0,
        frame_ms: 16,
        camera: cam.clone(),
        camera_block_kind: 0,
        player_position: [16 * B, 16 * B, 12 * B],
        previous_frustum: frustum_planes(&cam.view, &projection([w as i32, h as i32], false)),
        window: ChunkWindow { origin: [0, 0], size: 1 },
        chunks: vec![ChunkInput {
            coords: [0, 0],
            has_buffers: true,
            aabb_min: build.bounds_min,
            aabb_max: build.bounds_max,
            center: [16 * B, 16 * B, 11 * B],
            buffers: chunk_buffer_inputs(&build),
        }],
        chunk_props: vec![vec![ModelDraw::new(0x004b_d160, 7, translation([10.0, 20.0, 11.0]), [1.0; 4])]],
        lights: vec![PointLightSource { position: [16 * B, 16 * B, 12 * B], radius: 10.0, color: [1.0, 0.5, 0.2] }],
        stars: vec![[0.0, 1.0, 0.5, 0.01], [0.3, 1.0, 0.2, 0.01]],
        sun_texture: 1,
        shadow_texture: 2,
        cloud_model: 7,
        cloud_model_size: [2, 2],
        objects: vec![ModelDraw::new(0x004b_2c1b, 7, translation([16.0, 16.0, 11.0]), [1.0; 4])],
        creatures: vec![CreatureInput {
            id: 5,
            position: [16 * B, 18 * B, 15 * B],
            height: 2.0,
            ghost: 0.5,
            parts: vec![ModelDraw::new(0, 7, translation([16.0, 18.0, 15.0]), [1.0; 4])],
            ..Default::default()
        }],
        creature_order: vec![5],
        creature_extras: vec![shiny, particle],
        shadows: vec![[fv(15.0, 15.0, 0.0, 0.0), fv(16.0, 15.0, 1.0, 0.0), fv(16.0, 16.0, 1.0, 1.0), fv(15.0, 16.0, 0.0, 1.0)]],
        ribbons: vec![(0..32)
            .map(|i| FixedVertex { position: [16.0 + (i / 2) as f32 * 0.1, 15.0, 12.0 + (i % 2) as f32], color_argb: 0x80ff_ffff, uv: [0.0; 2] })
            .collect()],
        far_objects: vec![ModelDraw { alpha: 0.5, ..ModelDraw::new(0x004b_d160, 7, translation([20.0, 30.0, 11.0]), [1.0; 4]) }],
        map_pan: [0.0; 3],
        map_zoom: 1.0,
        minimap_markers: vec![],
        quick_wheel: vec![],
        gui: gui.clone(),
        hud_visible: true,
        gui_models: vec![],
        gui_creatures: vec![],
        hud_models: vec![hud],
        map_compass: None,
        map: Some(map),
        world: &flat,
    };
    let frame = build_frame(&inputs);
    assert!(frame.pass(PassKind::Terrain).unwrap().draws.len() >= 2);

    let mut stats = Vec::new();
    // Two frames: the second reuses the pipeline cache and the streams.
    for _ in 0..2 {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let target = FrameTarget::new(&back.color_view, &back.depth_view, w, h);
        stats.push(exec.encode(&device, &queue, &mut encoder, target, &frame, &mut res));
        queue.submit([encoder.finish()]);
    }
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if let Some(e) = block_on(scope.pop()) {
        panic!("wgpu validation error: {e}");
    }
    eprintln!("{stats:?}");
    let s = stats[1];
    assert_eq!(s.skipped, 0, "{s:?}");
    assert_eq!(s.draws, 21, "{s:?}");
    assert!(s.render_passes >= 5, "{s:?}");
    assert_eq!(stats[0].pipelines, stats[1].pipelines);

    // The map screen and the loading screen too.
    for mode in [FrameMode::MapScreen, FrameMode::GuiOnly] {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut inp = RenderInputs { mode, map_compass: Some(hud), ..inputs_clone(&inputs) };
        inp.gui = gui.clone();
        let frame = build_frame(&inp);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let target = FrameTarget::new(&back.color_view, &back.depth_view, w, h);
        let s = exec.encode(&device, &queue, &mut encoder, target, &frame, &mut res);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if let Some(e) = block_on(scope.pop()) {
            panic!("wgpu validation error ({mode:?}): {e}");
        }
        assert_eq!(s.skipped, 0, "{mode:?} {s:?}");
    }

    // A clipping mask (a GUI draw masked by a render surface, Node::render 0x00632910) and
    // the anti-aliasing option: 4 samples resolving into `back`.
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut masked = gui_draw(64.0, 32.0);
    masked.mask_surface = Some(1);
    masked.filter = 1;
    let mut inp = inputs_clone(&inputs);
    inp.gui = vec![
        GuiCommand::BeginSurface { surface: 1, clear_argb: Some(0) },
        GuiCommand::Draw(gui_draw(64.0, 32.0)),
        GuiCommand::EndSurface,
        GuiCommand::Draw(masked),
    ];
    let frame = build_frame(&inp);
    let samples = 4;
    exec.set_sample_count(&device, samples);
    let ms = |format| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    let (mc, md) = (ms(format), ms(cw_render::gpu::pipelines::DEPTH_FORMAT));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let target = FrameTarget { color: &mc, depth: &md, width: w, height: h, samples, resolve: Some(&back.color_view) };
    let s = exec.encode(&device, &queue, &mut encoder, target, &frame, &mut res);
    queue.submit([encoder.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if let Some(e) = block_on(scope.pop()) {
        panic!("wgpu validation error (msaa, mask): {e}");
    }
    assert_eq!(s.skipped, 0, "msaa {s:?}");
}

/// `RenderInputs` is not `Clone` (it borrows trait objects); copy the parts the second
/// frames need.
fn inputs_clone<'a>(i: &RenderInputs<'a>) -> RenderInputs<'a> {
    RenderInputs {
        mode: i.mode,
        screen: i.screen,
        white_clear: i.white_clear,
        time_of_day_ms: i.time_of_day_ms,
        clock_ms: i.clock_ms,
        frame_ms: i.frame_ms,
        camera: i.camera.clone(),
        camera_block_kind: i.camera_block_kind,
        player_position: i.player_position,
        previous_frustum: i.previous_frustum,
        window: i.window,
        chunks: i.chunks.clone(),
        chunk_props: i.chunk_props.clone(),
        lights: i.lights.clone(),
        stars: i.stars.clone(),
        sun_texture: i.sun_texture,
        shadow_texture: i.shadow_texture,
        cloud_model: i.cloud_model,
        cloud_model_size: i.cloud_model_size,
        objects: i.objects.clone(),
        creatures: i.creatures.clone(),
        creature_order: i.creature_order.clone(),
        creature_extras: i.creature_extras.clone(),
        shadows: i.shadows.clone(),
        ribbons: i.ribbons.clone(),
        far_objects: i.far_objects.clone(),
        map_pan: i.map_pan,
        map_zoom: i.map_zoom,
        minimap_markers: i.minimap_markers.clone(),
        quick_wheel: i.quick_wheel.clone(),
        gui: i.gui.clone(),
        hud_visible: i.hud_visible,
        gui_models: i.gui_models.clone(),
        gui_creatures: i.gui_creatures.clone(),
        hud_models: i.hud_models.clone(),
        map_compass: i.map_compass,
        map: i.map.as_ref().map(|m| MapScene {
            tiles: m.tiles,
            models: m.models,
            world: m.world,
            creatures: m.creatures,
            player_position: m.player_position,
            state: m.state,
        }),
        world: i.world,
    }
}
