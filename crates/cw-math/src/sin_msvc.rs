//! Bit-exact port of MSVCR110's SSE2 `sin`: `msvcr110.dll` export `_libm_sse2_sin_precise`
//! (RVA 0x3dcf0). The exported `sin` (RVA 0x32e20) dispatches on SSE2 machines to a copy at
//! RVA 0x4ba18 that differs only in returning through `st(0)` instead of `xmm0`.
//!
//! Recovered the same way as [`crate::cos_msvc`]: the export was disassembled from
//! `game/MSVCR110.dll` (capstone + pefile) and compared instruction by instruction with the
//! `cos` routine. The SSE2 main path (RVA 0x3dd21..0x3de6d) is identical to `cos` except for
//! the table index: `sin` uses `k mod 64` (`add edx, 0x1c7600`) where `cos` uses
//! `(k + 16) mod 64` (`0x1c7610`), i.e. `cos(x) = sin(x + pi/2)`. Its 64-entry table
//! (RVA 0x3dec0) and its constants (RVA 0x3e6c0..0x3e72f) are byte-identical to the `cos`
//! ones, so they are shared from `cos_msvc.rs` (made `pub(crate)` there for this).
//!
//! # Paths (selected by `hw = (bits >> 48) & 0x7fff`, as in `cos`)
//!
//! - `hw < 0x3030` (`|x| < 2^-252`): if the exponent field is zero (zero or subnormal),
//!   `x * (1 - 2^-53)`; otherwise `(2^55 * x - x) * 2^-55`. Both return `x` in practice and
//!   exist to raise the inexact / underflow flags; they are transcribed literally.
//! - `hw <= 0x40f5` (`|x| < 90112`): the SSE2 path, [`sse2_sin`].
//! - otherwise (large, infinite or NaN): the x87 fallback at RVA 0x32eae, `fld qword; fsin;
//!   fstp qword`, structured exactly like the `fcos` one (same `fprem1` reduction by the same
//!   constant for `|x| >= 2^63`, same NaN handling). Emulated by [`x87_sin`].
//!
//! As for `cos`, the DLL takes the x87 path for every input when MXCSR or the x87 control word
//! is not at its default; the game never changes them.

use crate::cos_msvc::{
    dd_cos, dd_sin, mulmod, pow2, pow2mod, round_ext_then_double, COS2, COS4, COS6, COS8,
    FPREM_DIVISOR, INV_PIO32, P1, P2, P3, PI66, SHIFTER, SIN3, SIN5, SIN7, SIN9, TABLE,
};

// Constants of the tiny-argument path (RVA 0x3e730..0x3e747).
const TWO55: f64 = f64::from_bits(0x4360000000000000); // 2^55        (0x3e730)
const TWO_M55: f64 = f64::from_bits(0x3c80000000000000); // 2^-55     (0x3e738)
const ONE_M: f64 = f64::from_bits(0x3fefffffffffffff); // 1 - 2^-53    (0x3e740)

/// Sine exactly as `msvcr110.dll!_libm_sse2_sin_precise` (RVA 0x3dcf0) computes it, under
/// the default MXCSR and x87 control word.
pub fn sin(x: f64) -> f64 {
    let hw = ((x.to_bits() >> 48) as u16) & 0x7fff;
    // `sub ax, 0x3030; cmp ax, 0x10c5; ja slow`, then `jg x87` (signed) in the slow block.
    let t = hw.wrapping_sub(0x3030);
    if t <= 0x10c5 {
        sse2_sin(x)
    } else if (t as i16) > 0x10c5 {
        x87_sin(x)
    } else if t >> 4 == 0xcfd {
        // `shr ax, 4; cmp ax, 0xcfd`: exponent field zero (zero or subnormal).
        x * ONE_M
    } else {
        (TWO55 * x - x) * TWO_M55
    }
}

/// The SSE2 path (RVA 0x3dd38..0x3de6d) for `2^-252 <= |x| < 90112`, one statement per
/// instruction in the original order; identical to `cos_msvc::sse2_cos` apart from the table
/// index. `_s` / `_c` name the low and high lanes of a packed register.
fn sse2_sin(x: f64) -> f64 {
    let xk = x * INV_PIO32; // xmm1
    let k = (xk + SHIFTER) - SHIFTER; // xmm1: round to nearest even
    let ki = k as i32; // cvtsd2si of xk: same rounding, same value
    let e = &TABLE[(ki.wrapping_add(0x1c7600) & 0x3f) as usize];
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
    let mut s = t1 * r; // xmm7
    let mut d = r0 - r; // xmm3
    a_s *= r;
    a_c *= r;
    let r2 = r * r; // xmm0
    d -= p2k;
    let mut corr = p3k - d; // xmm1
    let q = t0 + t3; // xmm2.lo
    s -= q; // xmm7
    let mut m_s = q * r; // xmm2
    let mut m_c = t1;
    let mut b_s = SIN5 * r2; // xmm6
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
    let u = t3r + t1; // xmm3
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

/// Emulates the x87 fallback, `round_to_double(fsin(x))`, used for `|x| >= 90112`.
///
/// Same model and caveat as `cos_msvc::x87_cos`: `fsin` is modelled as reducing modulo pi/2
/// with Intel's 66-bit pi (exactly, with integers) and rounding the sine of the reduced value
/// to a 64-bit significand, then `fstp qword` rounds to double. For `|x| >= 2^63` the DLL
/// first takes the exact `fprem1` remainder by `FPREM_DIVISOR` (sign kept, since sine is odd).
/// Infinity returns the x87 default NaN, a NaN returns itself quieted.
fn x87_sin(x: f64) -> f64 {
    let bits = x.to_bits();
    let abs = bits & 0x7fff_ffff_ffff_ffff;
    if abs >= 0x7ff0_0000_0000_0000 {
        return if abs == 0x7ff0_0000_0000_0000 {
            f64::from_bits(0xfff8_0000_0000_0000)
        } else {
            f64::from_bits(bits | 0x0008_0000_0000_0000)
        };
    }
    let mut neg = bits >> 63 != 0;
    let exp = (abs >> 52) as i32; // |x| >= 90112, so x is normal
    let m = (abs & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
    let e = exp - 1075; // |x| = m * 2^e
    // |value fsin sees| as n * 2^s; its sign is folded into `neg`.
    let (n, s) = if abs >= 0x43e0_0000_0000_0000 {
        // |x| >= 2^63: exact IEEE remainder by an odd integer (no ties).
        let d = FPREM_DIVISOR;
        let rem = mulmod(u128::from(m) % d, pow2mod(e as u32, d), d);
        if 2 * rem > d {
            neg = !neg;
            (d - rem, 0)
        } else {
            (rem, 0)
        }
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
    let hi = rem as f64;
    let lo = (rem - hi as i128) as f64;
    let r = (hi * pow2(-67), lo * pow2(-67));
    let (v, qneg) = match quad & 3 {
        0 => (dd_sin(r), false),
        1 => (dd_cos(r), false),
        2 => (dd_sin(r), true),
        _ => (dd_cos(r), true),
    };
    let y = round_ext_then_double(v);
    if neg != qneg {
        -y
    } else {
        y
    }
}
