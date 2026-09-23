//! `cube::CharacterStyleWidget` (vtable 0x006fd0e4, ctor 0x00427ce0, 0x1a8 bytes): the
//! character creation rows Race / Gender / Class / Face / Haircut with left/right arrows, and
//! the hair colour palette.
//!
//! | Field | Offset | Rust |
//! |---|---|---|
//! | GameController* | +0x160 | (the caller) |
//! | 10 arrow buttons | +0x164..+0x188 | [`ARROWS`] |
//! | race 0..7 | +0x18c | [`CharacterStyle::race`] |
//! | class 0..3 | +0x190 | [`CharacterStyle::class`] |
//! | gender 0/1 | +0x194 | [`CharacterStyle::gender`] |
//! | face index | +0x198 | [`CharacterStyle::face`] |
//! | haircut index | +0x19c | [`CharacterStyle::haircut`] |
//! | hair colour | +0x1a0..+0x1a2 | [`CharacterStyle::hair`] |
//! | hovered palette colour | +0x1a3..+0x1a5 | [`CharacterStyle::hover`] |
//!
//! The arrows are cloned from the `leftbutton`/`rightbutton` templates and connected with
//! `connect<CharacterStyleWidget>` 0x00427bc0 on event 2 (left press); every callback plays
//! sound 0x55 (menu-select) at the listener and re-applies the style ([`CharacterStyle::apply`],
//! 0x0042c080). The palette is a transparent 280x150 hit shape at (10, 200) connected to 0x0042b8b0 (the nodes are
//! built by `ui::members` from the ctor).

use glam::Vec2;

use cw_net::entity::ITEM_SIZE;

use super::{GameView, UiAction};

/// The arrows: `(member offset, callback, row)`; rows are laid out by 0x0042bb00 at
/// y = 12, 42, 72, 102, 132 (left arrow x = 100, right arrow x = width - its width - 15).
pub const ARROWS: [(u32, u32, &str); 10] = [
    (0x164, 0x0042ba60, "race -"),
    (0x168, 0x0042bab0, "race +"),
    (0x16c, 0x0042b810, "class -"),
    (0x170, 0x0042b860, "class +"),
    (0x174, 0x0042b990, "gender toggle"),
    (0x178, 0x0042b990, "gender toggle"),
    (0x17c, 0x0042b910, "face -"),
    (0x180, 0x0042b950, "face +"),
    (0x184, 0x0042b9e0, "haircut -"),
    (0x188, 0x0042ba20, "haircut +"),
];

/// Row y of each arrow pair (0x0042bb00): race, gender, class, face, haircut.
pub const ARROW_ROWS: [(u32, u32, f32); 5] =
    [(0x164, 0x168, 12.0), (0x174, 0x178, 42.0), (0x16c, 0x170, 72.0), (0x17c, 0x180, 102.0), (0x184, 0x188, 132.0)];

/// One arrow press (the callback it runs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow {
    /// 0x0042ba60: race - 1, wrapping to 7.
    RacePrev,
    /// 0x0042bab0: race + 1, wrapping to 0.
    RaceNext,
    /// 0x0042b810: class - 1, wrapping to 3.
    ClassPrev,
    /// 0x0042b860: class + 1, wrapping to 0.
    ClassNext,
    /// 0x0042b990: gender `(g + 1) % 2` (both arrows).
    Gender,
    /// 0x0042b910: face - 1 (wrapped by apply).
    FacePrev,
    /// 0x0042b950: face + 1.
    FaceNext,
    /// 0x0042b9e0: haircut - 1.
    HairPrev,
    /// 0x0042ba20: haircut + 1.
    HairNext,
    /// 0x0042b8b0: palette click, hair colour = hovered colour.
    PickColor,
}

/// The widget's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterStyle {
    /// +0x18c.
    pub race: i32,
    /// +0x190.
    pub class: i32,
    /// +0x194.
    pub gender: i32,
    /// +0x198.
    pub face: i32,
    /// +0x19c.
    pub haircut: i32,
    /// +0x1a0.
    pub hair: [u8; 3],
    /// +0x1a3.
    pub hover: [u8; 3],
}

impl Default for CharacterStyle {
    /// ctor 0x00427ce0: zeros, `+0x1a0 = 0xff32c8ff` (hair (255, 200, 50), hover red 255),
    /// `+0x1a4 = 0xffff`.
    fn default() -> Self {
        CharacterStyle { race: 0, class: 0, gender: 0, face: 0, haircut: 0, hair: [0xff, 0xc8, 0x32], hover: [0xff; 3] }
    }
}

/// The face and haircut model ranges per entity type (0x0042c080): `(face0, face1, hair0,
/// hair1)`; types 2 and 6 (and anything else) take the default.
pub fn style_ranges(entity_type: i32) -> (i32, i32, i32, i32) {
    match entity_type {
        0 => (0x4d4, 0x4d7, 0x500, 0x509),
        1 => (0x4d8, 0x4dd, 0x50a, 0x513),
        3 => (0x4f3, 0x4f8, 0x4f9, 0x4ff),
        4 => (0x4b, 0x4f, 0x50, 0x55),
        5 => (0x56, 0x5a, 0x5b, 0x60),
        7 => (0x62, 99, 100, 0x69),
        8 => (0x6a, 0x6e, 100, 0x69),
        9 => (0x11a, 0x11e, 0x11f, 0x121),
        10 => (0x122, 0x126, 0x127, 299),
        11 => (0x514, 0x518, 0x51e, 0x527),
        12 => (0x519, 0x51d, 0x528, 0x52b),
        13 => (0x52c, 0x530, 0x531, 0x535),
        14 => (0x536, 0x539, 0x53a, 0x53d),
        15 => (0x12f, 0x134, 0x135, 0x13a),
        16 => (0x13b, 0x140, 0x141, 0x146),
        _ => (0x4de, 0x4e3, 0x4e4, 0x4f2),
    }
}

/// Race names of the update body (0x00428e40).
pub const RACE_NAMES: [&str; 8] = ["Human", "Elf", "Dwarf", "Orc", "Goblin", "Lizard", "Undead", "Frogman"];
/// Class names (0x00428e40).
pub const CLASS_NAMES: [&str; 4] = ["Warrior", "Ranger", "Mage", "Rogue"];

/// The 16 palette base colours in insertion order (0x0042bd20 calls of 0x00428e40).
pub const PALETTE: [[f32; 3]; 16] = [
    [1.0, 0.0, 0.0],
    [1.0, 0.25, 0.0],
    [1.0, 0.5, 0.0],
    [1.0, 1.0, 0.0],
    [0.5, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 1.0, 0.5],
    [0.0, 1.0, 1.0],
    [0.0, 0.5, 1.0],
    [0.0, 0.25, 1.0],
    [0.0, 0.0, 1.0],
    [0.25, 0.0, 1.0],
    [0.5, 0.0, 1.0],
    [0.75, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [0.5, 0.5, 0.5],
];

/// One palette cell: its rect (x, y, 17x17 outer, 15x15 inner at +1) and colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaletteCell {
    /// Outer top-left.
    pub pos: Vec2,
    /// RGB.
    pub color: [f32; 3],
}

/// The palette of 0x00428e40: column `i` (base colour) at `x = 13 + 17i`, row `j` at
/// `y = 197 + 17j` while `y < 316` (7 rows). Rows 0..2 darken (`c * (0.25j + 0.2)`), row 3 is
/// the base, rows 4.. lighten toward white by `(j - 3) * 0.25`.
pub fn palette() -> Vec<PaletteCell> {
    let mut out = Vec::new();
    for (i, base) in PALETTE.iter().enumerate() {
        let x = (i as i32) * 0x11 + 0xd;
        let mut y = 0xc5;
        let mut j = 0;
        while y < 0x13c {
            let mut c = *base;
            if j < 3 {
                let f = j as f32 * 0.25 + 0.2;
                c = [f * c[0], f * c[1], f * c[2]];
            }
            if j > 3 {
                let t = (j as f32 - 3.0) * 0.25;
                c = [c[0] + (1.0 - c[0]) * t, c[1] + (1.0 - c[1]) * t, c[2] + (1.0 - c[2]) * t];
            }
            out.push(PaletteCell { pos: Vec2::new(x as f32, y as f32), color: c });
            y += 0x11;
            j += 1;
        }
    }
    out
}

/// One row of the update body: label at (15, y), value centred at `(width - 110) * 0.5 + 100`.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRow {
    /// "Race", "Gender", "Class", "Face", "Haircut".
    pub label: &'static str,
    /// The value text.
    pub value: String,
    /// Baseline y (27, 57, 87, 117, 147).
    pub y: f32,
}

impl CharacterStyle {
    /// 0x0042bd90: back to the defaults (used by "Select" on "New character").
    pub fn reset(&mut self) {
        *self = CharacterStyle::default();
    }

    /// Runs one arrow callback, then [`CharacterStyle::apply`]; returns the sound.
    pub fn press(&mut self, arrow: Arrow, game: &mut GameView) -> UiAction {
        match arrow {
            Arrow::RacePrev => {
                self.race -= 1;
                if self.race < 0 {
                    self.race = 7;
                }
            }
            Arrow::RaceNext => {
                self.race += 1;
                if 7 < self.race {
                    self.race = 0;
                }
            }
            Arrow::ClassPrev => {
                self.class -= 1;
                if self.class < 0 {
                    self.class = 3;
                }
            }
            Arrow::ClassNext => {
                self.class += 1;
                if 3 < self.class {
                    self.class = 0;
                }
            }
            // `(g + 1) & 0x80000001` with the sign fix-up: `(g + 1) % 2` rounding toward 0.
            Arrow::Gender => self.gender = self.gender.wrapping_add(1) % 2,
            Arrow::FacePrev => self.face -= 1,
            Arrow::FaceNext => self.face += 1,
            Arrow::HairPrev => self.haircut -= 1,
            Arrow::HairNext => self.haircut += 1,
            Arrow::PickColor => self.hair = self.hover,
        }
        self.apply(game);
        UiAction::PlaySound { id: 0x55, volume: 1.0, pitch: 1.0 }
    }

    /// 0x0042c080: writes the style to the local creature under the world lock. Race and
    /// gender give the entity type (`creature+0x64`), class + 1 goes to `+0x140`, face and
    /// haircut wrap inside the type's ranges, and the style object (`creature+0x1d28`) gets
    /// race, gender, face, haircut and the colour. The colour written is the *hovered* one
    /// (`+0x1a3`): the original tests the address `this + 0x1a3` against null, which is never
    /// true, so the `+0x1a0` branch is dead. The appearance is then rebuilt from the style
    /// (0x00428750 defaults, 0x0043f7c0; left to the controller).
    pub fn apply(&mut self, game: &mut GameView) {
        let g = self.gender;
        let ty = match self.race {
            0 => Some(g + 2),
            1 => Some(g),
            2 => Some(g + 9),
            3 => Some(g + 0xb),
            4 => Some(g + 4),
            5 => Some(g + 7),
            6 => Some(g + 0xf),
            7 => Some(g + 0xd),
            _ => None,
        };
        if let Some(t) = ty {
            game.player.0[0x54..0x58].copy_from_slice(&t.to_le_bytes());
        }
        game.player.0[0x130] = (self.class as u8).wrapping_add(1);
        let et = i32::from_le_bytes(game.player.0[0x54..0x58].try_into().unwrap());
        let (f0, f1, h0, h1) = style_ranges(et);
        if self.face < 0 {
            self.face = f1 - f0;
        }
        if (f1 - f0) + 1 <= self.face {
            self.face = 0;
        }
        if self.haircut < 0 {
            self.haircut = h1 - h0;
        }
        if (h1 - h0) + 1 <= self.haircut {
            self.haircut = 0;
        }
        game.style.face = self.face;
        game.style.haircut = self.haircut;
        game.style.hair_color = self.hover;
        game.style.race = self.race;
        game.style.gender = self.gender as u8;
        // 0x0042c18e..: the appearance rebuilt (0x00428750, 0x00459800, 0x0043f7c0), then the
        // starter kit 0x004772b0: both run by the controller ([`starter_kit`]).
        game.style_applied = true;
    }

    /// The rows the update body 0x00428e40 draws.
    pub fn rows(&self) -> Vec<StyleRow> {
        let race = if (0..8).contains(&self.race) { RACE_NAMES[self.race as usize].to_string() } else { String::new() };
        let gender = if self.gender == 0 { "Male" } else { "Female" }.to_string();
        let class = if (0..4).contains(&self.class) { CLASS_NAMES[self.class as usize].to_string() } else { String::new() };
        vec![
            StyleRow { label: "Race", value: race, y: 27.0 },
            StyleRow { label: "Gender", value: gender, y: 57.0 },
            StyleRow { label: "Class", value: class, y: 87.0 },
            StyleRow { label: "Face", value: format!("Face {}", self.face + 1), y: 117.0 },
            StyleRow { label: "Haircut", value: format!("Haircut {}", self.haircut + 1), y: 147.0 },
        ]
    }

    /// The palette part of 0x00428e40: `prev = hover; hover = hair`; a cell under the local
    /// cursor (`x0 <= x < x0 + 17`, `y0 <= y < y0 + 17`, integer cursor) becomes the hover
    /// colour (`(int)(c * 255) & 0xff` per channel); if the hover colour changed, the style
    /// is re-applied (0x0042c080(1)).
    pub fn palette_hover(&mut self, cursor: Vec2, game: &mut GameView) {
        let prev = self.hover;
        self.hover = self.hair;
        let (cx, cy) = (cursor.x as i32, cursor.y as i32);
        for cell in palette() {
            let (x0, y0) = (cell.pos.x as i32, cell.pos.y as i32);
            if x0 <= cx && y0 <= cy && cx < x0 + 0x11 && cy < y0 + 0x11 {
                let b = |v: f32| ((v * 255.0) as i32 & 0xff) as u8;
                self.hover = [b(cell.color[0]), b(cell.color[1]), b(cell.color[2])];
            }
        }
        if self.hover != prev {
            self.apply(game);
        }
    }
}

// ---------------------------------------------------------------------------------------
// The starter kit (`GameController` 0x004772b0, called by apply 0x0042c080)
// ---------------------------------------------------------------------------------------

/// Entity offset of equipment slot `k` (`creature+0x300 + k * 0x118`, `k` = 0..12; the UI's
/// [`GameView::equipment`] holds slots 1..12).
const fn equip_at(k: usize) -> usize {
    0x2f0 + k * ITEM_SIZE
}

/// One equipment slot of the view: slot 0 lives in the player's block only.
fn slot_mut(game: &mut GameView, k: usize) -> &mut [u8] {
    if k == 0 {
        &mut game.player.0[equip_at(0)..equip_at(0) + ITEM_SIZE]
    } else {
        &mut game.equipment[k - 1][..]
    }
}

/// `Item::Item` 0x0042f3e0 with the type (`+0`), sub type (`+1`), material (`+0xd`) and
/// level (`+0x10`) the kit writes.
fn kit_item(ty: u8, sub: u8, material: u8, level: u16) -> Vec<u8> {
    let mut v = super::inventory::empty_item();
    v[0] = ty;
    v[1] = sub;
    v[0xd] = material;
    v[0x10..0x12].copy_from_slice(&level.to_le_bytes());
    v
}

/// `GameController` 0x004772b0: the new character's equipment, bag and recipes for its
/// class (`creature+0x140`, 1 Warrior .. 4 Rogue). Called at the end of every
/// [`CharacterStyle::apply`] (so every style change resets them). In order:
///
/// 1. 0x004772e0..0x004772f8: the thirteen equipment slots (`creature+0x300`) take a fresh
///    `Item` each (0x0043bc00 then 0x0044af00: zeros, level 1).
/// 2. 0x004772fd..0x0047734d: a fresh `Inventory` (0x0043c020) assigned to the bag
///    (`creature+0x11dc`, 0x0044ad30): no tabs, the held stack (`+0x11e8`/`+0x11ec`) and the
///    coins (`+0x1304`/`+0x1308`) zero; then four tabs (0x00487380(4), each reserving 50,
///    0x0044d660).
/// 3. 0x0047741d (jump table 0x004777d8): the class weapon in slot 7 (`creature+0xaa8`, the
///    right hand; drawn on the back while idle, `pose.rs` `weapons_away`), level 1: Warrior
///    sub type 0x11 material 2; Ranger 6 (bow) material 2; Mage 0xa (staff) material 2;
///    Rogue 3 (dagger) material 1 and a second dagger in slot 6 (`+0x990`). The bag gets
///    (`Inventory::addItem(item, -1)` 0x0046ebe0): Warrior two (3, 0) material 1 and one
///    (3, 0xd); Ranger one (3, 7) material 2 and one (3, 0xb) material 2; Mage two (3, 0xc)
///    material 0xb and one (3, 0xb) material 2; Rogue two (3, 4) material 1 and one (3, 5).
/// 4. 0x00477683: slot 9 (`+0xcd8`) a ring (9) material 0xb, slot 8 (`+0xbc0`) a ring
///    material 0xc; five (1, 1) into the bag; slot 10 (`+0xdf0`) the lamp (0x18) material 1.
/// 5. 0x00477761: HP (`creature+0x16c`) = `maxHp` 0x00444db0; the learned formulas
///    (`*(creature+0x1d28)+0x14`) cleared; then 0x0047fae0(1) ([`learn_level_formulas`]).
pub fn starter_kit(game: &mut GameView, rng: &mut cw_math::rand::MsvcRand) {
    use super::inventory::add_item;
    for k in 0..13 {
        let fresh = super::inventory::empty_item();
        slot_mut(game, k).copy_from_slice(&fresh);
    }
    game.inventory.clear();
    game.held = super::ItemStack::empty();
    game.coins = 0;
    game.platinum = 0;
    game.inventory.resize(4, Vec::new());
    let class = game.player.0[0x130];
    // (weapon sub type, material), then the bag items as (count, sub type, material).
    let (weapon, bag): ((u8, u8), &[(u32, u8, u8)]) = match class {
        1 => ((0x11, 2), &[(2, 0, 1), (1, 0xd, 1)]),
        2 => ((6, 2), &[(1, 7, 2), (1, 0xb, 2)]),
        3 => ((0xa, 2), &[(2, 0xc, 0xb), (1, 0xb, 2)]),
        4 => ((3, 1), &[(2, 4, 1), (1, 5, 1)]),
        _ => ((0, 0), &[]),
    };
    if (1..=4).contains(&class) {
        slot_mut(game, 7).copy_from_slice(&kit_item(3, weapon.0, weapon.1, 1));
        if class == 4 {
            slot_mut(game, 6).copy_from_slice(&kit_item(3, 3, 1, 1));
        }
        for &(n, sub, mat) in bag {
            let it = kit_item(3, sub, mat, 1);
            for _ in 0..n {
                add_item(game, &it, -1);
            }
        }
    }
    slot_mut(game, 9).copy_from_slice(&kit_item(9, 0, 0xb, 1));
    slot_mut(game, 8).copy_from_slice(&kit_item(9, 0, 0xc, 1));
    let potion = kit_item(1, 1, 0, 1);
    for _ in 0..5 {
        add_item(game, &potion, -1);
    }
    slot_mut(game, 10).copy_from_slice(&kit_item(0x18, 0, 1, 1));
    // The view's equipment is written into the block by the controller's write-back; maxHp
    // reads the block, so the slots are copied in first.
    for k in 1..13 {
        let o = equip_at(k);
        let src = game.equipment[k - 1].clone();
        game.player.0[o..o + ITEM_SIZE].copy_from_slice(&src);
    }
    let hp = cw_sim::stats::max_hp(&game.player);
    game.player.0[0x15c..0x160].copy_from_slice(&hp.to_le_bytes());
    game.formulas.clear();
    learn_level_formulas(game, 1, rng);
}

/// `GameController` 0x0047fae0(level): the recipes a level teaches, appended to the learned
/// formulas when not known yet (0x00444a90, [`super::inventory::knows_formula`]). The item
/// starts zeroed with level (`+0x10`) = `level`; `level % 4` (MSVC remainder) 1 teaches
/// (1, 1), 3 teaches (2, 1); `level % 5` 1: (1, 4) and (1, 8), 2: (1, 9), 3: (1, 6), 4: (1, 5)
/// (type and sub type as the word `sub << 8 | type`). Level 1 also teaches (0xb, 0x1a),
/// (0xb, 0x16), the class's (0xb, 9) with level 1 and material 0x1b / 0x1a / 0x19 (Rogue,
/// Ranger, Mage; the Mage test is made twice) and (0xb, 0xa) level 1 with materials 1, 2,
/// 0xb, 0xc. Then an armour recipe: the sub type cleared, type 7/4/6/5 by `level % 4`
/// (0/1/2/3), `+4` = `rand() % 11`, level = `level`, material by class 1/0x1a/0x19/0x1b; and
/// a weapon recipe: type 3, `+4` = `rand() % 11`, sub type and material by class and
/// `level % 3` (Warrior 1: `rand() % 3 + 0xf`, `rand() % 3`, 0xd; Ranger 2: 6, 7, 8; Mage:
/// 0xa or 0xb with material 2, or 0xc with material 0xb/0xc by `rand() % 2`; Rogue 1: 3, 5, 4).
/// The class-9 recipes of level 1 go through 0x0044d460 instead of the inline list insert
/// (assumed the same `push_back`).
pub fn learn_level_formulas(game: &mut GameView, level: i32, rng: &mut cw_math::rand::MsvcRand) {
    let mut it = vec![0u8; ITEM_SIZE];
    it[0x10..0x12].copy_from_slice(&(level as u16).to_le_bytes());
    let learn = |game: &mut GameView, it: &[u8]| {
        if !super::inventory::knows_formula(game, it) {
            game.formulas.push(it.to_vec());
        }
    };
    let set_word = |it: &mut Vec<u8>, w: u16| it[0..2].copy_from_slice(&w.to_le_bytes());
    let m4 = level % 4;
    if m4 == 1 {
        set_word(&mut it, 0x101);
        learn(game, &it);
    }
    if m4 == 3 {
        set_word(&mut it, 0x201);
        learn(game, &it);
    }
    let by5: &[u16] = match level % 5 {
        1 => &[0x401, 0x801],
        2 => &[0x901],
        3 => &[0x601],
        4 => &[0x501],
        _ => &[],
    };
    for &w in by5 {
        set_word(&mut it, w);
        learn(game, &it);
    }
    let class = game.player.0[0x130];
    if level == 1 {
        for w in [0x1a0b, 0x160b] {
            set_word(&mut it, w);
            learn(game, &it);
        }
        let class_mats: [(u8, u8); 4] = [(4, 0x1b), (2, 0x1a), (3, 0x19), (3, 0x19)];
        for (c, mat) in class_mats {
            if class == c {
                set_word(&mut it, 0x90b);
                it[0xd] = mat;
                it[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
                learn(game, &it);
            }
        }
        for mat in [1u8, 2, 0xb, 0xc] {
            set_word(&mut it, 0xa0b);
            it[0xd] = mat;
            it[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
            learn(game, &it);
        }
    }
    // The armour recipe: `local_120 &= 0xff` keeps the type byte and clears the sub type.
    it[1] = 0;
    it[0x10..0x12].copy_from_slice(&(level as u16).to_le_bytes());
    let r = rng.rand() % 0xb;
    it[4..8].copy_from_slice(&r.to_le_bytes());
    match m4 {
        0 => it[0] = 7,
        1 => it[0] = 4,
        2 => it[0] = 6,
        3 => it[0] = 5,
        _ => {}
    }
    match class {
        1 => it[0xd] = 1,
        2 => it[0xd] = 0x1a,
        3 => it[0xd] = 0x19,
        4 => it[0xd] = 0x1b,
        _ => {}
    }
    learn(game, &it);
    // The weapon recipe (the sub type byte kept from above unless the class sets it).
    it[0] = 3;
    let r = rng.rand() % 0xb;
    it[4..8].copy_from_slice(&r.to_le_bytes());
    let m3 = level % 3;
    match class {
        1 => {
            it[0xd] = 1;
            match m3 {
                0 => it[1] = (rng.rand() % 3) as u8 + 0xf,
                1 => it[1] = (rng.rand() % 3) as u8,
                2 => it[1] = 0xd,
                _ => {}
            }
        }
        2 => {
            it[0xd] = 2;
            match m3 {
                0 => it[1] = 6,
                1 => it[1] = 7,
                2 => it[1] = 8,
                _ => {}
            }
        }
        3 => match m3 {
            0 => {
                it[1] = 10;
                it[0xd] = 2;
            }
            1 => {
                it[1] = 0xb;
                it[0xd] = 2;
            }
            2 => {
                it[1] = 0xc;
                // `rand() & 0x80000001` with the sign fix-up: odd gives material 0xc.
                it[0xd] = if rng.rand() % 2 != 0 { 0xc } else { 0xb };
            }
            _ => {}
        },
        4 => {
            it[0xd] = 1;
            match m3 {
                0 => it[1] = 3,
                1 => it[1] = 5,
                2 => it[1] = 4,
                _ => {}
            }
        }
        _ => {}
    }
    learn(game, &it);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_rows_and_ranges() {
        let mut game = GameView::default();
        let mut s = CharacterStyle::default();
        s.apply(&mut game);
        // Human male (type 2): default ranges, 6 faces and 15 haircuts.
        assert_eq!(i32::from_le_bytes(game.player.0[0x54..0x58].try_into().unwrap()), 2);
        assert_eq!(game.player.0[0x130], 1);
        s.press(Arrow::FacePrev, &mut game);
        assert_eq!(s.face, 5);
        s.press(Arrow::FaceNext, &mut game);
        assert_eq!(s.face, 0);
        s.press(Arrow::RacePrev, &mut game);
        assert_eq!(s.race, 7);
        s.press(Arrow::Gender, &mut game);
        // Frogman female = 14.
        assert_eq!(i32::from_le_bytes(game.player.0[0x54..0x58].try_into().unwrap()), 14);
        let rows = s.rows();
        assert_eq!(rows[0].value, "Frogman");
        assert_eq!(rows[1].value, "Female");
        assert_eq!(rows[3].value, "Face 1");
        // The palette: 16 x 7 cells, the base colour in row 3.
        let p = palette();
        assert_eq!(p.len(), 112);
        assert_eq!(p[3].color, [1.0, 0.0, 0.0]);
        s.palette_hover(Vec2::new(14.0, 198.0), &mut game);
        assert_eq!(s.hover, [51, 0, 0]);
        s.press(Arrow::PickColor, &mut game);
        assert_eq!(s.hair, [51, 0, 0]);
    }

    #[test]
    fn starter_kit_per_class() {
        let mut rng = cw_math::rand::MsvcRand::new(1);
        // (class byte, slot 7 sub type, slot 7 material, slot 6 type)
        for (class, sub, mat, left) in [(1u8, 0x11u8, 2u8, 0u8), (2, 6, 2, 0), (3, 0xa, 2, 0), (4, 3, 1, 3)] {
            let mut game = GameView::default();
            let mut s = CharacterStyle::default();
            s.class = i32::from(class) - 1;
            s.apply(&mut game);
            assert!(game.style_applied);
            starter_kit(&mut game, &mut rng);
            // UI index 6 = slot 7 (`creature+0xaa8`), 5 = slot 6 (`+0x990`).
            assert_eq!((game.equipment[6][0], game.equipment[6][1], game.equipment[6][0xd]), (3, sub, mat));
            assert_eq!(game.equipment[5][0], left);
            assert_eq!((game.equipment[7][0], game.equipment[7][0xd]), (9, 0xc));
            assert_eq!((game.equipment[8][0], game.equipment[8][0xd]), (9, 0xb));
            assert_eq!(game.equipment[9][0], 0x18);
            // The block carries the same items (the pose reads it).
            assert_eq!(game.player.0[0xa98], 3);
            assert_eq!(game.inventory.len(), 4);
            assert!(game.inventory.iter().flatten().any(|st| st.item[0] == 1 && st.count == 5));
            assert!(!game.formulas.is_empty());
        }
    }
}
