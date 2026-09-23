//! The in-world target UI of `GameController::update` 0x00488ee0: the creature nameplates
//! (the "target frame": clones of the `*lifebar:small` templates in the container node
//! `GC+0x8007b0`), the crosshairs and their prompt text (the death "[R] Revive"), and the
//! quest tags (`questtag` +0x800868, `smallquesttag` +0x80086c, `questbar` +0x8007dc).
//!
//! Tier B. What is drawn is data here ([`PlateFrame`], [`CrosshairFrame`],
//! [`QuestTagFrame`]); the lead applies it to the nodes.
//!
//! # Map
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | 0x00490920..0x00490a31 | candidate list: every creature of the world map (`GC+0x2e8`, id order) alive (`0 < hp`) within 60 blocks of the player (`3600 >= d²`, render position `+0x1350` vs position `+0x10`) as `{creature, d², flag 0}` (12-byte entries, vector 0x004c1100), `std::sort` by `d²` ascending (0x00458ba0, insertion step 0x00454e30) | [`plate_candidates`] |
//! | 0x00498690..0x00498cc6 | the aim pass sets the entry flag (`+8`) when the creature's screen box covers the crosshair (and picks `GC+0x800a70`); not ported here (input `under_crosshair`) | — |
//! | 0x00499e9b..0x0049b18d | per entry in reverse (far to near): skip rules, line of sight, projection, template choice, texts and colours | [`nameplates`] |
//! | 0x004927da..0x00492977 | `crosshair` / `zoomcrosshair` visibility and position | [`crosshair`] |
//! | 0x00496890..0x00496922 | the crosshair text: L"" then "[R] Revive" when the player is dead | [`crosshair`] |
//! | 0x00496922..0x00496f37 | the interaction prompt that replaces it ("[R] Talk", "[R] Open", ...) | [`crate::prompts::interaction_prompt`], chosen by `GameUi::frame` |
//! | 0x00490690..0x004907dc | a server mission event of kind 2: `questtag` state "shine", its `objective` text, sound 0x1e | [`quest_complete_tag`] |
//! | 0x00490a79..0x00490a93 | `questtag` visible exactly while it animates | [`questtag_visible`] |
//! | 0x0048d054..0x0048d51c | `smallquesttag` / `questbar` for the mission cell under the player | [`small_quest_tag`] |
//!
//! The large `enemylifebar` .. `neutrallifebar` templates (+0x800790..+0x80079c) and the
//! `lockedenemy` / `lockedfriend` nodes (+0x8008b4/+0x8008b8) are only found and hidden by
//! the constructor; nothing in Cube.exe references them afterwards (displacement scan of
//! `.text`), so they stay hidden.
//!
//! # Death
//!
//! There is no death screen widget: while the local player's HP is `<= 0` the crosshair
//! prompt reads "[R] Revive" (0x004968d0), the target lists keep running, and R (the DirectInput
//! byte `GC+0xf`, 0x004974.. `update` Ghidra line 5211) respawns (0x00447110, 0x005a03d0,
//! position, `hp = maxHp`); that respawn is gameplay (`crate::player`), not UI.

use cw_net::EntityData;

use super::hud::{life_fill, life_text, name_of};
use crate::player::Mat4;

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i64_at(e: &EntityData, o: usize) -> i64 {
    i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap())
}

fn u16_at(e: &EntityData, o: usize) -> u16 {
    u16::from_le_bytes(e.0[o..o + 2].try_into().unwrap())
}

/// `vec3f::fromFixed` 0x0042c4a0: `(float)v * 1.5258789e-05` per axis.
fn from_fixed(v: [i64; 3]) -> [f32; 3] {
    let k = 1.525_878_9e-5f32;
    [(v[0] as f32) * k, (v[1] as f32) * k, (v[2] as f32) * k]
}

/// `vec3i64::fromFloat` 0x0042c460: `(i64)(f * 65536)` (truncating).
fn to_fixed(v: [f32; 3]) -> [i64; 3] {
    [(v[0] * 65536.0f32) as i64, (v[1] * 65536.0f32) as i64, (v[2] * 65536.0f32) as i64]
}

fn pos_of(e: &EntityData) -> [i64; 3] {
    [i64_at(e, 0), i64_at(e, 8), i64_at(e, 0x10)]
}

// ---------------------------------------------------------------------------------------
// Nameplates
// ---------------------------------------------------------------------------------------

/// Which `:small` template a nameplate clones (`GC+0x8007a0..+0x8007ac`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateKind {
    /// +0x8007a0 `enemylifebar:small`: a monster (hostility 1) of a non-peaceful type.
    Enemy,
    /// +0x8007a4 `friendlifebar:small`: players, NPCs, pets (hostility 0, 3, 5, ...).
    Friend,
    /// +0x8007a8 `staticlifebar:small`: hostility 6.
    Static,
    /// +0x8007ac `neutrallifebar:small`: a monster of a peaceful type (`isFriendlyType`
    /// 0x00444680).
    Neutral,
}

impl PlateKind {
    /// Index into `GcMembers::target_bars` (the `:small` half).
    pub fn member_index(self) -> usize {
        match self {
            PlateKind::Enemy => 4,
            PlateKind::Friend => 5,
            PlateKind::Static => 6,
            PlateKind::Neutral => 7,
        }
    }
}

/// 0x0049a336 / 0x0049a3c3: the template by hostility (`entity+0x50`).
pub fn plate_kind(e: &EntityData) -> PlateKind {
    match e.0[0x50] {
        1 => {
            if cw_sim::combat::friendly_type(e) {
                PlateKind::Neutral
            } else {
                PlateKind::Enemy
            }
        }
        6 => PlateKind::Static,
        _ => PlateKind::Friend,
    }
}

/// One entry of the candidate vector (12 bytes in the original).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateCandidate {
    /// `+0`: the creature id (the original holds the `Creature*`).
    pub id: i64,
    /// `+4`: the squared distance in blocks (render position vs the player's position).
    pub dist_sq: f32,
    /// `+8`: under the crosshair (set by the aim pass 0x00498bf8).
    pub under_crosshair: bool,
}

/// A creature the nameplates read.
#[derive(Clone, Copy, Debug)]
pub struct PlateCreature<'a> {
    /// World map key.
    pub id: i64,
    /// `creature+0x10`.
    pub entity: &'a EntityData,
    /// `creature+0x1350`: the smoothed render position.
    pub render_pos: [i64; 3],
    /// `creature+0x1180`: the step offset (`CreatureState::step_offset`).
    pub step_offset: f32,
}

/// 0x00490920..0x00490a31: the candidates, sorted by distance (ascending; the original's
/// `std::sort` is not stable, ties keep map order here). `creatures` in world-map (id)
/// order; `under_crosshair(id)` is the aim pass's flag.
pub fn plate_candidates(creatures: &[PlateCreature], player_pos: [i64; 3], under_crosshair: &dyn Fn(i64) -> bool) -> Vec<PlateCandidate> {
    let mut v = Vec::new();
    for c in creatures {
        // 0x0049093a: `0 >= hp` skips (`comiss 0, hp; jae`).
        if 0.0f32 >= f32_at(c.entity, 0x15c) {
            continue;
        }
        let d = from_fixed([c.render_pos[0] - player_pos[0], c.render_pos[1] - player_pos[1], c.render_pos[2] - player_pos[2]]);
        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        // 0x00490989: `3600 < d²` (or NaN) skips.
        if !(3600.0f32 >= d2) {
            continue;
        }
        v.push(PlateCandidate { id: c.id, dist_sq: d2, under_crosshair: under_crosshair(c.id) });
    }
    v.sort_by(|a, b| a.dist_sq.partial_cmp(&b.dist_sq).unwrap_or(std::cmp::Ordering::Equal));
    v
}

/// One nameplate clone in `GC+0x8007b0` (the container is emptied first, 0x00632870).
#[derive(Clone, Debug, PartialEq)]
pub struct PlateFrame {
    /// The creature.
    pub id: i64,
    /// Template cloned into the container (0x00636040).
    pub kind: PlateKind,
    /// Widget position: the projected point, `y` at least 50.
    pub position: [f32; 2],
    /// The `rarename` children: `Some(false)` hidden (flag 0 with the V toggle off), `Some(true)`
    /// shown (flag 0, V on), `None` left as the template has them (flag set).
    pub rare_name_visible: Option<bool>,
    /// `rarename` text: the entity name, else a generated name (0x005a0ed0) for flag 0x200 or
    /// hostility 3, else empty.
    pub rare_name: String,
    /// The colour of every `info` node's shape (`shape+0xb4`), `None` = left unchanged.
    pub info_color: Option<[f32; 4]>,
    /// `info` text: the type's `singular` text, then `" +" << power << endl` when the power
    /// byte `entity+0x198` is set.
    pub info: String,
    /// `text`: `hp/maxHp`.
    pub text: String,
    /// `bar` x scale `hp / maxHp`.
    pub bar: f32,
    /// `bar2` x scale `1 - hp / maxHp`.
    pub bar2: f32,
}

/// The camera and world queries the nameplates need.
pub struct PlateInputs<'a> {
    /// The local player's creature id and entity.
    pub player_id: i64,
    pub player: &'a EntityData,
    /// `GC+0x8007b4` (V): nameplates for every creature, with the rare names.
    pub show_all: bool,
    /// `GC+0x1d8`, `GC+0x1e0`: the render origin (`Camera::origin`).
    pub origin: [i64; 2],
    /// `GC+0x26c`: the view relative to the render origin (`Camera::view_shake`).
    pub view: &'a Mat4,
    /// `GC+0x800a90`: the projection.
    pub projection: &'a Mat4,
    /// Engine viewport width / height (`Engine+0x10c` / `+0x110`, 0x004279f0 / 0x004279e0).
    pub width: i32,
    pub height: i32,
    /// `[esp+0xe4]` of `update`: the light scale the visibility range multiplies with (its
    /// writer is earlier in `update`, not traced).
    pub light_scale: f32,
    /// `World::groundLightAt` 0x004718b0 at a block (0..255).
    pub ground_light: &'a dyn Fn(i32, i32, i32) -> i32,
    /// `World::lineOfSight` 0x0059ee90 from `GC+0x140` (the listener/eye) to `to` (fixed),
    /// flag 1, over at most `max` blocks.
    pub line_of_sight: &'a dyn Fn([i64; 3], f32) -> bool,
    /// `World::findEntity` 0x0042f000: the mode (`entity+0x58`) of a creature by id.
    pub mode_of: &'a dyn Fn(i64) -> Option<u8>,
    /// The `singular` text of a creature type (`textdb[typeName(type)]["singular"]`,
    /// 0x00480e00 over 0x0059aa60).
    /// `None` uses [`crate::names::creature_singular`] over the `text` argument of
    /// [`nameplates`].
    pub singular: Option<&'a dyn Fn(i32) -> String>,
}

/// The `info` colour of a nameplate (0x0049aadd..0x0049ad13): players (0, 0.8, 1, 1), statics
/// (0.8, 0.8, 0.8, 1); others by `levelCurve` 0x0043ca60 of their level against the player's:
/// more than +0.3 red (1, 0.2, 0.2, 1), more than +0.1 orange (1, 0.6, 0.2, 1), more than −0.1
/// cyan (0, 1, 1, 1), else unchanged.
pub fn plate_color(e: &EntityData, player: &EntityData) -> Option<[f32; 4]> {
    use cw_world::generate::level_curve;
    match e.0[0x50] {
        0 => return Some([0.0, 0.8, 1.0, 1.0]),
        6 => return Some([0.8, 0.8, 0.8, 1.0]),
        _ => {}
    }
    let lc = || level_curve(i32_at(e, 0x180) as f32);
    let lp = || level_curve(i32_at(player, 0x180) as f32);
    if lc() > lp() + 0.3f32 {
        Some([1.0, 0.2, 0.2, 1.0])
    } else if lc() > lp() + 0.1f32 {
        Some([1.0, 0.6, 0.2, 1.0])
    } else if lc() > lp() - 0.1f32 {
        Some([0.0, 1.0, 1.0, 1.0])
    } else {
        None
    }
}

/// The `rarename` text (0x0049a881..0x0049aa60).
pub fn rare_name(id: i64, e: &EntityData) -> String {
    let name = name_of(e);
    if !name.is_empty() {
        return name;
    }
    // Flag 0x200 of `entity+0x6e`, or hostility 3: the generated name of (id low dword,
    // entity type).
    if u16_at(e, 0x6e) & 0x200 != 0 || e.0[0x50] == 3 {
        return super::names::generate_name(id as u32, i32_at(e, 0x54));
    }
    String::new()
}

/// The visibility range of 0x00499f79..0x00499fc4: `f = light / 255 * scale`; 200 blocks, or
/// `f * 0.9 / 0.3 * 200 + 20` when `0.3 > f`.
pub fn plate_range(light: i32, scale: f32) -> f32 {
    let f = light as f32 / 255.0f32 * scale;
    if 0.3f32 > f {
        f * 0.9f32 / 0.3f32 * 200.0f32 + 20.0f32
    } else {
        200.0
    }
}

/// The nameplates of one frame (0x00499e9b..0x0049b18d): `candidates` from
/// [`plate_candidates`], walked from the farthest; `creature(id)` looks a creature up; `text`
/// is the World's dictionary (the default `singular`, 0x0049ae04..0x0049ae28).
pub fn nameplates<'a>(
    inp: &PlateInputs,
    candidates: &[PlateCandidate],
    creature: &dyn Fn(i64) -> Option<PlateCreature<'a>>,
    text: Option<&super::textdb::TextDb>,
) -> Vec<PlateFrame> {
    let mut out = Vec::new();
    let player = inp.player;
    for cand in candidates.iter().rev() {
        let Some(c) = creature(cand.id) else { continue };
        let e = c.entity;
        // 0x00499f02: never the player; not the mount of a riding player (0x6a with
        // `parent == player id`).
        if c.id == inp.player_id {
            continue;
        }
        if player.0[0x58] == 0x6a && i64_at(e, 0x188) == inp.player_id {
            continue;
        }
        // 0x00499f34: the range from the light at the creature's block.
        let p = pos_of(e);
        let light = (inp.ground_light)((p[0] / 0x10000) as i32, (p[1] / 0x10000) as i32, (p[2] / 0x10000) as i32);
        let range = plate_range(light, inp.light_scale);
        // 0x00499fca: the head point: render position + (0, 0, height * 0.5 - step offset).
        let off = to_fixed([0.0, 0.0, f32_at(e, 0x78) * 0.5f32 - c.step_offset]);
        let head = [c.render_pos[0] + off[0], c.render_pos[1] + off[1], c.render_pos[2] + off[2]];
        if !(inp.line_of_sight)(head, range) {
            continue;
        }
        // 0x0049a06c: through the view relative to the render origin.
        let rel = [head[0] + inp.origin[0], head[1] + inp.origin[1], head[2]];
        let mut v = inp.view.transform_point(from_fixed(rel));
        // 0x0049a149: riding / mounted creatures (modes 0x6b, 0x6a) get +2 on the view z.
        if matches!(e.0[0x58], 0x6b | 0x6a) {
            v[2] += 2.0;
        }
        // 0x0049a174: skip a creature whose parent is riding.
        let parent = i64_at(e, 0x188);
        if parent != 0 && (inp.mode_of)(parent) == Some(0x6a) {
            continue;
        }
        // 0x0049a1af: behind the camera.
        if !(v[2] > 0.0f32) {
            continue;
        }
        let ndc = inp.projection.transform_point(v);
        if !cand.under_crosshair {
            // 0x0049a1d5: `-1 > x`, `x > 1` (same for y) skip; NaN passes.
            if -1.0f32 > ndc[0] || ndc[0] > 1.0f32 || -1.0f32 > ndc[1] || ndc[1] > 1.0f32 {
                continue;
            }
        }
        // 0x0049a235: `ndc * (w, -h, 1) * 0.5 + (w, h, 0) * 0.5`, y at least 50.
        let (w, h) = (inp.width as f32, inp.height as f32);
        let x = ndc[0] * (w * 0.5f32) + w * 0.5f32;
        let mut y = ndc[1] * (-h * 0.5f32) + h * 0.5f32;
        if 50.0f32 > y {
            y = 50.0;
        }
        let rare_name_visible = if cand.under_crosshair {
            None
        } else {
            // 0x0049a3a0: without V only hostility 0 creatures get a plate.
            if !inp.show_all && e.0[0x50] != 0 {
                continue;
            }
            Some(inp.show_all)
        };
        let t = i32_at(e, 0x54);
        let mut info = match inp.singular {
            Some(f) => f(t),
            None => crate::names::creature_singular(text, t),
        };
        let power = e.0[0x198];
        if power != 0 {
            info.push_str(&format!(" +{power}\n"));
        }
        let fill = life_fill(e);
        out.push(PlateFrame {
            id: c.id,
            kind: plate_kind(e),
            position: [x, y],
            rare_name_visible,
            rare_name: rare_name(c.id, e),
            info_color: plate_color(e, player),
            info,
            text: life_text(e),
            bar: fill,
            bar2: 1.0f32 - fill,
        });
    }
    out
}

// ---------------------------------------------------------------------------------------
// Crosshairs
// ---------------------------------------------------------------------------------------

/// `crosshair` +0x8008ac and `zoomcrosshair` +0x8008b0.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrosshairFrame {
    /// `crosshair` shown: cursor not free, player flag 0x400 (`entity+0x114`) clear, LShift
    /// (`GC+0x14`) up.
    pub crosshair: bool,
    /// `zoomcrosshair` shown: cursor not free and flag 0x400 set.
    pub zoom: bool,
    /// Both at the screen centre `(w / 2, h / 2)`.
    pub position: [f32; 2],
    /// The crosshair text (0x00636ad0): empty, "[R] Revive" while dead, or the interaction
    /// prompt written afterwards (0x00496922..0x00496f37, [`crate::prompts`]).
    pub text: String,
}

/// 0x004927da..0x00492977 and 0x00496890..0x00496922. `cursor_free` is `isCursorFree`
/// (`flow::is_cursor_free`, GC slot 1); `interaction_prompt` is the "[R] Talk" / "[R] Ride"
/// ... text of the interaction code, which overwrites the death text when present.
pub fn crosshair(player: &EntityData, cursor_free: bool, shift: bool, width: i32, height: i32, interaction_prompt: Option<&str>) -> CrosshairFrame {
    let zoomed = u16_at(player, 0x114) & 0x400 != 0;
    let mut text = String::new();
    // 0x004968d9: `0 >= hp` (`comiss 0, hp; jb`).
    if 0.0f32 >= f32_at(player, 0x15c) {
        text = "[R] Revive".to_string();
    }
    if let Some(p) = interaction_prompt {
        text = p.to_string();
    }
    CrosshairFrame {
        crosshair: !cursor_free && !zoomed && !shift,
        zoom: !cursor_free && zoomed,
        position: [(width / 2) as f32, (height / 2) as f32],
        text,
    }
}

// ---------------------------------------------------------------------------------------
// Quest tags
// ---------------------------------------------------------------------------------------

/// The fields of the mission `cube::Cell` (0x68 bytes) the quest tags and the objective text
/// (0x00477fa0) read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuestCell {
    /// `+0x18`, `+0x1c`, `+0x20`: the landscape kind, variant and id (the objective's
    /// `scenery` names, 0x005953a0).
    pub kind: i32,
    pub variant: i32,
    pub id: i32,
    /// `+0x30`: the objective's `@name` seed.
    pub f30: i32,
    /// `+0x34`: mission type (0 none; 1 = a point mission at `+0x60`).
    pub mission: i32,
    /// `+0x38`: the mission creature type (the `@name` race and the `creature` names,
    /// 0x00594c80).
    pub creature: i32,
    /// `+0x41`: mission state (1 active, 2 done).
    pub state: u8,
    /// `+0x44` / `+0x48`: progress and total (the `questbar` fill).
    pub progress: i32,
    pub total: i32,
    /// `+0x4c`: the mission zone.
    pub zone: [i32; 2],
    /// `+0x60`: the mission point, in zones (`* 256 + 128` blocks).
    pub point: [i32; 2],
}

impl QuestCell {
    /// The fields of a world cell (`cw_world::region::Cell`: mission state at `+0x2c..`,
    /// saved monster at `+0x54..`).
    pub fn from_cell(c: &cw_world::region::Cell) -> Self {
        let (px, py) = c.monster.zone_xy();
        QuestCell {
            kind: c.kind,
            variant: c.variant,
            id: c.id,
            f30: c.mission.f30,
            mission: c.mission.f34,
            creature: c.mission.f38,
            state: c.mission.b41,
            progress: c.mission.f44,
            total: c.mission.f48,
            zone: [c.mission.f4c as i32, (c.mission.f4c >> 32) as i32],
            point: [px, py],
        }
    }

    /// The fields [`crate::names::objective_text`] reads.
    pub fn objective_cell(&self) -> crate::names::ObjectiveCell {
        crate::names::ObjectiveCell {
            kind: self.kind,
            variant: self.variant,
            id: self.id,
            f30: self.f30,
            mission: self.mission,
            creature: self.creature,
        }
    }
}

/// The static copy 0x0076b0d0 (`+0x30`, `+0x34`, `+0x4c`, `+0x41` of the last shown cell).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuestTagState {
    /// `None` after a reset (0x00465bb0).
    pub last: Option<(i32, i32, [i32; 2], u8)>,
}

/// What the quest tags do this frame; `None` fields are left unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestTagFrame {
    /// `smallquesttag` visibility.
    pub small_visible: Option<bool>,
    /// Play `setState("show")` on `smallquesttag` and set its `objective` text.
    pub small_show: Option<String>,
    /// `questbar` visibility and fill (`bar` +0x8007e0).
    pub quest_bar: Option<Option<f32>>,
    /// Play sound 0x2e (at the listener, volume 1, pitch 1).
    pub sound_2e: bool,
    /// `WorldMap::markVisited` 0x005fc160 of the mission zone (state 1).
    pub mark_zone: Option<[i32; 2]>,
}

/// 0x00490a79: `questtag.setVisible(questtag.isAnimating())` (`Node::isAnimating`
/// 0x006364f0: the node or a child has a running state animation), so the tag shows only
/// while its "shine" plays.
pub fn questtag_visible(animating: bool) -> bool {
    animating
}

/// `smallquesttag` / `questbar` for the cell under the player (0x0048d054..0x0048d51c).
/// `cell` is `World::getCell 0x00487da0` at the player's `(block / 256) / 8` (truncating) cell,
/// `falloff` its `Cell::falloff` 0x005fa4c0 at the player's fixed x, y; `player_pos`
/// the fixed position. `objective` builds the text (0x00477fa0, words joined by
/// `tooltip::join_words`).
pub fn small_quest_tag(
    state: &mut QuestTagState,
    cell: Option<&QuestCell>,
    falloff: f32,
    player_pos: [i64; 3],
    objective: &dyn Fn(&QuestCell) -> String,
) -> QuestTagFrame {
    let mut f = QuestTagFrame::default();
    let Some(c) = cell else {
        f.small_visible = Some(false);
        state.last = None;
        return f;
    };
    // 0x0048d0e0: a point mission uses `(1 - min(d² / 16900, 1))²` over the 2D distance to
    // its point.
    let mut k = falloff;
    if c.mission == 1 {
        let px = (player_pos[0] / 0x10000) as f32;
        let py = (player_pos[1] / 0x10000) as f32;
        let dx = (c.point[0] * 256 + 128) as f32 - px;
        let dy = (c.point[1] * 256 + 128) as f32 - py;
        let mut r = (dx * dx + dy * dy) / 16900.0f32;
        if r > 1.0f32 {
            r = 1.0;
        }
        k = (1.0f32 - r) * (1.0f32 - r);
    }
    // 0x0048d1b1: `0 >= k` hides the tag and resets the static.
    if !(k > 0.0f32) {
        f.small_visible = Some(false);
        state.last = None;
        return f;
    }
    if c.mission != 0 && c.state != 2 && k > 0.1f32 {
        if c.state == 1 {
            f.mark_zone = Some(c.zone);
        }
        let key = (c.f30, c.mission, c.zone, c.state);
        if state.last != Some(key) {
            state.last = Some(key);
            if c.state == 1 {
                f.small_show = Some(objective(c));
                f.sound_2e = true;
            }
        }
        f.small_visible = Some(true);
        f.quest_bar = Some(if c.total != 0 && c.progress != 0 { Some(c.progress as f32 / c.total as f32) } else { None });
    } else if c.state == 2 || 0.05f32 > k {
        f.small_visible = Some(false);
        f.quest_bar = Some(None);
    }
    f
}

/// A server mission event of kind 2 (0x004906da): `questtag` plays "shine", its `objective`
/// text becomes the cell's objective, and sound 0x1e plays. Returns the text to set.
pub fn quest_complete_tag(cell: &QuestCell, objective: &dyn Fn(&QuestCell) -> String) -> String {
    objective(cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(e: &mut EntityData, o: usize, v: f32) {
        e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn kinds_and_colors() {
        let mut e = EntityData::new_creature();
        e.0[0x50] = 6;
        assert_eq!(plate_kind(&e), PlateKind::Static);
        assert_eq!(PlateKind::Static.member_index(), 6);
        e.0[0x50] = 3;
        assert_eq!(plate_kind(&e), PlateKind::Friend);
        let mut p = EntityData::new_creature();
        p.0[0x180..0x184].copy_from_slice(&10i32.to_le_bytes());
        e.0[0x180..0x184].copy_from_slice(&40i32.to_le_bytes());
        assert_eq!(plate_color(&e, &p), Some([1.0, 0.2, 0.2, 1.0]));
        e.0[0x180..0x184].copy_from_slice(&10i32.to_le_bytes());
        assert_eq!(plate_color(&e, &p), Some([0.0, 1.0, 1.0, 1.0]));
        e.0[0x180..0x184].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(plate_color(&e, &p), None);
        e.0[0x50] = 0;
        assert_eq!(plate_color(&e, &p), Some([0.0, 0.8, 1.0, 1.0]));
    }

    #[test]
    fn range_and_candidates() {
        assert_eq!(plate_range(255, 1.0), 200.0);
        assert_eq!(plate_range(0, 1.0), 20.0);
        let mut a = EntityData::new_creature();
        wf(&mut a, 0x15c, 10.0);
        let b = a.clone();
        let mut dead = a.clone();
        wf(&mut dead, 0x15c, 0.0);
        let cs = [
            PlateCreature { id: 1, entity: &a, render_pos: [30 << 16, 0, 0], step_offset: 0.0 },
            PlateCreature { id: 2, entity: &b, render_pos: [10 << 16, 0, 0], step_offset: 0.0 },
            PlateCreature { id: 3, entity: &dead, render_pos: [0, 0, 0], step_offset: 0.0 },
            PlateCreature { id: 4, entity: &b, render_pos: [61 << 16, 0, 0], step_offset: 0.0 },
        ];
        let v = plate_candidates(&cs, [0; 3], &|id| id == 1);
        assert_eq!(v.iter().map(|c| c.id).collect::<Vec<_>>(), vec![2, 1]);
        assert!(v[1].under_crosshair);
    }

    #[test]
    fn plates_project_and_filter() {
        let player = EntityData::new_creature();
        let mut m = EntityData::new_creature();
        wf(&mut m, 0x15c, 50.0);
        m.0[0x50] = 1;
        // Identity view with the creature 5 blocks in front (z > 0), identity projection.
        let view = Mat4::IDENTITY;
        let proj = Mat4::IDENTITY;
        let light = |_x: i32, _y: i32, _z: i32| 255;
        let los = |_p: [i64; 3], _m: f32| true;
        let mode = |_id: i64| None;
        let singular = |_t: i32| "Goblin".to_string();
        let mut inp = PlateInputs {
            player_id: 99,
            player: &player,
            show_all: false,
            origin: [0, 0],
            view: &view,
            projection: &proj,
            width: 800,
            height: 600,
            light_scale: 1.0,
            ground_light: &light,
            line_of_sight: &los,
            mode_of: &mode,
            singular: Some(&singular),
        };
        let c = PlateCreature { id: 7, entity: &m, render_pos: [0, 0, 5 << 16], step_offset: 0.0 };
        let look = |id: i64| (id == 7).then_some(c);
        // A monster without V and off the crosshair: no plate.
        let cand = [PlateCandidate { id: 7, dist_sq: 25.0, under_crosshair: false }];
        assert!(nameplates(&inp, &cand, &look, None).is_empty());
        inp.show_all = true;
        let v = nameplates(&inp, &cand, &look, None);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].position, [400.0, 300.0]);
        assert_eq!(v[0].kind, PlateKind::Enemy);
        assert_eq!(v[0].rare_name_visible, Some(true));
        assert_eq!(v[0].info, "Goblin");
        // Under the crosshair the plate shows without V, rare names untouched.
        inp.show_all = false;
        let cand = [PlateCandidate { id: 7, dist_sq: 25.0, under_crosshair: true }];
        let v = nameplates(&inp, &cand, &look, None);
        assert_eq!(v[0].rare_name_visible, None);
    }

    #[test]
    fn crosshair_and_death() {
        let mut p = EntityData::new_creature();
        wf(&mut p, 0x15c, 10.0);
        let c = crosshair(&p, false, false, 801, 601, None);
        assert!(c.crosshair && !c.zoom);
        assert_eq!(c.position, [400.0, 300.0]);
        assert_eq!(c.text, "");
        wf(&mut p, 0x15c, 0.0);
        assert_eq!(crosshair(&p, false, false, 800, 600, None).text, "[R] Revive");
        p.0[0x114] = 0;
        p.0[0x115] = 4; // flag 0x400
        let c = crosshair(&p, false, false, 800, 600, None);
        assert!(!c.crosshair && c.zoom);
        assert!(!crosshair(&p, true, false, 800, 600, None).zoom);
    }

    #[test]
    fn small_tag_rules() {
        let mut s = QuestTagState::default();
        let obj = |_c: &QuestCell| "Kill the orc".to_string();
        let c = QuestCell { mission: 2, state: 1, progress: 1, total: 4, zone: [3, 4], ..Default::default() };
        let f = small_quest_tag(&mut s, Some(&c), 0.5, [0; 3], &obj);
        assert_eq!(f.small_show.as_deref(), Some("Kill the orc"));
        assert!(f.sound_2e);
        assert_eq!(f.quest_bar, Some(Some(0.25)));
        assert_eq!(f.small_visible, Some(true));
        // Same cell again: no new "show".
        let f = small_quest_tag(&mut s, Some(&c), 0.5, [0; 3], &obj);
        assert_eq!(f.small_show, None);
        // Between 0.05 and 0.1: unchanged.
        let f = small_quest_tag(&mut s, Some(&c), 0.07, [0; 3], &obj);
        assert_eq!(f.small_visible, None);
        // Out of range: hidden and reset.
        let f = small_quest_tag(&mut s, Some(&c), 0.0, [0; 3], &obj);
        assert_eq!(f.small_visible, Some(false));
        assert_eq!(s.last, None);
    }
}
