//! Bit-exact math primitives shared by world generation, simulation and the protocol.
//!
//! Everything here is Tier A: the output must match the original x86 binary bit for bit.
//! Doc comments cite the original function as `Server.exe 0x<address>`. Golden samples
//! captured from the running original with `tools/oracle/server_oracle.py` live in
//! `tests/golden/`.
//!
//! # Accuracy features
//!
//! Wherever bit-exact reproduction costs performance, the exact behaviour is the default and a
//! `native-*` Cargo feature opts into the fast path:
//!
//! - `native-libm`: [`cos`], [`sin`], [`asin`], [`acos`], [`exp`] and [`pow`] (and their `f32`
//!   forms [`asinf`], [`acosf`], [`expf`], [`powf`]) call the platform C library instead of the
//!   exact MSVCR110 ports. The platform results are within a few ulps but differ in the last
//!   bit on some inputs, and between platforms.
//!
//! # The C runtime math of Server.exe
//!
//! Server.exe links MSVCR110 dynamically; its math imports are exactly
//! `_libm_sse2_{sin,cos,asin,acos,exp,pow,sqrt}_precise` (no `atan2`, `log`, `fmod` or `_CI*`
//! x87 intrinsic, and no inline `fsin`/`fpatan`/`fsqrt`...). Each is ported here from
//! `game/msvcr110.dll` except `sqrt`, which is a plain `sqrtsd` (correctly rounded, so
//! `f64::sqrt` / `f32::sqrt` are already exact: `(float)sqrt((double)f)` equals the `f32`
//! square root because double rounding is harmless for `sqrt` at these precisions). The
//! game calls `asin`, `acos`, `exp` and `pow` through single-precision wrappers
//! (`(float)fn((double)x)`), provided here as [`asinf`], [`acosf`], [`expf`], [`powf`].
//! `tools/oracle/libm_emu.py` produces the goldens by running the DLL routines in an x86
//! emulator.

pub mod acos_msvc;
pub mod asin_msvc;
pub mod cos_msvc;
pub mod exp_msvc;
pub mod noise;
pub mod pow_msvc;
pub mod rand;
pub mod sin_msvc;
pub mod sort;
mod x87;


/// Cosine as the generator uses it.
///
/// Default: [`cos_msvc::cos`], a bit-exact port of MSVCR110's routine, identical on every
/// target. With the `native-libm` feature: the platform's `f64::cos`, which differs from the
/// original in the last bit on a few percent of inputs.
#[inline]
pub fn cos(x: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.cos()
    }
    #[cfg(not(feature = "native-libm"))]
    {
        cos_msvc::cos(x)
    }
}
/// Sine as the generator uses it: [`sin_msvc::sin`] by default, the platform's with `native-libm`.
#[inline]
pub fn sin(x: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.sin()
    }
    #[cfg(not(feature = "native-libm"))]
    {
        sin_msvc::sin(x)
    }
}

/// Arcsine: [`asin_msvc::asin`] by default, the platform's with `native-libm`.
#[inline]
pub fn asin(x: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.asin()
    }
    #[cfg(not(feature = "native-libm"))]
    {
        asin_msvc::asin(x)
    }
}

/// Arccosine: [`acos_msvc::acos`] by default, the platform's with `native-libm`.
#[inline]
pub fn acos(x: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.acos()
    }
    #[cfg(not(feature = "native-libm"))]
    {
        acos_msvc::acos(x)
    }
}

/// `e^x`: [`exp_msvc::exp`] by default, the platform's with `native-libm`.
#[inline]
pub fn exp(x: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.exp()
    }
    #[cfg(not(feature = "native-libm"))]
    {
        exp_msvc::exp(x)
    }
}

/// `x^y`: [`pow_msvc::pow`] by default, the platform's with `native-libm`.
#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    #[cfg(feature = "native-libm")]
    {
        x.powf(y)
    }
    #[cfg(not(feature = "native-libm"))]
    {
        pow_msvc::pow(x, y)
    }
}

/// `(float)asin((double)x)`, the wrapper at `Server.exe 0x00402480`.
#[inline]
pub fn asinf(x: f32) -> f32 {
    asin(f64::from(x)) as f32
}

/// `(float)acos((double)x)`, the wrapper `acosf` at `Server.exe 0x00548b00`.
#[inline]
pub fn acosf(x: f32) -> f32 {
    acos(f64::from(x)) as f32
}

/// `(float)exp((double)x)`, the wrapper `expf` at `Server.exe 0x00548b20`.
#[inline]
pub fn expf(x: f32) -> f32 {
    exp(f64::from(x)) as f32
}

/// `(float)pow((double)x, (double)y)`, the wrapper at `Server.exe 0x004055a0` and the same
/// sequence inlined at the other call sites.
#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    pow(f64::from(x), f64::from(y)) as f32
}

pub use noise::value_noise_2d;
pub use rand::MsvcRand;
