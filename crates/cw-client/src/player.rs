//! The local player's half of `cube::GameController::update` (`Cube.exe 0x00488ee0`): movement
//! input, the third-person camera, the attack and skill inputs, the special-item keys, the
//! respawn and the per-frame display bookkeeping. The client is authoritative for its own
//! entity: everything here writes the local player's entity block and creature state, which
//! the shared world tick (`cw_sim::update::update_creature` then
//! `cw_sim::physics::move_creature`, `World::tick` 0x0060c510) then moves, and the send loop
//! uploads.
//!
//! Tier B: the original's order, comparison directions and float shapes are kept; every block
//! cites its range. `update` is one 77 KB function; its gameplay ranges are split here by
//! responsibility (the interactions are in [`crate::interact`]). Range map, in frame order:
//!
//! | Range | Here |
//! |---|---|
//! | 0x0048b8ab..0x0048bd8c | [`build_skill_bar`] |
//! | 0x0048cda1..0x0048ce2e | [`LocalPlayer::pickup_step`] |
//! | 0x0048d4..(death latch, Ghidra line 1528) | [`LocalPlayer::death_latch`] |
//! | 0x00493b0f..0x0049413a | [`LocalPlayer::xp_display`] |
//! | 0x004943xx (per-creature `+0x13c8..+0x13d0`) | [`damage_display_tick`], [`damage_display_hit`] |
//! | 0x004965a2..0x0049662c | [`Camera::smooth`] |
//! | 0x0049736c..0x004973df | [`LocalPlayer::respawn`] (R while dead) |
//! | 0x00497723..0x00497892 | [`LocalPlayer::glide_steering`] |
//! | 0x00497892..0x00497950 | [`LocalPlayer::right_stick_camera`] |
//! | 0x00498049..0x00498078 | the movement gates in [`LocalPlayer::movement`] |
//! | 0x004982b3 + `applyMovementInput` 0x004a6b50 | [`LocalPlayer::apply_movement_input`] |
//! | 0x004982be..0x00498404 | [`LocalPlayer::movement_flags`] |
//! | 0x0049b18d..0x0049b58e | [`LocalPlayer::build_cursor`] |
//! | 0x0049b58e..0x0049c02f | [`LocalPlayer::attack_inputs`] |
//! | 0x0049c272..0x0049c2e6 | [`mode_timeout`] |
//! | 0x0049c2e6..0x0049ced6 | [`Camera::place`] |
//! | `onMouseMove` 0x0047ea00 (camera part) | [`Camera::on_mouse_move`], [`Camera::on_mouse_drag`] |
//! | `onMouseWheel` 0x0047ef40 | [`Camera::on_mouse_wheel`] |
//! | `onKeyDown` 0x0047e811, 0x0047e83c | [`toggle_lamp`], [`use_special_item`] |
//!
//! Helpers ported here because the server port has no counterpart: `skillCooldown` 0x0043e6a0,
//! `canInterrupt` 0x0043e350, the self-buff skills 0x00595850, the respawn point 0x005a03d0,
//! `Creature::resetForRespawn` 0x00447110, the matrix and lerp helpers (0x00423e70,
//! 0x00424610, 0x004244f0, 0x004243d0, 0x00424990, 0x00434b80, 0x00424a60, 0x00424730,
//! 0x00488e50, 0x004248a0, 0x004ac150, 0x00457460, 0x004aba20, 0x004573d0, 0x00468ca0).

// The comparisons keep the original's NaN behaviour, the clamps its two-compare shape and the
// sums their operand grouping.
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::manual_clamp, clippy::assign_op_pattern, clippy::excessive_precision, clippy::too_many_arguments, clippy::too_many_lines, clippy::nonminimal_bool, clippy::collapsible_else_if, clippy::float_cmp, clippy::needless_range_loop, clippy::neg_multiply, clippy::collapsible_if)]
#![allow(dead_code)]

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{Interact, Passive};
use cw_sim::combat::{Buff, CreatureState};
use cw_world::World;
use cw_world::inventory::Item;

use crate::input::{ControllerBytes, EdgeLatches};

// ---------------------------------------------------------------------------------------------
// Entity block accessors (`creature+0x10+X` is `entity+X`).

pub(crate) const K: f32 = 1.525_878_9e-5;

pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
pub(crate) fn i64_at(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn wi32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wu16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wi64(b: &mut [u8], o: usize, v: i64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn wf32(b: &mut [u8], o: usize, v: f32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
pub(crate) fn vec3_at(b: &[u8], o: usize) -> [f32; 3] {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}
pub(crate) fn set_vec3(b: &mut [u8], o: usize, v: [f32; 3]) {
    for (i, x) in v.iter().enumerate() {
        wf32(b, o + i * 4, *x);
    }
}
pub(crate) fn pos_of(e: &EntityData) -> [i64; 3] {
    [i64_at(&e.0, 0), i64_at(&e.0, 8), i64_at(&e.0, 16)]
}
pub(crate) fn set_pos(e: &mut EntityData, p: [i64; 3]) {
    for (i, v) in p.iter().enumerate() {
        wi64(&mut e.0, i * 8, *v);
    }
}

/// Entity offsets the input code touches (`creature+X` in the original is `entity+X-0x10`).
pub mod ent {
    /// `creature+0x34`: velocity.
    pub const VEL: usize = 0x24;
    /// `creature+0x40`: acceleration.
    pub const ACCEL: usize = 0x30;
    /// `creature+0x4c`: extra velocity.
    pub const EXTRA_VEL: usize = 0x3c;
    /// `creature+0x5c`: physics flags (1 on ground, 2 in water, 4 at a wall).
    pub const PHYS: usize = 0x4c;
    /// `creature+0x60`: hostile type.
    pub const HOSTILE: usize = 0x50;
    /// `creature+0x68`: current mode; `+0x6c` its time.
    pub const MODE: usize = 0x58;
    pub const MODE_TIME: usize = 0x5c;
    /// `creature+0x70`: the hit counter.
    pub const HIT_COUNTER: usize = 0x60;
    /// `creature+0x124`: flags (1 climb, 4 face the aim, 0x10 glide, 0x20 attackable,
    /// 0x40 run, 0x200 lamp, 0x400 skill 0x63).
    pub const FLAGS: usize = 0x114;
    /// `creature+0x128`: roll time; `+0x12c`: stun time.
    pub const ROLL: usize = 0x118;
    pub const STUN: usize = 0x11c;
    /// `creature+0x140`, `+0x141`: class, specialisation.
    pub const CLASS: usize = 0x130;
    pub const SPEC: usize = 0x131;
    /// `creature+0x144`: charged MP.
    pub const CHARGED_MP: usize = 0x134;
    /// `creature+0x160`: the aim ray hit, relative to the creature.
    pub const RAY_HIT: usize = 0x150;
    /// `creature+0x16c`, `+0x170`: HP, MP.
    pub const HP: usize = 0x15c;
    pub const MP: usize = 0x160;
    /// `creature+0x190`, `+0x194`: level, XP.
    pub const LEVEL: usize = 0x180;
    pub const XP: usize = 0x184;
    /// `creature+0x1a0`: the target id of a charged skill.
    pub const TARGET: usize = 0x190;
    /// `creature+0x1e8`: the consumable being used.
    pub const CONSUMING: usize = 0x1d8;
    /// Equipment slots (`creature+0x300 + i * 0x118`).
    pub const fn slot(i: usize) -> usize {
        0x2f0 + i * 0x118
    }
    /// `creature+0x1150`: the skill levels read by the skill bar (skill index 6..).
    pub const SKILLS_BAR: usize = 0x1140;
}

// ---------------------------------------------------------------------------------------------
// Math helpers.

/// `cos((double)(deg * 0.017453292f))` then single, as every rotation helper does it.
fn cos_deg(deg: f32) -> f32 {
    cw_math::cos(f64::from(deg * 0.017_453_292f32)) as f32
}
fn sin_deg(deg: f32) -> f32 {
    cw_math::sin(f64::from(deg * 0.017_453_292f32)) as f32
}

/// `_libm_sse2_sqrt_precise` on a float widened to double.
pub(crate) fn sqrt_f(x: f32) -> f32 {
    f64::from(x).sqrt() as f32
}

/// `0x004ac150(n, t)`: `x = 0; n times x = x + (1 - x) * t` in double (unrolled by eight in the
/// original, same operations), returned as single.
pub fn lerp_factor(n: i32, t: f32) -> f32 {
    let t = f64::from(t);
    let mut x = 0.0f64;
    let mut i = 0;
    while i < n {
        x = (1.0 - x) * t + x;
        i += 1;
    }
    x as f32
}

/// `0x004aba20(p, target, n, t)`: `p = (1 - f) * p + target * f`.
pub fn lerp_scalar(p: &mut f32, target: f32, n: i32, t: f32) {
    let f = lerp_factor(n, t);
    *p = (1.0 - f) * *p + target * f;
}

/// `0x00457460(p, target, n, t)` on three floats.
pub fn lerp_vec3(p: &mut [f32; 3], target: [f32; 3], n: i32, t: f32) {
    let f = lerp_factor(n, t);
    let g = 1.0 - f;
    let y = g * p[1] + target[1] * f;
    let z = g * p[2] + target[2] * f;
    p[0] = g * p[0] + target[0] * f;
    p[1] = y;
    p[2] = z;
}

/// `0x004573d0(p, target, n, t)` on a fixed value: `ftol((1 - f) * (float)p) +
/// ftol((float)target * f)`, single precision throughout.
pub fn lerp_fixed(p: &mut i64, target: i64, n: i32, t: f32) {
    let f = lerp_factor(n, t);
    let a = ((1.0f32 - f) * (*p as f32)) as i64;
    let b = ((target as f32) * f) as i64;
    *p = a.wrapping_add(b);
}

/// `0x00468ca0`: `(i64)(f * 65536) * v / 65536`, truncating.
fn fixed_scale(v: i64, f: f32) -> i64 {
    let k = (f * 65536.0f32) as i64;
    k.wrapping_mul(v) / 65536
}

/// `vec3i64::fromFloat` 0x0042c460: `(i64)(x * 65536)` per axis.
pub(crate) fn fix3(v: [f32; 3]) -> [i64; 3] {
    [(v[0] * 65536.0f32) as i64, (v[1] * 65536.0f32) as i64, (v[2] * 65536.0f32) as i64]
}
pub(crate) fn fix(v: f32) -> i64 {
    (v * 65536.0f32) as i64
}
/// `vec3f::fromFixed` 0x0042c4a0.
pub(crate) fn blocks3(v: [i64; 3]) -> [f32; 3] {
    [v[0] as f32 * K, v[1] as f32 * K, v[2] as f32 * K]
}
pub(crate) fn sub3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2])]
}
pub(crate) fn add3(a: [i64; 3], b: [i64; 3]) -> [i64; 3] {
    [a[0].wrapping_add(b[0]), a[1].wrapping_add(b[1]), a[2].wrapping_add(b[2])]
}
/// `vec3f::lengthSq` 0x00424860: `x*x + y*y + z*z`.
pub(crate) fn len_sq(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}
/// `vec3fNormalize` 0x004240f0.
pub(crate) fn normalize(v: &mut [f32; 3]) {
    let x = v[0];
    let s = 1.0f32 / sqrt_f(x * x + v[1] * v[1] + v[2] * v[2]);
    v[0] = x * s;
    v[1] = s * v[1];
    v[2] = s * v[2];
}
/// `vec3fLength` 0x00423f20.
pub(crate) fn length(v: [f32; 3]) -> f32 {
    sqrt_f(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
}
/// `vec3i64` dot 0x0043ac20: per axis `(a * b) / 65536` (64-bit, truncating), summed.
pub(crate) fn fixed_dot(a: [i64; 3], b: [i64; 3]) -> i64 {
    let mut s = a[0].wrapping_mul(b[0]) / 65536;
    s = s.wrapping_add(a[1].wrapping_mul(b[1]) / 65536);
    s.wrapping_add(a[2].wrapping_mul(b[2]) / 65536)
}

/// The 4x4 float matrix of the original (`float[16]`, index as in the decompilation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    /// `mat4Identity` 0x00423e70.
    pub const IDENTITY: Mat4 = Mat4([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    /// `mat4RotateZ` 0x00424610 (rows 0 and 1).
    pub fn rotate_z(&mut self, deg: f32) {
        let c = cos_deg(deg);
        let s = sin_deg(deg);
        let m = &mut self.0;
        for i in 0..4 {
            let a = m[i];
            m[i] = m[4 + i] * s + a * c;
            m[4 + i] = m[4 + i] * c - a * s;
        }
    }

    /// `0x004244f0` (rows 0 and 2): `r0 = r0 c - r2 s`, `r2 = r2 c + r0 s`.
    pub fn rotate_y(&mut self, deg: f32) {
        let c = cos_deg(deg);
        let s = sin_deg(deg);
        let m = &mut self.0;
        for i in 0..4 {
            let a = m[i];
            m[i] = a * c - m[8 + i] * s;
            m[8 + i] = m[8 + i] * c + a * s;
        }
    }

    /// `0x004243d0` (rows 1 and 2): `r1 = r2 s + r1 c`, `r2 = r2 c - r1 s`.
    pub fn rotate_x(&mut self, deg: f32) {
        let c = cos_deg(deg);
        let s = sin_deg(deg);
        let m = &mut self.0;
        for i in 0..4 {
            let a = m[4 + i];
            m[4 + i] = m[8 + i] * s + a * c;
            m[8 + i] = m[8 + i] * c - a * s;
        }
    }

    /// `0x00424990` / `0x00424a60`: row 3 += `x r0 + y r1 + z r2`.
    pub fn translate(&mut self, v: [f32; 3]) {
        let m = &mut self.0;
        m[12] = m[4] * v[1] + m[0] * v[0] + m[8] * v[2] + m[12];
        m[13] = m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13];
        m[14] = m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14];
        m[15] = m[3] * v[0] + m[7] * v[1] + m[11] * v[2] + m[15];
    }

    /// `0x00434b80`: row 3 += `x r0 + y r1`.
    pub fn translate2(&mut self, x: f32, y: f32) {
        let m = &mut self.0;
        m[12] = m[4] * y + m[0] * x + m[12];
        m[13] = m[1] * x + m[5] * y + m[13];
        m[14] = m[2] * x + m[6] * y + m[14];
        m[15] = m[3] * x + m[7] * y + m[15];
    }

    /// `0x00424730`: each row scaled unless its factor is exactly 1.
    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        let m = &mut self.0;
        if x != 1.0 {
            m[0] = m[0] * x;
            m[1] = x * m[1];
            m[2] = x * m[2];
            m[3] = x * m[3];
        }
        if y != 1.0 {
            for i in 4..8 {
                m[i] = m[i] * y;
            }
        }
        if z != 1.0 {
            for i in 8..12 {
                m[i] = m[i] * z;
            }
        }
    }

    /// `mat4MulVec3` 0x00488e50: the direction `v` through the rotation rows.
    pub fn mul_dir(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [m[4] * v[1] + v[0] * m[0] + m[8] * v[2], m[1] * v[0] + m[5] * v[1] + m[9] * v[2], m[2] * v[0] + m[6] * v[1] + m[10] * v[2]]
    }

    /// `0x004248a0` / `0x00488d60`: the point through the matrix, divided by its `w`.
    pub fn transform_point(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        let (x, y, z) = (v[0], v[1], v[2]);
        let w = 1.0f32 / (m[3] * x + m[7] * y + m[11] * z + m[15]);
        [w * (m[4] * y + x * m[0] + m[8] * z + m[12]), w * (m[1] * x + m[5] * y + m[9] * z + m[13]), w * (m[2] * x + m[6] * y + m[10] * z + m[14])]
    }
}

// ---------------------------------------------------------------------------------------------
// Events: what the gameplay half hands to the UI, audio and net ports.

/// Side effects of the gameplay code that belong to other modules.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `GameController::playSound` 0x00484350 (id, position, volume, pitch).
    Sound { id: u32, pos: [i64; 3], volume: f32, pitch: f32 },
    /// `applyMovementInput` 0x004a8e2e..0x004a8ed9: moving closes the widgets at `+0x8008bc`,
    /// `+0x8008c0`, `+0x8008c4`, `+0x8008dc`, `+0x8008f8`, `+0x800900`, `+0x800910` and clears
    /// `+0x8008f1`.
    CloseMovementPanels,
    /// 0x00493d72..0x00493ea6: the level went up ("LEVEL UP" text at 0x00493e35).
    LevelUp { level: i32 },
    /// 0x00493ed5..0x00494121: XP gained since the last frame ("+N XP" floating text).
    XpGained { amount: i32 },
    /// 0x00493b0f..0x00493d6d: the hit counter changed (the combo text).
    Combo { hits: i32 },
    /// 0x0049765f: "inventory full" (message 0x7010b0).
    InventoryFull,
    /// 0x00497424: R on a bed (static kind 0x2d): the map opens in bed mode
    /// (`+0x8006e4 = +0x800de4 = 1`).
    OpenMapFromBed,
    /// 0x0049743c: R on a static of kind 0x4d (the widget at `+0x8008f4`/`+0x8008dc`).
    OpenStaticPanel { kind: u32 },
    /// 0x0049756a: R on a crafting static (kinds 0x41, 0x47..0x4c; the widget at
    /// `+0x8008c0`, with `0x0046eb80(3 or 5)` for 0x41 / 0x47, 0x49, 0x4b).
    OpenCraftingPanel { kind: u32 },
    /// `0x004889e0`: R on a creature (NPC shop, class 0x80..0x83, 0x89) handled by the UI.
    CreatureMenu { id: i64 },
    /// 0x004969bf..0x00496a4f: R on a villager: its speech bubble (`0x004882e0(creature, 0)`,
    /// `ui::bubbles::creature_speech`; the branch of `+0x800940` is dead, see `ui::bubbles`).
    Talk { id: i64 },
    /// 0x0049755e..0x00497563: R on a static used through Interact 3: `0x00488030(static, 0)`,
    /// "There is nothing special." over the player for the kinds without their own use
    /// (`ui::bubbles::examine_static`).
    Examine { kind: u32 },
}

/// The UI state the gameplay half reads (the widgets live in `cw-ui`; the controller fills
/// this each frame).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiState {
    /// `plasma::Engine::hasTextFocus` 0x006531e0.
    pub text_focus: bool,
    /// `GameController::isCursorFree` 0x0047f1d0 (a map, menu or panel is open).
    pub cursor_free: bool,
    /// One of the six widgets at `+0x800874`, `+0x800880`, `+0x800890`, `+0x800888`,
    /// `+0x800894`, `+0x80088c` visible: no movement input (0x00498259).
    pub movement_panel_open: bool,
    /// One of `+0x800874`, `+0x800888`, `+0x800894`, `+0x80088c` visible: the camera does not
    /// follow the player (0x0049c3c4).
    pub camera_panel_open: bool,
    /// `+0x800880` or `+0x800890` visible: the camera aims at the feet (0x0049c4ff).
    pub camera_low_panel_open: bool,
    /// The chat line `+0x800a14` active (`+0x180`): no movement (0x00498049).
    pub chat_active: bool,
    /// `plasma::Engine` 0x00650ae0: a widget is under the cursor.
    pub widget_hovered: bool,
    /// The inventory widget `+0x8008bc` visible.
    pub inventory_open: bool,
    /// The map `+0x8006e4` open (the wheel zooms the map).
    pub map_open: bool,
    /// Click-to-walk active (`0x0076b040`, set by the map; not ported, see the report).
    pub click_to_walk: bool,
}

/// The options the camera reads (`GameController+0x170..+0x19c`, `Options::load` 0x004ce6e0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraOptions {
    /// `+0x18c`
    pub camera_speed: i32,
    /// `+0x190`
    pub camera_smoothness: i32,
    /// `+0x194`
    pub invert_y: bool,
}

// ---------------------------------------------------------------------------------------------
// The camera.

/// The camera fields of `cube::GameController`.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    /// `+0x140`: the eye, also the sound listener.
    pub eye: [i64; 3],
    /// `+0x158`: the point the camera orbits (near the player's head).
    pub target: [i64; 3],
    /// `+0x1a4..+0x1ac`: pitch (rotation about x, 0..180), roll (about y), yaw (about z),
    /// eased towards `rot_target`.
    pub rot: [f32; 3],
    /// `+0x1b0..+0x1b8`: what the mouse sets.
    pub rot_target: [f32; 3],
    /// `+0x1bc`: the distance, eased towards `dist_target` and cut by collisions.
    pub dist: f32,
    /// `+0x1c0`: the wheel's distance, 0..14.
    pub dist_target: f32,
    /// `+0x1c4`, `+0x1c8`: the map zoom and its target (0.01..10).
    pub map_zoom: f32,
    pub map_zoom_target: f32,
    /// `+0x1cc`: the distance after the collision pass.
    pub final_dist: f32,
    /// `+0x1d8`, `+0x1e0`: the render origin, `-eye.x` and `-eye.y`.
    pub origin: [i64; 2],
    /// `+0x1e8`: the screen shake (1.0 on sounds 0x51/0x52, decaying).
    pub shake: f32,
    /// `+0x1ec`: the camera's rotation and position (model matrix).
    pub model: Mat4,
    /// `+0x22c`: the view matrix.
    pub view: Mat4,
    /// `+0x26c`: the view relative to the render origin, with the shake.
    pub view_shake: Mat4,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            eye: [0; 3],
            target: [0; 3],
            rot: [0.0; 3],
            rot_target: [0.0; 3],
            dist: 0.0,
            // 0x00489d1a: the value the character screens reset it to (the constructor's
            // value was not read).
            dist_target: 10.0,
            map_zoom: 1.0,
            map_zoom_target: 1.0,
            final_dist: 0.0,
            origin: [0; 2],
            shake: 0.0,
            model: Mat4::IDENTITY,
            view: Mat4::IDENTITY,
            view_shake: Mat4::IDENTITY,
        }
    }
}

impl Camera {
    /// `onMouseMove` 0x0047ea00 with the cursor captured (0x0047ec94): pitch and yaw from the
    /// raw motion, pitch clamped to 0..180.
    pub fn on_mouse_move(&mut self, opts: &CameraOptions, dx: f32, dy: f32) {
        let k = opts.camera_speed as f32 * 0.005f32;
        let p = if !opts.invert_y { k * dy + self.rot_target[0] } else { self.rot_target[0] - k * dy };
        self.rot_target[0] = p;
        self.clamp_pitch();
        self.rot_target[2] = self.rot_target[2] - k * dx;
    }

    /// `onMouseMove` 0x0047ebd4 with the cursor free: dragging with the left button over no
    /// widget turns the camera by the cursor's motion (`engine+0xd4/+0xd8` minus
    /// `+0xdc/+0xe0`).
    pub fn on_mouse_drag(&mut self, opts: &CameraOptions, cursor_dx: f32, cursor_dy: f32) {
        let s = opts.camera_speed as f32;
        let d = cursor_dy * s * 0.005f32;
        if !opts.invert_y {
            self.rot_target[0] = d + self.rot_target[0];
        } else {
            self.rot_target[0] = self.rot_target[0] - d;
        }
        self.clamp_pitch();
        self.rot_target[2] = self.rot_target[2] - cursor_dx * s * 0.005f32;
    }

    fn clamp_pitch(&mut self) {
        if !(self.rot_target[0] <= 180.0) {
            self.rot_target[0] = 180.0;
        }
        if !(0.0 <= self.rot_target[0]) {
            self.rot_target[0] = 0.0;
        }
    }

    /// `onMouseWheel` 0x0047ef40: the distance by twice the notches (0..14), or the map zoom.
    pub fn on_mouse_wheel(&mut self, map_open: bool, delta: i32) {
        if !map_open {
            let d = self.dist_target - (delta.wrapping_mul(2)) as f32;
            self.dist_target = d;
            if !(0.0 <= d) {
                self.dist_target = 0.0;
            }
            if !(self.dist_target <= 14.0) {
                self.dist_target = 14.0;
            }
        } else {
            let z = self.map_zoom_target;
            let z = if delta >= 0 { z / 0.9f32 } else { z * 0.9f32 };
            self.map_zoom_target = z;
            if !(z <= 10.0) {
                self.map_zoom_target = 10.0;
            }
            if !(0.01f32 <= self.map_zoom_target) {
                self.map_zoom_target = 0.01;
            }
        }
    }

    /// 0x004965a2..0x0049662c: the angles ease with `pow(0.005, smoothness * 0.01)`, the
    /// distance and the map zoom with 0.004, all over `dt` milliseconds.
    pub fn smooth(&mut self, opts: &CameraOptions, dt: i32) {
        let t = cw_math::pow(0.005f32 as f64, f64::from(opts.camera_smoothness as f32 * 0.01f32)) as f32;
        let target = self.rot_target;
        lerp_vec3(&mut self.rot, target, dt, t);
        let d = self.dist_target;
        lerp_scalar(&mut self.dist, d, dt, 0.004);
        let z = self.map_zoom_target;
        lerp_scalar(&mut self.map_zoom, z, dt, 0.004);
    }

    /// The camera's rotation without translation: `I * rotZ(yaw) * rotY(roll) * rotX(pitch)`
    /// (0x0049c2e6..0x0049c362).
    pub fn rotation(&self) -> Mat4 {
        let mut r = Mat4::IDENTITY;
        r.rotate_z(self.rot[2]);
        r.rotate_y(self.rot[1]);
        r.rotate_x(self.rot[0]);
        r
    }

    /// 0x0049c2e6..0x0049ced6: follow the player, pull in against blocks, and rebuild the three
    /// matrices. `world_ms` is `world+0x8000bc` (the shake's phase), `mode` the player's mode.
    pub fn place(&mut self, world: &World, player: &EntityData, step_offset: f32, ui: &UiState, dt: i32, world_ms: i32) {
        let r = self.rotation();
        let ppos = pos_of(player);
        // 0x0049c367: a target more than 50 blocks away snaps to the player.
        if len_sq(blocks3(sub3(self.target, ppos))) > 2500.0 {
            self.target = ppos;
        }
        if !ui.camera_panel_open {
            // 0x0049c410: the target follows the player in x and y with lerp(dt, 0.1).
            let f = {
                let mut a = 0.0f32;
                lerp_scalar(&mut a, 1.0, dt, 0.1);
                a
            };
            for i in 0..2 {
                let d = ppos[i].wrapping_sub(self.target[i]);
                self.target[i] = self.target[i].wrapping_add(fixed_scale(d, f));
            }
            // 0x0049c4ff: and in z towards the head (lerp(dt, 0.005)).
            let scale = vec3_at(&player.0, 0x70);
            let base = ppos[2].wrapping_sub(fix(step_offset));
            let goal = if ui.camera_low_panel_open {
                base
            } else {
                match player.0[ent::MODE] {
                    0x54 | 0x53 => base.wrapping_add(fix(scale[0] * 0.5f32)),
                    0x6a | 0x6b => base.wrapping_add(fix(scale[2] * 0.5f32)).wrapping_add(fix(1.5)),
                    _ => base.wrapping_add(fix(scale[2] * 0.5f32)).wrapping_add(fix(0.5)),
                }
            };
            lerp_fixed(&mut self.target[2], goal, dt, 0.005);
        } else {
            // 0x0049c6c0: a menu screen is up (start, character select, server, world select).
            // The target is the world spawn (`0x00487fe0` = `world+0x8000f0`, floats through
            // `vec3i64::fromFloat` 0x0042c460; the port's spawn has no z, 0 as at 0x0046b5a0).
            self.target = fix3([world.spawn[0], world.spawn[1], 0.0]);
            // 0x0049c6d3..0x0049c740: `World::getColumn(x / 65536, y / 65536, null)` 0x004347a0
            // (`__alldiv`, truncating, 0x0042f570); when the zone exists, `target.z` =
            // `column+0x10` (base height) + `column+0x1c` (block count), as an int << 16
            // (0x00412080).
            let bx = (self.target[0] / 65536) as i32;
            let by = (self.target[1] / 65536) as i32;
            if let Some(col) = world.column(bx, by) {
                let top = col.height.wrapping_add(col.blocks.len() as i32);
                self.target[2] = i64::from(top) << 16;
            }
            // 0x0049c745..0x0049c76a: then 50 blocks up.
            self.target[2] = self.target[2].wrapping_add(50i64 << 16);
        }

        // 0x0049c76f: the eye backs off along the view direction.
        self.eye = self.target;
        let dir = r.mul_dir([0.0, 0.0, 1.0]);
        let d = self.dist;
        self.eye = sub3(self.eye, fix3([dir[0] * d, dir[1] * d, dir[2] * d]));
        // 0x0049c807: eight probes 0.25 block around the eye; one inside a solid block walks
        // through the solid along the view direction and the distance shrinks by that much.
        let mut min = self.dist;
        let quarter = 0x8000i64; // 0x00459c00(0.5) as the mulFixed factor
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    let off = [(((k as f64) - 0.5) * 65536.0) as i64, (((j as f64) - 0.5) * 65536.0) as i64, (((i as f64) - 0.5) * 65536.0) as i64];
                    let off = [off[0].wrapping_mul(quarter) / 65536, off[1].wrapping_mul(quarter) / 65536, off[2].wrapping_mul(quarter) / 65536];
                    let c = add3(self.eye, off);
                    if solid(block_fixed(world, c)) {
                        let t = cw_sim::path::sweep(world, c, dir, self.dist, true, true);
                        let v = self.dist - t;
                        if min > v {
                            min = v;
                        }
                    }
                }
            }
        }
        self.dist = min;
        self.eye = sub3(self.target, fix3([dir[0] * min, dir[1] * min, dir[2] * min]));
        self.final_dist = self.dist;
        // 0x0049ca80: an eye still inside a block goes back to the player along the ray from
        // the player (a walk through solid from an air block ends at once).
        if solid(block_fixed(world, self.eye)) {
            let mut v = blocks3(sub3(self.eye, ppos));
            if len_sq(v) > 0.0 {
                normalize(&mut v);
                let t = cw_sim::path::sweep(world, ppos, v, 100.0, true, true);
                if t >= 0.0 {
                    self.eye = add3(ppos, fix3([v[0] * t, v[1] * t, v[2] * t]));
                }
            }
        }
        // 0x0049cbe3: the camera matrix.
        let mut m = Mat4::IDENTITY;
        m.translate(blocks3(self.eye));
        m.rotate_z(self.rot[2]);
        m.rotate_y(self.rot[1]);
        m.rotate_x(self.rot[0]);
        self.model = m;
        // 0x0049cc59: the view matrix.
        let mut v = Mat4::IDENTITY;
        v.rotate_x(-self.rot[0]);
        v.rotate_y(-self.rot[1]);
        v.rotate_z(-self.rot[2]);
        v.translate(blocks3([self.eye[0].wrapping_neg(), self.eye[1].wrapping_neg(), self.eye[2].wrapping_neg()]));
        self.view = v;
        // 0x0049ccf7: the render origin.
        self.origin = [self.eye[0].wrapping_neg(), self.eye[1].wrapping_neg()];
        // 0x0049cd31: the origin-relative view with the shake.
        let mut s = Mat4::IDENTITY;
        let amp = self.shake * 0.5f32;
        let a = cw_math::cos(f64::from(world_ms as f32 * 0.04f32)) as f32 * amp;
        let b = cw_math::cos(f64::from(world_ms as f32 * 0.03f32)) as f32 * amp;
        s.translate2(b, a);
        s.rotate_x(-self.rot[0]);
        s.rotate_y(-self.rot[1]);
        s.rotate_z(-self.rot[2]);
        let neg = [self.eye[0].wrapping_neg(), self.eye[1].wrapping_neg(), self.eye[2].wrapping_neg()];
        let rel = sub3(neg, [self.origin[0], self.origin[1], 0]);
        s.translate(blocks3(rel));
        self.view_shake = s;
        // 0x0049ceb6: the shake decays.
        lerp_scalar(&mut self.shake, 0.0, dt, 0.005);
    }

    /// A world position relative to the render origin, as the aim code feeds `view_shake`:
    /// `pos + (origin.x, origin.y, 0)` in blocks.
    pub fn relative(&self, p: [i64; 3]) -> [f32; 3] {
        blocks3(add3(p, [self.origin[0], self.origin[1], 0]))
    }
}

/// `getBlockFixed` 0x0042f860 as the shared code does it (`World::block` of the block that holds
/// the fixed point, flooring).
pub(crate) fn block_fixed(world: &World, p: [i64; 3]) -> [u8; 4] {
    world.block((p[0] >> 16) as i32, (p[1] >> 16) as i32, (p[2] >> 16) as i32)
}

/// `blockIsSolid` 0x0043b480: any type but air (0) and water (2).
pub(crate) fn solid(b: [u8; 4]) -> bool {
    let t = b[3] & 0x1f;
    t != 0 && t != 2
}

// ---------------------------------------------------------------------------------------------
// Creature helpers the input code calls.

/// `cube::Creature::isGliding` 0x00444650 (named `isSwimmingDown` in the server map): flag 0x10,
/// falling, off the ground, not stunned, not rolling.
pub fn is_gliding(e: &EntityData) -> bool {
    u16_at(&e.0, ent::FLAGS) & 0x10 != 0 && !(0.0 <= f32_at(&e.0, ent::VEL + 8)) && u32_at(&e.0, ent::PHYS) & 1 == 0 && i32_at(&e.0, ent::STUN) <= 0 && i32_at(&e.0, ent::ROLL) <= 0
}

/// `EntityData::setFlag` 0x0042f160.
pub fn set_flag(e: &mut EntityData, mask: u16, on: bool) {
    let f = u16_at(&e.0, ent::FLAGS);
    wu16(&mut e.0, ent::FLAGS, if on { f | mask } else { f & !mask });
}

/// `0x0043e350`: whether the current mode lets another start: not mode 0x30, the animation
/// over or the mode one of the interruptible attacks, and no charged MP.
pub fn can_interrupt(e: &EntityData, guard: f32, haste: bool) -> bool {
    let mode = e.0[ent::MODE];
    if mode == 0x30 {
        return false;
    }
    let total = cw_sim::skills::skill_total_time(e, guard, haste);
    if i32_at(&e.0, ent::MODE_TIME) < total
        && !matches!(mode, 0 | 0x62 | 0x65 | 0x56 | 0xa | 2 | 1 | 9 | 4 | 3 | 0xd | 0xe | 0xf | 0x12 | 0x13 | 7 | 6 | 0x10 | 0xc | 0x39 | 0x3a | 0x43 | 0x41 | 0x42 | 0x16 | 0x1a | 0x25 | 0x2e | 0x2d | 0x1f | 0x21 | 0x22 | 0x1c | 0x1d)
    {
        return false;
    }
    f32_at(&e.0, ent::CHARGED_MP) <= 0.0
}

/// `skillCooldown` 0x0043e6a0 (`level < 0`: the creature's skill level).
pub fn skill_cooldown(e: &EntityData, mode: i32, level: i32) -> i32 {
    let f = || cw_sim::skills::skill_level_factor(e, mode, level);
    match mode {
        0x15 | 0x58 => (8000.0f32 - f() * 8000.0f32) as i32,
        0x30 => (20000.0f32 - f() * 12000.0f32) as i32,
        0x31 | 0x32 => (16000.0f32 - f() * 10000.0f32) as i32,
        0x36 | 0x60 => (20000.0f32 - f() * 14000.0f32) as i32,
        0x48 => 15000,
        0x56 => (60000.0f32 - f() * 40000.0f32) as i32,
        0x61 | 100 | 0x65 | 0x66 => (60000.0f32 - f() * 30000.0f32) as i32,
        99 => (12000.0f32 - f() * 10000.0f32) as i32,
        0x67 => (20000.0f32 - f() * 10000.0f32) as i32,
        _ => 0,
    }
}

/// `0x00595850(world, creature, mode, update)`: the self-buff skills 0x61, 0x64..0x67. Nothing
/// while the mode's cooldown runs; otherwise the buff (type, value, duration) is added to the
/// creature with a Passive record, and the cooldown starts.
pub fn self_buff(e: &mut EntityData, st: &mut CreatureState, id: i64, mode: i32, out: &mut ServerUpdate) {
    let key = mode as u8;
    if st.cooldowns.get(&key).copied().unwrap_or(0) > 0 {
        return;
    }
    let lf = |e: &EntityData| cw_sim::skills::skill_level_factor(e, mode, -1);
    let buff = match mode {
        0x61 => Some((3u8, 0.0f32, (lf(e) * 12000.0f32 + 8000.0f32) as i32)),
        100 => Some((0xc, lf(e) + 1.0f32, 10000)),
        0x65 => {
            // 0x00595906: the buff, then the stun ends; no Passive record.
            let b = make_buff(1, 0.75f32 - lf(e) * 0.3f32, 10000);
            cw_sim::combat::add_buff(st, &b);
            if i32_at(&e.0, ent::STUN) > 0 {
                wi32(&mut e.0, ent::STUN, 0);
            }
            st.cooldowns.insert(key, skill_cooldown(e, mode, -1));
            let _ = id;
            return;
        }
        0x66 => Some((2, 0.0, 10000)),
        0x67 => {
            let a = lf(e) + 1.0f32;
            Some((6, cw_sim::modes::magic_power(e) * 2.0f32 * a, 30000))
        }
        _ => None,
    };
    if let Some((ty, value, dur)) = buff {
        let b = make_buff(ty, value, dur);
        cw_sim::combat::add_buff(st, &b);
        let mut p = [0u8; 0x28];
        p[0..8].copy_from_slice(&id.to_le_bytes());
        p[8..16].copy_from_slice(&id.to_le_bytes());
        p[0x10..0x28].copy_from_slice(&b);
        out.passives.push(Passive(p));
    }
    st.cooldowns.insert(key, skill_cooldown(e, mode, -1));
}

fn make_buff(ty: u8, value: f32, duration: i32) -> Buff {
    let mut b = [0u8; 0x18];
    b[0] = ty;
    b[4..8].copy_from_slice(&value.to_le_bytes());
    b[8..0xc].copy_from_slice(&duration.to_le_bytes());
    b
}

/// The skill bar (`GameController+0x800814`) as 0x0048b8ab..0x0048bd8c rebuilds it every frame:
/// slot 0 (left mouse) the basic attack, slot 1 (right mouse) the weapon's second move, slots
/// 2..4 (keys 1..3) the class skills; a class skill with no points (`entity+0x1140 + 4k`, `k`
/// the slot less 2, slots up to 6) is emptied (0).
pub fn build_skill_bar(e: &EntityData, guard: f32, haste: bool) -> Vec<i32> {
    let mut bar: Vec<i32> = Vec::new();
    let w = e.0[ent::slot(7) + 1];
    let off = e.0[ent::slot(6) + 1];
    let spec1 = e.0[ent::SPEC] == 1;
    match w {
        0xa => {
            if spec1 {
                bar.extend([0x20, 0x21]);
            } else {
                bar.extend([0x1e, 0x1f]);
            }
        }
        0xc => {
            if spec1 {
                bar.extend([0x2a, 0x2b]);
            } else {
                bar.extend([0x28, 0x25]);
            }
        }
        0xb => {
            if spec1 {
                bar.extend([0x2c, 0x2d]);
            } else {
                bar.extend([0x26, 0x2e]);
            }
        }
        _ => {
            bar.push(i32::from(cw_sim::combat_ai::basic_mode(e, guard, haste)));
            let second = match w {
                5 => 5,
                3 => 0x11,
                4 => 0x14,
                _ if w == 6 || off == 6 => 0x19,
                _ if w == 7 || off == 7 => 0x18,
                8 => 0x1b,
                _ if cw_sim::combat_ai::heavy_weapon(e) => 0x3b,
                _ if off != 0xd => 0x3f,
                _ => 8,
            };
            bar.push(second);
        }
    }
    match e.0[ent::CLASS] {
        3 => bar.extend([if spec1 { 0x22 } else { 0x58 }, 0x67, 0x31]),
        4 => bar.extend([0x30, 0x4f, if e.0[ent::SPEC] == 0 { 0x61 } else { 0x60 }]),
        2 => bar.extend([0x15, 0x32, if spec1 { 0x64 } else { 99 }]),
        1 => bar.extend([0x36, 0x56, if e.0[ent::SPEC] == 0 { 0x66 } else { 0x65 }]),
        _ => {}
    }
    let mut i = 2usize;
    while i < bar.len() {
        if i + 4 <= 0xa && i32_at(&e.0, ent::SKILLS_BAR + (i - 2) * 4) <= 0 {
            bar[i] = 0;
        }
        i += 1;
    }
    bar
}

/// 0x0049c272..0x0049c2e6: a mode left running too long ends (outside the held and riding
/// modes, and while the right button is up): holding an item of type 0x14 (slot 12) goes to
/// mode 0x52 half a second after the animation; anything else to 0 after 10 s.
pub fn mode_timeout(e: &mut EntityData, right_mouse: bool, guard: f32, haste: bool) {
    let mode = e.0[ent::MODE];
    if matches!(mode, 0x53 | 0x6a | 0x6b | 0x52 | 0x54 | 8 | 0x23 | 0x4f | 0x1c | 0x1d | 0x24) || right_mouse {
        return;
    }
    if e.0[ent::slot(12)] == 0x14 && i32_at(&e.0, ent::MODE_TIME) > cw_sim::skills::skill_total_time(e, guard, haste) + 0x1f4 {
        e.0[ent::MODE] = 0x52;
        return;
    }
    if i32_at(&e.0, ent::MODE_TIME) > 0x2710 {
        e.0[ent::MODE] = 0;
    }
}

/// `onKeyDown` 0x0047e811 (F): toggle flag 0x200, the lamp.
pub fn toggle_lamp(e: &mut EntityData) {
    let on = (u16_at(&e.0, ent::FLAGS) >> 9) & 1 == 0;
    set_flag(e, 0x200, on);
}

/// `onKeyDown` 0x0047e83c (G): the special item in equipment slot 11. With none, gliding stops
/// and the boat (mode 0x6b) ends. Sub type 0 (the glider): glide unless already gliding or
/// more than two blocks below the ground last stood on (`creature+0x13bc`), and the mode ends.
/// Sub type 1 (the boat): mode 0x6b in water, or off again.
pub fn use_special_item(e: &mut EntityData, ground_z: f32) {
    let s = ent::slot(11);
    if e.0[s] != 0 {
        if e.0[s + 1] == 0 {
            let d = ((ground_z * 65536.0f32) as i64).wrapping_sub(pos_of(e)[2]);
            let near = d < (2i64 << 16);
            let on = near && u16_at(&e.0, ent::FLAGS) & 0x10 == 0;
            set_flag(e, 0x10, on);
            e.0[ent::MODE] = 0;
        }
        if e.0[s + 1] == 1 {
            if e.0[ent::MODE] == 0x6b {
                e.0[ent::MODE] = 0;
            } else if u32_at(&e.0, ent::PHYS) & 2 != 0 {
                e.0[ent::MODE] = 0x6b;
                wi32(&mut e.0, ent::MODE_TIME, 0);
            }
        }
    } else {
        let f = u16_at(&e.0, ent::FLAGS) & 0xffef;
        wu16(&mut e.0, ent::FLAGS, f);
        if e.0[ent::MODE] == 0x6b {
            e.0[ent::MODE] = 0;
        }
    }
}

/// `0x00447110` (`Creature::resetForRespawn`): the creature fields a respawn clears, then
/// `EntityData::respawn` 0x00447270 on the block (not read; see the report) and the path.
pub fn reset_for_respawn(st: &mut CreatureState) {
    st.ground_z = 0.0;
    st.modes.hit_count_copy = 0;
    st.prev_roll = 0;
    st.last_target = 0;
    st.ai.f13e0 = 0;
    st.block = 0.0;
    st.stamina = 1.0;
    st.threat.clear();
    st.hits_landed.clear();
    st.cooldowns.clear();
    st.riding.smoothing = 0;
    st.charge = 0.0;
    st.modes.free_cast = 0;
    st.clear_path();
}

/// `0x005a03d0(world, pos)`: the respawn point. The kind-0 statics of the 3x3 zones around the
/// position (zone rows `x - 1..x + 1`, each `y - 1..y + 1`, statics in zone order) are listed;
/// none: the position itself; one: it; more: the nearest in x/y is dropped and the nearest of
/// the rest is taken.
pub fn respawn_point(world: &World, pos: [i64; 3]) -> [i64; 3] {
    let zx = (pos[0] / 0x10000) >> 8;
    let zy = (pos[1] / 0x10000) >> 8;
    let (zx, zy) = (zx as i32, zy as i32);
    let mut found: Vec<[i64; 3]> = Vec::new();
    for x in zx - 1..=zx + 1 {
        for y in zy - 1..=zy + 1 {
            if x < 0 || y < 0 || x >= 0x10000 || y >= 0x10000 {
                continue;
            }
            if let Some(z) = world.zone(x, y) {
                for s in &z.statics {
                    if s.kind == 0 {
                        found.push([s.x, s.y, s.z]);
                    }
                }
            }
        }
    }
    match found.len() {
        0 => pos,
        1 => found[0],
        _ => {
            let d2 = |p: &[i64; 3]| {
                let x = p[0].wrapping_sub(pos[0]) as f32 * K;
                let y = p[1].wrapping_sub(pos[1]) as f32 * K;
                y * y + x * x
            };
            let nearest = |list: &[[i64; 3]]| {
                let mut best = -1.0f32;
                let mut at = None;
                for (i, p) in list.iter().enumerate() {
                    let d = d2(p);
                    if 0.0 > best || !(best <= d) {
                        best = d;
                        at = Some(i);
                    }
                }
                at
            };
            if let Some(i) = nearest(&found) {
                found.remove(i);
            }
            match nearest(&found) {
                Some(i) => found[i],
                None => pos,
            }
        }
    }
}

/// The display counters of a creature's recent damage (`creature+0x13c8` the sum, `+0x13cc`
/// the time since it began, `+0x13d0` the time since the last hit), which the HP bars read.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DamageDisplay {
    pub sum: f32,
    pub since_start: i32,
    pub since_hit: i32,
}

/// Ghidra line 4345 (per creature, every frame): after ten seconds without a hit the counters
/// reset.
pub fn damage_display_tick(d: &mut DamageDisplay, dt: i32) {
    d.since_hit += dt;
    if d.since_hit > 10000 {
        d.since_start = 0;
        d.sum = 0.0;
    }
    d.since_start += dt;
}

/// Ghidra line 4370: a positive damage record adds to the sum and restarts the hit timer.
pub fn damage_display_hit(d: &mut DamageDisplay, damage: f32) {
    if 0.0 < damage {
        d.sum = damage + d.sum;
        d.since_hit = 0;
    }
}

// ---------------------------------------------------------------------------------------------
// The local player.

/// The `GameController` fields and `update`'s static globals that belong to the local player.
#[derive(Debug, Clone)]
pub struct LocalPlayer {
    /// The creature id of `GameController+0x8006d0`.
    pub id: i64,
    pub camera: Camera,
    /// `+0x800a90`: the projection matrix (built by the render side, read by the aim code).
    pub projection: Mat4,
    /// `+0x8006d8`: the locked-on creature (0 none).
    pub lock_target: i64,
    /// `+0x8006d4`: cleared with the lock every frame at 0x004984fd (readers not identified).
    pub f8006d4: i64,
    /// `+0x800a70`: the creature under the crosshair (0 none).
    pub aimed_creature: i64,
    /// `+0x800a84`: the static under the crosshair (zone x, zone y, index; index -1 none).
    pub aimed_static: [i32; 3],
    /// `+0x800a78`: the ground item under the crosshair (zone x, zone y, index).
    pub aimed_item: [i32; 3],
    /// `+0x800d8` (`[0x200236]`): the creature whose menu R opened; cleared 25 blocks away.
    pub menu_creature: i64,
    /// `+0x800704`: build mode.
    pub build_mode: bool,
    /// `+0x800a40`: the quick-item menu (Tab) is open; `+0x800a44` its selection.
    pub quick_open: bool,
    pub quick_index: i32,
    /// `+0x800814`: the skill bar ([`build_skill_bar`]).
    pub skill_bar: Vec<i32>,
    pub latches: EdgeLatches,
    /// `0x0076b164`: how long Space has been held; `0x0076b168`: the jump boost window.
    pub jump_hold: i32,
    pub jump_boost: i32,
    /// `0x0076b140`: the quick menu's navigation latch; `0x0076b144`, `0x0076b148`: the
    /// auto-repeat of A and D in it.
    pub quick_nav_latch: bool,
    pub quick_repeat: [i32; 2],
    /// `0x0076b0c9`: the player is dead (set once HP reaches 0).
    pub dead_latch: bool,
    /// `creature+0x1d3c`, `+0x1d40`: the level and XP last shown; `0x0076b101`: the hit
    /// counter last shown (a byte).
    pub shown_level: i32,
    pub shown_xp: i32,
    pub shown_combo: u8,
    /// `creature+0x130c`: the interactions queued for the tick and the send loop.
    pub interact_queue: Vec<Interact>,
    /// `creature+0x11e8`, `+0x11ec`: the stack held by the cursor (count, item).
    pub held_count: i32,
    pub held_item: [u8; Item::SIZE],
    /// `+0x1000e68`: items picked up (the server's Pickup records for this player, filled by the
    /// net port), moved into the inventory one per 100 ms by [`LocalPlayer::pickup_step`];
    /// `+0x1000e70` its timer.
    pub pending_pickups: std::collections::VecDeque<[u8; Item::SIZE]>,
    pub pickup_timer: i32,
    /// `creature+0x11d8`: the flight time of the leaps 0x30 and 0x36 (`|v| / 50 * 1000` ms;
    /// its reader is not identified; `0x00447110` clears it).
    pub leap_time: i32,
    pub events: Vec<Event>,
}

impl LocalPlayer {
    pub fn new(id: i64) -> Self {
        LocalPlayer {
            id,
            camera: Camera::default(),
            projection: Mat4::IDENTITY,
            lock_target: 0,
            f8006d4: 0,
            aimed_creature: 0,
            aimed_static: [-1, -1, 0],
            aimed_item: [-1, -1, 0],
            menu_creature: 0,
            build_mode: false,
            quick_open: false,
            quick_index: 0,
            skill_bar: Vec::new(),
            latches: EdgeLatches::default(),
            jump_hold: 0,
            jump_boost: 0,
            quick_nav_latch: false,
            quick_repeat: [0; 2],
            dead_latch: false,
            shown_level: 1,
            shown_xp: 0,
            shown_combo: 0,
            interact_queue: Vec::new(),
            held_count: 0,
            held_item: [0; Item::SIZE],
            leap_time: 0,
            pending_pickups: std::collections::VecDeque::new(),
            pickup_timer: 0,
            events: Vec::new(),
        }
    }

    fn sound(&mut self, id: u32, pos: [i64; 3], volume: f32, pitch: f32) {
        self.events.push(Event::Sound { id, pos, volume, pitch });
    }

    /// Ghidra line 1528 (before the tick): the dead latch.
    pub fn death_latch(&mut self, e: &EntityData) {
        if 0.0 < f32_at(&e.0, ent::HP) {
            self.dead_latch = false;
        } else if !self.dead_latch {
            self.dead_latch = true;
        }
    }

    /// 0x0048cda1..0x0048ce2e (before the tick): the pickup queue. The timer runs every frame;
    /// with an item waiting and more than 100 ms gone, the first item goes into the inventory
    /// (`Inventory::addItem(item, -1)`) with sound 0x2d and leaves the queue.
    pub fn pickup_step(&mut self, e: &EntityData, st: &mut CreatureState, dt: i32) {
        self.pickup_timer += dt;
        if !self.pending_pickups.is_empty() && self.pickup_timer > 100 {
            self.pickup_timer = 0;
            if let Some(item) = self.pending_pickups.pop_front() {
                st.inventory.add_item(Item::from_bytes(&item), -1);
            }
            self.sound(0x2d, pos_of(e), 1.0, 1.0);
        }
    }

    /// 0x00493b0f..0x0049413a: the combo counter, the level-up (sound 0x1d) and the XP gained,
    /// against what was shown last frame.
    pub fn xp_display(&mut self, e: &EntityData) {
        let combo = i32_at(&e.0, ent::HIT_COUNTER);
        if combo != 0 && combo as u32 != u32::from(self.shown_combo) {
            self.events.push(Event::Combo { hits: combo });
        }
        self.shown_combo = e.0[ent::HIT_COUNTER];
        let level = i32_at(&e.0, ent::LEVEL);
        let xp = i32_at(&e.0, ent::XP);
        if level == self.shown_level {
            if self.shown_xp < xp {
                self.events.push(Event::XpGained { amount: xp - self.shown_xp });
                self.shown_xp = xp;
            }
        } else {
            self.events.push(Event::LevelUp { level });
            self.sound(0x1d, pos_of(e), 1.0, 1.0);
            self.shown_level = level;
            self.shown_xp = 0;
        }
    }

    /// 0x00497383..0x004973df: R while dead ([`crate::interact::r_key`] tests the key): the
    /// respawn. The creature fields reset, the position goes to [`respawn_point`] and HP to the
    /// maximum.
    pub fn respawn(&mut self, world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>) -> bool {
        let Some(e) = entities.get_mut(&self.id) else { return false };
        reset_for_respawn(states.entry(self.id).or_default());
        self.leap_time = 0;
        let p = respawn_point(world, pos_of(e));
        set_pos(e, p);
        let max = cw_sim::stats::max_hp(e);
        wf32(&mut e.0, ent::HP, max);
        true
    }

    /// 0x00497723..0x00497892, before the movement input: the acceleration starts from zero
    /// each frame, except while gliding, where it eases towards ten times the horizontal
    /// heading (lerp(dt, 0.001)) and is pushed sideways by the body tilt.
    pub fn glide_steering(&mut self, e: &mut EntityData, dt: i32) {
        if is_gliding(e) {
            let mut v = vec3_at(&e.0, ent::VEL);
            v[2] = 0.0;
            if len_sq(v) > 0.0 {
                normalize(&mut v);
                // 0x004977c7: cross((0, 0, 1), v).
                let u = [0.0f32, 0.0, 1.0];
                let side = [u[1] * v[2] - u[2] * v[1], v[0] * u[2] - u[0] * v[2], u[0] * v[1] - v[0] * u[1]];
                let mut a = vec3_at(&e.0, ent::ACCEL);
                lerp_vec3(&mut a, [v[0] * 10.0f32, v[1] * 10.0f32, v[2] * 10.0f32], dt, 0.001);
                let tilt = f32_at(&e.0, 0x1c);
                let s = [side[0] * tilt, side[1] * tilt, side[2] * tilt];
                let s = [s[0] * 0.05f32, s[1] * 0.05f32, s[2] * 0.05f32];
                a = [a[0] - s[0], a[1] - s[1], a[2] - s[2]];
                set_vec3(&mut e.0, ent::ACCEL, a);
            }
        } else {
            set_vec3(&mut e.0, ent::ACCEL, [0.0; 3]);
        }
    }

    /// 0x00497892..0x00497950: a right stick turns the camera through `onMouseMove` with
    /// `pow(axis / 32768, 3) * dt` (y negated).
    pub fn right_stick_camera(&mut self, input: &ControllerBytes, opts: &CameraOptions, dt: i32) {
        let rs = input.right_stick;
        if rs[0].wrapping_mul(rs[0]).wrapping_add(rs[1].wrapping_mul(rs[1])) == 0 {
            return;
        }
        let k = 3.051_757_8e-5f32;
        let y = cw_math::pow(f64::from(rs[1] as f32 * k), 3.0) as f32;
        let dy = -(y * dt as f32);
        let x = cw_math::pow(f64::from(rs[0] as f32 * k), 3.0) as f32;
        let dx = x * dt as f32;
        self.camera.on_mouse_move(opts, dx, dy);
    }

    /// 0x00498049..0x00498409 without the quick-item and build branches (in
    /// [`crate::interact`]): with the chat inactive and the player not stunned, the movement
    /// input (unless a panel is open) and the flags. Click-to-walk (`0x0076b040`, 0x00498085) is
    /// not ported.
    pub fn movement(&mut self, world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, input: &ControllerBytes, ui: &UiState, dt: i32, guard_haste: (f32, bool)) {
        if ui.chat_active {
            return;
        }
        let Some(e) = entities.get(&self.id) else { return };
        if i32_at(&e.0, ent::STUN) > 0 {
            return;
        }
        if ui.click_to_walk {
            return;
        }
        if !ui.movement_panel_open {
            self.apply_movement_input(world, entities, states, input, ui, dt);
        }
        if let Some(e) = entities.get_mut(&self.id) {
            let stamina = states.get(&self.id).map_or(1.0, |s| s.stamina);
            Self::movement_flags(e, input, stamina, guard_haste);
        }
    }

    /// 0x004982be..0x00498404: flag 4 off; flag 0x40 (run) is Space while gliding with stamina
    /// (off without), else not-Shift; without Shift, an attack still running (or a held
    /// attack) outside the travel modes turns the player to its aim (flag 4) and walks (0x40
    /// off); flag 1 (climb) is Ctrl.
    pub fn movement_flags(e: &mut EntityData, input: &ControllerBytes, stamina: f32, (guard, haste): (f32, bool)) {
        set_flag(e, 4, false);
        let flags = u16_at(&e.0, ent::FLAGS);
        if flags & 0x10 != 0 && u32_at(&e.0, ent::PHYS) & 1 == 0 {
            let run = if stamina > 0.0 { input.space() } else { false };
            set_flag(e, 0x40, run);
        } else {
            set_flag(e, 0x40, !input.shift());
        }
        if !input.shift() {
            let mode = e.0[ent::MODE];
            if !matches!(mode, 0x6d | 0x53 | 0x6a | 0x6b | 0x69 | 0x52 | 0x22 | 0x54 | 0x50 | 0x51 | 0x4f | 0xb) {
                let total = cw_sim::skills::skill_total_time(e, guard, haste);
                let mode = e.0[ent::MODE];
                if i32_at(&e.0, ent::MODE_TIME) < total + 0xc8 || matches!(mode, 0x18 | 0x19 | 0x1b | 8 | 0x24 | 0x40 | 0x3b | 0x3f | 0x1c | 0x23) {
                    if !matches!(e.0[ent::MODE], 0x58 | 0x57 | 0x59 | 0x5c) {
                        set_flag(e, 4, true);
                    }
                    set_flag(e, 0x40, false);
                }
            }
        }
        set_flag(e, 1, input.ctrl());
    }

    /// `GameController::applyMovementInput` 0x004a6b50: the direction keys (and the left
    /// stick) to the acceleration of the player, or of its mount; the dodge roll; the lock-on
    /// (the `+0x16` byte, never set by the original's input); the jump.
    pub fn apply_movement_input(&mut self, world: &World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, input: &ControllerBytes, ui: &UiState, dt: i32) {
        // 0x004a6b73: nothing while a text field has focus.
        if ui.text_focus {
            return;
        }
        let Some(player) = entities.get(&self.id).cloned() else { return };
        let cursor_free = ui.cursor_free;
        // 0x004a6b8c: 80 blocks/s² on foot, 8 while gliding.
        let speed = if is_gliding(&player) { 8.0f32 } else { 80.0f32 };
        // 0x004a6ba6: the heading matrix, identity turned by the eased yaw.
        let mut hm = Mat4::IDENTITY;
        hm.rotate_z(self.camera.rot[2]);
        let m = hm.0;
        let c = self.camera.model.0;
        let pst = states.get(&self.id).cloned().unwrap_or_default();
        // 0x004a6c9d: the dodge roll is armed by the middle button with the cursor captured
        // and a quarter of the stamina.
        let roll = !cursor_free && input.middle_mouse() && pst.stamina >= 0.25f32;
        // 0x004a6cf3: the lock-on drops a dead target or one farther than 100 blocks.
        if self.lock_target != 0 {
            let keep = match entities.get(&self.lock_target) {
                Some(t) if 0.0 < f32_at(&t.0, ent::HP) => {
                    let d = blocks3(sub3(pos_of(t), pos_of(&player)));
                    d[1] * d[1] + d[0] * d[0] + d[2] * d[2] <= 10000.0
                }
                _ => false,
            };
            if !keep {
                self.lock_target = 0;
            }
        }
        // 0x004a6e36: flag 4 off on the player; the mover is the mount when mounted.
        if let Some(p) = entities.get_mut(&self.id) {
            let f = u16_at(&p.0, ent::FLAGS) & 0xfffb;
            wu16(&mut p.0, ent::FLAGS, f);
        }
        let mount = pst.modes.mount;
        let mover_id = if mount != 0 { mount } else { self.id };
        // A missing mount is a null dereference in the original.
        let Some(mut mv) = entities.get(&mover_id).cloned() else { return };
        let mut mst = states.get(&mover_id).cloned().unwrap_or_default();

        let fwd = [m[0] * 0.0f32 - m[4] * 1.0f32 + m[8] * 0.0f32, m[1] * 0.0f32 - m[5] * 1.0f32 + m[9] * 0.0f32, m[2] * 0.0f32 - m[6] * 1.0f32 + m[10] * 0.0f32];
        let side = [m[4] * 0.0f32 + m[0] + m[8] * 0.0f32, m[5] * 0.0f32 + m[1] + m[9] * 0.0f32, m[6] * 0.0f32 + m[2] + m[10] * 0.0f32];
        let swim_w = [c[4] * 0.0f32 + c[0] * 0.0f32 + c[8], c[5] * 0.0f32 + c[1] * 0.0f32 + c[9], c[6] * 0.0f32 + c[2] * 0.0f32 + c[10]];
        let swim_s = [c[4] * 0.0f32 + c[0] * 0.0f32 - c[8] * 1.0f32, c[5] * 0.0f32 + c[1] * 0.0f32 - c[9] * 1.0f32, c[6] * 0.0f32 + c[2] * 0.0f32 - c[10] * 1.0f32];
        let swim_d = [c[4] * 0.0f32 - c[0] * 1.0f32 + c[8] * 0.0f32, c[5] * 0.0f32 - c[1] * 1.0f32 + c[9] * 0.0f32, c[6] * 0.0f32 - c[2] * 1.0f32 + c[10] * 0.0f32];
        let swim_a = [c[4] * 0.0f32 + c[0] + c[8] * 0.0f32, c[5] * 0.0f32 + c[1] + c[9] * 0.0f32, c[6] * 0.0f32 + c[2] + c[10] * 0.0f32];

        let flags = |e: &EntityData| u16_at(&e.0, ent::FLAGS);
        let phys = |e: &EntityData| u32_at(&e.0, ent::PHYS);
        let climbing = |e: &EntityData| flags(e) & 1 != 0 && phys(e) & 4 != 0;
        let swimming = |e: &EntityData| phys(e) & 2 != 0 && e.0[ent::MODE] != 0x6b;
        let may_walk = |e: &EntityData, stamina: f32| flags(e) & 0x10 == 0 || phys(e) & 1 != 0 || stamina > 0.0;
        let may_roll = |e: &EntityData| (i32_at(&e.0, ent::ROLL) <= 0 || e.0[ent::MODE] != 0) && phys(e) & 3 != 0;
        let add_accel = |e: &mut EntityData, a: [f32; 3]| {
            let x = vec3_at(&e.0, ent::ACCEL);
            set_vec3(&mut e.0, ent::ACCEL, [x[0] + a[0], x[1] + a[1], x[2] + a[2]]);
        };
        let sub_accel = |e: &mut EntityData, a: [f32; 3]| {
            let x = vec3_at(&e.0, ent::ACCEL);
            set_vec3(&mut e.0, ent::ACCEL, [x[0] - a[0], x[1] - a[1], x[2] - a[2]]);
        };
        let scaled = |v: [f32; 3], s: f32| [v[0] * s, v[1] * s, v[2] * s];
        let do_roll = |e: &mut EntityData, st: &mut CreatureState, v: [f32; 3]| {
            set_vec3(&mut e.0, ent::VEL, [v[0] + 0.0f32, v[1] + 0.0f32, v[2] + 5.0f32]);
            wi32(&mut e.0, ent::ROLL, 600);
            st.stamina = st.stamina - 0.25f32;
            e.0[ent::MODE] = 0;
        };
        let wall = mst.wall_normal;
        let climb_side = [wall[1] - wall[2] * 0.0f32, wall[2] * 0.0f32 - wall[0], wall[0] * 0.0f32 - wall[1] * 0.0f32];

        // 0x004a6ec0: W.
        if input.forward() {
            if climbing(&mv) {
                let z = speed * 0.2f32 + f32_at(&mv.0, ent::ACCEL + 8);
                wf32(&mut mv.0, ent::ACCEL + 8, z);
            } else if !roll {
                if swimming(&mv) {
                    add_accel(&mut mv, scaled(swim_w, speed));
                } else if may_walk(&mv, mst.stamina) {
                    add_accel(&mut mv, scaled(fwd, speed));
                }
            } else if may_roll(&mv) {
                do_roll(&mut mv, &mut mst, scaled(fwd, 40.0));
            }
        }
        // 0x004a7240: S.
        if input.back() {
            if climbing(&mv) {
                let z = f32_at(&mv.0, ent::ACCEL + 8) - speed * 0.2f32;
                wf32(&mut mv.0, ent::ACCEL + 8, z);
            } else if !roll {
                if swimming(&mv) {
                    add_accel(&mut mv, scaled(swim_s, speed));
                } else if may_walk(&mv, mst.stamina) {
                    sub_accel(&mut mv, scaled(fwd, speed));
                }
            } else if may_roll(&mv) {
                do_roll(&mut mv, &mut mst, scaled(fwd, -40.0));
            }
        }
        // 0x004a75d0: D.
        if input.right() {
            if climbing(&mv) {
                sub_accel(&mut mv, scaled(climb_side, speed * 0.2f32));
            } else if !roll {
                if swimming(&mv) {
                    add_accel(&mut mv, scaled(swim_d, speed));
                } else if may_walk(&mv, mst.stamina) {
                    sub_accel(&mut mv, scaled(side, speed));
                }
            } else if may_roll(&mv) {
                do_roll(&mut mv, &mut mst, scaled(side, -40.0));
            }
        }
        // 0x004a79a0: A.
        if input.left() {
            if climbing(&mv) {
                add_accel(&mut mv, scaled(climb_side, speed * 0.2f32));
            } else if !roll {
                if swimming(&mv) {
                    add_accel(&mut mv, scaled(swim_a, speed));
                } else if may_walk(&mv, mst.stamina) {
                    add_accel(&mut mv, scaled(side, speed));
                }
            } else if may_roll(&mv) {
                do_roll(&mut mv, &mut mst, scaled(side, 40.0));
            }
        }
        // 0x004a7d76: the left stick (`+0x124`, `+0x128`).
        let (ax, ay) = (input.left_stick[0], input.left_stick[1]);
        let k = 3.051_757_8e-5f32;
        if ax.wrapping_mul(ax).wrapping_add(ay.wrapping_mul(ay)) != 0 {
            if climbing(&mv) {
                let a1 = ax.wrapping_neg() as f32 * k;
                let x = wall[1] * a1 - wall[2] * 0.0f32;
                let y = wall[2] * 0.0f32 - wall[0] * a1;
                let z = wall[0] * 0.0f32 - wall[1] * 0.0f32;
                let f = speed * 0.2f32;
                let acc = vec3_at(&mv.0, ent::ACCEL);
                let nz = z * f + acc[2];
                set_vec3(&mut mv.0, ent::ACCEL, [x * f + acc[0], y * f + acc[1], nz]);
                wf32(&mut mv.0, ent::ACCEL + 8, ay as f32 * f * k + nz);
            } else if !roll {
                if phys(&mv) & 2 != 0 {
                    let b8 = ax.wrapping_neg() as f32 * k;
                    let b0 = ay as f32 * k;
                    let v = [c[0] * b8 + c[4] * 0.0f32 + c[8] * b0, c[1] * b8 + c[5] * 0.0f32 + c[9] * b0, c[2] * b8 + c[6] * 0.0f32 + c[10] * b0];
                    add_accel(&mut mv, scaled(v, speed));
                } else if may_walk(&mv, mst.stamina) {
                    let c4 = ax.wrapping_neg() as f32 * k;
                    let c0 = ay.wrapping_neg() as f32 * k;
                    let v = [m[4] * c0 + m[0] * c4 + m[8] * 0.0f32, c0 * m[5] + m[1] * c4 + m[9] * 0.0f32, c0 * m[6] + c4 * m[2] + m[10] * 0.0f32];
                    add_accel(&mut mv, scaled(v, speed));
                }
            } else if may_roll(&mv) {
                let d0 = ax.wrapping_neg() as f32 * k;
                let cc = ay.wrapping_neg() as f32 * k;
                let v = [m[4] * cc + m[0] * d0 + m[8] * 0.0f32, cc * m[5] + m[1] * d0 + m[9] * 0.0f32, cc * m[6] + d0 * m[2] + m[10] * 0.0f32];
                do_roll(&mut mv, &mut mst, scaled(v, 40.0));
            }
        }
        // Write the mover back before the lock-on reads the creature map.
        entities.insert(mover_id, mv.clone());
        states.insert(mover_id, mst.clone());
        // 0x004a8263: the lock-on.
        if input.lock_on() && !self.latches.lock_on {
            self.lock_on(world, entities);
        }
        self.latches.lock_on = input.lock_on();
        let Some(mut mv) = entities.get(&mover_id).cloned() else { return };
        // 0x004a8cb4: mode 0x34 ends.
        if mv.0[ent::MODE] == 0x34 {
            mv.0[ent::MODE] = 0;
        }
        // 0x004a8cc1: the jump.
        self.jump(&mut mv, input.space(), dt);
        self.latches.space = input.space();
        entities.insert(mover_id, mv);
        // 0x004a8dea: any acceleration on the player closes the panels.
        if let Some(p) = entities.get(&self.id) {
            let a = vec3_at(&p.0, ent::ACCEL);
            if !(a[0] * a[0] + a[1] * a[1] + a[2] * a[2] <= 0.0) {
                self.events.push(Event::CloseMovementPanels);
            }
        }
    }

    /// `applyMovementInput` 0x004a8cc1..0x004a8dea: Space. Held less than 200 ms on the
    /// ground (or rolling) while not rising: the jump, `min(|v_xy| + 2, 10)` up and a 100 ms
    /// boost window; in water below 10, a slow rise; while the window is open (or in water),
    /// `0.08 * dt` more up to 10.
    fn jump(&mut self, mv: &mut EntityData, space: bool, dt: i32) {
        let vz_o = ent::VEL + 8;
        let mut boost;
        let mut take_boost = false;
        if !space {
            self.jump_hold = 0;
            boost = self.jump_boost;
        } else {
            self.jump_hold += dt;
            if self.jump_hold <= 0 {
                boost = self.jump_boost;
            } else {
                let phys = u32_at(&mv.0, ent::PHYS);
                let vz = f32_at(&mv.0, vz_o);
                // 0x004a8cab: `comiss 0, vz; jb` (a NaN speed does not jump).
                let jump = if self.jump_hold >= 0xc8 {
                    false
                } else if phys & 1 != 0 {
                    0.0 >= vz
                } else {
                    i32_at(&mv.0, ent::ROLL) != 0 && 0.0 >= vz
                };
                if jump {
                    let v = vec3_at(&mv.0, ent::VEL);
                    let h = sqrt_f(v[1] * v[1] + v[0] * v[0]);
                    let mut up = 10.0f32;
                    if 10.0f32 > h + 2.0f32 {
                        let h = sqrt_f(v[1] * v[1] + v[0] * v[0]);
                        up = h + 2.0f32;
                    }
                    wf32(&mut mv.0, vz_o, up);
                    boost = 100;
                } else {
                    boost = self.jump_boost;
                }
                let phys = u32_at(&mv.0, ent::PHYS);
                let vz = f32_at(&mv.0, vz_o);
                if phys & 2 != 0 && 10.0f32 > vz {
                    boost = 100;
                    wf32(&mut mv.0, vz_o, dt as f32 * 0.001f32 + vz);
                    take_boost = true;
                }
            }
        }
        if take_boost || boost > 0 {
            if space {
                let vz = f32_at(&mv.0, vz_o);
                if 10.0f32 > vz {
                    let n = dt as f32 * 0.08f32 + vz;
                    wf32(&mut mv.0, vz_o, n);
                    if n >= 10.0f32 {
                        wf32(&mut mv.0, vz_o, 10.0);
                    }
                }
            }
        }
        let mut b = boost - dt;
        if b < 0 {
            b = 0;
        }
        self.jump_boost = b;
    }

    /// `applyMovementInput` 0x004a8263..0x004a8cae: the lock-on. Among the hostile (type 1 or
    /// flag 0x20) living creatures on screen (through `view_shake` then the projection, in
    /// front of the camera and inside -1..1) with a clear line from the creature to the eye
    /// (200 blocks, statics included): the nearest to the player, and the nearest one farther
    /// than the current target. Pressing again on the nearest moves to the next.
    fn lock_on(&mut self, world: &World, entities: &BTreeMap<i64, EntityData>) {
        let Some(player) = entities.get(&self.id) else { return };
        let ppos = pos_of(player);
        let on_screen = |pos: [i64; 3]| -> bool {
            let v = self.camera.relative(pos);
            let p = self.camera.view_shake.transform_point(v);
            if p[2] <= 0.0 {
                return false;
            }
            let s = self.projection.transform_point(p);
            !(-1.0f32 > s[0]) && !(s[0] > 1.0f32) && !(-1.0f32 > s[1]) && !(s[1] > 1.0f32)
        };
        let dist = |pos: [i64; 3]| {
            let d = sub3(pos, ppos);
            fixed_dot(d, d) as f32 * K
        };
        let candidate = |e: &EntityData| (e.0[ent::HOSTILE] == 1 || e.0[ent::FLAGS] & 0x20 != 0) && 0.0 < f32_at(&e.0, ent::HP);
        let mut best = 10000.0f32;
        let mut first = 0i64;
        for (id, e) in entities {
            if !candidate(e) || !on_screen(pos_of(e)) {
                continue;
            }
            let d = dist(pos_of(e));
            if best > d && cw_sim::path::line_of_sight(world, pos_of(e), self.camera.eye, true, 200.0) {
                best = d;
                first = *id;
            }
        }
        let current = if self.lock_target != 0 { entities.get(&self.lock_target).map_or(0.0, |t| dist(pos_of(t))) } else { 0.0 };
        let mut best2 = 10000.0f32;
        let mut second = 0i64;
        for (id, e) in entities {
            if !candidate(e) || !on_screen(pos_of(e)) {
                continue;
            }
            let d = dist(pos_of(e));
            if best2 > d && d >= current && cw_sim::path::line_of_sight(world, pos_of(e), self.camera.eye, true, 200.0) && *id != self.lock_target {
                best2 = d;
                second = *id;
            }
        }
        let mut t = first;
        if t == self.lock_target {
            t = 0;
        }
        self.lock_target = t;
        if second != 0 {
            self.lock_target = second;
        }
    }

    /// 0x0049b18d..0x0049b58e: the ray hit (`entity+0x150`, relative to the player). In build
    /// mode WASD, Ctrl and Space move it by `dt * 0.005` blocks along the camera's yaw;
    /// otherwise (outside click-to-walk) Shift points it one block ahead of the body's yaw and
    /// without Shift it is the aim point.
    pub fn build_cursor(&mut self, e: &mut EntityData, input: &ControllerBytes, ui: &UiState, dt: i32, aim_point: [i64; 3]) {
        if self.build_mode {
            let mut r = Mat4::IDENTITY;
            r.rotate_z(self.camera.rot[2]);
            let step = dt as f32 * 0.005f32;
            let moves: [(bool, [f32; 3]); 6] = [
                (input.forward(), [0.0, -1.0, 0.0]),
                (input.back(), [0.0, 1.0, 0.0]),
                (input.left(), [1.0, 0.0, 0.0]),
                (input.right(), [-1.0, 0.0, 0.0]),
                (input.ctrl(), [0.0, 0.0, -1.0]),
                (input.space(), [0.0, 0.0, 1.0]),
            ];
            for (on, v) in moves {
                if on {
                    let d = r.mul_dir(v);
                    let h = vec3_at(&e.0, ent::RAY_HIT);
                    // 0x00412850 is `+=`.
                    set_vec3(&mut e.0, ent::RAY_HIT, [h[0] + d[0] * step, h[1] + d[1] * step, h[2] + d[2] * step]);
                }
            }
        } else if !ui.click_to_walk {
            if input.shift() {
                let mut r = Mat4::IDENTITY;
                r.rotate_z(f32_at(&e.0, 0x20));
                set_vec3(&mut e.0, ent::RAY_HIT, r.mul_dir([0.0, 1.0, 0.0]));
            } else {
                // 0x0049b55e: the aim point of the frame ([`crate::interact::Aim::point`])
                // relative to the player.
                let rel = blocks3(sub3(aim_point, pos_of(e)));
                set_vec3(&mut e.0, ent::RAY_HIT, rel);
            }
        }
    }

    /// 0x0049b58e..0x0049c02f: the attack and skill inputs, with the cursor captured and out of
    /// build mode. The `+0x18` byte (never set by the input) starts mode 0x69. Slots 1..4 of
    /// the skill bar follow the right button and keys 1..3 (4 with a fifth slot): a press starts
    /// the slot's mode by its own rules (slot 1 only when the current mode can be interrupted),
    /// a release ends the held modes. Without any slot pressed, the left button starts the
    /// basic attack. Held blocks (0x1c, 0x1d) end when their button is up.
    pub fn attack_inputs(&mut self, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, input: &ControllerBytes, ui: &UiState, out: &mut ServerUpdate) {
        // 0x0049b58e: the R, E, T latches are stored here, before the skip.
        self.latches.r = input.key_r();
        self.latches.e = input.key_e();
        self.latches.t = input.key_t();
        if ui.cursor_free || self.build_mode {
            return;
        }
        let id = self.id;
        let (guard, haste) = cw_sim::util::guard_haste(states, id);
        let Some(mut e) = entities.get(&id).cloned() else { return };
        let mut st = states.get(&id).cloned().unwrap_or_default();
        // 0x0049b5be: the `+0x18` byte.
        if input.key_0x18() && !self.latches.key_0x18 && (e.0[ent::MODE] == 0 || !(i32_at(&e.0, ent::MODE_TIME) <= cw_sim::skills::skill_total_time(&e, guard, haste))) {
            e.0[ent::MODE] = 0x69;
            wi32(&mut e.0, ent::MODE_TIME, 0);
        }
        self.latches.key_0x18 = input.key_0x18();
        let bar = self.skill_bar.clone();
        let mut any = false;
        let n = bar.len() as i32;
        let restore_slot12 = |e: &mut EntityData, st: &mut CreatureState| {
            // 0x0049b686: an item of type 0x14 in slot 12 goes back to the inventory.
            let s = ent::slot(12);
            if e.0[s] == 0x14 {
                st.inventory.add_item(Item::from_bytes(&e.0[s..s + Item::SIZE]), -1);
                e.0[s..s + Item::SIZE].copy_from_slice(&Item::NEW.to_bytes());
            }
        };
        if n - 1 > 0 {
            let mut i = 1usize;
            while ((i as i32) - 1) < n - 1 {
                let entry = bar[i];
                if entry != 0 {
                    if input.at(i + 4) {
                        any = true;
                        let allowed = can_interrupt(&e, guard, haste) || (i as i32) - 1 > 0;
                        if allowed && cw_sim::combat_ai::can_use(&e, &st, entry) {
                            restore_slot12(&mut e, &mut st);
                            self.press_skill(world, &mut e, &mut st, entry, guard, haste, out);
                        }
                    } else if entry == i32::from(e.0[ent::MODE]) {
                        release_skill(&mut e, entry, guard, haste);
                    }
                }
                i += 1;
            }
        }
        // 0x0049bf70: the basic attack.
        if !any && e.0[ent::MODE] != 0x30 && input.left_mouse() {
            let (g, h) = (st.block, cw_sim::util::has_buff(&st, 0xc));
            cw_sim::combat_ai::choose_basic(&mut e, g, h);
            restore_slot12(&mut e, &mut st);
        }
        // 0x0049bfc8: the held blocks end with their button.
        if !input.right_mouse() && bar.get(1).copied() == Some(0x1c) && e.0[ent::MODE] == 0x1c {
            e.0[ent::MODE] = 0;
        }
        if !input.left_mouse() && bar.first().copied() == Some(0x1d) && e.0[ent::MODE] == 0x1d {
            e.0[ent::MODE] = 0;
        }
        if any {
            set_flag(&mut e, 1, false);
        }
        entities.insert(id, e);
        states.insert(id, st);
    }

    /// The press table of 0x0049b6db (byte index 0x0049d57c into 0x0049d52c).
    fn press_skill(&mut self, _world: &mut World, e: &mut EntityData, st: &mut CreatureState, entry: i32, guard: f32, haste: bool, out: &mut ServerUpdate) {
        let id = self.id;
        let mp = f32_at(&e.0, ent::MP);
        let start = |e: &mut EntityData, m: u8| {
            e.0[ent::MODE] = m;
            wi32(&mut e.0, ent::MODE_TIME, 0);
            wi32(&mut e.0, ent::ROLL, 0);
        };
        match entry {
            3 | 4 | 0x17 | 0x1f | 0x21 | 0x25 | 0x2d | 0x2e | 0x41 | 0x42 | 0x5e | 0x5f => {
                cw_sim::combat_ai::choose_special(e, st, guard, haste);
            }
            5 | 0x11 | 0x14 => {
                if mp > 0.0 {
                    start(e, entry as u8);
                }
            }
            0xa => {
                if e.0[ent::MODE] != 0xa {
                    start(e, 0xa);
                }
            }
            0xb => start(e, 0xb),
            8 | 0x18 | 0x19 | 0x1b | 0x3b | 0x3f | 0x40 => {
                if i32::from(e.0[ent::MODE]) != entry {
                    start(e, entry as u8);
                }
            }
            0x1c => {
                if mp > 0.0 && i32::from(e.0[ent::MODE]) != entry {
                    e.0[ent::MODE] = entry as u8;
                    wi32(&mut e.0, ent::MODE_TIME, 0);
                }
            }
            0x22 => {
                if !(i32_at(&e.0, ent::MODE_TIME) < cw_sim::skills::skill_total_time(e, guard, haste)) {
                    e.0[ent::MODE] = 0x22;
                    wi32(&mut e.0, ent::MODE_TIME, 0);
                    wi64(&mut e.0, ent::TARGET, id);
                }
            }
            0x36 => {
                e.0[ent::MODE] = 0x36;
                let h = vec3_at(&e.0, ent::RAY_HIT);
                let mut v = [h[0] * 2.0f32, h[1] * 2.0f32, h[2] * 2.0f32];
                self.leap_time = (length(v) / 50.0f32 * 1000.0f32) as i32;
                if len_sq(v) > 2500.0 {
                    normalize(&mut v);
                    v = [v[0] * 50.0f32, v[1] * 50.0f32, v[2] * 50.0f32];
                }
                v[2] = 15.0;
                set_vec3(&mut e.0, ent::VEL, v);
                wi32(&mut e.0, ent::MODE_TIME, 0);
                wi32(&mut e.0, ent::ROLL, 0);
            }
            0x30 => {
                e.0[ent::MODE] = 0x30;
                let mut v = vec3_at(&e.0, ent::RAY_HIT);
                self.leap_time = (length(v) / 50.0f32 * 1000.0f32) as i32;
                if len_sq(v) > 0.0 {
                    normalize(&mut v);
                    v = [v[0] * 50.0f32, v[1] * 50.0f32, v[2] * 50.0f32];
                    if v[2] > 5.0 {
                        v[2] = 5.0;
                    }
                }
                set_vec3(&mut e.0, ent::VEL, v);
                wi32(&mut e.0, ent::MODE_TIME, 0);
                wi32(&mut e.0, ent::ROLL, 0);
            }
            0x32 => {
                // 0x0049baf4: a dash on the extra velocity, no mode.
                let h = vec3_at(&e.0, ent::RAY_HIT);
                let mut v = [h[0] * -1.0f32, h[1] * -1.0f32, h[2] * -1.0f32];
                v[2] = 0.0;
                if len_sq(v) > 0.0 {
                    normalize(&mut v);
                    let s = cw_sim::skills::skill_level_factor(e, 0x32, -1) * 15.0f32 + 20.0f32;
                    v = [v[0] * s, v[1] * s, v[2] * s];
                    v[2] = cw_sim::skills::skill_level_factor(e, 0x32, -1) * 12.0f32 + 5.0f32;
                }
                set_vec3(&mut e.0, ent::EXTRA_VEL, v);
                st.cooldowns.insert(0x32, skill_cooldown(e, 0x32, -1));
            }
            0x60 => {
                e.0[ent::MODE] = 0x60;
                let h = vec3_at(&e.0, ent::RAY_HIT);
                let mut v = [h[0] * -1.0f32, h[1] * -1.0f32, h[2] * -1.0f32];
                v[2] = 0.0;
                if len_sq(v) > 0.0 {
                    normalize(&mut v);
                    v = [v[0] * 30.0f32, v[1] * 30.0f32, v[2] * 30.0f32];
                    v[2] = 20.0;
                }
                set_vec3(&mut e.0, ent::VEL, v);
                wi32(&mut e.0, ent::MODE_TIME, 0);
                wi32(&mut e.0, ent::ROLL, 600);
            }
            0x61 | 100 | 0x66 => self_buff(e, st, id, entry, out),
            0x65 => {
                self_buff(e, st, id, entry, out);
                self.sound(0x5d, pos_of(e), 1.0, 1.0);
            }
            0x67 => {
                self_buff(e, st, id, entry, out);
                self.sound(0x5c, pos_of(e), 1.0, 1.0);
            }
            0x63 => {
                set_flag(e, 0x400, true);
                st.cooldowns.insert(0x63, skill_cooldown(e, 0x63, -1));
            }
            _ => {
                let cost = cw_sim::util::mana_cost(e, st, entry, -1);
                if !(mp < cost) {
                    start(e, entry as u8);
                }
            }
        }
    }
}

/// The release table of 0x0049bdf2 (byte index 0x0049d610 into 0x0049d5e4): a held mode whose
/// button went up turns into its follow-up.
fn release_skill(e: &mut EntityData, entry: i32, guard: f32, haste: bool) {
    let set = |e: &mut EntityData, m: u8| {
        e.0[ent::MODE] = m;
        wi32(&mut e.0, ent::MODE_TIME, 0);
    };
    let windup = |e: &mut EntityData, m: u8| {
        e.0[ent::MODE] = m;
        let w = cw_sim::skills::skill_windup(e, guard, haste, -1);
        wi32(&mut e.0, ent::MODE_TIME, w);
    };
    match entry {
        8 => set(e, 0x68),
        0xa => {
            if i32_at(&e.0, ent::MODE_TIME) > cw_sim::skills::skill_total_time(e, guard, haste) {
                e.0[ent::MODE] = 3;
                let t = cw_sim::skills::skill_total_time(e, guard, haste);
                wi32(&mut e.0, ent::MODE_TIME, t);
            }
        }
        0x18 => set(e, 0x17),
        0x19 => set(e, 0x37),
        0x1b => set(e, 0x1a),
        0x23 => set(e, 0),
        0x24 => set(e, 0x25),
        0x3b => {
            if e.0[ent::SPEC] == 1 {
                set(e, 0x3d);
            } else {
                windup(e, 0x3c);
            }
        }
        0x3f => windup(e, 0xb),
        0x40 => windup(e, 0x10),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// The frame: the gameplay ranges of `update` in the original's order.

/// What the gameplay half needs from the rest of the controller for one frame.
pub struct FrameInput<'a> {
    pub input: &'a ControllerBytes,
    pub ui: &'a UiState,
    pub opts: &'a CameraOptions,
    /// `update`'s `dt` in milliseconds.
    pub dt: i32,
    /// `world+0x8000bc` (the camera shake's phase).
    pub world_ms: i32,
    /// `GameController+0x2bc`: the zone the item aim scans around.
    pub center_zone: [i32; 2],
    pub models: &'a dyn crate::interact::ItemModels,
}

impl LocalPlayer {
    /// 0x0048b8ab..0x0048ce2e, before `World::tick` (0x0048cf2e): the skill bar, the dead latch
    /// and the pickup queue.
    pub fn before_tick(&mut self, entities: &BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, dt: i32) {
        let Some(e) = entities.get(&self.id) else { return };
        let (guard, haste) = cw_sim::util::guard_haste(states, self.id);
        self.skill_bar = build_skill_bar(e, guard, haste);
        self.death_latch(e);
        let st = states.entry(self.id).or_default();
        self.pickup_step(e, st, dt);
    }

    /// 0x00491f16..0x0049ced6, after `World::tick`: the gameplay ranges in order (the UI and
    /// rendering ranges between them belong to the other modules; see the report).
    pub fn after_tick(&mut self, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, out: &mut ServerUpdate, f: &FrameInput) {
        use crate::interact as ia;
        let id = self.id;
        if !entities.contains_key(&id) {
            return;
        }
        // 0x00491f16: the quick items and Q.
        {
            let mut e = entities[&id].clone();
            let mut st = states.get(&id).cloned().unwrap_or_default();
            ia::quick_use_q(self, &mut e, &mut st, f.input, f.ui);
            entities.insert(id, e);
            states.insert(id, st);
        }
        // 0x00493b0f: the combo, level and XP display.
        self.xp_display(&entities[&id]);
        // 0x004965a2: the camera eases.
        self.camera.smooth(f.opts, f.dt);
        // 0x0049662f..0x004975bb: the held stack, the villager and mount, R.
        ia::drop_held_item(self, f.input, f.ui);
        ia::talk_or_mount(self, entities, f.input, f.ui);
        ia::r_key(self, world, entities, states, f.input);
        // 0x004975c1..0x00497723: E and T.
        {
            let e = entities[&id].clone();
            let st = states.get(&id).cloned().unwrap_or_default();
            ia::e_key(self, world, &e, &st, f.input);
        }
        ia::t_key(self, f.input);
        // 0x00497723: the acceleration restarts (or the glide steers).
        if let Some(e) = entities.get_mut(&id) {
            self.glide_steering(e, f.dt);
        }
        // 0x00497892: the right stick.
        self.right_stick_camera(f.input, f.opts, f.dt);
        // 0x00497952..0x00498409: the quick menu, build mode or the movement.
        ia::quick_menu_prelude(self, f.input);
        let (guard, haste) = cw_sim::util::guard_haste(states, id);
        if !ia::build_r(self, f.input) {
            let mut e = entities[&id].clone();
            let mut st = states.get(&id).cloned().unwrap_or_default();
            if ia::quick_menu(self, &mut e, &mut st, f.input, f.dt) {
                entities.insert(id, e);
                states.insert(id, st);
            } else {
                self.movement(world, entities, states, f.input, f.ui, f.dt, (guard, haste));
            }
        }
        // 0x00498409..0x00499e9b: the aim.
        let aim = ia::aim(self, world, entities, states, f.models, f.dt, f.center_zone);
        if let Some(e) = entities.get_mut(&id) {
            ia::aim_targets(self, e, states, f.input, aim.nearest_on_crosshair);
        }
        // 0x00496548: the creature menu closes 25 blocks away.
        if self.menu_creature != 0 {
            let far = match (entities.get(&self.menu_creature), entities.get(&id)) {
                (Some(c), Some(e)) => !(len_sq(blocks3(sub3(pos_of(c), pos_of(e)))) < 625.0),
                _ => true,
            };
            if far {
                self.menu_creature = 0;
            }
        }
        // 0x0049b18d: the ray hit.
        if let Some(e) = entities.get_mut(&id) {
            self.build_cursor(e, f.input, f.ui, f.dt, aim.point);
        }
        // 0x0049b58e: the attack inputs.
        self.attack_inputs(world, entities, states, f.input, f.ui, out);
        // 0x0049c272: the mode timeout.
        let (guard, haste) = cw_sim::util::guard_haste(states, id);
        if let Some(e) = entities.get_mut(&id) {
            mode_timeout(e, f.input.right_mouse(), guard, haste);
        }
        // 0x0049c2e6: the camera follows and the matrices are rebuilt.
        let step = states.get(&id).map_or(0.0, |s| s.step_offset);
        let e = entities[&id].clone();
        self.camera.place(world, &e, step, f.ui, f.dt, f.world_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputState, dik};
    use cw_world::zone::Zone;

    /// A 256x256 zone of solid ground up to z = 10 (every column starts at height 10 with no
    /// blocks: below is solid, above is air).
    fn flat_world() -> (World, [i64; 3]) {
        let mut world = World::new(1);
        let (zx, zy) = (0x8000, 0x8000);
        let mut z = Zone::new(zx, zy);
        for c in z.columns.iter_mut() {
            c.height = 10;
        }
        world.insert_zone(Box::new(z));
        let x = (i64::from(zx) * 256 + 128) << 16;
        let y = (i64::from(zy) * 256 + 128) << 16;
        (world, [x, y, 10 << 16])
    }

    fn player_at(pos: [i64; 3]) -> EntityData {
        let mut e = EntityData::constructed();
        e.0[ent::HOSTILE] = 0;
        set_pos(&mut e, pos);
        // The box of a human.
        set_vec3(&mut e.0, 0x70, [0.96, 0.96, 2.16]);
        e
    }

    /// One 20 ms step of the shared tick for the player alone.
    fn tick(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64) {
        let mut out = ServerUpdate::default();
        let mut projectiles = Vec::new();
        let mut dirty = std::collections::BTreeSet::new();
        if cw_sim::update::update_creature(world, entities, states, &mut projectiles, &mut dirty, id, 20, &mut out) {
            cw_sim::physics::move_creature(world, entities, states, id, 20, &mut out);
        }
    }

    #[test]
    fn lerp_factor_matches_closed_form() {
        assert_eq!(lerp_factor(0, 0.1), 0.0);
        let f = lerp_factor(10, 0.1);
        assert!((f - (1.0 - 0.9f32.powi(10))).abs() < 1e-6);
    }

    #[test]
    fn heading_matrix() {
        let mut m = Mat4::IDENTITY;
        m.rotate_z(0.0);
        assert_eq!(m.0[0], 1.0);
        assert_eq!(m.0[5], 1.0);
        let mut m = Mat4::IDENTITY;
        m.rotate_z(90.0);
        assert!((m.0[1] - 1.0).abs() < 1e-6 && (m.0[4] + 1.0).abs() < 1e-6);
    }

    /// 0x0049c6c0..0x0049c76a: with the start screen (or another menu screen) up, the camera
    /// orbits the world spawn from 50 blocks above that column's top, whatever the player does.
    #[test]
    fn menu_camera_orbits_fifty_blocks_above_the_spawn_column() {
        let mut world = World::new(1);
        let sx = world.spawn[0] as i32;
        let sy = world.spawn[1] as i32;
        let mut z = Zone::new(sx / 256, sy / 256);
        let col = z.column_mut(sx, sy);
        col.height = 10;
        col.blocks = vec![[1, 1, 1, 1]; 3];
        world.insert_zone(Box::new(z));
        let ui = UiState { camera_panel_open: true, ..UiState::default() };
        let mut c = Camera { rot: [100.0, 0.0, 0.0], dist: 10.0, ..Camera::default() };
        // The player far away at the origin (the snap to the player happens first and is
        // overridden).
        c.place(&world, &player_at([0, 0, 0]), 0.0, &ui, 16, 0);
        let spawn = fix3([world.spawn[0], world.spawn[1], 0.0]);
        assert_eq!(c.target, [spawn[0], spawn[1], (13 + 50) << 16]);
        // The eye backs off 10 blocks along the view direction (nothing solid up there).
        assert!(c.eye[2] > c.target[2]);
        // No zone at the spawn yet: the spawn's own z (0) plus 50.
        let world = World::new(1);
        let mut c = Camera { rot: [100.0, 0.0, 0.0], dist: 10.0, ..Camera::default() };
        c.place(&world, &player_at([0, 0, 0]), 0.0, &ui, 16, 0);
        assert_eq!(c.target, [spawn[0], spawn[1], 50 << 16]);
    }

    #[test]
    fn wheel_and_mouse_clamps() {
        let mut c = Camera::default();
        c.on_mouse_wheel(false, -10);
        assert_eq!(c.dist_target, 14.0);
        c.on_mouse_wheel(false, 10);
        assert_eq!(c.dist_target, 0.0);
        let o = CameraOptions { camera_speed: 100, camera_smoothness: 50, invert_y: false };
        c.on_mouse_move(&o, 0.0, 1000.0);
        assert_eq!(c.rot_target[0], 180.0);
        c.on_mouse_move(&o, 10.0, -1000.0);
        assert_eq!(c.rot_target[0], 0.0);
        assert_eq!(c.rot_target[2], -5.0);
    }

    #[test]
    fn skill_bar_for_a_warrior_with_a_sword() {
        let mut e = EntityData::constructed();
        e.0[ent::CLASS] = 1;
        e.0[ent::slot(7)] = 3;
        e.0[ent::slot(7) + 1] = 0;
        wi32(&mut e.0, ent::SKILLS_BAR, 1);
        let bar = build_skill_bar(&e, 0.0, false);
        // The basic attack, the sword's second move (0x3f without a shield), the class skills
        // with the second and third emptied for want of points.
        assert_eq!(bar[1], 0x3f);
        assert_eq!(&bar[2..], &[0x36, 0, 0]);
    }

    #[test]
    fn walk_forward_through_the_shared_tick() {
        let (mut world, start) = flat_world();
        let id = 1i64;
        let mut entities = BTreeMap::new();
        entities.insert(id, player_at(start));
        let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
        states.insert(id, CreatureState { stamina: 1.0, ..CreatureState::default() });
        let mut p = LocalPlayer::new(id);
        let mut input = InputState::default();
        input.press(dik::W);
        let bytes = ControllerBytes::from_input(&input);
        let ui = UiState::default();
        // Let the player settle on the ground first.
        for _ in 0..10 {
            tick(&mut world, &mut entities, &mut states, id);
        }
        let before = pos_of(&entities[&id]);
        for _ in 0..50 {
            if let Some(e) = entities.get_mut(&id) {
                p.glide_steering(e, 20);
            }
            p.movement(&world, &mut entities, &mut states, &bytes, &ui, 20, (0.0, false));
            tick(&mut world, &mut entities, &mut states, id);
        }
        let after = pos_of(&entities[&id]);
        let moved = blocks3(sub3(after, before));
        // Yaw 0 heads towards -y; one second of walking covers several blocks.
        assert!(moved[1] < -2.0, "moved {moved:?}");
        assert!(moved[0].abs() < 0.01, "moved {moved:?}");
        assert!(moved[2].abs() < 0.5, "moved {moved:?}");
        // The movement set the run flag (no Shift) and left the climb flag off.
        let flags = u16_at(&entities[&id].0, ent::FLAGS);
        assert!(flags & 0x40 != 0 && flags & 1 == 0);
    }

    #[test]
    fn jump_leaves_the_ground() {
        let (mut world, start) = flat_world();
        let id = 1i64;
        let mut entities = BTreeMap::new();
        entities.insert(id, player_at(start));
        let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
        states.insert(id, CreatureState { stamina: 1.0, ..CreatureState::default() });
        for _ in 0..10 {
            tick(&mut world, &mut entities, &mut states, id);
        }
        assert!(u32_at(&entities[&id].0, ent::PHYS) & 1 != 0, "on the ground");
        let mut p = LocalPlayer::new(id);
        let mut input = InputState::default();
        input.press(dik::SPACE);
        let bytes = ControllerBytes::from_input(&input);
        let ui = UiState::default();
        let z0 = pos_of(&entities[&id])[2];
        let mut top = z0;
        for _ in 0..10 {
            if let Some(e) = entities.get_mut(&id) {
                p.glide_steering(e, 20);
            }
            p.movement(&world, &mut entities, &mut states, &bytes, &ui, 20, (0.0, false));
            tick(&mut world, &mut entities, &mut states, id);
            top = top.max(pos_of(&entities[&id])[2]);
        }
        assert!(top > z0 + (1 << 15), "rose {}", (top - z0) as f32 / 65536.0);
    }

    /// The aim (0x00498409..0x00499e9b) and R on a villager (0x00496922..0x00496a4f): a
    /// villager three blocks in front of the camera is aimed, and R (edge) queues Interact 2
    /// with its spawn triple and the talk event.
    #[test]
    fn villager_in_front_is_aimed_and_r_talks() {
        let (mut world, start) = flat_world();
        let id = 1i64;
        let vid = 7i64;
        let mut entities = BTreeMap::new();
        entities.insert(id, player_at(start));
        let mut villager = player_at(add3(start, [0, -(3 << 16), 0]));
        villager.0[ent::HOSTILE] = 3;
        wi32(&mut villager.0, 0x1a0, 11);
        wi32(&mut villager.0, 0x1a4, 12);
        wi32(&mut villager.0, 0x1a8, 13);
        entities.insert(vid, villager);
        let mut states: BTreeMap<i64, CreatureState> = BTreeMap::new();
        states.insert(id, CreatureState { stamina: 1.0, ..CreatureState::default() });
        states.insert(vid, CreatureState::default());
        for _ in 0..10 {
            tick(&mut world, &mut entities, &mut states, id);
            tick(&mut world, &mut entities, &mut states, vid);
        }
        for (k, s) in states.iter_mut() {
            s.riding.render_pos = pos_of(&entities[k]);
        }
        // Midday; the camera behind the player looking towards -y (yaw 0, pitch 90).
        world.time_of_day = 12 * 3_600_000;
        let mut p = LocalPlayer::new(id);
        p.camera.rot = [90.0, 0.0, 0.0];
        p.camera.rot_target = p.camera.rot;
        p.camera.dist = 5.0;
        p.camera.dist_target = 5.0;
        p.camera.target = pos_of(&entities[&id]);
        let ui = UiState::default();
        p.camera.place(&world, &entities[&id], 0.0, &ui, 16, 0);
        p.projection = Mat4(crate::controller::from_d3d(&cw_render::passes::projection([1280, 720], false)));
        struct NoModels;
        impl crate::interact::ItemModels for NoModels {
            fn size(&self, _: u32) -> Option<[i32; 3]> {
                None
            }
        }
        let aim = crate::interact::aim(&mut p, &world, &entities, &mut states, &NoModels, 16, [0x8000, 0x8000]);
        assert_eq!(p.aimed_creature, vid, "aim {aim:?}");
        assert!(aim.highlighted.contains(&vid));
        // R pressed this frame (the latch is still up from the last one).
        let mut input = InputState::default();
        input.press(dik::R);
        let bytes = ControllerBytes::from_input(&input);
        crate::interact::talk_or_mount(&mut p, &mut entities, &bytes, &ui);
        assert_eq!(p.interact_queue.len(), 1);
        let it = &p.interact_queue[0];
        assert_eq!(it.0[0x128], 2);
        assert_eq!([i32_at(&it.0, 0x118), i32_at(&it.0, 0x11c), i32_at(&it.0, 0x120)], [11, 12, 13]);
        assert!(p.events.contains(&Event::Talk { id: vid }));
    }

    #[test]
    fn lamp_and_glider_keys() {
        let mut e = EntityData::constructed();
        toggle_lamp(&mut e);
        assert!(u16_at(&e.0, ent::FLAGS) & 0x200 != 0);
        toggle_lamp(&mut e);
        assert!(u16_at(&e.0, ent::FLAGS) & 0x200 == 0);
        // A glider (type 0x17, sub type 0) in slot 11, standing where the ground was.
        e.0[ent::slot(11)] = 0x17;
        use_special_item(&mut e, 0.0);
        assert!(u16_at(&e.0, ent::FLAGS) & 0x10 != 0);
        use_special_item(&mut e, 0.0);
        assert!(u16_at(&e.0, ent::FLAGS) & 0x10 == 0);
        // No item: the glide flag clears.
        set_flag(&mut e, 0x10, true);
        e.0[ent::slot(11)] = 0;
        use_special_item(&mut e, 0.0);
        assert!(u16_at(&e.0, ent::FLAGS) & 0x10 == 0);
    }
}
