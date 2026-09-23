//! `ByteBuffer`: the original's `{u8* begin, u8* end, u8* cap, i32 pos}`, a `std::vector<char>`
//! with a cursor at `+0xc`, used by every packet writer and reader.
//!
//! - `write(p, n)` (`Server.exe 0x004168f0`): `resize(size + n)`, copy at `pos`, `pos += n`. On a
//!   fresh buffer that is an append; with `pos < size` it overwrites from `pos` and still grows
//!   the end by `n`, which is what the original does too.
//! - `read(out, n)` (`Server.exe 0x00415c90`): when `pos + n > size` the cursor moves to the end
//!   and nothing is copied; otherwise the bytes are copied and the cursor advances.
//! - `resize` is `std::vector<char>::resize` (`Server.exe 0x00413180`): growth is zero-filled.

/// A growable byte vector with a read/write cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ByteBuffer {
    pub data: Vec<u8>,
    pub pos: usize,
}

impl ByteBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer over existing bytes with the cursor at the start (what `decompress` and the
    /// receive path leave behind).
    pub fn from_vec(data: Vec<u8>) -> Self {
        Self { data, pos: 0 }
    }

    /// `end - begin`.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Bytes left after the cursor.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// `ByteBuffer::write`, `Server.exe 0x004168f0`.
    pub fn write(&mut self, bytes: &[u8]) {
        let n = bytes.len();
        self.data.resize(self.data.len() + n, 0);
        self.data[self.pos..self.pos + n].copy_from_slice(bytes);
        self.pos += n;
    }

    pub fn write_u8(&mut self, v: u8) {
        self.write(&[v]);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_i32(&mut self, v: i32) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.write(&v.to_le_bytes());
    }

    /// `ByteBuffer::read`, `Server.exe 0x00415c90`: the next `n` bytes, or `None` with the cursor
    /// moved to the end when they do not fit (the caller's destination is left untouched).
    pub fn read(&mut self, n: usize) -> Option<&[u8]> {
        if self.pos + n > self.data.len() {
            self.pos = self.data.len();
            return None;
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }

    pub fn read_u8(&mut self) -> Option<u8> {
        self.read(1).map(|s| s[0])
    }

    pub fn read_u16(&mut self) -> Option<u16> {
        self.read(2).map(|s| u16::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn read_u32(&mut self) -> Option<u32> {
        self.read(4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        self.read(4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn read_u64(&mut self) -> Option<u64> {
        self.read(8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn read_i64(&mut self) -> Option<i64> {
        self.read(8).map(|s| i64::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn read_f32(&mut self) -> Option<f32> {
        self.read(4).map(|s| f32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::ByteBuffer;

    #[test]
    fn write_grows_from_the_cursor_and_read_clamps() {
        let mut b = ByteBuffer::new();
        b.write_u32(7);
        b.write(&[1, 2, 3]);
        assert_eq!(b.data, [7, 0, 0, 0, 1, 2, 3]);
        b.pos = 0;
        assert_eq!(b.read_u32(), Some(7));
        assert_eq!(b.read(4), None);
        assert_eq!(b.pos, 7);
        assert_eq!(b.read_u8(), None);
        // A write with the cursor rewound overwrites in place and still grows the end.
        let mut c = ByteBuffer::from_vec(vec![9, 9, 9]);
        c.write(&[1, 2]);
        assert_eq!(c.data, [1, 2, 9, 0, 0]);
        assert_eq!(c.pos, 2);
    }
}
