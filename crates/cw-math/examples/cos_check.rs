//! Compares `cw_math::cos` with MSVCR110 samples from `server_oracle.py cos`.
//! Usage: cargo run --release -p cw-math --example cos_check -- <samples.txt>
fn main() {
    let path = std::env::args().nth(1).expect("path to samples");
    let text = std::fs::read_to_string(path).unwrap();
    let mut n = 0;
    let mut bad = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        let mut it = line.split(' ');
        let x = f64::from_bits(u64::from_le_bytes(hex(it.next().unwrap())));
        let want = f64::from_bits(u64::from_le_bytes(hex(it.next().unwrap())));
        let got = cw_math::cos(x);
        let native = x.cos();
        if got.to_bits() != want.to_bits() {
            bad.push((x, want, got, native, (got.to_bits() as i64 - want.to_bits() as i64)));
        }
        n += 1;
    }
    println!("{} of {} differ", bad.len(), n);
    let native_bad = bad.iter().filter(|b| b.3.to_bits() != b.1.to_bits()).count();
    println!("of those, native f64::cos also differs on {native_bad}");
    for b in bad.iter().take(12) {
        println!("x={:.17e} want={:.17e} got={:.17e} native={:.17e} ulp_diff={}", b.0, b.1, b.2, b.3, b.4);
    }
}
fn hex(s: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    for i in 0..8 { out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap(); }
    out
}
