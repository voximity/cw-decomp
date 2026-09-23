//! Bit-exact port of the wire encoding of Server.exe (Cube World Alpha, 2013-07-23).
//!
//! Tier A: every encoder and decoder here reproduces the original byte for byte. Doc comments
//! cite the original as `Server.exe 0x<address>`; `analysis/notes/protocol.md` is the survey of
//! the protocol this follows. Golden samples captured from the running original with
//! `tools/oracle/server_oracle.py entity` live in `tests/golden/`.
//!
//! Nothing here does I/O: [`bytebuf::ByteBuffer`] is the original's `std::vector<char>` plus
//! cursor, [`entity`] is the masked entity delta that packet 0 and the Join packet carry,
//! [`packet`] holds the packet layouts and framing, and [`compress`] the zlib framing of the
//! two compressed packets.

pub mod bytebuf;
pub mod compress;
pub mod entity;
pub mod packet;

pub use bytebuf::ByteBuffer;
pub use entity::{ENTITY_SIZE, EntityData, read_delta, write_delta};
pub use packet::{ClientPacket, Parse, ServerPacket, ServerUpdate, parse_client_packet};
