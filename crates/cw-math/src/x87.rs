//! Just enough of the x87 unit, in software, to reproduce the few places where the MSVCR110
//! SSE2 routines drop into 80-bit arithmetic (the scaling step of `exp` and `pow` near
//! overflow/underflow). Values carry a 64-bit significand and an unbounded exponent; every
//! operation rounds to nearest-even at 64 bits, as the x87 does with precision control set to
//! extended (`or cw, 0x300`), and [`Ext::to_f64`] is `fstp qword` (a single correct rounding
//! to the double grid, subnormals included).

/// `(-1)^neg * m * 2^e`, with `m` either zero or normalized (bit 63 set).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ext {
    pub neg: bool,
    pub m: u64,
    pub e: i32,
}

/// Rounds `m * 2^e` (`m` < 2^128, `sticky`: nonzero bits below `m`) to a 64-bit significand.
fn round64(neg: bool, m: u128, e: i32, sticky: bool) -> Ext {
    if m == 0 {
        return Ext { neg, m: 0, e: 0 };
    }
    let lz = m.leading_zeros() as i32;
    // Normalize so bit 127 is set.
    let m = m << lz;
    let e = e - lz;
    let hi = (m >> 64) as u64;
    let lo = m as u64;
    let half = 1u64 << 63;
    let up = lo > half || (lo == half && (sticky || hi & 1 == 1));
    let (hi, e) = if up {
        match hi.checked_add(1) {
            Some(v) => (v, e + 64),
            None => (half, e + 65),
        }
    } else {
        (hi, e + 64)
    };
    Ext { neg, m: hi, e }
}

impl Ext {
    /// `fld qword`: exact. Only finite values are needed here.
    pub fn from_f64(x: f64) -> Ext {
        let b = x.to_bits();
        let neg = b >> 63 != 0;
        let ef = ((b >> 52) & 0x7ff) as i32;
        let f = b & 0x000f_ffff_ffff_ffff;
        debug_assert!(ef != 0x7ff);
        let (m, e) = if ef == 0 {
            (f, -1074)
        } else {
            (f | 1 << 52, ef - 1075)
        };
        round64(neg, u128::from(m), e, false)
    }

    /// `fmul`, rounded to 64 bits.
    pub fn mul(self, o: Ext) -> Ext {
        round64(
            self.neg != o.neg,
            u128::from(self.m) * u128::from(o.m),
            self.e + o.e,
            false,
        )
    }

    /// `fadd`, rounded to 64 bits (round to nearest: an exact zero sum is +0).
    pub fn add(self, o: Ext) -> Ext {
        if self.m == 0 {
            return if o.m == 0 {
                Ext {
                    neg: self.neg && o.neg,
                    m: 0,
                    e: 0,
                }
            } else {
                o
            };
        }
        if o.m == 0 {
            return self;
        }
        let (a, b) = if self.e >= o.e { (self, o) } else { (o, self) };
        // Place `a` with 62 guard bits; shift `b` right by the exponent difference.
        let d = (a.e - b.e) as u32;
        let am = u128::from(a.m) << 62;
        let bw = u128::from(b.m) << 62;
        let (bm, sticky) = if d >= 127 {
            (0, true)
        } else {
            (bw >> d, bw & ((1u128 << d) - 1) != 0)
        };
        let e = a.e - 62;
        if a.neg == b.neg {
            round64(a.neg, am + bm, e, sticky)
        } else if am > bm || (am == bm && sticky) {
            // Subtracting a sticky remainder: borrow one unit and keep the sticky flag.
            let r = am - bm - u128::from(sticky);
            round64(a.neg, r, e, sticky)
        } else if am == bm {
            Ext {
                neg: false,
                m: 0,
                e: 0,
            }
        } else {
            round64(b.neg, bm - am, e, sticky)
        }
    }

    /// `fstp qword` under round-to-nearest: one rounding to the double grid (subnormal or
    /// normal), overflowing to infinity.
    pub fn to_f64(self) -> f64 {
        let sign = if self.neg { 1u64 << 63 } else { 0 };
        if self.m == 0 {
            return f64::from_bits(sign);
        }
        // value = m * 2^e, m in [2^63, 2^64). Unbiased exponent of the leading bit: e + 63.
        let lead = self.e + 63;
        // Bits to drop: normal numbers keep 53 bits; subnormals keep bits down to 2^-1074.
        let drop = if lead >= -1022 {
            11
        } else {
            11 + (-1022 - lead)
        };
        let m = u128::from(self.m);
        let (q, rem, half) = if drop >= 66 {
            (0u128, m, 1u128 << 65) // below half of the smallest subnormal: value < 2^-1075
        } else {
            (m >> drop, m & ((1u128 << drop) - 1), 1u128 << (drop - 1))
        };
        let q = if rem > half || (rem == half && q & 1 == 1) {
            q + 1
        } else {
            q
        };
        // q * 2^(e + drop); build the double from q and that exponent.
        let e = self.e + drop;
        if q == 0 {
            return f64::from_bits(sign);
        }
        let q = q as u64;
        let bits = if lead < -1022 || (q < 1 << 52) {
            // Subnormal (possibly rounded up to the smallest normal: q == 2^52 carries into
            // the exponent field naturally).
            q
        } else {
            // Normal; rounding may have carried q to 2^53.
            let (q, e) = if q >= 1 << 53 {
                (q >> 1, e + 1)
            } else {
                (q, e)
            };
            let ef = e + 52 + 1023;
            if ef >= 0x7ff {
                return f64::from_bits(sign | 0x7ff0_0000_0000_0000);
            }
            (u64::from(ef as u32) << 52) | (q & 0x000f_ffff_ffff_ffff)
        };
        f64::from_bits(sign | bits)
    }
}

#[cfg(test)]
mod tests {
    use super::Ext;

    #[test]
    fn round_trips_doubles() {
        for &x in &[
            1.0,
            -2.5,
            5e-324,
            2.2250738585072014e-308,
            1.7976931348623157e308,
            0.1,
        ] {
            assert_eq!(Ext::from_f64(x).to_f64().to_bits(), f64::to_bits(x));
        }
    }

    #[test]
    fn double_arithmetic_agrees_when_exact_or_rounded_once() {
        let a = Ext::from_f64(1.0).add(Ext::from_f64(2f64.powi(-60)));
        // 1 + 2^-60 is representable in 64 bits, then rounds to 1.0 as a double.
        assert_eq!(a.to_f64(), 1.0);
        assert_eq!(Ext::from_f64(3.0).mul(Ext::from_f64(0.5)).to_f64(), 1.5);
        let two = |k: i32| Ext {
            neg: false,
            m: 1 << 63,
            e: k - 63,
        };
        assert_eq!(two(-1074).to_f64().to_bits(), 1);
        assert_eq!(two(-1075).to_f64().to_bits(), 0); // tie to even
        assert_eq!(Ext::from_f64(1.5).mul(two(-1075)).to_f64().to_bits(), 1);
        assert_eq!(
            Ext::from_f64(1.0)
                .add(Ext::from_f64(-1.0))
                .to_f64()
                .to_bits(),
            0
        );
    }
}
