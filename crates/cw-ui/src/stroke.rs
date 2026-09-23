//! The stroke of a `plasma::SmoothMeshShape` (Cube.exe): the strip geometry built by the
//! stroke section of `SmoothMeshShape::rebuild` 0x00644fa0, and the stroke branch of
//! `buildDrawings` 0x00648d60 that turns it into the stroke drawing (+0xc14), with the caps
//! (0x0063e020 round, 0x0063ea00 square) and the dash splitter 0x0063f3b0.
//!
//! Tier B: the geometry rules and the f32 operation order of the original (sums of two
//! terms may be commuted; longer sums and all products keep their grouping).
//!
//! # Address map
//!
//! | Original | Rust |
//! |---|---|
//! | 0x00644fa0, the per-curve resize of +0xbc0/+0xbcc/+0xbd8/+0xbe4 (≈0x006452xx) | [`build_stroke`] (setup) |
//! | 0x00644fa0, stroke loop per curve point (≈0x00645e00..0x006484f0) | [`build_stroke`] |
//! | ── open start point | [`StrokeBuilder::open_start`] |
//! | ── open end point | [`StrokeBuilder::open_end`] |
//! | ── interior / closed point: bisector, miter at FIXED corners | [`StrokeBuilder::interior`] |
//! | ── bevel joint (strokeJointType ≠ 1) | [`StrokeBuilder::interior`] (bevel arm) |
//! | ── round joint (strokeJointType = 1), rotation by 0x00423e70 identity + sin/cos | [`StrokeBuilder::interior`] (round arm) |
//! | ── u advance `0x006426d0(len, w0, w1)` = `len·2/(w0+w1)` | [`u_advance`] |
//! | 0x00648d60 stroke branch (solid) | [`fill_stroke_drawing`] |
//! | 0x0063e020 round cap | [`round_cap`] |
//! | 0x0063ea00 square cap | [`square_cap`] |
//! | 0x0063f3b0 dash splitter | [`dashes`] |
//! | 0x006414c0 walk along the curve by a u distance | [`walk`] |
//! | 0x0063ef20 emit an interpolated strip pair | [`emit_pair`] |
//!
//! Helpers of the original (all trivial): 0x004db110 `vector<vector<T>>::operator[]`
//! (stride 0xc), 0x00468c70 / 0x00428980 / 0x00468c60 element address (stride 8 / 16 / 4),
//! 0x00468f20 Vector2 add (`out = b + a`), 0x00468df0 Vector2 sub (`out = a - b`),
//! 0x0040ea50 Vector2 ctor, 0x00642590 `vector<Vector2>::push_back`, 0x0042bd20
//! `vector<Vector4>::push_back`, 0x0066add0 `vector<int>::push_back`, 0x00487f50 /
//! 0x00487f60 element counts (4 / 8 byte elements), 0x00423ee0 Vector2 length,
//! 0x00423e70 identity matrix, 0x00428ac0 / 0x00428ba0 / 0x00428c80 / 0x00428d00 vector
//! reallocation.
//!
//! # Field values
//!
//! - `strokeJointType` (+0x860): 1 round joint, anything else a bevel joint. Joints are made
//!   only at original (level-0) curve points whose kind is [`kind::FIXED`] and whose turn is
//!   sharper than `acos(0.95)`; every other point gets one pair on the (unit) bisector.
//! - `strokeCapType` (+0x864): 1 round cap (20 segments, +0xb28), 2 square cap, anything
//!   else none. Solid strokes get caps only when open; every dash gets caps (round dashes
//!   with 10 segments, `+0xb28 / 2`).
//! - `strokeAlignment` (+0x868): an integer multiplier of the half-width offset added to
//!   both sides of the strip: 0 centred, 1 / -1 shifted by half the width to one side.
//! - `strokePattern` (+0xbf8): 0 solid; non-zero dashed (unless `strokeGap` is 0), with
//!   `strokeDash` / `strokeGap` (+0xbf0 / +0xbf4) in "u" units (length / width).

// Operand order follows the original's f32 evaluation.
#![allow(clippy::assign_op_pattern, clippy::neg_multiply)]

use crate::drawing::{Drawing, Vec2, Vec4};
use crate::shape::{kind, Curves, ShapeSource, FLAG_OPEN};

/// `SmoothMeshShape+0xb28`: segments of a round cap, 20 (set by the ctor 0x0063c2b0, never
/// written elsewhere). Dashes use half of it.
pub const ROUND_CAP_SEGMENTS: i32 = 20;

/// The stroke strips `rebuild` 0x00644fa0 leaves for `buildDrawings` 0x00648d60, per outline
/// curve, plus the shape fields `buildDrawings` reads with them.
///
/// Each curve's strip is a list of vertex *pairs* (`positions[2k]` on the `+normal` side,
/// `positions[2k+1]` on the `-normal` side).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeGeometry {
    /// +0xbcc: strip positions per curve.
    pub positions: Vec<Vec<Vec2>>,
    /// +0xbd8: strip colours per curve (the subdivided stroke colours).
    pub colors: Vec<Vec<Vec4>>,
    /// +0xbe4: strip uvs per curve: u = accumulated `len·2/(w0+w1)`, v = 0 / 1.
    pub tex_coords: Vec<Vec<Vec2>>,
    /// +0xbc0: per curve point (n points, n + 1 when closed: the first point is repeated at
    /// the end), the index into `positions` of its first vertex.
    pub point_starts: Vec<Vec<i32>>,
    /// +0xba8: the subdivided curve positions (read by the dash walker).
    pub curve_positions: Vec<Vec<Vec2>>,
    /// +0xb9c: the subdivided stroke widths (read by the dash walker).
    pub curve_widths: Vec<Vec<f32>>,
    /// `flags` bit 3 clear.
    pub closed: bool,
    /// +0x864 `strokeCapType`.
    pub cap_type: i32,
    /// +0xb28.
    pub cap_segments: i32,
    /// +0xbf8 `strokePattern`.
    pub pattern: i32,
    /// +0xbf0 `strokeDash`.
    pub dash: f32,
    /// +0xbf4 `strokeGap`.
    pub gap: f32,
}

#[inline]
fn sqrtf(x: f32) -> f32 {
    // libm_sse2_sqrt_precise on the widened value, rounded back: the correctly rounded
    // f32 square root.
    (x as f64).sqrt() as f32
}

#[inline]
fn cosf(x: f32) -> f32 {
    (x as f64).cos() as f32
}

#[inline]
fn sinf(x: f32) -> f32 {
    (x as f64).sin() as f32
}

/// `0x006426d0(len, w0, w1)`: `len·2 / (w0 + w1)`, the u length of a segment (its length
/// in units of the mean stroke width).
pub fn u_advance(len: f32, w0: f32, w1: f32) -> f32 {
    (len * 2.0) / (w0 + w1)
}

/// Per-curve output of the stroke loop.
struct StrokeBuilder<'a> {
    p: &'a [Vec2],
    w: &'a [f32],
    c: &'a [Vec4],
    k: &'a [u32],
    stride: u32,
    align: f32,
    joint: i32,
    pos: Vec<Vec2>,
    col: Vec<Vec4>,
    uv: Vec<Vec2>,
    u: f32,
}

impl StrokeBuilder<'_> {
    fn width(&self, i: usize) -> f32 {
        self.w.get(i).copied().unwrap_or(0.1)
    }

    fn color(&self, i: usize) -> Vec4 {
        self.c.get(i).copied().unwrap_or([0.0; 4])
    }

    /// One pair `(a, b)`, the colour of point `i` twice and the uvs `(u, 0)`, `(u, 1)`.
    fn pair(&mut self, a: Vec2, b: Vec2, i: usize) {
        self.pos.push(a);
        self.pos.push(b);
        let c = self.color(i);
        self.col.push(c);
        self.col.push(c);
        self.uv.push([self.u, 0.0]);
        self.uv.push([self.u, 1.0]);
    }

    /// The common pair shape `((p + v) + al, (p - m) + al)`.
    fn pair_around(&mut self, i: usize, v: Vec2, m: Vec2, al: Vec2) {
        let p = self.p[i];
        let a = [(p[0] + v[0]) + al[0], (p[1] + v[1]) + al[1]];
        let b = [(p[0] - m[0]) + al[0], (p[1] - m[1]) + al[1]];
        self.pair(a, b, i);
    }

    /// Open start (point 0): the normal of the first segment, reversed tangent
    /// `d = p0 - p1` (the point `stride` further when p1 coincides), `n = (-d.y, d.x)`
    /// scaled to half the width.
    fn open_start(&mut self) {
        let p = self.p;
        let len = p.len() as u32;
        let s = self.stride;
        let mut dx = p[0][0] - p[1][0];
        let mut dy = p[0][1] - p[1][1];
        if dy * dy + dx * dx == 0.0 {
            let j = ((((s + 1) as i32 / s as i32) as u32).wrapping_mul(s) % len) as usize;
            dx = p[0][0] - p[j][0];
            dy = p[0][1] - p[j][1];
        }
        let nx = -dy;
        let ny = dx;
        let l = sqrtf(nx * nx + ny * ny);
        let hw = self.width(0) * 0.5;
        let nx = nx * (1.0 / l) * hw;
        let ny = ny * (1.0 / l) * hw;
        let al = [nx * self.align, ny * self.align];
        self.pair_around(0, [nx, ny], [nx, ny], al);
    }

    /// Open end (point `i = len - 1`): `d = p[i-1] - p[i]` (a point further back when they
    /// coincide), same normal rule with the width of point `i`.
    fn open_end(&mut self, i: usize) {
        let p = self.p;
        let len = p.len() as u32;
        let s = self.stride;
        let mut dx = p[i - 1][0] - p[i][0];
        let mut dy = p[i - 1][1] - p[i][1];
        if dy * dy + dx * dx == 0.0 {
            let j = ((len.wrapping_sub(s).wrapping_sub(1).wrapping_add(i as u32)) / s).wrapping_mul(s) % len;
            let j = j as usize;
            dx = p[j][0] - p[i][0];
            dy = p[j][1] - p[i][1];
        }
        let nx = -dy;
        let ny = dx;
        let l = sqrtf(nx * nx + ny * ny);
        let hw = self.width(i) * 0.5;
        let nx = nx * (1.0 / l) * hw;
        let ny = ny * (1.0 / l) * hw;
        let al = [nx * self.align, ny * self.align];
        self.pair_around(i, [nx, ny], [nx, ny], al);
    }

    /// An interior point (or any point of a closed curve).
    fn interior(&mut self, i: usize) {
        let p = self.p;
        let len = p.len() as u32;
        let s = self.stride;
        let iu = i as u32;
        // a = prev - cur (a coincident prev falls back to the previous level-0 point).
        let prev = (len.wrapping_add(iu.wrapping_sub(1)) % len) as usize;
        let mut ax = p[prev][0] - p[i][0];
        let mut ay = p[prev][1] - p[i][1];
        if ay * ay + ax * ax == 0.0 {
            let j = ((len.wrapping_sub(s).wrapping_add(iu.wrapping_sub(1))) / s).wrapping_mul(s) % len;
            let j = j as usize;
            ax = p[j][0] - p[i][0];
            ay = p[j][1] - p[i][1];
        }
        // b = cur - next (a coincident next falls back to the next level-0 point).
        let next = ((iu + 1) % len) as usize;
        let mut bx = p[i][0] - p[next][0];
        let mut by = p[i][1] - p[next][1];
        if by * by + bx * bx == 0.0 {
            let j = ((((iu + 1 + s) as i32 / s as i32) as u32).wrapping_mul(s) % len) as usize;
            bx = p[i][0] - p[j][0];
            by = p[i][1] - p[j][1];
        }
        // n1 = (-a.y, a.x), n2 = (-b.y, b.x), each normalised when non-zero.
        let (mut n1x, mut n1y) = (-ay, ax);
        let l1 = n1x * n1x + n1y * n1y;
        if 0.0 < l1 {
            let r = 1.0 / sqrtf(l1);
            n1x = n1x * r;
            n1y = n1y * r;
        }
        let n1_degenerate = 0.0 >= l1;
        let (mut n2x, mut n2y) = (-by, bx);
        let l2 = n2x * n2x + bx * bx;
        if 0.0 < l2 {
            let r = 1.0 / sqrtf(l2);
            n2x = n2x * r;
            n2y = bx * r;
        }
        // Bisector (unit), miter-scaled by 1/max(dot(n, bis), 0.3) at FIXED corners.
        let corner = (i as i32 % s as i32 == 0) && (self.k.get(i).copied().unwrap_or(0) & 0xffff) as u16 == kind::FIXED;
        let bsx = n2x + n1x;
        let bsy = n2y + n1y;
        let lb = bsy * bsy + bsx * bsx;
        let (mut bisx, mut bisy);
        if lb <= 0.0 {
            bisx = 0.0;
            bisy = 0.0;
        } else {
            let r = 1.0 / sqrtf(lb);
            bisx = bsx * r;
            bisy = bsy * r;
            if corner {
                let (nx, ny) = if n1_degenerate { (n2x, n2y) } else { (n1x, n1y) };
                let dot = ny * bisy + nx * bisx;
                let div = if dot <= 0.3 { 0.3f32 } else { dot };
                bisx = bisx / div;
                bisy = bisy / div;
            }
        }
        let hw = self.width(i) * 0.5;
        let offy = hw * bisy;
        let offx = hw * bisx;
        let off = [offx, offy];
        let al = [offx * self.align, offy * self.align];
        if !corner {
            self.pair_around(i, off, off, al);
            return;
        }
        let dotn = n2y * n1y + n2x * n1x;
        if 0.95 <= dotn {
            // Nearly straight: one pair.
            self.pair_around(i, off, off, al);
            return;
        }
        // Which side is outside: the sign of dot(b, off).
        let side = by * offy + bx * offx;
        if self.joint != 1 {
            // Bevel joint: the incoming normal, the miter point on one side and the mean of
            // the normals on the other, the outgoing normal.
            let n1s = [n1x * hw, n1y * hw];
            let n2s = [n2x * hw, n2y * hw];
            let mid = [(n2s[0] + n1s[0]) * 0.5, (n2s[1] + n1s[1]) * 0.5];
            let (a, b) = if 0.0 <= side { (off, mid) } else { (mid, off) };
            self.pair_around(i, n1s, n1s, al);
            self.pair_around(i, a, b, al);
            self.pair_around(i, n2s, n2s, al);
            return;
        }
        // Round joint: (int)(|angle| / 180 · 20) steps; the outer side rotates from the
        // incoming normal, the inner side stays on the mean of the normals.
        let mut deg = ((dotn as f64).acos() as f32) * 57.29578;
        let steps = ((deg.abs() / 180.0) * 20.0) as i32;
        let flip = side < 0.0;
        if flip {
            deg = -deg;
        }
        let step = deg / steps as f32;
        let n2s = [n2x * hw, n2y * hw];
        let n1s = [n1x * hw, n1y * hw];
        self.pair_around(i, n1s, n1s, al);
        // Rotation about z by `step` degrees applied to the identity (0x00423e70).
        let ang = step * 0.017453292;
        let (c, sn) = (cosf(ang), sinf(ang));
        let (m00, m01, m03) = (1.0f32, 0.0f32, 0.0f32);
        let (m10, m11, m13) = (0.0f32, 1.0f32, 0.0f32);
        let r0x = sn * m10 + c * m00;
        let r1x = c * m10 - sn * m00;
        let r0y = sn * m11 + c * m01;
        let r1y = c * m11 - sn * m01;
        let r0w = sn * m13 + c * m03;
        let r1w = c * m13 - sn * m03;
        let (m30, m31, m33) = (0.0f32, 0.0f32, 1.0f32);
        let rot = |v: Vec2| -> Vec2 {
            let x = v[1] * r1x + v[0] * r0x + m30;
            let y = v[1] * r1y + v[0] * r0y + m31;
            let inv = 1.0 / (v[1] * r1w + v[0] * r0w + m33);
            [x * inv, y * inv]
        };
        let mid = [(n2s[0] + n1s[0]) * 0.5, (n2s[1] + n1s[1]) * 0.5];
        let (mut plus, mut minus) = if flip { (mid, n1s) } else { (n1s, mid) };
        if 1 < steps {
            for _ in 0..steps - 1 {
                if flip {
                    minus = rot(minus);
                } else {
                    plus = rot(plus);
                }
                self.pair_around(i, plus, minus, al);
            }
        }
        self.pair_around(i, n2s, n2s, al);
    }
}

/// The stroke section of `SmoothMeshShape::rebuild` 0x00644fa0: for each subdivided outline
/// curve, one or more vertex pairs per curve point (`n` points when open, `n + 1` when
/// closed, the first point repeated at the end), coloured by the subdivided stroke colours,
/// with u running along the curve in width units.
///
/// `levels` is the subdivision level of 0x0063deb0 (the curve stride is `2^levels`; only
/// points at multiples of the stride can be joints).
///
/// Open curves with fewer than two points and closed curves with none are skipped (the
/// original reads out of bounds or divides by zero there).
pub fn build_stroke(s: &ShapeSource, curves: &Curves, levels: i32) -> StrokeGeometry {
    let open = s.flags & FLAG_OPEN != 0;
    let stride = 1u32 << levels.clamp(0, 30);
    let mut g = StrokeGeometry {
        closed: !open,
        cap_type: s.stroke_cap_type,
        cap_segments: ROUND_CAP_SEGMENTS,
        pattern: s.stroke_pattern,
        dash: s.stroke_dash,
        gap: s.stroke_gap,
        curve_positions: curves.positions.clone(),
        curve_widths: curves.stroke_widths.clone(),
        ..Default::default()
    };
    let empty_c: Vec<Vec4> = Vec::new();
    let empty_w: Vec<f32> = Vec::new();
    let empty_k: Vec<u32> = Vec::new();
    for (ci, p) in curves.positions.iter().enumerate() {
        let len = p.len();
        let count = if open { len } else { len + 1 };
        let mut starts = vec![0i32; count];
        if len == 0 || (open && len < 2) {
            g.positions.push(Vec::new());
            g.colors.push(Vec::new());
            g.tex_coords.push(Vec::new());
            g.point_starts.push(starts);
            continue;
        }
        let mut b = StrokeBuilder {
            p,
            w: curves.stroke_widths.get(ci).unwrap_or(&empty_w),
            c: curves.stroke_colors.get(ci).unwrap_or(&empty_c),
            k: curves.kinds.get(ci).unwrap_or(&empty_k),
            stride,
            align: s.stroke_alignment as f32,
            joint: s.stroke_joint_type,
            pos: Vec::new(),
            col: Vec::new(),
            uv: Vec::new(),
            u: 0.0,
        };
        for (kk, start) in starts.iter_mut().enumerate() {
            let i = kk % len;
            *start = b.pos.len() as i32;
            if open && i == 0 {
                b.open_start();
            } else if open && i == len - 1 {
                b.open_end(i);
            } else {
                b.interior(i);
            }
            // u += len(p[i] - p[i+1]) · 2 / (w[i] + w[i+1]).
            let next = (i + 1) % len;
            let dx = p[i][0] - p[next][0];
            let dy = p[i][1] - p[next][1];
            let l = sqrtf(dx * dx + dy * dy);
            b.u = u_advance(l, b.width(i), b.width(next)) + b.u;
        }
        g.positions.push(b.pos);
        g.colors.push(b.col);
        g.tex_coords.push(b.uv);
        g.point_starts.push(starts);
    }
    g
}

// ---------------------------------------------------------------------------------------
// buildDrawings 0x00648d60, stroke branch
// ---------------------------------------------------------------------------------------

fn push_vertex(st: &mut Drawing, p: Vec2, c: Vec4, uv: Vec2) {
    st.positions.push(p);
    st.colors.push(c);
    st.tex_coords.push(uv);
}

/// The stroke branch of `buildDrawings` 0x00648d60: empties the stroke drawing's
/// positions, colours, uvs and indices, then either (solid: `strokePattern == 0` or
/// `strokeGap == 0`) appends each curve's strip, with caps when the curve is open, and
/// indexes it as a triangle strip (`(i, i+1, i+2)` for even `i`, `(i, i+2, i+1)` for odd,
/// modulo the strip length; closed strips wrap, open ones stop two short), or cuts every
/// curve into dashes ([`dashes`]). Sets the drawing's dirty flags to 0xf.
pub fn fill_stroke_drawing(st: &mut Drawing, g: &StrokeGeometry) {
    st.positions.clear();
    st.colors.clear();
    st.tex_coords.clear();
    st.indices.clear();
    if g.pattern == 0 || g.gap == 0.0 {
        for c in 0..g.positions.len() {
            let base = st.positions.len() as i32;
            let n = g.positions[c].len();
            let open = !g.closed;
            if open && n >= 4 {
                match g.cap_type {
                    1 => round_cap(st, g, c, 0, 0.0, false, g.cap_segments),
                    2 => square_cap(st, g, c, 0, 0.0, false),
                    _ => {}
                }
            }
            for k in 0..n {
                push_vertex(st, g.positions[c][k], g.colors[c][k], g.tex_coords[c][k]);
            }
            if open && n >= 4 {
                match g.cap_type {
                    1 => round_cap(st, g, c, n - 4, 1.0, true, g.cap_segments),
                    2 => square_cap(st, g, c, n - 4, 1.0, true),
                    _ => {}
                }
            }
            let m = st.positions.len() as i32 - base;
            let count = if open { m - 2 } else { m };
            let mut i = 0i32;
            while i < count {
                st.indices.push((i % m + base) as u32);
                st.indices.push((((i & 1) + 1 + i) % m + base) as u32);
                st.indices.push(((((i + 1) % 2) + i + 1) % m + base) as u32);
                i += 1;
            }
        }
    } else {
        for c in 0..g.curve_positions.len() {
            dashes(st, g, c, g.dash, g.gap);
        }
    }
    st.flags = 0xf;
}

/// `0x0063e020` round cap on the strip of curve `c`, between pair `k` and pair
/// `j = (k + 2) mod len` at parameter `t` (0 at the start, 1 at the end), with `n`
/// segments. Appends `n` pairs to the drawing: a quarter circle from the tip to the sides
/// (start) or from the sides to the tip (end), coloured by the interpolated strip colour,
/// uvs spanning a half width in u.
pub fn round_cap(st: &mut Drawing, g: &StrokeGeometry, c: usize, k: usize, t: f32, end: bool, n: i32) {
    let p = &g.positions[c];
    let uvs = &g.tex_coords[c];
    let cols = &g.colors[c];
    let len = p.len();
    if len == 0 {
        return;
    }
    let j = (k + 2) % len;
    let d = 1.0 - t;
    let cy = d * 0.5 * (p[k + 1][1] + p[k][1]) + t * 0.5 * (p[j + 1][1] + p[j][1]);
    let cx = d * 0.5 * (p[k + 1][0] + p[k][0]) + t * 0.5 * (p[j + 1][0] + p[j][0]);
    let rx = (d * p[k][0] + p[j][0] * t) - cx;
    let ry = (d * p[k][1] + p[j][1] * t) - cy;
    // q = (r.y, -r.x): along the strip, away from it at the start.
    let qx = ry;
    let qy = -rx;
    let uc = uvs[j][0] * t + d * uvs[k][0];
    let vc = 0.5f32;
    let lr = sqrtf(qy * qy + ry * ry);
    let mx = (p[k + 1][0] + p[k][0]) * 0.5 - (p[j + 1][0] + p[j][0]) * 0.5;
    let my = (p[k + 1][1] + p[k][1]) * 0.5 - (p[j + 1][1] + p[j][1]) * 0.5;
    let tmp = (uvs[j][0] - uvs[k][0]) * lr;
    let lm = sqrtf(mx * mx + my * my);
    let du = -(tmp / lm);
    let col: Vec4 = std::array::from_fn(|q| d * cols[k][q] + cols[j][q] * t);
    // uv radius vectors: along the strip (du, 0), across it (0, -0.5).
    let (uqx, uqy) = (du, 0.0f32);
    let (urx, ury) = (0.0f32, -0.5f32);
    let mut a0 = 0.0f32;
    if end {
        a0 = (0.5 / n as f32 + 1.0) * 1.5707964 + 0.0;
    }
    if 0 < n {
        let nd = n as f32 + 0.5;
        for i in 0..n {
            let a = ((i as f32 + 0.5) * 1.5707964) / nd + a0;
            let cs = cosf(a);
            let sn = sinf(a);
            let bx = cx + qx * cs;
            let by = cy + qy * cs;
            st.positions.push([bx + rx * sn, by + ry * sn]);
            st.positions.push([bx - rx * sn, by - ry * sn]);
            st.colors.push(col);
            st.colors.push(col);
            let ub = uc + uqx * cs;
            let vb = vc + uqy * cs;
            st.tex_coords.push([ub + urx * sn, vb + ury * sn]);
            st.tex_coords.push([ub - urx * sn, vb - ury * sn]);
        }
    }
}

/// `0x0063ea00` square cap: one pair half a width beyond the strip end (before pair `k`
/// at the start, beyond pair `j = k + 2` at the end), at `t` as in [`round_cap`].
pub fn square_cap(st: &mut Drawing, g: &StrokeGeometry, c: usize, k: usize, t: f32, end: bool) {
    let p = &g.positions[c];
    let uvs = &g.tex_coords[c];
    let cols = &g.colors[c];
    let len = p.len();
    if len == 0 {
        return;
    }
    let j = (k + 2) % len;
    let d = 1.0 - t;
    let cy = (p[k + 1][1] + p[k][1]) * d * 0.5 + (p[j + 1][1] + p[j][1]) * t * 0.5;
    let cx = (p[k][0] + p[k + 1][0]) * d * 0.5 + (p[j][0] + p[j + 1][0]) * t * 0.5;
    let ry = (p[k][1] * d + p[j][1] * t) - cy;
    let rx = (p[k][0] * d + p[j][0] * t) - cx;
    let uc = uvs[j][0] * t + d * uvs[k][0];
    let mut qy = -rx;
    let mut qx = ry;
    let lr = sqrtf(qy * qy + ry * ry);
    let my = (p[k + 1][1] + p[k][1]) * 0.5 - (p[j + 1][1] + p[j][1]) * 0.5;
    let mx = (p[k][0] + p[k + 1][0]) * 0.5 - (p[j][0] + p[j + 1][0]) * 0.5;
    let tmp = (uvs[j][0] - uvs[k][0]) * lr;
    let lm = sqrtf(mx * mx + my * my);
    let mut du = -(tmp / lm);
    if end {
        qx = ry * -1.0;
        du = -du;
        qy = qy * -1.0;
    }
    let col = [
        cols[j][0] * t + d * cols[k][0],
        cols[j][1] * t + d * cols[k][1],
        cols[j][2] * t + d * cols[k][2],
        d * cols[k][3] + cols[j][3] * t,
    ];
    let bx = cx + qx;
    let by = cy + qy;
    st.positions.push([bx + rx, by + ry]);
    st.positions.push([bx - rx, by - ry]);
    st.colors.push(col);
    st.colors.push(col);
    let u = du + uc;
    st.tex_coords.push([u, 0.0]);
    st.tex_coords.push([u, 1.0]);
}

/// `0x0063ef20`: appends the pair interpolated at `t` between strip pair `idx` and pair
/// `(idx + 2) mod len` (positions, colours and uvs).
pub fn emit_pair(st: &mut Drawing, g: &StrokeGeometry, c: usize, idx: usize, t: f32) {
    let p = &g.positions[c];
    let cols = &g.colors[c];
    let uvs = &g.tex_coords[c];
    let j = (idx + 2) % p.len();
    let d = 1.0 - t;
    st.positions.push([p[idx][0] * d + p[j][0] * t, p[idx][1] * d + p[j][1] * t]);
    st.positions.push([p[idx + 1][0] * d + p[j + 1][0] * t, p[idx + 1][1] * d + p[j + 1][1] * t]);
    st.colors.push(std::array::from_fn(|q| d * cols[idx][q] + cols[j][q] * t));
    st.colors.push(std::array::from_fn(|q| d * cols[idx + 1][q] + cols[j + 1][q] * t));
    st.tex_coords.push([d * uvs[idx][0] + uvs[j][0] * t, d * uvs[idx][1] + uvs[j][1] * t]);
    st.tex_coords.push([uvs[idx + 1][0] * d + uvs[j + 1][0] * t, uvs[idx + 1][1] * d + uvs[j + 1][1] * t]);
}

/// `0x006414c0 walk(curve, seg, t, dist)`: from parameter `t` of segment `seg`, move `dist`
/// u units (length / width, with the width varying linearly along the segment) and return
/// `(ok, seg, t)`. The original recurses into the next segment (wrapping modulo the point
/// count) and fails when the result lies before any segment it passed (a wrap-around).
///
/// Simplification: the recursion is a loop with a cap of `64 · (n + 1)` steps, which
/// returns failure where the original would recurse without end (all segments of zero
/// length).
pub fn walk(g: &StrokeGeometry, c: usize, seg: usize, t: f32, dist: f32) -> (bool, usize, f32) {
    let p = &g.curve_positions[c];
    let w = &g.curve_widths[c];
    let n = p.len();
    let wv = |i: usize| w.get(i).copied().unwrap_or(0.1);
    let (mut s, mut t, mut dist) = (seg, t, dist);
    let mut max_s = seg;
    let cap = 64 * (n + 1);
    for _ in 0..cap {
        let nxt = (s + 1) % n;
        if dist <= 0.0 {
            dist = 0.0;
        }
        let dy = p[s][1] - p[nxt][1];
        let dx = p[s][0] - p[nxt][0];
        let len = sqrtf(dy * dy + dx * dx);
        let w1 = wv(nxt);
        let w0 = wv(s);
        let f7 = (len * t * 2.0) / ((1.0 - t) * w0 + w1 * t + w0) + dist;
        let mut f9 = 1.0 - ((w1 - w0) * f7) / (len * 2.0);
        if f9 < 1e-7 {
            f9 = 1e-7;
        }
        let f9 = (f7 * w0) / f9;
        if f9 <= len {
            return (s >= max_s, s, f9 / len);
        }
        dist = dist - ((len - len * t) * 2.0) / (w1 + w0);
        s = nxt;
        t = 0.0;
        if s > max_s {
            max_s = s;
        }
        // A wrap sets s below max_s; the result then fails as in the original.
    }
    (false, s, 0.0)
}

/// `0x0063f3b0 dashes(curve, dash, gap)`: cuts the strip of curve `c` into dashes. The
/// curve's u length (the sum of [`u_advance`] over its segments) is divided into a whole
/// number of dash + gap periods (the pattern is stretched to fit); each dash gets start
/// and end pairs interpolated on the strip, every strip pair between them, and the caps of
/// `strokeCapType` at both ends (round dashes with half the cap segments); each dash is
/// indexed as its own strip.
pub fn dashes(st: &mut Drawing, g: &StrokeGeometry, c: usize, dash: f32, gap: f32) {
    if g.positions[c].is_empty() {
        return;
    }
    let p = &g.curve_positions[c];
    let w = &g.curve_widths[c];
    let n = p.len();
    if n == 0 {
        return;
    }
    let wv = |i: usize| w.get(i).copied().unwrap_or(0.1);
    let mut segs = n as i32;
    if !g.closed {
        segs -= 1;
    }
    let mut total = 0.0f32;
    for s in 0..segs.max(0) as usize {
        let nxt = (s + 1) % n;
        let dy = p[s][1] - p[nxt][1];
        let dx = p[s][0] - p[nxt][0];
        let len = sqrtf(dy * dy + dx * dx);
        total = (len * 2.0) / (wv(s) + wv(nxt)) + total;
    }
    let count = (total / (dash + gap)) as i64 as u32;
    if count == 0 {
        return;
    }
    let scale = total / ((count as i32) as f32 * (dash + gap));
    let dash = scale * dash;
    let gap = scale * gap;
    let half_caps = g.cap_segments / 2;
    let starts = &g.point_starts[c];
    let lp = g.positions[c].len() as u32;
    let pair_before = |pt: usize| -> usize {
        ((starts[pt % starts.len()] as u32).wrapping_add(lp).wrapping_sub(2) % lp) as usize
    };
    let mut seg = 0usize;
    let mut frac = 0.0f32;
    let mut k = 0u32;
    if (count as i32) <= 0 {
        return;
    }
    loop {
        let base = st.positions.len() as i32;
        let (ok, mut eseg, mut efrac) = walk(g, c, seg, frac, dash);
        if !ok {
            break;
        }
        if k == count && segs <= eseg as i32 {
            eseg = (segs - 1) as usize;
            efrac = 1.0;
            if (eseg as i32) < seg as i32 {
                break;
            }
        }
        let sidx = pair_before(seg + 1);
        let mut pairs = 0i32;
        match g.cap_type {
            1 => {
                round_cap(st, g, c, sidx, frac, false, half_caps);
                pairs = half_caps;
            }
            2 => {
                square_cap(st, g, c, sidx, frac, false);
                pairs = 1;
            }
            _ => {}
        }
        emit_pair(st, g, c, sidx, frac);
        let mut cnt = pairs + 1;
        let eidx = pair_before(eseg + 1);
        if seg < eseg {
            let sp = starts[seg + 1] as u32;
            if sp <= eidx as u32 {
                cnt = cnt + ((eidx as u32 - sp) >> 1) as i32 + 1;
                let mut idx = sp;
                while idx <= eidx as u32 {
                    emit_pair(st, g, c, idx as usize, 0.0);
                    idx += 2;
                }
            }
        }
        emit_pair(st, g, c, eidx, efrac);
        let mut total_pairs = cnt + 1;
        match g.cap_type {
            1 => {
                round_cap(st, g, c, eidx, efrac, true, half_caps);
                total_pairs += half_caps;
            }
            2 => {
                square_cap(st, g, c, eidx, efrac, true);
                total_pairs = cnt + 2;
            }
            _ => {}
        }
        // The gap; its success is not checked.
        let (_, gseg, gfrac) = walk(g, c, eseg, efrac, gap);
        seg = gseg;
        frac = gfrac;
        for q in 0..(total_pairs - 1).max(0) {
            let a = base + q * 2;
            let b = base + (q + 1) * 2;
            let a1 = q * 2 + 1 + base;
            let b1 = q * 2 + 3 + base;
            st.indices.extend_from_slice(&[a as u32, b as u32, a1 as u32, a1 as u32, b as u32, b1 as u32]);
        }
        k += 1;
        if count <= k {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{FLAG_STROKE, Keyed};

    fn curves(pts: Vec<Vec2>, width: f32, kinds: u32, closed: bool) -> Curves {
        let n = pts.len();
        Curves {
            positions: vec![pts],
            colors: vec![vec![[1.0; 4]; n]],
            tex_coords: vec![vec![[0.0; 2]; n]],
            kinds: vec![vec![kinds; n]],
            stroke_colors: vec![vec![[0.0, 0.0, 0.0, 1.0]; n]],
            stroke_widths: vec![vec![width; n]],
            extrusion_front: vec![Vec::new()],
            extrusion_back: vec![Vec::new()],
            closed,
        }
    }

    fn source(open: bool) -> ShapeSource {
        let mut s = ShapeSource::default();
        s.flags = FLAG_STROKE | if open { FLAG_OPEN } else { 0 };
        s.stroke_widths = Keyed::constant(Vec::new());
        s
    }

    #[test]
    fn open_line_is_a_rectangle() {
        let s = source(true);
        let c = curves(vec![[0.0, 0.0], [4.0, 0.0]], 2.0, 0, false);
        let g = build_stroke(&s, &c, 0);
        assert_eq!(g.positions[0], vec![[0.0, -1.0], [0.0, 1.0], [4.0, -1.0], [4.0, 1.0]]);
        assert_eq!(g.tex_coords[0], vec![[0.0, 0.0], [0.0, 1.0], [2.0, 0.0], [2.0, 1.0]]);
        assert_eq!(g.point_starts[0], vec![0, 2]);
        let mut d = Drawing::new();
        fill_stroke_drawing(&mut d, &g);
        assert_eq!(d.positions.len(), 4);
        assert_eq!(d.indices, vec![0, 1, 2, 1, 3, 2]);
        assert_eq!(d.flags, 0xf);
    }

    #[test]
    fn alignment_shifts_both_sides() {
        let mut s = source(true);
        s.stroke_alignment = 1;
        let c = curves(vec![[0.0, 0.0], [4.0, 0.0]], 2.0, 0, false);
        let g = build_stroke(&s, &c, 0);
        assert_eq!(g.positions[0][0], [0.0, -2.0]);
        assert_eq!(g.positions[0][1], [0.0, 0.0]);
    }

    #[test]
    fn closed_square_strips() {
        let sq = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let s = source(false);
        // Smooth points: one pair each, first point repeated.
        let g = build_stroke(&s, &curves(sq.clone(), 2.0, 0, true), 0);
        assert_eq!(g.positions[0].len(), 10);
        assert_eq!(g.point_starts[0], vec![0, 2, 4, 6, 8]);
        let mut d = Drawing::new();
        fill_stroke_drawing(&mut d, &g);
        assert_eq!(d.indices.len(), 30);
        // FIXED corners, bevel joint: three pairs per point; the miter point sits at the
        // corner offset by the scaled bisector (|off| = hw / cos 45°).
        let g = build_stroke(&s, &curves(sq.clone(), 2.0, kind::FIXED as u32, true), 0);
        assert_eq!(g.positions[0].len(), 30);
        // Round joint: 1 + (90/180·20 - 1) + 1 pairs per point.
        let mut sr = source(false);
        sr.stroke_joint_type = 1;
        let g = build_stroke(&sr, &curves(sq, 2.0, kind::FIXED as u32, true), 0);
        assert_eq!(g.positions[0].len(), 5 * 2 * 11);
        // Every rotated vertex stays at the half width from its corner.
        for (k, v) in g.positions[0][..22].iter().enumerate().step_by(2) {
            let r = (v[0] * v[0] + v[1] * v[1]).sqrt();
            assert!((r - 1.0).abs() < 1e-5, "pair {k}: {v:?}");
        }
    }

    #[test]
    fn caps_add_vertices() {
        let c = curves(vec![[0.0, 0.0], [4.0, 0.0]], 2.0, 0, false);
        let mut s = source(true);
        s.stroke_cap_type = 2;
        let g = build_stroke(&s, &c, 0);
        let mut d = Drawing::new();
        fill_stroke_drawing(&mut d, &g);
        assert_eq!(d.positions.len(), 8);
        assert_eq!(d.positions[0], [-1.0, -1.0]);
        assert_eq!(d.positions[1], [-1.0, 1.0]);
        assert_eq!(d.positions[6], [5.0, -1.0]);
        assert_eq!(d.positions[7], [5.0, 1.0]);
        assert_eq!(d.tex_coords[0][0], -0.5);
        assert_eq!(d.tex_coords[6][0], 2.5);
        assert_eq!(d.indices.len(), 6 * 3);
        s.stroke_cap_type = 1;
        let g = build_stroke(&s, &c, 0);
        fill_stroke_drawing(&mut d, &g);
        assert_eq!(d.positions.len(), 4 + 2 * 2 * ROUND_CAP_SEGMENTS as usize);
        // The round cap stays on the circle of radius 1 around the ends.
        for v in &d.positions[..40] {
            let r = (v[0] * v[0] + v[1] * v[1]).sqrt();
            assert!((r - 1.0).abs() < 1e-5, "{v:?}");
            assert!(v[0] <= 1e-6);
        }
    }

    #[test]
    fn dash_pattern_splits_into_strips() {
        let mut s = source(true);
        s.stroke_pattern = 1;
        s.stroke_dash = 1.0;
        s.stroke_gap = 1.0;
        let c = curves(vec![[0.0, 0.0], [20.0, 0.0]], 2.0, 0, false);
        let g = build_stroke(&s, &c, 0);
        let mut d = Drawing::new();
        fill_stroke_drawing(&mut d, &g);
        // u length 10, period 2: five dashes of two pairs each.
        assert_eq!(d.positions.len(), 20);
        assert_eq!(d.indices.len(), 30);
        let xs: Vec<f32> = d.positions.iter().step_by(2).map(|p| p[0]).collect();
        let want = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0, 18.0];
        for (a, b) in xs.iter().zip(want) {
            assert!((a - b).abs() < 1e-4, "{xs:?}");
        }
        assert_eq!(&d.indices[6..12], &[4, 6, 5, 5, 6, 7]);
    }

    #[test]
    fn walk_crosses_segments() {
        let s = source(true);
        let c = curves(vec![[0.0, 0.0], [2.0, 0.0], [4.0, 0.0]], 2.0, 0, false);
        let g = build_stroke(&s, &c, 0);
        let (ok, seg, t) = walk(&g, 0, 0, 0.5, 1.0);
        assert!(ok);
        assert_eq!(seg, 1);
        assert!((t - 0.5).abs() < 1e-6);
    }
}
