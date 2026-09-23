//! Bit-exact checks against samples captured from the original Server.exe.

use cw_math::{MsvcRand, value_noise_2d};

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn f64_from_hex(s: &str) -> f64 {
    f64::from_le_bytes(hex_to_bytes(s).try_into().unwrap())
}

fn f32_from_hex(s: &str) -> f32 {
    f32::from_le_bytes(hex_to_bytes(s).try_into().unwrap())
}

#[test]
fn value_noise_2d_matches_the_original_on_every_golden_sample() {
    let text = include_str!("golden/noise2d.txt");
    let mut checked = 0;
    let mut mismatches = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut it = line.split(' ');
        let x = f64_from_hex(it.next().unwrap());
        let y = f64_from_hex(it.next().unwrap());
        let want = f32_from_hex(it.next().unwrap());
        let got = value_noise_2d(x, y);
        if got.to_bits() != want.to_bits() {
            mismatches.push((x, y, want, got));
        }
        checked += 1;
    }
    assert!(checked >= 20000, "golden file is too small: {checked}");
    assert!(
        mismatches.is_empty(),
        "{} of {} samples differ; first: {:?}",
        mismatches.len(),
        checked,
        &mismatches[..mismatches.len().min(5)]
    );
}

#[test]
fn msvc_rand_matches_the_original_sequence_for_the_default_server_seed() {
    // Captured with `server_oracle.py rand --seed 26879 --count 20`.
    let want = [
        22278, 31446, 10031, 25463, 4253, 12641, 11232, 26032, 15693, 25723, 27979, 32382, 5102,
        21683, 14999, 19968, 19318, 22323, 26133, 10193,
    ];
    let mut r = MsvcRand::new(26879);
    let got: Vec<i32> = (0..20).map(|_| r.rand()).collect();
    assert_eq!(got, want);
}

#[test]
fn msvc_rand_default_state_starts_at_41() {
    // The well-known first value of an unseeded MSVC CRT.
    assert_eq!(MsvcRand::default().rand(), 41);
}

/// FNV-1a 64 over little-endian bytes, as `server_oracle.py hash` computes it.
fn fnv1a(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
}

fn lcg32(i: u32) -> u32 {
    i.wrapping_mul(0x9e3779b1)
}

/// The single-precision pi widened to double, as the original's callers pass it.
const PI_F: f64 = std::f32::consts::PI as f64;

#[test]
#[cfg_attr(feature = "native-libm", ignore = "native libm is only within one ulp of MSVCR110")]
fn cos_matches_msvcr110_exactly_on_every_golden_sample() {
    // `server_oracle.py cos --count 5000`.
    let text = include_str!("golden/cos_msvcr110.txt");
    let mut n = 0;
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut it = line.split(' ');
        let x = f64_from_hex(it.next().unwrap());
        let want = f64_from_hex(it.next().unwrap());
        let got = cw_math::cos(x);
        assert_eq!(got.to_bits(), want.to_bits(), "cos({x:e}) = {got:e}, original {want:e}");
        n += 1;
    }
    assert!(n >= 5000);
}

#[test]
#[cfg_attr(feature = "native-libm", ignore = "native libm is only within one ulp of MSVCR110")]
fn cos_matches_msvcr110_on_four_million_inputs_in_zero_to_pi() {
    // `server_oracle.py hash cos --count 4000000`
    let mut h = 0xcbf29ce484222325u64;
    for i in 0..4_000_000u32 {
        let x = (f64::from(lcg32(i)) / 4294967296.0) * PI_F;
        h = fnv1a(h, &cw_math::cos(x).to_le_bytes());
    }
    assert_eq!(format!("{h:x}"), "57c31dd13a01d34d");
}

#[test]
fn cos_is_within_one_ulp_of_msvcr110_with_any_feature() {
    let text = include_str!("golden/cos_msvcr110.txt");
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let mut it = line.split(' ');
        let x = f64_from_hex(it.next().unwrap());
        let want = f64_from_hex(it.next().unwrap());
        let diff = (cw_math::cos(x).to_bits() as i64 - want.to_bits() as i64).abs();
        assert!(diff <= 1, "cos({x:e}) is {diff} ulps from the original");
    }
}

#[test]
fn value_noise_2d_matches_the_original_on_two_million_inputs() {
    // `server_oracle.py hash noise --count 2000000`
    let mut h = 0xcbf29ce484222325u64;
    for i in 0..2_000_000u32 {
        let x = (f64::from(lcg32(2 * i)) / 4294967296.0) * 1.2e6 - 1e5;
        let y = (f64::from(lcg32(2 * i + 1)) / 4294967296.0) * 1.2e6 - 1e5;
        h = fnv1a(h, &value_noise_2d(x, y).to_le_bytes());
    }
    assert_eq!(format!("{h:x}"), "c96906678f3161cc");
}

/// `(x, want)` pairs of a golden sample file.
fn golden_pairs(text: &str) -> Vec<(f64, f64)> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|line| {
            let mut it = line.split(' ');
            (f64_from_hex(it.next().unwrap()), f64_from_hex(it.next().unwrap()))
        })
        .collect()
}

fn assert_sin_matches_exactly(text: &str, min: usize) {
    let pairs = golden_pairs(text);
    assert!(pairs.len() >= min, "golden file is too small: {}", pairs.len());
    let bad: Vec<_> = pairs
        .iter()
        .filter(|(x, want)| cw_math::sin(*x).to_bits() != want.to_bits())
        .map(|&(x, want)| (x, want, cw_math::sin(x)))
        .collect();
    assert!(
        bad.is_empty(),
        "{} of {} samples differ; first: {:?}",
        bad.len(),
        pairs.len(),
        &bad[..bad.len().min(5)]
    );
}

#[test]
#[cfg_attr(feature = "native-libm", ignore = "native libm is only within one ulp of MSVCR110")]
fn sin_matches_msvcr110_exactly_on_every_golden_sample() {
    // 200000 samples over [0, 2*PI_F).
    assert_sin_matches_exactly(include_str!("golden/sin_msvcr110.txt"), 200_000);
}

#[test]
#[cfg_attr(feature = "native-libm", ignore = "native libm is only within one ulp of MSVCR110")]
fn sin_matches_msvcr110_exactly_on_wide_golden_samples() {
    // 100000 samples over [-400000, 400000], covering the x87 `fsin` fallback (|x| >= 90112).
    assert_sin_matches_exactly(include_str!("golden/sin_msvcr110_wide.txt"), 100_000);
}

#[test]
#[cfg_attr(feature = "native-libm", ignore = "native libm is only within one ulp of MSVCR110")]
fn sin_matches_msvcr110_on_four_million_inputs_in_zero_to_two_pi() {
    // `server_oracle.py hash sin --count 4000000` over [0, 2*PI_F).
    let mut h = 0xcbf29ce484222325u64;
    for i in 0..4_000_000u32 {
        let x = (f64::from(lcg32(i)) / 4294967296.0) * 2.0 * PI_F;
        h = fnv1a(h, &cw_math::sin(x).to_le_bytes());
    }
    assert_eq!(format!("{h:x}"), "c221d0186c77319b");
}

#[test]
fn sin_is_within_one_ulp_of_msvcr110_with_any_feature() {
    for (x, want) in golden_pairs(include_str!("golden/sin_msvcr110.txt")) {
        let diff = (cw_math::sin(x).to_bits() as i64 - want.to_bits() as i64).abs();
        assert!(diff <= 1, "sin({x:e}) is {diff} ulps from the original");
    }
}
