//! 16.16 fixed-point position helpers shared by the generators. Entity positions in the original are
//! `i64` block coordinates scaled by 65536; `_ftol2` (`FUN_0054a946`) truncates toward zero, which is
//! Rust's `as i64`.

/// `int64FromBlock`, `Server.exe 0x004cde40`: whole blocks to 16.16 fixed, `(i64)v << 16`.
#[inline]
pub fn from_block(v: i32) -> i64 {
    i64::from(v) << 16
}

/// `int64SubDouble`, `Server.exe 0x004ce290`: `v - _ftol2(d * -65536.0)` in double, so `d = 0.5`
/// adds half a block.
#[inline]
pub fn add_double(v: i64, d: f64) -> i64 {
    v - (d * -65536.0) as i64
}

/// The sibling of `int64SubDouble`, `Server.exe 0x004e0700`: `v - _ftol2(d * 65536.0)` in double.
#[inline]
pub fn sub_double(v: i64, d: f64) -> i64 {
    v - (d * 65536.0) as i64
}

/// `Server.exe 0x00401530`: `v + _ftol2(f * 65536.0f)`, the multiply in float.
#[inline]
pub fn add_float(v: i64, f: f32) -> i64 {
    v + (f * 65536.0f32) as i64
}

/// The block coordinate `cube::World::getBlockFixed` (`Server.exe 0x00406050`) uses: a negative
/// value moves down one block before the truncating division by 65536.
#[inline]
pub fn to_block(v: i64) -> i32 {
    let v = if v < 0 { v - 65536 } else { v };
    (v / 65536) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(from_block(-3), -3 * 65536);
        assert_eq!(add_double(from_block(2), 0.5), 2 * 65536 + 32768);
        assert_eq!(sub_double(from_block(2), 0.1), 2 * 65536 - 6553);
        assert_eq!(add_float(0, 0.5), 32768);
        assert_eq!(to_block(65535), 0);
        assert_eq!(to_block(-1), -1);
        assert_eq!(to_block(-65536), -2);
    }
}
