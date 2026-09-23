//! Bit-exact checks of the MSVCR110 math ports against samples of the DLL routines run in an
//! x86 emulator (`tools/oracle/libm_emu.py golden`; the emulator reproduces the cos/sin samples
//! captured from the running Server.exe, `libm_emu.py selftest`).

fn f64_from_hex(s: &str) -> f64 {
    let b: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    f64::from_le_bytes(b.try_into().unwrap())
}

fn rows(text: &str) -> Vec<Vec<f64>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split(' ').map(f64_from_hex).collect())
        .collect()
}

/// Bits compared exactly, except that any NaN matches any NaN when `loose_nan` is set.
fn same(got: f64, want: f64, loose_nan: bool) -> bool {
    got.to_bits() == want.to_bits() || (loose_nan && got.is_nan() && want.is_nan())
}

fn check(
    name: &str,
    text: &str,
    min: usize,
    f: impl Fn(&[f64]) -> f64,
    loose_nan: bool,
    max_ulps: i64,
) {
    let rows = rows(text);
    assert!(
        rows.len() >= min,
        "{name}: golden file is too small: {}",
        rows.len()
    );
    let mut bad = Vec::new();
    for r in &rows {
        let (args, want) = r.split_at(r.len() - 1);
        let want = want[0];
        if max_ulps != 0 && args.iter().any(|a| a.is_nan()) {
            // C99 `pow(1, NaN) = pow(NaN, 0) = 1`; MSVCR110 returns the NaN.
            continue;
        }
        let got = f(args);
        let ok = if max_ulps == 0 {
            same(got, want, loose_nan)
        } else {
            (got.is_nan() && want.is_nan())
                || (got.to_bits() as i64 - want.to_bits() as i64).abs() <= max_ulps
        };
        if !ok {
            bad.push((args.to_vec(), want, got));
        }
    }
    assert!(
        bad.is_empty(),
        "{name}: {} of {} samples differ; first: {:?}",
        bad.len(),
        rows.len(),
        &bad[..bad.len().min(8)]
    );
}

macro_rules! golden {
    ($exact:ident, $close:ident, $name:literal, $file:literal, $min:expr, |$a:ident| $e:expr) => {
        #[test]
        #[cfg_attr(
            feature = "native-libm",
            ignore = "native libm is only close to MSVCR110"
        )]
        fn $exact() {
            check($name, include_str!($file), $min, |$a: &[f64]| $e, false, 0);
        }
        #[test]
        fn $close() {
            // Any feature: within a few ulps (NaN payloads aside).
            check($name, include_str!($file), $min, |$a: &[f64]| $e, true, 4);
        }
    };
}

golden!(
    exp_matches_msvcr110_exactly,
    exp_is_close_to_msvcr110,
    "exp",
    "golden/exp_msvcr110.txt",
    8000,
    |a| cw_math::exp(a[0])
);
golden!(
    asin_matches_msvcr110_exactly,
    asin_is_close_to_msvcr110,
    "asin",
    "golden/asin_msvcr110.txt",
    6000,
    |a| cw_math::asin(a[0])
);
golden!(
    acos_matches_msvcr110_exactly,
    acos_is_close_to_msvcr110,
    "acos",
    "golden/acos_msvcr110.txt",
    6000,
    |a| cw_math::acos(a[0])
);
golden!(
    pow_matches_msvcr110_exactly,
    pow_is_close_to_msvcr110,
    "pow",
    "golden/pow_msvcr110.txt",
    11000,
    |a| cw_math::pow(a[0], a[1])
);

fn lcg32(i: u32) -> u32 {
    i.wrapping_mul(0x9e3779b1)
}

/// FNV-1a 64 over the little-endian results, as `libm_emu.py hash` computes it.
fn hash_of(count: u32, f: impl Fn(f64, f64) -> f64) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for i in 0..count {
        let a = f64::from(lcg32(2 * i)) / 4294967296.0;
        let b = f64::from(lcg32(2 * i + 1)) / 4294967296.0;
        for byte in f(a, b).to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:x}")
}

#[test]
#[cfg_attr(
    feature = "native-libm",
    ignore = "native libm is only close to MSVCR110"
)]
fn asin_acos_match_msvcr110_on_200000_inputs_in_minus_one_to_one() {
    // `libm_emu.py hash asin` / `hash acos` (x = 2a - 1).
    assert_eq!(
        hash_of(200_000, |a, _| cw_math::asin(a * 2.0 - 1.0)),
        "d39c2ee20c686860"
    );
    assert_eq!(
        hash_of(200_000, |a, _| cw_math::acos(a * 2.0 - 1.0)),
        "f34405c413e17c13"
    );
}

#[test]
#[cfg_attr(
    feature = "native-libm",
    ignore = "native libm is only close to MSVCR110"
)]
fn exp_matches_msvcr110_on_200000_inputs_including_overflow_and_underflow() {
    // `libm_emu.py hash exp` (x = 1500a - 750).
    assert_eq!(
        hash_of(200_000, |a, _| cw_math::exp(a * 1500.0 - 750.0)),
        "d0f4e1947291a0a9"
    );
}

#[test]
#[cfg_attr(
    feature = "native-libm",
    ignore = "native libm is only close to MSVCR110"
)]
fn pow_matches_msvcr110_on_200000_inputs_including_overflow_and_underflow() {
    // `libm_emu.py hash pow` (x = 4a, y = 400b - 200).
    assert_eq!(
        hash_of(200_000, |a, b| cw_math::pow(a * 4.0, b * 400.0 - 200.0)),
        "6d4d1d2985dacf7b"
    );
}

#[test]
#[cfg_attr(
    feature = "native-libm",
    ignore = "native libm is only close to MSVCR110"
)]
fn pow_matches_msvcr110_on_200000_wide_inputs() {
    // `libm_emu.py hash pow_wide` (x = 1e6 a, y = 160b - 80): subnormal results and both
    // scaled paths.
    assert_eq!(
        hash_of(200_000, |a, b| cw_math::pow(a * 1e6, b * 160.0 - 80.0)),
        "5e382197af01790d"
    );
    let (mut sub, mut big) = (0, 0);
    for i in 0..200_000u32 {
        let v = cw_math::pow(
            f64::from(lcg32(2 * i)) / 4294967296.0 * 1e6,
            f64::from(lcg32(2 * i + 1)) / 4294967296.0 * 160.0 - 80.0,
        );
        sub += usize::from(v != 0.0 && v.abs() < f64::MIN_POSITIVE);
        big += usize::from(v.abs() > 1e300);
    }
    assert!(
        sub > 50 && big > 50,
        "coverage: {sub} subnormal, {big} near overflow"
    );
}
