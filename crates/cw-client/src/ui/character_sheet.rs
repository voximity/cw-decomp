//! `cube::CharacterWidget` (vtable 0x006fd5b4, ctor `Cube.exe 0x00434d90`, 0x164 bytes): the
//! character sheet in the 260x200 `blackwidget` panel left of the inventory (`GC+0x800960`,
//! placed at (20, 340) by `onResize`, shown with the inventory, `flow.rs`).
//!
//! Its slot 1, `CharacterWidget::update` `Cube.exe 0x00434e30..0x00439110` (16.7 KB), draws
//! nothing but texts: no shapes, icons or models. Every line is built in one
//! `std::wstringstream` (emptied with `str(L"")` 0x00411b90 / 0x0040f3c0 before each line)
//! and drawn twice with `FontEngine::drawText` 0x00639b30 (font `resource1.dat`, spacing 0,
//! line spacing 2): an outline pass (stroke 3, white fill, black stroke colour) then a fill
//! pass (stroke 0, the line's colour, stroke colour 0). The extrusion colour is 0 in every
//! call. All values are of the local player (`GC+0x8006d0`, `0x00411740`); client creature
//! offsets are the entity block's plus 0x10.
//!
//! | Range | y (x = 10) | Text | Value (at x = 160 unless noted) | Fill |
//! |---|---|---|---|---|
//! | 0x00434eea..0x004350f6 | 20 | the name (`creature+0x1168`, widened by 0x006089c0), size 12, flags 0x10, wrap 180 | | white |
//! | 0x004350fb..0x0043548f | 36 | `"LVL " << (int)level` + class + `" \| "` specialisation (jump table 0x004390f4), size 12 | | (1, 0.5, 0, 1) |
//! | 0x00435494..0x00435ed9 | 56 | "Power", size 10 | `(int)` `0x00445f10` ([`level_cap`]) | (0.3, 0.6, 1, 1) |
//! | 0x00435ede..0x00436432 | 76 | "HP" | `round1(maxHp)` 0x00444db0 ([`cw_sim::stats::max_hp`]) | same |
//! | 0x00436437..0x0043698b | 92 | "ARMOR" | `round1(armor)` 0x0043cff0 ([`cw_sim::combat_ai::armor`]) | same |
//! | 0x00436990..0x00436f0a | 108 | "RESI" | `round1(resistance)` 0x004467a0 ([`cw_sim::combat_ai::resistance`]) | same |
//! | 0x00436eff..0x00437499 | 124 | "CRIT" | `round1(critChance · 100) << "%"` 0x0043e9e0 ([`crit_chance`]) | same |
//! | 0x0043749e..0x00437a28 | 140 | "TEMPO" | `round1(attackSpeed · 100) << "%"` 0x00447700 ([`cw_sim::skills::attack_speed`]) | same |
//! | 0x00437a1d..0x00437fa2 | 156 | "REG" | `round1(regen · 10 / 2^((level - 1) · 0.25) + 100) << "%"`; at x = 210 `"(" << round1(regen) << ")"` (regen 0x00446150, [`regen`]) | same |
//! | 0x00437fa7..0x004386ff | 172 | "Weapon Rating" | `(int)(r · 100 + 0.5) << "%"` ([`weapon_rating`]) | [`rating_color`] |
//! | 0x00438704..0x00438f07 | 188 | "Armor Rating" | `(int)(r · 100 + 0.5) << "%"` ([`armor_rating`]) | [`rating_color`] |
//!
//! Numbers go through `wostream <<`: `int` (`??6...@H@Z`) for the level, power and the two
//! ratings, `float` (`??6...@M@Z`, `%g`, [`fmt_g6`]) for the rest. `round1` is 0x00439110
//! ([`round1`]).

use cw_net::EntityData;
use glam::Vec2;

use super::hud::{class_suffix, name_of};
use super::inventory::{cvtt, is_two_handed, level_cap};
use super::item_description::{item_regen, round1};
use crate::net::fmt_g6;

/// The creature state outside the entity block that the sheet reads (from the local
/// player's `CreatureState`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SheetState {
    /// `creature+0x1190` (`CreatureState::block`): `critChance` adds `0.15 ×` it.
    pub guard: f32,
    /// A type-0xc buff is held (`attackSpeed` 0x00447700).
    pub haste: bool,
    /// A type-0xb buff is held (`critChance` 0x0043e9e0 is then 1).
    pub crit_buff: bool,
}

impl SheetState {
    /// From the local player's `CreatureState` (none: all zero).
    pub fn of(st: Option<&cw_sim::combat::CreatureState>) -> Self {
        st.map_or_else(Self::default, |s| SheetState {
            guard: s.block,
            haste: cw_sim::util::has_buff(s, 0xc),
            crit_buff: cw_sim::util::has_buff(s, 0xb),
        })
    }
}

/// One `drawText` 0x00639b30 call, widget-local.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetText {
    /// The text.
    pub text: String,
    /// Pen position.
    pub pos: Vec2,
    /// Font size.
    pub size: f32,
    /// Stroke radius (3 for the outline pass, 0 for the fill pass).
    pub stroke: f32,
    /// Fill colour.
    pub color: [f32; 4],
    /// Stroke colour.
    pub stroke_color: [f32; 4],
    /// Flags (0x10 wraps at [`SheetText::wrap`]).
    pub flags: u32,
    /// Wrap width (-1: none).
    pub wrap: f32,
}

/// Every call's line spacing (the fourth `drawText` argument, 2.0).
pub const LINE_SPACING: f32 = 2.0;

const WHITE: [f32; 4] = [1.0; 4];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const NONE: [f32; 4] = [0.0; 4];
/// The LVL line's fill (1, 0.5, 0, 1).
const ORANGE: [f32; 4] = [1.0, 0.5, 0.0, 1.0];
/// The values' fill (0x3e99999a, 0x3f19999a, 1, 1).
const VALUE: [f32; 4] = [0.3, 0.6, 1.0, 1.0];

/// The equipment item of `slot` (`entity+0x2f0 + slot × 0x118`).
fn slot(e: &EntityData, s: usize) -> &[u8] {
    let o = 0x2f0 + s * cw_net::entity::ITEM_SIZE;
    &e.0[o..o + cw_net::entity::ITEM_SIZE]
}

fn level(e: &EntityData) -> i32 {
    i32::from_le_bytes(e.0[0x180..0x184].try_into().unwrap())
}

/// The item's `(float)(i16)Item+0x10` level and `Item+0xc` rarity (`movswl` / `movzbl`).
fn item_level_rarity(item: &[u8]) -> (f32, i32) {
    (f32::from(i16::from_le_bytes([item[0x10], item[0x11]])), i32::from(item[0xc]))
}

/// `0x00445f60(level, rarity)` = [`cw_sim::stats::item_power`] (the same code).
fn item_power(level: f32, rarity: i32) -> f32 {
    cw_sim::stats::item_power(level, rarity)
}

/// `0x0043e9e0` (`critChance`): 1 with a type-0xb buff, else `0x0043ea40` (the level curve
/// `/ 8 · 0.1` plus each slot's `0x004c6ba0`, = `Server.exe 0x00409ac0`,
/// [`cw_sim::modes::block_chance`]) `+ creature+0x1190 × 0.15`.
pub fn crit_chance(e: &EntityData, st: &SheetState) -> f32 {
    if st.crit_buff {
        return 1.0;
    }
    cw_sim::modes::block_chance(e) + st.guard * 0.15f32
}

/// `0x00446150`: the sum of `0x004c78c0` ([`item_regen`]) over slots 6 and 7 (type 3), 2 (4),
/// 3 (6), 4 (5), 5 (7), 1 (8), 8 and 9 (9), each counted when it holds that type.
pub fn regen(e: &EntityData) -> f32 {
    let mut v = 0.0f32;
    for (s, ty) in [(6usize, 3u8), (7, 3), (2, 4), (3, 6), (4, 5), (5, 7), (1, 8), (8, 9), (9, 9)] {
        let it = slot(e, s);
        if it[0] == ty {
            v = item_regen(it) + v;
        }
    }
    v
}

/// 0x004382b8..0x00438425: `itemPower(level, rarity) / itemPower(creature level, 0)` of the
/// right hand (slot 6) then `+` that of the left hand (slot 7), each when a weapon (type 3);
/// halved unless the left hand holds a two-handed weapon (`0x00444230`: slot 7 type 3 and
/// `Item::isTwoHanded`).
pub fn weapon_rating(e: &EntityData) -> f32 {
    let base = || item_power(level(e) as f32, 0);
    let mut r = 0.0f32;
    let right = slot(e, 6);
    if right[0] == 3 {
        let (l, q) = item_level_rarity(right);
        r = item_power(l, q) / base() + 0.0f32;
    }
    let left = slot(e, 7);
    if left[0] == 3 {
        let (l, q) = item_level_rarity(left);
        r = item_power(l, q) / base() + r;
    }
    if !is_two_handed(left) {
        r = r * 0.5f32;
    }
    r
}

/// 0x00438a9c..0x00438d29: the same ratio of the shoulders (slot 5, type 7), the chest
/// (slot 2, type 4, doubled before the division), the boots (slot 4, type 5) and the gloves
/// (slot 3, type 6), summed in that order, over 5.
pub fn armor_rating(e: &EntityData) -> f32 {
    let base = || item_power(level(e) as f32, 0);
    let mut a = 0.0f32;
    for (s, ty, k) in [(5usize, 7u8, None), (2, 4, Some(2.0f32)), (4, 5, None), (3, 6, None)] {
        let it = slot(e, s);
        if it[0] == ty {
            let (l, q) = item_level_rarity(it);
            let mut p = item_power(l, q);
            if let Some(k) = k {
                p = p * k;
            }
            a = p / base() + a;
        }
    }
    a / 5.0f32
}

/// 0x0043845a..0x00438536 (and 0x00438d29..0x00438e0d): the rating's fill. Each test is
/// `comiss r, c; jbe` (`c > r`), so a NaN falls through to the last colour.
pub fn rating_color(r: f32) -> [f32; 4] {
    if 0.8f32 > r {
        [0.7, 0.7, 0.7, 1.0]
    } else if 1.1f32 > r {
        [1.0, 1.0, 1.0, 1.0]
    } else if 1.2f32 > r {
        [0.0, 1.0, 0.0, 1.0]
    } else if 1.5f32 > r {
        [0.25, 0.25, 1.0, 1.0]
    } else if 1.8f32 > r {
        [0.5, 0.0, 1.0, 1.0]
    } else {
        [1.0, 1.0, 0.0, 1.0]
    }
}

/// `float` through `wostream <<` after `round1`.
fn g(x: f32) -> String {
    fmt_g6(f64::from(round1(x)))
}

/// The specialisation suffix of the LVL line (`creature+0x141`: 0 first, 1 second; others
/// nothing).
fn spec_suffix(class: u8, spec: u8) -> &'static str {
    let (a, b) = match class {
        1 => (" | Berserker", " | Guardian"),
        2 => (" | Sniper", " | Scout"),
        3 => (" | Fire", " | Water"),
        4 => (" | Assassin", " | Ninja"),
        _ => return "",
    };
    match spec {
        0 => a,
        1 => b,
        _ => "",
    }
}

/// `CharacterWidget::update` 0x00434e30: every `drawText` call of the frame, in order.
pub fn texts(p: &EntityData, st: &SheetState) -> Vec<SheetText> {
    let mut v = Vec::with_capacity(42);
    let mut pair = |text: &str, x: i32, y: i32, size: f32, fill: [f32; 4], flags: u32, wrap: f32| {
        let pos = Vec2::new(x as f32, y as f32);
        v.push(SheetText { text: text.to_string(), pos, size, stroke: 3.0, color: WHITE, stroke_color: BLACK, flags, wrap });
        v.push(SheetText { text: text.to_string(), pos, size, stroke: 0.0, color: fill, stroke_color: NONE, flags, wrap });
    };
    // 0x00434e5a: the pen, x = 10 (`[ebp-0x50]`), y = 20 (`[ebp-0x4c]`).
    let x = 10;
    let mut y = 20;
    // 0x00434eea..0x004350f6: the name, size 12, flags 0x10, wrap 180.
    pair(&name_of(p), x, y, 12.0, WHITE, 0x10, 180.0);
    y += 0x10;
    // 0x004350fb..0x0043548f: "LVL " << level, class, specialisation; size 12, orange.
    let (class, spec) = (p.0[0x130], p.0[0x131]);
    let lvl = format!("LVL {}{}{}", level(p), class_suffix(class), spec_suffix(class, spec));
    pair(&lvl, x, y, 12.0, ORANGE, 0, -1.0);
    y += 0x14;
    // 0x00435494..0x00435ed9: "Power", value `0x00445f10` at x + 150.
    pair("Power", x, y, 10.0, WHITE, 0, -1.0);
    pair(&level_cap(level(p)).to_string(), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x14;
    // 0x00435ede..0x00436432: "HP", round1(maxHp).
    pair("HP", x, y, 10.0, WHITE, 0, -1.0);
    pair(&g(cw_sim::stats::max_hp(p)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x00436437..0x0043698b: "ARMOR", round1(armor).
    pair("ARMOR", x, y, 10.0, WHITE, 0, -1.0);
    pair(&g(cw_sim::combat_ai::armor(p)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x00436990..0x00436efa: "RESI", round1(resistance).
    pair("RESI", x, y, 10.0, WHITE, 0, -1.0);
    pair(&g(cw_sim::combat_ai::resistance(p)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x00436eff..0x00437499: "CRIT", round1(critChance × 100) "%".
    pair("CRIT", x, y, 10.0, WHITE, 0, -1.0);
    pair(&format!("{}%", g(crit_chance(p, st) * 100.0f32)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x0043749e..0x00437a18: "TEMPO", round1(attackSpeed × 100) "%".
    pair("TEMPO", x, y, 10.0, WHITE, 0, -1.0);
    let tempo = cw_sim::skills::attack_speed(p, st.guard, st.haste);
    pair(&format!("{}%", g(tempo * 100.0f32)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x00437a1d..0x00437fa2: "REG", round1(regen × 10 / pow(2, (level - 1) × 0.25) + 100)
    // "%" (0x00411d10 is `(float)pow((double)2, (double)e)`), then "(" round1(regen) ")" at
    // x + 200.
    pair("REG", x, y, 10.0, WHITE, 0, -1.0);
    let r = regen(p) * 10.0f32;
    let d = cw_sim::stats::pow(2.0, f64::from((level(p) - 1) as f32 * 0.25f32)) as f32;
    pair(&format!("{}%", g(r / d + 100.0f32)), x + 0x96, y, 10.0, VALUE, 0, -1.0);
    pair(&format!("({})", g(regen(p))), x + 0xc8, y, 10.0, VALUE, 0, -1.0);
    y += 0x10;
    // 0x00437fa7..0x004386ff: "Weapon Rating", (int)(r × 100 + 0.5) "%" in the rating colour.
    pair("Weapon Rating", x, y, 10.0, WHITE, 0, -1.0);
    let wr = weapon_rating(p);
    pair(&format!("{}%", cvtt(wr * 100.0f32 + 0.5f32)), x + 0x96, y, 10.0, rating_color(wr), 0, -1.0);
    y += 0x10;
    // 0x00438704..0x00438f07: "Armor Rating", the same.
    pair("Armor Rating", x, y, 10.0, WHITE, 0, -1.0);
    let ar = armor_rating(p);
    pair(&format!("{}%", cvtt(ar * 100.0f32 + 0.5f32)), x + 0x96, y, 10.0, rating_color(ar), 0, -1.0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: usize = 0x118;

    fn player(level: i32, class: u8, spec: u8) -> EntityData {
        let mut e = EntityData::new_creature();
        e.0[0x180..0x184].copy_from_slice(&level.to_le_bytes());
        e.0[0x130] = class;
        e.0[0x131] = spec;
        e.0[0x1158..0x115c].copy_from_slice(b"Bob\0");
        e
    }

    fn put_item(e: &mut EntityData, slot: usize, ty: u8, sub: u8, level: i16, rarity: u8) {
        let o = 0x2f0 + slot * ITEM;
        e.0[o..o + ITEM].fill(0);
        e.0[o] = ty;
        e.0[o + 1] = sub;
        e.0[o + 0xc] = rarity;
        e.0[o + 0x10..o + 0x12].copy_from_slice(&level.to_le_bytes());
    }

    fn fill_pass<'a>(t: &'a [SheetText], text: &str) -> &'a SheetText {
        t.iter().filter(|t| t.text == text).nth(1).unwrap_or_else(|| panic!("no fill pass of {text:?}"))
    }

    #[test]
    fn layout_and_order() {
        let e = player(1, 1, 0);
        let t = texts(&e, &SheetState::default());
        // 2 header lines + 7 rows of label and value + REG's second value + 2 ratings, two
        // passes each.
        assert_eq!(t.len(), 2 * (2 + 7 * 2 + 1 + 2 * 2));
        let order: Vec<&str> = t.iter().step_by(2).map(|t| t.text.as_str()).collect();
        assert_eq!(order[0], "Bob");
        assert_eq!(order[1], "LVL 1 Warrior | Berserker");
        assert_eq!(order[2], "Power");
        assert_eq!(order[3], "1");
        assert_eq!(order[4], "HP");
        assert_eq!(order[6], "ARMOR");
        assert_eq!(order[8], "RESI");
        assert_eq!(order[10], "CRIT");
        assert_eq!(order[11], "1.3%");
        assert_eq!(order[12], "TEMPO");
        assert_eq!(order[14], "REG");
        assert_eq!(order[15], "100%");
        assert_eq!(order[16], "(0)");
        assert_eq!(order[17], "Weapon Rating");
        assert_eq!(order[18], "0%");
        assert_eq!(order[19], "Armor Rating");
        assert_eq!(order[20], "0%");
        // Name: (10, 20), size 12, flags 0x10 wrapped at 180; outline pass then fill pass.
        assert_eq!(t[0].pos, Vec2::new(10.0, 20.0));
        assert_eq!((t[0].size, t[0].stroke, t[0].flags, t[0].wrap), (12.0, 3.0, 0x10, 180.0));
        assert_eq!(t[0].stroke_color, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!((t[1].stroke, t[1].color, t[1].stroke_color), (0.0, [1.0; 4], [0.0; 4]));
        // LVL line (10, 36), orange fill.
        assert_eq!(t[2].pos, Vec2::new(10.0, 36.0));
        assert_eq!((t[3].color, t[3].flags, t[3].wrap), ([1.0, 0.5, 0.0, 1.0], 0, -1.0));
        // Rows: y 56, 76, 92, 108, 124, 140, 156, 172, 188; values at x 160 in (0.3, 0.6, 1).
        let ys = [56.0, 76.0, 92.0, 108.0, 124.0, 140.0, 156.0, 172.0, 188.0];
        for (k, label) in ["Power", "HP", "ARMOR", "RESI", "CRIT", "TEMPO", "REG", "Weapon Rating", "Armor Rating"].iter().enumerate() {
            let l = fill_pass(&t, label);
            assert_eq!(l.pos, Vec2::new(10.0, ys[k]), "{label}");
            assert_eq!((l.size, l.color), (10.0, [1.0; 4]), "{label}");
        }
        let v = &t[7];
        assert_eq!(v.pos, Vec2::new(160.0, 56.0));
        assert_eq!(v.color, [0.3, 0.6, 1.0, 1.0]);
        assert_eq!(t[33].pos, Vec2::new(210.0, 156.0));
        // Ratings: grey below 0.8.
        assert_eq!(t[37].color, [0.7, 0.7, 0.7, 1.0]);
        assert_eq!(t[37].pos, Vec2::new(160.0, 172.0));
        assert_eq!(t[41].pos, Vec2::new(160.0, 188.0));
    }

    #[test]
    fn stats_from_cw_sim() {
        let mut e = player(10, 3, 1);
        put_item(&mut e, 2, 4, 0, 10, 2);
        let st = SheetState { guard: 0.0, haste: false, crit_buff: false };
        let t = texts(&e, &st);
        let g = |x: f32| crate::net::fmt_g6(f64::from(crate::ui::item_description::round1(x)));
        assert_eq!(t[2].text, "LVL 10 Mage | Water");
        assert_eq!(t[6].text, crate::ui::inventory::level_cap(10).to_string());
        assert_eq!(t[10].text, g(cw_sim::stats::max_hp(&e)));
        assert_eq!(t[14].text, g(cw_sim::combat_ai::armor(&e)));
        assert_eq!(t[18].text, g(cw_sim::combat_ai::resistance(&e)));
        assert_eq!(t[22].text, format!("{}%", g(cw_sim::modes::block_chance(&e) * 100.0)));
        assert_eq!(t[26].text, format!("{}%", g(cw_sim::skills::attack_speed(&e, 0.0, false) * 100.0)));
        // The crit buff makes the chance 1.
        let t = texts(&e, &SheetState { crit_buff: true, ..st });
        assert_eq!(t[22].text, "100%");
    }

    #[test]
    fn weapon_and_armor_rating() {
        let mut e = player(1, 1, 0);
        // One one-handed weapon of the player's level: 1 / 1, halved.
        put_item(&mut e, 6, 3, 0, 1, 0);
        let t = texts(&e, &SheetState::default());
        assert_eq!(t[36].text, "50%");
        assert_eq!(t[37].color, [0.7, 0.7, 0.7, 1.0]);
        // A two-handed weapon in the left hand (slot 7): not halved.
        put_item(&mut e, 6, 0, 0, 0, 0);
        put_item(&mut e, 7, 3, 0xf, 1, 0);
        let t = texts(&e, &SheetState::default());
        assert_eq!(t[36].text, "100%");
        assert_eq!(t[37].color, [1.0; 4]);
        // Armour: chest counts twice, gloves, boots and shoulders once, over 5.
        for (s, ty) in [(2, 4), (3, 6), (4, 5), (5, 7)] {
            put_item(&mut e, s, ty, 0, 1, 0);
        }
        let t = texts(&e, &SheetState::default());
        assert_eq!(t[40].text, "100%");
    }

    /// The sheet reaches the renderer's widget texts only while its panel is shown.
    #[test]
    fn widget_texts_follow_panel() {
        use std::collections::BTreeMap;
        let mut gui = cw_ui::widget::Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let mut game = crate::ui::GameView::default();
        game.player = player(3, 2, 1);
        let mut ui = crate::ui::GameUi::new(gui, &mut crate::ui::members::NoPlx, &game);
        let w = ui.m.character.unwrap();
        let p = ui.gui.parent_widget(w).unwrap();
        let n = ui.gui.widgets[p].node;
        let out = crate::ui::FrameOutput::default();
        ui.gui.nodes[n].visible = false;
        let mut t = BTreeMap::new();
        crate::ui::present_panels::widget_texts(&ui, &game, &out, &mut t);
        assert!(!t.contains_key(&w));
        ui.gui.nodes[n].visible = true;
        crate::ui::present_panels::widget_texts(&ui, &game, &out, &mut t);
        let v = &t[&w];
        assert_eq!(v.len(), 42);
        assert_eq!(String::from_utf16_lossy(&v[2].text), "LVL 3 Ranger | Scout");
        assert_eq!(v[0].style.line_spacing, 2.0);
        assert_eq!((v[0].style.flags, v[0].style.wrap_width), (0x10, 180.0));
    }

    #[test]
    fn rating_colors() {
        assert_eq!(rating_color(0.79), [0.7, 0.7, 0.7, 1.0]);
        assert_eq!(rating_color(0.8), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(rating_color(1.1), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(rating_color(1.2), [0.25, 0.25, 1.0, 1.0]);
        assert_eq!(rating_color(1.5), [0.5, 0.0, 1.0, 1.0]);
        assert_eq!(rating_color(1.8), [1.0, 1.0, 0.0, 1.0]);
        // `comiss` + `jbe`: NaN skips every branch to the last colour.
        assert_eq!(rating_color(f32::NAN), [1.0, 1.0, 0.0, 1.0]);
    }
}
