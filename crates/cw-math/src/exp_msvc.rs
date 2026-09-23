//! Bit-exact port of MSVCR110's SSE2 `exp`: `msvcr110.dll` export `_libm_sse2_exp_precise`
//! (RVA 0x379c0), which Server.exe imports (thunk `0x0054b1dc`) and calls from one place, the
//! single-precision wrapper `expf` at `Server.exe 0x00548b20`
//! (`(float)exp((double)x)`, used by `cube::World::tick` 0x005322d0).
//!
//! Transcribed from the disassembly (capstone over `game/msvcr110.dll`); every SSE2 step is a
//! correctly rounded double operation, so the same order in plain `f64` reproduces it on any
//! IEEE-754 target. Checked against `tests/golden/exp_msvcr110.txt`, produced by running the
//! DLL routine in an emulator (`tools/oracle/libm_emu.py`).
//!
//! # Algorithm
//!
//! `n = round(x * 64/ln2)` (shifter trick), `m = n >> 6`, `j = n & 63`,
//! `r = (x - k*L1) - k*L2` with `ln2/64 = L1 + L2`, then
//! `exp(x) = 2^m * 2^(j/64) * (1 + p)` with `p = r + T_lo[j] + r^2 (a2 + a3 r) + r^4 (a4 + a5 r)`
//! and `2^(j/64) = 1 + T_hi[j]` stored as the fraction bits of a double whose exponent is
//! patched in to build `S = 2^m * 2^(j/64)` directly. The result is `p*S + S`.
//!
//! When `2^m` is outside the normal range (`m < -894` or `m > 1022`), the last step runs on
//! the x87 unit with 64-bit precision: `S' = 2^(m - m/2) * 2^(j/64)`, `(p*S' + S') * 2^(m/2)`,
//! then `fstp qword` rounds once to the double (or subnormal) grid.
//!
//! # Special arguments (RVA 0x37c34)
//!
//! `|x| < 2^-54`: `x + 1`. `|x| >= 1024`: finite positive overflows (`MAX * MAX = inf`), finite
//! negative underflows (`MIN * MIN = 0`); `+inf` gives `inf`, `-inf` gives `0`, a NaN is
//! returned quieted. Overflow, underflow and NaN go through the math error handler
//! (RVA 0xc09c8), which with the default `_matherr` only sets `errno` and returns the value
//! unchanged; `errno` is not modelled.

use crate::x87::Ext;

/// `[T_lo, T_hi]` for `j = n & 63`: `2^(j/64) = 1 + T_hi + T_lo` with `T_hi` holding only
/// fraction bits (RVA 0x37d00).
#[rustfmt::skip]
const TABLE: [[u64; 2]; 64] = [
    [0x0000000000000000, 0x0000000000000000], // 0
    [0x3cad7bbf0e03754d, 0x00002c9a3e778060], // 1
    [0x3c8cd2523567f613, 0x000059b0d3158574], // 2
    [0x3c60f74e61e6c861, 0x0000874518759bc8], // 3
    [0x3c979aa65d837b6c, 0x0000b5586cf9890f], // 4
    [0x3c3ebe3d702f9cd1, 0x0000e3ec32d3d1a2], // 5
    [0x3ca3516e1e63bcd8, 0x00011301d0125b50], // 6
    [0x3ca4c55426f0387b, 0x0001429aaea92ddf], // 7
    [0x3ca9515362523fb6, 0x000172b83c7d517a], // 8
    [0x3c8b898c3f1353bf, 0x0001a35beb6fcb75], // 9
    [0x3c9aecf73e3a2f5f, 0x0001d4873168b9aa], // 10
    [0x3c8a6f4144a6c38d, 0x0002063b88628cd6], // 11
    [0x3c968efde3a8a894, 0x0002387a6e756238], // 12
    [0x3c80472b981fe7f2, 0x00026b4565e27cdd], // 13
    [0x3c82f7e16d09ab31, 0x00029e9df51fdee1], // 14
    [0x3c8b3782720c0ab3, 0x0002d285a6e4030b], // 15
    [0x3c834d754db0abb6, 0x000306fe0a31b715], // 16
    [0x3c8fdd395dd3f84a, 0x00033c08b26416ff], // 17
    [0x3ca12f8ccc187d29, 0x000371a7373aa9ca], // 18
    [0x3ca7d229738b5e8b, 0x0003a7db34e59ff6], // 19
    [0x3c859f48a72a4c6d, 0x0003dea64c123422], // 20
    [0x3ca8b846259d9205, 0x0004160a21f72e29], // 21
    [0x3c4363ed60c2ac12, 0x00044e086061892d], // 22
    [0x3c6ecce1daa10379, 0x000486a2b5c13cd0], // 23
    [0x3c7690cebb7aafb0, 0x0004bfdad5362a27], // 24
    [0x3ca083cc9b282a09, 0x0004f9b2769d2ca6], // 25
    [0x3ca509b0c1aae707, 0x0005342b569d4f81], // 26
    [0x3c93350518fdd78e, 0x00056f4736b527da], // 27
    [0x3c9063e1e21c5409, 0x0005ab07dd485429], // 28
    [0x3c9432e62b64c035, 0x0005e76f15ad2148], // 29
    [0x3ca0128499f08c0a, 0x0006247eb03a5584], // 30
    [0x3c99f0870073dc06, 0x0006623882552224], // 31
    [0x3c998d4d0da05571, 0x0006a09e667f3bcc], // 32
    [0x3ca52bb986ce4786, 0x0006dfb23c651a2e], // 33
    [0x3ca32092206f0dab, 0x00071f75e8ec5f73], // 34
    [0x3ca061228e17a7a6, 0x00075feb564267c8], // 35
    [0x3ca244ac461e9f86, 0x0007a11473eb0186], // 36
    [0x3c65ebe1abd66c55, 0x0007e2f336cf4e62], // 37
    [0x3c96fe9fbbff67d0, 0x00082589994cce12], // 38
    [0x3c951f1414c801df, 0x000868d99b4492ec], // 39
    [0x3c8db72fc1f0eab4, 0x0008ace5422aa0db], // 40
    [0x3c7bf68359f35f44, 0x0008f1ae99157736], // 41
    [0x3ca360ba9c06283c, 0x00093737b0cdc5e4], // 42
    [0x3c95e8d120f962aa, 0x00097d829fde4e4f], // 43
    [0x3c71affc2b91ce27, 0x0009c49182a3f090], // 44
    [0x3c9b6d34589a2ebd, 0x000a0c667b5de564], // 45
    [0x3c95277c9ab89880, 0x000a5503b23e255c], // 46
    [0x3c8469846e735ab3, 0x000a9e6b5579fdbf], // 47
    [0x3c8c1a7792cb3387, 0x000ae89f995ad3ad], // 48
    [0x3ca22466dc2d1d96, 0x000b33a2b84f15fa], // 49
    [0x3ca1112eb19505ae, 0x000b7f76f2fb5e46], // 50
    [0x3c74ffd70a5fddcd, 0x000bcc1e904bc1d2], // 51
    [0x3c736eae30af0cb3, 0x000c199bdd85529c], // 52
    [0x3c84e08fd10959ac, 0x000c67f12e57d14b], // 53
    [0x3c676b2c6c921968, 0x000cb720dcef9069], // 54
    [0x3c93700936df99b3, 0x000d072d4a07897b], // 55
    [0x3c74a385a63d07a7, 0x000d5818dcfba487], // 56
    [0x3c8e5a50d5c192ac, 0x000da9e603db3285], // 57
    [0x3c98bb731c4a9792, 0x000dfc97337b9b5e], // 58
    [0x3c74b604603a88d3, 0x000e502ee78b3ff6], // 59
    [0x3c916f2792094926, 0x000ea4afa2a490d9], // 60
    [0x3c8ec3bc41aa2008, 0x000efa1bee615a27], // 61
    [0x3c8a64a931d185ee, 0x000f50765b6e4540], // 62
    [0x3c77893b4d91cd9d, 0x000fa7c1819e90d8], // 63
];

// Constants (RVA 0x37c70..0x37cff and 0x38100..0x38127). Packed ones hold the same value in
// both lanes unless two are listed.
const EXP_MASK: u64 = 0xfff0000000000000; // (0x37c70)
const SHIFTER: f64 = f64::from_bits(0x4338000000000000); // 1.5 * 2^52   (0x37ca0)
const INV_L: f64 = f64::from_bits(0x40571547652b82fe); // 64/ln2          (0x37cb0)
const L1: f64 = f64::from_bits(0x3f862e42fefa0000); // ln2/64, head       (0x37cc0)
const L2: f64 = f64::from_bits(0x3d1cf79abc9e3b3a); // ln2/64, tail       (0x37cd0)
const A5: f64 = f64::from_bits(0x3f811074b1d108e5); // ~1/120  (0x37ce0, low lane)
const A3: f64 = f64::from_bits(0x3fc555555566a45a); // ~1/6    (0x37ce8, high lane)
const A4: f64 = f64::from_bits(0x3fa5555726eced80); // ~1/24   (0x37cf0, low lane)
const A2: f64 = f64::from_bits(0x3fdfffffffffe17b); // ~1/2    (0x37cf8, high lane)
const ONE: f64 = f64::from_bits(0x3ff0000000000000); //             (0x38100)
const INF: f64 = f64::from_bits(0x7ff0000000000000); //             (0x38108)
const MAX: f64 = f64::from_bits(0x7fefffffffffffff); //             (0x38118)
const MIN_NORMAL: f64 = f64::from_bits(0x0010000000000000); //      (0x38120)

/// `exp` exactly as `msvcr110.dll!_libm_sse2_exp_precise` (RVA 0x379c0) computes it, under
/// the default MXCSR and x87 control word.
pub fn exp(x: f64) -> f64 {
    let hw = ((x.to_bits() >> 48) as u32) & 0x7fff;
    // `edx = 0x408f - hw; eax = hw - 0x3c90; (edx | eax) >= 0x80000000` -> special.
    if (0x408fu32.wrapping_sub(hw) | hw.wrapping_sub(0x3c90)) >= 0x8000_0000 {
        return special(x);
    }
    let xk = x * INV_L;
    let n_bits = (xk + SHIFTER).to_bits(); // xmm7
    let k = f64::from_bits(n_bits) - SHIFTER; // xmm1
    let c1 = L1 * k; // xmm2
    let c2 = L2 * k; // xmm3
    let r = (x - c1) - c2; // xmm0, both lanes
    let n = n_bits as u32 as i32; // movd eax, xmm7
    let j = (n & 0x3f) as usize;
    let m = n >> 6; // sar eax, 6
    let t = TABLE[j];
    let (t_lo, t_hi) = (f64::from_bits(t[0]), t[1]);
    let q5 = A5 * r; // xmm4 = [a5 r, a3 r]
    let q3 = A3 * r;
    let r2 = r * r; // xmm0 = [r2, r2]
    let s4 = A4 + q5; // xmm5 = [a4 + a5 r, a2 + a3 r]
    let s2 = A2 + q3;
    let r4 = r2 * r2; // xmm0.lo
    let u = r + t_lo; // xmm1.lo
    // xmm7 = ((n & 0xffffffc0) + 0xffc0) << 46: the biased exponent m + 1023 in bits 52..63.
    let scale_bits = ((u64::from(n as u32) & 0xffff_ffc0) + 0xffc0) << 46;
    let p4 = r4 * s4; // xmm0 = [r4 (a4 + a5 r), r2 (a2 + a3 r)]
    let p2 = r2 * s2;
    let u = u + p4; // xmm1.lo
    let s = t_hi | scale_bits; // xmm2 = S
    let p = p2 + u; // xmm0.lo
    if (m as u32).wrapping_add(0x37e) <= 0x77c {
        let s = f64::from_bits(s);
        return p * s + s;
    }
    // RVA 0x37aee: split the scale over two factors and finish on the x87 unit.
    let h = m >> 1; // eax
    let l = m - h; // edx
    let s1 = f64::from_bits((s & !EXP_MASK) | ((u64::from((h + 0x3ff) as u32)) << 52)); // xmm6
    let v = Ext::from_f64(p).mul(Ext::from_f64(s1)); // fmul st(1), st(0)
    let v = v.add(Ext::from_f64(s1)); // faddp
    // `fld qword xmm4` with xmm4 = (l + 0x3ff) << 52, then fmulp: an exact power of two
    // whenever the game can reach this path (|l| <= 1022 for |x| < 1024).
    let y = v.mul(Ext::from_f64(f64::from_bits(
        u64::from((l + 0x3ff) as u32) << 52,
    )));
    // Overflow (inf) and underflow (zero or subnormal) go through the error handler, which
    // returns the value unchanged.
    y.to_f64()
}

/// RVA 0x37c34: `|x| < 2^-54`, `|x| >= 1024`, infinities and NaNs.
fn special(x: f64) -> f64 {
    let bits = x.to_bits();
    let hi = (bits >> 32) as u32;
    let lo = bits as u32;
    let ahi = hi & 0x7fff_ffff;
    if ahi < 0x4090_0000 {
        return x + ONE;
    }
    if ahi < 0x7ff0_0000 {
        return if hi < 0x8000_0000 {
            MAX * MAX
        } else {
            MIN_NORMAL * MIN_NORMAL
        };
    }
    if ahi > 0x7ff0_0000 || lo != 0 {
        // NaN: error 1002, whose handler reloads the argument through `fld`/`fstp`.
        return f64::from_bits(bits | 0x0008_0000_0000_0000);
    }
    if hi == 0x7ff0_0000 { INF } else { 0.0 }
}
