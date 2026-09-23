//! Creature pose: the per-body-part transforms of `Cube.exe 0x004128f0` (70 KB, 851 blocks),
//! proposed name `cube::CreatureFigure::draw`.
//!
//! The original is a `__thiscall` on an object embedded in the creature at `creature+0x1498`
//! (this module's [`PoseState`]): `+0` is the creature pointer, `+4`/`+8` the animation time and
//! animation mode that `GameController::render` copies in every frame (0x004b4ef0: time = entity
//! mode time `0x5c`, mode = [`anim_mode_for`] of the entity mode `0x58`), `+0xc..+0xbc` the
//! smoothed pose parameters, `+0xbc..+0xe8` the part model pointers, `+0xec..+0x52c` one 4x4
//! matrix per part, `+0x56c..+0x86c` four 16-point weapon trails. It is called from `render`
//! (0x004b5cf3 for the five after-images, 0x004b5db2 for the creature itself), from 0x004ba8fe /
//! 0x004ba9a5 and from two preview widgets (0x004269e1, 0x00607e31).
//!
//! Its arguments (`ret 0x28`): `param_2` the `CubeShader`, `param_3`/`param_4` two matrices the
//! shader binds with the world matrix, `param_5` the model vector (`GameController+0x300`),
//! `param_6` the frame time in ms (`GameController+0x8006e8`), `param_7` the shader colour,
//! `param_8`/`param_9` two i64 added to the render position x/y (`GameController+0x1d8/+0x1e0`),
//! `param_10` a hide mask, `param_11` the creature's pet (the creature map lookup of
//! `creature+0x11c8`).
//!
//! It runs in five stages, kept in this order (comment map to address ranges):
//!
//! | Stage | Address range | Here |
//! | --- | --- | --- |
//! | model slots from items and appearance | 0x004128f0..0x00412bc6 | `Figure::models` |
//! | base pose from the appearance, walk swing, aim lean, hands/elbows/feet | 0x00412bc6..0x00413af6 | `Figure::prologue` |
//! | the switch over the animation mode (73 entries) | 0x00413af6..0x0041e660 | `Figure::anim_switch` |
//! | smoothing of the pose state, root and body matrices | 0x0041e660..0x0041f720 | `Figure::smooth`, `Figure::frames` |
//! | one matrix and draw per part, weapon trails | 0x0041f720..0x00423d46 | `Figure::draw`, `Figure::update_trails` |
//!
//! Tier B: the transform order, the constants and the float/double mix of every expression
//! follow the assembly (the Ghidra output of this function lost every `this` of the small
//! `__thiscall` helpers; the port was made from a capstone listing annotated with the esp
//! depth). The matrix helpers keep the original's operation order, so the matrices should
//! match to the last bit given the same inputs and the MSVC `sin`/`cos` of `cw-math`.
//!
//! Matrices are Direct3D row-vector matrices (`v' = v * M`), stored in [`glam::Mat4`] with the
//! same 16 floats, which glam reads as the columns: `Mat4 * v` in glam equals the original's
//! `v * M`. Every helper pre-multiplies (a local transform), like `glRotate`.

// The arithmetic keeps the operand order of the assembly (`a = b + a`), its explicit
// clamps and its nested tests on purpose.
#![allow(
    clippy::assign_op_pattern,
    clippy::manual_clamp,
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::collapsible_if,
    clippy::collapsible_else_if
)]

use cw_net::EntityData;
use glam::Mat4;

/// A 3-float vector as the original stores it (x, y, z).
pub type V3 = [f32; 3];
type M = [f32; 16];

const PI_D: f64 = std::f64::consts::PI;

// ------------------------------------------------------------------------------------------
// Entity block readers (the creature object has the block at +0x10: `creature+X` below is
// `entity+(X-0x10)`).
// ------------------------------------------------------------------------------------------

fn rf(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}
fn ri(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}
fn ru16(e: &EntityData, o: usize) -> u16 {
    u16::from_le_bytes(e.0[o..o + 2].try_into().unwrap())
}
fn ri16(e: &EntityData, o: usize) -> i16 {
    i16::from_le_bytes(e.0[o..o + 2].try_into().unwrap())
}
fn rv3(e: &EntityData, o: usize) -> V3 {
    [rf(e, o), rf(e, o + 4), rf(e, o + 8)]
}

/// Entity offsets this module reads (entity block offsets, `creature+0x10+X` in the original).
mod off {
    pub const VEL: usize = 0x24;
    pub const ACCEL: usize = 0x30;
    pub const LOOK_PITCH: usize = 0x48;
    pub const PHYS: usize = 0x4c;
    pub const HOSTILE: usize = 0x50;
    pub const TYPE: usize = 0x54;
    pub const MODE: usize = 0x58;
    pub const MODE_TIME: usize = 0x5c;
    pub const HAIR_RGB: usize = 0x6a;
    pub const APP_FLAGS: usize = 0x6e;
    pub const SCALE: usize = 0x70;
    pub const HEAD_MODEL: usize = 0x7c;
    pub const HAIR_MODEL: usize = 0x7e;
    pub const HAND_MODEL: usize = 0x80;
    pub const FOOT_MODEL: usize = 0x82;
    pub const BODY_MODEL: usize = 0x84;
    pub const TAIL_MODEL: usize = 0x86;
    pub const SHOULDER2_MODEL: usize = 0x88;
    pub const WING_MODEL: usize = 0x8a;
    pub const HEAD_SCALE: usize = 0x8c;
    pub const BODY_SCALE: usize = 0x90;
    pub const HAND_SCALE: usize = 0x94;
    pub const FOOT_SCALE: usize = 0x98;
    pub const SHOULDER2_SCALE: usize = 0x9c;
    pub const WEAPON_SCALE: usize = 0xa0;
    pub const TAIL_SCALE: usize = 0xa4;
    pub const SHOULDER_SCALE: usize = 0xa8;
    pub const WING_SCALE: usize = 0xac;
    pub const BODY_PITCH: usize = 0xb0;
    pub const ARM_PITCH: usize = 0xb4;
    pub const FEET_PITCH: usize = 0xc0;
    pub const WING_PITCH: usize = 0xc4;
    pub const BACK_PITCH: usize = 0xc8;
    pub const BODY_OFFSET: usize = 0xcc;
    pub const HEAD_OFFSET: usize = 0xd8;
    pub const HAND_OFFSET: usize = 0xe4;
    pub const FOOT_OFFSET: usize = 0xf0;
    pub const TAIL_OFFSET: usize = 0xfc;
    pub const WING_OFFSET: usize = 0x108;
    pub const FLAGS: usize = 0x114;
    pub const ROLL: usize = 0x118;
    pub const SLOWED: usize = 0x120;
    pub const SHOW_PATCH: usize = 0x12c;
    pub const CLASS: usize = 0x130;
    pub const SPEC: usize = 0x131;
    pub const CHARGED_MP: usize = 0x134;
    pub const UNUSED_138: usize = 0x138;
    pub const UNUSED_144: usize = 0x144;
    pub const RAY: usize = 0x150;
    pub const CONSUMABLE: usize = 0x1d8;
    pub const fn equip(slot: usize) -> usize {
        0x2f0 + slot * 0x118
    }
}

fn item(e: &EntityData, o: usize) -> &[u8] {
    &e.0[o..o + 0x118]
}

// ------------------------------------------------------------------------------------------
// Math helpers of the original, same operation order.
// ------------------------------------------------------------------------------------------

/// `cos`/`sin` of `(double)(deg * 0.017453292f)` as the rotation helpers take them.
fn cs(deg: f32) -> (f32, f32) {
    let r = f64::from(deg * 0.017_453_292_f32);
    (cw_math::cos(r) as f32, cw_math::sin(r) as f32)
}

/// 0x0040e420: `(float)cos((double)x)`.
fn cosf(x: f32) -> f32 {
    cw_math::cos(f64::from(x)) as f32
}

/// 0x00424b50: `(float)sin((double)x)`.
fn sinf(x: f32) -> f32 {
    cw_math::sin(f64::from(x)) as f32
}

/// `_libm_sse2_sqrt_precise` on a float widened to double.
fn sqrt_d(x: f32) -> f32 {
    f64::from(x).sqrt() as f32
}

/// `mat4Identity` 0x00423e70.
const IDENTITY: M = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `mat4RotateZ` 0x00424610.
fn rot_z(m: &mut M, deg: f32) {
    let (c, s) = cs(deg);
    for i in 0..4 {
        let a = m[i];
        let b = m[4 + i];
        m[i] = b * s + a * c;
        m[4 + i] = b * c - a * s;
    }
}

/// `mat4RotateX` 0x004243d0.
fn rot_x(m: &mut M, deg: f32) {
    let (c, s) = cs(deg);
    for i in 0..4 {
        let a = m[4 + i];
        let b = m[8 + i];
        m[4 + i] = b * s + a * c;
        m[8 + i] = b * c - a * s;
    }
}

/// `mat4RotateY` 0x004244f0.
fn rot_y(m: &mut M, deg: f32) {
    let (c, s) = cs(deg);
    for i in 0..4 {
        let a = m[i];
        let b = m[8 + i];
        m[i] = a * c - b * s;
        m[8 + i] = b * c + a * s;
    }
}

/// `mat4Translate` 0x00424a60 (and 0x00424990 with a vector).
fn translate(m: &mut M, x: f32, y: f32, z: f32) {
    for i in 0..4 {
        m[12 + i] = m[i] * x + m[4 + i] * y + m[8 + i] * z + m[12 + i];
    }
}

fn translate_v(m: &mut M, v: V3) {
    translate(m, v[0], v[1], v[2]);
}

/// `mat4Scale` 0x00424730: rows are only touched when the factor is not 1.
#[allow(clippy::float_cmp)]
fn scale(m: &mut M, x: f32, y: f32, z: f32) {
    if x != 1.0 {
        for i in 0..4 {
            m[i] *= x;
        }
    }
    if y != 1.0 {
        for i in 4..8 {
            m[i] *= y;
        }
    }
    if z != 1.0 {
        for i in 8..12 {
            m[i] *= z;
        }
    }
}

fn scale1(m: &mut M, s: f32) {
    scale(m, s, s, s);
}

/// 0x00412400: `this = r * this` (row-major).
fn premul(m: &mut M, r: &M) {
    let t = *m;
    for row in 0..4 {
        for c in 0..4 {
            m[4 * row + c] = r[4 * row] * t[c] + r[4 * row + 1] * t[4 + c] + r[4 * row + 2] * t[8 + c]
                + r[4 * row + 3] * t[12 + c];
        }
    }
}

/// `mat4RotateAxis` 0x004241b0 (0x00424170 takes the axis as a vector): the axis is normalised
/// (nothing happens for a zero axis), then the Rodrigues matrix is pre-multiplied.
fn rot_axis(m: &mut M, deg: f32, x: f32, y: f32, z: f32) {
    let len = sqrt_d(x * x + y * y + z * z);
    if len == 0.0 {
        return;
    }
    let (c, s) = cs(deg);
    let (x, y, z) = (x / len, y / len, z / len);
    let sz = s * z;
    let k = 1.0 - c;
    let xy = k * x * y;
    let xz = k * x * z;
    let yz = k * y * z;
    let r: M = [
        x * x * k + c,
        xy + sz,
        xz - s * y,
        0.0,
        xy - sz,
        y * y * k + c,
        yz + s * x,
        0.0,
        xz + s * y,
        yz - s * x,
        z * z * k + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    premul(m, &r);
}

/// 0x004248a0: transform a point with the perspective divide.
fn xform_pt(m: &M, v: V3) -> V3 {
    let (x, y, z) = (v[0], v[1], v[2]);
    let w = 1.0 / (m[3] * x + m[7] * y + m[11] * z + m[15]);
    [
        w * (m[4] * y + x * m[0] + m[8] * z + m[12]),
        w * (m[1] * x + m[5] * y + m[9] * z + m[13]),
        w * (m[2] * x + m[6] * y + m[10] * z + m[14]),
    ]
}

/// `lerpRepeat` 0x00411d40 from 0 toward 1: `n` times `v += (1 - v) * t`, the frame-rate
/// independent blend factor the smoothing uses.
fn blend(n: i32, t: f32) -> f32 {
    let mut v = 0.0f32;
    for _ in 0..n.max(0) {
        v = (1.0 - v) * t + v;
    }
    v
}

/// `turnTowards` 0x00423f70 (Server.exe 0x005306d0): `a + asin(clamp(sin(b)cos(a) -
/// cos(b)sin(a))) * k` in degrees. Duplicated from `cw_sim::physics` (crate-private there).
fn turn_towards(a: f32, b: f32, k: f32) -> f32 {
    let ra = f64::from((f64::from(a / 180.0f32) * PI_D) as f32);
    let rb = f64::from((f64::from(b / 180.0f32) * PI_D) as f32);
    let (ca, sa) = (cw_math::cos(ra), cw_math::sin(ra));
    let (cb, sb) = (cw_math::cos(rb), cw_math::sin(rb));
    let mut x = (sb * ca - cb * sa) as f32;
    if x > 1.0 {
        x = 1.0;
    } else if !(-1.0f32 <= x) {
        x = -1.0;
    }
    let s = cw_math::asin(f64::from(x)) as f32;
    ((f64::from(s) * (f64::from(k) * 180.0 / PI_D)) as f32) + a
}

/// `v += (target - v) * f` per component (0x004121c0, 0x00451510, 0x00412850).
fn approach(v: &mut V3, target: V3, f: f32) {
    for i in 0..3 {
        let d = target[i] - v[i];
        v[i] += d * f;
    }
}

fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn half(v: V3) -> V3 {
    [v[0] * 0.5, v[1] * 0.5, v[2] * 0.5]
}

/// `(float)((double)(x * 180.0f) / PI)`, the degree conversion the draw code uses.
fn deg_d(x: f32) -> f32 {
    (f64::from(x) / PI_D) as f32
}

/// 16.16 fixed to float, 0x004120f0: `(float)i64 * (1/65536)`.
fn fix2f(v: i64) -> f32 {
    (v as f32) * 1.525_878_9e-5
}

/// `float * 65536` truncated to i64 (0x00412220 / 0x004122e0 through `__ftol2`).
fn f2fix(f: f32) -> i64 {
    (f * 65536.0f32) as i64
}

// ------------------------------------------------------------------------------------------
// Public types.
// ------------------------------------------------------------------------------------------

/// Where the model of a part comes from. The original resolves `Model*` pointers; the port
/// uses indices into the model vector (`GameController+0x300`), which the caller maps to
/// meshes.
pub trait ModelSource {
    /// `0x004ec400` (6.6 KB, not ported here): the model of an item block (0x118 bytes), or
    /// `None` for an empty slot or an item without a model.
    fn item_model(&self, item: &[u8]) -> Option<u32>;
    /// The voxel size of a model (`Model+0x44`, `+0x48`, `+0x4c`: x, y, z).
    fn model_size(&self, model: u32) -> [i32; 3];
    /// Length of the model vector; appearance model ids and the fixed indices (0x347, 0x348,
    /// 0x9f6, 0xa06) are bounds-checked against it (0x004120c0).
    fn model_count(&self) -> usize;
}

/// The walk-cycle fields of the creature outside the entity block: `creature+0x1188` (the
/// walk blend, 0..1, how much the walk animation shows) and `creature+0x118c` (the walk
/// phase, radians). The world tick advances them ([`advance_walk_cycle`]); the server port
/// left them out because nothing but this function reads them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WalkCycle {
    /// `creature+0x1188`.
    pub blend: f32,
    /// `creature+0x118c`.
    pub phase: f32,
}

/// Everything the pose reads besides the entity block, with where the original takes it.
pub struct PoseInputs<'a> {
    /// The model vector (`param_5`, `GameController+0x300`) and `itemModel` 0x004ec400.
    pub models: &'a dyn ModelSource,
    /// Frame time in ms (`param_6`, `GameController+0x8006e8`): the iteration count of every
    /// smoothing `lerpRepeat`.
    pub dt_ms: i32,
    /// The shader colour (`param_7`); parts drawn with another colour say so in
    /// [`PartTransform::color`].
    pub color: [f32; 4],
    /// Smoothed render position `creature+0x1350` (3 x i64, 16.16 fixed), which the shared
    /// tick moves toward the entity position.
    pub render_pos: [i64; 3],
    /// Smoothed render rotation `creature+0x1374` (pitch, roll, yaw in degrees).
    pub render_rot: V3,
    /// `*param_8`, `*param_9` (`GameController+0x1d8`, `+0x1e0`): added to the render position x
    /// and y before the conversion to float (the camera-relative origin).
    pub origin: [i64; 2],
    /// Step smoothing z offset `creature+0x1180`, subtracted from the render position z.
    pub step_z: f32,
    /// `creature+0x1188` / `creature+0x118c`.
    pub walk: WalkCycle,
    /// Mount id `creature+0x11c0` (i64) is non-zero: the creature rides another one.
    pub mounted: bool,
    /// `param_11`: the creature's pet (the lookup of the id at `creature+0x11c8`), read by the
    /// riding pose (animation 0x3a) for its scale and type.
    pub pet: Option<&'a EntityData>,
    /// Combo counter `creature+0x1314`: `(n / 2) % 3` picks the spin variants of some attacks.
    pub combo: i32,
    /// Charge `creature+0x13b4` (`CreatureState::charge`), read by animation 0x42.
    pub charge: f32,
    /// Guard `creature+0x1190` and "has buff 0xc" for the skill timing tables
    /// (`skillWindup` 0x0043caa0, `skillDuration` 0x00447310, `skillTotalTime` 0x0043d1a0,
    /// `skillRecovery` 0x00444270), which are `cw_sim::skills` here.
    pub guard: f32,
    pub haste: bool,
    /// `param_10`: bit 0 head, 1 hair, 2 shoulder armour, 3 body, 4 left hand, 5 right hand,
    /// 6 both weapons, 7 left foot, 8 right foot are not drawn (first person, previews).
    pub hide: u32,
    /// Animation mode override (`creature+0x14a0` as the preview widgets set it); `None` takes
    /// [`anim_mode_for`] of the entity as `GameController::render` does.
    pub anim_mode: Option<i32>,
    /// Hit flash `creature+0x1184` (from `cw_sim`'s creature state). Not read by 0x004128f0:
    /// `GameController::render` applies it to the colour it passes (`param_7`); kept here so
    /// the caller has one place for it.
    pub hit_flash: f32,
}

/// Which part a transform belongs to, in the original's draw order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartSlot {
    /// Glider (equipment slot 11 while gliding, flag 0x10), 0x0041f733.
    Glider,
    /// Boat (equipment slot 11 in mode 0x6b), 0x0041f8c6.
    Boat,
    /// Model 0xa06 spinning between the hands in animation 0x48, 0x0041fa97.
    SpinProp,
    /// Body: chest armour (slot 2) or the appearance body model, matrix `+0x16c`.
    Body,
    /// Model 0x9f6 on the body while the show-patch timer (`0x12c`) is negative.
    BodyPatch,
    /// Neck item (slot 1), matrix `+0x3ec`.
    Neck,
    /// Shoulder armour (slot 5), matrix `+0x3ac`.
    ShoulderArmor,
    /// Tail (appearance `0x86`), matrix `+0x52c`.
    Tail,
    /// Head (appearance `0x7c`), matrix `+0x1ac`.
    Head,
    /// Hair (appearance `0x7e`, hair colour), matrix `+0x1ec`.
    Hair,
    /// Upper arms (appearance `0x88`) at the two elbow points, matrices `+0x22c`, `+0x26c`.
    UpperArmLeft,
    UpperArmRight,
    /// Consumable being used (modes 0x50/0x51), between the hands.
    Consumable,
    /// Pet item (slot 12) in mode 0x52, between the hands.
    PetItem,
    /// Model 0x347 (arrows) on the back when slot 6 / slot 7 holds a bow or crossbow.
    QuiverLeft,
    QuiverRight,
    /// Weapon in slot 7 (right hand or on the back), matrix `+0x2ac`.
    WeaponRight,
    /// Weapon or shield in slot 6 (left hand or on the back), matrix `+0x2ec`.
    WeaponLeft,
    /// Slot 6 item of weapon sub type 0xe, drawn on the back instead of in the hand.
    BackItem,
    /// Hands (hand armour slot 4 or the appearance hand), matrices `+0x32c`, `+0x36c` (the
    /// right one mirrored).
    HandLeft,
    /// Lamp (slot 10, flag 0x200) at the left hand.
    Lamp,
    HandRight,
    /// Wings (appearance `0x8a`), matrices `+0x42c`, `+0x46c`.
    WingLeft,
    WingRight,
    /// Feet (foot armour slot 3 or the appearance foot), matrices `+0x4ac`, `+0x4ec`.
    FootLeft,
    FootRight,
}

/// One drawn part: `CubeShader` world matrix, model, and the shader state at the draw.
#[derive(Debug, Clone, PartialEq)]
pub struct PartTransform {
    pub slot: PartSlot,
    /// Index into the model vector.
    pub model: u32,
    /// The world matrix passed to 0x004482a0 (row-vector matrix in glam's storage, see the
    /// module doc). The model is drawn with its voxel origin at the model corner; the matrix
    /// already contains the centring translation.
    pub matrix: Mat4,
    /// Shader colour at the draw (0x00448280): the `color` input, the hair colour, the charged
    /// weapon pulse or white for the lamp.
    pub color: [f32; 4],
    /// The value of 0x00448fe0 at the draw: 1 for armour of material 1, 0xb, 0xc or 0x16
    /// (0x004c7be0), else 0. Probably the metal shine.
    pub shine: f32,
    /// The right hand is scaled by -1 in x: the original flips the cull mode (render state
    /// 0x16 = 3) around its draw.
    pub mirrored: bool,
}

/// The output of [`build_pose`].
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    /// The parts in draw order.
    pub parts: Vec<PartTransform>,
    /// The root matrix `L_14e0` (render position, roll, render rotation, lean, scale).
    pub root: Mat4,
    /// The four weapon trails (`+0x56c` right tip, `+0x62c` right base, `+0x6ec` left tip,
    /// `+0x7ac` left base), 16 points each, newest first.
    pub trails: [[V3; 16]; 4],
    /// `creature+0x1188` after the call: animation 1 (the charge) writes 0 into it during its
    /// first 500 ms (0x004176d5); the caller stores it back into its walk-cycle state.
    pub walk_blend: f32,
}

/// The animation state object at `creature+0x1498` (smoothed pose parameters and the per-part
/// matrices the function keeps between frames). Zero is the constructed state.
#[derive(Debug, Clone, PartialEq)]
pub struct PoseState {
    /// `+0x04` animation time, `+0x08` animation mode (reset to 0 by some animations when they
    /// end, as the original does).
    pub anim_time: i32,
    pub anim_mode: i32,
    /// `+0x0c` left-hand offset, `+0x18` right-hand offset, `+0x24` left-arm rotation,
    /// `+0x30` right-arm rotation (smoothed toward the animation's targets).
    pub off_l: V3,
    pub off_r: V3,
    pub rot_l: V3,
    pub rot_r: V3,
    /// `+0x54` body rotation.
    pub body_rot: V3,
    /// `+0x60` whole-figure lean (turned toward the target with `turnTowards`).
    pub lean: V3,
    /// `+0x6c` body offset, `+0x78` head offset.
    pub body_off: V3,
    pub head_off: V3,
    /// `+0x84` head rotation, `+0x90` tail rotation (both `turnTowards`).
    pub head_rot: V3,
    pub tail_rot: V3,
    /// `+0x9c`: upper-body twist (only y is used).
    pub twist: V3,
    /// `+0xa8`: yaw lean from aiming against the movement.
    pub yaw_lean: f32,
    /// `+0xac`: root offset.
    pub root_off: V3,
    /// `+0xb8`: swimming depth offset.
    pub swim_z: f32,
    /// `+0x12c`: the upper-body frame of the previous frame (animation 0x48 reads it before it
    /// is rebuilt).
    pub upper_frame: [f32; 16],
    /// `+0x56c`, `+0x62c`, `+0x6ec`, `+0x7ac`.
    pub trails: [[V3; 16]; 4],
    /// `+0x86c`: the normalised ray direction (entity `0x150`), smoothed while the entity mode
    /// is 0x1c, zero otherwise. Not read by this function.
    pub aim_dir: V3,
    /// `+0x880`, `+0x888`: `*param_8`, `*param_9` of this frame.
    pub origin: [i64; 2],
}

impl Default for PoseState {
    fn default() -> Self {
        PoseState {
            anim_time: 0,
            anim_mode: 0,
            off_l: [0.0; 3],
            off_r: [0.0; 3],
            rot_l: [0.0; 3],
            rot_r: [0.0; 3],
            body_rot: [0.0; 3],
            lean: [0.0; 3],
            body_off: [0.0; 3],
            head_off: [0.0; 3],
            head_rot: [0.0; 3],
            tail_rot: [0.0; 3],
            twist: [0.0; 3],
            yaw_lean: 0.0,
            root_off: [0.0; 3],
            swim_z: 0.0,
            upper_frame: [0.0; 16],
            trails: [[[0.0; 3]; 16]; 4],
            aim_dir: [0.0; 3],
            origin: [0; 2],
        }
    }
}

/// The animation mode `GameController::render` stores at `creature+0x14a0` for an entity
/// (0x004b4ef6..0x004b5338): a table from the entity mode (`0x58`), with three special cases,
/// and 0x38 while the timer at `0x120` runs.
pub fn anim_mode_for(e: &EntityData) -> i32 {
    let m = i32::from(e.0[off::MODE]);
    let a = match m {
        1 => 0x10,
        2 => 0xf,
        3 | 9 | 0x13 | 0x3e => 0x12,
        4 | 0x12 => 0x11,
        5 => 0x21,
        6 => 0x14,
        7 => 0x13,
        8 => 9,
        0xa => 0xa,
        0xb => 0x1c,
        0xc => 0x1d,
        0xd => 0x1e,
        0xe => 0x1f,
        0xf => 0x20,
        0x10 | 0x11 => 0x23,
        0x14 | 0x15 => 0x32,
        0x16 | 0x17 => 0x15,
        0x18 | 0x19 | 0x32 | 0x37 => 0x16,
        0x1a => 0x42,
        0x1b => 0x43,
        0x1c => 0xb,
        0x1e..=0x22 | 0x31 | 0x57..=0x5a | 0x5c | 0x69 => 0x3d,
        0x23 => 0x3e,
        0x24 => 0x1b,
        0x25 | 0x2b => 0x17,
        0x26 | 0x2c..=0x2e | 0x5e | 0x5f => 0x1a,
        0x27 | 0x29 => 0x18,
        0x28 | 0x2a => 0x19,
        0x2f | 0x36 => 1,
        0x30 => {
            // 0x004b51ca
            let w = off::equip(7);
            let s = off::equip(6);
            if e.0[w] == 3 && (e.0[w + 1] == 6 || e.0[w + 1] == 7) {
                0x16
            } else if e.0[s] == 3 {
                if e.0[s + 1] == 0xd { 9 } else { 0x23 }
            } else {
                0x22
            }
        }
        0x33 => 0x46,
        0x39 | 0x3c => 2,
        0x3a => 3,
        0x3b => 4,
        0x3d | 0x42 => 0x31,
        0x3f => 6,
        0x40 => 5,
        0x41 => 0x30,
        0x43 => 0x25,
        0x44 => 0x26,
        0x45 => 0x27,
        0x46 => 0x28,
        0x47 | 0x48 => 0x29,
        0x49 => 0x2a,
        0x4a => 0x2b,
        0x4b => 0x2c,
        0x4c => 0x2d,
        0x4d => 0x2e,
        0x4e => 0x2f,
        0x4f => 0x36,
        0x50 => 0x33,
        0x51 => 0x34,
        0x52 => 0x35,
        0x53 => 0x39,
        0x54 => 0x37,
        0x56 => 0x3c,
        0x5b => 0x3f,
        0x5d => {
            // 0x004b52dc
            if e.0[off::equip(6)] == 0 { 0x31 } else { 0x23 }
        }
        0x60 => 0x41,
        0x62 => 8,
        0x65 | 0x67 => 0x44,
        0x68 => 0x45,
        0x6a => 0x3a,
        0x6b => 0x3b,
        0x6c => 0x47,
        0x6d => 0x48,
        0x6e => 0x49,
        _ => 0,
    };
    // 0x004b532f
    if ri(e, off::SLOWED) > 0 { 0x38 } else { a }
}

// ------------------------------------------------------------------------------------------
// The function.
// ------------------------------------------------------------------------------------------

/// The locals of 0x004128f0 that carry the pose from the prologue through the animation switch
/// to the draws. Field comments give the stack slot (`L_xxxx` = Ghidra's `local_xxxx`).
struct Figure<'a> {
    e: &'a EntityData,
    inp: &'a PoseInputs<'a>,
    st: &'a mut PoseState,
    parts: Vec<PartTransform>,
    /// Shader colour and shine as the draw sequence leaves them (the original sets them on the
    /// shader and some parts inherit the previous part's value).
    cur_color: [f32; 4],
    cur_shine: f32,

    // Model slots `this+0xbc..+0xe8`.
    m_head: Option<u32>,
    m_hair: Option<u32>,
    m_body: Option<u32>,
    m_arm: Option<u32>,
    m_hands: Option<u32>,
    m_wings: Option<u32>,
    m_tail: Option<u32>,
    m_feet: Option<u32>,
    m_weapon_r: Option<u32>,
    m_weapon_l: Option<u32>,
    m_shoulder: Option<u32>,
    m_neck: Option<u32>,
    /// `L_16ec`: slot 6 item of sub type 0xe, drawn on the back.
    m_back: Option<u32>,

    // Appearance scales.
    head_scale: f32,     // L_16d8
    body_scale: f32,     // L_16bc
    hand_scale: f32,     // L_1698
    foot_scale: f32,     // L_168c
    arm_scale: f32,      // L_16f8
    weapon_scale: f32,   // L_1694
    weapon_scale_l: f32, // L_1690
    tail_scale: f32,     // L_169c
    shoulder_scale: f32, // L_1684

    /// `L_172c/L_1724/L_1718`: the appearance body offset (the target of animation 0x29).
    body0: V3,
    /// `L_1510`: body position.
    body: V3,
    /// `L_1558`: head position.
    head: V3,
    /// `L_1528`: tail position; `L_1708/L_16ac/L_16a8` its initial value.
    tail: V3,
    tail0: V3,
    /// `L_15e8`, `L_15c4`: left and right hand positions.
    hand_l: V3,
    hand_r: V3,
    /// `L_15b8`, `L_1504`: the upper-arm (elbow) points.
    elbow_l: V3,
    elbow_r: V3,
    /// `L_1564`, `L_1570`: foot positions.
    foot_l: V3,
    foot_r: V3,
    /// `L_1534`, `L_151c`: arm rotations (appearance arm pitch/roll/yaw in animation 0).
    arm_l: V3,
    arm_r: V3,
    /// `L_14ec`, `L_14f8`: extra weapon rotations (never written in this build).
    wpn_l: V3,
    wpn_r: V3,

    // Targets of the smoothed state.
    t_rot_r: V3,    // L_160c -> +0x30
    t_rot_l: V3,    // L_15dc -> +0x24
    t_off_r: V3,    // L_15f4 -> +0x18
    t_off_l: V3,    // L_15d0 -> +0x0c
    t_body_rot: V3, // L_1600 -> +0x54
    t_lean: V3,     // L_15ac -> +0x60
    t_head_rot: V3, // L_15a0 -> +0x84
    t_tail_rot: V3, // L_1540 -> +0x90
    t_twist: V3,    // L_157c -> +0x9c
    t_body_off: V3, // L_154c -> +0x6c
    t_head_off: V3, // L_1588 -> +0x78
    t_root_off: V3, // L_1594 -> +0xac

    /// `L_1714`: `sin(phase + pi/2)` of the walk cycle.
    wave: f32,
    /// `L_16fc`: walk swing angle (radians), `L_16a0` its value before the late reductions.
    swing: f32,
    swing_raw: f32,
    /// `L_16d9`: running (sprint flag with speed, or mode 0x30).
    running: bool,
    /// `L_16e0`: swim depth target (-> +0xb8).
    swim_z: f32,
    /// `L_16cc`: yaw lean target (-> +0xa8).
    yaw_lean: f32,
    /// `L_16b0`, `L_16b4`: leg lift of the left and right foot.
    leg_l: f32,
    leg_r: f32,
    /// `L_1710`: foot yaw, `L_16d0`/`L_16e4` foot pitch left/right, `L_16d4` extra foot pitch.
    foot_yaw: f32,
    foot_ang_l: f32,
    foot_ang_r: f32,
    feet_lift: f32,
    /// `L_1719`: the left foot is turned 60 degrees instead of pitched.
    foot_flag: bool,
    /// `L_1730`: smoothing rate of the pose state (`lerpRepeat` factor per ms).
    rate: f32,
    /// `L_1708` = |swing| and `L_172c` = 4 |swing| (16 |swing| in mode 0x4f): the body bob.
    bob: f32,
    bob4: f32,
    /// `L_14e0`, kept for the trail code.
    root_cache: M,
    /// `creature+0x1188` as this call sees it: animation 1 clears it before the draws read it.
    walk_blend: f32,
}

/// `Cube.exe 0x004128f0` without persistent state: a fresh [`PoseState`] (as after
/// construction), so the smoothed parameters start from zero and move toward the animation's
/// targets by `dt_ms`. `time_ms` is the animation clock (`creature+0x149c`, which
/// `GameController::render` sets to the entity's mode time `0x5c` every frame).
pub fn build_pose(e: &EntityData, time_ms: i32, extra: &PoseInputs) -> Pose {
    let mut st = PoseState::default();
    build_pose_with(e, time_ms, extra, &mut st)
}

/// `Cube.exe 0x004128f0` with the persistent animation object (`creature+0x1498`).
pub fn build_pose_with(e: &EntityData, time_ms: i32, extra: &PoseInputs, st: &mut PoseState) -> Pose {
    st.anim_time = time_ms;
    st.origin = extra.origin;
    st.anim_mode = extra.anim_mode.unwrap_or_else(|| anim_mode_for(e));
    let mut f = Figure::new(e, extra, st);
    f.models();
    f.prologue();
    f.anim_switch();
    f.smooth();
    let (root, base) = f.frames();
    f.draw(&root, &base);
    Pose { parts: f.parts, root: Mat4::from_cols_array(&root), trails: f.st.trails, walk_blend: f.walk_blend }
}

impl<'a> Figure<'a> {
    fn new(e: &'a EntityData, inp: &'a PoseInputs<'a>, st: &'a mut PoseState) -> Self {
        let z = [0.0f32; 3];
        Figure {
            e,
            inp,
            st,
            parts: Vec::new(),
            cur_color: inp.color,
            cur_shine: 0.0,
            m_head: None,
            m_hair: None,
            m_body: None,
            m_arm: None,
            m_hands: None,
            m_wings: None,
            m_tail: None,
            m_feet: None,
            m_weapon_r: None,
            m_weapon_l: None,
            m_shoulder: None,
            m_neck: None,
            m_back: None,
            head_scale: 0.0,
            body_scale: 0.0,
            hand_scale: 0.0,
            foot_scale: 0.0,
            arm_scale: 0.0,
            weapon_scale: 0.0,
            weapon_scale_l: 0.0,
            tail_scale: 0.0,
            shoulder_scale: 0.0,
            body0: z,
            body: z,
            head: z,
            tail: z,
            tail0: z,
            hand_l: z,
            hand_r: z,
            elbow_l: z,
            elbow_r: z,
            foot_l: z,
            foot_r: z,
            arm_l: z,
            arm_r: z,
            wpn_l: z,
            wpn_r: z,
            t_rot_r: z,
            t_rot_l: z,
            t_off_r: z,
            t_off_l: z,
            t_body_rot: z,
            t_lean: z,
            t_head_rot: z,
            t_tail_rot: z,
            t_twist: z,
            t_body_off: z,
            t_head_off: z,
            t_root_off: z,
            wave: 0.0,
            swing: 0.0,
            swing_raw: 0.0,
            running: false,
            swim_z: 0.0,
            yaw_lean: 0.0,
            leg_l: 0.0,
            leg_r: 0.0,
            foot_yaw: 0.0,
            foot_ang_l: 0.0,
            foot_ang_r: 0.0,
            feet_lift: 0.0,
            foot_flag: false,
            rate: 0.0,
            bob: 0.0,
            bob4: 0.0,
            root_cache: IDENTITY,
            walk_blend: inp.walk.blend,
        }
    }

    // --- small accessors ------------------------------------------------------------------

    fn mode(&self) -> u8 {
        self.e.0[off::MODE]
    }
    fn mode_time(&self) -> i32 {
        ri(self.e, off::MODE_TIME)
    }
    fn flags(&self) -> u16 {
        ru16(self.e, off::FLAGS)
    }
    fn app_flags(&self) -> u16 {
        ru16(self.e, off::APP_FLAGS)
    }
    fn phys(&self) -> u32 {
        ri(self.e, off::PHYS) as u32
    }
    fn scale_v(&self) -> V3 {
        rv3(self.e, off::SCALE)
    }
    /// Equipment slot `slot` type and sub type.
    fn eq(&self, slot: usize) -> (u8, u8) {
        let o = off::equip(slot);
        (self.e.0[o], self.e.0[o + 1])
    }
    /// `skillWindup(creature, -1)` 0x0043caa0.
    fn wu(&self) -> i32 {
        cw_sim::skills::skill_windup(self.e, self.inp.guard, self.inp.haste, -1)
    }
    /// `skillDuration(creature, -1)` 0x00447310.
    fn du(&self) -> i32 {
        cw_sim::skills::skill_duration(self.e, self.inp.guard, self.inp.haste, -1)
    }
    /// `skillTotalTime(creature)` 0x0043d1a0.
    fn tt(&self) -> i32 {
        cw_sim::skills::skill_total_time(self.e, self.inp.guard, self.inp.haste)
    }
    /// `skillRecovery(creature, -1)` 0x00444270: the third part of `skillTotalTime`.
    fn rc(&self) -> i32 {
        self.tt() - self.wu() - self.du()
    }
    /// `consumableDuration` 0x004c6b80 of the consumable slot: 3000 ms for type 1 sub type 1,
    /// 10000 ms for other type 1 items, 0 otherwise.
    fn consumable_duration(&self) -> i32 {
        let it = item(self.e, off::CONSUMABLE);
        if it[0] != 1 {
            0
        } else if it[1] == 1 {
            3000
        } else {
            10000
        }
    }
    /// `cube::Creature::hasTwoHandedWeapon` 0x00444230.
    fn two_handed(&self) -> bool {
        let (t, s) = self.eq(7);
        t == 3 && matches!(s, 0xf | 0x10 | 0x11 | 5 | 0xa | 0xb | 0x12 | 8 | 6 | 7)
    }
    /// 0x004120c0 on the model vector.
    fn fixed_model(&self, index: u32) -> Option<u32> {
        ((index as usize) < self.inp.models.model_count()).then_some(index)
    }
    fn app_model(&self, o: usize) -> Option<u32> {
        let id = ri16(self.e, o);
        if id < 0 || id as usize >= self.inp.models.model_count() {
            None
        } else {
            Some(id as u32)
        }
    }
    fn item_model(&self, o: usize) -> Option<u32> {
        self.inp.models.item_model(item(self.e, o))
    }
    fn size(&self, m: u32) -> [f32; 3] {
        let s = self.inp.models.model_size(m);
        [s[0] as f32, s[1] as f32, s[2] as f32]
    }
    fn size_i(&self, m: u32) -> [i32; 3] {
        self.inp.models.model_size(m)
    }
    /// The usual centring `translate(sx * -0.5, sy * -0.5, sz * -0.5)`.
    fn center(&self, m: &mut M, model: u32) {
        let s = self.size(model);
        translate(m, s[0] * -0.5, s[1] * -0.5, s[2] * -0.5);
    }
    /// 0x004c7be0 of an equipment item: 1 for a non-empty item of material 1, 0xb, 0xc, 0x16.
    fn shine_of(&self, o: usize) -> f32 {
        let it = item(self.e, o);
        if it[0] != 0 && matches!(it[0xd], 1 | 0xb | 0xc | 0x16) { 1.0 } else { 0.0 }
    }
    fn emit(&mut self, slot: PartSlot, model: u32, m: &M) {
        self.emit_with(slot, model, m, false);
    }
    fn emit_with(&mut self, slot: PartSlot, model: u32, m: &M, mirrored: bool) {
        self.parts.push(PartTransform {
            slot,
            model,
            matrix: Mat4::from_cols_array(m),
            color: self.cur_color,
            shine: self.cur_shine,
            mirrored,
        });
    }

    /// 0x004128f0..0x00412bc6: the model of each slot. Armour replaces the appearance model of
    /// the hands, feet and body; the other items have no fallback.
    fn models(&mut self) {
        self.m_hands = self.item_model(off::equip(4));
        self.m_feet = self.item_model(off::equip(3));
        self.m_shoulder = self.item_model(off::equip(5));
        self.m_neck = self.item_model(off::equip(1));
        self.m_weapon_r = self.item_model(off::equip(7));
        self.m_weapon_l = self.item_model(off::equip(6));
        self.m_body = self.item_model(off::equip(2));
        // 0x00412a37: head and hair were just cleared, so the appearance always fills them.
        self.m_head = self.app_model(off::HEAD_MODEL);
        self.m_hair = self.app_model(off::HAIR_MODEL);
        if self.m_hands.is_none() {
            self.m_hands = self.app_model(off::HAND_MODEL);
        }
        if self.m_feet.is_none() {
            self.m_feet = self.app_model(off::FOOT_MODEL);
        }
        if self.m_body.is_none() {
            self.m_body = self.app_model(off::BODY_MODEL);
        }
        self.m_tail = self.app_model(off::TAIL_MODEL);
        self.m_wings = self.app_model(off::WING_MODEL);
        self.m_arm = self.app_model(off::SHOULDER2_MODEL);
    }

    /// 0x00412bc6..0x00413af6.
    fn prologue(&mut self) {
        let e = self.e;
        self.head_scale = rf(e, off::HEAD_SCALE);
        self.body_scale = rf(e, off::BODY_SCALE);
        self.hand_scale = rf(e, off::HAND_SCALE);
        self.foot_scale = rf(e, off::FOOT_SCALE);
        self.arm_scale = rf(e, off::SHOULDER2_SCALE);
        self.weapon_scale = rf(e, off::WEAPON_SCALE);
        self.weapon_scale_l = self.weapon_scale;
        self.tail_scale = rf(e, off::TAIL_SCALE);
        self.shoulder_scale = rf(e, off::SHOULDER_SCALE);
        self.body0 = rv3(e, off::BODY_OFFSET);
        self.body = self.body0;
        self.head = rv3(e, off::HEAD_OFFSET);
        self.wpn_r = [0.0; 3];
        self.wpn_l = [0.0; 3];
        self.arm_l = [0.0; 3];
        self.arm_r = [0.0; 3];
        if self.st.anim_mode == 0 {
            // 0x00412d61: the appearance arm pitch/roll/yaw, mirrored for the left arm.
            let a = rv3(e, off::ARM_PITCH);
            self.arm_l = [a[0], -a[1], -a[2]];
            self.arm_r = a;
        }

        // 0x00412de3: walk swing.
        let walk = self.inp.walk;
        let ph = (f64::from(walk.phase) + std::f64::consts::FRAC_PI_2) as f32;
        self.wave = cw_math::sin(f64::from(ph)) as f32;
        let mode = self.mode();
        let mut sw = (f64::from(walk.blend) * 2.199_114_820_062_152 * f64::from(self.wave)) as f32;
        if mode == 0x4f {
            sw *= 0.5;
        }
        if mode == 0x6a || mode == 0x6b {
            sw *= 0.1;
        }
        let flags = self.flags();
        let phys = self.phys();
        let acc = rv3(e, off::ACCEL);
        let vel = rv3(e, off::VEL);
        let sprint = (flags & 1 == 0 || phys & 4 == 0)
            && flags & 0x40 != 0
            && acc[0] * acc[0] + acc[1] * acc[1] + acc[2] * acc[2] > 0.1
            && vel[1] * vel[1] + vel[0] * vel[0] > 0.1;
        if sprint || mode == 0x30 {
            sw *= 1.25;
            self.running = true;
        } else {
            self.running = false;
        }
        let app = self.app_flags();
        if app & 2 != 0 && phys & 1 == 0 {
            sw = 0.0;
        }
        if app & 1 != 0 {
            sw *= 0.75;
        }
        self.swing_raw = sw;
        if self.eq(7).1 == 5 && mode != 0 {
            sw *= 0.25;
        }
        let anim = self.st.anim_mode;
        if (anim == 0x33 || anim == 0x34) && self.mode_time() < self.consumable_duration() {
            sw *= 0.1;
        }
        if anim == 0x35 {
            sw *= 0.1;
        }
        if matches!(mode, 0x16 | 0x24 | 0x3f | 8 | 0x1c) || matches!(anim, 2 | 3 | 0x30 | 0x31 | 0x25) {
            sw *= 0.5;
        }
        if flags & 0x10 != 0 {
            sw = 0.0;
        }
        self.swing = sw;

        // 0x00412ff3: animation targets start at zero (the struct is built zeroed).
        self.swim_z = 0.0;
        self.yaw_lean = 0.0;
        // 0x00413190: `cube::Creature::isSwimmingDown` 0x00444650.
        if swimming_down(e) {
            self.swim_z = self.scale_v()[2] * 0.5;
        }
        // 0x004131b1: aiming across the movement leans the body (yaw).
        if flags & 4 != 0 {
            let a2 = acc[1] * acc[1] + acc[0] * acc[0];
            if a2 > 5.0 {
                let ray = rv3(e, off::RAY);
                let r2 = ray[1] * ray[1] + ray[0] * ray[0];
                if r2 > 0.0 {
                    let s = 1.0 / sqrt_d(a2);
                    let ax = acc[0] * s;
                    let ay = acc[1] * s;
                    let s2 = 1.0 / sqrt_d(r2);
                    let ry = ray[1] * s2;
                    let rx = ray[0] * s2;
                    let mut v = rx * ay - ry * ax;
                    if -1.0 > v {
                        v = -1.0;
                    } else if v > 1.0 {
                        v = 1.0;
                    }
                    let ang = (f64::from(cw_math::asin(f64::from(v)) as f32) * 57.295_779_513_082_32) as f32;
                    self.yaw_lean = ang;
                    if -0.2 > ry * ay + rx * ax {
                        self.yaw_lean = -ang;
                    }
                }
            }
        }

        // 0x00413351: foot swing and leg lift.
        self.foot_yaw = (f64::from(self.swing_raw * 0.0 * 80.0) / PI_D) as f32;
        let t = (f64::from(self.swing_raw * 180.0) / PI_D) as f32;
        self.foot_ang_l = t;
        self.foot_ang_r = -t;
        if self.foot_ang_l > 0.0 {
            self.foot_ang_l *= 1.5;
        }
        if self.foot_ang_r > 0.0 {
            self.foot_ang_r *= 1.5;
        }
        let leg = walk.blend * 4.0;
        self.leg_l = leg * self.wave;
        self.leg_r = (1.0 - self.wave) * leg;
        if app & 2 != 0 && phys & 1 == 0 {
            self.leg_l = 0.0;
            self.leg_r = 0.0;
        }
        if 0.0 > self.foot_ang_l {
            self.foot_ang_l *= 1.5;
        }
        if 0.0 > self.foot_ang_r {
            self.foot_ang_r *= 1.5;
        }
        if self.inp.mounted && e.0[off::HOSTILE] != 5 {
            self.foot_yaw = 10.0;
            self.foot_ang_l = 20.0;
            self.foot_ang_r = 20.0;
        }
        if matches!(anim, 0x39 | 0x34 | 0x3a | 0x3b) {
            self.foot_yaw = 10.0;
            self.foot_ang_l = 40.0;
            self.foot_ang_r = 40.0;
        }
        if anim == 0x37 {
            self.foot_ang_l = -10.0;
            self.foot_ang_r = -10.0;
        }

        // 0x004134e3: hands.
        let ho = rv3(e, off::HAND_OFFSET);
        let (mut hl, mut hr);
        let (zl, zr);
        if app & 1 != 0 {
            hl = [-ho[0], ho[1], ho[2]];
            hr = ho;
            zl = ho[2];
            zr = ho[2];
        } else {
            let r = ho[0] * walk.blend + ho[0];
            let c1 = cw_math::cos(f64::from(sw)) as f32;
            let x = -(c1 * r);
            let s1 = cw_math::sin(f64::from(sw)) as f32;
            let sr = s1 * r;
            let z = (ho[2] - 4.1) + walk.blend * 6.0;
            hl = [x, ho[1] + -sr, z];
            let z2 = walk.blend * 6.0 + (ho[2] - 4.1);
            hr = [-x, ho[1] + sr, z2];
            zl = z;
            zr = z2;
        }
        // 0x0041369e
        hl[1] = (2.1 - walk.blend * 5.0) + hl[1];
        hr[1] = (2.1 - walk.blend * 5.0) + hr[1];
        if self.inp.mounted {
            hl[1] += 2.0;
            hr[1] += 2.0;
            hl[0] *= 0.5;
            hr[0] *= 0.5;
        }
        self.hand_l = hl;
        self.hand_r = hr;
        // 0x00413743: elbows halfway between the hands and the shoulders.
        self.elbow_l = [(ho[0] * -0.3 + hl[0]) * 0.5, ((hl[1] - 8.0) * 0.5) * 0.5, (zl + (ho[2] + 12.0)) * 0.5];
        self.elbow_r = [
            (hr[0] + ho[0] * 0.3) * 0.5,
            (f64::from((hr[1] - 8.0) * 0.5) * 0.5) as f32,
            (zr + (ho[2] + 12.0)) * 0.5,
        ];
        // 0x00413825: feet and tail.
        let fo = rv3(e, off::FOOT_OFFSET);
        self.foot_l = [-fo[0], fo[1], fo[2]];
        self.foot_r = fo;
        self.tail0 = rv3(e, off::TAIL_OFFSET);
        self.tail = self.tail0;
        // 0x00413896: a slot 6 weapon of sub type 0xe goes on the back instead of the hand.
        let (t6, s6) = self.eq(6);
        if t6 == 3 && s6 == 0xe {
            self.m_back = self.item_model(off::equip(6));
            self.m_weapon_l = None;
        }
        self.feet_lift = 0.0;
        self.foot_flag = false;
        self.rate = 0.01;
        // 0x0041394c: flag 0x400 (sitting).
        if flags & 0x400 != 0 {
            let sc = self.scale_v();
            self.t_lean = [-70.0, 0.0, 0.0];
            self.t_head_off[0] = 0.0;
            self.t_head_rot[0] = 70.0;
            self.t_off_l = [0.0, 3.0, 1.0];
            self.t_root_off[2] = (sc[1] - sc[2]) * 0.5;
            self.t_off_r = [0.0, 3.0, 1.0];
            self.t_head_off[1] = -1.0;
            self.t_head_off[2] = 0.0;
            if self.eq(7).1 == 7 {
                self.t_off_r[1] = 11.0;
            }
            self.foot_ang_l = -20.0;
            self.foot_ang_r = -20.0;
            if mode != 0 && self.mode_time() < self.tt() {
                self.t_off_l[0] += 0.0;
                self.t_off_l[1] -= 5.0;
                self.t_off_l[2] += 6.0;
                self.t_off_r[0] += 0.0;
                self.t_off_r[1] -= 5.0;
                self.t_off_r[2] += 6.0;
            }
        }
    }
}

/// `cube::Creature::isSwimmingDown` 0x00444650 (Server.exe 0x0040f6e0; duplicated from
/// `cw_sim::physics`, private there).
fn swimming_down(e: &EntityData) -> bool {
    ru16(e, off::FLAGS) & 0x10 != 0
        && !(0.0 <= rf(e, 0x2c))
        && ri(e, off::PHYS) & 1 == 0
        && ri(e, 0x11c) <= 0
        && ri(e, off::ROLL) <= 0
}

/// Modes in which the weapons hang on the back (0x004214db, 0x00421b4d).
fn weapons_away(mode: u8) -> bool {
    matches!(mode, 0 | 0x15 | 0x6d | 0x6a | 0x6b | 0x69 | 0x5b | 0x60 | 0x50 | 0x52 | 0x55 | 0x4f | 0x53 | 0x51 | 0x54)
}

impl Figure<'_> {
    /// 0x0041e660..0x0041f147: stun/climb/run adjustments of the targets, then every smoothed
    /// parameter moves toward its target, and the smoothed offsets are added to the hands,
    /// elbows and arms.
    fn smooth(&mut self) {
        let e = self.e;
        if self.st.anim_mode == 0 {
            self.st.anim_time = 0;
        }
        // 0x0041e66d: elbows move halfway to the hands.
        self.elbow_l = half(add3(self.hand_l, self.elbow_l));
        self.elbow_r = half(add3(self.hand_r, self.elbow_r));
        let app = self.app_flags();
        let sc = self.scale_v();
        // 0x0041e6f1: stunned (entity 0x11c): lie down.
        if ri(e, 0x11c) > 0 {
            if app & 1 != 0 {
                self.t_lean = [0.0, 90.0, 0.0];
                self.t_root_off[2] = (sc[0] - sc[2]) * 0.5;
            } else {
                self.t_lean = [90.0, 0.0, 0.0];
                self.t_root_off[2] = (sc[1] - sc[2]) * 0.5;
            }
        }
        // 0x0041e807: climbing.
        if self.flags() & 1 != 0 && self.phys() & 4 != 0 {
            self.t_off_r[2] += 7.0;
            self.t_off_l[2] += 7.0;
            self.t_rot_l[0] += 90.0;
            self.t_rot_r[0] += 90.0;
        }
        // 0x0041e88e: slot 6 weapon of sub type 0x14.
        if self.eq(6).1 == 0x14 {
            self.t_rot_l[0] = 0.0;
            self.t_off_l[1] += 4.0;
            self.t_off_l[2] += 4.0;
        }
        // 0x0041e8e7: running or moving forward leans the figure.
        let mode = self.mode();
        if app & 1 == 0 && mode != 0x6a && mode != 0x6b {
            if self.running {
                self.t_lean[0] -= 25.0;
                self.t_head_rot[0] += 10.0;
            } else {
                let v = rv3(e, off::VEL);
                if v[0] * v[0] + v[1] * v[1] > 0.1 {
                    let a = rv3(e, off::ACCEL);
                    if v[1] * a[1] + v[0] * a[0] + v[2] * a[2] > 0.0 {
                        self.t_lean[0] -= 10.0;
                        self.t_head_rot[0] += 5.0;
                    }
                }
            }
        }
        // 0x0041e9d4: `lerpRepeat(0 -> 1, dt, rate)`.
        let k = blend(self.inp.dt_ms, self.rate);
        approach(&mut self.st.rot_r, self.t_rot_r, k);
        approach(&mut self.st.rot_l, self.t_rot_l, k);
        approach(&mut self.st.off_r, self.t_off_r, k);
        approach(&mut self.st.off_l, self.t_off_l, k);
        approach(&mut self.st.body_rot, self.t_body_rot, k);
        approach(&mut self.st.root_off, self.t_root_off, k);
        self.st.swim_z = (self.swim_z - self.st.swim_z) * k + self.st.swim_z;
        for i in 0..3 {
            self.st.lean[i] = turn_towards(self.st.lean[i], self.t_lean[i], k);
        }
        for i in 0..3 {
            self.st.head_rot[i] = turn_towards(self.st.head_rot[i], self.t_head_rot[i], k);
        }
        for i in 0..3 {
            self.st.tail_rot[i] = turn_towards(self.st.tail_rot[i], self.t_tail_rot[i], k);
        }
        self.st.yaw_lean = (self.yaw_lean - self.st.yaw_lean) * k + self.st.yaw_lean;
        approach(&mut self.st.twist, self.t_twist, k);
        approach(&mut self.st.body_off, self.t_body_off, k);
        approach(&mut self.st.head_off, self.t_head_off, k);
        // 0x0041ef7b: mode 0x1c smooths the normalised ray direction into +0x86c.
        if mode == 0x1c {
            let mut d = rv3(e, off::RAY);
            let l2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if l2 > 0.0 {
                // 0x004240f0
                let inv = 1.0 / sqrt_d(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                d = [d[0] * inv, inv * d[1], inv * d[2]];
            }
            let k2 = blend(self.inp.dt_ms, 0.01);
            approach(&mut self.st.aim_dir, d, k2);
        } else {
            self.st.aim_dir = [0.0; 3];
        }
        // 0x0041f065
        self.arm_r = add3(self.st.rot_r, self.arm_r);
        self.arm_l = add3(self.st.rot_l, self.arm_l);
        self.hand_r = add3(self.st.off_r, self.hand_r);
        self.hand_l = add3(self.st.off_l, self.hand_l);
        let o = self.st.off_r;
        self.elbow_r = add3([o[0] * 0.4, o[1] * 0.4, o[2] * 0.4], self.elbow_r);
        let o = self.st.off_l;
        self.elbow_l = add3([o[0] * 0.4, o[1] * 0.4, o[2] * 0.4], self.elbow_l);
        // 0x0041f0f4: the hide mask.
        let h = self.inp.hide;
        if h & 1 != 0 {
            self.m_head = None;
        }
        if h & 2 != 0 {
            self.m_hair = None;
        }
        if h & 8 != 0 {
            self.m_body = None;
        }
        if h & 4 != 0 {
            self.m_shoulder = None;
        }
        if h & 0x40 != 0 {
            self.m_weapon_r = None;
            self.m_weapon_l = None;
        }
        self.cur_color = self.inp.color;
    }

    /// 0x0041f154..0x0041f720: the root matrix `L_14e0` and the body frame `+0xec`. Also sets
    /// the bob (`L_1708`, `L_172c`) the draws use.
    fn frames(&mut self) -> (M, M) {
        let e = self.e;
        let inp = self.inp;
        let mut root = IDENTITY;
        let rp = inp.render_pos;
        let z = rp[2].wrapping_sub(f2fix(inp.step_z)).wrapping_add(f2fix(self.st.root_off[2]));
        let y = rp[1].wrapping_add(inp.origin[1]);
        let x = rp[0].wrapping_add(inp.origin[0]);
        translate(&mut root, fix2f(x), fix2f(y), fix2f(z));
        let sc = self.scale_v();
        // 0x0041f225: rolling.
        let roll = ri(e, off::ROLL);
        if roll != 0 {
            let skip = e.0[off::CLASS] == 4
                && e.0[off::SPEC] == 1
                && self.mode_time() < self.tt()
                && matches!(self.mode(), 0x11 | 5 | 0x14);
            if !skip {
                let ang = (1.0 - roll as f32 / 600.0) * -360.0;
                let v = rv3(e, off::VEL);
                // 0x0041f2a0..0x0041f2d6: `cross` 0x00412390 of the velocity (`this`) with
                // (0, 0, 1) built by 0x0040ea90 (args x = 0, y = 0, z = 1.0):
                // (a.y*b.z - a.z*b.y, b.x*a.z - a.x*b.z, a.x*b.y - b.x*a.y).
                let b = [0.0f32, 0.0, 1.0];
                let mut axis = [v[1] * b[2] - v[2] * b[1], b[0] * v[2] - v[0] * b[2], v[0] * b[1] - b[0] * v[1]];
                if ri(e, off::TYPE) == 0x65 {
                    axis = v;
                }
                let l2 = axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2];
                if l2 > 0.01 {
                    let inv = 1.0 / sqrt_d(axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]);
                    axis = [axis[0] * inv, inv * axis[1], inv * axis[2]];
                    rot_axis(&mut root, ang, axis[0], axis[1], axis[2]);
                }
                self.hand_l[2] = sc[2] * 5.0 / sc[0] + self.hand_l[2];
                self.hand_r[2] = sc[2] * 5.0 / sc[0] + self.hand_r[2];
            }
        }
        // 0x0041f3b6: gliding lifts the hands the same way.
        if self.flags() & 0x10 != 0 {
            self.hand_l[2] = sc[2] * 5.0 / sc[0] + self.hand_l[2];
            self.hand_r[2] = sc[2] * 5.0 / sc[0] + self.hand_r[2];
        }
        let rr = inp.render_rot;
        rot_z(&mut root, rr[2]);
        translate(&mut root, 0.0, 0.0, self.st.swim_z);
        rot_y(&mut root, rr[1]);
        rot_x(&mut root, rr[0]);
        translate(&mut root, self.st.root_off[0], self.st.root_off[1], 0.0);
        rot_z(&mut root, self.st.lean[2]);
        rot_y(&mut root, self.st.lean[1]);
        rot_x(&mut root, self.st.lean[0]);
        translate(&mut root, 0.0, 0.0, -self.st.swim_z);
        scale1(&mut root, sc[0] / 11.2);
        // 0x0041f5cc: bob.
        self.bob = self.swing_raw.abs();
        self.bob4 = self.bob * 4.0;
        if self.mode() == 0x4f {
            self.bob4 *= 4.0;
        }
        self.root_cache = root;
        let mut base = root;
        translate(&mut base, 0.0, 0.0, self.bob4);
        translate_v(&mut base, self.st.body_off);
        rot_z(&mut base, self.st.body_rot[2]);
        rot_y(&mut base, self.st.body_rot[1]);
        rot_x(&mut base, rf(e, off::BODY_PITCH) + self.st.body_rot[0]);
        (root, base)
    }

    /// `translate(sz44 * -0.5, sz48 * -0.5, z)` with a given z, as several weapon cases end.
    fn center_z(&self, m: &mut M, model: u32, z: f32) {
        let s = self.size(model);
        translate(m, s[0] * -0.5, s[1] * -0.5, z);
    }

    /// The degree angle of the walk sway, `(double)(swing * f * 180) / PI`.
    fn sway(&self, f: f32) -> f32 {
        deg_d(self.swing * f * 180.0)
    }

    /// 0x0041f720..0x00423d46: every part's matrix, in the original's draw order.
    fn draw(&mut self, root: &M, base: &M) {
        let e = self.e;
        let inp = self.inp;
        let sc = self.scale_v();
        let flags = self.flags();
        let mode = self.mode();
        let anim = self.st.anim_mode;
        let look = rf(e, off::LOOK_PITCH);
        let lean = self.st.lean;
        let body_rot = self.st.body_rot;

        // 0x0041f726: glider (slot 11) while gliding.
        if flags & 0x10 != 0 {
            if let Some(g) = self.item_model(off::equip(11)) {
                let mut m = *base;
                let z = sc[2] * 8.0 / sc[0];
                translate_v(&mut m, add3([0.0, 0.0, z], self.body));
                rot_x(&mut m, -lean[0]);
                scale1(&mut m, self.body_scale * 2.0);
                self.center(&mut m, g);
                self.emit(PartSlot::Glider, g, &m);
            }
        }
        // 0x0041f8c6: boat (slot 11) in mode 0x6b.
        if mode == 0x6b {
            if let Some(b) = self.item_model(off::equip(11)) {
                let mut m = *base;
                let z = sc[2] * -9.0 / sc[0];
                translate_v(&mut m, add3([0.0, 0.0, z], self.body));
                rot_x(&mut m, -lean[0]);
                rot_z(&mut m, -lean[2]);
                scale1(&mut m, self.body_scale * 2.0);
                translate(&mut m, 1.0, 2.0, 0.0);
                self.center_z(&mut m, b, 0.0);
                self.emit(PartSlot::Boat, b, &m);
            }
        }
        // 0x0041fa97: animation 0x48 spins model 0xa06 between the hands (on last frame's
        // upper-body frame, which is only rebuilt below).
        if anim == 0x48 && self.st.anim_time < 5000 {
            if let Some(p) = self.fixed_model(0xa06) {
                let mut m = self.st.upper_frame;
                translate_v(&mut m, add3([0.0, 0.0, 8.0], half(add3(self.hand_r, self.hand_l))));
                rot_z(&mut m, self.st.anim_time as f32 * 0.02);
                rot_y(&mut m, 45.0);
                rot_x(&mut m, 45.0);
                scale1(&mut m, 1.0);
                self.center(&mut m, p);
                self.emit(PartSlot::SpinProp, p, &m);
            }
        }
        // 0x0041fc52: body.
        let sway_d = f64::from(self.swing * 0.5 * 180.0) / PI_D;
        if let Some(b) = self.m_body {
            let mut m = *base;
            translate_v(&mut m, self.body);
            rot_z(&mut m, (f64::from(self.st.yaw_lean * 0.5) + sway_d) as f32);
            scale1(&mut m, self.body_scale);
            self.center(&mut m, b);
            self.cur_shine = self.shine_of(off::equip(2));
            self.emit(PartSlot::Body, b, &m);
            self.cur_shine = 0.0;
            // 0x0041fdaa: show-patch timer negative: model 0x9f6 on the body.
            if 0.0 > rf(e, off::SHOW_PATCH) {
                if let Some(p) = self.fixed_model(0x9f6) {
                    let mut m = *base;
                    translate_v(&mut m, self.body);
                    rot_z(&mut m, (f64::from(self.st.yaw_lean * 0.5) + sway_d) as f32);
                    scale1(&mut m, self.body_scale);
                    translate(&mut m, 0.0, self.size(b)[1] * -0.5, 0.0);
                    scale1(&mut m, 0.5);
                    rot_y(&mut m, 45.0);
                    rot_z(&mut m, 180.0);
                    let s = self.size(p);
                    translate(&mut m, s[0] * -0.5, 0.0, s[2] * -0.5);
                    self.emit(PartSlot::BodyPatch, p, &m);
                }
            }
        }
        // 0x0041ff5f: neck (slot 1).
        if let Some(n) = self.m_neck {
            let mut m = *base;
            translate_v(&mut m, add3([0.0, 3.0, 6.05], self.body));
            rot_z(&mut m, (sway_d + f64::from(self.st.yaw_lean * 0.5)) as f32);
            let s = f64::from(self.swing);
            rot_x(&mut m, ((s * 0.7 * s) * 180.0 / PI_D) as f32);
            scale1(&mut m, self.body_scale);
            let si = self.size_i(n);
            translate(&mut m, si[0] as f32 * -0.5, -1.5, (-si[2]) as f32);
            self.cur_shine = self.shine_of(off::equip(1));
            self.emit(PartSlot::Neck, n, &m);
            self.cur_shine = 0.0;
        }
        // 0x00420103: shoulder armour (slot 5).
        if let Some(s) = self.m_shoulder {
            let mut m = *base;
            translate(&mut m, 0.0, 0.0, self.walk_blend * 2.0 + 4.0);
            translate_v(&mut m, self.body);
            rot_z(&mut m, sway_d as f32);
            scale1(&mut m, self.shoulder_scale);
            scale1(&mut m, self.body_scale + 0.1);
            self.center(&mut m, s);
            self.cur_shine = self.shine_of(off::equip(5));
            self.emit(PartSlot::ShoulderArmor, s, &m);
            self.cur_shine = 0.0;
        }
        // 0x004202a0: tail.
        if let Some(t) = self.m_tail {
            let mut m = *base;
            translate_v(&mut m, self.body);
            translate_v(&mut m, self.tail);
            rot_x(&mut m, rf(e, off::BACK_PITCH));
            rot_z(&mut m, deg_d(self.swing * -180.0));
            rot_z(&mut m, self.st.tail_rot[2]);
            rot_y(&mut m, self.st.tail_rot[1]);
            rot_x(&mut m, self.st.tail_rot[0]);
            scale1(&mut m, self.tail_scale);
            scale1(&mut m, self.body_scale);
            let si = self.size_i(t);
            translate(&mut m, si[0] as f32 * -0.5, (-si[1]) as f32, si[2] as f32 * -0.5);
            self.emit(PartSlot::Tail, t, &m);
        }
        // 0x00420433: head and hair share the head frame.
        let head_frame = |f: &Figure, m: &mut M| {
            let ho = f.st.head_off;
            translate(m, ho[0] + f.head[0], ho[1] + f.head[1], (ho[2] + f.head[2]) - f.bob4 * 0.5);
            translate(m, 0.0, 0.0, -3.0);
            rot_x(m, -body_rot[0] - rf(f.e, off::BODY_PITCH));
            rot_y(m, -body_rot[1]);
            rot_z(m, f.st.head_rot[2] - body_rot[2]);
            rot_y(m, f.st.head_rot[1]);
            rot_x(m, (look - f.inp.render_rot[0]) + f.st.head_rot[0]);
            translate(m, 0.0, 0.0, 3.0);
            scale1(m, f.head_scale);
        };
        if let Some(h) = self.m_head {
            let mut m = *base;
            head_frame(self, &mut m);
            let s = self.size(h);
            let ky = if ri(e, off::TYPE) == 0x65 { -0.2 } else { -0.5 };
            translate(&mut m, s[0] * -0.5, s[1] * ky, s[2] * -0.5);
            self.emit(PartSlot::Head, h, &m);
        }
        if let Some(h) = self.m_hair {
            let mut m = *base;
            head_frame(self, &mut m);
            self.center(&mut m, h);
            // 0x00420902: the hair colour, times the shader colour unless appearance flag 0x400.
            let c = [
                f32::from(e.0[off::HAIR_RGB]) / 255.0,
                f32::from(e.0[off::HAIR_RGB + 1]) / 255.0,
                f32::from(e.0[off::HAIR_RGB + 2]) / 255.0,
                1.0,
            ];
            self.cur_color = if self.app_flags() & 0x400 != 0 {
                c
            } else {
                let p = inp.color;
                [c[0] * p[0], c[1] * p[1], c[2] * p[2], c[3] * p[3]]
            };
            self.emit(PartSlot::Hair, h, &m);
            self.cur_color = inp.color;
        }
        // 0x00420a48: the upper-body frame `+0x12c`.
        let mut up = *root;
        translate(&mut up, 0.0, 0.0, self.bob4);
        translate_v(&mut up, self.st.body_off);
        let climbing = flags & 1 != 0 && self.phys() & 4 != 0;
        if !climbing && !matches!(anim, 2 | 3 | 0x25) {
            rot_x(&mut up, look * 2.0);
        }
        if flags & 0x400 != 0 {
            rot_x(&mut up, 60.0);
        }
        translate(&mut up, 0.0, 0.0, -4.0);
        rot_y(&mut up, self.st.twist[1]);
        translate(&mut up, 0.0, 0.0, 4.0);
        rot_z(&mut up, body_rot[2]);
        rot_y(&mut up, body_rot[1]);
        rot_x(&mut up, body_rot[0]);
        self.st.upper_frame = up;
        // 0x00420bc1: upper arms at the elbow points.
        if let Some(a) = self.m_arm {
            for (slot, p) in [(PartSlot::UpperArmLeft, self.elbow_l), (PartSlot::UpperArmRight, self.elbow_r)] {
                let mut m = up;
                translate_v(&mut m, p);
                scale1(&mut m, self.arm_scale);
                self.center(&mut m, a);
                self.emit(slot, a, &m);
            }
        }
        let between = add3(self.hand_r, self.hand_l);
        // 0x00420de6: the consumable being eaten or drunk.
        if (mode == 0x50 || mode == 0x51) && self.mode_time() < self.consumable_duration() {
            if let Some(c) = self.item_model(off::CONSUMABLE) {
                let mut m = up;
                translate_v(&mut m, half(between));
                rot_x(&mut m, self.arm_l[0]);
                scale1(&mut m, self.hand_scale);
                self.center(&mut m, c);
                self.emit(PartSlot::Consumable, c, &m);
            }
        }
        // 0x00420f58: the pet item (slot 12) in mode 0x52.
        if mode == 0x52 {
            if let Some(p) = self.item_model(off::equip(12)) {
                let mut m = up;
                translate_v(&mut m, half(between));
                rot_x(&mut m, self.arm_l[0]);
                scale1(&mut m, self.hand_scale);
                self.center_z(&mut m, p, 0.0);
                self.emit(PartSlot::PetItem, p, &m);
            }
        }
        // 0x00421092: arrows on the back for a bow or crossbow in slot 6, then slot 7.
        for (slot, s, a) in [(PartSlot::QuiverLeft, 6usize, 45.0f32), (PartSlot::QuiverRight, 7, -45.0)] {
            let (t, sub) = self.eq(s);
            if t == 3 && (sub == 6 || sub == 7) {
                if let Some(q) = self.fixed_model(0x347) {
                    let mut m = *base;
                    rot_z(&mut m, self.sway(0.25));
                    translate(&mut m, 0.0, -7.1, 0.1);
                    rot_axis(&mut m, a, 0.0, 1.0, 0.0);
                    rot_axis(&mut m, 90.0, 0.0, 0.0, 1.0);
                    scale1(&mut m, self.weapon_scale + 0.01);
                    self.center(&mut m, q);
                    self.emit(slot, q, &m);
                }
            }
        }
        let charged = rf(e, off::CHARGED_MP) > 0.0;
        let pulse = |f: &Figure| {
            let k = (cosf(f.st.anim_time as f32 * 0.01) + 1.0) * 0.25 + 1.0;
            let p = f.inp.color;
            [p[0] * k, p[1] * k, p[2] * k, p[3] * k]
        };
        // 0x00421438: right weapon (slot 7).
        if let Some(w) = self.m_weapon_r {
            let mut m = up;
            if charged {
                self.cur_color = pulse(self);
            }
            let sub = self.eq(7).1;
            let si = self.size_i(w);
            let s = self.size(w);
            let away = (weapons_away(mode) || flags & 0x10 != 0) && sub != 4 && sub != 0xc;
            if away {
                // 0x0042153f: on the back.
                m = *base;
                rot_z(&mut m, self.sway(0.25));
                translate(&mut m, 0.0, -7.1, 0.1);
                rot_axis(&mut m, if sub == 0xa { -45.0 } else { -135.0 }, 0.0, 1.0, 0.0);
                rot_axis(&mut m, 90.0, 0.0, 0.0, 1.0);
                scale1(&mut m, self.weapon_scale);
                self.center(&mut m, w);
            } else {
                // 0x0042165b: in the right hand.
                translate_v(&mut m, self.hand_r);
                rot_z(&mut m, self.arm_r[2]);
                rot_y(&mut m, self.arm_r[1]);
                rot_x(&mut m, self.arm_r[0]);
                rot_axis(&mut m, self.wpn_r[2], 0.0, 0.0, 1.0);
                rot_axis(&mut m, self.wpn_r[1], 0.0, 1.0, 0.0);
                rot_axis(&mut m, look * 0.5 + self.wpn_r[0], 1.0, 0.0, 0.0);
                scale1(&mut m, self.weapon_scale);
                if Some(w) == self.fixed_model(0x348) {
                    translate(&mut m, s[0] * -0.5 - 1.0, 0.5, 1.0);
                } else {
                    match sub {
                        0xa => translate(&mut m, s[0] * -0.5, s[1] * -0.5, -10.0),
                        0xc => {
                            scale1(&mut m, 0.6);
                            translate(&mut m, s[0] * -0.5, (1 - si[1]) as f32, s[2] * -0.5);
                        }
                        6 => translate(&mut m, s[0] * -0.5, 1.5 - s[1], s[2] * -0.5),
                        7 => translate(&mut m, s[0] * -0.5, 0.0, s[2] * -0.5),
                        8 => translate(&mut m, s[0] * -0.5, -2.0, -4.0),
                        4 => self.center(&mut m, w),
                        _ => translate(&mut m, s[0] * -0.5, s[1] * -0.5, -4.0),
                    }
                }
            }
            self.cur_shine = self.shine_of(off::equip(7));
            self.emit(PartSlot::WeaponRight, w, &m);
            self.cur_shine = 0.0;
            if charged {
                self.cur_color = inp.color;
            }
            // 0x00421a33: the weapon's base and tip for the trail.
            self.st.trails[1][0] = xform_pt(&m, [s[0] * 0.5, s[1] * 0.5, 0.0]);
            self.st.trails[0][0] = xform_pt(&m, [s[0] * 0.5, s[1] * 0.5, s[2]]);
        }
        // 0x00421b23: left weapon or shield (slot 6).
        if let Some(w) = self.m_weapon_l {
            let mut m = up;
            let sub = self.eq(6).1;
            let si = self.size_i(w);
            let s = self.size(w);
            if weapons_away(mode) && sub != 0x14 && sub != 4 && sub != 0xc {
                // 0x00421bc1: on the back.
                m = *base;
                translate(&mut m, 0.5, -0.65, 0.5);
                rot_z(&mut m, self.sway(0.25));
                match sub {
                    0xd => {
                        translate(&mut m, 0.0, -8.2, 0.0);
                        rot_axis(&mut m, 180.0, 0.0, 0.0, 1.0);
                    }
                    6 | 7 => {
                        translate(&mut m, 0.0, -6.2, 0.0);
                        rot_axis(&mut m, -45.0, 0.0, 1.0, 0.0);
                        rot_axis(&mut m, 90.0, 0.0, 0.0, 1.0);
                    }
                    _ => {
                        translate(&mut m, 0.0, -7.2, 0.0);
                        rot_axis(&mut m, -225.0, 0.0, 1.0, 0.0);
                        rot_axis(&mut m, 90.0, 0.0, 0.0, 1.0);
                    }
                }
                scale1(&mut m, self.weapon_scale_l);
                self.center(&mut m, w);
                self.cur_shine = self.shine_of(off::equip(6));
                self.emit(PartSlot::WeaponLeft, w, &m);
                self.cur_shine = 0.0;
            } else {
                // 0x00421e13: in the left hand (between the hands in animation 0x47).
                if anim == 0x47 {
                    translate_v(&mut m, add3([0.0, 0.0, 8.0], half(between)));
                } else {
                    translate_v(&mut m, self.hand_l);
                }
                rot_z(&mut m, self.arm_l[2]);
                rot_y(&mut m, self.arm_l[1]);
                rot_x(&mut m, self.arm_l[0]);
                rot_axis(&mut m, self.wpn_l[2], 0.0, 0.0, 1.0);
                rot_axis(&mut m, self.wpn_l[1], 0.0, 1.0, 0.0);
                rot_axis(&mut m, look * 0.5 + self.wpn_l[0], 1.0, 0.0, 0.0);
                scale1(&mut m, self.weapon_scale_l);
                if Some(w) == self.fixed_model(0x348) {
                    translate(&mut m, 1.0 - s[0] * 0.5, 0.5, 1.0);
                } else if anim == 0x47 {
                    self.center(&mut m, w);
                } else {
                    match sub {
                        6 => translate(&mut m, s[0] * -0.5, 1.5 - s[1], s[2] * -0.5),
                        7 => translate(&mut m, s[0] * -0.5, 0.0, s[2] * -0.5),
                        0xc => {
                            scale1(&mut m, 0.6);
                            translate(&mut m, s[0] * -0.5, (1 - si[1]) as f32, s[2] * -0.5);
                        }
                        0xd => translate(&mut m, s[0] * -0.5, 3.0, s[2] * -0.5),
                        // 0x004221c8: this tests the right weapon's sub type (slot 7).
                        _ if self.eq(7).1 == 8 => translate(&mut m, s[0] * -0.5, -2.0, -4.0),
                        4 => {
                            rot_y(&mut m, 180.0);
                            self.center(&mut m, w);
                        }
                        _ => translate(&mut m, s[0] * -0.5, s[1] * -0.5, -4.0),
                    }
                }
                if charged {
                    self.cur_color = pulse(self);
                }
                self.cur_shine = self.shine_of(off::equip(6));
                self.emit(PartSlot::WeaponLeft, w, &m);
                self.cur_shine = 0.0;
                if charged {
                    self.cur_color = inp.color;
                }
            }
            // 0x0042234d
            self.st.trails[3][0] = xform_pt(&m, [s[0] * 0.5, s[1] * 0.5, 0.0]);
            self.st.trails[2][0] = xform_pt(&m, [s[0] * 0.5, s[1] * 0.5, s[2]]);
        }
        // 0x0042243d: slot 6 item of sub type 0xe on the back.
        if let Some(b) = self.m_back {
            let mut m = *base;
            translate(&mut m, 0.65, -0.5, 0.3);
            rot_z(&mut m, self.sway(0.25));
            translate(&mut m, 0.0, -6.2, 0.0);
            rot_axis(&mut m, -45.0, 0.0, 1.0, 0.0);
            rot_axis(&mut m, 90.0, 0.0, 0.0, 1.0);
            scale1(&mut m, rf(e, off::WEAPON_SCALE));
            self.center(&mut m, b);
            self.emit(PartSlot::BackItem, b, &m);
        }
        self.update_trails();
        self.cur_color = inp.color;
        // 0x00422ff4: hands (slot 4 or the appearance hand); the right one is mirrored.
        if let Some(h) = self.m_hands {
            let app1 = self.app_flags() & 1 != 0;
            if inp.hide & 0x10 == 0 {
                let mut m = up;
                if app1 {
                    rot_axis(&mut m, self.foot_ang_r, 1.0, 0.0, 0.0);
                }
                translate_v(&mut m, self.hand_l);
                rot_z(&mut m, self.arm_l[2]);
                rot_y(&mut m, self.arm_l[1]);
                if app1 {
                    rot_axis(&mut m, self.foot_ang_r, 1.0, 0.0, 0.0);
                }
                rot_x(&mut m, self.arm_l[0]);
                scale1(&mut m, self.hand_scale);
                self.center(&mut m, h);
                self.cur_shine = self.shine_of(off::equip(4));
                self.emit(PartSlot::HandLeft, h, &m);
                // 0x00423219: the lamp (slot 10) while flag 0x200 is set.
                if flags & 0x200 != 0 {
                    if let Some(l) = self.item_model(off::equip(10)) {
                        let mut m = up;
                        let hl = self.hand_l;
                        translate(&mut m, hl[0], hl[1] + 2.0, hl[2] - 2.0);
                        scale1(&mut m, 0.8);
                        let si = self.size_i(l);
                        translate(&mut m, si[0] as f32 * -0.5, 1.0 - si[1] as f32 * 0.5, (2 - si[2]) as f32);
                        self.cur_shine = 0.0;
                        self.cur_color = [1.0; 4];
                        self.emit(PartSlot::Lamp, l, &m);
                        self.cur_color = inp.color;
                    }
                }
            }
            if inp.hide & 0x20 == 0 {
                let mut m = up;
                if app1 {
                    rot_axis(&mut m, self.foot_ang_l, 1.0, 0.0, 0.0);
                }
                translate_v(&mut m, self.hand_r);
                rot_z(&mut m, self.arm_r[2]);
                rot_y(&mut m, self.arm_r[1]);
                if app1 {
                    rot_axis(&mut m, self.foot_ang_l, 1.0, 0.0, 0.0);
                }
                rot_x(&mut m, self.arm_r[0]);
                scale(&mut m, -self.hand_scale, self.hand_scale, self.hand_scale);
                self.center(&mut m, h);
                // Drawn with whatever shine the left hand (or the lamp) left set.
                self.emit_with(PartSlot::HandRight, h, &m, true);
            }
            self.cur_shine = 0.0;
        }
        // 0x004235db: wings flap with the walk cycle.
        if let Some(w) = self.m_wings {
            let ang = cosf(inp.walk.phase * 0.5) * (self.walk_blend * 90.0) - 45.0;
            let wo = rv3(e, off::WING_OFFSET);
            let ws = rf(e, off::WING_SCALE);
            let wp = rf(e, off::WING_PITCH);
            let s = self.size(w);
            for (slot, x, a) in [(PartSlot::WingLeft, -wo[0], ang), (PartSlot::WingRight, wo[0], -ang)] {
                let mut m = up;
                translate(&mut m, x, wo[1], wo[2]);
                rot_x(&mut m, wp);
                rot_y(&mut m, a);
                rot_z(&mut m, -90.0);
                rot_x(&mut m, 90.0);
                scale1(&mut m, ws);
                translate(&mut m, s[0] * -0.5, 0.0, s[2] * -0.5);
                self.emit(slot, w, &m);
            }
        }
        // 0x004238ca: feet on the root frame.
        if let Some(f) = self.m_feet {
            let fp = rf(e, off::FEET_PITCH);
            if inp.hide & 0x80 == 0 {
                let mut m = *root;
                rot_z(&mut m, self.st.yaw_lean);
                let t = self.feet_lift + self.foot_ang_l;
                rot_axis(&mut m, fp + t, 1.0, 0.0, 0.0);
                rot_y(&mut m, self.foot_yaw);
                let fl = self.foot_l;
                translate(&mut m, fl[0], fl[1], self.bob * 3.0 + fl[2] + self.leg_l);
                if mode == 0x4f {
                    rot_x(&mut m, self.foot_ang_l * -1.5);
                }
                scale1(&mut m, self.foot_scale);
                if self.foot_flag {
                    rot_z(&mut m, 60.0);
                } else {
                    rot_axis(&mut m, t, 1.0, 0.0, 0.0);
                }
                self.center(&mut m, f);
                self.cur_shine = self.shine_of(off::equip(3));
                self.emit(PartSlot::FootLeft, f, &m);
                self.cur_shine = 0.0;
            }
            if inp.hide & 0x100 == 0 {
                let mut m = *root;
                rot_z(&mut m, self.st.yaw_lean);
                self.feet_lift += self.foot_ang_r;
                let t = self.feet_lift;
                rot_axis(&mut m, t, 1.0, 0.0, 0.0);
                rot_y(&mut m, -self.foot_yaw);
                let fr = self.foot_r;
                translate(&mut m, fr[0], fr[1], self.bob * 3.0 + fr[2] + self.leg_r);
                if mode == 0x4f {
                    rot_x(&mut m, self.foot_ang_r * -1.5);
                }
                scale1(&mut m, self.foot_scale);
                rot_axis(&mut m, fp + t, 1.0, 0.0, 0.0);
                self.center(&mut m, f);
                self.cur_shine = self.shine_of(off::equip(3));
                self.emit(PartSlot::FootRight, f, &m);
                self.cur_shine = 0.0;
            }
        }
    }

    /// 0x004225fd..0x00422fe5: the weapon trails. The newest point of each trail is moved by
    /// `-(render_pos + origin)` in x/y, then the trails follow the newest point during the
    /// swing of the attack animations, or collapse onto it otherwise.
    fn update_trails(&mut self) {
        let inp = self.inp;
        let rp = inp.render_pos;
        let d = [
            fix2f(rp[0].wrapping_neg().wrapping_sub(self.st.origin[0])),
            fix2f(rp[1].wrapping_neg().wrapping_sub(self.st.origin[1])),
            0.0,
        ];
        let tr = &mut self.st.trails;
        for k in [3usize, 2, 1, 0] {
            tr[k][0] = add3(d, tr[k][0]);
        }
        let anim = self.st.anim_mode;
        let t = self.st.anim_time;
        let mt = ri(self.e, off::MODE_TIME);
        let wu = self.wu();
        let du = self.du();
        // 0x004226f6: animation 0x45 draws a ring during its swing.
        if anim == 0x45 && t > wu {
            if t < wu + du {
                let mut ring = IDENTITY;
                let z = rp[2].wrapping_sub(f2fix(inp.step_z)).wrapping_add(f2fix(self.st.root_off[2]));
                translate(
                    &mut ring,
                    fix2f(rp[0].wrapping_add(inp.origin[0])),
                    fix2f(rp[1].wrapping_add(inp.origin[1])),
                    fix2f(z),
                );
                rot_z(&mut ring, inp.render_rot[2]);
                let r = (t - wu) as f32 * 20.0 / (wu + du) as f32;
                let r2 = r * 0.5;
                for i in 0..16 {
                    let a = (f64::from(i) * PI_D * 0.0625 - std::f64::consts::FRAC_PI_2) as f32;
                    let c = cosf(a);
                    let s = sinf(a);
                    let v = [s, c, 0.0];
                    let p = xform_pt(&ring, [v[0] * r2, v[1] * r2, v[2] * r2]);
                    self.st.trails[1][i as usize] = add3(d, p);
                    let p = xform_pt(&ring, [v[0] * r, v[1] * r, v[2] * r]);
                    self.st.trails[0][i as usize] = add3(d, p);
                }
                return;
            }
        }
        let tt = self.tt();
        let rc = self.rc();
        let root = self.root_cache;
        let tr = &mut self.st.trails;
        // 0x004229f9: melee swings: every trail follows its newest point.
        if matches!(anim, 0x1e | 0x1f | 0x20 | 0xe | 0xc | 0xa | 8 | 0x1c | 1 | 0x1d | 2 | 3 | 0x30 | 0x31 | 0x23)
            && t > wu
            && t < tt
        {
            let k = blend(inp.dt_ms, 0.05);
            for j in 1..16 {
                for a in [1usize, 0, 3, 2] {
                    let prev = tr[a][j - 1];
                    approach(&mut tr[a][j], prev, k);
                }
            }
            return;
        }
        // 0x00422bb5: the right weapon's trail sits on its base, the left one follows.
        if matches!(anim, 0xf | 0x11 | 0x13) && mt > wu {
            let k = blend(inp.dt_ms, 0.05);
            let d0 = tr[3][0];
            // 0x00422c0c: `D0 = D0` (a self-assignment in the original).
            let _ = d0;
            for i in 0..3 {
                tr[2][0][i] = (tr[2][0][i] - d0[i]) * 0.0 + tr[2][0][i];
            }
            let b0 = tr[1][0];
            for j in 1..16 {
                tr[1][j] = b0;
                tr[0][j] = b0;
                let prev = tr[3][j - 1];
                approach(&mut tr[3][j], prev, k);
                let prev = tr[2][j - 1];
                approach(&mut tr[2][j], prev, k);
            }
            return;
        }
        // 0x00422d18: shots: the right trail starts at fixed points of the root (animation
        // 0x32) or at the midpoint of its ends, the left one collapses.
        if matches!(anim, 0x10 | 0x12 | 0x14 | 0x32) && mt > wu && mt < rc / 2 + wu + du {
            let k = blend(inp.dt_ms, 0.075);
            if anim == 0x32 {
                tr[0][0] = xform_pt(&root, [20.0, 0.0, -12.0]);
                tr[1][0] = xform_pt(&root, [8.0, 0.0, -12.0]);
            } else {
                let a0 = tr[0][0];
                let b0 = tr[1][0];
                tr[1][0] = add3(half(a0), half(b0));
                let b0 = tr[1][0];
                for i in 0..3 {
                    tr[0][0][i] += (a0[i] - b0[i]) * 0.0;
                }
            }
            let d0 = tr[3][0];
            for j in 1..16 {
                let prev = tr[1][j - 1];
                approach(&mut tr[1][j], prev, k);
                let prev = tr[0][j - 1];
                approach(&mut tr[0][j], prev, k);
                tr[3][j] = d0;
                tr[2][j] = d0;
            }
            return;
        }
        // 0x00422f87: no attack: every trail collapses onto its newest point.
        for a in 0..4 {
            let p0 = tr[a][0];
            for j in 1..16 {
                tr[a][j] = p0;
            }
        }
    }
}

/// `v + f * (target - v)`, the per-component blend of animation 0x29.
fn lerp1(v: f32, target: f32, f: f32) -> f32 {
    v + f * (target - v)
}

impl Figure<'_> {
    /// Hand of the other arm on the right hand (two-handed grips): `hand_l = hand_r`,
    /// `hand_l.x += dx`, and the left elbow follows (+10 y, -4 z).
    fn grip(&mut self, dx: f32) {
        self.hand_l = self.hand_r;
        self.hand_l[0] += dx;
        self.elbow_l[1] += 10.0;
        self.elbow_l[2] -= 4.0;
    }

    /// 0x00415771..0x00415de9: animation 0x29 pulls head, hands, feet and tail toward the body
    /// centre by `f` (`L_172c/L_1724/L_1718` is the appearance body offset).
    fn collapse(&mut self, f: f32) {
        let [bx, by, bz] = self.body0;
        self.head[2] = lerp1(self.head[2], bz - 2.0, f);
        self.head[0] = lerp1(self.head[0], bx - 0.0, f);
        self.head[1] = lerp1(self.head[1], by - 0.0, f);
        for h in [&mut self.hand_l, &mut self.hand_r, &mut self.foot_l, &mut self.foot_r] {
            h[0] = lerp1(h[0], bx, f);
            h[2] = lerp1(h[2], bz, f);
            h[1] = lerp1(h[1], by, f);
        }
        let t0 = self.tail0;
        self.tail = [lerp1(t0[0], bx + 0.0, f), lerp1(t0[1], by + 4.0, f), lerp1(t0[2], bz + 4.0, f)];
    }

    /// 0x00413af6..0x0041e660: the switch over the animation mode (`jmp [eax*4 + 0x423d4c]`
    /// with `eax = mode - 1`, modes 1..0x49; 0xc..0xe, 0x24, 0x40 and the others do nothing).
    /// Each arm sets the targets of the smoothed state and moves the hands, feet, head and
    /// tail for the current animation time; the comment gives the arm's address.
    #[allow(clippy::cognitive_complexity)]
    fn anim_switch(&mut self) {
        let t = self.st.anim_time;
        let anim = self.st.anim_mode;
        match anim {
            // 0x00413afd
            0x39 => {
                self.t_root_off[2] = -0.14;
                self.t_body_rot[0] = 10.0;
                self.t_off_l = [0.0, 4.0, 0.0];
                self.t_off_r = [0.0, 4.0, 0.0];
                self.t_head_off = [0.0, 2.0, -2.0];
            }
            // 0x00413bb1
            0x48 => {
                if t < 5000 {
                    self.t_body_rot[0] = 20.0;
                    self.t_head_rot[0] = 30.0;
                    self.t_off_l = [0.0, 0.0, 16.0];
                    self.t_off_r = [0.0, 0.0, 16.0];
                }
            }
            // 0x00413c3d: riding the pet.
            0x3a => {
                if let Some(pet) = self.inp.pet {
                    let z = rf(pet, off::SCALE + 8) * 0.75 + self.leg_l * 0.3;
                    self.t_root_off[1] -= 0.25;
                    self.t_root_off[2] = z + self.t_root_off[2];
                    let o = riding_offset(ri(pet, off::TYPE));
                    self.t_root_off[0] = o[0] + self.t_root_off[0];
                    self.t_root_off[1] = o[1] + self.t_root_off[1];
                    let mut z = o[2] + self.t_root_off[2];
                    self.t_root_off[2] = z;
                    if matches!(ri(pet, off::TYPE), 0x16 | 0x17) {
                        z -= 0.75;
                        self.t_root_off[2] = z;
                    }
                    self.swim_z = -z;
                }
                self.t_body_rot[0] = 20.0;
                self.t_off_l = [4.0, 6.0, -3.0];
                self.t_off_r = [-4.0, 6.0, -3.0];
                self.t_head_off = [0.0, 2.0, -2.0];
                self.t_rot_l = [60.0, 90.0, 60.0];
                self.t_rot_r = [60.0, -90.0, -60.0];
            }
            // 0x00413e2b
            0x3b => {
                self.t_root_off[2] += 1.25;
                self.t_body_rot[0] = 20.0;
                self.t_head_rot[2] = -50.0;
                self.t_off_l = [4.0, 6.0, -3.0];
                self.t_off_r = [-4.0, 6.0, -3.0];
                self.t_head_off = [0.0, 1.0, 0.0];
                self.t_head_rot = [0.0, 0.0, 30.0];
                self.t_rot_l = [60.0, 90.0, 60.0];
                self.t_rot_r = [60.0, -90.0, -60.0];
            }
            // 0x00413f91
            0x37 => {
                self.t_lean = [-90.0, 0.0, 0.0];
                let sc = self.scale_v();
                self.t_head_rot[0] = 20.0;
                self.t_root_off[2] = (sc[1] - sc[2]) * 0.5 - 0.15;
                self.t_head_off = [0.0, -1.0, 0.0];
                self.t_off_l = [0.0, 0.0, 3.0];
                self.t_off_r = [0.0, 0.0, 3.0];
            }
            // 0x0041409a, falls through into 0x33.
            0x34 | 0x33 => {
                if anim == 0x34 {
                    self.t_root_off[2] = -0.14;
                    self.t_body_rot[0] = 10.0;
                    self.t_off_l = [0.0, 4.0, 0.0];
                    self.t_off_r = [0.0, 4.0, 0.0];
                    self.t_head_off = [0.0, 2.0, -2.0];
                }
                // 0x0041414f
                if self.mode_time() < self.consumable_duration() {
                    let c = cw_math::cos(f64::from(t as f32 * 0.02)) as f32;
                    self.t_head_rot = [c * 5.0 + 45.0, 0.0, 0.0];
                    self.t_body_rot = [10.0, 0.0, 0.0];
                    self.t_lean = [10.0, 0.0, 0.0];
                    self.t_off_l = [1.0, 7.0, 8.0];
                    self.t_off_r = [-1.0, 7.0, 8.0];
                    self.t_rot_l = [100.0, 0.0, 0.0];
                    self.t_rot_r = [100.0, 0.0, 0.0];
                }
            }
            // 0x004142fc
            0x35 => {
                self.t_body_rot = [-30.0, 0.0, 0.0];
                self.t_off_l = [4.0, 6.0, 6.0];
                self.t_off_r = [-4.0, 6.0, 6.0];
                self.t_body_off[1] = 3.0;
            }
            // 0x004143ab
            0x36 => {
                let a = rv3(self.e, off::ACCEL);
                if a[0] * a[0] + a[1] * a[1] + a[2] * a[2] > 0.0 {
                    self.t_lean[0] = 20.0;
                    self.t_body_rot[0] = -10.0;
                    self.t_head_rot[0] = -20.0;
                    self.t_rot_l[0] = -20.0;
                    self.t_off_l[1] += 4.0;
                    self.t_rot_r[0] = -20.0;
                    self.t_off_r[1] += 4.0;
                    self.feet_lift = -40.0;
                    self.foot_l[2] += 3.0;
                    self.foot_r[2] += 3.0;
                    self.foot_r[1] += 4.0;
                    self.foot_l[1] += 4.0;
                }
            }
            // 0x004144af
            0x46 => {
                if t < self.wu() {
                    self.t_lean[0] = -30.0;
                } else if t < self.wu() + self.du() {
                    self.t_lean[0] = 120.0;
                }
            }
            // 0x004144fc
            0x26 => {
                if t < self.du() / 2 + self.wu() {
                    self.t_head_rot = [-30.0, 10.0, 90.0];
                    self.t_body_rot = [0.0, 0.0, 60.0];
                    self.t_head_off = [-3.0, -2.0, 0.0];
                } else if t < self.tt() {
                    self.t_head_rot = [30.0, -10.0, -30.0];
                    self.t_body_rot = [10.0, 0.0, -20.0];
                    self.t_head_off = [3.0, 0.0, 2.0];
                }
            }
            // 0x0041466c
            0x27 => {
                if t < self.du() / 2 + self.wu() {
                    self.t_head_rot = [-70.0, 0.0, 0.0];
                    self.t_body_rot = [-10.0, 0.0, 0.0];
                    self.t_head_off = [0.0, -3.0, -1.0];
                    self.t_rot_l[0] = 20.0;
                    self.t_rot_r[0] = 20.0;
                } else if t < self.tt() {
                    self.t_head_rot = [50.0, 0.0, 0.0];
                    self.t_body_rot = [20.0, 0.0, 0.0];
                    self.t_body_off = [0.0, -3.0, 1.0];
                    self.t_head_off = [0.0, 0.0, 2.0];
                    self.t_rot_r[0] = -20.0;
                    self.t_rot_l[0] = -20.0;
                    self.t_tail_rot = [-10.0, 0.0, 0.0];
                }
            }
            // 0x00414878
            0x2b => {
                if t < self.du() / 2 + self.wu() {
                    self.t_head_rot = [-20.0, 0.0, 0.0];
                    self.t_body_rot = [50.0, 0.0, 0.0];
                    self.t_body_off = [0.0, -2.0, 2.0];
                    self.t_head_off = [0.0, -6.0, -2.0];
                    self.t_rot_l[0] = 20.0;
                    self.t_rot_r[0] = 30.0;
                    self.t_tail_rot = [-50.0, 0.0, 0.0];
                } else if t < self.tt() {
                    self.t_head_rot = [-10.0, 0.0, 0.0];
                    self.t_body_rot = [0.0, 0.0, 0.0];
                    self.t_head_off = [0.0, 0.0, 1.0];
                    self.t_rot_l[0] = 0.0;
                    self.t_rot_r[0] = 0.0;
                    self.rate = 0.02;
                }
            }
            // 0x00414a6e
            0x2c | 0x49 => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.t_head_rot = [10.0, 0.0, 0.0];
                    self.t_body_rot = [-20.0, 0.0, 0.0];
                    self.t_body_off = [0.0, -1.0, 0.0];
                    self.t_head_off = [0.0, 1.0, 1.0];
                    self.t_rot_l[0] = 20.0;
                    self.t_rot_r[0] = 20.0;
                    self.t_off_l[2] = 2.0;
                    self.t_off_r[2] = 2.0;
                    self.t_tail_rot = [30.0, 0.0, 10.0];
                } else if t < du / 3 + wu {
                    self.t_head_rot = [-30.0, 0.0, 0.0];
                    self.t_body_rot = [50.0, 0.0, 0.0];
                    self.t_body_off = [0.0, 0.0, 1.0];
                    self.t_head_off = [0.0, -4.0, 2.0];
                    self.t_rot_l[0] = 60.0;
                    self.t_rot_r[0] = 70.0;
                    self.t_tail_rot = [-30.0, 0.0, 0.0];
                } else if t < (du * 2) / 3 + wu {
                    self.t_head_rot = [-30.0, 0.0, 0.0];
                    self.t_body_rot = [40.0, 0.0, 0.0];
                    self.t_body_off = [0.0, 1.0, 1.0];
                    self.t_head_off = [0.0, -2.0, 2.0];
                    self.t_rot_l[0] = 70.0;
                    self.t_rot_r[0] = -30.0;
                    self.t_off_r = [0.0, 3.0, -2.0];
                    self.t_tail_rot = [-30.0, 0.0, -10.0];
                } else if t < wu + du {
                    self.t_head_rot = [-30.0, 0.0, 0.0];
                    self.t_body_rot = [10.0, 0.0, 0.0];
                    self.t_body_off = [0.0, 1.0, 1.0];
                    self.t_head_off = [0.0, -2.0, 2.0];
                    self.t_rot_l[0] = -30.0;
                    self.t_rot_r[0] = -40.0;
                    self.t_off_l = [0.0, 3.0, -2.0];
                    self.t_tail_rot = [-30.0, 0.0, -10.0];
                } else if t < self.tt() {
                    self.t_head_rot = [10.0, 0.0, -10.0];
                    self.t_body_rot = [-20.0, 0.0, 0.0];
                    self.t_body_off = [0.0, 1.0, 0.0];
                    self.t_head_off = [0.0, 1.0, 1.0];
                    self.t_rot_l[0] = 20.0;
                    self.t_rot_r[0] = 20.0;
                    self.t_off_l[2] = 2.0;
                    self.t_off_r[2] = 2.0;
                    self.t_tail_rot = [30.0, 0.0, 10.0];
                }
            }
            // 0x00415113
            0x2a => {
                if t < self.du() / 2 + self.wu() {
                    self.t_head_rot = [-10.0, 10.0, 30.0];
                    self.t_body_rot = [-10.0, 0.0, 0.0];
                    self.t_head_off = [-3.0, 0.0, -1.0];
                    self.t_rot_l[0] = 20.0;
                    self.t_rot_r[0] = 20.0;
                    self.rate = 0.005;
                } else if t < self.tt() {
                    self.t_head_rot = [50.0, 30.0, 0.0];
                    if t < self.wu() + self.du() {
                        self.t_head_rot[1] = -30.0;
                    }
                    self.t_body_rot = [30.0, 0.0, 0.0];
                    self.t_body_off = [0.0, -1.0, 2.0];
                    self.t_head_off = [0.0, 0.0, 2.0];
                    self.t_rot_r[0] = -60.0;
                    self.rate = 0.005;
                    self.t_rot_l[0] = -20.0;
                    self.t_tail_rot = [-20.0, 0.0, 0.0];
                }
            }
            // 0x00415332
            0x2f => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.t_body_rot[2] = 10.0;
                    self.t_body_rot[1] = -30.0;
                    self.t_rot_l[1] = 30.0;
                    self.t_rot_r[1] = 30.0;
                    self.t_head_rot[2] = -60.0;
                } else if t < wu + du {
                    self.t_rot_l[1] = -80.0;
                    self.t_rot_r[1] = -80.0;
                    self.t_lean[1] = (t - wu) as f32 / du as f32 * 360.0;
                } else if t < self.tt() {
                    self.t_body_rot[1] = 30.0;
                    self.t_rot_l[1] = -30.0;
                    self.t_rot_r[1] = -30.0;
                }
            }
            // 0x00415420
            0x2e => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.t_body_rot[2] = -10.0;
                    self.t_body_rot[1] = 30.0;
                    self.t_rot_l[1] = -30.0;
                    self.t_rot_r[1] = -30.0;
                    self.t_head_rot[2] = 60.0;
                } else if t < wu + du {
                    self.t_rot_l[1] = 80.0;
                    self.t_rot_r[1] = 80.0;
                    self.t_lean[1] = (t - wu) as f32 / du as f32 * -360.0;
                } else if t < self.tt() {
                    self.t_body_rot[1] = -30.0;
                    self.t_rot_l[1] = 30.0;
                    self.t_rot_r[1] = 30.0;
                }
            }
            // 0x0041550e
            0x2d => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.t_body_rot[0] = -10.0;
                    self.t_head_rot[0] = 10.0;
                    self.t_tail_rot[0] = -60.0;
                } else if t < wu + du {
                    self.t_rot_l[0] = 80.0;
                    self.t_rot_r[0] = 80.0;
                    self.t_head_rot[0] = -60.0;
                    self.t_lean[0] = (t - wu) as f32 / du as f32 * -360.0;
                    self.t_head_off = [0.0, -3.0, 0.0];
                } else if t < self.tt() {
                    self.t_body_rot[0] = 20.0;
                    self.t_head_rot[0] = -10.0;
                    self.t_head_off = [0.0, -2.0, 0.0];
                }
            }
            // 0x00415624
            0x28 => {
                if t < self.wu() {
                    self.t_head_rot[2] = 90.0;
                    self.rate = 0.002;
                    self.t_head_rot = [0.0, 10.0, -60.0];
                    self.t_tail_rot = [0.0, 0.0, -70.0];
                } else if t < self.tt() {
                    self.t_head_rot[2] = 200.0;
                    self.t_head_rot = [0.0, -20.0, 60.0];
                    self.t_head_off = [-3.0, 0.0, -2.0];
                    self.t_tail_rot = [0.0, 0.0, 70.0];
                }
            }
            // 0x00415771: shrink into the body, vanish, grow back.
            0x29 => {
                let wu = self.wu();
                if t < wu {
                    let f = t as f32 / wu as f32;
                    self.collapse(f);
                } else if t < wu + self.du() {
                    // 0x00415a78: head, hands, feet and tail are not drawn.
                    self.m_head = None;
                    self.m_hands = None;
                    self.m_feet = None;
                    self.m_tail = None;
                    self.t_body_off = [0.0, 0.0, -3.0];
                } else if t < self.tt() {
                    let g = 1.0 - (t - wu - self.du()) as f32 / self.rc() as f32;
                    self.collapse(g);
                }
            }
            // 0x00415dee
            0x25 => {
                let (el_z, el_y);
                if t < self.wu() {
                    self.t_rot_r = [-90.0, -90.0, 80.0];
                    self.t_body_rot[2] = -80.0;
                    el_z = self.elbow_l[2];
                    el_y = self.elbow_l[1];
                } else if t < self.tt() {
                    self.t_rot_r = [-90.0, 0.0, -90.0];
                    self.hand_r[0] += 4.0;
                    self.t_body_rot[2] = 90.0;
                    self.foot_flag = true;
                    self.foot_l[1] -= 4.0;
                    self.foot_r[1] += 4.0;
                    self.rate = 0.05;
                    el_z = self.elbow_l[2];
                    el_y = self.elbow_l[1];
                } else {
                    // 0x00415f11: the animation ends.
                    self.st.anim_mode = 0;
                    self.hand_r[1] += 4.0;
                    self.hand_r[0] -= 8.0;
                    self.hand_l = self.hand_r;
                    self.hand_l[0] -= 2.0;
                    self.t_rot_r[0] = -60.0;
                    self.t_rot_r[2] = 20.0;
                    self.t_body_rot[2] = -20.0;
                    self.hand_l[2] -= 1.0;
                    el_y = self.elbow_l[1] + 10.0;
                    el_z = self.elbow_l[2] - 4.0;
                }
                // 0x00415fe3
                self.hand_l = self.hand_r;
                self.hand_l[0] += 2.0;
                self.elbow_l[1] = el_y + 10.0;
                self.elbow_l[2] = el_z - 4.0;
                self.t_rot_l = self.t_rot_r;
            }
            // 0x00416052
            4 => {
                let wu = self.wu();
                if t < wu / 2 {
                    let mut f = t as f32 / (wu / 2) as f32;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    let g = 1.0 - f;
                    self.t_lean[0] = f * 5.0;
                    self.t_body_rot[1] = f * 10.0;
                    self.t_body_rot[2] = f * -20.0;
                    self.t_rot_r = [g * 0.0 + f * 60.0, g * 120.0 + f * 60.0, g * 0.0 + f * 60.0];
                    self.grip(2.0);
                    self.rate = 0.005;
                    self.t_rot_l = self.t_rot_r;
                } else {
                    if self.e.0[off::CLASS] == 1 && self.e.0[off::SPEC] == 1 {
                        self.t_off_r = [2.0, 7.0, 4.0];
                        self.t_rot_r = [0.0, -60.0, 0.0];
                        self.rate = 0.01;
                    } else {
                        let mut f = (t - wu / 2) as f32;
                        f = f / (wu / 2) as f32 * 1.5;
                        if f > 1.0 {
                            f = 1.0;
                        }
                        let g = 1.0 - f;
                        self.t_lean[0] = 0.0;
                        self.t_body_rot = [20.0, 0.0, 0.0];
                        self.t_rot_r = [g * 90.0 + f * 30.0, g * 0.0 + f * 0.0, g * 90.0 + f * 0.0];
                        let c = cw_math::cos(f64::from(t as f32 * 0.5)) as f32;
                        self.hand_r[2] = c * 0.5 + self.hand_r[2];
                        self.t_off_r = [-6.0, 0.0, f * 16.0];
                        self.rate = 0.005;
                    }
                    // 0x004163bc
                    self.t_off_l = self.t_off_r;
                    self.grip(2.0);
                    self.t_rot_l = self.t_rot_r;
                }
            }
            // 0x0041644b
            2 => {
                let wu = self.wu();
                if t < wu {
                    let mut f = t as f32 / wu as f32 * 1.5;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    let g = 1.0 - f;
                    self.t_lean[0] = f * 10.0 + 5.0;
                    self.t_body_rot = [10.0, 10.0, -60.0];
                    self.t_rot_r = [g * 90.0 + f * 60.0, g * 0.0 + f * 0.0, g * 90.0 + f * 60.0];
                    self.t_off_r = [0.0, 0.0, f * 14.0];
                    self.t_off_l = self.t_off_r;
                    self.grip(2.0);
                    self.rate = 0.01;
                    self.t_rot_l = self.t_rot_r;
                } else if t < self.tt() {
                    let mut f = (t - wu) as f32 / self.du() as f32;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    self.t_lean[0] = -10.0;
                    self.t_body_rot[1] = 10.0;
                    self.t_body_rot[2] = 60.0;
                    self.rate = 0.04;
                    self.t_rot_r = [-70.0, 0.0, -60.0];
                    let a = (f64::from(f) * PI_D * 0.5) as f32;
                    let s = cw_math::sin(f64::from(a)) as f32;
                    self.t_off_r = [0.0, s * 2.0, 0.0];
                    self.t_off_l = self.t_off_r;
                    self.hand_l = self.hand_r;
                    self.hand_l[0] += 2.0;
                    self.hand_l[1] -= 1.0;
                    self.t_rot_l = self.t_rot_r;
                } else {
                    // 0x004167b8: the animation ends.
                    self.st.anim_mode = 0;
                    self.hand_r[1] += 2.0;
                    self.hand_r[2] += 3.0;
                    self.hand_l = self.hand_r;
                    self.hand_l[0] -= 3.0;
                    self.t_rot_r[0] = 0.0;
                    self.t_rot_r[1] = 20.0;
                    self.hand_l[2] -= 3.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.t_rot_l = self.t_rot_r;
                    self.t_rot_l = self.t_rot_r;
                }
            }
            // 0x004168ac
            0x30 => {
                self.t_twist = rv3(self.e, off::UNUSED_138);
                let wu = self.wu();
                if t < wu / 2 {
                    let mut f = t as f32 / (wu / 2) as f32;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    let g = 1.0 - f;
                    self.t_body_rot[1] = f * 10.0;
                    self.t_body_rot[2] = f * -80.0;
                    self.t_rot_r = [g * -90.0 + f * -90.0, g * 0.0 + f * 90.0, g * 0.0 + f * -30.0];
                    self.grip(2.0);
                    self.rate = 0.01;
                } else if t < wu {
                    self.t_body_rot = [10.0, 0.0, -90.0];
                    self.t_rot_r = [0.0, 110.0, -20.0];
                    self.grip(2.0);
                    self.rate = 0.01;
                } else if t < self.tt() {
                    self.t_lean[2] = 20.0;
                    self.t_body_rot = [0.0, 0.0, 90.0];
                    self.t_rot_r = [0.0, 90.0, 90.0];
                    self.grip(-2.0);
                    self.rate = 0.01;
                } else {
                    // 0x00416bd0: the animation ends.
                    self.hand_l = self.hand_r;
                    self.st.anim_mode = 0;
                    self.hand_l[0] -= 2.0;
                    self.hand_l[2] -= 1.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.t_rot_r[1] = 90.0;
                    self.t_body_rot[2] = -80.0;
                    self.t_rot_l = self.t_rot_r;
                }
                // 0x00416c9f: the spin.
                let mut f = (t - self.wu()) as f32 / self.du() as f32;
                if f > 1.0 {
                    f = 1.0;
                } else if 0.0 > f {
                    f = 0.0;
                }
                self.t_lean[2] = f * 360.0;
                self.t_rot_l = self.t_rot_r;
            }
            // 0x00416d25
            0x31 => {
                self.t_twist = rv3(self.e, off::UNUSED_138);
                let wu = self.wu();
                if t < wu {
                    self.t_lean[2] = 30.0;
                    self.t_body_rot = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-90.0, -90.0, 90.0];
                    // 0x00416ddd with the operands swapped: the right hand goes to the left.
                    self.hand_r = self.hand_l;
                    self.hand_l[0] += 2.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.rate = 0.005;
                    self.t_rot_l = self.t_rot_r;
                } else if t < self.tt() {
                    self.t_body_rot = [0.0, 0.0, 0.0];
                    self.t_rot_r = [-90.0, -90.0, 0.0];
                    if t < self.wu() + self.du() {
                        self.t_root_off[0] = 0.25;
                    }
                    // 0x00416f15
                    self.grip(-2.0);
                    let mut f = (t - self.wu()) as f32 / self.du() as f32;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    self.t_lean[2] = f * -360.0;
                    self.rate = 0.03;
                    self.t_rot_l = self.t_rot_r;
                } else {
                    // 0x0041700f: the animation ends.
                    self.hand_l = self.hand_r;
                    self.st.anim_mode = 0;
                    self.hand_l[0] -= 2.0;
                    self.hand_l[2] -= 1.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.t_rot_r = [-90.0, 90.0, 0.0];
                    self.t_body_rot[2] = 0.0;
                    self.t_rot_l = self.t_rot_r;
                    self.t_rot_l = self.t_rot_r;
                }
            }
            // 0x004170ee
            3 => {
                let wu = self.wu();
                if t < wu {
                    self.t_lean[2] = 0.0;
                    self.t_rot_r = [-90.0, 0.0, 0.0];
                    self.t_off_r = [-5.0, 8.0, 0.0];
                    self.t_off_l = self.t_off_r;
                    self.grip(2.0);
                    self.rate = 0.005;
                    self.t_rot_l = self.t_rot_r;
                } else if t < self.du() / 2 + wu {
                    self.t_lean = [10.0, 0.0, -10.0];
                    self.t_body_rot = [30.0, 0.0, -30.0];
                    self.t_rot_r = [0.0, 0.0, 90.0];
                    self.t_off_r = [-8.0, -4.0, 20.0];
                    self.t_off_l = self.t_off_r;
                    self.grip(2.0);
                    self.rate = 0.005;
                    self.t_rot_l = self.t_rot_r;
                } else if t < self.tt() {
                    self.t_lean[2] = -30.0;
                    self.t_body_rot = [0.0, 20.0, -60.0];
                    self.t_rot_r = [70.0, 0.0, 90.0];
                    self.t_off_r = [5.0, -2.0, 0.0];
                    self.t_off_l = self.t_off_r;
                    self.grip(2.0);
                    self.rate = 0.02;
                    self.t_rot_l = self.t_rot_r;
                } else {
                    // 0x004173e0: the animation ends.
                    self.hand_l = self.hand_r;
                    self.st.anim_mode = 0;
                    self.hand_l[0] -= 2.0;
                    self.hand_l[2] -= 1.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.t_rot_r[1] = 90.0;
                    self.t_body_rot[2] = -80.0;
                    self.t_rot_l = self.t_rot_r;
                    self.t_rot_l = self.t_rot_r;
                }
            }
            // 0x004174a0: charge (0.5 s build-up).
            1 => {
                let tf = t as f32;
                if 500.0 > tf {
                    let mut f = tf / 500.0;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    if self.two_handed() {
                        self.t_rot_r[0] = f * 120.0;
                        self.t_off_r[1] = 0.0;
                        self.t_off_r[2] = 14.0;
                        self.t_off_r[0] = -self.hand_r[0];
                        self.t_off_l[1] = 0.0;
                        self.t_off_l[2] = 12.0;
                        self.t_off_l[0] = 2.0 - self.t_off_l[0];
                    } else {
                        self.t_off_r = [1.0, 0.0, 14.0];
                        self.t_off_l = [-1.0, 0.0, 14.0];
                        self.t_rot_l[0] = -100.0;
                        self.t_rot_r[0] = -100.0;
                    }
                    // 0x00417649
                    let b = f * -40.0;
                    self.t_body_rot[0] = b;
                    self.feet_lift = f * -50.0;
                    self.t_lean[0] = b;
                    if self.mode() == 0x30 {
                        let mut s = tf / 500.0 * 360.0;
                        if s > 360.0 {
                            s = 360.0;
                        }
                        self.t_lean[0] = b - s;
                    }
                    // 0x004176d5: the walk blend `creature+0x1188` is cleared.
                    self.walk_blend = 0.0;
                } else if t < self.tt() {
                    let mut f = (t - 500) as f32 / 500.0 * 2.0;
                    if f > 1.0 {
                        f = 1.0;
                    }
                    self.t_lean[0] = (1.0 - f) * -60.0;
                    self.t_body_rot[0] = -20.0;
                    self.t_rot_r[0] = -70.0;
                    if self.two_handed() {
                        self.t_off_r[1] = 4.0;
                        self.t_off_r[2] = 0.0;
                        self.t_off_r[0] = -self.hand_r[0];
                        self.t_off_l[1] = 4.0;
                        self.t_off_l[2] = 0.0;
                        self.t_off_l[0] = 2.0 - self.t_off_l[0];
                        self.rate = 0.1;
                    } else {
                        self.t_off_r = [1.0, 3.0, 8.0];
                        self.t_off_l = [-1.0, 3.0, 8.0];
                        self.t_rot_l[0] = -150.0;
                        self.t_rot_r[0] = -150.0;
                        self.rate = 0.1;
                    }
                } else if self.two_handed() {
                    // 0x004178e0
                    self.hand_r[1] += 4.0;
                    self.hand_r[0] -= 8.0;
                    self.hand_l = self.hand_r;
                    self.hand_l[0] -= 2.0;
                    self.hand_l[2] -= 1.0;
                    self.elbow_l[1] += 10.0;
                    self.elbow_l[2] -= 4.0;
                    self.t_rot_r[0] = -60.0;
                    self.t_rot_r[2] = 20.0;
                } else {
                    self.t_off_r = [1.0, 3.0, 2.0];
                    self.t_off_l = [-1.0, 3.0, 2.0];
                    self.t_rot_l[0] = -250.0;
                    self.t_rot_r[0] = -250.0;
                }
                // 0x00417a63: a shield keeps its own pose.
                let (t6, s6) = self.eq(6);
                if t6 == 3 && s6 == 0xd {
                    self.t_rot_l = [-250.0, -90.0, 0.0];
                }
            }
            // 0x00417ab1
            7 => {
                let tt = self.tt();
                if t < tt / 2 {
                    self.t_twist = rv3(self.e, off::UNUSED_144);
                    self.rate = 0.005;
                } else if t < tt {
                    let f = (t - tt / 2) as f32 / (tt / 2) as f32;
                    let mut k = f * f * f * 15.0;
                    if k > 1.0 {
                        k = 1.0;
                    }
                    let a = rv3(self.e, off::UNUSED_138);
                    let b = rv3(self.e, off::UNUSED_144);
                    let g = 1.0 - k;
                    self.t_twist = [a[0] * k + b[0] * g, a[1] * k + b[1] * g, a[2] * k + b[2] * g];
                    self.rate = 0.03;
                } else {
                    self.t_twist = rv3(self.e, off::UNUSED_138);
                }
                // 0x00417bcf
                self.t_rot_r = [0.0, 90.0, 0.0];
                if self.two_handed() {
                    self.grip(2.0);
                }
            }
            // 0x00417c84: bow and crossbow aim (the bow can be in either hand).
            0x16 => {
                let s7 = self.eq(7).1;
                if s7 != 6 && s7 != 7 {
                    self.t_off_l[1] += 3.0;
                    self.t_off_l[0] -= 4.0;
                    self.t_off_l[2] += 2.0;
                    self.t_off_r[1] += 2.0;
                    self.t_off_r[0] -= 10.0;
                    self.t_off_r[2] += 1.0;
                    self.t_body_rot[2] = -60.0;
                    self.t_rot_l[2] = 60.0;
                    self.t_rot_l[1] = -90.0;
                    self.t_rot_r[2] = 60.0;
                    if self.eq(6).0 == 3 {
                        self.m_weapon_r = self.fixed_model(0x348);
                    }
                    if t < 300 {
                        self.t_off_l[2] -= 1.0;
                        self.t_rot_l[0] += 20.0;
                        self.t_rot_l[1] -= 20.0;
                    }
                } else {
                    // 0x00417e1d
                    self.t_off_r[1] += 3.0;
                    self.t_off_r[0] += 4.0;
                    self.t_off_r[2] += 2.0;
                    self.t_off_l[1] += 2.0;
                    self.t_off_l[0] += 10.0;
                    self.t_off_l[2] += 1.0;
                    self.t_body_rot[2] = 60.0;
                    self.t_rot_r[2] = -60.0;
                    self.t_rot_r[1] = -90.0;
                    self.t_rot_l[2] = -60.0;
                    if self.eq(7).0 == 3 {
                        self.m_weapon_l = self.fixed_model(0x348);
                    }
                    if t < 300 {
                        self.t_off_r[2] -= 1.0;
                        self.t_rot_r[0] += 20.0;
                        self.t_rot_r[1] += 20.0;
                    }
                }
            }
            // 0x00417f9e: charged cast (`creature+0x13b4`).
            0x42 => {
                let wu = self.wu();
                if self.inp.charge > 0.0 {
                    if t < wu {
                        self.t_body_rot[2] = -90.0;
                        self.t_off_r = [2.0, -2.0, 1.0];
                        self.t_rot_r = [0.0, 10.0, 0.0];
                        self.m_weapon_l = self.m_weapon_r;
                    } else if t < wu + 300 {
                        self.t_body_rot[2] = 10.0;
                        self.t_off_l = [-2.0, -2.0, 1.0];
                        self.t_rot_l = [0.0, -10.0, 0.0];
                        self.m_weapon_l = self.m_weapon_r;
                        self.m_weapon_r = None;
                    } else if t < self.tt() {
                        self.m_weapon_r = None;
                        self.t_body_rot[2] = 90.0;
                        self.m_weapon_l = None;
                    }
                    // 0x00418129
                    self.rate = 0.03;
                    if t > self.wu() && t < self.tt() {
                        let mut z = (t - self.wu()) as f32 * 360.0 / 200.0;
                        self.t_lean[2] = z;
                        if z > 700.0 {
                            z = 700.0;
                            self.t_lean[2] = z;
                        }
                    }
                } else if t < wu {
                    self.t_body_rot[2] = -90.0;
                    self.t_off_r = [2.0, -2.0, 1.0];
                    self.t_rot_r = [0.0, 10.0, 0.0];
                } else if t < self.tt() {
                    self.t_body_rot[2] = 90.0;
                    self.m_weapon_r = None;
                }
            }
            // 0x0041825e
            0x43 => {
                self.t_body_rot[2] = rf(self.e, off::CHARGED_MP) * -90.0;
                self.t_off_r = [2.0, -2.0, 1.0];
                self.t_rot_r = [0.0, 10.0, 0.0];
                self.hand_r[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_r[2];
                self.hand_l[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_l[2];
                if rf(self.e, off::CHARGED_MP) > 0.0 {
                    self.m_weapon_l = self.m_weapon_r;
                }
            }
            // 0x004183ac: uses the entity's mode time, not the animation time.
            0x47 => {
                let mt = self.mode_time();
                if mt < self.wu() {
                    self.t_off_r = [-3.0, -2.0, 10.0];
                    self.t_off_l = [3.0, -2.0, 10.0];
                    self.m_weapon_l = self.fixed_model(0x835);
                    self.weapon_scale_l *= 1.5;
                    self.t_body_rot[0] = 40.0;
                    self.t_lean[0] = 10.0;
                } else if mt < self.du() + self.wu() {
                    self.rate = 0.01;
                    self.t_off_r = [0.0, 5.0, 0.0];
                    self.t_off_l = [0.0, 5.0, 0.0];
                    self.t_body_rot[0] = -20.0;
                    self.t_lean[0] = -40.0;
                } else {
                    self.t_off_r = [2.0, -1.0, 0.0];
                    self.t_off_l = [-2.0, -1.0, 0.0];
                }
            }
            // 0x00418591: bow and crossbow shot.
            0x15 => {
                let s7 = self.eq(7).1;
                if s7 != 6 && s7 != 7 {
                    if self.eq(6).1 == 7 {
                        self.t_rot_l[1] = -90.0;
                    }
                    if t < self.tt() * 2 {
                        if self.eq(6).1 == 6 {
                            self.t_off_l[1] += 5.0;
                            self.t_off_l[0] -= 7.0;
                        } else {
                            self.t_off_l[1] += 2.0;
                            self.t_off_l[0] -= 3.0;
                        }
                        // 0x00418650
                        self.t_off_l[2] += 4.0;
                        self.t_off_r[1] += 3.0;
                        self.t_off_r[0] -= 10.0;
                        self.t_off_r[2] += 3.0;
                        self.t_body_rot[2] -= 70.0;
                        self.t_rot_l[2] = 70.0;
                        self.t_rot_r[2] = 70.0;
                        if t < self.tt() / 2 {
                            self.t_off_l[2] -= 2.0;
                            self.t_rot_l[0] = -5.0;
                            self.t_off_r = [-5.0, 1.0, 0.0];
                            self.m_weapon_r = None;
                            self.t_body_rot[2] = -50.0;
                            self.t_rot_l[2] = 70.0;
                            return;
                        }
                    } else {
                        // 0x004187b7
                        self.t_off_l[0] -= 1.0;
                        self.t_rot_l[0] = -30.0;
                        self.t_rot_r[0] = -30.0;
                        self.t_rot_r[1] = 45.0;
                        if self.eq(6).1 == 7 {
                            self.t_rot_l[2] = 50.0;
                        }
                    }
                    // 0x0041882b
                    if self.eq(6).0 == 3 {
                        self.m_weapon_r = self.fixed_model(0x348);
                    }
                } else {
                    if s7 == 7 {
                        self.t_rot_r[1] = -90.0;
                    }
                    if t < self.tt() * 2 {
                        if self.eq(7).1 == 6 {
                            self.t_off_r[1] += 5.0;
                            self.t_off_r[0] += 7.0;
                        } else {
                            self.t_off_r[1] += 2.0;
                            self.t_off_r[0] += 3.0;
                        }
                        // 0x004188f0
                        self.t_off_r[2] += 4.0;
                        self.t_off_l[1] += 3.0;
                        self.t_off_l[0] += 10.0;
                        self.t_off_l[2] += 3.0;
                        self.t_body_rot[2] = 70.0;
                        self.t_rot_r[2] = -70.0;
                        self.t_rot_l[2] = -70.0;
                        if t < self.tt() / 2 {
                            self.t_off_r[2] -= 2.0;
                            self.t_rot_r[0] = -5.0;
                            self.t_off_l = [5.0, 1.0, 0.0];
                            self.m_weapon_l = None;
                            self.t_body_rot[2] = 50.0;
                            self.t_rot_r[2] = -70.0;
                            return;
                        }
                    } else {
                        // 0x00418a4d
                        self.t_off_r[0] += 1.0;
                        self.t_rot_r[0] = -30.0;
                        self.t_rot_l[0] = -30.0;
                        self.t_rot_l[1] = -45.0;
                    }
                    // 0x00418aa0
                    if self.eq(7).0 == 3 {
                        self.m_weapon_l = self.fixed_model(0x348);
                    }
                }
            }
            // 0x00418ac6
            0x1c => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.t_body_rot[2] = -80.0;
                    self.t_lean[2] = 0.0;
                    self.t_rot_r = [-90.0, 90.0, -90.0];
                    self.t_rot_l = [20.0, 90.0, 90.0];
                } else if t < wu + du {
                    self.t_body_rot[2] = -90.0;
                    self.t_lean[2] = (t - wu) as f32 * 360.0 / du as f32;
                    self.t_rot_l = [0.0, 100.0, 90.0];
                    self.t_rot_r = [0.0, 90.0, 0.0];
                    self.rate = 0.2;
                } else {
                    self.t_body_rot[2] = 80.0;
                    self.t_lean[2] = 0.0;
                    self.t_rot_l = [90.0, 90.0, 270.0];
                    self.t_rot_r = [0.0, 90.0, 90.0];
                }
            }
            // 0x00418cdf
            0x3c => {
                if t < 600 {
                    self.t_body_rot[2] = -80.0;
                    self.t_lean[2] = 0.0;
                    self.t_rot_r = [-90.0, 90.0, -90.0];
                    self.t_rot_l = [20.0, 90.0, 90.0];
                } else if t < 5000 {
                    self.t_body_rot[2] = 0.0;
                    self.t_lean[2] = (t - 600) as f32 * 360.0 / 300.0;
                    self.t_rot_l = [0.0, -90.0, 0.0];
                    self.t_rot_r = [0.0, 90.0, 0.0];
                    self.rate = 0.2;
                } else {
                    self.t_body_rot[2] = 80.0;
                    self.t_lean[2] = 0.0;
                    self.t_rot_l = [90.0, 90.0, 270.0];
                    self.t_rot_r = [0.0, 90.0, 90.0];
                }
            }
            // 0x00418e84: alternating punches every 500 ms.
            0x3f => {
                let wu = self.wu();
                if t < self.du() + 400 + wu {
                    let alt = t > wu && ((t - wu + 100) / 500) % 2 != 1;
                    if alt {
                        if ((t - self.wu() + 100) / 1000) % 2 != 0 {
                            self.t_off_l = [2.0, -2.0, 12.0];
                            self.t_off_r = [0.0, 0.0, -5.0];
                            self.rate = 0.03;
                            self.t_body_rot = [-10.0, 0.0, 20.0];
                            self.t_lean = [-5.0, 0.0, 10.0];
                        } else {
                            self.t_off_l = [0.0, 0.0, -5.0];
                            self.t_off_r = [-2.0, -2.0, 12.0];
                            self.rate = 0.03;
                            self.t_body_rot = [-10.0, 0.0, -20.0];
                            self.t_lean = [-5.0, 0.0, -10.0];
                        }
                    } else {
                        // 0x004190d7
                        self.t_off_l = [2.0, -2.0, 12.0];
                        self.t_off_r = [-2.0, -2.0, 12.0];
                        self.t_body_rot[0] = 20.0;
                        self.t_lean[0] = 10.0;
                    }
                }
            }
            // 0x0041916a
            0x45 => {
                let wd = self.wu() + self.du();
                if t < wd {
                    self.rate = 0.03;
                    self.t_lean[2] = t as f32 * -360.0 / wd as f32;
                    self.t_rot_r = [-90.0, -90.0, -90.0];
                    self.t_off_r = [4.0, 3.0, 0.0];
                    self.t_body_rot[2] = t as f32 * -60.0 / wd as f32;
                    self.t_rot_l = [0.0, 0.0, 90.0];
                } else {
                    self.t_rot_l = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-60.0, -60.0, -60.0];
                }
            }
            // 0x00419347
            0x1d => {
                if t < self.wu() {
                    self.t_rot_r = [-80.0, -90.0, 60.0];
                    self.t_rot_l = [-80.0, 90.0, -60.0];
                    self.t_off_r[1] += 3.0;
                    self.t_off_r[0] -= 5.0;
                    self.t_off_l[0] += 5.0;
                } else if t < self.tt() {
                    self.t_body_rot = [0.0, 0.0, -30.0];
                    self.t_off_r[1] -= 5.0;
                    self.t_off_l[1] -= 5.0;
                    self.t_rot_l = [-90.0, 90.0, 110.0];
                    self.t_rot_r = [-90.0, -90.0, -110.0];
                    self.rate = 0.04;
                    self.t_root_off[1] = -0.5;
                } else {
                    // 0x0041952a: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    self.t_rot_l[0] = -90.0;
                    self.t_body_rot[2] = 20.0;
                    self.t_rot_r = [-120.0, -90.0, 30.0];
                    self.guard_pose([-80.0, 40.0, 60.0]);
                }
            }
            // 0x00419748
            0x23 => {
                if t < self.wu() {
                    self.t_rot_r = [-90.0, 0.0, 0.0];
                    self.t_rot_l = [-90.0, 0.0, 40.0];
                    self.t_off_r = [1.0, -7.0, 0.0];
                    self.t_off_l = [-1.0, -7.0, 0.0];
                    self.t_body_rot[2] = -40.0;
                    self.t_root_off[1] = -0.5;
                    self.rate = 0.005;
                    self.foot_flag = true;
                } else if t < self.tt() {
                    self.t_rot_r = [-90.0, -60.0, -30.0];
                    self.t_rot_l = [-90.0, 60.0, -50.0];
                    self.t_off_r = [3.0, 5.0, 2.0];
                    self.t_off_l = [6.0, 8.0, 2.0];
                    self.t_body_rot[2] = 40.0;
                    self.rate = 0.05;
                    self.foot_flag = true;
                } else {
                    // 0x0041996e
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -60.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        self.guard_pose([-80.0, 40.0, 60.0]);
                    }
                }
            }
            // 0x00419a23
            0x22 => {
                self.t_rot_r = [-90.0, -90.0, 80.0];
                self.t_body_rot[2] = -80.0;
            }
            // 0x00419a6f
            5 => {
                self.t_rot_r = [-90.0, 0.0, -40.0];
                self.t_rot_l = [-90.0, 0.0, 40.0];
                self.hand_r[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_r[2];
                self.hand_l[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_l[2];
                self.t_off_r = [1.0, -7.0, 0.0];
                self.t_off_l = [-1.0, -7.0, 0.0];
                self.t_body_rot[2] = -40.0;
                self.rate = 0.005;
            }
            // 0x00419bee
            6 => {
                if self.e.0[off::SPEC] == 1 {
                    self.t_rot_r = [-50.0, -60.0, 40.0];
                    self.t_rot_l = [-90.0, 90.0, -50.0];
                    self.hand_r[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_r[2];
                    self.hand_l[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_l[2];
                    self.t_off_r = [-3.0, 7.0, 3.0];
                    self.t_off_l = [-2.0, 2.0, 0.0];
                    self.t_body_rot[2] = -20.0;
                    self.rate = 0.005;
                } else {
                    self.t_rot_r = [-90.0, 0.0, -40.0];
                    self.t_rot_l = [-90.0, 90.0, -40.0];
                    self.hand_r[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_r[2];
                    self.hand_l[2] = cosf(t as f32 * 0.5) * 0.5 + self.hand_l[2];
                    self.t_off_r = [1.0, -7.0, 0.0];
                    self.t_off_l = [-1.0, 0.0, 1.0];
                    self.t_body_rot[2] = -40.0;
                    self.rate = 0.005;
                }
            }
            // 0x00419ebd
            8 => {
                if self.m_weapon_l.is_some() {
                    if t < 100 {
                        self.t_body_rot[2] = 80.0;
                        self.t_rot_l = [0.0, 0.0, 90.0];
                        self.t_rot_r = [-45.0, 45.0, 0.0];
                    } else if t < 500 {
                        self.t_body_rot[2] = -90.0;
                        self.t_rot_l = [0.0, 60.0, 90.0];
                        self.t_off_l[2] += 5.0;
                        self.t_rot_r = [-90.0, 0.0, 45.0];
                    } else {
                        self.t_body_rot[2] = -30.0;
                        self.t_rot_l = [0.0, 100.0, 60.0];
                        self.t_rot_r = [-45.0, 45.0, 0.0];
                    }
                } else {
                    if t < 100 {
                        self.t_body_rot[2] = -80.0;
                        self.t_rot_r = [0.0, 0.0, 90.0];
                    } else if t < 500 {
                        self.t_body_rot[2] = 90.0;
                        self.t_rot_r = [0.0, 60.0, -90.0];
                        self.t_rot_r[2] += 5.0;
                        self.t_off_l[0] += 2.0;
                        self.t_off_r[0] += 2.0;
                        self.t_off_l[1] += 3.0;
                        self.t_off_r[1] += 3.0;
                        self.t_off_l[2] += 3.0;
                        self.t_off_r[2] += 3.0;
                    } else {
                        self.hand_r[1] += 2.0;
                        self.hand_r[2] += 3.0;
                        self.hand_l = self.hand_r;
                        self.hand_l[0] -= 3.0;
                        self.hand_l[2] -= 3.0;
                        self.elbow_l[1] += 10.0;
                        self.elbow_l[2] -= 4.0;
                        self.t_rot_r[0] = 0.0;
                        self.t_rot_r[1] = 20.0;
                    }
                    // 0x0041a29a
                    self.hand_l = self.hand_r;
                    self.hand_l[0] -= 3.0;
                    self.hand_l[2] -= 3.0;
                    self.t_rot_l = self.t_rot_r;
                }
            }
            // 0x0041a2fd: uses the entity's mode time for the end.
            0x41 => {
                if self.mode_time() < self.tt() {
                    self.t_body_rot = [0.0, 0.0, sinf(t as f32 * 0.03) * 90.0];
                    self.t_off_r = [0.0, 0.0, 0.0];
                    self.t_off_l = [0.0, 0.0, 0.0];
                    self.rate = 0.04;
                }
            }
            // 0x0041a3e6
            0x1b => {
                self.t_body_rot[2] = -30.0;
                self.t_rot_r = [0.0, -45.0, -60.0];
                self.t_rot_l = [-45.0, -45.0, 0.0];
                self.hand_sway(t);
            }
            // 0x0041a5c1
            0x17 => {
                if t < self.wu() {
                    self.t_body_rot[2] = -30.0;
                    self.t_rot_r = [0.0, -45.0, -60.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                    self.hand_sway(t);
                } else {
                    self.t_body_rot[2] = 60.0;
                    self.t_rot_r = [0.0, -45.0, -60.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                }
            }
            // 0x0041a80b
            0x1a => {
                if t < self.wu() {
                    self.t_body_rot[2] = -30.0;
                    self.t_rot_r = [0.0, -45.0, -60.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                    self.hand_sway(t);
                } else if t < self.tt() {
                    self.t_body_rot[2] = 90.0;
                    self.t_rot_r = [-90.0, 0.0, -90.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                    self.t_off_r = [3.0, 0.0, 1.0];
                    self.rate = 0.03;
                } else {
                    self.t_rot_r = [-60.0, 0.0, 0.0];
                    self.t_off_r = [3.0, 0.0, 1.0];
                }
            }
            // 0x0041ab1a
            0x19 => {
                if t < self.wu() {
                    self.hand_sway_only(t);
                    self.t_body_rot[2] = -20.0;
                } else if t < self.tt() {
                    self.t_root_off[0] = -0.5;
                    self.t_body_rot[2] = 60.0;
                    self.t_lean[2] = 30.0;
                    self.t_rot_r = [0.0, -45.0, -60.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                    self.t_off_r = [2.0, 0.0, 0.0];
                    self.foot_flag = true;
                }
            }
            // 0x0041ad90
            0x18 => {
                if t < self.wu() {
                    self.hand_sway_only(t);
                    self.t_body_rot[2] = 20.0;
                } else if t < self.tt() {
                    self.t_root_off[0] = 0.5;
                    self.t_body_rot[2] = -60.0;
                    self.t_lean[2] = -30.0;
                    self.t_rot_l = [0.0, 45.0, 60.0];
                    self.t_rot_r = [-45.0, 45.0, 0.0];
                    self.t_off_l = [-2.0, 0.0, 0.0];
                    self.foot_flag = true;
                }
            }
            // 0x0041b006
            0xb => {
                if self.app_flags() & 4 == 0 {
                    self.t_body_rot[2] = 60.0;
                    self.t_rot_r = [0.0, -45.0, -60.0];
                    self.t_rot_l = [-45.0, -45.0, 0.0];
                }
            }
            // 0x0041b08b
            0x3d => {
                let (wu, du) = (self.wu(), self.du());
                if t < wu {
                    self.hand_circles(t);
                } else if t < wu + du {
                    self.t_body_rot[0] = -20.0;
                    self.t_body_rot[2] = 10.0;
                    self.t_off_l = [2.0, 4.0, 0.0];
                    self.t_off_r = [-2.0, 6.0, 0.0];
                    self.t_rot_l = [0.0, 45.0, 0.0];
                    self.t_rot_r = [0.0, -45.0, 0.0];
                } else if t < self.tt() {
                    self.t_off_l = [-3.0, -6.0, 0.0];
                    self.t_off_r = [3.0, -6.0, 0.0];
                    self.t_rot_l = [-90.0, 0.0, 130.0];
                    self.t_rot_r = [-90.0, 0.0, -130.0];
                    self.rate = 0.03;
                }
            }
            // 0x0041b3f0
            0x3e => self.hand_circles(t),
            // 0x0041b54e
            9 => {
                if t < self.wu() {
                    self.t_body_rot[2] = 80.0;
                    self.t_rot_l = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-45.0, 45.0, 0.0];
                    if self.mode() == 0x30 {
                        self.t_lean[2] = (-t) as f32 / 300.0 * 360.0;
                    }
                } else {
                    self.t_body_rot[2] = -90.0;
                    self.t_rot_l = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-90.0, 0.0, 45.0];
                }
            }
            // 0x0041b689
            0x44 => {
                if t < self.wu() {
                    self.t_body_rot[0] = -20.0;
                    self.t_rot_l = [-90.0, 0.0, 0.0];
                    self.t_rot_r = [-90.0, 0.0, 0.0];
                    self.t_off_r = [1.0, -2.0, 0.0];
                    self.t_off_l = [-1.0, -2.0, 0.0];
                } else {
                    self.t_body_rot[0] = 0.0;
                    self.t_rot_l = [0.0, 45.0, 0.0];
                    self.t_rot_r = [0.0, -45.0, 0.0];
                    self.t_off_r = [-2.0, 5.0, 0.0];
                    self.t_off_l = [2.0, 6.0, 0.0];
                }
            }
            // 0x0041b850
            0xa => {
                if t < self.wu() {
                    self.t_body_rot[2] = 80.0;
                    self.t_rot_l = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-45.0, 45.0, 0.0];
                    if self.mode() == 0x30 {
                        self.t_lean[2] = (-t) as f32 / 300.0 * 360.0;
                    }
                } else if t < self.tt() {
                    self.t_body_rot[2] = -90.0;
                    self.t_rot_l = [0.0, 0.0, 90.0];
                    self.t_rot_r = [-90.0, 0.0, 45.0];
                    // 0x0041be09 without the halving.
                    self.hand_l[0] = sinf((t - 600) as f32 * 0.1) + self.hand_l[0];
                    self.rate = 0.03;
                } else {
                    // 0x0041b9f3: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -90.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        if self.eq(6).1 == 0xd {
                            self.guard_pose([0.0, 0.0, 90.0]);
                        } else {
                            self.guard_pose([-80.0, 40.0, 60.0]);
                        }
                    }
                }
            }
            // 0x0041badc
            0x14 => {
                if t > self.wu() && t < self.tt() {
                    self.t_body_rot[2] = 45.0;
                    self.t_lean[2] = 45.0;
                    self.t_rot_r = [0.0, -90.0, -90.0];
                    self.t_off_r = [8.0, 0.0, 4.0];
                    self.t_rot_l = [0.0, -90.0, -30.0];
                    self.t_off_l = [-1.0, -4.0, -1.0];
                    self.shot_tail();
                } else {
                    self.cast_idle(t, 0.01);
                }
            }
            // 0x0041be27
            0x13 => {
                if t > self.wu() && t < self.tt() {
                    self.t_body_rot[2] = -20.0;
                    self.t_lean[2] = -20.0;
                    self.t_rot_l = [0.0, 90.0, 40.0];
                    self.t_off_l = [0.0, 8.0, 4.0];
                    self.t_rot_r = [0.0, 90.0, 30.0];
                    self.t_off_r = [1.0, -4.0, -1.0];
                    self.shot_tail();
                } else {
                    self.cast_idle(t, 0.005);
                }
            }
            // 0x0041c115
            0x1e => {
                let wu = self.wu();
                if t < wu {
                    let f = t as f32 / wu as f32;
                    self.t_rot_r = [-80.0, 90.0, -90.0];
                    self.t_off_r = [-10.0, 8.0, 8.0];
                    self.t_body_rot[2] = -45.0;
                    self.t_lean[2] = f * -20.0;
                    self.t_root_off[0] = f * 0.0;
                    if combo_is(self.inp.combo, 2) && t < self.du() / 2 + self.wu() {
                        self.t_lean[2] = f * 360.0 + self.t_lean[2];
                    }
                    self.rate = 0.04;
                    self.combo_tail();
                } else if t < self.rc() + wu + self.du() {
                    let mut g = (t - self.wu()) as f32 * 1.2 / self.du() as f32;
                    if g > 1.0 {
                        g = 1.0;
                    }
                    let h = 1.0 - g;
                    self.t_root_off[0] = h * 0.0 - g * 0.8;
                    self.t_rot_r = mix([-110.0, 60.0, 0.0], g, [-90.0, 90.0, 48.0], h);
                    self.t_off_r = mix([-12.0, 0.0, -1.0], g, [-10.0, 8.0, 8.0], h);
                    self.t_head_rot[0] = -10.0;
                    self.t_body_rot[2] = g * 90.0 - h * 45.0;
                    self.t_lean[2] = g * 30.0;
                    self.rate = 0.05;
                    self.foot_yaw = g * 10.0;
                    self.combo_tail();
                } else {
                    self.t_off_r = [2.0, 0.0, 0.0];
                    self.t_rot_r = [0.0, 0.0, 0.0];
                    self.t_body_rot[2] = -30.0;
                    self.rate = 0.03;
                    self.combo_tail();
                }
            }
            // 0x0041c67c
            0x1f => {
                let wu = self.wu();
                if t < wu {
                    self.t_rot_r = [0.0, -90.0, 0.0];
                    self.t_off_r = [-8.0, 6.0, 6.0];
                    self.t_head_rot[0] = -10.0;
                    self.t_head_rot[2] = 30.0;
                    self.t_body_rot[2] = 45.0;
                    self.rate = 0.05;
                    self.t_root_off[0] = -0.0;
                    self.t_lean[2] = 20.0;
                    let f = t as f32 / wu as f32;
                    if combo_is(self.inp.combo, 2) && t < self.du() / 2 + self.wu() {
                        self.t_lean[2] -= f * 360.0;
                    }
                } else if t < self.tt() {
                    let mut g = (t - self.wu()) as f32 * 1.2 / self.du() as f32;
                    if g > 1.0 {
                        g = 1.0;
                    }
                    let h = 1.0 - g;
                    self.t_root_off[0] = g * 0.8 + h * -0.0;
                    self.t_head_rot[2] = -30.0;
                    self.t_rot_r = mix([0.0, -90.0, -130.0], g, [0.0, -90.0, 20.0], h);
                    self.t_off_r = mix([0.0, 2.0, 0.0], g, [-8.0, 6.0, 6.0], h);
                    self.t_body_rot[2] = h * 45.0 - g * 45.0;
                    self.t_lean[2] = g * -30.0;
                    self.t_lean[0] = g * -20.0;
                    self.rate = 0.05;
                    self.foot_yaw = g * 10.0;
                } else {
                    self.t_off_r = [2.0, 0.0, 0.0];
                    self.t_rot_r = [0.0, 0.0, 0.0];
                    self.t_body_rot[2] = -30.0;
                    self.rate = 0.03;
                }
                self.follow_right();
            }
            // 0x0041cbda
            0x20 => {
                let wu = self.wu();
                if t < wu {
                    let f = t as f32 / wu as f32;
                    self.t_off_r = [2.0, 2.0, 14.0];
                    self.t_rot_r = [50.0, 0.0, 0.0];
                    self.t_body_rot[2] = -45.0;
                    self.t_lean[2] = f * -20.0;
                    self.t_root_off[0] = f * 0.5;
                    self.rate = 0.04;
                } else if t < self.rc() + wu + self.du() {
                    let mut g = (t - self.wu()) as f32 * 2.0 / self.du() as f32;
                    if g > 1.0 {
                        g = 1.0;
                    }
                    let h = 1.0 - g;
                    self.t_lean[0] = h * 20.0 - g * 10.0;
                    self.t_rot_r = mix([-90.0, 0.0, -45.0], g, [90.0, 0.0, 45.0], h);
                    self.t_off_r = mix([0.0, 9.0, 0.0], g, [2.0, 2.0, 14.0], h);
                    self.t_body_rot[2] = g * 45.0 - h * 45.0;
                    self.rate = 0.05;
                } else {
                    self.t_off_r = [2.0, 0.0, 0.0];
                    self.t_rot_r = [0.0, 0.0, 0.0];
                    self.t_body_rot[2] = -30.0;
                    self.rate = 0.03;
                }
                self.follow_right();
            }
            // 0x0041cf8b
            0x21 => {
                let wu = self.wu();
                if t < wu {
                    self.t_off_r = [4.0, 2.0, 8.0];
                    self.t_rot_r = [-90.0, -180.0, 90.0];
                    self.t_body_rot[2] = -90.0;
                    self.t_root_off[1] = -1.0;
                    self.rate = 0.02;
                } else if t < wu + self.du() {
                    self.t_off_r = [6.0, 0.0, 0.0];
                    self.t_root_off[1] = 0.5;
                    self.t_body_rot[2] = 90.0;
                    self.t_rot_r = [-90.0, 0.0, -90.0];
                    self.foot_flag = true;
                    self.rate = 0.05;
                } else {
                    self.t_off_r = [2.0, 0.0, 0.0];
                    self.t_rot_r = [0.0, 0.0, 0.0];
                    self.t_body_rot[2] = -30.0;
                    self.rate = 0.03;
                }
                self.follow_right();
            }
            // 0x0041d123
            0x12 => {
                if t < self.wu() {
                    self.t_body_rot[2] = -40.0;
                    self.t_rot_l = [-90.0, -90.0, -30.0];
                    self.t_rot_r = [-90.0, 0.0, 20.0];
                    self.t_off_r[1] -= 6.0;
                } else if t < self.tt() {
                    self.t_body_rot[2] = 90.0;
                    self.t_rot_l = if self.eq(6).1 == 0xd { [0.0, 0.0, 40.0] } else { [-40.0, 0.0, -30.0] };
                    self.t_off_l = [-3.0, 0.0, 4.0];
                    self.t_rot_r = [-90.0, -90.0, -90.0];
                    self.t_off_r = [3.0, -1.0, 0.0];
                    self.foot_flag = true;
                    self.rate = 0.05;
                } else {
                    // 0x0041d305: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -90.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        if self.eq(6).1 == 0xd {
                            self.guard_pose([0.0, 0.0, 90.0]);
                        } else {
                            self.guard_pose([-80.0, 40.0, 60.0]);
                        }
                    }
                }
            }
            // 0x0041d3ee
            0x11 => {
                if t < self.wu() {
                    self.t_body_rot[2] = 40.0;
                    self.t_rot_r = [-90.0, 90.0, 30.0];
                    self.t_rot_l = [-90.0, 0.0, -20.0];
                    self.t_off_l[1] -= 6.0;
                } else if t < self.tt() {
                    self.t_body_rot[2] = -90.0;
                    self.t_rot_r = [-40.0, 0.0, 30.0];
                    self.t_off_r = [3.0, 0.0, 4.0];
                    self.t_rot_l = [-90.0, 90.0, 90.0];
                    self.t_off_l = [-3.0, -1.0, 0.0];
                    self.foot_flag = true;
                    self.rate = 0.05;
                } else {
                    // 0x0041d575: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -90.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        self.guard_pose([-80.0, 40.0, 60.0]);
                    }
                }
            }
            // 0x0041d62d
            0x32 => {
                self.t_rot_r = [60.0, -20.0, 20.0];
                self.t_off_r = [0.0, 2.0, 4.0];
                self.t_rot_l = [60.0, 20.0, -20.0];
                self.t_off_l = [0.0, 2.0, 4.0];
                let wu = self.wu();
                if t < wu {
                    self.t_body_rot[2] = 50.0;
                    self.t_rot_l[0] = -90.0;
                    self.t_rot_l[2] = 90.0;
                    self.t_rot_r[0] = -90.0;
                    self.t_off_l = [-4.0, -4.0, -2.0];
                    self.t_off_r = [0.0, 4.0, 1.0];
                } else if t < wu + self.du() {
                    let mut f = (t - self.wu()) as f32 / self.du() as f32 * 1.5;
                    if f > 1.0 {
                        f = 1.0;
                    } else if 0.0 > f {
                        f = 0.0;
                    }
                    self.t_lean[2] = f * -270.0;
                    self.t_lean[1] = f * -40.0;
                    self.t_body_off[2] += 4.0;
                    self.foot_yaw = f * 40.0;
                    self.t_rot_l[0] = -90.0;
                    self.t_rot_l[2] = 90.0;
                    self.t_rot_r[0] = -90.0;
                    self.t_off_l = [-4.0, -4.0, -2.0];
                    self.t_off_r = [0.0, 4.0, 1.0];
                    self.t_head_rot[0] = -30.0;
                    self.t_head_rot[2] = -60.0;
                    self.rate = 0.03;
                } else {
                    // 0x0041d9af
                    self.t_body_rot[2] = 0.0;
                    self.t_lean[2] = 0.0;
                    self.idle_circles(t);
                    self.t_body_off[2] = sinf(t as f32 * 0.005) * 0.5 + self.t_body_off[2];
                }
            }
            // 0x0041db88
            0xf => {
                if combo_is(self.inp.combo, 2) {
                    self.t_lean[2] = self.spin(t, -360.0);
                    self.rate = 0.03;
                } else {
                    self.t_lean[2] = 0.0;
                }
                self.t_twist = rv3(self.e, off::UNUSED_138);
                if t < self.wu() {
                    self.t_body_rot[2] = 40.0;
                    self.t_rot_l = [0.0, -90.0, 30.0];
                    self.t_off_l[1] -= 4.0;
                    self.t_off_l[2] += 4.0;
                    self.t_rot_r = [0.0, -90.0, -120.0];
                    self.t_off_r[1] -= 6.0;
                } else if t < self.tt() {
                    self.t_body_rot[2] = -90.0;
                    self.t_root_off[0] = -0.1;
                    self.t_rot_l = [-90.0, -90.0, -30.0];
                    self.t_rot_r = [0.0, -90.0, -120.0];
                    self.t_off_r[1] -= 6.0;
                } else {
                    // 0x0041ddaf: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -90.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        self.guard_pose([-80.0, 40.0, 60.0]);
                    }
                }
                // 0x0041dfe8
                if self.eq(6).1 == 0xd {
                    self.t_rot_l = [0.0, 0.0, 90.0];
                }
            }
            // 0x0041e029
            0x10 => {
                self.t_twist = rv3(self.e, off::UNUSED_138);
                if combo_is(self.inp.combo, 1) {
                    self.t_lean[2] = self.spin(t, -360.0);
                    self.rate = 0.03;
                } else {
                    self.t_lean[2] = 0.0;
                }
                if t < self.wu() {
                    self.t_body_rot[2] = 90.0;
                    self.t_rot_r = [0.0, -90.0, 0.0];
                    self.t_rot_l = [-90.0, 0.0, 90.0];
                    self.t_off_r[1] += 6.0;
                    self.t_off_r[2] += 4.0;
                } else if t < self.tt() {
                    self.t_body_rot[2] = 20.0;
                    self.t_rot_l = [-90.0, -60.0, 90.0];
                    self.t_off_l[1] -= 4.0;
                    self.t_off_l[2] += 4.0;
                    self.t_rot_r = [0.0, -90.0, -220.0];
                    self.t_off_r[1] -= 6.0;
                    self.t_root_off[0] = 0.1;
                    self.foot_l[1] -= 4.0;
                    self.foot_r[1] += 4.0;
                    self.foot_flag = true;
                } else {
                    // 0x0041e2fc: the animation ends.
                    self.t_twist = [0.0; 3];
                    self.st.anim_mode = 0;
                    let ty = ri(self.e, off::TYPE);
                    if ty != 0x60 && ty != 0x57 {
                        self.t_rot_l[0] = -60.0;
                        self.t_body_rot[2] = 20.0;
                        self.t_rot_r = [-120.0, -90.0, 30.0];
                        self.guard_pose([-80.0, 40.0, 60.0]);
                    }
                }
                // 0x0041e535
                if self.eq(6).1 == 0xd {
                    self.t_rot_l = [0.0, 0.0, 90.0];
                }
            }
            // 0x0041e576: dazed (the timer at entity 0x120, see `anim_mode_for`).
            0x38 => {
                self.t_body_rot[0] += 30.0;
                self.t_off_l = [-2.0, 0.0, 0.0];
                self.t_off_r = [2.0, 0.0, 0.0];
                self.t_rot_l = [-10.0, -10.0, 90.0];
                self.t_rot_r = [-10.0, 10.0, -90.0];
            }
            _ => {}
        }
    }

    /// 0x004195c6 / 0x004195dd: after an attack the off hand guards; a two-handed grip (right
    /// weapon without a left one) moves both hands onto the weapon.
    fn guard_pose(&mut self, rot_l: V3) {
        self.t_rot_l = rot_l;
        self.t_off_l[1] -= 4.0;
        self.t_off_l[2] += 2.0;
        if self.m_weapon_r.is_some() && self.m_weapon_l.is_none() {
            self.hand_r[1] += 2.0;
            self.hand_r[2] += 3.0;
            self.hand_l = self.hand_r;
            self.hand_l[0] -= 3.0;
            self.hand_l[2] -= 3.0;
            self.elbow_l[1] += 10.0;
            self.elbow_l[2] -= 4.0;
            self.t_rot_r[0] = 0.0;
            self.t_rot_r[1] = 20.0;
            self.t_rot_l = self.t_rot_r;
        }
    }

    /// 0x0041a460..0x0041a5bc (animations 0x1b, 0x17, 0x1a): hands up 4 and swaying.
    fn hand_sway(&mut self, t: i32) {
        self.hand_sway_only(t);
    }

    /// The sway itself: `off.y += 4`, `off.x += sin(t * 0.04) / 2`, `off.z += cos(t * 0.05) / 2`,
    /// right then left.
    fn hand_sway_only(&mut self, t: i32) {
        self.t_off_r[1] += 4.0;
        self.t_off_l[1] += 4.0;
        self.t_off_r[0] = sinf(t as f32 * 0.04) * 0.5 + self.t_off_r[0];
        self.t_off_r[2] = cosf(t as f32 * 0.05) * 0.5 + self.t_off_r[2];
        self.t_off_l[0] = sinf(t as f32 * 0.04) * 0.5 + self.t_off_l[0];
        self.t_off_l[2] = cosf(t as f32 * 0.05) * 0.5 + self.t_off_l[2];
    }

    /// 0x0041b08b..0x0041b1fb (animation 0x3d before the wind-up ends, and 0x3e): both hands
    /// circle in front of the body, half a turn apart.
    fn hand_circles(&mut self, t: i32) {
        self.t_body_rot[0] = 20.0;
        self.t_head_rot[0] = 20.0;
        let a = t as f32 * 0.005;
        let s = sinf(a) * 5.0 + 5.0;
        self.t_off_l = [cosf(a) * 3.0 - 1.0, 5.0, s];
        let b = (f64::from(t as f32 * 0.005) + PI_D) as f32;
        let s = sinf(b) * 5.0 + 5.0;
        self.t_off_r = [cosf(b) * 3.0 + 1.0, 5.0, s];
    }

    /// 0x0041bc2d / 0x0041d9af: the casting idle of animations 0x13, 0x14, 0x32: arms raised
    /// with the hands circling.
    fn idle_circles(&mut self, t: i32) {
        self.t_rot_r = [60.0, -20.0, 20.0];
        let a = t as f32 * 0.01;
        let c = cosf(a) + 4.0;
        self.t_off_r = [0.0, sinf(a) + 2.0, c];
        self.t_rot_l = [60.0, 20.0, -20.0];
        let b = (f64::from(t as f32 * 0.01) + std::f64::consts::FRAC_PI_2) as f32;
        let c = cosf(b) + 4.0;
        self.t_off_l = [0.0, sinf(b) + 2.0, c];
    }

    /// 0x0041bc2d / 0x0041bf3c: outside the shot of animations 0x14 (`k` 0.01) and 0x13 (`k`
    /// 0.005): the casting idle, the body bobbing with `sin(t * k) / 2`.
    fn cast_idle(&mut self, t: i32, k: f32) {
        self.t_body_rot[2] = 0.0;
        self.t_lean[2] = 0.0;
        self.idle_circles(t);
        self.t_body_off[2] = sinf(t as f32 * k) * 0.5 + self.t_body_off[2];
        self.rate = 0.03;
    }

    /// 0x0041bbec: end of the shot pose of animations 0x13/0x14.
    fn shot_tail(&mut self) {
        self.t_head_rot[0] = -10.0;
        self.t_body_off[1] = 3.0;
        self.rate = 0.03;
    }

    /// 0x0041c5b3: animation 0x1e: the left hand copies the right (two-handed swing).
    fn combo_tail(&mut self) {
        self.follow_right();
    }

    /// 0x0041c5b3 / 0x0041cb3f: left offsets and rotation copy the right, the left hand sits
    /// just beside the right one.
    fn follow_right(&mut self) {
        self.t_off_l = self.t_off_r;
        self.t_rot_l = self.t_rot_r;
        self.hand_l = self.hand_r;
        self.hand_l[0] -= 0.2;
        self.hand_l[1] -= 0.2;
        self.hand_l[2] -= 1.0;
    }

    /// `clamp((t - windup) / duration, 0, 1) * k` (0x0041dba6), NaN passing through.
    fn spin(&self, t: i32, k: f32) -> f32 {
        let mut f = (t - self.wu()) as f32 / self.du() as f32;
        if f > 1.0 {
            f = 1.0;
        } else if 0.0 > f {
            f = 0.0;
        }
        f * k
    }
}

/// `(combo / 2) % 3 == r` on `creature+0x1314` (truncating division, C remainder).
fn combo_is(combo: i32, r: i32) -> bool {
    (combo / 2) % 3 == r
}

/// `a * f + b * g` per component (0x00451510 twice and 0x00412280).
fn mix(a: V3, f: f32, b: V3, g: f32) -> V3 {
    [a[0] * f + b[0] * g, a[1] * f + b[1] * g, a[2] * f + b[2] * g]
}

/// 0x00446950: the riding offset of a pet type.
fn riding_offset(ty: i32) -> V3 {
    let z = match ty {
        0x1a | 0x1e => -0.9,
        0x21 => -0.2,
        0x4a => -0.3,
        0x4b => 0.1,
        0x97 => -0.35,
        _ => 0.0,
    };
    [0.0, 0.0, z]
}

// ------------------------------------------------------------------------------------------
// The walk cycle as the world tick advances it.
// ------------------------------------------------------------------------------------------

/// The walk-cycle update of the shared world tick: `Server.exe 0x005447a6..0x00544e3e`
/// (`Cube.exe 0x0061e9c6..`, byte-identical; `analysis/notes/functions/
/// 005322d0_pseudo_542b95.md` section (r) and (t)). The server port leaves it out; the client
/// runs it for every creature in its 20 ms tick steps, right after the mounted-creature
/// section (q).
///
/// `mount` is the rider case of section (q): when the creature rides another one (id at
/// `creature+0x11c0` found, the mount's HP above 0, the rider's hostile type not 5), the
/// rider takes half the mount's blend and its phase and nothing else happens here. The
/// caller resolves that condition.
///
/// The walking branch advances the phase by the horizontal speed (scaled by
/// `sqrt(0.8 / scale.x)`, slowed while an attack locks the feet, doubled in mode 0x4f) and
/// raises the blend by the elapsed seconds; climbing and swimming use their own rates; in the
/// air the phase resets and the blend still grows. The blend is then capped at 1 and decays
/// toward 0 (`lerpRepeat(.., 0, dt, 0.005)`).
///
/// Not ported: the client-only footstep sound at 0x00544b4f (a `Sound` record when the phase
/// crosses a multiple of pi on the ground above speed 5, kind 0x20 or 0x21 + rand() % 3 on
/// type 3 blocks), which belongs to the audio module.
pub fn advance_walk_cycle(
    e: &EntityData,
    w: &mut WalkCycle,
    dt: i32,
    guard: f32,
    haste: bool,
    mount: Option<WalkCycle>,
) {
    let dt_f = dt as f32;
    let dt_s = dt_f * 0.001;
    // 0x5586d8 (0.8) / scale.x through `sqrtf` 0x004024e0.
    let sx = rf(e, off::SCALE);
    let s = || (0.8f32 / sx).sqrt();
    'after: {
        if let Some(m) = mount {
            // 0x005449af
            w.blend = m.blend * 0.5;
            w.phase = m.phase;
            break 'after;
        }
        let phys = ri(e, off::PHYS) as u32;
        let flags = ru16(e, off::FLAGS);
        let app = ru16(e, off::APP_FLAGS);
        let on_ground = phys & 1 != 0;
        let climbing = flags & 1 != 0 && phys & 4 != 0;
        let mode = e.0[off::MODE];
        let v = rv3(e, off::VEL);
        if on_ground || app & 2 != 0 || climbing || phys & 2 != 0 {
            let v2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            if v2 > 0.5 && mode != 0x6b {
                if climbing {
                    // 0x0054481b
                    let len = sqrt_d(v2);
                    w.phase = s() * ((len * dt_s) * 6.0) + w.phase;
                    w.blend = dt_s * 0.2 + w.blend;
                    break 'after;
                }
                // 0x005449c8
                let hlen = sqrt_d(v[0] * v[0] + v[1] * v[1]);
                let k;
                if on_ground {
                    let mt = ri(e, off::MODE_TIME);
                    let locked = mode == 0x30
                        || (mode == 0x36 && mt < cw_sim::skills::skill_total_time(e, guard, haste))
                        || (matches!(mode, 6 | 7 | 0x14 | 0x13 | 0x12 | 0x11 | 0xa)
                            && mt < cw_sim::skills::skill_windup(e, guard, haste, -1));
                    if !locked {
                        // 0x00544a50
                        let kk = (hlen * dt_s) * 1.5;
                        k = if mode == 0x4f { (s() * kk) * 2.0 } else { s() * kk };
                    } else {
                        // 0x00544ac7
                        k = s() * (((dt_f * 1e-4) * hlen) * 1.5);
                    }
                } else {
                    // 0x00544ccb
                    k = s() * (((dt_f * 0.002) * hlen) * 1.5);
                }
                w.phase = k + w.phase;
                w.blend = w.blend + dt_s;
                break 'after;
            }
        }
        // 0x00544d53
        if !on_ground && phys & 2 == 0 && !climbing {
            w.phase = 0.0;
            w.blend = s() * dt_s + w.blend;
        }
    }
    // 0x00544dfb
    if w.blend > 1.0 {
        w.blend = 1.0;
    }
    for _ in 0..dt.max(0) {
        w.blend = (0.0 - w.blend) * 0.005 + w.blend;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Models;

    impl ModelSource for Models {
        fn item_model(&self, _item: &[u8]) -> Option<u32> {
            None
        }
        fn model_size(&self, _model: u32) -> [i32; 3] {
            [8, 6, 10]
        }
        fn model_count(&self) -> usize {
            0x1000
        }
    }

    fn w16(e: &mut EntityData, o: usize, v: i16) {
        e.0[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn wf(e: &mut EntityData, o: usize, v: f32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn wi(e: &mut EntityData, o: usize, v: i32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// A standing humanoid: models 1..6 in the appearance, no tail or wings, unit scales.
    fn humanoid() -> EntityData {
        let mut e = EntityData::ZERO;
        for (i, o) in [off::HEAD_MODEL, off::HAIR_MODEL, off::HAND_MODEL, off::FOOT_MODEL, off::BODY_MODEL]
            .into_iter()
            .enumerate()
        {
            w16(&mut e, o, i as i16 + 1);
        }
        w16(&mut e, off::TAIL_MODEL, -1);
        w16(&mut e, off::SHOULDER2_MODEL, 6);
        w16(&mut e, off::WING_MODEL, -1);
        for o in [off::SCALE, off::SCALE + 4, off::SCALE + 8] {
            wf(&mut e, o, 1.0);
        }
        for o in (off::HEAD_SCALE..=off::WING_SCALE).step_by(4) {
            wf(&mut e, o, 1.0);
        }
        wf(&mut e, off::HAND_OFFSET, 6.0);
        wf(&mut e, off::HAND_OFFSET + 8, 5.0);
        wf(&mut e, off::FOOT_OFFSET, 2.0);
        wf(&mut e, off::FOOT_OFFSET + 8, -5.0);
        wf(&mut e, off::HEAD_OFFSET + 8, 5.0);
        wi(&mut e, off::PHYS, 1);
        wi(&mut e, 0x180, 1);
        for o in (0x168..0x17c).step_by(4) {
            wf(&mut e, o, 1.0);
        }
        e
    }

    fn inputs(models: &Models, walk: WalkCycle) -> PoseInputs<'_> {
        PoseInputs {
            models,
            dt_ms: 16,
            color: [1.0; 4],
            render_pos: [0; 3],
            render_rot: [0.0; 3],
            origin: [0; 2],
            step_z: 0.0,
            walk,
            mounted: false,
            pet: None,
            combo: 0,
            charge: 0.0,
            guard: 0.0,
            haste: false,
            hide: 0,
            anim_mode: None,
            hit_flash: 0.0,
        }
    }

    fn part(p: &Pose, slot: PartSlot) -> &PartTransform {
        p.parts.iter().find(|t| t.slot == slot).unwrap_or_else(|| panic!("no {slot:?}"))
    }

    /// Off-diagonal of the rotation-scale block.
    fn off_diag(m: &Mat4) -> f32 {
        let a = m.to_cols_array();
        [a[1], a[2], a[4], a[6], a[8], a[9]].iter().fold(0.0f32, |acc, v| acc.max(v.abs()))
    }

    #[test]
    fn idle_creature_has_unrotated_parts_with_appearance_models() {
        let e = humanoid();
        let models = Models;
        let pose = build_pose(&e, 0, &inputs(&models, WalkCycle::default()));
        assert_eq!(part(&pose, PartSlot::Head).model, 1);
        assert_eq!(part(&pose, PartSlot::Hair).model, 2);
        assert_eq!(part(&pose, PartSlot::HandLeft).model, 3);
        assert_eq!(part(&pose, PartSlot::HandRight).model, 3);
        assert!(part(&pose, PartSlot::HandRight).mirrored);
        assert_eq!(part(&pose, PartSlot::FootLeft).model, 4);
        assert_eq!(part(&pose, PartSlot::FootRight).model, 4);
        assert_eq!(part(&pose, PartSlot::Body).model, 5);
        assert_eq!(part(&pose, PartSlot::UpperArmLeft).model, 6);
        assert!(pose.parts.iter().all(|p| p.slot != PartSlot::Tail && p.slot != PartSlot::WingLeft));
        // The root is the scale 1 / 11.2 at the origin.
        let r = pose.root.to_cols_array();
        assert!((r[0] - 1.0 / 11.2).abs() < 1e-6 && (r[5] - r[0]).abs() < 1e-6 && (r[10] - r[0]).abs() < 1e-6);
        assert!(off_diag(&pose.root) < 1e-6);
        // Nothing turns at rest: every part keeps an axis-aligned scale.
        for p in &pose.parts {
            assert!(off_diag(&p.matrix) < 1e-5, "{:?} is rotated: {:?}", p.slot, p.matrix);
            let d = p.matrix.to_cols_array();
            assert!(d[0].abs() > 0.0 && d[5] > 0.0 && d[10] > 0.0, "{:?}", p.slot);
        }
        // The hands are mirror images across x.
        let l = part(&pose, PartSlot::HandLeft).matrix.w_axis;
        let rgt = part(&pose, PartSlot::HandRight).matrix.w_axis;
        assert!(l.x < 0.0 && rgt.x > 0.0);
        assert_eq!(pose.walk_blend, 0.0);
    }

    #[test]
    fn walking_feet_alternate() {
        let e = humanoid();
        let models = Models;
        // Centre of the foot model (size 8 x 6 x 10) in world space.
        let foot_z = |phase: f32| {
            let pose = build_pose(&e, 0, &inputs(&models, WalkCycle { blend: 0.2, phase }));
            (
                part(&pose, PartSlot::FootLeft).matrix.transform_point3(glam::Vec3::new(4.0, 3.0, 5.0)).z,
                part(&pose, PartSlot::FootRight).matrix.transform_point3(glam::Vec3::new(4.0, 3.0, 5.0)).z,
            )
        };
        let (l0, r0) = foot_z(0.0);
        let (l1, r1) = foot_z(std::f32::consts::PI);
        assert!(l0 > r0, "left foot up at phase 0: {l0} {r0}");
        assert!(r1 > l1, "right foot up at phase pi: {l1} {r1}");
        // The swing turns the feet in opposite directions.
        let pose = build_pose(&e, 0, &inputs(&models, WalkCycle { blend: 0.2, phase: 0.0 }));
        let fl = part(&pose, PartSlot::FootLeft).matrix.to_cols_array();
        let fr = part(&pose, PartSlot::FootRight).matrix.to_cols_array();
        assert!(fl[6] * fr[6] < 0.0, "{} {}", fl[6], fr[6]);
    }

    #[test]
    fn anim_mode_table() {
        let mut e = EntityData::ZERO;
        for (mode, anim) in [(0u8, 0), (1, 0x10), (0x16, 0x15), (0x44, 0x26), (0x50, 0x33), (0x6e, 0x49), (0x6f, 0)] {
            e.0[off::MODE] = mode;
            assert_eq!(anim_mode_for(&e), anim, "mode {mode:#x}");
        }
        e.0[off::MODE] = 0x30;
        assert_eq!(anim_mode_for(&e), 0x22);
        e.0[off::equip(6)] = 3;
        e.0[off::equip(6) + 1] = 0xd;
        assert_eq!(anim_mode_for(&e), 9);
        wi(&mut e, off::SLOWED, 1);
        assert_eq!(anim_mode_for(&e), 0x38);
    }

    #[test]
    fn walk_cycle_advances_on_the_ground_and_decays() {
        let mut e = humanoid();
        wf(&mut e, off::VEL, 3.0);
        let mut w = WalkCycle::default();
        for _ in 0..50 {
            advance_walk_cycle(&e, &mut w, 20, 0.0, false, None);
        }
        // Walking settles where the growth (dt_s per tick) meets the decay (0.995^dt): about 0.19.
        assert!(w.phase > 0.0 && w.blend > 0.15 && w.blend < 0.25, "{w:?}");
        // Standing still: the blend decays, the phase stays.
        wf(&mut e, off::VEL, 0.0);
        let before = w;
        advance_walk_cycle(&e, &mut w, 20, 0.0, false, None);
        assert_eq!(w.phase, before.phase);
        assert!(w.blend < before.blend);
        // In the air the phase resets.
        wi(&mut e, off::PHYS, 0);
        advance_walk_cycle(&e, &mut w, 20, 0.0, false, None);
        assert_eq!(w.phase, 0.0);
    }

    #[test]
    fn charge_clears_the_walk_blend() {
        let mut e = humanoid();
        e.0[off::MODE] = 0x2f;
        let models = Models;
        let pose = build_pose(&e, 100, &inputs(&models, WalkCycle { blend: 0.7, phase: 1.0 }));
        assert_eq!(pose.walk_blend, 0.0);
    }

    /// 0x0041f2a0..0x0041f337: the roll turns the root about `vel × (0, 0, 1)`, a horizontal
    /// axis across the direction of travel, so the figure tumbles forward.
    #[test]
    fn roll_tumbles_forward_about_a_horizontal_axis() {
        let mut e = humanoid();
        wf(&mut e, off::VEL, 1.0);
        // A quarter through the 600 ms roll: -90 degrees about (0, -1, 0).
        wi(&mut e, off::ROLL, 450);
        let models = Models;
        let pose = build_pose(&e, 0, &inputs(&models, WalkCycle::default()));
        let s = 1.0 / 11.2;
        let up = pose.root.transform_vector3(glam::Vec3::Z);
        let side = pose.root.transform_vector3(glam::Vec3::Y);
        // The side axis (across the travel direction) is the rotation axis: it stays put.
        assert!(side.abs_diff_eq(glam::Vec3::new(0.0, s, 0.0), 1e-5), "side {side:?}");
        // The top of the figure has turned toward +x, the direction of travel.
        assert!(up.abs_diff_eq(glam::Vec3::new(s, 0.0, 0.0), 1e-5), "up {up:?}");
    }

    #[test]
    fn matrix_helpers_match_glam() {
        let mut m = IDENTITY;
        rot_z(&mut m, 30.0);
        rot_x(&mut m, -20.0);
        translate(&mut m, 1.0, 2.0, 3.0);
        rot_axis(&mut m, 45.0, 0.0, 1.0, 0.0);
        let g = Mat4::from_rotation_z(30f32.to_radians())
            * Mat4::from_rotation_x((-20f32).to_radians())
            * Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0))
            * Mat4::from_axis_angle(glam::Vec3::Y, 45f32.to_radians());
        let a = Mat4::from_cols_array(&m);
        assert!(a.abs_diff_eq(g, 1e-5), "{a:?} vs {g:?}");
    }
}
