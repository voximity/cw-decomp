//! The world seed and the sub-seed table derived from it, `Server.exe 0x004d83a0` (`cube::World::load`).

use cw_math::MsvcRand;

/// Byte offset of the seed inside `cube::World`; the sub-seeds follow it.
const SEED_OFFSET: usize = 0x800164;
/// Number of int32 slots from the seed to the end of the object (+0x80029c).
const SLOTS: usize = 78;

/// The seed and every sub-seed of a loaded world, in the original's memory order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seeds {
    slots: [i32; SLOTS],
}

impl Seeds {
    /// Reproduces `World::load`: `srand(seed)` then `rand() % 100000` into each slot in a
    /// fixed order. The order is not the memory order, so it is spelled out here.
    pub fn from_seed(seed: i32) -> Self {
        Self::from_seed_with_rng(seed).0
    }

    /// [`Seeds::from_seed`] returning also the `rand` state `World::load` leaves on the thread
    /// that called it: the main thread, whose stream the world tick draws from afterwards
    /// (`main` 0x00549c50 draws once before `load`, which `srand` discards).
    pub fn from_seed_with_rng(seed: i32) -> (Self, MsvcRand) {
        let mut rng = MsvcRand::new(seed as u32);
        let mut slots = [0i32; SLOTS];
        slots[0] = seed;
        let mut fill = |offset: usize, count: usize| {
            for i in 0..count {
                slots[(offset - SEED_OFFSET) / 4 + i] = rng.rand() % 100000;
            }
        };
        fill(0x800188, 1);
        fill(0x800168, 4);
        fill(0x80018c, 20);
        fill(0x8001dc, 2);
        fill(0x8001e4, 2);
        fill(0x8001ec, 2);
        fill(0x8001f4, 2);
        fill(0x8001fc, 8);
        fill(0x80021c, 6);
        fill(0x800234, 6);
        fill(0x80024c, 10);
        fill(0x800274, 4);
        fill(0x800284, 2);
        fill(0x800178, 4);
        fill(0x80028c, 2);
        fill(0x800294, 2);
        (Self { slots }, rng)
    }

    /// The world seed (`world+0x800164`).
    pub fn seed(&self) -> i32 {
        self.slots[0]
    }

    /// The int32 stored at `world + offset`, for offsets in `0x800164..0x80029c`.
    /// Offsets are cited as in the decompilation so the two sides can be compared directly.
    #[inline]
    pub fn at(&self, offset: usize) -> i32 {
        debug_assert!(offset >= SEED_OFFSET && offset.is_multiple_of(4));
        self.slots[(offset - SEED_OFFSET) / 4]
    }

    /// `at(offset)` widened to `f64`, the form every noise call uses.
    #[inline]
    pub fn f(&self, offset: usize) -> f64 {
        f64::from(self.at(offset))
    }

    /// Every slot in memory order, seed first.
    pub fn slots(&self) -> &[i32; SLOTS] {
        &self.slots
    }
}
