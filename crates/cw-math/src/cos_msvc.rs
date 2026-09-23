//! Bit-exact port of MSVCR110's SSE2 `cos`: `msvcr110.dll` export `_libm_sse2_cos_precise`
//! (RVA 0x36f80). The exported `cos` (RVA 0x326e7) dispatches on SSE2 machines to an identical
//! copy at RVA 0x44b07, and `__libm_sse2_cos` (RVA 0xaeea8) is the same code once more.
//!
//! The routine is Intel's SSE2 libm `cos`. Every step of its main path is a scalar or packed
//! SSE2 double operation, each correctly rounded, so transcribing the exact operation order
//! into plain `f64` arithmetic reproduces it bit for bit on any IEEE-754 target. No fused
//! multiply-add is used (Rust never contracts `a * b + c` on its own).
//!
//! # Paths (selected by the top 16 bits of `|x|`: `hw = (bits >> 48) & 0x7fff`)
//!
//! - `hw < 0x3030` (`|x| < 2^-252`): returns `1.0 - |x|`.
//! - `hw <= 0x40f5` (`|x| < 90112`): the SSE2 path, [`sse2_cos`].
//! - otherwise (large, infinite or NaN): the x87 fallback at RVA 0x32775, which executes the
//!   `fcos` instruction on the value and stores the result as a double. Emulated by
//!   [`x87_cos`]; see the caveat there.
//!
//! The DLL also takes the x87 path for *every* input when MXCSR or the x87 control word is not
//! at its default (all exceptions masked, round to nearest). The game never changes them.
//!
//! # SSE2 algorithm
//!
//! 1. `k = round(x * 32/pi)` via the `1.5 * 2^52` shifter (the table index uses the same value
//!    taken with `cvtsd2si`).
//! 2. Cody-Waite reduction with pi/32 split in three parts: `r0 = x - k*P1`, `r = r0 - k*P2`,
//!    and the low part folded into `corr = k*P3 - ((r0 - r) - k*P2)`. `P1` has 33 significant
//!    bits, so `k*P1` is exact for every `k` reachable here.
//! 3. `cos(x) = C_k cos(r) - S_k sin(r)` with `C_k = cos(k pi/32)`, `S_k = sin(k pi/32)` from a
//!    64-entry table indexed by `(k + 16) mod 64`. Entry `[a, b, c, q]` holds
//!    `-S_k = a + q` (`q` a nearby power of two or zero) and `C_k = b + c` (`c` a tail).
//! 4. `sin(r)` and `cos(r) - 1` are Taylor polynomials with coefficients `±1/2!..1/9!`,
//!    evaluated two at a time in packed registers (low lane: sine, high lane: cosine).
//! 5. The big terms `C_hi + q*r + a*r` are summed with exact error compensation, the small
//!    terms (table tail, reduction tail times derivative, polynomial tails) are accumulated in
//!    a fixed order, and the head is added last.
//!
//! # x87 / precision
//!
//! The SSE2 path contains no x87 instruction and no extended precision. Only the
//! `|x| >= 90112` fallback uses the x87 unit (`fld qword`, `fcos`, `fstp qword`), which is the
//! only place 80-bit precision appears: `fcos` produces a 64-bit-significand result (the
//! precision-control field does not apply to transcendental instructions), rounded to double
//! by the store.

/// `[-S_k - q, C_hi, C_lo, q]` for table index `(k + 16) mod 64`, raw bits from
/// `msvcr110.dll` RVA 0x37140 (identical copy at RVA 0x44ca0).
#[rustfmt::skip]
pub(crate) const TABLE: [[u64; 4]; 64] = [
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x3ff0000000000000], // 0
    [0xbf73b92e176d6d31, 0x3fb917a6bc29b42c, 0xbc3e2718e0000000, 0x3ff0000000000000], // 1
    [0xbf93ad06011469fb, 0x3fc8f8b83c69a60b, 0xbc626d19c0000000, 0x3ff0000000000000], // 2
    [0xbfa60bea939d225a, 0x3fd294062ed59f06, 0xbc75d28da0000000, 0x3ff0000000000000], // 3
    [0xbfb37ca1866b95cf, 0x3fd87de2a6aea963, 0xbc672cede0000000, 0x3ff0000000000000], // 4
    [0xbfbe3a6873fa1279, 0x3fde2b5d3806f63b, 0x3c5e0d8920000000, 0x3ff0000000000000], // 5
    [0xbfc592675bc57974, 0x3fe1c73b39ae68c8, 0x3c8b25dd20000000, 0x3ff0000000000000], // 6
    [0xbfcd0dfe53aba2fd, 0x3fe44cf325091dd6, 0x3c68076a20000000, 0x3ff0000000000000], // 7
    [0x3fca827999fcef32, 0x3fe6a09e667f3bcd, 0xbc8bdd3420000000, 0x3fe0000000000000], // 8
    [0x3fc133cc94247758, 0x3fe8bc806b151741, 0xbc82c5e120000000, 0x3fe0000000000000], // 9
    [0x3fac73b39ae68c87, 0x3fea9b66290ea1a3, 0x3c39f630e0000000, 0x3fe0000000000000], // 10
    [0xbf9d4a2c7f909c4e, 0x3fec38b2f180bdb1, 0xbc76e0b180000000, 0x3fe0000000000000], // 11
    [0xbfbe087565455a75, 0x3fed906bcf328d46, 0x3c7457e620000000, 0x3fe0000000000000], // 12
    [0x3fa4a03176acf82d, 0x3fee9f4156c62dda, 0x3c8760b1e0000000, 0x3fd0000000000000], // 13
    [0xbfac1d1f0e5967d5, 0x3fef6297cff75cb0, 0x3c75621720000000, 0x3fd0000000000000], // 14
    [0xbf9ba1650f592f50, 0x3fefd88da3d12526, 0xbc887df640000000, 0x3fc0000000000000], // 15
    [0x0000000000000000, 0x3ff0000000000000, 0x0000000000000000, 0x0000000000000000], // 16
    [0x3f9ba1650f592f50, 0x3fefd88da3d12526, 0xbc887df640000000, 0xbfc0000000000000], // 17
    [0x3fac1d1f0e5967d5, 0x3fef6297cff75cb0, 0x3c75621720000000, 0xbfd0000000000000], // 18
    [0xbfa4a03176acf82d, 0x3fee9f4156c62dda, 0x3c8760b1e0000000, 0xbfd0000000000000], // 19
    [0x3fbe087565455a75, 0x3fed906bcf328d46, 0x3c7457e620000000, 0xbfe0000000000000], // 20
    [0x3f9d4a2c7f909c4e, 0x3fec38b2f180bdb1, 0xbc76e0b180000000, 0xbfe0000000000000], // 21
    [0xbfac73b39ae68c87, 0x3fea9b66290ea1a3, 0x3c39f630e0000000, 0xbfe0000000000000], // 22
    [0xbfc133cc94247758, 0x3fe8bc806b151741, 0xbc82c5e120000000, 0xbfe0000000000000], // 23
    [0xbfca827999fcef32, 0x3fe6a09e667f3bcd, 0xbc8bdd3420000000, 0xbfe0000000000000], // 24
    [0x3fcd0dfe53aba2fd, 0x3fe44cf325091dd6, 0x3c68076a20000000, 0xbff0000000000000], // 25
    [0x3fc592675bc57974, 0x3fe1c73b39ae68c8, 0x3c8b25dd20000000, 0xbff0000000000000], // 26
    [0x3fbe3a6873fa1279, 0x3fde2b5d3806f63b, 0x3c5e0d8920000000, 0xbff0000000000000], // 27
    [0x3fb37ca1866b95cf, 0x3fd87de2a6aea963, 0xbc672cede0000000, 0xbff0000000000000], // 28
    [0x3fa60bea939d225a, 0x3fd294062ed59f06, 0xbc75d28da0000000, 0xbff0000000000000], // 29
    [0x3f93ad06011469fb, 0x3fc8f8b83c69a60b, 0xbc626d19c0000000, 0xbff0000000000000], // 30
    [0x3f73b92e176d6d31, 0x3fb917a6bc29b42c, 0xbc3e2718e0000000, 0xbff0000000000000], // 31
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0xbff0000000000000], // 32
    [0x3f73b92e176d6d31, 0xbfb917a6bc29b42c, 0x3c3e2718e0000000, 0xbff0000000000000], // 33
    [0x3f93ad06011469fb, 0xbfc8f8b83c69a60b, 0x3c626d19c0000000, 0xbff0000000000000], // 34
    [0x3fa60bea939d225a, 0xbfd294062ed59f06, 0x3c75d28da0000000, 0xbff0000000000000], // 35
    [0x3fb37ca1866b95cf, 0xbfd87de2a6aea963, 0x3c672cede0000000, 0xbff0000000000000], // 36
    [0x3fbe3a6873fa1279, 0xbfde2b5d3806f63b, 0xbc5e0d8920000000, 0xbff0000000000000], // 37
    [0x3fc592675bc57974, 0xbfe1c73b39ae68c8, 0xbc8b25dd20000000, 0xbff0000000000000], // 38
    [0x3fcd0dfe53aba2fd, 0xbfe44cf325091dd6, 0xbc68076a20000000, 0xbff0000000000000], // 39
    [0xbfca827999fcef32, 0xbfe6a09e667f3bcd, 0x3c8bdd3420000000, 0xbfe0000000000000], // 40
    [0xbfc133cc94247758, 0xbfe8bc806b151741, 0x3c82c5e120000000, 0xbfe0000000000000], // 41
    [0xbfac73b39ae68c87, 0xbfea9b66290ea1a3, 0xbc39f630e0000000, 0xbfe0000000000000], // 42
    [0x3f9d4a2c7f909c4e, 0xbfec38b2f180bdb1, 0x3c76e0b180000000, 0xbfe0000000000000], // 43
    [0x3fbe087565455a75, 0xbfed906bcf328d46, 0xbc7457e620000000, 0xbfe0000000000000], // 44
    [0xbfa4a03176acf82d, 0xbfee9f4156c62dda, 0xbc8760b1e0000000, 0xbfd0000000000000], // 45
    [0x3fac1d1f0e5967d5, 0xbfef6297cff75cb0, 0xbc75621720000000, 0xbfd0000000000000], // 46
    [0x3f9ba1650f592f50, 0xbfefd88da3d12526, 0x3c887df640000000, 0xbfc0000000000000], // 47
    [0x0000000000000000, 0xbff0000000000000, 0x0000000000000000, 0x0000000000000000], // 48
    [0xbf9ba1650f592f50, 0xbfefd88da3d12526, 0x3c887df640000000, 0x3fc0000000000000], // 49
    [0xbfac1d1f0e5967d5, 0xbfef6297cff75cb0, 0xbc75621720000000, 0x3fd0000000000000], // 50
    [0x3fa4a03176acf82d, 0xbfee9f4156c62dda, 0xbc8760b1e0000000, 0x3fd0000000000000], // 51
    [0xbfbe087565455a75, 0xbfed906bcf328d46, 0xbc7457e620000000, 0x3fe0000000000000], // 52
    [0xbf9d4a2c7f909c4e, 0xbfec38b2f180bdb1, 0x3c76e0b180000000, 0x3fe0000000000000], // 53
    [0x3fac73b39ae68c87, 0xbfea9b66290ea1a3, 0xbc39f630e0000000, 0x3fe0000000000000], // 54
    [0x3fc133cc94247758, 0xbfe8bc806b151741, 0x3c82c5e120000000, 0x3fe0000000000000], // 55
    [0x3fca827999fcef32, 0xbfe6a09e667f3bcd, 0x3c8bdd3420000000, 0x3fe0000000000000], // 56
    [0xbfcd0dfe53aba2fd, 0xbfe44cf325091dd6, 0xbc68076a20000000, 0x3ff0000000000000], // 57
    [0xbfc592675bc57974, 0xbfe1c73b39ae68c8, 0xbc8b25dd20000000, 0x3ff0000000000000], // 58
    [0xbfbe3a6873fa1279, 0xbfde2b5d3806f63b, 0xbc5e0d8920000000, 0x3ff0000000000000], // 59
    [0xbfb37ca1866b95cf, 0xbfd87de2a6aea963, 0x3c672cede0000000, 0x3ff0000000000000], // 60
    [0xbfa60bea939d225a, 0xbfd294062ed59f06, 0x3c75d28da0000000, 0x3ff0000000000000], // 61
    [0xbf93ad06011469fb, 0xbfc8f8b83c69a60b, 0x3c626d19c0000000, 0x3ff0000000000000], // 62
    [0xbf73b92e176d6d31, 0xbfb917a6bc29b42c, 0x3c3e2718e0000000, 0x3ff0000000000000], // 63
];

// Constants at RVA 0x37940..0x379b8, in the order the code loads them.
pub(crate) const SIN3: f64 = f64::from_bits(0xbfc5555555555555); // -1/3!  (0x37940, low lane)
pub(crate) const COS2: f64 = f64::from_bits(0xbfe0000000000000); // -1/2!  (0x37948, high lane)
pub(crate) const SIN5: f64 = f64::from_bits(0x3f81111111111111); //  1/5!  (0x37950, low lane)
pub(crate) const COS4: f64 = f64::from_bits(0x3fa5555555555555); //  1/4!  (0x37958, high lane)
pub(crate) const SIN7: f64 = f64::from_bits(0xbf2a01a01a01a01a); // -1/7!  (0x37960, low lane)
pub(crate) const COS6: f64 = f64::from_bits(0xbf56c16c16c16c17); // -1/6!  (0x37968, high lane)
pub(crate) const SIN9: f64 = f64::from_bits(0x3ec71de3a556c734); //  1/9!  (0x37970, low lane)
pub(crate) const COS8: f64 = f64::from_bits(0x3efa01a01a01a01a); //  1/8!  (0x37978, high lane)
pub(crate) const INV_PIO32: f64 = f64::from_bits(0x40245f306dc9c883); // 32/pi         (0x37980)
pub(crate) const SHIFTER: f64 = f64::from_bits(0x4338000000000000); // 1.5 * 2^52      (0x37988)
pub(crate) const P2: f64 = f64::from_bits(0x3d90b4611a600000); // pi/32, middle part    (0x37990)
pub(crate) const P1: f64 = f64::from_bits(0x3fb921fb54400000); // pi/32, head, 33 bits  (0x379a0)
pub(crate) const P3: f64 = f64::from_bits(0x3b63198a2e037073); // pi/32, tail           (0x379a8)
const ONE: f64 = f64::from_bits(0x3ff0000000000000); //                      (0x379b0)

/// Cosine exactly as `msvcr110.dll!_libm_sse2_cos_precise` (RVA 0x36f80) computes it, under
/// the default MXCSR and x87 control word.
pub fn cos(x: f64) -> f64 {
    let hw = ((x.to_bits() >> 48) as u16) & 0x7fff;
    // `sub ax, 0x3030; cmp ax, 0x10c5; ja slow`, then `jg x87` (signed) in the slow block.
    let t = hw.wrapping_sub(0x3030);
    if t <= 0x10c5 {
        sse2_cos(x)
    } else if (t as i16) > 0x10c5 {
        x87_cos(x)
    } else {
        // Tiny argument: clear the sign bit and return 1 - |x|.
        ONE - f64::from_bits(x.to_bits() & 0x7fff_ffff_ffff_ffff)
    }
}

/// The SSE2 path (RVA 0x36fc8..0x370fd) for `2^-252 <= |x| < 90112`, one statement per
/// instruction in the original order. `_s` / `_c` name the low (sine) and high (cosine)
/// lanes of a packed register.
fn sse2_cos(x: f64) -> f64 {
    let xk = x * INV_PIO32; // xmm1
    let k = (xk + SHIFTER) - SHIFTER; // xmm1: round to nearest even
    let ki = k as i32; // cvtsd2si of xk: same rounding, same value
    let e = &TABLE[(ki.wrapping_add(0x1c7610) & 0x3f) as usize];
    let (t0, t1, t2, t3) = (
        f64::from_bits(e[0]),
        f64::from_bits(e[1]),
        f64::from_bits(e[2]),
        f64::from_bits(e[3]),
    );

    let p1k = P1 * k; // xmm3
    let p2k = P2 * k; // xmm2, both lanes
    let r0 = x - p1k; // xmm0, xmm4
    let p3k = k * P3; // xmm1.lo
    let r = r0 - p2k; // xmm4; xmm0 = [r, r]
    let mut a_s = SIN9 * r0; // xmm5 = [1/9!, 1/8!] * r0
    let mut a_c = COS8 * r0;
    let mut s = t1 * r; // xmm7 = C_hi * r
    let mut d = r0 - r; // xmm3
    a_s *= r;
    a_c *= r;
    let r2 = r * r; // xmm0
    d -= p2k;
    let mut corr = p3k - d; // xmm1 = -(low part of r)
    let q = t0 + t3; // xmm2.lo = -S_k
    s -= q; // xmm7 = C_hi*r + S_k: derivative for the low part of r
    let mut m_s = q * r; // xmm2 = [-S_k * r, C_hi]
    let mut m_c = t1;
    let mut b_s = SIN5 * r2; // xmm6 = [1/5!, 1/4!] * r2
    let mut b_c = COS4 * r2;
    let t3r = t3 * r; // xmm3
    m_s *= r2;
    m_c *= r2;
    let r4 = r2 * r2; // xmm0
    a_s += SIN7;
    a_c += COS6;
    let t0r = r * t0; // xmm4
    b_s += SIN3;
    b_c += COS2;
    a_s *= r4;
    a_c *= r4;
    let x0 = t3r; // xmm0.lo
    let u = t3r + t1; // xmm3 = q*r + C_hi
    corr *= s;
    let x7 = t0r; // xmm7
    let head = t0r + u; // xmm4
    b_s += a_s;
    b_c += a_c;
    let mut v = t1 - u; // xmm5
    let mut w = u - head; // xmm3
    corr += t2;
    b_s *= m_s;
    b_c *= m_c;
    v += x0;
    w += x7;
    corr += v;
    corr += w;
    corr += b_s;
    corr += b_c;
    head + corr
}

// -----------------------------------------------------------------------------------------
// x87 fallback (RVA 0x32775, called from RVA 0x3711f): `fld qword; fcos; fstp qword`.
// -----------------------------------------------------------------------------------------

/// The 66-bit pi of the x87 `fcos` argument reduction as an integer: `pi66 = PI66 * 2^-66`,
/// hence `pi66/2 = PI66 * 2^-67`.
pub(crate) const PI66: u128 = 0xC_90FD_AA22_168C_234C;

/// `fprem1` divisor the DLL uses when `fcos` reports `|x| >= 2^63`: the 80-bit constant at
/// RVA 0x60346 (exponent 0x403e, significand 0xc90fdaa22168c235), an integer (about pi*2^62).
pub(crate) const FPREM_DIVISOR: u128 = 0xc90f_daa2_2168_c235;

/// Emulates the x87 fallback, `round_to_double(fcos(x))`, used for `|x| >= 90112`.
///
/// Caveat: this part is a model, not a transcription, and it is not covered by the reference
/// samples (which lie in `[0, pi)`). `fcos` is modelled as Intel documents it: the argument is
/// reduced modulo pi/2 with a 66-bit pi (done here exactly with integers), and the cosine of
/// the reduced value is rounded to a 64-bit significand, which `fstp qword` rounds again to
/// 53 bits (both round to nearest even). Real `fcos` is not guaranteed to be correctly rounded
/// to 64 bits, and other CPU vendors may reduce differently, so rare last-bit differences are
/// possible here. For `|x| >= 2^63` the DLL first reduces with `fprem1` by [`FPREM_DIVISOR`];
/// that remainder is exact and is reproduced exactly. Infinity returns the x87 default NaN
/// (`0xfff8000000000000`), a NaN returns itself quieted.
fn x87_cos(x: f64) -> f64 {
    let bits = x.to_bits();
    let abs = bits & 0x7fff_ffff_ffff_ffff;
    if abs >= 0x7ff0_0000_0000_0000 {
        return if abs == 0x7ff0_0000_0000_0000 {
            f64::from_bits(0xfff8_0000_0000_0000)
        } else {
            f64::from_bits(bits | 0x0008_0000_0000_0000)
        };
    }
    let exp = (abs >> 52) as i32; // |x| >= 90112, so x is normal
    let m = (abs & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
    let e = exp - 1075; // |x| = m * 2^e
    // The value fcos sees, as n * 2^s.
    let (n, s) = if abs >= 0x43e0_0000_0000_0000 {
        // |x| >= 2^63: exact IEEE remainder by an odd integer (no ties); fcos is even.
        let d = FPREM_DIVISOR;
        let rem = mulmod(u128::from(m) % d, pow2mod(e as u32, d), d);
        (if 2 * rem > d { d - rem } else { rem }, 0)
    } else {
        (u128::from(m), e)
    };
    // n * 2^(s+67) mod 4*PI66: quadrant and remainder, in units of 2^-67.
    let m4 = 4 * PI66;
    let rr = mulmod(n % m4, pow2mod((s + 67) as u32, m4), m4);
    let mut quad = (rr / PI66) as u32;
    let mut rem = (rr % PI66) as i128;
    if 2 * rem > PI66 as i128 {
        rem -= PI66 as i128;
        quad += 1;
    }
    // |rem| < 2^67, so the double-double is exact.
    let hi = rem as f64;
    let lo = (rem - hi as i128) as f64;
    let r = (hi * pow2(-67), lo * pow2(-67));
    let (v, neg) = match quad & 3 {
        0 => (dd_cos(r), false),
        1 => (dd_sin(r), true),
        2 => (dd_cos(r), true),
        _ => (dd_sin(r), false),
    };
    let y = round_ext_then_double(v);
    if neg {
        -y
    } else {
        y
    }
}

/// `2^k` for `-1022 <= k <= 1023`.
pub(crate) fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// `(a * b) mod m` for `a, b < m < 2^72`, by 32-bit Horner steps (no intermediate overflow).
pub(crate) fn mulmod(a: u128, b: u128, m: u128) -> u128 {
    let mut acc = 0u128;
    for i in (0..3).rev() {
        let bi = (b >> (32 * i)) & 0xffff_ffff;
        acc = ((acc << 32) % m + (a * bi) % m) % m;
    }
    acc
}

/// `2^k mod m`.
pub(crate) fn pow2mod(k: u32, m: u128) -> u128 {
    let mut acc = 1 % m;
    for _ in 0..k {
        acc = (acc << 1) % m;
    }
    acc
}

pub(crate) type Dd = (f64, f64);

fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    (s, (a - (s - bb)) + (b - bb))
}

fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    (s, b - (s - a))
}

fn split(a: f64) -> Dd {
    let c = 134217729.0 * a;
    let hi = c - (c - a);
    (hi, a - hi)
}

/// Dekker's exact product (no FMA).
fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    (p, ((ah * bh - p) + ah * bl + al * bh) + al * bl)
}

fn dd_add(a: Dd, b: Dd) -> Dd {
    let (s, e) = two_sum(a.0, b.0);
    let (t, f) = two_sum(a.1, b.1);
    let (s, e) = quick_two_sum(s, e + t);
    quick_two_sum(s, e + f)
}

fn dd_mul(a: Dd, b: Dd) -> Dd {
    let (p, e) = two_prod(a.0, b.0);
    quick_two_sum(p, e + (a.0 * b.1 + a.1 * b.0))
}

fn dd_div_small(a: Dd, d: f64) -> Dd {
    let q1 = a.0 / d;
    let (p, e) = two_prod(q1, d);
    let t = (((a.0 - p) - e) + a.1) / d;
    quick_two_sum(q1, t)
}

/// Taylor series `term - term*r^2/((n+1)(n+2)) + ...` in double-double (|r| <= pi/4; 30
/// further terms reach far below 2^-106).
fn dd_series(r2: Dd, mut term: Dd, mut n: f64) -> Dd {
    let mut sum = term;
    for _ in 0..30 {
        term = dd_div_small(dd_mul(term, r2), (n + 1.0) * (n + 2.0));
        term = (-term.0, -term.1);
        n += 2.0;
        sum = dd_add(sum, term);
    }
    sum
}

pub(crate) fn dd_cos(r: Dd) -> Dd {
    dd_series(dd_mul(r, r), (1.0, 0.0), 0.0)
}

pub(crate) fn dd_sin(r: Dd) -> Dd {
    dd_series(dd_mul(r, r), r, 1.0)
}

/// Rounds a double-double to a 64-bit significand (x87 extended) and then to double, both to
/// nearest even, as `fcos` followed by `fstp qword` does.
pub(crate) fn round_ext_then_double(v: Dd) -> f64 {
    let (mut hi, mut lo) = v;
    let neg = hi < 0.0;
    if neg {
        hi = -hi;
        lo = -lo;
    }
    if hi == 0.0 {
        return 0.0;
    }
    let e = ((hi.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    // Scale so the value is an integer part n in [2^63, 2^64) plus a fraction.
    let mut scale = 63 - e;
    let (n, frac) = loop {
        let h = (hi * pow2(scale)) as i128; // exact integer
        let l = lo * pow2(scale);
        let mut t = l as i64;
        if (t as f64) > l {
            t -= 1;
        }
        let n = h + i128::from(t);
        if n < 1i128 << 63 {
            scale += 1; // hi was a power of two and lo negative
            continue;
        }
        break (n as u128, l - t as f64);
    };
    let mut n = n;
    if frac > 0.5 || (frac == 0.5 && n & 1 == 1) {
        n += 1;
    }
    let mut q = n >> 11;
    let low = n & 0x7ff;
    if low > 0x400 || (low == 0x400 && q & 1 == 1) {
        q += 1;
    }
    let y = q as f64 * pow2(11 - scale);
    if neg {
        -y
    } else {
        y
    }
}
