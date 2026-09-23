//! The `.cub` voxel model format.
//!
//! `Server.exe 0x0042f9a0` (`cube::Model::load`) reads three little-endian `u32` sizes
//! followed by exactly `x * y * z` RGB byte triples, from a decoded data1.db blob or from
//! a file, and hands them to `0x00430230` (`cube::Model::setVoxels`). Black `[0, 0, 0]`
//! means an empty voxel in the game's models.
//!
//! Voxels are kept in file order here. Which axis varies fastest is decided by
//! `setVoxels`, which has not been read yet, so no `(x, y, z)` accessor is offered until
//! it has.

use crate::{Error, Result};

/// A parsed `.cub` model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CubModel {
    /// Sizes along the three axes, in the order they appear in the file.
    pub size: [u32; 3],
    /// `size[0] * size[1] * size[2]` RGB triples, in file order.
    pub voxels: Vec<[u8; 3]>,
}

const HEADER: usize = 12;

fn malformed(reason: impl Into<String>) -> Error {
    Error::Malformed {
        format: "cub",
        reason: reason.into(),
    }
}

impl CubModel {
    /// Parse a decoded `.cub` blob. The whole slice must be consumed.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER {
            return Err(malformed(format!(
                "{} bytes is too short for the 12-byte header",
                bytes.len()
            )));
        }
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let size = [u32_at(0), u32_at(4), u32_at(8)];
        let count = (size[0] as usize)
            .checked_mul(size[1] as usize)
            .and_then(|n| n.checked_mul(size[2] as usize))
            .ok_or_else(|| malformed(format!("size {size:?} overflows")))?;
        let payload = count
            .checked_mul(3)
            .ok_or_else(|| malformed(format!("size {size:?} overflows")))?;
        let body = &bytes[HEADER..];
        if body.len() != payload {
            return Err(malformed(format!(
                "size {size:?} needs {payload} payload bytes, found {}",
                body.len()
            )));
        }
        let voxels = body.as_chunks::<3>().0.to_vec();
        Ok(Self { size, voxels })
    }

    /// Serialise back to the exact file layout.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.voxels.len() * 3);
        for d in self.size {
            out.extend_from_slice(&d.to_le_bytes());
        }
        for v in &self.voxels {
            out.extend_from_slice(v);
        }
        out
    }
}
