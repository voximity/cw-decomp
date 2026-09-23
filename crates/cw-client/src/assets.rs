//! The device-side assets of the `GameController` ctor 0x00459c40 and the GUI stream of
//! `Engine::render` 0x00650980, between the controller and the wgpu sink.
//!
//! The controller owns an [`AssetState`] (the font engine and glyph atlas of `Engine+0x34`,
//! the ctor's procedural textures and meshes waiting for the device). Each frame:
//!
//! - `Controller::render` calls [`gui_frame`]: the widget tree through
//!   [`cw_ui::render::render_gui`], returning the command list for `RenderInputs::gui`; then
//!   [`take_upload`] moves the vertex/index streams, the render surfaces the commands need,
//!   the glyph atlas (when it changed) and the pending textures and meshes into a
//!   [`FrameUpload`] that travels in `FrameResources`;
//! - `ExecSink::render` hands the [`FrameUpload`] to the executor with [`upload`].

use std::path::PathBuf;

use cw_render::frame::{GuiCommand, TextureRef};
use cw_render::gpu::device::GpuContext;
use cw_render::gpu::exec::Resources;
use cw_render::gpu::pipelines::GuiVertex;
use cw_render::gpu::textures::{GpuTexture, TextureAddress, TextureFilter, TextureSampling};
use cw_ui::font::{DiskFonts, FontEngine, TextStyle, ENGINE_FONT_SEARCH_PATH};
use cw_ui::render::{render_gui, scene_texture_ref, GlyphAtlas, GuiFonts, GuiView, WidgetText, GLYPH_ATLAS_TEXTURE};

use crate::controller::Controller;
use crate::ui::start_menu::{self, MenuItem};

/// A texture waiting for the device: (handle, width, height, RGBA8, sampling).
pub type PendingTexture = (TextureRef, u32, u32, Vec<u8>, TextureSampling);

/// What the GUI and the ctor's assets keep between frames (a `Controller` field).
#[derive(Default)]
pub struct AssetState {
    /// `Engine+0x34`: the font engine (search path `c:\windows\fonts`, 0x004c8807).
    fonts: Option<FontEngine>,
    /// The game folder the fonts are read from (the original's working directory).
    game_dir: PathBuf,
    /// The glyph atlas and the version last handed to the sink.
    atlas: GlyphAtlas,
    atlas_sent: u64,
    /// This frame's GUI stream.
    vertices: Vec<GuiVertex>,
    indices: Vec<u32>,
    /// Render surfaces of this frame's commands, and the viewport they are sized to.
    surfaces: Vec<u32>,
    viewport: [u32; 2],
    /// The textures to upload once.
    pending_textures: Vec<PendingTexture>,
    /// The `.plx` textures and the ctor's assets were queued.
    textures_collected: bool,
    /// Model meshes to insert before the executor builds them lazily (the cloud tint, the
    /// map's border post).
    pending_meshes: Vec<(u32, cw_render::mesh::ModelMesh)>,
}

/// One frame's device work for the sink ([`upload`]).
#[derive(Default)]
pub struct FrameUpload {
    /// The GUI vertex stream (`Resources::set_gui_stream`).
    pub vertices: Vec<GuiVertex>,
    /// The GUI index stream.
    pub indices: Vec<u32>,
    /// Render surfaces to create at `viewport` (`D3D9RenderSurface::resize` 0x0068d350).
    pub surfaces: Vec<u32>,
    /// The viewport.
    pub viewport: [u32; 2],
    /// The glyph atlas when it changed: (width, height, RGBA8).
    pub atlas: Option<(u32, u32, Vec<u8>)>,
    /// Textures not uploaded yet.
    pub textures: Vec<PendingTexture>,
    /// Meshes not uploaded yet.
    pub meshes: Vec<(u32, cw_render::mesh::ModelMesh)>,
}

/// A `.plx` texture's sampler state (`D3D9Texture::bind` 0x0068bc90 from the format record:
/// filters 0 point / 1 linear / 2 anisotropic, wraps 0 border / 1 wrap / 2 mirror).
fn sampling(t: &cw_ui::loader::Texture) -> TextureSampling {
    let filter = |v: i32| match v {
        0 => TextureFilter::Point,
        2 => TextureFilter::Anisotropic,
        _ => TextureFilter::Linear,
    };
    let wrap = |v: i32| match v {
        0 => TextureAddress::Border,
        2 => TextureAddress::Mirror,
        _ => TextureAddress::Wrap,
    };
    TextureSampling { min: filter(t.min_filter), mag: filter(t.max_filter), address_u: wrap(t.horizontal_wrap), address_v: wrap(t.vertical_wrap) }
}

/// RGBA8 of a `.plx` texture: 4 bytes per texel as stored, or RGB expanded with alpha 255
/// (`D3D9Texture::upload` 0x0068bde0 picks A8R8G8B8 / R8G8B8 by the channel count).
fn texture_rgba(t: &cw_ui::loader::Texture) -> Option<Vec<u8>> {
    let (w, h) = (t.width.max(0) as usize, t.height.max(0) as usize);
    let n = w * h;
    if n == 0 {
        return None;
    }
    if t.pixels.len() >= n * 4 {
        Some(t.pixels[..n * 4].to_vec())
    } else if t.pixels.len() >= n * 3 {
        Some(cw_render::gpu::textures::rgb8_to_rgba8(&t.pixels[..n * 3]))
    } else {
        None
    }
}

/// The start menu's and the system menu's text calls (`StartMenuWidget` 0x00583320,
/// `SystemWidget` 0x00587710): per item an outline pass (white, black stroke of the item's
/// outline width) and a fill pass (the item colour), `resource1.dat`, centred, pixel-snapped.
fn menu_texts(items: &[MenuItem]) -> Vec<WidgetText> {
    let mut out = Vec::new();
    for it in items {
        let text: Vec<u16> = it.text.encode_utf16().collect();
        let style = |stroke: f32| TextStyle {
            size: it.size,
            stroke_radius: stroke,
            spacing: 0.0,
            line_spacing: 0.0,
            wrap_width: 0.0,
            flags: cw_ui::font::align::H_CENTER,
            pixel_snap: true,
        };
        out.push(WidgetText {
            font: "resource1.dat".into(),
            text: text.clone(),
            origin: it.pos,
            style: style(it.outline),
            color: [1.0; 4],
            stroke_color: [0.0, 0.0, 0.0, 1.0],
        });
        out.push(WidgetText {
            font: "resource1.dat".into(),
            text,
            origin: it.pos,
            style: style(0.0),
            color: it.color,
            stroke_color: [0.0; 4],
        });
    }
    out
}

/// `Engine::render` 0x00650980 (called at 0x004bb07b) over the widget tree and the loaded
/// `.plx` scenes: the commands for `RenderInputs::gui`; the streams wait in `s` for
/// [`take_upload`].
///
/// Performance note for a future optimiser: on the start screen this is ~7 ms of the ~11 ms
/// frame (release build, `CW_CLIENT_STATS=1` phase `gui`); in a world ~0.5 ms. The animated
/// `start.plx` shapes change every frame, so their drawings re-tessellate and their fringe
/// buffers miss the cache; the whole stream is also rebuilt and re-uploaded every frame.
pub fn gui_frame(c: &Controller, s: &mut AssetState) -> Vec<GuiCommand> {
    if s.fonts.is_none() {
        let mut e = FontEngine::new();
        e.add_search_path(ENGINE_FONT_SEARCH_PATH);
        s.fonts = Some(e);
        s.game_dir = c.game_dir.clone();
    }
    let ui = &c.ui;
    // The stream is drawn at the back buffer's resolution (`GuiView::scale`): laid out in
    // client units, every transform scaled to device pixels, glyphs rasterised at the size
    // they are shown. Stretching a stream laid out at the client size instead (a HiDPI
    // window) magnifies the glyph bitmaps, which blurs the text.
    let (w, h, scale) = match c.back_buffer {
        [bw, bh] if bw > 0 && bh > 0 => (bw, bh, c.pixel_scale),
        _ => (c.screen[0].max(1) as u32, c.screen[1].max(1) as u32, 1.0),
    };
    let mut view = GuiView { width: w, height: h, scale, time_ms: c.game.engine_time_ms, ..GuiView::default() };
    // The game widgets' slot-1 text (the same computation `GameUi::frame` did this frame).
    if ui.visible(ui.m.start_menu_node) && let Some(wd) = ui.m.start_menu {
        let mut st = ui.start_menu;
        let items = start_menu::start_menu_update(&mut st, ui.local_cursor(Some(wd)), ui.gui.width(wd));
        view.widget_texts.insert(wd, menu_texts(&items));
    }
    if ui.visible(ui.m.system_panel) && let Some(wd) = ui.m.system {
        let mut st = ui.system_menu_state;
        let items = start_menu::system_menu_update(&mut st, ui.local_cursor(Some(wd)), ui.gui.width(wd));
        view.widget_texts.insert(wd, menu_texts(&items));
    }
    crate::ui::present::widget_texts(&c.ui, &c.game, &c.ui_out, &mut view.widget_texts);
    crate::ui::present_panels::widget_rects(&c.ui, &c.ui_out, &mut view.widget_rects);
    view.fill_colors = crate::ui::present_panels::node_colors(&c.ui, &c.ui_out);
    let scenes: Vec<&cw_ui::loader::PlxScene> = c.plx.loaded.iter().map(|l| &l.scene).collect();
    if !s.textures_collected {
        s.textures_collected = true;
        queue_world_assets(s, c);
        for (si, sc) in scenes.iter().enumerate() {
            for (ti, t) in sc.textures.iter().enumerate() {
                if let Some(px) = texture_rgba(t) {
                    s.pending_textures.push((scene_texture_ref(si, ti), t.width as u32, t.height as u32, px, sampling(t)));
                }
            }
        }
    }
    let files = DiskFonts { base: s.game_dir.clone() };
    let engine = s.fonts.as_mut().expect("created above");
    let f = render_gui(&ui.gui, &scenes, GuiFonts { engine, files: &files }, &mut s.atlas, &view);
    s.surfaces = f.surfaces();
    s.viewport = [w, h];
    s.vertices = f.vertices;
    s.indices = f.indices;
    f.commands
}

/// This frame's device work, moved out of `s` for the sink.
pub fn take_upload(s: &mut AssetState) -> FrameUpload {
    let atlas = (s.atlas.version != s.atlas_sent && !s.atlas.is_empty()).then(|| {
        s.atlas_sent = s.atlas.version;
        (s.atlas.width, s.atlas.height, s.atlas.rgba.clone())
    });
    FrameUpload {
        vertices: std::mem::take(&mut s.vertices),
        indices: std::mem::take(&mut s.indices),
        surfaces: std::mem::take(&mut s.surfaces),
        viewport: s.viewport,
        atlas,
        textures: std::mem::take(&mut s.pending_textures),
        meshes: std::mem::take(&mut s.pending_meshes),
    }
}

/// The `ExecSink::render` side: the GUI stream, its render surfaces (viewport sized,
/// `D3D9RenderSurface::resize` 0x0068d350), the glyph atlas when it changed, the textures
/// and meshes not uploaded yet.
pub fn upload(ctx: &GpuContext<'_>, res: &mut Resources, u: FrameUpload) {
    let (device, queue) = (&ctx.device, &ctx.queue);
    res.set_gui_stream(u.vertices, u.indices);
    // The GUI's own viewport (`Engine+0x10c`/`+0x110`): the back buffer's size
    // (`Controller::set_back_buffer`), the client size before the first `resetDevice`.
    res.set_gui_viewport((u.viewport[0] > 0 && u.viewport[1] > 0).then_some((u.viewport[0], u.viewport[1])));
    for &id in &u.surfaces {
        res.ensure_surface(device, id, u.viewport[0], u.viewport[1]);
    }
    if let Some((w, h, rgba)) = u.atlas {
        // Performance note for a future optimiser: a new glyph re-creates and re-uploads the
        // whole atlas texture (a `write_texture` of the changed rows would do); it happens only
        // when text with new glyphs first shows.
        let t = GpuTexture::from_rgba8(device, queue, "glyph atlas", w, h, &rgba);
        let sampling = TextureSampling {
            min: TextureFilter::Linear,
            mag: TextureFilter::Linear,
            address_u: TextureAddress::Border,
            address_v: TextureAddress::Border,
        };
        res.add_texture(device, GLYPH_ATLAS_TEXTURE, t, sampling);
    }
    for (id, mesh) in u.meshes {
        res.models.insert_mesh(device, id, &mesh);
    }
    for (id, w, h, px, sampling) in u.textures {
        let t = GpuTexture::from_rgba8(device, queue, "plx texture", w, h, &px);
        res.add_texture(device, id, t, sampling);
    }
}


// ---------------------------------------------------------------------------------------------
// The world assets of the ctor (0x00459c40).

/// Texture handle the port gives the sun texture `GC+0x8006e0`.
pub const SUN_TEXTURE: TextureRef = 1;
/// Texture handle of the blob-shadow texture `GC+0x8006dc`.
pub const SHADOW_TEXTURE: TextureRef = 2;

/// `GC+0x800718`: the prop model table (`std::vector<Sprite*>` resized to 64 at 0x0046254c,
/// slots filled from the model vector `GC+0x300` at 0x00462563..0x00462bf9). Index = the
/// chunk prop kind; value = model-vector index (`ModelRef`), -1 for the null slots 18 and
/// 59..63. The render loop (0x004bd160) skips kinds < 0, >= 64 and 0x12, and 0x3c are the
/// tree sparks. (`scene::statics::STATIC_MODELS` is the other table, `GC+0x800724`.)
pub const PROP_MODELS: [i32; 64] = [
    0x977, 0x978, 0x979, 0x97a, 0x97b, 0x99f, 0x9a1, 0x9a0, // flowers2 .. alga
    0x97c, 0x97d, 0x97e, 0x97f, 0x980, 0x981, 0x982, 0x983, // lava-flower .. stone
    0x984, 0x99e, -1, 0x985, 0x986, 0x987, 0x988, 0x989, // stone2, christmas-tree, null, tendril ..
    0x98a, 0x98b, 0x98c, 0x98d, 0x98e, 0x98f, 0x990, 0x991, // pineapple-leaves .. water-lily01
    0x992, 0x993, 0x994, 0x995, 0x996, 0x997, 0x998, 0x999, // water-lily02, signs
    0x99a, 0x99b, 0x99c, 0x99d, 0x9a2, 0x9a3, 0x9a4, 0x9a5, // weavingmill-sign .. inca-art4
    0x9a6, 0x9a8, 0x9a7, 0x9a9, 0x9aa, 0x9ab, 0x9ac, 0x9ad, // crest1 .. liana
    0x9ae, 0x9af, 0x9b0, -1, -1, -1, -1, -1, // chandelier, cobwebs, cobwebs2, null
];

/// Model handle of the map.s border post `WorldMap+0x8000b0` (outside the model table and
/// the map tile range `cw_render::map::TILE_MODEL_BASE`).
pub const BORDER_POST_MODEL: u32 = 0x3fff_fff1;

/// The cloud model: `render`'s prologue (0x004ac2b0..0x004ac312) takes model-vector slot
/// 0xa08 (`cloud02.cub`) when the vector has at least 0xa09 models.
pub const CLOUD_MODEL: u32 = 0xa08;
/// The tint the prologue writes into the cloud Sprite (+0x5c..+0x5e) every frame, so its
/// mesh (`buildMesh` 0x004e7870, built lazily) blends toward (0, 100, 200).
pub const CLOUD_TINT: [u8; 3] = [0x00, 0x64, 0xc8];

/// Stars `GC+0x80075c` (0x004636a8..0x0046385f): 3000 of them, 4 `rand()` each, from the
/// CRT stream right after `World::load`'s `srand(timeGetTime())` and its 77 draws. Each is a
/// direction on the upper hemisphere (radius 100, z up) and a size `w` in 0.01..0.11,
/// doubled for one in ten. The order of the float/double conversions is the original's.
pub fn stars(rng: &mut cw_math::rand::MsvcRand) -> Vec<[f32; 4]> {
    (0..3000)
        .map(|_| {
            let theta = (rng.rand() as f64 * 6.283185307179586 / 32767.0) as f32;
            let phi = (rng.rand() as f64 * 1.5707963267948966 / 32767.0) as f32;
            let r = cw_math::cos(phi as f64) as f32 * 100.0;
            let mut w = rng.rand() as f32 * 0.1 / 32767.0 + 0.01;
            let z = cw_math::sin(phi as f64) as f32 * 100.0;
            let y = cw_math::sin(theta as f64) as f32 * r;
            let x = cw_math::cos(theta as f64) as f32 * r;
            if rng.rand() % 10 == 0 {
                w *= 2.0;
            }
            [x, y, z, w]
        })
        .collect()
}

/// The stars as the original seeds them (`timeGetTime()`, then `World::load`'s 77 draws).
pub fn stars_for_seed(seed: u32) -> Vec<[f32; 4]> {
    let mut rng = cw_math::rand::MsvcRand::new(seed);
    for _ in 0..77 {
        rng.rand();
    }
    stars(&mut rng)
}

/// The blob-shadow texture `GC+0x8006dc` (0x00462231..0x00462389): 256x256 A8R8G8B8,
/// white with `A = (int)(v³ · 128)`, `v = max(1 − (dx² + dy²) · 2⁻¹⁴, 0)` about the centre
/// 127.5. RGBA8.
pub fn shadow_texture_rgba() -> Vec<u8> {
    let mut out = Vec::with_capacity(256 * 256 * 4);
    for i in 0..256 {
        for j in 0..256 {
            let dx = j as f32 - 127.5;
            let dy = i as f32 - 127.5;
            let mut v = 1.0 - (dx * dx + dy * dy) * 6.103_515_6e-5;
            if 0.0 > v {
                v = 0.0;
            }
            let a = (v * v * v * 128.0) as i32 as u8;
            out.extend_from_slice(&[255, 255, 255, a]);
        }
    }
    out
}

/// The sun texture `GC+0x8006e0` (0x0046239f..0x0046252f): 256x256, `t = clamp(1.05 −
/// len · 1.05 / 128, 0, 1)`, `t4 = t⁴`, R = G = 255, B = `(int)(t4 · 155 + 100)`,
/// A = `(int)(t4 · 255)`. RGBA8.
pub fn sun_texture_rgba() -> Vec<u8> {
    let mut out = Vec::with_capacity(256 * 256 * 4);
    for i in 0..256 {
        for j in 0..256 {
            let dx = j as f32 - 127.5;
            let dy = i as f32 - 127.5;
            let len = ((dx * dx + dy * dy) as f64).sqrt() as f32;
            let mut t = (1.05 - ((len * 1.05) * 0.0078125) as f64) as f32;
            if 0.0 > t {
                t = 0.0;
            } else if !(t <= 1.0) {
                t = 1.0;
            }
            let t4 = t * t * t * t;
            let b = (t4 * 155.0 + 100.0) as i32 as u8;
            let a = (t4 * 255.0) as i32 as u8;
            out.extend_from_slice(&[255, 255, b, a]);
        }
    }
    out
}

/// Queues the ctor.s device assets for the first upload: the sun and shadow textures, the
/// cloud mesh with its tint (built before the executor.s lazy build could use tint 0) and
/// the map.s border post.
fn queue_world_assets(s: &mut AssetState, c: &Controller) {
    let clamp = TextureSampling {
        min: TextureFilter::Linear,
        mag: TextureFilter::Linear,
        address_u: TextureAddress::Border,
        address_v: TextureAddress::Border,
    };
    s.pending_textures.push((SUN_TEXTURE, 256, 256, sun_texture_rgba(), clamp));
    s.pending_textures.push((SHADOW_TEXTURE, 256, 256, shadow_texture_rgba(), clamp));
    if let Some(m) = c.models.models.get(CLOUD_MODEL as usize) {
        s.pending_meshes.push((CLOUD_MODEL, cw_render::mesh::build_world_model_mesh(m, CLOUD_TINT)));
    }
    // `WorldMap+0x8000b0`: the border post the `WorldMap` ctor 0x005fae40 builds.
    let (size, voxels) = cw_render::map::border_post_voxels();
    s.pending_meshes.push((BORDER_POST_MODEL, cw_render::map::mesh_tile_voxels(size, &voxels)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_textures() {
        let sh = shadow_texture_rgba();
        let sun = sun_texture_rgba();
        assert_eq!((sh.len(), sun.len()), (256 * 256 * 4, 256 * 256 * 4));
        // Centre texel (128,128): dx = dy = 0.5.
        let c = (128 * 256 + 128) * 4;
        assert_eq!(&sh[c..c + 4], &[255, 255, 255, 127]);
        // t = 1.05 - 0.707 * 1.05 / 128 > 1: clamped to 1.
        assert_eq!(&sun[c..c + 4], &[255, 255, 255, 255]);
        // Corners are empty.
        assert_eq!(sh[3], 0);
        assert_eq!(sun[3], 0);
    }

    #[test]
    fn star_hemisphere() {
        let s = stars_for_seed(12345);
        assert_eq!(s.len(), 3000);
        for st in &s {
            let r = (st[0] * st[0] + st[1] * st[1] + st[2] * st[2]).sqrt();
            assert!((r - 100.0).abs() < 0.01 && st[2] >= 0.0);
            assert!(st[3] >= 0.01 && st[3] <= 0.22);
        }
        assert_eq!(PROP_MODELS[18], -1);
        assert_eq!(PROP_MODELS[58], 0x9b0);
    }

    /// The screen rectangles (device pixels, through WorldView) and atlas rectangles (texels)
    /// of the glyph quads of a GUI frame.
    fn glyph_rects(cmds: &[GuiCommand], s: &AssetState) -> Vec<([f32; 4], [f32; 4])> {
        let mut out = Vec::new();
        for c in cmds {
            let GuiCommand::Draw(g) = c else { continue };
            if g.texture != Some(GLYPH_ATLAS_TEXTURE) {
                continue;
            }
            let m = &g.world_view;
            let screen = |p: [f32; 2]| [p[0] * m[0][0] + p[1] * m[1][0] + m[3][0], p[0] * m[0][1] + p[1] * m[1][1] + m[3][1]];
            let (aw, ah) = (s.atlas.width as f32, s.atlas.height as f32);
            for q in s.vertices[g.vertices.start as usize..g.vertices.end as usize].chunks_exact(4) {
                let (a, b) = (screen(q[0].position), screen(q[2].position));
                out.push(([a[0], a[1], b[0], b[1]], [q[0].uv[0] * aw, q[0].uv[1] * ah, q[2].uv[0] * aw, q[2].uv[1] * ah]));
            }
        }
        out
    }

    /// A HiDPI window (`app.rs`: the controller at the logical size, the back buffer physical):
    /// the GUI stream is built for the back buffer, and the start menu's text is rasterised at
    /// the size it is displayed, one atlas texel per back-buffer pixel, where the logical
    /// layout puts it. Skipped without `CW_GAME_DIR`.
    #[test]
    fn hidpi_gui_text_is_drawn_at_back_buffer_resolution() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut c = Controller::new(dir, [1280, 720], vec![(1280, 720)], false, String::new());
        // Second frames: the atlas no longer grows (UVs are normalised to its size).
        let mut s1 = AssetState::default();
        gui_frame(&c, &mut s1);
        let one = glyph_rects(&gui_frame(&c, &mut s1), &s1);
        assert!(!one.is_empty(), "the start menu draws text");
        c.set_back_buffer(2560, 1440, 2.0);
        c.on_resize(1280, 720);
        let mut s2 = AssetState::default();
        gui_frame(&c, &mut s2);
        let cmds = gui_frame(&c, &mut s2);
        let two = glyph_rects(&cmds, &s2);
        assert_eq!(take_upload(&mut s2).viewport, [2560, 1440], "the GUI viewport is the back buffer");
        assert_eq!(two.len(), one.len());
        for (k, ((r, t), (o, _))) in two.iter().zip(&one).enumerate() {
            let (sw, sh, tw, th) = (r[2] - r[0], r[3] - r[1], t[2] - t[0], t[3] - t[1]);
            assert!((sw - tw).abs() < 0.01 && (sh - th).abs() < 0.01, "quad {k}: {sw}x{sh} px drawn from {tw}x{th} texels");
            assert!(r.iter().all(|v| (v - v.round()).abs() < 0.01), "quad {k} off the pixel grid: {r:?}");
            // Where the logical layout puts it (the pen advances with the logical metrics
            // the widgets measured with), give or take the rounding of the bitmap boxes.
            assert!((r[0] - 2.0 * o[0]).abs() <= 3.0 && (r[1] - 2.0 * o[1]).abs() <= 3.0, "quad {k}: {r:?} vs 2 x {o:?}");
        }
    }
}
