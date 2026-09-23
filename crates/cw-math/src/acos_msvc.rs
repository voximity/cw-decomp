//! Bit-exact port of MSVCR110's SSE2 `acos`: `msvcr110.dll` export `_libm_sse2_acos_precise`
//! (RVA 0x33155), which Server.exe imports (thunk `0x0054b1d6`) and calls only through the
//! single-precision wrapper `acosf` at `Server.exe 0x00548b00` (`(float)acos((double)x)`),
//! itself called from `cube::World::tick` 0x005322d0.
//!
//! Transcribed from the disassembly (capstone over `game/msvcr110.dll`), one statement per
//! SSE2 instruction in the original order; `_lo` / `_hi` name the lanes of a packed register.
//! Checked against `tests/golden/acos_msvcr110.txt` (`tools/oracle/libm_emu.py`). The two
//! tables are byte-identical to `asin`'s (RVA 0x33680 and 0x34580) and are shared with
//! [`crate::asin_msvc`].
//!
//! # Paths (by `k = (bits >> 44) & 0x7ffff`, as in `asin`)
//!
//! - `0.0625 <= |x| < 0.8652`: [`mid`], `acos(x) = pi/2 - asin(c) - asin(d)` with the table
//!   point `c` and `d` as in `asin`.
//! - `0.8652 <= |x| < 0.9922`: [`upper`], through `s = sqrt(1 - x^2)` and the table at `s`,
//!   with `pi` folded in for negative `x`.
//! - `2^-60 <= |x| < 0.0625`: [`small`], `pi/2 - x - x^3 p(x^2)`.
//! - `0.9922 <= |x| < 1`: [`near_one`], `2 asin(sqrt((1 - |x|)/2))`, reflected about `pi`
//!   for negative `x`.
//! - otherwise [`special`]: `|x| < 2^-60` returns `pi/2`; `x == 1` returns `+0`, `x == -1`
//!   returns `pi`; a NaN returns itself quieted; `|x| > 1` returns the default NaN. The last
//!   two go through the math error handler, which returns the value unchanged.

use crate::asin_msvc::{ASIN_TABLE, HALF_18, MASK_18, SQRT_TABLE};

// Constants (RVA 0x34d80..0x34e3f).
const PIO2_LO: f64 = f64::from_bits(0x3c91a62633145c07); //  (0x34d80)
const PIO2_HI: f64 = f64::from_bits(0x3ff921fb54442d18); //  (0x34d88)
const NEG_PI: [u64; 2] = [0xbca1a62633145c07, 0xc00921fb54442d18]; // -pi lo/hi (0x34d90)
const PI_LO: u64 = 0x3ca1a62633145c07; //                    (0x34da0)
const PI_HI: u64 = 0x400921fb54442d18; //                    (0x34da8)
const MASK_27: u64 = 0xfffffffff8000000; //                  (0x34db0)
const NC3: f64 = f64::from_bits(0xbfc5555555555555); // -1/6   (0x34dc8)
const NC5: f64 = f64::from_bits(0xbfb3333333333333); // -3/40  (0x34dd0)
const NC7: f64 = f64::from_bits(0xbfa6db6db6db6db7); // -5/112 (0x34dd8)
// Packed pairs `[lo, hi]` of the odd series (RVA 0x34de0..0x34e0f).
const P_DE: [f64; 2] = [
    f64::from_bits(0x3f96e8ba2e8ba2e9),
    f64::from_bits(0x3fb3333333333333),
];
const P_DF: [f64; 2] = [
    f64::from_bits(0x3f9f1c71c71c71c7),
    f64::from_bits(0x3fc5555555555555),
];
const P_E0: [f64; 2] = [
    f64::from_bits(0x3f91c4ec4ec4ec4f),
    f64::from_bits(0x3fa6db6db6db6db7),
];
const ABS: u64 = 0x7fffffffffffffff; //                      (0x34e20)
const ONE: f64 = f64::from_bits(0x3ff0000000000000); //      (0x34e30)
const HALF: f64 = f64::from_bits(0x3fe0000000000000); //     (0x34e38)

const SIGN: u64 = 0x8000_0000_0000_0000;

#[inline]
fn f(b: u64) -> f64 {
    f64::from_bits(b)
}

/// `acos` exactly as `msvcr110.dll!_libm_sse2_acos_precise` (RVA 0x33155) computes it, under
/// the default MXCSR and x87 control word.
pub fn acos(x: f64) -> f64 {
    let bits = x.to_bits();
    let k = ((bits >> 44) as u32) & 0x7ffff;
    let a = k.wrapping_sub(0x3fb00);
    if a < 0x3bb {
        return mid(x, (bits >> 44) as u32);
    }
    let b = a.wrapping_sub(0x3bb);
    if b < 0x41 {
        return upper(x);
    }
    let c = b.wrapping_add(0x3bbb);
    if c < 0x3800 {
        return small(x);
    }
    let d = c.wrapping_sub(0x3bfc);
    if d < 4 {
        return near_one(x);
    }
    special(x, d.wrapping_add(0x3fefc))
}

/// RVA 0x331dd: `0.0625 <= |x| < 0.8652`.
fn mid(x: f64, edx: u32) -> f64 {
    let x2 = x * x; // xmm1
    let s = (ONE - x2).sqrt(); // xmm3
    let c = f(MASK_18 & x.to_bits() | HALF_18); // xmm2
    let i = ((((edx & 0xffff) & 0xffff_fffc).wrapping_sub(0xfb00)) >> 2) as usize;
    let sc = f(SQRT_TABLE[i]); // xmm1
    let t = ASIN_TABLE[i]; // xmm4
    let x7 = x + c; // xmm7
    let x0 = x - c; // xmm0
    let x7 = x7 * x0;
    let x6 = x * sc; // xmm6
    let x3 = s * c; // xmm3
    let x1 = x6;
    let x6 = x6 + x3;
    let delta = x7 / x6; // xmm7
    let d = x1 - x3; // xmm1
    let sign = c.to_bits() & SIGN;
    let d2 = d * d;
    let d3 = d * d2; // xmm3
    let q7 = NC7 * d2; // xmm0
    let t_lo = f(t[0] ^ sign) - PIO2_LO; // xmm4 = (table ^ sign) - [pio2_lo, pio2_hi]
    let t_hi = f(t[1] ^ sign) - PIO2_HI;
    let q3 = NC3 * d3; // xmm5
    let d5 = d3 * d2; // xmm3
    let q = q7 + NC5; // xmm0
    let q = q * d5;
    let q3 = q3 - t_lo; // xmm5
    let q = q + q3;
    let q = q - delta;
    q - t_hi
}

/// RVA 0x332b4: `0.8652 <= |x| < 0.9922`.
fn upper(x: f64) -> f64 {
    let bits = x.to_bits();
    let neg = bits >> 63 != 0; // pmovmskb bit 7
    let xh = f((bits >> 38) << 38); // xmm7
    let sgn = bits & SIGN; // xmm4 = ~ABS & x
    let x1 = x - xh; // xmm1
    let x6 = xh; // xmm6
    let xh2 = xh * xh; // xmm7
    let x0 = x + x6; // xmm0
    let h5 = HALF_18 | sgn; // xmm5
    let x3 = ONE - xh2; // xmm3
    let x0 = x0 * x1; // x^2 - xh^2
    let x4 = x3; // xmm4
    let s = (x3 - x0).sqrt(); // xmm3
    let sh = f(MASK_18 & s.to_bits() | h5); // xmm2, carries the sign of x
    let j = ((((s.to_bits() << 2) >> 48) as u32).wrapping_sub(0xfec0)) as usize;
    let fold = if neg { NEG_PI } else { [0, 0] }; // xmm3 = broadcast mask & -pi
    let x7 = s * f(SQRT_TABLE[j]);
    let x6 = x6 * sh;
    let x1 = x1 * sh;
    let sh2 = sh * sh; // xmm2
    let x6 = x6 - x7;
    let e = x6 + x1; // xmm6
    let x4 = x4 - sh2;
    let x7 = x7 + x7;
    let x4 = x4 - x0;
    let x7 = x7 + e;
    let corr = x4 / x7; // xmm4
    let t = ASIN_TABLE[j];
    let c_lo = f(fold[0]) + f(t[0]); // xmm3 = fold + table
    let c_hi = f(fold[1]) + f(t[1]);
    let e2 = e * e; // xmm6
    let q7 = NC7 * e2; // xmm0
    let e3 = e * e2; // xmm1
    let q3 = NC3 * e3; // xmm5
    let e5 = e3 * e2; // xmm1
    let q = q7 + NC5; // xmm0
    let q = q * e5;
    let q3 = q3 + c_lo; // xmm5
    let q = q + q3; // xmm0
    let h = corr + c_hi; // xmm4
    let l = c_hi - h; // xmm3
    let l = corr + l; // xmm5
    let q = q + l;
    let q = q + h;
    f(q.to_bits() ^ if neg { SIGN } else { 0 })
}

/// RVA 0x333e4: `2^-60 <= |x| < 0.0625`.
fn small(x: f64) -> f64 {
    let x2 = x * x; // xmm0 = [x^2, x^2]
    let x3 = x * x2; // xmm1
    let p_lo = P_DE[0] * x2; // xmm6
    let p_hi = P_DE[1] * x2;
    let x4 = x2 * x2; // xmm0
    let x6 = x3 * x3; // xmm1.lo
    let p_lo = p_lo + P_DF[0];
    let p_hi = p_hi + P_DF[1];
    let q_lo = P_E0[0] * x4; // xmm4
    let q_hi = P_E0[1] * x4;
    let x9 = x6 * x3;
    let p_lo = p_lo + q_lo;
    let p_hi = p_hi + q_hi;
    let r_lo = x9 * p_lo; // xmm1
    let r_hi = x3 * p_hi;
    let h = PIO2_HI - x; // xmm0
    let l = PIO2_LO - r_lo; // xmm5
    let t = PIO2_HI - h; // xmm6
    let l = l - r_hi;
    let t = x - t; // xmm7
    let l = l - t;
    h + l
}

/// RVA 0x33475: `0.9922 <= |x| < 1`.
fn near_one(x: f64) -> f64 {
    let ax = f(x.to_bits() & ABS); // xmm7
    let hx = ax * HALF;
    let w = HALF - hx; // xmm4, xmm7 = xmm5 = [w, w]
    let r = w.sqrt(); // xmm4.lo
    let p_lo = P_DE[0] * w; // xmm1
    let p_hi = P_DE[1] * w;
    let sign = x.to_bits() & SIGN; // eax = word 3, `& 0x8000`
    let w2 = w * w; // xmm7
    let p_lo = P_DF[0] + p_lo; // xmm2
    let p_hi = P_DF[1] + p_hi;
    let q_lo = P_E0[0] * w2; // xmm3
    let q_hi = P_E0[1] * w2;
    let fold = if x < 0.0 { NEG_PI } else { [0, 0] }; // cmpltsd x, 0 -> mask & -pi
    let w3 = w2 * w; // xmm7.lo
    let p_lo = p_lo + q_lo;
    let p_hi = p_hi + q_hi;
    let p_lo = p_lo * w3; // xmm2.lo
    let p_lo = p_lo * w; // xmm2 *= [w, w]
    let p_hi = p_hi * w;
    let rh = f(MASK_27 & r.to_bits()); // xmm1
    let rl = r - rh; // xmm4
    let r2 = r + r; // xmm3.lo
    let rh2 = rh * rh; // xmm1
    let rs = r2 - rl; // xmm3.lo = r + rh
    let w5 = w - rh2; // xmm5
    let rl = rl * rs; // xmm4
    let w5 = w5 - rl;
    let corr = w5 / r; // xmm5 (xmm3 = [r, r])
    let r2 = r + r; // xmm3 = [2r, 2r]
    let p_lo = p_lo * r2; // xmm2
    let p_hi = p_hi * r2;
    let v = p_lo + f(fold[0]); // xmm2.lo
    let v = v + p_hi;
    let v = v + corr;
    let v = v + r2;
    let y = f(fold[1]) + v; // xmm0
    f(y.to_bits() ^ sign)
}

/// RVA 0x3355c: tiny, `|x| >= 1`, infinities and NaNs. `k` is `(bits >> 44) & 0x7ffff`.
fn special(x: f64, k: u32) -> f64 {
    let bits = x.to_bits();
    if k < 0x3ff00 {
        // |x| < 2^-60 (RVA 0x33634).
        return PIO2_HI + PIO2_LO;
    }
    let ahi = ((bits >> 32) as u32) & 0x7fff_ffff;
    let lo = bits as u32;
    if (0x3ff0_0000u32.wrapping_sub(ahi) | lo) == 0 {
        // |x| == 1 (RVA 0x33601): `(PI_HI & m) + (PI_LO & m)`, m = all ones when negative.
        let m = if bits >> 63 != 0 { u64::MAX } else { 0 };
        return f(PI_HI & m) + f(PI_LO & m);
    }
    if bits & ABS > 0x7ff0_0000_0000_0000 {
        // NaN: `x + 0.0` (quiets it), error 1008, returned unchanged.
        return f(bits | 0x0008_0000_0000_0000);
    }
    // |x| > 1: `0 * inf`, the default NaN, error 58.
    f(0xfff8_0000_0000_0000)
}
