//! The game's single noise primitive.

/// Pi as the original stores it: the `float` constant widened to `double`
/// (`Server.exe` data at 0x00573788, bytes `97d17e5afb210940`).
const PI_F: f64 = f64::from_bits(0x400921FB5A7ED197);

/// Integer hash of one lattice point, `Server.exe 0x004d5d30` inner sequence.
///
/// All arithmetic is wrapping 32-bit, as on x86. The result is in `0..=0x7fffffff`.
#[inline]
fn hash(n: i32) -> i32 {
    let m = n ^ (n << 13);
    let h = m
        .wrapping_mul(m)
        .wrapping_mul(0xec4d)
        .wrapping_add(0x131071f)
        .wrapping_mul(m)
        .wrapping_sub(0x2df722f3);
    h & 0x7fffffff
}

/// Lattice value in `(-1, 1]`.
#[inline]
fn lattice(n: i32) -> f64 {
    1.0 - f64::from(hash(n)) * 9.313225746154785e-10 // 2^-30, `Server.exe` data at 0x00573728
}

/// Two-dimensional value noise with cosine interpolation, `Server.exe 0x004d5d30`.
///
/// Called throughout world generation as `value_noise_2d(bx as f64 * scale + offset, ...)`.
/// The lattice cell is chosen by truncation toward zero (`cvttsd2si`), the four corner
/// hashes are combined as `x + y * 57` in wrapping `i32`, and the result is computed in
/// `f64` and rounded once to `f32`.
pub fn value_noise_2d(x: f64, y: f64) -> f32 {
    // `cvttsd2si` saturates to i32::MIN on overflow and NaN; `as` does the same for the
    // in-range cases used here (all inputs are far inside i32).
    let ix = x as i32;
    let iy = y as i32;
    let fx = f64::from(ix);
    let fy = f64::from(iy);
    let ix1 = (fx + 1.0) as i32;
    let iy1 = (fy + 1.0) as i32;

    let row0 = iy.wrapping_mul(57);
    let row1 = iy1.wrapping_mul(57);
    let a = ix.wrapping_add(row0);
    let b = row0.wrapping_add(ix1);
    let c = ix.wrapping_add(row1);
    let d = row1.wrapping_add(ix1);

    let sx = (1.0 - crate::cos((x - fx) * PI_F)) * 0.5;
    let sy = (1.0 - crate::cos((y - fy) * PI_F)) * 0.5;

    let top = lattice(a) * (1.0 - sx) + lattice(b) * sx;
    let bottom = lattice(c) * (1.0 - sx) + lattice(d) * sx;
    (top * (1.0 - sy) + bottom * sy) as f32
}
