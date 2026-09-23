//! `plasma::Engine::render` (`Cube.exe 0x00650980`) and the `D3D9Engine` / `D3D9Drawing` draw
//! path it drives, as the GUI command stream the wgpu executor consumes
//! (`cw_render::frame::GuiCommand`, executed by `cw_render::gpu::exec`).
//!
//! # The original, and where each piece went
//!
//! ```text
//! Engine::render 0x00650980
//!   D3D9Engine::beginFrame 0x00688b60 (slot 19)     states, Proj (+0x264), transform = I
//!   blur scratch surface (+0x44)->resize(viewport)   SURFACE_BLUR_TEMP
//!   Node::render(root, 1) 0x00632910                 Renderer::node
//!     pushTransform 0x00688db0 (slot 21)             world = local · parent
//!     [render-surface caching, 0x006329b1..]         drawn directly (see below)
//!     clip  = display.clipping && !(flags & 0x14)
//!     blur  = engine+0x18c bit 2 && display.flags & 1 && blurRadius > 0 && !(flags & 8)
//!     shape slot 14 (TextShape pixel-snap bounds)    TextShape::rebuild_for_transform
//!     r = (|col0| + |col1|) / 2 · blurRadius / 1.4   blur_extent
//!     visible = !(flags & 1) || 0x00636190(node)     Renderer::on_screen
//!     clip or blur:
//!       blur: pushSurface 0x0064edb0 (scale 8/r > 8)  BeginSurface(level), Renderer::scale
//!       clip: pushSurface + clearRect 0x0064eff0      BeginSurface(level + 1)
//!       shape draw (blur only, when visible)
//!       children (flags & ~8)
//!       clip: popSurface 0x006509f0 (+0x4c = mask),   EndSurface, draw_shape with
//!             shape draw masked (filter 0x0068ab70)   GuiDraw::mask_surface + filter
//!       blur: 2 × (horizontal, vertical) passes       Blur × 4 through SURFACE_BLUR_TEMP
//!             radius min(r, 8)/2 then min(r, 8),      Blit (src_scale)
//!             popSurface, drawCopy slot 5 (scale)
//!     else: shape draw                               Renderer::draw_shape
//!     widget slot 1 (game widgets, Edit caret)       Renderer::widget_draw
//!     children (no clip, no blur)
//!     popTransform 0x0068a260 (slot 22)
//!   endFrame 0x0068a250
//!
//! SmoothMeshShape::draw 0x00641280    extrusion, fill, stroke (engine colours +0x17c/15c/16c)
//! 0x0064ac00                          per-drawing texture, opacity, brightness, contrast,
//!                                     saturation, texture matrix (stroke: stretch)
//! D3D9Drawing::draw 0x0068ab70        Proj, WorldView, NormalMatrix, aaOffset, baseColor,
//!                                     widget deformation, mask, filter → GuiDraw
//! TextShape::draw 0x00663da0          bindWidgetShader 0x00688e70 + ScalableFont::draw
//! D3D9Engine::drawTexturedRect 0x00689af0  one textured quad per glyph (atlas sub-rect)
//! D3D9Engine::drawCaret 0x006897c0 / drawInvertedRect 0x00689950   GuiDraw { subtract }
//! Edit slot 1 0x006378e0              selection rect and blinking caret of the focused Edit
//! ```
//!
//! Tier B for the traversal order and the constants, Tier C for the batching: every drawing
//! is one [`GuiDraw`]; the glyph quads of one `ScalableFont::draw` call share one draw over the
//! [`GlyphAtlas`] (the original binds one small texture per glyph).
//!
//! # Deliberate simplifications (see the report)
//!
//! - **Render-surface caching** of widgets with flag 0x40 (`Widget+0x128` bit 6, surface
//!   `Widget+0x3c`, dirty byte `Widget+0x134`) and nodes with flag 0x400 (`Node+0xc8` bit
//!   10, surface `Node+0xd0`, dirty byte `Node+0xcc`), `0x006329b1..0x00632e90`, blitted by
//!   `drawScaled` when not dirty, stays a direct draw: cw-ui's widget layer models neither
//!   flag nor the two dirty bytes (their writers, the attribute setters that mark a subtree
//!   for re-render, are not traced), and a cache without them would show stale content.
//!   The pixels are the same up to the cache's pixel snap; the cost it saved is mostly gone
//!   now that each drawing's fringe buffers are cached ([`Drawing::cached_buffers`]). One
//!   visible difference: a cached widget scaled below 1 (`Widget+0x28`) is drawn with
//!   anti-aliasing off (`Engine+0x18c &= ~1`, 0x00632a3e) in the original.
//! - **Clipping** inside a downscaled blur: the original pushes the clip level with scale
//!   1 (0x006331a7) while the enclosing blur level is at `8 / r`; the port keeps the
//!   enclosing scale for the clip level so the mask and the shape line up (the original's
//!   combination was not observed in the shipped `.plx` files).
//! - Surface operations cover the whole surface (the `[0, scale]²` corner for a downscaled
//!   level; `GuiCommand::Blit`/`Blur` map it onto their rectangle), which is equivalent to
//!   the original's `bounds ± r` rectangles because everything outside is the cleared zero.
//! - The mask of a clipping node is not applied to its widget's slot-1 texts (the
//!   original's `bindWidgetShader` 0x00688e70 would bind `Engine+0x4c` for them too when
//!   given the node).
//! - The depth trick of `0x0068ab70` (engine+0x18c bit 3: ZFUNC GREATER per drawing) is off
//!   in the shipped engine (`+0x18c = 7`, ctor 0x0064d0f0) and not ported.
//! - The on-screen test uses the drawings' bounds for mesh shapes (the original reads
//!   `MeshShape` slots 6/7, whose key bounds the port only computes for unsubdivided shapes).

// NaN-aware comparisons and operand order follow the original.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::collapsible_if, clippy::assign_op_pattern)]

use std::collections::{BTreeMap, HashMap};

use cw_render::frame::{D3dMatrix, GuiCommand, GuiDraw, TextureRef};
use cw_render::gpu::pipelines::GuiVertex as ExecVertex;
use glam::{Affine2, Mat4, Vec2, Vec4};

use crate::drawing::{Drawing, GuiVertex};
use crate::font::{FontEngine, FontFileSource, GlyphKey, GlyphLayer, SizeState, TextDraw, TextStyle};
use crate::loader::{Display, PlxScene, SceneShape, SharedShape};
use crate::shape::{DrawingKind, Shape, SmoothMeshShape};
use crate::widget::{evaluate_transformation, node_flags, Gui, NodeId, WidgetId, WidgetKind};

// ---------------------------------------------------------------------------------------
// Handles shared with the executor
// ---------------------------------------------------------------------------------------

/// The blur scratch surface (`Engine+0x44`, created by `Engine::render` 0x00650980 and
/// resized to the viewport every frame).
pub const SURFACE_BLUR_TEMP: u32 = 0xff;

/// The level surfaces of `Engine+0x38` (`pushSurface` 0x0064edb0 creates one per nesting
/// level, viewport-sized): level `l` (1-based) is surface `SURFACE_LEVEL_BASE + l`.
pub const SURFACE_LEVEL_BASE: u32 = 0x100;

/// The render surface of nesting level `level` (1-based).
pub fn level_surface(level: u32) -> u32 {
    SURFACE_LEVEL_BASE + level
}

/// Texture handle of the glyph atlas (the port's stand-in for the per-glyph textures of
/// `0x0065ea80` / `0x0065e340`).
pub const GLYPH_ATLAS_TEXTURE: TextureRef = 0x7f00_0000;

/// Base of the `.plx` texture handles: texture `t` of scene `s` (the order of the `scenes`
/// slice given to [`render_gui`]) is `GUI_TEXTURE_BASE + (s << 12) + t` (the engine's
/// `texture + 0x4c` handles, made unique across files).
pub const GUI_TEXTURE_BASE: TextureRef = 0x7e00_0000;

/// The handle of texture `texture` of scene `scene` (see [`GUI_TEXTURE_BASE`]).
pub fn scene_texture_ref(scene: usize, texture: usize) -> TextureRef {
    GUI_TEXTURE_BASE + ((scene as u32) << 12) + texture as u32
}

/// `D3D9Engine+0x2a4`: the anti-aliasing offset ("333?" = 0.7, ctor 0x006887a0), uploaded as
/// `aaOffset` while `Engine+0x18c` bit 0 is set (it is: the Engine ctor 0x0064d0f0 stores 7).
pub const AA_OFFSET: f32 = 0.7;

/// `Engine+0x18c` as the Engine ctor 0x0064d0f0 leaves it (7): bit 0 anti-aliasing, bit 1
/// widget render-surface caching, bit 2 blur. Bit 3 (the depth trick) is off.
pub const ENGINE_FLAGS: u32 = 7;

// ---------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------

/// The font engine and where it reads font files (`Engine+0x34`).
pub struct GuiFonts<'a> {
    pub engine: &'a mut FontEngine,
    pub files: &'a dyn FontFileSource,
}

/// One `FontEngine::drawText` / `ScalableFont::draw` 0x0065bc70 call a game widget makes in
/// its slot-1 body (e.g. `StartMenuWidget` 0x00583320), in the widget node's space.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetText {
    /// Font file (`resource1.dat`, ...).
    pub font: String,
    /// UTF-16 text.
    pub text: Vec<u16>,
    /// Pen origin (the `x, y` arguments).
    pub origin: Vec2,
    /// Size, stroke radius, spacing, line spacing, wrap width, flags, pixel snap.
    pub style: TextStyle,
    /// Fill colour.
    pub color: [f32; 4],
    /// Stroke colour.
    pub stroke_color: [f32; 4],
}

/// Per-frame parameters of [`render_gui`].
#[derive(Clone, Debug, Default)]
pub struct GuiView {
    /// Viewport (`Engine+0x10c`, `+0x110`).
    pub width: u32,
    pub height: u32,
    /// `Engine+0xe8`: the engine clock in ms (the caret blinks every 500 ms, 0x006897c0).
    pub time_ms: i32,
    /// The game widgets' text calls, drawn at the widget's slot-1 point of the traversal.
    pub widget_texts: BTreeMap<WidgetId, Vec<WidgetText>>,
    /// Display visibility the caller sets for this frame before `Engine::render` (the
    /// `GameController::render` writes of 0x004ae27e..0x004ae370: `GC+0x800884` visible only
    /// when no menu screen is), overriding [`crate::widget::Node::visible`].
    pub visibility: BTreeMap<NodeId, bool>,
    /// Display fill colours the game widgets write this frame (`Display+0xf8`, e.g. the
    /// InventoryWidget tab icons 0x004c2050), overriding the scene's current value.
    pub fill_colors: BTreeMap<NodeId, [f32; 4]>,
}

/// What [`render_gui`] produces: the GUI vertex and index streams
/// (`Resources::set_gui_stream`) and the command list (`RenderInputs::gui`).
#[derive(Clone, Debug, Default)]
pub struct GuiFrame {
    pub vertices: Vec<ExecVertex>,
    pub indices: Vec<u32>,
    pub commands: Vec<GuiCommand>,
}

impl GuiFrame {
    /// The render surfaces the commands use (to create at the viewport size before the frame).
    pub fn surfaces(&self) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        for c in &self.commands {
            let s = match c {
                GuiCommand::BeginSurface { surface, .. }
                | GuiCommand::Blit { surface, .. }
                | GuiCommand::Blur { surface, .. }
                | GuiCommand::Downsample { surface, .. } => *surface,
                _ => continue,
            };
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// The number of [`GuiCommand::Draw`]s.
    pub fn draw_count(&self) -> usize {
        self.commands.iter().filter(|c| matches!(c, GuiCommand::Draw(_))).count()
    }
}

// ---------------------------------------------------------------------------------------
// Glyph atlas
// ---------------------------------------------------------------------------------------

/// The glyph bitmaps of every text drawn so far, packed into one RGBA8 texture
/// ([`GLYPH_ATLAS_TEXTURE`]). Each entry is an [`crate::font::AbFace::rasterize`] bitmap
/// (RGB 255, alpha = coverage, 1-pixel transparent border), keyed by font and [`GlyphKey`].
/// Shelf packing; the texture grows in height and is rebuilt from scratch when full.
/// [`GlyphAtlas::version`] changes whenever the pixels change, so the owner re-uploads.
#[derive(Clone, Debug)]
pub struct GlyphAtlas {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Bumped on every change of `rgba` or of its size.
    pub version: u64,
    entries: HashMap<(String, GlyphKey), Option<[u32; 4]>>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
}

/// Largest atlas height before the atlas is cleared and repacked.
const ATLAS_MAX_HEIGHT: u32 = 4096;

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new(1024, 256)
    }
}

impl GlyphAtlas {
    /// An empty atlas.
    pub fn new(width: u32, height: u32) -> Self {
        GlyphAtlas {
            width,
            height,
            rgba: vec![0; (width * height * 4) as usize],
            version: 1,
            entries: HashMap::new(),
            shelf_x: 0,
            shelf_y: 0,
            shelf_h: 0,
        }
    }

    /// Number of cached glyphs.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No glyph cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.rgba.iter_mut().for_each(|b| *b = 0);
        self.shelf_x = 0;
        self.shelf_y = 0;
        self.shelf_h = 0;
        self.version += 1;
    }

    fn grow(&mut self) -> bool {
        if self.height >= ATLAS_MAX_HEIGHT {
            return false;
        }
        let h = (self.height * 2).min(ATLAS_MAX_HEIGHT);
        self.rgba.resize((self.width * h * 4) as usize, 0);
        self.height = h;
        self.version += 1;
        true
    }

    /// Place a `w`×`h` bitmap (1 texel of padding around it); `None` when it cannot fit.
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (pw, ph) = (w + 1, h + 1);
        if pw > self.width {
            return None;
        }
        loop {
            if self.shelf_x + pw > self.width {
                self.shelf_y += self.shelf_h;
                self.shelf_x = 0;
                self.shelf_h = 0;
            }
            if self.shelf_y + ph <= self.height {
                let at = (self.shelf_x, self.shelf_y);
                self.shelf_x += pw;
                self.shelf_h = self.shelf_h.max(ph);
                return Some(at);
            }
            if !self.grow() {
                return None;
            }
        }
    }

    /// The atlas rectangle `[x, y, w, h]` (texels) of a glyph, rasterising it on a miss.
    /// `None` when the glyph has no bitmap.
    pub fn glyph(&mut self, fonts: &mut GuiFonts<'_>, font: &str, st: &SizeState, ch: u16, stroke: bool) -> Option<[u32; 4]> {
        let key = (font.to_string(), glyph_key(st, ch, stroke));
        if let Some(r) = self.entries.get(&key) {
            return *r;
        }
        let f = fonts.engine.get_font(fonts.files, font)?;
        let bmp = f.face.rasterize(st, ch, stroke);
        let rect = match bmp {
            Some(b) if b.width > 0 && b.height > 0 => {
                let at = match self.place(b.width, b.height) {
                    Some(at) => Some(at),
                    None => {
                        // Full: start over (quads already emitted this frame may show the
                        // wrong glyphs for one frame).
                        self.clear();
                        self.place(b.width, b.height)
                    }
                };
                at.map(|(x, y)| {
                    for row in 0..b.height {
                        let src = (row * b.width * 4) as usize;
                        let dst = (((y + row) * self.width + x) * 4) as usize;
                        self.rgba[dst..dst + (b.width * 4) as usize].copy_from_slice(&b.rgba[src..src + (b.width * 4) as usize]);
                    }
                    self.version += 1;
                    [x, y, b.width, b.height]
                })
            }
            _ => None,
        };
        self.entries.insert(key, rect);
        rect
    }
}

/// The glyph cache key the font module uses (`GlyphKey::new`, which is private there):
/// sizes and stroke radius in 26.6 as `(int)(x * 64 + 0.5)`.
fn glyph_key(st: &SizeState, ch: u16, stroke: bool) -> GlyphKey {
    let f = |x: f32| (x * 64.0 + 0.5) as i32;
    GlyphKey {
        ch,
        stroke,
        pixel: st.pixel_mode,
        size64: (f(st.size_x), f(st.size_y)),
        matrix: st.ft_matrix,
        stroke64: if stroke { f(st.stroke_radius) } else { 0 },
    }
}

// ---------------------------------------------------------------------------------------
// Matrices
// ---------------------------------------------------------------------------------------

/// A D3D row-major flat matrix as the executor's [`D3dMatrix`] rows.
fn rows(f: &[f32; 16]) -> D3dMatrix {
    [[f[0], f[1], f[2], f[3]], [f[4], f[5], f[6], f[7]], [f[8], f[9], f[10], f[11]], [f[12], f[13], f[14], f[15]]]
}

/// glam column-vector matrix → D3D row-vector rows (glam's columns are D3D's rows).
fn d3d(m: &Mat4) -> D3dMatrix {
    rows(&m.to_cols_array())
}

/// Registers `j = 0, 1` of a D3D matrix uploaded through `0x0068a910` (transpose, then the
/// first two registers): its first two columns.
fn two_columns(m: &D3dMatrix) -> [[f32; 4]; 2] {
    let c = |j: usize| [m[0][j], m[1][j], m[2][j], m[3][j]];
    [c(0), c(1)]
}

/// A 2D affine map as a column-vector 4×4.
fn mat4_of_affine(a: &Affine2) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(a.matrix2.x_axis.x, a.matrix2.x_axis.y, 0.0, 0.0),
        Vec4::new(a.matrix2.y_axis.x, a.matrix2.y_axis.y, 0.0, 0.0),
        Vec4::Z,
        Vec4::new(a.translation.x, a.translation.y, 0.0, 1.0),
    )
}

/// `D3D9Engine+0x264` as `beginFrame` 0x00688b60 builds it for a `w`×`h` viewport.
pub fn gui_proj(w: u32, h: u32) -> D3dMatrix {
    let (fw, fh) = (w as f32, h as f32);
    [
        [2.0 / (fw - 0.0), 0.0, 0.0, 0.0],
        [0.0, 2.0 / (0.0 - fh), 0.0, 0.0],
        [0.0, 0.0, f32::from_bits(0xb3d6_bf93), 0.0],
        [(fw + 0.0) / (0.0 - fw), (fh + 0.0) / (fh - 0.0), f32::from_bits(0x33d6_bf93), 1.0],
    ]
}

/// Transform a point with the perspective divide (row-vector D3D semantics).
fn project(m: &Mat4, p: Vec2) -> Vec2 {
    let v = *m * Vec4::new(p.x, p.y, 0.0, 1.0);
    Vec2::new(v.x / v.w, v.y / v.w)
}

fn to_exec(v: &GuiVertex) -> ExecVertex {
    ExecVertex { position: v.pos, color: v.color, normal0: v.n0, normal1: v.n1, uv: v.uv }
}

// ---------------------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------------------

/// Per-node objects the `.plx` scenes give the renderer: the Display and the scene the
/// node's shape textures index.
struct NodeInfo<'s> {
    display: Option<&'s Display>,
    scene: Option<usize>,
}

struct Renderer<'g, 's, 'f, 'a> {
    gui: &'g Gui,
    scenes: &'s [&'s PlxScene],
    info: HashMap<NodeId, NodeInfo<'s>>,
    fonts: GuiFonts<'f>,
    atlas: &'a mut GlyphAtlas,
    view: &'g GuiView,
    proj: D3dMatrix,
    level: u32,
    /// The content scale of the current target (the product of the `scale` arguments of
    /// the open `pushSurface` 0x0064edb0 calls; 1 on the back buffer): every draw's
    /// transform is scaled by it.
    scale: f32,
    /// The mask of the shape being drawn: `Engine+0x4c` (a level surface) and the PS c0
    /// `filter` 0x0068ab70 / 0x00688e70 pick from the node's Display.
    mask: Option<(u32, u8)>,
    out: GuiFrame,
}

/// `plasma::Engine::render` 0x00650980: the GUI of `gui` (from its root) as a command
/// stream. `scenes` are the `.plx` scenes loaded into `gui` (their order fixes the texture
/// handles, [`scene_texture_ref`]); `atlas` accumulates the glyph bitmaps.
pub fn render_gui(gui: &Gui, scenes: &[&PlxScene], fonts: GuiFonts<'_>, atlas: &mut GlyphAtlas, view: &GuiView) -> GuiFrame {
    let mut info: HashMap<NodeId, NodeInfo<'_>> = HashMap::new();
    for (si, sc) in scenes.iter().enumerate() {
        for (&n, &(_, di, _)) in &sc.node_objects {
            info.insert(n, NodeInfo { display: sc.displays.get(di), scene: Some(si) });
        }
    }
    let mut r = Renderer {
        gui,
        scenes,
        info,
        fonts,
        atlas,
        view,
        proj: gui_proj(view.width.max(1), view.height.max(1)),
        level: 0,
        scale: 1.0,
        mask: None,
        out: GuiFrame::default(),
    };
    // beginFrame 0x00688b60: the current transform is the identity; the root node's own
    // Transformation (and the engine root matrix) come in through pushTransform.
    if let Some(root) = gui.root {
        let base = mat4_of_affine(&gui.root_matrix);
        r.node(root, &base, 1);
    }
    r.out
}

/// PS c0 `filter` of a drawing of a clipping node (0x0068ab70 / 0x00688e70): with the
/// Display's `clipping` attribute set, 2 (multiply) when `flags & 2`, else 3 (mask alpha)
/// when `flags & 4`, else 1 (lerp to the mask); 0 without clipping. (The original also
/// leaves the mask unbound when `flags & 1` and clipping is 0; a clipping node always binds
/// it.)
fn mask_filter(d: Option<&Display>) -> u8 {
    match d {
        Some(d) if d.clipping.current != 0 => {
            if d.flags & 2 != 0 {
                2
            } else {
                (((d.flags as i8 as i32) & 4 | 2) >> 1) as u8
            }
        }
        // A node clipping through `Node.clip` without a Display (cloned nodes): lerp.
        _ => 1,
    }
}

/// `Display.flags` bit 0 and the blur radius of a node (0 when it has no Display).
fn blur_of(d: Option<&Display>) -> f32 {
    match d {
        Some(d) if d.flags & 1 != 0 && d.blur_radius.current > 0.0 => d.blur_radius.current,
        _ => 0.0,
    }
}

impl Renderer<'_, '_, '_, '_> {
    fn node_local(&self, n: NodeId) -> Mat4 {
        let node = &self.gui.nodes[n];
        Mat4::from_cols_array(&evaluate_transformation(node.translation, node.rotation, node.pivot, &node.deformation))
    }

    /// The node's world matrix (`Node+0x48`) computed from the root, for nodes other than
    /// the one being drawn (the widget's node of a deformed shape).
    fn world_of(&self, n: NodeId) -> Mat4 {
        let mut chain = vec![n];
        let mut cur = self.gui.nodes[n].parent;
        while let Some(p) = cur {
            chain.push(p);
            cur = self.gui.nodes[p].parent;
        }
        let mut m = mat4_of_affine(&self.gui.root_matrix);
        for &c in chain.iter().rev() {
            m = m * self.node_local(c);
        }
        m
    }

    fn scene_of(&self, n: NodeId) -> Option<usize> {
        // Nodes cloned at run time (members.rs `clone_subtree`) are not in any scene's map:
        // take the nearest ancestor's scene.
        let mut cur = Some(n);
        while let Some(c) = cur {
            if let Some(s) = self.info.get(&c).and_then(|i| i.scene) {
                return Some(s);
            }
            cur = self.gui.nodes[c].parent;
        }
        None
    }

    fn shape(&self, n: NodeId) -> Option<SharedShape> {
        let sh = self.gui.nodes[n].shape.as_ref()?;
        sh.as_any()?.downcast_ref::<SharedShape>().cloned()
    }

    /// `Node::render` 0x00632910.
    fn node(&mut self, n: NodeId, parent: &Mat4, flags: u32) {
        let node = &self.gui.nodes[n];
        // Display visibility (+0x94[+0x68]) and Node.flags bit 2.
        let shown = self.view.visibility.get(&n).copied().unwrap_or(node.visible);
        if !shown || node.flags & node_flags::DISABLED != 0 {
            return;
        }
        // pushTransform 0x00688db0: current = local · current.
        let world = *parent * self.node_local(n);
        let display = self.info.get(&n).and_then(|i| i.display);
        let clip = node.clip && flags & 0x14 == 0;
        let radius = blur_of(display);
        let blur = ENGINE_FLAGS & 4 != 0 && radius > 0.0 && flags & 8 == 0;
        let shape = self.shape(n);
        // Shape slot 14 (0x00664920): pixel-snapped TextShape bounds for this transform.
        if let Some(s) = &shape {
            if let SceneShape::Text(t) = &mut *s.borrow_mut() {
                if !t.shape.dirty {
                    t.shape.rebuild(&mut *self.fonts.engine, self.fonts.files);
                }
                t.shape.rebuild_for_transform(&mut *self.fonts.engine, self.fonts.files, &world.to_cols_array());
            }
        }
        // The effective blur radius (0x00632f40..0x00632fe0): mean column length of the
        // current transform times the radius, / 1.4. Above 8 the subtree is rendered at
        // scale `8 / r` (`fStack_f8`, pushSurface's argument) and blurred with radius 8
        // (`fStack_c0`).
        let (r, surface_scale, blur_radius) = if blur {
            let f = world.to_cols_array();
            let a = ((f[5] * f[5] + f[1] * f[1]) as f64).sqrt() as f32;
            let b = ((f[4] * f[4] + f[0] * f[0]) as f64).sqrt() as f32;
            let r = ((a + b) * 0.5 * radius) / 1.4;
            if 8.0 < r { (r, 8.0 / r, 8.0) } else { (r, 1.0, r) }
        } else {
            (0.0, 1.0, 1.0)
        };
        let _ = r;
        let visible = if flags & 1 != 0 { self.on_screen(n, &world, shape.as_ref(), radius) } else { true };
        let child_flags = flags & !8;

        if visible || (blur && !clip) {
            if clip || blur {
                let parent_scale = self.scale;
                if blur {
                    // pushSurface 0x0064edb0(fStack_f8) + clearRect(bounds ± 2r) (slot 8):
                    // the whole level is cleared (outside the rectangle is never read).
                    self.level += 1;
                    let l = level_surface(self.level);
                    self.out.commands.push(GuiCommand::BeginSurface { surface: l, clear_argb: Some(0) });
                    self.scale *= surface_scale;
                }
                if clip {
                    // pushSurface 0x0064edb0 (scale 1 inside a blur, the frame's `0` alone;
                    // the port keeps the enclosing scale so the mask lines up, see the
                    // module docs) + clearRect 0x0064eff0 of the node's bounds.
                    self.level += 1;
                    let l = level_surface(self.level);
                    self.out.commands.push(GuiCommand::BeginSurface { surface: l, clear_argb: Some(0) });
                } else if visible {
                    // Blur alone: the shape into the level, unmasked (0x006331bc).
                    self.draw_shape(n, &world, shape.as_ref());
                }
                self.children(n, &world, child_flags);
                if clip {
                    // popSurface 0x006509f0: Engine+0x4c = the level just ended; then the
                    // clipping node's shape (slot 2) drawn with it as the mask
                    // (0x00633236..0x00633243 inside a blur, 0x00633420..0x0063349a alone).
                    let l = level_surface(self.level);
                    self.out.commands.push(GuiCommand::EndSurface);
                    self.level -= 1;
                    self.mask = Some((l, mask_filter(display)));
                    self.draw_shape(n, &world, shape.as_ref());
                    self.mask = None;
                }
                if blur {
                    self.blur_chain(blur_radius, parent_scale);
                    self.scale = parent_scale;
                }
            } else {
                // Engine+0x4c = 0 (no mask), shape slot 2.
                self.draw_shape(n, &world, shape.as_ref());
            }
        }
        // The widget's slot 1 (game widgets draw their text; Edit its caret).
        if let Some(w) = self.gui.nodes[n].widget {
            self.widget_draw(w, n, &world);
        }
        if !clip && !blur && flags & 0x10 == 0 {
            self.children(n, &world, child_flags);
        }
        // popTransform 0x0068a260: the caller's `world` is untouched.
    }

    fn children(&mut self, n: NodeId, world: &Mat4, flags: u32) {
        for &c in &self.gui.nodes[n].children {
            self.node(c, world, flags);
        }
    }

    /// The blur of the current level surface and its copy into the parent target
    /// (`0x00633250..0x0063349b`): end the level, clear the scratch surface, then for
    /// `i = 2, 1` a horizontal pass level → scratch and a vertical pass scratch → level with
    /// radius `r / i` (`r` capped at 8), then `popSurface` 0x006509f0 and `drawCopy` (slot
    /// 5) of the level with the pushSurface scale. The level holds its content at
    /// `self.scale` (the surface's `[0, scale]²` corner); the passes stay in that corner and
    /// the copy scales it back up to the parent's scale.
    fn blur_chain(&mut self, r: f32, parent_scale: f32) {
        let l = level_surface(self.level);
        let t = SURFACE_BLUR_TEMP;
        let s = self.scale;
        let (w, h) = (self.view.width as f32, self.view.height as f32);
        let inner = [0.0, 0.0, w * s, h * s];
        let dest = [0.0, 0.0, w * parent_scale, h * parent_scale];
        let c = &mut self.out.commands;
        c.push(GuiCommand::EndSurface);
        c.push(GuiCommand::BeginSurface { surface: t, clear_argb: Some(0) });
        c.push(GuiCommand::EndSurface);
        for i in [2i32, 1] {
            let radius = r / i as f32;
            c.push(GuiCommand::BeginSurface { surface: t, clear_argb: None });
            c.push(GuiCommand::Blur { surface: l, horizontal: true, radius, rect: inner, src_scale: s });
            c.push(GuiCommand::EndSurface);
            c.push(GuiCommand::BeginSurface { surface: l, clear_argb: None });
            c.push(GuiCommand::Blur { surface: t, horizontal: false, radius, rect: inner, src_scale: s });
            c.push(GuiCommand::EndSurface);
        }
        c.push(GuiCommand::Blit { surface: l, color: [1.0; 4], rect: dest, src_scale: s });
        self.level -= 1;
    }

    /// `0x00636190`: the shape's box (deformed by the owner widget, grown by the Display blur
    /// radius) under the node's world matrix overlaps the viewport. No shape: true.
    fn on_screen(&self, n: NodeId, world: &Mat4, shape: Option<&SharedShape>, radius: f32) -> bool {
        let Some(shape) = shape else { return true };
        let (mut p0, mut p1) = match &*shape.borrow() {
            SceneShape::Mesh(m, _) => {
                let (mut lo, mut hi) = ([0.0; 2], [0.0; 2]);
                let mut first = true;
                m.expand_bounds(&mut lo, &mut hi, &crate::shape::identity(), &mut first);
                (Vec2::from_array(lo), Vec2::from_array(hi))
            }
            SceneShape::Text(t) => (t.shape.min, t.shape.max),
            SceneShape::Generic(g) => (Vec2::from_array(g.position), Vec2::from_array(g.size)),
        };
        if let Some((wbm, inv, du)) = self.deformation(n, world) {
            let d = |p: Vec2| project(&inv, du.deform(project(&wbm, p)));
            p0 = d(p0);
            p1 = d(p1);
        }
        if radius > 0.0 {
            p0 -= Vec2::splat(radius);
            p1 += Vec2::splat(radius);
        }
        let corners = [p0, Vec2::new(p1.x, p0.y), p1, Vec2::new(p0.x, p1.y)].map(|p| project(world, p));
        let (mut lo, mut hi) = (corners[0], corners[0]);
        for c in &corners[1..] {
            lo = lo.min(*c);
            hi = hi.max(*c);
        }
        let (w, h) = (self.view.width as f32, self.view.height as f32);
        !(hi.x < 0.0) && !(w <= lo.x) && !(hi.y < 0.0) && !(h <= lo.y)
    }

    /// The widget deformation of a node's shape (`0x0068ab70`, `Node+0x44` the owner
    /// widget): `(widgetBindMatrix, inverseWidgetBindMatrix, constants)` as column-vector
    /// matrices, when the deformation is not the identity.
    fn deformation(&self, n: NodeId, world: &Mat4) -> Option<(Mat4, Mat4, crate::widget::DeformUniforms)> {
        let w = self.gui.owner(n)?;
        let du = self.gui.deform_uniforms(w);
        if du.is_identity() {
            return None;
        }
        let wd = &self.gui.widgets[w];
        // widget+0xa8 (the file's bindMatrix) = inverse of the stored +0xe8.
        let file_bind = mat4_of_affine(&wd.bind_matrix.inverse());
        let widget_world = self.world_of(wd.node);
        // Row-vector nodeWorld · inv(widgetNodeWorld) · bindMatrix.
        let wbm = file_bind * widget_world.inverse() * *world;
        Some((wbm, wbm.inverse(), du))
    }

    /// Shape slot 2: `SmoothMeshShape::draw` 0x00641280 or `TextShape::draw` 0x00663da0.
    fn draw_shape(&mut self, n: NodeId, world: &Mat4, shape: Option<&SharedShape>) {
        let Some(shape) = shape else { return };
        let sh = shape.borrow();
        match &*sh {
            SceneShape::Mesh(m, _) => {
                let fill = match self.view.fill_colors.get(&n) {
                    Some(c) => *c,
                    None => self.info.get(&n).and_then(|i| i.display).map_or([1.0; 4], |d| d.fill_color.current),
                };
                let deform = self.deformation(n, world);
                let scene = self.scene_of(n);
                for (kind, d) in m.drawings() {
                    self.draw_drawing(world, m, kind, d, fill, deform.as_ref(), scene);
                }
            }
            SceneShape::Text(t) => {
                let text = self.gui.nodes[n].text.clone().unwrap_or_else(|| t.shape.text().to_vec());
                let Some(font) = t.shape.font.clone() else { return };
                let style = t.shape.style();
                let color = t.shape.source.color.to_array();
                let stroke = t.shape.source.stroke_color.to_array();
                drop(sh);
                let flat = world.to_cols_array();
                let td = match self.fonts.engine.get_font(self.fonts.files, &font) {
                    Some(f) => f.draw(&text, Vec2::ZERO, &flat, &style),
                    None => return,
                };
                self.emit_text(&font, &td, color, stroke);
            }
            SceneShape::Generic(_) => {}
        }
    }

    /// `D3D9Drawing::draw` 0x0068ab70 of one drawing, with the per-drawing state of
    /// 0x0064ac00.
    #[allow(clippy::too_many_arguments)]
    fn draw_drawing(
        &mut self,
        world: &Mat4,
        m: &SmoothMeshShape,
        kind: DrawingKind,
        d: &Drawing,
        fill: [f32; 4],
        deform: Option<&(Mat4, Mat4, crate::widget::DeformUniforms)>,
        scene: Option<usize>,
    ) {
        // The VB/IB of the drawing (`D3D9Drawing::update` 0x0068b7f0 runs only after a
        // rebuild): cached on the drawing. Performance note for a future optimiser: the
        // buffers are still copied into the frame's GUI stream every frame.
        let b = d.cached_buffers();
        if b.indices.is_empty() {
            return;
        }
        let s = &m.source;
        // 0x0064ac00: fill and extrusion use the texture attributes, stroke its own.
        let (tex, opacity, brightness, contrast, saturation, tmat) = match kind {
            DrawingKind::Fill | DrawingKind::Extrusion => (
                s.texture.current,
                s.texture_opacity.current,
                s.texture_brightness.current,
                s.texture_contrast.current,
                s.texture_saturation.current,
                m.texture_matrix,
            ),
            DrawingKind::Stroke => {
                let mut t = crate::shape::identity();
                let stretch = s.stroke_texture_stretch.current;
                if stretch > 0.0 {
                    let k = 1.0 / stretch;
                    if k != 1.0 {
                        for v in &mut t[0..4] {
                            *v *= k;
                        }
                    }
                }
                (
                    s.stroke_texture.current,
                    s.stroke_texture_opacity.current,
                    s.stroke_texture_brightness.current,
                    s.stroke_texture_contrast.current,
                    s.stroke_texture_saturation.current,
                    t,
                )
            }
        };
        // Engine texture lookup 0x00659ef0: the scene's texture table index.
        let texture = match scene {
            Some(sc) if tex >= 0 && (tex as usize) < self.scenes[sc].textures.len() => Some(scene_texture_ref(sc, tex as usize)),
            _ => None,
        };
        let base = self.out.vertices.len() as u32;
        self.out.vertices.extend(b.vertices.iter().map(to_exec));
        let first = self.out.indices.len() as u32;
        self.out.indices.extend_from_slice(&b.indices);
        let mut g = self.base_draw(world, base..self.out.vertices.len() as u32, first..self.out.indices.len() as u32);
        // baseColor = Display fill colour × engine colour (+0x15c/+0x16c/+0x17c, all white).
        g.base_color = fill;
        g.aa_offset = if ENGINE_FLAGS & 1 != 0 { AA_OFFSET } else { 0.0 };
        g.texture_enabled = texture.is_some();
        g.texture = texture;
        g.texture_matrix = two_columns(&rows(&tmat));
        g.texture_opacity = opacity;
        g.texture_brightness = brightness;
        g.texture_contrast = contrast;
        g.texture_saturation = saturation;
        if let Some((wbm, inv, du)) = deform {
            g.deformation_enabled = true;
            g.widget_bind_matrix = d3d(wbm);
            g.inverse_widget_bind_matrix = d3d(inv);
            g.widget_bind_pos = du.bind_pos.to_array();
            g.widget_bind_size = du.bind_size.to_array();
            g.deformed_widget_pos = du.deformed_pos.to_array();
            g.deformed_widget_size = du.deformed_size.to_array();
        }
        self.out.commands.push(GuiCommand::Draw(g));
    }

    /// The constants every GUI draw starts from: Proj (+0x264), WorldView (+0x224), the
    /// NormalMatrix (inverse of the transform, 0x0058c440), the MaskMatrix (the transform
    /// scaled to the viewport), no deformation, no texture, white, filter 0.
    fn base_draw(&self, world: &Mat4, vertices: std::ops::Range<u32>, indices: std::ops::Range<u32>) -> GuiDraw {
        // The target content scale (pushSurface 0x0064edb0 with a blur downscale).
        let scaled;
        let world = if self.scale != 1.0 {
            scaled = Mat4::from_scale(glam::Vec3::new(self.scale, self.scale, 1.0)) * *world;
            &scaled
        } else {
            world
        };
        let wv = d3d(world);
        // NormalMatrix: `inverse` then `transpose` then 0x0068a910's transpose: registers =
        // the first two rows of the inverse.
        let inv = d3d(&world.inverse());
        let normal = [inv[0], inv[1]];
        let (w, h) = (self.view.width.max(1) as f32, self.view.height.max(1) as f32);
        let scale = Mat4::from_scale(glam::Vec3::new(1.0 / w, 1.0 / h, 1.0));
        let mask = two_columns(&d3d(&(scale * *world)));
        GuiDraw {
            vertices,
            indices,
            proj: self.proj,
            world_view: wv,
            deformation_enabled: false,
            widget_bind_matrix: cw_render::frame::IDENTITY,
            inverse_widget_bind_matrix: cw_render::frame::IDENTITY,
            mask_matrix: mask,
            texture_matrix: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            normal_matrix: normal,
            widget_bind_pos: [0.0; 2],
            widget_bind_size: [0.0; 2],
            deformed_widget_pos: [0.0; 2],
            deformed_widget_size: [0.0; 2],
            aa_offset: 0.0,
            base_color: [1.0; 4],
            texture_enabled: false,
            filter: self.mask.map_or(0, |m| m.1),
            texture_opacity: 1.0,
            texture_brightness: 1.0,
            texture_contrast: 1.0,
            texture_saturation: 1.0,
            texture: None,
            mask: None,
            mask_surface: self.mask.map(|m| m.0),
            subtract: false,
        }
    }

    /// The glyph quads of one `ScalableFont::draw` (each a `drawTexturedRect` 0x00689af0 fan
    /// `(x0,y0) (x1,y0) (x1,y1) (x0,y1)` under `bindWidgetShader` 0x00688e70: texture on,
    /// identity texture matrix, white base colour), as one draw over the atlas.
    fn emit_text(&mut self, font: &str, td: &TextDraw, color: [f32; 4], stroke: [f32; 4]) {
        let base = self.out.vertices.len() as u32;
        let first = self.out.indices.len() as u32;
        let (aw, ah) = (self.atlas.width as f32, self.atlas.height as f32);
        let mut quads = Vec::new();
        for q in &td.quads {
            let is_stroke = q.layer == GlyphLayer::Stroke;
            if let Some(r) = self.atlas.glyph(&mut self.fonts, font, &td.state, q.ch, is_stroke) {
                quads.push((*q, r));
            }
        }
        // The atlas may have grown while rasterising: UVs use its final size.
        let (aw2, ah2) = (self.atlas.width as f32, self.atlas.height as f32);
        let _ = (aw, ah);
        for (q, r) in quads {
            let c = if q.layer == GlyphLayer::Stroke { stroke } else { color };
            let (u0, v0) = (r[0] as f32 / aw2, r[1] as f32 / ah2);
            let (u1, v1) = ((r[0] + r[2]) as f32 / aw2, (r[1] + r[3]) as f32 / ah2);
            let (x0, y0) = (q.pos.x, q.pos.y);
            let (x1, y1) = (q.pos.x + q.size.x, q.pos.y + q.size.y);
            let k = self.out.vertices.len() as u32 - base;
            for (p, uv) in [([x0, y0], [u0, v0]), ([x1, y0], [u1, v0]), ([x1, y1], [u1, v1]), ([x0, y1], [u0, v1])] {
                self.out.vertices.push(ExecVertex { position: p, color: c, normal0: [0.0; 2], normal1: [0.0; 2], uv });
            }
            self.out.indices.extend_from_slice(&[k, k + 1, k + 2, k, k + 2, k + 3]);
        }
        if self.out.indices.len() as u32 == first {
            return;
        }
        let world = Mat4::from_cols_array(&td.transform);
        let mut g = self.base_draw(&world, base..self.out.vertices.len() as u32, first..self.out.indices.len() as u32);
        g.texture_enabled = true;
        g.texture = Some(GLYPH_ATLAS_TEXTURE);
        self.out.commands.push(GuiCommand::Draw(g));
    }

    /// Widget vtable slot 1 at its point of `Node::render`: the game widgets' text calls
    /// (given by the caller in [`GuiView::widget_texts`]) and `Edit` 0x006378e0.
    ///
    /// The game widgets also draw item models here (`GameController 0x004758c0`, D3D draws
    /// in the middle of the traversal): before their texts (`InventoryWidget` 0x004c2577,
    /// `SpriteWidget` 0x0051c6a7, `PreviewWidget` 0x004d5d02, ...) or after them (the
    /// inventory's cursor stack, 0x004c595a). The port marks both points with
    /// [`GuiCommand::WidgetMark`] and `cw_render::passes` draws the models there.
    fn widget_draw(&mut self, w: WidgetId, n: NodeId, world: &Mat4) {
        let mark = |after_texts| GuiCommand::WidgetMark(cw_render::frame::GuiAnchor { widget: w as u32, after_texts });
        self.out.commands.push(mark(false));
        if let Some(texts) = self.view.widget_texts.get(&w) {
            let flat = world.to_cols_array();
            for t in texts.clone() {
                let td = match self.fonts.engine.get_font(self.fonts.files, &t.font) {
                    Some(f) => f.draw(&t.text, t.origin, &flat, &t.style),
                    None => continue,
                };
                self.emit_text(&t.font, &td, t.color, t.stroke_color);
            }
        }
        self.out.commands.push(mark(true));
        if let WidgetKind::Edit(e) = &self.gui.widgets[w].kind {
            // Only the focused Edit (0x006531e0 == this) draws.
            if self.gui.focused == Some(w) {
                let tn = e.text_node.unwrap_or(n);
                self.edit_caret(tn, e.caret, e.selection, world);
            }
        }
    }

    /// `Edit` slot 1 0x006378e0: the selection as `drawInvertedRect` (slot 4) from
    /// `(x0, -size)` sized `(x1 - x0, size * 1.25)`, then the caret (`drawCaret`, slot 3) at
    /// `(caret_x, -size)`, height `size * 1.25`, visible in odd 500 ms periods.
    fn edit_caret(&mut self, tn: NodeId, caret: i32, selection: i32, world: &Mat4) {
        let Some(shape) = self.shape(tn) else { return };
        let (text, font, style) = {
            let sh = shape.borrow();
            let SceneShape::Text(t) = &*sh else { return };
            let Some(font) = t.shape.font.clone() else { return };
            let text = self.gui.nodes[tn].text.clone().unwrap_or_else(|| t.shape.text().to_vec());
            (text, font, t.shape.style())
        };
        let len = text.len() as i32;
        let (mut a, mut b) = (caret, caret + selection);
        if b < a {
            std::mem::swap(&mut a, &mut b);
        }
        if a < 0 {
            a = 0;
        }
        if b > len {
            b = len;
        }
        // Layout under the text node's world matrix (`+0x148 node +0x48`).
        let tflat = self.world_of(tn).to_cols_array();
        let Some(f) = self.fonts.engine.get_font(self.fonts.files, &font) else { return };
        let x0 = f.caret(&text, a, &tflat, &style).0.x;
        let x1 = f.caret(&text, b, &tflat, &style).0.x;
        let cx = (caret >= 0 && caret <= len).then(|| f.caret(&text, caret, &tflat, &style).0.x);
        let size = style.size;
        self.subtract_rect(world, Vec2::new(x0, -size), Vec2::new(x1 - x0, size * 1.25));
        if let Some(cx) = cx {
            // (time / 500) odd: drawn.
            if (self.view.time_ms / 500) & 1 != 0 {
                // The original draws a LINELIST line; a 1-unit-wide quad here.
                self.subtract_rect(world, Vec2::new(cx, -size), Vec2::new(1.0, size * 1.25));
            }
        }
    }

    /// `drawInvertedRect` 0x00689950 / `drawCaret` 0x006897c0: an opaque black (0xff000000)
    /// quad with `BLENDOP SUBTRACT, ONE/ONE` under the current transform.
    fn subtract_rect(&mut self, world: &Mat4, p: Vec2, s: Vec2) {
        if s.x == 0.0 || s.y == 0.0 {
            return;
        }
        let base = self.out.vertices.len() as u32;
        let first = self.out.indices.len() as u32;
        for q in [p, Vec2::new(p.x + s.x, p.y), p + s, Vec2::new(p.x, p.y + s.y)] {
            self.out.vertices.push(ExecVertex {
                position: q.to_array(),
                color: [0.0, 0.0, 0.0, 1.0],
                normal0: [0.0; 2],
                normal1: [0.0; 2],
                uv: [0.0; 2],
            });
        }
        self.out.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        let mut g = self.base_draw(world, base..base + 4, first..first + 6);
        g.subtract = true;
        self.out.commands.push(GuiCommand::Draw(g));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::DiskFonts;
    use crate::loader::{load, LoadOptions};

    #[test]
    fn proj_matches_executor() {
        // The executor's registers are the transposed D3D matrix.
        let p = gui_proj(800, 600);
        let r = cw_render::gpu::pipelines::gui_projection(800, 600);
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(p[j][i], r[i][j], "({i},{j})");
            }
        }
    }

    /// Each widget's slot 1 is marked in the stream before and after its texts
    /// (`GuiCommand::WidgetMark`, where `cw_render::passes` draws the widget's item models),
    /// at its point of `Node::render` 0x00632910: after the node's shape, before the
    /// children (and so before a child widget's marks).
    #[test]
    fn widget_marks_around_slot_one() {
        use cw_render::frame::GuiAnchor;
        let mut gui = Gui::new();
        let root = gui.add_plain_node(None, "root");
        let a = gui.add_plain_node(Some(root), "a");
        let wa = gui.add_widget(a, &crate::widget::WidgetSource::default());
        let b = gui.add_plain_node(Some(a), "b");
        let wb = gui.add_widget(b, &crate::widget::WidgetSource::default());
        let mut engine = FontEngine::new();
        let files = DiskFonts { base: std::path::PathBuf::from("/nonexistent") };
        let mut atlas = GlyphAtlas::default();
        let view = GuiView { width: 800, height: 600, ..Default::default() };
        let f = render_gui(&gui, &[], GuiFonts { engine: &mut engine, files: &files }, &mut atlas, &view);
        let mark = |w: usize, after_texts: bool| GuiCommand::WidgetMark(GuiAnchor { widget: w as u32, after_texts });
        assert_eq!(f.commands, vec![mark(wa, false), mark(wa, true), mark(wb, false), mark(wb, true)]);
        assert_eq!(f.draw_count(), 0);
    }

    /// A game widget's text call (the start menu's "Start Game", 0x00583320) becomes one
    /// textured draw over the atlas. Skipped without `CW_GAME_DIR` (resource1.dat).
    #[test]
    fn widget_text_uses_atlas() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut gui = Gui::new();
        let root = gui.add_plain_node(None, "root");
        let n = gui.add_plain_node(Some(root), "menu");
        let w = gui.add_widget(n, &crate::widget::WidgetSource::default());
        let mut engine = FontEngine::new();
        let files = DiskFonts { base: dir };
        let mut atlas = GlyphAtlas::default();
        let style = TextStyle {
            size: 18.0,
            stroke_radius: 4.0,
            spacing: 0.0,
            line_spacing: 0.0,
            wrap_width: 0.0,
            flags: crate::font::align::H_CENTER,
            pixel_snap: true,
        };
        let text = WidgetText {
            font: "resource1.dat".into(),
            text: "Start Game".encode_utf16().collect(),
            origin: Vec2::new(100.0, 30.0),
            style,
            color: [1.0; 4],
            stroke_color: [0.0, 0.0, 0.0, 1.0],
        };
        let mut view = GuiView { width: 800, height: 600, ..Default::default() };
        view.widget_texts.insert(w, vec![text]);
        let f = render_gui(&gui, &[], GuiFonts { engine: &mut engine, files: &files }, &mut atlas, &view);
        assert_eq!(f.draw_count(), 1);
        let GuiCommand::Draw(g) = &f.commands[1] else { panic!() };
        assert_eq!(g.texture, Some(GLYPH_ATLAS_TEXTURE));
        // 9 letters (the space has an empty bitmap or a blank quad), fill + stroke each.
        let quads = (g.vertices.end - g.vertices.start) / 4;
        assert!((18..=20).contains(&quads), "{quads} quads");
        assert!(atlas.len() >= 16);
        assert!(atlas.rgba.chunks(4).any(|p| p[3] > 0));
    }

    #[test]
    fn atlas_packs_and_grows() {
        let mut a = GlyphAtlas::new(64, 16);
        let mut seen = Vec::new();
        for _ in 0..20 {
            let at = a.place(10, 10).unwrap();
            assert!(!seen.contains(&at));
            seen.push(at);
        }
        assert!(a.height > 16);
        assert_eq!(a.rgba.len(), (a.width * a.height * 4) as usize);
    }

    /// `start.plx` through [`render_gui`] headlessly: one draw per non-empty drawing of the
    /// visible shapes, the vertex stream is the sum of their buffers, every index is in range.
    /// Skipped without `CW_GAME_DIR`.
    #[test]
    fn start_plx_renders() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let bytes = std::fs::read(dir.join("start.plx")).unwrap();
        let doc = cw_formats::plx::parse(&bytes).unwrap();
        let (mut gui, scene) = load(&doc, LoadOptions::default());
        gui.viewport = glam::IVec2::new(1280, 720);
        let mut engine = FontEngine::new();
        engine.add_search_path(crate::font::ENGINE_FONT_SEARCH_PATH);
        let files = DiskFonts { base: dir.clone() };
        let mut atlas = GlyphAtlas::default();
        let view = GuiView { width: 1280, height: 720, ..Default::default() };
        let f = render_gui(&gui, &[&scene], GuiFonts { engine: &mut engine, files: &files }, &mut atlas, &view);

        // Independent count of the mesh drawings of drawable nodes: visible (with every
        // ancestor), not disabled. start.plx has no blur, so nothing is culled by surfaces.
        let mut expect_draws = 0;
        let mut expect_verts = 0;
        let mut stack = vec![gui.root.unwrap()];
        while let Some(n) = stack.pop() {
            let node = &gui.nodes[n];
            if !node.visible || node.flags & node_flags::DISABLED != 0 {
                continue;
            }
            stack.extend(node.children.iter().copied());
            let Some(sh) = node.shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<SharedShape>()) else {
                continue;
            };
            if let SceneShape::Mesh(m, _) = &*sh.borrow() {
                for (_, d) in m.drawings() {
                    let b = d.clone().build_buffers();
                    if !b.indices.is_empty() {
                        expect_draws += 1;
                        expect_verts += b.vertices.len();
                    }
                }
            }
        }
        let mesh_draws: Vec<&GuiDraw> = f
            .commands
            .iter()
            .filter_map(|c| match c {
                GuiCommand::Draw(g) if g.texture != Some(GLYPH_ATLAS_TEXTURE) && !g.subtract => Some(g),
                _ => None,
            })
            .collect();
        let mesh_verts: u32 = mesh_draws.iter().map(|g| g.vertices.end - g.vertices.start).sum();
        eprintln!(
            "start.plx: {} commands, {} mesh draws, {} vertices, {} indices, {} glyphs",
            f.commands.len(),
            mesh_draws.len(),
            f.vertices.len(),
            f.indices.len(),
            atlas.len()
        );
        let texts = scene.shapes.iter().filter(|s| matches!(&*s.borrow(), SceneShape::Text(_))).count();
        eprintln!("start.plx: {texts} text shapes; {expect_draws} drawings ({expect_verts} vertices) before culling");
        // The on-screen test (0x00636190) drops the shapes outside 1280x720 of the file as
        // loaded (no onResize here); what is drawn is a subset of the drawable drawings.
        assert!(mesh_draws.len() <= expect_draws);
        assert!(mesh_verts as usize <= expect_verts);
        // Regression counts of the shipped start.plx (lyon tessellation, fringe, culling).
        assert_eq!((mesh_draws.len(), f.vertices.len()), (76, 325_717), "start.plx draw/vertex counts changed");
        for c in &f.commands {
            if let GuiCommand::Draw(g) = c {
                let nv = g.vertices.end - g.vertices.start;
                for &i in &f.indices[g.indices.start as usize..g.indices.end as usize] {
                    assert!(i < nv);
                }
                assert!(g.vertices.end as usize <= f.vertices.len());
            }
        }
        // The logo (`cubeworld`) clips its gradient to the letters: some drawings are masked
        // by a level surface the stream renders first.
        let masked: Vec<u32> = mesh_draws.iter().filter_map(|g| g.mask_surface).collect();
        assert!(!masked.is_empty(), "no clipping node drew with a mask");
        for s in masked {
            assert!(f.surfaces().contains(&s));
            assert!(mesh_draws.iter().any(|g| g.mask_surface == Some(s) && g.filter != 0));
        }
    }
}
