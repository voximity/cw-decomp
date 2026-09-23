//! The zlib framing of packets 0 and 4: `u32 len` then `len` bytes of a zlib stream.
//!
//! The original links zlib 1.2.3 (`game/zlib1.dll`): `zlibCompressVec` (`Server.exe 0x00412d00`)
//! calls one-shot `compress()` at the default level 6, and `zlibInflateVec` (`0x00412e20`)
//! inflates in 128000-byte chunks until `Z_STREAM_END`, keeping any partial output on error.
//! Only that zlib version at that level reproduces the compressed bytes; this port uses
//! `flate2`, whose streams are valid but not byte-identical, so tests and the protocol oracle
//! compare decompressed payloads.

use std::io::{Read, Write};

/// `compress()` at level 6 into a zlib stream.
pub fn compress(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(bytes.len() / 2 + 16), flate2::Compression::new(6));
    e.write_all(bytes).expect("in-memory write");
    e.finish().expect("in-memory finish")
}

/// Inflates a zlib stream. Like the original, the bytes produced before an error are returned
/// with the error, since the caller keeps the partial output either way.
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, (Vec<u8>, std::io::Error)> {
    let mut d = flate2::read::ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    match d.read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(e) => Err((out, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        let z = compress(&data);
        assert!(z.len() < data.len());
        assert_eq!(decompress(&z).unwrap(), data);
        assert!(decompress(&z[..z.len() / 2]).is_err());
    }
}
