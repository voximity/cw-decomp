//! Skill tooltips: `GameController::skillVariables` `Cube.exe 0x00478800` (the
//! `@level`/`@health`/... expander), the skill tooltip `0x004a5710` and the specialization
//! tooltip `0x004a62c0` (text and word layout), the class-trainer test `0x0047f030`, and the
//! objective text joiner of the quest tag (`0x00477fa0`).
//!
//! # What the original does
//!
//! `0x00478800(skill, level, map<wstring, wstring>* vars)` (thiscall on the GameController;
//! `skill` at `[ebp+8]` is never read) fills `vars` with 39 entries. Each value is written to
//! one `std::wstringstream`, taken with `str()`, stored under its key (`operator[]` then
//! assign, so a second call overwrites), and the stream is emptied again (`str(L"")`).
//! Floats go through `wostream << float` (promoted to double, `%g`, 6 significant digits:
//! [`crate::net::fmt_g6`]); most are first rounded to two decimals by `round2` 0x004874a0
//! (SSE: `(float)(int)(x * 100f + 0.5f) * 0.01f`, negatives mirrored, `-0` folded to `+0`).
//! The skill formulas are the shared ones (`skillFactor` 0x0043c980, `skillCooldown`
//! 0x0043e6a0, `skillLevelFactor` 0x0043ed60, `skillDuration` 0x00447310, `skillMpCost`
//! 0x00444ae0, the movement curves 0x0043e660 / 0x0043f770 / 0x004476a0 / 0x0043e2c0 and the
//! unnamed `0x00446aa0`); those with a creature argument run on the local player
//! (`GC+0x8006d0`).
//!
//! The only caller, the skill tooltip `0x004a5710`, renders `description:<skill>`,
//! `skill:level`, `details:<skill>` and `skill:nextlevel` from the text database
//! ([`super::textdb`], `Speech::render` 0x004e4a20 over the markup renderer 0x004e4350) into
//! coloured word lines ([`skill_tooltip_lines`]) and lays the words out itself
//! ([`layout_tooltip`]).

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;

use super::textdb::{Lines, QuestText, TextDb};
use crate::net::fmt_g6;

/// `round2` 0x004874a0: `x < 0` (the `comiss 0, x; jbe` test, so NaN takes the positive
/// path) mirrors the positive branch and folds `-0` into `+0`; otherwise
/// `(float)(int)(x * 100f + 0.5f) * 0.01f` (cvttss2si truncates toward zero).
pub fn round2(x: f32) -> f32 {
    if 0.0f32 > x {
        let r = -round2(-x);
        if r == 0.0 {
            return 0.0;
        }
        return r;
    }
    let t = (x * 100.0f32 + 0.5f32) as i32;
    (t as f32) * 0.01f32
}

/// `skillFactor` 0x0043c980: `level < 1 ? 0 : 1 - 1 / (level * 0.1 + 1)` (x87 result spilled
/// to a float by every caller here).
pub fn skill_factor(level: i32) -> f32 {
    if level < 1 {
        return 0.0;
    }
    (1.0f64 - 1.0f64 / ((level as f32) as f64 * 0.1f64 + 1.0f64)) as f32
}

/// `0x00446aa0` (unnamed; feeds `@ridingspeed`): `level < 1 ? 0 : skillFactor(level) + 1`.
pub fn skill_factor_plus_one(level: i32) -> f32 {
    if level < 1 {
        return 0.0;
    }
    ((1.0f64 - 1.0f64 / ((level as f32) as f64 * 0.1f64 + 1.0f64)) + 1.0f64) as f32
}

/// `skillMpCost` 0x00444ae0 for mode 0x22 (the healing stream, the only mode the tooltip
/// asks for): `(1 - skillLevelFactor(0x22, level) * 0.75) * 0.125`.
fn healing_stream_mp_cost(e: &EntityData, level: i32) -> f32 {
    let lf = cw_sim::skills::skill_level_factor(e, 0x22, level) as f64;
    ((1.0f64 - lf * 0.75f64) * 0.125f64) as f32
}

/// `skillCooldown(mode, level) / 1000` rounded (`(float)ms / 1000f`, then [`round2`]).
fn cooldown_s(e: &EntityData, mode: i32, level: i32) -> String {
    let ms = crate::player::skill_cooldown(e, mode, level);
    fmt_g6(round2(ms as f32 / 1000.0f32) as f64)
}

/// The variables of `0x00478800` for `level`, in insertion order (`std::map` sorts them; the
/// returned map is a `BTreeMap` for the same reason). `player` is the local player's entity
/// (`GC+0x8006d0 + 0x10`).
pub fn skill_variables(player: &EntityData, level: i32) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut put = |k: &str, v: String| {
        m.insert(k.to_string(), v);
    };
    let pct = |x: f32| format!("{}%", fmt_g6(x as f64));
    let lf = |mode: i32, lv: i32| cw_sim::skills::skill_level_factor(player, mode, lv);
    // 0x004788xx: `ss << level`.
    put("@level", level.to_string());
    // 0x00478a2b: skillFactor(level) * 50, round2 inlined (same shape as 0x004874a0).
    put("@health", pct(round2(skill_factor(level) * 50.0f32)));
    // 0x00478c02: 0x00446aa0(level) * 100.
    put("@ridingspeed", pct(round2(skill_factor_plus_one(level) * 100.0f32)));
    // 0x00478dd9: climbFactor 0x0043e660.
    put("@climbingpower", pct(round2(cw_sim::skills::climb_factor(level) * 100.0f32)));
    // 0x00478fb5: glideFactor 0x0043f770.
    put("@flyingspeed", pct(round2(cw_sim::skills::glide_factor(level) * 100.0f32)));
    // 0x004790ff: swimFactor 0x004476a0.
    put("@swimmingspeed", pct(round2(cw_sim::skills::swim_factor(level) * 100.0f32)));
    // 0x00479249: rideFactor 0x0043e2c0.
    put("@boatspeed", pct(round2(cw_sim::skills::ride_factor(level) * 100.0f32)));
    // 0x0047939c..: cooldowns in seconds.
    put("@smashcooldown", cooldown_s(player, 0x36, level));
    put("@cyclonecooldown", cooldown_s(player, 0x56, level));
    // 0x00479599: skillDuration(0x56) on the player (a constant 5000 ms).
    let dur = cw_sim::skills::skill_duration(player, 0.0, false, 0x56);
    put("@cycloneduration", fmt_g6(round2(dur as f32 / 1000.0f32) as f64));
    put("@bulwarkcooldown", cooldown_s(player, 0x65, level));
    // 0x0047979c: (skillLevelFactor(0x65) * 0.3 + 0.25) * 100.
    put("@bulwarkamount", pct(round2((lf(0x65, level) * 0.3f32 + 0.25f32) * 100.0f32)));
    // 0x0047aa..: the literal L"10" (0x00702230).
    put("@bulwarkduration", "10".to_string());
    put("@warfrenzycooldown", cooldown_s(player, 0x66, level));
    put("@warfrenzyduration", "10".to_string());
    // 0x00479ac7: (skillLevelFactor(0x66) * 9 + 1) * 100.
    put("@warfrenzyamount", pct(round2((lf(0x66, level) * 9.0f32 + 1.0f32) * 100.0f32)));
    put("@rangerkickcooldown", cooldown_s(player, 0x15, level));
    // 0x00479c76: skillFactor(level) * 100 with the wide L"%" (0x006fd728).
    put("@rangerkickknockback", pct(round2(skill_factor(level) * 100.0f32)));
    put("@retreatcooldown", cooldown_s(player, 0x32, level));
    put("@retreatdistance", pct(round2(skill_factor(level) * 100.0f32)));
    put("@aimcooldown", cooldown_s(player, 99, level));
    put("@aimstealth", pct(round2(skill_factor(level) * 100.0f32)));
    put("@swiftnesscooldown", cooldown_s(player, 100, level));
    put("@swiftnessamount", pct(round2(skill_factor(level) * 100.0f32)));
    put("@swiftnessduration", "10".to_string());
    put("@fireexplosioncooldown", cooldown_s(player, 0x58, level));
    put("@fireexplosionknockback", pct(round2(skill_factor(level) * 100.0f32)));
    // L"30" (0x00702474).
    put("@manashieldduration", "30".to_string());
    put("@manashieldcooldown", cooldown_s(player, 0x67, level));
    // 0x0047a5c2: skillLevelFactor(0x67) * 100.
    put("@manashieldpower", pct(round2(lf(0x67, level) * 100.0f32)));
    put("@teleportcooldown", cooldown_s(player, 0x31, level));
    // 0x0047a76a: skillMpCost(0x22) * 100 * 8, no suffix.
    put("@healingstreamcost", fmt_g6(round2(healing_stream_mp_cost(player, level) * 100.0f32 * 8.0f32) as f64));
    put("@interceptcooldown", cooldown_s(player, 0x30, level));
    put("@shurikencooldown", cooldown_s(player, 0x60, level));
    put("@camouflagecooldown", cooldown_s(player, 0x61, level));
    // 0x0047aaaa: skillLevelFactor(0x61, -1): the PLAYER's own level, not `level` (a quirk
    // of the original: the duration shown does not change with the previewed level).
    let cam = (lf(0x61, -1) * 12000.0f32 + 8000.0f32) / 1000.0f32;
    put("@camouflageduration", fmt_g6(round2(cam) as f64));
    put("@sneakcooldown", cooldown_s(player, 0x4f, level));
    // 0x0047ac5e: skillLevelFactor(0x4f) * 0.4 * 100 and * 100.
    put("@sneakspeed", pct(round2(lf(0x4f, level) * 0.4f32 * 100.0f32)));
    put("@sneakstealth", pct(round2(lf(0x4f, level) * 100.0f32)));
    m
}

/// `substitute` as the original does it: the text is parsed as speech markup
/// ([`QuestText::parse`] 0x004da850) and rendered with no tags (0x004e4350) into coloured
/// lines. Only whole tokens starting with `@` are variables; a token whose full text is not
/// a key renders nothing (it is not kept verbatim); `%` starts a new line; `[... $style]`
/// colours the words.
pub fn substitute(text: &str, vars: &BTreeMap<String, String>) -> Lines {
    let q = QuestText::parse(text);
    let mut lines: Lines = vec![Vec::new()];
    q.render_node(0, &BTreeSet::new(), vars, &mut lines);
    lines
}

/// The skill names of the client World's map at `World+0x800114` (filled by the World ctor
/// 0x0058eb00 at 0x005926a0..; read by 0x005a5a60): skill index to dictionary name.
pub const SKILL_NAMES: [&str; 11] = [
    "SkillPetTaming",
    "SkillPetRiding",
    "SkillClimbing",
    "SkillHangGliding",
    "SkillSwimming",
    "SkillBoatDriving",
    "SkillAbility1",
    "SkillAbility2",
    "SkillAbility3",
    "SkillAbility4",
    "SkillAbility5",
];

/// The ability names of the map at `World+0x80011c` (0x005927ed..; read by 0x00594bf0):
/// attack mode to dictionary name.
pub const ABILITY_NAMES: [(i32, &str); 16] = [
    (0x36, "AbilitySmash"),
    (0x56, "AbilityCyclone"),
    (0x65, "AbilityBulwark"),
    (0x66, "AbilityWarFrenzy"),
    (0x15, "AbilityRangerKick"),
    (0x32, "AbilityRetreat"),
    (99, "AbilityAim"),
    (100, "AbilitySwiftness"),
    (0x58, "AbilityFireExplosion"),
    (0x67, "AbilityManaShield"),
    (0x31, "AbilityTeleport"),
    (0x22, "AbilityHealingStream"),
    (0x30, "AbilityIntercept"),
    (0x60, "AbilityShuriken"),
    (0x61, "AbilityCamouflage"),
    (0x4f, "AbilitySneak"),
];

fn ability_name(mode: i32) -> String {
    // 0x00594bf0: a missing mode gives L"" (0x006fccac).
    ABILITY_NAMES.iter().find(|(m, _)| *m == mode).map(|(_, n)| n.to_string()).unwrap_or_default()
}

/// The dictionary name of skill `skill` in the tooltip 0x004a5710 (0x004a5870..0x004a5a60):
/// skills 6, 7 and 8 are the class abilities, chosen by `class` (`creature+0x140`) and the
/// skill widget's specialization (`SkillWidget+0x18c`); the others come from
/// [`SKILL_NAMES`] (0x005a5a60, `""` when out of range). An unknown class leaves the name
/// empty.
pub fn skill_key_name(skill: i32, class: u8, spec: i32) -> String {
    let mode = match (skill, class) {
        (6, 1) => 0x36,
        (6, 2) => 0x15,
        (6, 3) => {
            if spec == 1 {
                0x22
            } else {
                0x58
            }
        }
        (6, 4) => 0x30,
        (7, 1) => 0x56,
        (7, 2) => 0x32,
        (7, 3) => 0x67,
        (7, 4) => 0x4f,
        (8, 1) => {
            if spec == 0 {
                0x66
            } else {
                0x65
            }
        }
        (8, 2) => {
            if spec == 1 {
                100
            } else {
                99
            }
        }
        (8, 3) => 0x31,
        (8, 4) => {
            if spec == 1 {
                0x60
            } else {
                0x61
            }
        }
        (6..=8, _) => return String::new(),
        _ => {
            return usize::try_from(skill).ok().and_then(|i| SKILL_NAMES.get(i)).map(|s| s.to_string()).unwrap_or_default();
        }
    };
    ability_name(mode)
}

/// The text of the skill tooltip `GameController 0x004a5710(skill, level, x, y, 1.0, 275)`
/// (called by `PreviewWidget::update` 0x004d50a0 with `Preview+0x168` / `+0x16c`), in
/// render order, with no tags:
///
/// 1. `description:<name>` with the variables of `level`;
/// 2. when `level > 0`: `skill:level` and `details:<name>` (same variables);
/// 3. the variables of `level + 1` (0x00478800 overwrites every key of the same map):
///    `skill:nextlevel` and `details:<name>`.
///
/// `class` is `creature+0x140`, `spec` `SkillWidget+0x18c`. The caller lays the lines out
/// with [`layout_tooltip`].
pub fn skill_tooltip_lines(db: &TextDb, player: &EntityData, skill: i32, level: i32, class: u8, spec: i32) -> Lines {
    let name = skill_key_name(skill, class, spec);
    let tags = BTreeSet::new();
    let mut lines = Lines::new();
    let vars = skill_variables(player, level);
    db.render(&format!("description:{name}"), &tags, &vars, &mut lines);
    if 0 < level {
        db.render("skill:level", &tags, &vars, &mut lines);
        db.render(&format!("details:{name}"), &tags, &vars, &mut lines);
    }
    let vars = skill_variables(player, level + 1);
    db.render("skill:nextlevel", &tags, &vars, &mut lines);
    db.render(&format!("details:{name}"), &tags, &vars, &mut lines);
    lines
}

/// The text of the specialization tooltip `0x004a62c0(class, spec, x, y, 1.0, 275)`
/// (`PreviewWidget::update` 0x004d50a0 with `creature+0x140` and `Preview+0x170`): the one
/// speech `specialization:<class>:<name>`; other classes or specs render nothing.
pub fn specialization_tooltip_lines(db: &TextDb, class: u8, spec: i32) -> Lines {
    let key = match (class, spec) {
        (1, 0) => "specialization:warrior:berserker",
        (1, 1) => "specialization:warrior:guardian",
        (2, 0) => "specialization:ranger:sniper",
        (2, 1) => "specialization:ranger:scout",
        (3, 0) => "specialization:mage:fire",
        (3, 1) => "specialization:mage:water",
        (4, 0) => "specialization:rogue:assassin",
        (4, 1) => "specialization:rogue:ninja",
        _ => return Lines::new(),
    };
    let mut lines = Lines::new();
    db.render(key, &BTreeSet::new(), &BTreeMap::new(), &mut lines);
    lines
}

/// One word the tooltip draws. The original draws it twice with `FontEngine::drawText`
/// 0x0065bc70 at `(x, y)` and font size `size`: first an outline pass (stroke 3, colours
/// `(1,1,1,1)` / `(0,0,0,1)`: the black outline), then the fill pass in `color` (stroke 0).
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipWord {
    /// The word.
    pub text: String,
    /// Pen position (the int layout cursor converted to float).
    pub x: f32,
    /// Pen y.
    pub y: f32,
    /// Font size: 12 on the first line, 10 after.
    pub size: f32,
    /// Fill colour.
    pub color: [f32; 4],
}

/// The outline stroke of the first draw pass (`0x40400000`).
pub const TOOLTIP_OUTLINE: f32 = 3.0;

/// The tooltip width both callers pass (`0x113`).
pub const TOOLTIP_WIDTH: i32 = 0x113;

/// The word layout shared by 0x004a5710 (0x004a5d5a..) and 0x004a62c0 (0x004a6560..), from
/// the top-left `(x0, y0)` with wrap width `width`. `measure(text, size)` is
/// `ScalableFont::getBounds` 0x0065e720 on the engine font (`max.x - min.x`). The font must
/// exist (both functions return before any work when `resource1.dat` is missing,
/// 0x00639800).
///
/// Per line (font size 12 for the first, 10 after; `y += 18` after each): the first word,
/// and the word after a `-`, sits at the cursor; other words advance it by 3 when they are
/// glue (`. - , ; ! ? : / )`) or by the font size (the space) otherwise. A non-glue word
/// whose right edge `x + (int)w` passes `x0 + width` wraps (`y += 4 + size`, `x = x0`).
/// After drawing, `x += (int)w`, and `/` or `(` pull the cursor back by 9.
pub fn layout_tooltip(lines: &Lines, x0: i32, y0: i32, width: i32, measure: &mut dyn FnMut(&str, f32) -> f32) -> Vec<TooltipWord> {
    let mut out = Vec::new();
    let mut size: i32 = 12;
    let mut y = y0;
    for line in lines {
        let mut first = true;
        let mut x = x0;
        for w in line {
            let t = w.text.as_str();
            let wf = measure(t, size as f32);
            let glue = matches!(t, "." | "-" | "," | ";" | "!" | "?" | ":" | "/" | ")");
            if !first {
                x += if glue { 3 } else { size };
            }
            first = t == "-";
            if !glue && x0 + width < wf as i32 + x {
                y = y + 4 + size;
                x = x0;
            }
            out.push(TooltipWord { text: w.text.clone(), x: x as f32, y: y as f32, size: size as f32, color: w.color });
            x += wf as i32;
            if t == "/" || t == "(" {
                x -= 9;
            }
        }
        y += 0x12;
        size = 10;
    }
    out
}

/// `GameController 0x0047f030`: is a class trainer of the player's class within 4 blocks?
/// Walks the World's creature map (`GC+0x2e8`, `std::map<id, Creature*>`, value at node
/// +0x18) and returns true for the first creature whose appearance flags (`entity+0x6e`,
/// `creature+0x7e`) have bit 0x40 and whose class (`entity+0x130`) equals the player's,
/// when `(dx/65536)^2 + (dy/65536)^2 + (dz/65536)^2 < 16` with `d = player - creature`
/// (wrapping int64, `fild` to float, `mulss` by `1/65536`, single-precision squares summed
/// x, y then z; the `comiss 16, sum; ja` test is false on NaN). `creatures` is every
/// creature of that map (the order does not change the result); `player` is
/// `GC+0x8006d0 + 0x10`. Used by the skill widget (0x004dd810) and its Learn test
/// (0x004df880, `SkillWidget::learn_hit`'s `trainer`).
pub fn class_trainer_nearby<'a>(player: &EntityData, creatures: impl IntoIterator<Item = &'a EntityData>) -> bool {
    let pos = |e: &EntityData, k: usize| i64::from_le_bytes(e.0[8 * k..8 * k + 8].try_into().unwrap());
    let scale = 1.525_878_9e-5f32; // 0x006fcd94
    for c in creatures {
        if c.0[0x6e] & 0x40 == 0 || c.0[0x130] != player.0[0x130] {
            continue;
        }
        let dx = (pos(player, 0).wrapping_sub(pos(c, 0)) as f32) * scale;
        let dy = (pos(player, 1).wrapping_sub(pos(c, 1)) as f32) * scale;
        let dz = (pos(player, 2).wrapping_sub(pos(c, 2)) as f32) * scale;
        let sum = dx * dx + dy * dy + dz * dz;
        if 16.0f32 > sum {
            return true;
        }
    }
    false
}

/// The word joiner of the objective text `0x00477fa0` (the quest tag): words are joined with
/// one space (L" ", 0x006fd844), except before `.` `-` `,` `;` (the `compare` tests) or `!`
/// `?` `:` (the `operator==` tests), and never after `-`.
pub fn join_words<S: AsRef<str>>(words: &[S]) -> String {
    let mut out = String::new();
    // `bVar8`: suppress the space before the next word (first word, or after "-").
    let mut no_space = true;
    for w in words {
        let w = w.as_ref();
        let glue = matches!(w, "." | "-" | "," | ";" | "!" | "?" | ":");
        if !no_space && !glue {
            out.push(' ');
        }
        no_space = w == "-";
        out.push_str(w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> EntityData {
        // A player (hostile type 0 at entity+0x50) with every skill at level 3.
        let mut e = EntityData::new_creature();
        e.0[0x50] = 0;
        for k in 0..11 {
            e.0[0x1128 + 4 * k..0x1128 + 4 * k + 4].copy_from_slice(&3i32.to_le_bytes());
        }
        e
    }

    #[test]
    fn round2_matches_0x004874a0() {
        assert_eq!(round2(16.666_666), 16.67);
        assert_eq!(round2(-16.666_666), -16.67);
        assert_eq!(round2(-0.001).to_bits(), 0.0f32.to_bits());
        assert_eq!(round2(2.0), 2.0);
    }

    #[test]
    fn expansion_samples() {
        let e = player();
        let v = skill_variables(&e, 5);
        assert_eq!(v["@level"], "5");
        // skillFactor(5) = 1 - 1/1.5 = 0.3333; * 50 = 16.667 -> 16.67.
        assert_eq!(v["@health"], "16.67%");
        assert_eq!(v["@ridingspeed"], "133.33%");
        assert_eq!(v["@bulwarkduration"], "10");
        assert_eq!(v["@manashieldduration"], "30");
        assert_eq!(v["@cycloneduration"], "5");
        // skillCooldown(0x36, 5) = 20000 - lf * 14000, lf = 1/3 -> 15333 ms -> 15.33 s.
        assert_eq!(v["@smashcooldown"], "15.33");
        // Camouflage duration uses the player's own level (3): lf = 1 - 1/1.3.
        let lf3 = 1.0f32 - 1.0f32 / (3.0f32 * 0.1f32 + 1.0f32);
        let cam = round2((lf3 * 12000.0 + 8000.0) / 1000.0);
        assert_eq!(v["@camouflageduration"], fmt_g6(cam as f64));
        assert_eq!(v.len(), 39);
        let t = substitute("Heals [@health $number], lasts @bulwarkduration s (@level).", &v);
        assert_eq!(texts(&t[0]), vec!["Heals", "16.67%", ",", "lasts", "10", "s", "(", "5", ")", "."]);
        assert_eq!(t[0][1].color, [0.3, 1.0, 0.3, 1.0]);
        assert_eq!(texts(&substitute("no vars @unknown", &v)[0]), vec!["no", "vars"]);
    }

    fn texts(l: &[super::super::textdb::Word]) -> Vec<&str> {
        l.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn skill_names() {
        assert_eq!(skill_key_name(0, 1, 0), "SkillPetTaming");
        assert_eq!(skill_key_name(9, 1, 0), "SkillAbility4");
        assert_eq!(skill_key_name(6, 3, 1), "AbilityHealingStream");
        assert_eq!(skill_key_name(6, 3, 0), "AbilityFireExplosion");
        assert_eq!(skill_key_name(8, 1, 0), "AbilityWarFrenzy");
        assert_eq!(skill_key_name(8, 1, 1), "AbilityBulwark");
        assert_eq!(skill_key_name(8, 4, 0), "AbilityCamouflage");
        assert_eq!(skill_key_name(7, 0, 0), "");
        assert_eq!(skill_key_name(11, 1, 0), "");
    }

    #[test]
    fn skill_tooltip_order() {
        let xml = "<root>\
            <speech key=\"description:SkillPetTaming\">[Pet Master $stress] % Your pets.</speech>\
            <speech key=\"skill:level\">% % [LVL @level $number] %</speech>\
            <speech key=\"skill:nextlevel\">% % [Next Level $stress] %</speech>\
            <speech key=\"details:SkillPetTaming\">Health [@health $number].</speech>\
            </root>";
        let db = TextDb::from_xml(xml.as_bytes()).unwrap();
        let e = player();
        let l = skill_tooltip_lines(&db, &e, 0, 5, 1, 0);
        let all: Vec<Vec<&str>> = l.iter().map(|x| texts(x)).collect();
        assert_eq!(all[0], vec!["Pet", "Master"]);
        assert_eq!(all[1], vec!["Your", "pets", "."]);
        assert_eq!(all[3], vec!["LVL", "5"]);
        assert_eq!(all[4], vec!["Health", "16.67%", "."]);
        assert_eq!(all[6], vec!["Next", "Level"]);
        // skillFactor(6) * 50 = 18.75.
        assert_eq!(all[7], vec!["Health", "18.75%", "."]);
        // Level 0: no "skill:level" block.
        let l0 = skill_tooltip_lines(&db, &e, 0, 0, 1, 0);
        assert_eq!(l0.len(), 5);
    }

    #[test]
    fn layout_rules() {
        let w = |t: &str| super::super::textdb::Word { text: t.to_string(), color: [1.0; 4] };
        let lines = vec![vec![w("Aa"), w("bb"), w("."), w("x-"), w("-"), w("y")], vec![w("long"), w("word")]];
        // Every word measures 10 * len / 2.
        let mut m = |t: &str, _s: f32| t.chars().count() as f32 * 5.0;
        let out = layout_tooltip(&lines, 100, 50, 60, &mut m);
        let pos: Vec<(f32, f32, f32)> = out.iter().map(|o| (o.x, o.y, o.size)).collect();
        // "Aa" at 100; "bb" after 10 + 12 = 122; "." glue at 132 + 3 = 135; "x-" at
        // 140 + 12 = 152 -> 162 > 160: wraps to (100, 66); "-" glue at 110 + 3 = 113; "y"
        // follows "-" without a space at 118.
        assert_eq!(pos[0], (100.0, 50.0, 12.0));
        assert_eq!(pos[1], (122.0, 50.0, 12.0));
        assert_eq!(pos[2], (135.0, 50.0, 12.0));
        assert_eq!(pos[3], (100.0, 66.0, 12.0));
        assert_eq!(pos[4], (113.0, 66.0, 12.0));
        assert_eq!(pos[5], (118.0, 66.0, 12.0));
        // Next line: y + 18, size 10.
        assert_eq!(pos[6], (100.0, 84.0, 10.0));
        assert_eq!(pos[7], (130.0, 84.0, 10.0));
    }

    #[test]
    fn trainer_test() {
        let mut p = EntityData::new_creature();
        p.0[0x130] = 3;
        let mut t = EntityData::new_creature();
        t.0[0x130] = 3;
        t.0[0x6e] = 0x40;
        // 3.9 blocks away on x.
        t.0[0..8].copy_from_slice(&((3.9 * 65536.0) as i64).to_le_bytes());
        assert!(class_trainer_nearby(&p, [&t]));
        t.0[0..8].copy_from_slice(&(4i64 * 65536).to_le_bytes());
        assert!(!class_trainer_nearby(&p, [&t]));
        t.0[0..8].copy_from_slice(&0i64.to_le_bytes());
        t.0[0x130] = 2;
        assert!(!class_trainer_nearby(&p, [&t]));
        t.0[0x130] = 3;
        t.0[0x6e] = 0;
        assert!(!class_trainer_nearby(&p, [&t]));
    }

    #[test]
    fn objective_joiner() {
        let w = ["Kill", "Lugotar", ",", "the", "orc", "-", "chief", "!"];
        assert_eq!(join_words(&w), "Kill Lugotar, the orc-chief!");
    }
}
