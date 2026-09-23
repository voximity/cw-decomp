//! `plasma::Font`, `plasma::FontEngine`, `plasma::ScalableFont` and `plasma::TextShape`
//! (Cube.exe).
//!
//! # What the original does
//!
//! Text is **not** vector geometry in the shipped game. `plasma::ScalableFont` (0x124 bytes,
//! ctor `0x0065a900`) wraps a statically linked FreeType face. Every glyph is rasterised by
//! FreeType (with the node's 2×2 transform applied through `FT_Set_Transform`) into a small
//! RGBA texture with a 1-pixel transparent border, and each character is drawn as one
//! textured screen quad through `D3D9Engine::drawTexturedRect` (engine slot 5, `0x00689af0`).
//! `Engine::createGlyphShapes` `0x00650e80` (FreeType outlines → `SmoothMeshShape`s) is
//! reached only from the `.pla` reader `0x006555d0` (no `.pla` ships), so it is not ported.
//!
//! # Which fonts the game loads (Tier A fact, see the report)
//!
//! - `resource1.dat` and `resource2.dat` from the working directory: TrueType files
//!   (FontForge, family names "resource1" Bold / "resource2" Medium) requested by name
//!   through `FontEngine::getFont` in the `GameController` ctor (`0x0045ddd5`,
//!   `0x0045de2d`); if either fails to load, `GameController+0x1a0` (the quit flag) is set.
//!   The game widgets draw with them through `FontEngine::drawText` `0x00639b30`.
//! - `tahoma.ttf`: the `TextShape.font.wfileName` of the `TextShape`s in `gui.plx` and
//!   `help.plx`; not shipped, found through the engine search path `c:\windows\fonts`
//!   (added by `initDirect3D` `0x004c8807` via `FontEngine::addSearchPath` `0x00639390`).
//! - `arial.ttf`: `TextShape::rebuild` `0x00664770` falls back to it when the shape's font
//!   cannot be loaded.
//!
//! The file extension decides the loader (`ScalableFont::load` `0x0065f260`): `TTF` or `DAT`
//! (upper-cased) go to FreeType, anything else to a `.plx` "vector font" whose glyphs are
//! child shapes of a node (`0x0065f3d0`). The latter is not used by any shipped data and is
//! not ported ([`VECTOR_FONT_SCALE`] stands for its scale factor, always 1 for FreeType).
//!
//! # Porting notes
//!
//! Layout (`measure` `0x0065d530`, `wrap` `0x00660d50`, `caret` `0x0065ded0`, the draw loop
//! `0x0065c040`) is ported statement by statement over a [`GlyphSource`], so tests can use
//! synthetic metrics. [`AbFace`] implements [`GlyphSource`] with `ab_glyph`, emulating
//! FreeType's 26.6 fixed-point scaling (`FT_Set_Char_Size` with 0 dpi = 72, `FT_MulFix`,
//! `FT_DivFix`) for advances and bitmap boxes. Rasterisation is Tier C.
//!
//! Text is UTF-16 (`std::wstring` in the original); functions take `&[u16]` and read index
//! `len` as the terminating 0, as the original does through `c_str()`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ab_glyph::{Font as _, FontVec, GlyphId, Outline, OutlineCurve, OutlinedGlyph, Point};
use glam::{Vec2, Vec4};

// ---------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------

/// The engine's font search directory, `Cube.exe 0x00702d94`, registered by
/// `initDirect3D 0x004c8720` (at `0x004c8807`) through `FontEngine::addSearchPath 0x00639390`.
pub const ENGINE_FONT_SEARCH_PATH: &str = r"c:\windows\fonts";
/// `TextShape::rebuild 0x00664770` falls back to this font (`Cube.exe 0x0071ed58`).
pub const TEXT_SHAPE_FALLBACK_FONT: &str = "arial.ttf";
/// The two fonts the `GameController` ctor requests (`0x006fcd24`, `0x0070027c`).
pub const GAME_FONTS: [&str; 2] = ["resource1.dat", "resource2.dat"];
/// The separator `FontEngine::getFont` puts between a search directory and the file name
/// (`L"/"` at `0x006fd42c`).
pub const SEARCH_PATH_SEPARATOR: &str = "/";
/// Scale factor of the unported `.plx` vector-font branch (`ScalableFont+0xf4 != 0`):
/// `size / (bboxMax.y − bboxMin.y)`. Always 1 for FreeType faces.
pub const VECTOR_FONT_SCALE: f32 = 1.0;
/// Glyph bitmaps whose transformed box exceeds this edge (sqrt of the area) are rendered at
/// a reduced scale (`0x0065ea80`).
pub const MAX_GLYPH_BITMAP_EDGE: f32 = 500.0;
/// `DAT_00768f6c` (a `.data` float, 500.0): upper clamp of the pixel-mode stroke radius in
/// `0x00660b60`. Assumed never written at run time.
pub const MAX_PIXEL_STROKE_RADIUS: f32 = 500.0;
/// FreeType load flags used by the original (`FT_Load_Char` = `0x00692920`).
pub const FT_LOAD_NO_HINTING_NO_BITMAP: i32 = 10;
/// Pixel-mode load flags: `FT_LOAD_NO_BITMAP` only (hinting on).
pub const FT_LOAD_NO_BITMAP: i32 = 8;

/// Text alignment and wrapping flags (`TextShape.flags`, `TextShape+0x1ec`, and the `flags`
/// argument of the layout functions).
pub mod align {
    /// Centre horizontally on the origin.
    pub const H_CENTER: u32 = 1;
    /// Right-align on the origin.
    pub const RIGHT: u32 = 2;
    /// Centre vertically.
    pub const V_CENTER: u32 = 4;
    /// Bottom-align.
    pub const BOTTOM: u32 = 8;
    /// Word-wrap at the wrap width (`0x00660d50`).
    pub const WRAP: u32 = 0x10;
}

// ---------------------------------------------------------------------------------------
// FreeType fixed point
// ---------------------------------------------------------------------------------------

/// `FT_MulFix`: `(a·b + 0x8000) >> 16` computed on magnitudes, sign applied after.
pub fn ft_mul_fix(a: i64, b: i64) -> i64 {
    let s = (a < 0) != (b < 0);
    let r = (a.abs() * b.abs() + 0x8000) >> 16;
    if s { -r } else { r }
}

/// `FT_DivFix`: `(a << 16 + b/2) / b` on magnitudes.
pub fn ft_div_fix(a: i64, b: i64) -> i64 {
    if b == 0 {
        return 0x7fff_ffff;
    }
    let s = (a < 0) != (b < 0);
    let (a, b) = (a.abs(), b.abs());
    let r = ((a << 16) + (b >> 1)) / b;
    if s { -r } else { r }
}

/// `FT_MulDiv`: `(a·b + c/2) / c` on magnitudes.
pub fn ft_mul_div(a: i64, b: i64, c: i64) -> i64 {
    if c == 0 {
        return 0x7fff_ffff;
    }
    let s = ((a < 0) != (b < 0)) != (c < 0);
    let r = (a.abs() * b.abs() + c.abs() / 2) / c.abs();
    if s { -r } else { r }
}

fn pix_floor(x: i64) -> i64 {
    x & !63
}
fn pix_ceil(x: i64) -> i64 {
    (x + 63) & !63
}
fn pix_round(x: i64) -> i64 {
    (x + 32) & !63
}

/// `(int)(x * 64.0 + 0.5)` as the original computes 26.6 sizes (`cvttss2si`, truncation).
fn to_26_6(x: f32) -> i32 {
    (x * 64.0 + 0.5) as i32
}

// ---------------------------------------------------------------------------------------
// Size state and glyph records
// ---------------------------------------------------------------------------------------

/// The per-call size state of a `ScalableFont` (fields `+0x78..+0x108`), set by
/// `setSize 0x006605c0` before every measure or draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeState {
    /// `+0xe0`: horizontal size in layout units (pixels at `FT_Set_Char_Size` 72 dpi).
    pub size_x: f32,
    /// `+0xe4`: vertical size; also the line advance base and the initial `min.y = −size`.
    pub size_y: f32,
    /// `+0xe8`: stroke radius (0 = no stroke glyphs).
    pub stroke_radius: f32,
    /// `+0xec`: extra advance added after every character but the last on a line.
    pub spacing: f32,
    /// `+0xf0`: extra line advance added to `size_y` at each `'\n'`.
    pub line_spacing: f32,
    /// `+0x108`: pixel mode (hinted glyphs at the device pixel size, pen truncated to
    /// integers). Set by `0x00660270`, cleared by `0x006606f0`.
    pub pixel_mode: bool,
    /// `+0x88/+0x8c/+0x98/+0x9c`: the 2×2 linear part of the transform the pen is
    /// projected through when drawing, `[m00, m01, m10, m11]` in D3D row-vector order
    /// (`x' = x·m00 + y·m10`). Identity in pixel mode.
    pub linear: [f32; 4],
    /// `+0x78..+0x84`: the `FT_Matrix` (16.16) handed to `FT_Set_Transform`:
    /// `[xx, xy, yx, yy] = [m00, −m10, −m01, m11]` (y flipped into FreeType's y-up space).
    pub ft_matrix: [i32; 4],
}

impl Default for SizeState {
    fn default() -> Self {
        Self {
            size_x: 0.0,
            size_y: 0.0,
            stroke_radius: 0.0,
            spacing: 0.0,
            line_spacing: 0.0,
            pixel_mode: false,
            linear: [1.0, 0.0, 0.0, 1.0],
            ft_matrix: [0x10000, 0, 0, 0x10000],
        }
    }
}

impl SizeState {
    /// `setSize 0x006605c0` (FreeType branch): `transform` is the current 4×4 transform as
    /// the D3D row-major flat array (`glam::Mat4::to_cols_array()` gives this order).
    pub fn set(
        &mut self,
        size: f32,
        stroke_radius: f32,
        spacing: f32,
        line_spacing: f32,
        transform: &[f32; 16],
        pixel_snap: bool,
    ) {
        if pixel_snap {
            // 0x00660b60 then 0x00660270.
            let (px, stroke, sp, ls) = pixel_sizes(transform, size, stroke_radius, spacing, line_spacing);
            self.set_pixel_size(px.x, px.y, stroke);
            self.spacing = sp;
            self.line_spacing = ls;
            return;
        }
        // 0x006606f0.
        self.set_transformed_size(size, stroke_radius, transform);
        self.spacing = spacing;
        self.line_spacing = line_spacing;
    }

    /// `0x006606f0`: vector (transformed) mode. The FreeType face is set to `size` px with
    /// the transform's 2×2 part; the stroke radius is scaled by the shorter axis.
    fn set_transformed_size(&mut self, size: f32, stroke_radius: f32, m: &[f32; 16]) {
        self.linear = [m[0], m[1], m[4], m[5]];
        self.ft_matrix = [
            (m[0] * 65536.0) as i32,
            -((m[4] * 65536.0) as i32),
            -((m[1] * 65536.0) as i32),
            (m[5] * 65536.0) as i32,
        ];
        if size <= 0.0 {
            return;
        }
        self.size_x = size;
        self.size_y = size;
        self.pixel_mode = false;
        let row1 = ((m[5] + m[1] * 0.0).powi(2) + (m[4] + m[0] * 0.0).powi(2)).sqrt();
        let row0 = ((m[1] + m[5] * 0.0).powi(2) + (m[0] + m[4] * 0.0).powi(2)).sqrt();
        let min = if row0 <= row1 { row0 } else { row1 };
        self.stroke_radius = min * stroke_radius;
    }

    /// `0x00660270`: pixel mode at `(w, h)` device pixels, no FreeType transform.
    fn set_pixel_size(&mut self, w: f32, h: f32, stroke: f32) {
        let h = if h <= 0.0 { w } else { h };
        let w = if w > 0.0 { w } else { h };
        if !(w > 0.0 || h > 0.0) || !(stroke >= 0.0) {
            return;
        }
        self.size_y = h;
        self.size_x = w;
        self.stroke_radius = stroke;
        self.pixel_mode = true;
        self.linear = [1.0, 0.0, 0.0, 1.0];
        self.ft_matrix = [0x10000, 0, 0, 0x10000];
    }
}

/// `0x00660b60`: device pixel sizes for pixel mode. Returns `((w, h), stroke, spacing,
/// lineSpacing)`: the axis scales of the transform times `size`, rounded to 1/64 and
/// clamped to `[1, 1000]`.
pub fn pixel_sizes(m: &[f32; 16], size: f32, stroke: f32, spacing: f32, line_spacing: f32) -> (Vec2, f32, f32, f32) {
    let a = m[5] * 0.0 + m[1];
    let b = m[4] * 0.0 + m[0];
    let c = m[1] * 0.0 + m[5];
    let d = m[0] * 0.0 + m[4];
    let sx = (a * a + b * b).sqrt();
    let sy = (c * c + d * d).sqrt();
    let mut w = ((sx * size * 64.0 + 0.5) as i32) as f32 * 0.015625;
    let mut h = ((sy * size * 64.0 + 0.5) as i32) as f32 * 0.015625;
    if 1000.0 < w {
        w = 1000.0;
    }
    if 1000.0 < h {
        h = 1000.0;
    }
    let mut stroke_out = (h / size + w / size) * 0.5 * stroke;
    let spacing_out = (w / size) * spacing;
    let line_out = (h / size) * line_spacing;
    if w < 1.0 {
        w = 1.0;
    }
    if h < 1.0 {
        h = 1.0;
    }
    if MAX_PIXEL_STROKE_RADIUS < stroke_out {
        stroke_out = MAX_PIXEL_STROKE_RADIUS;
    }
    (Vec2::new(w, h), stroke_out, spacing_out, line_out)
}

/// One cached glyph, the 0x34-byte record built by `0x0065ea80` (transformed mode) or
/// `0x0065e340` (pixel mode). Coordinates are y-down layout units relative to the pen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphRecord {
    /// `[0], [1]`: `(bitmap_left − 1, −1 − bitmap_top) / reduction`: top-left of the quad,
    /// including the 1-pixel border.
    pub offset: Vec2,
    /// `[2], [3]`: `(width + 2, rows + 2) / reduction`: quad size.
    pub size: Vec2,
    /// `[4], [5]`: advance. Transformed mode: `metrics.horiAdvance / 64` (unscaled by the
    /// transform); pixel mode: the hinted `FT_Glyph` advance (16.16 / 65536).
    pub advance: Vec2,
    /// `[10] != 0`: a texture was created (always, since the bordered bitmap is ≥ 2×2).
    pub has_texture: bool,
}

/// Where glyph metrics come from: a FreeType face in the original, [`AbFace`] in the port,
/// or a synthetic table in tests.
pub trait GlyphSource {
    /// The glyph record for UTF-16 unit `c` at the current size (`0x0065ea80` /
    /// `0x0065e340`); `stroke` selects the stroker cache (`ScalableFont+0xcc`).
    /// `None` when FreeType fails to load the glyph.
    fn glyph(&mut self, st: &SizeState, c: u16, stroke: bool) -> Option<GlyphRecord>;
    /// `FT_Get_Kerning(face, index(left), index(right), mode)` `.x`: mode `FT_KERNING_UNSCALED`
    /// (font units) when `!st.pixel_mode`, `FT_KERNING_DEFAULT` (26.6, grid-fitted) otherwise.
    fn kerning_raw(&mut self, st: &SizeState, left: u16, right: u16) -> i32;
    /// `face->units_per_EM` (`face+0x44`).
    fn units_per_em(&self) -> u16;
}

/// Character at `i`, reading the terminating 0 at `len` (and beyond) like `c_str()`.
#[inline]
fn at(text: &[u16], i: i32) -> u16 {
    if i >= 0 && (i as usize) < text.len() { text[i as usize] } else { 0 }
}

const NL: u16 = 10;
const CR: u16 = 13;
const SPACE: u16 = 0x20;

/// The pen delta from kerning, as every layout loop computes it: unscaled mode
/// `(float)k · size_x / (float)upem`; pixel mode `(float)(k >> 6)`.
fn kerning_delta<S: GlyphSource>(src: &mut S, st: &SizeState, left: u16, right: u16) -> f32 {
    let k = src.kerning_raw(st, left, right);
    if !st.pixel_mode {
        (k as f32 * st.size_x) / src.units_per_em() as f32
    } else {
        (k >> 6) as f32
    }
}

// ---------------------------------------------------------------------------------------
// Layout: measure 0x0065d530
// ---------------------------------------------------------------------------------------

/// `ScalableFont::measure` `0x0065d530`: the bounds of `text` in layout units, y down,
/// origin at the first baseline.
///
/// - `line_start >= 0` measures only the line starting there (up to its `'\n'`).
/// - `pen_mode` (the last argument, `true` for drawing): bounds of the pen positions
///   (`min` stays `(0, −size_y)`), and the terminating 0 is visited. `false`: ink bounds of
///   the glyph quads (inset by the 1-pixel border).
/// - `flags` 1/2/4/8 shift the result by the whole text's bounds (always measured with
///   `line_start = −1`), as the original's recursion does.
pub fn measure<S: GlyphSource>(
    src: &mut S,
    st: &SizeState,
    text: &[u16],
    flags: u32,
    line_start: i32,
    pen_mode: bool,
) -> (Vec2, Vec2) {
    let len = text.len() as i32;
    let mut start = 0i32;
    let mut end = len;
    if line_start >= 0 {
        start = line_start;
        let mut j = line_start;
        while j <= len {
            if at(text, j) == NL {
                end = j;
                break;
            }
            j += 1;
        }
    }
    let mut min = Vec2::new(0.0, -st.size_y);
    let mut max = Vec2::ZERO;
    let mut pen = 0.0f32; // local_84 / local_10
    let mut pen_y = 0.0f32; // fStack_c
    let vs = VECTOR_FONT_SCALE;
    let mut first = true;
    if pen_mode {
        end += 1;
    }
    let mut i = start;
    while i < end {
        let c = at(text, i);
        let mut g = src.glyph(st, c, false);
        if 0.0 < st.stroke_radius {
            // The stroke glyph replaces the fill glyph for bounds and advance.
            g = src.glyph(st, c, true);
        }
        match (g, pen_mode) {
            (None, false) => {}
            (Some(g), false) => {
                if c != NL && c != CR {
                    if !first {
                        let v = pen + g.offset.x + 1.0;
                        if !(min.x <= v) {
                            min.x = v;
                        }
                        let v = pen_y + g.offset.y + 1.0;
                        if !(min.y <= v) {
                            min.y = v;
                        }
                        let v = (pen + g.offset.x + g.size.x) - 1.0;
                        if !(v < max.x || v == max.x) {
                            max.x = v;
                        }
                        let v = (pen_y + g.offset.y + g.size.y) - 1.0;
                        if !(v < max.y || v == max.y) {
                            max.y = v;
                        }
                    } else {
                        first = false;
                        min = Vec2::new(pen + g.offset.x + 1.0, pen_y + g.offset.y + 1.0);
                        let fx = g.size.x + pen + g.offset.x;
                        let fy = g.size.y + pen_y + g.offset.y;
                        max = Vec2::new(fx - 1.0, fy - 1.0);
                    }
                }
            }
            (_, true) => {
                // LAB_0065db55: extend max by the pen.
                if !(pen < max.x || pen == max.x) {
                    max.x = pen;
                }
                if !(pen_y < max.y || pen_y == max.y) {
                    max.y = pen_y;
                }
            }
        }
        if c == NL {
            pen = 0.0;
            pen_y = st.line_spacing + st.size_y + pen_y;
        } else {
            if c == 0 {
                break;
            }
            if let Some(g) = g {
                pen = g.advance.x * vs + pen;
            }
            if i < end - 1 && at(text, i + 1) != NL {
                pen += kerning_delta(src, st, c, at(text, i + 1));
                pen = st.spacing + pen;
                if st.pixel_mode {
                    pen = pen as i32 as f32;
                }
            }
        }
        i += 1;
    }
    if flags != 0 {
        let (m0, m1) = measure(src, st, text, 0, -1, pen_mode);
        if flags & align::H_CENTER != 0 {
            let c = (m1.x + m0.x) * 0.5;
            min.x -= c;
            max.x -= c;
        } else if flags & align::RIGHT != 0 {
            min.x -= m1.x;
            max.x -= m1.x;
        }
        if flags & align::V_CENTER != 0 {
            let c = (m1.y + m0.y) * 0.5;
            min.y -= c;
            max.y -= c;
        } else if flags & align::BOTTOM != 0 {
            min.y -= m1.y;
            max.y -= m1.y;
        }
    }
    (min, max)
}

// ---------------------------------------------------------------------------------------
// Word wrap 0x00660d50
// ---------------------------------------------------------------------------------------

/// `ScalableFont::wrap` `0x00660d50`: replaces the last break candidate (a space) before
/// the line overflows with `'\n'`, in place. Break candidates are `' '`, `'\n'`, `'\r'` and
/// the last character; the overflow test (`wrap_width <= x`) happens *at* a candidate,
/// before its own advance is added, so a word may overhang the width.
pub fn wrap<S: GlyphSource>(src: &mut S, st: &SizeState, text: &mut [u16], wrap_width: f32) {
    let len = text.len() as i32;
    let mut x = 0.0f32; // local_8
    let mut last_x = 0.0f32; // local_c
    let mut last = -1i32; // local_1c
    let vs = VECTOR_FONT_SCALE;
    let mut i = 0i32;
    while i < len {
        let mut next_x = 0.0f32; // fVar9
        let c = text[i as usize];
        if c == SPACE || c == NL || c == CR || i == len - 1 {
            // LAB_00660e03
            if wrap_width <= x {
                if last >= 0 {
                    text[last as usize] = NL;
                }
                x -= last_x;
            }
            last_x = x;
            last = i;
        }
        if c == NL || c == CR {
            last_x = 0.0;
        } else {
            let mut adv = src.glyph(st, c, false).map_or(0.0, |g| g.advance.x);
            if i < len - 1 {
                adv += kerning_delta(src, st, c, text[i as usize + 1]);
                x = st.spacing + x;
            }
            next_x = if !st.pixel_mode { adv * vs + x } else { (adv + x) as i32 as f32 };
        }
        x = next_x;
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------
// Caret 0x0065ded0
// ---------------------------------------------------------------------------------------

/// `ScalableFont::caretRect` `0x0065ded0` (used by `plasma::Edit` through `0x0065e8d0`):
/// the top-left and size of the caret box in front of character `index`. The size is the
/// advance of that character by `size_y`. Note the original adds kerning and spacing only
/// for characters before `index − 1`.
pub fn caret<S: GlyphSource>(
    src: &mut S,
    st: &SizeState,
    text: &[u16],
    index: i32,
    flags: u32,
) -> (Vec2, Vec2) {
    let vs = VECTOR_FONT_SCALE;
    let len = text.len() as i32;
    let index = if len < index { len } else { index };
    let mut pos = Vec2::new(0.0, -st.size_y);
    let mut size = Vec2::new(0.0, st.size_y);
    let (w_min, w_max) = measure(src, st, text, 0, -1, false);
    let (l_min, l_max) = measure(src, st, text, flags, 0, false);
    let h = flags & align::H_CENTER;
    if h != 0 {
        pos.x -= (((l_max.x - l_min.x) - w_max.x) + w_min.x) * 0.5;
    } else if flags & align::RIGHT != 0 {
        pos.x -= ((l_max.x - l_min.x) - w_max.x) + w_min.x;
    }
    let mut i = 0i32;
    if -1 < index {
        loop {
            let c = at(text, i);
            if c == NL && i != index {
                pos.x = 0.0;
                let (a, b) = measure(src, st, text, flags, i + 1, false);
                if h != 0 {
                    pos.x -= (((b.x - a.x) - w_max.x) + w_min.x) * 0.5;
                } else if flags & align::RIGHT != 0 {
                    pos.x -= ((b.x - a.x) - w_max.x) + w_min.x;
                }
                pos.y = st.line_spacing + st.size_y + pos.y;
            } else {
                if c == 0 {
                    break;
                }
                if let Some(g) = src.glyph(st, c, false) {
                    let a = g.advance.x * vs;
                    if i == index {
                        size.x = a;
                    } else {
                        pos.x = a + pos.x;
                    }
                }
                if i < index - 1 {
                    pos.x += kerning_delta(src, st, c, at(text, i + 1));
                    let v = st.spacing + pos.x;
                    pos.x = v;
                    if st.pixel_mode {
                        pos.x = v as i32 as f32;
                    }
                }
            }
            i += 1;
            if i > index {
                break;
            }
        }
    }
    if flags != 0 {
        let (m0, m1) = measure(src, st, text, 0, -1, false);
        if h != 0 {
            pos.x -= (m1.x + m0.x) * 0.5;
        } else if flags & align::RIGHT != 0 {
            pos.x -= m1.x;
        }
        if flags & align::V_CENTER != 0 {
            pos.y -= (m1.y + m0.y) * 0.5;
        } else if flags & align::BOTTOM != 0 {
            pos.y -= m1.y;
        }
    }
    (pos, size)
}

// ---------------------------------------------------------------------------------------
// Draw loop 0x0065c040
// ---------------------------------------------------------------------------------------

/// Which colour a glyph quad uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlyphLayer {
    /// The fill glyph, drawn with the text colour (`TextShape.color`).
    Fill,
    /// The stroker glyph (`FT_Glyph_Stroke`, round caps and joins), drawn after the fill
    /// glyph of the same character with the stroke colour.
    Stroke,
}

/// One `drawTexturedRect` call (engine slot 5, `0x00689af0`): `pos`/`size` in the space of
/// [`TextDraw::transform`], texture coordinates `(0,0)..(1,1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphQuad {
    /// Integer top-left (truncated as the original does).
    pub pos: Vec2,
    /// The record's quad size.
    pub size: Vec2,
    /// The character (UTF-16 unit).
    pub ch: u16,
    /// Fill or stroke glyph.
    pub layer: GlyphLayer,
}

/// The draw loop `0x0065c040`: glyph quads for `text` with the pen starting at `origin`.
/// The caller supplies the size state already set (and the wrap already applied).
pub fn layout_quads<S: GlyphSource>(
    src: &mut S,
    st: &SizeState,
    text: &[u16],
    origin: Vec2,
    flags: u32,
) -> Vec<GlyphQuad> {
    let vs = VECTOR_FONT_SCALE;
    let mut out = Vec::new();
    let (min0, _max0) = measure(src, st, text, 0, -1, true);
    let (min_a, max_a) = measure(src, st, text, flags, -1, true);
    let f12 = min_a.x - min0.x;
    let len = text.len() as i32;
    let x = f12 + origin.x;
    let mut y = (min_a.y - min0.y) + origin.y; // local_1c4
    let line_x = |src: &mut S, start: i32, fl: u32| -> f32 {
        let (lmin, lmax) = measure(src, st, text, fl, start, true);
        if flags & align::H_CENTER != 0 {
            x - (((lmax.x - lmin.x) - max_a.x) + f12) * 0.5
        } else if flags & align::RIGHT != 0 {
            x - (((lmax.x - lmin.x) - max_a.x) + f12)
        } else {
            x
        }
    };
    // First line: measured with `flags`; later lines with flags 0 (as the original).
    let mut pen = line_x(src, 0, flags); // local_1b8
    let project = |p: Vec2| -> Vec2 {
        let m = st.linear;
        Vec2::new(m[2] * p.y + p.x * m[0], m[3] * p.y + m[1] * p.x)
    };
    let mut i = 0i32;
    while i < len {
        let c = at(text, i);
        let next_pen;
        if c == NL {
            next_pen = line_x(src, i + 1, 0);
            y = st.line_spacing + st.size_y + y;
        } else {
            let mut adv = 0.0f32; // local_1c0
            let g = src.glyph(st, c, false);
            if let Some(g) = g {
                if g.has_texture {
                    let p = project(Vec2::new(pen, y));
                    let pos = if !st.pixel_mode {
                        Vec2::new((p.x + g.offset.x) as i32 as f32, (g.offset.y + p.y) as i32 as f32)
                    } else {
                        Vec2::new(
                            ((p.x as i32) + (g.offset.x as i32)) as f32,
                            ((p.y as i32) + (g.offset.y as i32)) as f32,
                        )
                    };
                    out.push(GlyphQuad { pos, size: g.size, ch: c, layer: GlyphLayer::Fill });
                }
                adv = g.advance.x;
            }
            if 0.0 < st.stroke_radius {
                if let Some(s) = src.glyph(st, c, true) {
                    if s.has_texture {
                        let p = project(Vec2::new(pen, y));
                        let pos = if !st.pixel_mode {
                            Vec2::new((s.offset.x + p.x) as i32 as f32, (s.offset.y + p.y) as i32 as f32)
                        } else {
                            Vec2::new(
                                ((p.x as i32) + (s.offset.x as i32)) as f32,
                                ((p.y as i32) + (s.offset.y as i32)) as f32,
                            )
                        };
                        out.push(GlyphQuad { pos, size: s.size, ch: c, layer: GlyphLayer::Stroke });
                    }
                }
            }
            if i < len - 1 {
                adv += kerning_delta(src, st, c, at(text, i + 1));
                pen = st.spacing + pen;
            }
            next_pen = if !st.pixel_mode { adv * vs + pen } else { (pen + adv) as i32 as f32 };
        }
        pen = next_pen;
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------
// FreeType emulation over ab_glyph
// ---------------------------------------------------------------------------------------

/// The fields of the TrueType `head` table the port needs (read directly; `ab_glyph` does
/// not expose them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadTable {
    /// `head.flags`; bit 3 = "force ppem to integer" (FreeType then rounds the scale).
    pub flags: u16,
    /// `unitsPerEm`.
    pub units_per_em: u16,
    /// `xMin, yMin, xMax, yMax` (FreeType's `face->bbox`).
    pub bbox: [i16; 4],
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(o)?, *b.get(o + 1)?, *b.get(o + 2)?, *b.get(o + 3)?]))
}

fn find_table<'a>(b: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = be16(b, 4)? as usize;
    for i in 0..n {
        let r = 12 + 16 * i;
        if b.get(r..r + 4)? == tag {
            let off = be32(b, r + 8)? as usize;
            let len = be32(b, r + 12)? as usize;
            return b.get(off..off + len);
        }
    }
    None
}

/// Reads `head` from an sfnt file.
pub fn read_head(b: &[u8]) -> Option<HeadTable> {
    let t = find_table(b, b"head")?;
    Some(HeadTable {
        flags: be16(t, 16)?,
        units_per_em: be16(t, 18)?,
        bbox: [be16(t, 36)? as i16, be16(t, 38)? as i16, be16(t, 40)? as i16, be16(t, 42)? as i16],
    })
}

/// Reads a Windows-platform (3) `name` record (1 family, 2 style, 6 PostScript name).
pub fn read_name(b: &[u8], name_id: u16) -> Option<String> {
    let t = find_table(b, b"name")?;
    let count = be16(t, 2)? as usize;
    let so = be16(t, 4)? as usize;
    for j in 0..count {
        let r = 6 + 12 * j;
        let (p, nid, len, off) = (be16(t, r)?, be16(t, r + 6)?, be16(t, r + 8)? as usize, be16(t, r + 10)? as usize);
        if p == 3 && nid == name_id {
            let s = t.get(so + off..so + off + len)?;
            let u: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            return Some(String::from_utf16_lossy(&u));
        }
    }
    None
}

/// A loaded face: the FreeType side of `ScalableFont` (`+0xd8` `FT_Face`, `+0xdc`
/// `FT_Stroker`, glyph caches `+0xc8`/`+0xcc`), emulated with `ab_glyph`.
pub struct AbFace {
    font: FontVec,
    head: HeadTable,
    /// `+0x38`: family name (`face->family_name`).
    pub family_name: String,
    /// `+0x20`: style name.
    pub style_name: String,
    /// `+0x50`: `FT_Get_Postscript_Name`.
    pub postscript_name: String,
    /// `+0xf8..+0x104`: `face->bbox / units_per_EM`.
    pub bbox_em: [f32; 4],
    cache: HashMap<GlyphKey, Option<GlyphRecord>>,
}

/// Glyph cache key: the original keeps one cache per size (and matrix / stroke radius),
/// here flattened into one map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// UTF-16 unit.
    pub ch: u16,
    /// Stroke cache.
    pub stroke: bool,
    /// Pixel mode.
    pub pixel: bool,
    /// `(size_x·64, size_y·64)` rounded as the original.
    pub size64: (i32, i32),
    /// `FT_Matrix` (identity in pixel mode).
    pub matrix: [i32; 4],
    /// `stroke_radius · 64` rounded.
    pub stroke64: i32,
}

impl GlyphKey {
    fn new(st: &SizeState, ch: u16, stroke: bool) -> Self {
        Self {
            ch,
            stroke,
            pixel: st.pixel_mode,
            size64: (to_26_6(st.size_x), to_26_6(st.size_y)),
            matrix: st.ft_matrix,
            stroke64: if stroke { to_26_6(st.stroke_radius) } else { 0 },
        }
    }
}

/// A rasterised glyph texture as `0x0065ea80` builds it: RGBA8, rows top to bottom, 1-pixel
/// transparent border, RGB = 255, A = coverage (not premultiplied, as the original).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    /// `bitmap.width + 2`.
    pub width: u32,
    /// `bitmap.rows + 2`.
    pub height: u32,
    /// `width · height · 4` bytes.
    pub rgba: Vec<u8>,
}

/// FreeType-like scaled cbox of a glyph: `(left, top, width, rows)` in pixels of the
/// rendered bitmap and the 26.6 transformed outline points.
struct Cbox {
    left: i64,
    top: i64,
    width: i64,
    rows: i64,
}

impl AbFace {
    /// `ScalableFont::loadFreeType` `0x0065fb80` / `0x0065fef0` (memory). `None` when the data
    /// is not a font (`FT_New_Face` fails).
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Self> {
        let head = read_head(&bytes)?;
        let family_name = read_name(&bytes, 1).unwrap_or_default();
        let style_name = read_name(&bytes, 2).unwrap_or_default();
        let postscript_name = read_name(&bytes, 6).unwrap_or_default();
        let font = FontVec::try_from_vec(bytes).ok()?;
        let u = head.units_per_em as f32;
        let bbox_em = [
            head.bbox[0] as i32 as f32 / u,
            head.bbox[1] as i32 as f32 / u,
            head.bbox[2] as i32 as f32 / u,
            head.bbox[3] as i32 as f32 / u,
        ];
        Some(Self { font, head, family_name, style_name, postscript_name, bbox_em, cache: HashMap::new() })
    }

    /// The `head` table.
    pub fn head(&self) -> HeadTable {
        self.head
    }

    fn glyph_id(&self, c: u16) -> GlyphId {
        // FT_Get_Char_Index with the Unicode cmap; surrogates have no glyph.
        char::from_u32(c as u32).map_or(GlyphId(0), |ch| self.font.glyph_id(ch))
    }

    /// `x_scale`/`y_scale` (16.16) after `FT_Set_Char_Size(w64, h64, 0, 0)`: 72 dpi, so the
    /// requested 26.6 size is the scaled size; with `head.flags & 8` FreeType rounds the
    /// ppem first (`tt_size_reset`).
    fn scales(&self, st: &SizeState) -> (i64, i64) {
        let upem = self.head.units_per_em as i64;
        let (w, h) = (to_26_6(st.size_x) as i64, to_26_6(st.size_y) as i64);
        if self.head.flags & 8 != 0 {
            let (xp, yp) = ((w + 32) >> 6, (h + 32) >> 6);
            (ft_div_fix(xp << 6, upem), ft_div_fix(yp << 6, upem))
        } else {
            (ft_div_fix(w, upem), ft_div_fix(h, upem))
        }
    }

    /// Scaled (and, in transformed mode, `FT_Outline_Transform`ed) 26.6 control points.
    fn points_26_6(&self, st: &SizeState, id: GlyphId, matrix: [i32; 4]) -> (Vec<Vec<(i64, i64)>>, Option<Outline>) {
        let (xs, ys) = self.scales(st);
        let Some(outline) = self.font.outline(id) else { return (Vec::new(), None) };
        let tf = |p: Point| -> (i64, i64) {
            let x = ft_mul_fix(p.x.round() as i64, xs);
            let y = ft_mul_fix(p.y.round() as i64, ys);
            let [xx, xy, yx, yy] = matrix.map(|v| v as i64);
            (ft_mul_fix(x, xx) + ft_mul_fix(y, xy), ft_mul_fix(x, yx) + ft_mul_fix(y, yy))
        };
        let pts = outline
            .curves
            .iter()
            .map(|c| match *c {
                OutlineCurve::Line(a, b) => vec![tf(a), tf(b)],
                OutlineCurve::Quad(a, b, c) => vec![tf(a), tf(b), tf(c)],
                OutlineCurve::Cubic(a, b, c, d) => vec![tf(a), tf(b), tf(c), tf(d)],
            })
            .collect();
        (pts, Some(outline))
    }

    fn cbox(&self, st: &SizeState, id: GlyphId, matrix: [i32; 4], stroke64: i64) -> Cbox {
        let (pts, _) = self.points_26_6(st, id, matrix);
        let mut it = pts.iter().flatten();
        let Some(&(x0, y0)) = it.next() else {
            return Cbox { left: 0, top: 0, width: 0, rows: 0 };
        };
        let (mut x_min, mut y_min, mut x_max, mut y_max) = (x0, y0, x0, y0);
        for &(x, y) in it {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
        // Approximation of FT_Glyph_Stroke's outline growth: the cbox grows by the radius.
        x_min -= stroke64;
        y_min -= stroke64;
        x_max += stroke64;
        y_max += stroke64;
        let (x_min, y_min, x_max, y_max) = (pix_floor(x_min), pix_floor(y_min), pix_ceil(x_max), pix_ceil(y_max));
        Cbox { left: x_min >> 6, top: y_max >> 6, width: (x_max - x_min) >> 6, rows: (y_max - y_min) >> 6 }
    }

    /// The bitmap-size reduction of `0x0065ea80`: if the transformed glyph box is larger
    /// than 500 px (sqrt of the area), the transform is scaled by `500 / sqrt(area)`.
    fn reduction(&self, st: &SizeState, id: GlyphId) -> f32 {
        let (xs, ys) = self.scales(st);
        let Some(o) = self.font.outline(id) else { return 1.0 };
        // metrics.width / height (26.6, unhinted = scaled cbox size) / 64.
        let w = (ft_mul_fix(o.bounds.max.x.round() as i64, xs) - ft_mul_fix(o.bounds.min.x.round() as i64, xs)) as f32
            * 0.015625;
        // ab_glyph bounds are y-flipped (min.y = y_max).
        let h = (ft_mul_fix(o.bounds.min.y.round() as i64, ys) - ft_mul_fix(o.bounds.max.y.round() as i64, ys)) as f32
            * 0.015625;
        let [xx, xy, yx, yy] = st.ft_matrix.map(|v| v as f32 * 1.525_878_9e-5);
        // Corners (0,0), (w,0), (w,h), (0,h) through the 2×2 (the original also includes
        // the origin through its min/max init at 0).
        let pts = [(w * xx, w * -yx), (w * xx + h * -xy, w * -yx + h * yy), (h * -xy, h * yy)];
        let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let r = MAX_GLYPH_BITMAP_EDGE / ((y1 - y0) * (x1 - x0)).sqrt();
        if r < 1.0 { r } else { 1.0 }
    }

    fn build_record(&self, st: &SizeState, c: u16, stroke: bool) -> Option<GlyphRecord> {
        let id = self.glyph_id(c);
        let (xs, _) = self.scales(st);
        let adv_units = self.font.h_advance_unscaled(id).round() as i64;
        let adv26 = ft_mul_fix(adv_units, xs);
        if !st.pixel_mode {
            // 0x0065ea80
            let red = self.reduction(st, id);
            let m = if red < 1.0 {
                let [xx, xy, yx, yy] = st.ft_matrix.map(|v| v as f32 * 1.525_878_9e-5);
                [(xx * red * 65536.0) as i32, -((-xy * red * 65536.0) as i32), -((-yx * red * 65536.0) as i32), (yy * red * 65536.0) as i32]
            } else {
                st.ft_matrix
            };
            let s64 = if stroke { (st.stroke_radius * 64.0 * red + 0.5) as i64 } else { 0 };
            let b = self.cbox(st, id, m, s64);
            Some(GlyphRecord {
                offset: Vec2::new((b.left - 1) as f32 / red, (-1 - b.top) as f32 / red),
                size: Vec2::new((b.width + 2) as i32 as f32 / red, (b.rows + 2) as f32 / red),
                advance: Vec2::new(adv26 as f32 * 0.015625, 0.0),
                has_texture: true,
            })
        } else {
            // 0x0065e340: hinted glyph at the device size. Hinting is not emulated: the
            // advance is rounded to whole pixels as TrueType hinting does for the phantom
            // points; the box is the unhinted one.
            let s64 = if stroke { to_26_6(st.stroke_radius) as i64 } else { 0 };
            let b = self.cbox(st, id, [0x10000, 0, 0, 0x10000], s64);
            Some(GlyphRecord {
                offset: Vec2::new((b.left - 1) as f32, (-1 - b.top) as f32),
                size: Vec2::new((b.width + 2) as f32, (b.rows + 2) as f32),
                advance: Vec2::new((pix_round(adv26) << 10) as f32 * 1.525_878_9e-5, 0.0),
                has_texture: true,
            })
        }
    }

    /// Rasterises the glyph texture (`FT_Glyph_To_Bitmap` + the RGBA copy loop of
    /// `0x0065ea80`/`0x0065e340`). Tier C: coverage comes from `ab_glyph`; the stroke glyph
    /// is approximated by a ring of ±radius around the outline (dilate − erode).
    pub fn rasterize(&mut self, st: &SizeState, c: u16, stroke: bool) -> Option<GlyphBitmap> {
        let rec = self.glyph(st, c, stroke)?;
        let id = self.glyph_id(c);
        let red = if st.pixel_mode { 1.0 } else { self.reduction(st, id) };
        let w = (rec.size.x * red).round() as i64;
        let h = (rec.size.y * red).round() as i64;
        let left = (rec.offset.x * red).round() as i64 + 1;
        let top = -((rec.offset.y * red).round() as i64 + 1);
        let (bw, bh) = ((w - 2).max(0), (h - 2).max(0));
        let mut cov = vec![0f32; (bw * bh) as usize];
        let matrix = if st.pixel_mode { [0x10000, 0, 0, 0x10000] } else { st.ft_matrix };
        let matrix = if red < 1.0 { matrix.map(|v| (v as f32 * red) as i32) } else { matrix };
        let (pts, outline) = self.points_26_6(st, id, matrix);
        if let Some(outline) = outline {
            // Rebuild the outline in pixel units (y up) and let ab_glyph rasterise it.
            let mut k = pts.iter();
            let p = |q: &(i64, i64)| Point { x: q.0 as f32 / 64.0, y: q.1 as f32 / 64.0 };
            let curves: Vec<OutlineCurve> = outline
                .curves
                .iter()
                .map(|cv| {
                    let q = k.next().unwrap();
                    match cv {
                        OutlineCurve::Line(..) => OutlineCurve::Line(p(&q[0]), p(&q[1])),
                        OutlineCurve::Quad(..) => OutlineCurve::Quad(p(&q[0]), p(&q[1]), p(&q[2])),
                        OutlineCurve::Cubic(..) => OutlineCurve::Cubic(p(&q[0]), p(&q[1]), p(&q[2]), p(&q[3])),
                    }
                })
                .collect();
            let mut bounds = ab_glyph::Rect::default();
            let mut first = true;
            for q in pts.iter().flatten() {
                let (x, y) = (q.0 as f32 / 64.0, q.1 as f32 / 64.0);
                if first {
                    bounds = ab_glyph::Rect { min: Point { x, y }, max: Point { x, y } };
                    first = false;
                }
                // ab_glyph stores bounds y-flipped: min = (x_min, y_max), max = (x_max, y_min).
                bounds.min.x = bounds.min.x.min(x);
                bounds.min.y = bounds.min.y.max(y);
                bounds.max.x = bounds.max.x.max(x);
                bounds.max.y = bounds.max.y.min(y);
            }
            let og = OutlinedGlyph::new(
                id.with_scale(1.0),
                Outline { bounds, curves },
                ab_glyph::PxScaleFactor { horizontal: 1.0, vertical: 1.0 },
            );
            let pb = og.px_bounds();
            // px_bounds is y-down; the bitmap's top-left is (left, −top) in that space.
            let (ox, oy) = (pb.min.x as i64 - left, pb.min.y as i64 + top);
            let s64 = if stroke { (st.stroke_radius * red).max(0.0) } else { 0.0 };
            // (A stroke glyph's cbox already includes the radius, so ox/oy carry it.)
            og.draw(|x, y, v| {
                let (bx, by) = (x as i64 + ox, y as i64 + oy);
                if bx >= 0 && by >= 0 && bx < bw && by < bh {
                    cov[(by * bw + bx) as usize] = v;
                }
            });
            if stroke && s64 > 0.0 {
                cov = stroke_ring(&cov, bw as usize, bh as usize, s64);
            }
        }
        let (tw, th) = (bw as u32 + 2, bh as u32 + 2);
        let mut rgba = vec![0u8; (tw * th * 4) as usize];
        for y in 0..th {
            for x in 0..tw {
                let o = ((y * tw + x) * 4) as usize;
                rgba[o] = 0xff;
                rgba[o + 1] = 0xff;
                rgba[o + 2] = 0xff;
                let inside = x >= 1 && y >= 1 && (x as i64) <= bw && (y as i64) <= bh;
                rgba[o + 3] = if inside {
                    (cov[((y as i64 - 1) * bw + (x as i64 - 1)) as usize].clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                } else {
                    0
                };
            }
        }
        Some(GlyphBitmap { width: tw, height: th, rgba })
    }
}

/// Ring of ±`r` px around a coverage mask (Tier C stand-in for `FT_Glyph_Stroke`).
fn stroke_ring(cov: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    // Performance note for a future optimiser: brute-force disc filter, O(n·r²).
    let ri = r.ceil() as i64;
    let mut out = vec![0f32; cov.len()];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (mut hi, mut lo) = (0f32, 1f32);
            for dy in -ri..=ri {
                for dx in -ri..=ri {
                    if ((dx * dx + dy * dy) as f32) > r * r {
                        continue;
                    }
                    let (sx, sy) = (x + dx, y + dy);
                    let v = if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                        cov[(sy as usize) * w + sx as usize]
                    } else {
                        0.0
                    };
                    hi = hi.max(v);
                    lo = lo.min(v);
                }
            }
            out[y as usize * w + x as usize] = (hi - lo).clamp(0.0, 1.0);
        }
    }
    out
}

impl GlyphSource for AbFace {
    fn glyph(&mut self, st: &SizeState, c: u16, stroke: bool) -> Option<GlyphRecord> {
        let key = GlyphKey::new(st, c, stroke);
        if let Some(r) = self.cache.get(&key) {
            return *r;
        }
        let r = self.build_record(st, c, stroke);
        self.cache.insert(key, r);
        r
    }

    fn kerning_raw(&mut self, st: &SizeState, left: u16, right: u16) -> i32 {
        let k = self.font.kern_unscaled(self.glyph_id(left), self.glyph_id(right)).round() as i64;
        if !st.pixel_mode {
            return k as i32;
        }
        // FT_KERNING_DEFAULT (FreeType 2.4 FT_Get_Kerning): scale, damp below 25 ppem,
        // round to the pixel grid.
        let (xs, _) = self.scales(st);
        let mut x = ft_mul_fix(k, xs);
        let x_ppem = (to_26_6(st.size_x) as i64 + 32) >> 6;
        if x_ppem < 25 {
            x = ft_mul_div(x, x_ppem, 25);
        }
        pix_round(x) as i32
    }

    fn units_per_em(&self) -> u16 {
        self.head.units_per_em
    }
}

// ---------------------------------------------------------------------------------------
// ScalableFont
// ---------------------------------------------------------------------------------------

/// `plasma::ScalableFont` (vtable `0x0071ebb0`: slot 0 dtor `0x0065ade0`; the class has no
/// other virtuals). A face plus its size state; all layout calls lock `+0x10c`.
pub struct ScalableFont {
    /// The face (`+0xd8`).
    pub face: AbFace,
    /// The size state (`+0x78..+0x108`).
    pub state: SizeState,
}

/// What `ScalableFont::draw` `0x0065bc70` hands to the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDraw {
    /// The transform set on the engine for the quads (engine slot 15 `setTransform`,
    /// D3D row-major). Transformed mode: pure translation by the rounded translation of
    /// the incoming transform (rotation and scale are baked into the glyph bitmaps).
    /// Pixel mode: the incoming transform with its rows divided by the pixel scale and the
    /// translation rounded.
    pub transform: [f32; 16],
    /// Glyph quads in draw order.
    pub quads: Vec<GlyphQuad>,
    /// The size state the quads were laid out with (the bitmap cache key).
    pub state: SizeState,
}

/// Parameters shared by the `ScalableFont` measure/draw entry points (the TextShape fields
/// `+0x1bc..+0x1ec` or `FontEngine::drawText`'s arguments).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size in layout units.
    pub size: f32,
    /// Stroke radius (0 = none).
    pub stroke_radius: f32,
    /// Extra advance per character.
    pub spacing: f32,
    /// Extra line advance.
    pub line_spacing: f32,
    /// Wrap width (used when `flags & WRAP`).
    pub wrap_width: f32,
    /// [`align`] flags.
    pub flags: u32,
    /// Pixel snapping (TextShape `+0x1f0 & 1`).
    pub pixel_snap: bool,
}

impl ScalableFont {
    /// Wraps a loaded face.
    pub fn new(face: AbFace) -> Self {
        Self { face, state: SizeState::default() }
    }

    fn wrapped(&mut self, text: &[u16], style: &TextStyle, wrap_width: f32) -> Vec<u16> {
        let mut t = text.to_vec();
        if style.flags & align::WRAP != 0 {
            let st = self.state;
            wrap(&mut self.face, &st, &mut t, wrap_width);
        }
        t
    }

    /// `ScalableFont::getBounds` `0x0065e720`: `(min, max)` of `text` in the caller's units
    /// (results are multiplied back by `size / size_x` and `size / size_y`, which undoes the
    /// pixel-mode scale).
    pub fn bounds(&mut self, text: &[u16], transform: &[f32; 16], style: &TextStyle, line_start: i32, pen_mode: bool) -> (Vec2, Vec2) {
        self.state.set(style.size, style.stroke_radius, style.spacing, style.line_spacing, transform, style.pixel_snap);
        let fy = style.size / self.state.size_y;
        let fx = style.size / self.state.size_x;
        let wrap_w = if style.pixel_snap { style.wrap_width / fx } else { style.wrap_width };
        let t = self.wrapped(text, style, wrap_w);
        let st = self.state;
        let (mut a, mut b) = measure(&mut self.face, &st, &t, style.flags, line_start, pen_mode);
        a.x *= fx;
        a.y *= fy;
        b.x *= fx;
        b.y *= fy;
        (a, b)
    }

    /// `ScalableFont::getCaret` `0x0065e8d0` (Edit's caret and selection boxes).
    pub fn caret(&mut self, text: &[u16], index: i32, transform: &[f32; 16], style: &TextStyle) -> (Vec2, Vec2) {
        self.state.set(style.size, style.stroke_radius, style.spacing, style.line_spacing, transform, style.pixel_snap);
        let fy = style.size / self.state.size_y;
        let fx = style.size / self.state.size_x;
        let wrap_w = if style.pixel_snap { style.wrap_width / fx } else { style.wrap_width };
        let t = self.wrapped(text, style, wrap_w);
        let st = self.state;
        let (mut p, mut s) = caret(&mut self.face, &st, &t, index, style.flags);
        p.x *= fx;
        p.y *= fy;
        s.x *= fx;
        s.y *= fy;
        (p, s)
    }

    /// `ScalableFont::draw` `0x0065bc70`: lays out `text` at `origin` under the engine's
    /// current `transform` and returns the quads to draw with `drawTexturedRect`.
    pub fn draw(&mut self, text: &[u16], origin: Vec2, transform: &[f32; 16], style: &TextStyle) -> TextDraw {
        self.state.set(style.size, style.stroke_radius, style.spacing, style.line_spacing, transform, style.pixel_snap);
        let mut wrap_w = style.wrap_width;
        let mut t4 = *transform;
        if style.pixel_snap {
            let fx = style.size / self.state.size_x;
            wrap_w = style.wrap_width / fx;
            let fy = style.size / self.state.size_y;
            if fx != 1.0 {
                for v in &mut t4[0..4] {
                    *v *= fx;
                }
            }
            if fy != 1.0 {
                for v in &mut t4[4..8] {
                    *v *= fy;
                }
            }
            t4[12] = (t4[12] + 0.5) as i32 as f32;
            t4[13] = (t4[13] + 0.5) as i32 as f32;
        } else {
            let tx = (transform[12] + 0.5) as i32 as f32;
            let ty = (transform[13] + 0.5) as i32 as f32;
            t4 = glam::Mat4::IDENTITY.to_cols_array();
            t4[12] = tx;
            t4[13] = ty;
        }
        let t = self.wrapped(text, style, wrap_w);
        let st = self.state;
        let quads = layout_quads(&mut self.face, &st, &t, origin, style.flags);
        TextDraw { transform: t4, quads, state: st }
    }
}

// ---------------------------------------------------------------------------------------
// FontEngine
// ---------------------------------------------------------------------------------------

/// `plasma::FontEngine` (vtable `0x0071e8f8`, ctor `0x00638fa0`, 0x3c bytes; owned by the
/// engine at `Engine+0x34`).
#[derive(Default)]
pub struct FontEngine {
    /// `+0x10`: fonts by file base name (case-sensitive `std::map<wstring>`).
    fonts: HashMap<String, ScalableFont>,
    /// `+0x1c`: search directories (`addSearchPath 0x00639390`).
    pub search_paths: Vec<String>,
    /// `+0x8`: in-memory files by path, consulted first (`0x0065d4c0`).
    pub memory_files: HashMap<String, Vec<u8>>,
}

/// The file loader `FontEngine` uses (the file system in the game; tests substitute it).
pub trait FontFileSource {
    /// Reads a file, `None` if it does not exist.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// Reads fonts from the file system, relative paths against `base` (the working
/// directory of `Cube.exe`, i.e. the game directory).
pub struct DiskFonts {
    /// Base directory for relative paths.
    pub base: PathBuf,
}

impl FontFileSource for DiskFonts {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let p = Path::new(path);
        let p = if p.is_absolute() || path.contains(':') { p.to_path_buf() } else { self.base.join(p) };
        std::fs::read(p).ok()
    }
}

/// The base name as `getFont` computes it: after the last `'/'` or `'\\'`.
pub fn font_base_name(path: &str) -> &str {
    let a = path.rfind('/').map_or(-1, |i| i as i64);
    let b = path.rfind('\\').map_or(-1, |i| i as i64);
    let i = a.max(b);
    &path[(i + 1) as usize..]
}

/// The loader choice of `0x0065f260`: `true` for FreeType (`TTF`, `DAT`), `false` for the
/// `.plx` vector font branch.
pub fn is_freetype_path(path: &str) -> bool {
    let ext = path.rfind('.').map_or(path, |i| &path[i + 1..]).to_ascii_uppercase();
    ext == "TTF" || ext == "DAT"
}

impl FontEngine {
    /// Empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// `addSearchPath 0x00639390`.
    pub fn add_search_path(&mut self, dir: &str) {
        self.search_paths.push(dir.to_string());
    }

    fn load(&self, files: &dyn FontFileSource, path: &str, use_memory: bool) -> Option<AbFace> {
        if !is_freetype_path(path) {
            // 0x0065f3d0 .plx vector font: not ported (no shipped font uses it).
            return None;
        }
        let bytes = match use_memory.then(|| self.memory_files.get(path)).flatten() {
            Some(b) if !b.is_empty() => b.clone(),
            _ => files.read(path)?,
        };
        AbFace::from_bytes(bytes)
    }

    /// `FontEngine::getFont` `0x00639800`: the cached font for `path`'s base name, loading it
    /// on a miss from `path` itself (memory files first), then from each search directory
    /// joined with `"/"`. A failed load leaves no cache entry (so it is retried next time).
    pub fn get_font(&mut self, files: &dyn FontFileSource, path: &str) -> Option<&mut ScalableFont> {
        let name = font_base_name(path).to_string();
        if !self.fonts.contains_key(&name) {
            let mut face = self.load(files, path, true);
            if face.is_none() {
                for dir in &self.search_paths {
                    let p = format!("{dir}{SEARCH_PATH_SEPARATOR}{name}");
                    if let Some(f) = self.load(files, &p, false) {
                        face = Some(f);
                        break;
                    }
                }
            }
            let face = face?;
            self.fonts.insert(name.clone(), ScalableFont::new(face));
        }
        self.fonts.get_mut(&name)
    }

    /// `FontEngine::drawText` `0x00639b30`, the game widgets' text call:
    /// `drawText(font, text, spacing, lineSpacing, x, y, size, strokeRadius, color,
    /// strokeColor, extrusionColor, flags, wrapWidth, pixelSnap)`. Returns `None` when the
    /// font cannot be loaded. Colours are the caller's business (fill = `color`, stroke =
    /// `strokeColor`), as is binding the widget shader (engine slots 16/17 around the call).
    pub fn draw_text(
        &mut self,
        files: &dyn FontFileSource,
        font: &str,
        text: &[u16],
        origin: Vec2,
        transform: &[f32; 16],
        style: &TextStyle,
    ) -> Option<TextDraw> {
        let f = self.get_font(files, font)?;
        Some(f.draw(text, origin, transform, style))
    }
}

// ---------------------------------------------------------------------------------------
// TextShape
// ---------------------------------------------------------------------------------------

/// The `.plx` fields of a `TextShape` chunk (`PlxReader::readTextShape` `0x00685b10`), as
/// the integrator must map them from the `.plx` parser. Offsets are `TextShape` fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextShapeSource {
    /// `TextShape.name` (narrow) / `TextShape.wname` (wide) → `+0xc`.
    pub name: String,
    /// `TextShape.string`: keyed `Attribute.frame` / `Attribute.sequence` wide strings
    /// (`+0x5c` attribute, values `+0xa8`); one entry per key.
    pub strings: Vec<Vec<u16>>,
    /// `TextShape.color` → keyed colour attribute `+0xb4` ("colors").
    pub color: Vec4,
    /// `TextShape.strokeColor` → `+0x10c` ("strokeColors").
    pub stroke_color: Vec4,
    /// `TextShape.extrusionColor` → `+0x164` ("extrusionColors").
    pub extrusion_color: Vec4,
    /// `TextShape.flags` (int) → `+0x1ec`: [`align`] flags.
    pub flags: u32,
    /// `+0x1f0` bit 0 (pixel snapping): set after reading when the reader's option flags
    /// (`PlxReader+0x60`) have bit 0x20.
    pub pixel_snap: bool,
    /// `+0x1bc`: `TextShape.pixelSize` (float), `TextShape.fontSize` (int → float),
    /// `TextShape.font.size` (float) or `TextShape.font.pixelSize` (int → float).
    pub size: f32,
    /// `+0x1c0`: `TextShape.strokeRadius` (float), `TextShape.strokeWidth` (int · 0.5),
    /// `TextShape.font.strokeWidth` (float · 0.5) or `TextShape.font.glowRadius` (int · 0.5).
    pub stroke_radius: f32,
    /// `+0x1c4`: `TextShape.spacing`.
    pub spacing: f32,
    /// `+0x1c8`: `TextShape.lineSpacing`.
    pub line_spacing: f32,
    /// `+0x1e4`: `TextShape.wrapWidth`.
    pub wrap_width: f32,
    /// `+0x1cc`: `TextShape.fontName` / `wfontName` / `font.fileName` / `font.wfileName`.
    pub font_file: String,
}

/// `plasma::TextShape` (vtable `0x0071ecdc`, ctor `0x00663240`, 0x21c bytes).
///
/// Slot map: 0 dtor `0x00663770`; 1 [`TextShape::rebuild`] `0x00664770`; 2
/// [`TextShape::draw`] `0x00663da0`; 3 [`TextShape::hit_test`] `0x00664260`; 4
/// [`TextShape::contains_point`] `0x00664210`; 5 `false`; 6 [`TextShape::position`]
/// `0x00663c40`; 7 [`TextShape::size`] `0x00663c20`; 8, 9 `false` (with args); 10, 11 `ret`;
/// 12 `ret 0x18`; 13 clone `0x00663c60`; 14 [`TextShape::rebuild_for_transform`]
/// `0x00664920`; 15 `ret 4`; 16 `ret`; 17 `false` (isEmpty); 18 `ret 8`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextShape {
    /// The authored fields.
    pub source: TextShapeSource,
    /// Index of the current key of the string attribute (`+0x7c`).
    pub current_key: usize,
    /// `+0x1e8`: the resolved font file (the requested one or the Arial fallback); `None`
    /// when neither loads.
    pub font: Option<String>,
    /// `+0x1f4`: bounds min.
    pub min: Vec2,
    /// `+0x1fc`: bounds max.
    pub max: Vec2,
    /// `+0x2d`: set by `rebuild` (a Shape "changed" byte).
    pub dirty: bool,
}

impl TextShape {
    /// New shape from its `.plx` fields.
    pub fn new(source: TextShapeSource) -> Self {
        Self { source, current_key: 0, font: None, min: Vec2::ZERO, max: Vec2::ZERO, dirty: false }
    }

    /// The current string key.
    pub fn text(&self) -> &[u16] {
        self.source.strings.get(self.current_key).map_or(&[], |s| s.as_slice())
    }

    /// The layout parameters.
    pub fn style(&self) -> TextStyle {
        TextStyle {
            size: self.source.size,
            stroke_radius: self.source.stroke_radius,
            spacing: self.source.spacing,
            line_spacing: self.source.line_spacing,
            wrap_width: self.source.wrap_width,
            flags: self.source.flags,
            pixel_snap: self.source.pixel_snap,
        }
    }

    fn resolve_font<'e>(&mut self, engine: &'e mut FontEngine, files: &dyn FontFileSource) -> Option<&'e mut ScalableFont> {
        let file = self.source.font_file.clone();
        let name = if engine.get_font(files, &file).is_some() {
            file
        } else if engine.get_font(files, TEXT_SHAPE_FALLBACK_FONT).is_some() {
            TEXT_SHAPE_FALLBACK_FONT.to_string()
        } else {
            self.font = None;
            return None;
        };
        self.font = Some(name.clone());
        engine.get_font(files, &name)
    }

    /// Slot 1 `rebuild` `0x00664770`: resolves the font (falling back to Arial) and, unless
    /// pixel snapping is on, computes the ink bounds under the identity transform.
    pub fn rebuild(&mut self, engine: &mut FontEngine, files: &dyn FontFileSource) {
        self.dirty = true;
        let text = self.text().to_vec();
        let style = self.style();
        let Some(font) = self.resolve_font(engine, files) else {
            self.min = Vec2::ZERO;
            self.max = Vec2::ZERO;
            return;
        };
        if !style.pixel_snap {
            let id = glam::Mat4::IDENTITY.to_cols_array();
            let (a, b) = font.bounds(&text, &id, &style, -1, false);
            self.min = a;
            self.max = b;
        }
    }

    /// Slot 14 `0x00664920`: with pixel snapping, the bounds depend on the transform; the
    /// owner calls this with the node's transform.
    pub fn rebuild_for_transform(&mut self, engine: &mut FontEngine, files: &dyn FontFileSource, transform: &[f32; 16]) {
        let Some(name) = self.font.clone() else { return };
        if !self.source.pixel_snap {
            return;
        }
        let text = self.text().to_vec();
        let style = self.style();
        if let Some(font) = engine.get_font(files, &name) {
            let (a, b) = font.bounds(&text, transform, &style, -1, false);
            self.min = a;
            self.max = b;
        }
    }

    /// Slot 2 `draw` `0x00663da0`: the text at the shape origin under `transform`.
    pub fn draw(&self, engine: &mut FontEngine, files: &dyn FontFileSource, transform: &[f32; 16]) -> Option<TextDraw> {
        let name = self.font.as_ref()?;
        let font = engine.get_font(files, name)?;
        Some(font.draw(self.text(), Vec2::ZERO, transform, &self.style()))
    }

    /// Slot 4 `containsPoint` `0x00664210`: inclusive bounds test.
    pub fn contains_point(&self, p: Vec2) -> bool {
        self.min.x <= p.x && self.min.y <= p.y && p.x <= self.max.x && p.y <= self.max.y
    }

    /// Slot 6 `getPosition` `0x00663c40`: `min`.
    pub fn position(&self) -> Vec2 {
        self.min
    }

    /// Slot 7 `getSize` `0x00663c20`: returns `max` (not `max − min`), as the original.
    pub fn size(&self) -> Vec2 {
        self.max
    }

    /// Slot 3 `0x00664260` (`p`, `radius`, bounds transform `m1`, point transform `m2`, D3D
    /// row-major): true if `m2·p` is inside the bounds, or if `p` is within `radius` of an
    /// edge of the bounds rectangle transformed by `m1` (perspective divide included).
    pub fn hit_test(&self, p: Vec2, radius: f32, m1: &[f32; 16], m2: &[f32; 16]) -> bool {
        let tf = |m: &[f32; 16], v: Vec2| -> Vec2 {
            let w = 1.0 / (m[3] * v.x + m[7] * v.y + m[15]);
            Vec2::new(w * (m[0] * v.x + m[4] * v.y + m[12]), w * (m[1] * v.x + m[5] * v.y + m[13]))
        };
        if self.contains_point(tf(m2, p)) {
            return true;
        }
        let c = [
            tf(m1, Vec2::new(self.min.x, self.min.y)),
            tf(m1, Vec2::new(self.max.x, self.min.y)),
            tf(m1, Vec2::new(self.max.x, self.max.y)),
            tf(m1, Vec2::new(self.min.x, self.max.y)),
        ];
        for i in 0..4 {
            let a = c[i];
            let b = c[(i + 1) & 3];
            let pa = p - a;
            let ab = b - a;
            let l2 = ab.y * ab.y + ab.x * ab.x;
            let d2 = if 1e-20 <= l2 {
                let t = (pa.y * ab.y + pa.x * ab.x) / l2;
                if 0.0 < t {
                    let q = if t < 1.0 { pa - ab * t } else { p - b };
                    q.y * q.y + q.x * q.x
                } else {
                    pa.y * pa.y + pa.x * pa.x
                }
            } else {
                pa.y * pa.y + pa.x * pa.x
            };
            if d2 <= radius * radius {
                return true;
            }
        }
        false
    }
}

/// The Edit caret query (`ScalableFont::getCaret` `0x0065e8d0` on the edit's TextShape)
/// as [`crate::widget::TextMetrics`]: the caret box of character `index` gives its left
/// edge (x) and advance width.
pub struct TextShapeMetrics<'a> {
    /// The font engine (behind a `RefCell`: layout queries update the size state).
    pub engine: std::cell::RefCell<&'a mut FontEngine>,
    /// Where fonts are read from.
    pub files: &'a dyn FontFileSource,
    /// The resolved font file (`TextShape + 0x1e8`).
    pub font: String,
    /// The shape's layout parameters.
    pub style: TextStyle,
    /// The node transform the shape is laid out under (D3D row-major).
    pub transform: [f32; 16],
}

impl<'a> TextShapeMetrics<'a> {
    /// Metrics of `shape` (its resolved font, or the requested one before a rebuild).
    pub fn new(engine: &'a mut FontEngine, files: &'a dyn FontFileSource, shape: &TextShape, transform: [f32; 16]) -> Self {
        let font = shape.font.clone().unwrap_or_else(|| shape.source.font_file.clone());
        Self { engine: std::cell::RefCell::new(engine), files, font, style: shape.style(), transform }
    }
}

impl crate::widget::TextMetrics for TextShapeMetrics<'_> {
    fn glyph_box(&self, text: &[u16], index: usize) -> (f32, f32) {
        let mut e = self.engine.borrow_mut();
        match e.get_font(self.files, &self.font) {
            Some(f) => {
                let (p, s) = f.caret(text, index as i32, &self.transform, &self.style);
                (p.x, s.x)
            }
            None => (0.0, 0.0),
        }
    }
}

// ---------------------------------------------------------------------------------------
// plasma::Font / PixelFont / PlasmaFont (.pla only)
// ---------------------------------------------------------------------------------------

/// `plasma::Font` (vtable `0x0072053c`, ctor `0x00687d20`): a NamedObject registered in the
/// engine's font list with a numeric id (`+0x30`, one more than the largest id in use).
/// Slots 1–4 are pure; slot 5 is `ret 0x14`. Only the `.pla` reader creates the two
/// subclasses below, and no `.pla` file ships, so only the size getters are ported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontBase {
    /// `+0xc`: name.
    pub name: String,
    /// `+0x30`: id.
    pub id: i32,
}

/// `plasma::PixelFont` (vtable `0x0071f724`, ctor `0x0067f290`, 0x74 bytes). Slots: 1
/// `0x0067f720`, 2 `0x0067f450` (not ported), 3 [`PixelFont::cell_size`] `0x0067f420`, 4
/// [`PixelFont::offset`] `0x0067f400`, 5 `ret 0x14`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PixelFont {
    /// Base.
    pub base: FontBase,
    /// `+0x64/+0x68`.
    pub cell: Vec2,
    /// `+0x6c/+0x70`.
    pub offset: Vec2,
}

impl PixelFont {
    /// Slot 3 `0x0067f420`.
    pub fn cell_size(&self) -> Vec2 {
        self.cell
    }
    /// Slot 4 `0x0067f400`.
    pub fn offset(&self) -> Vec2 {
        self.offset
    }
}

/// `plasma::PlasmaFont` (vtable `0x0071f708`, ctor `0x0067e210`, 0x70 bytes). Slots: 1
/// `0x0067eed0`, 2 `0x0067ebb0`, 4 `0x0067e400`, 5 `0x0067e660` (not ported), 3
/// [`PlasmaFont::cell_size`] `0x0067e440`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlasmaFont {
    /// Base.
    pub base: FontBase,
    /// `+0x4c`: size.
    pub size: f32,
    /// `+0x60/+0x64`: cell size in 1/200 em.
    pub cell: Vec2,
}

impl PlasmaFont {
    /// Slot 3 `0x0067e440`: `cell · 0.005 · size`.
    pub fn cell_size(&self) -> Vec2 {
        Vec2::new(self.cell.x * 0.005 * self.size, self.cell.y * 0.005 * self.size)
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Monospaced synthetic face: every glyph advances 10, ink box 8×10 at (1, −10).
    struct Mono {
        kern: i32,
    }

    impl GlyphSource for Mono {
        fn glyph(&mut self, _st: &SizeState, c: u16, _stroke: bool) -> Option<GlyphRecord> {
            let (w, h) = if c == SPACE { (0.0, 0.0) } else { (8.0, 10.0) };
            let (left, top) = if c == SPACE { (0.0, 0.0) } else { (1.0, 10.0) };
            Some(GlyphRecord {
                offset: Vec2::new(left - 1.0, -1.0 - top),
                size: Vec2::new(w + 2.0, h + 2.0),
                advance: Vec2::new(10.0, 0.0),
                has_texture: true,
            })
        }
        fn kerning_raw(&mut self, _st: &SizeState, _l: u16, _r: u16) -> i32 {
            self.kern
        }
        fn units_per_em(&self) -> u16 {
            1000
        }
    }

    fn st(size: f32) -> SizeState {
        let mut s = SizeState::default();
        s.set(size, 0.0, 0.0, 0.0, &glam::Mat4::IDENTITY.to_cols_array(), false);
        s
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn measure_ink_bounds_single_line() {
        let s = st(12.0);
        let (a, b) = measure(&mut Mono { kern: 0 }, &s, &w("abc"), 0, -1, false);
        // First glyph ink at x 1..9, last glyph at 21..29; y −10..0.
        assert_eq!(a, Vec2::new(1.0, -10.0));
        assert_eq!(b, Vec2::new(29.0, 0.0));
    }

    #[test]
    fn measure_pen_bounds_and_lines() {
        let mut s = st(12.0);
        s.line_spacing = 3.0;
        let (a, b) = measure(&mut Mono { kern: 0 }, &s, &w("ab\ncde"), 0, -1, true);
        assert_eq!(a, Vec2::new(0.0, -12.0));
        // Pen after "cde" = 30 on the second line at y = 12 + 3.
        assert_eq!(b, Vec2::new(30.0, 15.0));
        // Second line alone.
        let (_, b) = measure(&mut Mono { kern: 0 }, &s, &w("ab\ncde"), 0, 3, true);
        assert_eq!(b, Vec2::new(30.0, 0.0));
    }

    #[test]
    fn spacing_and_kerning_between_characters_only() {
        let mut s = st(10.0);
        s.spacing = 2.0;
        // Kerning −100 units at size 10 / upem 1000 = −1 px.
        let (_, b) = measure(&mut Mono { kern: -100 }, &s, &w("ab"), 0, -1, false);
        // pen after 'a' = 10 − 1 + 2 = 11; 'b' ink max = 11 + 9 = 20.
        assert_eq!(b.x, 20.0);
    }

    #[test]
    fn alignment_flags() {
        let s = st(12.0);
        let t = w("abcd");
        let (a, b) = measure(&mut Mono { kern: 0 }, &s, &t, align::H_CENTER, -1, false);
        assert_eq!((a.x + b.x) * 0.5, 0.0);
        let (_, b) = measure(&mut Mono { kern: 0 }, &s, &t, align::RIGHT, -1, false);
        assert_eq!(b.x, 0.0);
        let (a, b) = measure(&mut Mono { kern: 0 }, &s, &t, align::V_CENTER, -1, false);
        assert_eq!((a.y + b.y) * 0.5, 0.0);
    }

    #[test]
    fn wrap_breaks_at_last_space() {
        let s = st(12.0);
        let mut t = w("aa bb cc");
        wrap(&mut Mono { kern: 0 }, &s, &mut t, 45.0);
        // "aa bb" reaches 50 at the second space (index 5) → the first space breaks; the
        // last character is a candidate too, and "bb cc" (50) overflows again there.
        assert_eq!(String::from_utf16(&t).unwrap(), "aa\nbb\ncc");
        let mut t = w("aa bb cc");
        wrap(&mut Mono { kern: 0 }, &s, &mut t, 55.0);
        assert_eq!(String::from_utf16(&t).unwrap(), "aa bb\ncc");
        let mut t = w("aa bb cc");
        wrap(&mut Mono { kern: 0 }, &s, &mut t, 1000.0);
        assert_eq!(String::from_utf16(&t).unwrap(), "aa bb cc");
    }

    #[test]
    fn caret_positions() {
        let s = st(12.0);
        let t = w("ab\ncd");
        let mut m = Mono { kern: 0 };
        assert_eq!(caret(&mut m, &s, &t, 0, 0), (Vec2::new(0.0, -12.0), Vec2::new(10.0, 12.0)));
        assert_eq!(caret(&mut m, &s, &t, 1, 0).0, Vec2::new(10.0, -12.0));
        assert_eq!(caret(&mut m, &s, &t, 4, 0).0, Vec2::new(10.0, 0.0));
        // Past the end: clamped to len, caret after the last character, width 0.
        assert_eq!(caret(&mut m, &s, &t, 99, 0), (Vec2::new(20.0, 0.0), Vec2::new(0.0, 12.0)));
    }

    #[test]
    fn caret_skips_spacing_before_index() {
        // The original adds spacing only while i < index − 1.
        let mut s = st(12.0);
        s.spacing = 5.0;
        let (p, _) = caret(&mut Mono { kern: 0 }, &s, &w("abc"), 2, 0);
        assert_eq!(p.x, 10.0 + 5.0 + 10.0);
    }

    #[test]
    fn quads_follow_pen_and_lines() {
        let s = st(12.0);
        let q = layout_quads(&mut Mono { kern: 0 }, &s, &w("ab\nc"), Vec2::new(100.0, 50.0), 0);
        assert_eq!(q.len(), 3);
        assert_eq!(q[0].pos, Vec2::new(100.0, 39.0));
        assert_eq!(q[1].pos, Vec2::new(110.0, 39.0));
        assert_eq!(q[2].pos, Vec2::new(100.0, 51.0));
        assert!(q.iter().all(|g| g.layer == GlyphLayer::Fill && g.size == Vec2::new(10.0, 12.0)));
    }

    #[test]
    fn centred_quads_are_centred_per_line() {
        let s = st(12.0);
        let q = layout_quads(&mut Mono { kern: 0 }, &s, &w("abcd"), Vec2::ZERO, align::H_CENTER);
        // Pen width 40 → the line starts at −20.
        assert_eq!(q[0].pos.x, -20.0);
    }

    #[test]
    fn pixel_sizes_round_and_scale() {
        let mut m = glam::Mat4::IDENTITY.to_cols_array();
        m[0] = 2.0;
        m[5] = 0.5;
        let (px, stroke, sp, ls) = pixel_sizes(&m, 10.0, 1.0, 1.0, 2.0);
        assert_eq!(px, Vec2::new(20.0, 5.0));
        assert_eq!(stroke, 1.25);
        assert_eq!(sp, 2.0);
        assert_eq!(ls, 1.0);
    }

    #[test]
    fn fixed_point_matches_freetype() {
        assert_eq!(ft_mul_fix(1000, 0x10000), 1000);
        assert_eq!(ft_mul_fix(-3, 0x8000), -2); // rounds half away from zero
        assert_eq!(ft_div_fix(12 * 64, 2048), 24576);
    }

    #[test]
    fn base_names_and_loaders() {
        assert_eq!(font_base_name(r"c:\windows\fonts/tahoma.ttf"), "tahoma.ttf");
        assert_eq!(font_base_name("resource1.dat"), "resource1.dat");
        assert!(is_freetype_path("resource2.dat"));
        assert!(is_freetype_path("x/Arial.TtF"));
        assert!(!is_freetype_path("font.plx"));
    }

    #[test]
    fn text_shape_hit_test() {
        let mut t = TextShape::new(TextShapeSource::default());
        t.min = Vec2::new(0.0, -10.0);
        t.max = Vec2::new(50.0, 0.0);
        let id = glam::Mat4::IDENTITY.to_cols_array();
        assert!(t.hit_test(Vec2::new(10.0, -5.0), 0.0, &id, &id));
        assert!(t.hit_test(Vec2::new(52.0, -5.0), 3.0, &id, &id));
        assert!(!t.hit_test(Vec2::new(60.0, -5.0), 3.0, &id, &id));
        assert_eq!(t.size(), t.max);
    }

    fn game_dir() -> Option<PathBuf> {
        let p = std::env::var("CW_GAME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../game"));
        p.join("resource1.dat").exists().then_some(p)
    }

    #[test]
    fn game_fonts_load_and_lay_out() {
        let Some(dir) = game_dir() else {
            eprintln!("skipping: game/resource1.dat not found (set CW_GAME_DIR)");
            return;
        };
        let files = DiskFonts { base: dir };
        let mut fe = FontEngine::new();
        fe.add_search_path(ENGINE_FONT_SEARCH_PATH);
        for name in GAME_FONTS {
            let f = fe.get_font(&files, name).expect("game font loads");
            assert_eq!(f.face.family_name, name.trim_end_matches(".dat"));
        }
        let f = fe.get_font(&files, "resource1.dat").unwrap();
        let id = glam::Mat4::IDENTITY.to_cols_array();
        let style = TextStyle { size: 16.0, stroke_radius: 0.0, spacing: 0.0, line_spacing: 0.0, wrap_width: 0.0, flags: 0, pixel_snap: false };
        let (a, b) = f.bounds(&w("Cube World"), &id, &style, -1, false);
        assert!(a.x >= -2.0 && a.y < 0.0 && b.x > 40.0 && b.y >= 0.0, "{a:?} {b:?}");
        let d = f.draw(&w("Cube World"), Vec2::ZERO, &id, &style);
        assert_eq!(d.quads.len(), 10);
        // Pixel mode lays out on whole pixels.
        let px = TextStyle { pixel_snap: true, ..style };
        let d = f.draw(&w("AV AV"), Vec2::ZERO, &id, &px);
        assert!(d.quads.iter().all(|q| q.pos.x.fract() == 0.0));
        let st = f.state;
        let bmp = f.face.rasterize(&st, 'A' as u16, false).unwrap();
        assert!(bmp.rgba.chunks(4).any(|p| p[3] > 128));
        // Border column is transparent.
        assert!((0..bmp.height).all(|y| bmp.rgba[(y * bmp.width * 4 + 3) as usize] == 0));
        // Stroked text: one fill and one stroke quad per character, stroke box larger.
        let stroked = TextStyle { stroke_radius: 1.5, ..style };
        let d = f.draw(&w("Ab"), Vec2::ZERO, &id, &stroked);
        assert_eq!(d.quads.iter().filter(|q| q.layer == GlyphLayer::Stroke).count(), 2);
        assert!(d.quads[1].size.x > d.quads[0].size.x);
        let st = f.state;
        let sb = f.face.rasterize(&st, 'A' as u16, true).unwrap();
        assert!(sb.rgba.chunks(4).any(|p| p[3] > 128));
    }

    #[test]
    fn tahoma_from_system_fonts() {
        let files = DiskFonts { base: PathBuf::from(".") };
        let mut fe = FontEngine::new();
        fe.add_search_path("C:/Windows/Fonts");
        let Some(f) = fe.get_font(&files, "tahoma.ttf") else {
            eprintln!("skipping: C:/Windows/Fonts/tahoma.ttf not found");
            return;
        };
        let id = glam::Mat4::IDENTITY.to_cols_array();
        let style = TextStyle { size: 12.0, stroke_radius: 0.0, spacing: 0.0, line_spacing: 0.0, wrap_width: 0.0, flags: 0, pixel_snap: false };
        let (p, s) = f.caret(&w("Hello"), 5, &id, &style);
        assert!(p.x > 20.0 && s.y == 12.0);
    }
}
