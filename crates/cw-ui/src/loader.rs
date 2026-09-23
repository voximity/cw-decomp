//! Building the GUI scene from a parsed `.plx` document: what `PlxReader::read`
//! (`Cube.exe 0x00681c70`) does after decoding each chunk.
//!
//! Map from the original to this module:
//!
//! ```text
//! 0x00681c70 PlxReader::read          load_into: element dispatch, index maps, child links
//! 0x00686820 readTexture              texture(): format defaults, zlib, texture table
//! 0x00685b10 readTextShape            text_shape()
//! 0x00684ef0 readSmoothMeshShape      smooth_mesh_shape(): attributes, texture remap,
//!                                     setFaces 0x00642a20 / setSubdivisions 0x00642a50
//! 0x00683870 readGenericShape         generic_shape() (no shipped file has one)
//! 0x00686ff0 readTransformation       transformation()
//! 0x00683270 readDisplay              display()
//! 0x00687440.. readWidget & subclasses widget_source()
//! 0x00683f00 readNode                 load_into: createNode 0x0064f4e0, setWidget
//!                                     0x00636ef0, flags, children
//! 0x006806b0.. Attribute readers       apply_attribute / apply_array (frames, sequences)
//! 0x00682a80 readSequence             movie()
//! ```
//!
//! Index maps. The reader numbers shapes (TextShape, SmoothMeshShape, GenericShape
//! together), transformations, displays, widgets (all seven widget tags together) and
//! nodes in file order, each map seeded with `-1 → null`. A reference to an index that
//! has no element also yields null (`std::map::operator[]` inserts a zero). The texture
//! table maps `Texture.id` to the engine's texture handle (`texture + 0x4c`); it is seeded
//! with `-1 → -1`, and an unknown id reads as 0.
//!
//! The first `Node` element does not create a node: it is read into the node the file is
//! loaded into (`PlxReader + 0x78`, the `loadFile` target), and only its `Node.child`
//! list is used (its shape, transformation, display, widget and flags are read and
//! dropped). Every other `Node` element creates a node with its shape, transformation
//! and display (a fresh default Transformation / Display when it names none), attaches
//! its widget, and sets its flags. When the file ends, every node's `Node.child` list is
//! linked in node-index order (`Node::addChild` 0x00630be0), which fixes the draw order.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::rc::Rc;

use cw_formats::plx::{
    ArrayAttribute, ArrayAttributeItem, Attribute, AttributeItem, DisplayField, Elem, FontField,
    GenericShapeField, Item, KeyField, NodeField, PlxDocument, SequenceField, SmoothMeshShapeField,
    TextShapeField, TextureField, TransformationField, WidgetField, WidgetKind as PlxWidgetKind,
};
use glam::Vec2 as GVec2;

use crate::drawing::{Mat4, Vec2, Vec4};
use crate::font::{TextShape, TextShapeSource};
use crate::shape::{identity, Interpolate, Keyed, Movie, Shape, ShapeSource, SmoothMeshShape};
use crate::widget::{
    evaluate_transformation, Gui, HitShape, NodeId, NodeSource, WidgetId, WidgetSource,
    WidgetSourceKind,
};

/// `PlxReader + 0x60` option bits (the third argument of `Engine::loadFile` 0x00653770).
pub mod reader_flags {
    /// 0x4: every node read gets `Node.flags |= 8`.
    pub const NODE_FLAG_8: u32 = 0x4;
    /// 0x8: skip the page properties (`pageWidth` …).
    pub const NO_PAGE: u32 = 0x8;
    /// 0x10: rebuild each SmoothMeshShape right after reading and register it (0x006507c0).
    pub const REBUILD_SHAPES: u32 = 0x10;
    /// 0x20: pixel-snapped TextShapes (`TextShape + 0x1f0` bit 0).
    pub const PIXEL_SNAP: u32 = 0x20;
    /// 0x40: check the "PlasmaGraphics" header before reading.
    pub const CHECK_HEADER: u32 = 0x40;
    /// What the GameController ctor passes for all five files (`push 0x20` before each
    /// `loadFile` call, e.g. at 0x0045c143).
    pub const GAME_CONTROLLER: u32 = PIXEL_SNAP;
}

/// Page properties (`PlxReader + 0x74` record: width +0, height +4, dpi +8, unit +0xc,
/// colour +0x10).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub dpi: Option<f32>,
    pub unit: Option<i32>,
    pub color: Option<Vec4>,
}

/// A `plasma::Texture` as `readTexture` 0x00686820 hands it to `Engine::createTexture`
/// (engine slot 2): the format record (pixel format, filters, wraps; all default 1) and
/// the pixels, inflated when the file stores `Texture.compressedPixels`.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub name: String,
    /// `Texture.id` (the key of the texture table; -1 = not created).
    pub id: i32,
    pub width: i32,
    pub height: i32,
    /// `Texture.format.pixelFormat` (`D3D9Texture::upload` 0x0068bde0 picks A8R8G8B8 or
    /// R8G8B8 from it).
    pub pixel_format: i32,
    pub min_filter: i32,
    pub max_filter: i32,
    pub horizontal_wrap: i32,
    pub vertical_wrap: i32,
    /// Raw pixel bytes (RGBA or RGB rows as stored).
    pub pixels: Vec<u8>,
}

/// `plasma::Transformation` (vtable 0x0071f344, ctor 0x00678080, 0x234 bytes): the four
/// keyed attributes (translation +0x48, deformation +0xa0, rotation +0xf8, pivot +0x150).
#[derive(Clone, Debug, PartialEq)]
pub struct Transformation {
    pub name: String,
    pub translation: Keyed<Vec2>,
    /// Degrees about x, y, z (`ContinuousAttribute<Vector3>`, 12-byte frames).
    pub rotation: Keyed<[f32; 3]>,
    pub pivot: Keyed<Vec2>,
    pub deformation: Keyed<Mat4>,
    /// Keyable +0x2c: a sequence is playing.
    pub active: bool,
}

impl Default for Transformation {
    /// The ctor 0x00678080 defaults: zero translation, rotation and pivot, identity
    /// deformation (two frames each: the working value and the transition scratch).
    fn default() -> Self {
        Self {
            name: String::new(),
            translation: Keyed::from_frames(vec![[0.0; 2]; 2]),
            rotation: Keyed::from_frames(vec![[0.0; 3]; 2]),
            pivot: Keyed::from_frames(vec![[0.0; 2]; 2]),
            deformation: Keyed::from_frames(vec![identity(); 2]),
            active: false,
        }
    }
}

impl Transformation {
    /// `Transformation::evaluate` 0x00678600 of the current values.
    pub fn matrix(&self) -> Mat4 {
        evaluate_transformation(
            GVec2::from_array(self.translation.current),
            self.rotation.current,
            GVec2::from_array(self.pivot.current),
            &self.deformation.current,
        )
    }
}

/// `plasma::Display` (vtable 0x0071f380, ctor 0x00678ef0, 0x210 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub name: String,
    /// `DiscreteAttribute<int>` at +0x48 (frames at +0x94): the node and its subtree are
    /// drawn, updated and hit only while non-zero (`Node::update` 0x006372a0 tests
    /// `+0x94[+0x68] != 0` as a 4-byte int). Default 1.
    pub visibility: Keyed<i32>,
    /// `DiscreteAttribute<int>` at +0xa0 (frames at +0xec): children are clipped to the
    /// node's shape. Default 0.
    pub clipping: Keyed<i32>,
    /// "fill" colour at +0xf8, default (1, 1, 1, 1).
    pub fill_color: Keyed<Vec4>,
    /// "stroke" colour at +0x150, default (1, 1, 1, 1).
    pub stroke_color: Keyed<Vec4>,
    /// +0x1a8, default 0.
    pub blur_radius: Keyed<f32>,
    /// `Display.flags`.
    pub flags: i32,
    /// Keyable +0x2c.
    pub active: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            name: String::new(),
            visibility: Keyed::from_frames(vec![1; 2]),
            clipping: Keyed::from_frames(vec![0; 2]),
            fill_color: Keyed::from_frames(vec![[1.0; 4]; 2]),
            stroke_color: Keyed::from_frames(vec![[1.0; 4]; 2]),
            blur_radius: Keyed::from_frames(vec![0.0; 2]),
            flags: 0,
            active: false,
        }
    }
}

/// A `TextShape` with its keyed attributes (the string +0x5c and the three colours;
/// [`TextShape::source`] holds their current values).
#[derive(Clone, Debug, PartialEq)]
pub struct TextShapeNode {
    pub shape: TextShape,
    pub string: Keyed<Vec<u16>>,
    pub color: Keyed<Vec4>,
    pub stroke_color: Keyed<Vec4>,
    pub extrusion_color: Keyed<Vec4>,
    pub active: bool,
}

impl TextShapeNode {
    /// Copies the attributes' current values into the shape.
    pub fn sync(&mut self) {
        self.shape.source.strings = vec![self.string.current.clone()];
        self.shape.current_key = 0;
        self.shape.source.color = glam::Vec4::from_array(self.color.current);
        self.shape.source.stroke_color = glam::Vec4::from_array(self.stroke_color.current);
        self.shape.source.extrusion_color = glam::Vec4::from_array(self.extrusion_color.current);
    }
}

/// A `GenericShape` (0x00683870): a shape loaded from another file. No shipped file has
/// one; kept for completeness.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenericShapeSource {
    pub name: String,
    pub source: String,
    pub position: Vec2,
    pub size: Vec2,
}

/// One shape of the file, in `Node.shape` index order.
#[derive(Clone, Debug)]
pub enum SceneShape {
    Mesh(SmoothMeshShape, bool),
    Text(TextShapeNode),
    Generic(GenericShapeSource),
}

impl SceneShape {
    /// The shape's name.
    pub fn name(&self) -> &str {
        match self {
            SceneShape::Mesh(m, _) => &m.source.name,
            SceneShape::Text(t) => &t.shape.source.name,
            SceneShape::Generic(g) => &g.name,
        }
    }
}

/// Shapes are shared between the scene (which draws and animates them) and the nodes
/// that reference them (the hit test); `Node.shape` indices may repeat.
pub type SharedShape = Rc<RefCell<SceneShape>>;

/// Shape slots 4 / 8 / 5 for the hit test.
impl HitShape for SharedShape {
    /// SmoothMeshShape `containsPoint` 0x00642430; TextShape 0x00664210 (inclusive
    /// bounds); GenericShape: never hit.
    fn contains_point(&self, p: GVec2) -> bool {
        match &*self.borrow() {
            SceneShape::Mesh(m, _) => m.contains_point(p.to_array()),
            SceneShape::Text(t) => t.shape.contains_point(p),
            SceneShape::Generic(_) => false,
        }
    }

    /// SmoothMeshShape `getBounds` 0x00641aa0 under the identity; TextShape min/max.
    fn bounds(&self) -> (GVec2, GVec2) {
        match &*self.borrow() {
            SceneShape::Mesh(m, _) => {
                let (mut lo, mut hi) = ([0.0; 2], [0.0; 2]);
                let mut first = true;
                m.expand_bounds(&mut lo, &mut hi, &identity(), &mut first);
                (GVec2::from_array(lo), GVec2::from_array(hi))
            }
            SceneShape::Text(t) => (t.shape.min, t.shape.max),
            SceneShape::Generic(g) => (GVec2::from_array(g.position), GVec2::from_array(g.size)),
        }
    }

    /// Shape slot 5: SmoothMeshShape 0x0043a000 returns true (its drawings follow the
    /// owning widget's 9-slice deformation, so the hit test un-deforms the point);
    /// TextShape 0x00411330 returns false.
    fn is_deformed(&self) -> bool {
        matches!(&*self.borrow(), SceneShape::Mesh(..))
    }

    /// Shape slot 13 `clone` (called by `Node::clone` 0x006326d0): a deep copy in a new
    /// shared cell, so the clone's text and animation are its own.
    fn clone_shape(&self) -> Option<Box<dyn HitShape>> {
        let copy: SharedShape = Rc::new(RefCell::new(self.borrow().clone()));
        Some(Box::new(copy))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// Per `Node` element: the gui node it became and the objects it references.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneNode {
    pub node: NodeId,
    pub name: String,
    pub shape: Option<usize>,
    pub transformation: Option<usize>,
    pub display: Option<usize>,
    pub widget: Option<WidgetId>,
    /// The widget element index (`Node.widget`).
    pub widget_element: Option<usize>,
    pub children: Vec<i32>,
    /// `Node.variable` pairs (name, value), handed to 0x006536d0 (not ported).
    pub variables: Vec<(String, String)>,
}

/// Everything a `.plx` load produces besides the widget tree itself.
#[derive(Clone, Debug, Default)]
pub struct PlxScene {
    pub page: Page,
    pub textures: Vec<Texture>,
    /// The reader's texture table: `Texture.id` → index into [`PlxScene::textures`]
    /// (standing in for the engine handle `texture + 0x4c`).
    pub texture_table: BTreeMap<i32, i32>,
    pub shapes: Vec<SharedShape>,
    pub transformations: Vec<Transformation>,
    pub displays: Vec<Display>,
    /// Widget elements in file order, with their tag.
    pub widget_sources: Vec<WidgetSource>,
    /// One per `Node` element (index 0 is the load target).
    pub nodes: Vec<SceneNode>,
    /// Per gui node: its (transformation, display) objects, index into the vectors above;
    /// default objects created for nodes that name none are appended there too.
    pub node_objects: BTreeMap<NodeId, (usize, usize, Option<usize>)>,
}

/// Load options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadOptions {
    /// [`reader_flags`].
    pub reader_flags: u32,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { reader_flags: reader_flags::GAME_CONTROLLER }
    }
}

// ---------------------------------------------------------------------------------------
// Attribute readers
// ---------------------------------------------------------------------------------------

fn movie(fields: &[SequenceField]) -> (String, Movie) {
    let mut name = String::new();
    let mut m = Movie::default();
    for f in fields {
        match f {
            SequenceField::Name(s) => name = s.text(),
            SequenceField::WName(s) => name = s.text(),
            SequenceField::Key(k) => {
                // 0x00682a80: frame, time, smoothness start at 0 per key.
                let (mut frame, mut time, mut smooth) = (0, 0, 0.0);
                for kf in k {
                    match kf {
                        KeyField::Frame(v) => frame = *v,
                        KeyField::Time(v) => time = *v,
                        KeyField::Smoothness(v) => smooth = *v,
                        KeyField::Other(_) => {}
                    }
                }
                m.add_key(frame, time, smooth);
            }
            SequenceField::Other(_) => {}
        }
    }
    (name, m)
}

/// Writes frame `i` the way the attribute readers do (0x006806b0: `if i == count
/// addKey()`, then read into slot `i`); frame 0 is the working value.
fn put_frame<T: Clone>(k: &mut Keyed<T>, i: usize, v: T) {
    if i == k.keys.len() {
        k.add_key();
    }
    if i < k.keys.len() {
        k.keys[i] = v.clone();
    }
    if i == 0 {
        k.current = v;
    }
}

/// An `Attribute<T>` reader (0x006806b0 / 0x006808f0 / 0x00680b40 / 0x00680d80 /
/// 0x00680fd0): frames in order, sequences by name.
fn apply_attribute<T: Clone, U>(k: &mut Keyed<T>, a: &Attribute<U>, conv: impl Fn(&U) -> T) {
    let mut i = 0;
    for it in &a.items {
        match it {
            AttributeItem::Frame(v) => {
                put_frame(k, i, conv(v));
                i += 1;
            }
            AttributeItem::Sequence(s) => {
                let (name, m) = movie(s);
                // 0x006619a0: get or create the sequence, then add the keys to it.
                let e = k.sequences.entry(name).or_default();
                for key in m.keys {
                    e.add_key(key.frame, key.time, key.smoothness);
                }
            }
            AttributeItem::Other(_) => {}
        }
    }
    k.changed = true;
}

/// An `ArrayAttribute<T>` reader (0x006800d0 / 0x006803c0 / 0x0067fde0):
/// `ArrayAttribute.size` resizes every frame (and the working value), frames fill slots in
/// order.
fn apply_array<T: Elem + Default>(k: &mut Keyed<Vec<T>>, a: &ArrayAttribute<T>) {
    let mut i = 0;
    for it in &a.items {
        match it {
            ArrayAttributeItem::Size(n) => {
                let n = (*n).max(0) as usize;
                for f in &mut k.keys {
                    f.resize(n, T::default());
                }
                k.current.resize(n, T::default());
            }
            ArrayAttributeItem::Frame(v) => {
                put_frame(k, i, v.clone());
                i += 1;
            }
            ArrayAttributeItem::Sequence(s) => {
                let (name, m) = movie(s);
                let e = k.sequences.entry(name).or_default();
                for key in m.keys {
                    e.add_key(key.frame, key.time, key.smoothness);
                }
            }
            ArrayAttributeItem::Other(_) => {}
        }
    }
    k.changed = true;
}

// ---------------------------------------------------------------------------------------
// Element readers
// ---------------------------------------------------------------------------------------

/// Inflates `Texture.compressedPixels` (0x00449540, zlib).
pub fn inflate(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    match flate2::read::ZlibDecoder::new(data).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(_) => {
            out.clear();
            flate2::read::DeflateDecoder::new(data).read_to_end(&mut out)?;
            Ok(out)
        }
    }
}

fn texture(fields: &[TextureField]) -> Texture {
    let mut t = Texture {
        name: String::new(),
        id: -1,
        width: 0,
        height: 0,
        pixel_format: 1,
        min_filter: 1,
        max_filter: 1,
        horizontal_wrap: 1,
        vertical_wrap: 1,
        pixels: Vec::new(),
    };
    for f in fields {
        match f {
            TextureField::Name(s) => t.name = s.text(),
            TextureField::WName(s) => t.name = s.text(),
            TextureField::Id(v) => t.id = *v,
            TextureField::PixelFormat(v) => t.pixel_format = *v,
            TextureField::MinFilter(v) => t.min_filter = *v,
            TextureField::MaxFilter(v) => t.max_filter = *v,
            TextureField::HorizontalWrap(v) => t.horizontal_wrap = *v,
            TextureField::VerticalWrap(v) => t.vertical_wrap = *v,
            TextureField::Width(v) => t.width = *v,
            TextureField::Height(v) => t.height = *v,
            TextureField::Pixels(b) => {
                if !b.0.is_empty() {
                    t.pixels = b.0.clone();
                }
            }
            TextureField::CompressedPixels(b) => {
                if !b.0.is_empty() {
                    // A stream that does not inflate leaves the pixels as they were
                    // (the original's inflate helper returns what it produced).
                    if let Ok(p) = inflate(&b.0) {
                        t.pixels = p;
                    }
                }
            }
            TextureField::Other(_) => {}
        }
    }
    t
}

fn text_shape(fields: &[TextShapeField], flags: u32) -> TextShapeNode {
    let mut src = TextShapeSource::default();
    let mut string = Keyed::from_frames(vec![Vec::<u16>::new()]);
    let mut color = Keyed::from_frames(vec![[1.0; 4]]);
    let mut stroke = Keyed::from_frames(vec![[0.0; 4]]);
    let mut extrusion = Keyed::from_frames(vec![[0.0; 4]]);
    for f in fields {
        match f {
            TextShapeField::Name(s) => src.name = s.text(),
            TextShapeField::WName(s) => src.name = s.text(),
            TextShapeField::String(a) => apply_attribute(&mut string, a, |w| w.0.clone()),
            TextShapeField::Color(a) => apply_attribute(&mut color, a, |c| *c),
            TextShapeField::StrokeColor(a) => apply_attribute(&mut stroke, a, |c| *c),
            TextShapeField::ExtrusionColor(a) => apply_attribute(&mut extrusion, a, |c| *c),
            TextShapeField::Flags(v) => src.flags = *v as u32,
            TextShapeField::PixelSize(v) => src.size = *v,
            TextShapeField::StrokeRadius(v) => src.stroke_radius = *v,
            TextShapeField::Spacing(v) => src.spacing = *v,
            TextShapeField::LineSpacing(v) => src.line_spacing = *v,
            TextShapeField::WrapWidth(v) => src.wrap_width = *v,
            TextShapeField::FontSize(v) => src.size = *v as f32,
            TextShapeField::StrokeWidth(v) => src.stroke_radius = *v as f32 * 0.5,
            TextShapeField::FontName(s) => src.font_file = s.text(),
            TextShapeField::WFontName(s) => src.font_file = s.text(),
            TextShapeField::Font(ff) => {
                for f in ff {
                    match f {
                        FontField::FileName(s) => src.font_file = s.text(),
                        FontField::WFileName(s) => src.font_file = s.text(),
                        FontField::PixelSize(v) => src.size = *v as f32,
                        FontField::GlowRadius(v) => src.stroke_radius = *v as f32 * 0.5,
                        FontField::Size(v) => src.size = *v,
                        FontField::StrokeWidth(v) => src.stroke_radius = *v * 0.5,
                        _ => {}
                    }
                }
            }
            TextShapeField::Other(_) => {}
        }
    }
    src.pixel_snap = flags & reader_flags::PIXEL_SNAP != 0;
    let mut n = TextShapeNode {
        shape: TextShape::new(src),
        string,
        color,
        stroke_color: stroke,
        extrusion_color: extrusion,
        active: false,
    };
    n.sync();
    n
}

fn smooth_mesh_shape(fields: &[SmoothMeshShapeField], table: &mut BTreeMap<i32, i32>) -> SmoothMeshShape {
    use SmoothMeshShapeField as F;
    let mut s = ShapeSource::default();
    let mut faces: Vec<Vec<u32>> = Vec::new();
    // 0x00642610 parses each face; setFaces runs once at the end.
    let remap = |k: &mut Keyed<i32>, table: &mut BTreeMap<i32, i32>| {
        // Each frame v becomes table[v] (operator[]: unknown ids insert 0).
        for i in 0..k.keys.len() {
            let v = *k.key(i);
            let m = *table.entry(v).or_insert(0);
            k.keys[i] = m;
            if i == 0 {
                k.current = m;
            }
        }
    };
    for f in fields {
        match f {
            F::Name(v) => s.name = v.text(),
            F::WName(v) => s.name = v.text(),
            F::Face(l) => faces.push(l.0.iter().map(|&i| i as u32).collect()),
            F::VertexFlags(l) => s.vertex_flags = l.0.iter().map(|&i| i as u32).collect(),
            F::VertexParameters(l) => s.vertex_parameters = l.0.clone(),
            F::VertexPositions(a) => apply_array(&mut s.vertex_positions, a),
            F::VertexTexCoords(a) => apply_array(&mut s.vertex_tex_coords, a),
            F::VertexColors(a) => apply_array(&mut s.vertex_colors, a),
            F::StrokeColors(a) => apply_array(&mut s.stroke_colors, a),
            F::ExtrusionFrontColors(a) => apply_array(&mut s.extrusion_front_colors, a),
            F::ExtrusionBackColors(a) => apply_array(&mut s.extrusion_back_colors, a),
            F::StrokeWidths(a) => apply_array(&mut s.stroke_widths, a),
            F::Texture(a) => {
                apply_attribute(&mut s.texture, a, |v| *v);
                remap(&mut s.texture, table);
            }
            F::StrokeTexture(a) => {
                apply_attribute(&mut s.stroke_texture, a, |v| *v);
                remap(&mut s.stroke_texture, table);
            }
            F::TextureTranslation(a) => apply_attribute(&mut s.texture_translation, a, |v| *v),
            F::TextureRotation(a) => apply_attribute(&mut s.texture_rotation, a, |v| *v),
            F::TextureDeformation(a) => apply_attribute(&mut s.texture_deformation, a, |v| *v),
            F::TexturePivot(a) => apply_attribute(&mut s.texture_pivot, a, |v| *v),
            F::TextureOpacity(a) => apply_attribute(&mut s.texture_opacity, a, |v| *v),
            F::TextureBrightness(a) => apply_attribute(&mut s.texture_brightness, a, |v| *v),
            F::TextureContrast(a) => apply_attribute(&mut s.texture_contrast, a, |v| *v),
            F::TextureSaturation(a) => apply_attribute(&mut s.texture_saturation, a, |v| *v),
            F::StrokeTextureOpacity(a) => apply_attribute(&mut s.stroke_texture_opacity, a, |v| *v),
            F::StrokeTextureBrightness(a) => apply_attribute(&mut s.stroke_texture_brightness, a, |v| *v),
            F::StrokeTextureContrast(a) => apply_attribute(&mut s.stroke_texture_contrast, a, |v| *v),
            F::StrokeTextureSaturation(a) => apply_attribute(&mut s.stroke_texture_saturation, a, |v| *v),
            F::StrokeTextureStretch(a) => apply_attribute(&mut s.stroke_texture_stretch, a, |v| *v),
            F::ExtrusionMatrix(a) => apply_attribute(&mut s.extrusion_matrix, a, |v| *v),
            F::Subdivisions(v) => s.subdivisions = *v,
            F::SmoothWeight(v) => s.smooth_weight = *v,
            F::Flags(v) => s.flags = *v as u32,
            F::StrokeJointType(v) => s.stroke_joint_type = *v,
            F::StrokeCapType(v) => s.stroke_cap_type = *v,
            F::StrokeAlignment(v) => s.stroke_alignment = *v,
            F::StrokePattern(v) => s.stroke_pattern = *v,
            F::StrokeDash(v) => s.stroke_dash = *v,
            F::StrokeGap(v) => s.stroke_gap = *v,
            F::Other(_) => {}
        }
    }
    s.faces = faces;
    // setFaces 0x00642a20 + setSubdivisions 0x00642a50 (clamped 0..=6) + rebuild.
    SmoothMeshShape::new(s)
}

fn generic_shape(fields: &[GenericShapeField]) -> GenericShapeSource {
    let mut g = GenericShapeSource::default();
    for f in fields {
        match f {
            GenericShapeField::Name(s) => g.name = s.text(),
            GenericShapeField::WName(s) => g.name = s.text(),
            GenericShapeField::Source(s) => g.source = s.text(),
            GenericShapeField::WSource(s) => g.source = s.text(),
            GenericShapeField::Position(v) => g.position = *v,
            GenericShapeField::Size(v) => g.size = *v,
            GenericShapeField::Other(_) => {}
        }
    }
    g
}

fn transformation(fields: &[TransformationField]) -> Transformation {
    let mut t = Transformation::default();
    for f in fields {
        match f {
            TransformationField::Name(s) => t.name = s.text(),
            TransformationField::WName(s) => t.name = s.text(),
            TransformationField::Translation(a) => apply_attribute(&mut t.translation, a, |v| *v),
            TransformationField::Rotation(a) => apply_attribute(&mut t.rotation, a, |v| *v),
            TransformationField::Pivot(a) => apply_attribute(&mut t.pivot, a, |v| *v),
            TransformationField::Deformation(a) => apply_attribute(&mut t.deformation, a, |v| *v),
            TransformationField::Other(_) => {}
        }
    }
    t
}

fn display(fields: &[DisplayField]) -> Display {
    let mut d = Display::default();
    for f in fields {
        match f {
            DisplayField::Name(s) => d.name = s.text(),
            DisplayField::WName(s) => d.name = s.text(),
            DisplayField::Visibility(a) => apply_attribute(&mut d.visibility, a, |v| *v),
            DisplayField::Clipping(a) => apply_attribute(&mut d.clipping, a, |v| *v),
            DisplayField::StrokeColor(a) => apply_attribute(&mut d.stroke_color, a, |v| *v),
            DisplayField::FillColor(a) => apply_attribute(&mut d.fill_color, a, |v| *v),
            DisplayField::BlurRadius(a) => apply_attribute(&mut d.blur_radius, a, |v| *v),
            DisplayField::Flags(v) => d.flags = *v,
            DisplayField::Other(_) => {}
        }
    }
    d
}

/// The widget readers (0x00687440 Widget, 0x00683070 Button, 0x00683de0 ListWidget,
/// 0x00683750 Edit, 0x00684970 ScrollButton, 0x00684c30 ScrollSlider, 0x00684770
/// PopUpButton), all through the common field reader 0x00687560.
pub fn widget_source(kind: PlxWidgetKind, fields: &[WidgetField]) -> WidgetSource {
    let mut w = WidgetSource::default();
    let (mut button_type, mut direction) = (0, 0);
    for f in fields {
        match f {
            WidgetField::Name(s) => w.name = s.text(),
            WidgetField::WName(s) => w.name = s.text(),
            WidgetField::Caption(s) => w.caption = s.text(),
            WidgetField::InnerBindPos(v) => w.inner_bind_pos = GVec2::from_array(*v),
            WidgetField::InnerBindSize(v) => w.inner_bind_size = GVec2::from_array(*v),
            WidgetField::BindPos(v) => w.bind_pos = GVec2::from_array(*v),
            WidgetField::BindSize(v) => w.bind_size = GVec2::from_array(*v),
            WidgetField::FramePos(v) => w.frame_pos = GVec2::from_array(*v),
            WidgetField::FrameSize(v) => w.frame_size = GVec2::from_array(*v),
            WidgetField::BindMatrix(m) => w.bind_matrix = *m,
            WidgetField::HorizontalAlignment(v) => w.horizontal_alignment = *v,
            WidgetField::VerticalAlignment(v) => w.vertical_alignment = *v,
            WidgetField::Flags(v) => w.flags = *v as u32,
            WidgetField::ButtonType(v) => button_type = *v,
            WidgetField::ScrollButtonDirection(v) | WidgetField::ScrollSliderDirection(v) => direction = *v,
            WidgetField::Other(_) => {}
        }
    }
    w.kind = match kind {
        PlxWidgetKind::Widget => WidgetSourceKind::Widget,
        PlxWidgetKind::Button => WidgetSourceKind::Button { button_type },
        PlxWidgetKind::PopUpButton => WidgetSourceKind::PopUpButton { button_type },
        PlxWidgetKind::ScrollButton => WidgetSourceKind::ScrollButton { direction, button_type },
        PlxWidgetKind::ScrollSlider => WidgetSourceKind::ScrollSlider { direction, button_type },
        PlxWidgetKind::Edit => WidgetSourceKind::Edit,
        PlxWidgetKind::ListWidget => WidgetSourceKind::ListWidget,
    };
    w
}

// ---------------------------------------------------------------------------------------
// PlxReader::read
// ---------------------------------------------------------------------------------------

/// `-1`/missing → None, as the reader's index maps do.
fn index(v: i32, len: usize) -> Option<usize> {
    (v >= 0 && (v as usize) < len).then_some(v as usize)
}

/// Loads a document into a fresh [`Gui`] (the load target becomes its root node).
pub fn load(doc: &PlxDocument, opts: LoadOptions) -> (Gui, PlxScene) {
    let mut gui = Gui::new();
    let scene = load_into(&mut gui, None, doc, opts);
    (gui, scene)
}

/// `PlxReader::read` 0x00681c70 into `gui`, under the node `target` (`loadFile`'s node
/// argument; a new root node when `None` and the gui has no root, else the gui root).
pub fn load_into(gui: &mut Gui, target: Option<NodeId>, doc: &PlxDocument, opts: LoadOptions) -> PlxScene {
    let flags = opts.reader_flags;
    let mut sc = PlxScene::default();
    sc.texture_table.insert(-1, -1);
    // Pass 1: every non-node element in file order (the reader creates them as it goes;
    // nodes only refer backwards in the shipped files, but map lookups make order moot).
    for it in &doc.items {
        match it {
            Item::Texture(f) => {
                let t = texture(f);
                if t.id != -1 {
                    let handle = sc.textures.len() as i32;
                    sc.texture_table.insert(t.id, handle);
                    sc.textures.push(t);
                }
            }
            Item::TextShape(f) => {
                sc.shapes.push(Rc::new(RefCell::new(SceneShape::Text(text_shape(f, flags)))));
            }
            Item::SmoothMeshShape(f) => {
                let m = smooth_mesh_shape(f, &mut sc.texture_table);
                sc.shapes.push(Rc::new(RefCell::new(SceneShape::Mesh(m, false))));
            }
            Item::GenericShape(f) => {
                sc.shapes.push(Rc::new(RefCell::new(SceneShape::Generic(generic_shape(f)))));
            }
            Item::Transformation(f) => sc.transformations.push(transformation(f)),
            Item::Display(f) => sc.displays.push(display(f)),
            Item::Widget(k, f) => sc.widget_sources.push(widget_source(*k, f)),
            Item::PageWidth(v) if flags & reader_flags::NO_PAGE == 0 => sc.page.width = Some(*v),
            Item::PageHeight(v) if flags & reader_flags::NO_PAGE == 0 => sc.page.height = Some(*v),
            Item::Dpi(v) if flags & reader_flags::NO_PAGE == 0 => sc.page.dpi = Some(*v),
            Item::Unit(v) if flags & reader_flags::NO_PAGE == 0 => sc.page.unit = Some(*v),
            Item::PageColor(v) if flags & reader_flags::NO_PAGE == 0 => sc.page.color = Some(*v),
            _ => {}
        }
    }
    // Pass 2: nodes (0x00683f00).
    let mut first = true;
    for fields in doc.nodes() {
        let mut sn = SceneNode::default();
        let (mut shape, mut tr, mut disp, mut widget, mut nflags) = (-1, -1, -1, -1, 0i32);
        for f in fields {
            match f {
                NodeField::Name(s) => sn.name = s.text(),
                NodeField::WName(s) => sn.name = s.text(),
                NodeField::Shape(v) => shape = *v,
                NodeField::Transformation(v) => tr = *v,
                NodeField::Display(v) => disp = *v,
                NodeField::Widget(v) => widget = *v,
                NodeField::Child(v) => sn.children.push(*v),
                NodeField::Flags(v) => nflags = *v,
                NodeField::Variable(p) => sn.variables.push((p.1.text(), p.0.text())),
                NodeField::Other(_) => {}
            }
        }
        if first {
            first = false;
            sn.node = match target.or(gui.root) {
                Some(n) => n,
                None => gui.add_plain_node(None, &sn.name),
            };
            sc.nodes.push(sn);
            continue;
        }
        sn.shape = index(shape, sc.shapes.len());
        // createNode 0x0064f4e0: a fresh Transformation / Display when none is named.
        let ti = match index(tr, sc.transformations.len()) {
            Some(i) => i,
            None => {
                sc.transformations.push(Transformation::default());
                sc.transformations.len() - 1
            }
        };
        let di = match index(disp, sc.displays.len()) {
            Some(i) => i,
            None => {
                sc.displays.push(Display::default());
                sc.displays.len() - 1
            }
        };
        sn.transformation = Some(ti);
        sn.display = Some(di);
        let mut node_flags = nflags as u32;
        if flags & reader_flags::NODE_FLAG_8 != 0 {
            node_flags |= 8;
        }
        let t = &sc.transformations[ti];
        let d = &sc.displays[di];
        let text = sn.shape.and_then(|i| match &*sc.shapes[i].borrow() {
            SceneShape::Text(t) => Some(String::from_utf16_lossy(&t.string.current)),
            _ => None,
        });
        let src = NodeSource {
            name: sn.name.clone(),
            flags: node_flags,
            translation: GVec2::from_array(t.translation.current),
            pivot: GVec2::from_array(t.pivot.current),
            rotation: t.rotation.current,
            deformation: Some(t.deformation.current),
            visible: d.visibility.current != 0,
            clip: d.clipping.current != 0,
            text,
        };
        // Created parentless; linked by the child pass below. The gui has a root by now
        // (the target), so no created node becomes the root.
        let n = gui.add_node(None, src);
        if let Some(si) = sn.shape {
            gui.nodes[n].shape = Some(Box::new(sc.shapes[si].clone()));
        }
        // setWidget 0x00636ef0.
        if let Some(wi) = index(widget, sc.widget_sources.len()) {
            sn.widget_element = Some(wi);
            sn.widget = Some(gui.add_widget(n, &sc.widget_sources[wi]));
        }
        sn.node = n;
        sc.node_objects.insert(n, (ti, di, sn.shape));
        sc.nodes.push(sn);
    }
    // End of file: link every node's children in node-index order (0x00681c70 tail).
    for i in 0..sc.nodes.len() {
        let parent = sc.nodes[i].node;
        for &c in &sc.nodes[i].children.clone() {
            if let Some(ci) = index(c, sc.nodes.len()) {
                let child = sc.nodes[ci].node;
                if child != parent {
                    gui.add_child(parent, child);
                }
            }
        }
    }
    sc
}

// ---------------------------------------------------------------------------------------
// Animation (plasma::Keyable driven by Node::setState / Node::update)
// ---------------------------------------------------------------------------------------

fn start<T: Clone>(k: &mut Keyed<T>, name: &str, period: i32, active: &mut bool) {
    // Keyable::setState 0x00664c10: every attribute that has the sequence.
    if k.sequence(name).is_some() && k.start_sequence(name, period) {
        *active = true;
    }
}

fn adv<T: Interpolate>(k: &mut Keyed<T>, dt: i32, any: &mut bool) {
    if k.advance(dt) {
        *any = true;
    }
}

fn seek<T: Interpolate>(k: &mut Keyed<T>, t: i32, any: &mut bool) {
    if k.seek(t) {
        *any = true;
    }
}

impl PlxScene {
    /// `Node::isAnimating` `Cube.exe 0x006364f0`: the node's shape (`+0x34`), Transformation
    /// (`+0x38`) or Display (`+0x3c`) has a sequence playing (Keyable `+0x2c`), tested in that
    /// order, or else one of its children (list order) whose flags (`+0xc8`) lack bit 2
    /// ([`crate::widget::node_flags::DISABLED`]) is animating, recursively. Visibility is not
    /// tested. A node this scene did not build has no objects of its own here (its children
    /// are still searched). The GameController uses it on `questtag` (`GC+0x800868`,
    /// 0x00490a79) to show the tag only while its "shine" plays.
    pub fn is_animating(&self, gui: &Gui, n: NodeId) -> bool {
        if let Some(&(ti, di, si)) = self.node_objects.get(&n) {
            let shape = si.is_some_and(|si| match &*self.shapes[si].borrow() {
                SceneShape::Mesh(_, active) => *active,
                SceneShape::Text(t) => t.active,
                SceneShape::Generic(_) => false,
            });
            if shape || self.transformations[ti].active || self.displays[di].active {
                return true;
            }
        }
        gui.nodes[n]
            .children
            .iter()
            .any(|&c| gui.nodes[c].flags & crate::widget::node_flags::DISABLED == 0 && self.is_animating(gui, c))
    }

    fn subtree(&self, gui: &Gui, n: NodeId, out: &mut Vec<NodeId>) {
        out.push(n);
        for &c in &gui.nodes[n].children {
            self.subtree(gui, c, out);
        }
    }

    /// `Node::setState(name, period)` 0x00636810 on `n` and its whole subtree: every
    /// attribute of the node's shape, transformation and display that has a sequence
    /// called `name` starts playing it ([`Keyed::start_sequence`]).
    pub fn set_state(&mut self, gui: &Gui, n: NodeId, name: &str, period: i32) {
        let mut nodes = Vec::new();
        self.subtree(gui, n, &mut nodes);
        for n in nodes {
            let Some(&(ti, di, si)) = self.node_objects.get(&n) else { continue };
            if let Some(si) = si {
                match &mut *self.shapes[si].borrow_mut() {
                    SceneShape::Mesh(m, active) => {
                        let s = &mut m.source;
                        start(&mut s.vertex_positions, name, period, active);
                        start(&mut s.vertex_tex_coords, name, period, active);
                        start(&mut s.vertex_colors, name, period, active);
                        start(&mut s.stroke_colors, name, period, active);
                        start(&mut s.extrusion_front_colors, name, period, active);
                        start(&mut s.extrusion_back_colors, name, period, active);
                        start(&mut s.stroke_widths, name, period, active);
                        start(&mut s.texture, name, period, active);
                        start(&mut s.stroke_texture, name, period, active);
                        start(&mut s.texture_translation, name, period, active);
                        start(&mut s.texture_rotation, name, period, active);
                        start(&mut s.texture_pivot, name, period, active);
                        start(&mut s.texture_opacity, name, period, active);
                        start(&mut s.extrusion_matrix, name, period, active);
                    }
                    SceneShape::Text(t) => {
                        let mut a = t.active;
                        start(&mut t.string, name, period, &mut a);
                        start(&mut t.color, name, period, &mut a);
                        start(&mut t.stroke_color, name, period, &mut a);
                        start(&mut t.extrusion_color, name, period, &mut a);
                        t.active = a;
                    }
                    SceneShape::Generic(_) => {}
                }
            }
            let t = &mut self.transformations[ti];
            let mut a = t.active;
            start(&mut t.translation, name, period, &mut a);
            start(&mut t.rotation, name, period, &mut a);
            start(&mut t.pivot, name, period, &mut a);
            start(&mut t.deformation, name, period, &mut a);
            t.active = a;
            let d = &mut self.displays[di];
            let mut a = d.active;
            start(&mut d.visibility, name, period, &mut a);
            start(&mut d.clipping, name, period, &mut a);
            start(&mut d.fill_color, name, period, &mut a);
            start(&mut d.stroke_color, name, period, &mut a);
            start(&mut d.blur_radius, name, period, &mut a);
            d.active = a;
        }
    }

    /// `Node::update(dt)` 0x006372a0 via `Engine::update` 0x00659fb0 (called from
    /// `GameController::update` with the frame time in ms): advances every playing
    /// attribute of visible nodes by `dt`, rebuilds changed shapes (Keyable slot 1) and
    /// pushes the new transformation/display values into the gui nodes. Invisible nodes
    /// (and their subtrees) are not advanced, as in the original.
    pub fn update(&mut self, gui: &mut Gui, dt: i32) {
        let Some(root) = gui.root else { return };
        self.update_node(gui, root, dt);
    }

    fn update_node(&mut self, gui: &mut Gui, n: NodeId, dt: i32) {
        if gui.nodes[n].flags & crate::widget::node_flags::DISABLED != 0 || !gui.nodes[n].visible {
            return;
        }
        if let Some(&(ti, di, si)) = self.node_objects.get(&n) {
            if let Some(si) = si {
                let mut sh = self.shapes[si].borrow_mut();
                match &mut *sh {
                    SceneShape::Mesh(m, active) if *active => {
                        let mut any = false;
                        let s = &mut m.source;
                        adv(&mut s.vertex_positions, dt, &mut any);
                        adv(&mut s.vertex_tex_coords, dt, &mut any);
                        adv(&mut s.vertex_colors, dt, &mut any);
                        adv(&mut s.stroke_colors, dt, &mut any);
                        adv(&mut s.extrusion_front_colors, dt, &mut any);
                        adv(&mut s.extrusion_back_colors, dt, &mut any);
                        adv(&mut s.stroke_widths, dt, &mut any);
                        adv(&mut s.texture, dt, &mut any);
                        adv(&mut s.stroke_texture, dt, &mut any);
                        adv(&mut s.texture_translation, dt, &mut any);
                        adv(&mut s.texture_rotation, dt, &mut any);
                        adv(&mut s.texture_pivot, dt, &mut any);
                        adv(&mut s.texture_opacity, dt, &mut any);
                        adv(&mut s.extrusion_matrix, dt, &mut any);
                        if any {
                            m.rebuild(false);
                        } else {
                            *active = false;
                        }
                    }
                    SceneShape::Text(t) if t.active => {
                        let mut any = false;
                        adv(&mut t.string, dt, &mut any);
                        adv(&mut t.color, dt, &mut any);
                        adv(&mut t.stroke_color, dt, &mut any);
                        adv(&mut t.extrusion_color, dt, &mut any);
                        if any {
                            t.sync();
                            gui.nodes[n].text = Some(t.string.current.clone());
                        } else {
                            t.active = false;
                        }
                    }
                    _ => {}
                }
            }
            let t = &mut self.transformations[ti];
            if t.active {
                let mut any = false;
                adv(&mut t.translation, dt, &mut any);
                adv(&mut t.rotation, dt, &mut any);
                adv(&mut t.pivot, dt, &mut any);
                adv(&mut t.deformation, dt, &mut any);
                if any {
                    let node = &mut gui.nodes[n];
                    node.translation = GVec2::from_array(t.translation.current);
                    node.rotation = t.rotation.current;
                    node.pivot = GVec2::from_array(t.pivot.current);
                    node.deformation = t.deformation.current;
                } else {
                    t.active = false;
                }
            }
            let d = &mut self.displays[di];
            if d.active {
                let mut any = false;
                adv(&mut d.visibility, dt, &mut any);
                adv(&mut d.clipping, dt, &mut any);
                adv(&mut d.fill_color, dt, &mut any);
                adv(&mut d.stroke_color, dt, &mut any);
                adv(&mut d.blur_radius, dt, &mut any);
                if any {
                    gui.nodes[n].visible = d.visibility.current != 0;
                    gui.nodes[n].clip = d.clipping.current != 0;
                } else {
                    d.active = false;
                }
            }
        }
        for c in gui.nodes[n].children.clone() {
            self.update_node(gui, c, dt);
        }
    }

    /// `Node::stateDuration(name)` 0x00636f70: the latest last-key time of sequence `name`
    /// over the subtree's attributes.
    pub fn state_duration(&self, gui: &Gui, n: NodeId, name: &str) -> i32 {
        use crate::shape::sequence_end as e;
        let mut nodes = Vec::new();
        self.subtree(gui, n, &mut nodes);
        let mut best = 0;
        for n in nodes {
            let Some(&(ti, di, si)) = self.node_objects.get(&n) else { continue };
            let mut v = Vec::new();
            if let Some(si) = si {
                match &*self.shapes[si].borrow() {
                    SceneShape::Mesh(m, _) => {
                        let s = &m.source;
                        v.extend([
                            e(&s.vertex_positions, name),
                            e(&s.vertex_tex_coords, name),
                            e(&s.vertex_colors, name),
                            e(&s.stroke_colors, name),
                            e(&s.extrusion_front_colors, name),
                            e(&s.extrusion_back_colors, name),
                            e(&s.stroke_widths, name),
                            e(&s.texture_translation, name),
                        ]);
                    }
                    SceneShape::Text(t) => v.extend([e(&t.string, name), e(&t.color, name), e(&t.stroke_color, name)]),
                    SceneShape::Generic(_) => {}
                }
            }
            let t = &self.transformations[ti];
            v.extend([e(&t.translation, name), e(&t.rotation, name), e(&t.pivot, name), e(&t.deformation, name)]);
            let d = &self.displays[di];
            v.extend([e(&d.visibility, name), e(&d.clipping, name), e(&d.fill_color, name)]);
            best = v.into_iter().fold(best, i32::max);
        }
        best
    }

    /// The "snap" form of a state change (0x00636f70 → 0x00636cb0 → 0x00636f10): start the
    /// state, seek every attribute to its end, then stop.
    pub fn snap_state(&mut self, gui: &mut Gui, n: NodeId, name: &str) {
        self.set_state(gui, n, name, 0);
        let end = self.state_duration(gui, n, name);
        let mut nodes = Vec::new();
        self.subtree(gui, n, &mut nodes);
        for node in nodes {
            let Some(&(ti, di, si)) = self.node_objects.get(&node) else { continue };
            let mut any = false;
            if let Some(si) = si {
                if let SceneShape::Mesh(m, active) = &mut *self.shapes[si].borrow_mut() {
                    let s = &mut m.source;
                    seek(&mut s.vertex_positions, end, &mut any);
                    seek(&mut s.vertex_colors, end, &mut any);
                    seek(&mut s.stroke_colors, end, &mut any);
                    s.vertex_positions.stop();
                    s.vertex_colors.stop();
                    s.stroke_colors.stop();
                    *active = false;
                    if any {
                        m.rebuild(false);
                    }
                }
            }
            let t = &mut self.transformations[ti];
            seek(&mut t.translation, end, &mut any);
            seek(&mut t.rotation, end, &mut any);
            t.translation.stop();
            t.rotation.stop();
            t.active = false;
            let d = &mut self.displays[di];
            seek(&mut d.visibility, end, &mut any);
            d.visibility.stop();
            d.active = false;
            let g = &mut gui.nodes[node];
            g.translation = GVec2::from_array(t.translation.current);
            g.rotation = t.rotation.current;
            g.visible = d.visibility.current != 0;
        }
    }

    /// Routes the gui's queued [`crate::widget::UiEvent::State`] events to
    /// [`PlxScene::set_state`] / [`PlxScene::snap_state`] (period 0, as the widget code
    /// passes it) and returns the other events.
    pub fn apply_state_events(&mut self, gui: &mut Gui) -> Vec<crate::widget::UiEvent> {
        let events = std::mem::take(&mut gui.events);
        let mut rest = Vec::new();
        for e in events {
            match e {
                crate::widget::UiEvent::State { node, name, snap } => {
                    if snap {
                        self.snap_state(gui, node, name);
                    } else {
                        self.set_state(gui, node, name, 0);
                    }
                }
                other => rest.push(other),
            }
        }
        rest
    }
}
