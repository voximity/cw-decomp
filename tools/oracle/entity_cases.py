r"""Deterministic input cases for the entity delta codec goldens (`server_oracle.py entity`).

`crates/cw-net/tests/entity_delta.rs` regenerates the same cases; the two generators must stay
identical, draw for draw. Every random draw is one xorshift32 step; a byte is the low byte of a
draw.

Case i (0-based), state = ((i + 1) * 0x9e3779b1) mod 2^32, or 1 when that is 0:

1. prev: 0x1168 random bytes.
2. Every item (consumable at 0x1d8, equipment at 0x2f0 + k * 0x118): spirit count u32 = draw % 33
   (the item comparator loops that many times).
3. If draw % 2 == 0: a NUL at name byte draw % 16 of prev.
4. cur = prev; then for each of the 48 fields in mask order, r = draw % 8:
   - r == 0: the field's bytes in cur are replaced with random bytes;
   - r == 1 and the field is floats: w = draw % 3: 0 puts NaN (7fc00000) in cur's first float,
     1 puts +0.0 in prev's and -0.0 in cur's first float, 2 puts NaN in both.
5. Every item of cur: if draw % 2 == 0 its spirit count is copied from prev, else it is draw % 33.
6. If draw % 2 == 0: a NUL at name byte draw % 16 of cur.
7. full = (i % 5 == 0).
8. target: 0x1168 random bytes (the block the reader writes into).
9. Two draws a, b: the delta given to the reader is truncated to b % (len + 1) bytes when
   a % 3 == 0, else whole.
"""
import struct
from dataclasses import dataclass

ENTITY_SIZE = 0x1168
ITEM_OFFSETS = [0x1d8] + [0x2f0 + k * 0x118 for k in range(13)]
# (offset, size, is_float) in mask-bit order; mirrors cw_net::entity::FIELDS.
FIELDS = [
    (0x000, 24, False), (0x018, 12, True), (0x024, 12, True), (0x030, 12, True), (0x03c, 12, True),
    (0x048, 4, True), (0x04c, 4, False), (0x050, 1, False), (0x054, 4, False), (0x058, 1, False),
    (0x05c, 4, False), (0x060, 4, False), (0x064, 4, False), (0x068, 0xac, False), (0x114, 2, False),
    (0x118, 4, False), (0x11c, 4, False), (0x120, 4, False), (0x124, 4, False), (0x128, 4, False),
    (0x12c, 4, True), (0x130, 1, False), (0x131, 1, False), (0x134, 4, True), (0x138, 12, True),
    (0x144, 12, True), (0x150, 12, True), (0x15c, 4, True), (0x160, 4, True), (0x164, 4, True),
    (0x168, 20, True), (0x17c, 1, False), (0x17d, 1, False), (0x180, 4, False), (0x184, 4, False),
    (0x188, 8, False), (0x190, 8, False), (0x198, 1, False), (0x19c, 4, False), (0x1a0, 12, False),
    (0x1b0, 24, False), (0x1cc, 12, False), (0x1c8, 1, False), (0x1d8, 0x118, False),
    (0x2f0, 0xe38, False), (0x1158, 16, False), (0x1128, 44, False), (0x1154, 4, False),
]
assert len(FIELDS) == 48
NAN = bytes.fromhex("0000c07f")
POS_ZERO = bytes.fromhex("00000000")
NEG_ZERO = bytes.fromhex("00000080")


class Xorshift32:
    def __init__(self, seed):
        self.s = seed & 0xffffffff or 1

    def draw(self):
        s = self.s
        s ^= (s << 13) & 0xffffffff
        s ^= s >> 17
        s ^= (s << 5) & 0xffffffff
        self.s = s
        return s

    def byte(self):
        return self.draw() & 0xff

    def bytes(self, n):
        return bytes(self.byte() for _ in range(n))


@dataclass
class Case:
    prev: bytes
    cur: bytes
    full: bool
    target: bytes
    trunc_a: int
    trunc_b: int


def make_case(i):
    rng = Xorshift32((i + 1) * 0x9e3779b1)
    prev = bytearray(rng.bytes(ENTITY_SIZE))
    for off in ITEM_OFFSETS:
        struct.pack_into("<I", prev, off + 0x114, rng.draw() % 33)
    if rng.draw() % 2 == 0:
        prev[0x1158 + rng.draw() % 16] = 0
    cur = bytearray(prev)
    for off, size, is_float in FIELDS:
        r = rng.draw() % 8
        if r == 0:
            cur[off:off + size] = rng.bytes(size)
        elif r == 1 and is_float:
            w = rng.draw() % 3
            if w == 0:
                cur[off:off + 4] = NAN
            elif w == 1:
                prev[off:off + 4] = POS_ZERO
                cur[off:off + 4] = NEG_ZERO
            else:
                prev[off:off + 4] = NAN
                cur[off:off + 4] = NAN
    for off in ITEM_OFFSETS:
        if rng.draw() % 2 == 0:
            cur[off + 0x114:off + 0x118] = prev[off + 0x114:off + 0x118]
        else:
            struct.pack_into("<I", cur, off + 0x114, rng.draw() % 33)
    if rng.draw() % 2 == 0:
        cur[0x1158 + rng.draw() % 16] = 0
    target = rng.bytes(ENTITY_SIZE)
    a = rng.draw()
    b = rng.draw()
    return Case(bytes(prev), bytes(cur), i % 5 == 0, target, a, b)


def truncation(case, delta_len):
    return case.trunc_b % (delta_len + 1) if case.trunc_a % 3 == 0 else delta_len
