//! Bit-exact port of the world generator in Server.exe.
//!
//! Tier A: every function here must reproduce the original byte for byte. Doc comments
//! cite the original as `Server.exe 0x<address>`; `analysis/notes/worldgen.md` holds the
//! structure notes. Golden samples captured from the running original with
//! `tools/oracle/server_oracle.py` live in `tests/golden/`.

pub mod appearance;
pub mod climate;
pub mod color;
pub mod creatures;
pub mod decoration;
pub mod dungeon;
pub mod features;
pub mod fixed;
pub mod generate;
pub mod height;
pub mod inventory;
pub mod missions;
pub mod model;
pub mod persistence;
pub mod postpass;
pub mod region;
pub mod save;
pub mod seeds;
pub mod settlement;
pub mod spawn_tables;
pub mod static_creatures;
pub mod statics;
pub mod surface;
pub mod tree;
pub mod warp;
pub mod world;
pub mod zone;

pub use climate::ClimatePoint;
pub use seeds::Seeds;
pub use world::World;
