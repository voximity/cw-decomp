//! The world clock at the start of `World::tick` (`Server.exe 0x005322d0`,
//! 0x00532340..0x005325fd): the time of day, the day rollover and the `time` blob's cadence.
//!
//! The ground items' countdowns (`+0x13c`, `+0x140`) run in the items pass
//! ([`crate::interact::items_pass`], 0x00532ca2..0x00533097). The pass collects no expired
//! items: its erase list (`[ebp-0x2bf8]`, drained at 0x00533097) is never filled, so a ground
//! item stays until a player picks it up.

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_world::World;

use crate::stats::max_hp;
use crate::util::{f32_at, wf32};

/// Milliseconds of a day (`0x5265c00`).
pub const DAY_MS: i32 = 86_400_000;

/// The world's play-time counter the clock keeps outside the day.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock {
    /// `world+0x8000bc`: milliseconds of ticks summed (wrapping), read by the `time` blob's
    /// cadence and the traps' 100 ms period (statics.rs).
    pub play_ms: i32,
}

/// 0x00532340..0x005325fd: one tick of the clock. A living player lying down (mode 0x54, HP >
/// 0) makes the time run 100 times faster than real time, else 10 times; a world without a
/// name (`world+0xa4 == 0`) stays at 09:00; past 24:00 the day advances (time - 86400000) and
/// every region's missions are re-armed (`clearRegionActivation` 0x00524500), as often as
/// needed. Returns whether the `time` blob is due: `play_ms` (already advanced by `dt`) and
/// `play_ms + dt` fall in different 10-second periods (the original compares these two, not
/// the old and new counter) in a named world; the caller writes it when the database is open
/// (`sub_413000(world+0xac)`). In the client's world the local player (`world+0xb8`) lying in
/// bed also regenerates (0x00532374), and its pet item gets its grown-up sub type (0x0053266b,
/// after the `time` blob in the original, which it does not touch).
pub fn advance_clock(world: &mut World, clock: &mut Clock, entities: &mut BTreeMap<i64, EntityData>, dt: i32) -> bool {
    // 0x00532340: any living player lying down.
    let mut sleeping = false;
    for (id, e) in entities.iter_mut() {
        // 0x00532367: comiss hp, 0; jbe skips (NaN too).
        if e.0[0x50] == 0 && e.0[0x58] == 0x54 && f32_at(&e.0, 0x15c) > 0.0f32 {
            // 0x00532374: the local player (`world+0xb8`, NULL on a server) regains
            // `maxHp * (dt * 0.001 * 0.05)` per tick (0x00558858, 0x005586b8), capped at maxHp
            // (0x0040fda0, x87 result stored as f32).
            if world.local_player == Some(*id) {
                let m = max_hp(e);
                let k = (dt as f32 * 0.001f32) * 0.05f32;
                let hp = m * k + f32_at(&e.0, 0x15c);
                wf32(&mut e.0, 0x15c, hp);
                let m = max_hp(e);
                // 0x005323e5: comiss hp, max; jbe keeps (NaN keeps).
                if f32_at(&e.0, 0x15c) > m {
                    let m = max_hp(e);
                    wf32(&mut e.0, 0x15c, m);
                }
            }
            sleeping = true;
        }
    }
    // 0x00532432
    clock.play_ms = clock.play_ms.wrapping_add(dt);
    let step = if sleeping { dt.wrapping_mul(100) } else { dt.wrapping_mul(10) };
    world.time_of_day = world.time_of_day.wrapping_add(step);
    if !world.has_name {
        world.time_of_day = 0x1ee_6280;
    }
    // 0x00532465: `cmp time, 0x5265c00; jle` (signed).
    while world.time_of_day > DAY_MS {
        world.day = world.day.wrapping_add(1);
        world.time_of_day = world.time_of_day.wrapping_sub(DAY_MS);
        world.clear_region_activation();
    }
    // 0x005324a4: `0x68db8bad` / `sar 0xc` is `/ 10000`, truncating.
    let acc = clock.play_ms;
    let due = acc.wrapping_add(dt) / 10000 != acc / 10000 && world.has_name;
    // 0x0053266b: the local player's pet item (slot 12 at entity+0x1010: type 0x13, sub type
    // 0x18) grows up (sub type 0x19) once its level (the i16 at item+0x10) is above 1.
    if let Some(lp) = world.local_player
        && let Some(e) = entities.get_mut(&lp)
    {
        let it = 0x2f0 + 12 * 0x118;
        if e.0[it] == 0x13 && e.0[it + 1] == 0x18 && i16::from_le_bytes([e.0[it + 0x10], e.0[it + 0x11]]) > 1 {
            e.0[it + 1] = 0x19;
        }
    }
    due
}
