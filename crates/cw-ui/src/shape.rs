//! `plasma::Shape` (Cube.exe vtable 0x007204ec) and `plasma::SmoothMeshShape` (vtable
//! 0x0071e92c, ctor 0x0063c2b0, size 0xc34): keyed vertex data, polygon faces, subdivision,
//! and the three drawings (fill +0xc10, stroke +0xc14, extrusion +0xc18).
//!
//! Pipeline of `SmoothMeshShape::rebuild` 0x00644fa0 (slot 1), with the Rust function for
//! each piece:
//!
//! ```text
//! dirty flags from the attributes' changed bytes            SmoothMeshShape::rebuild
//! 0x0066b9c0 vertex constraints (vertexFlags high word)     apply_vertex_constraints
//! 0x0063deb0 subdivision level (adaptive for glyphs)        SmoothMeshShape::levels
//! 0x0066c050 texture matrix                                 SmoothMeshShape::texture_matrix
//! 0x0066be90 bounds of the key positions (levels < 1)       SmoothMeshShape::key_bounds
//! 0x0063fec0/0x00671450/0x00671750 QuadMesh topology        QuadMesh::level0 / child
//! 0x00642ad0 QuadMesh::subdivide (mesh mode fill)           subdivide_mesh
//!   0x00671f80 subdivisionStep, 7 passes                    QuadMesh::step
//! curve arrays at 2^L stride, 0x0063ad70/0x0063b360/        build_curves / subdivide_curve
//!   0x0063a980/0x0063bba0 dyadic curve subdivision
//! stroke geometry (≈10 KB inside 0x00644fa0)                crate::stroke::build_stroke
//! 0x00648d60 buildDrawings                                  SmoothMeshShape::build_drawings
//!   stroke branch, caps 0x0063e020/0x0063ea00, dashes       crate::stroke::fill_stroke_drawing
//!   0x0063f3b0
//! ```
//!
//! Tier B/C. Floating-point operation order follows the original where it was read
//! (non-commutative sums of three or more terms are kept in order); the triangulation of
//! curve fills goes through lyon ([`crate::drawing`]).
//!
//! `SmoothMeshShape.flags` (+0x85c) bits:
//! [`FLAG_CURVE`], [`FLAG_STROKE`], [`FLAG_NO_FILL`], [`FLAG_OPEN`], [`FLAG_EXTRUSION`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};


use crate::drawing::{chain_loops, Drawing, Mat4, Vec2, Vec4};

/// Bit 0: fill the outline curves (GLU tessellation with a fitted colour gradient)
/// instead of the subdivided polygon mesh. Also makes every rebuild recompute the
/// drawings' outlines (the AA fringe and hit testing); in mesh mode only full rebuilds do.
pub const FLAG_CURVE: u32 = 1;
/// Bit 1: draw the stroke drawing (+0xc14).
pub const FLAG_STROKE: u32 = 2;
/// Bit 2: no fill drawing (+0xc10 is destroyed).
pub const FLAG_NO_FILL: u32 = 4;
/// Bit 3: the outline curves are open (no closing segment, stroke caps apply).
pub const FLAG_OPEN: u32 = 8;
/// Bit 4: draw the extrusion drawing (+0xc18).
pub const FLAG_EXTRUSION: u32 = 16;

/// Per-vertex `vertexFlags` low word (the "kind") values the subdivision reads.
pub mod kind {
    /// Fixed: never moved by smoothing (vertex flag bit 1).
    pub const FIXED: u16 = 1;
    /// Crease: boundary rule, edge midpoints stay on the crease (vertex flag bit 2).
    pub const CREASE: u16 = 2;
    /// Corner control point: first level uses the 1/(1+√2) arc weights (bits 2|3).
    pub const CORNER: u16 = 3;
}

/// `vertexFlags` high word values read by [`apply_vertex_constraints`].
pub mod constraint {
    /// 0x10000: the neighbours are tangent handles, placed along the direction to the
    /// next handle-kind vertex at distance `vertexParameters[i]`.
    pub const TANGENT: u32 = 1;
    /// 0x20000: the vertex slides between its nearest non-slider neighbours at parameter
    /// `vertexParameters[i]`.
    pub const SLIDER: u32 = 2;
}

// ---------------------------------------------------------------------------------------
// Keyed attributes
// ---------------------------------------------------------------------------------------

/// A value that `plasma::ContinuousAttribute<T>` / `ContinuousArrayAttribute<T>` can
/// interpolate component-wise.
pub trait Interpolate: Clone {
    /// The slot-6 interpolation of four frames (see [`Keyed::interpolate`]): `a`/`b` the
    /// smoothness of the segment's keys, `s` the segment parameter, `w` its Bernstein
    /// weights.
    #[allow(clippy::too_many_arguments)]
    fn slot6(v0: &Self, v1: &Self, v2: &Self, v3: &Self, a: f32, b: f32, s: f32, w: [f32; 4]) -> Self;
}

/// `ContinuousAttribute<float>::interpolate` 0x00669c70, per component:
///
/// ```text
/// f   = (v2 - v1) * 0.25
/// out = (v2 - ((v3 - v1)*0.25*b + (1-b)*f)) * w2
///     + ((v2 - v0)*0.25*a + (1-a)*f + v1) * w1
///     + v1 * w0 + v2 * w3
/// ```
///
/// i.e. a cubic Bézier from v1 to v2 whose inner control points blend a Catmull-Rom style
/// tangent ((v2-v0)/4, (v3-v1)/4) with the straight chord by `a`/`b`. The Vector3
/// (0x0066a950) and array (0x00669720, 0x00641b70) variants use the same formula with
/// slightly different association of the sums; this port uses the float order for all.
pub fn cubic_f32(v0: f32, v1: f32, v2: f32, v3: f32, a: f32, b: f32, w: [f32; 4]) -> f32 {
    let f = (v2 - v1) * 0.25;
    (v2 - ((v3 - v1) * 0.25 * b + (1.0 - b) * f)) * w[2]
        + ((v2 - v0) * 0.25 * a + (1.0 - a) * f + v1) * w[1]
        + v1 * w[0]
        + v2 * w[3]
}

impl Interpolate for f32 {
    fn slot6(v0: &Self, v1: &Self, v2: &Self, v3: &Self, a: f32, b: f32, _s: f32, w: [f32; 4]) -> Self {
        cubic_f32(*v0, *v1, *v2, *v3, a, b, w)
    }
}

impl<const N: usize> Interpolate for [f32; N] {
    fn slot6(v0: &Self, v1: &Self, v2: &Self, v3: &Self, a: f32, b: f32, _s: f32, w: [f32; 4]) -> Self {
        std::array::from_fn(|i| cubic_f32(v0[i], v1[i], v2[i], v3[i], a, b, w))
    }
}

/// `DiscreteAttribute<int>` slot 6 0x006531a0: frame `v1` while `s < 1`, else `v2` (the
/// texture indices and `Display.visibility`/`clipping`).
impl Interpolate for i32 {
    fn slot6(_v0: &Self, v1: &Self, v2: &Self, _v3: &Self, _a: f32, _b: f32, s: f32, _w: [f32; 4]) -> Self {
        if s < 1.0 { *v1 } else { *v2 }
    }
}

/// `DiscreteAttribute<wstring>` slot 6 0x006640c0 (vtable 0x0071ecb8, `TextShape.string`):
/// the same step rule as 0x006531a0.
impl Interpolate for Vec<u16> {
    fn slot6(_v0: &Self, v1: &Self, v2: &Self, _v3: &Self, _a: f32, _b: f32, s: f32, _w: [f32; 4]) -> Self {
        if s < 1.0 { v1.clone() } else { v2.clone() }
    }
}

/// Array attributes (`ContinuousArrayAttribute<T>`): element-wise over the length of the
/// v1 key (0x00669720 loops over key `i1`'s element count).
impl<T: Interpolate + ArrayElem> Interpolate for Vec<T> {
    fn slot6(v0: &Self, v1: &Self, v2: &Self, v3: &Self, a: f32, b: f32, s: f32, w: [f32; 4]) -> Self {
        (0..v1.len())
            .map(|i| T::slot6(&v0[i], &v1[i], &v2[i], &v3[i], a, b, s, w))
            .collect()
    }
}

/// Element types of `ContinuousArrayAttribute<T>`.
pub trait ArrayElem {}
impl ArrayElem for f32 {}
impl<const N: usize> ArrayElem for [f32; N] {}

/// The four weights `Attribute::evaluate` 0x00662300 passes to slot 6 for the segment
/// parameter `s` (cubic Bernstein basis, the original's operation order):
/// `((r·r)·r, ((s·3)·r)·r, ((s·3)·s)·r, (s·s)·s)` with `r = 1 - s`.
pub fn bernstein(s: f32) -> [f32; 4] {
    let r = 1.0 - s;
    [r * r * r, s * 3.0 * r * r, s * 3.0 * s * r, s * s * s]
}

/// One key of a `plasma::Movie` (12 bytes, `Attribute.sequence.key`): the attribute frame
/// shown at `time` (ms) and the smoothness of the curve through it (a float; the slot-6
/// interpolation multiplies with it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SeqKey {
    pub frame: i32,
    pub time: i32,
    pub smoothness: f32,
}

/// `plasma::Movie` (0x34 bytes, ctor 0x00677bc0; the attribute's working copy is the
/// 0x10-byte variant at `Attribute+0xc`): keys sorted by time, and a loop period
/// (`+0xc`; 0 = play once and hold the last key).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Movie {
    pub keys: Vec<SeqKey>,
    pub period: i32,
}

impl Movie {
    /// `0x006779e0 addKey(frame, time, smoothness)`: inserts before the first key whose
    /// time is ≥ `time` (so a new key goes before equal times), or appends; returns the
    /// index.
    pub fn add_key(&mut self, frame: i32, time: i32, smoothness: f32) -> usize {
        let at = self.keys.iter().position(|k| time <= k.time).unwrap_or(self.keys.len());
        self.keys.insert(at, SeqKey { frame, time, smoothness });
        at
    }

    /// `0x00630970 keyAt(i)`: wraps `i` modulo the key count when the movie loops, clamps
    /// it into range otherwise. Needs at least one key.
    pub fn key_at(&self, i: i32) -> SeqKey {
        let n = self.keys.len() as i32;
        if self.period != 0 {
            return self.keys[((i + n) % n) as usize];
        }
        let i = i.max(0).min(n - 1);
        self.keys[i as usize]
    }
}

/// `plasma::Attribute` (vtable 0x0071ebd0, ctor 0x00661480) and its Continuous/Discrete
/// templates: the frame values, the named sequences and the timeline state.
///
/// Frames live in the vector at `+0x4c`; the working value the shape reads is the frame at
/// the index in `+0x20`, which the constructor leaves at 0. So frame 0 *is* the working
/// value (here [`Keyed::current`], with `keys[0]` a stale copy of the file's frame 0), and
/// `setState` 0x00661df0 uses frame 1 as the scratch start of a transition. The shipped
/// files follow this convention: every animated attribute has frames 0 and 1 as the rest
/// value and its sequences key frames 2 and up.
///
/// Slots: 1 `getKeyCount` 0x00642580, 2 `addKey` 0x00668ea0, 3 `loadKey` 0x00669630, 4
/// `removeKey` 0x00669310, 5 `storeKey` 0x0066b020, 6 `interpolate` (0x00669c70 float,
/// 0x0066a950 Vector3, 0x00669720 / 0x00641b70 arrays). The byte at `+0x48` is the
/// "changed" flag `rebuild` turns into dirty bits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Keyed<T> {
    pub keys: Vec<T>,
    pub current: T,
    /// +0x48: set when the working value changed since the last rebuild.
    pub changed: bool,
    /// +0x4: named sequences (`Attribute.sequence`), keyed by the (wide) name.
    pub sequences: BTreeMap<String, Movie>,
    /// +0xc: the sequence being played (a filtered copy made by `setState`).
    pub working: Movie,
    /// +0x1c: play time in ms.
    pub time: i32,
    /// +0x3c: the frame last loaded by a hold (-1 after an interpolation).
    pub last_frame: i32,
    /// +0x40: the segment the last interpolation used (the search starts there).
    pub last_segment: i32,
}

impl<T: Clone> Keyed<T> {
    /// A constant (one frame, current = frame 0), what a static `.plx` attribute amounts to.
    pub fn constant(v: T) -> Self {
        Self::from_frames(vec![v])
    }

    /// Frames as read by the `.plx` attribute readers (0x006806b0 & co. write frame `i`
    /// into slot `i`, growing with slot 2); the working value is frame 0. `frames` must not
    /// be empty.
    pub fn from_frames(frames: Vec<T>) -> Self {
        Keyed {
            current: frames[0].clone(),
            keys: frames,
            changed: true,
            sequences: BTreeMap::new(),
            working: Movie::default(),
            time: 0,
            last_frame: -1,
            last_segment: 0,
        }
    }

    /// Frame `i` as the original reads it: frame 0 is the working value.
    pub fn key(&self, i: usize) -> &T {
        if i == 0 { &self.current } else { &self.keys[i] }
    }

    /// Slot 1 `getKeyCount` 0x00642580.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Slot 2 `addKey` 0x00668ea0: append the working value, return its index.
    pub fn add_key(&mut self) -> usize {
        self.keys.push(self.current.clone());
        self.keys.len() - 1
    }

    /// Slot 3 `loadKey` 0x00669630 (ignored out of range).
    pub fn load_key(&mut self, i: usize) {
        if i < self.keys.len() {
            self.current = self.key(i).clone();
        }
    }

    /// Slot 4 `removeKey` 0x00669310.
    pub fn remove_key(&mut self, i: usize) {
        self.keys.remove(i);
    }

    /// Slot 5 `storeKey` 0x0066b020 (ignored out of range).
    pub fn store_key(&mut self, i: usize) {
        if i < self.keys.len() {
            self.keys[i] = self.current.clone();
        }
    }

    /// `0x00661d90`: the named sequence, if any.
    pub fn sequence(&self, name: &str) -> Option<&Movie> {
        self.sequences.get(name)
    }

    /// `Attribute::startSequence` 0x00661df0 (from `Keyable::setState` 0x00664c10): makes
    /// the working movie a copy of sequence `name` with period `period`, keeping only the
    /// keys later than `period` when `period ≥ 1`; then, if the first key is not at time 0,
    /// stores the working value into frame 1 and adds a key (frame 1, time 0, the first
    /// key's smoothness), so the animation starts from the current value. Returns false
    /// (and changes nothing) when the sequence is missing or empty.
    pub fn start_sequence(&mut self, name: &str, period: i32) -> bool {
        let Some(seq) = self.sequences.get(name) else { return false };
        if seq.keys.is_empty() {
            return false;
        }
        let seq = seq.clone();
        self.working.keys.clear();
        self.working.period = period;
        self.time = 0;
        self.last_segment = 0;
        self.last_frame = -1;
        let n = seq.keys.len() as i32;
        for i in 0..n {
            let idx = if seq.period == 0 { i.max(0).min(n - 1) } else { (n + i) % n };
            let k = seq.keys[idx as usize];
            if period < 1 || period < k.time {
                self.working.add_key(k.frame, k.time, k.smoothness);
            }
        }
        if !self.working.keys.is_empty() && self.working.key_at(0).time != 0 {
            self.store_key(1);
            let s = self.working.key_at(0).smoothness;
            self.working.add_key(1, 0, s);
        }
        true
    }

    /// `0x006626b0`: stop (clear the working movie, time 0).
    pub fn stop(&mut self) {
        self.working.keys.clear();
        self.time = 0;
    }
}

impl<T: Interpolate> Keyed<T> {
    /// Slot 6 `interpolate(i0, i1, i2, i3, a, b, s, w0, w1, w2, w3)`: working value = the
    /// cubic of frames i0..i3 ([`cubic_f32`]); `a`/`b` are the smoothness of the segment's
    /// two keys, `s` (unused by the implementations) the segment parameter.
    pub fn interpolate(&mut self, i: [usize; 4], a: f32, b: f32, s: f32, w: [f32; 4]) {
        self.current = T::slot6(self.key(i[0]), self.key(i[1]), self.key(i[2]), self.key(i[3]), a, b, s, w);
        self.changed = true;
    }

    /// `Attribute::evaluate(movie, t)` 0x00662300 on the working movie. Before the first
    /// key (or with one key) it holds the first key's frame; after the last key of a
    /// non-looping movie it holds the last; otherwise it finds the segment containing `t`
    /// (starting from the last one used; looping movies shift key times by whole periods
    /// and close the loop with a segment from the last key to the first + period) and
    /// interpolates frames `(i-1, i, i+1, i+2)` (clamped or wrapped by [`Movie::key_at`])
    /// with `s = (t - t_i) / (t_{i+1} - t_i)` and [`bernstein`]`(s)`. Returns whether the
    /// working value changed. Any key naming a frame out of range aborts with false.
    pub fn evaluate(&mut self, t: i32) -> bool {
        let m = self.working.clone();
        let n = m.keys.len() as i32;
        if n == 0 {
            return false;
        }
        let nframes = self.keys.len() as i32;
        let hold = |this: &mut Self, k: SeqKey| {
            if this.last_frame != k.frame {
                this.load_key(k.frame as usize);
                this.last_frame = k.frame;
                this.changed = true;
                return true;
            }
            false
        };
        if n == 1 {
            return hold(self, m.key_at(0));
        }
        if t < m.key_at(0).time {
            return hold(self, m.key_at(0));
        }
        let last = m.key_at(n - 1);
        if !(t < last.time) && m.period == 0 {
            return hold(self, last);
        }
        self.last_frame = -1;
        let segs = if m.period > 0 { n } else { n - 1 };
        if segs <= self.last_segment {
            self.last_segment = 0;
        }
        for c in 0..segs {
            let i = (self.last_segment + c) % segs;
            let k1 = m.key_at(i);
            let mut j = i + 1;
            if m.period > 0 {
                j %= n;
            }
            let k2 = m.key_at(j);
            if k1.frame < 0 || k2.frame < 0 || nframes <= k1.frame || nframes <= k2.frame {
                return false;
            }
            let (mut t1, mut t2) = (k1.time, k2.time);
            if m.period > 0 {
                let off = (t / m.period) * m.period;
                t1 += off;
                t2 += off;
                if i == segs - 1 {
                    t2 += m.period;
                }
            }
            if t1 <= t && t <= t2 {
                let s = (t - t1) as f32 / (t2 - t1) as f32;
                let (i0, i3) = if m.period > 0 { ((i - 1 + n) % n, (i + 2) % n) } else { (i - 1, i + 2) };
                let k0 = m.key_at(i0);
                let k3 = m.key_at(i3);
                if k0.frame < 0 || k3.frame < 0 || nframes <= k0.frame || nframes <= k3.frame {
                    return false;
                }
                let idx = [k0.frame, k1.frame, k2.frame, k3.frame].map(|f| f as usize);
                self.interpolate(idx, k1.smoothness, k2.smoothness, s, bernstein(s));
                self.last_segment = i;
                self.changed = true;
                return true;
            }
        }
        false
    }

    /// `0x00662690 seek(t)`: set the time and evaluate there.
    pub fn seek(&mut self, t: i32) -> bool {
        self.time = t;
        self.evaluate(t)
    }

    /// `0x006626d0 advance(dt)`: evaluate at the current time, then add `dt`; a movie that
    /// is empty or no longer changes the value is cleared. Returns whether it changed.
    pub fn advance(&mut self, dt: i32) -> bool {
        if self.working.keys.is_empty() {
            self.working.keys.clear();
            return false;
        }
        let t = self.time;
        self.time = dt + t;
        let r = self.evaluate(t);
        if !r {
            self.working.keys.clear();
        }
        r
    }
}

/// The end time of sequence `name` in one attribute, as `Node::stateDuration` 0x00636f70
/// takes it: the time of its last key (0 without keys).
pub fn sequence_end<T>(k: &Keyed<T>, name: &str) -> i32 {
    match k.sequences.get(name) {
        Some(m) if !m.keys.is_empty() => m.key_at(m.keys.len() as i32 - 1).time,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------------------
// Source data
// ---------------------------------------------------------------------------------------

/// Everything `PlxReader::readSmoothMeshShape` 0x00684ef0 stores in a `SmoothMeshShape`.
///
/// This is the input contract for the `.plx` parser (`cw-formats/src/plx.rs`, being
/// written concurrently): each field names its chunk tag and the object offset it lands
/// at. Keyed fields are `plasma::*Attribute` objects in the original; a `.plx` stores
/// their keys, and a static shape has one key.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeSource {
    /// `SmoothMeshShape.name` (narrow) / `SmoothMeshShape.wname` (wide) → NamedObject +0xc.
    pub name: String,
    /// `SmoothMeshShape.face`: polygons as vertex index lists (int32 count + int32
    /// indices each; parsed by 0x00642610, stored by `setFaces` 0x0066b200 at +0x86c).
    /// Two-vertex faces are line segments (stroke only).
    pub faces: Vec<Vec<u32>>,
    /// `SmoothMeshShape.vertexFlags` (raw bytes → +0x2c4, one u32 per vertex): low 16 bits
    /// [`kind`], high 16 bits [`constraint`].
    pub vertex_flags: Vec<u32>,
    /// `SmoothMeshShape.vertexParameters` (raw bytes → +0x2d0, one f32 per vertex).
    pub vertex_parameters: Vec<f32>,
    /// `SmoothMeshShape.vertexPositions` → ContinuousArrayAttribute<Vector2> at +0x5c.
    pub vertex_positions: Keyed<Vec<Vec2>>,
    /// `SmoothMeshShape.vertexTexCoords` → ContinuousArrayAttribute<Vector2> at +0xb4.
    pub vertex_tex_coords: Keyed<Vec<Vec2>>,
    /// `SmoothMeshShape.vertexColors` → ContinuousArrayAttribute<Vector4> at +0x10c.
    pub vertex_colors: Keyed<Vec<Vec4>>,
    /// `SmoothMeshShape.strokeColors` → ContinuousArrayAttribute<Vector4> at +0x164.
    pub stroke_colors: Keyed<Vec<Vec4>>,
    /// `SmoothMeshShape.extrusionFrontColors` → ContinuousArrayAttribute<Vector4> at +0x214.
    pub extrusion_front_colors: Keyed<Vec<Vec4>>,
    /// `SmoothMeshShape.extrusionBackColors` → ContinuousArrayAttribute<Vector4> at +0x1bc.
    pub extrusion_back_colors: Keyed<Vec<Vec4>>,
    /// `SmoothMeshShape.strokeWidths` → ContinuousArrayAttribute<float> at +0x26c
    /// (clamped to ≥ 0.1 when used).
    pub stroke_widths: Keyed<Vec<f32>>,
    /// `SmoothMeshShape.texture` → attribute at +0x7ac (index into the file's Texture
    /// chunks, remapped by the reader through its texture table; -1 = none).
    pub texture: Keyed<i32>,
    /// `SmoothMeshShape.strokeTexture` → attribute at +0x804.
    pub stroke_texture: Keyed<i32>,
    /// `SmoothMeshShape.textureTranslation` → ContinuousAttribute<Vector2> at +0x2dc.
    pub texture_translation: Keyed<Vec2>,
    /// `SmoothMeshShape.textureRotation` → ContinuousAttribute<Vector3> at +0x38c, degrees
    /// about x, y, z.
    pub texture_rotation: Keyed<[f32; 3]>,
    /// `SmoothMeshShape.textureDeformation` → ContinuousAttribute<Matrix> at +0x3e4.
    pub texture_deformation: Keyed<Mat4>,
    /// `SmoothMeshShape.texturePivot` → ContinuousAttribute<Vector2> at +0x334.
    pub texture_pivot: Keyed<Vec2>,
    /// `SmoothMeshShape.textureOpacity/Brightness/Contrast/Saturation` →
    /// ContinuousAttribute<float> at +0x43c/+0x494/+0x4ec/+0x544 (shader 07 constants).
    pub texture_opacity: Keyed<f32>,
    pub texture_brightness: Keyed<f32>,
    pub texture_contrast: Keyed<f32>,
    pub texture_saturation: Keyed<f32>,
    /// `SmoothMeshShape.strokeTexture{Opacity,Brightness,Contrast,Saturation,Stretch}` →
    /// ContinuousAttribute<float> at +0x59c/+0x5f4/+0x64c/+0x6a4/+0x6fc.
    pub stroke_texture_opacity: Keyed<f32>,
    pub stroke_texture_brightness: Keyed<f32>,
    pub stroke_texture_contrast: Keyed<f32>,
    pub stroke_texture_saturation: Keyed<f32>,
    pub stroke_texture_stretch: Keyed<f32>,
    /// `SmoothMeshShape.extrusionMatrix` → ContinuousAttribute<Matrix> at +0x754.
    pub extrusion_matrix: Keyed<Mat4>,
    /// `SmoothMeshShape.subdivisions` (int32) → +0xb24 via 0x00642a50 (clamped 0..=6).
    pub subdivisions: i32,
    /// `SmoothMeshShape.smoothWeight` (float) → +0xc08. Default (ctor) (√2-1)/(√2/2).
    pub smooth_weight: f32,
    /// `SmoothMeshShape.flags` (int32) → +0x85c ([`FLAG_CURVE`] …).
    pub flags: u32,
    /// `SmoothMeshShape.strokeJointType` (int32) → +0x860.
    pub stroke_joint_type: i32,
    /// `SmoothMeshShape.strokeCapType` (int32) → +0x864 (1 round cap 0x0063e020, 2 square
    /// cap 0x0063ea00; only for open curves).
    pub stroke_cap_type: i32,
    /// `SmoothMeshShape.strokeAlignment` (int32) → +0x868.
    pub stroke_alignment: i32,
    /// `SmoothMeshShape.strokePattern` (int32) → +0xbf8.
    pub stroke_pattern: i32,
    /// `SmoothMeshShape.strokeDash` / `strokeGap` (float) → +0xbf0 / +0xbf4 (dash pattern
    /// through 0x0063f3b0).
    pub stroke_dash: f32,
    pub stroke_gap: f32,
}

/// The identity matrix (0x00423e70).
pub fn identity() -> Mat4 {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// The constructor default for `smoothWeight` (0x0063c2b0): `(√2 - 1) / (√2 · 0.5)`.
pub fn default_smooth_weight() -> f32 {
    let r = 2.0f64.sqrt() as f32;
    (r - 1.0) / (r * 0.5)
}

impl Default for ShapeSource {
    /// The values `SmoothMeshShape::SmoothMeshShape` 0x0063c2b0 and the attribute
    /// constructors leave before a reader fills anything.
    fn default() -> Self {
        ShapeSource {
            name: String::new(),
            faces: Vec::new(),
            vertex_flags: Vec::new(),
            vertex_parameters: Vec::new(),
            vertex_positions: Keyed::constant(Vec::new()),
            vertex_tex_coords: Keyed::constant(Vec::new()),
            vertex_colors: Keyed::constant(Vec::new()),
            stroke_colors: Keyed::constant(Vec::new()),
            extrusion_front_colors: Keyed::constant(Vec::new()),
            extrusion_back_colors: Keyed::constant(Vec::new()),
            stroke_widths: Keyed::constant(Vec::new()),
            texture: Keyed::constant(-1),
            stroke_texture: Keyed::constant(-1),
            texture_translation: Keyed::constant([0.0; 2]),
            texture_rotation: Keyed::constant([0.0; 3]),
            texture_deformation: Keyed::constant(identity()),
            texture_pivot: Keyed::constant([0.0; 2]),
            texture_opacity: Keyed::constant(1.0),
            texture_brightness: Keyed::constant(1.0),
            texture_contrast: Keyed::constant(1.0),
            texture_saturation: Keyed::constant(1.0),
            stroke_texture_opacity: Keyed::constant(1.0),
            stroke_texture_brightness: Keyed::constant(1.0),
            stroke_texture_contrast: Keyed::constant(1.0),
            stroke_texture_saturation: Keyed::constant(1.0),
            stroke_texture_stretch: Keyed::constant(1.0),
            extrusion_matrix: Keyed::constant(identity()),
            subdivisions: 3,
            smooth_weight: default_smooth_weight(),
            flags: 0,
            stroke_joint_type: 0,
            stroke_cap_type: 0,
            stroke_alignment: 0,
            stroke_pattern: 0,
            stroke_dash: 1.0,
            stroke_gap: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------------------
// The Shape interface
// ---------------------------------------------------------------------------------------

/// Which of a `SmoothMeshShape`'s drawings (and so which engine display state, engine
/// +0x15c / +0x16c / +0x17c in `draw` 0x00641280) a drawing is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawingKind {
    Fill,
    Stroke,
    Extrusion,
}

/// `plasma::Shape` (vtable 0x007204ec, 19 slots; ctor 0x00687b80). Slot map:
///
/// | Slot | Original | Rust |
/// |---|---|---|
/// | 0 | scalar deleting dtor 0x00687c60 | `Drop` |
/// | 1 | rebuild(bool full) | [`Shape::rebuild`] |
/// | 2 | draw(target) | [`Shape::drawings`] (the renderer draws them in order) |
/// | 3 | intersectsRect (circle test) | [`Shape::intersects_circle`] |
/// | 4 | containsPoint | [`Shape::contains_point`] |
/// | 5, 9, 12, 14–16, 18 | not identified | – |
/// | 6 | getPosition (stub 0x00687cf0 → (0,0)) | [`Shape::position`] |
/// | 7 | getSize (stub → (0,0)) | [`Shape::size`] |
/// | 8 | getBounds(min, max, matrix, first) | [`Shape::expand_bounds`] |
/// | 10 / 11 | invalidateGeometry / invalidateAppearance | [`Shape::invalidate`] |
/// | 13 | clone | `Clone` |
/// | 17 | isEmpty | [`Shape::is_empty`] |
pub trait Shape {
    fn rebuild(&mut self, full: bool);
    fn drawings(&self) -> Vec<(DrawingKind, &Drawing)>;
    fn contains_point(&self, p: Vec2) -> bool;
    fn intersects_circle(&self, p: Vec2, radius: f32, to_screen: &Mat4, to_local: &Mat4) -> bool;
    fn expand_bounds(&self, min: &mut Vec2, max: &mut Vec2, m: &Mat4, first: &mut bool) -> bool;
    fn position(&self) -> Vec2 {
        [0.0, 0.0]
    }
    fn size(&self) -> Vec2 {
        [0.0, 0.0]
    }
    fn invalidate(&mut self);
    fn is_empty(&self) -> bool;
}

// ---------------------------------------------------------------------------------------
// SmoothMeshShape
// ---------------------------------------------------------------------------------------

/// `plasma::SmoothMeshShape`: the source data, the derived topology and the drawings.
///
/// Slot overrides (vtable 0x0071e92c): 1 `rebuild` 0x00644fa0, 2 `draw` 0x00641280,
/// 3 `intersectsRect` 0x006424b0, 4 `containsPoint` 0x00642430, 8 `getBounds`
/// 0x00641aa0, 13 `clone` 0x00640e20, 17 `isEmpty` 0x00642410. Slots 6/7/10/11 are
/// inherited from `MeshShape` (0x00669080 / 0x00669060 / 0x0066be80 / 0x0066b1f0).
#[derive(Clone, Debug)]
pub struct SmoothMeshShape {
    pub source: ShapeSource,
    /// +0xb2c: adaptive subdivision (set for font glyphs, never by the `.plx` reader; the
    /// writer was not found).
    pub adaptive: bool,
    /// +0x8d8: the outline curves (boundary loops of the faces, `setFaces` 0x0066b200).
    pub curves: Vec<Vec<u32>>,
    /// +0x878..+0x884: bounds of the key positions (0x0066be90), what `getPosition` /
    /// `getSize` (MeshShape slots 6/7) return.
    pub key_bounds: (Vec2, Vec2),
    /// +0x890: the texture matrix (0x0066c050), for vertex shader 06's TextureMatrix.
    pub texture_matrix: Mat4,
    /// +0xc0c: dirty bits (0xf = everything).
    pub dirty: u32,
    pub fill: Option<Drawing>,
    pub stroke: Option<Drawing>,
    pub extrusion: Option<Drawing>,
}

impl SmoothMeshShape {
    /// `Engine::createSmoothMeshShape` 0x00650260 + the reader's `setFaces` /
    /// `setSubdivisions` (0x00642a20 / 0x00642a50) + the final `rebuild(true)`.
    pub fn new(mut source: ShapeSource) -> Self {
        source.subdivisions = source.subdivisions.clamp(0, 6);
        let curves = face_curves(&source.faces);
        let mut s = SmoothMeshShape {
            source,
            adaptive: false,
            curves,
            key_bounds: ([0.0; 2], [0.0; 2]),
            texture_matrix: identity(),
            dirty: 0xf,
            fill: None,
            stroke: None,
            extrusion: None,
        };
        s.rebuild(true);
        s
    }

    /// `setFaces` 0x0066b200 (via 0x00642a20): replace the faces and recompute the curves.
    pub fn set_faces(&mut self, faces: Vec<Vec<u32>>) {
        self.curves = face_curves(&faces);
        self.source.faces = faces;
        self.rebuild(true);
    }

    /// 0x0063deb0: the subdivision level. With `adaptive` and more than two subdivisions
    /// it is capped at `(int)(ln(2560 / Σ face sizes) / ln 4) + 1`, so dense glyphs
    /// subdivide less.
    pub fn levels(&self) -> i32 {
        let sub = self.source.subdivisions;
        if self.adaptive && sub > 2 {
            let total: usize = self.source.faces.iter().map(Vec::len).sum();
            let num = ((2560.0 / total as f32) as f64).ln() as f32;
            let den = 4.0f64.ln() as f32;
            let lvl = (num / den) as i32 + 1;
            if lvl < sub {
                return lvl;
            }
        }
        sub
    }

    /// 0x0066c050: texture matrix from textureTranslation T, texturePivot P,
    /// textureRotation (degrees x, y, z) and textureDeformation D, applied to the identity
    /// (0x00423e70) as: translate by T, translate by P, rotate about x, y, z, pre-multiply
    /// by D, translate by -P (row-vector convention: each step rewrites rows).
    pub fn compute_texture_matrix(&self) -> Mat4 {
        let s = &self.source;
        plasma_transform_matrix(
            s.texture_translation.current,
            s.texture_pivot.current,
            s.texture_rotation.current,
            &s.texture_deformation.current,
        )
    }

    /// Slot 1 `rebuild` 0x00644fa0 (plus `buildDrawings` 0x00648d60, which it calls).
    pub fn rebuild_impl(&mut self, full: bool) {
        let flags = self.source.flags;
        // Create or destroy the drawings (engine slot 12 createDrawing / 0x006504c0).
        sync_drawing(&mut self.fill, flags & FLAG_NO_FILL == 0);
        sync_drawing(&mut self.stroke, flags & FLAG_STROKE != 0);
        sync_drawing(&mut self.extrusion, flags & FLAG_EXTRUSION != 0);
        // Dirty bits from the attributes' changed bytes.
        if full {
            self.dirty = 0xf;
        } else {
            let s = &mut self.source;
            for (changed, bit) in [
                (&mut s.vertex_positions.changed, 1),
                (&mut s.vertex_colors.changed, 4),
                (&mut s.vertex_tex_coords.changed, 2),
                (&mut s.stroke_widths.changed, 1),
                (&mut s.stroke_colors.changed, 4),
                (&mut s.extrusion_front_colors.changed, 4),
                (&mut s.extrusion_back_colors.changed, 4),
            ] {
                if *changed {
                    self.dirty |= bit;
                    *changed = false;
                }
            }
        }
        self.dirty |= 1;

        // Working copies (the original edits the attributes' working values in place).
        let mut positions = self.source.vertex_positions.current.clone();
        apply_vertex_constraints(&mut positions, &self.curves, &self.source.vertex_flags, &self.source.vertex_parameters);

        let levels = self.levels();
        self.texture_matrix = self.compute_texture_matrix();
        if levels - 1 < 0 {
            self.key_bounds = bounds_of(&positions);
        } else if flags & FLAG_NO_FILL == 0 && flags & FLAG_CURVE == 0 {
            if let Some(fill) = self.fill.as_mut() {
                subdivide_mesh(
                    fill,
                    &self.source,
                    &positions,
                    levels,
                    self.source.smooth_weight,
                );
            }
        }
        let curves = build_curves(&self.source, &self.curves, &positions, levels.max(0) as u32);
        let stroke_geometry = if flags & FLAG_STROKE != 0 { Some(crate::stroke::build_stroke(&self.source, &curves, levels)) } else { None };
        self.build_drawings(&positions, &curves, stroke_geometry);
    }

    /// `buildDrawings` 0x00648d60.
    fn build_drawings(&mut self, positions: &[Vec2], curves: &Curves, stroke: Option<crate::stroke::StrokeGeometry>) {
        let flags = self.source.flags;
        let dirty = self.dirty;
        for d in [&mut self.fill, &mut self.stroke, &mut self.extrusion].into_iter().flatten() {
            d.flags = dirty;
            if flags & FLAG_CURVE != 0 && dirty & 1 != 0 {
                d.flags |= 8;
            }
        }
        self.dirty = 0;
        let s = &self.source;
        if let Some(fill) = self.fill.as_mut() {
            fill.curve_fill = flags & FLAG_CURVE != 0 && flags & FLAG_NO_FILL == 0;
            if s.subdivisions == 0 && flags & FLAG_CURVE == 0 {
                // Unsubdivided mesh: the key vertices and a triangle fan per face.
                fill.positions = positions.to_vec();
                fill.colors = s.vertex_colors.current.clone();
                fill.tex_coords = s.vertex_tex_coords.current.clone();
                let n = fill.positions.len();
                fill.colors.resize(n, [0.0; 4]);
                fill.tex_coords.resize(n, [0.0; 2]);
                fill.colors2.clear();
                fill.indices.clear();
                for face in &s.faces {
                    let m = face.len();
                    let mut k = 1;
                    while 1 < m.saturating_sub(1) && k < m - 1 {
                        fill.indices.extend_from_slice(&[face[0], face[k], face[k + 1]]);
                        k += 1;
                    }
                }
            }
            if s.subdivisions >= 0 && flags & FLAG_CURVE != 0 && flags & FLAG_NO_FILL == 0 {
                curve_fill(fill, curves);
            }
        }
        if flags & FLAG_EXTRUSION != 0 {
            if let Some(ex) = self.extrusion.as_mut() {
                ex.clear();
                for (ci, pts) in curves.positions.iter().enumerate() {
                    let mut contour = Vec::with_capacity(pts.len());
                    for (k, p) in pts.iter().enumerate() {
                        contour.push(ex.positions.len() as u32);
                        ex.positions.push(*p);
                        ex.colors.push(curves.extrusion_front[ci][k]);
                        ex.colors2.push(curves.extrusion_back[ci][k]);
                        ex.tex_coords.push(curves.tex_coords[ci][k]);
                    }
                    ex.contours.push(contour);
                }
                let m = s.extrusion_matrix.current;
                ex.tessellate_extrusion(&m);
                ex.flags = 0xf;
            }
        }
        if flags & FLAG_STROKE != 0 {
            if let (Some(st), Some(geom)) = (self.stroke.as_mut(), stroke) {
                crate::stroke::fill_stroke_drawing(st, &geom);
            }
        }
        for d in [&mut self.fill, &mut self.stroke, &mut self.extrusion].into_iter().flatten() {
            if d.flags & 8 != 0 {
                d.compute_outlines();
            }
            d.update();
            // D3D9Drawing::update 0x0068b7f0 rebuilds the VB/IB after this.
            d.buffers.invalidate();
        }
    }
}

/// The matrix both `SmoothMeshShape` 0x0066c050 (texture matrix) and
/// `Transformation::evaluate` 0x00678600 build, applied to the identity (0x00423e70):
/// translate by `t`, translate by the pivot `p`, rotate about x, y, z by `r` (degrees,
/// `cos`/`sin` of the float-rounded radians in double precision, as the
/// `libm_sse2_*_precise` calls), pre-multiply by `d`, translate by `-p`. Row-vector
/// convention (D3D): each step rewrites rows, so a point maps as
/// `p' = ((p - pivot)·D·R + pivot) + t`.
pub fn plasma_transform_matrix(t: Vec2, p: Vec2, r: [f32; 3], d: &Mat4) -> Mat4 {
    let mut m = identity();
    let add_row3 = |m: &mut Mat4, x: f32, y: f32| {
        m[12] = x * m[0] + y * m[4] + m[12];
        m[13] = m[5] * y + x * m[1] + m[13];
        m[14] = m[6] * y + x * m[2] + m[14];
        m[15] = m[7] * y + x * m[3] + m[15];
    };
    add_row3(&mut m, t[0], t[1]);
    add_row3(&mut m, p[0], p[1]);
    let cs = |deg: f32| {
        let a = deg * 0.017453292;
        ((a as f64).cos() as f32, (a as f64).sin() as f32)
    };
    // About x: rows 1 and 2.
    let (c, sn) = cs(r[0]);
    for k in 0..4 {
        let r1 = m[4 + k];
        m[4 + k] = m[8 + k] * sn + r1 * c;
        m[8 + k] = m[8 + k] * c - r1 * sn;
    }
    // About y: rows 0 and 2.
    let (c, sn) = cs(r[1]);
    for k in 0..4 {
        let r0 = m[k];
        m[k] = r0 * c - m[8 + k] * sn;
        m[8 + k] = m[8 + k] * c + r0 * sn;
    }
    // About z: rows 0 and 1.
    let (c, sn) = cs(r[2]);
    for k in 0..4 {
        let r0 = m[k];
        m[k] = r0 * c + sn * m[4 + k];
        m[4 + k] = c * m[4 + k] - r0 * sn;
    }
    // M = D * M, column by column.
    for k in 0..4 {
        let (c0, c1, c2, c3) = (m[k], m[4 + k], m[8 + k], m[12 + k]);
        m[k] = c1 * d[1] + c0 * d[0] + c2 * d[2] + d[3] * c3;
        m[4 + k] = d[5] * c1 + d[4] * c0 + c2 * d[6] + c3 * d[7];
        m[8 + k] = d[9] * c1 + d[8] * c0 + d[10] * c2 + c3 * d[11];
        m[12 + k] = c1 * d[13] + c0 * d[12] + d[14] * c2 + d[15] * c3;
    }
    add_row3(&mut m, p[0] * -1.0, p[1] * -1.0);
    m
}

fn sync_drawing(slot: &mut Option<Drawing>, wanted: bool) {
    match (wanted, slot.is_some()) {
        (true, false) => *slot = Some(Drawing::new()),
        (false, true) => *slot = None,
        _ => {}
    }
}

fn bounds_of(ps: &[Vec2]) -> (Vec2, Vec2) {
    let Some(first) = ps.first() else { return ([0.0; 2], [0.0; 2]) };
    let (mut lo, mut hi) = (*first, *first);
    for p in &ps[1..] {
        if p[0] < lo[0] {
            lo[0] = p[0];
        }
        if p[1] < lo[1] {
            lo[1] = p[1];
        }
        if hi[0] <= p[0] && p[0] != hi[0] {
            hi[0] = p[0];
        }
        if hi[1] <= p[1] && p[1] != hi[1] {
            hi[1] = p[1];
        }
    }
    (lo, hi)
}

impl Shape for SmoothMeshShape {
    fn rebuild(&mut self, full: bool) {
        self.rebuild_impl(full);
    }

    /// `draw` 0x00641280: extrusion first (flag bit 4), then fill (unless bit 2), then
    /// stroke (bit 1), each through the drawing's slot 2 with its engine display state.
    fn drawings(&self) -> Vec<(DrawingKind, &Drawing)> {
        let f = self.source.flags;
        let mut out = Vec::new();
        if let (Some(d), true) = (&self.extrusion, f & FLAG_EXTRUSION != 0) {
            out.push((DrawingKind::Extrusion, d));
        }
        if let (Some(d), true) = (&self.fill, f & FLAG_NO_FILL == 0) {
            out.push((DrawingKind::Fill, d));
        }
        if let (Some(d), true) = (&self.stroke, f & FLAG_STROKE != 0) {
            out.push((DrawingKind::Stroke, d));
        }
        out
    }

    /// `containsPoint` 0x00642430: fill, then stroke, then extrusion
    /// ([`Drawing::contains_point`] on each enabled one). Outlines are recomputed only when
    /// the drawing's dirty bits include bit 3 (a full rebuild, or curve mode), so after a
    /// partial rebuild of a mesh-mode shape the fill tests against stale outlines, as in
    /// the original.
    fn contains_point(&self, p: Vec2) -> bool {
        self.for_enabled(|d| d.contains_point(p))
    }

    /// `intersectsRect` 0x006424b0, same order with [`Drawing::intersects_circle`].
    fn intersects_circle(&self, p: Vec2, radius: f32, to_screen: &Mat4, to_local: &Mat4) -> bool {
        self.for_enabled(|d| d.intersects_circle(p, radius, to_screen, to_local))
    }

    /// `getBounds` 0x00641aa0: [`Drawing::expand_bounds`] on each enabled drawing.
    fn expand_bounds(&self, min: &mut Vec2, max: &mut Vec2, m: &Mat4, first: &mut bool) -> bool {
        let f = self.source.flags;
        if let (Some(d), true) = (&self.fill, f & FLAG_NO_FILL == 0) {
            d.expand_bounds(min, max, m, first);
        }
        if let (Some(d), true) = (&self.stroke, f & FLAG_STROKE != 0) {
            d.expand_bounds(min, max, m, first);
        }
        if let (Some(d), true) = (&self.extrusion, f & FLAG_EXTRUSION != 0) {
            d.expand_bounds(min, max, m, first);
        }
        true
    }

    /// MeshShape slot 6 0x00669080: +0x878 (key-position bounds minimum).
    fn position(&self) -> Vec2 {
        self.key_bounds.0
    }

    /// MeshShape slot 7 0x00669060: +0x880 (key-position bounds maximum; tentative).
    fn size(&self) -> Vec2 {
        self.key_bounds.1
    }

    /// MeshShape slots 10/11 (0x0066be80 / 0x0066b1f0) mark the position attribute
    /// changed; here: mark everything dirty for the next rebuild.
    fn invalidate(&mut self) {
        self.source.vertex_positions.changed = true;
    }

    /// `isEmpty` 0x00642410: no vertex records (+0xb30 == +0xb34).
    fn is_empty(&self) -> bool {
        self.source.vertex_positions.current.is_empty()
    }
}

impl SmoothMeshShape {
    fn for_enabled(&self, mut f: impl FnMut(&Drawing) -> bool) -> bool {
        let fl = self.source.flags;
        if let (Some(d), true) = (&self.fill, fl & FLAG_NO_FILL == 0) {
            if f(d) {
                return true;
            }
        }
        if let (Some(d), true) = (&self.stroke, fl & FLAG_STROKE != 0) {
            if f(d) {
                return true;
            }
        }
        if let (Some(d), true) = (&self.extrusion, fl & FLAG_EXTRUSION != 0) {
            if f(d) {
                return true;
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------------------
// Topology
// ---------------------------------------------------------------------------------------

/// `setFaces` 0x0066b200, the curve part: the boundary loops of the face set (+0x8d8).
///
/// A two-vertex face inserts both directions of its edge (a line segment becomes the loop
/// `[a, b]`); longer faces toggle each directed edge against its reverse. Loops chain as in
/// [`crate::drawing::chain_loops`]; with exactly one face, loops start at its first vertex
/// while it is available.
pub fn face_curves(faces: &[Vec<u32>]) -> Vec<Vec<u32>> {
    let mut set: BTreeSet<(u32, u32)> = BTreeSet::new();
    for f in faces {
        let n = f.len();
        if n < 2 {
            continue;
        }
        if n == 2 {
            set.insert((f[0], f[1]));
            set.insert((f[1], f[0]));
            continue;
        }
        for i in 0..n {
            let (a, b) = (f[i], f[(i + 1) % n]);
            if !set.remove(&(b, a)) {
                set.insert((a, b));
            }
        }
    }
    let mut succ: BTreeMap<u32, VecDeque<u32>> = BTreeMap::new();
    for f in faces {
        let n = f.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (a, b) = (f[i], f[(i + 1) % n]);
            if set.contains(&(a, b)) {
                succ.entry(a).or_default().push_back(b);
            }
        }
    }
    let prefer = if faces.len() == 1 { faces[0].first().copied() } else { None };
    chain_loops(succ, prefer)
}

/// 0x0066b9c0: apply the `vertexFlags` high-word constraints to the positions, curve by
/// curve (see [`constraint`]).
pub fn apply_vertex_constraints(pos: &mut [Vec2], curves: &[Vec<u32>], vflags: &[u32], params: &[f32]) {
    let hi = |v: u32| vflags.get(v as usize).copied().unwrap_or(0) & 0xffff_0000;
    let param = |v: u32| params.get(v as usize).copied().unwrap_or(0.0);
    for c in curves {
        let n = c.len();
        if n == 0 {
            continue;
        }
        for i in 0..n {
            let v = c[i];
            if hi(v) == 0x10000 {
                // Forward handle.
                let mut j = c[(i + 3) % n];
                if hi(j) != 0x10000 {
                    j = c[(i + 2) % n];
                }
                place_handle(pos, v, j, c[(i + 1) % n], param(v));
                // Backward handle.
                let mut j = c[(n - 3 + i) % n];
                if hi(j) != 0x10000 {
                    j = c[(n - 2 + i) % n];
                }
                place_handle(pos, v, j, c[(i + n - 1) % n], param(v));
            }
        }
        for k in 1..=n {
            let v = c[k - 1];
            if hi(v) != 0x20000 {
                continue;
            }
            let limit = n - 1 + k;
            let mut fwd = 0u32;
            let mut j = k;
            while j < limit {
                fwd = c[j % n];
                if hi(fwd) != 0x20000 {
                    break;
                }
                j += 1;
            }
            let mut back = 0u32;
            let mut j = limit;
            loop {
                if j == 0 || j - 1 < k {
                    break;
                }
                j -= 1;
                back = c[j % n];
                if hi(back) != 0x20000 {
                    break;
                }
            }
            let t = param(v);
            let pf = pos[fwd as usize];
            let pb = pos[back as usize];
            let lx = (1.0 - t) * pf[0];
            let ly = (1.0 - t) * pf[1];
            pos[v as usize] = [pb[0] * t + lx, ly + pb[1] * t];
        }
    }
}

fn place_handle(pos: &mut [Vec2], v: u32, j: u32, target: u32, dist: f32) {
    let pv = pos[v as usize];
    let pj = pos[j as usize];
    let dy = pj[1] - pv[1];
    let dx = pj[0] - pv[0];
    let l2 = dy * dy + dx * dx;
    if 0.0 < l2 {
        let s = dist / ((l2 as f64).sqrt() as f32);
        pos[target as usize] = [pv[0] + dx * s, pv[1] + dy * s];
    }
}

// ---------------------------------------------------------------------------------------
// Mesh subdivision (QuadMesh)
// ---------------------------------------------------------------------------------------

/// One `QuadVertex` record (0x34 bytes): value pointers (+0/+4/+8), accumulators
/// (+0xc..+0x28), accumulation count (+0x2c, also the list index while the topology is
/// built) and flags (+0x30).
///
/// Flags: bit 0 boundary, bit 1 fixed, bit 2 crease, bit 3 corner control point.
///
/// In the original the value pointers of every record reachable from the last level are
/// redirected into the fill drawing's arrays (0x00642ad0), so all levels share storage;
/// here the records live in one arena ([`QuadStore`]) and the last level's list is copied
/// into the drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuadVertex {
    pub pos: Vec2,
    pub col: Vec4,
    pub uv: Vec2,
    pub acc_pos: Vec2,
    pub acc_col: Vec4,
    pub acc_uv: Vec2,
    pub count: i32,
    pub flags: u8,
}

impl QuadVertex {
    fn reset(&mut self) {
        self.acc_pos = [0.0; 2];
        self.acc_col = [0.0; 4];
        self.acc_uv = [0.0; 2];
        self.count = 0;
    }

    fn accumulate(&mut self, p: Vec2, c: Vec4, u: Vec2) {
        self.count += 1;
        for k in 0..2 {
            self.acc_pos[k] = self.acc_pos[k] + p[k];
            self.acc_uv[k] = self.acc_uv[k] + u[k];
        }
        for k in 0..4 {
            self.acc_col[k] = self.acc_col[k] + c[k];
        }
    }
}

/// Every `QuadVertex` record of a shape: the base vertices (+0xb30), the polygon face
/// points (+0xb48), the polygon edge points (+0xb3c), and the points embedded in each
/// level's `QuadFace` (+0x10) and `QuadEdge` (+0x10).
pub type QuadStore = Vec<QuadVertex>;

/// A `QuadFace` (0x7c bytes): corner record pointers (+0..+0xc), the embedded face
/// point (+0x10, here an index into the [`QuadStore`]), the edge pointers (+0x44..+0x50,
/// edge `k` joins corners `k` and `k+1`) and the corners' list indices (+0x64..+0x70).
/// Children (+0x54) and the root face (+0x74) are implied by the child order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadFace {
    pub v: [usize; 4],
    pub edges: [usize; 4],
    pub fp: usize,
}

/// A `QuadEdge` (0x4c bytes): the two faces (+0/+4, null = missing), the two end records
/// (+8/+0xc) and the embedded edge point (+0x10).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadEdge {
    pub faces: [Option<usize>; 2],
    pub v: [usize; 2],
    pub ep: usize,
}

/// `plasma::QuadMesh` (0x60 bytes, one per level at SmoothMeshShape+0x8e4 + 0x60·i):
/// faces (+4, concurrent_vector of 0x7c), edges (+0x24, of 0x4c) and the vertex record
/// list (+0x44, pointers), all as indices into a [`QuadStore`].
///
/// Order (read from the builders, not assumed):
/// - Level 0 (`0x0063fec0` + `0x00671450`): one quad per polygon corner `k`, corners
///   `[face point, edge point of (f[k-1], f[k]), f[k], edge point of (f[k], f[k+1])]`;
///   edges in first-seen order over (face, corner) of `(q[k], q[k+1])`, a repeat sets the
///   second face (a third use overwrites it); the vertex list in first-seen order over
///   (face, corner).
/// - Level l+1 (`0x00671750`): child `k` of face `j` is face `4j+k` =
///   `[fp_j, ep(edge (k+3)%4), v_k, ep(edge k)]`; edges: first `4·faces` interior edges
///   (`4j+k` = (fp_j, ep(edge (k+3)%4)), faces `4j+k` and `4j+(k+3)%4`), then two per
///   parent edge `i` (`4F+2i+s` = (ep_i, v_s), faces: the child of each parent face whose
///   corner 2 is `v_s`); the vertex list is the parent's list, then the parent's edge
///   points in edge order, then its face points in face order.
/// - After each build, both ends of every edge with a missing face get flag bit 0; the
///   new edge and face points start with flags 0 (level 0 zeroes the whole list first).
#[derive(Clone, Debug, Default)]
pub struct QuadMesh {
    pub verts: Vec<usize>,
    pub faces: Vec<QuadFace>,
    pub edges: Vec<QuadEdge>,
}

impl QuadMesh {
    fn mark_boundary(&self, pts: &mut QuadStore) {
        for e in &self.edges {
            if e.faces[0].is_none() || e.faces[1].is_none() {
                pts[e.v[0]].flags |= 1;
                pts[e.v[1]].flags |= 1;
            }
        }
    }

    /// The level-0 part of `0x0063fec0` (the QuadFace/QuadEdge build) and `0x00671450`
    /// (vertex list, flag reset, boundary bits).
    fn level0(pts: &mut QuadStore, quads: &[[usize; 4]]) -> QuadMesh {
        let mut m = QuadMesh::default();
        let mut edge_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for (j, q) in quads.iter().enumerate() {
            let fp = pts.len();
            pts.push(QuadVertex::default());
            let mut edges = [0usize; 4];
            for k in 0..4 {
                let (a, b) = (q[k], q[(k + 1) % 4]);
                match edge_of.get(&(a, b)) {
                    Some(&e) => {
                        m.edges[e].faces[1] = Some(j);
                        edges[k] = e;
                    }
                    None => {
                        let e = m.edges.len();
                        let ep = pts.len();
                        pts.push(QuadVertex::default());
                        m.edges.push(QuadEdge { faces: [Some(j), None], v: [a, b], ep });
                        edge_of.insert((a, b), e);
                        edge_of.insert((b, a), e);
                        edges[k] = e;
                    }
                }
            }
            m.faces.push(QuadFace { v: *q, edges, fp });
        }
        m.mark_boundary(pts);
        // 0x00671450: list in first-seen order; flags zeroed; boundary bits again.
        let mut seen = BTreeSet::new();
        for f in &m.faces {
            for &v in &f.v {
                if seen.insert(v) {
                    m.verts.push(v);
                }
            }
        }
        for &v in &m.verts {
            pts[v].flags = 0;
        }
        m.mark_boundary(pts);
        m
    }

    /// `0x00671750`: the next level's topology from this one.
    fn child(&self, pts: &mut QuadStore) -> QuadMesh {
        let nf = self.faces.len();
        let mut m = QuadMesh::default();
        // A face slot a degenerate parent left unset (an edge used by three faces; the
        // original would dereference null) falls back to the face point.
        let ep_of = |f: &QuadFace, i: usize| self.edges.get(f.edges[i]).map_or(f.fp, |e| e.ep);
        for f in &self.faces {
            for k in 0..4 {
                let fp = pts.len();
                pts.push(QuadVertex::default());
                let prev = ep_of(f, (k + 3) % 4);
                let next = ep_of(f, k);
                m.faces.push(QuadFace { v: [f.fp, prev, f.v[k], next], edges: [usize::MAX; 4], fp });
            }
        }
        for (j, f) in self.faces.iter().enumerate() {
            for k in 0..4 {
                let e = m.edges.len();
                let ep = pts.len();
                pts.push(QuadVertex::default());
                let a = 4 * j + k;
                let b = 4 * j + (k + 3) % 4;
                m.edges.push(QuadEdge {
                    faces: [Some(a), Some(b)],
                    v: [f.fp, ep_of(f, (k + 3) % 4)],
                    ep,
                });
                m.faces[a].edges[0] = e;
                m.faces[b].edges[3] = e;
            }
        }
        for pe in &self.edges {
            for s in 0..2 {
                let e = m.edges.len();
                let ep = pts.len();
                pts.push(QuadVertex::default());
                let (epp, v) = (pe.ep, pe.v[s]);
                let mut faces = [None, None];
                for (slot, pf) in pe.faces.iter().enumerate() {
                    let Some(pf) = *pf else { continue };
                    for c in 4 * pf..4 * pf + 4 {
                        if m.faces[c].v[2] == v {
                            faces[slot] = Some(c);
                            if m.faces[c].v[1] == epp {
                                m.faces[c].edges[1] = e;
                            }
                            if m.faces[c].v[3] == epp {
                                m.faces[c].edges[2] = e;
                            }
                        }
                    }
                }
                m.edges.push(QuadEdge { faces, v: [epp, v], ep });
            }
        }
        debug_assert_eq!(m.edges.len(), 4 * nf + 2 * self.edges.len());
        m.verts = self.verts.clone();
        for e in &self.edges {
            pts[e.ep].flags = 0;
            m.verts.push(e.ep);
        }
        for f in &self.faces {
            pts[f.fp].flags = 0;
            m.verts.push(f.fp);
        }
        m.mark_boundary(pts);
        m
    }

    /// `QuadMesh::subdivisionStep` 0x00671f80 with weight `t` (`smoothWeight`): computes
    /// this level's face and edge points and moves its vertices. The seven passes run
    /// serially in the original's element order; the parallel ones write only their own
    /// element, so their order does not change the result, and the two serial
    /// accumulation passes keep the edge and face order.
    pub fn step(&self, pts: &mut QuadStore, t: f32) {
        let one_t = 1.0 - t;
        // 1. reset accumulators (inline in 0x0066dcb0).
        for &v in &self.verts {
            pts[v].reset();
        }
        // 2. face points (0x0066ed80).
        for f in &self.faces {
            let [a, b, c, d] = f.v.map(|i| pts[i]);
            let fp = &mut pts[f.fp];
            fp.pos = [
                (a.pos[0] + b.pos[0] + c.pos[0] + d.pos[0]) * 0.25,
                (d.pos[1] + c.pos[1] + b.pos[1] + a.pos[1]) * 0.25,
            ];
            fp.col = [
                (a.col[0] + b.col[0] + c.col[0] + d.col[0]) * 0.25,
                (d.col[1] + c.col[1] + b.col[1] + a.col[1]) * 0.25,
                (d.col[2] + c.col[2] + b.col[2] + a.col[2]) * 0.25,
                (d.col[3] + c.col[3] + b.col[3] + a.col[3]) * 0.25,
            ];
            fp.uv = [
                (d.uv[0] + a.uv[0] + b.uv[0] + c.uv[0]) * 0.25,
                (d.uv[1] + c.uv[1] + b.uv[1] + a.uv[1]) * 0.25,
            ];
        }
        // 3. edge points (0x0066eef0), weights (1-t, t).
        for e in &self.edges {
            let v0 = pts[e.v[0]];
            let v1 = pts[e.v[1]];
            let mut flags = pts[e.ep].flags & 0xf9;
            let (pos, col, uv);
            match e.faces {
                [Some(fa), Some(fb)] if v0.flags & 2 == 0 && v1.flags & 2 == 0 => {
                    let a = pts[self.faces[fa].fp];
                    let b = pts[self.faces[fb].fp];
                    pos = std::array::from_fn(|k| ((v0.pos[k] + v1.pos[k]) * one_t + (a.pos[k] + b.pos[k]) * t) * 0.5);
                    col = std::array::from_fn(|k| ((a.col[k] + b.col[k]) * t + (v0.col[k] + v1.col[k]) * one_t) * 0.5);
                    uv = std::array::from_fn(|k| ((v0.uv[k] + v1.uv[k]) * one_t + (a.uv[k] + b.uv[k]) * t) * 0.5);
                }
                _ => {
                    pos = std::array::from_fn(|k| (v0.pos[k] + v1.pos[k]) * 0.5);
                    col = std::array::from_fn(|k| (v0.col[k] + v1.col[k]) * 0.5);
                    uv = std::array::from_fn(|k| (v0.uv[k] + v1.uv[k]) * 0.5);
                    if v0.flags & 4 != 0 || v1.flags & 4 != 0 {
                        flags |= 4;
                    }
                }
            }
            let ep = &mut pts[e.ep];
            ep.pos = pos;
            ep.col = col;
            ep.uv = uv;
            ep.flags = flags;
        }
        // 4. edge accumulation, serial (0x0066cc10 -> 0x0066ec30): the plain midpoint of
        //    the edge's end vertices.
        for e in &self.edges {
            let v0 = pts[e.v[0]];
            let v1 = pts[e.v[1]];
            let mp: Vec2 = std::array::from_fn(|k| (v0.pos[k] + v1.pos[k]) * 0.5);
            let mc: Vec4 = std::array::from_fn(|k| (v0.col[k] + v1.col[k]) * 0.5);
            let mu: Vec2 = std::array::from_fn(|k| (v0.uv[k] + v1.uv[k]) * 0.5);
            let open = e.faces[0].is_none() || e.faces[1].is_none();
            for &vi in &e.v {
                let v = &mut pts[vi];
                if v.flags & 1 == 0 || open {
                    v.accumulate(mp, mc, mu);
                }
            }
        }
        // 5. face accumulation, serial (0x0066cc90 -> 0x0066f1d0).
        for f in &self.faces {
            let fp = pts[f.fp];
            for &vi in &f.v {
                let v = &mut pts[vi];
                if v.flags & 1 == 0 {
                    v.accumulate(fp.pos, fp.col, fp.uv);
                }
            }
        }
        // 6. vertex positions (0x0066f380), weights (t, 1-t).
        for &vi in &self.verts {
            let v = &mut pts[vi];
            if v.flags & 2 != 0 || v.count == 0 {
                continue;
            }
            let cnt = v.count as f32;
            if v.flags & 1 == 0 {
                let s = 0.75 / cnt;
                for k in 0..2 {
                    v.pos[k] = v.pos[k] * 0.25 + v.acc_pos[k] * s;
                    v.uv[k] = v.uv[k] * 0.25 + v.acc_uv[k] * s;
                }
                for k in 0..4 {
                    v.col[k] = v.col[k] * 0.25 + v.acc_col[k] * s;
                }
            } else if v.flags & 4 == 0 {
                let s = t / cnt;
                for k in 0..2 {
                    v.pos[k] = v.pos[k] * one_t + v.acc_pos[k] * s;
                    v.uv[k] = v.uv[k] * one_t + v.acc_uv[k] * s;
                }
                for k in 0..4 {
                    v.col[k] = v.col[k] * one_t + v.acc_col[k] * s;
                }
            } else {
                let s = 1.0 / cnt;
                for k in 0..2 {
                    v.pos[k] = v.acc_pos[k] * s;
                    v.uv[k] = v.acc_uv[k] * s;
                }
                for k in 0..4 {
                    v.col[k] = v.acc_col[k] * s;
                }
            }
        }
        // 7. edge finish (0x0066f280): crease edges on the boundary follow the moved ends.
        for e in &self.edges {
            let v0 = pts[e.v[0]];
            let v1 = pts[e.v[1]];
            let open = e.faces[0].is_none() || e.faces[1].is_none();
            if open && v0.flags & 4 != 0 && v1.flags & 4 != 0 {
                let ep = &mut pts[e.ep];
                ep.pos = std::array::from_fn(|k| (v1.pos[k] + v0.pos[k]) * 0.5);
                ep.col = std::array::from_fn(|k| (v1.col[k] + v0.col[k]) * 0.5);
                ep.uv = std::array::from_fn(|k| (v1.uv[k] + v0.uv[k]) * 0.5);
                ep.flags = (ep.flags & 0xfb) | 2;
            }
        }
    }
}

/// 1/(2cos(π/4) + 1) = 1/(1+√2), the arc weight of the corner rule (static at 0x0076de74,
/// computed from the float-rounded π/4 through double `cos`).
fn corner_weight() -> f32 {
    let quarter = 0.7853981852531433f64; // (float)(π/4)
    1.0 / ((quarter.cos() as f32) * 2.0 + 1.0)
}

/// The weight of `a` in the first-level edge point `a*w + b*(1-w)` from the corner bits.
fn corner_split(a_corner: bool, b_corner: bool, c: f32) -> f32 {
    match (a_corner, b_corner) {
        (true, false) => c,
        (false, true) => 1.0 - c,
        _ => 0.5,
    }
}

/// The polygon topology `0x0063fec0` derives from the faces (+0x86c): the unique edge
/// list (+0xb60, `(face, edge)` in first-seen order over faces with ≥ 3 vertices) and the
/// adjacency (+0xb54, per face and edge the face across it or -1).
///
/// Adjacency: every polygon (≥ 3 vertices) registers each reversed edge
/// `(f[k+1], f[k]) → face` (a later face overwrites an earlier one); each face of any size
/// then looks its edges `(f[k], f[k+1])` up. Two-vertex faces are looked up too, so a
/// segment lying on a polygon edge running the other way gets that polygon as neighbour.
pub fn polygon_topology(faces: &[Vec<u32>]) -> (Vec<(usize, usize)>, Vec<Vec<i64>>) {
    let mut unique = Vec::new();
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n <= 2 {
            continue;
        }
        for e in 0..n {
            let (a, b) = (f[e], f[(e + 1) % n]);
            if !seen.contains(&(a, b)) {
                seen.insert((a, b));
                seen.insert((b, a));
                unique.push((fi, e));
            }
        }
    }
    let mut across: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n <= 2 {
            continue;
        }
        for k in 0..n {
            across.insert((f[(k + 1) % n], f[k]), fi);
        }
    }
    let adj = faces
        .iter()
        .map(|f| {
            let n = f.len();
            (0..n).map(|k| across.get(&(f[k], f[(k + 1) % n])).map_or(-1, |&x| x as i64)).collect()
        })
        .collect();
    (unique, adj)
}

/// `QuadMesh::subdivide` 0x00642ad0 over the topology of `0x0063fec0`/`0x00671450`/
/// `0x00671750`: the polygon → quad first level (with the corner and crease rules), then
/// `levels - 1` [`QuadMesh::step`]s, written into the fill drawing in the last level's
/// vertex-list order (indices `[a, b, c, c, d, a]` per face of the last level).
///
/// With `levels - 1 < 0`, no faces or no vertices, the drawing is emptied.
pub fn subdivide_mesh(fill: &mut Drawing, s: &ShapeSource, positions: &[Vec2], levels: i32, t: f32) {
    fill.positions.clear();
    fill.colors.clear();
    fill.tex_coords.clear();
    fill.indices.clear();
    let nv = positions.len();
    if levels - 1 < 0 || s.faces.is_empty() || nv == 0 {
        return;
    }
    let one_t = 1.0 - t;
    let c = corner_weight();
    let faces = &s.faces;
    let (unique, adj) = polygon_topology(faces);

    // Records: base vertices, polygon face points, polygon edge points.
    let fp_base = nv;
    let ep_base = nv + faces.len();
    let mut pts: QuadStore = vec![QuadVertex::default(); ep_base + unique.len()];
    let mut edge_index: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (ui, &(fi, e)) in unique.iter().enumerate() {
        let f = &faces[fi];
        let (a, b) = (f[e], f[(e + 1) % f.len()]);
        edge_index.insert((a, b), ep_base + ui);
        edge_index.insert((b, a), ep_base + ui);
    }

    // --- topology (0x0063fec0, built by setFaces/setSubdivisions in the original) ---
    let mut quads: Vec<[usize; 4]> = Vec::new();
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n <= 2 {
            continue;
        }
        for k in 0..n {
            let v = f[k];
            quads.push([
                fp_base + fi,
                edge_index[&(v, f[(k + n - 1) % n])],
                v as usize,
                edge_index[&(v, f[(k + 1) % n])],
            ]);
        }
    }
    // The polygon edge points of boundary edges (e[1] == 0 among the level-0 quad edges
    // touching them) get bit 0 from `level0`.
    let mut meshes = vec![QuadMesh::level0(&mut pts, &quads)];
    for _ in 1..levels {
        let next = meshes.last().unwrap().child(&mut pts);
        meshes.push(next);
    }

    // --- values (0x00642ad0) ---
    // Base records: accumulators and flags cleared, boundary bits from the adjacency.
    for v in &mut pts[..nv] {
        v.reset();
        v.flags = 0;
    }
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        for e in 0..n {
            if adj[fi][e] < 0 {
                pts[f[e] as usize].flags |= 1;
                pts[f[(e + 1) % n] as usize].flags |= 1;
            }
        }
    }
    for i in 0..nv {
        let v = &mut pts[i];
        v.pos = positions[i];
        v.col = s.vertex_colors.current.get(i).copied().unwrap_or_default();
        v.uv = s.vertex_tex_coords.current.get(i).copied().unwrap_or_default();
        match s.vertex_flags.get(i).map(|f| (*f & 0xffff) as u16) {
            Some(kind::FIXED) => v.flags |= 2,
            Some(kind::CREASE) => v.flags |= 4,
            Some(kind::CORNER) => v.flags |= 0xc,
            _ => {}
        }
    }
    // Face points (+0xb48): the average of each polygon (≥ 3 vertices).
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n <= 2 {
            continue;
        }
        let mut sp = [0.0f32; 2];
        let mut sc = [0.0f32; 4];
        let mut su = [0.0f32; 2];
        for &vi in f {
            let v = &pts[vi as usize];
            for k in 0..2 {
                sp[k] += v.pos[k];
            }
            for k in 0..4 {
                sc[k] += v.col[k];
            }
            su[1] = v.uv[1] + su[1];
            su[0] += v.uv[0];
        }
        let inv = 1.0 / n as f32;
        let fp = &mut pts[fp_base + fi];
        fp.pos = sp.map(|x| x * inv);
        fp.col = sc.map(|x| x * inv);
        fp.uv = su.map(|x| x * inv);
    }
    // Edge points (+0xb3c).
    for (ui, &(fi, e)) in unique.iter().enumerate() {
        let f = &faces[fi];
        let n = f.len();
        let v0 = pts[f[e] as usize];
        let v1 = pts[f[(e + 1) % n] as usize];
        let nb = adj[fi][e];
        let a = pts[fp_base + fi];
        let b = if nb >= 0 { pts[fp_base + nb as usize] } else { QuadVertex::default() };
        let ep = &mut pts[ep_base + ui];
        ep.flags &= 0xf1;
        if nb < 0 || v0.flags & 2 != 0 || v1.flags & 2 != 0 {
            let w = corner_split(v0.flags & 8 != 0, v1.flags & 8 != 0, c);
            let w1 = 1.0 - w;
            ep.pos = std::array::from_fn(|k| v0.pos[k] * w + v1.pos[k] * w1);
            ep.col = std::array::from_fn(|k| v0.col[k] * w + v1.col[k] * w1);
            ep.uv = std::array::from_fn(|k| v0.uv[k] * w + v1.uv[k] * w1);
            if v0.flags & 4 != 0 || v1.flags & 4 != 0 {
                ep.flags |= 4;
            }
        } else {
            ep.pos = std::array::from_fn(|k| ((v0.pos[k] + v1.pos[k]) * one_t + (a.pos[k] + b.pos[k]) * t) * 0.5);
            ep.col = std::array::from_fn(|k| ((v0.col[k] + v1.col[k]) * one_t + (a.col[k] + b.col[k]) * t) * 0.5);
            ep.uv = std::array::from_fn(|k| ((v0.uv[k] + v1.uv[k]) * one_t + (a.uv[k] + b.uv[k]) * t) * 0.5);
        }
    }
    // Vertex accumulation from every polygon edge (interior edges twice, once per face).
    for (fi, f) in faces.iter().enumerate() {
        let n = f.len();
        if n <= 2 {
            continue;
        }
        for e in 0..n {
            let ia = f[e] as usize;
            let ib = f[(e + 1) % n] as usize;
            let (a, b) = (pts[ia], pts[ib]);
            let w = corner_split(a.flags & 8 != 0, b.flags & 8 != 0, c);
            let w1 = 1.0 - w;
            let mp: Vec2 = std::array::from_fn(|k| a.pos[k] * w + b.pos[k] * w1);
            let mc: Vec4 = std::array::from_fn(|k| a.col[k] * w + b.col[k] * w1);
            let mu: Vec2 = std::array::from_fn(|k| a.uv[k] * w + b.uv[k] * w1);
            let boundary = adj[fi][e] < 0;
            if boundary || a.flags & 1 == 0 {
                pts[ia].accumulate(mp, mc, mu);
            }
            if boundary || b.flags & 1 == 0 {
                pts[ib].accumulate(mp, mc, mu);
            }
        }
    }
    // Face accumulation.
    for (fi, f) in faces.iter().enumerate() {
        let fp = pts[fp_base + fi];
        for &vi in f {
            let v = &mut pts[vi as usize];
            if v.flags & 1 == 0 {
                v.accumulate(fp.pos, fp.col, fp.uv);
            }
        }
    }
    // Vertex update (first level: 0.25/0.75 for smooth vertices).
    for v in &mut pts[..nv] {
        if v.flags & 2 != 0 || v.count == 0 {
            continue;
        }
        let inv = 1.0 / v.count as f32;
        if v.flags & 1 == 0 {
            for k in 0..2 {
                v.pos[k] = v.pos[k] * 0.25 + inv * v.acc_pos[k] * 0.75;
                v.uv[k] = v.uv[k] * 0.25 + inv * v.acc_uv[k] * 0.75;
            }
            for k in 0..4 {
                v.col[k] = v.col[k] * 0.25 + inv * v.acc_col[k] * 0.75;
            }
        } else if v.flags & 4 == 0 {
            for k in 0..2 {
                v.pos[k] = one_t * v.pos[k] + inv * t * v.acc_pos[k];
                v.uv[k] = v.uv[k] * one_t + inv * v.acc_uv[k] * t;
            }
            for k in 0..4 {
                v.col[k] = v.col[k] * one_t + inv * t * v.acc_col[k];
            }
        } else {
            for k in 0..2 {
                v.pos[k] = inv * v.acc_pos[k];
                v.uv[k] = inv * v.acc_uv[k];
            }
            for k in 0..4 {
                v.col[k] = inv * v.acc_col[k];
            }
        }
    }
    // Edge finish for crease boundary edges.
    for (ui, &(fi, e)) in unique.iter().enumerate() {
        let f = &faces[fi];
        let n = f.len();
        let v0 = pts[f[e] as usize];
        let v1 = pts[f[(e + 1) % n] as usize];
        if adj[fi][e] < 0 && v0.flags & 4 != 0 && v1.flags & 4 != 0 {
            let ep = &mut pts[ep_base + ui];
            ep.pos = std::array::from_fn(|k| (v0.pos[k] + v1.pos[k]) * 0.5);
            ep.col = std::array::from_fn(|k| (v1.col[k] + v0.col[k]) * 0.5);
            ep.uv = std::array::from_fn(|k| (v1.uv[k] + v0.uv[k]) * 0.5);
            ep.flags = (ep.flags & 0xfb) | 2;
        }
    }
    // Corner control points are fixed from here on.
    for v in &mut pts[..nv] {
        if v.flags & 8 != 0 {
            v.flags = (v.flags & 0xf3) | 2;
        }
    }
    // `levels - 1` steps on meshes 0..levels-2.
    for m in &meshes[..meshes.len() - 1] {
        m.step(&mut pts, t);
    }
    // Write the last level (its vertex list) into the drawing.
    let last = meshes.last().unwrap();
    fill.positions = last.verts.iter().map(|&v| pts[v].pos).collect();
    fill.colors = last.verts.iter().map(|&v| pts[v].col).collect();
    fill.tex_coords = last.verts.iter().map(|&v| pts[v].uv).collect();
    let mut index_of = vec![u32::MAX; pts.len()];
    for (i, &v) in last.verts.iter().enumerate() {
        index_of[v] = i as u32;
    }
    for f in &last.faces {
        let [a, b, c2, d] = f.v.map(|i| index_of[i]);
        fill.indices.extend_from_slice(&[a, b, c2, c2, d, a]);
    }
}


// ---------------------------------------------------------------------------------------
// Curves
// ---------------------------------------------------------------------------------------

/// The subdivided outline curves (+0xba8 positions, +0xb6c colours, +0xbb4 uvs, +0xbfc
/// kinds, +0xb78 stroke colours, +0xb9c stroke widths, +0xb84/+0xb90 extrusion colours).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curves {
    pub positions: Vec<Vec<Vec2>>,
    pub colors: Vec<Vec<Vec4>>,
    pub tex_coords: Vec<Vec<Vec2>>,
    pub kinds: Vec<Vec<u32>>,
    pub stroke_colors: Vec<Vec<Vec4>>,
    pub stroke_widths: Vec<Vec<f32>>,
    pub extrusion_front: Vec<Vec<Vec4>>,
    pub extrusion_back: Vec<Vec<Vec4>>,
    pub closed: bool,
}

/// A value the dyadic curve subdivision can blend.
pub trait CurveValue: Copy + Default {
    fn mix(a: Self, wa: f32, b: Self) -> Self;
    fn mid(a: Self, b: Self) -> Self;
    fn relax(p: Self, m: Self, t: f32) -> Self;
}

impl<const N: usize> CurveValue for [f32; N]
where
    [f32; N]: Default,
{
    /// `a*w + (1-w)*b`.
    fn mix(a: Self, w: f32, b: Self) -> Self {
        std::array::from_fn(|k| a[k] * w + (1.0 - w) * b[k])
    }
    fn mid(a: Self, b: Self) -> Self {
        std::array::from_fn(|k| (a[k] + b[k]) * 0.5)
    }
    /// `p + (m - p) * t`.
    fn relax(p: Self, m: Self, t: f32) -> Self {
        std::array::from_fn(|k| p[k] + (m[k] - p[k]) * t)
    }
}

impl CurveValue for f32 {
    fn mix(a: Self, w: f32, b: Self) -> Self {
        (1.0 - w) * b + a * w
    }
    fn mid(a: Self, b: Self) -> Self {
        (a + b) * 0.5
    }
    fn relax(p: Self, m: Self, t: f32) -> Self {
        (m - p) * t + p
    }
}

/// One level of the dyadic curve subdivision: 0x0063ad70 (Vector2), 0x0063b360
/// (Vector4), 0x0063a980 (float). The array holds the curve at spacing `step` (points at
/// multiples of `step`, `count` segments); this fills the midpoints at `step/2`.
///
/// `level` is the level index: level 0 applies the corner rule (a midpoint next to a
/// [`kind::CORNER`] point is pulled to the 1/(1+√2) arc position, and corner points are
/// re-centred between their new neighbours). Later levels treat corners as plain points.
///
/// 1. Midpoints: `mid = p[a]*w + (1-w)*p[b]`; the midpoint's kind becomes CREASE when an
///    end is a crease (or, at level 0, a corner), else 0.
/// 2. Old points (interior only when open; all when closed, point 0 last): FIXED stay,
///    kind 0 relax towards the mean of their new neighbours by `t`, CORNER (level 0) and
///    other kinds move onto that mean.
/// 3. Segments whose both ends are creases (or corners at level 0) get their midpoint
///    reset to the plain mean of the (moved) ends, kind FIXED.
pub fn subdivide_curve<T: CurveValue>(p: &mut [T], kinds: &mut [u32], step: usize, count: usize, closed: bool, t: f32, level: u32) {
    let n = p.len();
    let nk = kinds.len();
    if n == 0 || nk == 0 {
        return;
    }
    let h = step / 2;
    let c = corner_weight();
    let k16 = |kinds: &[u32], i: usize| (kinds[i % nk] & 0xffff) as u16;
    for s in 0..count {
        let a = s * step;
        let b = a + step;
        let m = a + h;
        p[m % n] = T::mid(p[b % n], p[a % n]);
        let mut w = 0.5;
        let ka = k16(kinds, a);
        let kb = k16(kinds, b);
        if level == 0 {
            if ka == kind::CORNER && kb != ka {
                w = c;
            }
            if kb == kind::CORNER && ka != kind::CORNER {
                w = 1.0 - c;
            }
        }
        p[m % n] = T::mix(p[a % n], w, p[b % n]);
        let crease = ka == kind::CREASE
            || kb == kind::CREASE
            || (level == 0 && (ka == kind::CORNER || kb == kind::CORNER));
        kinds[m % nk] = if crease { 2 } else { 0 };
    }
    let total = if closed { count + 1 } else { count };
    if total > 1 {
        for s in 1..total {
            let i = s * step;
            let kd = k16(kinds, i);
            let nb = T::mid(p[(i + h) % n], p[(i - h) % n]);
            match kd {
                kind::FIXED => {}
                kind::CORNER => {
                    if level == 0 {
                        p[i % n] = nb;
                    }
                }
                0 => p[i % n] = T::relax(p[i % n], nb, t),
                _ => p[i % n] = nb,
            }
        }
    }
    for s in 0..total {
        let a = s * step;
        let b = a + step;
        let ka = k16(kinds, a);
        let kb = k16(kinds, b);
        let hard = |k: u16| k == kind::CREASE || (level == 0 && k == kind::CORNER);
        if hard(ka) && hard(kb) {
            let m = a + h;
            p[m % n] = T::mid(p[b % n], p[a % n]);
            kinds[m % nk] = 1;
        }
    }
}

/// 0x0063bba0: the same without kinds (extrusion colours): midpoints, relax every old
/// point by `t`, midpoints again.
pub fn subdivide_curve_plain(p: &mut [Vec4], step: usize, count: usize, closed: bool, t: f32) {
    let n = p.len();
    if n == 0 {
        return;
    }
    let h = step / 2;
    for s in 0..count {
        let a = s * step;
        p[(a + h) % n] = <Vec4 as CurveValue>::mid(p[(a + step) % n], p[a % n]);
    }
    let total = if closed { count + 1 } else { count };
    if total > 1 {
        for s in 1..total {
            let i = s * step;
            let nb = <Vec4 as CurveValue>::mid(p[(i + h) % n], p[(i - h) % n]);
            p[i % n] = <Vec4 as CurveValue>::relax(p[i % n], nb, t);
        }
    }
    for s in 0..total {
        let a = s * step;
        p[(a + h) % n] = <Vec4 as CurveValue>::mid(p[(a + step) % n], p[a % n]);
    }
}

/// The curve part of `rebuild` 0x00644fa0: lay each outline curve out at stride 2^L
/// (closed: n·2^L points, open: (n-1)·2^L + 1), then subdivide level by level
/// (positions, uvs, colours, then stroke colours/widths and extrusion colours when
/// enabled; all per-point arrays share one kinds array per curve).
pub fn build_curves(s: &ShapeSource, curves: &[Vec<u32>], positions: &[Vec2], levels: u32) -> Curves {
    let flags = s.flags;
    let closed = flags & FLAG_OPEN == 0;
    let stride = 1usize << levels;
    let mut out = Curves { closed, ..Default::default() };
    let stroke = flags & FLAG_STROKE != 0;
    let extrude = flags & FLAG_EXTRUSION != 0;
    for c in curves {
        let n = c.len();
        let len = if closed { n * stride } else { (n.saturating_sub(1)) * stride + 1 };
        let mut pos = vec![[0.0f32; 2]; len];
        let mut col = vec![[0.0f32; 4]; len];
        let mut uv = vec![[0.0f32; 2]; len];
        let mut kinds = vec![0u32; len];
        let mut scol = vec![[0.0f32; 4]; if stroke { len } else { 0 }];
        let mut sw = vec![0.0f32; if stroke { len } else { 0 }];
        let mut ef = vec![[0.0f32; 4]; if extrude { len } else { 0 }];
        let mut eb = vec![[0.0f32; 4]; if extrude { len } else { 0 }];
        for (k, &v) in c.iter().enumerate() {
            let i = k * stride;
            if i >= len {
                break;
            }
            let v = v as usize;
            pos[i] = positions.get(v).copied().unwrap_or_default();
            col[i] = s.vertex_colors.current.get(v).copied().unwrap_or_default();
            uv[i] = s.vertex_tex_coords.current.get(v).copied().unwrap_or_default();
            kinds[i] = s.vertex_flags.get(v).copied().unwrap_or(0);
            if stroke {
                scol[i] = s.stroke_colors.current.get(v).copied().unwrap_or_default();
                let w = s.stroke_widths.current.get(v).copied().unwrap_or(0.0);
                sw[i] = if w <= 0.1 && w != 0.1 { 0.1 } else { w };
            }
            if extrude {
                ef[i] = s.extrusion_front_colors.current.get(v).copied().unwrap_or_default();
                eb[i] = s.extrusion_back_colors.current.get(v).copied().unwrap_or_default();
            }
        }
        for level in 0..levels {
            let step = 1usize << (levels - level);
            let count = len / step;
            let t = s.smooth_weight;
            subdivide_curve(&mut pos, &mut kinds, step, count, closed, t, level);
            subdivide_curve(&mut uv, &mut kinds, step, count, closed, t, level);
            subdivide_curve(&mut col, &mut kinds, step, count, closed, t, level);
            if stroke {
                subdivide_curve(&mut scol, &mut kinds, step, count, closed, t, level);
                subdivide_curve(&mut sw, &mut kinds, step, count, closed, t, level);
            }
            if extrude {
                subdivide_curve_plain(&mut ef, step, count, closed, t);
                subdivide_curve_plain(&mut eb, step, count, closed, t);
            }
        }
        out.positions.push(pos);
        out.colors.push(col);
        out.tex_coords.push(uv);
        out.kinds.push(kinds);
        out.stroke_colors.push(scol);
        out.stroke_widths.push(sw);
        out.extrusion_front.push(ef);
        out.extrusion_back.push(eb);
    }
    out
}

/// The curve-fill branch of `buildDrawings`: one contour per curve, colours from a
/// least-squares linear gradient fitted to the curve colours, renormalised into [0,1]
/// per channel, then [`Drawing::tessellate`].
///
/// Fit: with means (x̄, ȳ, c̄) over all curve points and the sums Sxx, Syy, Sxy, Sxc_k,
/// Syc_k of centred products, `a_k = (Sxc_k·Syy - Syc_k·Sxy)/det`,
/// `b_k = (Syc_k·Sxx - Sxc_k·Sxy)/det`, `det = Syy·Sxx - Sxy²`, and
/// `fit_k(p) = c̄_k + dx·a_k + dy·b_k`. Each channel is then mapped by
/// `(fit - lo)/(hi - lo)` with `lo = min(0, fits)`, `hi = max(1, fits)`. A degenerate
/// (collinear) outline gives det = 0 and non-finite colours, as in the original.
pub fn curve_fill(fill: &mut Drawing, curves: &Curves) {
    fill.clear();
    let mut total = 0i32;
    let mut mc = [0.0f32; 4];
    let (mut mx, mut my) = (0.0f32, 0.0f32);
    for (ci, pts) in curves.positions.iter().enumerate() {
        for (k, p) in pts.iter().enumerate() {
            let c = curves.colors[ci][k];
            for j in 0..4 {
                mc[j] += c[j];
            }
            mx += p[0];
            my += p[1];
            total += 1;
        }
    }
    let inv = 1.0 / total as f32;
    for j in 0..4 {
        mc[j] *= inv;
    }
    mx *= inv;
    my *= inv;
    let (mut sxx, mut syy, mut sxy) = (0.0f32, 0.0f32, 0.0f32);
    let mut sxc = [0.0f32; 4];
    let mut syc = [0.0f32; 4];
    for (ci, pts) in curves.positions.iter().enumerate() {
        for (k, p) in pts.iter().enumerate() {
            let c = curves.colors[ci][k];
            let dx = p[0] - mx;
            let dy = p[1] - my;
            sxx = dx * dx + sxx;
            syy = dy * dy + syy;
            sxy = dy * dx + sxy;
            for j in 0..4 {
                let dc = c[j] - mc[j];
                sxc[j] = sxc[j] + dx * dc;
                syc[j] = syc[j] + dy * dc;
            }
        }
    }
    let det = syy * sxx - sxy * sxy;
    let a: [f32; 4] = std::array::from_fn(|j| (sxc[j] * syy - syc[j] * sxy) / det);
    let b: [f32; 4] = std::array::from_fn(|j| (syc[j] * sxx - sxc[j] * sxy) / det);
    let fit = |p: Vec2| -> Vec4 {
        let dx = p[0] - mx;
        let dy = p[1] - my;
        std::array::from_fn(|j| (mc[j] + dx * a[j]) + dy * b[j])
    };
    let mut lo = [0.0f32; 4];
    let mut hi = [1.0f32; 4];
    for pts in &curves.positions {
        for p in pts {
            let f = fit(*p);
            for j in 0..4 {
                if f[j] < lo[j] {
                    lo[j] = f[j];
                }
                if hi[j] <= f[j] && f[j] != hi[j] {
                    hi[j] = f[j];
                }
            }
        }
    }
    for (ci, pts) in curves.positions.iter().enumerate() {
        let mut contour = Vec::with_capacity(pts.len());
        for (k, p) in pts.iter().enumerate() {
            contour.push(fill.positions.len() as u32);
            fill.positions.push(*p);
            let f = fit(*p);
            fill.colors.push(std::array::from_fn(|j| (f[j] - lo[j]) / (hi[j] - lo[j])));
            fill.tex_coords.push(curves.tex_coords[ci][k]);
        }
        fill.contours.push(contour);
    }
    fill.tessellate();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_source(subdivisions: i32) -> ShapeSource {
        let mut s = ShapeSource::default();
        s.faces = vec![vec![0, 1, 2, 3]];
        s.vertex_positions = Keyed::constant(vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
        s.vertex_colors = Keyed::constant(vec![[1.0, 0.0, 0.0, 1.0]; 4]);
        s.vertex_tex_coords = Keyed::constant(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        s.vertex_flags = vec![0; 4];
        s.subdivisions = subdivisions;
        s
    }

    #[test]
    fn keyed_interpolation_endpoints_and_linear() {
        // Frame 0 is the working value; frames 1..4 hold 0..3.
        let mut k = Keyed::from_frames(vec![9.0f32, 0.0, 1.0, 2.0, 3.0]);
        k.interpolate([1, 2, 3, 4], 1.0, 1.0, 0.0, bernstein(0.0));
        assert_eq!(k.current, 1.0);
        k.interpolate([1, 2, 3, 4], 1.0, 1.0, 1.0, bernstein(1.0));
        assert_eq!(k.current, 2.0);
        // Evenly spaced keys: the Catmull-Rom control points sit on the line.
        k.interpolate([1, 2, 3, 4], 1.0, 1.0, 0.5, bernstein(0.5));
        assert!((k.current - 1.5).abs() < 1e-6);
        // The formula itself.
        let v = cubic_f32(0.0, 1.0, 3.0, 4.0, 0.5, 0.25, [0.1, 0.2, 0.3, 0.4]);
        let f = (3.0f32 - 1.0) * 0.25;
        let want = (3.0 - ((4.0 - 1.0) * 0.25 * 0.25 + 0.75 * f)) * 0.3
            + ((3.0 - 0.0) * 0.25 * 0.5 + 0.5 * f + 1.0) * 0.2
            + 1.0 * 0.1
            + 3.0 * 0.4;
        assert_eq!(v, want);
        // Arrays interpolate element-wise.
        let mut arr = Keyed::constant(vec![[0.0f32; 2]; 2]);
        arr.keys = vec![vec![[0.0, 0.0]; 2], vec![[1.0, 2.0]; 2], vec![[3.0, 4.0]; 2], vec![[4.0, 4.0]; 2]];
        arr.interpolate([0, 1, 2, 3], 0.0, 0.0, 0.0, bernstein(0.0));
        assert_eq!(arr.current, vec![[1.0, 2.0]; 2]);
    }

    #[test]
    fn keyed_key_slots() {
        let mut k = Keyed::constant(5.0f32);
        k.current = 7.0;
        assert_eq!(k.add_key(), 1);
        // Frame 0 is the working value itself: loading it changes nothing.
        k.load_key(0);
        assert_eq!(k.current, 7.0);
        k.current = 9.0;
        k.store_key(1);
        assert_eq!(k.keys[1], 9.0);
        k.load_key(1);
        assert_eq!(k.current, 9.0);
        k.remove_key(0);
        assert_eq!(k.key_count(), 1);
    }

    #[test]
    fn timeline_transition_from_current_value() {
        // gui.plx style: frames 0/1 rest, sequence "button:enter" = one key (frame 2 at 200 ms).
        let mut k = Keyed::from_frames(vec![0.0f32, 0.0, 10.0]);
        let mut m = Movie::default();
        m.add_key(2, 200, 0.0);
        k.sequences.insert("button:enter".into(), m);
        assert!(k.start_sequence("button:enter", 0));
        // A (frame 1, t 0) key was prepended.
        assert_eq!(k.working.keys, vec![SeqKey { frame: 1, time: 0, smoothness: 0.0 }, SeqKey { frame: 2, time: 200, smoothness: 0.0 }]);
        assert!(k.advance(100)); // evaluates t = 0
        assert_eq!(k.current, 0.0);
        assert!(k.advance(100)); // t = 100: halfway
        let w = bernstein(0.5);
        let f = (10.0f32 - 0.0) * 0.25;
        let want = (10.0 - ((10.0 - 0.0) * 0.25 * 0.0 + 1.0 * f)) * w[2] + ((10.0 - 0.0) * 0.25 * 0.0 + 1.0 * f + 0.0) * w[1] + 0.0 * w[0] + 10.0 * w[3];
        assert_eq!(k.current, want);
        assert!(k.advance(100)); // t = 200: holds frame 2
        assert_eq!(k.current, 10.0);
        assert!(!k.advance(100)); // nothing changes: the movie is cleared
        assert!(k.working.keys.is_empty());
        // Discrete attributes step at s = 1.
        let mut d = Keyed::from_frames(vec![1i32, 1, 0]);
        let mut m = Movie::default();
        m.add_key(2, 100, 0.0);
        d.sequences.insert("hide".into(), m);
        d.start_sequence("hide", 0);
        d.seek(50);
        assert_eq!(d.current, 1);
        d.seek(100);
        assert_eq!(d.current, 0);
        assert_eq!(sequence_end(&d, "hide"), 100);
    }

    #[test]
    fn face_curves_single_polygon_and_segment() {
        assert_eq!(face_curves(&[vec![2, 0, 1]]), vec![vec![2, 0, 1]]);
        // Two triangles sharing an edge: one loop around the quad.
        let loops = face_curves(&[vec![0, 1, 2], vec![0, 2, 3]]);
        assert_eq!(loops, vec![vec![0, 1, 2, 3]]);
        // A two-vertex face is a segment loop.
        assert_eq!(face_curves(&[vec![4, 5]]), vec![vec![4, 5]]);
    }

    #[test]
    fn subdivide_quad_one_level() {
        // subdivisions = 1: the polygon -> quad level only.
        let shape = SmoothMeshShape::new(quad_source(1));
        let fill = shape.fill.as_ref().unwrap();
        // 4 corners + 1 face point + 4 edge points.
        assert_eq!(fill.positions.len(), 9);
        // 4 quads, two triangles each.
        assert_eq!(fill.indices.len(), 4 * 6);
        // Level-0 vertex list (0x00671450): first-seen over the quads
        // [fp, ep(prev), v, ep(next)] -> fp, ep(3-0), v0, ep(0-1), v1, ep(1-2), v2, ep(2-3), v3.
        assert_eq!(fill.positions[0], [2.0, 2.0]);
        assert_eq!(fill.positions[1], [0.0, 2.0]);
        assert_eq!(fill.positions[3], [2.0, 0.0]);
        assert_eq!(fill.positions[5], [4.0, 2.0]);
        assert_eq!(fill.positions[7], [2.0, 4.0]);
        // First quad [fp, ep(3-0), v0, ep(0-1)].
        assert_eq!(&fill.indices[..6], &[0, 1, 2, 2, 3, 0]);
        // A lone quad: every vertex and edge is on the boundary. Corners use the boundary
        // rule (1-t)·p + t·avg(adjacent edge midpoints).
        let t = default_smooth_weight();
        let corner = fill.positions[2];
        let inv = 1.0 / 2.0f32;
        let want = [(1.0 - t) * 0.0 + inv * t * 2.0, (1.0 - t) * 0.0 + inv * t * 2.0];
        assert!((corner[0] - want[0]).abs() < 1e-6 && (corner[1] - want[1]).abs() < 1e-6, "{corner:?} vs {want:?}");
        // Colours are constant, uvs interpolate.
        assert!(fill.colors.iter().all(|c| *c == [1.0, 0.0, 0.0, 1.0]));
        assert_eq!(fill.tex_coords[0], [0.5, 0.5]);
    }

    #[test]
    fn subdivide_quad_two_levels_counts() {
        let shape = SmoothMeshShape::new(quad_source(2));
        let fill = shape.fill.as_ref().unwrap();
        // 3x3 grid -> 5x5 grid.
        assert_eq!(fill.positions.len(), 25);
        assert_eq!(fill.indices.len(), 16 * 6);
        // Fixed corners stay put (level-1 list starts with the level-0 list, where the
        // corners sit at 2, 4, 6, 8).
        let mut s = quad_source(2);
        s.vertex_flags = vec![kind::FIXED as u32; 4];
        let shape = SmoothMeshShape::new(s);
        let fill = shape.fill.as_ref().unwrap();
        let corners: Vec<Vec2> = [2, 4, 6, 8].iter().map(|&i| fill.positions[i]).collect();
        assert_eq!(corners, vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]);
        // Every index is in range and every quad is non-degenerate.
        assert!(fill.indices.iter().all(|&i| (i as usize) < 25));
    }

    #[test]
    fn unsubdivided_mesh_is_a_fan() {
        let shape = SmoothMeshShape::new(quad_source(0));
        let fill = shape.fill.as_ref().unwrap();
        assert_eq!(fill.positions.len(), 4);
        assert_eq!(fill.indices, vec![0, 1, 2, 0, 2, 3]);
        // A full rebuild (dirty 0xf, bit 3 set) computes the fill's outlines.
        assert_eq!(fill.outlines, vec![vec![0, 1, 2, 3]]);
        assert!(shape.contains_point([1.0, 1.0]));
        // A partial rebuild in mesh mode leaves the outlines as they were.
        let mut shape = shape;
        shape.source.vertex_positions.current[2] = [8.0, 8.0];
        shape.source.vertex_positions.changed = true;
        shape.rebuild(false);
        let fill = shape.fill.as_ref().unwrap();
        assert_eq!(fill.flags & 8, 0);
        assert_eq!(fill.positions[2], [8.0, 8.0]);
    }

    #[test]
    fn curve_mode_fill_has_outline_and_hits() {
        let mut s = quad_source(0);
        s.flags = FLAG_CURVE;
        s.vertex_colors = Keyed::constant(vec![[0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]]);
        let shape = SmoothMeshShape::new(s);
        let fill = shape.fill.as_ref().unwrap();
        assert_eq!(fill.positions.len(), 4);
        assert_eq!(fill.indices.len(), 6);
        assert_eq!(fill.outlines.len(), 1);
        assert!(shape.contains_point([1.0, 1.0]));
        assert!(!shape.contains_point([5.0, 1.0]));
        // The fitted gradient reproduces the red ramp along x.
        assert!((fill.colors[0][0] - 0.0).abs() < 1e-5);
        assert!((fill.colors[1][0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn curve_subdivision_counts_and_corner_weights() {
        // A closed square with 2 levels: 4 * 4 points.
        let mut s = quad_source(2);
        s.flags = FLAG_CURVE;
        let shape = SmoothMeshShape::new(s.clone());
        let pos = s.vertex_positions.current.clone();
        let curves = build_curves(&shape.source, &shape.curves, &pos, 2);
        assert_eq!(curves.positions[0].len(), 16);
        // Open: (n-1)*4+1.
        s.flags = FLAG_CURVE | FLAG_OPEN;
        let curves = build_curves(&s, &shape.curves, &pos, 2);
        assert_eq!(curves.positions[0].len(), 13);
        // Level-0 corner weight.
        let c = corner_weight();
        assert!((c - 1.0 / (1.0 + std::f32::consts::SQRT_2)).abs() < 1e-6);
        let mut p = vec![[0.0f32, 0.0], [0.0, 0.0], [4.0, 0.0]];
        let mut k = vec![kind::CORNER as u32, 0, 0];
        subdivide_curve(&mut p, &mut k, 2, 1, false, 0.5, 0);
        assert!((p[1][0] - (0.0 * c + (1.0 - c) * 4.0)).abs() < 1e-6);
        assert_eq!(k[1], 2);
    }

    #[test]
    fn stroke_and_extrusion_drawings() {
        let mut s = quad_source(0);
        s.flags = FLAG_STROKE | FLAG_EXTRUSION | FLAG_NO_FILL;
        s.stroke_colors = Keyed::constant(vec![[0.0, 0.0, 0.0, 1.0]; 4]);
        s.stroke_widths = Keyed::constant(vec![1.0; 4]);
        s.extrusion_front_colors = Keyed::constant(vec![[0.5; 4]; 4]);
        s.extrusion_back_colors = Keyed::constant(vec![[0.25; 4]; 4]);
        let mut m = identity();
        m[12] = 1.0;
        m[13] = 1.0;
        s.extrusion_matrix = Keyed::constant(m);
        let shape = SmoothMeshShape::new(s);
        assert!(shape.fill.is_none());
        let st = shape.stroke.as_ref().unwrap();
        // Closed curve: n + 1 pairs (point 0 repeated), one triangle per strip vertex.
        assert_eq!(st.positions.len(), 10);
        assert_eq!(st.indices.len(), 30);
        assert!(!st.outlines.is_empty());
        let ex = shape.extrusion.as_ref().unwrap();
        assert_eq!(ex.indices.len(), 12, "two walls face away from (1,1)");
        let kinds: Vec<_> = shape.drawings().iter().map(|d| d.0).collect();
        assert_eq!(kinds, vec![DrawingKind::Extrusion, DrawingKind::Stroke]);
    }

    #[test]
    fn adaptive_levels() {
        let mut s = SmoothMeshShape::new(quad_source(5));
        assert_eq!(s.levels(), 5);
        s.adaptive = true;
        // Σ face sizes = 4: ln(640)/ln(4) = 4.66 -> 5; min(5, 5).
        assert_eq!(s.levels(), 5);
        s.source.faces = vec![vec![0, 1, 2, 3]; 100];
        // 2560/400 = 6.4: ln/ln4 = 1.34 -> 2.
        assert_eq!(s.levels(), 2);
    }

    #[test]
    fn texture_matrix_translation_and_rotation() {
        let mut src = quad_source(0);
        src.texture_translation = Keyed::constant([2.0, 3.0]);
        let s = SmoothMeshShape::new(src.clone());
        let m = s.texture_matrix;
        assert_eq!((m[12], m[13]), (2.0, 3.0));
        src.texture_translation = Keyed::constant([0.0, 0.0]);
        src.texture_rotation = Keyed::constant([0.0, 0.0, 90.0]);
        let s = SmoothMeshShape::new(src);
        let m = s.texture_matrix;
        assert!((m[0]).abs() < 1e-6 && (m[1] - 1.0).abs() < 1e-6 && (m[4] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn vertex_constraint_slider() {
        let mut pos = vec![[0.0, 0.0], [5.0, 5.0], [10.0, 0.0]];
        let flags = vec![0, 0x20000, 0];
        let params = vec![0.0, 0.25, 0.0];
        apply_vertex_constraints(&mut pos, &[vec![0, 1, 2]], &flags, &params);
        // Between the forward (2) and backward (0) neighbours at t = 0.25 from forward.
        assert_eq!(pos[1], [0.0 * 0.25 + 0.75 * 10.0, 0.75 * 0.0 + 0.0 * 0.25]);
    }
}
