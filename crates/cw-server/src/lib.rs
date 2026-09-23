//! `cw-server` as a library: the pieces of Server.exe's `main` (0x00549c50) that the binary
//! (`main.rs`) wires into threads, exposed so that the client's singleplayer mode
//! (`cw-client` `singleplayer.rs`) can run the same server in-process.
//!
//! - [`server`]: the shared state ([`server::Server`]), the tick loop ([`server::tick_loop`],
//!   `main`'s loop) and the ported slices of `World::tick` 0x005322d0 ([`server::world_tick`],
//!   the creature spawns [`server::spawn_creatures`], the discovery answers
//!   [`server::zone_discovered`] / [`server::region_discovered`]);
//! - [`worldgen`]: the generation thread (`generationThread` 0x00549550);
//! - [`conn`]: the accept thread (`acceptLoop` 0x004254a0) with the per-connection receive
//!   and send threads;
//! - [`player`]: the player entity the handshake creates.
//!
//! Nothing here changes behaviour: the binary runs exactly these functions, in the same
//! threads, as before the split.

pub mod conn;
pub mod player;
pub mod server;
pub mod worldgen;
