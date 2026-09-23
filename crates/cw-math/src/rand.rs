//! The MSVC runtime's `rand`/`srand` (MSVCR110.dll), a 32-bit linear congruential generator.
//!
//! The original keeps one state per thread. World generation seeds it with
//! `srand(seed)` and then consumes values in a fixed order, so every `rand()` call of the
//! original must be mirrored by exactly one [`MsvcRand::rand`] call in the port.

/// State of one MSVC `rand` stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsvcRand {
    state: u32,
}

impl MsvcRand {
    /// `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// Equivalent to `srand(seed)` on an existing stream.
    pub fn seed(&mut self, seed: u32) {
        self.state = seed;
    }

    /// `rand()`: returns a value in `0..=0x7fff`.
    pub fn rand(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(214013).wrapping_add(2531011);
        ((self.state >> 16) & 0x7fff) as i32
    }
}

impl Default for MsvcRand {
    /// A fresh CRT thread starts with state 1.
    fn default() -> Self {
        Self::new(1)
    }
}
