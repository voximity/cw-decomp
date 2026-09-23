//! The simulation of Cube World Alpha's creatures as Server.exe's world tick runs it, over the
//! world of `cw-world` and the entity block of `cw-net`: creature stats (`stats`), the creature
//! a zone spawn becomes (`creature`), the interactions and statics of the tick (`interact`),
//! hits, buffs, death, loot and kill credit (`combat`), and the per-creature update of timers,
//! regeneration and threat (`update`).
//! Doc comments cite the original as `Server.exe 0x<address>`.
//!
//! Tier B: the tick's outputs depend on wall-clock deltas and on the original's physics and
//! AI, so it is verified against packet captures with the fields the tick moves masked
//! (`cw-server`'s `probe_diff` example); the pure parts (stats, spawn conversion, mission
//! rolls in `cw-world`) are bit-exact.

pub mod behavior;
pub mod behaviors_more;
pub mod combat;
pub mod combat_ai;
pub mod creature;
pub mod interact;
pub mod path;
pub mod physics;
pub mod skills;
pub mod stats;
pub mod update;
pub mod util;
pub mod projectile;
pub mod modes;
pub mod riding;
pub mod consumables;
pub mod statics;
pub mod day;
pub mod client;
