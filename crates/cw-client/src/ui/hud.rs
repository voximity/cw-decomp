//! The HUD content `GameController::update` 0x00488ee0 writes each frame into the `gui.plx`
//! bar nodes: texts, bar fills (the x scale of each bar's `bar` child, set as
//! `identity · scale(fill, 1, 1)` through 0x00423e70 / 0x00424730), node positions (the
//! widget position written through 0x00468df0 / 0x004288b0) and visibility. The drawing is
//! the renderer's.
//!
//! Tier B. Screen sizes are the client size `GC+0x11c` (w) / `GC+0x120` (h); `w / 2` is the
//! original's `cdq; sub; sar 1` (truncating) division.
//!
//! # Map of the ported ranges
//!
//! | Range | Node (member) | Content | Here |
//! |---|---|---|---|
//! | 0x00490a93..0x00490cf5 | `experiencebar` +0x8007c4 | `xp << "/" << xpNeeded` (0x00444d60), fill `xp / needed` ≤ 1 | [`hud_frame`] |
//! | 0x00490cf5..0x00490e37 | `manacubebar` +0x8007d8 | at (100, 130); the first `cubes % 4` children shown (all 4 while the node animates) | [`HudBars::manacube`] |
//! | 0x00490e37..0x004911d1 | `lifebar` +0x800768 (+0x80076c `bar`) | at (100, 30): `text` `hp/maxHp`, `info` the name, `class` `LVL n` + class suffix, fill `hp / maxHp` | [`HudBars::player`] |
//! | 0x004911d1..0x00491902 | pet: +0x800770 (lifebar clone), +0x8007cc/+0x8007d0 (experience clone), `ridingbar` +0x8007d4 | shown while `creature+0x11c8` names a live creature: its life frame at (100, 180) (`class` = `LVL n` + " " + the type name 0x0059aa60), its XP at (100, 225), riding stamina cubes at (100, 290) | [`HudBars::pet`] |
//! | 0x00491902..0x00491f16 | party: the 3 lifebar clones at +0x800778 (bars +0x800784) | all hidden, then one per other player (hostility 0) in world-map order at (310 + 220 i, 30) | [`HudBars::party`] |
//! | 0x00492977..0x00492bd4 | `mpbar` +0x8007b8 (+0x8007bc `bar`, +0x8007c0 `chargebar`) | at (w/2 + 8, h − 90): `(int)(mp·100 + 0.5) << "/100"`, fill `mp`, charge fill `entity+0x134` | [`HudBars::mp`] |
//! | 0x00492bd4..0x00492ddd | `hpbar` +0x8007e4 (+0x8007e8) | at (w/2 − 163, h − 90): `hp/maxHp`, fill | [`HudBars::hp_position`] |
//! | 0x00492ddd..0x00492f1d | `staminabar` +0x8007f4 (+0x8007f8) | shown while `1 > stamina`, at (w/2, h − 115), fill | [`HudBars::stamina`] |
//! | 0x00492f1d..0x00493b0f | `castbar` +0x8007ec (+0x8007f0) | the rules of [`cast_bar`] at (w/2, h − 150) | [`HudBars::cast`] |
//! | 0x00493b0f..0x00493d72 | `combopoints` +0x800ad0 | when the hit counter changes: `"     n HIT(S)"` (+ `"!"` at the maximum), colour, the `hit` state | [`HudBars::combo`] |
//! | 0x00494161..0x0049434d | `landscape` +0x800858 | the cell/region name (0x004e5320) into `landscape` (size 20), the zone's site name (0x004e5c10) into `landscapedetail` (size 12) | [`landscape_names`], [`HudState::landscape`] |
//! | 0x0049434d..0x004944fe | `landname` +0x800758 | the centre banner fed with the same names | [`LandnameState`] |
//!
//! The quick bar and the XP gain / "LEVEL UP!" texts are in `crate::player`; the nameplates,
//! crosshairs and quest tags in [`super::target`].

use cw_net::EntityData;

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i64_at(e: &EntityData, o: usize) -> i64 {
    i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap())
}

/// The HUD values of one frame (the part [`hud_frame`] computes from the entity alone).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudFrame {
    /// Experience text and fill.
    pub xp_text: String,
    /// 0..1.
    pub xp_fill: f32,
    /// Life text (both `lifebar` and `hpbar`).
    pub life_text: String,
    /// `hp / maxHp`.
    pub life_fill: f32,
    /// Mana text of `mpbar`: `(int)(mp * 100 + 0.5) << L"/100"` (0x004929a7..0x004929d2).
    pub mp_text: String,
    /// `mp`.
    pub mp_fill: f32,
    /// Stamina bar shown.
    pub stamina_visible: bool,
    /// Stamina fill.
    pub stamina_fill: f32,
}

/// 0x00444d60: the experience a level needs, `(int)((1 - 1 / ((level - 1) * 0.05 + 1)) * 1000
/// + 50)`.
pub fn xp_needed(level: i32) -> i32 {
    ((1.0f32 - 1.0f32 / ((level as f32 - 1.0f32) * 0.05f32 + 1.0f32)) * 1000.0f32 + 50.0f32) as i32
}

/// The HUD values for the local player (`entity+0x184` XP, `+0x180` level, `+0x15c` HP,
/// `+0x160` MP) and its stamina (`creature+0x1194`, outside the entity block).
pub fn hud_frame(player: &EntityData, stamina: f32) -> HudFrame {
    let xp = i32_at(player, 0x184);
    let need = xp_needed(i32_at(player, 0x180));
    let r = xp as f32 / need as f32;
    let xp_fill = if r <= 1.0 { r } else { 1.0 };
    let hp = f32_at(player, 0x15c);
    let max = cw_sim::stats::max_hp(player);
    let mp = f32_at(player, 0x160);
    HudFrame {
        xp_text: format!("{xp}/{need}"),
        xp_fill,
        life_text: life_text(player),
        life_fill: hp / max,
        mp_text: format!("{}/100", (mp * 100.0f32 + 0.5f32) as i32),
        mp_fill: mp,
        // 0x00492df8: `1.0 > s` shows the bar (NaN hides it).
        stamina_visible: 1.0f32 > stamina,
        stamina_fill: stamina,
    }
}

// ---------------------------------------------------------------------------------------
// Shared texts
// ---------------------------------------------------------------------------------------

/// `(int)hp << L"/" << (int)maxHp`: `cvttss2si` on the HP (`entity+0x15c`), `_ftol2` on
/// `maxHp` 0x00444db0 (both truncate).
pub fn life_text(e: &EntityData) -> String {
    let hp = f32_at(e, 0x15c) as i32;
    let max = cw_sim::stats::max_hp(e) as i32;
    format!("{hp}/{max}")
}

/// The class suffix of the `class` texts (narrow strings streamed with 0x0040e440, jump table
/// 0x0049d3c0 / 0x0049d3d0 on `entity+0x130 - 1`): 1 " Warrior", 2 " Ranger", 3 " Mage",
/// 4 " Rogue", anything else nothing.
pub fn class_suffix(class: u8) -> &'static str {
    match class {
        1 => " Warrior",
        2 => " Ranger",
        3 => " Mage",
        4 => " Rogue",
        _ => "",
    }
}

/// `L"LVL " << level` + [`class_suffix`] (the local player's and the party frames' `class`
/// text; level `entity+0x180`).
pub fn level_class_text(e: &EntityData) -> String {
    format!("LVL {}{}", i32_at(e, 0x180), class_suffix(e.0[0x130]))
}

/// The fill `hp / maxHp` (SSE `divss` of the HP by the x87 `maxHp` spilled to f32).
pub fn life_fill(e: &EntityData) -> f32 {
    f32_at(e, 0x15c) / cw_sim::stats::max_hp(e)
}

// ---------------------------------------------------------------------------------------
// The bars
// ---------------------------------------------------------------------------------------

/// A life frame: `lifebar` or one of its clones (`text`, `info`, `class` children and the
/// `bar` fill).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LifeFrame {
    /// Widget position.
    pub position: [f32; 2],
    /// `text`: `hp/maxHp`.
    pub text: String,
    /// `info`: the name (`entity+0x1158`, narrow, widened by 0x006089c0).
    pub info: String,
    /// `class`: `LVL n` + suffix.
    pub class: String,
    /// `bar` x scale.
    pub fill: f32,
}

/// The pet frames (0x004911d1..0x00491902).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetFrame {
    /// +0x800770 at (100, 180).
    pub life: LifeFrame,
    /// +0x8007cc at (100, 225): `xp/needed` of the pet.
    pub xp_text: String,
    /// +0x8007d0 fill: `xp / needed`, 1 when larger than 1.
    pub xp_fill: f32,
    /// `ridingbar` +0x8007d4 at (100, 290): the first `riding_cubes` children shown.
    pub riding_cubes: i32,
}

/// `mpbar` (0x00492977..0x00492bd4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MpFrame {
    /// (w/2 + 8, h − 90).
    pub position: [f32; 2],
    /// `text`: `n/100`.
    pub text: String,
    /// `bar` (+0x8007bc): `mp` (`entity+0x160`).
    pub fill: f32,
    /// `chargebar` (+0x8007c0): the charged MP `entity+0x134`.
    pub charge_fill: f32,
}

/// `staminabar` (0x00492ddd..0x00492f1d); `None` in [`HudBars::stamina`] = hidden.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaminaFrame {
    /// (w/2, h − 115).
    pub position: [f32; 2],
    /// `bar`: `creature+0x1194`.
    pub fill: f32,
}

/// `castbar` (0x00492f1d..0x00493b0f); `None` in [`HudBars::cast`] = hidden.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CastFrame {
    /// (w/2, h − 150).
    pub position: [f32; 2],
    /// `name` child text.
    pub name: String,
    /// `bar` (+0x8007f0) x scale.
    pub fill: f32,
}

/// The `combopoints` node update (0x00493b0f..0x00493d64), emitted only on a change.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComboFrame {
    /// The node text (0x00636ad0): `"     " << n << " HIT"/" HITS"`, `"!"` appended when
    /// `n >= maxBlock`.
    pub text: String,
    /// The colour written to every shape of the node (0x00457930, `shape+0xb4`): cyan
    /// (0, 1, 1, 1) at the maximum, else white.
    pub color: [f32; 4],
    /// `setState("hit")` 0x00636810 is played on the node.
    pub play_hit: bool,
}

/// `manacubebar` (0x00490cf5..0x00490e37).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManacubeFrame {
    /// (100, 130).
    pub position: [f32; 2],
    /// The first `shown` children are visible, the rest hidden.
    pub shown: i32,
}

/// The HUD bars of one frame (besides [`HudFrame`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudBars {
    /// `lifebar` +0x800768 at (100, 30).
    pub player: LifeFrame,
    /// `manacubebar`.
    pub manacube: ManacubeFrame,
    /// Pet frames; `None` hides +0x800770, +0x8007cc and +0x8007d4.
    pub pet: Option<PetFrame>,
    /// The three party slots (+0x800778[i]); `None` = hidden.
    pub party: [Option<LifeFrame>; 3],
    /// `mpbar`.
    pub mp: MpFrame,
    /// `hpbar` position (w/2 − 163, h − 90); its text and fill are [`HudFrame::life_text`] /
    /// [`HudFrame::life_fill`].
    pub hp_position: [f32; 2],
    /// `staminabar`.
    pub stamina: Option<StaminaFrame>,
    /// `castbar`.
    pub cast: Option<CastFrame>,
    /// `combopoints`: `Some` only on the frame the hit counter changed.
    pub combo: Option<ComboFrame>,
}

/// State the bars keep between frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudState {
    /// The static byte 0x0076b101: the hit counter (`entity+0x60`) of the last frame,
    /// truncated to a byte (compared with the full int, 0x00493b20).
    pub last_hits: u8,
    /// The HUD nodes the ctor 0x00459c40 makes besides the `GcMembers` ones (clones and
    /// containers; built on the first `present_hud::apply`).
    pub nodes: Option<HudNodes>,
    /// The `landname` banner (0x00494161..0x004944fe).
    pub landname: LandnameState,
    /// The texts `update` writes this frame into `landscape` +0x800858, `(big, small)`:
    /// `setChildText(L"landscape", area, 1)` with the 0x004e5320 result (stack slot
    /// `esp+0x1564`, 0x004942d2) then `setChildText(L"landscapedetail", site, 1)` with the
    /// 0x004e5c10 result (`esp+0x1594`, 0x0049431c) (0x004942b1..0x00494348); `None`
    /// without a world (left as they are).
    pub landscape: Option<(String, String)>,
    /// The `info` text of this frame ([`info_text`], 0x00494554..0x00494885); `None`
    /// without a world (left as it is).
    pub info: Option<String>,
    /// `Node::isAnimating` 0x006364f0 of `combopoints` (+0x800ad0), evaluated by the caller
    /// on the `gui.plx` scene before the apply (`present_hud::animation_inputs`).
    pub combopoints_animating: bool,
    /// `Node::setState` 0x00636810 calls of this frame's apply (`combopoints` "hit",
    /// `smallquesttag` "show"), played on the `.plx` scenes by `present_hud::play_states`.
    pub pending_states: Vec<(cw_ui::widget::NodeId, &'static str)>,
    /// Display fill colours written this frame (the `landname` fade, 0x004944d6), applied
    /// to the owning scene's Display by `present_hud::play_scene_writes`.
    pub pending_fill: Vec<(cw_ui::widget::NodeId, [f32; 4])>,
    /// The chat draws of `ChatWidget::update` 0x00439730 this frame (computed in the apply,
    /// turned into text calls by `present_hud::widget_texts`).
    pub chat_draws: Vec<super::chat::ChatDraw>,
    /// The dialog's answer options drawn this frame (0x004e5f90): `(text, x, y, hovered)`.
    pub speech_options: Vec<(String, f32, f32, bool)>,
}

/// Nodes of the HUD the ctor 0x00459c40 creates that `GcMembers` does not hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudNodes {
    /// +0x8007c8: the `bar` of `experiencebar`.
    pub xp_bar_fill: Option<cw_ui::widget::NodeId>,
    /// +0x8007cc / +0x8007d0: `experiencebar` cloned into the GUI root (0x00460f82, the pet's
    /// experience) and its `bar`.
    pub pet_xp: Option<cw_ui::widget::NodeId>,
    pub pet_xp_fill: Option<cw_ui::widget::NodeId>,
    /// +0x80076c: the `bar` of `lifebar`.
    pub life_fill: Option<cw_ui::widget::NodeId>,
    /// +0x800770 / +0x800774: `lifebar` cloned into the GUI root (0x00461211, the pet's life)
    /// and its `bar`.
    pub pet_life: Option<cw_ui::widget::NodeId>,
    pub pet_life_fill: Option<cw_ui::widget::NodeId>,
    /// +0x800778[3] / +0x800784[3]: three more `lifebar` clones (the party, 0x0046128c) and
    /// their `bar`s.
    pub party: Vec<cw_ui::widget::NodeId>,
    pub party_fill: Vec<Option<cw_ui::widget::NodeId>>,
    /// +0x8007bc / +0x8007c0: `mpbar`'s `bar` and `chargebar`.
    pub mp_fill: Option<cw_ui::widget::NodeId>,
    pub charge_fill: Option<cw_ui::widget::NodeId>,
    /// +0x8007e0: `questbar`'s `bar` (`questbar` itself is found under `smallquesttag`,
    /// 0x00461072, and hidden).
    pub quest_fill: Option<cw_ui::widget::NodeId>,
    /// `questbar` under `smallquesttag` (the ctor's lookup; `GcMembers::quest_bar` searches
    /// the GUI root, which finds the same node).
    pub quest_bar: Option<cw_ui::widget::NodeId>,
    /// +0x8007e8, +0x8007f0, +0x8007f8: the `bar`s of `hpbar`, `castbar`, `staminabar`.
    pub hp_fill: Option<cw_ui::widget::NodeId>,
    pub cast_fill: Option<cw_ui::widget::NodeId>,
    pub stamina_fill: Option<cw_ui::widget::NodeId>,
    /// +0x8007b0: the nameplate container (`createNode ''` under the GUI root, 0x00461741).
    pub plates: Option<cw_ui::widget::NodeId>,
    /// The nameplate clones kept for reuse, by template index (`PlateKind::member_index`
    /// - 4). The original deletes and re-clones them every frame (0x00632870 /
    /// 0x00636040); reusing them keeps the node arena bounded.
    pub plate_pool: [Vec<cw_ui::widget::NodeId>; 4],
    /// +0x800920[4] / +0x80092c[4]: the speech bubbles (clones of the `speech` template,
    /// 0x00461ab3..0x00461c15, each with a `SpeechWidget`, hidden; the template is deleted,
    /// 0x00461c21).
    pub bubbles: Vec<(cw_ui::widget::NodeId, cw_ui::widget::WidgetId)>,
    /// +0x8008a0 / +0x8008a4: the map overlay root under the engine root and the node of
    /// the `MapOverlayWidget` (ctor 0x0045c090 / 0x0045c0d7).
    pub overlay_root: Option<cw_ui::widget::NodeId>,
    pub overlay_node: Option<cw_ui::widget::NodeId>,
    pub overlay: Option<cw_ui::widget::WidgetId>,
}

/// The region banner `landname` (+0x800758) of `update` 0x00494161..0x004944fe: when the
/// names of the player's position (0x004e5c10 the zone's site name, compared with the static
/// 0x0076b104; 0x004e5320 the cell or region name, compared with 0x0076b11c) differ and the
/// timer 0x0076b134 is 0, its `name` text takes the 0x004e5320 result (0x004943e1,
/// `esp+0x1564`) and its `detail` text the 0x004e5c10 result (0x0049442b, `esp+0x1594`) and
/// the timer starts at 8000 ms;
/// while the timer runs the banner is shown with the Display fill colour
/// `(1, 1, 1, 1 − ((t / 8000 − 0.5) · 2)²)` and the timer counts down by `dt` (clamped to
/// 0); otherwise it is hidden.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LandnameState {
    /// 0x0076b104, 0x0076b11c.
    pub last: (String, String),
    /// 0x0076b134.
    pub timer_ms: i32,
    /// The names of this frame as `(name text, detail text)`: the 0x004e5320 and
    /// 0x004e5c10 results (`None` = unknown, read as unchanged). Either differing counts as
    /// a change, so the order does not affect the comparison.
    pub region: Option<(String, String)>,
}

/// What [`LandnameState::step`] does to the banner this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct LandnameFrame {
    /// Visibility of `landname`.
    pub visible: bool,
    /// New `name` / `detail` texts (on a change).
    pub texts: Option<(String, String)>,
    /// The Display fill colour (while shown).
    pub color: Option<[f32; 4]>,
}

impl LandnameState {
    /// One frame of 0x00494355..0x004944fe with the frame time `dt`.
    pub fn step(&mut self, dt: i32) -> LandnameFrame {
        let changed = self.region.as_ref().is_some_and(|r| *r != self.last);
        let mut texts = None;
        let t = self.timer_ms;
        if t == 0 {
            if !changed {
                // 0x0049438d: `setVisible(timer)` with the timer 0.
                return LandnameFrame { visible: false, texts: None, color: None };
            }
            // 0x004943ac: the statics take the names, the texts are written, 8000 ms.
            let r = self.region.clone().unwrap_or_default();
            self.last = r.clone();
            texts = Some(r);
            self.timer_ms = 0x1f40;
        }
        // 0x00494469: alpha from the timer before the countdown.
        let x = (self.timer_ms as f32 / 8000.0f32 - 0.5f32) * 2.0f32;
        let a = 1.0f32 - x * x;
        // 0x004944e8: `timer -= dt`, negative → 0.
        let mut n = self.timer_ms.wrapping_sub(dt);
        if n < 0 {
            n = 0;
        }
        self.timer_ms = n;
        LandnameFrame { visible: true, texts, color: Some([1.0, 1.0, 1.0, a]) }
    }
}

/// Everything [`hud_bars`] reads.
pub struct HudInputs<'a> {
    /// `GC+0x8006d0 + 0x10`.
    pub player: &'a EntityData,
    /// `creature+0x1194`.
    pub stamina: f32,
    /// `creature+0x1190` (`CreatureState::block`; the cast bar shows it as "Stealth").
    pub guard: f32,
    /// `creature+0x1198` (`riding.rs`, the riding stamina).
    pub riding_stamina: f32,
    /// The player holds the haste buff (type 0xc) for `skillWindup`.
    pub haste: bool,
    /// The pet: `World::findEntity(creature+0x11c8)` (0x004911e3).
    pub pet: Option<&'a EntityData>,
    /// The world's creatures (`GC+0x2e8`, id order) other than the local player whose
    /// hostility (`entity+0x50`) is 0, in map order (0x0049198b..0x00491f16).
    pub party: &'a [&'a EntityData],
    /// `GC+0x11c`, `GC+0x120`.
    pub width: i32,
    pub height: i32,
    /// `manacubebar` animates (`Node::isAnimating` 0x006364f0).
    pub manacube_animating: bool,
    /// The creature type name (`World+0x800104` map, 0x0059aa60; empty when unknown).
    /// `None` uses [`crate::names::creature_type_key`] (the key itself: 0x0049144e streams the
    /// key of 0x0059aa60, not its dictionary `singular`).
    pub type_name: Option<&'a dyn Fn(i32) -> String>,
    /// The client world (`GC+0x2e4`) for the landscape names ([`landscape_names`]); `None`
    /// leaves the `landscape` texts and the `landname` banner's names unchanged.
    pub world: Option<&'a cw_world::World>,
    /// The dictionary (`World+0x30`, `GC+0x314`) the landscape names read.
    pub text: Option<&'a super::textdb::TextDb>,
}

/// The cast bar modes whose bar waits for the wind-up (0x00492f45..0x00492fa2).
const CAST_MODES: [u8; 23] = [
    0x69, 0x2e, 0x2d, 0x26, 0x27, 0x28, 0x25, 0x5f, 0x5e, 0x57, 0x59, 0x5c, 0x2c, 0x29, 0x2a, 0x2b, 0x1e, 0x1f, 0x20, 0x21, 0x22,
    0x58, 0x31,
];

/// The cast bar's `name` by mode (jump tables 0x0049d430 / 0x0049d3e0 on `mode - 0x1e`,
/// 0x00493230..0x004939d5); other modes write L"".
pub fn cast_name(mode: u8) -> &'static str {
    match mode {
        0x31 => "Teleport",
        0x58 => "Fire Explosion",
        0x1e => "Fire Swirl",
        0x1f => "Fire Vortex",
        0x20 => "Water Swirl",
        0x21 => "Water Vortex",
        0x26..=0x28 => "Firebolt",
        0x25 => "Fireball",
        0x2e => "Fire Salvo",
        0x2d => "Water Salvo",
        0x5f => "Fireray",
        0x5e => "Firebeam",
        0x22 => "Healing Stream",
        0x29 | 0x2a | 0x2c => "Water drop",
        0x2b => "Water splash",
        0x57 => "Explosion",
        0x59 => "Lava",
        0x5c => "Summon",
        0x69 => "Teleport to City",
        _ => "",
    }
}

/// `castbar` 0x00492f1d..0x00493b0f for the player entity; `guard` is `creature+0x1190`.
/// `bp` is the block power `entity+0x164`, `mode` `entity+0x58`, `mode_time` `entity+0x5c`.
pub fn cast_bar(e: &EntityData, guard: f32, haste: bool) -> Option<(String, f32)> {
    let bp = f32_at(e, 0x164);
    let mode = e.0[0x58];
    let mode_time = i32_at(e, 0x5c);
    let windup = || cw_sim::skills::skill_windup(e, guard, haste, -1);
    // 0x00492f38: `1.0 > bp` shows the bar at once.
    let mut show = 1.0f32 > bp;
    if !show {
        // 0x00492f45: a cast mode still winding up (`mode_time <= windup`) shows it.
        if CAST_MODES.contains(&mode) && mode_time <= windup() {
            show = true;
        } else if guard > 0.0 {
            // 0x00492fb8.
            show = true;
        } else if cw_sim::util::is_blocking(e) {
            // 0x00492fda: `ucomiss bp, 0` + `lahf; test ah, 0x44; jp`: shown unless equal.
            show = bp != 0.0;
        }
    }
    if !show {
        return None;
    }
    // 0x004930b9: the fill and the name.
    if !cw_sim::util::is_mage(e) && guard > 0.0 {
        return Some(("Stealth".to_string(), guard));
    }
    if cw_sim::util::is_blocking(e) || 1.0f32 > bp {
        // 0x00493a3c: `max(bp, 0)` (`comiss 0, bp; jbe`).
        let f = if 0.0f32 > bp { 0.0 } else { bp };
        return Some(("Block Power".to_string(), f));
    }
    // 0x004931b1: `(float)mode_time / (float)windup`.
    let f = mode_time as f32 / windup() as f32;
    Some((cast_name(mode).to_string(), f))
}

/// The combo counter (0x00493b0f..0x00493d72); updates `state.last_hits`.
pub fn combo_points(state: &mut HudState, e: &EntityData) -> Option<ComboFrame> {
    let hits = i32_at(e, 0x60);
    let out = if hits != 0 && hits != i32::from(state.last_hits) {
        let mut text = format!("     {hits}{}", if hits == 1 { " HIT" } else { " HITS" });
        // 0x00493ba0: `hits >= maxBlock` appends "!" and turns the text cyan.
        let maxed = hits >= cw_sim::skills::max_block(e);
        if maxed {
            text.push('!');
        }
        let color = if maxed { [0.0, 1.0, 1.0, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
        Some(ComboFrame { text, color, play_hit: true })
    } else {
        None
    };
    // 0x00493d64: the static keeps the low byte.
    state.last_hits = hits as u8;
    out
}

fn life_frame(e: &EntityData, position: [f32; 2], class: String) -> LifeFrame {
    LifeFrame { position, text: life_text(e), info: name_of(e), class, fill: life_fill(e) }
}

/// `entity+0x1158` (16 bytes, NUL-terminated, narrow; widened byte-wise by 0x006089c0).
pub fn name_of(e: &EntityData) -> String {
    let raw = &e.0[0x1158..0x1168];
    let n = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    raw[..n].iter().map(|&b| char::from(b)).collect()
}

/// The HUD bars of one frame, in `update` order (0x00490cf5..0x00493d72), followed by the
/// landscape names of 0x00494161..0x0049434d ([`landscape_names`]).
pub fn hud_bars(state: &mut HudState, inp: &HudInputs) -> HudBars {
    let bars = bars_only(state, inp);
    landscape_names(state, inp);
    state.info = inp.world.map(|world| {
        // 0x00494595..0x00494642: the block as for the landscape names; climateA, climateB,
        // then the time.
        let x = (i64_at(inp.player, 0) / 0x10000) as i32;
        let y = (i64_at(inp.player, 8) / 0x10000) as i32;
        let humidity = world.climate_a(x, y);
        let temperature = world.climate_b(x, y);
        info_text(humidity, temperature, world.time_of_day)
    });
    bars
}

/// `update` 0x004941e1..0x0049426c: 0x004e5c10 (the zone's site name,
/// [`super::names::zone_site_name`], into `esp+0x1594`) and 0x004e5320 (the cell or region
/// name, [`super::names::area_name`], into `esp+0x1564`) (`ecx` = `GC+0x314`, the world
/// `GC+0x2e4`) at the player's block `(__alldiv(pos.x, 0x10000), __alldiv(pos.y,
/// 0x10000))` (0x004120b0 / 0x0042f570: truncating 64-bit division, low dword), in that
/// order. The area name is the big `landscape` line and the banner's `name`, the site name
/// the small `landscapedetail` line and the banner's `detail` ([`HudState::landscape`],
/// [`LandnameState::region`]). No `rand()` draw.
pub fn landscape_names(state: &mut HudState, inp: &HudInputs) {
    let Some(world) = inp.world else {
        state.landscape = None;
        return;
    };
    let x = (i64_at(inp.player, 0) / 0x10000) as i32;
    let y = (i64_at(inp.player, 8) / 0x10000) as i32;
    let site = super::names::zone_site_name(inp.text, world, x, y);
    let area = super::names::area_name(inp.text, world, x, y);
    state.landname.region = Some((area.clone(), site.clone()));
    state.landscape = Some((area, site));
}

/// One of the four TextShapes of the `landscape` banner (ctor 0x0045adae..0x0045b55a, each
/// `Engine::createTextShape(L"Hyara Planes", L"")` 0x006502e0 → `TextShape` ctor 0x00663240
/// with its defaults otherwise): fill colour (`+0xb4` key) white, pixel size `+0x1bc`
/// `size`, line spacing `+0x1c8` 3, font `+0x1cc` `resource1.dat` (0x00468000 with
/// 0x006fcd24), flags `+0x1ec` 1 (centred; written directly or through 0x00487e80), then
/// `rebuild(1)` (slot 1). The outline shapes (`stroke`) also take the stroke colour
/// (`+0x10c` key) (0, 0, 0, 1) and the stroke radius `+0x1c0` 3; the others keep the ctor's
/// radius 0.
pub fn banner_text_shape(size: f32, stroke: bool) -> cw_ui::loader::SharedShape {
    hud_text_shape(BANNER_TEXT, size, if stroke { 3.0 } else { 0.0 }, 3.0, cw_ui::font::align::H_CENTER)
}

/// One of the two TextShapes of the `info` node (ctor 0x0045bd3f..0x0045bfca, each
/// `createTextShape(L"", L"")` 0x006502e0): fill white (`+0xb4`), pixel size `+0x1bc` 12,
/// font `resource1.dat`, flags `+0x1ec` 2 (right-aligned, 0x00487e80(2)), line spacing left
/// at the ctor's 0, `rebuild(1)`. The outline shape (`stroke`, +0x800860's own) also takes
/// the stroke colour (0, 0, 0, 1) (`+0x10c`) and the stroke radius `+0x1c0` 2; the fill
/// shape (+0x800864, on the child `info`) keeps radius 0.
pub fn info_text_shape(stroke: bool) -> cw_ui::loader::SharedShape {
    hud_text_shape("", 12.0, if stroke { 2.0 } else { 0.0 }, 0.0, cw_ui::font::align::RIGHT)
}

/// A TextShape the GameController ctor builds at runtime (0x006502e0 → the `TextShape`
/// ctor 0x00663240, then the fields the ctor writes): white fill, black stroke colour,
/// `resource1.dat`, the given text, pixel size, stroke radius, line spacing and flags.
pub fn hud_text_shape(text: &str, size: f32, stroke_radius: f32, line_spacing: f32, flags: u32) -> cw_ui::loader::SharedShape {
    use cw_ui::font::{TextShape, TextShapeSource};
    use cw_ui::loader::{SceneShape, TextShapeNode};
    use cw_ui::shape::Keyed;
    let text: Vec<u16> = text.encode_utf16().collect();
    let src = TextShapeSource {
        strings: vec![text.clone()],
        size,
        stroke_radius,
        line_spacing,
        flags,
        font_file: "resource1.dat".into(),
        ..TextShapeSource::default()
    };
    let mut t = TextShapeNode {
        shape: TextShape::new(src),
        string: Keyed::from_frames(vec![text]),
        color: Keyed::from_frames(vec![[1.0; 4]]),
        stroke_color: Keyed::from_frames(vec![[0.0, 0.0, 0.0, 1.0]]),
        extrusion_color: Keyed::from_frames(vec![[0.0, 0.0, 0.0, 1.0]]),
        active: false,
    };
    t.sync();
    std::rc::Rc::new(std::cell::RefCell::new(SceneShape::Text(t)))
}

/// `update` 0x00494554..0x00494885: the `info` text, written with
/// `setChildText(L"info", text, 1)` 0x00636a00 on +0x800860 (0x0049483f..0x00494885). A
/// `std::wostringstream` (reset with `str(L"")`, 0x00411b90) receives, in this order:
/// `L"TIME "` (0x00701bac), `time / 3600000` (hours; `imul 0x4a90be59; sar 20`), `L":"`
/// (0x00701ba8), `setw(2)` (0x006fc378 via 0x004513f0) and `setfill(L'0')` (0x00458b90 via
/// 0x00451420) then `(time / 60000) % 60` (minutes; `imul 0x45e7b273; sar 14`, `idiv 60`),
/// `L"  TEMP "` (0x00701b98), `-20 - cvttss2si(climateB · -60)`, `L" °C  HUM "` (0x00701b84,
/// U+00B0 in the wide literal), `cvttss2si(climateA · 100)`, `L"%"` (0x006fd728). All
/// divisions are signed and truncate. `time` is `World+0x80015c` (0x00471910, the time of
/// day in ms), `humidity` climateA 0x005c4800 and `temperature` climateB 0x005c4dd0 at the
/// player's block. No `rand()` draw.
pub fn info_text(humidity: f32, temperature: f32, time_ms: i32) -> String {
    let hours = time_ms / 3_600_000;
    let minutes = (time_ms / 60_000) % 60;
    let temp = -20i32 - ((temperature * -60.0f32) as i32);
    let hum = (humidity * 100.0f32) as i32;
    // `setw(2)` + `setfill('0')`: right-aligned (the default adjustfield), padded on the left.
    let mut mm = minutes.to_string();
    while mm.len() < 2 {
        mm.insert(0, '0');
    }
    format!("TIME {hours}:{mm}  TEMP {temp} \u{b0}C  HUM {hum}%")
}

/// The ctor's text of the four banner shapes (0x006ffe24).
pub const BANNER_TEXT: &str = "Hyara Planes";

/// The `star1`..`star4` clones under `landscape` (ctor 0x0045c389..0x0045c593) that
/// `update` hides every frame (0x00494722..0x0049483a: `findNode(name)` 0x00633d70 on
/// +0x800858, `setVisible(0)` 0x00411a90). Nothing in Cube.exe shows them (the other
/// references to these names, 0x004d50de..0x004d5795, are `PreviewWidget::update` on the
/// item preview's own stars).
pub const LANDSCAPE_STARS: [&str; 4] = ["star1", "star2", "star3", "star4"];

fn bars_only(state: &mut HudState, inp: &HudInputs) -> HudBars {
    let p = inp.player;
    let (w, h) = (inp.width, inp.height);
    let half_w = w / 2;

    // 0x00490d70: `cubes % 4` (signed remainder), 4 while the node animates.
    let mut shown = i32_at(p, 0x1154) % 4;
    if inp.manacube_animating {
        shown = 4;
    }
    let manacube = ManacubeFrame { position: [100.0, 130.0], shown };

    let player = life_frame(p, [100.0, 30.0], level_class_text(p));

    // 0x004911d1: the pet.
    let pet = inp.pet.map(|pe| {
        // 0x00491437: `LVL n` + L" " + the type name (0x0059aa60 on `entity+0x54`).
        let t = i32_at(pe, 0x54);
        let name = match inp.type_name {
            Some(f) => f(t),
            None => crate::names::creature_type_key(t).to_string(),
        };
        let class = format!("LVL {} {}", i32_at(pe, 0x180), name);
        let xp = i32_at(pe, 0x184);
        let need = xp_needed(i32_at(pe, 0x180));
        // 0x0049171f: `r > 1` → 1 (NaN keeps r).
        let r = xp as f32 / need as f32;
        let xp_fill = if r > 1.0f32 { 1.0 } else { r };
        // 0x00491817: `min((int)(stamina * 5 + 0.9999), 5)`.
        let mut cubes = (inp.riding_stamina * 5.0f32 + 0.9999f32) as i32;
        if cubes > 5 {
            cubes = 5;
        }
        PetFrame { life: life_frame(pe, [100.0, 180.0], class), xp_text: format!("{xp}/{need}"), xp_fill, riding_cubes: cubes }
    });

    // 0x00491902: the party slots, x from 310 in steps of 220.
    let mut party: [Option<LifeFrame>; 3] = Default::default();
    let mut x = 0x136;
    for (slot, e) in party.iter_mut().zip(inp.party.iter()) {
        *slot = Some(life_frame(e, [x as f32, 30.0], level_class_text(e)));
        x += 0xdc;
    }

    let mp = MpFrame {
        position: [(half_w + 8) as f32, (h - 0x5a) as f32],
        text: format!("{}/100", (f32_at(p, 0x160) * 100.0f32 + 0.5f32) as i32),
        fill: f32_at(p, 0x160),
        charge_fill: f32_at(p, 0x134),
    };
    let hp_position = [(half_w - 0xa3) as f32, (h - 0x5a) as f32];
    let stamina = (1.0f32 > inp.stamina).then(|| StaminaFrame { position: [half_w as f32, (h - 0x73) as f32], fill: inp.stamina });
    let cast = cast_bar(p, inp.guard, inp.haste).map(|(name, fill)| CastFrame { position: [half_w as f32, (h - 0x96) as f32], name, fill });
    let combo = combo_points(state, p);
    HudBars { player, manacube, pet, party, mp, hp_position, stamina, cast, combo }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wi(e: &mut EntityData, o: usize, v: i32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn wf(e: &mut EntityData, o: usize, v: f32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn xp_curve_and_texts() {
        assert_eq!(xp_needed(1), 50);
        // Level 2: (1 - 1/1.05) * 1000 + 50 = 97.6 -> 97.
        assert_eq!(xp_needed(2), 97);
        let mut e = EntityData::new_creature();
        wi(&mut e, 0x180, 2);
        wi(&mut e, 0x184, 40);
        wf(&mut e, 0x160, 0.456);
        let h = hud_frame(&e, 1.0);
        assert_eq!(h.xp_text, "40/97");
        assert_eq!(h.mp_text, "46/100");
        assert!(!h.stamina_visible);
        assert!(!hud_frame(&e, f32::NAN).stamina_visible);
        assert!(hud_frame(&e, 0.5).stamina_visible);
    }

    #[test]
    fn class_and_names() {
        let mut e = EntityData::new_creature();
        wi(&mut e, 0x180, 7);
        e.0[0x130] = 3;
        assert_eq!(level_class_text(&e), "LVL 7 Mage");
        e.0[0x130] = 9;
        assert_eq!(level_class_text(&e), "LVL 7");
        e.0[0x1158..0x115c].copy_from_slice(b"Bob\0");
        assert_eq!(name_of(&e), "Bob");
    }

    #[test]
    fn cast_bar_rules() {
        let mut e = EntityData::new_creature();
        // Full block power, idle, no guard: hidden.
        wf(&mut e, 0x164, 1.0);
        e.0[0x58] = 0;
        assert_eq!(cast_bar(&e, 0.0, false), None);
        // Block power recharging: "Block Power" with max(bp, 0).
        wf(&mut e, 0x164, -0.5);
        assert_eq!(cast_bar(&e, 0.0, false), Some(("Block Power".into(), 0.0)));
        wf(&mut e, 0x164, 0.25);
        assert_eq!(cast_bar(&e, 0.0, false), Some(("Block Power".into(), 0.25)));
        // Guard on a non-caster: "Stealth" with the guard as fill.
        wf(&mut e, 0x164, 1.0);
        assert_eq!(cast_bar(&e, 0.5, false), Some(("Stealth".into(), 0.5)));
        // Fireball winding up: mode_time / windup.
        e.0[0x58] = 0x25;
        wi(&mut e, 0x5c, 0);
        let (n, f) = cast_bar(&e, 0.0, false).unwrap();
        assert_eq!(n, "Fireball");
        assert_eq!(f, 0.0);
        assert_eq!(cast_name(0x27), "Firebolt");
        assert_eq!(cast_name(0x30), "");
    }

    #[test]
    fn combo_changes_only() {
        let mut s = HudState::default();
        let mut e = EntityData::new_creature();
        assert_eq!(combo_points(&mut s, &e), None);
        wi(&mut e, 0x60, 1);
        let c = combo_points(&mut s, &e).unwrap();
        assert_eq!(c.text, "     1 HIT");
        assert!(c.play_hit);
        assert_eq!(combo_points(&mut s, &e), None);
        wi(&mut e, 0x60, 3);
        assert_eq!(combo_points(&mut s, &e).unwrap().text, "     3 HITS");
        // 256 hits truncate to the byte 0 in the static and still differ from it.
        wi(&mut e, 0x60, 256);
        assert!(combo_points(&mut s, &e).is_some());
        assert_eq!(s.last_hits, 0);
    }

    #[test]
    fn landname_banner() {
        let mut s = LandnameState::default();
        assert!(!s.step(16).visible);
        s.region = Some(("Alura".into(), "Plains".into()));
        let f = s.step(16);
        assert!(f.visible);
        assert_eq!(f.texts, Some(("Alura".into(), "Plains".into())));
        // t = 8000: x = 1, alpha 0; then 7984 left.
        assert_eq!(f.color, Some([1.0, 1.0, 1.0, 0.0]));
        assert_eq!(s.timer_ms, 8000 - 16);
        s.timer_ms = 4000;
        let f = s.step(5000);
        assert_eq!(f.color, Some([1.0, 1.0, 1.0, 1.0]));
        assert_eq!(f.texts, None);
        assert_eq!(s.timer_ms, 0);
        assert!(!s.step(16).visible);
    }

    /// 0x004941e1..0x0049426c: the names of the player's block feed both the `landscape`
    /// texts and the `landname` banner; without a world nothing is written.
    #[test]
    fn landscape_names_from_world() {
        let xml = r#"<root>
<landscape key="Lands of"><normal>Lands of @</normal></landscape>
<landscape key="Ocean"><normal>@ Ocean</normal></landscape>
</root>"#;
        let db = crate::ui::textdb::TextDb::from_xml(xml.as_bytes()).unwrap();
        let mut world = cw_world::World::new(1234);
        for rx in 0..4 {
            for ry in 0..4 {
                world.get_or_create_climate_point(rx, ry);
            }
        }
        let mut e = EntityData::new_creature();
        // Block (0x6000 + 0.5, 0x5000) in 16.16 fixed.
        e.0[0..8].copy_from_slice(&((0x6000i64 << 16) + 0x8000).to_le_bytes());
        e.0[8..16].copy_from_slice(&(0x5000i64 << 16).to_le_bytes());
        let party: [&EntityData; 0] = [];
        let mut inp = HudInputs {
            player: &e,
            stamina: 1.0,
            guard: 0.0,
            riding_stamina: 0.0,
            haste: false,
            pet: None,
            party: &party,
            width: 800,
            height: 600,
            manacube_animating: false,
            type_name: None,
            world: None,
            text: Some(&db),
        };
        let mut s = HudState::default();
        hud_bars(&mut s, &inp);
        assert_eq!(s.landscape, None);
        assert_eq!(s.info, None);
        assert_eq!(s.landname.region, None);
        inp.world = Some(&world);
        hud_bars(&mut s, &inp);
        let p = *world.nearest_climate_point(0x6000, 0x5000).unwrap();
        let detail = if p.elevation < 0 {
            format!("{} Ocean", super::super::names::generate_name(p.seed as u32, -1))
        } else {
            format!("Lands of {}", super::super::names::generate_name(p.seed as u32, -1))
        };
        // No region loaded: no zone record, no cell.
        // The area name is the big line / banner `name`, the (empty) site name the small one.
        assert_eq!(s.landscape, Some((detail.clone(), String::new())));
        assert_eq!(s.landname.region, Some((detail, String::new())));
        // 0x00494554..: the `info` text of the same block and the world's time of day.
        let info = info_text(world.climate_a(0x6000, 0x5000), world.climate_b(0x6000, 0x5000), world.time_of_day);
        assert_eq!(s.info, Some(info));
    }

    /// 0x00494554..0x00494713: the `info` stream's pieces and truncations.
    #[test]
    fn info_text_format() {
        // 13:05:59.999; humidity 0.456 -> 45; temperature 0.75 -> -20 - (-45) = 25.
        let t = 13 * 3_600_000 + 5 * 60_000 + 59_999;
        assert_eq!(info_text(0.456, 0.75, t), "TIME 13:05  TEMP 25 \u{b0}C  HUM 45%");
        // Midnight, the 0.5 fallbacks of climateA/B: 50%, -20 - (-30) = 10.
        assert_eq!(info_text(0.5, 0.5, 0), "TIME 0:00  TEMP 10 \u{b0}C  HUM 50%");
        // `cvttss2si` truncates toward zero: 0.1 * -60 = -6.0000...; 0.999 * 100 = 99.9 -> 99.
        assert_eq!(info_text(0.999, 0.01, 23 * 3_600_000 + 59 * 60_000), "TIME 23:59  TEMP -20 \u{b0}C  HUM 99%");
    }

    #[test]
    fn bars_layout() {
        let mut e = EntityData::new_creature();
        wi(&mut e, 0x1154, 7);
        let mut pet = EntityData::new_creature();
        wi(&mut pet, 0x180, 3);
        let name = |_t: i32| "Wolf".to_string();
        let party_a = EntityData::new_creature();
        let party = [&party_a, &party_a];
        let inp = HudInputs {
            player: &e,
            stamina: 0.5,
            guard: 0.0,
            riding_stamina: 0.41,
            haste: false,
            pet: Some(&pet),
            party: &party,
            width: 801,
            height: 600,
            manacube_animating: false,
            type_name: Some(&name),
            world: None,
            text: None,
        };
        let mut s = HudState::default();
        let b = hud_bars(&mut s, &inp);
        assert_eq!(b.manacube.shown, 3);
        assert_eq!(b.mp.position, [408.0, 510.0]);
        assert_eq!(b.hp_position, [237.0, 510.0]);
        assert_eq!(b.stamina.as_ref().unwrap().position, [400.0, 485.0]);
        let p = b.pet.unwrap();
        assert_eq!(p.life.class, "LVL 3 Wolf");
        // 0.41 * 5 + 0.9999 = 3.0499 -> 3.
        assert_eq!(p.riding_cubes, 3);
        assert_eq!(b.party[0].as_ref().unwrap().position, [310.0, 30.0]);
        assert_eq!(b.party[1].as_ref().unwrap().position, [530.0, 30.0]);
        assert!(b.party[2].is_none());
    }
}
