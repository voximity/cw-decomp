//! The four speech bubbles (`GC+0x800920[4]` nodes, `GC+0x80092c[4]` `cube::SpeechWidget`s,
//! clones of the `speech` template made by the ctor at 0x00461a68..0x00461c21): what a
//! villager says when the player presses R on it, the "There is nothing special." of an
//! examined static, and the mission dialogs of the received records.
//!
//! Tier B (text, timing, `rand()` draws, placement); the glyphs are the renderer's.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | `update` 0x0049081b..0x004908ae: every bubble's clock `+0x16c += dt` | [`advance_clocks`] |
//! | `update` 0x004966e2..0x004967ed: the page step (hide and free past the last page) | [`page_steps`] |
//! | `0x004882e0(creature, force)`: a creature's bubble (R on a villager at 0x00496a4f, the mission dialog at 0x0049071c) | [`select_bubble`], [`creature_speech`], [`region_reveal`] |
//! | `0x004e4bd0(world, kind, coords, lines)`: the villager's text | [`villager_lines`] |
//! | `0x00488030(static, force)`: R on a static without its own action (0x00497563) | [`examine_static`] |
//! | `update` 0x00496f37..0x00497333: the bubbles follow their creature | [`place_bubble`] |
//! | `SpeechWidget::update` 0x004e5f90 with the run layout 0x004e65a0 | [`layout`] |
//! | the World ctor 0x00593a97..0x00593dbd: the pet types pushed into `World+0x88` | [`PET_TYPES`] |
//!
//! `GC+0x800940`, read by the talk code (0x004969ab, 0x004974c2), has one writer, the ctor's
//! `mov [ebx+0x800940], 0` (0x0045a345): it is always null, so its branches are dead and
//! not ported.

use std::collections::{BTreeMap, BTreeSet};

use cw_math::rand::MsvcRand;
use cw_net::EntityData;
use cw_world::World;
use glam::Vec2;

use super::speech::SpeechWidget;
use super::textdb::{Lines, TextDb, Word, WHITE};
use crate::names::{add_entry_vars, creature_type_key, item_key, landscape_key};
use crate::ui::names::generate_name;

/// `World+0x88`: the creature types pet food exists for, pushed in this order by the World
/// ctor (Cube.exe 0x00593a97..0x00593dbd, Server.exe 0x004cd507..0x004cd836; 45 calls of
/// `vector<int>::push_back`). The villager text draws from it (0x004e4cfd, 0x004e5078) and
/// so does the loot's bait (`cw_sim::combat::random_consumable`). One copy, shared with the
/// server side.
pub use cw_world::world::PET_TYPES;

/// The run layout's line height (`+0x1cc += 0x12`, 0x004e67a0) and word gaps.
const LINE_HEIGHT: i32 = 0x12;

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

/// C's `/` on ints (truncation towards zero), as `(x + (x >> 31 & (n - 1))) >> log2 n`.
fn div_trunc(a: i32, b: i32) -> i32 {
    a / b
}

/// The bubble widgets, one per `speech` clone of the HUD (`HudNodes::bubbles`).
pub fn ensure(bubbles: &mut Vec<SpeechWidget>, n: usize) {
    if bubbles.len() != n {
        bubbles.resize(n, SpeechWidget::default());
    }
}

/// `update` 0x0049081b..0x004908ae: every bubble's `+0x16c += dt` (the dialog's own clock is
/// 0x004908bf).
pub fn advance_clocks(bubbles: &mut [SpeechWidget], dt: i32) {
    for b in bubbles {
        b.advance(dt);
    }
}

/// `update` 0x004966e2..0x004967ed: each bubble's page step ([`SpeechWidget::page_step`]);
/// the returned flags say which bubble nodes are hidden this frame (the page index past the
/// last page; `+0x188` is cleared with it).
pub fn page_steps(bubbles: &mut [SpeechWidget]) -> Vec<bool> {
    bubbles.iter_mut().map(|b| b.page_step()).collect()
}

/// The bubble search shared by 0x004882e0 (0x00488316..0x004883a0 / 0x004884a6..0x00488525)
/// and 0x00488030 (0x0048804e..0x004880c6): a bubble already following `creature`, without
/// `force`, moves on (the page has had its time: next page, clock and reveal count 0) or
/// shows its whole page (clock = the page time), and nothing more happens (`None`); with
/// `force` it is written again. Otherwise the **last** free bubble (`+0x188 == 0`) is
/// written; with none, nothing happens.
pub fn select_bubble(bubbles: &mut [SpeechWidget], creature: i64, force: bool) -> Option<usize> {
    if let Some(i) = bubbles.iter().position(|b| b.creature == creature) {
        if force {
            return Some(i);
        }
        let b = &mut bubbles[i];
        if b.page_done() {
            b.page += 1;
            b.elapsed_ms = 0;
            b.prev_revealed = 0;
        } else {
            b.elapsed_ms = b.page_time();
        }
        return None;
    }
    let mut free = None;
    for (i, b) in bubbles.iter().enumerate() {
        if b.creature == 0 {
            free = Some(i);
        }
    }
    free
}

/// What a written bubble starts with (0x00488920..0x00488974 / 0x00488130..0x00488232): the
/// node shown (the caller's), the creature and anchor, the pages, clock, page and reveal
/// count 0, no answer options, hovered option 0.
fn write_bubble(b: &mut SpeechWidget, creature: i64, pos: [i64; 3], lines: &Lines) {
    b.creature = creature;
    b.pos = pos;
    b.set_lines(lines);
    b.elapsed_ms = 0;
    b.prev_revealed = 0;
    b.page = 0;
    b.options.clear();
    b.hovered = 0;
}

/// `World::addSceneryVars("scenery", cell, ctx)` 0x005953a0 as the objective uses it: the
/// landscape entry of the cell's kind and variant, `@` replaced by the scenery name.
fn add_scenery_vars(db: &TextDb, cell: &cw_world::region::Cell, tags: &mut BTreeSet<String>, vars: &mut BTreeMap<String, String>) {
    let scenery_name = generate_name(cell.id as u32, -1);
    add_entry_vars(db, landscape_key(cell.kind, cell.variant), "scenery", Some(&scenery_name), tags, vars);
}

/// `World::addItemVars(prefix, item, ctx)` 0x00595010: the entry of the item's `(type, sub
/// type)` key (0x0059fbf0), tags and forms as [`add_entry_vars`] makes them.
fn add_item_vars(db: &TextDb, prefix: &str, item_type: u8, sub: u8, tags: &mut BTreeSet<String>, vars: &mut BTreeMap<String, String>) {
    add_entry_vars(db, item_key(i32::from(item_type), i32::from(sub)), prefix, None, tags, vars);
}

/// `0x004e4bd0(world, kind, coords, lines)` (a `cube::Speech` member): the villager's text,
/// appended to `lines`. `srand(seed)` first (the creature id's low dword, `coords[2]`, which
/// reseeds the CRT stream the client world shares: `rng`). `coords` are the zone
/// coordinates `creature+0x1dc/+0x1e0` (after 0x004882e0's level check).
///
/// - kind 4 with a cell at `coords / 8` whose kind is not 0: the `scenery` names of the cell
///   and `explored` (0x004e4c59..0x004e4cef);
/// - kind 3: a pet type `PET_TYPES[rand() % 45]`, its `creature` names, the `food` names of
///   the bait item (type 0x14, sub type the pet type), then `petfood`
///   (0x004e4cfd..0x004e4eac);
/// - otherwise (and kind 4 without that cell): `rand() % 5` picks the `itemvendoritem` item
///   (0x0b/0x0c, 0x18/0, 0x17/0, 0x17/1, 1/7), a pet type gives the `pet` names, `rand() % 11`
///   a monster type (0x6c, 0x6d, 0x72, 0x73, 0x77, 0x6f, 0x71, 0x70, 0x74, 0x6e, 0x75) the
///   `monster` names, and `random:<rand() % 30 + 1>` is rendered (0x004e4eb1..0x004e52b4).
pub fn villager_lines(db: &TextDb, world: &World, rng: &mut MsvcRand, kind: u8, coords: [i32; 2], seed: u32, lines: &mut Lines) {
    // 0x004e4c19: `srand(coords[2])`.
    *rng = MsvcRand::new(seed);
    // 0x004e4c46: `World::getCell(x / 8, y / 8)` (signed, truncating).
    let cell = world.cell(div_trunc(coords[0], 8), div_trunc(coords[1], 8));
    let mut tags = BTreeSet::new();
    let mut vars = BTreeMap::new();
    if kind == 4 {
        if let Some(c) = cell
            && c.kind != 0
        {
            add_scenery_vars(db, c, &mut tags, &mut vars);
            db.render("explored", &tags, &vars, lines);
            return;
        }
    } else if kind == 3 {
        // 0x004e4d0c: `rand() % size` (unsigned division).
        let pet = PET_TYPES[(rng.rand() as u32 % PET_TYPES.len() as u32) as usize];
        add_entry_vars(db, creature_type_key(pet), "creature", None, &mut tags, &mut vars);
        // 0x004e4d99: `Item::Item` inline, type 0x14, sub type the pet type's low byte.
        add_item_vars(db, "food", 0x14, pet as u8, &mut tags, &mut vars);
        db.render("petfood", &tags, &vars, lines);
        return;
    }
    // 0x004e4ef7: `rand() % 5` (signed division; `rand()` is never negative).
    let (item_type, sub) = match rng.rand() % 5 {
        0 => (0x0b, 0x0c),
        1 => (0x18, 0),
        2 => (0x17, 0),
        3 => (0x17, 1),
        _ => (1, 7),
    };
    add_item_vars(db, "itemvendoritem", item_type, sub, &mut tags, &mut vars);
    let pet = PET_TYPES[(rng.rand() as u32 % PET_TYPES.len() as u32) as usize];
    add_entry_vars(db, creature_type_key(pet), "pet", None, &mut tags, &mut vars);
    // 0x004e50d0: jump table 0x004e52f0 over `rand() % 11`.
    let monster = match rng.rand() % 11 {
        1 => 0x6d,
        2 => 0x72,
        3 => 0x73,
        4 => 0x77,
        5 => 0x6f,
        6 => 0x71,
        7 => 0x70,
        8 => 0x74,
        9 => 0x6e,
        10 => 0x75,
        _ => 0x6c,
    };
    add_entry_vars(db, creature_type_key(monster), "monster", None, &mut tags, &mut vars);
    // 0x004e5208: `"random:" << rand() % 30 + 1`.
    let k = rng.rand() % 30 + 1;
    db.render(&format!("random:{k}"), &tags, &vars, lines);
}

/// What 0x004882e0 did besides the bubble.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpeechOutcome {
    /// The bubble written (its node is shown by the caller), if any.
    pub shown: Option<usize>,
    /// Zones marked visited on the world map (`WorldMap::markVisited` 0x005fc160).
    pub visited: Vec<(i32, i32)>,
}

/// The creature a bubble is written for.
pub struct SpeechCreature<'a> {
    pub id: i64,
    pub entity: &'a EntityData,
    /// `creature+0x1350`: the render position (the anchor starts there).
    pub render_pos: [i64; 3],
}

/// `0x004882e0(creature, force)`. A creature with flag 0x80 (`creature+0x7e`, entity
/// `+0x6e`) reveals the cells of its home region on the map instead ([`region_reveal`]).
/// Otherwise, with a bubble from [`select_bubble`]: the zone coordinates
/// `creature+0x1dc/+0x1e0` go to `(-1, -1)` when the cell at `coords / 8` has a level more
/// than two away from the player's (0x00488536..0x004885ea); an innkeeper (class 0x84)
/// first renders `innkeeper` (0x004885f0..0x0048867a); then [`villager_lines`] with the
/// creature's speech kind `creature+0x1d8`, the map marks the zone visited (0x0048868b), and
/// when the first line has words the bubble is written, anchored at the render position
/// (0x004887b4..0x00488974).
///
/// The map entry copy of 0x004886a9..0x00488799 (16 bytes of the region's zone summary into
/// the map record of the zone) is not ported.
#[allow(clippy::too_many_arguments)]
pub fn creature_speech(
    bubbles: &mut [SpeechWidget],
    c: &SpeechCreature,
    force: bool,
    player_level: i32,
    db: Option<&TextDb>,
    world: &World,
    rng: &mut MsvcRand,
) -> SpeechOutcome {
    let mut out = SpeechOutcome::default();
    let e = c.entity;
    // 0x004882fe: the flag 0x80 branch.
    let Some(i) = select_bubble(bubbles, c.id, force) else { return out };
    if e.0[0x6e] & 0x80 != 0 {
        out.visited = region_reveal(world, [i32_at(e, 0x1a0), i32_at(e, 0x1a4)], player_level);
        return out;
    }
    // 0x00488536: the zone coordinates, invalidated by a cell of the wrong level.
    let mut coords = [i32_at(e, 0x1cc), i32_at(e, 0x1d0)];
    let (cx, cy) = (div_trunc(coords[0], 8), div_trunc(coords[1], 8));
    if (0..0x2000).contains(&cx)
        && (0..0x2000).contains(&cy)
        && let Some(cell) = world.cell(cx, cy)
        && (player_level + 2 < cell.level || cell.level < player_level - 2)
    {
        coords = [-1, -1];
    }
    let mut lines = Lines::new();
    if let Some(db) = db {
        // 0x004885f0: an innkeeper.
        if e.0[0x130] == 0x84 {
            db.render("innkeeper", &BTreeSet::new(), &BTreeMap::new(), &mut lines);
        }
        villager_lines(db, world, rng, e.0[0x1c8], coords, c.id as u32, &mut lines);
    } else {
        // Without the dictionary the texts are empty; `srand` still runs.
        *rng = MsvcRand::new(c.id as u32);
    }
    // 0x0048868b: `WorldMap::markVisited(coords)`.
    out.visited.push((coords[0], coords[1]));
    // 0x004887b4: the first line has words.
    if lines.first().is_some_and(|l| !l.is_empty()) {
        write_bubble(&mut bubbles[i], c.id, c.render_pos, &lines);
        out.shown = Some(i);
    }
    out
}

/// 0x0048838c..0x004884a1 (flag 0x80): the 8x8 cells of the region holding the creature's
/// home zone `(x / 64, y / 64)`, in memory order, each with a kind and a level at most two
/// above the player's, mark the zone of their centre visited (`(x / 65536) / 256`, both
/// truncating).
pub fn region_reveal(world: &World, home_zone: [i32; 2], player_level: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let (rx, ry) = (div_trunc(home_zone[0], 64), div_trunc(home_zone[1], 64));
    let Some(region) = world.region(rx, ry) else { return out };
    for c in region.cells.iter().take(64) {
        if c.kind != 0 && c.level <= player_level + 2 {
            let zx = ((c.x / 0x10000) as i32) / 256;
            let zy = ((c.y / 0x10000) as i32) / 256;
            out.push((zx, zy));
        }
    }
    out
}

/// `0x00488030(static, force)`: R on a static whose kind is not 1, 2, 3, 0xa, 0x12, 0x10,
/// 0x13 or 0x45 (0x004880ca) writes a bubble over the local player (the search of
/// [`select_bubble`] with the player as the creature) with the one white line "There is
/// nothing special." (0x00701db8, split by 0x004e5740), anchored at the player's position
/// (`creature+0x10`). Returns the bubble written.
pub fn examine_static(bubbles: &mut [SpeechWidget], kind: u32, player: i64, player_pos: [i64; 3], force: bool) -> Option<usize> {
    let i = select_bubble(bubbles, player, force)?;
    if matches!(kind, 1 | 2 | 3 | 0xa | 0x12 | 0x10 | 0x13 | 0x45) {
        return None;
    }
    let mut line = Vec::new();
    super::textdb::push_words(&mut line, "There is nothing special.", WHITE);
    write_bubble(&mut bubbles[i], player, player_pos, &vec![line]);
    Some(i)
}

/// A bubble's creature as the placement reads it.
pub struct BubbleTarget {
    /// `creature+0x1350`: the render position.
    pub render_pos: [i64; 3],
    /// `creature+0x1180`: the step offset.
    pub step_offset: f32,
    /// `creature+0x88` (entity `+0x78`): the box height.
    pub height: f32,
}

/// The camera of the placement (the nameplates' inputs, [`super::target::PlateInputs`]).
pub struct BubbleCamera<'a> {
    /// `GC+0x1d8`, `GC+0x1e0`: the render origin.
    pub origin: [i64; 2],
    /// `GC+0x26c`: the view relative to the render origin.
    pub view: &'a crate::player::Mat4,
    /// `GC+0x800a90`: the projection.
    pub projection: &'a crate::player::Mat4,
    pub width: i32,
    pub height: i32,
}

/// `vec3i64 * fixed` 0x0042c900 per axis: `(v * f) / 0x10000`, 64-bit, truncating.
fn mul_fixed(v: i64, f: i64) -> i64 {
    v.wrapping_mul(f) / 0x10000
}

/// `update` 0x00496f37..0x00497333 for one bubble with a creature: the anchor eases towards
/// the creature's feet (`render - (0, 0, step)`, fixed) by `lerpFactor(dt, 0.02)` as a fixed
/// factor; the head point `anchor + (origin.x, origin.y, height * 0.5 + 1)` goes through the
/// view; in front of the camera (`z > 0`, a `comiss`: NaN hides) it is projected and the
/// bubble placed at `screen - pivot - (125, 170)`. Returns the node translation, `None` to
/// hide the node (the creature stays).
pub fn place_bubble(b: &mut SpeechWidget, t: &BubbleTarget, cam: &BubbleCamera, dt: i32, pivot: Vec2) -> Option<Vec2> {
    // 0x00496f77: `0x004ac150(dt, 0.02)`.
    let f = crate::player::lerp_factor(dt, 0.02);
    // 0x00496fa9: `(0, 0, -step)` to fixed (0x0042c460), the factor to fixed (0x0042c580).
    let off = [0i64, 0, (-t.step_offset * 65536.0f32) as i64];
    let ff = (f * 65536.0f32) as i64;
    for i in 0..3 {
        let goal = t.render_pos[i].wrapping_add(off[i]);
        let d = goal.wrapping_sub(b.pos[i]);
        b.pos[i] = b.pos[i].wrapping_add(mul_fixed(d, ff));
    }
    // 0x004970a6: `height * 0.5 + 1` to fixed.
    let z = ((t.height * 0.5f32 + 1.0f32) * 65536.0f32) as i64;
    let rel = [b.pos[0].wrapping_add(cam.origin[0]), b.pos[1].wrapping_add(cam.origin[1]), b.pos[2].wrapping_add(z)];
    // 0x0042c4a0: each axis to float, then times 1/65536.
    let k = 1.525_878_9e-5f32;
    let p = [(rel[0] as f32) * k, (rel[1] as f32) * k, (rel[2] as f32) * k];
    let v = cam.view.transform_point(p);
    // 0x0049713f: `comiss v.z, 0` / `jbe` hides.
    if !(v[2] > 0.0f32) {
        return None;
    }
    let ndc = cam.projection.transform_point(v);
    let (w, h) = (cam.width as f32, cam.height as f32);
    let x = ndc[0] * (w * 0.5f32) + w * 0.5f32;
    let y = ndc[1] * (-h * 0.5f32) + h * 0.5f32;
    // 0x00497265..0x004972cb: `(screen - pivot) - (125, 170)`.
    Some(Vec2::new(x, y) - pivot - Vec2::new(125.0, 170.0))
}

/// One drawn run of a bubble (two text calls: an outline pass with a black stroke, then the
/// fill pass).
#[derive(Clone, Debug, PartialEq)]
pub struct RunDraw {
    pub text: String,
    /// Node space.
    pub origin: [f32; 2],
    pub color: [f32; 4],
}

/// The width 0x004e65a0 wraps against: `0x00627d50(widget)`, the widget's **local** width
/// (`(bindSize + stretch) − innerBindSize` on x, no world transform; 250 for a bubble made
/// by the ctor 0x004e5c90).
pub fn bubble_width(gui: &cw_ui::widget::Gui, w: cw_ui::widget::WidgetId) -> f32 {
    gui.local_size(w).x
}

/// `0x00439190(run, s)` against the punctuation of 0x004e6697..0x004e6710.
fn is_punct(t: &str) -> bool {
    matches!(t, "." | "-" | "," | ";" | "!" | "?" | ":")
}

/// The run layout 0x004e65a0 over the current page, as `SpeechWidget::update` 0x004e5f90
/// walks it (the cursor `+0x1c8/+0x1cc` and count `+0x1d0` reset each frame, the "no space"
/// flag starting set). Per run: an empty run is skipped; a run starting past the reveal limit
/// (`elapsed / ms_per_char < count`) ends the walk; its extent is measured at size 14; unless
/// the flag is set the cursor first moves 3 (punctuation) or 14 pixels; the flag becomes
/// "the run is `-`"; a word that is not punctuation and would pass `width - 30` starts a new
/// line (x 0, y + 18); the revealed part is drawn at `(x + 10 + pivot.x, y + 25 + pivot.y)`;
/// the cursor moves by `(int)` of the full extent and the count by `len + 1`.
pub fn layout(b: &SpeechWidget, width: f32, pivot: Vec2, measure: &dyn Fn(&str) -> f32) -> Vec<RunDraw> {
    let mut out = Vec::new();
    if !(-1 < b.page && (b.page as usize) < b.pages.len()) || b.ms_per_char == 0 {
        return out;
    }
    let Some(words) = b.texts.get(b.page as usize) else { return out };
    let limit = b.elapsed_ms / b.ms_per_char;
    let (mut x, mut y, mut count) = (0i32, 0i32, 0i32);
    let mut no_space = true;
    for w in words {
        let units: Vec<u16> = w.text.encode_utf16().collect();
        let len = units.len() as i32;
        if len == 0 {
            continue;
        }
        if limit < count {
            break;
        }
        let extent = measure(&w.text);
        let punct = is_punct(&w.text);
        if !no_space {
            x += if punct { 3 } else { 0xe };
        }
        no_space = w.text == "-";
        if !punct && width - 30.0f32 < (x + extent as i32) as f32 {
            x = 0;
            y += LINE_HEIGHT;
        }
        let shown = if limit < count + len { String::from_utf16_lossy(&units[..(limit - count).max(0) as usize]) } else { w.text.clone() };
        let origin = [(x + 10) as f32 + pivot.x, (y + 0x19) as f32 + pivot.y];
        out.push(RunDraw { text: shown, origin, color: w.color });
        x += extent as i32;
        count += len + 1;
    }
    out
}

/// The words of a line as plain text (tests and diagnostics).
pub fn line_text(line: &[Word]) -> String {
    line.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubbles() -> Vec<SpeechWidget> {
        vec![SpeechWidget::default(); 4]
    }

    #[test]
    fn search_takes_the_last_free_bubble_and_advances_an_existing_one() {
        let mut b = bubbles();
        assert_eq!(select_bubble(&mut b, 5, false), Some(3));
        b[3].creature = 5;
        b[3].set_lines(&vec![vec![Word { text: "Hi".into(), color: WHITE }], vec![Word { text: "Bye".into(), color: WHITE }]]);
        // Not revealed yet: the whole page shows.
        assert_eq!(select_bubble(&mut b, 5, false), None);
        assert_eq!(b[3].elapsed_ms, 3 * 40);
        // Revealed: the next page starts.
        assert_eq!(select_bubble(&mut b, 5, false), None);
        assert_eq!((b[3].page, b[3].elapsed_ms), (1, 0));
        // Forced: written again.
        assert_eq!(select_bubble(&mut b, 5, true), Some(3));
        // Another creature takes the last free one.
        assert_eq!(select_bubble(&mut b, 6, false), Some(2));
        for (i, x) in b.iter_mut().enumerate() {
            x.creature = i as i64 + 10;
        }
        assert_eq!(select_bubble(&mut b, 6, false), None);
    }

    #[test]
    fn examine_writes_nothing_special_over_the_player() {
        let mut b = bubbles();
        assert_eq!(examine_static(&mut b, 0x41, 1, [1, 2, 3], false), Some(3));
        assert_eq!(b[3].creature, 1);
        assert_eq!(b[3].pos, [1, 2, 3]);
        assert_eq!(line_text(&b[3].texts[0]), "There is nothing special.");
        assert_eq!(b[3].pages, vec![vec![5, 2, 7, 8]]);
        // A door is not examined (and the bubble over the player moves on instead).
        let mut b = bubbles();
        assert_eq!(examine_static(&mut b, 1, 1, [0; 3], false), None);
        assert!(b.iter().all(|x| x.creature == 0));
    }

    #[test]
    fn clock_and_page_step_free_the_bubble() {
        let mut b = bubbles();
        examine_static(&mut b, 0, 1, [0; 3], false);
        // (5 + 1 + 2 + 1 + 7 + 1 + 8 + 1) * 40 = 1040 ms of reveal, then 3 s.
        advance_clocks(&mut b, 4040);
        assert!(!page_steps(&mut b)[3]);
        advance_clocks(&mut b, 1);
        let hidden = page_steps(&mut b);
        assert!(hidden[3] && b[3].creature == 0);
        // Free bubbles are hidden every frame.
        assert!(hidden[0]);
    }

    #[test]
    fn layout_reveals_and_wraps() {
        let mut b = SpeechWidget::default();
        let line: Vec<Word> = ["Hello", ",", "big", "world"].iter().map(|t| Word { text: (*t).into(), color: WHITE }).collect();
        b.set_lines(&vec![line]);
        let m = |t: &str| 10.0 * t.len() as f32;
        // 3 characters revealed: "Hel".
        b.elapsed_ms = 3 * 40;
        let d = layout(&b, 400.0, Vec2::new(2.0, 3.0), &m);
        assert_eq!(d, vec![RunDraw { text: "Hel".into(), origin: [12.0, 28.0], color: WHITE }]);
        // Everything: the comma 3 px after "Hello", "big" 14 px after it.
        b.elapsed_ms = 10_000;
        let d = layout(&b, 400.0, Vec2::ZERO, &m);
        let o: Vec<[f32; 2]> = d.iter().map(|r| r.origin).collect();
        assert_eq!(o, vec![[10.0, 25.0], [63.0, 25.0], [87.0, 25.0], [131.0, 25.0]]);
        // A narrow bubble wraps "world" (x 121 + 50 > 150 - 30).
        let d = layout(&b, 150.0, Vec2::ZERO, &m);
        assert_eq!(d[3].origin, [10.0, 43.0]);
    }

    /// A bubble widget made as the ctor 0x004e5c90 makes it (`0x00627c00`: bind 250x200,
    /// inner 230x180 at (10, 10)) wraps at `0x00627d50 − 30` = 220 pixels: a sentence of
    /// 10-pixel characters breaks only where the next word would pass 220, not after every
    /// word (the play-test bug: the widget was 1x1, so every word wrapped).
    #[test]
    fn bubble_widget_wraps_a_sentence_at_220() {
        let mut gui = cw_ui::widget::Gui::new();
        let n = gui.add_plain_node(None, "");
        let w = gui.add_game_widget(n, &super::super::speech::widget_source(), cw_ui::widget::GameWidget::new(cw_ui::widget::GameWidgetClass::Speech));
        super::super::speech::init_widget(&mut gui, w);
        assert_eq!(bubble_width(&gui, w), 250.0);
        let mut b = SpeechWidget::default();
        let line: Vec<Word> = ["I", "have", "explored", "the", "Hyara", "Planes", "."].iter().map(|t| Word { text: (*t).into(), color: WHITE }).collect();
        b.set_lines(&vec![line]);
        b.elapsed_ms = 100_000;
        let d = layout(&b, bubble_width(&gui, w), Vec2::ZERO, &|t: &str| 10.0 * t.len() as f32);
        let rows: Vec<(String, f32)> = d.iter().map(|r| (r.text.clone(), r.origin[1])).collect();
        // x: I 0..10, have 24..64, explored 78..158, the 172..202, Hyara 216+50 > 220 wraps,
        // Planes 64..124, "." 127 (punctuation never wraps).
        assert_eq!(
            rows,
            vec![
                ("I".into(), 25.0),
                ("have".into(), 25.0),
                ("explored".into(), 25.0),
                ("the".into(), 25.0),
                ("Hyara".into(), 43.0),
                ("Planes".into(), 43.0),
                (".".into(), 43.0),
            ]
        );
        assert_eq!(d[4].origin[0], 10.0);
        assert_eq!(d[6].origin[0], 10.0 + 127.0);
    }

    #[test]
    fn placement_eases_and_projects() {
        use crate::player::Mat4;
        let mut b = SpeechWidget { creature: 3, pos: [0, 0, 0], ..SpeechWidget::default() };
        let t = BubbleTarget { render_pos: [10 << 16, 0, 0], step_offset: 0.0, height: 2.0 };
        // An identity view looking down +z would put everything at z = height/2 + 1 > 0.
        let cam = BubbleCamera { origin: [0, 0], view: &Mat4::IDENTITY, projection: &Mat4::IDENTITY, width: 800, height: 600 };
        let p = place_bubble(&mut b, &t, &cam, 0, Vec2::ZERO);
        // dt 0: the anchor stays; the screen point of (0, 0, 2) is (400, 300).
        assert_eq!(b.pos, [0, 0, 0]);
        assert_eq!(p, Some(Vec2::new(400.0 - 125.0, 300.0 - 170.0)));
        // A long frame moves it most of the way.
        place_bubble(&mut b, &t, &cam, 1000, Vec2::ZERO);
        assert!(b.pos[0] > 9 << 16 && b.pos[0] <= 10 << 16);
        // Behind the camera: hidden.
        let t = BubbleTarget { render_pos: [0, 0, -(10 << 16)], step_offset: 0.0, height: 2.0 };
        let mut b = SpeechWidget { creature: 3, pos: [0, 0, -(10 << 16)], ..SpeechWidget::default() };
        assert_eq!(place_bubble(&mut b, &t, &cam, 0, Vec2::ZERO), None);
    }

    fn dict() -> TextDb {
        TextDb::from_xml(
            br#"<dict>
<speech key="innkeeper">Welcome!</speech>
<speech key="explored">I have explored @scenery_normal .</speech>
<speech key="petfood">$creature[@creature_singular] eat $item[@food_singular] .</speech>
<speech key="random:1">One.</speech>
<speech key="random:2">Two.</speech>
<speech key="random:3">Three.</speech>
<speech key="random:4">Four.</speech>
<speech key="random:5">Five.</speech>
<speech key="random:6">Six.</speech>
<speech key="random:7">Seven.</speech>
<speech key="random:8">Eight.</speech>
<speech key="random:9">Nine.</speech>
<speech key="random:10">Ten.</speech>
<speech key="random:11">Eleven.</speech>
<speech key="random:12">Twelve.</speech>
<speech key="random:13">Thirteen.</speech>
<speech key="random:14">Fourteen.</speech>
<speech key="random:15">Fifteen.</speech>
<speech key="random:16">Sixteen.</speech>
<speech key="random:17">Seventeen.</speech>
<speech key="random:18">Eighteen.</speech>
<speech key="random:19">Nineteen.</speech>
<speech key="random:20">Twenty.</speech>
<speech key="random:21">Twenty-one.</speech>
<speech key="random:22">Twenty-two.</speech>
<speech key="random:23">Twenty-three.</speech>
<speech key="random:24">Twenty-four.</speech>
<speech key="random:25">Twenty-five.</speech>
<speech key="random:26">Twenty-six.</speech>
<speech key="random:27">Twenty-seven.</speech>
<speech key="random:28">Twenty-eight.</speech>
<speech key="random:29">Twenty-nine.</speech>
<speech key="random:30">Thirty.</speech>
</dict>"#,
        )
        .unwrap()
    }

    #[test]
    fn villager_text_draws_in_order_and_reseeds() {
        let db = dict();
        let world = World::new(1);
        let mut rng = MsvcRand::new(99);
        let mut lines = Lines::new();
        villager_lines(&db, &world, &mut rng, 0, [-1, -1], 1234, &mut lines);
        // The same draws on a stream seeded with the id: rand%5, rand%45, rand%11, rand%30+1.
        let mut r = MsvcRand::new(1234);
        let _ = (r.rand(), r.rand(), r.rand());
        let k = r.rand() % 30 + 1;
        let want = db.speech[&format!("random:{k}")].nodes.iter().map(|n| n.text.clone()).collect::<String>();
        assert!(!lines.is_empty());
        assert_eq!(line_text(&lines[0]).replace(' ', ""), want.replace(' ', ""));
        // The stream the tick uses was reseeded and has drawn four times.
        assert_eq!(rng, r);
    }

    /// The shipped dictionary has every key the villager text renders: the 30 `random:`
    /// texts, `petfood`, `innkeeper` (and `explored`); each kind gives words. Skipped
    /// without `CW_GAME_DIR`.
    #[test]
    fn game_dictionary_villager_texts() {
        let Ok(dir) = std::env::var("CW_GAME_DIR") else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let adb = cw_formats::db::AssetDb::open(std::path::Path::new(&dir).join("data4.db")).unwrap();
        let db = TextDb::load(&adb).unwrap();
        for k in 1..=30 {
            assert!(db.speech.contains_key(&format!("random:{k}")), "random:{k}");
        }
        for k in ["petfood", "innkeeper", "explored"] {
            assert!(db.speech.contains_key(k), "{k}");
        }
        let world = World::new(1);
        for (kind, seed) in [(0u8, 5u32), (3, 6), (4, 7), (0, 1234)] {
            let mut rng = MsvcRand::new(1);
            let mut lines = Lines::new();
            villager_lines(&db, &world, &mut rng, kind, [-1, -1], seed, &mut lines);
            assert!(lines.first().is_some_and(|l| l.iter().any(|w| !w.text.is_empty())), "kind {kind}: {lines:?}");
        }
    }

    #[test]
    fn innkeeper_speaks_first_and_the_bubble_follows() {
        let db = dict();
        let world = World::new(1);
        let mut rng = MsvcRand::new(1);
        let mut e = EntityData::constructed();
        e.0[0x130] = 0x84;
        e.0[0x1c8] = 0;
        let c = SpeechCreature { id: 42, entity: &e, render_pos: [7, 8, 9] };
        let mut b = bubbles();
        let o = creature_speech(&mut b, &c, false, 1, Some(&db), &world, &mut rng);
        assert_eq!(o.shown, Some(3));
        assert_eq!(b[3].creature, 42);
        assert_eq!(b[3].pos, [7, 8, 9]);
        assert_eq!(line_text(&b[3].texts[0]).split(' ').next(), Some("Welcome"));
        // R again before the text is out: the whole page shows, nothing else changes.
        let o = creature_speech(&mut b, &c, false, 1, Some(&db), &world, &mut rng);
        assert_eq!(o, SpeechOutcome::default());
        assert_eq!(b[3].elapsed_ms, b[3].page_time());
    }
}
