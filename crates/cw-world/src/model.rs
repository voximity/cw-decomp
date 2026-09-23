//! `cube::Model` voxel models, the model table `world->models` (`world+0x20`) and
//! `cube::World::placeModel`, `Server.exe 0x00524540`. See
//! `analysis/notes/functions/00524540_placeModel.md` and `analysis/notes/porting-brief.md`.
//!
//! # The model table
//!
//! `main` (`Server.exe 0x00549ead`) constructs the world, runs `World::load` (`0x004d83a0`) and
//! then calls `cube::Database::load(dir = "")` (`Server.exe 0x00431400`) with `this = world+0x1c`,
//! so the vector at `this+4` is `world+0x20`. That function:
//!
//! 1. resizes the vector to 0xa09 (2569) entries and fills every slot with `new Model(...)`
//!    (0x60 bytes, constructor `0x0042ebb0`: sizes 0, voxels null);
//! 2. opens `dir + "data1.db"` (the prefix is the empty string passed by `main`);
//! 3. makes 2557 unrolled calls `models[i]->load(key, data1, raw)` (`cube::Model::load`,
//!    `0x0042f9a0`). Each call site names its slot either as `mov ecx,[ebx]; mov ecx,[ecx+4*i]`
//!    or as `push i; mov ecx,ebx; call 0x00402bb0 (vector::operator[]); mov ecx,[eax]`, and its
//!    key as the `const char*` pushed to the `operator+(string, const char*)` call (`0x00430780`)
//!    right before it.
//!
//! The index-to-key mapping is therefore a fixed list compiled into the code, not derived from
//! the key names or a query. [`model_names::MODEL_LOADS`] holds it in call order. It was
//! extracted from `game/Server.exe` with the script below (`uv run --with capstone python gen.py
//! game/Server.exe crates/cw-world/src/model_names.rs`), which disassembles `Database::load` and
//! tracks `ecx`, `eax` and the pushed arguments of each call. `tests/model.rs` re-checks every
//! entry against the bytes of `Server.exe` when `CW_GAME_DIR` is set.
//!
//! Facts about the list:
//! - 19 slots are never loaded and stay empty models (1559..=1563, 1589..=1593, 2060, 2061,
//!   2071, 2353..=2356, 2419, 2420).
//! - Seven slots are loaded twice; the later call wins because `setVoxels` frees the old voxels:
//!   1224..=1228 and 1230 (the same saurian armour and backpack keys twice), and 1300
//!   (`orc-head-m01.cub`, then `orc-head.cub`).
//! - 21 keys have no blob in data1.db (for example `cubequest4.cub`, `obsidian-greatsword.cub`,
//!   `overground-dungeon01.cub`). `Model::load` then leaves the model as it was, so those slots
//!   stay empty unless an earlier call filled them.
//! - Slots used by the generator: 2123 (0x84b) `character-platform.cub` (the spawn structure of
//!   generateZone), 2300/2301 `palm-leaf.cub`/`palm-leaf-diagonal.cub`, 2544/2545
//!   `castle-arc.cub`/`temple-arc.cub`.
//!
//! `World::World` (`0x004c8570`) also calls `Model::load`, twelve times, with a null database
//! (loose files such as `framework-floor-wood.cub`) into `world+0x800154`, a different table.
//!
//! ```text
//! import struct, sys, capstone
//! from capstone import x86
//! exe, out = sys.argv[1], sys.argv[2]
//! b = open(exe, "rb").read()
//! pe = struct.unpack_from("<I", b, 0x3c)[0]
//! nsec, opt = struct.unpack_from("<H", b, pe + 6)[0], struct.unpack_from("<H", b, pe + 20)[0]
//! base = struct.unpack_from("<I", b, pe + 52)[0]
//! secs = [struct.unpack_from("<IIII", b, pe + 24 + opt + 40 * i + 8) for i in range(nsec)]
//! def off(v):
//!     return next(ro + v - base - va for vs, va, rs, ro in secs if 0 <= v - base - va < max(vs, rs))
//! def cstr(v):
//!     o = off(v); return b[o:b.index(b"\0", o)].decode()
//! md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32); md.detail = True
//! regs, stack, res = {}, [], []
//! def val(op, ins):
//!     if op.type == x86.X86_OP_IMM: return ("imm", op.imm & 0xffffffff)
//!     if op.type == x86.X86_OP_REG: return regs.get(ins.reg_name(op.reg))
//!     m = op.mem; r = ins.reg_name(m.base) if m.base else None; bv = regs.get(r)
//!     if m.index: return None
//!     if r == "ebx" and m.disp == 0: return ("vec",)                  # [ebx] = models.begin
//!     if bv == ("vec",) and m.disp % 4 == 0: return ("model", m.disp // 4)
//!     if bv and bv[0] == "elem" and m.disp == 0: return ("model", bv[1])
//!     return None
//! for ins in md.disasm(b[off(0x431400):off(0x45f080)], 0x431400):    # cube::Database::load
//!     ops = ins.operands
//!     if ins.mnemonic == "push": stack.append(val(ops[0], ins))
//!     elif ins.mnemonic == "mov" and ops[0].type == x86.X86_OP_REG: regs[ins.reg_name(ops[0].reg)] = val(ops[1], ins)
//!     elif ins.mnemonic == "call":
//!         t = ops[0].imm if ops[0].type == x86.X86_OP_IMM else None
//!         regs["eax"] = None
//!         if t == 0x430780: regs["eax"] = ("str", cstr(stack[-3][1]))  # operator+(string, const char*)
//!         elif t == 0x402bb0: regs["eax"] = ("elem", stack[-1][1])     # vector<Model*>::operator[]
//!         elif t == 0x42f9a0:                                           # Model::load(key, db, raw)
//!             m, key, raw = regs.get("ecx"), stack[-1], stack[-3]
//!             assert m[0] == "model" and key[0] == "str" and raw[0] == "imm", hex(ins.address)
//!             res.append((ins.address, m[1], key[1], raw[1]))
//!         regs["ecx"] = regs["edx"] = None; stack = []
//!     elif ins.mnemonic in ("lea", "add", "sub", "xor", "inc", "dec", "sar", "shl", "and", "or") \
//!             and ops and ops[0].type == x86.X86_OP_REG and ins.reg_name(ops[0].reg) != "esp":
//!         regs[ins.reg_name(ops[0].reg)] = None
//! assert len(res) == 2557
//! lines = ["//! Generated from Server.exe by the script in the `model` module docs; do not edit.", "//!",
//!          "//! Every `cube::Model::load(key, data1, raw)` call (`Server.exe 0x0042f9a0`) made by",
//!          "//! `cube::Database::load` (`Server.exe 0x00431400`), in call order.", "",
//!          "/// `(call address, index into world+0x20, data1.db key, raw)`; `raw` is `Model::load`'s third",
//!          "/// argument. A later entry for the same index reloads that model.",
//!          "pub static MODEL_LOADS: [(u32, u16, &str, bool); %d] = [" % len(res)]
//! lines += ['    (0x%08x, %d, "%s", %s),' % (a, i, k, "true" if f else "false") for a, i, k, f in res]
//! open(out, "w", newline="\n").write("\n".join(lines + ["];"]) + "\n")
//! ```
//!
//! # Model layout
//!
//! `cube::Model` (0x60 bytes): `+0x30` `u8*` RGB voxels, `+0x44/+0x48/+0x4c` sizes x, y, z,
//! `+0x55` the `raw` flag of `load`. `Model::load` with a database reads three little-endian
//! `u32` sizes and then the RGB triples from the decoded blob, and hands them to
//! `Model::setVoxels` (`0x00430230`), which copies them unchanged, so the voxel at `(x, y, z)`
//! is at `((sy * z + y) * sx + x) * 3`: x varies fastest, as in the `.cub` file.
//! `Model::getVoxel` (`0x00430730`) returns the static `DAT_00583dfc` (`[0, 0, 0]`, never
//! written) outside the model. When `raw` is false `setVoxels` also collects the positions of
//! pure red, green and blue voxels into three vectors (`+4`, `+0x10`, `+0x1c`); nothing in the
//! generator reads them, so they are not kept.


#[path = "model_names.rs"]
pub mod model_names;

use crate::fixed::from_block;
use crate::surface::Block;
use crate::world::World;
use crate::zone::{Prop, Spawn, SpawnAi, Static, Zone, set_block};

pub use model_names::MODEL_LOADS;

/// Number of slots of `world->models` (`Database::load` resizes the vector to 0xa09).
pub const MODEL_COUNT: usize = 0xa09;

/// `DAT_00583dfc`: the colour `Model::getVoxel` returns outside the model; an empty voxel.
pub const EMPTY: [u8; 3] = [0, 0, 0];

/// A voxel model as the original keeps it in `world->models[]` (`world+0x20`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    /// `+0x44`, `+0x48`, `+0x4c`: sizes along x, y, z.
    pub size: [i32; 3],
    /// `+0x30`: `size[0] * size[1] * size[2]` RGB voxels, x fastest, then y, then z. Empty when a
    /// size is not positive (the pointer is null in the original).
    pub voxels: Vec<[u8; 3]>,
    /// `+0x55`: the `raw` argument of `Model::load`.
    pub raw: bool,
}

impl Model {
    /// `Model::setVoxels(sx, sy, sz, data, raw)`, `Server.exe 0x00430230`, on a fresh model.
    /// `voxels` must hold `sx * sy * sz` triples when every size is positive.
    pub fn new(size: [i32; 3], voxels: Vec<[u8; 3]>, raw: bool) -> Self {
        if size.iter().all(|&s| s > 0) {
            assert_eq!(voxels.len(), size.iter().map(|&s| s as usize).product::<usize>());
            Self { size, voxels, raw }
        } else {
            Self { size, voxels: Vec::new(), raw }
        }
    }

    /// `Model::load(key, db, raw)`, `Server.exe 0x0042f9a0`, for a blob that was found: `bytes`
    /// is the decoded blob (`cw_formats::AssetDb::get`). The sizes are read as `i32`. A body
    /// shorter than the sizes ask for is not copied, leaving the zero-filled buffer the original
    /// allocates; extra bytes are ignored. A blob shorter than the 12-byte header (none in
    /// data1.db) is treated as sizes 0.
    pub fn from_cub_bytes(bytes: &[u8], raw: bool) -> Self {
        if bytes.len() < 12 {
            return Self { size: [0; 3], voxels: Vec::new(), raw };
        }
        let word = |o: usize| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let size = [word(0), word(4), word(8)];
        if size.iter().any(|&s| s <= 0) {
            return Self { size, voxels: Vec::new(), raw };
        }
        let count = size.iter().map(|&s| s as usize).product::<usize>();
        let body = &bytes[12..];
        let voxels =
            if body.len() >= count * 3 { body[..count * 3].as_chunks::<3>().0.to_vec() } else { vec![EMPTY; count] };
        Self { size, voxels, raw }
    }

    /// `Model::getVoxel(x, y, z)`, `Server.exe 0x00430730`: [`EMPTY`] outside the model.
    #[inline]
    pub fn voxel(&self, x: i32, y: i32, z: i32) -> [u8; 3] {
        let [sx, sy, sz] = self.size;
        if x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz {
            return EMPTY;
        }
        self.voxels[((sy * z + y) * sx + x) as usize]
    }
}

/// Fills `table` as `cube::Database::load` (`Server.exe 0x00431400`) fills `world->models`:
/// [`MODEL_COUNT`] empty models, then every [`MODEL_LOADS`] entry in call order. `blob` returns
/// the decoded data1.db blob for a key, or `None` when the key has no blob (then the slot keeps
/// what it had, as `Model::load` only stores `raw` before `getBlobVec` fails).
pub fn load_models(table: &mut Vec<Model>, mut blob: impl FnMut(&str) -> Option<Vec<u8>>) {
    table.clear();
    table.resize(MODEL_COUNT, Model::default());
    for &(_, index, key, raw) in MODEL_LOADS.iter() {
        let slot = &mut table[usize::from(index)];
        slot.raw = raw;
        if let Some(bytes) = blob(key) {
            *slot = Model::from_cub_bytes(&bytes, raw);
        }
    }
}

/// The `cw-formats` glue. Needs `cw-formats = { path = "../cw-formats" }` in
/// `crates/cw-world/Cargo.toml` (an optional dependency enables this module through its
/// implicit `cw-formats` feature; a plain dependency needs the `cfg` removed).
mod formats {
    use std::path::Path;

    use super::{Model, load_models};

    impl Model {
        /// A model from a parsed `.cub` file, as `Model::load` builds it from the same bytes.
        pub fn from_cub(cub: &cw_formats::CubModel, raw: bool) -> Self {
            Model::new(cub.size.map(|s| s as i32), cub.voxels.clone(), raw)
        }
    }

    /// [`load_models`] reading `game_dir/data1.db`.
    pub fn load_models_from_game_dir(game_dir: &Path, table: &mut Vec<Model>) -> cw_formats::Result<()> {
        let db = cw_formats::AssetDb::open(game_dir.join("data1.db"))?;
        let mut error = None;
        load_models(table, |key| match db.get(key) {
            Ok(bytes) => Some(bytes),
            Err(cw_formats::Error::NotFound(_)) => None,
            Err(e) => {
                error.get_or_insert(e);
                None
            }
        });
        error.map_or(Ok(()), Err)
    }
}

pub use formats::load_models_from_game_dir;

/// One entry of the block log `placeModel` fills when its last argument is not null: the 20-byte
/// element of the `std::list` at `log+0x18` (count at `+0x1c`), `{int x, y, z; u8 r, g, b, type;
/// int 0}`, pushed by `FUN_00420100`/`FUN_00528400` just before the matching `setBlock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggedBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: Block,
}

/// `{255, 255, 255, 0xc0}`: air with flags 0x40 | 0x80, written into the hollow interior of a
/// model column.
const HOLLOW: Block = [255, 255, 255, 0xc0];
/// `{0, 0, 0, 0x40}`: air written where a marker voxel was.
const AIR40: Block = [0, 0, 0, 0x40];

/// `Spawn::equipment` indexes of the slots `placeModel` writes (`+0x120 + 0x118 * i`).
const CHEST: usize = 2; // +0x350
const BOOTS: usize = 3; // +0x468
const GLOVES: usize = 4; // +0x580
const SHOULDER: usize = 5; // +0x698
const WEAPON: usize = 7; // +0x8c8

/// `_ftol2` (`FUN_0054a946`) of a double constant.
#[inline]
fn ftol(d: f64) -> i64 {
    d as i64
}

/// `fixed(0.5)` (`FUN_004dab30(0.5)` = `ftol(0.5 * 65536.0)`), `ftol(32768.0)` and
/// `FUN_00402510` of `0.5f` (`ftol(0.5f * 65536f)`): all 32768.
const HALF: i64 = 32768;

/// `FUN_00521ed0` (`Model::findColorBox(x, y, z, rot, &lo, &hi)`): the box of voxels with the
/// colour of `(x, y, z)` whose minimum corner that voxel is, rotated like the placement and with
/// `hi` exclusive. `None` unless the voxels at -x, -y and -z each have another colour.
fn find_color_box(model: &Model, x: i32, y: i32, z: i32, rot: i32) -> Option<([i32; 3], [i32; 3])> {
    let c = model.voxel(x, y, z);
    if model.voxel(x - 1, y, z) == c || model.voxel(x, y - 1, z) == c || model.voxel(x, y, z - 1) == c {
        return None;
    }
    let [sx, sy, sz] = model.size;
    let mut lo = [x, y, z];
    let mut hi = [x, y, z];
    // The three runs start from the corner independently.
    let mut i = x;
    while i < sx && model.voxel(i, y, z) == c {
        hi[0] = i;
        i += 1;
    }
    let mut i = y;
    while i < sy && model.voxel(x, i, z) == c {
        hi[1] = i;
        i += 1;
    }
    let mut i = z;
    while i < sz && model.voxel(x, y, i) == c {
        hi[2] = i;
        i += 1;
    }
    let turn = |p: [i32; 3]| match rot % 4 {
        1 => [p[1], sx - p[0] - 1, p[2]],
        2 => [sx - p[0] - 1, sy - p[1] - 1, p[2]],
        3 => [sy - p[1] - 1, p[0], p[2]],
        _ => p,
    };
    lo = turn(lo);
    hi = turn(hi);
    for a in 0..3 {
        if hi[a] < lo[a] {
            std::mem::swap(&mut lo[a], &mut hi[a]);
        }
        hi[a] += 1;
    }
    Some((lo, hi))
}

/// `FUN_004ff1a0` (`makeBoxEntity(model, &origin, rot, x, y, z, &static)`): fills the position,
/// orientation and size of `se` from the colour box at `(x, y, z)`; false (and `se` untouched)
/// when [`find_color_box`] fails.
fn make_box_static(model: &Model, origin: [i32; 3], rot: i32, x: i32, y: i32, z: i32, se: &mut Static) -> bool {
    let Some((lo, hi)) = find_color_box(model, x, y, z, rot) else { return false };
    // vec3<i64>((lo + hi) << 16) *= ftol(32768.0) through __allmul and __alldiv by 0x10000
    // (FUN_00402db0); the product's z is then replaced by lo.z << 16.
    se.x = (from_block(lo[0] + hi[0]) * HALF) / 65536 + from_block(origin[0]);
    se.y = (from_block(lo[1] + hi[1]) * HALF) / 65536 + from_block(origin[1]);
    se.z = from_block(lo[2]) + from_block(origin[2]);
    let dx = hi[0] - lo[0];
    let dy = hi[1] - lo[1];
    if dy < dx {
        se.rotation = 0;
        se.scale[0] = dx as f32;
        se.scale[1] = dy as f32;
    } else {
        se.rotation = 1;
        se.scale[0] = dy as f32;
        se.scale[1] = dx as f32;
    }
    se.scale[2] = (hi[2] - lo[2]) as f32;
    se.b30 = 1;
    true
}

/// The per-call constants of `placeModel`.
struct Placement<'a> {
    model: &'a Model,
    /// `param_2`
    pos: [i32; 3],
    /// `param_3`, unreduced.
    rot: i32,
    /// `param_5`
    kind: i32,
    /// Climate B at the origin (`local_6e4`).
    climate_b: f32,
}

/// One voxel being placed: model coordinates, world coordinates and whether it lies strictly
/// between the lowest and highest solid voxels of its column (`minZ < z < maxZ`).
#[derive(Clone, Copy)]
struct Cursor {
    mx: i32,
    my: i32,
    z: i32,
    wx: i32,
    wy: i32,
    wz: i32,
    interior: bool,
}

impl Cursor {
    fn fixed(&self, dx: i64, dy: i64, dz: i64) -> (i64, i64, i64) {
        (from_block(self.wx) + dx, from_block(self.wy) + dy, from_block(self.wz) + dz)
    }
}

/// Logs `block` at `(x, y, z)` when there is a log, then `setBlock`s it.
fn put(world: &World, zone: &mut Zone, log: &mut Option<&mut Vec<LoggedBlock>>, x: i32, y: i32, z: i32, block: Block) {
    if let Some(log) = log.as_deref_mut() {
        log.push(LoggedBlock { x, y, z, block });
    }
    set_block(world, zone, x, y, z, block);
}

impl World {
    /// `cube::World::placeModel(model, pos, rot, blockType, kind, zone, foundation, margins, NULL)`,
    /// `Server.exe 0x00524540`: [`World::place_model_logged`] without a block log.
    #[allow(clippy::too_many_arguments)]
    pub fn place_model(
        &mut self,
        zone: &mut Zone,
        model: &Model,
        pos: [i32; 3],
        rot: i32,
        block_type: u8,
        kind: i32,
        foundation: bool,
        margins: [i32; 4],
    ) {
        self.place_model_logged(zone, model, pos, rot, block_type, kind, foundation, margins, None);
    }

    /// `cube::World::placeModel(model, pos, rot, blockType, kind, zone, foundation, margins,
    /// log)`, `Server.exe 0x00524540`.
    ///
    /// `zone` is required: the original accepts null and then skips the zone overlap test and
    /// looks columns up in the whole world, but every caller passes the zone being generated.
    /// `log` is the optional block log (`param_9`; a `Vec` instead of the `std::list` at
    /// `log+0x18`). `rot` must not be negative: the original's `rot % 4` switches have no case for
    /// negative remainders and leave the world coordinates uninitialised.
    #[allow(clippy::too_many_arguments)]
    pub fn place_model_logged(
        &mut self,
        zone: &mut Zone,
        model: &Model,
        pos: [i32; 3],
        rot: i32,
        block_type: u8,
        kind: i32,
        foundation: bool,
        margins: [i32; 4],
        mut log: Option<&mut Vec<LoggedBlock>>,
    ) {
        debug_assert!(rot >= 0, "placeModel with a negative rotation ({rot})");
        let [sx, sy, sz] = model.size;
        let [px, py, pz] = pos;
        // Footprint in world space: the raw `rot` decides the swap (`& 0x80000001` idiom).
        let (fx, fy) = if rot % 2 == 0 { (sx, sy) } else { (sy, sx) };
        let zx0 = zone.x * 256;
        let zy0 = zone.y * 256;
        if !(zx0 < px + fx && zy0 < py + fy && px < zx0 + 256 && py < zy0 + 256) {
            return;
        }
        // getColumn(px, py, zone) ? column.climateB : climateB(px, py).
        let climate_b = if zone.contains(px, py) { zone.column(px, py).climate_b } else { self.climate_b(px, py) };
        let r = rot % 4;
        let m = margins;
        let (x0, y0, x1, y1) = match r {
            0 => (m[0], m[1], sx - m[2], sy - m[3]),
            1 => (m[3], m[0], sx - m[1], sy - m[2]),
            2 => (m[2], m[3], sx - m[0], sy - m[1]),
            3 => (m[1], m[2], sx - m[3], sy - m[0]),
            _ => (0, 0, sx, sy),
        };
        let map = |mx: i32, my: i32| match r {
            1 => (px + my, py + (sx - mx) - 1),
            2 => (px + (sx - mx) - 1, py + (sy - my) - 1),
            3 => (px + (sy - my) - 1, py + mx),
            _ => (px + mx, py + my),
        };
        let p = Placement { model, pos, rot, kind, climate_b };
        for mx in x0..x1 {
            for my in y0..y1 {
                // Vertical extent of the solid voxels of this column.
                let mut min_z = sz;
                let mut max_z = 0;
                for z in 0..sz {
                    if model.voxel(mx, my, z) != EMPTY {
                        min_z = min_z.min(z);
                        max_z = max_z.max(z);
                    }
                }
                // Foundation: extend the bottom voxel down to solid ground with type 6.
                if foundation && model.voxel(mx, my, 0) != EMPTY {
                    let (wx, wy) = map(mx, my);
                    if zone.contains(wx, wy) && zone.column(wx, wy).height < pz - 1 {
                        let mut h = pz - 1;
                        loop {
                            let t = zone.block(wx, wy, h)[3] & 0x1f;
                            if t != 0 && t != 2 {
                                break;
                            }
                            let v = model.voxel(mx, my, 0);
                            set_block(self, zone, wx, wy, h, [v[0], v[1], v[2], 6]);
                            h -= 1;
                            if h <= zone.column(wx, wy).height {
                                break;
                            }
                        }
                    }
                }
                // The voxels, top to bottom.
                for z in (0..sz).rev() {
                    let (wx, wy) = map(mx, my);
                    let wz = pz + z;
                    let v = model.voxel(mx, my, z);
                    let interior = min_z < z && z < max_z;
                    if v == EMPTY {
                        if interior {
                            put(self, zone, &mut log, wx, wy, wz, HOLLOW);
                        }
                        continue;
                    }
                    // An existing solid structure block (flag 0x40) is kept.
                    let b = zone.block(wx, wy, wz);
                    if b[3] & 0x40 != 0 {
                        let t = b[3] & 0x1f;
                        if t != 0 && t != 2 {
                            continue;
                        }
                    }
                    let at = Cursor { mx, my, z, wx, wy, wz, interior };
                    if matches!(kind, 1..=7 | 9..=15) && self.place_marker(zone, &mut log, &p, at, v) {
                        continue;
                    }
                    let block = if kind == 8 && v == [0, 0, 255] {
                        [255, 255, 255, 2]
                    } else {
                        [v[0], v[1], v[2], block_type | 0x40]
                    };
                    put(self, zone, &mut log, wx, wy, wz, block);
                }
            }
        }
    }

    /// The marker colours of `placeModel` for kinds 1-7 and 9-15, tested in the original's order.
    /// Returns false when `v` is not a marker colour (the voxel is then written normally).
    fn place_marker(
        &mut self,
        zone: &mut Zone,
        log: &mut Option<&mut Vec<LoggedBlock>>,
        p: &Placement,
        at: Cursor,
        v: [u8; 3],
    ) -> bool {
        let kind = p.kind;
        let rot = p.rot;
        let air = |world: &World, zone: &mut Zone, log: &mut Option<&mut Vec<LoggedBlock>>| {
            put(world, zone, log, at.wx, at.wy, at.wz, AIR40);
        };
        let air_inside = |world: &World, zone: &mut Zone, log: &mut Option<&mut Vec<LoggedBlock>>| {
            if at.interior {
                put(world, zone, log, at.wx, at.wy, at.wz, AIR40);
            }
        };
        // FURN(T, size): a static at the voxel centre facing -rot.
        let furniture = |zone: &mut Zone, kind: u32, scale: [f32; 3]| {
            let (x, y, z) = at.fixed(HALF, HALF, 0);
            zone.statics.push(Static { kind, x, y, z, rotation: rot.wrapping_neg(), scale, ..Static::NEW });
        };
        match v {
            [255, 0, 0] => {
                // Door. The original builds this static inline (same values as the constructor).
                let mut se = Static::NEW;
                if make_box_static(p.model, p.pos, rot, at.mx, at.my, at.z, &mut se) {
                    se.kind = u32::from(kind == 7) + 1;
                    zone.statics.push(se);
                }
                air(self, zone, log);
            }
            [0, 255, 0] => {
                let mut se = Static::NEW;
                if make_box_static(p.model, p.pos, rot, at.mx, at.my, at.z, &mut se) {
                    se.kind = 3;
                    zone.statics.push(se);
                }
                air(self, zone, log);
            }
            [0, 127, 127] => {
                if p.climate_b <= 0.5 {
                    return true;
                }
                let t = zone.block(at.wx, at.wy, at.wz)[3] & 0x1f;
                if t != 0 && t != 2 {
                    return true;
                }
                // rand() at 0x00525779
                if self.rng.rand() % 5 == 0 {
                    // rand() at 0x005257a4
                    let kind = (self.rng.rand() % 3 + 0x29) as u32;
                    let (x, y, z) = at.fixed(ftol(32964.608), ftol(32964.608), ftol(196.608));
                    zone.props.push(Prop {
                        kind,
                        x,
                        y,
                        z,
                        scale: f32::from_bits(0x3e19999a),
                        rotation: rot.wrapping_mul(-90) as f32,
                        f2c: [1.0; 3],
                        flags: 0,
                        ..Prop::NEW
                    });
                }
                air(self, zone, log);
            }
            [255, 127, 0] => {
                let (x, y, z) = at.fixed(HALF, HALF, 0);
                zone.statics.push(Static { kind: 0x13, x, y, z, rotation: 0, scale: [2.0, 3.0, 1.0], ..Static::NEW });
                let (x, y, z) = at.fixed(ftol(147456.0), HALF, 0);
                zone.statics.push(Static { kind: 0x14, x, y, z, rotation: 0, scale: [1.0; 3], ..Static::NEW });
                air_inside(self, zone, log);
            }
            [255, 127, 127] => {
                // rand() at 0x00525e0d
                let kind = (self.rng.rand() % 3 + 0x20) as u32;
                let (x, y, z) = at.fixed(HALF, HALF, 0);
                zone.statics.push(Static { kind, x, y, z, rotation: rot, scale: [2.0, 3.0, 1.0], ..Static::NEW });
                air_inside(self, zone, log);
            }
            [127, 127, 0] => {
                let (x, y, z) = at.fixed(HALF, HALF, 0);
                // rand() at 0x00526116 (orientation of the first static)
                let rotation = self.rng.rand() % 4;
                zone.statics.push(Static { kind: 0xc, x, y, z, rotation, scale: [3.0, 3.0, 1.0], ..Static::NEW });
                let (x, y, z) = at.fixed(ftol(176947.2), HALF, 0);
                // rand() at 0x0052622e (orientation of the second static, reused by its copy)
                let rotation = self.rng.rand() % 4;
                let mut second = Static { kind: 0x10, x, y, z, rotation, scale: [1.0, 1.0, 0.5], ..Static::NEW };
                zone.statics.push(second.clone());
                (second.x, second.y, second.z) = at.fixed(ftol(-111411.2), HALF, 0);
                zone.statics.push(second);
                // rand() at 0x00526350: stored into the already pushed copy's orientation, unused.
                self.rng.rand();
                air_inside(self, zone, log);
            }
            [255, 255, 0] => {
                let (x, y, z) = at.fixed(HALF, HALF, 0);
                zone.props.push(Prop {
                    kind: 0xd,
                    x,
                    y,
                    z,
                    scale: f32::from_bits(0x3dcccccd),
                    rotation: 0.0,
                    f2c: [f32::from_bits(0x3f19999a), 0.5, f32::from_bits(0x3ecccccd)],
                    flags: 1,
                    ..Prop::NEW
                });
                air_inside(self, zone, log);
            }
            [63, 0, 0] => {
                let (x, y, z) = at.fixed(HALF, HALF, HALF);
                zone.props.push(Prop {
                    kind: 0x30,
                    x,
                    y,
                    z,
                    scale: f32::from_bits(0x3d851eb8),
                    rotation: rot.wrapping_mul(-90) as f32,
                    f2c: [1.0; 3],
                    flags: 0,
                    ..Prop::NEW
                });
                air_inside(self, zone, log);
            }
            [0, 0, 255] => {
                let prop = match kind {
                    2 => Some(0x21),
                    3 => Some(0x22),
                    4 => Some(0x23),
                    5 => Some(0x24),
                    6 => Some(0x25),
                    13 => Some(0x26),
                    14 => Some(0x27),
                    15 => Some(0x28),
                    _ => None,
                };
                if let Some(prop) = prop {
                    // fixed(0.5), fixed(0.5), fixed(-0.5); kind 2 uses ftol(-32768.0) for z.
                    let (x, y, z) = at.fixed(HALF, HALF, -HALF);
                    zone.props.push(Prop {
                        kind: prop,
                        x,
                        y,
                        z,
                        scale: f32::from_bits(0x3dcccccd),
                        rotation: (-1i32).wrapping_sub(rot).wrapping_mul(90) as f32,
                        f2c: [1.0; 3],
                        flags: 0,
                        ..Prop::NEW
                    });
                }
                air_inside(self, zone, log);
            }
            [255, 0, 255] => {
                if p.climate_b > 0.5 {
                    let mut se = Static::NEW;
                    if make_box_static(p.model, p.pos, rot, at.mx, at.my, at.z, &mut se) {
                        // rand() at 0x00526b33
                        se.kind = (self.rng.rand() % 3 + 0x2f) as u32;
                        zone.statics.push(se);
                    }
                    air_inside(self, zone, log);
                }
            }
            [0, 255, 255] => match kind {
                13 => {
                    furniture(zone, 0x47, [3.0, 3.0, 6.0]);
                    air_inside(self, zone, log);
                }
                14 => {
                    furniture(zone, 0x4b, [1.5, 1.5, 1.0]);
                    air_inside(self, zone, log);
                }
                15 => {
                    furniture(zone, 0x49, [1.5, 1.5, 1.0]);
                    air_inside(self, zone, log);
                }
                _ => {
                    let (x, y, z) = at.fixed(HALF, HALF, HALF);
                    let level = self.spawn_level(from_block(at.wx), from_block(at.wy));
                    zone.spawns.push(Spawn {
                        x,
                        y,
                        z,
                        f28: 6,
                        entity_type: i32::from(kind != 10) + 0x8d,
                        rotation: 2i32.wrapping_sub(rot).wrapping_mul(90) as f32,
                        level,
                        ..Spawn::NEW
                    });
                    air(self, zone, log);
                }
            },
            [0, 0, 127] => {
                if kind == 13 {
                    furniture(
                        zone,
                        0x4d,
                        [f32::from_bits(0x3fe66666), f32::from_bits(0x3fe66666), f32::from_bits(0x3f99999a)],
                    );
                    air(self, zone, log);
                }
            }
            [0, 255, 127] => match kind {
                13 => {
                    furniture(zone, 0x48, [1.0; 3]);
                    air_inside(self, zone, log);
                }
                14 => {
                    furniture(
                        zone,
                        0x4c,
                        [f32::from_bits(0x3fe66666), f32::from_bits(0x3fe66666), f32::from_bits(0x3f99999a)],
                    );
                    air_inside(self, zone, log);
                }
                15 => {
                    furniture(zone, 0x4a, [1.5, 1.5, 3.0]);
                    air_inside(self, zone, log);
                }
                _ => {}
            },
            [0, 127, 0] => {
                // rand() at 0x0052783e
                let kind = (self.rng.rand() % 9 + 0x38) as u32;
                let (x, y, z) = at.fixed(HALF, HALF, 0);
                // rand() at 0x005278c7
                let rotation = self.rng.rand() % 4;
                let scale = if kind < 0x3c { [0.5; 3] } else { [1.0; 3] };
                zone.statics.push(Static { kind, x, y, z, rotation, scale, ..Static::NEW });
                air_inside(self, zone, log);
            }
            [127, 0, 127] => {
                self.place_guard(zone, p, at);
                air_inside(self, zone, log);
            }
            _ => return false,
        }
        true
    }

    /// The `(127, 0, 127)` marker: a creature with a patrol behaviour ([`SpawnAi::Sentry`]:
    /// `SequentialBehavior` of `CombatBehavior(20.0)`, `LookAtPlayerBehavior`,
    /// `WalkPathBehavior(2.0)` with the spawn position as its path, 0x00527fb2; building it calls
    /// no `rand()`).
    fn place_guard(&mut self, zone: &mut Zone, p: &Placement, at: Cursor) {
        let mut s = Spawn::NEW;
        s.f28 = 3;
        // rand() at 0x00527a1a
        s.entity_type = self.rng.rand() % 4;
        s.rotation = 2i32.wrapping_sub(p.rot).wrapping_mul(90) as f32;
        (s.x, s.y, s.z) = at.fixed(HALF, HALF, HALF);
        // `*(u16*)(C+0x350) = 4; *(u8*)(C+0x35d) = 6`: chest type 4, subtype 0, material 6.
        s.equipment[CHEST].item_type = 4;
        s.equipment[CHEST].sub_type = 0;
        s.equipment[CHEST].material = 6;
        s.level = self.spawn_level(from_block(at.wx), from_block(at.wy));
        // `+0x30` is written as a byte.
        let set_b30 = |s: &mut Spawn, v: u8| s.f30 = (s.f30 & 0xff00) | u16::from(v);
        match p.kind {
            6 => set_b30(&mut s, 0x83),
            7 => {
                s.level = 20;
                // rand() at 0x00527b44
                s.entity_type = self.rng.rand() % 2 + 2;
                set_b30(&mut s, 0x89);
            }
            9..=12 => {
                // `+0x7a` (appearance flags) is assigned, not ORed.
                s.appearance.flags = 0x40;
                s.level = 20;
                s.entity_type = 2;
                let (b30, weapon_material, armour_material) = match p.kind {
                    9 => (1, 1, 1),
                    10 => (2, 2, 0x1a),
                    11 => (4, 1, 0x1b),
                    _ => (3, 2, 0x19),
                };
                set_b30(&mut s, b30);
                let weapon = &mut s.equipment[WEAPON];
                match p.kind {
                    9 => {
                        weapon.item_type = 3;
                        // rand() at 0x00527b87
                        weapon.sub_type = (self.rng.rand() % 3 + 0xf) as u8;
                    }
                    10 => {
                        weapon.item_type = 3;
                        // rand() at 0x00527c9e
                        weapon.sub_type = (self.rng.rand() % 2 + 6) as u8;
                    }
                    // `*(u16*)(C+0x8c8) = 0x0503` / `0x0a03`.
                    11 => (weapon.item_type, weapon.sub_type) = (3, 5),
                    _ => (weapon.item_type, weapon.sub_type) = (3, 10),
                }
                let level = s.level as u16;
                // The five rarity rand() calls in slot order: weapon, shoulder, chest, boots,
                // gloves (kind 9: 0x00527b9e..0x00527c51; kinds 10-12 repeat the pattern up to
                // 0x00527f6f). Each slot's writes are type, rarity, level, material.
                for (slot, item_type, material) in [
                    (WEAPON, None, weapon_material),
                    (SHOULDER, Some(7), armour_material),
                    (CHEST, Some(4), armour_material),
                    (BOOTS, Some(6), armour_material),
                    (GLOVES, Some(5), armour_material),
                ] {
                    let item = &mut s.equipment[slot];
                    if let Some(t) = item_type {
                        item.item_type = t;
                    }
                    item.rarity = (self.rng.rand() % 5) as u8;
                    item.level = level;
                    item.material = material;
                }
            }
            13 => set_b30(&mut s, 0x85),
            14 => set_b30(&mut s, 0x86),
            15 => set_b30(&mut s, 0x87),
            _ => {}
        }
        s.ai = Some(SpawnAi::Sentry { path: vec![[s.x, s.y, s.z]] });
        zone.spawns.push(s);
    }
}
