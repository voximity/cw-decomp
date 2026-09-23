//! The item description block `GameController 0x004a28c0(Item*, x, y, scale, width, name,
//! classLine, unused)` (11.6 KB, `Cube.exe`): the lines under the item preview, in the
//! "Currently equipped" comparison blocks, and in the identification and adaption panels.
//!
//! Tier B. Every line is built in one `std::wstringstream` (emptied with `str(L"")` before
//! each line) and drawn twice with `FontEngine::drawText` 0x00639b30 (font `resource1.dat`):
//! an outline pass then a fill pass. No text-database markup is involved: the labels are
//! literals of the function, numbers go through `wostream <<` (`int`, `short`, `unsigned`
//! as decimal, `float` as `%g`, [`crate::net::fmt_g6`]); only the item name comes from the
//! dictionary (`World::itemName` 0x00598a50, [`crate::names::item_name`]).
//!
//! # Map
//!
//! | Range | Line | Value |
//! |---|---|---|
//! | 0x004a2934..0x004a2a55 | the name (only with `name`): `"Pet food: "` (type 0x14), the name, `" +" << level` (not for the count types, not for pets) | `0x004c76a0` ([`item_level`]); colour `0x004c7d20` ([`rarity_color`]) |
//! | 0x004a2b8b..0x004a2d95 | count types: `(i16)Item+0x10 << " X"` when above 1; pets `"LVL " << (i16)Item+0x10`; else `"Power " << level`, red when above the player's (`0x00445f10`, [`level_cap`]); `" (adapted)"` for `Item+0xe & 1` | |
//! | 0x004a2e6e..0x004a3287 | only with `classLine`: the classes that may use the item in red when the player's may not (`0x004c6f20`, [`class_allows`]); a known formula (`0x00444a90` on the product copy): "Already known" in red | |
//! | 0x004a3345..0x004a37c1 | pets: `"XP " << (u32)Item+4 << "/" << (int)(levelCurve(level) · 1000 + 50)`; "Ridable" (`0x00444760` on the sub type) | `levelCurve` 0x0043ca60 ([`level_fraction`]) |
//! | 0x004a37c6..0x004a3a53 | weapons: `"DMG " << round1(weaponStrength)` | `cube::Item::weaponStrength` 0x004c7f60 = [`cw_sim::modes::item_weapon_power`] |
//! | 0x004a3a58..0x004a3ce5 | `"HP " << round1(v)` when `v >= 0.1` | `itemHpBonus` 0x004c70b0 = [`cw_sim::stats::item_hp_bonus`] |
//! | 0x004a3cea..0x004a3f77 | `"ARMOR " << round1(v)` | `cube::Item::armor` 0x004c6a90 = [`cw_sim::combat_ai::item_armor`] |
//! | 0x004a3f7c..0x004a4209 | `"RESI " << round1(v)` | `cube::Item::resistance` 0x004c7af0 = [`cw_sim::combat_ai::item_resist`] |
//! | 0x004a420e..0x004a44ba | `"TEMPO " << round1(v · 100) << "%"` when `v >= 0.001` | `itemAttackSpeed` 0x004c7c00 = [`cw_sim::skills::item_attack_speed`] |
//! | 0x004a44bf..0x004a4ea2 | `"CRIT " << round1(v · 100) << "%"` when `v >= 0.001` | `cube::Item::critChance` 0x004c6ba0 = [`cw_sim::modes::item_chance`] |
//! | 0x004a4ea7..0x004a51ea | `"REG " << round1(v)` when `v >= 0.1` | unnamed 0x004c78c0 ([`item_regen`], not in cw-sim) |
//! | 0x004a51ef..0x004a56da | consumables (type 1): `"HP +" << (int)v` when `v >= 0.1` | `cube::Item::healAmount` 0x004c6e10 = [`cw_sim::interact::heal_amount`] |
//!
//! `round1` is 0x00439110 ([`round1`]). Each stat test is `comiss` + `jb` against the
//! threshold (`!(v < t)`: a NaN draws).

use glam::Vec2;

use super::inventory::{class_allows, cvtt, item_level, knows_formula, level_cap, level_fraction};
use super::inventory_widget::rarity_color;
use super::textdb::TextDb;
use super::GameView;
use crate::net::fmt_g6;

/// The arguments of `0x004a28c0` after the item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DescriptionArgs {
    /// `x` (int, converted to float for every draw).
    pub x: i32,
    /// `y`, the top of the block.
    pub y: i32,
    /// `scale`: font sizes and line advances are multiplied by it (1 for every caller).
    pub scale: f32,
    /// `width`: the wrap width of the name, level and class lines (`0x10` flag).
    pub width: i32,
    /// Draw the name line (`[ebp+0x1c]`).
    pub name: bool,
    /// Draw the class / "Already known" line (`[ebp+0x20]`).
    pub class_line: bool,
}

/// One `drawText` 0x00639b30 call of the block, in the caller's coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct DescText {
    /// The text.
    pub text: String,
    /// Pen position.
    pub pos: Vec2,
    /// Font size.
    pub size: f32,
    /// Stroke radius (0 for the fill pass).
    pub stroke: f32,
    /// Line spacing (3 for the name, 2 for the level and class lines, 0 for the stats).
    pub line_spacing: f32,
    /// Alignment / wrap flags (`0x10` wraps at [`DescText::wrap`]).
    pub flags: u32,
    /// Wrap width (-1: none).
    pub wrap: f32,
    /// Fill colour.
    pub color: [f32; 4],
    /// Stroke colour.
    pub stroke_color: [f32; 4],
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const NONE: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
/// The DMG / HP / ARMOR / RESI and "Ridable" fill (0.75, 0.5, 1, 1).
const LILAC: [f32; 4] = [0.75, 0.5, 1.0, 1.0];
/// TEMPO, CRIT and REG (0, 1, 1, 1).
const CYAN: [f32; 4] = [0.0, 1.0, 1.0, 1.0];
/// The pet XP line (0.5, 1, 1, 1).
const PALE_CYAN: [f32; 4] = [0.5, 1.0, 1.0, 1.0];
/// "HP +" (0.25, 1, 0.25, 1).
const GREEN: [f32; 4] = [0.25, 1.0, 0.25, 1.0];

/// `0x00439110`: `x < 0` (`comiss 0, x; jbe`, so NaN takes the positive path) mirrors the
/// positive branch and folds `-0` into `+0`; otherwise `(float)(int)(x * 10 + 0.5) * 0.1`
/// (cvttss2si).
pub fn round1(x: f32) -> f32 {
    if 0.0f32 > x {
        let r = -round1(-x);
        if r == 0.0 {
            return 0.0;
        }
        return r;
    }
    (cvtt(x * 10.0f32 + 0.5f32) as f32) * 0.1f32
}

/// `0x004c78c0` (unnamed; the "REG" line): for types 3..7, `itemPower(spirits · 0.1 + level,
/// rarity) · k · m` with `k` 0.1 (0.2 for chest armour, type 4) and `m = ((modifier << 3) %
/// 21) / 20` (unsigned, through a double), plus 0.5 for material 0x1a and 1 for 0x1b.
pub fn item_regen(item: &[u8]) -> f32 {
    let t = item[0];
    if !matches!(t, 3 | 4 | 7 | 5 | 6) {
        return 0.0;
    }
    let k = if t == 4 { 0.2f32 } else { 0.1f32 };
    let modifier = u32::from_le_bytes(item[4..8].try_into().unwrap());
    let r = modifier.wrapping_shl(3) % 0x15;
    let mut m = (f64::from(r) as f32) / 20.0f32;
    match item[0xd] {
        0x1a => m = m + 0.5f32,
        0x1b => m = m + 1.0f32,
        _ => {}
    }
    let spirits = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap()) as f32;
    let level = f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
    cw_sim::stats::item_power(spirits * 0.1f32 + level, i32::from(item[0xc])) * k * m
}

/// The "count" types of the level line (0x004a2b98..0x004a2c1b): coins (0xc), platinum (0xd),
/// 0x15, the blocks of type 0xb except sub type 0xe, 0, 0x19, pet food (0x14), 0x18 (lamps)
/// and 0x17. The name line (0x004a29c5..) also leaves out the level of pets (0x13).
fn count_type(item: &[u8]) -> bool {
    let t = item[0];
    t == 0xc || t == 0xd || t == 0x15 || (t == 0xb && item[1] != 0xe) || t == 0 || t == 0x19 || t == 0x14 || t == 0x18 || t == 0x17
}

/// The block builder: the stream's text drawn twice per line.
struct Block {
    out: Vec<DescText>,
    x: f32,
}

impl Block {
    #[allow(clippy::too_many_arguments)]
    fn pair(&mut self, text: &str, y: f32, size: f32, stroke: f32, line_spacing: f32, flags: u32, wrap: f32, outline: [f32; 4], outline_stroke: [f32; 4], fill: [f32; 4]) {
        let d = |stroke: f32, color: [f32; 4], stroke_color: [f32; 4]| DescText {
            text: text.to_string(),
            pos: Vec2::new(self.x, y),
            size,
            stroke,
            line_spacing,
            flags,
            wrap,
            color,
            stroke_color,
        };
        let a = d(stroke, outline, outline_stroke);
        let b = d(0.0, fill, NONE);
        self.out.push(a);
        self.out.push(b);
    }

    /// A stat line: size `scale * 10`, outline 2 (white on black), spacing 0, flags 0, no wrap.
    fn stat(&mut self, text: &str, y: f32, scale: f32, fill: [f32; 4]) {
        self.pair(text, y, scale * 10.0f32, 2.0, 0.0, 0, -1.0, WHITE, BLACK, fill);
    }
}

/// `0x004a28c0`: the draws of the block for `item`, from the top-left `(x, y)` of `a`. `db` is
/// the text database for the name; `game` gives the local player (`GC+0x8006d0`: level
/// `entity+0x180`, class `entity+0x130`) and the learned formulas.
pub fn describe(db: Option<&TextDb>, game: &GameView, item: &[u8], a: &DescriptionArgs) -> Vec<DescText> {
    let mut b = Block { out: Vec::new(), x: a.x as f32 };
    let s = a.scale;
    let wrap = a.width as f32;
    let mut y = a.y as f32;
    let player_level = i32::from_le_bytes(game.player.0[0x180..0x184].try_into().unwrap());
    let class = i32::from(game.player.0[0x130]);
    // 0x004a28f8: `0x004c7d20`, the name colour.
    let name_color = rarity_color(item);
    // 0x004a2934: the name (built even when it is not drawn).
    let mut line = String::new();
    if item[0] == 0x14 {
        line.push_str("Pet food: ");
    }
    line.push_str(&crate::names::item_name(db, item));
    if !count_type(item) && item[0] != 0x13 {
        line.push_str(&format!(" +{}", item_level(item)));
    }
    if a.name {
        // 0x004a2a5a..0x004a2b44: size `scale * 10`, line spacing 3, outline 3 white on black,
        // then the rarity colour; wrap `0x10` at `width`.
        b.pair(&line, y, s * 10.0f32, 3.0, 3.0, 0x10, wrap, WHITE, BLACK, name_color);
    }
    // 0x004a2b8b: the level line.
    y = s * 30.0f32 + y;
    let mut color = WHITE;
    let mut line = String::new();
    if count_type(item) {
        let n = i16::from_le_bytes([item[0x10], item[0x11]]);
        if n > 1 {
            line = format!("{n} X");
        }
    } else {
        if item[0] == 0x13 {
            line = format!("LVL {}", i16::from_le_bytes([item[0x10], item[0x11]]));
        } else {
            line = format!("Power {}", item_level(item));
        }
        // 0x004a2c8a: red when the item's level is above the player's (`0x00445f10`).
        if item_level(item) > level_cap(player_level) {
            color = RED;
        }
    }
    if item[0xe] & 1 != 0 {
        line.push_str(" (adapted)");
    }
    // 0x004a2d9a..0x004a2e69: size `scale * 9`, line spacing 2, outline 3 in the line colour on
    // black, then the fill in the same colour.
    let small = s * 9.0f32;
    b.pair(&line, y, small, 3.0, 2.0, 0x10, wrap, color, BLACK, color);
    if a.class_line {
        y = s * 14.0f32 + y;
        if !class_allows(item, class) {
            // 0x004a2ec8: the classes that may use it.
            let mut line = String::new();
            for (k, n) in [(1, "Warrior "), (2, "Ranger "), (3, "Mage "), (4, "Rogue ")] {
                if class_allows(item, k) {
                    line.push_str(n);
                }
            }
            b.pair(&line, y, small, 3.0, 2.0, 0x10, wrap, WHITE, BLACK, RED);
        } else if item[0] == 2 {
            // 0x004a3107: the product copy (`type = Item+8`, `+8 = 0`) in the learned list.
            let mut f = item.to_vec();
            f[0] = item[8];
            f[8..0xc].fill(0);
            if knows_formula(game, &f) {
                y = s * 27.0f32 + y;
                b.pair("Already known", y, small, 3.0, 2.0, 0x10, wrap, WHITE, BLACK, RED);
            }
        }
    }
    // 0x004a32f8: the stats start 16 lower.
    let l16 = s * 16.0f32;
    y = y + l16;
    let l14 = s * 14.0f32;
    if item[0] == 0x13 {
        // 0x004a3345: `(int)(levelCurve((float)(i16)level) * 1000 + 50)`.
        let lv = f32::from(i16::from_le_bytes([item[0x10], item[0x11]]));
        let max = cvtt(level_fraction(lv) * 1000.0f32 + 50.0f32);
        let xp = u32::from_le_bytes(item[4..8].try_into().unwrap());
        b.stat(&format!("XP {xp}/{max}"), y, s, PALE_CYAN);
        y = y + l14;
        if crate::interact::ridable_type(i32::from(item[1])) {
            b.stat("Ridable", y, s, LILAC);
            y = y + l14;
        }
    }
    if item[0] == 3 {
        let v = round1(cw_sim::modes::item_weapon_power(item));
        b.stat(&format!("DMG {}", fmt_g6(f64::from(v))), y, s, LILAC);
        y = s * 14.0f32 + y;
    }
    let v = cw_sim::stats::item_hp_bonus(item);
    if !(v < 0.1f32) {
        b.stat(&format!("HP {}", fmt_g6(f64::from(round1(v)))), y, s, LILAC);
        y = s * 14.0f32 + y;
    }
    let v = cw_sim::combat_ai::item_armor(item);
    if !(v < 0.1f32) {
        b.stat(&format!("ARMOR {}", fmt_g6(f64::from(round1(v)))), y, s, LILAC);
        y = y + l16;
    }
    let v = cw_sim::combat_ai::item_resist(item);
    if !(v < 0.1f32) {
        b.stat(&format!("RESI {}", fmt_g6(f64::from(round1(v)))), y, s, LILAC);
        y = y + l16;
    }
    let v = cw_sim::skills::item_attack_speed(item);
    if !(v < 0.001f32) {
        b.stat(&format!("TEMPO {}%", fmt_g6(f64::from(round1(v * 100.0f32)))), y, s, CYAN);
        y = y + l16;
    }
    let v = cw_sim::modes::item_chance(item);
    if !(v < 0.001f32) {
        b.stat(&format!("CRIT {}%", fmt_g6(f64::from(round1(v * 100.0f32)))), y, s, CYAN);
        y = y + l16;
    }
    let v = item_regen(item);
    if !(v < 0.1f32) {
        // 0x004a4f9a: the outline pass is white here too, the fill cyan.
        b.stat(&format!("REG {}", fmt_g6(f64::from(round1(v)))), y, s, CYAN);
        y = y + l16;
    }
    if item[0] == 1 {
        let v = cw_sim::interact::heal_amount(item);
        if !(v < 0.1f32) {
            // 0x004a5245: `__ftol2` (truncation); both passes green.
            let n = v as i64 as i32;
            let t = format!("HP +{n}");
            b.pair(&t, y, s * 10.0f32, 2.0, 0.0, 0, -1.0, GREEN, BLACK, GREEN);
        }
    }
    b.out
}

/// [`DescText`]s as widget text calls, each moved by `offset` (the caller's coordinates to
/// the widget node's space).
pub fn widget_texts(d: &[DescText], offset: Vec2) -> Vec<cw_ui::render::WidgetText> {
    d.iter()
        .filter(|t| !t.text.is_empty())
        .map(|t| cw_ui::render::WidgetText {
            font: super::present_hud::FONT.into(),
            text: t.text.encode_utf16().collect(),
            origin: t.pos + offset,
            style: cw_ui::font::TextStyle {
                size: t.size,
                stroke_radius: t.stroke,
                spacing: 0.0,
                line_spacing: t.line_spacing,
                // -1 (0xbf800000) is "no wrap width"; only the 0x10 flag wraps.
                wrap_width: if t.wrap < 0.0 { 0.0 } else { t.wrap },
                flags: t.flags,
                pixel_snap: true,
            },
            color: t.color,
            stroke_color: t.stroke_color,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gear(t: u8, level: i16, rarity: u8) -> Vec<u8> {
        let mut it = super::super::crafting::default_item();
        it[0] = t;
        it[0xc] = rarity;
        it[0x10..0x12].copy_from_slice(&level.to_le_bytes());
        it
    }

    fn game(level: i32, class: u8) -> GameView {
        let mut g = GameView::default();
        g.player.0[0x180..0x184].copy_from_slice(&level.to_le_bytes());
        g.player.0[0x130] = class;
        g
    }

    fn texts(v: &[DescText]) -> Vec<&str> {
        v.iter().step_by(2).map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn round1_matches_0x00439110() {
        // `(float)124 * 0.1f` is 12.400001; `%g` prints 12.4.
        assert_eq!(round1(12.35), 124.0f32 * 0.1f32);
        assert_eq!(fmt_g6(f64::from(round1(12.35))), "12.4");
        assert_eq!(fmt_g6(f64::from(round1(12.345))), "12.3");
        assert_eq!(fmt_g6(f64::from(round1(-12.35))), "-12.4");
        assert_eq!(round1(-0.01).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn weapon_block() {
        let mut sword = gear(3, 5, 1);
        sword[1] = 0; // one-handed sword: warriors only.
        let g = game(5, 1);
        let a = DescriptionArgs { x: 14, y: 25, scale: 1.0, width: 280, name: true, class_line: true };
        let d = describe(None, &g, &sword, &a);
        let t = texts(&d);
        assert!(t[0].ends_with(&format!(" +{}", item_level(&sword))), "{t:?}");
        assert_eq!(t[1], format!("Power {}", item_level(&sword)));
        let dmg = round1(cw_sim::modes::item_weapon_power(&sword));
        assert_eq!(t[2], format!("DMG {}", fmt_g6(f64::from(dmg))));
        // Name: outline pass white on black, stroke 3, then the rarity colour (green).
        assert_eq!((d[0].stroke, d[0].color, d[0].stroke_color), (3.0, WHITE, BLACK));
        assert_eq!(d[1].color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!((d[0].pos, d[0].size, d[0].flags, d[0].wrap), (Vec2::new(14.0, 25.0), 10.0, 0x10, 280.0));
        // Level at y + 30, stats from y + 30 + 14 + 16.
        assert_eq!(d[2].pos.y, 55.0);
        assert_eq!(d[4].pos.y, 85.0);
        // Another class: the allowed classes in red, the rest unchanged.
        let g = game(5, 3);
        let d = describe(None, &g, &sword, &a);
        assert_eq!(texts(&d)[2], "Warrior ");
        assert_eq!(d[5].color, RED);
    }

    #[test]
    fn level_colour_and_counts() {
        let g = game(1, 1);
        let a = DescriptionArgs { x: 0, y: 0, scale: 1.0, width: 300, name: false, class_line: false };
        // A level-10 chest for a level-1 player: red "Power".
        let d = describe(None, &g, &gear(4, 10, 0), &a);
        assert_eq!(d[0].color, RED);
        assert!(d.iter().any(|t| t.text.starts_with("ARMOR ")));
        // Coins: the level field as a count.
        let d = describe(None, &g, &gear(0xc, 7, 0), &a);
        assert_eq!(d[0].text, "7 X");
        // A consumable: "HP +" in green.
        let mut potion = gear(1, 1, 0);
        potion[1] = 1;
        let d = describe(None, &g, &potion, &a);
        let last = d.last().unwrap();
        assert_eq!(last.text, format!("HP +{}", cw_sim::interact::heal_amount(&potion) as i32));
        assert_eq!(last.color, GREEN);
    }
}
