//! Asset blob de-obfuscation.
//!
//! Every value in the `blobs` tables of data1.db through data4.db is stored shuffled and
//! inverted; the asset readers call `Server.exe 0x00412f80` (`decodeBlob`) on the vector
//! `getBlobVec` filled. The world save databases are not obfuscated (see `save.rs`).
//! `decodeBlob` undoes the shuffle in place on a `std::vector<char>`:
//!
//! ```text
//! for i in (n-1) down to 0:
//!     j = (KEY[i % 44] + i) % n
//!     swap(data[i], data[j])
//! for every byte: b = ~b
//! ```
//!
//! [`encode`] is the exact inverse: invert every byte, then replay the swaps in the
//! opposite order.

/// The 44-entry key table at `Server.exe 0x0055aa68`. Cube.exe carries the same table.
pub const KEY: [u32; 44] = [
    4242, 9551, 840, 84800, 9242, 9846, 127, 9, 9483, 394, 123, 4834, 32444, 24355, 2433, 17,
    34234, 42342, 4243, 14, 184934, 1987, 3094, 1901, 89409, 4813, 37, 143, 3490, 19483, 1343, 432,
    84732, 9184, 9612, 1233, 3434, 1839, 2984, 1993, 2984, 4895, 816583, 13,
];

/// Swap partner of index `i` in a buffer of length `n`.
///
/// The original computes `(KEY[i % 44] + i) % n` in 32-bit unsigned arithmetic; the sum
/// cannot overflow because blobs are far smaller than `u32::MAX - 816583`.
#[inline]
fn partner(i: usize, n: usize) -> usize {
    let k = KEY[i % KEY.len()] as usize;
    (k + i) % n
}

/// Turn stored bytes into the real file contents, in place.
pub fn decode(data: &mut [u8]) {
    let n = data.len();
    for i in (0..n).rev() {
        data.swap(i, partner(i, n));
    }
    for b in data.iter_mut() {
        *b = !*b;
    }
}

/// Turn real file contents into the stored form, in place. Inverse of [`decode`].
pub fn encode(data: &mut [u8]) {
    let n = data.len();
    for b in data.iter_mut() {
        *b = !*b;
    }
    for i in 0..n {
        data.swap(i, partner(i, n));
    }
}
