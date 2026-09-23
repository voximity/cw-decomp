//! The `rand` state `World::load` leaves on the thread that calls it.
//!
//! `Server.exe 0x004d83a0` / `Cube.exe 0x005a52e0` (byte-identical in this part): the seed is
//! stored at `world+0x800164`, then `srand(seed)` (Server.exe 0x004d83ea, Cube.exe 0x005a532a)
//! and 77 `rand() % 100000` draws fill the sub-seed table (the last at Server.exe 0x004d85f0,
//! Cube.exe 0x005a5530). Nothing else in `load` draws: the unload pass (`saveZone`), the
//! database open and the `time` blob read reach no `rand`/`srand`.

use cw_math::MsvcRand;
use cw_world::{Seeds, World};

#[test]
fn new_world_rng_is_srand_seed_after_the_77_sub_seed_draws() {
    for seed in [0, 1, 123, 26879, 1_234_567, -5] {
        let mut expected = MsvcRand::new(seed as u32);
        for _ in 0..77 {
            expected.rand();
        }
        let world = World::new(seed);
        assert_eq!(world.rng, expected, "seed {seed}");
        assert_eq!(world.rng, Seeds::from_seed_with_rng(seed).1, "seed {seed}");
    }
}

#[test]
fn generator_copy_keeps_a_fresh_thread_stream() {
    // Server.exe's generation thread never calls `load`: its stream starts at 1 (the CRT's
    // per-thread `_holdrand`), and `createRegion`/`generateZone` reseed it before any draw.
    let world = World::new(26879);
    assert_eq!(world.generator_copy().rng, MsvcRand::default());
}
