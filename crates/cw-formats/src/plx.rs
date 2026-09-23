//! `.plx` Plasma graphics documents: the client's GUI scenes (`gui.plx`, `start.plx`,
//! `help.plx`, `quest-tag.plx`, `cursor.plx`).
//!
//! Read by `PlxReader::read` (`Cube.exe 0x00681c70`), which `Engine::loadFile`
//! (`Cube.exe 0x00653770`) picks for the `.plg`, `.pld` and `.plx` extensions.
//!
//! # Wire format
//!
//! The file is a sequence of chunks, every one `[i32 nameId][i32 size][size bytes]`,
//! little endian. Chunk names are interned: before the first chunk that uses a name comes a
//! *name definition*, a chunk with `nameId == 0` whose payload is `[i32 id][i32 len][len
//! bytes]` (`readTag`, `Cube.exe 0x00688220`, reads at most one definition before each tag).
//! Names are the dotted field names of the Plasma classes (`SmoothMeshShape.vertexPositions`,
//! `Attribute.frame`, ...). A chunk's payload is either a plain value or, for elements and
//! animated attributes, a nested chunk sequence that ends exactly at the chunk end
//! (`enterChunk 0x00688180` pushes the end offset, `leaveChunk 0x00688490` tests it,
//! `skipChunk 0x006886f0` skips an unknown tag).
//!
//! The document starts with `PlasmaGraphics` (the format version, an `i32`; the reader
//! throws for a version above 1), then `Seal`. The `Seal` payload is a length-prefixed
//! byte string: "PlasmaGraphics" encrypted with a 64-bit key. The reader tries the keys the
//! engine registered (the GameController constructor registers
//! `plasma_hash("PlasmaXGraphics")`, `Cube.exe 0x0045c011`), then 0, then
//! `plasma_hash("PlasmaGraphics")`, and keeps the first one that decrypts the seal. Every
//! name definition after the seal is encrypted with that key ([`decrypt`],
//! `Cube.exe 0x006880c0`). All five shipped files use `plasma_hash("PlasmaXGraphics")` =
//! `0xfb3a260b5ec7cd5c`.
//!
//! The writer that produced the shipped files numbers names from 1 in order of first use
//! and defines each right before its first use. [`encode`] does the same, which makes
//! `encode(parse(x)) == x` for every shipped file (tested).
//!
//! # Model
//!
//! [`Chunk`] is the lossless generic tree (which tags hold nested chunks is fixed by the
//! readers, see [`is_group`]). [`PlxDocument`] is the typed view: one [`Item`] per top-level
//! chunk, each element a list of typed fields in file order (so repeated fields such as
//! `Node.child` and `SmoothMeshShape.face` and the field order survive). A field whose name
//! the original reader does not know, or whose payload does not decode as the type the
//! reader expects, is kept as an `Other(Chunk)` so that nothing is lost.
//!
//! Objects refer to each other by index: a `Node.shape` is the index of the shape among
//! all `TextShape`, `SmoothMeshShape` and `GenericShape` elements in file order, a
//! `Node.transformation` the index among `Transformation` elements, `Node.display` among
//! `Display` elements, `Node.widget` among all widget elements (`Widget`, `Button`,
//! `ListWidget`, `Edit`, `ScrollButton`, `ScrollSlider`, `PopUpButton`) and `Node.child`
//! among `Node` elements; -1 is "none" (the reader's index maps are seeded with -1 -> null).
//! The first `Node` is read into the load target (its own fields are dropped, only its children
//! are kept). `SmoothMeshShape.texture` values are `Texture.id`s; the loader remaps them through
//! the texture table (-1 stays -1, an unknown id becomes 0). `Transformation.deformation`
//! frame 1 is the transition scratch and always garbage; frame 0 is valid in every shipped file.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::{Error, Result};

const FORMAT: &str = "plx";

/// The name of the first chunk, and the plaintext of the seal.
pub const MAGIC: &str = "PlasmaGraphics";
/// The string whose [`plasma_hash`] the client registers as the name key
/// (`Cube.exe 0x0045c011`, in the GameController constructor).
pub const KEY_STRING: &str = "PlasmaXGraphics";
/// Highest version `PlxReader::read` accepts.
pub const MAX_VERSION: i32 = 1;

// ---------------------------------------------------------------------------------------
// Key hash and name cipher
// ---------------------------------------------------------------------------------------

/// The 64-bit string hash `Cube.exe 0x00687b10`: `h = 0x0003ffffffffffe5`, then
/// `h = h * 31 + byte` for every byte (unsigned), wrapping.
pub fn plasma_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0x0003_ffff_ffff_ffe5;
    for &b in bytes {
        h = h.wrapping_mul(31).wrapping_add(u64::from(b));
    }
    h
}

/// The key the shipped files are sealed with: `plasma_hash("PlasmaXGraphics")`.
pub fn default_key() -> u64 {
    plasma_hash(KEY_STRING.as_bytes())
}

/// Keys `PlxReader::read` tries on the seal, in order, when the engine registered only
/// [`default_key`]: the registered keys, then 0, then `plasma_hash("PlasmaGraphics")`
/// (`Cube.exe 0x00681c70`, the `Seal` branch).
pub fn default_candidate_keys() -> Vec<u64> {
    vec![default_key(), 0, plasma_hash(MAGIC.as_bytes())]
}

/// Decrypt a name or seal (`Cube.exe 0x006880c0`). With `n = data.len()` and `k` the key's
/// eight little-endian bytes, byte `i` of the input lands at `t = (i + key_lo % n) % n`
/// (`key_lo` the low 32 bits) and becomes `in[i] - k[t & 7]`.
pub fn decrypt(data: &[u8], key: u64) -> Vec<u8> {
    let n = data.len();
    let mut out = vec![0u8; n];
    if n == 0 {
        return out;
    }
    let k = key.to_le_bytes();
    let shift = (key as u32 as usize) % n;
    for (i, &b) in data.iter().enumerate() {
        let t = (i + shift) % n;
        out[t] = b.wrapping_sub(k[t & 7]);
    }
    out
}

/// The inverse of [`decrypt`] (the original has no writer; this is what its tool did).
pub fn encrypt(plain: &[u8], key: u64) -> Vec<u8> {
    let n = plain.len();
    let mut out = vec![0u8; n];
    if n == 0 {
        return out;
    }
    let k = key.to_le_bytes();
    let shift = (key as u32 as usize) % n;
    for (i, o) in out.iter_mut().enumerate() {
        let t = (i + shift) % n;
        *o = plain[t].wrapping_add(k[t & 7]);
    }
    out
}

/// The key among `candidates` that decrypts `sealed` to "PlasmaGraphics", as the `Seal`
/// branch of `Cube.exe 0x00681c70` picks it (first match wins).
pub fn find_seal_key(sealed: &[u8], candidates: &[u64]) -> Option<u64> {
    candidates
        .iter()
        .copied()
        .find(|&k| decrypt(sealed, k) == MAGIC.as_bytes())
}

// ---------------------------------------------------------------------------------------
// Generic chunk tree
// ---------------------------------------------------------------------------------------

/// One chunk: a name and either raw payload bytes or nested chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub name: String,
    pub body: Body,
}

/// The payload of a [`Chunk`].
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A plain value, read with a single `istream::read` by the original.
    Leaf(Vec<u8>),
    /// Nested chunks up to the chunk end.
    Group(Vec<Chunk>),
}

impl Chunk {
    pub fn leaf(name: &str, bytes: Vec<u8>) -> Self {
        Self { name: name.to_string(), body: Body::Leaf(bytes) }
    }
    pub fn group(name: &str, children: Vec<Chunk>) -> Self {
        Self { name: name.to_string(), body: Body::Group(children) }
    }
}

/// Top-level element tags and their readers (`Cube.exe 0x00681c70` dispatch).
pub const ELEMENT_TAGS: [(&str, u32); 14] = [
    ("Texture", 0x0068_6820),
    ("TextShape", 0x0068_5b10),
    ("SmoothMeshShape", 0x0068_4ef0),
    ("GenericShape", 0x0068_3870),
    ("Transformation", 0x0068_6ff0),
    ("Display", 0x0068_3270),
    ("Widget", 0x0068_7440),
    ("Button", 0x0068_3070),
    ("ListWidget", 0x0068_3de0),
    ("Edit", 0x0068_3750),
    ("ScrollButton", 0x0068_4970),
    ("ScrollSlider", 0x0068_4c30),
    ("PopUpButton", 0x0068_4770),
    ("Node", 0x0068_3f00),
];

/// Fields read by an `Attribute<T>` reader (`0x006806b0`, `0x006808f0`, `0x00680b40`,
/// `0x00680d80`, `0x00680fd0`, or the inline one for `TextShape.string`): children are
/// `Attribute.frame` and `Attribute.sequence`.
const ATTRIBUTE_FIELDS: [&str; 29] = [
    "TextShape.string",
    "TextShape.color",
    "TextShape.strokeColor",
    "TextShape.extrusionColor",
    "Display.visibility",
    "Display.clipping",
    "Display.strokeColor",
    "Display.fillColor",
    "Display.blurRadius",
    "Transformation.translation",
    "Transformation.rotation",
    "Transformation.pivot",
    "Transformation.deformation",
    "SmoothMeshShape.texture",
    "SmoothMeshShape.strokeTexture",
    "SmoothMeshShape.textureTranslation",
    "SmoothMeshShape.textureRotation",
    "SmoothMeshShape.textureDeformation",
    "SmoothMeshShape.texturePivot",
    "SmoothMeshShape.textureOpacity",
    "SmoothMeshShape.textureBrightness",
    "SmoothMeshShape.textureContrast",
    "SmoothMeshShape.textureSaturation",
    "SmoothMeshShape.strokeTextureOpacity",
    "SmoothMeshShape.strokeTextureBrightness",
    "SmoothMeshShape.strokeTextureContrast",
    "SmoothMeshShape.strokeTextureSaturation",
    "SmoothMeshShape.strokeTextureStretch",
    "SmoothMeshShape.extrusionMatrix",
];

/// Fields read by an `ArrayAttribute<T>` reader (`0x006800d0`, `0x006803c0`,
/// `0x0067fde0`): children are `ArrayAttribute.size`, `ArrayAttribute.frame` and
/// `Attribute.sequence`.
const ARRAY_ATTRIBUTE_FIELDS: [&str; 7] = [
    "SmoothMeshShape.vertexPositions",
    "SmoothMeshShape.vertexTexCoords",
    "SmoothMeshShape.vertexColors",
    "SmoothMeshShape.strokeColors",
    "SmoothMeshShape.extrusionFrontColors",
    "SmoothMeshShape.extrusionBackColors",
    "SmoothMeshShape.strokeWidths",
];

/// Whether the original reader descends into a chunk of this name (enter, loop over tags
/// until the chunk end) rather than reading its payload as one value.
pub fn is_group(name: &str) -> bool {
    ELEMENT_TAGS.iter().any(|(t, _)| *t == name)
        || ATTRIBUTE_FIELDS.contains(&name)
        || ARRAY_ATTRIBUTE_FIELDS.contains(&name)
        || matches!(
            name,
            "Attribute.sequence" | "Attribute.sequence.key" | "TextShape.font"
        )
}

fn malformed(reason: impl Into<String>) -> Error {
    Error::Malformed { format: FORMAT, reason: reason.into() }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    names: HashMap<i32, String>,
    key: u64,
    candidates: &'a [u64],
}

impl Reader<'_> {
    fn i32(&mut self) -> Result<i32> {
        let b = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| malformed(format!("truncated at offset {}", self.pos)))?;
        self.pos += 4;
        Ok(i32::from_le_bytes(b.try_into().unwrap()))
    }

    fn bytes(&mut self, n: usize, end: usize) -> Result<&[u8]> {
        if n > end.saturating_sub(self.pos) {
            return Err(malformed(format!("{n} bytes at offset {} overrun the chunk", self.pos)));
        }
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }

    /// `enterChunk 0x00688180`: read the size, return the end offset.
    fn enter(&mut self, limit: usize) -> Result<usize> {
        let size = self.i32()?;
        let end = usize::try_from(size)
            .ok()
            .and_then(|s| self.pos.checked_add(s))
            .filter(|&e| e <= limit)
            .ok_or_else(|| malformed(format!("chunk size {size} at offset {} overruns", self.pos - 4)))?;
        Ok(end)
    }

    /// `readTag 0x00688220`: a name id, preceded by at most one name definition.
    fn tag(&mut self, limit: usize) -> Result<String> {
        let mut id = self.i32()?;
        if id == 0 {
            let end = self.enter(limit)?;
            let def = self.i32()?;
            let len = self.i32()?;
            let len = usize::try_from(len).map_err(|_| malformed("negative name length"))?;
            let raw = self.bytes(len, end)?.to_vec();
            // Before the seal the key is 0 and the name is read as plain text (0x00688510).
            let plain = if self.key == 0 { raw } else { decrypt(&raw, self.key) };
            if self.pos != end {
                return Err(malformed(format!("name definition {def} does not fill its chunk")));
            }
            let name = String::from_utf8(plain)
                .map_err(|_| malformed(format!("name {def} is not text (wrong seal key?)")))?;
            self.names.insert(def, name);
            id = self.i32()?;
        }
        self.names
            .get(&id)
            .cloned()
            .ok_or_else(|| malformed(format!("undefined name id {id} at offset {}", self.pos - 4)))
    }

    fn sequence(&mut self, end: usize) -> Result<Vec<Chunk>> {
        let mut out = Vec::new();
        while self.pos < end {
            let name = self.tag(end)?;
            let cend = self.enter(end)?;
            let body = if is_group(&name) {
                Body::Group(self.sequence(cend)?)
            } else {
                let b = self.data[self.pos..cend].to_vec();
                self.pos = cend;
                if name == "Seal" {
                    self.key = seal_key(&b, self.candidates)?;
                }
                Body::Leaf(b)
            };
            out.push(Chunk { name, body });
        }
        Ok(out)
    }
}

/// The key a `Seal` payload (`[i32 len][len bytes]`) selects among `candidates`.
fn seal_key(payload: &[u8], candidates: &[u64]) -> Result<u64> {
    let sealed = Str::decode(payload).ok_or_else(|| malformed("Seal payload is not a string"))?;
    find_seal_key(&sealed.0, candidates).ok_or_else(|| malformed("no key opens the Seal"))
}

/// Parse the generic chunk tree, trying `candidates` on the seal. Returns the chunks and
/// the key the seal selected (0 when there is no seal).
pub fn parse_chunks(data: &[u8], candidates: &[u64]) -> Result<(Vec<Chunk>, u64)> {
    let mut r = Reader { data, pos: 0, names: HashMap::new(), key: 0, candidates };
    let chunks = r.sequence(data.len())?;
    Ok((chunks, r.key))
}

struct Writer {
    out: Vec<u8>,
    ids: HashMap<String, i32>,
    key: u64,
    seal_key: u64,
}

impl Writer {
    fn tag(&mut self, name: &str) {
        let id = match self.ids.get(name) {
            Some(&id) => id,
            None => {
                let id = self.ids.len() as i32 + 1;
                self.ids.insert(name.to_string(), id);
                let raw = if self.key == 0 {
                    name.as_bytes().to_vec()
                } else {
                    encrypt(name.as_bytes(), self.key)
                };
                self.out.extend_from_slice(&0i32.to_le_bytes());
                self.out.extend_from_slice(&(8 + raw.len() as i32).to_le_bytes());
                self.out.extend_from_slice(&id.to_le_bytes());
                self.out.extend_from_slice(&(raw.len() as i32).to_le_bytes());
                self.out.extend_from_slice(&raw);
                id
            }
        };
        self.out.extend_from_slice(&id.to_le_bytes());
    }

    fn sequence(&mut self, chunks: &[Chunk]) {
        for c in chunks {
            self.tag(&c.name);
            let at = self.out.len();
            self.out.extend_from_slice(&[0; 4]);
            match &c.body {
                Body::Leaf(b) => {
                    self.out.extend_from_slice(b);
                    if c.name == "Seal" {
                        self.key = self.seal_key;
                    }
                }
                Body::Group(g) => self.sequence(g),
            }
            let size = (self.out.len() - at - 4) as i32;
            self.out[at..at + 4].copy_from_slice(&size.to_le_bytes());
        }
    }
}

/// Serialise a chunk tree. Names are numbered from 1 in order of first use and defined
/// right before it; definitions after the `Seal` chunk are encrypted with `key`.
pub fn encode_chunks(chunks: &[Chunk], key: u64) -> Vec<u8> {
    let mut w = Writer { out: Vec::new(), ids: HashMap::new(), key: 0, seal_key: key };
    w.sequence(chunks);
    w.out
}

// ---------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------

/// A value stored in a chunk: decoded from and encoded to a [`Body`] exactly.
pub trait FieldValue: Sized {
    fn from_body(body: &Body) -> Option<Self>;
    fn to_body(&self) -> Body;
}

/// A fixed-size element of an [`ArrayAttribute`] frame, also usable as a plain field.
pub trait Elem: Copy {
    const SIZE: usize;
    fn read(b: &[u8]) -> Self;
    fn write(&self, out: &mut Vec<u8>);
}

impl Elem for i32 {
    const SIZE: usize = 4;
    fn read(b: &[u8]) -> Self {
        i32::from_le_bytes(b[..4].try_into().unwrap())
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
}

impl Elem for f32 {
    const SIZE: usize = 4;
    fn read(b: &[u8]) -> Self {
        // from_bits keeps every bit, NaN payloads included.
        f32::from_bits(u32::from_le_bytes(b[..4].try_into().unwrap()))
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_bits().to_le_bytes());
    }
}

impl<const N: usize> Elem for [f32; N] {
    const SIZE: usize = 4 * N;
    fn read(b: &[u8]) -> Self {
        std::array::from_fn(|i| f32::read(&b[4 * i..]))
    }
    fn write(&self, out: &mut Vec<u8>) {
        for v in self {
            v.write(out);
        }
    }
}

macro_rules! elem_field {
    ($($t:ty),*) => {$(
        impl FieldValue for $t {
            fn from_body(body: &Body) -> Option<Self> {
                match body {
                    Body::Leaf(b) if b.len() == <$t as Elem>::SIZE => Some(<$t as Elem>::read(b)),
                    _ => None,
                }
            }
            fn to_body(&self) -> Body {
                let mut out = Vec::with_capacity(<$t as Elem>::SIZE);
                self.write(&mut out);
                Body::Leaf(out)
            }
        }
    )*};
}
elem_field!(i32, f32, Vec2, Vec3, Color, Mat4);

/// Two floats (positions, sizes, texture translation and pivot).
pub type Vec2 = [f32; 2];
/// Three floats (the `rotation` attributes, 12 bytes each).
pub type Vec3 = [f32; 3];
/// An RGBA colour as four floats (`plasma::Color`, 16 bytes).
pub type Color = [f32; 4];
/// A 4x4 float matrix, row-major as stored (64 bytes). The `deformation` and
/// `extrusionMatrix` frames often hold uninitialised memory past the used 2x2 part; it is
/// kept bit for bit.
pub type Mat4 = [f32; 16];

/// A narrow string, `[i32 len][len bytes]` (`Cube.exe 0x00688510`, which then cuts the
/// value at the first NUL; the raw bytes are kept).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Str(pub Vec<u8>);

/// A wide string, `[i32 len][len UTF-16 units]` (`Cube.exe 0x00688610`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WStr(pub Vec<u16>);

/// A length-prefixed byte blob, `[i32 len][len bytes]` (texture pixels).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Blob(pub Vec<u8>);

/// A count-prefixed `i32` array, `[i32 n][n x i32]` (`SmoothMeshShape.face`,
/// `vertexFlags`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntList(pub Vec<i32>);

/// A count-prefixed `f32` array, `[i32 n][n x f32]` (`SmoothMeshShape.vertexParameters`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FloatList(pub Vec<f32>);

/// Two narrow strings back to back (`Node.variable`: read as `a`, then `b`, and stored with
/// `b` as the first argument of `0x0064bec0`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StrPair(pub Str, pub Str);

impl Str {
    fn decode(b: &[u8]) -> Option<Self> {
        let (s, rest) = Self::decode_prefix(b)?;
        rest.is_empty().then_some(s)
    }
    fn decode_prefix(b: &[u8]) -> Option<(Self, &[u8])> {
        let n = usize::try_from(i32::read(b.get(..4)?)).ok()?;
        let body = b.get(4..4 + n)?;
        Some((Str(body.to_vec()), &b[4 + n..]))
    }
    fn write(&self, out: &mut Vec<u8>) {
        (self.0.len() as i32).write(out);
        out.extend_from_slice(&self.0);
    }
    /// The value as the reader sees it: up to the first NUL, bytes as Latin-1.
    pub fn text(&self) -> String {
        self.0.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
    }
}

impl WStr {
    /// The value as text (unpaired surrogates become U+FFFD).
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

impl FieldValue for Str {
    fn from_body(body: &Body) -> Option<Self> {
        match body {
            Body::Leaf(b) => Str::decode(b),
            Body::Group(_) => None,
        }
    }
    fn to_body(&self) -> Body {
        let mut out = Vec::new();
        self.write(&mut out);
        Body::Leaf(out)
    }
}

impl FieldValue for Blob {
    fn from_body(body: &Body) -> Option<Self> {
        Str::from_body(body).map(|s| Blob(s.0))
    }
    fn to_body(&self) -> Body {
        Str(self.0.clone()).to_body()
    }
}

impl FieldValue for StrPair {
    fn from_body(body: &Body) -> Option<Self> {
        let Body::Leaf(b) = body else { return None };
        let (a, rest) = Str::decode_prefix(b)?;
        let (c, rest) = Str::decode_prefix(rest)?;
        rest.is_empty().then_some(StrPair(a, c))
    }
    fn to_body(&self) -> Body {
        let mut out = Vec::new();
        self.0.write(&mut out);
        self.1.write(&mut out);
        Body::Leaf(out)
    }
}

impl FieldValue for WStr {
    fn from_body(body: &Body) -> Option<Self> {
        let Body::Leaf(b) = body else { return None };
        let n = usize::try_from(i32::read(b.get(..4)?)).ok()?;
        if b.len() != 4 + 2 * n {
            return None;
        }
        Some(WStr(b[4..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()))
    }
    fn to_body(&self) -> Body {
        let mut out = Vec::with_capacity(4 + 2 * self.0.len());
        (self.0.len() as i32).write(&mut out);
        for u in &self.0 {
            out.extend_from_slice(&u.to_le_bytes());
        }
        Body::Leaf(out)
    }
}

fn counted<T: Elem>(body: &Body) -> Option<Vec<T>> {
    let Body::Leaf(b) = body else { return None };
    let n = usize::try_from(i32::read(b.get(..4)?)).ok()?;
    if b.len() != 4 + n * T::SIZE {
        return None;
    }
    Some(b[4..].chunks_exact(T::SIZE).map(T::read).collect())
}

fn write_counted<T: Elem>(v: &[T]) -> Body {
    let mut out = Vec::with_capacity(4 + v.len() * T::SIZE);
    (v.len() as i32).write(&mut out);
    for e in v {
        e.write(&mut out);
    }
    Body::Leaf(out)
}

impl FieldValue for IntList {
    fn from_body(body: &Body) -> Option<Self> {
        counted(body).map(IntList)
    }
    fn to_body(&self) -> Body {
        write_counted(&self.0)
    }
}

impl FieldValue for FloatList {
    fn from_body(body: &Body) -> Option<Self> {
        counted(body).map(FloatList)
    }
    fn to_body(&self) -> Body {
        write_counted(&self.0)
    }
}

/// A list of typed fields read from a group chunk (an element, a sequence, a key, a font).
pub trait FieldSet: Sized {
    fn from_chunk(chunk: &Chunk) -> Self;
    fn to_chunk(&self) -> Chunk;
}

impl<F: FieldSet> FieldValue for Vec<F> {
    fn from_body(body: &Body) -> Option<Self> {
        match body {
            Body::Group(g) => Some(g.iter().map(F::from_chunk).collect()),
            Body::Leaf(_) => None,
        }
    }
    fn to_body(&self) -> Body {
        Body::Group(self.iter().map(F::to_chunk).collect())
    }
}

macro_rules! fields {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $var:ident = $tag:literal : $ty:ty, )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq)]
        pub enum $name {
            $( $(#[$vmeta])* #[doc = concat!("`", $tag, "`")] $var($ty), )*
            /// A chunk the original reader skips, or whose payload does not decode as
            /// the expected type; kept verbatim.
            Other(Chunk),
        }

        impl FieldSet for $name {
            fn from_chunk(chunk: &Chunk) -> Self {
                match chunk.name.as_str() {
                    $( $tag => {
                        if let Some(v) = <$ty as FieldValue>::from_body(&chunk.body) {
                            return Self::$var(v);
                        }
                    } )*
                    _ => {}
                }
                Self::Other(chunk.clone())
            }
            fn to_chunk(&self) -> Chunk {
                match self {
                    $( Self::$var(v) => Chunk { name: $tag.to_string(), body: v.to_body() }, )*
                    Self::Other(c) => c.clone(),
                }
            }
        }

        impl $name {
            /// The chunk name this field is stored under.
            pub fn tag(&self) -> &str {
                match self {
                    $( Self::$var(_) => $tag, )*
                    Self::Other(c) => &c.name,
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------------------
// Keyframed attributes
// ---------------------------------------------------------------------------------------

/// `plasma::Attribute<T>`: a keyframed value. Each `Attribute.frame` chunk appends one
/// frame (the reader grows the frame array through the attribute's vtable slots 1 and 2);
/// `Attribute.sequence` chunks add named animations over those frames
/// (`Cube.exe 0x00682a80`).
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute<T> {
    pub items: Vec<AttributeItem<T>>,
}

/// One child of an [`Attribute`].
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeItem<T> {
    /// `Attribute.frame`: the value of the next frame.
    Frame(T),
    /// `Attribute.sequence`.
    Sequence(Vec<SequenceField>),
    Other(Chunk),
}

impl<T> Attribute<T> {
    /// The frame values in order.
    pub fn frames(&self) -> impl Iterator<Item = &T> {
        self.items.iter().filter_map(|i| match i {
            AttributeItem::Frame(v) => Some(v),
            _ => None,
        })
    }
    /// The named sequences in order.
    pub fn sequences(&self) -> impl Iterator<Item = &Vec<SequenceField>> {
        self.items.iter().filter_map(|i| match i {
            AttributeItem::Sequence(s) => Some(s),
            _ => None,
        })
    }
}

impl<T: FieldValue> FieldValue for Attribute<T> {
    fn from_body(body: &Body) -> Option<Self> {
        let Body::Group(g) = body else { return None };
        let items = g
            .iter()
            .map(|c| match c.name.as_str() {
                "Attribute.frame" => T::from_body(&c.body)
                    .map(AttributeItem::Frame)
                    .unwrap_or_else(|| AttributeItem::Other(c.clone())),
                "Attribute.sequence" => Vec::<SequenceField>::from_body(&c.body)
                    .map(AttributeItem::Sequence)
                    .unwrap_or_else(|| AttributeItem::Other(c.clone())),
                _ => AttributeItem::Other(c.clone()),
            })
            .collect();
        Some(Self { items })
    }
    fn to_body(&self) -> Body {
        Body::Group(
            self.items
                .iter()
                .map(|i| match i {
                    AttributeItem::Frame(v) => Chunk { name: "Attribute.frame".into(), body: v.to_body() },
                    AttributeItem::Sequence(s) => Chunk { name: "Attribute.sequence".into(), body: s.to_body() },
                    AttributeItem::Other(c) => c.clone(),
                })
                .collect(),
        )
    }
}

/// `plasma::ArrayAttribute<T>`: a keyframed array (mesh vertices, colours, stroke widths).
/// `ArrayAttribute.size` resizes every frame; each `ArrayAttribute.frame` holds `size`
/// elements (`Cube.exe 0x006800d0`, `0x006803c0`, `0x0067fde0`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayAttribute<T> {
    pub items: Vec<ArrayAttributeItem<T>>,
}

/// One child of an [`ArrayAttribute`].
#[derive(Debug, Clone, PartialEq)]
pub enum ArrayAttributeItem<T> {
    /// `ArrayAttribute.size`.
    Size(i32),
    /// `ArrayAttribute.frame`.
    Frame(Vec<T>),
    /// `Attribute.sequence`.
    Sequence(Vec<SequenceField>),
    Other(Chunk),
}

impl<T> ArrayAttribute<T> {
    /// The frames in order.
    pub fn frames(&self) -> impl Iterator<Item = &Vec<T>> {
        self.items.iter().filter_map(|i| match i {
            ArrayAttributeItem::Frame(v) => Some(v),
            _ => None,
        })
    }
}

impl<T: Elem> FieldValue for ArrayAttribute<T> {
    fn from_body(body: &Body) -> Option<Self> {
        let Body::Group(g) = body else { return None };
        let items = g
            .iter()
            .map(|c| {
                let typed = match (c.name.as_str(), &c.body) {
                    ("ArrayAttribute.size", b) => i32::from_body(b).map(ArrayAttributeItem::Size),
                    ("ArrayAttribute.frame", Body::Leaf(b)) if b.len() % T::SIZE == 0 => Some(
                        ArrayAttributeItem::Frame(b.chunks_exact(T::SIZE).map(T::read).collect()),
                    ),
                    ("Attribute.sequence", b) => {
                        Vec::<SequenceField>::from_body(b).map(ArrayAttributeItem::Sequence)
                    }
                    _ => None,
                };
                typed.unwrap_or_else(|| ArrayAttributeItem::Other(c.clone()))
            })
            .collect();
        Some(Self { items })
    }
    fn to_body(&self) -> Body {
        Body::Group(
            self.items
                .iter()
                .map(|i| match i {
                    ArrayAttributeItem::Size(n) => Chunk { name: "ArrayAttribute.size".into(), body: n.to_body() },
                    ArrayAttributeItem::Frame(v) => {
                        let mut out = Vec::with_capacity(v.len() * T::SIZE);
                        for e in v {
                            e.write(&mut out);
                        }
                        Chunk::leaf("ArrayAttribute.frame", out)
                    }
                    ArrayAttributeItem::Sequence(s) => Chunk { name: "Attribute.sequence".into(), body: s.to_body() },
                    ArrayAttributeItem::Other(c) => c.clone(),
                })
                .collect(),
        )
    }
}

fields! {
    /// A field of `Attribute.sequence` (`Cube.exe 0x00682a80`): a named animation.
    pub enum SequenceField {
        Name = "Attribute.sequence.name": Str,
        WName = "Attribute.sequence.wname": WStr,
        /// One key; the reader calls `0x006779e0(frame, time, smoothness)` when the key
        /// chunk ends.
        Key = "Attribute.sequence.key": Vec<KeyField>,
    }
}

fields! {
    /// A field of `Attribute.sequence.key`.
    pub enum KeyField {
        /// Index into the attribute's frames.
        Frame = "Attribute.sequence.key.frame": i32,
        /// Time of the key in milliseconds.
        Time = "Attribute.sequence.key.time": i32,
        /// Interpolation smoothness (f32, confirmed: `Attribute::evaluate` 0x00662300 passes it
        /// as the float `a`/`b` of the slot-6 interpolation; all shipped values are 0).
        Smoothness = "Attribute.sequence.key.smoothness": f32,
    }
}

// ---------------------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------------------

fields! {
    /// A field of a `Texture` element (`plasma::Texture`, reader `Cube.exe 0x00686820`).
    /// The texture is created with (width, height, pixels, format, wname) when the chunk
    /// ends, unless `id` is -1; `id` is what `SmoothMeshShape.texture` frames refer to.
    pub enum TextureField {
        Name = "Texture.name": Str,
        WName = "Texture.wname": WStr,
        Id = "Texture.id": i32,
        PixelFormat = "Texture.format.pixelFormat": i32,
        MinFilter = "Texture.format.minFilter": i32,
        MaxFilter = "Texture.format.maxFilter": i32,
        HorizontalWrap = "Texture.format.horizontalWrap": i32,
        VerticalWrap = "Texture.format.verticalWrap": i32,
        Width = "Texture.width": i32,
        Height = "Texture.height": i32,
        /// Raw pixels.
        Pixels = "Texture.pixels": Blob,
        /// zlib stream of the pixels (inflated by `0x005842d0`/`zlibInflateVec`).
        CompressedPixels = "Texture.compressedPixels": Blob,
    }
}

fields! {
    /// A field of a `TextShape` element (`plasma::TextShape`, reader `Cube.exe 0x00685b10`).
    pub enum TextShapeField {
        Name = "TextShape.name": Str,
        WName = "TextShape.wname": WStr,
        /// The text, keyframed.
        String = "TextShape.string": Attribute<WStr>,
        Color = "TextShape.color": Attribute<Color>,
        StrokeColor = "TextShape.strokeColor": Attribute<Color>,
        ExtrusionColor = "TextShape.extrusionColor": Attribute<Color>,
        Flags = "TextShape.flags": i32,
        PixelSize = "TextShape.pixelSize": f32,
        StrokeRadius = "TextShape.strokeRadius": f32,
        Spacing = "TextShape.spacing": f32,
        LineSpacing = "TextShape.lineSpacing": f32,
        WrapWidth = "TextShape.wrapWidth": f32,
        /// Older form of `pixelSize`, an integer.
        FontSize = "TextShape.fontSize": i32,
        /// Older form of `strokeRadius`, an integer halved on load.
        StrokeWidth = "TextShape.strokeWidth": i32,
        FontName = "TextShape.fontName": Str,
        WFontName = "TextShape.wfontName": WStr,
        Font = "TextShape.font": Vec<FontField>,
    }
}

fields! {
    /// A field of the older `TextShape.font` block.
    pub enum FontField {
        FileName = "TextShape.font.fileName": Str,
        WFileName = "TextShape.font.wfileName": WStr,
        PixelSize = "TextShape.font.pixelSize": i32,
        GlowRadius = "TextShape.font.glowRadius": i32,
        Size = "TextShape.font.size": f32,
        StrokeWidth = "TextShape.font.strokeWidth": f32,
        StrokeGlow = "TextShape.font.strokeGlow": i32,
        PixelFont = "TextShape.font.pixelFont": i32,
    }
}

fields! {
    /// A field of a `SmoothMeshShape` element (`plasma::SmoothMeshShape`, reader
    /// `Cube.exe 0x00684ef0`): a subdivided vector shape with fill, stroke and extrusion.
    pub enum SmoothMeshShapeField {
        Name = "SmoothMeshShape.name": Str,
        WName = "SmoothMeshShape.wname": WStr,
        /// One contour as vertex indices; repeated per contour (`0x00642610`).
        Face = "SmoothMeshShape.face": IntList,
        VertexFlags = "SmoothMeshShape.vertexFlags": IntList,
        VertexParameters = "SmoothMeshShape.vertexParameters": FloatList,
        VertexPositions = "SmoothMeshShape.vertexPositions": ArrayAttribute<Vec2>,
        VertexTexCoords = "SmoothMeshShape.vertexTexCoords": ArrayAttribute<Vec2>,
        VertexColors = "SmoothMeshShape.vertexColors": ArrayAttribute<Color>,
        StrokeColors = "SmoothMeshShape.strokeColors": ArrayAttribute<Color>,
        ExtrusionFrontColors = "SmoothMeshShape.extrusionFrontColors": ArrayAttribute<Color>,
        ExtrusionBackColors = "SmoothMeshShape.extrusionBackColors": ArrayAttribute<Color>,
        StrokeWidths = "SmoothMeshShape.strokeWidths": ArrayAttribute<f32>,
        /// `Texture.id` per frame, -1 for none (remapped to loaded textures on read).
        Texture = "SmoothMeshShape.texture": Attribute<i32>,
        StrokeTexture = "SmoothMeshShape.strokeTexture": Attribute<i32>,
        TextureTranslation = "SmoothMeshShape.textureTranslation": Attribute<Vec2>,
        TextureRotation = "SmoothMeshShape.textureRotation": Attribute<Vec3>,
        TextureDeformation = "SmoothMeshShape.textureDeformation": Attribute<Mat4>,
        TexturePivot = "SmoothMeshShape.texturePivot": Attribute<Vec2>,
        TextureOpacity = "SmoothMeshShape.textureOpacity": Attribute<f32>,
        TextureBrightness = "SmoothMeshShape.textureBrightness": Attribute<f32>,
        TextureContrast = "SmoothMeshShape.textureContrast": Attribute<f32>,
        TextureSaturation = "SmoothMeshShape.textureSaturation": Attribute<f32>,
        StrokeTextureOpacity = "SmoothMeshShape.strokeTextureOpacity": Attribute<f32>,
        StrokeTextureBrightness = "SmoothMeshShape.strokeTextureBrightness": Attribute<f32>,
        StrokeTextureContrast = "SmoothMeshShape.strokeTextureContrast": Attribute<f32>,
        StrokeTextureSaturation = "SmoothMeshShape.strokeTextureSaturation": Attribute<f32>,
        StrokeTextureStretch = "SmoothMeshShape.strokeTextureStretch": Attribute<f32>,
        ExtrusionMatrix = "SmoothMeshShape.extrusionMatrix": Attribute<Mat4>,
        Subdivisions = "SmoothMeshShape.subdivisions": i32,
        SmoothWeight = "SmoothMeshShape.smoothWeight": f32,
        Flags = "SmoothMeshShape.flags": i32,
        StrokeJointType = "SmoothMeshShape.strokeJointType": i32,
        StrokeCapType = "SmoothMeshShape.strokeCapType": i32,
        StrokeAlignment = "SmoothMeshShape.strokeAlignment": i32,
        StrokePattern = "SmoothMeshShape.strokePattern": i32,
        StrokeDash = "SmoothMeshShape.strokeDash": f32,
        StrokeGap = "SmoothMeshShape.strokeGap": f32,
    }
}

fields! {
    /// A field of a `GenericShape` element (`plasma::GenericShape`, reader
    /// `Cube.exe 0x00683870`): a shape loaded from another file. Not used by the shipped
    /// files.
    pub enum GenericShapeField {
        Name = "GenericShape.name": Str,
        WName = "GenericShape.wname": WStr,
        Source = "GenericShape.source": Str,
        WSource = "GenericShape.wsource": WStr,
        Position = "GenericShape.position": Vec2,
        Size = "GenericShape.size": Vec2,
    }
}

fields! {
    /// A field of a `Transformation` element (`plasma::Transformation`, reader
    /// `Cube.exe 0x00686ff0`).
    pub enum TransformationField {
        Name = "Transformation.name": Str,
        WName = "Transformation.wname": WStr,
        Translation = "Transformation.translation": Attribute<Vec2>,
        Rotation = "Transformation.rotation": Attribute<Vec3>,
        Pivot = "Transformation.pivot": Attribute<Vec2>,
        Deformation = "Transformation.deformation": Attribute<Mat4>,
    }
}

fields! {
    /// A field of a `Display` element (`plasma::Display`, reader `Cube.exe 0x00683270`).
    pub enum DisplayField {
        Name = "Display.name": Str,
        WName = "Display.wname": WStr,
        Visibility = "Display.visibility": Attribute<i32>,
        Clipping = "Display.clipping": Attribute<i32>,
        StrokeColor = "Display.strokeColor": Attribute<Color>,
        FillColor = "Display.fillColor": Attribute<Color>,
        BlurRadius = "Display.blurRadius": Attribute<f32>,
        Flags = "Display.flags": i32,
    }
}

fields! {
    /// A field of a widget element (`plasma::Widget` and subclasses). The common fields
    /// are read by `Cube.exe 0x00687560`; `Button.type` by the `Button`, `ScrollButton`,
    /// `ScrollSlider` and `PopUpButton` readers, the directions by their own readers.
    pub enum WidgetField {
        Name = "Widget.name": Str,
        WName = "Widget.wname": WStr,
        Caption = "Widget.caption": WStr,
        InnerBindPos = "Widget.innerBindPos": Vec2,
        InnerBindSize = "Widget.innerBindSize": Vec2,
        BindPos = "Widget.bindPos": Vec2,
        BindSize = "Widget.bindSize": Vec2,
        FramePos = "Widget.framePos": Vec2,
        FrameSize = "Widget.frameSize": Vec2,
        BindMatrix = "Widget.bindMatrix": Mat4,
        HorizontalAlignment = "Widget.horizontalAlignment": i32,
        VerticalAlignment = "Widget.verticalAlignment": i32,
        Flags = "Widget.flags": i32,
        ButtonType = "Button.type": i32,
        ScrollButtonDirection = "ScrollButton.direction": i32,
        ScrollSliderDirection = "ScrollSlider.direction": i32,
    }
}

fields! {
    /// A field of a `Node` element (`plasma::Node`, reader `Cube.exe 0x00683f00`): the
    /// scene graph. References are indices, see the module documentation.
    pub enum NodeField {
        Name = "Node.name": Str,
        WName = "Node.wname": WStr,
        Shape = "Node.shape": i32,
        Transformation = "Node.transformation": i32,
        Display = "Node.display": i32,
        Widget = "Node.widget": i32,
        /// One child node index; repeated.
        Child = "Node.child": i32,
        Flags = "Node.flags": i32,
        Variable = "Node.variable": StrPair,
    }
}

/// Which widget class a widget element instantiates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Widget,
    Button,
    ListWidget,
    Edit,
    ScrollButton,
    ScrollSlider,
    PopUpButton,
}

impl WidgetKind {
    const ALL: [(WidgetKind, &'static str); 7] = [
        (WidgetKind::Widget, "Widget"),
        (WidgetKind::Button, "Button"),
        (WidgetKind::ListWidget, "ListWidget"),
        (WidgetKind::Edit, "Edit"),
        (WidgetKind::ScrollButton, "ScrollButton"),
        (WidgetKind::ScrollSlider, "ScrollSlider"),
        (WidgetKind::PopUpButton, "PopUpButton"),
    ];
    /// The top-level tag.
    pub fn tag(self) -> &'static str {
        Self::ALL.iter().find(|(k, _)| *k == self).unwrap().1
    }
    fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, t)| *t == tag).map(|(k, _)| *k)
    }
}

/// One top-level chunk of a `.plx` document.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `PlasmaGraphics`: the format version.
    Version(i32),
    /// `Seal`: "PlasmaGraphics" encrypted with the name key (the raw sealed bytes).
    Seal(Str),
    /// `pageWidth` (written to the engine's page record +0).
    PageWidth(f32),
    /// `pageHeight` (+4).
    PageHeight(f32),
    /// `dpi` (+8).
    Dpi(f32),
    /// `unit` (+0xc).
    Unit(i32),
    /// `pageColor` (+0x10).
    PageColor(Color),
    Texture(Vec<TextureField>),
    TextShape(Vec<TextShapeField>),
    SmoothMeshShape(Vec<SmoothMeshShapeField>),
    GenericShape(Vec<GenericShapeField>),
    Transformation(Vec<TransformationField>),
    Display(Vec<DisplayField>),
    /// A widget element of the given class.
    Widget(WidgetKind, Vec<WidgetField>),
    Node(Vec<NodeField>),
    /// A chunk the reader skips, or a known one whose payload does not decode.
    Other(Chunk),
}

impl Item {
    fn from_chunk(c: &Chunk) -> Self {
        fn leaf<T: FieldValue>(c: &Chunk, f: impl FnOnce(T) -> Item) -> Item {
            T::from_body(&c.body).map(f).unwrap_or_else(|| Item::Other(c.clone()))
        }
        match c.name.as_str() {
            "PlasmaGraphics" => leaf(c, Item::Version),
            "Seal" => leaf(c, Item::Seal),
            "pageWidth" => leaf(c, Item::PageWidth),
            "pageHeight" => leaf(c, Item::PageHeight),
            "dpi" => leaf(c, Item::Dpi),
            "unit" => leaf(c, Item::Unit),
            "pageColor" => leaf(c, Item::PageColor),
            "Texture" => leaf(c, Item::Texture),
            "TextShape" => leaf(c, Item::TextShape),
            "SmoothMeshShape" => leaf(c, Item::SmoothMeshShape),
            "GenericShape" => leaf(c, Item::GenericShape),
            "Transformation" => leaf(c, Item::Transformation),
            "Display" => leaf(c, Item::Display),
            "Node" => leaf(c, Item::Node),
            t => match WidgetKind::from_tag(t) {
                Some(k) => leaf(c, |f| Item::Widget(k, f)),
                None => Item::Other(c.clone()),
            },
        }
    }

    fn to_chunk(&self) -> Chunk {
        fn mk<T: FieldValue>(name: &str, v: &T) -> Chunk {
            Chunk { name: name.to_string(), body: v.to_body() }
        }
        match self {
            Item::Version(v) => mk("PlasmaGraphics", v),
            Item::Seal(v) => mk("Seal", v),
            Item::PageWidth(v) => mk("pageWidth", v),
            Item::PageHeight(v) => mk("pageHeight", v),
            Item::Dpi(v) => mk("dpi", v),
            Item::Unit(v) => mk("unit", v),
            Item::PageColor(v) => mk("pageColor", v),
            Item::Texture(v) => mk("Texture", v),
            Item::TextShape(v) => mk("TextShape", v),
            Item::SmoothMeshShape(v) => mk("SmoothMeshShape", v),
            Item::GenericShape(v) => mk("GenericShape", v),
            Item::Transformation(v) => mk("Transformation", v),
            Item::Display(v) => mk("Display", v),
            Item::Widget(k, v) => mk(k.tag(), v),
            Item::Node(v) => mk("Node", v),
            Item::Other(c) => c.clone(),
        }
    }
}

/// A parsed `.plx` document (what `PlxReader::read`, `Cube.exe 0x00681c70`, loads).
#[derive(Debug, Clone, PartialEq)]
pub struct PlxDocument {
    /// The key that opened the seal and encrypts every name defined after it.
    pub key: u64,
    /// Top-level chunks in file order.
    pub items: Vec<Item>,
}

/// Parse a `.plx` file with the keys the client registers ([`default_candidate_keys`]).
pub fn parse(data: &[u8]) -> Result<PlxDocument> {
    parse_with_keys(data, &default_candidate_keys())
}

/// Parse with an explicit list of seal keys, tried in order.
pub fn parse_with_keys(data: &[u8], candidates: &[u64]) -> Result<PlxDocument> {
    let (chunks, key) = parse_chunks(data, candidates)?;
    let items: Vec<Item> = chunks.iter().map(Item::from_chunk).collect();
    // 0x00681c70: "PlasmaGraphics" with a version above 1 throws.
    for it in &items {
        if let Item::Version(v) = it
            && *v > MAX_VERSION
        {
            return Err(malformed(format!("unsupported version {v}")));
        }
    }
    Ok(PlxDocument { key, items })
}

/// Serialise a document; `encode(&parse(x)?) == x` for the shipped files.
pub fn encode(doc: &PlxDocument) -> Vec<u8> {
    encode_chunks(&doc.chunks(), doc.key)
}

impl PlxDocument {
    /// The document as a generic chunk tree.
    pub fn chunks(&self) -> Vec<Chunk> {
        self.items.iter().map(Item::to_chunk).collect()
    }

    /// Every `Node` element in order (index = node index; the first is read into the load
    /// target and only its children are kept).
    pub fn nodes(&self) -> impl Iterator<Item = &Vec<NodeField>> {
        self.items.iter().filter_map(|i| match i {
            Item::Node(n) => Some(n),
            _ => None,
        })
    }

    /// Every shape in `Node.shape` index order.
    pub fn shapes(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| {
            matches!(i, Item::TextShape(_) | Item::SmoothMeshShape(_) | Item::GenericShape(_))
        })
    }

    /// A text rendering of the chunk tree: one line per chunk with its size, and a short
    /// value preview for leaves (the `cwtool plx dump` output).
    pub fn dump(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "key {:#018x}", self.key);
        dump_chunks(&mut out, &self.chunks(), 0);
        out
    }
}

fn dump_chunks(out: &mut String, chunks: &[Chunk], depth: usize) {
    let pad = "  ".repeat(depth);
    let mut i = 0;
    while i < chunks.len() {
        let c = &chunks[i];
        // Collapse runs of identical-named leaves (frames, faces, children) to one line.
        let mut run = 1;
        if matches!(c.body, Body::Leaf(_)) {
            while i + run < chunks.len()
                && chunks[i + run].name == c.name
                && matches!(chunks[i + run].body, Body::Leaf(_))
            {
                run += 1;
            }
        }
        match &c.body {
            Body::Group(g) => {
                let _ = writeln!(out, "{pad}{} {{{} chunks}}", c.name, g.len());
                dump_chunks(out, g, depth + 1);
            }
            Body::Leaf(b) if run > 1 => {
                let total: usize = chunks[i..i + run]
                    .iter()
                    .map(|c| match &c.body {
                        Body::Leaf(b) => b.len(),
                        Body::Group(_) => 0,
                    })
                    .sum();
                let _ = writeln!(out, "{pad}{} x{run} [{total} bytes] first: {}", c.name, preview(&c.name, b));
            }
            Body::Leaf(b) => {
                let _ = writeln!(out, "{pad}{} [{}] {}", c.name, b.len(), preview(&c.name, b));
            }
        }
        i += run;
    }
}

fn preview(name: &str, b: &[u8]) -> String {
    let wide = [".wname", ".wfontName", ".wsource", ".wfileName", "Widget.caption"];
    if wide.iter().any(|w| name.ends_with(w))
        && let Some(s) = WStr::from_body(&Body::Leaf(b.to_vec()))
    {
        return format!("{:?}", s.text());
    }
    // A text frame (`TextShape.string`) or other wide string.
    if b.len() > 4
        && let Some(s) = WStr::from_body(&Body::Leaf(b.to_vec()))
        && !s.0.is_empty()
        && s.0.iter().all(|&u| (0x20..0x7f).contains(&u))
    {
        return format!("{:?}", s.text());
    }
    // Four-byte values: small magnitudes read as integers, the rest as floats.
    let word = |w: &[u8]| {
        let i = i32::read(w);
        if (-0x0010_0000..0x0010_0000).contains(&i) {
            format!("{i}")
        } else {
            format!("{:?}", f32::read(w))
        }
    };
    if b.len().is_multiple_of(4) && (4..=16).contains(&b.len()) {
        let words: Vec<String> = b.chunks_exact(4).map(word).collect();
        return words.join(" ");
    }
    let hex: String = b.iter().take(16).map(|x| format!("{x:02x}")).collect();
    if b.len() > 16 { format!("{hex}...") } else { hex }
}
