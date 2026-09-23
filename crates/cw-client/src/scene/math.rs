//! The small matrix and vector helpers `GameController::render` (`Cube.exe 0x004ac260`) and its
//! draw helpers call, with the original's operation order (Tier B: the float expressions are
//! written in the order the assembly evaluates them, so results match to the last bit given
//! the same inputs; `sin`/`cos` are the MSVCR110 ports of `cw-math`).
//!
//! Matrices are [`D3dMatrix`]: Direct3D row-major, row vectors (`v' = v * M`), translation in
//! row 3; element `m[i][j]` is the original's float `i * 4 + j`. Every "pre" helper applies a
//! local transform (`M = T · M`), like `glTranslate`/`glRotate`.

// `m = expr + m` keeps the original's term order visible.
#![allow(clippy::assign_op_pattern)]

use cw_render::frame::{D3dMatrix, IDENTITY};

/// `1/65536` as the original writes it (`0x006fcd94`).
pub const INV_FIXED: f32 = 1.525_878_9e-5;

/// Degrees to radians as the original multiplies (`0x006fcdac`, 0.017453292).
pub const DEG: f32 = 0.017_453_292;

/// `libm_sse2_cos_precise` of a float widened to double, narrowed back.
#[inline]
pub fn cosf(x: f32) -> f32 {
    cw_math::cos(x as f64) as f32
}

/// `libm_sse2_sin_precise` of a float widened to double, narrowed back.
#[inline]
pub fn sinf(x: f32) -> f32 {
    cw_math::sin(x as f64) as f32
}

/// `__ftol2`: truncation toward zero (saturating in Rust; the original's out-of-range
/// result is `0x8000000000000000`, never reached by the values used here).
#[inline]
pub fn ftol(v: f32) -> i64 {
    v as i64
}

/// The block coordinate `cube::World::getBlockFixed 0x0042f860` uses: a negative value is
/// lowered by one block before the truncating division (so exact negative multiples land one
/// block low, as in the original).
#[inline]
pub fn block_of_fixed(v: i64) -> i32 {
    (if v < 0 { v.wrapping_sub(0x10000) } else { v } / 65536) as i32
}

/// `vec3i64::fromFloat 0x0042c460`: `ftol(v * 65536)` per axis.
#[inline]
pub fn fixed_from_float(v: [f32; 3]) -> [i64; 3] {
    [ftol(v[0] * 65536.0), ftol(v[1] * 65536.0), ftol(v[2] * 65536.0)]
}

/// `vec3f::fromFixed 0x0042c4a0`: `(float)v * (1/65536)` per axis.
#[inline]
pub fn float_from_fixed(v: [i64; 3]) -> [f32; 3] {
    [(v[0] as f32) * INV_FIXED, (v[1] as f32) * INV_FIXED, (v[2] as f32) * INV_FIXED]
}

/// `vec3i64 + vec3i64` (`0x0042c800`), wrapping like the original's `add`/`adc`.
#[inline]
pub fn add3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_add(b[0]), a[1].wrapping_add(b[1]), a[2].wrapping_add(b[2])]
}

/// `vec3i64 - vec3i64` (`vec3i64::operator- 0x0042c7a0`).
#[inline]
pub fn sub3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2])]
}

/// `vec3i64::dot 0x0043ac20`: `Σ (a_i · b_i) / 65536` (each product divided, truncating).
#[inline]
pub fn dot_fixed(a: [i64; 3], b: [i64; 3]) -> i64 {
    (a[0].wrapping_mul(b[0]) / 65536)
        .wrapping_add(a[1].wrapping_mul(b[1]) / 65536)
        .wrapping_add(a[2].wrapping_mul(b[2]) / 65536)
}

/// The distance key of a light or far object: `0x004120f0(dot(d, d))` =
/// `(float)dot · (1/65536)`, the squared distance in blocks².
#[inline]
pub fn dist2_key(a: [i64; 3], b: [i64; 3]) -> f32 {
    let d = sub3(a, b);
    (dot_fixed(d, d) as f32) * INV_FIXED
}

/// `vec3f::lengthSq 0x00424860`: `x² + y² + z²` in that order.
#[inline]
pub fn length_sq(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// `0x00424830`: `x² + y²`.
#[inline]
pub fn length_sq2(v: [f32; 2]) -> f32 {
    v[0] * v[0] + v[1] * v[1]
}

/// `mat4Identity 0x00423e70`.
#[inline]
pub fn identity() -> D3dMatrix {
    IDENTITY
}

/// `0x00424990` (vector argument) and `0x00424a60` (three floats): `M = T(t) · M`, row 3
/// accumulated in the original's term order.
pub fn pre_translate(m: &mut D3dMatrix, t: [f32; 3]) {
    let [x, y, z] = t;
    m[3][0] = m[1][0] * y + m[0][0] * x + m[2][0] * z + m[3][0];
    m[3][1] = m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1];
    m[3][2] = m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2];
    m[3][3] = m[0][3] * x + m[1][3] * y + m[2][3] * z + m[3][3];
}

/// `0x00424730`: `M = S(s) · M`; each row is left untouched when its factor is exactly 1.
pub fn pre_scale(m: &mut D3dMatrix, s: [f32; 3]) {
    if s[0] != 1.0 {
        m[0][0] *= s[0];
        m[0][1] *= s[0];
        m[0][2] *= s[0];
        m[0][3] *= s[0];
    }
    if s[1] != 1.0 {
        m[1][0] *= s[1];
        m[1][1] *= s[1];
        m[1][2] *= s[1];
        m[1][3] *= s[1];
    }
    if s[2] != 1.0 {
        m[2][0] *= s[2];
        m[2][1] *= s[2];
        m[2][2] *= s[2];
        m[2][3] *= s[2];
    }
}

/// `mat4RotateZ 0x00424610`: `M = Rz(degrees) · M`.
pub fn pre_rotate_z(m: &mut D3dMatrix, degrees: f32) {
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    for j in 0..4 {
        let a = m[0][j];
        let b = m[1][j];
        m[0][j] = b * s + a * c;
        m[1][j] = b * c - a * s;
    }
}

/// `0x00412400`: `M = R · M` (full 4x4 product, the original's term order).
pub fn pre_multiply(m: &mut D3dMatrix, r: &D3dMatrix) {
    let old = *m;
    for j in 0..4 {
        let (m0, m1, m2, m3) = (old[0][j], old[1][j], old[2][j], old[3][j]);
        m[0][j] = r[0][0] * m0 + m1 * r[0][1] + m2 * r[0][2] + r[0][3] * m3;
        m[1][j] = r[1][1] * m1 + r[1][0] * m0 + r[1][2] * m2 + m3 * r[1][3];
        m[2][j] = r[2][1] * m1 + r[2][0] * m0 + r[2][2] * m2 + r[2][3] * m3;
        m[3][j] = m1 * r[3][1] + m0 * r[3][0] + r[3][2] * m2 + r[3][3] * m3;
    }
}

/// `0x004241b0(angle, x, y, z)`: `M = R(axis, degrees) · M` for an axis that need not be
/// normalised (a zero axis leaves `M` unchanged).
pub fn pre_rotate_axis(m: &mut D3dMatrix, degrees: f32, axis: [f32; 3]) {
    let len = ((axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]) as f64).sqrt() as f32;
    if len == 0.0 {
        return;
    }
    let c = cosf(degrees * DEG);
    let s = sinf(degrees * DEG);
    let (x, y, z) = (axis[0] / len, axis[1] / len, axis[2] / len);
    let t = 1.0 - c;
    let sz = s * z;
    let xy = t * x * y;
    let xz = t * x * z;
    let yz = t * y * z;
    let r = [
        [x * x * t + c, xy + sz, xz - s * y, 0.0],
        [xy - sz, y * y * t + c, yz + s * x, 0.0],
        [xz + s * y, yz - s * x, z * z * t + c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    pre_multiply(m, &r);
}

/// `0x004248a0`: the point `p` through `M` with the perspective divide (row vector).
pub fn transform_point(m: &D3dMatrix, p: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = p;
    let w = 1.0 / (m[0][3] * x + m[1][3] * y + m[2][3] * z + m[3][3]);
    [
        w * (m[1][0] * y + x * m[0][0] + m[2][0] * z + m[3][0]),
        w * (m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1]),
        w * (m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2]),
    ]
}

/// `mat4MulVec3 0x00488e50`: the direction `v` through the upper 3x3 of `M`.
pub fn transform_vector(m: &D3dMatrix, v: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = v;
    [
        m[1][0] * y + x * m[0][0] + m[2][0] * z,
        m[0][1] * x + m[1][1] * y + m[2][1] * z,
        m[0][2] * x + m[1][2] * y + m[2][2] * z,
    ]
}

/// Render-space translation of a world fixed-point position: `(p + offset) / 65536` for x and
/// y (the render offset `GameController+0x1d8`/`+0x1e0`), `p / 65536` for z.
pub fn render_translation(p: [i64; 3], offset: [i64; 2]) -> [f32; 3] {
    [
        (p[0].wrapping_add(offset[0]) as f32) * INV_FIXED,
        (p[1].wrapping_add(offset[1]) as f32) * INV_FIXED,
        (p[2] as f32) * INV_FIXED,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_z_quarter_turn_maps_x_to_y() {
        let mut m = identity();
        pre_rotate_z(&mut m, 90.0);
        let p = transform_point(&m, [1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn axis_rotation_about_z_matches_rotate_z() {
        let mut a = identity();
        let mut b = identity();
        pre_rotate_z(&mut a, 37.0);
        pre_rotate_axis(&mut b, 37.0, [0.0, 0.0, 1.0]);
        for i in 0..4 {
            for j in 0..4 {
                assert!((a[i][j] - b[i][j]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn translate_then_point() {
        let mut m = identity();
        pre_translate(&mut m, [1.0, 2.0, 3.0]);
        assert_eq!(transform_point(&m, [0.0; 3]), [1.0, 2.0, 3.0]);
    }
}
