//! The entity delta codec against the original: `tools/oracle/entity_cases.py` describes the
//! deterministic cases (regenerated here, draw for draw), `server_oracle.py entity` ran them
//! through `writeEntityDelta` and `readEntityDelta` in Server.exe, and `golden/entity_delta.txt`
//! holds the hashes of the inputs and outputs.

use cw_net::entity::{Compare, FIELDS};
use cw_net::{ByteBuffer, EntityData, read_delta, write_delta};
use sha2::{Digest, Sha256};

const ITEM_OFFSETS: [usize; 14] = [0x1d8, 0x2f0, 0x408, 0x520, 0x638, 0x750, 0x868, 0x980, 0xa98, 0xbb0, 0xcc8, 0xde0, 0xef8, 0x1010];
const NAN: [u8; 4] = [0x00, 0x00, 0xc0, 0x7f];
const POS_ZERO: [u8; 4] = [0; 4];
const NEG_ZERO: [u8; 4] = [0x00, 0x00, 0x00, 0x80];

struct Xorshift32(u32);

impl Xorshift32 {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 1 } else { seed })
    }

    fn draw(&mut self) -> u32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s
    }

    fn byte(&mut self) -> u8 {
        self.draw() as u8
    }

    fn fill(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.byte();
        }
    }
}

struct Case {
    prev: EntityData,
    cur: EntityData,
    full: bool,
    target: EntityData,
    trunc_a: u32,
    trunc_b: u32,
}

fn make_case(i: u32) -> Case {
    let mut rng = Xorshift32::new((i + 1).wrapping_mul(0x9e37_79b1));
    let mut prev = EntityData::ZERO;
    rng.fill(&mut prev.0);
    for off in ITEM_OFFSETS {
        let n = rng.draw() % 33;
        prev.0[off + 0x114..off + 0x118].copy_from_slice(&n.to_le_bytes());
    }
    if rng.draw().is_multiple_of(2) {
        let p = (rng.draw() % 16) as usize;
        prev.0[0x1158 + p] = 0;
    }
    let mut cur = prev.clone();
    for field in FIELDS {
        let (off, size) = (field.offset, field.size);
        let r = rng.draw() % 8;
        if r == 0 {
            rng.fill(&mut cur.0[off..off + size]);
        } else if r == 1 && matches!(field.compare, Compare::F32(_)) {
            match rng.draw() % 3 {
                0 => cur.0[off..off + 4].copy_from_slice(&NAN),
                1 => {
                    prev.0[off..off + 4].copy_from_slice(&POS_ZERO);
                    cur.0[off..off + 4].copy_from_slice(&NEG_ZERO);
                }
                _ => {
                    prev.0[off..off + 4].copy_from_slice(&NAN);
                    cur.0[off..off + 4].copy_from_slice(&NAN);
                }
            }
        }
    }
    for off in ITEM_OFFSETS {
        if rng.draw().is_multiple_of(2) {
            let v: [u8; 4] = prev.0[off + 0x114..off + 0x118].try_into().unwrap();
            cur.0[off + 0x114..off + 0x118].copy_from_slice(&v);
        } else {
            let n = rng.draw() % 33;
            cur.0[off + 0x114..off + 0x118].copy_from_slice(&n.to_le_bytes());
        }
    }
    if rng.draw().is_multiple_of(2) {
        let p = (rng.draw() % 16) as usize;
        cur.0[0x1158 + p] = 0;
    }
    let mut target = EntityData::ZERO;
    rng.fill(&mut target.0);
    let trunc_a = rng.draw();
    let trunc_b = rng.draw();
    Case { prev, cur, full: i.is_multiple_of(5), target, trunc_a, trunc_b }
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn entity_delta_matches_the_original() {
    let text = include_str!("golden/entity_delta.txt");
    let mut checked = 0;
    let mut errors = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split(' ').collect();
        let i: u32 = f[0].parse().unwrap();
        let case = make_case(i);
        assert_eq!(case.full, f[1] == "1", "case {i}: full flag");
        assert_eq!(sha(&case.prev.0), f[5], "case {i}: the prev block generator diverged");
        assert_eq!(sha(&case.cur.0), f[6], "case {i}: the cur block generator diverged");

        let mut buf = ByteBuffer::new();
        write_delta(&mut buf, &case.prev, &case.cur, case.full);
        let delta = buf.into_vec();
        let mask = u64::from_le_bytes(delta[..8].try_into().unwrap());
        let want_len: usize = f[2].parse().unwrap();
        if delta.len() != want_len || format!("{mask:016x}") != f[3] || sha(&delta) != f[4] {
            errors.push(format!("case {i}: delta len {} mask {mask:016x} vs len {want_len} mask {}", delta.len(), f[3]));
            continue;
        }

        let trunc: usize = f[7].parse().unwrap();
        let expected_trunc = if case.trunc_a.is_multiple_of(3) { (case.trunc_b as usize) % (delta.len() + 1) } else { delta.len() };
        assert_eq!(trunc, expected_trunc, "case {i}: truncation");
        let mut reader = ByteBuffer::from_vec(delta[..trunc].to_vec());
        let mut target = case.target.clone();
        read_delta(&mut reader, &mut target);
        if sha(&target.0) != f[8] || reader.pos.to_string() != f[9] {
            errors.push(format!("case {i}: read result differs (pos {} vs {})", reader.pos, f[9]));
        }
        checked += 1;
    }
    assert!(checked >= 100, "only {checked} cases");
    assert!(errors.is_empty(), "{} of {checked} cases differ:\n{}", errors.len(), errors.join("\n"));
}
