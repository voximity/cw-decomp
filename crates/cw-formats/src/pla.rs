//! `.pla` files: the older, numeric-tag Plasma graphics format.
//!
//! Read by `Cube.exe 0x006555d0`, which `Engine::loadFile` (`Cube.exe 0x00653770`) picks for
//! the `.pla` extension. No `.pla` file ships with the alpha (none on disk, none in the
//! asset databases), so this reader is ported from the decompilation alone and tested on
//! synthetic data only.
//!
//! # Wire format
//!
//! Like `.plx` ([`crate::plx`]) the file is a sequence of `[i32 tag][i32 size][size bytes]`
//! records, but tags are small integers instead of interned names, and there is no seal.
//! Every reader starts a record by reading its size (`0x00650a20`); unknown tags are skipped
//! by size (`0x00659e80`), so the record structure is always recoverable. The top-level
//! loop runs until a read fails (end of file).
//!
//! Top-level tags (the switch in `0x006555d0`):
//!
//! | Tag | Content | Reader |
//! |---|---|---|
//! | 1 | Transformation | `0x00659320` |
//! | 2, 15, 16 | skipped | `0x00659e80` |
//! | 3 | MeshShape | `0x00657f80` |
//! | 4 | shape (appended to a list, counted as a shape) | `0x00658630` |
//! | 5 | Texture: records 1 name, 2 id, 3/4/5/9/10 the five format ints, 6 width, 7 height, 8 pixels | inline |
//! | 6 | Font: records 1 name, 2 file name, 3 pixel size, 4 glow | inline |
//! | 7 | Node: records 1 name, 2 (ignored int), 3 child, 4 shape, 5 transformation, 6 widget, 7 flags, 8/9 keyed attributes (`0x0064cbe0`), 10 variable (two strings), 11 keyed attribute (`0x0064ca40`) | inline |
//! | 8 | (`0x00654ff0`) | `0x00654ff0` |
//! | 9 | name table entry: records 1 id, 3 string | inline |
//! | 10 | CurveShape | `0x00654240` |
//! | 11, 12 | widget | `0x00659740` |
//! | 13 | Button | `0x00654000` |
//! | 14 | widget | `0x00654700` |
//! | 17 | widget | `0x00657a00` |
//! | 18 | widget | `0x00657ce0` |
//! | 19 | widget | `0x00654df0` |
//! | 20 | page width (f32) | inline |
//! | 21 | page height (f32) | inline |
//! | 22 | page dpi (f32, default 100) | inline |
//! | 23 | page unit (i32) | inline |
//! | 24 | shape | `0x00654900` |
//!
//! Records 5, 6, 7 and 9 are parsed into their sub-records; every other record, and the
//! sub-records of 7 read by the keyed-attribute readers, stay opaque bytes (their inner
//! layouts were not decompiled; see the report). Strings are `[i32 len][len bytes]`
//! (`0x00658530`).

use crate::{Error, Result};

const FORMAT: &str = "pla";

/// One `.pla` record.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaChunk {
    pub tag: i32,
    pub body: PlaBody,
}

/// The payload of a [`PlaChunk`].
#[derive(Debug, Clone, PartialEq)]
pub enum PlaBody {
    Leaf(Vec<u8>),
    Group(Vec<PlaChunk>),
}

/// Top-level tags whose payload is a sequence of sub-records (see the module table).
pub const GROUP_TAGS: [i32; 4] = [5, 6, 7, 9];

/// A parsed `.pla` file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaDocument {
    pub chunks: Vec<PlaChunk>,
}

/// Page settings from tags 20 to 23; `None` where the tag is absent (the reader then keeps
/// the defaults, dpi 100 and unit 0 set at `0x006555d0` entry).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlaPage {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub dpi: Option<f32>,
    pub unit: Option<i32>,
}

fn malformed(reason: impl Into<String>) -> Error {
    Error::Malformed { format: FORMAT, reason: reason.into() }
}

fn read_seq(data: &[u8], mut pos: usize, end: usize, groups: bool) -> Result<Vec<PlaChunk>> {
    let mut out = Vec::new();
    while pos < end {
        if end - pos < 8 {
            return Err(malformed(format!("truncated record header at offset {pos}")));
        }
        let tag = i32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let size = i32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        let start = pos + 8;
        let cend = usize::try_from(size)
            .ok()
            .and_then(|s| start.checked_add(s))
            .filter(|&e| e <= end)
            .ok_or_else(|| malformed(format!("record size {size} at offset {} overruns", pos + 4)))?;
        let body = if groups && GROUP_TAGS.contains(&tag) {
            PlaBody::Group(read_seq(data, start, cend, false)?)
        } else {
            PlaBody::Leaf(data[start..cend].to_vec())
        };
        out.push(PlaChunk { tag, body });
        pos = cend;
    }
    Ok(out)
}

/// Parse a `.pla` file (`Cube.exe 0x006555d0`).
pub fn parse(data: &[u8]) -> Result<PlaDocument> {
    Ok(PlaDocument { chunks: read_seq(data, 0, data.len(), true)? })
}

fn write_seq(out: &mut Vec<u8>, chunks: &[PlaChunk]) {
    for c in chunks {
        out.extend_from_slice(&c.tag.to_le_bytes());
        let at = out.len();
        out.extend_from_slice(&[0; 4]);
        match &c.body {
            PlaBody::Leaf(b) => out.extend_from_slice(b),
            PlaBody::Group(g) => write_seq(out, g),
        }
        let size = (out.len() - at - 4) as i32;
        out[at..at + 4].copy_from_slice(&size.to_le_bytes());
    }
}

/// Serialise a document; the inverse of [`parse`].
pub fn encode(doc: &PlaDocument) -> Vec<u8> {
    let mut out = Vec::new();
    write_seq(&mut out, &doc.chunks);
    out
}

impl PlaDocument {
    /// The page settings (tags 20 to 23; the last occurrence wins, as in the reader).
    pub fn page(&self) -> PlaPage {
        let mut p = PlaPage::default();
        for c in &self.chunks {
            let PlaBody::Leaf(b) = &c.body else { continue };
            let Some(w) = b.get(..4).map(|w| <[u8; 4]>::try_from(w).unwrap()) else { continue };
            match c.tag {
                20 => p.width = Some(f32::from_le_bytes(w)),
                21 => p.height = Some(f32::from_le_bytes(w)),
                22 => p.dpi = Some(f32::from_le_bytes(w)),
                23 => p.unit = Some(i32::from_le_bytes(w)),
                _ => {}
            }
        }
        p
    }

    /// The name table (tag 9): `(id, name)` pairs; the reader stores a name only when the
    /// id record is present and not -1.
    pub fn names(&self) -> Vec<(i32, Vec<u8>)> {
        let mut out = Vec::new();
        for c in &self.chunks {
            let (9, PlaBody::Group(g)) = (c.tag, &c.body) else { continue };
            let mut id = -1;
            let mut name = Vec::new();
            for s in g {
                let PlaBody::Leaf(b) = &s.body else { continue };
                match s.tag {
                    1 if b.len() >= 4 => id = i32::from_le_bytes(b[..4].try_into().unwrap()),
                    3 => name = read_str(b).unwrap_or_default(),
                    _ => {}
                }
            }
            if id != -1 {
                out.push((id, name));
            }
        }
        out
    }
}

/// `[i32 len][len bytes]` (`Cube.exe 0x00658530`), raw bytes.
pub fn read_str(b: &[u8]) -> Option<Vec<u8>> {
    let n = usize::try_from(i32::from_le_bytes(b.get(..4)?.try_into().ok()?)).ok()?;
    b.get(4..4 + n).map(|s| s.to_vec())
}
